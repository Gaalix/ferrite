use std::{cell::Cell, rc::Rc, sync::Arc};

use gpui::{
    canvas, deferred, div, img, linear_color_stop, linear_gradient, prelude::*, px, relative, rgb,
    rgba, AnimationExt, Bounds, Context, CursorStyle, FontWeight, MouseButton, MouseMoveEvent,
    Pixels, Point, ScrollEvent, SharedString, Window, WindowControlArea,
};
use gpui_mpv::{mpv_viewport, MediaTrack, TrackType};

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{
        compute_scrub_preview_left, summarize_available_stream_addons, PlayerPopover,
        ShaderPresetChoice, StreamResolutionChoice, ToneMappingChoice, UpscaleChoice,
        STREAM_PAGE_SIZE,
    },
    theme::{
        editorial_serif, format_timestamp_secs, glass_capsule_shadow, keycap, meta_divider,
        mono_font, movietracker_mark, neutral_chip, obsidian_fade_in, summarize_stream_for_humans,
        tabular_nums, transparent_cutout, ui_sans, ACCENT, ACCENT_AMBER, ACCENT_HOVER, ACCENT_SOFT,
        BG_ELEVATED, BG_OBSIDIAN, BG_OSD_GLASS, BG_POPOVER_GLASS, BG_SURFACE, BG_SURFACE_3,
        BORDER_ACCENT, BORDER_SOFT, BORDER_STRONG, BORDER_SUBTLE, STATUS_WATCHED, TEXT_MUTED,
        TEXT_PRIMARY, TEXT_SECONDARY,
    },
};

const DRAWER_DETAIL_KEY_OFFSET: usize = 10_000;
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

impl AppShell {
    pub fn render_player_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_fullscreen = window.is_fullscreen();
        let hud_visible = self.is_player_osd_visible();
        let player_csd_controls = self.render_csd_window_controls(window, "player", true);
        let overlay =
            self.render_player_osd_overlay(is_fullscreen, hud_visible, player_csd_controls, cx);

        if let Some(ctrl) = &self.mpv {
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
        }
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
        let ep_label = session.and_then(|s| s.episode_label.clone());
        let meta_id_opt = session.map(|s| s.meta_id.clone());

        let (intro_window, next_video, model_poster_url, model_bg_url) = self.core.with_model(|m| {
            let intro = m
                .player
                .intro_outro
                .as_ref()
                .and_then(|io| io.intro.clone());
            let next_vid = m.player.next_video.clone();

            let mut poster: Option<String> = None;
            let mut background: Option<String> = None;
            if !video_ready {
                if let Some(mid) = meta_id_opt.as_deref() {
                    if let Some(meta_item) = m
                        .meta_details
                        .meta_items
                        .iter()
                        .find_map(|res| res.content.as_ref().and_then(|c| c.ready()))
                        .filter(|item| item.preview.id == mid)
                    {
                        poster = meta_item.preview.poster.as_ref().map(|u| u.to_string());
                        background = meta_item.preview.background.as_ref().map(|u| u.to_string());
                    }
                    if poster.is_none() {
                        if let Some(lib_item) = m.ctx.library.items.get(mid) {
                            poster = lib_item.poster.as_ref().map(|u| u.to_string());
                        }
                    }
                }
            }
            (intro, next_vid, poster, background)
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
                    ep_label.as_deref(),
                    meta_id_opt.as_deref(),
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
            .when(stats_open, |el| el.child(self.render_stats_for_nerds_card(cx)))
            .when(active_skip_to_secs.is_some() || show_next_ep_prompt, |el| {
                let nv_clone = next_video.clone();
                el.child(
                    div()
                        .absolute()
                        .right(px(32.0))
                        .bottom(if hud_visible { px(132.0) } else { px(36.0) })
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
                                    .child(svg_icon(IconName::SkipForward, 14.0, rgb(TEXT_PRIMARY)))
                                    .child("Skip intro"),
                            )
                        })
                        .when(show_next_ep_prompt, |n_el| {
                            let nv_inner = nv_clone.clone();
                            n_el.child(
                                div()
                                    .id("btn-next-episode")
                                    .cursor_pointer()
                                    .h(px(40.0))
                                    .px(px(18.0))
                                    .rounded(px(999.0))
                                    .bg(rgba(BG_OSD_GLASS))
                                    .border_1()
                                    .border_color(if is_prewarmed {
                                        rgba(0x4ade8066)
                                    } else {
                                        rgba(BORDER_STRONG)
                                    })
                                    .hover(|s| s.bg(rgb(BG_ELEVATED)))
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
                                            if let Some(vid) = &nv_inner {
                                                this.advance_to_next_episode(vid.clone(), cx);
                                            }
                                        }),
                                    )
                                    .child(svg_icon(
                                        IconName::StepForward,
                                        14.0,
                                        rgb(if is_prewarmed { STATUS_WATCHED } else { ACCENT }),
                                    ))
                                    .child(if is_prewarmed {
                                        "Next episode (Ready)"
                                    } else {
                                        "Next episode"
                                    }),
                            )
                        }),
                )
            })
            .child(if hud_visible {
                div()
                    .id("player-top-titlebar")
                    .when(!is_fullscreen, |el| {
                        el.window_control_area(WindowControlArea::Drag)
                    })
                    .px(px(24.0))
                    .pt(px(20.0))
                    .flex()
                    .items_center()
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
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .id("player-btn-back")
                                    .cursor_pointer()
                                    .h(px(36.0))
                                    .px(px(14.0))
                                    .rounded(px(999.0))
                                    .bg(rgba(BG_OSD_GLASS))
                                    .border_1()
                                    .border_color(rgba(BORDER_SUBTLE))
                                    .hover(|s| {
                                        s.bg(rgb(BG_ELEVATED)).border_color(rgba(BORDER_STRONG))
                                    })
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(px(13.0))
                                    .text_color(rgb(TEXT_PRIMARY))
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
                                    .child(svg_icon(IconName::ChevronLeft, 15.0, rgb(TEXT_PRIMARY)))
                                    .child("Back"),
                            )
                            .child(
                                div()
                                    .h(px(36.0))
                                    .px(px(16.0))
                                    .rounded(px(999.0))
                                    .bg(rgba(BG_OSD_GLASS))
                                    .border_1()
                                    .border_color(rgba(BORDER_SUBTLE))
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .font(editorial_serif())
                                            .text_size(px(20.0))
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child(title),
                                    )
                                    .when_some(ep_label, |el, ep| {
                                        el.child(meta_divider()).child(
                                            div()
                                                .font(mono_font())
                                                .text_xs()
                                                .text_color(rgb(ACCENT))
                                                .child(ep),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(self.osd_icon_btn(
                                "player-btn-stats",
                                IconName::Info,
                                Some("Stats"),
                                stats_open,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_player_stats_for_nerds(cx);
                                }),
                            ))
                            .child(self.osd_icon_btn(
                                "player-btn-top-fullscreen",
                                if is_fullscreen {
                                    IconName::Minimize
                                } else {
                                    IconName::Maximize
                                },
                                None::<&'static str>,
                                is_fullscreen,
                                cx.listener(|_this, _, window, _cx| {
                                    window.toggle_fullscreen();
                                }),
                            ))
                            .when_some(player_csd_controls, |row, csd| row.child(csd)),
                    )
                    .with_animation("player-top-osd-fade", obsidian_fade_in(), |el, delta| {
                        el.opacity(delta)
                    })
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            .child(if hud_visible {
                self.render_player_bottom_deck(
                    is_fullscreen,
                    video_ready,
                    intro_bounds_secs,
                    scrub_bounds,
                    vol_slider_bounds,
                    cx,
                )
                .into_any_element()
            } else {
                div().into_any_element()
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

    fn render_stats_for_nerds_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                format!("{}x{}", snap.video_width, snap.video_height)
            } else {
                format!(
                    "{}x{} ({short_codec})",
                    snap.video_width, snap.video_height
                )
            }
        } else if snap.buffering || session.is_some_and(|s| !s.loaded_into_mpv) {
            "Buffering".to_owned()
        } else {
            "Idle".to_owned()
        };

        let pipeline_str = format!(
            "{} / {}",
            self.preferences.tone_mapping.label(),
            self.preferences.upscale_mode.label()
        );

        let (dl_bps, peers_opt, _is_torrent) = self.live_player_transport_telemetry();
        let download_str = format!("{:.2} MB/s", dl_bps / (1024.0 * 1024.0));

        let cache_mb = (self.engine.cache_usage_bytes() as f64) / (1024.0 * 1024.0);
        let storage_str = format!(
            "{} ({cache_mb:.0} MB)",
            self.preferences.storage_mode.short_badge()
        );

        let swarm_str = peers_opt.map(|peers| format!("{peers} connected peers"));

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
                        .text_xs()
                        .text_color(rgb(TEXT_MUTED))
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .font(mono_font())
                        .text_xs()
                        .text_color(rgb(TEXT_PRIMARY))
                        .text_right()
                        .truncate()
                        .child(value),
                )
        };

        div()
            .absolute()
            .top(px(68.0))
            .right(px(24.0))
            .w(px(356.0))
            .overflow_hidden()
            .p(px(14.0))
            .rounded(px(10.0))
            .bg(rgba(BG_POPOVER_GLASS))
            .border_1()
            .border_color(rgba(BORDER_SUBTLE))
            .shadow(glass_capsule_shadow())
            .flex()
            .flex_col()
            .gap(px(6.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .pb(px(4.0))
                    .border_b_1()
                    .border_color(rgba(BORDER_SOFT))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
                            .child("Stats for nerds"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(keycap("I"))
                            .child(
                                div()
                                    .id("btn-close-stats-for-nerds")
                                    .cursor_pointer()
                                    .p(px(2.0))
                                    .rounded(px(4.0))
                                    .hover(|s| s.bg(rgb(BG_ELEVATED)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.toggle_player_stats_for_nerds(cx);
                                        }),
                                    )
                                    .child(svg_icon(IconName::X, 12.0, rgb(TEXT_MUTED))),
                            ),
                    ),
            )
            .child(stat_row("Decoder", hwdec_str))
            .child(stat_row("Video", video_str))
            .child(stat_row("Download", download_str))
            .child(stat_row("Pipeline", pipeline_str))
            .child(stat_row(
                "Demux buffer",
                format!("{:.1}s", snap.demuxer_cache_duration.max(0.0)),
            ))
            .child(stat_row("Cache", storage_str))
            .when_some(swarm_str, |el, sw| el.child(stat_row("Swarm", sw)))
    }

    fn render_player_bottom_deck(
        &self,
        is_fullscreen: bool,
        video_ready: bool,
        intro_bounds_secs: Option<(f64, f64)>,
        scrub_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
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

        let sub_delay_ms = (snap.sub_delay * 1000.0).round() as i64;
        let candidate_count = self
            .playback_session
            .as_ref()
            .map(|s| s.candidates.len())
            .unwrap_or(0);

        let speed_label = format!("{:.2}", self.playback_speed)
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
            + "x";

        let scrub_bounds_prepaint = Rc::clone(&scrub_bounds);
        let scrub_bounds_move = Rc::clone(&scrub_bounds);
        let scrub_bounds_click = Rc::clone(&scrub_bounds);

        div()
            .w_full()
            .px(px(24.0))
            .pb(px(22.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(10.0))
            .when_some(self.player_popover, |el, pop| {
                el.child(self.render_active_player_popover(pop, vol_slider_bounds, cx))
            })
            .when(video_ready, |deck| {
                deck.child(
                    div()
                        .w_full()
                        .max_w(px(940.0))
                        .px(px(18.0))
                        .py(px(14.0))
                        .rounded(px(12.0))
                        .bg(rgba(BG_OSD_GLASS))
                        .border_1()
                        .border_color(rgba(BORDER_SUBTLE))
                        .shadow(glass_capsule_shadow())
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .w(px(58.0))
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_PRIMARY))
                                        .child(format_timestamp_secs(pos)),
                                )
                                .child(
                                    div()
                                        .id("player-scrub-track")
                                        .cursor_pointer()
                                        .flex_1()
                                        .h(px(18.0))
                                        .relative()
                                        .flex()
                                        .items_center()
                                        .on_mouse_move(cx.listener(
                                            move |this, ev: &MouseMoveEvent, _, cx| {
                                                if let Some(b) = scrub_bounds_move.get() {
                                                    let (frac, local_x, track_w) =
                                                        scrub_metrics_from_point(ev.position, b);
                                                    this.on_scrub_bar_hover(
                                                        frac,
                                                        Some((local_x, track_w)),
                                                        cx,
                                                    );
                                                }
                                            },
                                        ))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                                cx.stop_propagation();
                                                this.is_scrubbing = true;
                                                if let Some(b) = scrub_bounds_click.get() {
                                                    let (frac, local_x, track_w) =
                                                        scrub_metrics_from_point(ev.position, b);
                                                    this.on_scrub_bar_hover(
                                                        frac,
                                                        Some((local_x, track_w)),
                                                        cx,
                                                    );
                                                    let dur =
                                                        this.player_snapshot.duration.max(0.0);
                                                    if dur > 0.0 {
                                                        this.seek_player_absolute(
                                                            (frac as f64) * dur,
                                                            cx,
                                                        );
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
                                                .bg(rgb(BG_SURFACE_3))
                                                .relative()
                                                .overflow_hidden()
                                                .child(
                                                    div()
                                                        .absolute()
                                                        .top_0()
                                                        .bottom_0()
                                                        .left_0()
                                                        .w(relative(cache_frac))
                                                        .bg(rgba(0xf5f1e833)),
                                                )
                                                .when_some(
                                                    intro_frac_range,
                                                    |bar, (intro_start, intro_w)| {
                                                        bar.child(
                                                            div()
                                                                .absolute()
                                                                .top_0()
                                                                .bottom_0()
                                                                .left(relative(intro_start))
                                                                .w(relative(intro_w))
                                                                .bg(rgba(0xfbbf2477)),
                                                        )
                                                    },
                                                )
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
                                                .ml(px(-5.0))
                                                .w(px(10.0))
                                                .h(px(10.0))
                                                .rounded(px(999.0))
                                                .bg(rgb(ACCENT)),
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
                                                let preview_left = compute_scrub_preview_left(
                                                    cursor_x, track_w, card_w,
                                                );

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
                                                                                    .object_fit(
                                                                                        gpui::ObjectFit::Cover,
                                                                                    ),
                                                                            ),
                                                                    )
                                                                })
                                                                .child(
                                                                    div()
                                                                        .font(mono_font())
                                                                        .text_xs()
                                                                        .text_color(rgb(TEXT_PRIMARY))
                                                                        .child(format_timestamp_secs(
                                                                            preview_secs,
                                                                        )),
                                                                ),
                                                        )
                                                        .with_priority(10),
                                                    )
                                            },
                                        ),
                                )
                                .child(
                                    div()
                                        .w(px(58.0))
                                        .flex()
                                        .justify_end()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_SECONDARY))
                                        .child(format_timestamp_secs(duration)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap(px(10.0))
                                .on_mouse_move(cx.listener(|this, _, _, cx| {
                                    this.clear_scrub_hover(cx);
                                }))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .id("osd-play-toggle")
                                                .cursor_pointer()
                                                .w(px(36.0))
                                                .h(px(36.0))
                                                .rounded(px(999.0))
                                                .bg(rgb(ACCENT))
                                                .hover(|s| s.bg(rgb(ACCENT_HOVER)))
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
                                                    15.0,
                                                    rgb(TEXT_PRIMARY),
                                                )),
                                        )
                                        .child(self.osd_icon_btn(
                                            "osd-seek-b10",
                                            IconName::RotateCcw,
                                            Some("10s"),
                                            false,
                                            cx.listener(|this, _, _, cx| {
                                                this.seek_player_relative(-10.0, cx);
                                            }),
                                        ))
                                        .child(self.osd_icon_btn(
                                            "osd-seek-f10",
                                            IconName::RotateCw,
                                            Some("10s"),
                                            false,
                                            cx.listener(|this, _, _, cx| {
                                                this.seek_player_relative(10.0, cx);
                                            }),
                                        ))
                                        .child(meta_divider())
                                        .child({
                                            let vol_pop_open =
                                                self.player_popover == Some(PlayerPopover::Volume);
                                            let fg = if vol_pop_open || snap.muted {
                                                ACCENT
                                            } else {
                                                TEXT_PRIMARY
                                            };
                                            let vol_label = if snap.muted {
                                                "Muted".to_owned()
                                            } else {
                                                format!("{:.0}%", snap.volume.clamp(0.0, 100.0))
                                            };
                                            div()
                                                .id("osd-volume-btn")
                                                .cursor_pointer()
                                                .h(px(32.0))
                                                .px(px(10.0))
                                                .rounded(px(999.0))
                                                .bg(if vol_pop_open {
                                                    rgba(ACCENT_SOFT)
                                                } else {
                                                    rgb(BG_ELEVATED)
                                                })
                                                .border_1()
                                                .border_color(if vol_pop_open {
                                                    rgba(BORDER_ACCENT)
                                                } else {
                                                    rgba(BORDER_SUBTLE)
                                                })
                                                .hover(|s| {
                                                    s.bg(rgb(BG_SURFACE_3))
                                                        .border_color(rgba(BORDER_STRONG))
                                                })
                                                .flex()
                                                .items_center()
                                                .gap(px(6.0))
                                                .font(mono_font())
                                                .font_features(tabular_nums())
                                                .text_xs()
                                                .text_color(rgb(fg))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.toggle_player_popover(
                                                            PlayerPopover::Volume,
                                                            cx,
                                                        );
                                                    }),
                                                )
                                                .on_scroll(cx.listener(
                                                    |this, ev: &ScrollEvent, _, cx| {
                                                        cx.stop_propagation();
                                                        this.adjust_player_volume_by_wheel(
                                                            ev.delta, cx,
                                                        );
                                                    },
                                                ))
                                                .child(svg_icon(
                                                    if snap.muted || snap.volume <= 0.5 {
                                                        IconName::VolumeX
                                                    } else {
                                                        IconName::Volume2
                                                    },
                                                    13.5,
                                                    rgb(fg),
                                                ))
                                                .child(vol_label)
                                        }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .when(sub_delay_ms != 0, |el| {
                                            el.child(neutral_chip(format!("Sub {sub_delay_ms:+}ms")))
                                        })
                                        .child(self.osd_text_btn(
                                            "osd-pop-speed",
                                            speed_label,
                                            self.player_popover
                                                == Some(PlayerPopover::PlaybackSpeed),
                                            cx.listener(|this, _, _, cx| {
                                                this.toggle_player_popover(
                                                    PlayerPopover::PlaybackSpeed,
                                                    cx,
                                                );
                                            }),
                                        ))
                                        .child(self.osd_icon_btn(
                                            "osd-pop-audio",
                                            IconName::Music,
                                            Some("Audio"),
                                            self.player_popover
                                                == Some(PlayerPopover::AudioTracks),
                                            cx.listener(|this, _, _, cx| {
                                                this.toggle_player_popover(
                                                    PlayerPopover::AudioTracks,
                                                    cx,
                                                );
                                            }),
                                        ))
                                        .child(self.osd_icon_btn(
                                            "osd-pop-subs",
                                            IconName::Subtitles,
                                            Some("Subtitles"),
                                            self.player_popover == Some(PlayerPopover::Subtitles),
                                            cx.listener(|this, _, _, cx| {
                                                this.toggle_player_popover(
                                                    PlayerPopover::Subtitles,
                                                    cx,
                                                );
                                            }),
                                        ))
                                        .child(self.osd_icon_btn(
                                            "osd-pop-shaders",
                                            IconName::Sliders,
                                            Some("Quality"),
                                            self.player_popover
                                                == Some(PlayerPopover::VideoShaders),
                                            cx.listener(|this, _, _, cx| {
                                                this.toggle_player_popover(
                                                    PlayerPopover::VideoShaders,
                                                    cx,
                                                );
                                            }),
                                        ))
                                        .when(candidate_count > 1, |el| {
                                            el.child(self.osd_icon_btn(
                                                "osd-pop-switch-stream",
                                                IconName::Layers,
                                                Some(format!("Source ({candidate_count})")),
                                                self.player_popover
                                                    == Some(PlayerPopover::SwitchStream),
                                                cx.listener(|this, _, _, cx| {
                                                    this.toggle_player_popover(
                                                        PlayerPopover::SwitchStream,
                                                        cx,
                                                    );
                                                }),
                                            ))
                                        })
                                        .child(self.osd_icon_btn(
                                            "osd-fullscreen",
                                            if is_fullscreen {
                                                IconName::Minimize
                                            } else {
                                                IconName::Maximize
                                            },
                                            None::<&'static str>,
                                            is_fullscreen,
                                            cx.listener(|_this, _, window, _cx| {
                                                window.toggle_fullscreen();
                                            }),
                                        )),
                                ),
                        ),
                )
            })
            .with_animation("player-bottom-osd-fade", obsidian_fade_in(), |el, delta| {
                el.opacity(delta)
            })
    }

    fn osd_icon_btn(
        &self,
        id: &'static str,
        icon: IconName,
        label: Option<impl Into<SharedString>>,
        active: bool,
        on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> impl IntoElement {
        let fg = if active { ACCENT } else { TEXT_PRIMARY };
        div()
            .id(id)
            .cursor_pointer()
            .h(px(32.0))
            .px(px(10.0))
            .rounded(px(999.0))
            .bg(if active {
                rgba(ACCENT_SOFT)
            } else {
                rgb(BG_ELEVATED)
            })
            .border_1()
            .border_color(if active {
                rgba(BORDER_ACCENT)
            } else {
                rgba(BORDER_SUBTLE)
            })
            .hover(|s| s.bg(rgb(BG_SURFACE_3)).border_color(rgba(BORDER_STRONG)))
            .flex()
            .items_center()
            .gap(px(6.0))
            .font(ui_sans(FontWeight::MEDIUM))
            .text_xs()
            .text_color(rgb(fg))
            .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                cx.stop_propagation();
                on_click(ev, window, cx);
            })
            .child(svg_icon(icon, 13.5, rgb(fg)))
            .when_some(label.map(Into::into), |el, text| el.child(text))
    }

    fn osd_text_btn(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        active: bool,
        on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> impl IntoElement {
        let fg = if active { ACCENT } else { TEXT_PRIMARY };
        div()
            .id(id)
            .cursor_pointer()
            .h(px(32.0))
            .px(px(10.0))
            .rounded(px(999.0))
            .bg(if active {
                rgba(ACCENT_SOFT)
            } else {
                rgb(BG_ELEVATED)
            })
            .border_1()
            .border_color(if active {
                rgba(BORDER_ACCENT)
            } else {
                rgba(BORDER_SUBTLE)
            })
            .hover(|s| s.bg(rgb(BG_SURFACE_3)).border_color(rgba(BORDER_STRONG)))
            .flex()
            .items_center()
            .justify_center()
            .font(mono_font())
            .text_xs()
            .text_color(rgb(fg))
            .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                cx.stop_propagation();
                on_click(ev, window, cx);
            })
            .child(label.into())
    }

    fn render_active_player_popover(
        &self,
        popover: PlayerPopover,
        vol_slider_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        match popover {
            PlayerPopover::Volume => self
                .render_volume_popover(vol_slider_bounds, cx)
                .into_any_element(),
            PlayerPopover::SwitchStream => self.render_switch_stream_drawer(cx).into_any_element(),
            PlayerPopover::AudioTracks => self
                .render_track_popover(TrackType::Audio, "Audio track", "A", cx)
                .into_any_element(),
            PlayerPopover::Subtitles => self
                .render_track_popover(TrackType::Sub, "Subtitles", "S", cx)
                .into_any_element(),
            PlayerPopover::VideoShaders => self.render_video_shader_popover(cx).into_any_element(),
            PlayerPopover::PlaybackSpeed => self.render_speed_popover(cx).into_any_element(),
        }
    }

    fn render_volume_popover(
        &self,
        vol_slider_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snap = &self.player_snapshot;
        let muted = snap.muted;
        let clamped_vol = snap.volume.clamp(0.0, 100.0);
        let vol_frac = (clamped_vol / 100.0).clamp(0.0, 1.0) as f32;
        let readout = if muted {
            "Muted".to_owned()
        } else {
            format!("{clamped_vol:.0}%")
        };

        let bounds_prepaint = Rc::clone(&vol_slider_bounds);
        let bounds_down = Rc::clone(&vol_slider_bounds);
        let bounds_move = vol_slider_bounds;

        div()
            .w_full()
            .max_w(px(940.0))
            .flex()
            .justify_start()
            .pl(px(116.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .id("player-volume-popover")
                    .w(px(284.0))
                    .px(px(14.0))
                    .py(px(10.0))
                    .rounded_xl()
                    .bg(rgb(BG_ELEVATED))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .shadow(glass_capsule_shadow())
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| {
                        cx.stop_propagation();
                        this.adjust_player_volume_by_wheel(ev.delta, cx);
                    }))
                    .child(
                        div()
                            .id("vol-popover-mute")
                            .cursor_pointer()
                            .w(px(32.0))
                            .h(px(32.0))
                            .rounded(px(999.0))
                            .bg(if muted {
                                rgba(ACCENT_SOFT)
                            } else {
                                rgb(BG_SURFACE_3)
                            })
                            .border_1()
                            .border_color(if muted {
                                rgba(BORDER_ACCENT)
                            } else {
                                rgba(BORDER_SUBTLE)
                            })
                            .hover(|s| s.border_color(rgba(BORDER_STRONG)))
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
                            .child(svg_icon(
                                if muted || clamped_vol <= 0.5 {
                                    IconName::VolumeX
                                } else {
                                    IconName::Volume2
                                },
                                14.0,
                                rgb(if muted { ACCENT } else { TEXT_PRIMARY }),
                            )),
                    )
                    .child(
                        div()
                            .id("vol-slider-track")
                            .cursor_pointer()
                            .flex_1()
                            .h(px(24.0))
                            .relative()
                            .flex()
                            .items_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.is_dragging_volume = true;
                                    if let Some(b) = bounds_down.get() {
                                        let (frac, _, _) =
                                            scrub_metrics_from_point(ev.position, b);
                                        this.set_player_volume_user(
                                            f64::from(frac) * 100.0,
                                            true,
                                            cx,
                                        );
                                    }
                                }),
                            )
                            .on_mouse_move(cx.listener(
                                move |this, ev: &MouseMoveEvent, _, cx| {
                                    if this.is_dragging_volume {
                                        if let Some(b) = bounds_move.get() {
                                            let (frac, _, _) =
                                                scrub_metrics_from_point(ev.position, b);
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
                            .child(
                                canvas(
                                    move |bounds, _window, _cx| {
                                        bounds_prepaint.set(Some(bounds));
                                    },
                                    |_bounds, (), _window, _cx| {},
                                )
                                .absolute()
                                .inset_0(),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .h(px(6.0))
                                    .rounded(px(999.0))
                                    .bg(rgb(BG_SURFACE_3))
                                    .relative()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .bottom_0()
                                            .left_0()
                                            .w(relative(vol_frac))
                                            .bg(if muted {
                                                rgba(0xff5e3a66)
                                            } else {
                                                rgb(ACCENT)
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(relative(vol_frac))
                                    .ml(px(-7.0))
                                    .w(px(14.0))
                                    .h(px(14.0))
                                    .rounded(px(999.0))
                                    .bg(rgb(TEXT_PRIMARY))
                                    .border_1()
                                    .border_color(rgba(BORDER_STRONG)),
                            ),
                    )
                    .child(
                        div()
                            .w(px(44.0))
                            .flex()
                            .justify_end()
                            .font(mono_font())
                            .font_features(tabular_nums())
                            .text_xs()
                            .text_color(rgb(if muted { ACCENT } else { TEXT_PRIMARY }))
                            .child(readout),
                    ),
            )
    }

    fn render_track_popover(
        &self,
        kind: TrackType,
        heading: &'static str,
        shortcut_key: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tracks: Vec<MediaTrack> = self
            .player_snapshot
            .tracks
            .iter()
            .filter(|t| t.track_type == kind)
            .cloned()
            .collect();
        let any_sub_selected = tracks.iter().any(|t| t.selected);

        div()
            .w_full()
            .max_w(px(940.0))
            .flex()
            .justify_end()
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .id("player-track-popover-scroll")
                    .w(px(380.0))
                    .max_h(px(340.0))
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .rounded(px(10.0))
                    .bg(rgba(BG_POPOVER_GLASS))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .shadow(glass_capsule_shadow())
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .pb(px(6.0))
                                    .border_b_1()
                                    .border_color(rgba(BORDER_SOFT))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .font(ui_sans(FontWeight::MEDIUM))
                                                    .text_size(px(13.0))
                                                    .text_color(rgb(TEXT_PRIMARY))
                                                    .child(heading),
                                            )
                                            .child(keycap(shortcut_key)),
                                    )
                                    .when(kind == TrackType::Sub, |el| {
                                        el.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(6.0))
                                                .child(
                                                    div()
                                                        .id("sub-delay-minus")
                                                        .cursor_pointer()
                                                        .px(px(8.0))
                                                        .py(px(3.0))
                                                        .rounded(px(4.0))
                                                        .bg(rgb(BG_ELEVATED))
                                                        .border_1()
                                                        .border_color(rgba(BORDER_SUBTLE))
                                                        .hover(|s| s.bg(rgb(BG_SURFACE_3)))
                                                        .font(mono_font())
                                                        .text_xs()
                                                        .text_color(rgb(TEXT_PRIMARY))
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(|this, _, _, cx| {
                                                                this.adjust_subtitle_delay(-0.1, cx);
                                                            }),
                                                        )
                                                        .child("-100ms"),
                                                )
                                                .child(
                                                    div()
                                                        .id("sub-delay-plus")
                                                        .cursor_pointer()
                                                        .px(px(8.0))
                                                        .py(px(3.0))
                                                        .rounded(px(4.0))
                                                        .bg(rgb(BG_ELEVATED))
                                                        .border_1()
                                                        .border_color(rgba(BORDER_SUBTLE))
                                                        .hover(|s| s.bg(rgb(BG_SURFACE_3)))
                                                        .font(mono_font())
                                                        .text_xs()
                                                        .text_color(rgb(TEXT_PRIMARY))
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(|this, _, _, cx| {
                                                                this.adjust_subtitle_delay(0.1, cx);
                                                            }),
                                                        )
                                                        .child("+100ms"),
                                                ),
                                        )
                                    }),
                            )
                            .when(kind == TrackType::Sub, |el| {
                                let off_selected = !any_sub_selected;
                                el.child(
                                    div()
                                        .id("track-sub-off")
                                        .flex_shrink_0()
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(8.0))
                                        .rounded(px(6.0))
                                        .bg(if off_selected {
                                            rgba(ACCENT_SOFT)
                                        } else {
                                            rgba(0x00000000)
                                        })
                                        .hover(|s| s.bg(rgb(BG_ELEVATED)))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .text_xs()
                                        .text_color(if off_selected {
                                            rgb(ACCENT)
                                        } else {
                                            rgb(TEXT_PRIMARY)
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.user_select_subtitle_track(None, cx);
                                            }),
                                        )
                                        .child("Subtitles off")
                                        .when(off_selected, |row| {
                                            row.child(svg_icon(IconName::Check, 13.0, rgb(ACCENT)))
                                        }),
                                )
                            })
                            .children(tracks.into_iter().enumerate().map(|(idx, tr)| {
                                let tid = tr.id;
                                let primary_name = tr
                                    .title
                                    .clone()
                                    .or_else(|| tr.lang.as_ref().map(|l| l.to_ascii_uppercase()))
                                    .unwrap_or_else(|| format!("Track #{tid}"));

                                let mut secondary_tags: Vec<String> = Vec::with_capacity(3);
                                if let Some(lang) = &tr.lang {
                                    if tr.title.is_some() {
                                        secondary_tags.push(lang.to_ascii_uppercase());
                                    }
                                }
                                if let Some(codec) = &tr.codec {
                                    secondary_tags.push(codec.to_ascii_uppercase());
                                }
                                if tr.external {
                                    secondary_tags.push("Addon".to_owned());
                                }

                                div()
                                    .id(("player-track-item", idx))
                                    .flex_shrink_0()
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(8.0))
                                    .rounded(px(6.0))
                                    .bg(if tr.selected {
                                        rgba(ACCENT_SOFT)
                                    } else {
                                        rgba(0x00000000)
                                    })
                                    .hover(|s| s.bg(rgb(BG_ELEVATED)))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| match kind {
                                            TrackType::Audio => {
                                                this.user_select_audio_track(tid, cx)
                                            }
                                            TrackType::Sub => {
                                                this.user_select_subtitle_track(Some(tid), cx)
                                            }
                                            TrackType::Video => {}
                                        }),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .min_w(px(0.0))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(if tr.selected {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgb(TEXT_PRIMARY)
                                                    })
                                                    .truncate()
                                                    .child(primary_name),
                                            )
                                            .when(!secondary_tags.is_empty(), |meta_row| {
                                                meta_row.child(
                                                    div()
                                                        .font(mono_font())
                                                        .text_size(px(11.0))
                                                        .text_color(rgb(TEXT_MUTED))
                                                        .child(secondary_tags.join(" / ")),
                                                )
                                            }),
                                    )
                                    .when(tr.selected, |row| {
                                        row.child(svg_icon(IconName::Check, 13.0, rgb(ACCENT)))
                                    })
                            })),
                    ),
            )
    }

    fn render_video_shader_popover(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = &self.preferences;
        let exp = prefs.experimental_video;

        div()
            .w_full()
            .max_w(px(940.0))
            .flex()
            .justify_end()
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .w(px(420.0))
                    .p(px(16.0))
                    .rounded(px(10.0))
                    .bg(rgba(BG_POPOVER_GLASS))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .shadow(glass_capsule_shadow())
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(
                        div()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
                            .child("HDR tone-mapping"),
                    )
                    .child(
                        div().flex().flex_wrap().gap(px(6.0)).children(
                            ToneMappingChoice::ALL
                                .into_iter()
                                .enumerate()
                                .map(|(idx, choice)| {
                                    let active = prefs.tone_mapping == choice;
                                    div()
                                        .id(("osd-tm-pill", idx))
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(5.0))
                                        .rounded(px(999.0))
                                        .bg(if active {
                                            rgb(ACCENT)
                                        } else {
                                            rgb(BG_ELEVATED)
                                        })
                                        .border_1()
                                        .border_color(if active {
                                            rgb(ACCENT)
                                        } else {
                                            rgb(BG_SURFACE_3)
                                        })
                                        .text_xs()
                                        .text_color(rgb(TEXT_PRIMARY))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.set_tone_mapping_choice(choice, cx);
                                            }),
                                        )
                                        .child(choice.label())
                                }),
                        ),
                    )
                    .child(
                        div()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
                            .child("GPU upscaler"),
                    )
                    .child(
                        div().flex().flex_wrap().gap(px(6.0)).children(
                            UpscaleChoice::ALL
                                .into_iter()
                                .enumerate()
                                .map(|(idx, choice)| {
                                    let active = prefs.upscale_mode == choice;
                                    div()
                                        .id(("osd-upscale-pill", idx))
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(5.0))
                                        .rounded(px(999.0))
                                        .bg(if active {
                                            rgb(ACCENT)
                                        } else {
                                            rgb(BG_ELEVATED)
                                        })
                                        .border_1()
                                        .border_color(if active {
                                            rgb(ACCENT)
                                        } else {
                                            rgb(BG_SURFACE_3)
                                        })
                                        .text_xs()
                                        .text_color(rgb(TEXT_PRIMARY))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.set_upscale_choice(choice, cx);
                                            }),
                                        )
                                        .child(choice.label())
                                }),
                        ),
                    )
                    .child(
                        div()
                            .pt(px(4.0))
                            .border_t_1()
                            .border_color(rgba(BORDER_SOFT))
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_xs()
                            .text_color(rgb(ACCENT_AMBER))
                            .child("Experimental video lab (opt-in, off by default)"),
                    )
                    .child(
                        div().flex().flex_wrap().gap(px(6.0)).children(
                            ShaderPresetChoice::ALL
                                .into_iter()
                                .enumerate()
                                .map(|(idx, preset)| {
                                    let active = exp.shader_preset == preset;
                                    div()
                                        .id(("osd-shader-pill", idx))
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(4.5))
                                        .rounded(px(999.0))
                                        .bg(if active {
                                            rgba(0xfbbf2422)
                                        } else {
                                            rgb(BG_ELEVATED)
                                        })
                                        .border_1()
                                        .border_color(if active {
                                            rgba(0xfbbf2466)
                                        } else {
                                            rgba(BORDER_SUBTLE)
                                        })
                                        .text_xs()
                                        .text_color(if active {
                                            rgb(ACCENT_AMBER)
                                        } else {
                                            rgb(TEXT_SECONDARY)
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.preferences.experimental_video.shader_preset =
                                                    preset;
                                                this.apply_experimental_video_to_mpv();
                                                let _ = this.preferences.save();
                                                cx.notify();
                                            }),
                                        )
                                        .child(preset.label())
                                }),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .id("osd-toggle-deband")
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(5.0))
                                    .rounded(px(999.0))
                                    .bg(if exp.deband_enabled {
                                        rgba(0x4ade801f)
                                    } else {
                                        rgb(BG_ELEVATED)
                                    })
                                    .border_1()
                                    .border_color(if exp.deband_enabled {
                                        rgba(0x4ade8066)
                                    } else {
                                        rgba(BORDER_SUBTLE)
                                    })
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .text_xs()
                                    .text_color(if exp.deband_enabled {
                                        rgb(STATUS_WATCHED)
                                    } else {
                                        rgb(TEXT_SECONDARY)
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.preferences.experimental_video.deband_enabled =
                                                !this
                                                    .preferences
                                                    .experimental_video
                                                    .deband_enabled;
                                            this.apply_experimental_video_to_mpv();
                                            let _ = this.preferences.save();
                                            cx.notify();
                                        }),
                                    )
                                    .when(exp.deband_enabled, |b| {
                                        b.child(svg_icon(
                                            IconName::Check,
                                            12.0,
                                            rgb(STATUS_WATCHED),
                                        ))
                                    })
                                    .child(if exp.deband_enabled {
                                        "Deband on"
                                    } else {
                                        "Deband off"
                                    }),
                            )
                            .child(
                                div()
                                    .id("osd-toggle-dv")
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(5.0))
                                    .rounded(px(999.0))
                                    .bg(if exp.dolby_vision_reshape {
                                        rgba(0x4ade801f)
                                    } else {
                                        rgb(BG_ELEVATED)
                                    })
                                    .border_1()
                                    .border_color(if exp.dolby_vision_reshape {
                                        rgba(0x4ade8066)
                                    } else {
                                        rgba(BORDER_SUBTLE)
                                    })
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .text_xs()
                                    .text_color(if exp.dolby_vision_reshape {
                                        rgb(STATUS_WATCHED)
                                    } else {
                                        rgb(TEXT_SECONDARY)
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
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
                                    )
                                    .when(exp.dolby_vision_reshape, |b| {
                                        b.child(svg_icon(
                                            IconName::Check,
                                            12.0,
                                            rgb(STATUS_WATCHED),
                                        ))
                                    })
                                    .child(if exp.dolby_vision_reshape {
                                        "DV reshape on"
                                    } else {
                                        "DV reshape off"
                                    }),
                            ),
                    ),
            )
    }

    fn render_speed_popover(&self, cx: &mut Context<Self>) -> impl IntoElement {
        const SPEEDS: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
        div()
            .w_full()
            .max_w(px(940.0))
            .flex()
            .justify_end()
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .p(px(10.0))
                    .rounded(px(10.0))
                    .bg(rgba(BG_POPOVER_GLASS))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .shadow(glass_capsule_shadow())
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .children(SPEEDS.into_iter().enumerate().map(|(idx, spd)| {
                        let active = (self.playback_speed - spd).abs() < 0.01;
                        div()
                            .id(("osd-speed-item", idx))
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(999.0))
                            .bg(if active {
                                rgb(ACCENT)
                            } else {
                                rgb(BG_ELEVATED)
                            })
                            .font(mono_font())
                            .text_xs()
                            .text_color(rgb(TEXT_PRIMARY))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_playback_speed_user(spd, cx);
                                }),
                            )
                            .child(format!("{spd}x"))
                    })),
            )
    }

    fn render_switch_stream_drawer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((active_idx, list_arc)) = self
            .playback_session
            .as_ref()
            .map(|s| (s.active_index, Arc::clone(&s.candidates)))
        else {
            return div().into_any_element();
        };

        let has_debrid = self.has_debrid_access();
        let addon_counts = summarize_available_stream_addons(&list_arc);
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

        div()
            .w_full()
            .max_w(px(940.0))
            .flex()
            .justify_end()
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .id("switch-stream-drawer-scroll")
                    .w(px(520.0))
                    .max_h(px(400.0))
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .rounded(px(10.0))
                    .bg(rgba(BG_POPOVER_GLASS))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .shadow(glass_capsule_shadow())
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                        div()
                            .pb(px(6.0))
                            .border_b_1()
                            .border_color(rgba(BORDER_SOFT))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(px(13.5))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .child("Switch source (keeps current timestamp)"),
                            )
                            .child(
                                div()
                                    .font(mono_font())
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(format!(
                                        "{filtered_total} / {} sources",
                                        list_arc.len()
                                    )),
                            ),
                    )
                    .when(list_arc.len() > 4, |drawer| {
                        drawer.child(
                            div()
                                .pb(px(4.0))
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(5.0))
                                .child(drawer_filter_chip(
                                    "drawer-filter-all",
                                    "All",
                                    is_unfiltered,
                                    cx.listener(|this, _, _, cx| {
                                        this.stream_filter.reset();
                                        this.player_source_limit = STREAM_PAGE_SIZE;
                                        cx.notify();
                                    }),
                                ))
                                .children(
                                    StreamResolutionChoice::ALL.into_iter().enumerate().map(
                                        |(r_idx, res)| {
                                            let active =
                                                self.stream_filter.resolutions.contains(&res);
                                            drawer_filter_chip(
                                                ("drawer-filter-res", r_idx),
                                                res.label(),
                                                active,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.stream_filter.toggle_resolution(res);
                                                    this.player_source_limit = STREAM_PAGE_SIZE;
                                                    cx.notify();
                                                }),
                                            )
                                        },
                                    ),
                                )
                                .child(drawer_filter_chip(
                                    "drawer-filter-hdr",
                                    "HDR",
                                    self.stream_filter.hdr_only,
                                    cx.listener(|this, _, _, cx| {
                                        this.stream_filter.hdr_only = !this.stream_filter.hdr_only;
                                        this.player_source_limit = STREAM_PAGE_SIZE;
                                        cx.notify();
                                    }),
                                ))
                                .when(has_debrid, |row| {
                                    row.child(drawer_filter_chip(
                                        "drawer-filter-cached",
                                        "Cached",
                                        self.stream_filter.cached_only,
                                        cx.listener(|this, _, _, cx| {
                                            this.stream_filter.cached_only =
                                                !this.stream_filter.cached_only;
                                            this.player_source_limit = STREAM_PAGE_SIZE;
                                            cx.notify();
                                        }),
                                    ))
                                })
                                .when(addon_counts.len() > 1, |row| {
                                    row.children(addon_counts.into_iter().enumerate().map(
                                        |(a_idx, (a_key, a_name, _cnt))| {
                                            let active =
                                                self.stream_filter.is_addon_selected(&a_key);
                                            drawer_filter_chip(
                                                ("drawer-filter-addon", a_idx),
                                                a_name,
                                                active,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.stream_filter.toggle_addon(&a_key);
                                                    this.player_source_limit = STREAM_PAGE_SIZE;
                                                    cx.notify();
                                                }),
                                            )
                                        },
                                    ))
                                }),
                        )
                    })
                    .children(filtered_indices.into_iter().take(limit).map(|orig_idx| {
                        let cand = &list_arc[orig_idx];
                        let is_current = orig_idx == active_idx;
                        let detail_key = DRAWER_DETAIL_KEY_OFFSET + orig_idx;
                        let details_open = self.expanded_stream_rows.contains(&detail_key);
                        let summary = summarize_stream_for_humans(&cand.parsed);
                        let raw_title = details_open.then(|| cand.parsed.display_title.clone());

                        div()
                            .id(("drawer-stream-row", orig_idx))
                            .cursor_pointer()
                            .p(px(10.0))
                            .rounded(px(8.0))
                            .bg(if is_current {
                                rgba(ACCENT_SOFT)
                            } else {
                                rgb(BG_SURFACE)
                            })
                            .border_1()
                            .border_color(if is_current {
                                rgba(BORDER_ACCENT)
                            } else {
                                rgba(BORDER_SUBTLE)
                            })
                            .hover(|s| s.bg(rgb(BG_ELEVATED)).border_color(rgba(BORDER_STRONG)))
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.switch_active_player_stream(orig_idx, false, cx);
                                }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .gap(px(8.0))
                                            .when(is_current, |row| {
                                                row.child(
                                                    div()
                                                        .px(px(8.0))
                                                        .py(px(2.0))
                                                        .rounded(px(999.0))
                                                        .bg(rgb(ACCENT))
                                                        .font(mono_font())
                                                        .text_size(px(11.0))
                                                        .text_color(rgb(TEXT_PRIMARY))
                                                        .child("Playing"),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .font(mono_font())
                                                    .text_xs()
                                                    .text_color(rgb(TEXT_PRIMARY))
                                                    .child(summary.resolution),
                                            )
                                            .when_some(summary.hdr_label, |row, hdr| {
                                                row.child(meta_divider()).child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(TEXT_PRIMARY))
                                                        .child(hdr),
                                                )
                                            })
                                            .when_some(summary.notable_audio, |row, aud| {
                                                row.child(meta_divider()).child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(TEXT_SECONDARY))
                                                        .child(aud),
                                                )
                                            })
                                            .child(meta_divider())
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(if cand.parsed.is_cached {
                                                        rgb(STATUS_WATCHED)
                                                    } else {
                                                        rgb(TEXT_SECONDARY)
                                                    })
                                                    .child(summary.availability_text),
                                            )
                                            .when_some(summary.size_text, |row, sz| {
                                                row.child(meta_divider()).child(
                                                    div()
                                                        .font(mono_font())
                                                        .text_xs()
                                                        .text_color(rgb(TEXT_MUTED))
                                                        .child(sz),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id(("drawer-stream-details-btn", orig_idx))
                                            .cursor_pointer()
                                            .px(px(8.0))
                                            .py(px(3.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(BG_ELEVATED))
                                            .hover(|s| s.bg(rgb(BG_SURFACE_3)))
                                            .text_size(px(11.0))
                                            .text_color(rgb(TEXT_MUTED))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.toggle_stream_row_details(detail_key, cx);
                                                }),
                                            )
                                            .child(if details_open { "Hide" } else { "Details" }),
                                    ),
                            )
                            .when_some(summary.addon_label, |col, addon| {
                                col.child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(addon),
                                )
                            })
                            .when_some(raw_title, |col, title_str| {
                                col.child(
                                    div()
                                        .pt(px(6.0))
                                        .border_t_1()
                                        .border_color(rgba(BORDER_SOFT))
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        .when(!summary.technical_specs.is_empty(), |d_el| {
                                            d_el.child(
                                                div()
                                                    .font(mono_font())
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(TEXT_SECONDARY))
                                                    .child(summary.technical_specs.join(" / ")),
                                            )
                                        })
                                        .child(
                                            div()
                                                .font(mono_font())
                                                .text_size(px(11.0))
                                                .text_color(rgb(TEXT_MUTED))
                                                .child(title_str),
                                        ),
                                )
                            })
                    }))
                    .when(remaining > 0, |drawer| {
                        let next_step = remaining.min(STREAM_PAGE_SIZE);
                        drawer.child(
                            div()
                                .id("btn-drawer-show-more")
                                .cursor_pointer()
                                .h(px(32.0))
                                .px(px(12.0))
                                .rounded(px(6.0))
                                .bg(rgb(BG_ELEVATED))
                                .border_1()
                                .border_color(rgba(BORDER_SUBTLE))
                                .hover(|s| s.border_color(rgba(BORDER_STRONG)))
                                .flex()
                                .items_center()
                                .justify_between()
                                .font(ui_sans(FontWeight::MEDIUM))
                                .text_xs()
                                .text_color(rgb(TEXT_PRIMARY))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
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
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(format!("{remaining} remaining")),
                                ),
                        )
                    }),
                    ),
            )
            .into_any_element()
    }
}

fn drawer_filter_chip(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id.into())
        .cursor_pointer()
        .h(px(26.0))
        .px(px(10.0))
        .rounded(px(999.0))
        .bg(if active {
            rgb(ACCENT)
        } else {
            rgb(BG_ELEVATED)
        })
        .border_1()
        .border_color(if active {
            rgb(ACCENT)
        } else {
            rgba(BORDER_SUBTLE)
        })
        .hover(|s| {
            if active {
                s.bg(rgb(ACCENT_HOVER))
            } else {
                s.bg(rgb(BG_SURFACE_3)).border_color(rgba(BORDER_STRONG))
            }
        })
        .flex()
        .items_center()
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(px(11.5))
        .text_color(if active {
            rgb(TEXT_PRIMARY)
        } else {
            rgb(TEXT_SECONDARY)
        })
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(label.into())
}
