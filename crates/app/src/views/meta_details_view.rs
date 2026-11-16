use std::{collections::HashMap, ops::Range, sync::Arc};

use gpui::{
    div, img, linear_color_stop, linear_gradient, point, prelude::*, px, relative, rgb, rgba,
    AnyElement, ClipboardItem, Context, Div, FontWeight, MouseButton, MouseDownEvent, RenderImage,
    ScrollEvent, SharedString, Stateful, Window,
};
use stremio_core_runtime::{
    build_canonical_magnet,
    stremio_core::{
        constants::CINEMETA_URL,
        models::common::Loadable,
        types::{
            addon::ResourceRequest,
            library::LibraryItem,
            resource::{
                MetaItem, MetaItemBehaviorHints, MetaItemPreview, PosterShape, Stream,
                StreamSource, Video,
            },
            watched_bitfield::WatchedBitField,
        },
    },
    EditorialMediaHit, EnrichedCollection, PersonCreditChip,
};
use url::Url;

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{
        build_default_meta_request, build_subtitles_resource_path, classify_library_item_status,
        extract_cast_and_directors_from_links, extract_genres, extract_imdb_rating,
        find_cached_preview_in_model, format_movietracker_episode_code,
        pick_default_series_episode_id, resolve_ready_meta_item, resume_offset_for_library_item,
        sync_memoized_enriched_streams, sync_memoized_meta_streams, AddonStreamCounts,
        BrowsingContextMenu, CachedTitlePreview, LibraryStatusTab, PlaybackCandidate,
        StreamResolutionChoice, ToastKind, STREAM_PAGE_SIZE,
    },
    theme::{
        compact_audio_badge, compact_codec_badge, compact_hdr_badge, editorial_serif,
        estimate_bitrate_mbps, format_bitrate_mbps, format_episode_air_date,
        format_timestamp_secs, glass_capsule_shadow, meta_divider, mono_font,
        parse_runtime_minutes, placeholder_tint_for_id, poster_card_shadow, semantic_status_pill,
        sentence_case_label, summarize_stream_for_humans, tabular_nums, ui_sans, ACCENT,
        ACCENT_AMBER, ACCENT_SOFT, BG_SURFACE, BORDER_ACCENT, BORDER_SOFT, CTA_ON_ACCENT,
        GLASS_BORDER, GLASS_BORDER_STRONG, GLASS_CARD_BG, GLASS_ROW_HOVER_BG, GLASS_TROUGH_BG,
        RADIUS_CARD, RADIUS_CONTROL, RADIUS_PILL, RADIUS_POSTER, SEGMENT_ACTIVE_BG,
        SEGMENT_ACTIVE_TEXT, STATUS_WATCHED, STATUS_WATCHING, STATUS_WATCHLIST, TEXT_MUTED,
        TEXT_PRIMARY, TEXT_SECONDARY,
    },
    ui_components::{
        glass_back_button, glass_dropdown_option, glass_dropdown_panel, glass_segmented_bar,
        glass_segmented_item, primary_accent_button, render_poster_card, render_stream_source_row,
        secondary_glass_button, PosterCardProps, StreamSourceRowProps,
    },
    views::search_view::person_initials,
    virtual_grid::PosterAspect,
};

const EPISODE_ROW_CARD_HEIGHT_PX: f32 = 116.0;
const EPISODE_ROW_STRIDE_PX: f32 = 124.0;
const EPISODE_LIST_MAX_HEIGHT_PX: f32 = 560.0;
const SIDE_BY_SIDE_MIN_WIDTH_PX: f32 = 1320.0;

struct MetaDetailsSnapshot {
    meta: MetaItem,
    meta_request: ResourceRequest,
    active_video_id: String,
    addon_playable_arc: Arc<Vec<PlaybackCandidate>>,
    stream_addons_ready: usize,
    stream_addons_total: usize,
    has_stream_request: bool,
    has_stream_addons: bool,
    in_library: bool,
    library_item: Option<LibraryItem>,
    watched: Option<WatchedBitField>,
    last_used_stream: Option<(Url, Stream)>,
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum TrailerTarget {
    DirectStream(Box<Stream>),
    ExternalUrl(String),
}

#[derive(Clone)]
struct BestEpisodePlay {
    candidates: Arc<Vec<PlaybackCandidate>>,
    index: usize,
    episode_label: Option<String>,
    resume_secs: Option<f64>,
}

struct ResolvedMetaHeroImages {
    preblurred_backdrop: Option<Arc<RenderImage>>,
    hero_backdrop: Option<Arc<RenderImage>>,
    title_logo_tex: Option<Arc<RenderImage>>,
    poster_tex: Option<Arc<RenderImage>>,
}

struct StreamPickerPanelArgs<'a> {
    type_name: &'a str,
    meta_id: &'a str,
    active_video_id: &'a str,
    title: &'a str,
    episode_label: Option<String>,
    runtime_minutes: Option<u32>,
    meta_request: ResourceRequest,
    resume_secs: Option<f64>,
    has_debrid: bool,
    all_playable_arc: Arc<Vec<PlaybackCandidate>>,
    available_addons_arc: AddonStreamCounts,
    filtered_indices: Vec<usize>,
    last_used_orig_idx: Option<usize>,
    addons_ready: usize,
    addons_total: usize,
    has_stream_request: bool,
    has_stream_addons: bool,
}

struct SeasonsPanelArgs<'a> {
    type_name: &'a str,
    meta_id: &'a str,
    meta_preview: MetaItemPreview,
    meta_request: ResourceRequest,
    in_library: bool,
    seasons: &'a [u32],
    active_season: Option<u32>,
    episodes: Arc<Vec<Video>>,
    active_video_id: &'a str,
    series_runtime: Option<&'a str>,
    library_item: Option<&'a LibraryItem>,
    watched: Option<&'a WatchedBitField>,
    best_play_for_selected: Option<BestEpisodePlay>,
    panel_width_px: Option<f32>,
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
                .get(&meta.preview.id)
                .or_else(|| model.ctx.library.items.get(route_id))
                .is_some_and(|i| !i.removed);
            let library_item = model
                .meta_details
                .library_item
                .clone()
                .or_else(|| model.ctx.library.items.get(&meta.preview.id).cloned())
                .or_else(|| model.ctx.library.items.get(route_id).cloned());
            let watched = model.meta_details.watched.clone();
            let last_used_stream =
                model
                    .meta_details
                    .last_used_stream
                    .as_ref()
                    .and_then(|res| match &res.content {
                        Some(Loadable::Ready(Some(stream))) => {
                            Some((res.request.base.clone(), stream.clone()))
                        }
                        _ => None,
                    });

            let active_video_id = pick_default_series_episode_id(
                &meta,
                selected_stream_vid.as_deref().or(route_video_id),
                library_item
                    .as_ref()
                    .and_then(|li| li.state.video_id.as_deref()),
            )
            .unwrap_or_else(|| meta.preview.id.clone());

            let (addon_playable_arc, _, stream_addons_ready, stream_addons_total) =
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

            Ok(MetaDetailsSnapshot {
                meta,
                meta_request,
                active_video_id,
                addon_playable_arc,
                stream_addons_ready,
                stream_addons_total,
                has_stream_request,
                has_stream_addons,
                in_library,
                library_item,
                watched,
                last_used_stream,
            })
        });

        let MetaDetailsSnapshot {
            meta,
            meta_request,
            active_video_id,
            addon_playable_arc,
            stream_addons_ready,
            stream_addons_total,
            has_stream_request,
            has_stream_addons,
            in_library,
            library_item,
            watched,
            last_used_stream,
        } = match details_snap {
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
        if let Some(col_id) = enriched
            .as_ref()
            .and_then(|e| e.collection.as_ref())
            .map(|c| c.id)
        {
            self.request_collection_if_needed(col_id, cx);
        }

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

        let (all_playable_arc, available_addons_arc) = sync_memoized_enriched_streams(
            &mut self.memoized_enriched_streams,
            &meta.preview.id,
            &active_video_id,
            &addon_playable_arc,
            prowlarr_slice,
            self.prowlarr_request_seq,
            private_cfg.active_debrid,
            &self.local_debrid_cache_map,
            private_cfg.has_local_debrid_configured() && private_cfg.debrid_cached_only_filter,
        );
        let last_used_orig_idx =
            find_last_used_stream_index(&all_playable_arc, last_used_stream.as_ref());

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

        let season_episodes = Arc::new(match active_season {
            Some(season) => meta
                .videos_by_season(season)
                .into_iter()
                .cloned()
                .collect(),
            None => Vec::new(),
        });

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

        let ResolvedMetaHeroImages {
            preblurred_backdrop,
            hero_backdrop,
            title_logo_tex,
            poster_tex,
        } = self.resolve_meta_hero_images(
            &meta.preview.id,
            enriched.as_ref().and_then(|e| e.backdrop_url.as_deref()),
            meta.preview.background.as_ref(),
            enriched.as_ref().and_then(|e| e.logo_url.as_deref()),
            meta.preview.logo.as_ref(),
            enriched.as_ref().and_then(|e| e.poster_url.as_deref()),
            meta.preview.poster.as_ref(),
        );

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
        combined_credits.extend(credit_cast.into_iter().take(16));

        let imdb = extract_imdb_rating(&meta.preview).or_else(|| {
            enriched
                .as_ref()
                .and_then(|e| e.imdb_rating.clone())
                .filter(|s| !s.trim().is_empty())
        });
        let mut genres = extract_genres(&meta.preview, 5);
        if genres.is_empty() {
            if let Some(e) = enriched.as_ref() {
                genres = e.genres.iter().take(5).cloned().collect();
            }
        }
        let certification = enriched
            .as_ref()
            .and_then(|e| e.certification.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let tagline = enriched
            .as_ref()
            .and_then(|e| e.tagline.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let release_year = meta
            .preview
            .release_info
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                enriched
                    .as_ref()
                    .and_then(|e| e.year.clone())
                    .filter(|s| !s.trim().is_empty())
            });
        let runtime_display = meta
            .preview
            .runtime
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                enriched
                    .as_ref()
                    .and_then(|e| e.runtime.clone())
                    .filter(|s| !s.trim().is_empty())
            });
        let description_text = meta
            .preview
            .description
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                enriched
                    .as_ref()
                    .and_then(|e| e.overview.clone())
                    .filter(|s| !s.trim().is_empty())
            });

        let preview_for_toggle = meta.preview.clone();
        let resume_secs = resume_offset_for_library_item(library_item.as_ref(), &active_video_id);
        let lib_status = library_item.as_ref().and_then(classify_library_item_status);
        let is_watched = library_item
            .as_ref()
            .is_some_and(|li| !li.removed && (li.watched() || li.progress() >= 95.0));
        let trailer_target = resolve_trailer_target(
            &meta.preview.trailer_streams,
            enriched
                .as_ref()
                .and_then(|e| e.trailer_youtube_key.as_deref()),
        );

        let side_by_side = self.viewport_size_px.0 >= SIDE_BY_SIDE_MIN_WIDTH_PX;
        let seasons_panel_width = side_by_side.then(|| {
            let avail = (self.viewport_size_px.0 - 64.0).min(1440.0);
            (avail * 0.43).clamp(480.0, 560.0)
        });

        let has_seasons = !seasons.is_empty();
        let runtime_minutes = runtime_display.as_deref().and_then(parse_runtime_minutes);
        let best_play_for_selected = filtered_indices
            .first()
            .copied()
            .or_else(|| (!all_playable_arc.is_empty()).then_some(0))
            .map(|idx| BestEpisodePlay {
                candidates: Arc::clone(&all_playable_arc),
                index: idx,
                episode_label: episode_header_label.clone(),
                resume_secs,
            });

        let seasons_panel = has_seasons.then(|| {
            self.render_series_seasons_panel(
                SeasonsPanelArgs {
                    type_name: &meta.preview.r#type,
                    meta_id: &meta.preview.id,
                    meta_preview: meta.preview.clone(),
                    meta_request: meta_request.clone(),
                    in_library,
                    seasons: &seasons,
                    active_season,
                    episodes: season_episodes,
                    active_video_id: &active_video_id,
                    series_runtime: runtime_display.as_deref(),
                    library_item: library_item.as_ref(),
                    watched: watched.as_ref(),
                    best_play_for_selected,
                    panel_width_px: seasons_panel_width,
                },
                cx,
            )
        });

        let primary_cta_label = match (&active_ep_code, resume_secs) {
            (Some(code), Some(_)) => format!("Continue {code}"),
            (Some(code), None) => format!("Play {code}"),
            (None, Some(s)) => format!("Resume ({})", format_timestamp_secs(s)),
            (None, None) => "Watch".to_owned(),
        };

        let meta_type_for_genre = meta.preview.r#type.clone();
        let hero_right_col = div()
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
                    .child(sentence_case_label(&meta.preview.r#type))
                    .when_some(certification, |el, cert| {
                        el.child(render_certification_pill(&cert))
                    })
                    .when_some(release_year, |el, yr| {
                        el.child(meta_divider())
                            .child(div().font(mono_font()).child(yr))
                    })
                    .when_some(runtime_display, |el, rt| {
                        el.child(meta_divider())
                            .child(div().font(mono_font()).child(rt))
                    })
                    .when_some(imdb, |el, r| {
                        el.child(meta_divider()).child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.5))
                                .font(mono_font())
                                .text_color(rgb(TEXT_PRIMARY))
                                .child(svg_icon(IconName::Star, 12.5, rgb(ACCENT_AMBER)))
                                .child(r),
                        )
                    })
                    .when_some(lib_status, |el, st| {
                        let pill = match st {
                            LibraryStatusTab::Watched => semantic_status_pill(
                                IconName::Check,
                                "Watched",
                                STATUS_WATCHED,
                                0x4ade804d,
                                0x4ade801c,
                            ),
                            LibraryStatusTab::Watching => semantic_status_pill(
                                IconName::Play,
                                "Watching",
                                STATUS_WATCHING,
                                0x60a5fa4d,
                                0x60a5fa1c,
                            ),
                            LibraryStatusTab::Watchlist => semantic_status_pill(
                                IconName::Bookmark,
                                "In library",
                                STATUS_WATCHLIST,
                                0xc084fc4d,
                                0xc084fc1c,
                            ),
                        };
                        el.child(pill)
                    }),
            )
            .child(if let Some(logo_img) = title_logo_tex {
                img(logo_img)
                    .h(px(84.0))
                    .max_w(px(420.0))
                    .object_fit(gpui::ObjectFit::Contain)
                    .into_any_element()
            } else {
                div()
                    .font(editorial_serif())
                    .text_size(px(58.0))
                    .line_height(gpui::relative(1.06))
                    .text_color(rgb(TEXT_PRIMARY))
                    .child(meta.preview.name.clone())
                    .into_any_element()
            })
            .when_some(tagline, |el, line| {
                el.child(
                    div()
                        .max_w(px(640.0))
                        .font(ui_sans(FontWeight::NORMAL))
                        .italic()
                        .text_size(px(15.0))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(line),
                )
            })
            .when_some(description_text, |el, desc| {
                el.child(
                    div()
                        .max_w(px(660.0))
                        .text_size(px(14.5))
                        .line_height(gpui::relative(1.45))
                        .text_color(rgb(TEXT_PRIMARY))
                        .opacity(0.88)
                        .child(desc),
                )
            })
            .when(!genres.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.0))
                        .children(genres.into_iter().enumerate().map(|(idx, genre)| {
                            let g_type = meta_type_for_genre.clone();
                            let g_val = genre.clone();
                            render_genre_chip(
                                idx,
                                genre,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_discover_genre(&g_type, &g_val, cx);
                                }),
                            )
                        })),
                )
            })
            .child(
                div()
                    .mt(px(8.0))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(10.0))
                    .when_some(filtered_indices.first().copied(), |el, best_play_idx| {
                        let top_cands = Arc::clone(&all_playable_arc);
                        let p_type = meta.preview.r#type.clone();
                        let p_id = meta.preview.id.clone();
                        let p_vid = active_video_id.clone();
                        let p_title = meta.preview.name.clone();
                        let p_ep = episode_header_label.clone();
                        let p_mreq = meta_request.clone();

                        el.child(primary_accent_button(
                            "meta-btn-play-best",
                            Some(IconName::Play),
                            primary_cta_label,
                            cx.listener(move |this, _, _, cx| {
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
                            }),
                        ))
                    })
                    .when_some(trailer_target, |el, target| {
                        let t_type = meta.preview.r#type.clone();
                        let t_id = meta.preview.id.clone();
                        let t_title = meta.preview.name.clone();
                        el.child(secondary_glass_button(
                            "meta-btn-trailer",
                            Some(IconName::Clapperboard),
                            "Trailer",
                            false,
                            cx.listener(move |this, _, _, cx| match &target {
                                TrailerTarget::DirectStream(stream) => {
                                    let cand = PlaybackCandidate::from_stream_and_request(
                                        stream.as_ref().clone(),
                                        None,
                                        &[],
                                    );
                                    this.start_playback_session(
                                        t_type.clone(),
                                        t_id.clone(),
                                        format!("{t_id}:trailer"),
                                        t_title.clone(),
                                        Some("Trailer".to_owned()),
                                        None,
                                        None,
                                        Arc::new(vec![cand]),
                                        0,
                                        None,
                                        cx,
                                    );
                                }
                                TrailerTarget::ExternalUrl(url) => {
                                    cx.open_url(url);
                                }
                            }),
                        ))
                    })
                    .child(secondary_glass_button(
                        "meta-btn-toggle-lib",
                        Some(if in_library {
                            IconName::Check
                        } else {
                            IconName::Bookmark
                        }),
                        if in_library {
                            "In Library"
                        } else {
                            "Add to Library"
                        },
                        in_library,
                        cx.listener(move |this, _, _, cx| {
                            this.core.toggle_library_item(preview_for_toggle.clone());
                            cx.notify();
                        }),
                    ))
                    .child(secondary_glass_button(
                        "meta-btn-mark-watched",
                        Some(if is_watched {
                            IconName::Check
                        } else {
                            IconName::Eye
                        }),
                        if is_watched {
                            "Watched"
                        } else {
                            "Mark watched"
                        },
                        is_watched,
                        cx.listener(move |this, _, _, cx| {
                            this.core.mark_meta_watched(!is_watched);
                            cx.notify();
                        }),
                    )),
            )
            .into_any_element();

        let library_statuses = self.cached_library_statuses();
        let collection_info = enriched.as_ref().and_then(|e| e.collection.as_ref()).map(|c| {
            let loaded = self
                .active_collection
                .as_ref()
                .and_then(|(cid, col)| (*cid == c.id).then_some(col.clone()));
            (c.clone(), loaded, self.loading_collection_id == Some(c.id))
        });
        let recommendations = enriched
            .as_ref()
            .map(|e| e.recommendations.clone())
            .unwrap_or_default();

        let main_sections = div()
            .w_full()
            .flex_shrink_0()
            .max_w(px(1440.0))
            .mx_auto()
            .px(px(32.0))
            .flex()
            .when(side_by_side, |d| d.items_start())
            .when(!side_by_side, |d| d.flex_col())
            .gap(px(32.0))
            .when_some(seasons_panel, |el, panel| el.child(panel))
            .child(
                div()
                    .when(side_by_side, |d| d.flex_1().min_w(px(0.0)))
                    .when(!side_by_side, |d| d.w_full())
                    .child(self.render_stream_picker_panel(
                        StreamPickerPanelArgs {
                            type_name: &meta.preview.r#type,
                            meta_id: &meta.preview.id,
                            active_video_id: &active_video_id,
                            title: &meta.preview.name,
                            episode_label: episode_header_label,
                            runtime_minutes,
                            meta_request,
                            resume_secs,
                            has_debrid,
                            all_playable_arc,
                            available_addons_arc,
                            filtered_indices,
                            last_used_orig_idx,
                            addons_ready: stream_addons_ready,
                            addons_total: stream_addons_total,
                            has_stream_request,
                            has_stream_addons,
                        },
                        cx,
                    )),
            );

        let collection_section = collection_info.and_then(|(stub_col, loaded_opt, is_loading)| {
            self.render_collection_section(
                &stub_col,
                loaded_opt.as_ref(),
                is_loading,
                &meta.preview.id,
                &library_statuses,
                cx,
            )
        });

        let recommendations_section = (!recommendations.is_empty()).then(|| {
            self.render_editorial_poster_rail(
                "meta-recommendations-section",
                "meta-recommendations-rail",
                "More Like This".to_owned(),
                None,
                &recommendations,
                None,
                &library_statuses,
                cx,
            )
        });

        div()
            .id("meta-details-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.meta_scroll_handle)
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
                    .child(Self::render_hero_backdrop_shell(
                        &meta.preview.id,
                        preblurred_backdrop,
                        hero_backdrop,
                        poster_tex,
                        "btn-details-back",
                        hero_right_col,
                        cx,
                    ))
                    .when(!combined_credits.is_empty(), |el| {
                        el.child(self.render_cast_rail(combined_credits, cx))
                    })
                    .child(main_sections)
                    .when_some(collection_section, |el, section| el.child(section))
                    .when_some(recommendations_section, |el, section| el.child(section)),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_meta_hero_images(
        &self,
        meta_id: &str,
        enriched_backdrop: Option<&str>,
        preview_backdrop: Option<&Url>,
        enriched_logo: Option<&str>,
        preview_logo: Option<&Url>,
        enriched_poster: Option<&str>,
        preview_poster: Option<&Url>,
    ) -> ResolvedMetaHeroImages {
        let is_imdb = meta_id.starts_with("tt");
        let metahub_bg = is_imdb
            .then(|| format!("https://images.metahub.space/background/medium/{meta_id}/img"));
        let metahub_logo =
            is_imdb.then(|| format!("https://images.metahub.space/logo/medium/{meta_id}/img"));

        let mut ambient_urls: Vec<&str> = Vec::with_capacity(4);
        if let Some(u) = enriched_backdrop {
            ambient_urls.push(u);
        }
        if let Some(u) = preview_backdrop {
            ambient_urls.push(u.as_str());
        }
        if let Some(u) = metahub_bg.as_deref() {
            ambient_urls.push(u);
        }
        if let Some(u) = preview_poster {
            ambient_urls.push(u.as_str());
        } else if let Some(u) = enriched_poster {
            ambient_urls.push(u);
        }
        let preblurred_backdrop = self.poster_cache.resolve_first_available_ambient(ambient_urls);

        let mut hero_urls: Vec<&str> = Vec::with_capacity(3);
        if let Some(u) = enriched_backdrop {
            hero_urls.push(u);
        }
        if let Some(u) = preview_backdrop {
            hero_urls.push(u.as_str());
        }
        if let Some(u) = metahub_bg.as_deref() {
            hero_urls.push(u);
        }
        let hero_backdrop = self.poster_cache.resolve_first_available_hero(hero_urls);

        let mut logo_urls: Vec<&str> = Vec::with_capacity(3);
        if let Some(u) = enriched_logo {
            logo_urls.push(u);
        }
        if let Some(u) = preview_logo {
            logo_urls.push(u.as_str());
        }
        if let Some(u) = metahub_logo.as_deref() {
            logo_urls.push(u);
        }
        let title_logo_tex = self.poster_cache.resolve_first_available_logo(logo_urls);

        let poster_tex = preview_poster
            .map(Url::as_str)
            .or(enriched_poster)
            .and_then(|u| self.poster_cache.resolve_poster_image(u));

        ResolvedMetaHeroImages {
            preblurred_backdrop,
            hero_backdrop,
            title_logo_tex,
            poster_tex,
        }
    }

    fn render_hero_backdrop_shell(
        placeholder_id: &str,
        preblurred_backdrop: Option<Arc<RenderImage>>,
        hero_backdrop: Option<Arc<RenderImage>>,
        poster_tex: Option<Arc<RenderImage>>,
        back_btn_id: &'static str,
        right_col: AnyElement,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .w_full()
            .flex_shrink_0()
            .relative()
            .overflow_hidden()
            .bg(rgb(placeholder_tint_for_id(placeholder_id)))
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
                            .child(
                                div().flex().child(glass_back_button(
                                    back_btn_id,
                                    "Back",
                                    cx.listener(|this, _, _, cx| {
                                        this.navigate_back(cx);
                                    }),
                                )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_end()
                                    .gap(px(36.0))
                                    .child(
                                        div()
                                            .w(px(236.0))
                                            .h(px(354.0))
                                            .flex_shrink_0()
                                            .rounded(px(RADIUS_POSTER))
                                            .overflow_hidden()
                                            .bg(rgb(BG_SURFACE))
                                            .border_1()
                                            .border_color(rgba(GLASS_BORDER))
                                            .shadow(poster_card_shadow())
                                            .when_some(poster_tex, |el, tex| {
                                                el.child(
                                                    img(tex)
                                                        .size_full()
                                                        .object_fit(gpui::ObjectFit::Cover),
                                                )
                                            }),
                                    )
                                    .child(right_col),
                            ),
                    ),
            )
    }

    fn render_cast_rail(
        &self,
        combined_credits: Vec<PersonCreditChip>,
        cx: &mut Context<Self>,
    ) -> Div {
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
                            .child("Click any person for filmography"),
                    ),
            )
            .child(
                div()
                    .id("meta-cast-rail")
                    .w_full()
                    .overflow_x_scroll()
                    .pb(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .children(combined_credits.into_iter().enumerate().map(|(idx, chip)| {
                        let pk = chip.person_key.clone();
                        let pname = chip.name.clone();
                        let headshot_tex = chip
                            .headshot_url
                            .as_deref()
                            .and_then(|u| self.poster_cache.resolve_thumbnail_image(u));
                        let initials = person_initials(&chip.name);
                        let role_label = chip.role_or_character.clone().unwrap_or_else(|| {
                            if chip.is_director {
                                "Director".to_owned()
                            } else {
                                "Cast".to_owned()
                            }
                        });

                        div()
                            .id(("meta-cast-card", idx))
                            .flex_shrink_0()
                            .w(px(216.0))
                            .cursor_pointer()
                            .p(px(10.0))
                            .rounded(px(RADIUS_CARD))
                            .bg(rgba(GLASS_CARD_BG))
                            .border_1()
                            .border_color(rgba(GLASS_BORDER))
                            .hover(|s| {
                                s.bg(rgba(GLASS_ROW_HOVER_BG))
                                    .border_color(rgba(GLASS_BORDER_STRONG))
                            })
                            .flex()
                            .items_center()
                            .gap(px(12.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.open_person_details(pk.clone(), pname.clone(), cx);
                                }),
                            )
                            .child(
                                div()
                                    .w(px(46.0))
                                    .h(px(46.0))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .overflow_hidden()
                                    .bg(rgba(GLASS_TROUGH_BG))
                                    .border_1()
                                    .border_color(rgba(GLASS_BORDER))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(if let Some(tex) = headshot_tex {
                                        img(tex)
                                            .size_full()
                                            .object_fit(gpui::ObjectFit::Cover)
                                            .into_any_element()
                                    } else {
                                        div()
                                            .font(editorial_serif())
                                            .text_size(px(16.0))
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(initials)
                                            .into_any_element()
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::SEMIBOLD))
                                            .text_size(px(13.5))
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .truncate()
                                            .child(chip.name),
                                    )
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::NORMAL))
                                            .text_size(px(12.0))
                                            .text_color(if chip.is_director {
                                                rgb(ACCENT)
                                            } else {
                                                rgb(TEXT_MUTED)
                                            })
                                            .truncate()
                                            .child(role_label),
                                    ),
                            )
                    })),
            )
    }

    fn render_collection_section(
        &self,
        stub_col: &EnrichedCollection,
        loaded_col: Option<&EnrichedCollection>,
        is_loading: bool,
        current_meta_id: &str,
        library_statuses: &HashMap<String, LibraryStatusTab>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let col = loaded_col.unwrap_or(stub_col);
        let heading = collection_heading(&col.name);
        if col.parts.is_empty() {
            if !is_loading {
                return None;
            }
            return Some(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .max_w(px(1440.0))
                    .mx_auto()
                    .px(px(32.0))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
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
                                    .child(heading),
                            )
                            .child(
                                div()
                                    .font(mono_font())
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .child("Loading collection..."),
                            ),
                    )
                    .into_any_element(),
            );
        }

        let badge = Some(SharedString::from(format!("{} titles", col.parts.len())));
        Some(
            self.render_editorial_poster_rail(
                "meta-collection-section",
                "meta-collection-rail",
                heading,
                badge,
                &col.parts,
                Some(current_meta_id),
                library_statuses,
                cx,
            )
            .into_any_element(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_editorial_poster_rail(
        &self,
        section_id: &'static str,
        rail_id: &'static str,
        heading: String,
        right_badge: Option<SharedString>,
        hits: &[EditorialMediaHit],
        current_meta_id: Option<&str>,
        library_statuses: &HashMap<String, LibraryStatusTab>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(section_id)
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
                    .gap(px(12.0))
                    .child(
                        div()
                            .font(editorial_serif())
                            .text_size(px(26.0))
                            .text_color(rgb(TEXT_PRIMARY))
                            .child(heading),
                    )
                    .when_some(right_badge, |el, badge| {
                        el.child(
                            div()
                                .font(mono_font())
                                .text_xs()
                                .text_color(rgb(TEXT_MUTED))
                                .child(badge),
                        )
                    }),
            )
            .child(
                div()
                    .id(rail_id)
                    .w_full()
                    .overflow_x_scroll()
                    .pb(px(8.0))
                    .flex()
                    .items_start()
                    .gap(px(16.0))
                    .children(hits.iter().enumerate().map(|(idx, hit)| {
                        let hit_id = hit
                            .imdb_id
                            .clone()
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| hit.canonical_id.clone());
                        let is_current = current_meta_id.is_some_and(|cid| cid == hit_id);
                        let status_tab = library_statuses.get(&hit_id).copied();
                        let in_lib = status_tab.is_some();
                        let is_watched = status_tab == Some(LibraryStatusTab::Watched);
                        let tex = hit
                            .poster_url
                            .as_deref()
                            .and_then(|u| self.poster_cache.resolve_poster_image(u));
                        let subtitle = if is_current {
                            Some(SharedString::from(match hit.year.as_deref() {
                                Some(y) if !y.trim().is_empty() => format!("{y} · This title"),
                                _ => "This title".to_owned(),
                            }))
                        } else {
                            let type_label = (!hit.media_type.trim().is_empty())
                                .then(|| sentence_case_label(&hit.media_type));
                            match (
                                hit.year.as_deref().filter(|s| !s.trim().is_empty()),
                                type_label,
                            ) {
                                (Some(y), Some(t)) => Some(SharedString::from(format!("{y} · {t}"))),
                                (Some(y), None) => Some(SharedString::from(y.to_owned())),
                                (None, Some(t)) => Some(SharedString::from(t)),
                                (None, None) => None,
                            }
                        };

                        let open_type = hit.media_type.clone();
                        let open_id = hit_id.clone();
                        let play_type = hit.media_type.clone();
                        let play_id = hit_id.clone();
                        let hover_type = hit.media_type.clone();
                        let hover_id = hit_id.clone();
                        let hover_bg = hit.backdrop_url.clone();
                        let ctx_type = hit.media_type.clone();
                        let ctx_id = hit_id.clone();
                        let ctx_title = hit.title.clone();
                        let preview_for_toggle = editorial_hit_to_preview(hit, &hit_id);
                        let preview_for_ctx = preview_for_toggle.clone();

                        render_poster_card(
                            PosterCardProps::new((rail_id, idx), hit.title.clone(), hit_id)
                                .subtitle(subtitle)
                                .image(tex)
                                .aspect(PosterAspect::Portrait2x3)
                                .fixed_width_px(Some(164.0))
                                .image_height_px(Some(246.0))
                                .status_tab(status_tab)
                                .in_library(in_lib)
                                .imdb_rating(hit.imdb_rating.clone().map(SharedString::from))
                                .on_open(cx.listener(move |this, _, _, cx| {
                                    this.open_editorial_media_hit(
                                        open_type.clone(),
                                        open_id.clone(),
                                        None,
                                        cx,
                                    );
                                }))
                                .on_quick_play(cx.listener(move |this, _, _, cx| {
                                    this.quick_resume_or_open(
                                        play_type.clone(),
                                        play_id.clone(),
                                        None,
                                        cx,
                                    );
                                }))
                                .on_toggle_library(cx.listener(move |this, _, _, cx| {
                                    this.core.toggle_library_item(preview_for_toggle.clone());
                                    cx.notify();
                                }))
                                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                    this.on_card_hover_prefetch(
                                        &hover_type,
                                        &hover_id,
                                        hover_bg.as_deref(),
                                        *hovered,
                                        cx,
                                    );
                                }))
                                .on_context_menu(cx.listener(
                                    move |this, ev: &MouseDownEvent, _, cx| {
                                        let menu = BrowsingContextMenu::new(
                                            ev.position,
                                            ctx_type.clone(),
                                            ctx_id.clone(),
                                            ctx_title.clone(),
                                        )
                                        .preview(Some(preview_for_ctx.clone()))
                                        .is_in_library(in_lib)
                                        .is_watched(is_watched);
                                        this.open_browsing_context_menu(menu, cx);
                                    },
                                )),
                        )
                    })),
            )
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
                        .child(div().flex().child(glass_back_button(
                            "btn-details-back",
                            "Back",
                            cx.listener(|this, _, _, cx| {
                                this.navigate_back(cx);
                            }),
                        )))
                        .child(
                            div()
                                .p(px(32.0))
                                .rounded(px(RADIUS_CARD))
                                .bg(rgba(GLASS_CARD_BG))
                                .border_1()
                                .border_color(rgba(GLASS_BORDER))
                                .font(editorial_serif())
                                .text_size(px(28.0))
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(status_text),
                        ),
                )
                .into_any_element();
        };

        let ResolvedMetaHeroImages {
            preblurred_backdrop,
            hero_backdrop,
            title_logo_tex,
            poster_tex,
        } = self.resolve_meta_hero_images(
            &preview.id,
            None,
            preview.background.as_ref(),
            None,
            preview.logo.as_ref(),
            None,
            preview.poster.as_ref(),
        );

        let loading_type = preview.r#type.clone();
        let loading_genres = preview.genres.clone();
        let right_col = div()
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
                    .child(sentence_case_label(&preview.r#type))
                    .when_some(
                        preview.release_info.filter(|s| !s.trim().is_empty()),
                        |el, yr| {
                            el.child(meta_divider())
                                .child(div().font(mono_font()).child(yr))
                        },
                    )
                    .when_some(preview.imdb_rating, |el, r| {
                        el.child(meta_divider()).child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.5))
                                .font(mono_font())
                                .text_color(rgb(TEXT_PRIMARY))
                                .child(svg_icon(IconName::Star, 12.5, rgb(ACCENT_AMBER)))
                                .child(r),
                        )
                    }),
            )
            .child(if let Some(logo_img) = title_logo_tex {
                img(logo_img)
                    .h(px(84.0))
                    .max_w(px(420.0))
                    .object_fit(gpui::ObjectFit::Contain)
                    .into_any_element()
            } else {
                div()
                    .font(editorial_serif())
                    .text_size(px(58.0))
                    .line_height(gpui::relative(1.06))
                    .text_color(rgb(TEXT_PRIMARY))
                    .child(preview.name)
                    .into_any_element()
            })
            .when_some(preview.description, |el, desc| {
                el.child(
                    div()
                        .max_w(px(640.0))
                        .text_size(px(14.5))
                        .text_color(rgb(TEXT_PRIMARY))
                        .opacity(0.88)
                        .child(desc),
                )
            })
            .when(!loading_genres.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.0))
                        .children(loading_genres.into_iter().enumerate().map(|(idx, genre)| {
                            let g_type = loading_type.clone();
                            let g_val = genre.clone();
                            render_genre_chip(
                                idx,
                                genre,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_discover_genre(&g_type, &g_val, cx);
                                }),
                            )
                        })),
                )
            })
            .into_any_element();

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
                    .child(Self::render_hero_backdrop_shell(
                        &preview.id,
                        preblurred_backdrop,
                        hero_backdrop,
                        poster_tex,
                        "btn-details-back",
                        right_col,
                        cx,
                    ))
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
                                    .rounded(px(RADIUS_CARD))
                                    .bg(rgba(GLASS_CARD_BG))
                                    .border_1()
                                    .border_color(rgba(GLASS_BORDER))
                                    .font(editorial_serif())
                                    .text_size(px(24.0))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(status_text),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_series_seasons_panel(
        &mut self,
        args: SeasonsPanelArgs<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let SeasonsPanelArgs {
            type_name,
            meta_id,
            meta_preview,
            meta_request,
            in_library,
            seasons,
            active_season,
            episodes,
            active_video_id,
            series_runtime,
            library_item,
            watched,
            best_play_for_selected,
            panel_width_px,
        } = args;

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

        let ep_count = episodes.len();
        let all_season_watched =
            ep_count > 0 && watched.is_some_and(|w| episodes.iter().all(|ep| w.get_video(&ep.id)));

        let scroll_key = (meta_id.to_owned(), active_video_id.to_owned());
        if self.episode_list_scrolled_key.as_ref() != Some(&scroll_key) {
            if let Some(active_idx) = episodes.iter().position(|ep| ep.id == active_video_id) {
                self.episode_list_scroll_handle
                    .scroll_to_item(active_idx, gpui::ScrollStrategy::Center);
            }
            self.episode_list_scrolled_key = Some(scroll_key);
        }

        let resume_info = library_item.and_then(|li| {
            li.state
                .video_id
                .as_ref()
                .map(|vid| (vid.clone(), li.state.time_offset, li.state.duration))
        });
        let watched_snap = watched.cloned();
        let sel_type = type_name.to_owned();
        let sel_id = meta_id.to_owned();
        let active_vid_owned = active_video_id.to_owned();
        let list_height_px = if ep_count == 0 {
            96.0
        } else {
            (ep_count as f32 * EPISODE_ROW_STRIDE_PX).min(EPISODE_LIST_MAX_HEIGHT_PX)
        };

        let mut panel = div().flex().flex_col().gap(px(14.0));
        if let Some(w_px) = panel_width_px {
            panel = panel.w(px(w_px)).flex_shrink_0();
        } else {
            panel = panel.w_full();
        }

        panel
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(10.0))
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
                    .when_some(active_season.filter(|_| ep_count > 0), |hdr, season_num| {
                        hdr.child(secondary_glass_button(
                            "meta-btn-mark-season-watched",
                            Some(if all_season_watched {
                                IconName::Check
                            } else {
                                IconName::Eye
                            }),
                            if all_season_watched {
                                "Season watched"
                            } else {
                                "Mark season watched"
                            },
                            all_season_watched,
                            cx.listener(move |this, _, _, cx| {
                                this.core
                                    .mark_season_watched(season_num, !all_season_watched);
                                cx.notify();
                            }),
                        ))
                    }),
            )
            .child(
                div()
                    .id("season-pills-scroll")
                    .w_full()
                    .overflow_x_scroll()
                    .pb(px(2.0))
                    .child(
                        div().flex().child(
                            glass_segmented_bar("meta-season-tabs-bar").children(
                                seasons.iter().copied().enumerate().map(|(idx, s_num)| {
                                    let is_active = active_season == Some(s_num);
                                    let label = if s_num == 0 {
                                        "Specials".to_owned()
                                    } else {
                                        format!("Season {s_num}")
                                    };
                                    glass_segmented_item(
                                        ("season-pill", idx),
                                        label,
                                        is_active,
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
                                }),
                            ),
                        ),
                    ),
            )
            .child(if ep_count == 0 {
                div()
                    .w_full()
                    .h(px(list_height_px))
                    .rounded(px(RADIUS_CARD))
                    .bg(rgba(GLASS_CARD_BG))
                    .border_1()
                    .border_color(rgba(GLASS_BORDER))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(rgb(TEXT_MUTED))
                    .child("No episodes listed for this season.")
                    .into_any_element()
            } else {
                div()
                    .id("meta-season-episodes-frame")
                    .w_full()
                    .p(px(8.0))
                    .rounded(px(RADIUS_CARD))
                    .bg(rgba(GLASS_CARD_BG))
                    .border_1()
                    .border_color(rgba(GLASS_BORDER))
                    .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                        let dy = f32::from(event.delta.pixel_delta(window.line_height()).y);
                        if dy.abs() <= 0.1 {
                            return;
                        }
                        let guard = this.episode_list_scroll_handle.0.borrow();
                        let max_y = f32::from(guard.base_handle.max_offset().y);
                        if max_y <= 1.0 {
                            return;
                        }
                        let cur_y = f32::from(guard.base_handle.offset().y);
                        let prev_y = cur_y - dy;
                        let can_scroll_down = dy < 0.0 && prev_y > -max_y + 1.0;
                        let can_scroll_up = dy > 0.0 && prev_y < -1.0;
                        if can_scroll_down || can_scroll_up {
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        gpui::uniform_list(
                            "meta-season-episodes",
                            ep_count,
                            cx.processor(move |this, visible_rows: Range<usize>, _window, cx| {
                                visible_rows
                                    .map(|idx| {
                                        let video = &episodes[idx];
                                        let ep = format_episode_row(
                                            idx,
                                            video,
                                            &sel_id,
                                            &active_vid_owned,
                                            runtime_label.as_deref(),
                                            resume_info.as_ref(),
                                            watched_snap.as_ref(),
                                        );
                                        let thumb_tex = ep.thumb_url.as_deref().and_then(|u| {
                                            this.poster_cache.resolve_episode_still_image(u)
                                        });
                                        let click_type = sel_type.clone();
                                        let click_id = sel_id.clone();
                                        let click_vid = ep.video_id.clone();
                                        let play_type = sel_type.clone();
                                        let play_id = sel_id.clone();
                                        let play_vid = ep.video_id.clone();
                                        let play_selected = ep.is_selected;
                                        let best_play = best_play_for_selected.clone();
                                        let play_mreq = meta_request.clone();
                                        let play_title = meta_preview.name.clone();
                                        let ep_for_watch = video.clone();
                                        let ep_is_watched = ep.is_watched;
                                        let ctx_type = sel_type.clone();
                                        let ctx_id = sel_id.clone();
                                        let ctx_vid = ep.video_id.clone();
                                        let ctx_title = format!("{} · {}", ep.ep_code, ep.title);
                                        let ctx_preview = meta_preview.clone();
                                        let ctx_video = video.clone();
                                        let ctx_has_progress =
                                            ep.progress_fraction.is_some() && !ep.is_watched;
                                        let grp: SharedString =
                                            format!("episode-card-grp-{idx}").into();

                                        div()
                                            .w_full()
                                            .h(px(EPISODE_ROW_STRIDE_PX))
                                            .py(px(4.0))
                                            .child(
                                                div()
                                                    .id(("episode-row", idx))
                                                    .group(grp.clone())
                                                    .cursor_pointer()
                                                    .w_full()
                                                    .h(px(EPISODE_ROW_CARD_HEIGHT_PX))
                                                    .p(px(10.0))
                                                    .rounded(px(RADIUS_CONTROL))
                                                    .bg(if ep.is_selected {
                                                        rgba(ACCENT_SOFT)
                                                    } else {
                                                        rgba(GLASS_TROUGH_BG)
                                                    })
                                                    .border_1()
                                                    .border_color(if ep.is_selected {
                                                        rgba(BORDER_ACCENT)
                                                    } else {
                                                        rgba(GLASS_BORDER)
                                                    })
                                                    .hover(|s| {
                                                        if ep.is_selected {
                                                            s.bg(rgba(ACCENT_SOFT))
                                                        } else {
                                                            s.bg(rgba(GLASS_ROW_HOVER_BG))
                                                                .border_color(rgba(
                                                                    GLASS_BORDER_STRONG,
                                                                ))
                                                        }
                                                    })
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(12.0))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            this.pending_quick_resume = None;
                                                            this.select_episode_for_streams(
                                                                click_type.clone(),
                                                                click_id.clone(),
                                                                click_vid.clone(),
                                                                cx,
                                                            );
                                                        }),
                                                    )
                                                    .on_mouse_down(
                                                        MouseButton::Right,
                                                        cx.listener(
                                                            move |this,
                                                                  ev: &MouseDownEvent,
                                                                  _,
                                                                  cx| {
                                                                cx.stop_propagation();
                                                                let menu =
                                                                    BrowsingContextMenu::new(
                                                                        ev.position,
                                                                        ctx_type.clone(),
                                                                        ctx_id.clone(),
                                                                        ctx_title.clone(),
                                                                    )
                                                                    .video_id(Some(
                                                                        ctx_vid.clone(),
                                                                    ))
                                                                    .preview(Some(
                                                                        ctx_preview.clone(),
                                                                    ))
                                                                    .is_in_library(in_library)
                                                                    .is_watched(ep_is_watched)
                                                                    .can_rewind_cw(ctx_has_progress)
                                                                    .video(Some((
                                                                        ctx_video.clone(),
                                                                        ep_is_watched,
                                                                    )));
                                                                this.open_browsing_context_menu(
                                                                    menu, cx,
                                                                );
                                                            },
                                                        ),
                                                    )
                                                    .child(
                                                        div()
                                                            .w(px(160.0))
                                                            .h(px(90.0))
                                                            .flex_shrink_0()
                                                            .rounded(px(RADIUS_POSTER))
                                                            .overflow_hidden()
                                                            .relative()
                                                            .bg(rgb(placeholder_tint_for_id(
                                                                &ep.video_id,
                                                            )))
                                                            .border_1()
                                                            .border_color(rgba(GLASS_BORDER))
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
                                                                    .id(("ep-thumb-play", idx))
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
                                                                    .on_mouse_down(
                                                                        MouseButton::Left,
                                                                        cx.listener(
                                                                            move |this, _, _, cx| {
                                                                                cx.stop_propagation();
                                                                                if play_selected {
                                                                                    if let Some(BestEpisodePlay {
                                                                                        candidates,
                                                                                        index,
                                                                                        episode_label,
                                                                                        resume_secs,
                                                                                    }) = best_play
                                                                                        .as_ref()
                                                                                    {
                                                                                        let subs = Some(build_subtitles_resource_path(&play_type, &play_vid));
                                                                                        this.start_playback_session(
                                                                                            play_type.clone(),
                                                                                            play_id.clone(),
                                                                                            play_vid.clone(),
                                                                                            play_title.clone(),
                                                                                            episode_label.clone(),
                                                                                            Some(play_mreq.clone()),
                                                                                            subs,
                                                                                            Arc::clone(candidates),
                                                                                            *index,
                                                                                            *resume_secs,
                                                                                            cx,
                                                                                        );
                                                                                        return;
                                                                                    }
                                                                                }
                                                                                this.select_episode_for_streams(
                                                                                    play_type.clone(),
                                                                                    play_id.clone(),
                                                                                    play_vid.clone(),
                                                                                    cx,
                                                                                );
                                                                                this.pending_quick_resume = Some((
                                                                                    play_type.clone(),
                                                                                    play_id.clone(),
                                                                                    Some(play_vid.clone()),
                                                                                ));
                                                                            },
                                                                        ),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .w(px(32.0))
                                                                            .h(px(32.0))
                                                                            .rounded_full()
                                                                            .bg(if ep.is_selected {
                                                                                rgb(ACCENT)
                                                                            } else {
                                                                                rgba(0x141417eb)
                                                                            })
                                                                            .border_1()
                                                                            .border_color(
                                                                                if ep.is_selected {
                                                                                    rgb(ACCENT)
                                                                                } else {
                                                                                    rgba(GLASS_BORDER_STRONG)
                                                                                },
                                                                            )
                                                                            .flex()
                                                                            .items_center()
                                                                            .justify_center()
                                                                            .child(svg_icon(
                                                                                IconName::Play,
                                                                                13.0,
                                                                                if ep.is_selected {
                                                                                    rgb(CTA_ON_ACCENT)
                                                                                } else {
                                                                                    rgb(TEXT_PRIMARY)
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
                                                            .flex()
                                                            .flex_col()
                                                            .justify_between()
                                                            .gap(px(3.0))
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
                                                                                rgba(GLASS_TROUGH_BG)
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
                                                                                    rgb(TEXT_SECONDARY)
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
                                                                                FontWeight::SEMIBOLD,
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
                                                                    .child(
                                                                        div()
                                                                            .id((
                                                                                "ep-mark-watched",
                                                                                idx,
                                                                            ))
                                                                            .flex_shrink_0()
                                                                            .cursor_pointer()
                                                                            .h(px(24.0))
                                                                            .px(px(if ep.is_watched {
                                                                                8.0
                                                                            } else {
                                                                                6.0
                                                                            }))
                                                                            .rounded(px(
                                                                                RADIUS_PILL,
                                                                            ))
                                                                            .bg(if ep.is_watched {
                                                                                rgba(0x4ade801c)
                                                                            } else {
                                                                                rgba(GLASS_TROUGH_BG)
                                                                            })
                                                                            .border_1()
                                                                            .border_color(
                                                                                if ep.is_watched {
                                                                                    rgba(0x4ade804d)
                                                                                } else {
                                                                                    rgba(GLASS_BORDER)
                                                                                },
                                                                            )
                                                                            .hover(|s| {
                                                                                s.border_color(rgba(
                                                                                    GLASS_BORDER_STRONG,
                                                                                ))
                                                                            })
                                                                            .flex()
                                                                            .items_center()
                                                                            .justify_center()
                                                                            .gap(px(4.0))
                                                                            .text_size(px(10.5))
                                                                            .font_weight(
                                                                                FontWeight::MEDIUM,
                                                                            )
                                                                            .text_color(
                                                                                if ep.is_watched {
                                                                                    rgb(STATUS_WATCHED)
                                                                                } else {
                                                                                    rgb(TEXT_MUTED)
                                                                                },
                                                                            )
                                                                            .on_mouse_down(
                                                                                MouseButton::Left,
                                                                                cx.listener(
                                                                                    move |this,
                                                                                          _,
                                                                                          _,
                                                                                          cx| {
                                                                                        cx.stop_propagation();
                                                                                        this.core.mark_video_watched(
                                                                                            ep_for_watch.clone(),
                                                                                            !ep_is_watched,
                                                                                        );
                                                                                        cx.notify();
                                                                                    },
                                                                                ),
                                                                            )
                                                                            .child(svg_icon(
                                                                                if ep.is_watched {
                                                                                    IconName::Check
                                                                                } else {
                                                                                    IconName::Eye
                                                                                },
                                                                                11.0,
                                                                                if ep.is_watched {
                                                                                    rgb(STATUS_WATCHED)
                                                                                } else {
                                                                                    rgb(TEXT_MUTED)
                                                                                },
                                                                            ))
                                                                            .when(
                                                                                ep.is_watched,
                                                                                |btn| {
                                                                                    btn.child(
                                                                                        "Watched",
                                                                                    )
                                                                                },
                                                                            ),
                                                                    ),
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
                                                                                    .bg(rgba(
                                                                                        GLASS_TROUGH_BG,
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
                                                                    .text_size(px(12.0))
                                                                    .line_height(px(16.5))
                                                                    .line_clamp(2)
                                                                    .text_color(
                                                                        if ep.synopsis.is_some() {
                                                                            rgb(TEXT_SECONDARY)
                                                                        } else {
                                                                            rgb(TEXT_MUTED)
                                                                        },
                                                                    )
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
                        .w_full()
                        .h(px(list_height_px)),
                    )
                    .into_any_element()
            })
    }

    fn render_stream_picker_panel(
        &self,
        args: StreamPickerPanelArgs<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let StreamPickerPanelArgs {
            type_name,
            meta_id,
            active_video_id,
            title,
            episode_label,
            runtime_minutes,
            meta_request,
            resume_secs,
            has_debrid,
            all_playable_arc,
            available_addons_arc,
            filtered_indices,
            last_used_orig_idx,
            addons_ready,
            addons_total,
            has_stream_request,
            has_stream_addons,
        } = args;

        let total_unfiltered = all_playable_arc.len();
        let filtered_total = filtered_indices.len();
        let total_pages = filtered_total.div_ceil(STREAM_PAGE_SIZE).max(1);
        let cur_page = ((self.visible_stream_limit.max(STREAM_PAGE_SIZE) - 1) / STREAM_PAGE_SIZE)
            .min(total_pages - 1);
        let page_start = cur_page * STREAM_PAGE_SIZE;
        let page_end = (page_start + STREAM_PAGE_SIZE).min(filtered_total);

        let all_res_active = self.stream_filter.resolutions.is_empty() && !self.stream_filter.hdr_only;
        let is_unfiltered = self.stream_filter.is_unfiltered(has_debrid);
        let addon_label = self
            .stream_filter
            .addon_dropdown_label(&available_addons_arc);
        let addon_filter_active = !self.stream_filter.selected_addons.is_empty();
        let addon_dropdown_open = self.meta_addon_dropdown_open;

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_baseline()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .font(editorial_serif())
                                    .text_size(px(30.0))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .child("Sources"),
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
                                    .font_features(tabular_nums())
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(format!(
                                        "{filtered_total} of {total_unfiltered} ({addons_ready}/{addons_total} addons)"
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                glass_segmented_bar("meta-stream-filter-bar")
                                    .child(glass_segmented_item(
                                        "stream-filter-all",
                                        "All",
                                        all_res_active,
                                        cx.listener(|this, _, _, cx| {
                                            this.stream_filter.resolutions.clear();
                                            this.stream_filter.hdr_only = false;
                                            this.expanded_stream_rows.clear();
                                            this.visible_stream_limit = STREAM_PAGE_SIZE;
                                            cx.notify();
                                        }),
                                    ))
                                    .children(StreamResolutionChoice::ALL.into_iter().enumerate().map(
                                        |(idx, res)| {
                                            let active = !self.stream_filter.hdr_only
                                                && self.stream_filter.resolutions.len() == 1
                                                && self.stream_filter.resolutions.contains(&res);
                                            glass_segmented_item(
                                                ("stream-filter-res", idx),
                                                res.label(),
                                                active,
                                                cx.listener(move |this, _, _, cx| {
                                                    let is_sole_active = !this
                                                        .stream_filter
                                                        .hdr_only
                                                        && this.stream_filter.resolutions.len()
                                                            == 1
                                                        && this
                                                            .stream_filter
                                                            .resolutions
                                                            .contains(&res);
                                                    if is_sole_active {
                                                        this.stream_filter.resolutions.clear();
                                                    } else {
                                                        this.stream_filter.resolutions.clear();
                                                        this.stream_filter.resolutions.insert(res);
                                                        this.stream_filter.hdr_only = false;
                                                    }
                                                    this.expanded_stream_rows.clear();
                                                    this.visible_stream_limit = STREAM_PAGE_SIZE;
                                                    cx.notify();
                                                }),
                                            )
                                        },
                                    ))
                                    .child(glass_segmented_item(
                                        "stream-filter-hdr",
                                        "HDR",
                                        self.stream_filter.hdr_only,
                                        cx.listener(|this, _, _, cx| {
                                            this.stream_filter.hdr_only =
                                                !this.stream_filter.hdr_only;
                                            if this.stream_filter.hdr_only {
                                                this.stream_filter.resolutions.clear();
                                            }
                                            this.expanded_stream_rows.clear();
                                            this.visible_stream_limit = STREAM_PAGE_SIZE;
                                            cx.notify();
                                        }),
                                    )),
                            )
                            .when(has_debrid, |bar| {
                                bar.child(render_compact_glass_toggle(
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
                            })
                            .when(!available_addons_arc.is_empty(), |bar| {
                                let addons_for_menu = Arc::clone(&available_addons_arc);
                                bar.child(
                                    div()
                                        .relative()
                                        .child(
                                            div()
                                                .id("stream-addon-dropdown-btn")
                                                .cursor_pointer()
                                                .h(px(38.0))
                                                .px(px(12.0))
                                                .rounded(px(RADIUS_CONTROL))
                                                .bg(if addon_filter_active || addon_dropdown_open {
                                                    rgba(ACCENT_SOFT)
                                                } else {
                                                    rgba(GLASS_TROUGH_BG)
                                                })
                                                .border_1()
                                                .border_color(
                                                    if addon_filter_active || addon_dropdown_open {
                                                        rgba(BORDER_ACCENT)
                                                    } else {
                                                        rgba(GLASS_BORDER)
                                                    },
                                                )
                                                .hover(|s| {
                                                    s.bg(rgba(GLASS_ROW_HOVER_BG)).border_color(
                                                        if addon_filter_active
                                                            || addon_dropdown_open
                                                        {
                                                            rgb(ACCENT)
                                                        } else {
                                                            rgba(GLASS_BORDER_STRONG)
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
                                                    12.0,
                                                    if addon_filter_active || addon_dropdown_open {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgb(TEXT_SECONDARY)
                                                    },
                                                ))
                                                .child(addon_label)
                                                .child(svg_icon(
                                                    IconName::ChevronDown,
                                                    12.0,
                                                    if addon_filter_active || addon_dropdown_open {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgb(TEXT_MUTED)
                                                    },
                                                )),
                                        )
                                        .when(addon_dropdown_open, |wrap| {
                                            wrap.child(
                                                gpui::deferred(
                                                    glass_dropdown_panel(
                                                        "stream-addon-dropdown-menu",
                                                        292.0,
                                                    )
                                                    .absolute()
                                                    .top(px(44.0))
                                                    .right_0()
                                                    .shadow(glass_capsule_shadow())
                                                    .child(
                                                        div()
                                                            .id("stream-addon-dropdown-scroll")
                                                            .w_full()
                                                            .max_h(px(320.0))
                                                            .overflow_y_scroll()
                                                            .flex()
                                                            .flex_col()
                                                            .gap(px(2.0))
                                                            .child(glass_dropdown_option(
                                                                "stream-addon-opt-all",
                                                                "All addons",
                                                                Some(SharedString::from(format!(
                                                                    "{total_unfiltered} streams"
                                                                ))),
                                                                !addon_filter_active,
                                                                cx.listener(|this, _, _, cx| {
                                                                    this.stream_filter
                                                                        .selected_addons
                                                                        .clear();
                                                                    this.expanded_stream_rows
                                                                        .clear();
                                                                    this.visible_stream_limit =
                                                                        STREAM_PAGE_SIZE;
                                                                    this.meta_addon_dropdown_open =
                                                                        false;
                                                                    cx.notify();
                                                                }),
                                                            ))
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
                                                                            glass_dropdown_option(
                                                                                (
                                                                                    "stream-addon-opt",
                                                                                    a_idx,
                                                                                ),
                                                                                a_name,
                                                                                Some(SharedString::from(format!("{count} streams"))),
                                                                                selected,
                                                                                cx.listener(move |this, _, _, cx| {
                                                                                    this.stream_filter.toggle_addon(&a_key);
                                                                                    this.expanded_stream_rows.clear();
                                                                                    this.visible_stream_limit = STREAM_PAGE_SIZE;
                                                                                    cx.notify();
                                                                                }),
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
                            .when(!is_unfiltered, |bar| {
                                bar.child(render_compact_glass_toggle(
                                    "stream-filter-reset",
                                    "Reset",
                                    Some(IconName::X),
                                    false,
                                    cx.listener(|this, _, _, cx| {
                                        this.stream_filter.reset();
                                        this.meta_addon_dropdown_open = false;
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
                        "Reset filters to view every available stream.",
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
                    .rounded(px(RADIUS_CARD))
                    .bg(rgba(GLASS_CARD_BG))
                    .border_1()
                    .border_color(rgba(GLASS_BORDER))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(8.0))
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
                    .when(!is_querying && total_unfiltered > 0 && !is_unfiltered, |el| {
                        el.child(
                            div().pt(px(6.0)).child(secondary_glass_button(
                                "btn-streams-empty-reset",
                                Some(IconName::RotateCcw),
                                "Reset filters",
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.stream_filter.reset();
                                    this.meta_addon_dropdown_open = false;
                                    this.expanded_stream_rows.clear();
                                    this.visible_stream_limit = STREAM_PAGE_SIZE;
                                    cx.notify();
                                }),
                            )),
                        )
                    })
                    .into_any_element()
            } else {
                let page_slice = &filtered_indices[page_start..page_end];
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .children(page_slice.iter().copied().map(|orig_idx| {
                        let cand = &all_playable_arc[orig_idx];
                        let summary = summarize_stream_for_humans(&cand.parsed);
                        let headline = [
                            Some(summary.resolution),
                            compact_hdr_badge(cand.parsed.hdr),
                            compact_codec_badge(cand.parsed.codec),
                            compact_audio_badge(cand.parsed.audio),
                        ]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · ");

                        let bitrate_text =
                            estimate_bitrate_mbps(cand.parsed.size_bytes, runtime_minutes)
                                .map(format_bitrate_mbps);
                        let metadata_line = [
                            summary.size_text.clone(),
                            bitrate_text,
                            Some(summary.availability_text.clone()),
                        ]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · ");

                        let release_line = cand.raw_release_filename();
                        let release_title =
                            (!release_line.is_empty()).then(|| SharedString::from(release_line.clone()));
                        let addon_display = summary
                            .addon_label
                            .or_else(|| {
                                (!cand.addon_name.trim().is_empty())
                                    .then(|| cand.addon_name.clone())
                            })
                            .map(SharedString::from);

                        let is_last_used = last_used_orig_idx == Some(orig_idx);
                        let is_expanded = self.expanded_stream_rows.contains(&orig_idx);
                        let raw_filename = is_expanded
                            .then(|| (!release_line.is_empty()).then(|| SharedString::from(release_line)))
                            .flatten();
                        let info_hash_str = cand.info_hash_hex();
                        let info_hash = is_expanded
                            .then(|| info_hash_str.clone().map(SharedString::from))
                            .flatten();
                        let technical_specs: Vec<SharedString> = if is_expanded {
                            summary
                                .technical_specs
                                .into_iter()
                                .map(SharedString::from)
                                .collect()
                        } else {
                            Vec::new()
                        };

                        let copy_url_pair = is_expanded
                            .then(|| stream_copyable_url_or_magnet(cand))
                            .flatten();
                        let copy_hash_val = is_expanded.then_some(info_hash_str).flatten();

                        let cands_for_click = Arc::clone(&all_playable_arc);
                        let s_type = type_name.to_owned();
                        let s_id = meta_id.to_owned();
                        let s_vid = active_video_id.to_owned();
                        let s_title = title.to_owned();
                        let s_ep = episode_label.clone();
                        let s_mreq = meta_request.clone();

                        let mut props = StreamSourceRowProps::new(
                            ("stream-row", orig_idx),
                            headline,
                            metadata_line,
                        )
                        .release_title(release_title)
                        .addon_label(addon_display)
                        .is_cached(cand.parsed.is_cached)
                        .is_last_used(is_last_used)
                        .is_expanded(is_expanded)
                        .raw_filename(raw_filename)
                        .info_hash(info_hash)
                        .technical_specs(technical_specs)
                        .on_play(cx.listener(move |this, _, _, cx| {
                            let subs = Some(build_subtitles_resource_path(&s_type, &s_vid));
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
                        }))
                        .on_toggle_expand(cx.listener(move |this, _, _, cx| {
                            this.toggle_stream_row_details(orig_idx, cx);
                        }));

                        if let Some((copy_val, is_magnet)) = copy_url_pair {
                            props = props.on_copy_url(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy_val.clone()));
                                this.push_toast(
                                    ToastKind::Success,
                                    if is_magnet {
                                        "Copied magnet link"
                                    } else {
                                        "Copied stream URL"
                                    },
                                    None,
                                    cx,
                                );
                            }));
                        }
                        if let Some(hash_val) = copy_hash_val {
                            props = props.on_copy_hash(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(hash_val.clone()));
                                this.push_toast(
                                    ToastKind::Success,
                                    "Copied info hash",
                                    Some(hash_val.clone()),
                                    cx,
                                );
                            }));
                        }

                        render_stream_source_row(props)
                    }))
                    .when(total_pages > 1, |list| {
                        list.child(self.render_stream_pagination_bar(
                            cur_page,
                            total_pages,
                            page_start,
                            page_end,
                            filtered_total,
                            cx,
                        ))
                    })
                    .into_any_element()
            })
    }

    fn render_stream_pagination_bar(
        &self,
        cur_page: usize,
        total_pages: usize,
        page_start: usize,
        page_end: usize,
        filtered_total: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        let has_prev = cur_page > 0;
        let has_next = cur_page + 1 < total_pages;

        div()
            .mt(px(4.0))
            .px(px(16.0))
            .py(px(10.0))
            .rounded(px(RADIUS_CARD))
            .bg(rgba(GLASS_CARD_BG))
            .border_1()
            .border_color(rgba(GLASS_BORDER))
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(px(12.0))
            .child(
                div()
                    .font(mono_font())
                    .font_features(tabular_nums())
                    .text_xs()
                    .text_color(rgb(TEXT_MUTED))
                    .child(format!(
                        "Showing {}–{} of {filtered_total} streams",
                        page_start + 1,
                        page_end
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(render_pager_step_button(
                        "btn-streams-prev-page",
                        IconName::ChevronLeft,
                        "Prev",
                        has_prev,
                        false,
                        cx.listener(move |this, _, _, cx| {
                            if cur_page > 0 {
                                this.visible_stream_limit = cur_page * STREAM_PAGE_SIZE;
                                this.expanded_stream_rows.clear();
                                cx.notify();
                            }
                        }),
                    ))
                    .when(total_pages <= 7, |row| {
                        row.children((0..total_pages).map(|p_idx| {
                            let active = p_idx == cur_page;
                            div()
                                .id(("btn-streams-page", p_idx))
                                .cursor_pointer()
                                .w(px(30.0))
                                .h(px(30.0))
                                .rounded(px(RADIUS_POSTER))
                                .bg(if active {
                                    rgb(SEGMENT_ACTIVE_BG)
                                } else {
                                    rgba(GLASS_TROUGH_BG)
                                })
                                .border_1()
                                .border_color(if active {
                                    rgb(SEGMENT_ACTIVE_BG)
                                } else {
                                    rgba(GLASS_BORDER)
                                })
                                .hover(|s| {
                                    if active {
                                        s
                                    } else {
                                        s.bg(rgba(GLASS_ROW_HOVER_BG))
                                            .border_color(rgba(GLASS_BORDER_STRONG))
                                    }
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .font(mono_font())
                                .font_features(tabular_nums())
                                .text_xs()
                                .text_color(rgb(if active {
                                    SEGMENT_ACTIVE_TEXT
                                } else {
                                    TEXT_SECONDARY
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.visible_stream_limit = (p_idx + 1) * STREAM_PAGE_SIZE;
                                        this.expanded_stream_rows.clear();
                                        cx.notify();
                                    }),
                                )
                                .child(format!("{}", p_idx + 1))
                        }))
                    })
                    .when(total_pages > 7, |row| {
                        row.child(
                            div()
                                .px(px(10.0))
                                .font(mono_font())
                                .font_features(tabular_nums())
                                .text_xs()
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(format!("Page {} of {total_pages}", cur_page + 1)),
                        )
                    })
                    .child(render_pager_step_button(
                        "btn-streams-next-page",
                        IconName::ChevronRight,
                        "Next",
                        has_next,
                        true,
                        cx.listener(move |this, _, _, cx| {
                            if cur_page + 1 < total_pages {
                                this.visible_stream_limit = (cur_page + 2) * STREAM_PAGE_SIZE;
                                this.expanded_stream_rows.clear();
                                cx.notify();
                            }
                        }),
                    )),
            )
    }
}

fn format_episode_row(
    idx: usize,
    ep: &Video,
    meta_id: &str,
    active_video_id: &str,
    runtime_label: Option<&str>,
    resume_info: Option<&(String, u64, u64)>,
    watched: Option<&WatchedBitField>,
) -> EpisodeRowData {
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
    } else if let Some((saved_vid, time_offset, duration)) = resume_info {
        if saved_vid == &ep.id && *time_offset > 0 && *duration > 0 {
            Some((*time_offset as f32 / *duration as f32).clamp(0.02, 1.0))
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
        runtime_label: runtime_label.map(str::to_owned),
        synopsis,
        is_selected,
        is_watched,
        progress_fraction,
    }
}

fn resolve_trailer_target(
    trailer_streams: &[Stream],
    youtube_key: Option<&str>,
) -> Option<TrailerTarget> {
    if let Some(direct) = trailer_streams.iter().find(|s| {
        matches!(
            &s.source,
            StreamSource::Url { url } if matches!(url.scheme(), "http" | "https")
        )
    }) {
        return Some(TrailerTarget::DirectStream(Box::new(direct.clone())));
    }

    for stream in trailer_streams {
        match &stream.source {
            StreamSource::YouTube { yt_id } if !yt_id.trim().is_empty() => {
                return Some(TrailerTarget::ExternalUrl(format!(
                    "https://www.youtube.com/watch?v={}",
                    yt_id.trim()
                )));
            }
            StreamSource::External {
                external_url: Some(url),
                ..
            } => {
                return Some(TrailerTarget::ExternalUrl(url.to_string()));
            }
            _ => {}
        }
    }

    youtube_key
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|yt_id| TrailerTarget::ExternalUrl(format!("https://www.youtube.com/watch?v={yt_id}")))
}

fn find_last_used_stream_index(
    candidates: &[PlaybackCandidate],
    last_used: Option<&(Url, Stream)>,
) -> Option<usize> {
    let (base_url, last_stream) = last_used?;

    if let Some(idx) = candidates.iter().position(|c| {
        c.stream_request.as_ref().is_some_and(|r| r.base == *base_url)
            && c.stream.is_source_match(last_stream)
    }) {
        return Some(idx);
    }

    if let Some(idx) = candidates
        .iter()
        .position(|c| stream_sources_equivalent(&c.stream.source, &last_stream.source))
    {
        return Some(idx);
    }

    candidates.iter().position(|c| {
        c.stream_request.as_ref().is_some_and(|r| r.base == *base_url)
            && c.stream.is_binge_match(last_stream)
    })
}

fn stream_sources_equivalent(a: &StreamSource, b: &StreamSource) -> bool {
    if a == b {
        return true;
    }
    match (a, b) {
        (
            StreamSource::Torrent {
                info_hash: ha,
                file_idx: fa,
                ..
            },
            StreamSource::Torrent {
                info_hash: hb,
                file_idx: fb,
                ..
            },
        ) => ha == hb && fa == fb,
        _ => false,
    }
}

fn stream_copyable_url_or_magnet(cand: &PlaybackCandidate) -> Option<(String, bool)> {
    match &cand.stream.source {
        StreamSource::Url { url } => Some((url.to_string(), false)),
        StreamSource::Torrent {
            info_hash,
            announce,
            ..
        } => {
            let display = cand.raw_release_filename();
            let display_opt = (!display.trim().is_empty()).then_some(display.as_str());
            Some((
                build_canonical_magnet(*info_hash, announce, display_opt),
                true,
            ))
        }
        StreamSource::YouTube { yt_id } if !yt_id.trim().is_empty() => Some((
            format!("https://www.youtube.com/watch?v={}", yt_id.trim()),
            false,
        )),
        StreamSource::PlayerFrame { player_frame_url } => {
            Some((player_frame_url.to_string(), false))
        }
        StreamSource::External {
            external_url: Some(url),
            ..
        }
        | StreamSource::External {
            android_tv_url: Some(url),
            ..
        } => Some((url.to_string(), false)),
        StreamSource::Nzb { url: Some(url), .. } => Some((url.to_string(), false)),
        StreamSource::Nzb { urls, .. } => urls.first().map(|u| (u.to_string(), false)),
        StreamSource::Rar { urls, .. }
        | StreamSource::Zip { urls, .. }
        | StreamSource::Zip7 { urls, .. }
        | StreamSource::Tgz { urls, .. }
        | StreamSource::Tar { urls, .. } => urls.first().map(|a| (a.url.to_string(), false)),
        _ => None,
    }
}

fn editorial_hit_to_preview(hit: &EditorialMediaHit, canonical_id: &str) -> MetaItemPreview {
    MetaItemPreview {
        id: canonical_id.to_owned(),
        r#type: if hit.media_type.trim().is_empty() {
            "movie".to_owned()
        } else {
            hit.media_type.clone()
        },
        name: hit.title.clone(),
        poster: hit.poster_url.as_deref().and_then(|u| Url::parse(u).ok()),
        background: hit.backdrop_url.as_deref().and_then(|u| Url::parse(u).ok()),
        logo: hit.logo_url.as_deref().and_then(|u| Url::parse(u).ok()),
        description: hit.overview.clone(),
        release_info: hit.year.clone(),
        runtime: None,
        released: None,
        poster_shape: PosterShape::Poster,
        links: Vec::new(),
        trailer_streams: Vec::new(),
        behavior_hints: MetaItemBehaviorHints::default(),
    }
}

fn collection_heading(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.to_ascii_lowercase().starts_with("the ") {
        format!("Part of {trimmed}")
    } else {
        format!("Part of the {trimmed}")
    }
}

fn render_certification_pill(cert: &str) -> Div {
    div()
        .px(px(7.0))
        .py(px(1.5))
        .rounded(px(4.0))
        .bg(rgba(GLASS_TROUGH_BG))
        .border_1()
        .border_color(rgba(GLASS_BORDER_STRONG))
        .font(mono_font())
        .text_size(px(11.0))
        .text_color(rgb(TEXT_PRIMARY))
        .child(cert.to_owned())
}

fn render_genre_chip(
    idx: usize,
    label: String,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
) -> Stateful<Div> {
    div()
        .id(("meta-genre-chip", idx))
        .cursor_pointer()
        .h(px(26.0))
        .px(px(10.0))
        .rounded(px(RADIUS_PILL))
        .bg(rgba(GLASS_TROUGH_BG))
        .border_1()
        .border_color(rgba(GLASS_BORDER))
        .hover(|s| {
            s.bg(rgba(GLASS_ROW_HOVER_BG))
                .border_color(rgba(BORDER_ACCENT))
                .text_color(rgb(TEXT_PRIMARY))
        })
        .flex()
        .items_center()
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(px(12.0))
        .text_color(rgb(TEXT_SECONDARY))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label)
}

fn render_compact_glass_toggle(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    icon: Option<IconName>,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
) -> Stateful<Div> {
    let fg = if active { ACCENT } else { TEXT_SECONDARY };
    div()
        .id(id.into())
        .cursor_pointer()
        .h(px(38.0))
        .px(px(12.0))
        .rounded(px(RADIUS_CONTROL))
        .bg(if active {
            rgba(ACCENT_SOFT)
        } else {
            rgba(GLASS_TROUGH_BG)
        })
        .border_1()
        .border_color(if active {
            rgba(BORDER_ACCENT)
        } else {
            rgba(GLASS_BORDER)
        })
        .hover(|s| {
            s.bg(if active {
                rgba(ACCENT_SOFT)
            } else {
                rgba(GLASS_ROW_HOVER_BG)
            })
            .border_color(if active {
                rgb(ACCENT)
            } else {
                rgba(GLASS_BORDER_STRONG)
            })
        })
        .flex()
        .items_center()
        .gap(px(5.0))
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(px(12.5))
        .text_color(rgb(fg))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .when_some(icon, |el, ic| el.child(svg_icon(ic, 12.0, rgb(fg))))
        .child(label.into())
}

fn render_pager_step_button(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    enabled: bool,
    icon_trailing: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
) -> Stateful<Div> {
    let fg = if enabled { TEXT_PRIMARY } else { TEXT_MUTED };
    let mut btn = div()
        .id(id)
        .h(px(30.0))
        .px(px(10.0))
        .rounded(px(RADIUS_POSTER))
        .bg(rgba(GLASS_TROUGH_BG))
        .border_1()
        .border_color(rgba(GLASS_BORDER))
        .flex()
        .items_center()
        .gap(px(4.0))
        .font(ui_sans(FontWeight::MEDIUM))
        .text_xs()
        .text_color(rgb(fg));

    if enabled {
        btn = btn
            .cursor_pointer()
            .hover(|s| {
                s.bg(rgba(GLASS_ROW_HOVER_BG))
                    .border_color(rgba(GLASS_BORDER_STRONG))
            })
            .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                cx.stop_propagation();
                on_click(ev, window, cx);
            });
    }

    if icon_trailing {
        btn.child(label).child(svg_icon(icon, 12.0, rgb(fg)))
    } else {
        btn.child(svg_icon(icon, 12.0, rgb(fg))).child(label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stremio_core_runtime::stremio_core::types::resource::{SeriesInfo, StreamBehaviorHints};

    fn make_url_stream(url: &str) -> Stream {
        Stream {
            source: StreamSource::Url {
                url: Url::parse(url).expect("valid url"),
            },
            name: None,
            description: None,
            thumbnail: None,
            subtitles: Vec::new(),
            behavior_hints: StreamBehaviorHints::default(),
        }
    }

    fn make_torrent_stream(hash_byte: u8, binge_group: Option<&str>) -> Stream {
        Stream {
            source: StreamSource::Torrent {
                info_hash: [hash_byte; 20],
                file_idx: Some(1),
                announce: vec!["udp://tracker.example.com:6969".to_owned()],
                file_must_include: Vec::new(),
            },
            name: Some("1080p".to_owned()),
            description: Some("Movie.2024.1080p.WEB-DL.mkv".to_owned()),
            thumbnail: None,
            subtitles: Vec::new(),
            behavior_hints: StreamBehaviorHints {
                binge_group: binge_group.map(str::to_owned),
                filename: Some("Movie.2024.1080p.WEB-DL.mkv".to_owned()),
                ..StreamBehaviorHints::default()
            },
        }
    }

    #[test]
    fn trailer_target_prefers_direct_http_then_youtube_then_tmdb_fallback() {
        let yt = Stream {
            source: StreamSource::YouTube {
                yt_id: "dQw4w9WgXcQ".to_owned(),
            },
            name: None,
            description: None,
            thumbnail: None,
            subtitles: Vec::new(),
            behavior_hints: StreamBehaviorHints::default(),
        };
        let direct = make_url_stream("https://cdn.example.com/trailer.mp4");

        assert_eq!(
            resolve_trailer_target(&[yt.clone(), direct.clone()], Some("fallbackKey")),
            Some(TrailerTarget::DirectStream(Box::new(direct)))
        );
        assert_eq!(
            resolve_trailer_target(&[yt], Some("fallbackKey")),
            Some(TrailerTarget::ExternalUrl(
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_owned()
            ))
        );
        assert_eq!(
            resolve_trailer_target(&[], Some("Way9Dexny3w")),
            Some(TrailerTarget::ExternalUrl(
                "https://www.youtube.com/watch?v=Way9Dexny3w".to_owned()
            ))
        );
        assert_eq!(resolve_trailer_target(&[], Some("   ")), None);
    }

    #[test]
    fn last_used_stream_matches_source_then_binge_group() {
        let base_a = Url::parse("https://addon-a.example.com/manifest.json").unwrap();
        let req_a = build_default_meta_request(&base_a, "movie", "tt0133093");
        let cand0 = PlaybackCandidate::from_stream_and_request(
            make_torrent_stream(0x11, Some("grp-1")),
            Some(req_a.clone()),
            &[],
        );
        let cand1 = PlaybackCandidate::from_stream_and_request(
            make_torrent_stream(0x22, Some("grp-2")),
            Some(req_a),
            &[],
        );
        let list = [cand0, cand1];

        let exact_last = (base_a.clone(), make_torrent_stream(0x22, None));
        assert_eq!(find_last_used_stream_index(&list, Some(&exact_last)), Some(1));

        let binge_last = (base_a, make_torrent_stream(0x99, Some("grp-1")));
        assert_eq!(find_last_used_stream_index(&list, Some(&binge_last)), Some(0));
    }

    #[test]
    fn copyable_stream_url_and_magnet_handle_http_and_torrent() {
        let url_cand = PlaybackCandidate::from_stream_and_request(
            make_url_stream("https://cdn.example.com/stream.mkv"),
            None,
            &[],
        );
        assert_eq!(
            stream_copyable_url_or_magnet(&url_cand),
            Some(("https://cdn.example.com/stream.mkv".to_owned(), false))
        );

        let tor_cand = PlaybackCandidate::from_stream_and_request(
            make_torrent_stream(0xab, None),
            None,
            &[],
        );
        let (magnet, is_magnet) = stream_copyable_url_or_magnet(&tor_cand).unwrap();
        assert!(is_magnet);
        assert!(magnet.starts_with("magnet:?xt=urn:btih:abababab"));
        assert!(magnet.contains("tr=udp%3A%2F%2Ftracker.example.com%3A6969"));
    }

    #[test]
    fn episode_formatter_guards_metahub_urls_for_non_imdb_ids() {
        let video = Video {
            id: "tmdb:123:1:1".to_owned(),
            title: "Pilot".to_owned(),
            released: None,
            overview: Some("Opening chapter.".to_owned()),
            thumbnail: None,
            streams: Vec::new(),
            series_info: Some(SeriesInfo {
                season: 1,
                episode: 1,
            }),
            epg_info: None,
            trailer_streams: Vec::new(),
        };

        let non_imdb = format_episode_row(0, &video, "tmdb:123", "tmdb:123:1:1", None, None, None);
        assert_eq!(non_imdb.thumb_url, None);

        let imdb = format_episode_row(0, &video, "tt0903747", "tt0903747:1:1", None, None, None);
        assert_eq!(
            imdb.thumb_url.as_deref(),
            Some("https://images.metahub.space/episode/medium/tt0903747/tmdb:123:1:1/img")
        );
    }
}
