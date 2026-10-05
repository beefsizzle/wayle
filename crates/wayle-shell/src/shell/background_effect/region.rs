//! Pure geometry for blur regions: rounded rectangles as unions of row
//! rectangles, and the corner radius wayle's stylesheet gives a container.
//!
//! A `wl_region` is a set of integer rectangles, so a rounded shape is
//! approximated by one rectangle per pixel row inside each corner band
//! (rows with the same inset are merged). That keeps the blur inside the
//! visible rounded card instead of leaking out at the corners.

use std::cell::Cell;

use gtk::prelude::*;
use relm4::gtk;

/// An integer rectangle in surface-local (logical) coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: i32,
    pub(crate) height: i32,
}

impl Rect {
    pub(crate) fn is_empty(self) -> bool {
        self.width <= 0 || self.height <= 0
    }
}

/// Corner radius of a container, as wayle's stylesheet resolves it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CornerRadius {
    /// A fixed radius in logical pixels.
    Px(f64),
    /// `--radius-full` (9999px): GTK clamps it to half the shorter side.
    Full,
}

/// Base radius tokens in `rem`, mirroring `$base-radius-*` in
/// `wayle-styling/scss/tokens/_index.scss` (a unit test keeps them in sync).
const BASE_RADIUS_REM: [(&str, f64); 4] =
    [("sm", 0.375), ("md", 0.625), ("lg", 0.875), ("xl", 1.125)];

/// Resolve a rounding CSS value such as `var(--bar-radius-lg)` (as produced by
/// `RoundingLevel::to_css_values` / `to_bar_css_values`) to a radius, given the
/// scale variable it is multiplied by and the pixel size of `1rem`.
///
/// Unknown values resolve to no rounding, which only makes the blur region a
/// plain rectangle.
pub(crate) fn radius_from_css_var(css: &str, scale: f64, rem_px: f64) -> CornerRadius {
    let token = css
        .trim()
        .trim_start_matches("var(")
        .trim_end_matches(')')
        .trim_start_matches("--");
    let size = token.rsplit('-').next().unwrap_or_default();
    match size {
        "full" => CornerRadius::Full,
        _ => {
            let rem = BASE_RADIUS_REM
                .iter()
                .find_map(|(name, rem)| (*name == size).then_some(*rem))
                .unwrap_or(0.0);
            CornerRadius::Px(rem * rem_px * scale)
        }
    }
}

/// The union of row rectangles covering `rect` with all four corners rounded
/// by `radius` (clamped to half the shorter side, as CSS does).
///
/// Each corner row is sampled at its vertical centre and its horizontal inset
/// rounded to the nearest pixel, so the region matches GTK's antialiased
/// corner to within half a pixel.
pub(crate) fn rounded_rect(rect: Rect, radius: CornerRadius) -> Vec<Rect> {
    if rect.is_empty() {
        return Vec::new();
    }

    let half_short_side = f64::from(rect.width.min(rect.height)) / 2.0;
    let radius = match radius {
        CornerRadius::Full => half_short_side,
        CornerRadius::Px(px) => px.clamp(0.0, half_short_side),
    };
    let band = radius.ceil() as i32;
    if band == 0 {
        return vec![rect];
    }

    let insets: Vec<i32> = (0..band)
        .map(|row| {
            let dy = radius - (f64::from(row) + 0.5);
            if dy <= 0.0 {
                0
            } else {
                (radius - (radius * radius - dy * dy).sqrt()).round() as i32
            }
        })
        .collect();

    let mut rows: Vec<Rect> = Vec::with_capacity(2 * insets.len() + 1);
    let mut push = |y: i32, height: i32, inset: i32| {
        let row = Rect {
            x: rect.x + inset,
            y,
            width: rect.width - 2 * inset,
            height,
        };
        if row.is_empty() {
            return;
        }
        // Merge with the previous row when it has the same horizontal span.
        if let Some(last) = rows.last_mut()
            && last.x == row.x
            && last.width == row.width
            && last.y + last.height == row.y
        {
            last.height += row.height;
            return;
        }
        rows.push(row);
    };

    for (row, inset) in insets.iter().enumerate() {
        push(rect.y + row as i32, 1, *inset);
    }
    let middle = rect.height - 2 * band;
    if middle > 0 {
        push(rect.y + band, middle, 0);
    }
    for (row, inset) in insets.iter().enumerate().rev() {
        push(rect.y + rect.height - 1 - row as i32, 1, *inset);
    }

    rows
}

/// The border box of `widget` in the surface-local coordinates of `native`
/// (what `ext_background_effect_surface_v1.set_blur_region` expects), with
/// each edge rounded to the nearest logical pixel.
pub(crate) fn widget_rect_in_surface(
    native: &(impl IsA<gtk::Native> + IsA<gtk::Widget>),
    widget: &impl IsA<gtk::Widget>,
) -> Option<Rect> {
    let bounds = widget.compute_bounds(native)?;
    let (offset_x, offset_y) = native.surface_transform();

    let left = (f64::from(bounds.x()) + offset_x).round() as i32;
    let top = (f64::from(bounds.y()) + offset_y).round() as i32;
    let right = (f64::from(bounds.x() + bounds.width()) + offset_x).round() as i32;
    let bottom = (f64::from(bounds.y() + bounds.height()) + offset_y).round() as i32;

    let rect = Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    };
    (!rect.is_empty()).then_some(rect)
}

/// GTK's default font size when no `gtk-font-name` size is set (points).
const DEFAULT_FONT_SIZE_PT: f64 = 10.0;

/// The size of `1rem` in logical pixels, computed the way GTK's CSS engine
/// does (`gtk_css_font_size_get_default_px`): the `gtk-font-name` size,
/// converted from points with the `gtk-xft-dpi` setting unless it is an
/// absolute (pixel) size.
///
/// `font_size_pango` is in Pango units (0 when unset), `xft_dpi` in 1/1024
/// dots per inch (≤ 0 when unset, meaning 96).
pub(crate) fn rem_px(font_size_pango: i32, size_is_absolute: bool, xft_dpi: i32) -> f64 {
    let dpi = if xft_dpi > 0 {
        f64::from(xft_dpi) / 1024.0
    } else {
        96.0
    };
    let pango_scale = f64::from(gtk::pango::SCALE);
    if font_size_pango <= 0 {
        DEFAULT_FONT_SIZE_PT * dpi / 72.0
    } else if size_is_absolute {
        f64::from(font_size_pango) / pango_scale
    } else {
        f64::from(font_size_pango) / pango_scale * dpi / 72.0
    }
}

thread_local! {
    /// [`current_rem_px`] cache, cleared when the GTK font settings change.
    static REM_PX: Cell<Option<f64>> = const { Cell::new(None) };
    static REM_PX_WATCHED: Cell<bool> = const { Cell::new(false) };
}

/// [`rem_px`] for the current GTK settings (cached; recomputed after a
/// `gtk-font-name` / `gtk-xft-dpi` change).
pub(crate) fn current_rem_px() -> f64 {
    if let Some(cached) = REM_PX.with(Cell::get) {
        return cached;
    }
    let Some(settings) = gtk::Settings::default() else {
        return rem_px(0, false, 0);
    };
    if !REM_PX_WATCHED.with(|watched| watched.replace(true)) {
        settings.connect_gtk_font_name_notify(|_| REM_PX.with(|cell| cell.set(None)));
        settings.connect_gtk_xft_dpi_notify(|_| REM_PX.with(|cell| cell.set(None)));
    }
    let value = settings_rem_px(&settings);
    REM_PX.with(|cell| cell.set(Some(value)));
    value
}

fn settings_rem_px(settings: &gtk::Settings) -> f64 {
    let (size, absolute) = settings
        .gtk_font_name()
        .map(|name| {
            let description = gtk::pango::FontDescription::from_string(&name);
            (description.size(), description.is_size_absolute())
        })
        .unwrap_or((0, false));
    rem_px(size, absolute, settings.gtk_xft_dpi())
}

#[cfg(test)]
mod tests {
    use wayle_config::schemas::styling::RoundingLevel;

    use super::*;

    fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn area(rows: &[Rect]) -> i64 {
        rows.iter()
            .map(|r| i64::from(r.width) * i64::from(r.height))
            .sum()
    }

    /// Every row stays inside `outer`, rows do not overlap and are ordered
    /// top to bottom, and together they span the full height.
    fn assert_well_formed(rows: &[Rect], outer: Rect) {
        let mut next_y = outer.y;
        for row in rows {
            assert!(!row.is_empty(), "empty row {row:?}");
            assert_eq!(row.y, next_y, "rows must be contiguous: {rows:?}");
            assert!(row.x >= outer.x && row.x + row.width <= outer.x + outer.width);
            next_y = row.y + row.height;
        }
        assert_eq!(next_y, outer.y + outer.height, "rows must span the height");
    }

    #[test]
    fn zero_radius_is_the_rectangle_itself() {
        let r = rect(10, 4, 300, 40);
        assert_eq!(rounded_rect(r, CornerRadius::Px(0.0)), vec![r]);
    }

    #[test]
    fn empty_rect_yields_no_region() {
        assert!(rounded_rect(rect(0, 0, 0, 40), CornerRadius::Px(8.0)).is_empty());
        assert!(rounded_rect(rect(0, 0, 40, -1), CornerRadius::Full).is_empty());
    }

    #[test]
    fn rounded_rect_is_symmetric_and_close_to_true_area() {
        let r = rect(16, 8, 400, 120);
        let radius = 14.0;
        let rows = rounded_rect(r, CornerRadius::Px(radius));
        assert_well_formed(&rows, r);

        // Vertical symmetry: the k-th row from the top mirrors the k-th from the bottom.
        let n = rows.len();
        for k in 0..n / 2 {
            let (top, bottom) = (rows[k], rows[n - 1 - k]);
            assert_eq!(
                (top.x, top.width, top.height),
                (bottom.x, bottom.width, bottom.height)
            );
        }
        // Horizontal symmetry about the rectangle centre.
        for row in &rows {
            assert_eq!(row.x - r.x, (r.x + r.width) - (row.x + row.width));
        }

        // Within a pixel per corner row of the analytic rounded-rect area.
        let exact = f64::from(r.width) * f64::from(r.height)
            - (4.0 - std::f64::consts::PI) * radius * radius;
        let error = (area(&rows) as f64 - exact).abs();
        assert!(error <= 4.0 * radius, "area error {error} too large");

        // The corner pixel itself is never covered; the edge midpoints are.
        assert!(rows[0].x > r.x);
        assert!(rows.iter().any(|row| row.x == r.x && row.width == r.width));
    }

    #[test]
    fn insets_shrink_monotonically_towards_the_middle() {
        let r = rect(0, 0, 200, 200);
        let rows = rounded_rect(r, CornerRadius::Px(20.0));
        let top_half: Vec<i32> = rows
            .iter()
            .take_while(|row| row.width < r.width)
            .map(|row| row.x)
            .collect();
        assert!(!top_half.is_empty());
        assert!(top_half.windows(2).all(|pair| pair[0] > pair[1]));
    }

    #[test]
    fn radius_is_clamped_like_css() {
        let r = rect(0, 0, 300, 30);
        let huge = rounded_rect(r, CornerRadius::Px(9999.0));
        let full = rounded_rect(r, CornerRadius::Full);
        assert_eq!(huge, full);
        assert_well_formed(&full, r);
        // A pill is rounded over its whole height: only rows near the middle
        // reach the full width.
        assert!(full.len() > 10, "{full:?}");
        assert!(full[0].x > r.x + 5);
    }

    #[test]
    fn vertical_pill_is_well_formed() {
        let r = rect(4, 6, 36, 500);
        let rows = rounded_rect(r, CornerRadius::Full);
        assert_well_formed(&rows, r);
        assert!(rows.iter().any(|row| row.height >= 400), "{rows:?}");
    }

    #[test]
    fn css_vars_resolve_to_scaled_radius() {
        assert_eq!(
            radius_from_css_var("var(--bar-radius-lg)", 2.0, 16.0),
            CornerRadius::Px(0.875 * 16.0 * 2.0)
        );
        assert_eq!(
            radius_from_css_var("var(--radius-md)", 1.0, 13.0),
            CornerRadius::Px(0.625 * 13.0)
        );
        assert_eq!(
            radius_from_css_var("var(--radius-none)", 1.0, 16.0),
            CornerRadius::Px(0.0)
        );
        assert_eq!(
            radius_from_css_var("var(--radius-full)", 1.0, 16.0),
            CornerRadius::Full
        );
        assert_eq!(
            radius_from_css_var("garbage", 1.0, 16.0),
            CornerRadius::Px(0.0)
        );
    }

    #[test]
    fn every_rounding_level_maps_to_a_known_token() {
        for level in [
            RoundingLevel::None,
            RoundingLevel::Sm,
            RoundingLevel::Md,
            RoundingLevel::Lg,
            RoundingLevel::Full,
        ] {
            for css in [
                level.to_css_values().container,
                level.to_bar_css_values().container,
            ] {
                let resolved = radius_from_css_var(css, 1.0, 16.0);
                let expect_zero = matches!(level, RoundingLevel::None);
                match resolved {
                    CornerRadius::Full => assert!(!expect_zero),
                    CornerRadius::Px(px) => assert_eq!(px == 0.0, expect_zero, "{css}"),
                }
            }
        }
    }

    #[test]
    fn base_radius_tokens_match_the_stylesheet() {
        let path = wayle_styling::scss_dir().join("tokens/_index.scss");
        let scss = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(!scss.is_empty(), "cannot read {}", path.display());
        for (name, rem) in BASE_RADIUS_REM {
            let needle = format!("$base-radius-{name}: {rem}rem;");
            assert!(
                scss.contains(&needle),
                "{} lacks `{needle}`",
                path.display()
            );
        }
    }

    #[test]
    fn rem_follows_gtk_font_settings() {
        let pango = gtk::pango::SCALE;
        // Unset: GTK's 10pt default at 96 dpi.
        assert!((rem_px(0, false, -1) - 10.0 * 96.0 / 72.0).abs() < 1e-9);
        // "Sans 11" at 96 dpi.
        assert!((rem_px(11 * pango, false, 96 * 1024) - 11.0 * 96.0 / 72.0).abs() < 1e-9);
        // "Sans 12" at 120 dpi.
        assert!((rem_px(12 * pango, false, 120 * 1024) - 20.0).abs() < 1e-9);
        // Absolute pixel size ignores dpi.
        assert!((rem_px(16 * pango, true, 120 * 1024) - 16.0).abs() < 1e-9);
    }
}
