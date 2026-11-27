use gpui::{div, prelude::*, px, rgb, rgba, Context, FontWeight, MouseButton, SharedString};
use gpui_mpv::{MediaTrack, TrackType};
use stremio_core_runtime::{
    human_language_name, is_forced_subtitle_track, is_sdh_subtitle_track,
};

use crate::{
    icons::{svg_icon_weighted, IconName, IconWeight},
    shell::AppShell,
    state::{NativeSubtitleMatchStatus, PlayerPopover, SubtitleBackgroundChoice},
    theme::ui_sans,
};

use super::components::{
    drilldown_row, parse_hex_rgb, player_hud_palette, player_sheet_card, section_eyebrow,
    segmented_item, segmented_trough, selectable_row, sheet_header_back, sheet_header_close,
    stepper_row, subtitle_color_short_name, subtitle_size_short_badge, toggle_switch_row,
    SUBTITLE_SWATCHES,
};

const SUBTITLE_SIZE_PRESETS: [(&str, u32); 4] = [("S", 38), ("M", 46), ("L", 56), ("XL", 66)];
const SUBTITLE_POSITION_PRESETS: [(u32, &str); 4] =
    [(100, "Lower"), (96, "Cinema"), (88, "Raised"), (80, "High")];

fn extract_audio_channels(tr: &MediaTrack) -> Option<&'static str> {
    for src in [tr.codec.as_deref(), tr.title.as_deref()]
        .into_iter()
        .flatten()
    {
        let lower = src.to_ascii_lowercase();
        if lower.contains("7.1") || lower.contains("8ch") {
            return Some("7.1");
        }
        if lower.contains("5.1") || lower.contains("6ch") {
            return Some("5.1");
        }
        if lower.contains("2.0") || lower.contains("stereo") || lower.contains("2ch") {
            return Some("2.0");
        }
    }
    None
}

fn normalize_audio_codec(codec: &str) -> String {
    let lower = codec.trim().to_ascii_lowercase();
    if lower.contains("eac3") || lower.contains("e-ac-3") || lower.contains("dd+") {
        "Dolby Digital+".to_owned()
    } else if lower.contains("ac3") || lower.contains("ac-3") {
        "Dolby Digital".to_owned()
    } else if lower.contains("truehd") {
        "TrueHD".to_owned()
    } else if lower.contains("dts") {
        "DTS".to_owned()
    } else if lower.contains("aac") {
        "AAC".to_owned()
    } else if lower.contains("opus") {
        "Opus".to_owned()
    } else if lower.contains("flac") {
        "FLAC".to_owned()
    } else {
        codec.trim().to_ascii_uppercase()
    }
}

fn format_audio_track_display(tr: &MediaTrack) -> (String, String) {
    let lang_opt = tr
        .lang
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.eq_ignore_ascii_case("und"));
    let primary = lang_opt
        .map(human_language_name)
        .or_else(|| {
            tr.title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| format!("Track {}", tr.id));

    let mut meta = Vec::with_capacity(2);
    if let Some(codec) = tr.codec.as_deref().filter(|c| !c.trim().is_empty()) {
        let mut label = normalize_audio_codec(codec);
        if let Some(ch) = extract_audio_channels(tr) {
            label.push(' ');
            label.push_str(ch);
        }
        meta.push(label);
    } else if let Some(ch) = extract_audio_channels(tr) {
        meta.push(ch.to_owned());
    }

    if let Some(raw_title) = tr.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let lower = raw_title.to_ascii_lowercase();
        let is_redundant = lower == primary.to_ascii_lowercase()
            || lang_opt.is_some_and(|l| lower == l.to_ascii_lowercase());
        if !is_redundant {
            if lower.contains("commentary") {
                meta.push("Commentary".to_owned());
            } else if lower.contains("atmos") {
                meta.push("Atmos".to_owned());
            } else if lower.contains("original") {
                meta.push("Original".to_owned());
            } else if meta.is_empty() {
                meta.push(raw_title.to_owned());
            }
        }
    }

    (primary, meta.join(" · "))
}

fn format_subtitle_track_display(tr: &MediaTrack) -> (String, String) {
    if let Some((left, right)) = tr
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .and_then(|title| title.split_once(" — "))
    {
        return (
            left.trim().to_owned(),
            right.trim().replace(" — ", " · "),
        );
    }

    let is_sdh = tr.hearing_impaired || is_sdh_subtitle_track(tr.title.as_deref());
    let is_forced = tr.forced || (!tr.external && is_forced_subtitle_track(tr.title.as_deref(), tr.forced));
    let mut primary = tr
        .lang
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.eq_ignore_ascii_case("und"))
        .map(human_language_name)
        .or_else(|| {
            tr.title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| format!("Subtitle {}", tr.id));
    if is_sdh && !primary.to_ascii_uppercase().contains("SDH") {
        primary.push_str(" SDH");
    }

    let mut meta = vec![if tr.external {
        "External".to_owned()
    } else {
        "Embedded".to_owned()
    }];
    if let Some(raw_title) = tr.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let lower = raw_title.to_ascii_lowercase();
        let primary_lower = primary.to_ascii_lowercase();
        let lang_lower = tr.lang.as_deref().map(|l| l.trim().to_ascii_lowercase());
        let lang_name_lower = tr
            .lang
            .as_deref()
            .map(human_language_name)
            .map(|n| n.to_ascii_lowercase());
        let is_redundant = lower == primary_lower
            || lang_lower.as_deref() == Some(lower.as_str())
            || lang_name_lower.as_deref() == Some(lower.as_str())
            || matches!(lower.as_str(), "sdh" | "forced" | "hi" | "cc" | "full")
            || lang_name_lower.as_deref().is_some_and(|ln| {
                lower == format!("{ln} sdh")
                    || lower == format!("{ln} (sdh)")
                    || lower == format!("{ln} forced")
                    || lower == format!("{ln} (forced)")
            });
        if !is_redundant {
            meta.push(raw_title.to_owned());
        }
    }
    if is_forced && !tr.external {
        meta.push("Forced".to_owned());
    }
    if let Some(codec) = tr.codec.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        let lower = codec.to_ascii_lowercase();
        let clean_codec = if lower.contains("subrip") || lower == "srt" {
            "SRT".to_owned()
        } else if lower.contains("ass") || lower.contains("ssa") {
            "ASS".to_owned()
        } else if lower.contains("pgs") {
            "PGS".to_owned()
        } else if lower.contains("vtt") {
            "VTT".to_owned()
        } else if lower.contains("dvd_subtitle") || lower.contains("vobsub") {
            "VobSub".to_owned()
        } else {
            codec.to_ascii_uppercase()
        };
        meta.push(clean_codec);
    }

    (primary, meta.join(" · "))
}

impl AppShell {
    pub(super) fn render_audio_and_subtitles_popover(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = player_hud_palette();
        let prefs = &self.preferences;
        let audio_tracks: Vec<MediaTrack> = self
            .player_snapshot
            .tracks
            .iter()
            .filter(|t| t.track_type == TrackType::Audio)
            .cloned()
            .collect();
        let sub_tracks: Vec<MediaTrack> = self
            .player_snapshot
            .tracks
            .iter()
            .filter(|t| t.track_type == TrackType::Sub)
            .cloned()
            .collect();
        let any_sub_selected = sub_tracks.iter().any(|t| t.selected);
        let (preferred_addon_subs, other_addon_subs) = self.collect_grouped_addon_subtitles();
        let other_lang_count = other_addon_subs.len();
        let show_other_langs = self.show_other_sub_langs;
        let remembered_for_show = self
            .playback_session
            .as_ref()
            .is_some_and(|s| s.applied_series_track_prefs);
        let active_size_idx = match prefs.subtitle_font_size {
            0..=41 => 0,
            42..=51 => 1,
            52..=61 => 2,
            _ => 3,
        };

        let delay_secs = self.player_snapshot.sub_delay;
        let delay_label = if delay_secs.abs() < 0.05 {
            "0.0 s".to_owned()
        } else {
            format!("{delay_secs:+.1} s")
        };

        let is_searching = matches!(
            self.native_sub_match.status,
            NativeSubtitleMatchStatus::Searching
        );
        let is_syncing = matches!(
            self.native_sub_match.status,
            NativeSubtitleMatchStatus::AudioSyncing
        );
        let search_busy = is_searching || is_syncing;
        let search_label = if is_searching {
            "Searching…"
        } else if is_syncing {
            "Syncing…"
        } else {
            "Search online"
        };

        let appearance_summary = format!(
            "{} · {}",
            subtitle_color_short_name(&prefs.subtitle_color_hex),
            subtitle_size_short_badge(prefs.subtitle_font_size)
        );

        player_sheet_card("player-audio-subs-popover")
            .child(sheet_header_close(
                "Audio & subtitles",
                "audio-subs-close",
                cx.listener(|this, _, _, cx| {
                    this.toggle_player_popover(PlayerPopover::AudioAndSubtitles, cx);
                }),
            ))
            .child(
                div()
                    .id("player-audio-subs-scroll")
                    .max_h(px(340.0))
                    .overflow_y_scroll()
                    .px(px(16.0))
                    .py(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    // Audio tracks
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(div().px(px(4.0)).pb(px(2.0)).child(section_eyebrow("Audio")))
                            .when(audio_tracks.is_empty(), |col| {
                                col.child(
                                    div()
                                        .px(px(14.0))
                                        .py(px(10.0))
                                        .text_size(px(12.5))
                                        .text_color(rgb(t.text_muted))
                                        .child("No audio tracks detected"),
                                )
                            })
                            .children(audio_tracks.into_iter().enumerate().map(|(idx, tr)| {
                                let tid = tr.id;
                                let (primary, secondary) = format_audio_track_display(&tr);
                                selectable_row(
                                    ("player-audio-track-item", idx),
                                    primary,
                                    (!secondary.is_empty()).then(|| SharedString::from(secondary)),
                                    tr.selected,
                                    cx.listener(move |this, _, _, cx| {
                                        this.user_select_audio_track(tid, cx);
                                    }),
                                )
                            })),
                    )
                    // Subtitle tracks (embedded + auto-listed addon subtitles grouped by language)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .px(px(4.0))
                                    .pb(px(2.0))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .child(section_eyebrow("Subtitles"))
                                    .when(remembered_for_show, |row| {
                                        row.child(
                                            div()
                                                .font(ui_sans(FontWeight::MEDIUM))
                                                .text_size(px(11.5))
                                                .text_color(rgb(t.accent))
                                                .child("Remembered for this show"),
                                        )
                                    }),
                            )
                            .child(selectable_row(
                                "player-sub-track-off",
                                "Off",
                                None,
                                !any_sub_selected,
                                cx.listener(|this, _, _, cx| {
                                    this.user_select_subtitle_track(None, cx);
                                }),
                            ))
                            .children(sub_tracks.into_iter().enumerate().map(|(idx, tr)| {
                                let tid = tr.id;
                                let (primary, secondary) = format_subtitle_track_display(&tr);
                                selectable_row(
                                    ("player-sub-track-item", idx),
                                    primary,
                                    (!secondary.is_empty()).then(|| SharedString::from(secondary)),
                                    tr.selected,
                                    cx.listener(move |this, _, _, cx| {
                                        this.user_select_subtitle_track(Some(tid), cx);
                                    }),
                                )
                            }))
                            .when(!preferred_addon_subs.is_empty(), |col| {
                                let mut flat_idx = 0usize;
                                let mut rows = Vec::new();
                                for (lang_name, items) in preferred_addon_subs {
                                    for (sub_url, sub_label, lang_code) in items {
                                        let idx = flat_idx;
                                        flat_idx += 1;
                                        let attached = self
                                            .playback_session
                                            .as_ref()
                                            .is_some_and(|s| s.attached_external_subs.contains(&sub_url));
                                        let title_for_mpv = format!("{lang_name} — {sub_label}");
                                        let url_clone = sub_url.clone();
                                        let lang_clone = lang_code.clone();
                                        rows.push(selectable_row(
                                            ("player-addon-pref-sub", idx),
                                            lang_name.clone(),
                                            Some(SharedString::from(format!("Online · {sub_label}"))),
                                            attached,
                                            cx.listener(move |this, _, _, cx| {
                                                this.attach_and_select_addon_subtitle(
                                                    url_clone.clone(),
                                                    title_for_mpv.clone(),
                                                    lang_clone.clone(),
                                                    cx,
                                                );
                                            }),
                                        ));
                                    }
                                }
                                col.children(rows)
                            })
                            .when(other_lang_count > 0, |col| {
                                col.child(
                                    div()
                                        .id("player-sub-other-langs-toggle")
                                        .mt(px(4.0))
                                        .px(px(12.0))
                                        .py(px(8.0))
                                        .rounded(px(10.0))
                                        .cursor_pointer()
                                        .bg(rgba(t.glass_card_bg))
                                        .border_1()
                                        .border_color(rgba(t.glass_border))
                                        .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.show_other_sub_langs =
                                                    !this.show_other_sub_langs;
                                                cx.notify();
                                            }),
                                        )
                                        .child(
                                            div()
                                                .font(ui_sans(FontWeight::MEDIUM))
                                                .text_size(px(13.0))
                                                .text_color(rgb(t.text_secondary))
                                                .child(format!(
                                                    "Other languages ({other_lang_count})"
                                                )),
                                        )
                                        .child(svg_icon_weighted(
                                            if show_other_langs {
                                                IconName::ChevronDown
                                            } else {
                                                IconName::ChevronRight
                                            },
                                            IconWeight::Regular,
                                            13.0,
                                            rgb(t.text_secondary),
                                        )),
                                )
                                .when(show_other_langs, |inner| {
                                    let mut flat_idx = 0usize;
                                    let mut lang_blocks = Vec::new();
                                    for (lang_name, items) in other_addon_subs {
                                        let mut sub_rows = Vec::new();
                                        for (sub_url, sub_label, lang_code) in items {
                                            let idx = flat_idx;
                                            flat_idx += 1;
                                            let attached = self
                                                .playback_session
                                                .as_ref()
                                                .is_some_and(|s| {
                                                    s.attached_external_subs.contains(&sub_url)
                                                });
                                            let title_for_mpv =
                                                format!("{lang_name} — {sub_label}");
                                            let url_clone = sub_url.clone();
                                            let lang_clone = lang_code.clone();
                                            sub_rows.push(selectable_row(
                                                ("player-addon-other-sub", idx),
                                                lang_name.clone(),
                                                Some(SharedString::from(format!(
                                                    "Online · {sub_label}"
                                                ))),
                                                attached,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.attach_and_select_addon_subtitle(
                                                        url_clone.clone(),
                                                        title_for_mpv.clone(),
                                                        lang_clone.clone(),
                                                        cx,
                                                    );
                                                }),
                                            ));
                                        }
                                        lang_blocks.extend(sub_rows);
                                    }
                                    inner.children(lang_blocks)
                                })
                            }),
                    )
                    // Action buttons: Search online | Load file...
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .id("sub-btn-search-online")
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .h(px(36.0))
                                    .rounded(px(10.0))
                                    .border_1()
                                    .border_color(rgba(t.glass_border))
                                    .bg(rgba(t.glass_card_bg))
                                    .when(!search_busy, |btn| {
                                        btn.cursor_pointer()
                                            .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .gap(px(7.0))
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(px(12.5))
                                    .text_color(rgb(t.text_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            if !search_busy {
                                                this.trigger_native_subtitle_auto_match(true, cx);
                                            }
                                        }),
                                    )
                                    .child(svg_icon_weighted(
                                        if search_busy {
                                            IconName::RefreshCw
                                        } else {
                                            IconName::Search
                                        },
                                        IconWeight::Regular,
                                        14.0,
                                        rgb(t.text_primary),
                                    ))
                                    .child(search_label),
                            )
                            .child(
                                div()
                                    .id("sub-btn-load-file")
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .h(px(36.0))
                                    .rounded(px(10.0))
                                    .border_1()
                                    .border_color(rgba(t.glass_border))
                                    .bg(rgba(t.glass_card_bg))
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .gap(px(7.0))
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(px(12.5))
                                    .text_color(rgb(t.text_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.prompt_and_load_local_subtitle(cx);
                                        }),
                                    )
                                    .child(svg_icon_weighted(
                                        IconName::FileText,
                                        IconWeight::Regular,
                                        14.0,
                                        rgb(t.text_primary),
                                    ))
                                    .child("Load file..."),
                            ),
                    ),
            )
            .child(
                div()
                    .px(px(18.0))
                    .py(px(12.0))
                    .border_t_1()
                    .border_color(rgba(t.glass_divider))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(px(13.5))
                                    .text_color(rgb(t.text_primary))
                                    .child("Subtitle size"),
                            )
                            .child(
                                segmented_trough().w(px(164.0)).children(
                                    SUBTITLE_SIZE_PRESETS.into_iter().enumerate().map(
                                        |(idx, (label, size_px))| {
                                            segmented_item(
                                                ("sub-quick-size", idx),
                                                label,
                                                active_size_idx == idx,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.set_subtitle_font_size(size_px, cx);
                                                }),
                                            )
                                        },
                                    ),
                                ),
                            ),
                    )
                    .child(stepper_row(
                        "Subtitle delay",
                        "sub-footer-delay-minus",
                        "sub-footer-delay-reset",
                        "sub-footer-delay-plus",
                        delay_label,
                        cx.listener(|this, _, _, cx| {
                            this.adjust_subtitle_delay(-0.1, cx);
                        }),
                        cx.listener(|this, _, _, cx| {
                            let cur = this.player_snapshot.sub_delay;
                            this.adjust_subtitle_delay(-cur, cx);
                        }),
                        cx.listener(|this, _, _, cx| {
                            this.adjust_subtitle_delay(0.1, cx);
                        }),
                    ))
                    .child(drilldown_row(
                        "sub-footer-style-btn",
                        "Appearance",
                        appearance_summary,
                        cx.listener(|this, _, _, cx| {
                            this.player_popover = Some(PlayerPopover::SubtitleAppearance);
                            cx.notify();
                        }),
                    )),
            )
    }

    pub(super) fn render_subtitle_appearance_popover(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = player_hud_palette();
        let prefs = &self.preferences;
        let active_size_idx = match prefs.subtitle_font_size {
            0..=41 => 0,
            42..=51 => 1,
            52..=61 => 2,
            _ => 3,
        };
        let active_bg = prefs.effective_subtitle_background();
        let active_color_name = subtitle_color_short_name(&prefs.subtitle_color_hex);

        let reset_btn = div()
            .id("sub-appearance-reset")
            .cursor_pointer()
            .px(px(6.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .hover(move |s| {
                s.bg(rgba(t.glass_row_hover_bg))
                    .text_color(rgb(t.text_primary))
            })
            .font(ui_sans(FontWeight::MEDIUM))
            .text_size(px(12.5))
            .text_color(rgb(t.text_secondary))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.reset_subtitle_style_prefs(cx);
                }),
            )
            .child("Reset")
            .into_any_element();

        player_sheet_card("player-subtitle-appearance-popover")
            .child(sheet_header_back(
                "Subtitle appearance",
                "sub-appearance-back",
                cx.listener(|this, _, _, cx| {
                    this.player_popover = Some(PlayerPopover::AudioAndSubtitles);
                    cx.notify();
                }),
                Some(reset_btn),
            ))
            .child(
                div()
                    .px(px(18.0))
                    .py(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    // Size
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(section_eyebrow("Size"))
                            .child(segmented_trough().children(
                                SUBTITLE_SIZE_PRESETS.into_iter().enumerate().map(
                                    |(idx, (label, size_px))| {
                                        segmented_item(
                                            ("sub-appearance-size", idx),
                                            label,
                                            idx == active_size_idx,
                                            cx.listener(move |this, _, _, cx| {
                                                this.set_subtitle_font_size(size_px, cx);
                                            }),
                                        )
                                    },
                                ),
                            )),
                    )
                    // Color
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(section_eyebrow("Color"))
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::NORMAL))
                                            .text_size(px(12.0))
                                            .text_color(rgb(t.text_secondary))
                                            .child(active_color_name),
                                    ),
                            )
                            .child(div().flex().items_center().gap(px(10.0)).children(
                                SUBTITLE_SWATCHES.iter().enumerate().map(|(idx, (hex, _))| {
                                    let selected =
                                        prefs.subtitle_color_hex.eq_ignore_ascii_case(hex);
                                    let color_u32 = parse_hex_rgb(hex);
                                    let hex_owned = (*hex).to_owned();
                                    div()
                                        .id(("studio-color-swatch", idx))
                                        .w(px(32.0))
                                        .h(px(32.0))
                                        .rounded_full()
                                        .cursor_pointer()
                                        .p(px(2.0))
                                        .border_2()
                                        .border_color(if selected {
                                            rgb(t.accent)
                                        } else {
                                            rgba(0x00000000)
                                        })
                                        .when(!selected, |d| {
                                            d.hover(move |s| {
                                                s.border_color(rgba(t.glass_border_strong))
                                            })
                                        })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.set_subtitle_color_hex(&hex_owned, cx);
                                            }),
                                        )
                                        .child(
                                            div()
                                                .w(px(24.0))
                                                .h(px(24.0))
                                                .rounded_full()
                                                .bg(rgb(color_u32)),
                                        )
                                }),
                            )),
                    )
                    // Background
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(section_eyebrow("Background"))
                            .child(segmented_trough().children(
                                SubtitleBackgroundChoice::ALL.into_iter().enumerate().map(
                                    |(idx, choice)| {
                                        segmented_item(
                                            ("sub-appearance-bg", idx),
                                            choice.label(),
                                            active_bg == choice,
                                            cx.listener(move |this, _, _, cx| {
                                                this.set_subtitle_background_choice(choice, cx);
                                            }),
                                        )
                                    },
                                ),
                            )),
                    )
                    // Position
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(section_eyebrow("Position"))
                            .child(segmented_trough().children(
                                SUBTITLE_POSITION_PRESETS.into_iter().enumerate().map(
                                    |(idx, (pos, label))| {
                                        segmented_item(
                                            ("studio-pos-preset", idx),
                                            label,
                                            prefs.subtitle_vertical_pos == pos,
                                            cx.listener(move |this, _, _, cx| {
                                                this.set_subtitle_vertical_pos(pos, cx);
                                            }),
                                        )
                                    },
                                ),
                            )),
                    )
                    // Fine-tune
                    .child(stepper_row(
                        "Fine-tune",
                        "studio-pos-up",
                        "studio-pos-reset",
                        "studio-pos-down",
                        format!("{}%", prefs.subtitle_vertical_pos),
                        cx.listener(|this, _, _, cx| {
                            this.step_subtitle_vertical_pos(-2, cx);
                        }),
                        cx.listener(|this, _, _, cx| {
                            this.set_subtitle_vertical_pos(96, cx);
                        }),
                        cx.listener(|this, _, _, cx| {
                            this.step_subtitle_vertical_pos(2, cx);
                        }),
                    )),
            )
            .child(
                div()
                    .px(px(18.0))
                    .py(px(12.0))
                    .border_t_1()
                    .border_color(rgba(t.glass_divider))
                    .child(toggle_switch_row(
                        "sub-appearance-ass-override",
                        "Apply to styled (ASS) subtitles",
                        Some("Overrides the file's own fonts and sizing"),
                        prefs.subtitle_ass_scale_override,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_subtitle_ass_scale_override(cx);
                        }),
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(
        id: i64,
        track_type: TrackType,
        title: Option<&str>,
        lang: Option<&str>,
        codec: Option<&str>,
    ) -> MediaTrack {
        MediaTrack {
            id,
            track_type,
            title: title.map(str::to_owned),
            lang: lang.map(str::to_owned),
            codec: codec.map(str::to_owned),
            selected: false,
            external: false,
            forced: false,
            hearing_impaired: false,
            default: false,
        }
    }

    #[test]
    fn audio_track_display_formats_dolby_digital_variants() {
        let ac3 = track(1, TrackType::Audio, Some("Stereo"), Some("eng"), Some("ac3"));
        assert_eq!(
            format_audio_track_display(&ac3),
            ("English".to_owned(), "Dolby Digital 2.0".to_owned())
        );

        let eac3 = track(2, TrackType::Audio, Some("5.1"), Some("eng"), Some("eac3"));
        assert_eq!(
            format_audio_track_display(&eac3),
            ("English".to_owned(), "Dolby Digital+ 5.1".to_owned())
        );
    }

    #[test]
    fn subtitle_track_display_formats_embedded_pgs_and_sdh() {
        let sdh = track(
            1,
            TrackType::Sub,
            Some("SDH"),
            Some("eng"),
            Some("hdmv_pgs_subtitle"),
        );
        assert_eq!(
            format_subtitle_track_display(&sdh),
            ("English SDH".to_owned(), "Embedded · PGS".to_owned())
        );

        let fre = track(
            2,
            TrackType::Sub,
            None,
            Some("fre"),
            Some("hdmv_pgs_subtitle"),
        );
        assert_eq!(
            format_subtitle_track_display(&fre),
            ("French".to_owned(), "Embedded · PGS".to_owned())
        );

        let non_english = track(
            3,
            TrackType::Sub,
            Some("Non-English"),
            Some("eng"),
            Some("subrip"),
        );
        assert_eq!(
            format_subtitle_track_display(&non_english),
            ("English".to_owned(), "Embedded · Non-English · Forced · SRT".to_owned())
        );

        let online_translated = track(
            4,
            TrackType::Sub,
            Some("English — OpenSubtitles — Foreign translated · Synced (+0.2s)"),
            Some("eng"),
            None,
        );
        assert_eq!(
            format_subtitle_track_display(&online_translated),
            (
                "English".to_owned(),
                "OpenSubtitles · Foreign translated · Synced (+0.2s)".to_owned()
            )
        );

        let mut hi_flag_only = track(5, TrackType::Sub, None, Some("eng"), Some("subrip"));
        hi_flag_only.hearing_impaired = true;
        assert_eq!(
            format_subtitle_track_display(&hi_flag_only),
            ("English SDH".to_owned(), "Embedded · SRT".to_owned())
        );
    }
}
