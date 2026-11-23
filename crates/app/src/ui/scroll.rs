use gpui::{
    div, prelude::*, px, rgb, rgba, App, Div, FontWeight, MouseButton, MouseDownEvent, Stateful,
    Window,
};

use crate::{
    icons::{svg_icon, IconName},
    theme::{active_theme, glass_capsule_shadow, sp, ui_sans, RADIUS_PILL},
    ui::layout::page_gutter_px,
};

/// Minimum vertical scroll distance (in logical pixels) before the floating
/// "Back to top" button appears.
pub const BACK_TO_TOP_THRESHOLD_PX: f32 = 320.0;

/// Minimum vertical overflow (in logical pixels) required before showing the
/// custom vertical scrollbar.
pub const SCROLLBAR_OVERFLOW_EPSILON_PX: f32 = 1.0;

/// Interactive hit-rail width (at 100% UI scale) for accessible scrollbar targeting.
pub const SCROLLBAR_RAIL_WIDTH_PX: f32 = 14.0;

/// Visual thumb pill width (at 100% UI scale) at rest.
pub const SCROLLBAR_THUMB_WIDTH_REST_PX: f32 = 5.0;

/// Visual thumb pill width (at 100% UI scale) while hovered or dragging.
pub const SCROLLBAR_THUMB_WIDTH_ACTIVE_PX: f32 = 8.0;

/// Minimum thumb pill height (at 100% UI scale) so long infinite-scroll lists
/// remain easy to grab and drag.
pub const SCROLLBAR_MIN_THUMB_HEIGHT_PX: f32 = 40.0;

/// Right window inset for the vertical scrollbar rail, clearing the 5px CSD
/// right resize strip on floating windows while staying inside the page gutter.
pub const SCROLLBAR_RIGHT_INSET_PX: f32 = 5.0;

/// Minimum top clearance (at 100% UI scale) so the scrollbar rail stays cleanly
/// below the floating TopNav across both scrolled (60px) and resting (68px) states.
pub const SCROLLBAR_TOPNAV_CLEARANCE_PX: f32 = 72.0;

/// Bottom window inset (at 100% UI scale) for the vertical scrollbar rail.
pub const SCROLLBAR_BOTTOM_INSET_PX: f32 = 10.0;

/// Tolerance (in logical pixels) used to detect post-prepaint scroll geometry
/// changes (`max_offset_y`, container bounds, or clamped `scroll_y`) that require
/// a 1-frame follow-up render to synchronize the scrollbar and Back-to-top button.
pub const SCROLL_SYNC_EPSILON_PX: f32 = 0.5;

/// Pure geometry metrics for a vertical scrollbar track and thumb.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollbarMetrics {
    /// Total height of the interactive scrollbar track in logical pixels.
    pub track_height: f32,
    /// Height of the draggable thumb pill in logical pixels.
    pub thumb_height: f32,
    /// Top offset of the thumb pill relative to the top of the track (`0.0..=max_thumb_top`).
    pub thumb_top: f32,
    /// Maximum vertical scroll distance (`content_height - viewport_height`).
    pub max_offset_y: f32,
    /// Current clamped vertical scroll distance (`0.0..=max_offset_y`).
    pub scroll_y: f32,
}

/// Returns `true` when the active scrollable view has scrolled past the
/// floating "Back to top" threshold.
#[inline]
pub fn should_show_back_to_top(scroll_y: f32) -> bool {
    scroll_y.is_finite() && scroll_y > BACK_TO_TOP_THRESHOLD_PX
}

/// Computes the right inset (in logical pixels) for the floating "Back to top"
/// button, aligned with `page_gutter_px(viewport_w)` while always staying clear
/// of the vertical scrollbar rail on the right edge.
#[inline]
pub fn back_to_top_right_inset_px(viewport_w: f32, ui_scale: f32) -> f32 {
    let scale = if ui_scale.is_finite() && ui_scale > 0.0 {
        ui_scale
    } else {
        1.0
    };
    let min_clear = SCROLLBAR_RIGHT_INSET_PX + SCROLLBAR_RAIL_WIDTH_PX * scale + 10.0 * scale;
    page_gutter_px(viewport_w).max(min_clear)
}

/// Computes `(track_top, track_height)` in window-space logical pixels for the
/// vertical scrollbar rail.
///
/// Aligns with the active scroll container's vertical bounds when available
/// (so Discover/Library grids and Settings' detail pane stay aligned with their
/// scrollable regions), while always clearing the floating TopNav at the top
/// and keeping a clean bottom inset.
pub fn compute_scrollbar_track_span(
    container_top: f32,
    container_bottom: f32,
    viewport_height: f32,
    is_hero_route: bool,
    ui_scale: f32,
) -> Option<(f32, f32)> {
    if !viewport_height.is_finite() || viewport_height <= 1.0 {
        return None;
    }
    let scale = if ui_scale.is_finite() && ui_scale > 0.0 {
        ui_scale
    } else {
        1.0
    };
    let topnav_clearance = SCROLLBAR_TOPNAV_CLEARANCE_PX * scale;
    let max_bottom = (viewport_height - SCROLLBAR_BOTTOM_INSET_PX * scale).max(0.0);
    let has_valid_bounds = container_top.is_finite()
        && container_bottom.is_finite()
        && container_bottom > container_top + 1.0;

    let track_top = if has_valid_bounds {
        container_top.max(topnav_clearance)
    } else if is_hero_route {
        topnav_clearance
    } else {
        76.0 * scale
    };

    let track_bottom = if has_valid_bounds {
        (container_bottom - 4.0 * scale).min(max_bottom)
    } else {
        max_bottom
    };

    let track_height = track_bottom - track_top;
    if !track_height.is_finite() || track_height <= 1.0 {
        return None;
    }
    Some((track_top, track_height))
}

/// Computes thumb height and position for a vertical scrollbar.
///
/// Returns `None` whenever the container does not overflow (`max_offset_y <= 1.0`)
/// or any dimension is invalid/non-positive.
pub fn compute_scrollbar_metrics(
    scroll_y: f32,
    max_offset_y: f32,
    viewport_height: f32,
    track_height: f32,
    min_thumb_height: f32,
) -> Option<ScrollbarMetrics> {
    if !scroll_y.is_finite()
        || !max_offset_y.is_finite()
        || !viewport_height.is_finite()
        || !track_height.is_finite()
        || !min_thumb_height.is_finite()
    {
        return None;
    }
    if max_offset_y <= SCROLLBAR_OVERFLOW_EPSILON_PX
        || viewport_height <= 1.0
        || track_height <= 1.0
    {
        return None;
    }

    let content_height = viewport_height + max_offset_y;
    let visible_ratio = (viewport_height / content_height).clamp(0.0, 1.0);
    let clamped_min_thumb = min_thumb_height.max(1.0).min(track_height);
    let thumb_height = (track_height * visible_ratio).clamp(clamped_min_thumb, track_height);
    let max_thumb_top = (track_height - thumb_height).max(0.0);
    let clamped_scroll = scroll_y.clamp(0.0, max_offset_y);
    let progress = (clamped_scroll / max_offset_y).clamp(0.0, 1.0);
    let thumb_top = progress * max_thumb_top;

    Some(ScrollbarMetrics {
        track_height,
        thumb_height,
        thumb_top,
        max_offset_y,
        scroll_y: clamped_scroll,
    })
}

/// Converts a window-space vertical pointer coordinate (`pointer_y`) into a
/// clamped scroll distance (`0.0..=metrics.max_offset_y`), preserving the
/// pointer's `grab_offset_y` inside the thumb.
pub fn scroll_offset_for_pointer_y(
    pointer_y: f32,
    track_top: f32,
    grab_offset_y: f32,
    metrics: ScrollbarMetrics,
) -> f32 {
    let max_scroll = metrics.max_offset_y.max(0.0);
    if !pointer_y.is_finite() || !track_top.is_finite() || !grab_offset_y.is_finite() {
        return metrics.scroll_y.clamp(0.0, max_scroll);
    }
    let max_thumb_top = (metrics.track_height - metrics.thumb_height).max(0.0);
    if max_thumb_top <= f32::EPSILON || max_scroll <= 0.0 {
        return 0.0;
    }
    let desired_thumb_top = (pointer_y - track_top - grab_offset_y).clamp(0.0, max_thumb_top);
    ((desired_thumb_top / max_thumb_top) * max_scroll).clamp(0.0, max_scroll)
}

/// Classifies a mouse-down at window-space `pointer_y` on the scrollbar rail.
///
/// Returns `(grab_offset_y, immediate_scroll_y)`:
/// - Pressing inside the thumb preserves the exact grab point within the thumb
///   and returns `None` so the content doesn't jump until the pointer moves.
/// - Pressing on the track outside the thumb centers the thumb at the cursor
///   (`thumb_height * 0.5`) and returns `Some(target_scroll_y)` to jump right
///   away while allowing seamless continuous dragging.
pub fn scrollbar_press_action(
    pointer_y: f32,
    track_top: f32,
    metrics: ScrollbarMetrics,
) -> (f32, Option<f32>) {
    let local_y = pointer_y - track_top;
    let hit_thumb =
        local_y >= metrics.thumb_top && local_y <= metrics.thumb_top + metrics.thumb_height;
    if hit_thumb {
        let grab_offset = (local_y - metrics.thumb_top).clamp(0.0, metrics.thumb_height);
        (grab_offset, None)
    } else {
        let grab_offset = metrics.thumb_height * 0.5;
        let target = scroll_offset_for_pointer_y(pointer_y, track_top, grab_offset, metrics);
        (grab_offset, Some(target))
    }
}

/// Returns `true` when post-prepaint scroll geometry `(max_offset_y, top, bottom, scroll_y)`
/// diverges from the snapshot read during `render()` by more than `SCROLL_SYNC_EPSILON_PX`.
#[inline]
pub fn scroll_geometry_changed(
    rendered: (f32, f32, f32, f32),
    post_prepaint: (f32, f32, f32, f32),
) -> bool {
    let diff = |a: f32, b: f32| {
        if !a.is_finite() || !b.is_finite() {
            return false;
        }
        (a - b).abs() > SCROLL_SYNC_EPSILON_PX
    };
    diff(rendered.0, post_prepaint.0)
        || diff(rendered.1, post_prepaint.1)
        || diff(rendered.2, post_prepaint.2)
        || diff(rendered.3, post_prepaint.3)
}

/// Renders the floating glass "Back to top" pill button anchored bottom-right.
pub fn render_back_to_top_button(
    viewport_w: f32,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let theme = active_theme();
    let accent_ink = if theme.is_light {
        theme.accent_press
    } else {
        theme.accent
    };
    let right_inset = back_to_top_right_inset_px(viewport_w, theme.ui_scale);

    div()
        .id("ferrite-back-to-top-btn")
        .occlude()
        .cursor_pointer()
        .absolute()
        .bottom(sp(24.0))
        .right(px(right_inset))
        .rounded(px(RADIUS_PILL))
        .bg(rgba(theme.glass_popover_bg))
        .border_1()
        .border_color(rgba(theme.glass_border_strong))
        .shadow(glass_capsule_shadow())
        .overflow_hidden()
        .hover(move |s| {
            s.border_color(rgba(theme.border_accent))
                .bg(rgba(theme.glass_sheet_bg))
        })
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(
            div()
                .h(sp(38.0))
                .px(sp(14.0))
                .bg(rgba(theme.glass_card_bg))
                .hover(move |s| s.bg(rgba(theme.accent_soft)))
                .flex()
                .items_center()
                .gap(sp(7.0))
                .child(svg_icon(
                    IconName::ArrowUp,
                    14.0 * theme.ui_scale,
                    rgb(accent_ink),
                ))
                .child(
                    div()
                        .font(ui_sans(FontWeight::SEMIBOLD))
                        .text_size(sp(12.5))
                        .text_color(rgb(theme.text_primary))
                        .whitespace_nowrap()
                        .child("Back to top"),
                ),
        )
}

/// Renders an accessible, interactive vertical scrollbar overlay rail and thumb.
///
/// The rail provides a generous `14px` (scaled) hit target along the right edge
/// of the viewport. At rest the thumb is a sleek translucent `5px` pill; on hover
/// or while dragging it widens to `8px` with increased contrast and reveals a
/// subtle track trough.
pub fn render_vertical_scrollbar(
    track_top: f32,
    metrics: ScrollbarMetrics,
    is_hovered: bool,
    is_dragging: bool,
    on_mouse_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_hover: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let theme = active_theme();
    let is_active = is_hovered || is_dragging;
    let rail_w = SCROLLBAR_RAIL_WIDTH_PX * theme.ui_scale;
    let thumb_w = if is_active {
        SCROLLBAR_THUMB_WIDTH_ACTIVE_PX * theme.ui_scale
    } else {
        SCROLLBAR_THUMB_WIDTH_REST_PX * theme.ui_scale
    };
    let thumb_left = ((rail_w - thumb_w) * 0.5).max(0.0);

    let track_bg = if is_active {
        rgba(theme.glass_trough_bg)
    } else {
        rgba(0x00000000)
    };
    let track_border = if is_active {
        rgba(theme.glass_divider)
    } else {
        rgba(0x00000000)
    };

    let thumb_bg = if is_dragging {
        let accent_rgba = (theme.accent << 8) | 0xE6;
        rgba(accent_rgba)
    } else if is_hovered {
        if theme.is_light {
            rgba(0x0F172A88)
        } else {
            rgba(0xFFFFFF99)
        }
    } else if theme.is_light {
        rgba(0x0F172A47)
    } else {
        rgba(0xFFFFFF4D)
    };

    div()
        .id("ferrite-vertical-scrollbar")
        .occlude()
        .cursor_pointer()
        .absolute()
        .top(px(track_top))
        .right(px(SCROLLBAR_RIGHT_INSET_PX))
        .w(px(rail_w))
        .h(px(metrics.track_height))
        .rounded(px(RADIUS_PILL))
        .bg(track_bg)
        .border_1()
        .border_color(track_border)
        .on_hover(on_hover)
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_mouse_down(ev, window, cx);
        })
        .child(
            div()
                .absolute()
                .top(px(metrics.thumb_top))
                .left(px(thumb_left))
                .w(px(thumb_w))
                .h(px(metrics.thumb_height))
                .rounded(px(RADIUS_PILL))
                .bg(thumb_bg),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_to_top_visibility_threshold_and_non_finite_values() {
        assert!(!should_show_back_to_top(0.0));
        assert!(!should_show_back_to_top(-120.0));
        assert!(!should_show_back_to_top(319.9));
        assert!(!should_show_back_to_top(BACK_TO_TOP_THRESHOLD_PX));
        assert!(!should_show_back_to_top(f32::NAN));
        assert!(!should_show_back_to_top(f32::INFINITY));
        assert!(!should_show_back_to_top(f32::NEG_INFINITY));

        assert!(should_show_back_to_top(BACK_TO_TOP_THRESHOLD_PX + 0.1));
        assert!(should_show_back_to_top(640.0));
        assert!(should_show_back_to_top(25_000.0));
    }

    #[test]
    fn back_to_top_right_inset_clears_scrollbar_rail_across_viewports() {
        let narrow_100 = back_to_top_right_inset_px(800.0, 1.0);
        assert!(narrow_100 >= SCROLLBAR_RIGHT_INSET_PX + SCROLLBAR_RAIL_WIDTH_PX + 10.0);

        let narrow_125 = back_to_top_right_inset_px(800.0, 1.25);
        assert!(
            narrow_125 >= SCROLLBAR_RIGHT_INSET_PX + SCROLLBAR_RAIL_WIDTH_PX * 1.25 + 12.5
        );

        let wide_100 = back_to_top_right_inset_px(1920.0, 1.0);
        assert_eq!(wide_100, page_gutter_px(1920.0));
    }

    #[test]
    fn scrollbar_metrics_hidden_when_non_overflowing_or_invalid() {
        assert_eq!(
            compute_scrollbar_metrics(0.0, 0.0, 800.0, 700.0, 40.0),
            None
        );
        assert_eq!(
            compute_scrollbar_metrics(0.0, 0.5, 800.0, 700.0, 40.0),
            None
        );
        assert_eq!(
            compute_scrollbar_metrics(0.0, SCROLLBAR_OVERFLOW_EPSILON_PX, 800.0, 700.0, 40.0),
            None
        );
        assert_eq!(
            compute_scrollbar_metrics(0.0, -50.0, 800.0, 700.0, 40.0),
            None
        );
        assert_eq!(
            compute_scrollbar_metrics(0.0, 500.0, 0.0, 700.0, 40.0),
            None
        );
        assert_eq!(
            compute_scrollbar_metrics(0.0, 500.0, 800.0, 0.0, 40.0),
            None
        );
        assert_eq!(
            compute_scrollbar_metrics(f32::NAN, 500.0, 800.0, 700.0, 40.0),
            None
        );
        assert_eq!(
            compute_scrollbar_metrics(0.0, f32::INFINITY, 800.0, 700.0, 40.0),
            None
        );
    }

    #[test]
    fn scrollbar_metrics_clamps_minimum_thumb_height_for_long_lists() {
        // Very long catalog (100k px overflow): proportional thumb would be < 6px,
        // so it must clamp to min_thumb_height (40px).
        let m = compute_scrollbar_metrics(0.0, 100_000.0, 800.0, 700.0, 40.0)
            .expect("overflowing list should produce metrics");
        assert!((m.thumb_height - 40.0).abs() < 1e-4);
        assert_eq!(m.thumb_top, 0.0);

        // When track itself is shorter than min_thumb_height, clamp to track_height.
        let tiny = compute_scrollbar_metrics(50.0, 500.0, 400.0, 28.0, 40.0)
            .expect("tiny track still returns bounded metrics");
        assert!((tiny.thumb_height - 28.0).abs() < 1e-4);
        assert_eq!(tiny.thumb_top, 0.0);
    }

    #[test]
    fn scrollbar_metrics_top_mid_bottom_and_out_of_range_scroll_positions() {
        // viewport = 500, max_offset = 1500 => content = 2000 => visible_ratio = 0.25
        // track = 800 => thumb_height = 200, max_thumb_top = 600
        let top = compute_scrollbar_metrics(0.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        assert!((top.thumb_height - 200.0).abs() < 1e-4);
        assert!((top.thumb_top - 0.0).abs() < 1e-4);

        let mid = compute_scrollbar_metrics(750.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        assert!((mid.thumb_height - 200.0).abs() < 1e-4);
        assert!((mid.thumb_top - 300.0).abs() < 1e-4);

        let bottom = compute_scrollbar_metrics(1500.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        assert!((bottom.thumb_top - 600.0).abs() < 1e-4);
        assert!((bottom.thumb_top + bottom.thumb_height - 800.0).abs() < 1e-4);

        // Out-of-bounds scroll values clamp cleanly.
        let under = compute_scrollbar_metrics(-200.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        assert_eq!(under.scroll_y, 0.0);
        assert_eq!(under.thumb_top, 0.0);

        let over = compute_scrollbar_metrics(5000.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        assert_eq!(over.scroll_y, 1500.0);
        assert!((over.thumb_top - 600.0).abs() < 1e-4);
    }

    #[test]
    fn pointer_y_to_scroll_offset_conversion_and_clamping() {
        // track_top = 100, track_height = 800, thumb_height = 200 => max_thumb_top = 600, max_offset = 1500
        let metrics = compute_scrollbar_metrics(0.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        let track_top = 100.0;
        let grab_center = metrics.thumb_height * 0.5; // 100.0

        // Pointer at top edge of track -> clamps to 0.0
        assert_eq!(
            scroll_offset_for_pointer_y(100.0, track_top, grab_center, metrics),
            0.0
        );
        // Pointer far above track -> clamps to 0.0
        assert_eq!(
            scroll_offset_for_pointer_y(-500.0, track_top, grab_center, metrics),
            0.0
        );
        // Pointer at exact vertical center of track (100 + 400 = 500) -> 750.0 (exact midpoint)
        assert!(
            (scroll_offset_for_pointer_y(500.0, track_top, grab_center, metrics) - 750.0).abs()
                < 1e-3
        );
        // Pointer at bottom edge of track (100 + 800 = 900) -> clamps to 1500.0
        assert_eq!(
            scroll_offset_for_pointer_y(900.0, track_top, grab_center, metrics),
            1500.0
        );
        // Pointer far below track -> clamps to 1500.0
        assert_eq!(
            scroll_offset_for_pointer_y(2500.0, track_top, grab_center, metrics),
            1500.0
        );
        // Dragging from a specific grab point inside the thumb (e.g. 40px below thumb top)
        // Moving pointer to y = 100 + 300 + 40 = 440 places thumb_top at 300 => scroll 750.0
        assert!(
            (scroll_offset_for_pointer_y(440.0, track_top, 40.0, metrics) - 750.0).abs() < 1e-3
        );
        // Non-finite pointer preserves current clamped scroll_y
        let mid_metrics = compute_scrollbar_metrics(420.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        assert_eq!(
            scroll_offset_for_pointer_y(f32::NAN, track_top, 40.0, mid_metrics),
            420.0
        );
    }

    #[test]
    fn scrollbar_press_action_distinguishes_thumb_grab_from_track_jump() {
        // track_top = 100, thumb_height = 200, thumb_top = 300 (occupies y = 400..=600)
        let metrics = compute_scrollbar_metrics(750.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        let track_top = 100.0;

        // Pressing inside the thumb at y = 460 -> grab_offset = 60, no jump
        let (grab_in, jump_in) = scrollbar_press_action(460.0, track_top, metrics);
        assert!((grab_in - 60.0).abs() < 1e-4);
        assert_eq!(jump_in, None);

        // Pressing track above thumb at y = 250 -> centers thumb around y = 250 => thumb_top = 50 => scroll = 125
        let (grab_above, jump_above) = scrollbar_press_action(250.0, track_top, metrics);
        assert!((grab_above - 100.0).abs() < 1e-4);
        let target_above = jump_above.expect("track click above thumb should jump immediately");
        assert!((target_above - 125.0).abs() < 1e-3);

        // Pressing track at bottom edge y = 900 -> jumps to max_offset (1500)
        let (_, jump_bottom) = scrollbar_press_action(900.0, track_top, metrics);
        assert_eq!(jump_bottom, Some(1500.0));
    }

    #[test]
    fn scrollbar_track_span_aligns_with_topnav_and_subpane_bounds() {
        // Hero route full-height container (0..900) clears floating TopNav (72px)
        let (hero_top, hero_h) =
            compute_scrollbar_track_span(0.0, 900.0, 900.0, true, 1.0).unwrap();
        assert_eq!(hero_top, SCROLLBAR_TOPNAV_CLEARANCE_PX);
        assert_eq!(hero_top + hero_h, 900.0 - SCROLLBAR_BOTTOM_INSET_PX);

        // Discover/Library sub-pane starting below sticky header & toolbar (220..876)
        let (grid_top, grid_h) =
            compute_scrollbar_track_span(220.0, 876.0, 900.0, false, 1.0).unwrap();
        assert_eq!(grid_top, 220.0);
        assert_eq!(grid_top + grid_h, 872.0);

        // Unmeasured container (0..0) falls back cleanly
        let (fb_top, fb_h) =
            compute_scrollbar_track_span(0.0, 0.0, 900.0, false, 1.0).unwrap();
        assert_eq!(fb_top, 76.0);
        assert_eq!(fb_top + fb_h, 890.0);

        // Invalid viewport returns None
        assert_eq!(
            compute_scrollbar_track_span(0.0, 900.0, 0.0, true, 1.0),
            None
        );
    }

    #[test]
    fn scroll_geometry_changed_detects_prepaint_deltas_and_converges() {
        let base = (1200.0, 76.0, 900.0, 0.0);
        // Identical or sub-epsilon deltas do not retrigger render.
        assert!(!scroll_geometry_changed(base, base));
        assert!(!scroll_geometry_changed(base, (1200.4, 76.2, 899.7, 0.1)));
        assert!(!scroll_geometry_changed(base, (f32::NAN, 76.0, 900.0, 0.0)));

        // Meaningful delta on any axis triggers a 1-frame sync render.
        assert!(scroll_geometry_changed((0.0, 76.0, 900.0, 0.0), base));
        assert!(scroll_geometry_changed(base, (1200.0, 210.0, 900.0, 0.0)));
        assert!(scroll_geometry_changed(base, (1200.0, 76.0, 760.0, 0.0)));
        assert!(scroll_geometry_changed(base, (1200.0, 76.0, 900.0, 340.0)));
    }
}
