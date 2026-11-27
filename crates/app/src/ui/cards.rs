use std::sync::Arc;

use gpui::{
    div, img, linear_color_stop, linear_gradient, prelude::*, px, relative, rgb, rgba, AnyElement,
    App, ElementId, FontWeight, MouseButton, MouseDownEvent, ObjectFit, RenderImage, SharedString,
    Window,
};

use crate::{
    icons::{svg_icon, svg_icon_weighted, IconName, IconWeight},
    theme::{
        active_theme, editorial_serif, mono_font, placeholder_tint_for_id, poster_card_shadow,
        poster_hover_glow_shadow, tabular_nums, CardDensityChoice, ThemePalette, RADIUS_PILL,
        RADIUS_POSTER,
    },
    ui_components::{ClickHandler, HoverHandler},
    virtual_grid::PosterAspect,
};

pub use crate::ui_components::{
    card_action_dispatcher, clamp_progress_fraction, format_notification_badge_label,
    render_poster_card, render_prepared_poster_card, CardActionDispatcher, PosterCardAction,
    PosterCardCapabilities, PosterCardProps, PreparedPosterCard, PreparedPosterRenderCtx,
};

/// Canonical poster sizing metrics derived from the active `ThemePalette` (`CardDensityChoice` × `ui_scale`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PosterMetrics {
    pub card_width_px: f32,
    pub gap_px: f32,
    pub metadata_height_px: f32,
}

impl PosterMetrics {
    /// Unscaled metadata block height below poster artwork (`56px` at 100% UI scale).
    pub const BASE_METADATA_HEIGHT_PX: f32 = 56.0;

    /// Computes density- and scale-aware poster metrics for `theme`.
    pub fn from_theme(theme: &ThemePalette) -> Self {
        let scale = sanitize_ui_scale(theme.ui_scale);
        Self {
            card_width_px: theme.density.target_poster_width_px() * scale,
            gap_px: theme.density.grid_gap_px() * scale,
            metadata_height_px: Self::BASE_METADATA_HEIGHT_PX * scale,
        }
    }

    /// Computes density- and scale-aware poster metrics for the currently active theme.
    #[inline]
    pub fn from_active_theme() -> Self {
        Self::from_theme(&active_theme())
    }

    /// Computes the poster artwork height for `aspect` at `self.card_width_px`.
    #[inline]
    pub fn image_height_px(self, aspect: PosterAspect) -> f32 {
        (self.card_width_px * aspect.height_over_width()).ceil()
    }

    /// Computes total poster card height (artwork + metadata block) for `aspect`.
    #[inline]
    pub fn card_height_px(self, aspect: PosterAspect) -> f32 {
        self.image_height_px(aspect) + self.metadata_height_px
    }
}

#[inline]
fn sanitize_ui_scale(ui_scale: f32) -> f32 {
    if ui_scale.is_finite() && ui_scale > 0.0 {
        ui_scale
    } else {
        1.0
    }
}

/// Canonical 16:9 landscape card width scaled by `CardDensityChoice` and `ui_scale`:
/// Compact `256.0`, Comfortable `300.0`, Cinema `344.0` multiplied by `ui_scale`.
#[inline]
pub fn landscape_card_width_px(density: CardDensityChoice, ui_scale: f32) -> f32 {
    let base = match density {
        CardDensityChoice::Compact => 256.0,
        CardDensityChoice::Comfortable => 300.0,
        CardDensityChoice::Cinema => 344.0,
    };
    base * sanitize_ui_scale(ui_scale)
}

/// Returns `(width_px, height_px)` for the unified 16:9 landscape card under `theme`.
#[inline]
pub fn landscape_card_dimensions(theme: &ThemePalette) -> (f32, f32) {
    let w = landscape_card_width_px(theme.density, theme.ui_scale);
    let h = (w * PosterAspect::Landscape16x9.height_over_width()).ceil();
    (w, h)
}

/// Corner badge rendered on a unified 16:9 landscape card (e.g., `"NEW"`, `"TODAY"`, `"MAY 14"`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LandscapeCardBadge {
    pub label: SharedString,
    pub accent: bool,
}

impl LandscapeCardBadge {
    pub fn accent(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            accent: true,
        }
    }

    pub fn neutral(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            accent: false,
        }
    }
}

/// Props for the unified 16:9 landscape media card shared across Library Up Next,
/// Calendar Up Next, and the Home hero dock (Continue Watching & Featured).
pub struct LandscapeCardProps {
    pub id: ElementId,
    pub title: SharedString,
    pub placeholder_tint: u32,
    pub subtitle: Option<SharedString>,
    pub image: Option<Arc<RenderImage>>,
    pub width_px: Option<f32>,
    pub top_left_badge: Option<LandscapeCardBadge>,
    pub top_right_badge: Option<LandscapeCardBadge>,
    pub episode_badge: Option<(SharedString, bool)>,
    pub imdb_rating: Option<SharedString>,
    pub progress_fraction: Option<f32>,
    pub is_active: bool,
    pub is_focused: bool,
    pub play_label: Option<SharedString>,
    pub on_open: Option<ClickHandler>,
    pub on_quick_play: Option<ClickHandler>,
    pub on_quick_peek: Option<ClickHandler>,
    pub on_context_menu: Option<ClickHandler>,
    pub on_hover: Option<HoverHandler>,
}

impl LandscapeCardProps {
    pub fn new(
        id: impl Into<ElementId>,
        title: impl Into<SharedString>,
        placeholder_id: impl AsRef<str>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            placeholder_tint: placeholder_tint_for_id(placeholder_id.as_ref()),
            subtitle: None,
            image: None,
            width_px: None,
            top_left_badge: None,
            top_right_badge: None,
            episode_badge: None,
            imdb_rating: None,
            progress_fraction: None,
            is_active: false,
            is_focused: false,
            play_label: None,
            on_open: None,
            on_quick_play: None,
            on_quick_peek: None,
            on_context_menu: None,
            on_hover: None,
        }
    }

    pub fn subtitle(mut self, subtitle: impl Into<Option<SharedString>>) -> Self {
        self.subtitle = subtitle.into();
        self
    }

    pub fn image(mut self, image: impl Into<Option<Arc<RenderImage>>>) -> Self {
        self.image = image.into();
        self
    }

    pub fn width_px(mut self, width_px: impl Into<Option<f32>>) -> Self {
        self.width_px = width_px.into();
        self
    }

    pub fn top_left_badge(mut self, badge: impl Into<Option<LandscapeCardBadge>>) -> Self {
        self.top_left_badge = badge.into();
        self
    }

    pub fn top_right_badge(mut self, badge: impl Into<Option<LandscapeCardBadge>>) -> Self {
        self.top_right_badge = badge.into();
        self
    }

    /// Convenience helper setting a top-left accent `"NEW"` / `"{n} NEW"` badge when `count > 0`.
    pub fn notification_count(mut self, count: usize) -> Self {
        if let Some(label) = format_notification_badge_label(count) {
            self.top_left_badge = Some(LandscapeCardBadge::accent(label));
        }
        self
    }

    /// Inline episode code chip (e.g., `"S1 · E3"`); tinted green when `watched` is true, else accent/light.
    pub fn episode_badge(mut self, code: impl Into<SharedString>, watched: bool) -> Self {
        self.episode_badge = Some((code.into(), watched));
        self
    }

    pub fn imdb_rating(mut self, rating: impl Into<Option<SharedString>>) -> Self {
        self.imdb_rating = rating.into();
        self
    }

    pub fn progress_fraction(mut self, fraction: impl Into<Option<f32>>) -> Self {
        self.progress_fraction = fraction.into();
        self
    }

    pub fn is_active(mut self, active: bool) -> Self {
        self.is_active = active;
        self
    }

    pub fn is_focused(mut self, focused: bool) -> Self {
        self.is_focused = focused;
        self
    }

    /// When set (e.g., `Some("Resume")`), renders a bottom-right accent pill CTA for `on_quick_play`
    /// instead of the top-right circular play button.
    pub fn play_label(mut self, label: impl Into<Option<SharedString>>) -> Self {
        self.play_label = label.into();
        self
    }

    pub fn on_open(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open = Some(Box::new(handler));
        self
    }

    pub fn on_open_handler(mut self, handler: ClickHandler) -> Self {
        self.on_open = Some(handler);
        self
    }

    pub fn on_quick_play(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_quick_play = Some(Box::new(handler));
        self
    }

    pub fn on_quick_play_handler(mut self, handler: ClickHandler) -> Self {
        self.on_quick_play = Some(handler);
        self
    }

    pub fn on_quick_peek(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_quick_peek = Some(Box::new(handler));
        self
    }

    pub fn on_quick_peek_handler(mut self, handler: ClickHandler) -> Self {
        self.on_quick_peek = Some(handler);
        self
    }

    pub fn on_context_menu(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_context_menu = Some(Box::new(handler));
        self
    }

    pub fn on_context_menu_handler(mut self, handler: ClickHandler) -> Self {
        self.on_context_menu = Some(handler);
        self
    }

    pub fn on_hover(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Some(Box::new(handler));
        self
    }

    pub fn on_hover_handler(mut self, handler: HoverHandler) -> Self {
        self.on_hover = Some(handler);
        self
    }
}

/// Renders the unified 16:9 media card (`RADIUS_POSTER`, density/scale-aware width, consistent
/// cinema scrim, progress bar, quick peek/play overlays, and overlaid metadata).
pub fn render_landscape_card(props: LandscapeCardProps) -> AnyElement {
    let theme = active_theme();
    let (default_w, _) = landscape_card_dimensions(&theme);
    let card_w = props
        .width_px
        .filter(|w| w.is_finite() && *w > 0.0)
        .unwrap_or(default_w);
    let card_h = (card_w * PosterAspect::Landscape16x9.height_over_width()).ceil();

    let needs_child_ids = props.on_quick_peek.is_some() || props.on_quick_play.is_some();
    let base_id = needs_child_ids.then(|| Arc::new(props.id.clone()));
    let grp: SharedString = format!("landscape-card-grp-{}", props.id).into();

    let progress = clamp_progress_fraction(props.progress_fraction);
    let hover_glow = poster_hover_glow_shadow(&theme);
    let scrim_top_transparent = theme.scrim_bottom & 0xFFFFFF00;
    let top_hairline_rgba = if theme.is_light {
        0xFFFFFF80
    } else {
        0xFFFFFF24
    };
    let is_highlighted = props.is_active || props.is_focused;
    let bottom_offset_px = if progress.is_some() { 10.0 } else { 9.0 };

    // Split quick-play placement: pill CTA at bottom-right when `play_label` is set,
    // otherwise circular icon button in the top-right quick actions cluster.
    let (top_right_play, bottom_right_play) = match (props.on_quick_play, props.play_label) {
        (Some(handler), Some(label)) => (None, Some((handler, label))),
        (Some(handler), None) => (Some(handler), None),
        (None, _) => (None, None),
    };

    let mut card = div()
        .id(props.id)
        .group(grp.clone())
        .flex_shrink_0()
        .w(px(card_w))
        .h(px(card_h))
        .rounded(px(RADIUS_POSTER))
        .overflow_hidden()
        .relative()
        .bg(rgb(props.placeholder_tint))
        .when(theme.is_light, |el| el.shadow(poster_card_shadow()));

    if is_highlighted {
        card = card
            .border_2()
            .border_color(rgb(theme.accent))
            .shadow(hover_glow);
    } else {
        card = card
            .border_1()
            .border_color(rgba(theme.glass_border))
            .group_hover(grp.clone(), move |s| {
                s.border_color(rgba(theme.border_poster_hover))
                    .shadow(hover_glow)
            });
    }

    if let Some(on_open) = props.on_open {
        card = card
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, on_open);
    }

    if let Some(on_context_menu) = props.on_context_menu {
        card = card.on_mouse_down(MouseButton::Right, move |ev, window, cx| {
            cx.stop_propagation();
            on_context_menu(ev, window, cx);
        });
    }

    if let Some(on_hover) = props.on_hover {
        card = card.on_hover(on_hover);
    }

    // 1. 16:9 artwork layer.
    card = card.child(if let Some(tex) = props.image {
        img(tex)
            .size_full()
            .object_fit(ObjectFit::Cover)
            .into_any_element()
    } else {
        div()
            .size_full()
            .p(theme.sp(12.0))
            .bg(rgb(theme.bg_surface))
            .flex()
            .items_start()
            .child(
                div()
                    .w_full()
                    .font(editorial_serif())
                    .text_size(theme.sp(18.0))
                    .text_color(rgb(theme.text_muted))
                    .truncate()
                    .child(props.title.clone()),
            )
            .into_any_element()
    });

    // 2. Cinema bottom scrim for consistent white-ink readability across all themes.
    card = card.child(
        div()
            .absolute()
            .inset_0()
            .bg(linear_gradient(
                180.0,
                linear_color_stop(rgba(scrim_top_transparent), 0.18),
                linear_color_stop(rgba(0x0B0D14E6), 1.0),
            )),
    );

    // 3. Top hairline (active accent indicator or subtle glass highlight).
    if props.is_active {
        card = card.child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(2.0))
                .bg(rgb(theme.accent)),
        );
    } else {
        card = card.child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(1.0))
                .bg(rgba(top_hairline_rgba)),
        );
    }

    // 4. Optional top-left corner badge.
    if let Some(badge) = props.top_left_badge {
        card = card.child(render_corner_badge(badge, true, &theme));
    }

    // 5. Optional top-right corner cluster (badge and/or hover quick-peek & quick-play circles).
    let has_top_right_controls = props.on_quick_peek.is_some() || top_right_play.is_some();
    if has_top_right_controls {
        let mut actions = div()
            .absolute()
            .top(theme.sp(8.0))
            .right(theme.sp(8.0))
            .flex()
            .items_center()
            .gap(theme.sp(5.0));

        if props.is_active {
            actions = actions
                .opacity(0.94)
                .group_hover(grp.clone(), |s| s.opacity(1.0));
        } else {
            // `invisible()` hits GPUI's Visibility::Hidden early-out (zero GPU primitives at rest).
            actions = actions
                .invisible()
                .group_hover(grp.clone(), |s| s.visible());
        }

        if let (Some(on_peek), Some(base_id)) = (props.on_quick_peek, base_id.as_ref()) {
            let peek_id =
                ElementId::NamedChild(Arc::clone(base_id), SharedString::new_static("peek"));
            actions = actions.child(
                div()
                    .id(peek_id)
                    .cursor_pointer()
                    .w(theme.sp(28.0))
                    .h(theme.sp(28.0))
                    .rounded(px(RADIUS_PILL))
                    .bg(rgba(0x0B0D14CC))
                    .border_1()
                    .border_color(rgba(0xFFFFFF2E))
                    .hover(move |s| {
                        s.bg(rgb(theme.accent_press))
                            .border_color(rgb(theme.accent))
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                        cx.stop_propagation();
                        on_peek(ev, window, cx);
                    })
                    .child(svg_icon(
                        IconName::Eye,
                        13.0 * theme.ui_scale,
                        rgb(0xF5F1E8),
                    )),
            );
        }

        if let (Some(on_play), Some(base_id)) = (top_right_play, base_id.as_ref()) {
            let play_id =
                ElementId::NamedChild(Arc::clone(base_id), SharedString::new_static("play"));
            actions = actions.child(
                div()
                    .id(play_id)
                    .cursor_pointer()
                    .w(theme.sp(34.0))
                    .h(theme.sp(34.0))
                    .rounded(px(RADIUS_PILL))
                    .bg(rgb(theme.accent))
                    .hover(move |s| s.bg(rgb(theme.accent_hover)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                        cx.stop_propagation();
                        on_play(ev, window, cx);
                    })
                    .child(svg_icon_weighted(
                        IconName::Play,
                        IconWeight::Fill,
                        14.0 * theme.ui_scale,
                        rgb(theme.cta_on_accent),
                    )),
            );
        }

        card = card.child(actions);
    } else if let Some(badge) = props.top_right_badge {
        card = card.child(render_corner_badge(badge, false, &theme));
    }

    // 6. Bottom metadata overlay (+ optional Resume pill CTA).
    let has_meta_line = props.episode_badge.is_some()
        || props.imdb_rating.is_some()
        || props.subtitle.is_some();

    let mut meta_col = div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(theme.sp(2.0))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(theme.text_body())
                .line_height(theme.sp(19.0))
                .text_color(rgb(0xF5F1E8))
                .truncate()
                .child(props.title),
        );

    if has_meta_line {
        let mut meta_row = div()
            .w_full()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap(theme.sp(6.0));

        if let Some((ep_code, watched)) = props.episode_badge {
            meta_row = meta_row.child(
                div()
                    .flex_shrink_0()
                    .px(theme.sp(6.0))
                    .py(theme.sp(1.5))
                    .rounded(px(4.0))
                    .bg(rgba(0xFFFFFF22))
                    .font(mono_font())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(theme.text_micro())
                    .text_color(if watched {
                        rgb(theme.status_watched)
                    } else {
                        rgb(0xF5F1E8)
                    })
                    .child(ep_code),
            );
        }

        if let Some(rating) = props.imdb_rating {
            meta_row = meta_row.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(theme.sp(3.0))
                    .font(mono_font())
                    .text_size(theme.text_micro())
                    .text_color(rgb(0xF5F1E8))
                    .child(svg_icon(
                        IconName::Star,
                        11.5 * theme.ui_scale,
                        rgb(0xfbbf24),
                    ))
                    .child(rating),
            );
        }

        if let Some(subtitle) = props.subtitle {
            meta_row = meta_row.child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .font_weight(FontWeight::MEDIUM)
                    .font_features(tabular_nums())
                    .text_size(theme.text_micro())
                    .line_height(theme.sp(16.0))
                    .text_color(rgb(0xD8D8E2))
                    .truncate()
                    .child(subtitle),
            );
        }

        meta_col = meta_col.child(meta_row);
    }

    let mut bottom_overlay = div()
        .absolute()
        .left(theme.sp(10.0))
        .right(theme.sp(10.0))
        .bottom(theme.sp(bottom_offset_px))
        .flex()
        .items_end()
        .justify_between()
        .gap(theme.sp(8.0))
        .child(meta_col);

    if let (Some((on_play, label)), Some(base_id)) = (bottom_right_play, base_id.as_ref()) {
        let play_id = ElementId::NamedChild(Arc::clone(base_id), SharedString::new_static("play"));
        bottom_overlay = bottom_overlay.child(
            div()
                .id(play_id)
                .cursor_pointer()
                .flex_shrink_0()
                .h(theme.sp(28.0))
                .px(theme.sp(10.0))
                .rounded(px(RADIUS_PILL))
                .bg(rgb(theme.accent))
                .hover(move |s| s.bg(rgb(theme.accent_hover)))
                .flex()
                .items_center()
                .gap(theme.sp(5.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(theme.sp(11.5))
                .text_color(rgb(theme.cta_on_accent))
                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                    cx.stop_propagation();
                    on_play(ev, window, cx);
                })
                .child(svg_icon_weighted(
                    IconName::Play,
                    IconWeight::Fill,
                    10.5 * theme.ui_scale,
                    rgb(theme.cta_on_accent),
                ))
                .child(label),
        );
    }

    card = card.child(bottom_overlay);

    // 7. Bottom playback progress bar.
    if let Some(frac) = progress {
        card = card.child(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(theme.sp(3.0))
                .bg(rgba(0xFFFFFF2E))
                .child(div().h_full().w(relative(frac)).bg(rgb(theme.accent))),
        );
    }

    card.into_any_element()
}

fn render_corner_badge(badge: LandscapeCardBadge, is_left: bool, theme: &ThemePalette) -> AnyElement {
    let mut el = div()
        .absolute()
        .top(theme.sp(8.0))
        .h(theme.sp(20.0))
        .px(theme.sp(7.0))
        .rounded(px(RADIUS_PILL))
        .flex()
        .items_center()
        .justify_center()
        .font(mono_font())
        .font_weight(FontWeight::BOLD)
        .text_size(theme.sp(10.0));

    el = if is_left {
        el.left(theme.sp(8.0))
    } else {
        el.right(theme.sp(8.0))
    };

    if badge.accent {
        el.bg(rgb(theme.accent))
            .text_color(rgb(theme.cta_on_accent))
            .child(badge.label)
            .into_any_element()
    } else {
        el.bg(rgba(0x0B0D14D9))
            .border_1()
            .border_color(rgba(0xFFFFFF26))
            .text_color(rgb(0xF5F1E8))
            .child(badge.label)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{build_palette, AccentChoice, ThemePreset, UiScaleChoice};

    #[test]
    fn poster_metrics_scales_across_all_density_and_ui_scale_combinations() {
        for density in CardDensityChoice::ALL {
            for scale_choice in UiScaleChoice::ALL {
                let scale = scale_choice.factor();
                let theme = build_palette(
                    ThemePreset::EtherealDark,
                    AccentChoice::EmberCoral,
                    true,
                    scale,
                    density,
                );
                let metrics = PosterMetrics::from_theme(&theme);

                let expected_w = density.target_poster_width_px() * scale;
                let expected_gap = density.grid_gap_px() * scale;
                let expected_meta_h = 56.0 * scale;

                assert!(
                    (metrics.card_width_px - expected_w).abs() < 1e-4,
                    "density={density:?} scale={scale}: got width {}, expected {expected_w}",
                    metrics.card_width_px
                );
                assert!(
                    (metrics.gap_px - expected_gap).abs() < 1e-4,
                    "density={density:?} scale={scale}: got gap {}, expected {expected_gap}",
                    metrics.gap_px
                );
                assert!(
                    (metrics.metadata_height_px - expected_meta_h).abs() < 1e-4,
                    "density={density:?} scale={scale}: got meta_h {}, expected {expected_meta_h}",
                    metrics.metadata_height_px
                );
                assert_eq!(
                    metrics.image_height_px(PosterAspect::Portrait2x3),
                    (expected_w * 1.5).ceil()
                );
                assert_eq!(
                    metrics.card_height_px(PosterAspect::Portrait2x3),
                    (expected_w * 1.5).ceil() + expected_meta_h
                );
            }
        }
    }

    #[test]
    fn landscape_card_dimensions_scale_with_density_and_ui_scale() {
        for (density, base_w) in [
            (CardDensityChoice::Compact, 256.0_f32),
            (CardDensityChoice::Comfortable, 300.0_f32),
            (CardDensityChoice::Cinema, 344.0_f32),
        ] {
            for scale_choice in UiScaleChoice::ALL {
                let scale = scale_choice.factor();
                let theme = build_palette(
                    ThemePreset::PureOled,
                    AccentChoice::ElectricViolet,
                    false,
                    scale,
                    density,
                );
                let (w, h) = landscape_card_dimensions(&theme);
                assert!((w - base_w * scale).abs() < 1e-4);
                assert_eq!(h, (w * (9.0 / 16.0)).ceil());
            }
        }

        // Non-finite scale sanitizes safely to 1.0.
        assert_eq!(
            landscape_card_width_px(CardDensityChoice::Comfortable, f32::NAN),
            300.0
        );
    }

    #[test]
    fn landscape_card_props_builders_configure_badges_and_handlers() {
        let props = LandscapeCardProps::new("ls-1", "Severance", "tt11280740")
            .subtitle(Some(SharedString::new_static("24m left")))
            .notification_count(2)
            .episode_badge("S2 · E4", false)
            .imdb_rating(Some(SharedString::new_static("8.7")))
            .progress_fraction(Some(0.45))
            .is_active(true)
            .is_focused(true)
            .play_label(Some(SharedString::new_static("Resume")))
            .on_open(|_, _, _| {})
            .on_quick_play(|_, _, _| {})
            .on_quick_peek(|_, _, _| {})
            .on_context_menu(|_, _, _| {})
            .on_hover(|_, _, _| {});

        assert_eq!(
            props.top_left_badge,
            Some(LandscapeCardBadge::accent("2 NEW"))
        );
        assert_eq!(
            props.episode_badge,
            Some((SharedString::new_static("S2 · E4"), false))
        );
        assert!(props.is_active);
        assert!(props.is_focused);
        assert!(props.on_open.is_some());
        assert!(props.on_quick_play.is_some());
        assert!(props.on_quick_peek.is_some());
        assert!(props.on_context_menu.is_some());
        assert!(props.on_hover.is_some());
    }
}
