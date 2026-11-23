use gpui::{
    div, prelude::*, px, rgb, rgba, AnyElement, App, BoxShadow, Div, ElementId, FontWeight,
    MouseButton, MouseDownEvent, SharedString, Stateful, Window,
};

use crate::icons::{svg_icon_weighted, IconName, IconWeight};
use crate::theme::{
    active_theme, build_palette, tabular_nums, ui_sans, ThemePalette, ThemePreset,
};

pub(super) const SHEET_WIDTH: f32 = 388.0;
const FALLBACK_SUBTITLE_COLOR: u32 = 0xF5F1E8;

/// Resolves a dark-cinematic HUD palette from any runtime theme palette.
/// When `LightMist` is active, glass surfaces and ink stay dark-cinematic
/// (Ethereal Dark) so the player OSD and floating sheets never turn blinding
/// white over live video playback, while preserving the user's chosen accent,
/// ambient bleed, UI scale, and density tokens.
pub(super) const fn cinema_hud_palette_from(t: ThemePalette) -> ThemePalette {
    if t.is_light {
        build_palette(
            ThemePreset::EtherealDark,
            t.accent_choice,
            t.ambient_bleed,
            t.ui_scale,
            t.density,
        )
    } else {
        t
    }
}

#[inline]
pub(super) fn player_hud_palette() -> ThemePalette {
    cinema_hud_palette_from(active_theme())
}

pub(super) fn player_glass_shadow() -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.0), px(12.0), rgba(0x00000080).into()).blur_radius(px(32.0)),
        BoxShadow::new(px(0.0), px(2.0), rgba(0x0000004d).into()).blur_radius(px(6.0)),
    ]
}

pub(super) fn player_meta_divider(t: &ThemePalette) -> Div {
    div()
        .w(px(1.0))
        .h(px(11.0))
        .flex_shrink_0()
        .bg(rgba(t.glass_divider))
}

pub(super) const SUBTITLE_SWATCHES: &[(&str, &str)] = &[
    ("#F5F1E8", "Warm cream"),
    ("#FFFFFF", "Pure white"),
    ("#E5E7EB", "Soft silver"),
    ("#FDE68A", "Cinema amber"),
    ("#BAE6FD", "Cool ice"),
];

pub(super) fn parse_hex_rgb(hex: &str) -> u32 {
    let clean = hex.trim().trim_start_matches('#');
    if clean.len() == 6 {
        u32::from_str_radix(clean, 16).unwrap_or(FALLBACK_SUBTITLE_COLOR)
    } else if clean.len() == 8 {
        u32::from_str_radix(clean, 16)
            .map(|v| v & 0x00FF_FFFF)
            .unwrap_or(FALLBACK_SUBTITLE_COLOR)
    } else {
        FALLBACK_SUBTITLE_COLOR
    }
}

pub(super) fn subtitle_size_short_badge(font_size: u32) -> &'static str {
    match font_size {
        0..=41 => "S",
        42..=51 => "M",
        52..=61 => "L",
        _ => "XL",
    }
}

pub(super) fn subtitle_color_short_name(hex: &str) -> String {
    let trimmed = hex.trim();
    for &(swatch_hex, name) in SUBTITLE_SWATCHES {
        if trimmed.eq_ignore_ascii_case(swatch_hex) {
            return name.to_string();
        }
    }
    trimmed.to_ascii_uppercase()
}

/// Outer frosted card container shared by all right-anchored player floating sheets.
pub(super) fn player_sheet_card(id: impl Into<ElementId>) -> Stateful<Div> {
    let t = player_hud_palette();
    div()
        .id(id)
        .w(px(SHEET_WIDTH))
        .rounded(px(16.0))
        .bg(rgba(t.glass_sheet_bg))
        .border_1()
        .border_color(rgba(t.glass_border))
        .shadow(player_glass_shadow())
        .overflow_hidden()
        .flex()
        .flex_col()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

pub(super) fn sheet_header_close(
    title: impl Into<SharedString>,
    close_id: &'static str,
    on_close: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let t = player_hud_palette();
    div()
        .w_full()
        .px(px(20.0))
        .py(px(15.0))
        .border_b_1()
        .border_color(rgba(t.glass_divider))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .font(ui_sans(FontWeight::SEMIBOLD))
                .text_size(px(15.0))
                .text_color(rgb(t.text_primary))
                .child(title.into()),
        )
        .child(
            div()
                .id(close_id)
                .w(px(28.0))
                .h(px(28.0))
                .rounded_full()
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                    cx.stop_propagation();
                    on_close(ev, window, cx);
                })
                .child(svg_icon_weighted(
                    IconName::X,
                    IconWeight::Regular,
                    15.0,
                    rgb(t.text_secondary),
                )),
        )
}

pub(super) fn sheet_header_back(
    title: impl Into<SharedString>,
    back_id: &'static str,
    on_back: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    right_slot: Option<AnyElement>,
) -> Div {
    let t = player_hud_palette();
    div()
        .w_full()
        .px(px(18.0))
        .py(px(15.0))
        .border_b_1()
        .border_color(rgba(t.glass_divider))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .id(back_id)
                .rounded(px(8.0))
                .px(px(4.0))
                .py(px(2.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(6.0))
                .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                    cx.stop_propagation();
                    on_back(ev, window, cx);
                })
                .child(svg_icon_weighted(
                    IconName::ChevronLeft,
                    IconWeight::Regular,
                    16.0,
                    rgb(t.text_secondary),
                ))
                .child(
                    div()
                        .font(ui_sans(FontWeight::SEMIBOLD))
                        .text_size(px(15.0))
                        .text_color(rgb(t.text_primary))
                        .child(title.into()),
                ),
        )
        .when_some(right_slot, |d, el| d.child(el))
}

pub(super) fn section_eyebrow(label: impl Into<SharedString>) -> Div {
    let t = player_hud_palette();
    div()
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(px(12.0))
        .text_color(rgb(t.text_muted))
        .child(label.into())
}

pub(super) fn segmented_trough() -> Div {
    let t = player_hud_palette();
    div()
        .w_full()
        .p(px(3.0))
        .rounded(px(10.0))
        .bg(rgba(t.glass_trough_bg))
        .flex()
        .items_center()
        .gap(px(2.0))
}

pub(super) fn segmented_item(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = player_hud_palette();
    let base = div()
        .id(id)
        .flex_1()
        .h(px(32.0))
        .px(px(6.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .whitespace_nowrap()
        .text_size(px(12.5))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        });
    if active {
        base.bg(rgb(t.accent))
            .font(ui_sans(FontWeight::SEMIBOLD))
            .text_color(rgb(t.cta_on_accent))
    } else {
        base.bg(rgba(0x00000000))
            .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
            .font(ui_sans(FontWeight::MEDIUM))
            .text_color(rgb(t.text_secondary))
    }
    .child(label.into())
}

pub(super) fn selectable_row(
    id: impl Into<ElementId>,
    primary: impl Into<SharedString>,
    secondary: Option<SharedString>,
    selected: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = player_hud_palette();
    div()
        .id(id)
        .w_full()
        .flex_shrink_0()
        .px(px(14.0))
        .py(px(10.0))
        .rounded(px(10.0))
        .cursor_pointer()
        .bg(if selected {
            rgba(t.accent_soft)
        } else {
            rgba(0x00000000)
        })
        .border_1()
        .border_color(if selected {
            rgba(t.border_accent)
        } else {
            rgba(0x00000000)
        })
        .hover(move |s| {
            if selected {
                s
            } else {
                s.bg(rgba(t.glass_row_hover_bg))
            }
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .font(ui_sans(FontWeight::MEDIUM))
                        .text_size(px(13.5))
                        .text_color(rgb(t.text_primary))
                        .truncate()
                        .child(primary.into()),
                )
                .when_some(secondary, |d, sec| {
                    d.child(
                        div()
                            .font(ui_sans(FontWeight::NORMAL))
                            .text_size(px(12.0))
                            .text_color(rgb(t.text_muted))
                            .truncate()
                            .child(sec),
                    )
                }),
        )
        .when(selected, |d| {
            d.child(svg_icon_weighted(
                IconName::Check,
                IconWeight::Fill,
                15.0,
                rgb(t.accent),
            ))
        })
}

pub(super) fn drilldown_row(
    id: &'static str,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = player_hud_palette();
    div()
        .id(id)
        .w_full()
        .py(px(8.0))
        .px(px(4.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(
            div()
                .font(ui_sans(FontWeight::MEDIUM))
                .text_size(px(13.5))
                .text_color(rgb(t.text_primary))
                .child(label.into()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .font(ui_sans(FontWeight::NORMAL))
                        .text_size(px(13.0))
                        .text_color(rgb(t.text_secondary))
                        .child(value.into()),
                )
                .child(svg_icon_weighted(
                    IconName::ChevronRight,
                    IconWeight::Regular,
                    14.0,
                    rgb(t.text_muted),
                )),
        )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn stepper_row(
    label: impl Into<SharedString>,
    minus_id: &'static str,
    reset_id: &'static str,
    plus_id: &'static str,
    value: impl Into<SharedString>,
    on_minus: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_reset: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_plus: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let t = player_hud_palette();
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .font(ui_sans(FontWeight::MEDIUM))
                .text_size(px(13.5))
                .text_color(rgb(t.text_primary))
                .child(label.into()),
        )
        .child(
            div()
                .h(px(34.0))
                .min_w(px(114.0))
                .rounded_full()
                .bg(rgba(t.glass_trough_bg))
                .px(px(6.0))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(4.0))
                .child(
                    div()
                        .id(minus_id)
                        .w(px(24.0))
                        .h(px(24.0))
                        .rounded_full()
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                            cx.stop_propagation();
                            on_minus(ev, window, cx);
                        })
                        .child(svg_icon_weighted(
                            IconName::Minus,
                            IconWeight::Regular,
                            13.0,
                            rgb(t.text_secondary),
                        )),
                )
                .child(
                    div()
                        .id(reset_id)
                        .flex_1()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .whitespace_nowrap()
                        .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                        .font(ui_sans(FontWeight::MEDIUM))
                        .font_features(tabular_nums())
                        .text_size(px(12.5))
                        .text_color(rgb(t.text_primary))
                        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                            cx.stop_propagation();
                            on_reset(ev, window, cx);
                        })
                        .child(value.into()),
                )
                .child(
                    div()
                        .id(plus_id)
                        .w(px(24.0))
                        .h(px(24.0))
                        .rounded_full()
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                            cx.stop_propagation();
                            on_plus(ev, window, cx);
                        })
                        .child(svg_icon_weighted(
                            IconName::Plus,
                            IconWeight::Regular,
                            13.0,
                            rgb(t.text_secondary),
                        )),
                ),
        )
}

pub(super) fn toggle_switch_row(
    id: &'static str,
    label: impl Into<SharedString>,
    description: Option<&'static str>,
    enabled: bool,
    on_toggle: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = player_hud_palette();
    div()
        .id(id)
        .w_full()
        .py(px(4.0))
        .px(px(2.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_toggle(ev, window, cx);
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .font(ui_sans(FontWeight::MEDIUM))
                        .text_size(px(13.5))
                        .text_color(rgb(t.text_primary))
                        .child(label.into()),
                )
                .when_some(description, |d, desc| {
                    d.child(
                        div()
                            .font(ui_sans(FontWeight::NORMAL))
                            .text_size(px(11.5))
                            .text_color(rgb(t.text_muted))
                            .child(desc),
                    )
                }),
        )
        .child(
            div()
                .w(px(40.0))
                .h(px(22.0))
                .flex_shrink_0()
                .rounded_full()
                .p(px(2.0))
                .flex()
                .items_center()
                .when(enabled, |s| s.bg(rgb(t.accent)).justify_end())
                .when(!enabled, |s| {
                    s.bg(rgba(t.glass_border_strong)).justify_start()
                })
                .child(
                    div()
                        .w(px(18.0))
                        .h(px(18.0))
                        .rounded_full()
                        .bg(rgb(0xFFFFFF)),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{AccentChoice, CardDensityChoice, UiScaleChoice};

    #[test]
    fn parse_hex_rgb_parses_rgb_and_falls_back_cleanly() {
        assert_eq!(parse_hex_rgb("#F5F1E8"), 0xF5F1E8);
        assert_eq!(parse_hex_rgb("fde68a"), 0xFDE68A);
        assert_eq!(parse_hex_rgb("#FFBAE6FD"), 0xBAE6FD);
        assert_eq!(parse_hex_rgb("not-hex"), FALLBACK_SUBTITLE_COLOR);
    }

    #[test]
    fn cinema_hud_palette_stays_dark_under_light_mist_and_syncs_all_accents() {
        for preset in ThemePreset::ALL {
            for accent in AccentChoice::ALL {
                let raw = build_palette(
                    preset,
                    accent,
                    true,
                    UiScaleChoice::S100.factor(),
                    CardDensityChoice::Comfortable,
                );
                let hud = cinema_hud_palette_from(raw);
                assert!(!hud.is_light, "player HUD must remain dark-cinematic");
                assert_eq!(hud.accent_choice, accent);
                assert_eq!(hud.accent, accent.hex());
                assert_eq!(hud.accent_hover, accent.hover_hex());
                assert_eq!(hud.cta_on_accent, accent.on_accent_hex());
                if preset.is_light() {
                    assert_eq!(hud.preset, ThemePreset::EtherealDark);
                    assert_ne!(hud.glass_sheet_bg, raw.glass_sheet_bg);
                } else {
                    assert_eq!(hud.preset, preset);
                    assert_eq!(hud.glass_sheet_bg, raw.glass_sheet_bg);
                }
            }
        }
    }

    #[test]
    fn subtitle_size_short_badge_maps_thresholds() {
        assert_eq!(subtitle_size_short_badge(38), "S");
        assert_eq!(subtitle_size_short_badge(41), "S");
        assert_eq!(subtitle_size_short_badge(42), "M");
        assert_eq!(subtitle_size_short_badge(51), "M");
        assert_eq!(subtitle_size_short_badge(52), "L");
        assert_eq!(subtitle_size_short_badge(61), "L");
        assert_eq!(subtitle_size_short_badge(62), "XL");
        assert_eq!(subtitle_size_short_badge(66), "XL");
    }

    #[test]
    fn subtitle_color_short_name_matches_swatches_and_uppercases_custom_hex() {
        assert_eq!(subtitle_color_short_name("#f5f1e8"), "Warm cream");
        assert_eq!(subtitle_color_short_name(" #BAE6FD "), "Cool ice");
        assert_eq!(subtitle_color_short_name(" #a1b2c3 "), "#A1B2C3");
    }
}
