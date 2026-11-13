use std::{ops::Range, sync::Arc};

use gpui::{
    deferred, div, img, linear_color_stop, linear_gradient, point, prelude::*, px, relative, rgb,
    rgba, Context, FontWeight, MouseButton, ScrollDelta, ScrollEvent, SharedString, Window,
};
use stremio_core_runtime::{
    stremio_core::{
        constants::CINEMETA_URL,
        models::common::Loadable,
        types::{
            addon::ResourceRequest, library::LibraryItem, resource::Video,
            watched_bitfield::WatchedBitField,
        },
    },
    PersonCreditChip,
};

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{
        build_default_meta_request, build_subtitles_resource_path, classify_library_item_status,
        extract_cast_and_directors_from_links, extract_genres, extract_imdb_rating,
        find_cached_preview_in_model, format_movietracker_episode_code,
        merge_and_enrich_stream_candidates, pick_default_series_episode_id,
        resolve_ready_meta_item, resume_offset_for_library_item, scroll_near_bottom,
        sync_memoized_meta_streams, CachedTitlePreview, LibraryStatusTab, PlaybackCandidate,
        StreamResolutionChoice, STREAM_PAGE_SIZE,
    },
    theme::{
        compact_audio_badge, compact_codec_badge, compact_hdr_badge, editorial_serif,
        estimate_bitrate_mbps, format_bitrate_mbps, format_episode_air_date,
        format_timestamp_secs, glass_capsule_shadow, meta_divider, mono_font,
        parse_runtime_minutes, placeholder_tint_for_id, poster_card_shadow,
        semantic_status_pill, sentence_case_label, summarize_stream_for_humans, tabular_nums,
        ui_sans, ACCENT, ACCENT_AMBER, ACCENT_HOVER, ACCENT_SOFT, BG_ELEVATED, BG_INPUT,
        BG_OBSIDIAN, BG_POPOVER_GLASS, BG_SURFACE, BORDER_POSTER_HOVER, BORDER_SOFT,
        BORDER_STRONG, BORDER_SUBTLE, STATUS_WATCHED, STATUS_WATCHING, STATUS_WATCHLIST,
        TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY,
    },
    views::search_view::person_initials,
};

#[derive(Clone)]
struct EpisodeRowData {
    video_id: String,
    ep_code: String,
    title: String,
    thumb_url: Option<String>,
    air_date: Option<String>,
    runtime_label: Option<String>,
    synopsis: Option<String>,
    is_selected: bool,
    is_watched: bool,
    progress_fraction: Option<f32>,
}

impl AppShell {
    pub fn render_meta_details_view(
        &mut self,
        route_type: &str,
        route_id: &str,
        route_video_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let has_debrid = self.has_debrid_access();
        let synthetic_fallback = self
            .synthetic_tmdb_meta
            .as_ref()
            .and_then(|(sid, m)| (sid == route_id).then(|| m.clone()));
        let memo_slot = &mut self.memoized_meta_streams;

        let details_snap = self.core.with_model(|model| {
            let meta_pair = resolve_ready_meta_item(&model.meta_details.meta_items)
                .map(|(m, req)| (m.clone(), req))
                .or_else(|| {
                    synthetic_fallback.map(|m| {
                        let req = model
                            .meta_details
                            .meta_items
                            .first()
                            .map(|r| r.request.clone())
                            .unwrap_or_else(|| {
                                build_default_meta_request(
                                    &CINEMETA_URL,
                                    &m.preview.r#type,
                                    &m.preview.id,
                                )
                            });
                        (m, req)
                    })
                });
            let is_meta_loading = model
                .meta_details
                .meta_items
                .iter()
                .any(|r| matches!(r.content, Some(Loadable::Loading)));

            let Some((meta, meta_request)) = meta_pair else {
                let cached = find_cached_preview_in_model(model, route_id);
                return Err((is_meta_loading, cached.map(Box::new)));
            };

            let selected_stream_vid = model
                .meta_details
                .selected
                .as_ref()
                .and_then(|s| s.stream_path.as_ref())
                .map(|p| p.id.clone());
            let has_stream_request = selected_stream_vid.is_some();
            let has_stream_addons = !model.ctx.profile.addons.is_empty();

            let in_library = model
                .ctx
                .library
                .items
                .get(route_id)
                .is_some_and(|i| !i.removed);
            let library_item = model
                .meta_details
                .library_item
                .clone()
                .or_else(|| model.ctx.library.items.get(route_id).cloned());
            let watched = model.meta_details.watched.clone();

            let active_video_id = pick_default_series_episode_id(
                &meta,
                selected_stream_vid.as_deref().or(route_video_id),
                library_item
                    .as_ref()
                    .and_then(|li| li.state.video_id.as_deref()),
            )
            .unwrap_or_else(|| meta.preview.id.clone());

            let (all_playable_arc, available_addons_arc, stream_addons_ready, stream_addons_total) =
                sync_memoized_meta_streams(
                    memo_slot,
                    route_type,
                    route_id,
                    &active_video_id,
                    has_debrid,
                    model
                        .meta_details
                        .meta_streams
                        .iter()
                        .chain(model.meta_details.streams.iter()),
                    &model.ctx.profile.addons,
                );

            Ok((
                meta,
                meta_request,
                active_video_id,
                all_playable_arc,
                available_addons_arc,
                stream_addons_ready,
                stream_addons_total,
                has_stream_request,
                has_stream_addons,
                in_library,
                library_item,
                watched,
            ))
        });

        let (
            meta,
            meta_request,
            active_video_id,
            addon_playable_arc,
            _addon_available_arc,
            stream_addons_ready,
            stream_addons_total,
            has_stream_request,
            has_stream_addons,
            in_library,
            library_item,
            watched,
        ) = match details_snap {
            Ok(ready) => ready,
            Err((is_meta_loading, cached_preview)) => {
                return self
                    .render_loading_meta_details_view(
                        route_type,
                        route_id,
                        is_meta_loading,
                        cached_preview.map(|b| *b),
                        cx,
                    )
                    .into_any_element();
            }
        };

        self.maybe_trigger_stream_enrichments(cx);
        self.request_background_enrichment_if_needed(&meta.preview.r#type, &meta.preview.id, cx);
        let enriched = self.core.cached_enriched_meta(&meta.preview.id);
        let private_cfg = self.core.private_integrations();
        let prowlarr_slice = if self.prowlarr_fallback_key.as_ref()
            == Some(&(
                route_type.to_owned(),
                meta.preview.id.clone(),
                active_video_id.clone(),
            )) {
            self.prowlarr_fallback_streams.as_slice()
        } else {
            &[]
        };
        let (all_playable_arc, available_addons_arc) = merge_and_enrich_stream_candidates(
            &addon_playable_arc,
            prowlarr_slice,
            private_cfg.active_debrid,
            &self.local_debrid_cache_map,
            private_cfg.has_local_debrid_configured() && private_cfg.debrid_cached_only_filter,
        );

        let mut seasons: Vec<u32> = meta
            .videos
            .iter()
            .filter_map(|v| v.series_info.as_ref().map(|s| s.season))
            .collect();
        seasons.sort_unstable();
        seasons.dedup();
        if seasons.len() > 1 && seasons.first() == Some(&0) {
            let zero = seasons.remove(0);
            seasons.push(zero);
        }

        let active_season = self
            .selected_season
            .filter(|s| seasons.contains(s))
            .or_else(|| {
                meta.videos
                    .iter()
                    .find(|v| v.id == active_video_id)
                    .and_then(|v| v.series_info.as_ref().map(|s| s.season))
            })
            .or_else(|| seasons.first().copied());

        let season_episodes: Vec<Video> = match active_season {
            Some(season) => meta
                .videos_by_season(season)
                .into_iter()
                .cloned()
                .collect(),
            None => Vec::new(),
        };

        let active_episode_obj = meta.videos.iter().find(|v| v.id == active_video_id);
        let active_ep_code = active_episode_obj
            .and_then(|ep| {
                ep.series_info
                    .as_ref()
                    .map(|si| format!("S{} E{}", si.season, si.episode))
            })
            .or_else(|| format_movietracker_episode_code(Some(&active_video_id)));

        let episode_header_label = active_episode_obj.map(|ep| match &ep.series_info {
            Some(si) => format!("S{} E{} - {}", si.season, si.episode, ep.title),
            None => ep.title.clone(),
        });

        let filtered_indices: Vec<usize> = all_playable_arc
            .iter()
            .enumerate()
            .filter_map(|(orig_idx, c)| {
                self.stream_filter
                    .matches_candidate(c, has_debrid)
                    .then_some(orig_idx)
            })
            .collect();
        let filtered_total = filtered_indices.len();

        let metahub_bg = meta
            .preview
            .id
            .starts_with("tt")
            .then(|| {
                format!(
                    "https://images.metahub.space/background/medium/{}/img",
                    meta.preview.id
                )
            });
        let metahub_logo = meta
            .preview
            .id
            .starts_with("tt")
            .then(|| {
                format!(
                    "https://images.metahub.space/logo/medium/{}/img",
                    meta.preview.id
                )
            });

        let preblurred_backdrop = enriched
            .as_ref()
            .and_then(|e| e.backdrop_url.as_deref())
            .and_then(|u| self.poster_cache.resolve_ambient_bleed(u))
            .or_else(|| {
                self.poster_cache
                    .resolve_ambient_url(meta.preview.background.as_ref())
            })
            .or_else(|| {
                metahub_bg
                    .as_deref()
                    .and_then(|u| self.poster_cache.resolve_ambient_bleed(u))
            })
            .or_else(|| {
                self.poster_cache
                    .resolve_ambient_url(meta.preview.poster.as_ref())
            });
        let hero_backdrop = enriched
            .as_ref()
            .and_then(|e| e.backdrop_url.as_deref())
            .and_then(|u| {
                self.poster_cache
                    .resolve_hero_backdrop_image(u)
                    .or_else(|| self.poster_cache.resolve_widescreen_image(u))
            })
            .or_else(|| {
                self.poster_cache
                    .resolve_hero_backdrop_url(meta.preview.background.as_ref())
            })
            .or_else(|| {
                metahub_bg
                    .as_deref()
                    .and_then(|u| self.poster_cache.resolve_widescreen_image(u))
            });
        let title_logo_tex = enriched
            .as_ref()
            .and_then(|e| e.logo_url.as_deref())
            .and_then(|u| self.poster_cache.resolve_logo_image(u))
            .or_else(|| self.poster_cache.resolve_logo_url(meta.preview.logo.as_ref()))
            .or_else(|| {
                metahub_logo
                    .as_deref()
                    .and_then(|u| self.poster_cache.resolve_logo_image(u))
            });
        let poster_tex = self
            .poster_cache
            .resolve_poster_url(meta.preview.poster.as_ref());

        let (mut credit_directors, mut credit_cast) = enriched
            .as_ref()
            .filter(|e| !e.directors.is_empty() || !e.cast.is_empty())
            .map(|e| (e.directors.clone(), e.cast.clone()))
            .unwrap_or_else(|| extract_cast_and_directors_from_links(&meta.preview.links));
        if credit_directors.is_empty() && credit_cast.is_empty() {
            let (fallback_dir, fallback_cast) =
                extract_cast_and_directors_from_links(&meta.preview.links);
            credit_directors = fallback_dir;
            credit_cast = fallback_cast;
        }
        let mut combined_credits: Vec<PersonCreditChip> =
            credit_directors.into_iter().take(2).collect();
        combined_credits.extend(credit_cast.into_iter().take(12));

        let imdb = extract_imdb_rating(&meta.preview);
        let genres = extract_genres(&meta.preview, 3);
        let preview_for_toggle = meta.preview.clone();
        let resume_secs = resume_offset_for_library_item(library_item.as_ref(), &active_video_id);
        let lib_status = library_item.as_ref().and_then(classify_library_item_status);
        let has_seasons = !seasons.is_empty();
        let runtime_minutes = meta
            .preview
            .runtime
            .as_deref()
            .and_then(parse_runtime_minutes);
        let seasons_panel = has_seasons.then(|| {
            self.render_series_seasons_panel(
                &meta.preview.r#type,
                &meta.preview.id,
                &seasons,
                active_season,
                &season_episodes,
                &active_video_id,
                meta.preview.runtime.as_deref(),
                library_item.as_ref(),
                watched.as_ref(),
                cx,
            )
        });

        let primary_cta_label = match (&active_ep_code, resume_secs) {
            (Some(code), Some(_)) => format!("Continue {code}"),
            (Some(code), None) => format!("Play {code}"),
            (None, Some(s)) => format!("Resume ({})", format_timestamp_secs(s)),
            (None, None) => "Watch".to_owned(),
        };

        div()
            .id("meta-details-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.meta_scroll_handle)
            .on_scroll(cx.listener(
                move |this, event: &ScrollEvent, _window, cx| {
                    let down_px: f32 = match event.delta {
                        ScrollDelta::Pixels(delta) => -f32::from(delta.y),
                        ScrollDelta::Lines(lines) => -lines.y * 36.0,
                    };
                    if down_px > 0.0
                        && this.visible_stream_limit < filtered_total
                        && scroll_near_bottom(&this.meta_scroll_handle, down_px, 360.0)
                    {
                        this.visible_stream_limit = (this
                            .visible_stream_limit
                            .max(STREAM_PAGE_SIZE)
                            + STREAM_PAGE_SIZE)
                            .min(filtered_total);
                        cx.notify();
                    }
                },
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.meta_addon_dropdown_open {
                        this.meta_addon_dropdown_open = false;
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .w_full()
                    .pb(px(72.0))
                    .flex()
                    .flex_col()
                    .gap(px(36.0))
                    .child(
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .relative()
                            .overflow_hidden()
                            .bg(rgb(placeholder_tint_for_id(&meta.preview.id)))
                            .border_b_1()
                            .border_color(rgba(BORDER_SOFT))
                            .when_some(preblurred_backdrop, |el, bleed| {
                                el.child(
                                    img(bleed)
                                        .size_full()
                                        .absolute()
                                        .inset_0()
                                        .object_fit(gpui::ObjectFit::Cover)
                                        .opacity(0.62),
                                )
                            })
                            .when_some(hero_backdrop, |el, sharp_bg| {
                                el.child(
                                    img(sharp_bg)
                                        .size_full()
                                        .absolute()
                                        .inset_0()
                                        .object_fit(gpui::ObjectFit::Cover)
                                        .opacity(0.24),
                                )
                            })
                            .child(
                                div()
                                    .w_full()
                                    .bg(linear_gradient(
                                        180.0,
                                        linear_color_stop(rgba(0x0b0b0d45), 0.0),
                                        linear_color_stop(rgba(0x0b0b0dfa), 1.0),
                                    ))
                                    .child(
                                        div()
                                            .w_full()
                                            .max_w(px(1440.0))
                                            .mx_auto()
                                            .px(px(32.0))
                                            .pt(px(22.0))
                                            .pb(px(36.0))
                                            .flex()
                                            .flex_col()
                                            .gap(px(24.0))
                                            .child(Self::render_back_button(cx))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_end()
                                                    .gap(px(36.0))
                                                    .child(
                                                        div()
                                                            .w(px(240.0))
                                                            .h(px(360.0))
                                                            .flex_shrink_0()
                                                            .rounded(px(6.0))
                                                            .overflow_hidden()
                                                            .bg(rgb(BG_SURFACE))
                                                            .border_1()
                                                            .border_color(rgba(BORDER_SUBTLE))
                                                            .shadow(poster_card_shadow())
                                                            .when_some(poster_tex, |el, tex| {
                                                                el.child(
                                                                    img(tex)
                                                                        .size_full()
                                                                        .object_fit(
                                                                            gpui::ObjectFit::Cover,
                                                                        ),
                                                                )
                                                            }),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w(px(0.0))
                                                            .flex()
                                                            .flex_col()
                                                            .gap(px(12.0))
                                                            .child(
                                                                div()
                                                                    .flex()
                                                                    .flex_wrap()
                                                                    .items_center()
                                                                    .gap(px(10.0))
                                                                    .text_size(px(13.0))
                                                                    .text_color(rgb(TEXT_SECONDARY))
                                                                    .child(sentence_case_label(
                                                                        &meta.preview.r#type,
                                                                    ))
                                                                    .when(
                                                                        !genres.is_empty(),
                                                                        |el| {
                                                                            el.child(meta_divider())
                                                                                .child(
                                                                                    genres
                                                                                        .join(", "),
                                                                                )
                                                                        },
                                                                    )
                                                                    .when_some(
                                                                        meta.preview
                                                                            .release_info
                                                                            .clone()
                                                                            .filter(|s| {
                                                                                !s.trim().is_empty()
                                                                            }),
                                                                        |el, yr| {
                                                                            el.child(meta_divider())
                                                                                .child(
                                                                                    div()
                                                                                        .font(
                                                                                            mono_font(),
                                                                                        )
                                                                                        .child(yr),
                                                                                )
                                                                        },
                                                                    )
                                                                    .when_some(
                                                                        meta.preview
                                                                            .runtime
                                                                            .clone()
                                                                            .filter(|s| {
                                                                                !s.trim().is_empty()
                                                                            }),
                                                                        |el, rt| {
                                                                            el.child(meta_divider())
                                                                                .child(
                                                                                    div()
                                                                                        .font(
                                                                                            mono_font(),
                                                                                        )
                                                                                        .child(rt),
                                                                                )
                                                                        },
                                                                    )
                                                                    .when_some(imdb, |el, r| {
                                                                        el.child(meta_divider())
                                                                            .child(
                                                                                div()
                                                                                    .flex()
                                                                                    .items_center()
                                                                                    .gap(px(4.5))
                                                                                    .font(
                                                                                        mono_font(),
                                                                                    )
                                                                                    .text_color(
                                                                                        rgb(TEXT_PRIMARY),
                                                                                    )
                                                                                    .child(
                                                                                        svg_icon(
                                                                                            IconName::Star,
                                                                                            12.5,
                                                                                            rgb(ACCENT_AMBER),
                                                                                        ),
                                                                                    )
                                                                                    .child(r),
                                                                            )
                                                                    })
                                                                    .when_some(
                                                                        lib_status,
                                                                        |el, st| {
                                                                            let pill = match st {
                                                                                LibraryStatusTab::Watched => {
                                                                                    semantic_status_pill(
                                                                                        IconName::Check,
                                                                                        "Watched",
                                                                                        STATUS_WATCHED,
                                                                                        0x4ade804d,
                                                                                        0x4ade801c,
                                                                                    )
                                                                                }
                                                                                LibraryStatusTab::Watching => {
                                                                                    semantic_status_pill(
                                                                                        IconName::Play,
                                                                                        "Watching",
                                                                                        STATUS_WATCHING,
                                                                                        0x60a5fa4d,
                                                                                        0x60a5fa1c,
                                                                                    )
                                                                                }
                                                                                LibraryStatusTab::Watchlist => {
                                                                                    semantic_status_pill(
                                                                                        IconName::Bookmark,
                                                                                        "On watchlist",
                                                                                        STATUS_WATCHLIST,
                                                                                        0xc084fc4d,
                                                                                        0xc084fc1c,
                                                                                    )
                                                                                }
                                                                            };
                                                                            el.child(pill)
                                                                        },
                                                                    ),
                                                            )
                                                            .child(if let Some(logo_img) = title_logo_tex {
                                                                img(logo_img)
                                                                    .h(px(84.0))
                                                                    .max_w(px(420.0))
                                                                    .object_fit(
                                                                        gpui::ObjectFit::Contain,
                                                                    )
                                                                    .into_any_element()
                                                            } else {
                                                                div()
                                                                    .font(editorial_serif())
                                                                    .text_size(px(58.0))
                                                                    .line_height(gpui::relative(
                                                                        1.06,
                                                                    ))
                                                                    .text_color(rgb(TEXT_PRIMARY))
                                                                    .child(
                                                                        meta.preview.name.clone(),
                                                                    )
                                                                    .into_any_element()
                                                            })
                                                            .when_some(
                                                                meta.preview.description.clone().or_else(|| {
                                                                    enriched
                                                                        .as_ref()
                                                                        .and_then(|e| e.overview.clone())
                                                                }),
                                                                |el, desc| {
                                                                    el.child(
                                                                        div()
                                                                            .max_w(px(640.0))
                                                                            .text_size(px(14.5))
                                                                            .text_color(rgb(
                                                                                TEXT_PRIMARY,
                                                                            ))
                                                                            .opacity(0.88)
                                                                            .child(desc),
                                                                    )
                                                                },
                                                            )
                                                            .child(
                                                                div()
                                                                    .mt(px(8.0))
                                                                    .flex()
                                                                    .items_center()
                                                                    .gap(px(12.0))
                                                                    .when_some(
                                                                        filtered_indices
                                                                            .first()
                                                                            .copied(),
                                                                        |el, best_play_idx| {
                                                                            let top_cands =
                                                                                Arc::clone(
                                                                                    &all_playable_arc,
                                                                                );
                                                                            let p_type = meta
                                                                                .preview
                                                                                .r#type
                                                                                .clone();
                                                                            let p_id = meta
                                                                                .preview
                                                                                .id
                                                                                .clone();
                                                                            let p_vid =
                                                                                active_video_id
                                                                                    .clone();
                                                                            let p_title = meta
                                                                                .preview
                                                                                .name
                                                                                .clone();
                                                                            let p_ep =
                                                                                episode_header_label
                                                                                    .clone();
                                                                            let p_mreq =
                                                                                meta_request
                                                                                    .clone();

                                                                            el.child(
                                                                                div()
                                                                                    .id("meta-btn-play-best")
                                                                                    .cursor_pointer()
                                                                                    .h(px(46.0))
                                                                                    .px(px(22.0))
                                                                                    .rounded(px(6.0))
                                                                                    .bg(rgb(ACCENT))
                                                                                    .hover(|s| {
                                                                                        s.bg(rgb(
                                                                                            ACCENT_HOVER,
                                                                                        ))
                                                                                    })
                                                                                    .flex()
                                                                                    .items_center()
                                                                                    .gap(px(9.0))
                                                                                    .font(ui_sans(
                                                                                        FontWeight::SEMIBOLD,
                                                                                    ))
                                                                                    .text_size(px(
                                                                                        14.5,
                                                                                    ))
                                                                                    .font_features(
                                                                                        tabular_nums(),
                                                                                    )
                                                                                    .text_color(rgb(
                                                                                        BG_OBSIDIAN,
                                                                                    ))
                                                                                    .on_mouse_down(
                                                                                        MouseButton::Left,
                                                                                        cx.listener(
                                                                                            move |this,
                                                                                                  _,
                                                                                                  _,
                                                                                                  cx| {
                                                                                                let subs = Some(build_subtitles_resource_path(&p_type, &p_vid));
                                                                                                this.start_playback_session(
                                                                                                    p_type.clone(),
                                                                                                    p_id.clone(),
                                                                                                    p_vid.clone(),
                                                                                                    p_title.clone(),
                                                                                                    p_ep.clone(),
                                                                                                    Some(p_mreq.clone()),
                                                                                                    subs,
                                                                                                    Arc::clone(&top_cands),
                                                                                                    best_play_idx,
                                                                                                    resume_secs,
                                                                                                    cx,
                                                                                                );
                                                                                            },
                                                                                        ),
                                                                                    )
                                                                                    .child(svg_icon(
                                                                                        IconName::Play,
                                                                                        15.0,
                                                                                        rgb(BG_OBSIDIAN),
                                                                                    ))
                                                                                    .child(
                                                                                        primary_cta_label,
                                                                                    ),
                                                                            )
                                                                        },
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .id("meta-btn-toggle-lib")
                                                                            .cursor_pointer()
                                                                            .h(px(46.0))
                                                                            .px(px(18.0))
                                                                            .rounded(px(6.0))
                                                                            .bg(rgb(BG_ELEVATED))
                                                                            .border_1()
                                                                            .border_color(
                                                                                if in_library {
                                                                                    rgb(STATUS_WATCHLIST)
                                                                                } else {
                                                                                    rgba(BORDER_SUBTLE)
                                                                                },
                                                                            )
                                                                            .hover(|s| {
                                                                                s.border_color(rgba(
                                                                                    BORDER_STRONG,
                                                                                ))
                                                                            })
                                                                            .flex()
                                                                            .items_center()
                                                                            .gap(px(8.0))
                                                                            .font(ui_sans(
                                                                                FontWeight::MEDIUM,
                                                                            ))
                                                                            .text_size(px(14.0))
                                                                            .text_color(rgb(
                                                                                TEXT_PRIMARY,
                                                                            ))
                                                                            .on_mouse_down(
                                                                                MouseButton::Left,
                                                                                cx.listener(
                                                                                    move |this,
                                                                                          _,
                                                                                          _,
                                                                                          cx| {
                                                                                        this.core
                                                                                            .toggle_library_item(
                                                                                                preview_for_toggle.clone(),
                                                                                            );
                                                                                        cx.notify();
                                                                                    },
                                                                                ),
                                                                            )
                                                                            .child(svg_icon(
                                                                                if in_library {
                                                                                    IconName::Check
                                                                                } else {
                                                                                    IconName::Bookmark
                                                                                },
                                                                                14.5,
                                                                                if in_library {
                                                                                    rgb(STATUS_WATCHLIST)
                                                                                } else {
                                                                                    rgb(TEXT_PRIMARY)
                                                                                },
                                                                            ))
                                                                            .child(if in_library {
                                                                                "On watchlist"
                                                                            } else {
                                                                                "Add to watchlist"
                                                                            }),
                                                                    ),
                                                            ),
                                                    ),
                                            ),
                                    ),
                            ),
                    )
                    .when(!combined_credits.is_empty(), |el| {
                        el.child(
                            div()
                                .w_full()
                                .flex_shrink_0()
                                .max_w(px(1440.0))
                                .mx_auto()
                                .px(px(32.0))
                                .flex()
                                .flex_col()
                                .gap(px(14.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_baseline()
                                        .justify_between()
                                        .child(
                                            div()
                                                .font(editorial_serif())
                                                .text_size(px(26.0))
                                                .text_color(rgb(TEXT_PRIMARY))
                                                .child("Cast & Crew"),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(TEXT_MUTED))
                                                .child("Click any name for filmography"),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap(px(10.0))
                                        .children(combined_credits.into_iter().enumerate().map(
                                            |(idx, chip)| {
                                                let pk = chip.person_key.clone();
                                                let pname = chip.name.clone();
                                                let headshot_tex = chip
                                                    .headshot_url
                                                    .as_deref()
                                                    .and_then(|u| {
                                                        self.poster_cache.resolve_thumbnail_image(u)
                                                    });
                                                let initials = person_initials(&chip.name);
                                                let role_label = chip
                                                    .role_or_character
                                                    .clone()
                                                    .unwrap_or_else(|| {
                                                        if chip.is_director {
                                                            "Director".to_owned()
                                                        } else {
                                                            "Cast".to_owned()
                                                        }
                                                    });

                                                div()
                                                    .id(("meta-cast-chip", idx))
                                                    .cursor_pointer()
                                                    .h(px(54.0))
                                                    .pl(px(8.0))
                                                    .pr(px(14.0))
                                                    .rounded(px(999.0))
                                                    .bg(rgb(BG_SURFACE))
                                                    .border_1()
                                                    .border_color(rgba(BORDER_SUBTLE))
                                                    .hover(|s| {
                                                        s.bg(rgb(BG_ELEVATED))
                                                            .border_color(rgba(BORDER_POSTER_HOVER))
                                                    })
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(10.0))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            this.open_person_details(
                                                                pk.clone(),
                                                                pname.clone(),
                                                                cx,
                                                            );
                                                        }),
                                                    )
                                                    .child(
                                                        div()
                                                            .w(px(38.0))
                                                            .h(px(38.0))
                                                            .flex_shrink_0()
                                                            .rounded(px(999.0))
                                                            .overflow_hidden()
                                                            .bg(rgb(BG_ELEVATED))
                                                            .border_1()
                                                            .border_color(rgba(BORDER_SUBTLE))
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .child(if let Some(tex) = headshot_tex {
                                                                img(tex)
                                                                    .size_full()
                                                                    .object_fit(
                                                                        gpui::ObjectFit::Cover,
                                                                    )
                                                                    .into_any_element()
                                                            } else {
                                                                div()
                                                                    .font(editorial_serif())
                                                                    .text_size(px(15.0))
                                                                    .text_color(rgb(TEXT_SECONDARY))
                                                                    .child(initials)
                                                                    .into_any_element()
                                                            }),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .flex_col()
                                                            .gap(px(1.5))
                                                            .child(
                                                                div()
                                                                    .font(ui_sans(
                                                                        FontWeight::SEMIBOLD,
                                                                    ))
                                                                    .text_size(px(13.0))
                                                                    .text_color(rgb(TEXT_PRIMARY))
                                                                    .max_w(px(180.0))
                                                                    .truncate()
                                                                    .child(chip.name),
                                                            )
                                                            .child(
                                                                div()
                                                                    .font(mono_font())
                                                                    .text_size(px(11.0))
                                                                    .text_color(
                                                                        if chip.is_director {
                                                                            rgb(ACCENT)
                                                                        } else {
                                                                            rgb(TEXT_MUTED)
                                                                        },
                                                                    )
                                                                    .max_w(px(180.0))
                                                                    .truncate()
                                                                    .child(role_label),
                                                            ),
                                                    )
                                            },
                                        )),
                                ),
                        )
                    })
                    .child(
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .max_w(px(1440.0))
                            .mx_auto()
                            .px(px(32.0))
                            .flex()
                            .items_start()
                            .gap(px(32.0))
                            .when_some(seasons_panel, |el, panel| el.child(panel))
                            .child(self.render_stream_picker_panel(
                                &meta.preview.r#type,
                                &meta.preview.id,
                                &active_video_id,
                                &meta.preview.name,
                                episode_header_label,
                                runtime_minutes,
                                meta_request,
                                resume_secs,
                                has_debrid,
                                all_playable_arc,
                                available_addons_arc,
                                filtered_indices,
                                stream_addons_ready,
                                stream_addons_total,
                                has_stream_request,
                                has_stream_addons,
                                cx,
                            )),
                    ),
            )
            .into_any_element()
    }

    fn render_loading_meta_details_view(
        &self,
        route_type: &str,
        route_id: &str,
        is_meta_loading: bool,
        cached_preview: Option<CachedTitlePreview>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let status_text = if is_meta_loading {
            format!("Loading details and streams for {route_type}...")
        } else {
            format!("No metadata addon returned details for {route_id}.")
        };

        let Some(preview) = cached_preview else {
            return div()
                .id("meta-details-loading")
                .size_full()
                .overflow_y_scroll()
                .child(
                    div()
                        .w_full()
                        .max_w(px(1440.0))
                        .mx_auto()
                        .p(px(32.0))
                        .flex()
                        .flex_col()
                        .gap(px(20.0))
                        .child(Self::render_back_button(cx))
                        .child(
                            div()
                                .p(px(32.0))
                                .rounded(px(10.0))
                                .bg(rgb(BG_SURFACE))
                                .border_1()
                                .border_color(rgba(BORDER_SUBTLE))
                                .font(editorial_serif())
                                .text_size(px(28.0))
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(status_text),
                        ),
                )
                .into_any_element();
        };

        let metahub_bg = format!("https://images.metahub.space/background/medium/{}/img", preview.id);
        let preblurred_backdrop = self
            .poster_cache
            .resolve_ambient_url(preview.background.as_ref())
            .or_else(|| self.poster_cache.resolve_ambient_bleed(&metahub_bg))
            .or_else(|| self.poster_cache.resolve_ambient_url(preview.poster.as_ref()));
        let hero_backdrop = self
            .poster_cache
            .resolve_hero_backdrop_url(preview.background.as_ref())
            .or_else(|| self.poster_cache.resolve_widescreen_image(&metahub_bg));
        let poster_tex = self.poster_cache.resolve_poster_url(preview.poster.as_ref());

        div()
            .id("meta-details-loading")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .pb(px(72.0))
                    .flex()
                    .flex_col()
                    .gap(px(36.0))
                    .child(
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .relative()
                            .overflow_hidden()
                            .bg(rgb(placeholder_tint_for_id(&preview.id)))
                            .border_b_1()
                            .border_color(rgba(BORDER_SOFT))
                            .when_some(preblurred_backdrop, |el, bleed| {
                                el.child(
                                    img(bleed)
                                        .size_full()
                                        .absolute()
                                        .inset_0()
                                        .object_fit(gpui::ObjectFit::Cover)
                                        .opacity(0.62),
                                )
                            })
                            .when_some(hero_backdrop, |el, sharp_bg| {
                                el.child(
                                    img(sharp_bg)
                                        .size_full()
                                        .absolute()
                                        .inset_0()
                                        .object_fit(gpui::ObjectFit::Cover)
                                        .opacity(0.24),
                                )
                            })
                            .child(
                                div()
                                    .w_full()
                                    .bg(linear_gradient(
                                        180.0,
                                        linear_color_stop(rgba(0x0b0b0d45), 0.0),
                                        linear_color_stop(rgba(0x0b0b0dfa), 1.0),
                                    ))
                                    .child(
                                        div()
                                            .w_full()
                                            .max_w(px(1440.0))
                                            .mx_auto()
                                            .px(px(32.0))
                                            .pt(px(22.0))
                                            .pb(px(36.0))
                                            .flex()
                                            .flex_col()
                                            .gap(px(24.0))
                                            .child(Self::render_back_button(cx))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_end()
                                                    .gap(px(36.0))
                                                    .child(
                                                        div()
                                                            .w(px(240.0))
                                                            .h(px(360.0))
                                                            .flex_shrink_0()
                                                            .rounded(px(6.0))
                                                            .overflow_hidden()
                                                            .bg(rgb(BG_SURFACE))
                                                            .border_1()
                                                            .border_color(rgba(BORDER_SUBTLE))
                                                            .shadow(poster_card_shadow())
                                                            .when_some(poster_tex, |el, tex| {
                                                                el.child(
                                                                    img(tex)
                                                                        .size_full()
                                                                        .object_fit(
                                                                            gpui::ObjectFit::Cover,
                                                                        ),
                                                                )
                                                            }),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w(px(0.0))
                                                            .flex()
                                                            .flex_col()
                                                            .gap(px(12.0))
                                                            .child(
                                                                div()
                                                                    .flex()
                                                                    .flex_wrap()
                                                                    .items_center()
                                                                    .gap(px(10.0))
                                                                    .text_size(px(13.0))
                                                                    .text_color(rgb(TEXT_SECONDARY))
                                                                    .child(sentence_case_label(
                                                                        &preview.r#type,
                                                                    ))
                                                                    .when(
                                                                        !preview.genres.is_empty(),
                                                                        |el| {
                                                                            el.child(meta_divider())
                                                                                .child(
                                                                                    preview
                                                                                        .genres
                                                                                        .join(", "),
                                                                                )
                                                                        },
                                                                    )
                                                                    .when_some(
                                                                        preview
                                                                            .release_info
                                                                            .filter(|s| {
                                                                                !s.trim().is_empty()
                                                                            }),
                                                                        |el, yr| {
                                                                            el.child(meta_divider())
                                                                                .child(
                                                                                    div()
                                                                                        .font(
                                                                                            mono_font(),
                                                                                        )
                                                                                        .child(yr),
                                                                                )
                                                                        },
                                                                    )
                                                                    .when_some(
                                                                        preview
                                                                            .imdb_rating,
                                                                        |el, r| {
                                                                            el.child(meta_divider())
                                                                                .child(
                                                                                    div()
                                                                                        .flex()
                                                                                        .items_center()
                                                                                        .gap(px(4.5))
                                                                                        .font(
                                                                                            mono_font(),
                                                                                        )
                                                                                        .text_color(
                                                                                            rgb(TEXT_PRIMARY),
                                                                                        )
                                                                                        .child(svg_icon(
                                                                                            IconName::Star,
                                                                                            12.5,
                                                                                            rgb(ACCENT_AMBER),
                                                                                        ))
                                                                                        .child(r),
                                                                                )
                                                                        },
                                                                    ),
                                                            )
                                                            .child(
                                                                div()
                                                                    .font(editorial_serif())
                                                                    .text_size(px(58.0))
                                                                    .line_height(gpui::relative(
                                                                        1.06,
                                                                    ))
                                                                    .text_color(rgb(TEXT_PRIMARY))
                                                                    .child(preview.name),
                                                            )
                                                            .when_some(
                                                                preview.description,
                                                                |el, desc| {
                                                                    el.child(
                                                                        div()
                                                                            .max_w(px(640.0))
                                                                            .text_size(px(14.5))
                                                                            .text_color(rgb(
                                                                                TEXT_PRIMARY,
                                                                            ))
                                                                            .opacity(0.88)
                                                                            .child(desc),
                                                                    )
                                                                },
                                                            ),
                                                    ),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .max_w(px(1440.0))
                            .mx_auto()
                            .px(px(32.0))
                            .child(
                                div()
                                    .p(px(32.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(BG_SURFACE))
                                    .border_1()
                                    .border_color(rgba(BORDER_SUBTLE))
                                    .font(editorial_serif())
                                    .text_size(px(24.0))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(status_text),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_back_button(cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().child(
            div()
                .id("btn-details-back")
                .cursor_pointer()
                .h(px(34.0))
                .px(px(12.0))
                .rounded(px(6.0))
                .bg(rgba(0x141417cc))
                .border_1()
                .border_color(rgba(BORDER_SUBTLE))
                .hover(|s| s.bg(rgb(BG_ELEVATED)).border_color(rgba(BORDER_STRONG)))
                .flex()
                .items_center()
                .gap(px(6.0))
                .font(ui_sans(FontWeight::MEDIUM))
                .text_size(px(13.0))
                .text_color(rgb(TEXT_PRIMARY))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.navigate_back(cx);
                    }),
                )
                .child(svg_icon(IconName::ChevronLeft, 14.0, rgb(TEXT_SECONDARY)))
                .child("Back"),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_series_seasons_panel(
        &mut self,
        type_name: &str,
        meta_id: &str,
        seasons: &[u32],
        active_season: Option<u32>,
        episodes: &[Video],
        active_video_id: &str,
        series_runtime: Option<&str>,
        library_item: Option<&LibraryItem>,
        watched: Option<&WatchedBitField>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let season_heading = match active_season {
            Some(0) => "Specials".to_owned(),
            Some(n) => format!("Season {n}"),
            None => "Episodes".to_owned(),
        };

        let runtime_label = series_runtime
            .and_then(parse_runtime_minutes)
            .map(|mins| format!("{mins}m"))
            .or_else(|| {
                series_runtime
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            });

        let episode_rows: Vec<EpisodeRowData> = episodes
            .iter()
            .enumerate()
            .map(|(idx, ep)| {
                let is_selected = ep.id == active_video_id;
                let is_watched = watched.is_some_and(|w| w.get_video(&ep.id));
                let ep_code = ep
                    .series_info
                    .as_ref()
                    .map(|s| format!("S{:02} E{:02}", s.season, s.episode))
                    .unwrap_or_else(|| format!("E{:02}", idx + 1));
                let title = if ep.title.trim().is_empty() {
                    format!("Episode {}", idx + 1)
                } else {
                    ep.title.clone()
                };
                let thumb_url = ep
                    .thumbnail
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .or_else(|| {
                        meta_id.starts_with("tt").then(|| {
                            format!(
                                "https://images.metahub.space/episode/medium/{meta_id}/{}/img",
                                ep.id
                            )
                        })
                    });
                let air_date = ep.released.map(format_episode_air_date);
                let synopsis = ep
                    .overview
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                let progress_fraction = if is_watched {
                    Some(1.0)
                } else if let Some(li) = library_item {
                    if li.state.video_id.as_deref() == Some(ep.id.as_str())
                        && li.state.time_offset > 0
                        && li.state.duration > 0
                    {
                        Some(
                            (li.state.time_offset as f32 / li.state.duration as f32)
                                .clamp(0.02, 1.0),
                        )
                    } else {
                        None
                    }
                } else {
                    None
                };

                EpisodeRowData {
                    video_id: ep.id.clone(),
                    ep_code,
                    title,
                    thumb_url,
                    air_date,
                    runtime_label: runtime_label.clone(),
                    synopsis,
                    is_selected,
                    is_watched,
                    progress_fraction,
                }
            })
            .collect();

        let ep_count = episode_rows.len();
        let scroll_key = (meta_id.to_owned(), active_video_id.to_owned());
        if self.episode_list_scrolled_key.as_ref() != Some(&scroll_key) {
            if let Some(active_idx) = episode_rows.iter().position(|row| row.is_selected) {
                self.episode_list_scroll_handle
                    .scroll_to_item(active_idx, gpui::ScrollStrategy::Center);
            }
            self.episode_list_scrolled_key = Some(scroll_key);
        }

        let episodes_arc = Arc::new(episode_rows);
        let sel_type = type_name.to_owned();
        let sel_id = meta_id.to_owned();
        let list_height_px = if ep_count == 0 {
            96.0
        } else {
            (ep_count as f32 * 112.0).min(560.0)
        };

        div()
            .w(px(468.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .child(
                        div()
                            .font(editorial_serif())
                            .text_size(px(30.0))
                            .text_color(rgb(TEXT_PRIMARY))
                            .child(season_heading),
                    )
                    .child(
                        div()
                            .font(mono_font())
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
                            .child(format!("{ep_count} episodes")),
                    ),
            )
            .child(
                div()
                    .id("season-pills-scroll")
                    .w_full()
                    .overflow_x_scroll()
                    .pb(px(2.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .children(seasons.iter().copied().enumerate().map(|(idx, s_num)| {
                        let is_active = active_season == Some(s_num);
                        let label = if s_num == 0 {
                            "Specials".to_owned()
                        } else {
                            format!("Season {s_num}")
                        };

                        div()
                            .id(("season-pill", idx))
                            .cursor_pointer()
                            .flex_shrink_0()
                            .h(px(32.0))
                            .px(px(14.0))
                            .rounded(px(999.0))
                            .bg(if is_active {
                                rgba(ACCENT_SOFT)
                            } else {
                                rgb(BG_SURFACE)
                            })
                            .border_1()
                            .border_color(if is_active {
                                rgb(ACCENT)
                            } else {
                                rgba(BORDER_SUBTLE)
                            })
                            .hover(|s| {
                                s.border_color(if is_active {
                                    rgb(ACCENT)
                                } else {
                                    rgba(BORDER_STRONG)
                                })
                            })
                            .flex()
                            .items_center()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(px(13.0))
                            .font_features(tabular_nums())
                            .text_color(if is_active {
                                rgb(ACCENT)
                            } else {
                                rgb(TEXT_SECONDARY)
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.selected_season = Some(s_num);
                                    this.episode_list_scroll_handle
                                        .0
                                        .borrow()
                                        .base_handle
                                        .set_offset(point(px(0.0), px(0.0)));
                                    cx.notify();
                                }),
                            )
                            .child(label)
                    })),
            )
            .child(if ep_count == 0 {
                div()
                    .w_full()
                    .h(px(list_height_px))
                    .rounded(px(8.0))
                    .bg(rgb(BG_SURFACE))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(rgb(TEXT_MUTED))
                    .child("No episodes listed for this season.")
                    .into_any_element()
            } else {
                div()
                    .w_full()
                    .h(px(list_height_px))
                    .flex()
                    .flex_col()
                    .child(
                        gpui::uniform_list(
                            "meta-season-episodes",
                            ep_count,
                            cx.processor(move |this, visible_rows: Range<usize>, _window, cx| {
                                visible_rows
                                    .map(|idx| {
                                        let ep = &episodes_arc[idx];
                                        let thumb_tex = ep.thumb_url.as_deref().and_then(|u| {
                                            this.poster_cache.resolve_widescreen_image(u)
                                        });
                                        let click_type = sel_type.clone();
                                        let click_id = sel_id.clone();
                                        let click_vid = ep.video_id.clone();
                                        let grp: SharedString =
                                            format!("episode-card-grp-{idx}").into();

                                        div()
                                            .w_full()
                                            .h(px(112.0))
                                            .pb(px(8.0))
                                            .child(
                                                div()
                                                    .id(("episode-row", idx))
                                                    .group(grp.clone())
                                                    .cursor_pointer()
                                                    .w_full()
                                                    .h(px(104.0))
                                                    .p(px(9.0))
                                                    .rounded(px(8.0))
                                                    .bg(if ep.is_selected {
                                                        rgba(ACCENT_SOFT)
                                                    } else {
                                                        rgb(BG_SURFACE)
                                                    })
                                                    .border_1()
                                                    .border_color(if ep.is_selected {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgba(BORDER_SUBTLE)
                                                    })
                                                    .hover(|s| {
                                                        if ep.is_selected {
                                                            s.bg(rgba(ACCENT_SOFT))
                                                        } else {
                                                            s.bg(rgb(BG_ELEVATED))
                                                                .border_color(rgba(BORDER_STRONG))
                                                        }
                                                    })
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(12.0))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            this.select_episode_for_streams(
                                                                click_type.clone(),
                                                                click_id.clone(),
                                                                click_vid.clone(),
                                                                cx,
                                                            );
                                                        }),
                                                    )
                                                    .child(
                                                        div()
                                                            .w(px(152.0))
                                                            .h(px(86.0))
                                                            .flex_shrink_0()
                                                            .rounded(px(6.0))
                                                            .overflow_hidden()
                                                            .relative()
                                                            .bg(rgb(placeholder_tint_for_id(
                                                                &ep.video_id,
                                                            )))
                                                            .border_1()
                                                            .border_color(rgba(BORDER_SOFT))
                                                            .child(if let Some(tex) = thumb_tex {
                                                                img(tex)
                                                                    .size_full()
                                                                    .object_fit(
                                                                        gpui::ObjectFit::Cover,
                                                                    )
                                                                    .into_any_element()
                                                            } else {
                                                                div()
                                                                    .size_full()
                                                                    .flex()
                                                                    .items_center()
                                                                    .justify_center()
                                                                    .font(mono_font())
                                                                    .text_xs()
                                                                    .text_color(rgb(TEXT_MUTED))
                                                                    .child(ep.ep_code.clone())
                                                                    .into_any_element()
                                                            })
                                                            .child(
                                                                div()
                                                                    .absolute()
                                                                    .inset_0()
                                                                    .flex()
                                                                    .items_center()
                                                                    .justify_center()
                                                                    .bg(if ep.is_selected {
                                                                        rgba(0x0b0b0d55)
                                                                    } else {
                                                                        rgba(0x00000000)
                                                                    })
                                                                    .opacity(if ep.is_selected {
                                                                        1.0
                                                                    } else {
                                                                        0.0
                                                                    })
                                                                    .group_hover(grp, |s| {
                                                                        s.opacity(1.0)
                                                                            .bg(rgba(0x0b0b0d66))
                                                                    })
                                                                    .child(
                                                                        div()
                                                                            .w(px(28.0))
                                                                            .h(px(28.0))
                                                                            .rounded(px(999.0))
                                                                            .bg(if ep.is_selected {
                                                                                rgb(ACCENT)
                                                                            } else {
                                                                                rgba(0x141417e0)
                                                                            })
                                                                            .border_1()
                                                                            .border_color(
                                                                                if ep.is_selected {
                                                                                    rgb(ACCENT)
                                                                                } else {
                                                                                    rgba(
                                                                                        BORDER_STRONG,
                                                                                    )
                                                                                },
                                                                            )
                                                                            .flex()
                                                                            .items_center()
                                                                            .justify_center()
                                                                            .child(svg_icon(
                                                                                IconName::Play,
                                                                                12.0,
                                                                                if ep.is_selected {
                                                                                    rgb(BG_OBSIDIAN)
                                                                                } else {
                                                                                    rgb(
                                                                                        TEXT_PRIMARY,
                                                                                    )
                                                                                },
                                                                            )),
                                                                    ),
                                                            )
                                                            .when_some(
                                                                ep.progress_fraction,
                                                                |el, frac| {
                                                                    let bar_color =
                                                                        if ep.is_watched {
                                                                            rgb(STATUS_WATCHED)
                                                                        } else {
                                                                            rgb(ACCENT)
                                                                        };
                                                                    el.child(
                                                                        div()
                                                                            .absolute()
                                                                            .bottom_0()
                                                                            .left_0()
                                                                            .right_0()
                                                                            .h(px(2.5))
                                                                            .bg(rgba(0xffffff2e))
                                                                            .child(
                                                                                div()
                                                                                    .h_full()
                                                                                    .w(relative(
                                                                                        frac,
                                                                                    ))
                                                                                    .bg(bar_color),
                                                                            ),
                                                                    )
                                                                },
                                                            ),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w(px(0.0))
                                                            .h_full()
                                                            .py(px(2.0))
                                                            .flex()
                                                            .flex_col()
                                                            .justify_between()
                                                            .child(
                                                                div()
                                                                    .w_full()
                                                                    .min_w(px(0.0))
                                                                    .flex()
                                                                    .items_center()
                                                                    .gap(px(8.0))
                                                                    .child(
                                                                        div()
                                                                            .flex_shrink_0()
                                                                            .px(px(6.0))
                                                                            .py(px(1.5))
                                                                            .rounded(px(4.0))
                                                                            .bg(if ep.is_selected {
                                                                                rgba(ACCENT_SOFT)
                                                                            } else {
                                                                                rgb(BG_ELEVATED)
                                                                            })
                                                                            .font(mono_font())
                                                                            .font_weight(
                                                                                FontWeight::SEMIBOLD,
                                                                            )
                                                                            .text_size(px(11.0))
                                                                            .text_color(
                                                                                if ep.is_selected {
                                                                                    rgb(ACCENT)
                                                                                } else {
                                                                                    rgb(
                                                                                        TEXT_SECONDARY,
                                                                                    )
                                                                                },
                                                                            )
                                                                            .child(
                                                                                ep.ep_code.clone(),
                                                                            ),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .flex_1()
                                                                            .min_w(px(0.0))
                                                                            .font(ui_sans(
                                                                                FontWeight::BOLD,
                                                                            ))
                                                                            .text_size(px(13.5))
                                                                            .text_color(rgb(
                                                                                TEXT_PRIMARY,
                                                                            ))
                                                                            .truncate()
                                                                            .child(
                                                                                ep.title.clone(),
                                                                            ),
                                                                    )
                                                                    .when(ep.is_watched, |row| {
                                                                        row.child(
                                                                            div()
                                                                                .flex_shrink_0()
                                                                                .px(px(7.0))
                                                                                .py(px(2.0))
                                                                                .rounded(px(999.0))
                                                                                .bg(rgba(0x4ade801c))
                                                                                .border_1()
                                                                                .border_color(rgba(
                                                                                    0x4ade804d,
                                                                                ))
                                                                                .flex()
                                                                                .items_center()
                                                                                .gap(px(4.0))
                                                                                .text_size(px(10.5))
                                                                                .font_weight(
                                                                                    FontWeight::MEDIUM,
                                                                                )
                                                                                .text_color(rgb(
                                                                                    STATUS_WATCHED,
                                                                                ))
                                                                                .child(svg_icon(
                                                                                    IconName::Check,
                                                                                    10.0,
                                                                                    rgb(
                                                                                        STATUS_WATCHED,
                                                                                    ),
                                                                                ))
                                                                                .child("Watched"),
                                                                        )
                                                                    }),
                                                            )
                                                            .child(
                                                                div()
                                                                    .w_full()
                                                                    .min_w(px(0.0))
                                                                    .flex()
                                                                    .items_center()
                                                                    .gap(px(8.0))
                                                                    .text_xs()
                                                                    .font_features(tabular_nums())
                                                                    .text_color(rgb(TEXT_MUTED))
                                                                    .when_some(
                                                                        ep.air_date.clone(),
                                                                        |row, date| {
                                                                            row.child(
                                                                                div()
                                                                                    .font(mono_font())
                                                                                    .text_size(px(
                                                                                        11.0,
                                                                                    ))
                                                                                    .child(date),
                                                                            )
                                                                        },
                                                                    )
                                                                    .when_some(
                                                                        ep.runtime_label.clone(),
                                                                        |row, rt| {
                                                                            row.child(
                                                                                div()
                                                                                    .px(px(6.0))
                                                                                    .py(px(1.0))
                                                                                    .rounded(px(
                                                                                        4.0,
                                                                                    ))
                                                                                    .bg(rgb(
                                                                                        BG_ELEVATED,
                                                                                    ))
                                                                                    .font(
                                                                                        mono_font(),
                                                                                    )
                                                                                    .text_size(px(
                                                                                        10.5,
                                                                                    ))
                                                                                    .text_color(rgb(
                                                                                        TEXT_SECONDARY,
                                                                                    ))
                                                                                    .child(rt),
                                                                            )
                                                                        },
                                                                    ),
                                                            )
                                                            .child(
                                                                div()
                                                                    .w_full()
                                                                    .min_w(px(0.0))
                                                                    .text_xs()
                                                                    .text_color(
                                                                        if ep.synopsis.is_some() {
                                                                            rgb(TEXT_SECONDARY)
                                                                        } else {
                                                                            rgb(TEXT_MUTED)
                                                                        },
                                                                    )
                                                                    .truncate()
                                                                    .child(
                                                                        ep.synopsis
                                                                            .clone()
                                                                            .unwrap_or_else(|| {
                                                                                "No synopsis available."
                                                                                    .to_owned()
                                                                            }),
                                                                    ),
                                                            ),
                                                    ),
                                            )
                                    })
                                    .collect()
                            }),
                        )
                        .track_scroll(&self.episode_list_scroll_handle)
                        .flex_1()
                        .w_full()
                        .min_h(px(0.0)),
                    )
                    .into_any_element()
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn render_stream_picker_panel(
        &self,
        type_name: &str,
        meta_id: &str,
        active_video_id: &str,
        title: &str,
        episode_label: Option<String>,
        runtime_minutes: Option<u32>,
        meta_request: ResourceRequest,
        resume_secs: Option<f64>,
        has_debrid: bool,
        all_playable_arc: Arc<Vec<PlaybackCandidate>>,
        available_addons_arc: Arc<Vec<(String, String, usize)>>,
        filtered_indices: Vec<usize>,
        addons_ready: usize,
        addons_total: usize,
        has_stream_request: bool,
        has_stream_addons: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let total_unfiltered = all_playable_arc.len();
        let filtered_total = filtered_indices.len();
        let stream_limit = self.visible_stream_limit.max(STREAM_PAGE_SIZE);
        let visible_count = filtered_total.min(stream_limit);
        let remaining_streams = filtered_total.saturating_sub(visible_count);
        let is_unfiltered = self.stream_filter.is_unfiltered(has_debrid);
        let addon_label = self
            .stream_filter
            .addon_dropdown_label(&available_addons_arc);
        let addon_filter_active = !self.stream_filter.selected_addons.is_empty();
        let addon_dropdown_open = self.meta_addon_dropdown_open;

        div()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(14.0))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .font(editorial_serif())
                                    .text_size(px(30.0))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .child("Streams"),
                            )
                            .when_some(episode_label.clone(), |el, ep| {
                                el.child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(ACCENT))
                                        .child(ep),
                                )
                            })
                            .child(
                                div()
                                    .font(mono_font())
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(format!(
                                        "{filtered_total} of {total_unfiltered} ({addons_ready}/{addons_total} sources)"
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.0))
                            .child(render_stream_filter_toggle(
                                "stream-filter-all",
                                "All",
                                None,
                                is_unfiltered,
                                cx.listener(|this, _, _, cx| {
                                    this.stream_filter.reset();
                                    this.meta_addon_dropdown_open = false;
                                    this.expanded_stream_rows.clear();
                                    this.visible_stream_limit = STREAM_PAGE_SIZE;
                                    cx.notify();
                                }),
                            ))
                            .when(!available_addons_arc.is_empty(), |bar| {
                                let addons_for_menu = Arc::clone(&available_addons_arc);
                                bar.child(
                                    div()
                                        .relative()
                                        .child(
                                            div()
                                                .id("stream-addon-dropdown-btn")
                                                .cursor_pointer()
                                                .h(px(32.0))
                                                .px(px(13.0))
                                                .rounded(px(999.0))
                                                .bg(if addon_filter_active || addon_dropdown_open {
                                                    rgba(ACCENT_SOFT)
                                                } else {
                                                    rgb(BG_SURFACE)
                                                })
                                                .border_1()
                                                .border_color(
                                                    if addon_filter_active || addon_dropdown_open {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgba(BORDER_SUBTLE)
                                                    },
                                                )
                                                .hover(|s| {
                                                    s.border_color(
                                                        if addon_filter_active || addon_dropdown_open
                                                        {
                                                            rgb(ACCENT)
                                                        } else {
                                                            rgba(BORDER_STRONG)
                                                        },
                                                    )
                                                })
                                                .flex()
                                                .items_center()
                                                .gap(px(6.0))
                                                .font(ui_sans(FontWeight::MEDIUM))
                                                .text_size(px(12.5))
                                                .text_color(
                                                    if addon_filter_active || addon_dropdown_open {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgb(TEXT_SECONDARY)
                                                    },
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.meta_addon_dropdown_open =
                                                            !this.meta_addon_dropdown_open;
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(svg_icon(
                                                    IconName::Layers,
                                                    11.5,
                                                    if addon_filter_active || addon_dropdown_open {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgb(TEXT_SECONDARY)
                                                    },
                                                ))
                                                .child(addon_label)
                                                .child(svg_icon(
                                                    IconName::ChevronDown,
                                                    11.5,
                                                    if addon_filter_active || addon_dropdown_open {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgb(TEXT_MUTED)
                                                    },
                                                )),
                                        )
                                        .when(addon_dropdown_open, |wrap| {
                                            wrap.child(
                                                deferred(
                                                    div()
                                                        .id("stream-addon-dropdown-menu")
                                                        .occlude()
                                                        .absolute()
                                                        .top(px(38.0))
                                                        .right_0()
                                                        .w(px(284.0))
                                                        .max_h(px(320.0))
                                                        .overflow_y_scroll()
                                                        .p(px(8.0))
                                                        .rounded(px(10.0))
                                                        .bg(rgba(BG_POPOVER_GLASS))
                                                        .border_1()
                                                        .border_color(rgba(BORDER_SUBTLE))
                                                        .shadow(glass_capsule_shadow())
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            |_, _, cx| {
                                                                cx.stop_propagation();
                                                            },
                                                        )
                                                        .child(
                                                            div()
                                                                .w_full()
                                                                .flex()
                                                                .flex_col()
                                                                .gap(px(3.0))
                                                                .child(
                                                                    div()
                                                                        .id("stream-addon-opt-all")
                                                                        .flex_shrink_0()
                                                                        .cursor_pointer()
                                                                        .px(px(10.0))
                                                                        .py(px(8.0))
                                                                        .rounded(px(6.0))
                                                                        .bg(if !addon_filter_active {
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
                                                                            cx.listener(|this, _, _, cx| {
                                                                                cx.stop_propagation();
                                                                                this.stream_filter
                                                                                    .selected_addons
                                                                                    .clear();
                                                                                this.expanded_stream_rows
                                                                                    .clear();
                                                                                this.visible_stream_limit =
                                                                                    STREAM_PAGE_SIZE;
                                                                                cx.notify();
                                                                            }),
                                                                        )
                                                                        .child(
                                                                            div()
                                                                                .flex()
                                                                                .items_center()
                                                                                .gap(px(8.0))
                                                                                .child(svg_icon(
                                                                                    IconName::Check,
                                                                                    12.5,
                                                                                    if !addon_filter_active {
                                                                                        rgb(ACCENT)
                                                                                    } else {
                                                                                        rgba(0x00000000)
                                                                                    },
                                                                                ))
                                                                                .child(
                                                                                    div()
                                                                                        .font(ui_sans(
                                                                                            FontWeight::MEDIUM,
                                                                                        ))
                                                                                        .text_xs()
                                                                                        .text_color(
                                                                                            if !addon_filter_active
                                                                                            {
                                                                                                rgb(ACCENT)
                                                                                            } else {
                                                                                                rgb(TEXT_PRIMARY)
                                                                                            },
                                                                                        )
                                                                                        .child("All addons"),
                                                                                ),
                                                                        )
                                                                        .child(
                                                                            div()
                                                                                .font(mono_font())
                                                                                .text_xs()
                                                                                .text_color(rgb(TEXT_MUTED))
                                                                                .child(format!(
                                                                                    "{total_unfiltered}"
                                                                                )),
                                                                        ),
                                                                )
                                                                .children(
                                                                    addons_for_menu
                                                                        .iter()
                                                                        .cloned()
                                                                        .enumerate()
                                                                        .map(
                                                                            |(
                                                                                a_idx,
                                                                                (a_key, a_name, count),
                                                                            )| {
                                                                                let selected = self
                                                                                    .stream_filter
                                                                                    .is_addon_selected(
                                                                                        &a_key,
                                                                                    );
                                                                                div()
                                                                                    .id((
                                                                                        "stream-addon-opt",
                                                                                        a_idx,
                                                                                    ))
                                                                                    .flex_shrink_0()
                                                                                    .cursor_pointer()
                                                                                    .px(px(10.0))
                                                                                    .py(px(8.0))
                                                                                    .rounded(px(6.0))
                                                                                    .bg(if selected {
                                                                                        rgba(ACCENT_SOFT)
                                                                                    } else {
                                                                                        rgba(0x00000000)
                                                                                    })
                                                                                    .hover(|s| {
                                                                                        s.bg(rgb(
                                                                                            BG_ELEVATED,
                                                                                        ))
                                                                                    })
                                                                                    .flex()
                                                                                    .items_center()
                                                                                    .justify_between()
                                                                                    .gap(px(8.0))
                                                                                    .on_mouse_down(
                                                                                        MouseButton::Left,
                                                                                        cx.listener(
                                                                                            move |this,
                                                                                                  _,
                                                                                                  _,
                                                                                                  cx| {
                                                                                                cx.stop_propagation();
                                                                                                this.stream_filter
                                                                                                    .toggle_addon(
                                                                                                        &a_key,
                                                                                                    );
                                                                                                this.expanded_stream_rows
                                                                                                    .clear();
                                                                                                this.visible_stream_limit =
                                                                                                    STREAM_PAGE_SIZE;
                                                                                                cx.notify();
                                                                                            },
                                                                                        ),
                                                                                    )
                                                                                    .child(
                                                                                        div()
                                                                                            .flex_1()
                                                                                            .min_w(px(0.0))
                                                                                            .flex()
                                                                                            .items_center()
                                                                                            .gap(px(8.0))
                                                                                            .child(svg_icon(
                                                                                                IconName::Check,
                                                                                                12.5,
                                                                                                if selected
                                                                                                {
                                                                                                    rgb(ACCENT)
                                                                                                } else {
                                                                                                    rgba(0x00000000)
                                                                                                },
                                                                                            ))
                                                                                            .child(
                                                                                                div()
                                                                                                    .flex_1()
                                                                                                    .min_w(px(0.0))
                                                                                                    .font(ui_sans(
                                                                                                        FontWeight::MEDIUM,
                                                                                                    ))
                                                                                                    .text_xs()
                                                                                                    .text_color(
                                                                                                        if selected {
                                                                                                            rgb(ACCENT)
                                                                                                        } else {
                                                                                                            rgb(TEXT_PRIMARY)
                                                                                                        },
                                                                                                    )
                                                                                                    .truncate()
                                                                                                    .child(a_name),
                                                                                            ),
                                                                                    )
                                                                                    .child(
                                                                                        div()
                                                                                            .font(
                                                                                                mono_font(),
                                                                                            )
                                                                                            .text_xs()
                                                                                            .text_color(rgb(
                                                                                                TEXT_MUTED,
                                                                                            ))
                                                                                            .child(format!(
                                                                                                "{count}"
                                                                                            )),
                                                                                    )
                                                                            },
                                                                        ),
                                                                ),
                                                        ),
                                                )
                                                .with_priority(10),
                                            )
                                        }),
                                )
                            })
                            .children(StreamResolutionChoice::ALL.into_iter().enumerate().map(
                                |(idx, res)| {
                                    let active = self.stream_filter.resolutions.contains(&res);
                                    render_stream_filter_toggle(
                                        ("stream-filter-res", idx),
                                        res.label(),
                                        None,
                                        active,
                                        cx.listener(move |this, _, _, cx| {
                                            this.stream_filter.toggle_resolution(res);
                                            this.expanded_stream_rows.clear();
                                            this.visible_stream_limit = STREAM_PAGE_SIZE;
                                            cx.notify();
                                        }),
                                    )
                                },
                            ))
                            .child(render_stream_filter_toggle(
                                "stream-filter-hdr",
                                "HDR",
                                None,
                                self.stream_filter.hdr_only,
                                cx.listener(|this, _, _, cx| {
                                    this.stream_filter.hdr_only = !this.stream_filter.hdr_only;
                                    this.expanded_stream_rows.clear();
                                    this.visible_stream_limit = STREAM_PAGE_SIZE;
                                    cx.notify();
                                }),
                            ))
                            .when(has_debrid, |bar| {
                                bar.child(render_stream_filter_toggle(
                                    "stream-filter-cached",
                                    "Cached",
                                    Some(IconName::Zap),
                                    self.stream_filter.cached_only,
                                    cx.listener(|this, _, _, cx| {
                                        this.stream_filter.cached_only =
                                            !this.stream_filter.cached_only;
                                        this.expanded_stream_rows.clear();
                                        this.visible_stream_limit = STREAM_PAGE_SIZE;
                                        cx.notify();
                                    }),
                                ))
                            }),
                    ),
            )
            .child(if filtered_indices.is_empty() {
                let is_querying = addons_ready < addons_total
                    || self.prowlarr_fallback_loading
                    || (!has_stream_request && has_stream_addons);
                let (empty_heading, empty_sub) = if is_querying {
                    (
                        "Querying stream addons...",
                        "Fetching available streams for this title...",
                    )
                } else if total_unfiltered > 0 {
                    (
                        "No streams match this filter",
                        "Click All to reset active filters and view every available stream.",
                    )
                } else if has_stream_addons {
                    (
                        "No streams found",
                        "Connected stream addons returned no playable sources for this title.",
                    )
                } else {
                    (
                        "No streams found",
                        "Install a stream addon in Addons or sign into your Stremio account.",
                    )
                };

                div()
                    .p(px(36.0))
                    .rounded(px(8.0))
                    .bg(rgb(BG_SURFACE))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .font(editorial_serif())
                            .text_size(px(24.0))
                            .text_color(rgb(TEXT_PRIMARY))
                            .child(empty_heading),
                    )
                    .child(
                        div()
                            .text_size(px(13.5))
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(empty_sub),
                    )
                    .into_any_element()
            } else {
                div()
                    .rounded(px(8.0))
                    .bg(rgb(BG_SURFACE))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .children(
                        filtered_indices
                            .into_iter()
                            .take(stream_limit)
                            .enumerate()
                            .map(|(row_pos, orig_idx)| {
                                let cand = &all_playable_arc[orig_idx];
                                let summary = summarize_stream_for_humans(&cand.parsed);
                                let hdr_badge = compact_hdr_badge(cand.parsed.hdr);
                                let audio_badge = compact_audio_badge(cand.parsed.audio);
                                let codec_badge = compact_codec_badge(cand.parsed.codec);
                                let release_group = cand
                                    .parsed
                                    .release_group
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .map(str::to_owned);
                                let release_line = cand.raw_release_filename();
                                let bitrate_text =
                                    estimate_bitrate_mbps(cand.parsed.size_bytes, runtime_minutes)
                                        .map(format_bitrate_mbps);
                                let cache_or_peer_label = if cand.parsed.is_cached {
                                    match cand.parsed.debrid_provider.as_deref() {
                                        Some(p) if !p.is_empty() => format!("Cached · {p}"),
                                        _ => "Cached".to_owned(),
                                    }
                                } else if let Some(seeds) = cand.parsed.seeders {
                                    format!("{seeds} peers")
                                } else {
                                    summary.availability_text.clone()
                                };
                                let addon_display = summary.addon_label.or_else(|| {
                                    (!cand.addon_name.trim().is_empty())
                                        .then(|| cand.addon_name.clone())
                                });
                                let is_expanded = self.expanded_stream_rows.contains(&orig_idx);
                                let raw_filename = is_expanded.then(|| cand.raw_release_filename());
                                let info_hash = is_expanded.then(|| cand.info_hash_hex()).flatten();

                                let cands_for_click = Arc::clone(&all_playable_arc);
                                let s_type = type_name.to_owned();
                                let s_id = meta_id.to_owned();
                                let s_vid = active_video_id.to_owned();
                                let s_title = title.to_owned();
                                let s_ep = episode_label.clone();
                                let s_mreq = meta_request.clone();

                                div()
                                    .when(row_pos > 0, |el| {
                                        el.border_t_1().border_color(rgba(BORDER_SOFT))
                                    })
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .id(("stream-row", orig_idx))
                                            .cursor_pointer()
                                            .px(px(18.0))
                                            .py(px(13.0))
                                            .hover(|s| s.bg(rgb(BG_ELEVATED)))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap(px(16.0))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    let subs = Some(build_subtitles_resource_path(
                                                        &s_type, &s_vid,
                                                    ));
                                                    this.start_playback_session(
                                                        s_type.clone(),
                                                        s_id.clone(),
                                                        s_vid.clone(),
                                                        s_title.clone(),
                                                        s_ep.clone(),
                                                        Some(s_mreq.clone()),
                                                        subs,
                                                        Arc::clone(&cands_for_click),
                                                        orig_idx,
                                                        resume_secs,
                                                        cx,
                                                    );
                                                }),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.0))
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(5.0))
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .flex_wrap()
                                                            .items_center()
                                                            .gap(px(6.0))
                                                            .child(svg_icon(
                                                                IconName::Play,
                                                                12.5,
                                                                rgb(ACCENT),
                                                            ))
                                                            .child(
                                                                div()
                                                                    .font(mono_font())
                                                                    .font_weight(FontWeight::BOLD)
                                                                    .text_size(px(13.5))
                                                                    .text_color(rgb(TEXT_PRIMARY))
                                                                    .child(summary.resolution),
                                                            )
                                                            .when_some(hdr_badge, |el, hdr| {
                                                                el.child(render_stream_spec_badge(
                                                                    hdr,
                                                                    Some(ACCENT),
                                                                ))
                                                            })
                                                            .when_some(audio_badge, |el, aud| {
                                                                el.child(render_stream_spec_badge(
                                                                    aud, None,
                                                                ))
                                                            })
                                                            .child(
                                                                div()
                                                                    .px(px(7.0))
                                                                    .py(px(2.0))
                                                                    .rounded(px(999.0))
                                                                    .bg(if cand.parsed.is_cached {
                                                                        rgba(0x4ade801c)
                                                                    } else {
                                                                        rgb(BG_ELEVATED)
                                                                    })
                                                                    .border_1()
                                                                    .border_color(
                                                                        if cand.parsed.is_cached {
                                                                            rgba(0x4ade804d)
                                                                        } else {
                                                                            rgba(BORDER_SUBTLE)
                                                                        },
                                                                    )
                                                                    .font(mono_font())
                                                                    .text_size(px(11.0))
                                                                    .text_color(
                                                                        if cand.parsed.is_cached {
                                                                            rgb(STATUS_WATCHED)
                                                                        } else {
                                                                            rgb(TEXT_SECONDARY)
                                                                        },
                                                                    )
                                                                    .child(cache_or_peer_label),
                                                            )
                                                            .when_some(codec_badge, |el, codec| {
                                                                el.child(render_stream_spec_badge(
                                                                    codec, None,
                                                                ))
                                                            })
                                                            .when_some(
                                                                addon_display,
                                                                |el, addon| {
                                                                    el.child(
                                                                        div()
                                                                            .min_w(px(0.0))
                                                                            .text_xs()
                                                                            .text_color(rgb(
                                                                                TEXT_MUTED,
                                                                            ))
                                                                            .truncate()
                                                                            .child(addon),
                                                                    )
                                                                },
                                                            ),
                                                    )
                                                    .child(
                                                        div()
                                                            .w_full()
                                                            .min_w(px(0.0))
                                                            .flex()
                                                            .items_center()
                                                            .gap(px(6.0))
                                                            .when_some(
                                                                release_group,
                                                                |el, group| {
                                                                    el.child(
                                                                        div()
                                                                            .flex_shrink_0()
                                                                            .font(mono_font())
                                                                            .text_size(px(11.0))
                                                                            .text_color(rgb(
                                                                                TEXT_SECONDARY,
                                                                            ))
                                                                            .child(format!(
                                                                                "[{group}]"
                                                                            )),
                                                                    )
                                                                },
                                                            )
                                                            .child(
                                                                div()
                                                                    .flex_1()
                                                                    .min_w(px(0.0))
                                                                    .font(mono_font())
                                                                    .text_size(px(11.5))
                                                                    .text_color(rgb(TEXT_MUTED))
                                                                    .truncate()
                                                                    .text_ellipsis()
                                                                    .child(release_line),
                                                            ),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex_shrink_0()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(12.0))
                                                    .when_some(bitrate_text, |el, br| {
                                                        el.child(
                                                            div()
                                                                .px(px(7.0))
                                                                .py(px(2.0))
                                                                .rounded(px(4.0))
                                                                .bg(rgb(BG_ELEVATED))
                                                                .border_1()
                                                                .border_color(rgba(BORDER_SUBTLE))
                                                                .font(mono_font())
                                                                .font_features(tabular_nums())
                                                                .text_size(px(11.0))
                                                                .text_color(rgb(TEXT_SECONDARY))
                                                                .child(br),
                                                        )
                                                    })
                                                    .when_some(summary.size_text, |el, sz| {
                                                        el.child(
                                                            div()
                                                                .w(px(74.0))
                                                                .flex()
                                                                .justify_end()
                                                                .font(mono_font())
                                                                .font_features(tabular_nums())
                                                                .text_xs()
                                                                .text_color(rgb(TEXT_PRIMARY))
                                                                .child(sz),
                                                        )
                                                    })
                                                    .child(
                                                        div()
                                                            .id((
                                                                "stream-details-toggle",
                                                                orig_idx,
                                                            ))
                                                            .cursor_pointer()
                                                            .px(px(9.0))
                                                            .py(px(4.0))
                                                            .rounded(px(6.0))
                                                            .bg(if is_expanded {
                                                                rgba(ACCENT_SOFT)
                                                            } else {
                                                                rgba(0x00000000)
                                                            })
                                                            .hover(|s| s.bg(rgba(0xffffff0f)))
                                                            .flex()
                                                            .items_center()
                                                            .gap(px(4.0))
                                                            .text_xs()
                                                            .text_color(if is_expanded {
                                                                rgb(ACCENT)
                                                            } else {
                                                                rgb(TEXT_MUTED)
                                                            })
                                                            .on_mouse_down(
                                                                MouseButton::Left,
                                                                cx.listener(
                                                                    move |this, _, _, cx| {
                                                                        cx.stop_propagation();
                                                                        this.toggle_stream_row_details(
                                                                            orig_idx, cx,
                                                                        );
                                                                    },
                                                                ),
                                                            )
                                                            .child("Details")
                                                            .child(svg_icon(
                                                                if is_expanded {
                                                                    IconName::ChevronDown
                                                                } else {
                                                                    IconName::ChevronRight
                                                                },
                                                                11.5,
                                                                if is_expanded {
                                                                    rgb(ACCENT)
                                                                } else {
                                                                    rgb(TEXT_MUTED)
                                                                },
                                                            )),
                                                    ),
                                            ),
                                    )
                                    .when_some(raw_filename, |el, filename| {
                                        el.child(
                                            div()
                                                .px(px(18.0))
                                                .py(px(12.0))
                                                .bg(rgb(BG_INPUT))
                                                .border_t_1()
                                                .border_color(rgba(BORDER_SOFT))
                                                .flex()
                                                .flex_col()
                                                .gap(px(6.0))
                                                .child(
                                                    div()
                                                        .font(mono_font())
                                                        .text_xs()
                                                        .text_color(rgb(TEXT_SECONDARY))
                                                        .child(filename),
                                                )
                                                .when(!summary.technical_specs.is_empty(), |d_el| {
                                                    d_el.child(
                                                        div()
                                                            .flex()
                                                            .flex_wrap()
                                                            .items_center()
                                                            .gap(px(8.0))
                                                            .font(mono_font())
                                                            .text_xs()
                                                            .text_color(rgb(TEXT_MUTED))
                                                            .children(summary.technical_specs),
                                                    )
                                                })
                                                .when_some(info_hash, |d_el, hash| {
                                                    d_el.child(
                                                        div()
                                                            .font(mono_font())
                                                            .text_size(px(11.0))
                                                            .text_color(rgb(TEXT_MUTED))
                                                            .child(format!("info_hash {hash}")),
                                                    )
                                                }),
                                        )
                                    })
                            }),
                    )
                    .when(remaining_streams > 0, |card| {
                        let next_step = remaining_streams.min(STREAM_PAGE_SIZE);
                        card.child(
                            div()
                                .px(px(18.0))
                                .py(px(12.0))
                                .bg(rgb(BG_INPUT))
                                .border_t_1()
                                .border_color(rgba(BORDER_SOFT))
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(format!(
                                            "Showing {visible_count} of {filtered_total} streams"
                                        )),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .id("btn-streams-show-more")
                                                .cursor_pointer()
                                                .h(px(30.0))
                                                .px(px(12.0))
                                                .rounded(px(6.0))
                                                .bg(rgb(BG_ELEVATED))
                                                .border_1()
                                                .border_color(rgba(BORDER_SUBTLE))
                                                .hover(|s| s.border_color(rgba(BORDER_STRONG)))
                                                .flex()
                                                .items_center()
                                                .font(ui_sans(FontWeight::MEDIUM))
                                                .text_xs()
                                                .text_color(rgb(TEXT_PRIMARY))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.visible_stream_limit = (this
                                                            .visible_stream_limit
                                                            .max(STREAM_PAGE_SIZE)
                                                            + STREAM_PAGE_SIZE)
                                                            .min(filtered_total);
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(format!("Show {next_step} more")),
                                        )
                                        .child(
                                            div()
                                                .id("btn-streams-show-all")
                                                .cursor_pointer()
                                                .h(px(30.0))
                                                .px(px(12.0))
                                                .rounded(px(6.0))
                                                .bg(rgb(BG_SURFACE))
                                                .border_1()
                                                .border_color(rgba(BORDER_SUBTLE))
                                                .hover(|s| s.border_color(rgba(BORDER_STRONG)))
                                                .flex()
                                                .items_center()
                                                .font(mono_font())
                                                .text_xs()
                                                .text_color(rgb(TEXT_SECONDARY))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.visible_stream_limit = filtered_total;
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(format!("Show all ({filtered_total})")),
                                        ),
                                ),
                        )
                    })
                    .into_any_element()
            })
    }
}

fn render_stream_spec_badge(
    label: impl Into<SharedString>,
    accent_fg: Option<u32>,
) -> impl IntoElement {
    div()
        .px(px(7.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .bg(rgb(BG_ELEVATED))
        .border_1()
        .border_color(rgba(BORDER_SUBTLE))
        .font(mono_font())
        .text_size(px(11.0))
        .text_color(rgb(accent_fg.unwrap_or(TEXT_PRIMARY)))
        .child(label.into())
}

fn render_stream_filter_toggle(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    icon: Option<IconName>,
    active: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id.into())
        .cursor_pointer()
        .h(px(32.0))
        .px(px(13.0))
        .rounded(px(999.0))
        .bg(if active {
            rgba(ACCENT_SOFT)
        } else {
            rgb(BG_SURFACE)
        })
        .border_1()
        .border_color(if active {
            rgb(ACCENT)
        } else {
            rgba(BORDER_SUBTLE)
        })
        .hover(|s| {
            s.border_color(if active {
                rgb(ACCENT)
            } else {
                rgba(BORDER_STRONG)
            })
        })
        .flex()
        .items_center()
        .gap(px(5.0))
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(px(12.5))
        .text_color(if active {
            rgb(ACCENT)
        } else {
            rgb(TEXT_SECONDARY)
        })
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .when_some(icon, |el, ic| {
            el.child(svg_icon(
                ic,
                11.5,
                if active {
                    rgb(ACCENT)
                } else {
                    rgb(TEXT_SECONDARY)
                },
            ))
        })
        .child(label.into())
}
