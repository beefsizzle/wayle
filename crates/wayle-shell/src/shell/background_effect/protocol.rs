//! The `ext-background-effect-v1` client: binding the global on GDK's Wayland
//! connection and creating per-surface effect objects.
//!
//! GDK owns the `wl_display` and its default event queue. We wrap the same
//! display (through `gdk4-wayland`, which keeps one `wayland-client`
//! connection per `GdkDisplay`) and put every object we create on a private
//! event queue, so GDK never sees our events and we never dispatch its. The
//! only events we receive are the registry globals and the manager's
//! `capabilities`; they are read with two blocking round trips (the registry,
//! then the manager) at first use.
//!
//! Requests go into the connection's shared output buffer in call order, so a
//! `set_blur_region` sent before GDK's next `wl_surface.commit` is applied by
//! that commit.

use std::{
    cell::{OnceCell, RefCell},
    collections::HashSet,
    fmt,
    rc::Rc,
};

use gdk4_wayland::{WaylandDisplay, WaylandSurface, prelude::WaylandSurfaceExtManual};
use relm4::gtk::{gdk, glib::object::ObjectExt, prelude::*};
use tracing::{debug, info};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
    globals::{BindError, GlobalListContents, registry_queue_init},
    protocol::{wl_compositor::WlCompositor, wl_region::WlRegion, wl_registry::WlRegistry},
};
use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1::{self, ExtBackgroundEffectManagerV1},
    ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
};

use super::region::Rect;

/// Why background blur is unavailable. Logged once; never an error for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Unavailable {
    /// The GDK display is not a Wayland display (e.g. X11 / broadway).
    NotWayland,
    /// GDK's `wl_display` could not be wrapped.
    NoConnection,
    /// Registry enumeration or the capabilities round trip failed.
    Roundtrip(String),
    /// The compositor does not advertise `ext_background_effect_manager_v1`.
    NoGlobal,
    /// The compositor lacks a `wl_compositor` we can bind (should never happen).
    NoCompositor,
    /// The manager is advertised but its capabilities do not include blur.
    NoBlurCapability,
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotWayland => f.write_str("not running on a Wayland display"),
            Self::NoConnection => f.write_str("cannot access GDK's Wayland connection"),
            Self::Roundtrip(error) => write!(f, "Wayland round trip failed: {error}"),
            Self::NoGlobal => {
                f.write_str("compositor does not advertise ext_background_effect_manager_v1")
            }
            Self::NoCompositor => f.write_str("compositor has no usable wl_compositor"),
            Self::NoBlurCapability => {
                f.write_str("compositor's ext-background-effect has no blur capability")
            }
        }
    }
}

/// `capability` bit for blur in `ext_background_effect_manager_v1.capabilities`.
const CAPABILITY_BLUR: u32 = 1;

/// Decide whether blur can be used from what the registry and the manager
/// reported: whether the manager global was bound, and the last
/// `capabilities` bitfield received (if any).
///
/// A manager that has not sent `capabilities` after the round trip is treated
/// as lacking blur: the protocol sends the event on bind, and "no blur" only
/// disables a cosmetic effect.
pub(crate) fn evaluate(
    manager: Result<(), BindError>,
    capabilities: Option<u32>,
) -> Result<(), Unavailable> {
    match manager {
        Err(BindError::NotPresent | BindError::UnsupportedVersion) => Err(Unavailable::NoGlobal),
        Ok(()) if capabilities.is_some_and(|bits| bits & CAPABILITY_BLUR != 0) => Ok(()),
        Ok(()) => Err(Unavailable::NoBlurCapability),
    }
}

/// Event sink for the private queue.
#[derive(Default)]
pub(crate) struct QueueState {
    capabilities: Option<u32>,
}

impl Dispatch<WlRegistry, GlobalListContents> for QueueState {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Global add/remove after start-up is ignored: a manager that goes away
        // makes the compositor drop the effect on its own.
    }
}

impl Dispatch<ExtBackgroundEffectManagerV1, ()> for QueueState {
    fn event(
        state: &mut Self,
        _: &ExtBackgroundEffectManagerV1,
        event: ext_background_effect_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_background_effect_manager_v1::Event::Capabilities { flags } = event {
            let bits = match flags {
                WEnum::Value(flags) => flags.bits(),
                WEnum::Unknown(bits) => bits,
            };
            debug!(capabilities = bits, "ext-background-effect capabilities");
            state.capabilities = Some(bits);
        }
    }
}

wayland_client::delegate_noop!(QueueState: ignore WlCompositor);
wayland_client::delegate_noop!(QueueState: ignore WlRegion);
wayland_client::delegate_noop!(QueueState: ignore ExtBackgroundEffectSurfaceV1);

/// A live binding of `ext_background_effect_manager_v1` on GDK's connection.
pub(crate) struct BackgroundEffects {
    connection: Connection,
    queue: RefCell<EventQueue<QueueState>>,
    state: RefCell<QueueState>,
    handle: QueueHandle<QueueState>,
    compositor: WlCompositor,
    manager: ExtBackgroundEffectManagerV1,
    /// `GdkSurface`s (by address) that currently own an effect object. The
    /// protocol makes a second `get_background_effect` for the same
    /// `wl_surface` a fatal error, which would take the whole shell down;
    /// this guard turns such a bug into a skipped blur instead.
    attached: RefCell<HashSet<usize>>,
}

thread_local! {
    static INSTANCE: OnceCell<Option<Rc<BackgroundEffects>>> = const { OnceCell::new() };
}

impl BackgroundEffects {
    /// The process-wide binding, created on first use (on the GTK thread).
    /// `None` when the compositor cannot blur; the reason is logged once.
    pub(crate) fn get() -> Option<Rc<Self>> {
        INSTANCE.with(|cell| {
            cell.get_or_init(|| match Self::bind() {
                Ok(effects) => {
                    info!("compositor supports ext-background-effect blur");
                    Some(Rc::new(effects))
                }
                Err(reason) => {
                    info!(%reason, "background blur unavailable; blur settings are ignored");
                    None
                }
            })
            .clone()
        })
    }

    fn bind() -> Result<Self, Unavailable> {
        let display = gdk::Display::default()
            .and_then(|display| display.downcast::<WaylandDisplay>().ok())
            .ok_or(Unavailable::NotWayland)?;
        let backend = display
            .wl_display()
            .and_then(|wl_display| wl_display.backend().upgrade())
            .ok_or(Unavailable::NoConnection)?;
        let connection = Connection::from_backend(backend);

        let (globals, mut queue) = registry_queue_init::<QueueState>(&connection)
            .map_err(|error| Unavailable::Roundtrip(error.to_string()))?;
        let handle = queue.handle();

        let manager = globals.bind::<ExtBackgroundEffectManagerV1, _, _>(&handle, 1..=1, ());
        let manager_bound = manager.as_ref().map(|_| ()).map_err(clone_bind_error);

        let mut state = QueueState::default();
        if manager.is_ok() {
            queue
                .roundtrip(&mut state)
                .map_err(|error| Unavailable::Roundtrip(error.to_string()))?;
        }
        let verdict = evaluate(manager_bound, state.capabilities);
        let Ok(manager) = manager else {
            return Err(verdict.err().unwrap_or(Unavailable::NoGlobal));
        };
        if let Err(reason) = verdict {
            manager.destroy();
            return Err(reason);
        }

        let Ok(compositor) = globals.bind::<WlCompositor, _, _>(&handle, 1..=4, ()) else {
            manager.destroy();
            return Err(Unavailable::NoCompositor);
        };

        Ok(Self {
            connection,
            queue: RefCell::new(queue),
            state: RefCell::new(state),
            handle,
            compositor,
            manager,
            attached: RefCell::default(),
        })
    }

    /// Create the effect object for `surface`'s `wl_surface`.
    ///
    /// Returns `None` (logged) if the surface has no `wl_surface`, is
    /// destroyed, or already has an effect object from us.
    pub(crate) fn attach(self: &Rc<Self>, surface: &gdk::Surface) -> Option<SurfaceEffect> {
        if surface.is_destroyed() {
            return None;
        }
        let key = surface_key(surface);
        if self.attached.borrow().contains(&key) {
            debug!("surface already has a background effect; not creating another");
            return None;
        }

        // The `WlSurface` proxy wraps GDK's pointer without lifetime tracking:
        // use it only here, while `surface` is alive, and never keep it.
        let wl_surface = surface.downcast_ref::<WaylandSurface>()?.wl_surface()?;
        let object = self
            .manager
            .get_background_effect(&wl_surface, &self.handle, ());
        self.attached.borrow_mut().insert(key);

        Some(SurfaceEffect {
            effects: self.clone(),
            object,
            surface: surface.downgrade(),
            key,
            region: Vec::new(),
        })
    }

    /// Process capability changes that arrived meanwhile (non-blocking).
    fn dispatch_pending(&self) {
        let mut state = self.state.borrow_mut();
        if let Err(error) = self.queue.borrow_mut().dispatch_pending(&mut state) {
            debug!(%error, "ext-background-effect queue dispatch failed");
        }
    }

    fn flush(&self) {
        if let Err(error) = self.connection.flush() {
            debug!(%error, "flushing Wayland connection failed");
        }
    }
}

impl Drop for BackgroundEffects {
    fn drop(&mut self) {
        self.manager.destroy();
    }
}

/// Address of a `GdkSurface`, used as an identity key only (never dereferenced).
fn surface_key(surface: &gdk::Surface) -> usize {
    surface.as_ptr() as usize
}

fn clone_bind_error(error: &BindError) -> BindError {
    match error {
        BindError::UnsupportedVersion => BindError::UnsupportedVersion,
        BindError::NotPresent => BindError::NotPresent,
    }
}

/// One `ext_background_effect_surface_v1`, destroyed on drop.
pub(crate) struct SurfaceEffect {
    effects: Rc<BackgroundEffects>,
    object: ExtBackgroundEffectSurfaceV1,
    surface: gdk::glib::WeakRef<gdk::Surface>,
    key: usize,
    /// The region last sent (empty before the first `set_blur_region`).
    region: Vec<Rect>,
}

impl SurfaceEffect {
    /// Whether this effect still belongs to `surface` and may be updated.
    pub(crate) fn is_for(&self, surface: &gdk::Surface) -> bool {
        self.surface
            .upgrade()
            .is_some_and(|own| own == *surface && !own.is_destroyed())
    }

    /// Send `rects` as the pending blur region if it differs from the last one
    /// sent. Returns whether a request was sent (the caller must then make GTK
    /// commit the surface). Never call this after the `GdkSurface` is
    /// destroyed: check [`is_for`](Self::is_for) first.
    pub(crate) fn set_region(&mut self, rects: &[Rect]) -> bool {
        if self.region == rects {
            return false;
        }
        self.effects.dispatch_pending();

        let region = self
            .effects
            .compositor
            .create_region(&self.effects.handle, ());
        for rect in rects {
            region.add(rect.x, rect.y, rect.width, rect.height);
        }
        self.object.set_blur_region(Some(&region));
        region.destroy();
        self.effects.flush();

        self.region = rects.to_vec();
        true
    }
}

impl Drop for SurfaceEffect {
    fn drop(&mut self) {
        // Allowed even if the wl_surface is already gone (the object is then
        // inert); the compositor removes the effect on the next commit.
        self.object.destroy();
        self.effects.flush();
        self.effects.attached.borrow_mut().remove(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_global_is_unavailable() {
        assert_eq!(
            evaluate(Err(BindError::NotPresent), None),
            Err(Unavailable::NoGlobal)
        );
        assert_eq!(
            evaluate(Err(BindError::UnsupportedVersion), Some(CAPABILITY_BLUR)),
            Err(Unavailable::NoGlobal)
        );
    }

    #[test]
    fn blur_requires_the_capability_bit() {
        assert_eq!(evaluate(Ok(()), Some(CAPABILITY_BLUR)), Ok(()));
        assert_eq!(evaluate(Ok(()), Some(CAPABILITY_BLUR | 0b110)), Ok(()));
        assert_eq!(
            evaluate(Ok(()), Some(0)),
            Err(Unavailable::NoBlurCapability)
        );
        assert_eq!(
            evaluate(Ok(()), Some(0b10)),
            Err(Unavailable::NoBlurCapability)
        );
    }

    #[test]
    fn silent_manager_counts_as_no_blur() {
        assert_eq!(evaluate(Ok(()), None), Err(Unavailable::NoBlurCapability));
    }

    #[test]
    fn capability_bit_matches_the_protocol() {
        assert_eq!(
            ext_background_effect_manager_v1::Capability::Blur.bits(),
            CAPABILITY_BLUR
        );
    }

    #[test]
    fn unavailable_reasons_are_human_readable() {
        assert!(
            Unavailable::NoGlobal
                .to_string()
                .contains("ext_background_effect")
        );
        assert!(Unavailable::NoBlurCapability.to_string().contains("blur"));
    }
}
