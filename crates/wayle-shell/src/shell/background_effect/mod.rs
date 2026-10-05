//! Compositor-side background blur for wayle's own surfaces, through the
//! standard `ext-background-effect-v1` Wayland protocol.
//!
//! A compositor rule (e.g. a niri `layer-rule` with `background-effect`)
//! blurs the *whole* surface, including the transparent inset around the bar
//! and the transparent margin and shadow area around a dropdown card. With
//! the protocol, the client says exactly which part to blur, so wayle sends
//! the visible bar / card shape (rounded corners included) and nothing else.
//!
//! Which surfaces ask for blur:
//! - bar layer surfaces (`bar.background-blur`), and
//! - dropdown popovers (`bar.dropdown-blur`).
//!
//! Never the full-screen dismissal scrim (`wayle-dropdown-scrim`) and not the
//! OSD, notification popups, or systray menus: those are either invisible,
//! opaque by design, or have no opacity setting, so blur would cost GPU time
//! without showing.
//!
//! Lifecycle: a [`BackgroundBlur`] follows its `GtkNative` across
//! realize/unrealize (popovers get a fresh `GdkSurface` whenever they are
//! re-parented). The effect object is created lazily the first time a
//! non-empty shape is wanted, so with blur disabled nothing is ever sent to
//! the compositor, and it is destroyed before GTK destroys the surface. The
//! shape is recomputed on every frame-clock `layout` (after GTK allocated the
//! widgets, so the region lands in the same commit) and `after-paint`
//! (fallback), and only re-sent when it changed.

mod protocol;
mod region;

use std::{cell::RefCell, rc::Rc};

use relm4::gtk::{self, gdk, glib, prelude::*};

use self::{
    protocol::{BackgroundEffects, SurfaceEffect},
    region::{Rect, current_rem_px, radius_from_css_var, rounded_rect, widget_rect_in_surface},
};

/// Computes the blur region of a surface, in surface-local coordinates.
/// `None` or an empty list means "no blur".
type ShapeFn = dyn Fn() -> Option<Vec<Rect>>;

/// Keeps a `GtkNative`'s blur region in sync with a shape function.
///
/// Dropping it removes the effect (on the surface's next commit).
pub(crate) struct BackgroundBlur {
    inner: Rc<Inner>,
}

struct Inner {
    native: glib::WeakRef<gtk::Widget>,
    shape: Box<ShapeFn>,
    effect: RefCell<Option<SurfaceEffect>>,
    clock: RefCell<Option<(gdk::FrameClock, [glib::SignalHandlerId; 2])>>,
    native_handlers: RefCell<Vec<glib::SignalHandlerId>>,
}

impl BackgroundBlur {
    /// Track `native` (a `gtk::Window` or `gtk::Popover`) with `shape`.
    pub(crate) fn attach(
        native: &(impl IsA<gtk::Native> + IsA<gtk::Widget>),
        shape: impl Fn() -> Option<Vec<Rect>> + 'static,
    ) -> Self {
        let widget: &gtk::Widget = native.upcast_ref();
        let inner = Rc::new(Inner {
            native: widget.downgrade(),
            shape: Box::new(shape),
            effect: RefCell::default(),
            clock: RefCell::default(),
            native_handlers: RefCell::default(),
        });

        let realize = widget.connect_realize({
            let inner = Rc::downgrade(&inner);
            move |_| {
                if let Some(inner) = inner.upgrade() {
                    inner.hook_frame_clock();
                }
            }
        });
        // Runs before the class handler, i.e. before GTK destroys the
        // GdkSurface (and with it the wl_surface): the effect object must be
        // gone by then or a later request on it would be a protocol error.
        let unrealize = widget.connect_unrealize({
            let inner = Rc::downgrade(&inner);
            move |_| {
                if let Some(inner) = inner.upgrade() {
                    inner.release();
                }
            }
        });
        inner
            .native_handlers
            .borrow_mut()
            .extend([realize, unrealize]);

        if widget.is_realized() {
            inner.hook_frame_clock();
        }

        Self { inner }
    }

    /// Re-evaluate the shape now (e.g. after the opt-in setting changed).
    pub(crate) fn refresh(&self) {
        self.inner.update();
    }
}

impl Drop for BackgroundBlur {
    fn drop(&mut self) {
        let had_effect = self.inner.effect.borrow().is_some();
        self.inner.release();
        if let Some(native) = self.inner.native.upgrade() {
            for handler in self.inner.native_handlers.take() {
                native.disconnect(handler);
            }
            if had_effect {
                native.queue_draw();
            }
        }
    }
}

impl Inner {
    fn hook_frame_clock(self: &Rc<Self>) {
        self.unhook_frame_clock();
        let Some(clock) = self
            .native
            .upgrade()
            .and_then(|native| native.frame_clock())
        else {
            return;
        };
        let layout = clock.connect_layout({
            let inner = Rc::downgrade(self);
            move |_| {
                if let Some(inner) = inner.upgrade() {
                    inner.update();
                }
            }
        });
        let after_paint = clock.connect_after_paint({
            let inner = Rc::downgrade(self);
            move |_| {
                if let Some(inner) = inner.upgrade() {
                    inner.update();
                }
            }
        });
        *self.clock.borrow_mut() = Some((clock, [layout, after_paint]));
    }

    fn unhook_frame_clock(&self) {
        if let Some((clock, handlers)) = self.clock.take() {
            for handler in handlers {
                clock.disconnect(handler);
            }
        }
    }

    /// Stop tracking the current surface and destroy its effect object.
    fn release(&self) {
        self.unhook_frame_clock();
        drop(self.effect.take());
    }

    /// Bring the compositor-side region in line with the current shape.
    fn update(&self) {
        let Some(native) = self.native.upgrade() else {
            return;
        };
        if !native.is_mapped() {
            return;
        }
        let Some(surface) = native
            .downcast_ref::<gtk::Native>()
            .and_then(NativeExt::surface)
        else {
            return;
        };

        let rects = (self.shape)().unwrap_or_default();
        let mut effect = self.effect.borrow_mut();

        if effect
            .as_ref()
            .is_some_and(|effect| !effect.is_for(&surface))
        {
            *effect = None;
        }

        if rects.is_empty() {
            if effect.take().is_some() {
                // Destroying the object clears the region on the next commit.
                drop(effect);
                native.queue_draw();
            }
            return;
        }

        if effect.is_none() {
            let Some(effects) = BackgroundEffects::get() else {
                return;
            };
            *effect = effects.attach(&surface);
        }
        let Some(current) = effect.as_mut() else {
            return;
        };
        if current.set_region(&rects) {
            tracing::trace!(region = ?rects, "blur region updated");
            drop(effect);
            // set_blur_region is double-buffered: make GTK commit a frame.
            native.queue_draw();
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.release();
    }
}

/// Blur shape for a rounded `target` widget inside `native`: the target's
/// border box with its corners rounded by `radius_css` (a rounding CSS value
/// such as `var(--bar-radius-lg)`) times `scale`.
pub(crate) fn rounded_widget_shape(
    native: &(impl IsA<gtk::Native> + IsA<gtk::Widget>),
    target: &impl IsA<gtk::Widget>,
    radius_css: &str,
    scale: f64,
) -> Option<Vec<Rect>> {
    let rect = widget_rect_in_surface(native, target)?;
    let radius = radius_from_css_var(radius_css, scale, current_rem_px());
    Some(rounded_rect(rect, radius))
}
