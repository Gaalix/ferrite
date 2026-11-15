mod audio_subs_sheet;
mod components;
mod picture_playback_sheet;
mod sources_sheet;
mod stats_hud;

use std::{cell::Cell, rc::Rc};

use gpui::{
    canvas, deferred, div, img, linear_color_stop, linear_gradient, prelude::*, px, relative, rgb,
    rgba, AnimationExt, Bounds, Context, CursorStyle, FontWeight, MouseButton, MouseMoveEvent,
    Pixels, Point, ScrollEvent, SharedString, Window, WindowControlArea,
};
use gpui_mpv::mpv_viewport;

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{compute_scrub_preview_left, parse_season_episode_from_video_id, PlayerPopover},
    theme::{
        editorial_serif, format_timestamp_secs, glass_capsule_shadow, meta_divider, mono_font,
        movietracker_mark, obsidian_fade_in, semantic_status_pill, summarize_stream_for_humans,
        tabular_nums, transparent_cutout, ui_sans, ACCENT, ACCENT_HOVER, ACCENT_SOFT, BG_ELEVATED,
        BG_OBSIDIAN, BG_OSD_GLASS, BG_POPOVER_GLASS, BG_SURFACE, BG_SURFACE_3, BORDER_ACCENT,
        BORDER_STRONG, BORDER_SUBTLE, STATUS_WATCHED, TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY,
    },
};

const SCRUB_PREVIEW_THUMB_CARD_W: f32 = 208.0;
const SCRUB_PREVIEW_TIME_CARD_W: f32 = 88.0;

fn point_within_bounds(pt: Point<Pixels>, b: Bounds<Pixels>) -> bool {
    let px_x = pt.x / px(1.0);
    let px_y = pt.y / px(1.0);
    let bx = b.origin.x / px(1.0);
    let by = b.origin.y / px(1.0);
    let bw = b.size.width / px(1.0);
    let bh = b.size.height / px(1.0);
    px_x >= bx && px_x <= bx + bw && px_y >= by && px_y <= by + bh
}

fn scrub_metrics_from_point(pt: Point<Pixels>, b: Bounds<Pixels>) -> (f32, f32, f32) {
    let track_w = (b.size.width / px(1.0)).max(1.0);
    let raw_x = (pt.x - b.origin.x) / px(1.0);
    let clamped_x = raw_x.clamp(0.0, track_w);
    let frac = (clamped_x / track_w).clamp(0.0, 1.0);
    (frac, clamped_x, track_w)
}

fn sheet_icon_btn(
    id: &'static str,
    icon: IconName,
    icon_size: f32,
    active: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .cursor_pointer()
        .w(px(36.0))
        .h(px(36.0))
        .rounded(px(999.0))
        .bg(if active {
            rgba(0xFFFFFF1F)
        } else {
            rgba(0x00000000)
        })
        .hover(|s| {
            if active {
                s.bg(rgba(0xFFFFFF29))
            } else {
                s.bg(rgba(0xFFFFFF12))
            }
        })
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(svg_icon(icon, icon_size, rgb(0xFFFFFF)))
}

impl AppShell {
    pub fn render_player_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_fullscreen = window.is_fullscreen();
        let hud_visible = self.is_player_osd_visible();
        if let Some(ctrl) = &self.mpv {
            let _ = ctrl.sync_osd_controls_visible(hud_visible);
        }
        let player_csd_controls = AppShell::render_csd_window_controls(window, "player", false);
        let overlay =
            self.render_player_osd_overlay(is_fullscreen, hud_visible, player_csd_controls, cx);

        let viewport = if let Some(ctrl) = &self.mpv {
            mpv_viewport(ctrl.clone())
                .id("ferrite-mpv-viewport")
                .child(overlay)
                .into_any_element()
        } else {
            div()
                .id("ferrite-mpv-fallback")
                .size_full()
                .bg(rgb(BG_OBSIDIAN))
                .child(overlay)
                .into_any_element()
        };

        div()
            .id("ferrite-player-drop-root")
            .size_full()
            .drag_over::<gpui::ExternalPaths>(|style, _, _, _| {
                style
                    .border_2()
                    .border_color(rgb(ACCENT))
                    .bg(rgba(0x0B0B0E55))
            })
            .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, _window, cx| {
                this.handle_dropped_subtitle_paths(paths.paths(), cx);
            }))
            .child(viewport)
    }

    fn render_player_osd_overlay(
        &self,
        is_fullscreen: bool,
        hud_visible: bool,
        player_csd_controls: Option<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snap = &self.player_snapshot;
        let session = self.playback_session.as_ref();
        let video_ready = self.is_player_video_frame_ready();

        let title = session
            .map(|s| s.title.clone())
            .unwrap_or_else(|| "Now playing".to_owned());
        let meta_id_opt = session.map(|s| s.meta_id.as_str());

        let (intro_window, next_video, top_subtitle, model_poster_url, model_bg_url) =
            self.core.with_model(|m| {
                let intro = m
                    .player
                    .intro_outro
                    .as_ref()
                    .and_then(|io| io.intro.clone());
                let next_vid = m.player.next_video.clone();

                let matched_meta = meta_id_opt.and_then(|mid| {
                    m.meta_details
                        .meta_items
                        .iter()
                        .find_map(|res| {
                            res.content
                                .as_ref()
                                .and_then(|c| c.ready())
                                .filter(|item| item.preview.id == mid && !item.videos.is_empty())
                        })
                        .or_else(|| {
                            m.meta_details.meta_items.iter().find_map(|res| {
                                res.content
                                    .as_ref()
                                    .and_then(|c| c.ready())
                                    .filter(|item| item.preview.id == mid)
                            })
                        })
                });

                let computed_subtitle = session.and_then(|s| {
                    let video_obj =
                        matched_meta.and_then(|item| item.videos.iter().find(|v| v.id == s.video_id));
                    let ep_title = video_obj
                        .map(|v| v.title.trim())
                        .filter(|t| !t.is_empty());
                    let se = parse_season_episode_from_video_id(Some(&s.video_id)).or_else(|| {
                        video_obj
                            .and_then(|v| v.series_info.as_ref())
                            .map(|si| (si.season, si.episode))
                    });
                    if let Some((season, episode)) = se {
                        Some(match ep_title {
                            Some(ep_t) => format!("S{season} · E{episode} — {ep_t}"),
                            None => format!("S{season} · E{episode}"),
                        })
                    } else {
                        s.episode_label.clone()
                    }
                });

                let mut poster: Option<String> = None;
                let mut background: Option<String> = None;
                if !video_ready {
                    if let Some(meta_item) = matched_meta {
                        poster = meta_item.preview.poster.as_ref().map(|u| u.to_string());
                        background = meta_item.preview.background.as_ref().map(|u| u.to_string());
                    }
                    if poster.is_none() {
                        if let Some(mid) = meta_id_opt {
                            if let Some(lib_item) = m.ctx.library.items.get(mid) {
                                poster = lib_item.poster.as_ref().map(|u| u.to_string());
                            }
                        }
                    }
                }
                (intro, next_vid, computed_subtitle, poster, background)
            });

        let cur_ms = (snap.time_pos.max(0.0) * 1000.0) as u64;
        let active_skip_to_secs = intro_window.as_ref().and_then(|intro| {
            if video_ready
                && intro.to > intro.from + 2000
                && cur_ms >= intro.from
                && cur_ms + 1000 < intro.to
            {
                Some((intro.to as f64) / 1000.0)
            } else {
                None
            }
        });

        let show_next_ep_prompt = video_ready
            && next_video.is_some()
            && snap.duration > 60.0
            && ((snap.duration - snap.time_pos) <= 48.0 || snap.eof_reached);
        let is_prewarmed = self.next_episode_prewarmed;
        let stats_open = self.player_stats_for_nerds;

        let intro_bounds_secs = intro_window.as_ref().and_then(|intro| {
            if intro.to > intro.from {
                Some(((intro.from as f64) / 1000.0, (intro.to as f64) / 1000.0))
            } else {
                None
            }
        });

        let scrub_bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        let scrub_bounds_hit = Rc::clone(&scrub_bounds);
        let vol_slider_bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        let vol_bounds_hit = Rc::clone(&vol_slider_bounds);

        div()
            .id("player-osd-hit-layer")
            .size_full()
            .when(video_ready && !hud_visible, |el| el.cursor(CursorStyle::None))
            .bg(if video_ready {
                transparent_cutout()
            } else {
                rgb(BG_OBSIDIAN)
            })
            .relative()
            .flex()
            .flex_col()
            .justify_between()
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                this.bump_player_osd_activity(cx);
                if this.is_dragging_volume {
                    if let Some(b) = vol_bounds_hit.get() {
                        let (frac, _, _) = scrub_metrics_from_point(ev.position, b);
                        this.set_player_volume_user(f64::from(frac) * 100.0, true, cx);
                    }
                    return;
                }
                if this.is_scrubbing {
                    if let Some(b) = scrub_bounds_hit.get() {
                        let (frac, local_x, track_w) = scrub_metrics_from_point(ev.position, b);
                        this.on_scrub_bar_hover(frac, Some((local_x, track_w)), cx);
                        let dur = this.player_snapshot.duration.max(0.0);
                        if dur > 0.0 {
                            this.seek_player_absolute(f64::from(frac) * dur, cx);
                        }
                    }
                    return;
                }
                let inside_scrub = scrub_bounds_hit
                    .get()
                    .is_some_and(|b| point_within_bounds(ev.position, b));
                if !inside_scrub {
                    this.clear_scrub_hover(cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.end_player_drag_interactions(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.end_player_drag_interactions(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.bump_player_osd_activity(cx);
                    if this.player_popover.is_some() {
                        this.player_popover = None;
                        cx.notify();
                    } else if video_ready {
                        if let Some(mpv) = &this.mpv {
                            let _ = mpv.toggle_pause();
                        }
                    }
                }),
            )
            .when(!video_ready, |el| {
                el.child(self.render_player_loading_backdrop(
                    &title,
                    top_subtitle.as_deref(),
                    meta_id_opt,
                    model_poster_url.as_deref(),
                    model_bg_url.as_deref(),
                    cx,
                ))
            })
            .when(video_ready && self.hold_2x_active, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(28.0))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .px(px(14.0))
                                .py(px(6.0))
                                .rounded(px(999.0))
                                .bg(rgba(BG_OSD_GLASS))
                                .border_1()
                                .border_color(rgba(BORDER_ACCENT))
                                .shadow(glass_capsule_shadow())
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .font(mono_font())
                                .text_xs()
                                .text_color(rgb(TEXT_PRIMARY))
                                .child(svg_icon(IconName::SkipForward, 13.0, rgb(ACCENT)))
                                .child("2x speed"),
                        ),
                )
            })
            .when(stats_open, |el| {
                el.child(self.render_stats_for_nerds_card(hud_visible, cx))
            })
            .when(
                self.player_popover.is_none()
                    && (active_skip_to_secs.is_some() || show_next_ep_prompt),
                |el| {
                    let nv_clone = next_video.clone();
                    el.child(
                        div()
                            .absolute()
                            .right(px(28.0))
                            .bottom(if hud_visible { px(96.0) } else { px(32.0) })
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .when_some(active_skip_to_secs, |s_el, target_secs| {
                                s_el.child(
                                    div()
                                        .id("btn-skip-intro")
                                        .cursor_pointer()
                                        .h(px(40.0))
                                        .px(px(18.0))
                                        .rounded(px(999.0))
                                        .bg(rgb(ACCENT))
                                        .hover(|s| s.bg(rgb(ACCENT_HOVER)))
                                        .shadow(glass_capsule_shadow())
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .font(ui_sans(FontWeight::MEDIUM))
                                        .text_size(px(13.5))
                                        .text_color(rgb(TEXT_PRIMARY))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.seek_player_absolute(target_secs, cx);
                                            }),
                                        )
                                        .child(svg_icon(
                                            IconName::SkipForward,
                                            14.0,
                                            rgb(TEXT_PRIMARY),
                                        ))
                                        .child("Skip intro"),
                                )
                            })
                            .when(show_next_ep_prompt, |n_el| {
                                let nv_inner = nv_clone.clone();
                                let ep_badge = nv_inner.as_ref().and_then(|v| {
                                    v.series_info
                                        .as_ref()
                                        .map(|s| format!("S{:02}E{:02} · {}", s.season, s.episode, v.title))
                                });
                                let match_badge = self
                                    .prewarmed_next
                                    .as_ref()
                                    .map(|r| r.matched_summary_badge.trim().to_owned())
                                    .filter(|s| !s.is_empty());
                                let prebuffered =
                                    is_prewarmed || self.prewarmed_next_direct_url.is_some();
                                let auto_play = self.preferences.auto_play_next_episode;

                                n_el.child(
                                    div()
                                        .id("btn-next-episode")
                                        .cursor_pointer()
                                        .min_w(px(260.0))
                                        .max_w(px(360.0))
                                        .px(px(16.0))
                                        .py(px(10.0))
                                        .rounded(px(12.0))
                                        .bg(rgba(BG_POPOVER_GLASS))
                                        .border_1()
                                        .border_color(if prebuffered {
                                            rgba(0x4ade8066)
                                        } else {
                                            rgba(BORDER_STRONG)
                                        })
                                        .hover(|s| s.bg(rgb(BG_ELEVATED)))
                                        .shadow(glass_capsule_shadow())
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                if let Some(vid) = &nv_inner {
                                                    this.advance_to_next_episode(vid.clone(), cx);
                                                }
                                            }),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .gap(px(10.0))
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap(px(8.0))
                                                        .min_w(px(0.0))
                                                        .child(svg_icon(
                                                            IconName::StepForward,
                                                            14.0,
                                                            rgb(if prebuffered {
                                                                STATUS_WATCHED
                                                            } else {
                                                                ACCENT
                                                            }),
                                                        ))
                                                        .child(
                                                            div()
                                                                .font(ui_sans(FontWeight::SEMIBOLD))
                                                                .text_size(px(13.0))
                                                                .text_color(rgb(TEXT_PRIMARY))
                                                                .truncate()
                                                                .child(ep_badge.unwrap_or_else(
                                                                    || "Next episode".to_owned(),
                                                                )),
                                                        ),
                                                )
                                                .when(prebuffered, |r| {
                                                    r.child(semantic_status_pill(
                                                        IconName::Check,
                                                        "Pre-buffered",
                                                        STATUS_WATCHED,
                                                        0x4ade804d,
                                                        0x4ade801c,
                                                    ))
                                                }),
                                        )
                                        .when(match_badge.is_some() || auto_play, |card| {
                                            card.child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap(px(8.0))
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(TEXT_SECONDARY))
                                                    .when_some(match_badge, |row, b| {
                                                        row.child(
                                                            div().truncate().child(format!(
                                                                "Auto-matched · {b}"
                                                            )),
                                                        )
                                                    })
                                                    .when(auto_play, |row| {
                                                        row.child(
                                                            div()
                                                                .flex_shrink_0()
                                                                .text_color(rgb(TEXT_MUTED))
                                                                .child("Auto-plays at end"),
                                                        )
                                                    }),
                                            )
                                        }),
                                )
                            }),
                    )
                },
            )
            .child(if hud_visible {
                div()
                    .id("player-top-titlebar")
                    .when(!is_fullscreen, |el| {
                        el.window_control_area(WindowControlArea::Drag)
                    })
                    .w_full()
                    .h(px(108.0))
                    .bg(linear_gradient(
                        180.0,
                        linear_color_stop(rgba(0x000000B0), 0.0),
                        linear_color_stop(rgba(0x00000000), 1.0),
                    ))
                    .px(px(28.0))
                    .pt(px(18.0))
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(16.0))
                    .on_mouse_move(cx.listener(|this, _, _, cx| {
                        this.clear_scrub_hover(cx);
                    }))
                    .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                        cx.stop_propagation();
                        if !is_fullscreen {
                            if event.click_count >= 2 {
                                window.toggle_fullscreen();
                            } else {
                                window.start_window_move();
                            }
                        }
                    })
                    .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                        cx.stop_propagation();
                        if !is_fullscreen {
                            window.show_window_menu(event.position);
                        }
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .id("player-btn-back")
                                    .flex_shrink_0()
                                    .cursor_pointer()
                                    .w(px(34.0))
                                    .h(px(34.0))
                                    .rounded(px(999.0))
                                    .hover(|s| s.bg(rgba(0xFFFFFF14)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.exit_player(cx);
                                            if window.is_fullscreen() {
                                                window.toggle_fullscreen();
                                            }
                                            this.exit_fullscreen_on_next_render = false;
                                        }),
                                    )
                                    .child(svg_icon(
                                        IconName::ChevronLeft,
                                        18.0,
                                        rgb(TEXT_PRIMARY),
                                    )),
                            )
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::SEMIBOLD))
                                            .text_size(px(15.5))
                                            .line_height(px(20.0))
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .truncate()
                                            .child(title),
                                    )
                                    .when_some(top_subtitle, |col, sub| {
                                        col.child(
                                            div()
                                                .font(ui_sans(FontWeight::NORMAL))
                                                .text_size(px(12.5))
                                                .line_height(px(16.0))
                                                .text_color(rgb(TEXT_SECONDARY))
                                                .truncate()
                                                .child(sub),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .id("player-btn-stats")
                                    .cursor_pointer()
                                    .w(px(34.0))
                                    .h(px(34.0))
                                    .rounded(px(999.0))
                                    .bg(if stats_open {
                                        rgba(0xFFFFFF1C)
                                    } else {
                                        rgba(0x00000000)
                                    })
                                    .hover(|s| {
                                        if stats_open {
                                            s.bg(rgba(0xFFFFFF24))
                                        } else {
                                            s.bg(rgba(0xFFFFFF12))
                                        }
                                    })
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
                                    .child(svg_icon(IconName::Info, 18.0, rgb(TEXT_PRIMARY))),
                            )
                            .when_some(player_csd_controls, |row, csd| {
                                row.child(div().w(px(1.0)).h(px(16.0)).bg(rgba(0xFFFFFF22)))
                                    .child(csd)
                            }),
                    )
                    .with_animation("player-top-osd-fade", obsidian_fade_in(), |el, delta| {
                        el.opacity(delta)
                    })
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            .child(if hud_visible && video_ready {
                self.render_player_bottom_deck(
                    is_fullscreen,
                    intro_bounds_secs,
                    &scrub_bounds,
                    vol_slider_bounds,
                    cx,
                )
                .into_any_element()
            } else {
                div().into_any_element()
            })
            .when_some(self.player_popover, |el, pop| {
                el.child(
                    div()
                        .absolute()
                        .right(px(28.0))
                        .bottom(px(86.0))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(self.render_active_player_popover(pop, cx)),
                )
            })
    }

    fn render_player_loading_backdrop(
        &self,
        title: &str,
        ep_label: Option<&str>,
        meta_id: Option<&str>,
        model_poster_url: Option<&str>,
        model_bg_url: Option<&str>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let session = self.playback_session.as_ref();
        let loaded_into_mpv = session.is_some_and(|s| s.loaded_into_mpv);
        let candidate_count = session.map(|s| s.candidates.len()).unwrap_or(0);
        let active_cand = session.and_then(|s| s.active_candidate());
        let active_summary = active_cand.map(|c| summarize_stream_for_humans(&c.parsed));
        let (dl_bps, peers_opt, is_torrent) = self.live_player_transport_telemetry();

        let fallback_poster = meta_id
            .filter(|id| id.starts_with("tt"))
            .map(|id| format!("https://images.metahub.space/poster/medium/{id}/img"));
        let fallback_bg = meta_id
            .filter(|id| id.starts_with("tt"))
            .map(|id| format!("https://images.metahub.space/background/medium/{id}/img"));

        let effective_poster_url = model_poster_url.or(fallback_poster.as_deref());
        let effective_bg_url = model_bg_url
            .or(fallback_bg.as_deref())
            .or(effective_poster_url);

        let ambient_img = effective_bg_url.and_then(|u| self.poster_cache.resolve_ambient_bleed(u));
        let poster_img =
            effective_poster_url.and_then(|u| self.poster_cache.resolve_poster_image(u));

        let debrid_resolving = session.is_some_and(|s| s.debrid_resolving);
        let debrid_error = session.and_then(|s| s.debrid_error.clone());
        let status_label: SharedString = if let Some(err) = debrid_error {
            err.into()
        } else if debrid_resolving {
            "Resolving cached stream via local Debrid...".into()
        } else if !loaded_into_mpv {
            if is_torrent {
                "Connecting to torrent swarm...".into()
            } else {
                "Resolving stream source...".into()
            }
        } else {
            "Buffering video stream...".into()
        };

        let dl_mbps = dl_bps / (1024.0 * 1024.0);
        let show_dl_telemetry = is_torrent || dl_bps > 1024.0;

        div()
            .id("player-loading-showcase")
            .absolute()
            .inset_0()
            .bg(rgb(BG_OBSIDIAN))
            .overflow_hidden()
            .when_some(ambient_img, |el, amb| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .opacity(0.32)
                        .child(img(amb).size_full().object_fit(gpui::ObjectFit::Cover)),
                )
            })
            .child(
                div().absolute().inset_0().bg(linear_gradient(
                    180.0,
                    linear_color_stop(rgba(0x0b0b0db5), 0.0),
                    linear_color_stop(rgba(0x0b0b0df7), 1.0),
                )),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .px(px(32.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(18.0))
                    .child(
                        div()
                            .w(px(148.0))
                            .h(px(222.0))
                            .rounded(px(10.0))
                            .overflow_hidden()
                            .bg(rgb(BG_SURFACE))
                            .border_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .shadow(glass_capsule_shadow())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if let Some(p_img) = poster_img {
                                img(p_img)
                                    .size_full()
                                    .object_fit(gpui::ObjectFit::Cover)
                                    .into_any_element()
                            } else {
                                movietracker_mark(44.0).into_any_element()
                            }),
                    )
                    .child(
                        div()
                            .max_w(px(680.0))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .font(editorial_serif())
                                    .text_size(px(34.0))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .text_center()
                                    .child(title.to_owned()),
                            )
                            .when_some(ep_label, |col, ep| {
                                col.child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(ACCENT))
                                        .child(ep.to_owned()),
                                )
                            }),
                    )
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(8.0))
                            .rounded(px(999.0))
                            .bg(rgba(BG_OSD_GLASS))
                            .border_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .shadow(glass_capsule_shadow())
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_center()
                            .gap(px(10.0))
                            .child(svg_icon(IconName::RefreshCw, 13.0, rgb(ACCENT)))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(px(13.0))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .child(status_label),
                            )
                            .when(show_dl_telemetry, |pill| {
                                pill.child(meta_divider()).child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_SECONDARY))
                                        .child(format!("{dl_mbps:.2} MB/s")),
                                )
                            })
                            .when_some(peers_opt, |pill, peers| {
                                pill.child(meta_divider()).child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_SECONDARY))
                                        .child(format!("{peers} peers")),
                                )
                            })
                            .when_some(active_summary, |pill, sum| {
                                pill.child(meta_divider()).child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(match sum.addon_label {
                                            Some(addon) => format!("{} / {addon}", sum.resolution),
                                            None => sum.resolution.to_owned(),
                                        }),
                                )
                            }),
                    )
                    .when(candidate_count > 1, |col| {
                        let drawer_open = self.player_popover == Some(PlayerPopover::SwitchStream);
                        col.child(
                            div()
                                .id("loading-btn-switch-source")
                                .cursor_pointer()
                                .h(px(34.0))
                                .px(px(14.0))
                                .rounded(px(999.0))
                                .bg(if drawer_open {
                                    rgba(ACCENT_SOFT)
                                } else {
                                    rgb(BG_ELEVATED)
                                })
                                .border_1()
                                .border_color(if drawer_open {
                                    rgba(BORDER_ACCENT)
                                } else {
                                    rgba(BORDER_SUBTLE)
                                })
                                .hover(|s| {
                                    s.bg(rgb(BG_SURFACE_3)).border_color(rgba(BORDER_STRONG))
                                })
                                .flex()
                                .items_center()
                                .gap(px(7.0))
                                .font(ui_sans(FontWeight::MEDIUM))
                                .text_xs()
                                .text_color(if drawer_open {
                                    rgb(ACCENT)
                                } else {
                                    rgb(TEXT_SECONDARY)
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_player_popover(PlayerPopover::SwitchStream, cx);
                                    }),
                                )
                                .child(svg_icon(
                                    IconName::Layers,
                                    13.0,
                                    rgb(if drawer_open { ACCENT } else { TEXT_SECONDARY }),
                                ))
                                .child(format!("Switch source ({candidate_count})")),
                        )
                    }),
            )
    }

    fn render_player_bottom_deck(
        &self,
        is_fullscreen: bool,
        intro_bounds_secs: Option<(f64, f64)>,
        scrub_bounds: &Rc<Cell<Option<Bounds<Pixels>>>>,
        vol_slider_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snap = &self.player_snapshot;
        let duration = snap.duration.max(0.0);
        let pos = snap.time_pos.clamp(0.0, duration.max(0.001));
        let play_frac = if duration > 0.0 {
            (pos / duration).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let cache_end = (pos + snap.demuxer_cache_duration.max(0.0)).min(duration);
        let cache_frac = if duration > 0.0 {
            (cache_end / duration).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };

        let intro_frac_range = intro_bounds_secs.and_then(|(from_s, to_s)| {
            if duration > 1.0 && to_s > from_s {
                let start = (from_s / duration).clamp(0.0, 1.0) as f32;
                let width =
                    ((to_s - from_s) / duration).clamp(0.005, 1.0 - f64::from(start)) as f32;
                Some((start, width))
            } else {
                None
            }
        });

        let hover_preview_secs = self
            .scrub_hover_fraction
            .filter(|_| duration > 0.0)
            .map(|f| (f as f64) * duration);
        let hover_metrics = self.scrub_hover_metrics;
        let hover_thumb = self.hover_thumbnail_image.clone();

        let muted = snap.muted;
        let vol_frac = if muted {
            0.0
        } else {
            (snap.volume.clamp(0.0, 100.0) / 100.0) as f32
        };

        let has_session = self.playback_session.is_some();
        let popover = self.player_popover;
        let cc_active = popover.is_some_and(|p| p.is_audio_subs_family());
        let picture_active = popover.is_some_and(|p| p.is_picture_playback_family());
        let sources_active = popover == Some(PlayerPopover::SwitchStream);

        let scrub_bounds_prepaint = Rc::clone(scrub_bounds);
        let scrub_bounds_move = Rc::clone(scrub_bounds);
        let scrub_bounds_click = Rc::clone(scrub_bounds);

        let vol_bounds_prepaint = Rc::clone(&vol_slider_bounds);
        let vol_bounds_down = Rc::clone(&vol_slider_bounds);
        let vol_bounds_move = vol_slider_bounds;

        div()
            .w_full()
            .pt(px(44.0))
            .pb(px(18.0))
            .px(px(28.0))
            .bg(linear_gradient(
                180.0,
                linear_color_stop(rgba(0x00000000), 0.0),
                linear_color_stop(rgba(0x000000CC), 1.0),
            ))
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .id("player-scrub-track")
                    .cursor_pointer()
                    .w_full()
                    .h(px(18.0))
                    .relative()
                    .flex()
                    .items_center()
                    .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                        if let Some(b) = scrub_bounds_move.get() {
                            let (frac, local_x, track_w) = scrub_metrics_from_point(ev.position, b);
                            this.on_scrub_bar_hover(frac, Some((local_x, track_w)), cx);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.is_scrubbing = true;
                            if let Some(b) = scrub_bounds_click.get() {
                                let (frac, local_x, track_w) =
                                    scrub_metrics_from_point(ev.position, b);
                                this.on_scrub_bar_hover(frac, Some((local_x, track_w)), cx);
                                let dur = this.player_snapshot.duration.max(0.0);
                                if dur > 0.0 {
                                    this.seek_player_absolute((frac as f64) * dur, cx);
                                }
                            }
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.end_player_drag_interactions(cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.end_player_drag_interactions(cx);
                        }),
                    )
                    .child(
                        canvas(
                            move |bounds, _window, _cx| {
                                scrub_bounds_prepaint.set(Some(bounds));
                            },
                            |_bounds, (), _window, _cx| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(4.0))
                            .rounded(px(999.0))
                            .bg(rgba(0xFFFFFF26))
                            .relative()
                            .overflow_hidden()
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left_0()
                                    .w(relative(cache_frac))
                                    .bg(rgba(0xFFFFFF47)),
                            )
                            .when_some(intro_frac_range, |bar, (intro_start, intro_w)| {
                                bar.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .left(relative(intro_start))
                                        .w(relative(intro_w))
                                        .bg(rgba(0xfbbf2477)),
                                )
                            })
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left_0()
                                    .w(relative(play_frac))
                                    .bg(rgb(ACCENT)),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(relative(play_frac))
                            .ml(px(-6.5))
                            .w(px(13.0))
                            .h(px(13.0))
                            .rounded(px(999.0))
                            .bg(rgb(0xFFFFFF)),
                    )
                    .when_some(
                        hover_preview_secs.zip(hover_metrics),
                        |track_el, (preview_secs, (cursor_x, track_w))| {
                            let has_thumb = hover_thumb.is_some();
                            let card_w = if has_thumb {
                                SCRUB_PREVIEW_THUMB_CARD_W
                            } else {
                                SCRUB_PREVIEW_TIME_CARD_W
                            };
                            let preview_left =
                                compute_scrub_preview_left(cursor_x, track_w, card_w);

                            track_el
                                .child(
                                    div()
                                        .absolute()
                                        .left(px((cursor_x - 1.0).max(0.0)))
                                        .top(px(1.0))
                                        .w(px(2.0))
                                        .h(px(16.0))
                                        .rounded(px(1.0))
                                        .bg(rgb(TEXT_PRIMARY)),
                                )
                                .child(
                                    deferred(
                                        div()
                                            .absolute()
                                            .bottom(px(26.0))
                                            .left(px(preview_left))
                                            .w(px(card_w))
                                            .p(px(8.0))
                                            .rounded(px(8.0))
                                            .bg(rgba(BG_POPOVER_GLASS))
                                            .border_1()
                                            .border_color(rgba(BORDER_SUBTLE))
                                            .shadow(glass_capsule_shadow())
                                            .flex()
                                            .flex_col()
                                            .items_center()
                                            .gap(px(6.0))
                                            .when_some(hover_thumb, |t_el, thumb| {
                                                t_el.child(
                                                    div()
                                                        .w(px(192.0))
                                                        .h(px(108.0))
                                                        .rounded(px(4.0))
                                                        .bg(rgb(BG_SURFACE))
                                                        .overflow_hidden()
                                                        .child(
                                                            img(thumb)
                                                                .size_full()
                                                                .object_fit(gpui::ObjectFit::Cover),
                                                        ),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .font(mono_font())
                                                    .text_xs()
                                                    .text_color(rgb(TEXT_PRIMARY))
                                                    .child(format_timestamp_secs(preview_secs)),
                                            ),
                                    )
                                    .with_priority(10),
                                )
                        },
                    ),
            )
            .child(
                div()
                    .w_full()
                    .pt(px(8.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .on_mouse_move(cx.listener(|this, _, _, cx| {
                        this.clear_scrub_hover(cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .id("osd-play-toggle")
                                    .cursor_pointer()
                                    .w(px(36.0))
                                    .h(px(36.0))
                                    .rounded(px(999.0))
                                    .hover(|s| s.bg(rgba(0xFFFFFF14)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.bump_player_osd_activity(cx);
                                            if let Some(mpv) = &this.mpv {
                                                let _ = mpv.toggle_pause();
                                            }
                                        }),
                                    )
                                    .child(svg_icon(
                                        if snap.paused {
                                            IconName::Play
                                        } else {
                                            IconName::Pause
                                        },
                                        18.0,
                                        rgb(0xFFFFFF),
                                    )),
                            )
                            .child(
                                div()
                                    .id("osd-seek-b10")
                                    .cursor_pointer()
                                    .w(px(36.0))
                                    .h(px(36.0))
                                    .rounded(px(999.0))
                                    .hover(|s| s.bg(rgba(0xFFFFFF14)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.seek_player_relative(-10.0, cx);
                                        }),
                                    )
                                    .child(svg_icon(IconName::Replay10, 20.0, rgb(0xFFFFFF))),
                            )
                            .child(
                                div()
                                    .id("osd-seek-f10")
                                    .cursor_pointer()
                                    .w(px(36.0))
                                    .h(px(36.0))
                                    .rounded(px(999.0))
                                    .hover(|s| s.bg(rgba(0xFFFFFF14)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.seek_player_relative(10.0, cx);
                                        }),
                                    )
                                    .child(svg_icon(IconName::Forward10, 20.0, rgb(0xFFFFFF))),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .id("osd-volume-btn")
                                            .cursor_pointer()
                                            .w(px(34.0))
                                            .h(px(34.0))
                                            .rounded(px(999.0))
                                            .hover(|s| s.bg(rgba(0xFFFFFF14)))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.toggle_player_mute_user(cx);
                                                }),
                                            )
                                            .on_scroll(cx.listener(
                                                |this, ev: &ScrollEvent, _, cx| {
                                                    cx.stop_propagation();
                                                    this.adjust_player_volume_by_wheel(ev.delta, cx);
                                                },
                                            ))
                                            .child(svg_icon(
                                                if muted || snap.volume <= 0.5 {
                                                    IconName::VolumeX
                                                } else {
                                                    IconName::Volume2
                                                },
                                                19.0,
                                                rgb(0xFFFFFF),
                                            )),
                                    )
                                    .child(
                                        div()
                                            .id("vol-slider-track")
                                            .cursor_pointer()
                                            .w(px(92.0))
                                            .h(px(24.0))
                                            .relative()
                                            .flex()
                                            .items_center()
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(
                                                    move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                                        cx.stop_propagation();
                                                        this.is_dragging_volume = true;
                                                        if let Some(b) = vol_bounds_down.get() {
                                                            let (frac, _, _) =
                                                                scrub_metrics_from_point(
                                                                    ev.position,
                                                                    b,
                                                                );
                                                            this.set_player_volume_user(
                                                                f64::from(frac) * 100.0,
                                                                true,
                                                                cx,
                                                            );
                                                        }
                                                    },
                                                ),
                                            )
                                            .on_mouse_move(cx.listener(
                                                move |this, ev: &MouseMoveEvent, _, cx| {
                                                    if this.is_dragging_volume {
                                                        if let Some(b) = vol_bounds_move.get() {
                                                            let (frac, _, _) =
                                                                scrub_metrics_from_point(
                                                                    ev.position,
                                                                    b,
                                                                );
                                                            this.set_player_volume_user(
                                                                f64::from(frac) * 100.0,
                                                                true,
                                                                cx,
                                                            );
                                                        }
                                                    }
                                                },
                                            ))
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.end_player_drag_interactions(cx);
                                                }),
                                            )
                                            .on_mouse_up_out(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.end_player_drag_interactions(cx);
                                                }),
                                            )
                                            .on_scroll(cx.listener(
                                                |this, ev: &ScrollEvent, _, cx| {
                                                    cx.stop_propagation();
                                                    this.adjust_player_volume_by_wheel(ev.delta, cx);
                                                },
                                            ))
                                            .child(
                                                canvas(
                                                    move |bounds, _window, _cx| {
                                                        vol_bounds_prepaint.set(Some(bounds));
                                                    },
                                                    |_bounds, (), _window, _cx| {},
                                                )
                                                .absolute()
                                                .inset_0(),
                                            )
                                            .child(
                                                div()
                                                    .w_full()
                                                    .h(px(4.0))
                                                    .rounded(px(999.0))
                                                    .bg(rgba(0xFFFFFF29))
                                                    .relative()
                                                    .overflow_hidden()
                                                    .child(
                                                        div()
                                                            .absolute()
                                                            .top_0()
                                                            .bottom_0()
                                                            .left_0()
                                                            .w(relative(vol_frac))
                                                            .bg(rgb(0xFFFFFF)),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .absolute()
                                                    .left(relative(vol_frac))
                                                    .ml(px(-6.0))
                                                    .w(px(12.0))
                                                    .h(px(12.0))
                                                    .rounded(px(999.0))
                                                    .bg(rgb(0xFFFFFF)),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .pl(px(6.0))
                                    .flex()
                                    .items_center()
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .font_features(tabular_nums())
                                    .text_size(px(13.0))
                                    .child(
                                        div()
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child(format_timestamp_secs(pos)),
                                    )
                                    .child(
                                        div()
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(format!(" / {}", format_timestamp_secs(duration))),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(sheet_icon_btn(
                                "osd-pop-audio-subs",
                                IconName::ClosedCaption,
                                19.0,
                                cc_active,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_player_popover(
                                        PlayerPopover::AudioAndSubtitles,
                                        cx,
                                    );
                                }),
                            ))
                            .child(sheet_icon_btn(
                                "osd-pop-picture-playback",
                                IconName::Sliders,
                                18.5,
                                picture_active,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_player_popover(
                                        PlayerPopover::PictureAndPlayback,
                                        cx,
                                    );
                                }),
                            ))
                            .when(has_session, |row| {
                                row.child(sheet_icon_btn(
                                    "osd-pop-switch-stream",
                                    IconName::Layers,
                                    18.5,
                                    sources_active,
                                    cx.listener(|this, _, _, cx| {
                                        this.toggle_player_popover(
                                            PlayerPopover::SwitchStream,
                                            cx,
                                        );
                                    }),
                                ))
                            })
                            .child(sheet_icon_btn(
                                "osd-fullscreen",
                                if is_fullscreen {
                                    IconName::Minimize
                                } else {
                                    IconName::Maximize
                                },
                                18.5,
                                false,
                                cx.listener(|_this, _, window, _cx| {
                                    window.toggle_fullscreen();
                                }),
                            )),
                    ),
            )
            .with_animation("player-bottom-osd-fade", obsidian_fade_in(), |el, delta| {
                el.opacity(delta)
            })
    }

    fn render_active_player_popover(
        &self,
        popover: PlayerPopover,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        match popover {
            PlayerPopover::AudioAndSubtitles => {
                self.render_audio_and_subtitles_popover(cx).into_any_element()
            }
            PlayerPopover::SubtitleAppearance => self
                .render_subtitle_appearance_popover(cx)
                .into_any_element(),
            PlayerPopover::PictureAndPlayback => self
                .render_picture_and_playback_popover(cx)
                .into_any_element(),
            PlayerPopover::ToneMappingPicker => self
                .render_tone_mapping_picker_popover(cx)
                .into_any_element(),
            PlayerPopover::UpscalerPicker => {
                self.render_upscaler_picker_popover(cx).into_any_element()
            }
            PlayerPopover::ShaderPresetPicker => self
                .render_shader_preset_picker_popover(cx)
                .into_any_element(),
            PlayerPopover::SwitchStream => self.render_switch_stream_drawer(cx).into_any_element(),
        }
    }
}
