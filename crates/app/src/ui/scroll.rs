use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Instant,
};

use gpui::{
    div, point, prelude::*, px, quad, rgb, rgba, size, transparent_black, App, BorderStyle, Bounds,
    BoxShadow, Corners, CursorStyle, DispatchPhase, Div, Edges, Element, ElementId, EntityId,
    FontWeight, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Position, ScrollHandle, Stateful, Style,
    Window,
};

use crate::{
    icons::{svg_icon, IconName},
    theme::{active_theme, sp, ui_sans, RADIUS_PILL},
    ui::layout::page_gutter_px,
    views::top_nav::is_topnav_scrolled,
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

/// Minimum vertical delta (in logical pixels) required before scrollbar dragging
/// mutates scroll offset or triggers a view notification.
pub const SCROLLBAR_DRAG_EPSILON_PX: f32 = 0.5;

/// Active pointer drag state for the paint-time vertical scrollbar.
#[derive(Clone)]
pub struct ScrollbarDragState {
    pub route_key: String,
    pub handle: ScrollHandle,
    pub track_top: f32,
    pub grab_offset_y: f32,
    pub metrics: ScrollbarMetrics,
}

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

/// Evaluates the two coarse scroll-driven chrome booleans:
/// `(topnav_is_scrolled, show_back_to_top)`.
///
/// Used to gate follow-up view notifications strictly to boolean threshold
/// crossings (`24px` and `320px`) rather than raw pixel deltas.
#[inline]
pub fn scroll_chrome_flags(scroll_y: f32, suppressed: bool) -> (bool, bool) {
    (
        is_topnav_scrolled(scroll_y),
        !suppressed && should_show_back_to_top(scroll_y),
    )
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

/// Prepaint layout result for [`ScrollbarElement`].
pub struct ScrollbarPrepaint {
    hitbox: Hitbox,
    rail_bounds: Bounds<Pixels>,
    track_top: f32,
    metrics: ScrollbarMetrics,
    scale: f32,
}

/// Custom paint-time vertical scrollbar element mounted as a sibling AFTER
/// `active_content`.
///
/// Reads live `ScrollHandle` geometry (`offset`, `max_offset`, `bounds`) during
/// `prepaint` — immediately after `active_content` has clamped scroll offsets
/// and laid out its container — and paints the track and pill thumb directly via
/// `window.paint_quad(...)` in the same frame with zero follow-up re-renders.
///
/// Follow-up `window.on_next_frame(cx.notify)` is scheduled **strictly** when
/// one of the two coarse chrome booleans (`topnav_is_scrolled` at `24px` or
/// `show_back_to_top` at `320px`) flips.
pub struct ScrollbarElement {
    route_key: String,
    route_hash: u64,
    handle: ScrollHandle,
    is_hero_route: bool,
    suppressed: bool,
    notify_entity_id: EntityId,
    drag_state: Rc<RefCell<Option<ScrollbarDragState>>>,
    hovered_state: Rc<Cell<bool>>,
    last_chrome_flags: Rc<Cell<(bool, bool)>>,
    last_scroll_at: Rc<Cell<Option<Instant>>>,
    last_observed_scroll: Rc<Cell<(u64, f32)>>,
}

impl ScrollbarElement {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        route_key: String,
        route_hash: u64,
        handle: ScrollHandle,
        is_hero_route: bool,
        suppressed: bool,
        notify_entity_id: EntityId,
        drag_state: Rc<RefCell<Option<ScrollbarDragState>>>,
        hovered_state: Rc<Cell<bool>>,
        last_chrome_flags: Rc<Cell<(bool, bool)>>,
        last_scroll_at: Rc<Cell<Option<Instant>>>,
        last_observed_scroll: Rc<Cell<(u64, f32)>>,
    ) -> Self {
        Self {
            route_key,
            route_hash,
            handle,
            is_hero_route,
            suppressed,
            notify_entity_id,
            drag_state,
            hovered_state,
            last_chrome_flags,
            last_scroll_at,
            last_observed_scroll,
        }
    }
}

impl Element for ScrollbarElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<ScrollbarPrepaint>;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("ferrite-vertical-scrollbar".into()))
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.position = Position::Absolute;
        let layout_id = window.request_layout(style, [], cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let post_scroll_y = (-self.handle.offset().y / px(1.0)).max(0.0);
        let post_max_y = (self.handle.max_offset().y / px(1.0)).max(0.0);
        let post_bounds = self.handle.bounds();

        // Track active scroll motion on the same route so card hover-prefetch
        // timers are suppressed while items scroll past the pointer.
        let (prev_route_hash, prev_y) = self.last_observed_scroll.get();
        if prev_route_hash != self.route_hash {
            self.last_observed_scroll
                .set((self.route_hash, post_scroll_y));
        } else if (post_scroll_y - prev_y).abs() > SCROLLBAR_DRAG_EPSILON_PX {
            self.last_observed_scroll
                .set((self.route_hash, post_scroll_y));
            self.last_scroll_at.set(Some(Instant::now()));
        }

        // Gate follow-up notifications strictly on the two coarse chrome booleans
        // (TopNav scrolled >= 24px, Back-to-top visible > 320px). Never notify on
        // raw scroll_y, max_offset_y, or bounds deltas.
        let post_flags = scroll_chrome_flags(post_scroll_y, self.suppressed);
        if post_flags != self.last_chrome_flags.get() {
            self.last_chrome_flags.set(post_flags);
            let entity_id = self.notify_entity_id;
            window.on_next_frame(move |_window, cx| {
                cx.notify(entity_id);
            });
        }

        if self.suppressed {
            self.hovered_state.set(false);
            return None;
        }

        let theme = active_theme();
        let scale = if theme.ui_scale.is_finite() && theme.ui_scale > 0.0 {
            theme.ui_scale
        } else {
            1.0
        };
        let vp = window.viewport_size();
        let viewport_w = f32::from(vp.width);
        let viewport_h = f32::from(vp.height);

        let container_top = f32::from(post_bounds.top());
        let container_bottom = f32::from(post_bounds.bottom());
        let container_height = f32::from(post_bounds.size.height);

        let (track_top, track_height) = compute_scrollbar_track_span(
            container_top,
            container_bottom,
            viewport_h,
            self.is_hero_route,
            scale,
        )?;
        let vp_h = if container_height > 1.0 {
            container_height
        } else {
            track_height
        };
        let metrics = compute_scrollbar_metrics(
            post_scroll_y,
            post_max_y,
            vp_h,
            track_height,
            SCROLLBAR_MIN_THUMB_HEIGHT_PX * scale,
        )?;

        let rail_w = SCROLLBAR_RAIL_WIDTH_PX * scale;
        let rail_left = (viewport_w - SCROLLBAR_RIGHT_INSET_PX - rail_w).max(0.0);
        let rail_bounds = Bounds {
            origin: point(px(rail_left), px(track_top)),
            size: size(px(rail_w), px(metrics.track_height)),
        };
        let hitbox = window.insert_hitbox(rail_bounds, HitboxBehavior::BlockMouse);

        Some(ScrollbarPrepaint {
            hitbox,
            rail_bounds,
            track_top,
            metrics,
            scale,
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        let is_dragging = self.drag_state.borrow().is_some();
        let entity_id = self.notify_entity_id;

        // Register Capture-phase drag listeners ONLY while actively dragging.
        // When idle, zero capture listeners are installed on the window.
        if is_dragging {
            let drag_for_move = Rc::clone(&self.drag_state);
            let last_scroll_for_move = Rc::clone(&self.last_scroll_at);
            window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                let mut drag_ref = drag_for_move.borrow_mut();
                let Some(drag) = drag_ref.as_ref() else {
                    return;
                };
                if ev.pressed_button != Some(MouseButton::Left) {
                    *drag_ref = None;
                    cx.notify(entity_id);
                    return;
                }
                let target_y = scroll_offset_for_pointer_y(
                    f32::from(ev.position.y),
                    drag.track_top,
                    drag.grab_offset_y,
                    drag.metrics,
                );
                let cur_offset = drag.handle.offset();
                let prev_y = (-cur_offset.y / px(1.0)).max(0.0);
                if (target_y - prev_y).abs() >= SCROLLBAR_DRAG_EPSILON_PX {
                    drag.handle.set_offset(point(cur_offset.x, px(-target_y)));
                    last_scroll_for_move.set(Some(Instant::now()));
                    cx.notify(entity_id);
                }
            });

            let drag_for_up = Rc::clone(&self.drag_state);
            window.on_mouse_event(move |ev: &MouseUpEvent, phase, _window, cx| {
                if phase != DispatchPhase::Capture || ev.button != MouseButton::Left {
                    return;
                }
                if drag_for_up.borrow_mut().take().is_some() {
                    cx.notify(entity_id);
                }
            });
        }

        let Some(prep) = prepaint.take() else {
            return;
        };

        let is_hovered = prep.hitbox.is_hovered(window);
        self.hovered_state.set(is_hovered);
        let is_active = is_hovered || is_dragging;

        window.set_cursor_style(CursorStyle::PointingHand, &prep.hitbox);

        let theme = active_theme();
        let rail_w = f32::from(prep.rail_bounds.size.width);
        let thumb_w = if is_active {
            SCROLLBAR_THUMB_WIDTH_ACTIVE_PX * prep.scale
        } else {
            SCROLLBAR_THUMB_WIDTH_REST_PX * prep.scale
        };
        let thumb_left = ((rail_w - thumb_w) * 0.5).max(0.0);

        if is_active {
            window.paint_quad(quad(
                prep.rail_bounds,
                Corners::all(px(RADIUS_PILL)),
                rgba(theme.glass_trough_bg),
                Edges::all(px(1.0)),
                rgba(theme.glass_divider),
                BorderStyle::default(),
            ));
        }

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

        let thumb_bounds = Bounds {
            origin: point(
                prep.rail_bounds.origin.x + px(thumb_left),
                prep.rail_bounds.origin.y + px(prep.metrics.thumb_top),
            ),
            size: size(px(thumb_w), px(prep.metrics.thumb_height)),
        };
        window.paint_quad(quad(
            thumb_bounds,
            Corners::all(px(RADIUS_PILL)),
            thumb_bg,
            Edges::all(px(0.0)),
            transparent_black(),
            BorderStyle::default(),
        ));

        // Hover-enter / hover-leave detection on the rail (only when not dragging).
        if !is_dragging {
            let hover_hitbox = prep.hitbox.clone();
            let hovered_cell = Rc::clone(&self.hovered_state);
            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let now_hovered = hover_hitbox.is_hovered(window);
                if now_hovered != hovered_cell.get() {
                    hovered_cell.set(now_hovered);
                    cx.notify(entity_id);
                }
            });
        }

        // MouseDown on the scrollbar rail: thumb grab or immediate track jump.
        let down_hitbox = prep.hitbox;
        let handle_for_down = self.handle.clone();
        let drag_for_down = Rc::clone(&self.drag_state);
        let last_scroll_for_down = Rc::clone(&self.last_scroll_at);
        let route_key = self.route_key.clone();
        let track_top = prep.track_top;
        let metrics = prep.metrics;
        window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || ev.button != MouseButton::Left
                || !down_hitbox.is_hovered(window)
            {
                return;
            }
            cx.stop_propagation();
            let pointer_y = f32::from(ev.position.y);
            let (grab_offset_y, immediate_scroll_y) =
                scrollbar_press_action(pointer_y, track_top, metrics);
            if let Some(target_y) = immediate_scroll_y {
                let cur_x = handle_for_down.offset().x;
                handle_for_down.set_offset(point(cur_x, px(-target_y)));
                last_scroll_for_down.set(Some(Instant::now()));
            }
            *drag_for_down.borrow_mut() = Some(ScrollbarDragState {
                route_key: route_key.clone(),
                handle: handle_for_down.clone(),
                track_top,
                grab_offset_y,
                metrics,
            });
            cx.notify(entity_id);
        });
    }
}

/// Lightweight single-layer drop shadow for the floating Back-to-top pill.
#[inline]
fn back_to_top_shadow(is_light: bool) -> Vec<BoxShadow> {
    let color = if is_light {
        rgba(0x0F172A1E)
    } else {
        rgba(0x00000040)
    };
    vec![BoxShadow::new(px(0.0), px(2.0), color.into()).blur_radius(px(6.0))]
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
        .shadow(back_to_top_shadow(theme.is_light))
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
    fn scroll_chrome_flags_only_flip_at_coarse_thresholds() {
        assert_eq!(scroll_chrome_flags(0.0, false), (false, false));
        assert_eq!(scroll_chrome_flags(23.9, false), (false, false));
        assert_eq!(scroll_chrome_flags(24.0, false), (true, false));
        assert_eq!(scroll_chrome_flags(180.0, false), (true, false));
        assert_eq!(scroll_chrome_flags(320.0, false), (true, false));
        assert_eq!(scroll_chrome_flags(320.1, false), (true, true));
        assert_eq!(scroll_chrome_flags(9000.0, false), (true, true));

        // Suppressed overlay hides Back-to-top while preserving TopNav scrolled flag.
        assert_eq!(scroll_chrome_flags(640.0, true), (true, false));
        assert_eq!(scroll_chrome_flags(0.0, true), (false, false));
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
        let m = compute_scrollbar_metrics(0.0, 100_000.0, 800.0, 700.0, 40.0)
            .expect("overflowing list should produce metrics");
        assert!((m.thumb_height - 40.0).abs() < 1e-4);
        assert_eq!(m.thumb_top, 0.0);

        let tiny = compute_scrollbar_metrics(50.0, 500.0, 400.0, 28.0, 40.0)
            .expect("tiny track still returns bounded metrics");
        assert!((tiny.thumb_height - 28.0).abs() < 1e-4);
        assert_eq!(tiny.thumb_top, 0.0);
    }

    #[test]
    fn scrollbar_metrics_top_mid_bottom_and_out_of_range_scroll_positions() {
        let top = compute_scrollbar_metrics(0.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        assert!((top.thumb_height - 200.0).abs() < 1e-4);
        assert!((top.thumb_top - 0.0).abs() < 1e-4);

        let mid = compute_scrollbar_metrics(750.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        assert!((mid.thumb_height - 200.0).abs() < 1e-4);
        assert!((mid.thumb_top - 300.0).abs() < 1e-4);

        let bottom = compute_scrollbar_metrics(1500.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        assert!((bottom.thumb_top - 600.0).abs() < 1e-4);
        assert!((bottom.thumb_top + bottom.thumb_height - 800.0).abs() < 1e-4);

        let under = compute_scrollbar_metrics(-200.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        assert_eq!(under.scroll_y, 0.0);
        assert_eq!(under.thumb_top, 0.0);

        let over = compute_scrollbar_metrics(5000.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        assert_eq!(over.scroll_y, 1500.0);
        assert!((over.thumb_top - 600.0).abs() < 1e-4);
    }

    #[test]
    fn pointer_y_to_scroll_offset_conversion_and_clamping() {
        let metrics = compute_scrollbar_metrics(0.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        let track_top = 100.0;
        let grab_center = metrics.thumb_height * 0.5;

        assert_eq!(
            scroll_offset_for_pointer_y(100.0, track_top, grab_center, metrics),
            0.0
        );
        assert_eq!(
            scroll_offset_for_pointer_y(-500.0, track_top, grab_center, metrics),
            0.0
        );
        assert!(
            (scroll_offset_for_pointer_y(500.0, track_top, grab_center, metrics) - 750.0).abs()
                < 1e-3
        );
        assert_eq!(
            scroll_offset_for_pointer_y(900.0, track_top, grab_center, metrics),
            1500.0
        );
        assert_eq!(
            scroll_offset_for_pointer_y(2500.0, track_top, grab_center, metrics),
            1500.0
        );
        assert!(
            (scroll_offset_for_pointer_y(440.0, track_top, 40.0, metrics) - 750.0).abs() < 1e-3
        );
        let mid_metrics = compute_scrollbar_metrics(420.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        assert_eq!(
            scroll_offset_for_pointer_y(f32::NAN, track_top, 40.0, mid_metrics),
            420.0
        );
    }

    #[test]
    fn scrollbar_press_action_distinguishes_thumb_grab_from_track_jump() {
        let metrics = compute_scrollbar_metrics(750.0, 1500.0, 500.0, 800.0, 40.0).unwrap();
        let track_top = 100.0;

        let (grab_in, jump_in) = scrollbar_press_action(460.0, track_top, metrics);
        assert!((grab_in - 60.0).abs() < 1e-4);
        assert_eq!(jump_in, None);

        let (grab_above, jump_above) = scrollbar_press_action(250.0, track_top, metrics);
        assert!((grab_above - 100.0).abs() < 1e-4);
        let target_above = jump_above.expect("track click above thumb should jump immediately");
        assert!((target_above - 125.0).abs() < 1e-3);

        let (_, jump_bottom) = scrollbar_press_action(900.0, track_top, metrics);
        assert_eq!(jump_bottom, Some(1500.0));
    }

    #[test]
    fn scrollbar_track_span_aligns_with_topnav_and_subpane_bounds() {
        let (hero_top, hero_h) =
            compute_scrollbar_track_span(0.0, 900.0, 900.0, true, 1.0).unwrap();
        assert_eq!(hero_top, SCROLLBAR_TOPNAV_CLEARANCE_PX);
        assert_eq!(hero_top + hero_h, 900.0 - SCROLLBAR_BOTTOM_INSET_PX);

        let (grid_top, grid_h) =
            compute_scrollbar_track_span(220.0, 876.0, 900.0, false, 1.0).unwrap();
        assert_eq!(grid_top, 220.0);
        assert_eq!(grid_top + grid_h, 872.0);

        let (fb_top, fb_h) =
            compute_scrollbar_track_span(0.0, 0.0, 900.0, false, 1.0).unwrap();
        assert_eq!(fb_top, 76.0);
        assert_eq!(fb_top + fb_h, 890.0);

        assert_eq!(
            compute_scrollbar_track_span(0.0, 900.0, 0.0, true, 1.0),
            None
        );
    }
}
