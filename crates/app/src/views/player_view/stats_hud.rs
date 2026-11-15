use gpui::{div, prelude::*, px, rgb, rgba, Context, FontWeight, MouseButton};

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{ToneMappingChoice, UpscaleChoice},
    theme::{
        glass_capsule_shadow, mono_font, ui_sans, TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY,
    },
};

use super::components::{SHEET_BG, SHEET_BORDER};

impl AppShell {
    pub(super) fn render_stats_for_nerds_card(
        &self,
        hud_visible: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snap = &self.player_snapshot;
        let session = self.playback_session.as_ref();

        let hwdec_str = if snap.hwdec_current.is_empty() || snap.hwdec_current == "no" {
            "Software".to_owned()
        } else {
            snap.hwdec_current.clone()
        };

        let short_codec = snap.short_video_codec();
        let video_str = if snap.video_width > 0 && snap.video_height > 0 {
            if short_codec.is_empty() {
                format!("{}×{}", snap.video_width, snap.video_height)
            } else {
                format!(
                    "{}×{} · {short_codec}",
                    snap.video_width, snap.video_height
                )
            }
        } else if snap.buffering || session.is_some_and(|s| !s.loaded_into_mpv) {
            "Buffering".to_owned()
        } else {
            "Idle".to_owned()
        };

        let (dl_bps, peers_opt, _is_torrent) = self.live_player_transport_telemetry();
        let download_str = format!("{:.2} MB/s", dl_bps / (1024.0 * 1024.0));

        let tm_short = match self.preferences.tone_mapping {
            ToneMappingChoice::Auto => "Auto",
            ToneMappingChoice::Spline => "Spline",
            ToneMappingChoice::Bt2446a => "BT.2446a",
            ToneMappingChoice::St2094_40 => "ST2094-40",
            ToneMappingChoice::Hable => "Hable",
            ToneMappingChoice::Mobius => "Mobius",
            ToneMappingChoice::Reinhard => "Reinhard",
            ToneMappingChoice::Clip => "Clip",
        };
        let up_short = match self.preferences.upscale_mode {
            UpscaleChoice::Bilinear => "Bilinear",
            UpscaleChoice::Spline36 => "Spline36",
            UpscaleChoice::Lanczos => "Lanczos3",
            UpscaleChoice::EwaLanczos => "EWA Sharp",
        };
        let pipeline_str = format!("{tm_short} · {up_short}");

        let demux_str = format!("{:.1} s", snap.demuxer_cache_duration.max(0.0));

        let used_gb = (self.engine.cache_usage_bytes() as f64) / (1024.0 * 1024.0 * 1024.0);
        let budget_gb = self.preferences.storage_mode.budget_bytes() / (1024 * 1024 * 1024);
        let cache_str = format!("{used_gb:.1} / {budget_gb} GB");

        let peers_str = peers_opt.map(|p| format!("{p} connected"));

        let stat_row = |label: &'static str, value: String| {
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(14.0))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(12.5))
                        .text_color(rgb(TEXT_MUTED))
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .font(mono_font())
                        .text_size(px(12.5))
                        .text_color(rgb(TEXT_PRIMARY))
                        .text_right()
                        .truncate()
                        .child(value),
                )
        };

        div()
            .id("player-stats-hud")
            .absolute()
            .top(if hud_visible { px(58.0) } else { px(20.0) })
            .right(px(24.0))
            .w(px(312.0))
            .overflow_hidden()
            .px(px(16.0))
            .py(px(14.0))
            .rounded(px(14.0))
            .bg(rgba(SHEET_BG))
            .border_1()
            .border_color(rgba(SHEET_BORDER))
            .shadow(glass_capsule_shadow())
            .flex()
            .flex_col()
            .gap(px(7.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .pb(px(3.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(px(13.0))
                            .text_color(rgb(TEXT_SECONDARY))
                            .child("Playback stats"),
                    )
                    .child(
                        div()
                            .id("btn-close-stats-for-nerds")
                            .cursor_pointer()
                            .w(px(22.0))
                            .h(px(22.0))
                            .rounded(px(6.0))
                            .hover(|s| s.bg(rgba(0xFFFFFF14)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_player_stats_for_nerds(cx);
                                }),
                            )
                            .child(svg_icon(IconName::X, 13.0, rgb(TEXT_MUTED))),
                    ),
            )
            .child(stat_row("Decoder", hwdec_str))
            .child(stat_row("Video", video_str))
            .child(stat_row("Download", download_str))
            .child(stat_row("Pipeline", pipeline_str))
            .child(stat_row("Demux buffer", demux_str))
            .child(stat_row("Cache", cache_str))
            .when_some(peers_str, |el, peers| el.child(stat_row("Peers", peers)))
    }
}
