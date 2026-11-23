use gpui::{div, prelude::*, px, Div};

use crate::theme::active_theme;

/// Fallback horizontal page gutter when viewport width is unavailable or non-finite.
pub const PAGE_GUTTER_DEFAULT_PX: f32 = 32.0;

/// Minimum fluid horizontal page gutter (`clamp(24px, 3vw, 56px)`).
pub const PAGE_GUTTER_MIN_PX: f32 = 24.0;

/// Maximum fluid horizontal page gutter (`clamp(24px, 3vw, 56px)`).
pub const PAGE_GUTTER_MAX_PX: f32 = 56.0;

/// Compact horizontal gutter used by TopNav below `COMPACT_VIEWPORT_BREAKPOINT_PX`.
pub const PAGE_GUTTER_COMPACT_PX: f32 = 16.0;

/// Viewport width breakpoint below which TopNav enters compact chrome mode.
pub const COMPACT_VIEWPORT_BREAKPOINT_PX: f32 = 1040.0;

/// Maximum content width for `PageWidth::Focused` routes (Addons, Account, Settings, MetaDetails body).
pub const FOCUSED_PAGE_MAX_WIDTH_PX: f32 = 1200.0;

/// Standard top padding for non-hero pages below the floating TopNav.
pub const PAGE_PADDING_TOP_PX: f32 = 16.0;

/// Standard bottom padding for scrollable document/column pages.
pub const PAGE_PADDING_BOTTOM_PX: f32 = 72.0;

/// Compact bottom padding for `uniform_list`-backed full-height pages (Discover, Library).
pub const PAGE_LIST_PADDING_BOTTOM_PX: f32 = 24.0;

/// Standard vertical rhythm between major page bands.
pub const PAGE_SECTION_GAP_PX: f32 = 24.0;

/// Canonical page canvas width tier across Ferrite routes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PageWidth {
    /// Full-width fluid canvas (`w_full()`, no max-width cap) for browsing surfaces
    /// (Home, Discover, Library, Calendar, Search, Person).
    #[default]
    Wide,
    /// Focused management/reading canvas (`max_w(px(1200.0))` centered via `mx_auto()`)
    /// for Addons, Account, Settings, and MetaDetails body sections.
    Focused,
}

impl PageWidth {
    /// Returns the horizontal cap in logical pixels (`None` for `Wide`, `Some(1200.0)` for `Focused`).
    #[inline]
    pub const fn max_width_px(self) -> Option<f32> {
        match self {
            Self::Wide => None,
            Self::Focused => Some(FOCUSED_PAGE_MAX_WIDTH_PX),
        }
    }
}

/// Computes the canonical fluid horizontal gutter matching CSS `clamp(24px, 3vw, 56px)`.
///
/// Falls back to `32.0` when `viewport_w` is non-finite or `<= 0.0`.
#[inline]
pub fn page_gutter_px(viewport_w: f32) -> f32 {
    if !viewport_w.is_finite() || viewport_w <= 0.0 {
        return PAGE_GUTTER_DEFAULT_PX;
    }
    (viewport_w * 0.03).clamp(PAGE_GUTTER_MIN_PX, PAGE_GUTTER_MAX_PX)
}

/// Returns `true` when the viewport is narrower than `COMPACT_VIEWPORT_BREAKPOINT_PX` (`1040px`).
#[inline]
pub fn is_compact_viewport(viewport_w: f32) -> bool {
    viewport_w.is_finite() && viewport_w > 0.0 && viewport_w < COMPACT_VIEWPORT_BREAKPOINT_PX
}

/// Horizontal gutter for the floating TopNav bar: drops to `16.0` on compact viewports (`< 1040px`)
/// and matches `page_gutter_px(viewport_w)` everywhere else so nav edges align with page content.
#[inline]
pub fn topnav_gutter_px(viewport_w: f32) -> f32 {
    if is_compact_viewport(viewport_w) {
        PAGE_GUTTER_COMPACT_PX
    } else {
        page_gutter_px(viewport_w)
    }
}

/// Applies `PageWidth` horizontal constraints (`w_full()` plus centered `max_w(1200px)` for `Focused`).
#[inline]
pub fn apply_page_width(container: Div, width: PageWidth) -> Div {
    let base = container.w_full();
    match width {
        PageWidth::Wide => base,
        PageWidth::Focused => base.max_w(px(FOCUSED_PAGE_MAX_WIDTH_PX)).mx_auto(),
    }
}

/// Builds a standard non-hero page container with canonical width, fluid horizontal gutter,
/// and vertical rhythm (`pt(16)`, `pb(72)`, `gap(24)`).
pub fn page_container(width: PageWidth, viewport_w: f32) -> Div {
    let theme = active_theme();
    let gutter = page_gutter_px(viewport_w);
    apply_page_width(div(), width)
        .px(px(gutter))
        .pt(theme.sp(PAGE_PADDING_TOP_PX))
        .pb(theme.sp(PAGE_PADDING_BOTTOM_PX))
        .flex()
        .flex_col()
        .gap(theme.sp(PAGE_SECTION_GAP_PX))
}

/// Builds a full-height flex page container for `uniform_list`-backed routes (Discover, Library)
/// with canonical width, fluid horizontal gutter, and compact bottom inset (`pt(16)`, `pb(24)`, `gap(24)`).
pub fn full_height_page_container(width: PageWidth, viewport_w: f32) -> Div {
    let theme = active_theme();
    let gutter = page_gutter_px(viewport_w);
    apply_page_width(div(), width)
        .flex_1()
        .min_h(px(0.0))
        .px(px(gutter))
        .pt(theme.sp(PAGE_PADDING_TOP_PX))
        .pb(theme.sp(PAGE_LIST_PADDING_BOTTOM_PX))
        .flex()
        .flex_col()
        .gap(theme.sp(PAGE_SECTION_GAP_PX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_gutter_px_clamps_and_handles_invalid_widths() {
        assert_eq!(page_gutter_px(0.0), PAGE_GUTTER_DEFAULT_PX);
        assert_eq!(page_gutter_px(-400.0), PAGE_GUTTER_DEFAULT_PX);
        assert_eq!(page_gutter_px(f32::NAN), PAGE_GUTTER_DEFAULT_PX);
        assert_eq!(page_gutter_px(f32::INFINITY), PAGE_GUTTER_DEFAULT_PX);

        assert_eq!(page_gutter_px(600.0), 24.0);
        assert_eq!(page_gutter_px(800.0), 24.0);
        assert!((page_gutter_px(1000.0) - 30.0).abs() < f32::EPSILON);
        assert!((page_gutter_px(1200.0) - 36.0).abs() < f32::EPSILON);
        assert!((page_gutter_px(1600.0) - 48.0).abs() < f32::EPSILON);
        assert_eq!(page_gutter_px(1920.0), 56.0);
        assert_eq!(page_gutter_px(2560.0), 56.0);
        assert_eq!(page_gutter_px(3840.0), 56.0);
    }

    #[test]
    fn topnav_gutter_px_handles_compact_breakpoint_and_aligns_with_page_gutter() {
        assert!(is_compact_viewport(900.0));
        assert!(is_compact_viewport(1039.9));
        assert!(!is_compact_viewport(1040.0));
        assert!(!is_compact_viewport(0.0));
        assert!(!is_compact_viewport(f32::NAN));

        assert_eq!(topnav_gutter_px(760.0), PAGE_GUTTER_COMPACT_PX);
        assert_eq!(topnav_gutter_px(1039.0), PAGE_GUTTER_COMPACT_PX);
        assert!((topnav_gutter_px(1040.0) - page_gutter_px(1040.0)).abs() < f32::EPSILON);
        assert!((topnav_gutter_px(1440.0) - page_gutter_px(1440.0)).abs() < f32::EPSILON);
        assert_eq!(topnav_gutter_px(2560.0), 56.0);
        assert_eq!(topnav_gutter_px(0.0), PAGE_GUTTER_DEFAULT_PX);
        assert_eq!(topnav_gutter_px(f32::NAN), PAGE_GUTTER_DEFAULT_PX);
    }

    #[test]
    fn page_width_max_width_matches_design_contract() {
        assert_eq!(PageWidth::default(), PageWidth::Wide);
        assert_eq!(PageWidth::Wide.max_width_px(), None);
        assert_eq!(PageWidth::Focused.max_width_px(), Some(1200.0));
    }
}
