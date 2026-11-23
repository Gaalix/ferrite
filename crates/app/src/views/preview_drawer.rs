use std::sync::Arc;

use gpui::{
    div, img, linear_color_stop, linear_gradient, prelude::*, px, relative, rgb, rgba,
    AnimationExt, Context, FontWeight, MouseButton, ObjectFit,
};
use stremio_core_runtime::{
    stremio_core::{
        constants::CINEMETA_URL,
        runtime::msg::{Action, ActionCtx},
        types::resource::{
            Link, MetaItem, MetaItemBehaviorHints, MetaItemPreview, PosterShape, Stream,
            StreamSource,
        },
    },
    PersonCreditChip,
};
use url::Url;

use crate::{
    icons::{svg_icon, svg_icon_weighted, IconName, IconWeight},
    shell::AppShell,
    state::{
        classify_library_item_status, extract_cast_and_directors_from_links, extract_genres,
        extract_imdb_rating, find_cached_preview_in_model, format_movietracker_episode_code,
        pick_default_series_episode_id, resolve_ready_meta_item, resume_offset_for_library_item,
        LibraryStatusTab, PlaybackCandidate, ToastKind,
    },
    theme::{
        active_theme, editorial_serif, format_timestamp_secs, glass_capsule_shadow, meta_divider,
        mono_font, obsidian_fade_in, placeholder_tint_for_id, semantic_status_pill,
        sentence_case_label, sp, ui_sans, RADIUS_CONTROL, RADIUS_PILL, RADIUS_SHEET,
    },
    ui_components::{primary_accent_button, secondary_glass_button},
    views::search_view::person_initials,
};

const QUICK_PEEK_WIDTH_PX: f32 = 440.0;
const QUICK_PEEK_HEADER_HEIGHT_PX: f32 = 248.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuickPeekTrailerTarget {
    DirectStream(Box<Stream>),
    ExternalUrl(String),
}

pub fn resolve_quick_peek_trailer_target(
    trailer_streams: &[Stream],
    youtube_key: Option<&str>,
) -> Option<QuickPeekTrailerTarget> {
    if let Some(direct) = trailer_streams.iter().find(|s| {
        matches!(
            &s.source,
            StreamSource::Url { url } if matches!(url.scheme(), "http" | "https")
        )
    }) {
        return Some(QuickPeekTrailerTarget::DirectStream(Box::new(direct.clone())));
    }

    for stream in trailer_streams {
        match &stream.source {
            StreamSource::YouTube { yt_id } if !yt_id.trim().is_empty() => {
                return Some(QuickPeekTrailerTarget::ExternalUrl(format!(
                    "https://www.youtube.com/watch?v={}",
                    yt_id.trim()
                )));
            }
            StreamSource::External {
                external_url: Some(url),
                ..
            } => {
                return Some(QuickPeekTrailerTarget::ExternalUrl(url.to_string()));
            }
            _ => {}
        }
    }

    youtube_key
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|yt_id| {
            QuickPeekTrailerTarget::ExternalUrl(format!("https://www.youtube.com/watch?v={yt_id}"))
        })
}

pub fn format_quick_peek_watch_label(
    video_id: Option<&str>,
    resume_secs: Option<f64>,
) -> String {
    let ep_code = format_movietracker_episode_code(video_id);
    match (ep_code, resume_secs) {
        (Some(code), Some(_)) => format!("Resume {code}"),
        (Some(code), None) => format!("Watch {code}"),
        (None, Some(secs)) => format!("Resume ({})", format_timestamp_secs(secs)),
        (None, None) => "Watch".to_owned(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_quick_peek_fallback_preview(
    id: &str,
    type_name: &str,
    name: &str,
    poster_url: Option<&str>,
    backdrop_url: Option<&str>,
    logo_url: Option<&str>,
    year: Option<&str>,
    runtime: Option<&str>,
    description: Option<&str>,
    imdb_rating: Option<&str>,
    genres: &[String],
) -> MetaItemPreview {
    let mut links = Vec::new();
    if let Some(rating) = imdb_rating.map(str::trim).filter(|s| !s.is_empty()) {
        links.push(Link {
            name: rating.to_owned(),
            category: "imdb".to_owned(),
            url: CINEMETA_URL.clone(),
        });
    }
    for genre in genres {
        let trimmed = genre.trim();
        if !trimmed.is_empty() {
            links.push(Link {
                name: trimmed.to_owned(),
                category: "Genres".to_owned(),
                url: CINEMETA_URL.clone(),
            });
        }
    }

    MetaItemPreview {
        id: id.to_owned(),
        r#type: type_name.to_owned(),
        name: name.to_owned(),
        poster: poster_url.and_then(|u| Url::parse(u).ok()),
        background: backdrop_url.and_then(|u| Url::parse(u).ok()),
        logo: logo_url.and_then(|u| Url::parse(u).ok()),
        description: description.map(ToOwned::to_owned),
        release_info: year.map(ToOwned::to_owned),
        runtime: runtime.map(ToOwned::to_owned),
        released: None,
        poster_shape: PosterShape::Poster,
        links,
        trailer_streams: Vec::new(),
        behavior_hints: MetaItemBehaviorHints::default(),
    }
}

fn non_empty_opt(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

impl AppShell {
    pub fn render_quick_peek_drawer(&self, cx: &mut Context<Self>) -> Option<gpui::Stateful<gpui::Div>> {
        let target = self.quick_peek.as_ref()?.clone();
        let theme = active_theme();
        let accent_ink = if theme.is_light {
            theme.accent_press
        } else {
            theme.accent
        };

        let enriched = self
            .core
            .cached_enriched_meta(&target.id)
            .or_else(|| {
                target
                    .preview
                    .as_ref()
                    .and_then(|p| self.core.cached_enriched_meta(&p.id))
            });

        let canonical_imdb = target
            .id
            .starts_with("tt")
            .then(|| target.id.clone())
            .or_else(|| {
                enriched
                    .as_ref()
                    .and_then(|e| e.imdb_id.clone())
                    .filter(|s| s.starts_with("tt"))
            });

        let (ready_meta, cached_preview, library_item, meta_loading) = self.core.with_model(|m| {
            let ready: Option<MetaItem> = resolve_ready_meta_item(&m.meta_details.meta_items)
                .map(|(item, _)| item.clone())
                .filter(|item| {
                    item.preview.id == target.id
                        || canonical_imdb.as_deref() == Some(item.preview.id.as_str())
                })
                .or_else(|| {
                    self.synthetic_tmdb_meta
                        .as_ref()
                        .filter(|(sid, _)| sid == &target.id)
                        .map(|(_, item)| item.clone())
                });

            let loading = ready.is_none()
                && m.meta_details.selected.as_ref().is_some_and(|sel| {
                    sel.meta_path.id == target.id
                        || canonical_imdb.as_deref() == Some(sel.meta_path.id.as_str())
                })
                && m.meta_details
                    .meta_items
                    .iter()
                    .any(|item| matches!(item.content, Some(stremio_core_runtime::stremio_core::models::common::Loadable::Loading)));

            let cached = find_cached_preview_in_model(m, &target.id).or_else(|| {
                canonical_imdb
                    .as_deref()
                    .and_then(|tt| find_cached_preview_in_model(m, tt))
            });

            let lib = m
                .ctx
                .library
                .items
                .get(&target.id)
                .or_else(|| {
                    canonical_imdb
                        .as_deref()
                        .and_then(|tt| m.ctx.library.items.get(tt))
                })
                .cloned();

            (ready, cached, lib, loading)
        });

        let ready_preview = ready_meta.as_ref().map(|m| &m.preview);
        let target_preview = target.preview.as_ref();

        let effective_id = canonical_imdb
            .clone()
            .or_else(|| ready_preview.map(|p| p.id.clone()))
            .unwrap_or_else(|| target.id.clone());

        let effective_type = non_empty_opt(ready_preview.map(|p| p.r#type.clone()))
            .or_else(|| non_empty_opt(target_preview.map(|p| p.r#type.clone())))
            .or_else(|| non_empty_opt(cached_preview.as_ref().map(|p| p.r#type.clone())))
            .or_else(|| non_empty_opt(enriched.as_ref().map(|e| e.media_type.clone())))
            .unwrap_or_else(|| target.type_name.clone());

        let display_title = non_empty_opt(ready_preview.map(|p| p.name.clone()))
            .or_else(|| non_empty_opt(target_preview.map(|p| p.name.clone())))
            .or_else(|| non_empty_opt(cached_preview.as_ref().map(|p| p.name.clone())))
            .or_else(|| non_empty_opt(enriched.as_ref().map(|e| e.title.clone())))
            .unwrap_or_else(|| target.title.clone());

        let metahub_bg = canonical_imdb
            .as_deref()
            .map(|tt| format!("https://images.metahub.space/background/medium/{tt}/img"));
        let metahub_logo = canonical_imdb
            .as_deref()
            .map(|tt| format!("https://images.metahub.space/logo/medium/{tt}/img"));

        let backdrop_url_str = non_empty_opt(
            ready_preview.and_then(|p| p.background.as_ref().map(|u| u.to_string())),
        )
        .or_else(|| {
            non_empty_opt(
                target_preview.and_then(|p| p.background.as_ref().map(|u| u.to_string())),
            )
        })
        .or_else(|| non_empty_opt(enriched.as_ref().and_then(|e| e.backdrop_url.clone())))
        .or_else(|| {
            non_empty_opt(
                cached_preview
                    .as_ref()
                    .and_then(|p| p.background.as_ref().map(|u| u.to_string())),
            )
        })
        .or_else(|| metahub_bg.clone());

        let logo_url_str =
            non_empty_opt(ready_preview.and_then(|p| p.logo.as_ref().map(|u| u.to_string())))
                .or_else(|| {
                    non_empty_opt(
                        target_preview.and_then(|p| p.logo.as_ref().map(|u| u.to_string())),
                    )
                })
                .or_else(|| non_empty_opt(enriched.as_ref().and_then(|e| e.logo_url.clone())))
                .or_else(|| {
                    non_empty_opt(
                        cached_preview
                            .as_ref()
                            .and_then(|p| p.logo.as_ref().map(|u| u.to_string())),
                    )
                })
                .or_else(|| metahub_logo.clone());

        let poster_url_str = non_empty_opt(
            ready_preview.and_then(|p| p.poster.as_ref().map(|u| u.to_string())),
        )
        .or_else(|| {
            non_empty_opt(target_preview.and_then(|p| p.poster.as_ref().map(|u| u.to_string())))
        })
        .or_else(|| non_empty_opt(enriched.as_ref().and_then(|e| e.poster_url.clone())))
        .or_else(|| {
            non_empty_opt(
                cached_preview
                    .as_ref()
                    .and_then(|p| p.poster.as_ref().map(|u| u.to_string())),
            )
        })
        .or_else(|| {
            non_empty_opt(
                library_item
                    .as_ref()
                    .and_then(|li| li.poster.as_ref().map(|u| u.to_string())),
            )
        });

        let ambient_candidates: Vec<&str> = backdrop_url_str
            .as_deref()
            .into_iter()
            .chain(metahub_bg.as_deref())
            .chain(poster_url_str.as_deref())
            .collect();
        let ambient_tex = self
            .poster_cache
            .resolve_first_available_ambient(ambient_candidates);

        let hero_candidates: Vec<&str> = backdrop_url_str
            .as_deref()
            .into_iter()
            .chain(metahub_bg.as_deref())
            .chain(poster_url_str.as_deref())
            .collect();
        let backdrop_tex = self
            .poster_cache
            .resolve_first_available_hero(hero_candidates);

        let logo_candidates: Vec<&str> = logo_url_str
            .as_deref()
            .into_iter()
            .chain(metahub_logo.as_deref())
            .collect();
        let logo_tex = self.poster_cache.resolve_first_available_logo(logo_candidates);

        let year = non_empty_opt(ready_preview.and_then(|p| p.release_info.clone()))
            .or_else(|| non_empty_opt(target_preview.and_then(|p| p.release_info.clone())))
            .or_else(|| {
                non_empty_opt(
                    cached_preview
                        .as_ref()
                        .and_then(|p| p.release_info.clone()),
                )
            })
            .or_else(|| non_empty_opt(enriched.as_ref().and_then(|e| e.year.clone())));

        let runtime = non_empty_opt(ready_preview.and_then(|p| p.runtime.clone()))
            .or_else(|| non_empty_opt(target_preview.and_then(|p| p.runtime.clone())))
            .or_else(|| non_empty_opt(cached_preview.as_ref().and_then(|p| p.runtime.clone())))
            .or_else(|| non_empty_opt(enriched.as_ref().and_then(|e| e.runtime.clone())));

        let imdb_rating = ready_preview
            .and_then(extract_imdb_rating)
            .or_else(|| target_preview.and_then(extract_imdb_rating))
            .or_else(|| cached_preview.as_ref().and_then(|p| p.imdb_rating.clone()))
            .or_else(|| non_empty_opt(enriched.as_ref().and_then(|e| e.imdb_rating.clone())));

        let certification =
            non_empty_opt(enriched.as_ref().and_then(|e| e.certification.clone()));
        let tagline = non_empty_opt(enriched.as_ref().and_then(|e| e.tagline.clone()));

        let mut genres = ready_preview
            .map(|p| extract_genres(p, 4))
            .filter(|g| !g.is_empty())
            .or_else(|| {
                target_preview
                    .map(|p| extract_genres(p, 4))
                    .filter(|g| !g.is_empty())
            })
            .or_else(|| {
                cached_preview
                    .as_ref()
                    .map(|p| p.genres.clone())
                    .filter(|g| !g.is_empty())
            })
            .unwrap_or_default();
        if genres.is_empty() {
            if let Some(enr) = enriched.as_ref() {
                genres = enr.genres.iter().take(4).cloned().collect();
            }
        }

        let synopsis = non_empty_opt(ready_preview.and_then(|p| p.description.clone()))
            .or_else(|| non_empty_opt(target_preview.and_then(|p| p.description.clone())))
            .or_else(|| {
                non_empty_opt(
                    cached_preview
                        .as_ref()
                        .and_then(|p| p.description.clone()),
                )
            })
            .or_else(|| non_empty_opt(enriched.as_ref().and_then(|e| e.overview.clone())));

        let (mut directors, mut cast) = enriched
            .as_ref()
            .filter(|e| !e.directors.is_empty() || !e.cast.is_empty())
            .map(|e| (e.directors.clone(), e.cast.clone()))
            .unwrap_or_default();
        if directors.is_empty() && cast.is_empty() {
            if let Some(p) = ready_preview.or(target_preview) {
                let (d, c) = extract_cast_and_directors_from_links(&p.links);
                directors = d;
                cast = c;
            }
        }
        let mut credits: Vec<PersonCreditChip> = directors.into_iter().take(2).collect();
        credits.extend(cast.into_iter().take(10));

        let library_statuses = self.cached_library_statuses();
        let status_tab = library_statuses
            .get(&effective_id)
            .copied()
            .or_else(|| library_statuses.get(&target.id).copied())
            .or_else(|| library_item.as_ref().and_then(classify_library_item_status));
        let in_library =
            library_item.as_ref().is_some_and(|li| !li.removed) || status_tab.is_some();
        let is_watched = library_item
            .as_ref()
            .is_some_and(|li| !li.removed && (li.watched() || li.progress() >= 95.0))
            || status_tab == Some(LibraryStatusTab::Watched);

        let active_video_id = target
            .video_id
            .clone()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| {
                library_item
                    .as_ref()
                    .and_then(|li| li.state.video_id.clone())
                    .filter(|v| !v.trim().is_empty())
            })
            .or_else(|| {
                ready_meta.as_ref().and_then(|m| {
                    pick_default_series_episode_id(
                        m,
                        target.video_id.as_deref(),
                        library_item.as_ref().and_then(|li| li.state.video_id.as_deref()),
                    )
                })
            });

        let resume_secs = active_video_id
            .as_deref()
            .and_then(|vid| resume_offset_for_library_item(library_item.as_ref(), vid))
            .or_else(|| resume_offset_for_library_item(library_item.as_ref(), &effective_id));

        let watch_label =
            format_quick_peek_watch_label(active_video_id.as_deref(), resume_secs);

        let trailer_streams = ready_preview
            .map(|p| p.trailer_streams.as_slice())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                target_preview
                    .map(|p| p.trailer_streams.as_slice())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or(&[]);
        let trailer_target = resolve_quick_peek_trailer_target(
            trailer_streams,
            enriched
                .as_ref()
                .and_then(|e| e.trailer_youtube_key.as_deref()),
        );

        let preview_for_actions = ready_preview
            .cloned()
            .or_else(|| target_preview.cloned())
            .unwrap_or_else(|| {
                build_quick_peek_fallback_preview(
                    &effective_id,
                    &effective_type,
                    &display_title,
                    poster_url_str.as_deref(),
                    backdrop_url_str.as_deref(),
                    logo_url_str.as_deref(),
                    year.as_deref(),
                    runtime.as_deref(),
                    synopsis.as_deref(),
                    imdb_rating.as_deref(),
                    &genres,
                )
            });

        let scrim_bg = if theme.is_light {
            0x0B0D1447
        } else {
            0x00000073
        };
        let header_fade_top = (theme.glass_sheet_bg & 0xFFFFFF00) | 0x26;
        let header_fade_bottom = theme.glass_sheet_bg;

        let play_type = effective_type.clone();
        let play_id = effective_id.clone();
        let play_vid = active_video_id.clone();

        let details_type = effective_type.clone();
        let details_id = effective_id.clone();
        let details_vid = active_video_id.clone();

        let lib_id = effective_id.clone();
        let lib_preview = preview_for_actions.clone();
        let watch_id = effective_id.clone();
        let watch_preview = preview_for_actions;

        let header = div()
            .w_full()
            .h(sp(QUICK_PEEK_HEADER_HEIGHT_PX))
            .flex_shrink_0()
            .relative()
            .overflow_hidden()
            .bg(rgb(placeholder_tint_for_id(&effective_id)))
            .when_some(ambient_tex, |el, tex| {
                el.child(
                    img(tex)
                        .size_full()
                        .absolute()
                        .inset_0()
                        .object_fit(ObjectFit::Cover)
                        .opacity(0.65),
                )
            })
            .when_some(backdrop_tex, |el, tex| {
                el.child(
                    img(tex)
                        .size_full()
                        .absolute()
                        .inset_0()
                        .object_fit(ObjectFit::Cover)
                        .opacity(0.92),
                )
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(linear_gradient(
                        180.0,
                        linear_color_stop(rgba(header_fade_top), 0.10),
                        linear_color_stop(rgba(header_fade_bottom), 1.0),
                    )),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(linear_gradient(
                        180.0,
                        linear_color_stop(rgba(0x0B0D1410), 0.35),
                        linear_color_stop(rgba(0x0B0D14CC), 1.0),
                    )),
            )
            // Top bar inside header: status badge (left) + close X (right).
            .child(
                div()
                    .absolute()
                    .top(sp(14.0))
                    .left(sp(16.0))
                    .right(sp(14.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(8.0))
                            .when_some(status_tab, |row, st| {
                                let pill = match st {
                                    LibraryStatusTab::Watched => semantic_status_pill(
                                        IconName::Check,
                                        "Watched",
                                        theme.status_watched,
                                        0x4ade8055,
                                        0x0B0D14CC,
                                    ),
                                    LibraryStatusTab::Watching => semantic_status_pill(
                                        IconName::Play,
                                        "Watching",
                                        theme.status_watching,
                                        0x60a5fa55,
                                        0x0B0D14CC,
                                    ),
                                    LibraryStatusTab::Watchlist => semantic_status_pill(
                                        IconName::Bookmark,
                                        "In Library",
                                        theme.status_watchlist,
                                        0xc084fc55,
                                        0x0B0D14CC,
                                    ),
                                };
                                row.child(pill)
                            }),
                    )
                    .child(
                        div()
                            .id("quick-peek-close-btn")
                            .occlude()
                            .cursor_pointer()
                            .w(sp(32.0))
                            .h(sp(32.0))
                            .rounded(px(RADIUS_PILL))
                            .bg(rgba(0x0B0D14B8))
                            .border_1()
                            .border_color(rgba(0xFFFFFF2E))
                            .hover(|s| s.bg(rgba(0x0B0D14E6)).border_color(rgba(0xFFFFFF55)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_quick_peek(cx);
                                }),
                            )
                            .child(svg_icon(
                                IconName::X,
                                14.5 * theme.ui_scale,
                                rgb(0xF5F1E8),
                            )),
                    ),
            )
            // Bottom title / logo lockup over dark scrim.
            .child(
                div()
                    .absolute()
                    .left(sp(22.0))
                    .right(sp(22.0))
                    .bottom(sp(14.0))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(sp(4.0))
                    .child(if let Some(ltex) = logo_tex {
                        img(ltex)
                            .h(sp(56.0))
                            .max_w(sp(280.0))
                            .object_fit(ObjectFit::Contain)
                            .into_any_element()
                    } else {
                        div()
                            .w_full()
                            .font(editorial_serif())
                            .text_size(sp(30.0))
                            .line_height(relative(1.08))
                            .text_color(rgb(0xF5F1E8))
                            .child(display_title.clone())
                            .into_any_element()
                    }),
            );

        let type_for_genre = effective_type.clone();
        let metadata_strip = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(sp(8.0))
            .text_size(sp(12.5))
            .text_color(rgb(theme.text_secondary))
            .child(
                div()
                    .px(sp(8.0))
                    .py(sp(2.0))
                    .rounded(px(RADIUS_PILL))
                    .bg(rgba(theme.glass_trough_bg))
                    .border_1()
                    .border_color(rgba(theme.glass_border))
                    .font(mono_font())
                    .text_size(sp(11.0))
                    .text_color(rgb(theme.text_primary))
                    .child(sentence_case_label(&effective_type)),
            )
            .when_some(certification, |row, cert| {
                row.child(
                    div()
                        .px(sp(7.0))
                        .py(sp(2.0))
                        .rounded(px(4.0))
                        .bg(rgba(theme.glass_trough_bg))
                        .border_1()
                        .border_color(rgba(theme.glass_border))
                        .font(mono_font())
                        .text_size(sp(11.0))
                        .text_color(rgb(theme.text_secondary))
                        .child(cert),
                )
            })
            .when_some(year, |row, yr| {
                row.child(meta_divider())
                    .child(div().font(mono_font()).child(yr))
            })
            .when_some(runtime, |row, rt| {
                row.child(meta_divider())
                    .child(div().font(mono_font()).child(rt))
            })
            .when_some(imdb_rating, |row, rating| {
                row.child(meta_divider()).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(4.0))
                        .font(mono_font())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(theme.text_primary))
                        .child(svg_icon_weighted(
                            IconName::Star,
                            IconWeight::Fill,
                            12.0 * theme.ui_scale,
                            rgb(theme.accent_amber),
                        ))
                        .child(rating),
                )
            });

        let genre_row = (!genres.is_empty()).then(|| {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(6.0))
                .children(genres.into_iter().enumerate().map(|(idx, genre)| {
                    let g_type = type_for_genre.clone();
                    let g_val = genre.clone();
                    div()
                        .id(("quick-peek-genre", idx))
                        .cursor_pointer()
                        .px(sp(10.0))
                        .py(sp(4.0))
                        .rounded(px(RADIUS_PILL))
                        .bg(rgba(theme.glass_card_bg))
                        .border_1()
                        .border_color(rgba(theme.glass_border))
                        .hover(move |s| {
                            s.bg(rgba(theme.glass_row_hover_bg))
                                .border_color(rgba(theme.glass_border_strong))
                                .text_color(rgb(theme.text_primary))
                        })
                        .font(ui_sans(FontWeight::MEDIUM))
                        .text_size(sp(11.5))
                        .text_color(rgb(theme.text_secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_quick_peek(cx);
                                this.open_discover_genre(&g_type, &g_val, cx);
                            }),
                        )
                        .child(genre)
                }))
        });

        let body = div()
            .id("quick-peek-body-scroll")
            .track_scroll(&self.quick_peek_scroll_handle)
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_y_scroll()
            .px(sp(22.0))
            .py(sp(18.0))
            .flex()
            .flex_col()
            .gap(sp(16.0))
            .child(metadata_strip)
            .when_some(genre_row, |col, row| col.child(row))
            .when_some(tagline, |col, line| {
                col.child(
                    div()
                        .font(ui_sans(FontWeight::MEDIUM))
                        .italic()
                        .text_size(sp(13.5))
                        .text_color(rgb(theme.text_secondary))
                        .child(line),
                )
            })
            .child(match synopsis {
                Some(text) => div()
                    .font(ui_sans(FontWeight::NORMAL))
                    .text_size(sp(13.5))
                    .line_height(relative(1.52))
                    .text_color(rgb(theme.text_primary))
                    .child(text)
                    .into_any_element(),
                None => div()
                    .font(ui_sans(FontWeight::NORMAL))
                    .text_size(sp(13.0))
                    .text_color(rgb(theme.text_muted))
                    .child(if meta_loading {
                        "Loading synopsis and cast details..."
                    } else {
                        "No synopsis available yet. Open full details to inspect addon metadata and streams."
                    })
                    .into_any_element(),
            })
            .when(!credits.is_empty(), |col| {
                col.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(8.0))
                        .child(
                            div()
                                .font(ui_sans(FontWeight::SEMIBOLD))
                                .text_size(sp(11.5))
                                .text_color(rgb(theme.text_muted))
                                .child("CAST & CREW"),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(sp(6.0))
                                .children(credits.into_iter().enumerate().map(|(idx, chip)| {
                                    let person_key = chip.person_key.clone();
                                    let person_name = chip.name.clone();
                                    let initials = person_initials(&chip.name);
                                    let is_dir = chip.is_director;

                                    div()
                                        .id(("quick-peek-cast-chip", idx))
                                        .cursor_pointer()
                                        .h(sp(30.0))
                                        .pl(sp(4.0))
                                        .pr(sp(10.0))
                                        .rounded(px(RADIUS_PILL))
                                        .bg(rgba(theme.glass_trough_bg))
                                        .border_1()
                                        .border_color(rgba(theme.glass_border))
                                        .hover(move |s| {
                                            s.bg(rgba(theme.glass_row_hover_bg))
                                                .border_color(rgba(theme.border_accent))
                                        })
                                        .flex()
                                        .items_center()
                                        .gap(sp(6.0))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.close_quick_peek(cx);
                                                this.open_person_details(
                                                    person_key.clone(),
                                                    person_name.clone(),
                                                    cx,
                                                );
                                            }),
                                        )
                                        .child(
                                            div()
                                                .w(sp(22.0))
                                                .h(sp(22.0))
                                                .rounded(px(RADIUS_PILL))
                                                .bg(if is_dir {
                                                    rgba(theme.accent_soft)
                                                } else {
                                                    rgb(theme.bg_elevated)
                                                })
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .font(mono_font())
                                                .text_size(sp(9.5))
                                                .text_color(if is_dir {
                                                    rgb(accent_ink)
                                                } else {
                                                    rgb(theme.text_secondary)
                                                })
                                                .child(initials),
                                        )
                                        .child(
                                            div()
                                                .font(ui_sans(FontWeight::MEDIUM))
                                                .text_size(sp(12.0))
                                                .text_color(rgb(theme.text_primary))
                                                .child(chip.name),
                                        )
                                })),
                        ),
                )
            });

        let footer_actions = div()
            .flex_shrink_0()
            .w_full()
            .px(sp(22.0))
            .py(sp(16.0))
            .bg(rgba(theme.glass_trough_bg))
            .border_t_1()
            .border_color(rgba(theme.glass_divider))
            .flex()
            .flex_col()
            .gap(sp(10.0))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(sp(8.0))
                    .child(
                        primary_accent_button(
                            "quick-peek-watch-btn",
                            Some(IconName::Play),
                            watch_label,
                            cx.listener(move |this, _, _, cx| {
                                if play_id.starts_with("tmdb:") {
                                    this.open_editorial_media_hit(
                                        play_type.clone(),
                                        play_id.clone(),
                                        play_vid.clone(),
                                        cx,
                                    );
                                } else {
                                    this.quick_resume_or_open(
                                        play_type.clone(),
                                        play_id.clone(),
                                        play_vid.clone(),
                                        cx,
                                    );
                                }
                            }),
                        )
                        .flex_1(),
                    )
                    .when_some(trailer_target, |row, t_target| {
                        let t_type = effective_type.clone();
                        let t_id = effective_id.clone();
                        let t_title = display_title.clone();
                        row.child(secondary_glass_button(
                            "quick-peek-trailer-btn",
                            Some(IconName::Clapperboard),
                            "Trailer",
                            false,
                            cx.listener(move |this, _, _, cx| match &t_target {
                                QuickPeekTrailerTarget::DirectStream(stream) => {
                                    let addons = this
                                        .core
                                        .with_model(|m| m.ctx.profile.addons.clone());
                                    let cand = PlaybackCandidate::from_stream_and_request(
                                        stream.as_ref().clone(),
                                        None,
                                        &addons,
                                    );
                                    this.close_quick_peek(cx);
                                    this.start_playback_session(
                                        t_type.clone(),
                                        t_id.clone(),
                                        format!("{t_id}:trailer"),
                                        format!("{t_title} · Trailer"),
                                        Some("Trailer".to_owned()),
                                        None,
                                        None,
                                        Arc::new(vec![cand]),
                                        0,
                                        None,
                                        cx,
                                    );
                                }
                                QuickPeekTrailerTarget::ExternalUrl(url) => {
                                    cx.open_url(url);
                                }
                            }),
                        ))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(8.0))
                    .child(
                        secondary_glass_button(
                            "quick-peek-library-btn",
                            Some(if in_library {
                                IconName::Check
                            } else {
                                IconName::Plus
                            }),
                            if in_library {
                                "In Library"
                            } else {
                                "+ Library"
                            },
                            in_library,
                            cx.listener(move |this, _, _, cx| {
                                if in_library {
                                    this.core.remove_from_library(lib_id.clone());
                                    this.push_toast(
                                        ToastKind::Info,
                                        "Removed from library",
                                        None,
                                        cx,
                                    );
                                } else {
                                    this.core.toggle_library_item(lib_preview.clone());
                                    this.push_toast(
                                        ToastKind::Success,
                                        "Added to library",
                                        None,
                                        cx,
                                    );
                                }
                            }),
                        )
                        .h(sp(36.0))
                        .flex_1(),
                    )
                    .child(
                        secondary_glass_button(
                            "quick-peek-watched-btn",
                            Some(if is_watched {
                                IconName::Check
                            } else {
                                IconName::Eye
                            }),
                            if is_watched {
                                "Watched"
                            } else {
                                "Mark Watched"
                            },
                            is_watched,
                            cx.listener(move |this, _, _, cx| {
                                let next_watched = !is_watched;
                                if in_library {
                                    this.core
                                        .mark_library_item_watched(watch_id.clone(), next_watched);
                                } else {
                                    this.core.dispatch_action(Action::Ctx(
                                        ActionCtx::MetaItemMarkAsWatched {
                                            meta_item: watch_preview.clone(),
                                            is_watched: next_watched,
                                        },
                                    ));
                                }
                                cx.notify();
                            }),
                        )
                        .h(sp(36.0))
                        .flex_1(),
                    ),
            )
            .child(
                div()
                    .id("quick-peek-open-details-btn")
                    .cursor_pointer()
                    .w_full()
                    .h(sp(38.0))
                    .px(sp(14.0))
                    .rounded(px(RADIUS_CONTROL))
                    .bg(rgba(theme.glass_card_bg))
                    .border_1()
                    .border_color(rgba(theme.glass_border))
                    .hover(move |s| {
                        s.bg(rgba(theme.glass_row_hover_bg))
                            .border_color(rgba(theme.border_accent))
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(sp(8.0))
                    .font(ui_sans(FontWeight::SEMIBOLD))
                    .text_size(sp(13.0))
                    .text_color(rgb(theme.text_primary))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.open_editorial_media_hit(
                                details_type.clone(),
                                details_id.clone(),
                                details_vid.clone(),
                                cx,
                            );
                        }),
                    )
                    .child("Open Full Details & Streams →"),
            );

        let drawer = div()
            .id("quick-peek-drawer")
            .occlude()
            .absolute()
            .top(sp(12.0))
            .bottom(sp(12.0))
            .right(sp(12.0))
            .w(px(QUICK_PEEK_WIDTH_PX))
            .max_w(relative(0.92))
            .rounded(px(RADIUS_SHEET))
            .bg(rgba(theme.glass_sheet_bg))
            .border_1()
            .border_color(rgba(theme.glass_border))
            .shadow(glass_capsule_shadow())
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(body)
            .child(footer_actions)
            .with_animation("quick-peek-fade", obsidian_fade_in(), |el, delta| {
                el.opacity(delta)
            });

        Some(
            div()
                .id("quick-peek-scrim")
                .absolute()
                .inset_0()
                .bg(rgba(scrim_bg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_quick_peek(cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_quick_peek(cx);
                    }),
                )
                .child(drawer),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_peek_trailer_prefers_direct_http_stream_over_youtube() {
        let yt_only = resolve_quick_peek_trailer_target(&[], Some("Way9Dexny3w"));
        assert_eq!(
            yt_only,
            Some(QuickPeekTrailerTarget::ExternalUrl(
                "https://www.youtube.com/watch?v=Way9Dexny3w".to_owned()
            ))
        );

        let direct_stream = Stream {
            source: StreamSource::Url {
                url: Url::parse("https://cdn.example/trailer.mp4").unwrap(),
            },
            name: Some("Trailer".to_owned()),
            description: None,
            thumbnail: None,
            subtitles: Vec::new(),
            behavior_hints: Default::default(),
        };
        let resolved = resolve_quick_peek_trailer_target(&[direct_stream], Some("Way9Dexny3w"));
        assert!(matches!(resolved, Some(QuickPeekTrailerTarget::DirectStream(_))));
    }

    #[test]
    fn quick_peek_watch_label_and_fallback_preview_format_cleanly() {
        assert_eq!(format_quick_peek_watch_label(None, None), "Watch");
        assert_eq!(
            format_quick_peek_watch_label(None, Some(95.0)),
            "Resume (01:35)"
        );
        assert_eq!(
            format_quick_peek_watch_label(Some("tt0944947:2:5"), None),
            "Watch S2 E5"
        );
        assert_eq!(
            format_quick_peek_watch_label(Some("tt0944947:2:5"), Some(120.0)),
            "Resume S2 E5"
        );

        let preview = build_quick_peek_fallback_preview(
            "tt15239678",
            "movie",
            "Dune: Part Two",
            Some("https://example.com/poster.jpg"),
            Some("https://example.com/bg.jpg"),
            None,
            Some("2024"),
            Some("166 min"),
            Some("Paul Atreides unites with Chani and the Fremen."),
            Some("8.5"),
            &["Sci-Fi".to_owned(), "Adventure".to_owned()],
        );
        assert_eq!(preview.id, "tt15239678");
        assert_eq!(extract_imdb_rating(&preview).as_deref(), Some("8.5"));
        assert_eq!(
            extract_genres(&preview, 4),
            vec!["Sci-Fi".to_owned(), "Adventure".to_owned()]
        );
    }
}
