use std::{collections::HashMap, ops::Range, sync::Arc};

use gpui::{
    div, img, linear_color_stop, linear_gradient, prelude::*, px, rgb, rgba, AnyElement, Context,
    ElementId, FontWeight, MouseButton, MouseDownEvent, SharedString,
};
use stremio_core_runtime::{
    stremio_core::{
        constants::CINEMETA_URL,
        types::resource::{Link, MetaItemBehaviorHints, MetaItemPreview, PosterShape},
    },
    EditorialMediaHit,
};
use url::Url;

use crate::{
    icons::{svg_icon_weighted, IconName, IconWeight},
    shell::AppShell,
    state::{
        AppRoute, BrowsingContextMenu, FocusedInput, KbFocusableCard, LibraryStatusTab,
        QuickPeekTarget,
    },
    theme::{
        active_theme, editorial_serif, mono_font, placeholder_tint_for_id, poster_card_shadow,
        sentence_case_label, sp, ui_sans, ThemePalette, RADIUS_CARD, RADIUS_CONTROL, RADIUS_PILL,
        RADIUS_POSTER,
    },
    ui::{
        compute_app_grid_metrics, neutral_chip, page_container, page_gutter_px,
        primary_accent_button, render_empty_state, render_page_header, render_poster_card,
        render_section_header, search_field, secondary_glass_button, EmptyStateProps, PageHeader,
        PageWidth, PosterCardProps, PosterMetrics, SearchFieldProps, SectionHeader,
    },
    virtual_grid::{compute_dynamic_grid_metrics, slice_row_items, PosterAspect},
};

const PEOPLE_MIN_CARD_WIDTH_PX: f32 = 270.0;
const PEOPLE_GAP_PX: f32 = 16.0;

const CINEMA_INK_PRIMARY: u32 = 0xF5F1E8;
const CINEMA_INK_SECONDARY: u32 = 0xC8C8D2;
const CINEMA_GLASS_BG: u32 = 0xFFFFFF14;
const CINEMA_GLASS_HOVER_BG: u32 = 0xFFFFFF24;
const CINEMA_GLASS_BORDER: u32 = 0xFFFFFF29;
const CINEMA_GLASS_BORDER_STRONG: u32 = 0xFFFFFF42;
const CINEMA_DIVIDER: u32 = 0xFFFFFF24;

pub(crate) fn person_initials(name: &str) -> String {
    let mut parts = name.split_whitespace();
    let first = parts.next().and_then(|s| s.chars().next());
    let second = parts.next().and_then(|s| s.chars().next());
    match (first, second) {
        (Some(a), Some(b)) => format!("{}{}", a.to_ascii_uppercase(), b.to_ascii_uppercase()),
        (Some(a), None) => a.to_ascii_uppercase().to_string(),
        _ => "?".to_owned(),
    }
}

pub(crate) fn hit_target_id(hit: &EditorialMediaHit) -> String {
    hit.imdb_id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| hit.canonical_id.clone())
}

pub(crate) fn hit_library_status(
    hit: &EditorialMediaHit,
    library_statuses: &HashMap<String, LibraryStatusTab>,
) -> Option<LibraryStatusTab> {
    hit.imdb_id
        .as_deref()
        .and_then(|imdb| library_statuses.get(imdb))
        .or_else(|| library_statuses.get(&hit.canonical_id))
        .copied()
}

pub(crate) fn editorial_hit_to_meta_preview(hit: &EditorialMediaHit) -> MetaItemPreview {
    let mut links = Vec::new();
    if let Some(rating) = hit
        .imdb_rating
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        links.push(Link {
            name: rating.to_owned(),
            category: "imdb".to_owned(),
            url: CINEMETA_URL.clone(),
        });
    }
    for genre in &hit.genres {
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
        id: hit_target_id(hit),
        r#type: hit.media_type.clone(),
        name: hit.title.clone(),
        poster: hit.poster_url.as_deref().and_then(|u| Url::parse(u).ok()),
        background: hit
            .backdrop_url
            .as_deref()
            .and_then(|u| Url::parse(u).ok()),
        logo: hit.logo_url.as_deref().and_then(|u| Url::parse(u).ok()),
        description: hit.overview.clone(),
        release_info: hit.year.clone(),
        runtime: None,
        released: None,
        poster_shape: PosterShape::Poster,
        links,
        trailer_streams: Vec::new(),
        behavior_hints: MetaItemBehaviorHints::default(),
    }
}

fn format_search_status_label(total_results: usize, loading_tmdb: bool) -> String {
    if loading_tmdb {
        match total_results {
            0 => "Searching...".to_owned(),
            1 => "1 result · Updating...".to_owned(),
            n => format!("{n} results · Updating..."),
        }
    } else if total_results == 1 {
        "1 result".to_owned()
    } else {
        format!("{total_results} results")
    }
}

impl AppShell {
    pub(crate) fn trigger_editorial_quick_play(
        &mut self,
        media_type: String,
        target_id: String,
        cx: &mut Context<Self>,
    ) {
        if target_id.starts_with("tmdb:") {
            self.open_editorial_media_hit(media_type, target_id, None, cx);
        } else {
            self.quick_resume_or_open(media_type, target_id, None, cx);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_editorial_poster_card(
        &self,
        element_id: impl Into<ElementId>,
        hit: &EditorialMediaHit,
        subtitle: Option<SharedString>,
        image_height_px: f32,
        library_statuses: &HashMap<String, LibraryStatusTab>,
        is_focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target_id = hit_target_id(hit);
        let lib_status = hit_library_status(hit, library_statuses);
        let in_library = lib_status.is_some();
        let is_watched = lib_status == Some(LibraryStatusTab::Watched);
        let poster_tex = hit
            .poster_url
            .as_deref()
            .and_then(|u| self.poster_cache.resolve_poster_image(u));
        let preview = editorial_hit_to_meta_preview(hit);

        let open_type = hit.media_type.clone();
        let open_id = target_id.clone();
        let play_type = hit.media_type.clone();
        let play_id = target_id.clone();
        let toggle_preview = preview.clone();
        let peek_type = hit.media_type.clone();
        let peek_id = target_id.clone();
        let peek_title = hit.title.clone();
        let peek_preview = preview.clone();
        let ctx_type = hit.media_type.clone();
        let ctx_id = target_id.clone();
        let ctx_title = hit.title.clone();
        let ctx_preview = preview;
        let hover_type = hit.media_type.clone();
        let hover_id = target_id.clone();
        let hover_bg = hit.backdrop_url.clone();

        let props = PosterCardProps::new(element_id, hit.title.clone(), target_id)
            .subtitle(subtitle)
            .image(poster_tex)
            .aspect(PosterAspect::Portrait2x3)
            .image_height_px(Some(image_height_px))
            .status_tab(lib_status)
            .imdb_rating(hit.imdb_rating.clone().map(SharedString::from))
            .in_library(in_library)
            .is_focused(is_focused)
            .on_open(cx.listener(move |this, _, _, cx| {
                this.open_editorial_media_hit(open_type.clone(), open_id.clone(), None, cx);
            }))
            .on_quick_play(cx.listener(move |this, _, _, cx| {
                this.trigger_editorial_quick_play(play_type.clone(), play_id.clone(), cx);
            }))
            .on_toggle_library(cx.listener(move |this, _, _, cx| {
                this.core.toggle_library_item(toggle_preview.clone());
                cx.notify();
            }))
            .on_quick_peek(cx.listener(move |this, _, _, cx| {
                this.open_quick_peek(
                    QuickPeekTarget {
                        type_name: peek_type.clone(),
                        id: peek_id.clone(),
                        video_id: None,
                        title: peek_title.clone(),
                        preview: Some(peek_preview.clone()),
                    },
                    cx,
                );
            }))
            .on_context_menu(cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                let menu = BrowsingContextMenu::new(
                    ev.position,
                    ctx_type.clone(),
                    ctx_id.clone(),
                    ctx_title.clone(),
                )
                .preview(Some(ctx_preview.clone()))
                .is_in_library(in_library)
                .is_watched(is_watched);
                this.open_browsing_context_menu(menu, cx);
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.on_card_hover_prefetch(
                    &hover_type,
                    &hover_id,
                    hover_bg.as_deref(),
                    *hovered,
                    cx,
                );
            }));

        render_poster_card(props)
    }

    pub fn render_search_view(
        &mut self,
        route_query: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = active_theme();
        let accent_ink = if t.is_light { t.accent_press } else { t.accent };

        if self.full_search_query.trim().is_empty() && !route_query.trim().is_empty() {
            self.full_search_query = route_query.to_owned();
        }

        if let Some(top) = self.full_search_results.top_match.clone() {
            self.request_background_enrichment_if_needed(&top.media_type, &top.canonical_id, cx);
        }

        let res = self.full_search_results.clone();
        let enriched_top = res
            .top_match
            .as_ref()
            .and_then(|m| self.core.cached_enriched_meta(&m.canonical_id));

        let library_statuses = self.cached_library_statuses();

        let focused = self.focused_input == Some(FocusedInput::FullSearchInput);
        let q = self.full_search_query.clone();
        let q_empty = q.trim().is_empty();
        let search_display = self.format_input_with_caret(FocusedInput::FullSearchInput, &q);

        let total_results = res.movies.len() + res.series.len() + res.people.len();
        let status_label = format_search_status_label(total_results, self.full_search_loading_tmdb);

        let has_any_hits = res.top_match.is_some()
            || !res.movies.is_empty()
            || !res.series.is_empty()
            || !res.people.is_empty();

        let viewport_w = self.viewport_size_px.0;
        let side_pad = page_gutter_px(viewport_w);
        let media_metrics = compute_app_grid_metrics(
            res.movies.len().max(res.series.len()),
            viewport_w,
            0.0,
            PosterAspect::Portrait2x3,
            &t,
        );
        let people_metrics = compute_dynamic_grid_metrics(
            res.people.len(),
            viewport_w,
            0.0,
            side_pad * 2.0,
            None,
            PEOPLE_MIN_CARD_WIDTH_PX * t.ui_scale,
            PEOPLE_GAP_PX * t.ui_scale,
            PEOPLE_GAP_PX * t.ui_scale,
            0.0,
            PosterAspect::Square1x1,
        );

        let media_cols = media_metrics.columns.max(1);
        let people_cols = people_metrics.columns.max(1);

        let mut kb_items = Vec::new();
        let mut row_cursor = 0usize;
        let movies_kb_offset = kb_items.len();
        if !q_empty && !res.movies.is_empty() {
            for (idx, hit) in res.movies.iter().enumerate() {
                kb_items.push(KbFocusableCard {
                    route: AppRoute::MetaDetails {
                        type_name: hit.media_type.clone(),
                        id: hit_target_id(hit),
                        video_id: None,
                    },
                    row: row_cursor + idx / media_cols,
                    col: idx % media_cols,
                });
            }
            row_cursor += res.movies.len().div_ceil(media_cols);
        }
        let series_kb_offset = kb_items.len();
        if !q_empty && !res.series.is_empty() {
            for (idx, hit) in res.series.iter().enumerate() {
                kb_items.push(KbFocusableCard {
                    route: AppRoute::MetaDetails {
                        type_name: hit.media_type.clone(),
                        id: hit_target_id(hit),
                        video_id: None,
                    },
                    row: row_cursor + idx / media_cols,
                    col: idx % media_cols,
                });
            }
            row_cursor += res.series.len().div_ceil(media_cols);
        }
        let people_kb_offset = kb_items.len();
        if !q_empty && !res.people.is_empty() {
            for (idx, person) in res.people.iter().enumerate() {
                kb_items.push(KbFocusableCard {
                    route: AppRoute::Person {
                        person_key: person.person_key.clone(),
                        name: person.name.clone(),
                    },
                    row: row_cursor + idx / people_cols,
                    col: idx % people_cols,
                });
            }
        }
        self.set_kb_focus_items(kb_items);

        let top_match_el = res.top_match.as_ref().map(|top| {
            let mut merged = top.clone();
            if let Some(enr) = enriched_top.as_ref() {
                if merged.logo_url.is_none() {
                    merged.logo_url = enr.logo_url.clone();
                }
                if merged.backdrop_url.is_none() {
                    merged.backdrop_url = enr.backdrop_url.clone();
                }
                if merged.overview.is_none() {
                    merged.overview = enr.overview.clone();
                }
                if merged.genres.is_empty() {
                    merged.genres = enr.genres.clone();
                }
                if merged.year.is_none() {
                    merged.year = enr.year.clone();
                }
                if merged.imdb_rating.is_none() {
                    merged.imdb_rating = enr.imdb_rating.clone();
                }
            }
            let top_status = hit_library_status(&merged, &library_statuses);
            self.render_search_top_match(&merged, top_status, t, cx)
        });

        let people_rows: Vec<Vec<usize>> = (0..res.people.len())
            .collect::<Vec<_>>()
            .chunks(people_cols)
            .map(|c| c.to_vec())
            .collect();

        div()
            .id("full-search-scroll")
            .track_scroll(&self.search_scroll_handle)
            .size_full()
            .overflow_y_scroll()
            .child(
                page_container(PageWidth::Wide, viewport_w)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(sp(14.0))
                            .child(render_page_header(
                                PageHeader::new("Search")
                                    .back_button(
                                        "search-view-back",
                                        "Back",
                                        cx.listener(|this, _, _, cx| {
                                            this.navigate_back(cx);
                                        }),
                                    )
                                    .right_slot(
                                        (!q_empty).then(|| neutral_chip(status_label).into_any_element()),
                                    ),
                            ))
                            .child(search_field(
                                SearchFieldProps::new(
                                    "full-search-input",
                                    search_display,
                                    "Search movies, series, directors, actors...",
                                    q.is_empty(),
                                    focused,
                                )
                                .clear_id("full-search-clear")
                                .full_width()
                                .height_px(48.0)
                                .on_focus(cx.listener(|this, _, _, cx| {
                                    this.focused_input = Some(FocusedInput::FullSearchInput);
                                    this.input_cursors
                                        .entry(FocusedInput::FullSearchInput)
                                        .or_insert(this.full_search_query.len());
                                    cx.notify();
                                }))
                                .on_clear(cx.listener(|this, _, _, cx| {
                                    this.full_search_query.clear();
                                    this.palette_query.clear();
                                    this.input_cursors
                                        .remove(&FocusedInput::FullSearchInput);
                                    this.input_selected_all
                                        .remove(&FocusedInput::FullSearchInput);
                                    this.focused_input = Some(FocusedInput::FullSearchInput);
                                    this.refresh_full_search(cx);
                                })),
                            )),
                    )
                    .when(q_empty, |el| {
                        el.child(render_empty_state(EmptyStateProps::new(
                            IconName::Search,
                            "Search movies, series, and people",
                            "Find titles across your library, installed catalogs, and connected metadata sources.",
                        )))
                    })
                    .when(!q_empty && !has_any_hits && !self.full_search_loading_tmdb, |el| {
                        el.child(render_empty_state(
                            EmptyStateProps::new(
                                IconName::Search,
                                format!("No matches for \"{}\"", res.query),
                                "Try another title, actor, or director — or add a TMDB key in Settings for broader discovery.",
                            )
                            .action(secondary_glass_button(
                                "search-empty-clear",
                                Some(IconName::X),
                                "Clear search",
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.full_search_query.clear();
                                    this.palette_query.clear();
                                    this.input_cursors
                                        .remove(&FocusedInput::FullSearchInput);
                                    this.input_selected_all
                                        .remove(&FocusedInput::FullSearchInput);
                                    this.focused_input = Some(FocusedInput::FullSearchInput);
                                    this.refresh_full_search(cx);
                                }),
                            )),
                        ))
                    })
                    .when_some(top_match_el, |el, card| {
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(sp(12.0))
                                .child(render_section_header(SectionHeader::new("Top Match")))
                                .child(card),
                        )
                    })
                    .when(!res.movies.is_empty(), |el| {
                        el.child(self.render_search_media_section(
                            "Movies",
                            "search-movies-grid",
                            "search-movie-row",
                            "search-movie-card",
                            &self.search_movies_scroll,
                            &res.movies,
                            &library_statuses,
                            movies_kb_offset,
                            t,
                            cx,
                        ))
                    })
                    .when(!res.series.is_empty(), |el| {
                        el.child(self.render_search_media_section(
                            "Series",
                            "search-series-grid",
                            "search-series-row",
                            "search-series-card",
                            &self.search_series_scroll,
                            &res.series,
                            &library_statuses,
                            series_kb_offset,
                            t,
                            cx,
                        ))
                    })
                    .when(!res.people.is_empty(), |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(sp(16.0))
                                .child(render_section_header(
                                    SectionHeader::new("People")
                                        .subtitle(Some(SharedString::new_static(
                                            "Cast & Directors",
                                        )))
                                        .right_slot(Some(
                                            neutral_chip(format!("{} people", res.people.len()))
                                                .into_any_element(),
                                        )),
                                ))
                                .child(
                                    div()
                                        .w_full()
                                        .flex()
                                        .flex_col()
                                        .gap(sp(PEOPLE_GAP_PX))
                                        .children(people_rows.into_iter().enumerate().map(
                                            |(row_idx, row_indices)| {
                                                let empty_slots =
                                                    people_cols.saturating_sub(row_indices.len());
                                                div()
                                                    .id(("search-people-row", row_idx))
                                                    .w_full()
                                                    .flex()
                                                    .items_stretch()
                                                    .gap(sp(PEOPLE_GAP_PX))
                                                    .children(row_indices.into_iter().map(|idx| {
                                                        let person = &res.people[idx];
                                                        let pk = person.person_key.clone();
                                                        let pname = person.name.clone();
                                                        let headshot_tex = person
                                                            .headshot_url
                                                            .as_deref()
                                                            .and_then(|u| {
                                                                self.poster_cache
                                                                    .resolve_thumbnail_image(u)
                                                            });
                                                        let initials = person_initials(&person.name);
                                                        let known_for =
                                                            if person.known_for.is_empty() {
                                                                "View filmography".to_owned()
                                                            } else {
                                                                person.known_for.join(" · ")
                                                            };
                                                        let is_focused = self.kb_focus_index
                                                            == Some(people_kb_offset + idx);

                                                        let card = div()
                                                            .id(("search-person-card", idx))
                                                            .cursor_pointer()
                                                            .flex_1()
                                                            .min_w(px(0.0))
                                                            .p(sp(14.0))
                                                            .rounded(px(RADIUS_CARD))
                                                            .bg(rgba(t.glass_card_bg))
                                                            .hover(move |s| {
                                                                s.bg(rgba(t.glass_row_hover_bg))
                                                                    .border_color(if is_focused {
                                                                        rgb(t.accent)
                                                                    } else {
                                                                        rgba(t.glass_border_strong)
                                                                    })
                                                            })
                                                            .flex()
                                                            .items_center()
                                                            .gap(sp(14.0))
                                                            .on_mouse_down(
                                                                MouseButton::Left,
                                                                cx.listener(move |this, _, _, cx| {
                                                                    this.open_person_details(
                                                                        pk.clone(),
                                                                        pname.clone(),
                                                                        cx,
                                                                    );
                                                                }),
                                                            );

                                                        let card = if is_focused {
                                                            card.border_2()
                                                                .border_color(rgb(t.accent))
                                                        } else {
                                                            card.border_1()
                                                                .border_color(rgba(t.glass_border))
                                                        };

                                                        card.child(
                                                            div()
                                                                .w(sp(56.0))
                                                                .h(sp(56.0))
                                                                .flex_shrink_0()
                                                                .rounded(px(RADIUS_PILL))
                                                                .overflow_hidden()
                                                                .bg(rgba(t.glass_trough_bg))
                                                                .border_1()
                                                                .border_color(rgba(t.glass_border))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .child(if let Some(tex) =
                                                                    headshot_tex
                                                                {
                                                                    img(tex)
                                                                        .size_full()
                                                                        .object_fit(
                                                                            gpui::ObjectFit::Cover,
                                                                        )
                                                                        .into_any_element()
                                                                } else {
                                                                    div()
                                                                        .font(editorial_serif())
                                                                        .text_size(sp(20.0))
                                                                        .text_color(rgb(
                                                                            t.text_secondary,
                                                                        ))
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
                                                                .gap(sp(4.0))
                                                                .child(
                                                                    div()
                                                                        .font(ui_sans(
                                                                            FontWeight::SEMIBOLD,
                                                                        ))
                                                                        .text_size(sp(14.5))
                                                                        .text_color(rgb(
                                                                            t.text_primary,
                                                                        ))
                                                                        .truncate()
                                                                        .child(person.name.clone()),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .font(mono_font())
                                                                        .text_xs()
                                                                        .text_color(rgb(accent_ink))
                                                                        .child(
                                                                            person
                                                                                .department
                                                                                .clone(),
                                                                        ),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .text_xs()
                                                                        .text_color(rgb(t.text_muted))
                                                                        .truncate()
                                                                        .child(known_for),
                                                                ),
                                                        )
                                                    }))
                                                    .children((0..empty_slots).map(|_| {
                                                        div().flex_1().min_w(px(0.0))
                                                    }))
                                            },
                                        )),
                                ),
                        )
                    }),
            )
    }

    fn render_search_top_match(
        &self,
        hit: &EditorialMediaHit,
        lib_status: Option<LibraryStatusTab>,
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let target_id = hit_target_id(hit);
        let in_library = lib_status.is_some();
        let is_watched = lib_status == Some(LibraryStatusTab::Watched);
        let preview = editorial_hit_to_meta_preview(hit);

        let open_type = hit.media_type.clone();
        let open_id = target_id.clone();
        let watch_type = hit.media_type.clone();
        let watch_id = target_id.clone();
        let details_type = hit.media_type.clone();
        let details_id = target_id.clone();
        let toggle_preview = preview.clone();
        let peek_type = hit.media_type.clone();
        let peek_id = target_id.clone();
        let peek_title = hit.title.clone();
        let peek_preview = preview.clone();
        let ctx_type = hit.media_type.clone();
        let ctx_id = target_id.clone();
        let ctx_title = hit.title.clone();
        let ctx_preview = preview;
        let hover_type = hit.media_type.clone();
        let hover_id = target_id;
        let hover_bg = hit.backdrop_url.clone();

        let tt_id = hit
            .imdb_id
            .as_deref()
            .filter(|id| id.starts_with("tt"))
            .or_else(|| {
                hit.canonical_id
                    .starts_with("tt")
                    .then_some(hit.canonical_id.as_str())
            });
        let metahub_bg = tt_id
            .map(|imdb| format!("https://images.metahub.space/background/medium/{imdb}/img"));
        let metahub_logo =
            tt_id.map(|imdb| format!("https://images.metahub.space/logo/medium/{imdb}/img"));

        let ambient_tex = self.poster_cache.resolve_first_available_ambient(
            hit.backdrop_url
                .as_deref()
                .into_iter()
                .chain(metahub_bg.as_deref())
                .chain(hit.poster_url.as_deref()),
        );
        let backdrop_tex = self.poster_cache.resolve_first_available_hero(
            hit.backdrop_url
                .as_deref()
                .into_iter()
                .chain(metahub_bg.as_deref()),
        );
        let poster_tex = hit
            .poster_url
            .as_deref()
            .and_then(|u| self.poster_cache.resolve_poster_image(u));
        let logo_tex = self.poster_cache.resolve_first_available_logo(
            hit.logo_url
                .as_deref()
                .into_iter()
                .chain(metahub_logo.as_deref()),
        );

        let type_label = sentence_case_label(&hit.media_type);
        let cinema_meta_divider = || {
            div()
                .w(px(1.0))
                .h(px(11.0))
                .flex_shrink_0()
                .bg(rgba(CINEMA_DIVIDER))
        };

        div()
            .id("search-top-match-card")
            .cursor_pointer()
            .w_full()
            .h(sp(280.0))
            .rounded(px(RADIUS_CARD))
            .overflow_hidden()
            .relative()
            .bg(rgb(placeholder_tint_for_id(&hit.canonical_id)))
            .border_1()
            .border_color(rgba(t.glass_border))
            .hover(move |s| s.border_color(rgba(t.glass_border_strong)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.open_editorial_media_hit(open_type.clone(), open_id.clone(), None, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let menu = BrowsingContextMenu::new(
                        ev.position,
                        ctx_type.clone(),
                        ctx_id.clone(),
                        ctx_title.clone(),
                    )
                    .preview(Some(ctx_preview.clone()))
                    .is_in_library(in_library)
                    .is_watched(is_watched);
                    this.open_browsing_context_menu(menu, cx);
                }),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.on_card_hover_prefetch(
                    &hover_type,
                    &hover_id,
                    hover_bg.as_deref(),
                    *hovered,
                    cx,
                );
            }))
            .when_some(ambient_tex, |el, bleed| {
                el.child(
                    img(bleed)
                        .size_full()
                        .absolute()
                        .inset_0()
                        .object_fit(gpui::ObjectFit::Cover)
                        .opacity(0.65),
                )
            })
            .when_some(backdrop_tex, |el, tex| {
                el.child(
                    img(tex)
                        .size_full()
                        .absolute()
                        .inset_0()
                        .object_fit(gpui::ObjectFit::Cover)
                        .opacity(0.55),
                )
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(linear_gradient(
                        90.0,
                        linear_color_stop(rgba(0x0B0D14F2), 0.25),
                        linear_color_stop(rgba(0x0B0D1485), 1.0),
                    )),
            )
            .child(
                div()
                    .relative()
                    .size_full()
                    .p(sp(28.0))
                    .flex()
                    .items_center()
                    .gap(sp(28.0))
                    .child(
                        div()
                            .w(sp(148.0))
                            .h(sp(222.0))
                            .flex_shrink_0()
                            .rounded(px(RADIUS_POSTER))
                            .overflow_hidden()
                            .bg(rgb(0x141417))
                            .border_1()
                            .border_color(rgba(CINEMA_GLASS_BORDER))
                            .shadow(poster_card_shadow())
                            .child(if let Some(ptex) = poster_tex {
                                img(ptex)
                                    .size_full()
                                    .object_fit(gpui::ObjectFit::Cover)
                                    .into_any_element()
                            } else {
                                div()
                                    .size_full()
                                    .p(sp(14.0))
                                    .flex()
                                    .items_end()
                                    .child(
                                        div()
                                            .font(editorial_serif())
                                            .text_size(sp(20.0))
                                            .text_color(rgb(CINEMA_INK_SECONDARY))
                                            .child(hit.title.clone()),
                                    )
                                    .into_any_element()
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(sp(10.0))
                            .child(if let Some(ltex) = logo_tex {
                                img(ltex)
                                    .h(sp(72.0))
                                    .max_w(sp(380.0))
                                    .object_fit(gpui::ObjectFit::Contain)
                                    .into_any_element()
                            } else {
                                div()
                                    .font(editorial_serif())
                                    .text_size(sp(40.0))
                                    .line_height(gpui::relative(1.06))
                                    .text_color(rgb(CINEMA_INK_PRIMARY))
                                    .truncate()
                                    .child(hit.title.clone())
                                    .into_any_element()
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(sp(10.0))
                                    .font(mono_font())
                                    .text_xs()
                                    .text_color(rgb(CINEMA_INK_SECONDARY))
                                    .child(type_label)
                                    .when_some(hit.year.clone(), |el, y| {
                                        el.child(cinema_meta_divider()).child(y)
                                    })
                                    .when_some(hit.imdb_rating.clone(), |el, r| {
                                        el.child(cinema_meta_divider()).child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(sp(4.0))
                                                .text_color(rgb(CINEMA_INK_PRIMARY))
                                                .child(svg_icon_weighted(
                                                    IconName::Star,
                                                    IconWeight::Fill,
                                                    11.5 * t.ui_scale,
                                                    rgb(0xFBBF24),
                                                ))
                                                .child(r),
                                        )
                                    })
                                    .when(!hit.genres.is_empty(), |el| {
                                        el.child(cinema_meta_divider())
                                            .child(hit.genres.join(" · "))
                                    }),
                            )
                            .when_some(hit.overview.clone(), |el, desc| {
                                el.child(
                                    div()
                                        .max_w(px(640.0))
                                        .text_size(sp(13.5))
                                        .text_color(rgb(CINEMA_INK_SECONDARY))
                                        .max_h(sp(42.0))
                                        .overflow_hidden()
                                        .child(desc),
                                )
                            })
                            .child(
                                div()
                                    .mt(sp(4.0))
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(sp(10.0))
                                    .child(primary_accent_button(
                                        "search-top-match-watch",
                                        Some(IconName::Play),
                                        "Watch",
                                        cx.listener(move |this, _, _, cx| {
                                            this.trigger_editorial_quick_play(
                                                watch_type.clone(),
                                                watch_id.clone(),
                                                cx,
                                            );
                                        }),
                                    ))
                                    .child(render_cinema_glass_button(
                                        "search-top-match-details",
                                        IconName::Info,
                                        IconWeight::Regular,
                                        "Details",
                                        false,
                                        t,
                                        cx.listener(move |this, _, _, cx| {
                                            this.open_editorial_media_hit(
                                                details_type.clone(),
                                                details_id.clone(),
                                                None,
                                                cx,
                                            );
                                        }),
                                    ))
                                    .child(render_cinema_glass_button(
                                        "search-top-match-peek",
                                        IconName::Eye,
                                        IconWeight::Regular,
                                        "Quick Peek",
                                        false,
                                        t,
                                        cx.listener(move |this, _, _, cx| {
                                            this.open_quick_peek(
                                                QuickPeekTarget {
                                                    type_name: peek_type.clone(),
                                                    id: peek_id.clone(),
                                                    video_id: None,
                                                    title: peek_title.clone(),
                                                    preview: Some(peek_preview.clone()),
                                                },
                                                cx,
                                            );
                                        }),
                                    ))
                                    .child(render_cinema_glass_button(
                                        "search-top-match-watchlist",
                                        IconName::Bookmark,
                                        if in_library {
                                            IconWeight::Fill
                                        } else {
                                            IconWeight::Regular
                                        },
                                        if in_library {
                                            "On watchlist"
                                        } else {
                                            "Watchlist"
                                        },
                                        in_library,
                                        t,
                                        cx.listener(move |this, _, _, cx| {
                                            this.core.toggle_library_item(toggle_preview.clone());
                                            cx.notify();
                                        }),
                                    )),
                            ),
                    ),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_search_media_section(
        &self,
        heading: &'static str,
        list_id: &'static str,
        row_id_prefix: &'static str,
        card_id_prefix: &'static str,
        scroll_handle: &gpui::UniformListScrollHandle,
        hits: &[EditorialMediaHit],
        library_statuses: &Arc<HashMap<String, LibraryStatusTab>>,
        kb_idx_offset: usize,
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let total_items = hits.len();
        let grid_gap = PosterMetrics::from_theme(&t).gap_px;
        let metrics = compute_app_grid_metrics(
            total_items,
            self.viewport_size_px.0,
            0.0,
            PosterAspect::Portrait2x3,
            &t,
        );
        let columns = metrics.columns;
        let row_count = metrics.layout.row_count;
        let row_stride_px = metrics.row_stride_px;
        let image_height_px = metrics.image_height_px;

        let total_grid_h = row_count as f32 * row_stride_px;
        let max_section_h = (self.viewport_size_px.1 * 0.68).clamp(380.0, 720.0);
        let list_height_px = total_grid_h.min(max_section_h);

        let hits_arc = Arc::new(hits.to_vec());
        let statuses_arc = Arc::clone(library_statuses);

        div()
            .flex()
            .flex_col()
            .gap(sp(16.0))
            .child(render_section_header(
                SectionHeader::new(heading).right_slot(Some(
                    neutral_chip(format!("{total_items} titles")).into_any_element(),
                )),
            ))
            .child(
                div()
                    .w_full()
                    .h(px(list_height_px))
                    .flex()
                    .flex_col()
                    .child(
                        gpui::uniform_list(
                            list_id,
                            row_count,
                            cx.processor(move |this, visible_rows: Range<usize>, _window, cx| {
                                visible_rows
                                    .map(|row_idx| {
                                        let range = slice_row_items(total_items, columns, row_idx);
                                        let empty_slots = columns.saturating_sub(range.len());

                                        div()
                                            .id((row_id_prefix, row_idx))
                                            .w_full()
                                            .h(px(row_stride_px))
                                            .pb(px(grid_gap))
                                            .flex()
                                            .items_start()
                                            .gap(px(grid_gap))
                                            .children(range.map(|item_idx| {
                                                let hit = &hits_arc[item_idx];
                                                let sub = hit
                                                    .year
                                                    .clone()
                                                    .map(SharedString::from)
                                                    .or_else(|| {
                                                        Some(SharedString::from(
                                                            sentence_case_label(&hit.media_type),
                                                        ))
                                                    });
                                                let is_focused = this.kb_focus_index
                                                    == Some(kb_idx_offset + item_idx);
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.0))
                                                    .h_full()
                                                    .child(this.render_editorial_poster_card(
                                                        (card_id_prefix, item_idx),
                                                        hit,
                                                        sub,
                                                        image_height_px,
                                                        &statuses_arc,
                                                        is_focused,
                                                        cx,
                                                    ))
                                            }))
                                            .children(
                                                (0..empty_slots)
                                                    .map(|_| div().flex_1().min_w(px(0.0))),
                                            )
                                    })
                                    .collect()
                            }),
                        )
                        .track_scroll(scroll_handle)
                        .flex_1()
                        .w_full()
                        .min_h(px(0.0)),
                    ),
            )
    }
}

fn render_cinema_glass_button(
    id: impl Into<ElementId>,
    icon: IconName,
    weight: IconWeight,
    label: impl Into<SharedString>,
    active: bool,
    t: ThemePalette,
    on_click: impl Fn(&MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let fg = if active { t.accent } else { CINEMA_INK_PRIMARY };
    let active_bg = (t.accent << 8) | 0x29;
    let active_border = (t.accent << 8) | 0x73;
    div()
        .id(id)
        .cursor_pointer()
        .h(sp(40.0))
        .px(sp(16.0))
        .rounded(px(RADIUS_CONTROL))
        .bg(if active {
            rgba(active_bg)
        } else {
            rgba(CINEMA_GLASS_BG)
        })
        .border_1()
        .border_color(if active {
            rgba(active_border)
        } else {
            rgba(CINEMA_GLASS_BORDER)
        })
        .hover(move |s| {
            s.bg(if active {
                rgba(active_bg)
            } else {
                rgba(CINEMA_GLASS_HOVER_BG)
            })
            .border_color(if active {
                rgb(t.accent)
            } else {
                rgba(CINEMA_GLASS_BORDER_STRONG)
            })
        })
        .flex()
        .items_center()
        .justify_center()
        .gap(sp(8.0))
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(sp(13.5))
        .text_color(rgb(fg))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(svg_icon_weighted(icon, weight, 14.5 * t.ui_scale, rgb(fg)))
        .child(label.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_hit() -> EditorialMediaHit {
        EditorialMediaHit {
            canonical_id: "tmdb:693134".to_owned(),
            imdb_id: Some("tt15239678".to_owned()),
            tmdb_id: Some(693134),
            media_type: "movie".to_owned(),
            title: "Dune: Part Two".to_owned(),
            year: Some("2024".to_owned()),
            imdb_rating: Some("8.5".to_owned()),
            overview: Some("Follow the mythic journey of Paul Atreides.".to_owned()),
            poster_url: Some("https://image.tmdb.org/t/p/w342/poster.jpg".to_owned()),
            backdrop_url: Some("https://image.tmdb.org/t/p/w780/bg.jpg".to_owned()),
            logo_url: Some("https://image.tmdb.org/t/p/w500/logo.png".to_owned()),
            genres: vec!["Sci-Fi".to_owned(), "Adventure".to_owned()],
            score: 100,
            role_hint: Some("Paul Atreides".to_owned()),
        }
    }

    #[test]
    fn person_initials_formats_names_and_empty() {
        assert_eq!(person_initials("Denis Villeneuve"), "DV");
        assert_eq!(person_initials("Zendaya"), "Z");
        assert_eq!(person_initials("   "), "?");
    }

    #[test]
    fn editorial_hit_helpers_prefer_imdb_and_avoid_jargon() {
        let hit = sample_hit();
        assert_eq!(hit_target_id(&hit), "tt15239678");

        let mut statuses = HashMap::new();
        statuses.insert("tt15239678".to_owned(), LibraryStatusTab::Watchlist);
        assert_eq!(
            hit_library_status(&hit, &statuses),
            Some(LibraryStatusTab::Watchlist)
        );

        let preview = editorial_hit_to_meta_preview(&hit);
        assert_eq!(preview.id, "tt15239678");
        assert_eq!(preview.name, "Dune: Part Two");
        assert_eq!(preview.release_info.as_deref(), Some("2024"));
        assert_eq!(preview.links.len(), 3);

        let status = format_search_status_label(14, false);
        assert_eq!(status, "14 results");
        assert!(!status.contains("FTS5"));
        assert!(!status.contains("deduplicated"));
    }
}
