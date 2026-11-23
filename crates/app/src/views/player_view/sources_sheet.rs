use std::sync::Arc;

use gpui::{div, prelude::*, px, rgb, rgba, Context, FontWeight, MouseButton};

use crate::{
    icons::{svg_icon_weighted, IconName, IconWeight},
    shell::AppShell,
    state::{PlayerPopover, StreamResolutionChoice, STREAM_PAGE_SIZE},
    theme::{
        compact_audio_badge, compact_codec_badge, compact_hdr_badge, estimate_bitrate_mbps,
        mono_font, summarize_stream_for_humans, tabular_nums, ui_sans,
    },
};

use super::components::{player_hud_palette, player_sheet_card, segmented_item, segmented_trough};

impl AppShell {
    pub(super) fn render_switch_stream_drawer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = player_hud_palette();
        let Some((active_idx, list_arc)) = self
            .playback_session
            .as_ref()
            .map(|s| (s.active_index, Arc::clone(&s.candidates)))
        else {
            return div().into_any_element();
        };

        let has_debrid = self.has_debrid_access();
        let is_unfiltered = self.stream_filter.is_unfiltered(has_debrid);
        let filtered_indices: Vec<usize> = list_arc
            .iter()
            .enumerate()
            .filter_map(|(idx, cand)| {
                (idx == active_idx || self.stream_filter.matches_candidate(cand, has_debrid))
                    .then_some(idx)
            })
            .collect();
        let filtered_total = filtered_indices.len();
        let limit = self.player_source_limit.max(STREAM_PAGE_SIZE);
        let visible_count = filtered_total.min(limit);
        let remaining = filtered_total.saturating_sub(visible_count);
        let runtime_mins = (self.player_snapshot.duration > 60.0)
            .then(|| (self.player_snapshot.duration / 60.0).round() as u32);

        player_sheet_card("player-sources-sheet")
            .child(
                div()
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
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::SEMIBOLD))
                                            .text_size(px(15.0))
                                            .text_color(rgb(t.text_primary))
                                            .child("Sources"),
                                    )
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::NORMAL))
                                            .font_features(tabular_nums())
                                            .text_size(px(15.0))
                                            .text_color(rgb(t.text_muted))
                                            .child(format!("{}", list_arc.len())),
                                    ),
                            )
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::NORMAL))
                                    .text_size(px(12.0))
                                    .text_color(rgb(t.text_muted))
                                    .child("Switching keeps your place"),
                            ),
                    )
                    .child(
                        div()
                            .id("sources-sheet-close")
                            .cursor_pointer()
                            .flex_shrink_0()
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(999.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_player_popover(PlayerPopover::SwitchStream, cx);
                                }),
                            )
                            .child(svg_icon_weighted(
                                IconName::X,
                                IconWeight::Regular,
                                15.0,
                                rgb(t.text_secondary),
                            )),
                    ),
            )
            .child(
                div().px(px(16.0)).pt(px(12.0)).child(
                    segmented_trough()
                        .child(segmented_item(
                            "drawer-filter-all",
                            "All",
                            is_unfiltered,
                            cx.listener(|this, _, _, cx| {
                                this.stream_filter.reset();
                                this.player_source_limit = STREAM_PAGE_SIZE;
                                cx.notify();
                            }),
                        ))
                        .children(StreamResolutionChoice::ALL.into_iter().enumerate().map(
                            |(r_idx, res)| {
                                let active = !self.stream_filter.hdr_only
                                    && self.stream_filter.resolutions.len() == 1
                                    && self.stream_filter.resolutions.contains(&res);
                                segmented_item(
                                    ("drawer-filter-res", r_idx),
                                    res.label(),
                                    active,
                                    cx.listener(move |this, _, _, cx| {
                                        let is_sole_active = !this.stream_filter.hdr_only
                                            && this.stream_filter.resolutions.len() == 1
                                            && this.stream_filter.resolutions.contains(&res);
                                        if is_sole_active {
                                            this.stream_filter.reset();
                                        } else {
                                            this.stream_filter.resolutions.clear();
                                            this.stream_filter.resolutions.insert(res);
                                            this.stream_filter.hdr_only = false;
                                        }
                                        this.player_source_limit = STREAM_PAGE_SIZE;
                                        cx.notify();
                                    }),
                                )
                            },
                        ))
                        .child(segmented_item(
                            "drawer-filter-hdr",
                            "HDR",
                            self.stream_filter.hdr_only,
                            cx.listener(|this, _, _, cx| {
                                this.stream_filter.hdr_only = !this.stream_filter.hdr_only;
                                if this.stream_filter.hdr_only {
                                    this.stream_filter.resolutions.clear();
                                }
                                this.player_source_limit = STREAM_PAGE_SIZE;
                                cx.notify();
                            }),
                        )),
                ),
            )
            .child(
                div()
                    .id("switch-stream-drawer-scroll")
                    .w_full()
                    .max_h(px(340.0))
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .when(filtered_total == 0, |col| {
                                col.child(
                                    div()
                                        .px(px(14.0))
                                        .py(px(16.0))
                                        .font(ui_sans(FontWeight::NORMAL))
                                        .text_size(px(12.5))
                                        .text_color(rgb(t.text_muted))
                                        .child("No sources available"),
                                )
                            })
                            .children(filtered_indices.into_iter().take(limit).map(|orig_idx| {
                                let cand = &list_arc[orig_idx];
                                let is_current = orig_idx == active_idx;
                                let summary = summarize_stream_for_humans(&cand.parsed);
                                let line1 = [
                                    Some(summary.resolution),
                                    compact_hdr_badge(cand.parsed.hdr),
                                    compact_codec_badge(cand.parsed.codec),
                                    compact_audio_badge(cand.parsed.audio),
                                ]
                                .into_iter()
                                .flatten()
                                .collect::<Vec<_>>()
                                .join(" · ");

                                let clean_bitrate_opt =
                                    estimate_bitrate_mbps(cand.parsed.size_bytes, runtime_mins)
                                        .map(|mbps| format!("{mbps:.1} Mbps"));
                                let line2 = [
                                    summary.size_text,
                                    clean_bitrate_opt,
                                    Some(summary.availability_text),
                                ]
                                .into_iter()
                                .flatten()
                                .collect::<Vec<_>>()
                                .join(" · ");

                                let release_title = cand.parsed.display_title.clone();

                                div()
                                    .id(("drawer-stream-row", orig_idx))
                                    .flex_shrink_0()
                                    .cursor_pointer()
                                    .px(px(14.0))
                                    .py(px(11.0))
                                    .rounded(px(12.0))
                                    .bg(if is_current {
                                        rgba(t.accent_soft)
                                    } else {
                                        rgba(0x00000000)
                                    })
                                    .border_1()
                                    .border_color(if is_current {
                                        rgba(t.border_accent)
                                    } else {
                                        rgba(0x00000000)
                                    })
                                    .when(!is_current, |row| {
                                        row.hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(12.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.switch_active_player_stream(orig_idx, false, cx);
                                        }),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .flex()
                                            .flex_col()
                                            .gap(px(3.0))
                                            .child(
                                                div()
                                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                                    .text_size(px(13.5))
                                                    .text_color(rgb(t.text_primary))
                                                    .truncate()
                                                    .child(line1),
                                            )
                                            .child(
                                                div()
                                                    .font(ui_sans(FontWeight::NORMAL))
                                                    .font_features(tabular_nums())
                                                    .text_size(px(12.0))
                                                    .text_color(rgb(t.text_secondary))
                                                    .truncate()
                                                    .child(line2),
                                            )
                                            .when(!release_title.is_empty(), |col| {
                                                col.child(
                                                    div()
                                                        .font(mono_font())
                                                        .text_size(px(11.5))
                                                        .text_color(rgb(t.text_muted))
                                                        .truncate()
                                                        .child(release_title),
                                                )
                                            }),
                                    )
                                    .when(is_current, |row| {
                                        row.child(div().flex_shrink_0().child(svg_icon_weighted(
                                            IconName::Check,
                                            IconWeight::Fill,
                                            15.0,
                                            rgb(t.accent),
                                        )))
                                    })
                            }))
                            .when(remaining > 0, |drawer| {
                                let next_step = remaining.min(STREAM_PAGE_SIZE);
                                drawer.child(
                                    div()
                                        .id("btn-drawer-show-more")
                                        .flex_shrink_0()
                                        .cursor_pointer()
                                        .mt(px(4.0))
                                        .h(px(36.0))
                                        .px(px(14.0))
                                        .rounded(px(10.0))
                                        .bg(rgba(t.glass_trough_bg))
                                        .border_1()
                                        .border_color(rgba(t.glass_border))
                                        .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .font(ui_sans(FontWeight::MEDIUM))
                                        .text_size(px(12.5))
                                        .text_color(rgb(t.text_primary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.player_source_limit = (this
                                                    .player_source_limit
                                                    .max(STREAM_PAGE_SIZE)
                                                    + STREAM_PAGE_SIZE)
                                                    .min(filtered_total);
                                                cx.notify();
                                            }),
                                        )
                                        .child(format!("Show {next_step} more"))
                                        .child(
                                            div()
                                                .font(ui_sans(FontWeight::NORMAL))
                                                .font_features(tabular_nums())
                                                .text_size(px(12.0))
                                                .text_color(rgb(t.text_muted))
                                                .child(format!("{remaining} remaining")),
                                        ),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }
}
