use gpui::{div, prelude::*, px, rgb, rgba, Context, FontWeight};

use crate::{
    shell::AppShell,
    state::{PlayerPopover, ShaderPresetChoice, ToneMappingChoice, UpscaleChoice},
    theme::ui_sans,
};

use super::components::{
    drilldown_row, player_hud_palette, player_sheet_card, section_eyebrow, segmented_item,
    segmented_trough, selectable_row, sheet_header_back, sheet_header_close, toggle_switch_row,
};

const PLAYBACK_SPEEDS: [(f64, &str); 5] = [
    (0.75, "0.75×"),
    (1.0, "1×"),
    (1.25, "1.25×"),
    (1.5, "1.5×"),
    (2.0, "2×"),
];

fn upscaler_display_label(choice: UpscaleChoice) -> &'static str {
    match choice {
        UpscaleChoice::Bilinear => "Bilinear (fast)",
        other => other.label(),
    }
}

fn shader_display_label(preset: ShaderPresetChoice) -> &'static str {
    match preset {
        ShaderPresetChoice::Off => "Off (native)",
        other => other.label(),
    }
}

fn render_picker_sheet(
    card_id: &'static str,
    title: &'static str,
    back_id: &'static str,
    scroll_id: &'static str,
    rows: impl IntoIterator<Item = gpui::Stateful<gpui::Div>>,
    cx: &mut Context<AppShell>,
) -> impl IntoElement {
    player_sheet_card(card_id)
        .child(sheet_header_back(
            title,
            back_id,
            cx.listener(|this, _, _, cx| {
                this.player_popover = Some(PlayerPopover::PictureAndPlayback);
                cx.notify();
            }),
            None,
        ))
        .child(
            div()
                .id(scroll_id)
                .w_full()
                .max_h(px(360.0))
                .overflow_y_scroll()
                .child(
                    div()
                        .w_full()
                        .px(px(14.0))
                        .py(px(12.0))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .children(rows),
                ),
        )
}

impl AppShell {
    pub(super) fn render_picture_and_playback_popover(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = player_hud_palette();
        let prefs = &self.preferences;
        let exp = prefs.experimental_video;
        let upscaler_label = upscaler_display_label(prefs.upscale_mode);
        let shader_label = shader_display_label(exp.shader_preset);

        let quality_preset_idx = match (prefs.upscale_mode, exp.deband_enabled) {
            (UpscaleChoice::Bilinear, false) => Some(0),
            (UpscaleChoice::Spline36, false) => Some(1),
            (UpscaleChoice::EwaLanczos, true) => Some(2),
            _ => None,
        };
        let advanced_open = self.picture_advanced_open;

        player_sheet_card("player-picture-playback-sheet")
            .child(sheet_header_close(
                "Picture & playback",
                "picture-playback-close",
                cx.listener(|this, _, _, cx| {
                    this.toggle_player_popover(PlayerPopover::PictureAndPlayback, cx);
                }),
            ))
            .child(
                div()
                    .px(px(18.0))
                    .py(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(section_eyebrow("Speed"))
                            .child(segmented_trough().children(
                                PLAYBACK_SPEEDS.into_iter().enumerate().map(|(idx, (spd, label))| {
                                    let active = (self.playback_speed - spd).abs() < 0.01;
                                    segmented_item(
                                        ("pp-speed", idx),
                                        label,
                                        active,
                                        cx.listener(move |this, _, _, cx| {
                                            this.set_playback_speed_user(spd, cx);
                                        }),
                                    )
                                }),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(section_eyebrow("Picture quality"))
                            .child(
                                segmented_trough()
                                    .child(segmented_item(
                                        "pp-quality-fast",
                                        "Fast",
                                        quality_preset_idx == Some(0),
                                        cx.listener(|this, _, _, cx| {
                                            this.preferences.experimental_video.deband_enabled =
                                                false;
                                            this.preferences.experimental_video.shader_preset =
                                                ShaderPresetChoice::Off;
                                            this.apply_experimental_video_to_mpv();
                                            this.set_upscale_choice(UpscaleChoice::Bilinear, cx);
                                        }),
                                    ))
                                    .child(segmented_item(
                                        "pp-quality-balanced",
                                        "Balanced",
                                        quality_preset_idx == Some(1),
                                        cx.listener(|this, _, _, cx| {
                                            this.preferences.experimental_video.deband_enabled =
                                                false;
                                            this.preferences.experimental_video.shader_preset =
                                                ShaderPresetChoice::Off;
                                            this.apply_experimental_video_to_mpv();
                                            this.set_upscale_choice(UpscaleChoice::Spline36, cx);
                                        }),
                                    ))
                                    .child(segmented_item(
                                        "pp-quality-best",
                                        "Best",
                                        quality_preset_idx == Some(2),
                                        cx.listener(|this, _, _, cx| {
                                            this.preferences.experimental_video.deband_enabled =
                                                true;
                                            this.apply_experimental_video_to_mpv();
                                            this.set_upscale_choice(UpscaleChoice::EwaLanczos, cx);
                                        }),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .id("pp-advanced-toggle")
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(8.0))
                                    .rounded(px(10.0))
                                    .bg(rgba(t.glass_card_bg))
                                    .border_1()
                                    .border_color(rgba(t.glass_border))
                                    .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.picture_advanced_open =
                                                !this.picture_advanced_open;
                                            cx.notify();
                                        }),
                                    )
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::MEDIUM))
                                            .text_size(px(13.0))
                                            .text_color(rgb(t.text_primary))
                                            .child("Advanced mpv controls"),
                                    )
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::MEDIUM))
                                            .text_size(px(12.0))
                                            .text_color(rgb(t.text_secondary))
                                            .child(if advanced_open { "Hide" } else { "Show" }),
                                    ),
                            )
                            .when(advanced_open, |adv| {
                                adv.pt(px(6.0))
                                    .child(drilldown_row(
                                        "pp-row-tonemap",
                                        "HDR tone-mapping",
                                        prefs.tone_mapping.label(),
                                        cx.listener(|this, _, _, cx| {
                                            this.player_popover =
                                                Some(PlayerPopover::ToneMappingPicker);
                                            cx.notify();
                                        }),
                                    ))
                                    .child(drilldown_row(
                                        "pp-row-upscaler",
                                        "Upscaler",
                                        upscaler_label,
                                        cx.listener(|this, _, _, cx| {
                                            this.player_popover =
                                                Some(PlayerPopover::UpscalerPicker);
                                            cx.notify();
                                        }),
                                    ))
                                    .child(drilldown_row(
                                        "pp-row-shader",
                                        "Shader preset",
                                        shader_label,
                                        cx.listener(|this, _, _, cx| {
                                            this.player_popover =
                                                Some(PlayerPopover::ShaderPresetPicker);
                                            cx.notify();
                                        }),
                                    ))
                                    .child(toggle_switch_row(
                                        "osd-toggle-deband",
                                        "Deband",
                                        None,
                                        exp.deband_enabled,
                                        cx.listener(|this, _, _, cx| {
                                            this.preferences.experimental_video.deband_enabled =
                                                !this.preferences.experimental_video.deband_enabled;
                                            this.apply_experimental_video_to_mpv();
                                            let _ = this.preferences.save();
                                            cx.notify();
                                        }),
                                    ))
                                    .child(toggle_switch_row(
                                        "osd-toggle-dv",
                                        "Dolby Vision reshaping",
                                        None,
                                        exp.dolby_vision_reshape,
                                        cx.listener(|this, _, _, cx| {
                                            this.preferences
                                                .experimental_video
                                                .dolby_vision_reshape = !this
                                                .preferences
                                                .experimental_video
                                                .dolby_vision_reshape;
                                            this.apply_experimental_video_to_mpv();
                                            let _ = this.preferences.save();
                                            cx.notify();
                                        }),
                                    ))
                            }),
                    ),
            )
    }

    pub(super) fn render_tone_mapping_picker_popover(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = self.preferences.tone_mapping;
        let rows: Vec<_> = ToneMappingChoice::ALL
            .into_iter()
            .enumerate()
            .map(|(idx, choice)| {
                selectable_row(
                    ("pp-tonemap-item", idx),
                    choice.label(),
                    None,
                    current == choice,
                    cx.listener(move |this, _, _, cx| {
                        this.set_tone_mapping_choice(choice, cx);
                    }),
                )
                .flex_shrink_0()
            })
            .collect();

        render_picker_sheet(
            "player-tonemap-picker-sheet",
            "HDR tone-mapping",
            "tonemap-picker-back",
            "tonemap-picker-scroll",
            rows,
            cx,
        )
    }

    pub(super) fn render_upscaler_picker_popover(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = self.preferences.upscale_mode;
        let rows: Vec<_> = UpscaleChoice::ALL
            .into_iter()
            .enumerate()
            .map(|(idx, choice)| {
                selectable_row(
                    ("pp-upscaler-item", idx),
                    upscaler_display_label(choice),
                    None,
                    current == choice,
                    cx.listener(move |this, _, _, cx| {
                        this.set_upscale_choice(choice, cx);
                    }),
                )
                .flex_shrink_0()
            })
            .collect();

        render_picker_sheet(
            "player-upscaler-picker-sheet",
            "Upscaler",
            "upscaler-picker-back",
            "upscaler-picker-scroll",
            rows,
            cx,
        )
    }

    pub(super) fn render_shader_preset_picker_popover(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = self.preferences.experimental_video.shader_preset;
        let rows: Vec<_> = ShaderPresetChoice::ALL
            .into_iter()
            .enumerate()
            .map(|(idx, preset)| {
                selectable_row(
                    ("pp-shader-item", idx),
                    shader_display_label(preset),
                    None,
                    current == preset,
                    cx.listener(move |this, _, _, cx| {
                        this.preferences.experimental_video.shader_preset = preset;
                        this.apply_experimental_video_to_mpv();
                        let _ = this.preferences.save();
                        cx.notify();
                    }),
                )
                .flex_shrink_0()
            })
            .collect();

        render_picker_sheet(
            "player-shader-picker-sheet",
            "Shader preset",
            "shader-picker-back",
            "shader-picker-scroll",
            rows,
            cx,
        )
    }
}
