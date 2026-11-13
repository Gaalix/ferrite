use std::collections::HashMap;

use gpui::{
    div, img, linear_color_stop, linear_gradient, prelude::*, px, rgb, rgba, Context, ElementId,
    FontWeight, MouseButton, SharedString,
};
use stremio_core_runtime::EditorialMediaHit;

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{classify_library_item_status, FocusedInput, LibraryStatusTab},
    theme::{
        editorial_serif, meta_divider, mono_font, neutral_chip, placeholder_tint_for_id,
        poster_card_shadow, sentence_case_label, ui_sans, ACCENT, ACCENT_AMBER, ACCENT_HOVER,
        BG_ELEVATED, BG_INPUT, BG_OBSIDIAN, BG_SURFACE, BORDER_POSTER_HOVER, BORDER_SOFT,
        BORDER_STRONG, BORDER_SUBTLE, STATUS_WATCHED, STATUS_WATCHING, STATUS_WATCHLIST,
        TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY,
    },
};

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

impl AppShell {
    pub fn render_search_view(
        &mut self,
        route_query: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
            .and_then(|t| self.core.cached_enriched_meta(&t.canonical_id));

        let library_statuses: HashMap<String, LibraryStatusTab> = self.core.with_model(|model| {
            model
                .ctx
                .library
                .items
                .iter()
                .filter_map(|(id, li)| {
                    classify_library_item_status(li).map(|st| (id.clone(), st))
                })
                .collect()
        });

        let focused = self.focused_input == Some(FocusedInput::FullSearchInput);
        let q = self.full_search_query.clone();
        let q_empty = q.trim().is_empty();
        let search_display: SharedString = if q.is_empty() {
            if focused {
                "|".into()
            } else {
                "Search movies, series, directors, actors...".into()
            }
        } else if focused {
            format!("{q}|").into()
        } else {
            q.clone().into()
        };

        let total_dedup = res.movies.len() + res.series.len() + res.people.len();
        let latency_ms = (res.elapsed_us as f64) / 1000.0;
        let telemetry_text = if self.full_search_loading_tmdb {
            format!("{total_dedup} deduplicated · {latency_ms:.1} ms local · querying TMDB...")
        } else {
            format!("{total_dedup} deduplicated · {latency_ms:.1} ms FTS5")
        };

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
            self.render_search_top_match(&merged, cx)
        });

        let has_any_hits = res.top_match.is_some()
            || !res.movies.is_empty()
            || !res.series.is_empty()
            || !res.people.is_empty();

        div()
            .id("full-search-scroll")
            .track_scroll(&self.search_scroll_handle)
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .max_w(px(1440.0))
                    .mx_auto()
                    .px(px(32.0))
                    .pt(px(28.0))
                    .pb(px(72.0))
                    .flex()
                    .flex_col()
                    .gap(px(36.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(14.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(16.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(12.0))
                                            .child(
                                                div()
                                                    .id("search-view-back")
                                                    .cursor_pointer()
                                                    .h(px(34.0))
                                                    .px(px(12.0))
                                                    .rounded(px(999.0))
                                                    .bg(rgb(BG_SURFACE))
                                                    .border_1()
                                                    .border_color(rgba(BORDER_SUBTLE))
                                                    .hover(|s| {
                                                        s.bg(rgb(BG_ELEVATED))
                                                            .border_color(rgba(BORDER_STRONG))
                                                    })
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(6.0))
                                                    .font(ui_sans(FontWeight::MEDIUM))
                                                    .text_size(px(13.0))
                                                    .text_color(rgb(TEXT_SECONDARY))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, _, cx| {
                                                            this.navigate_back(cx);
                                                        }),
                                                    )
                                                    .child(svg_icon(
                                                        IconName::ChevronLeft,
                                                        13.5,
                                                        rgb(TEXT_SECONDARY),
                                                    ))
                                                    .child("Back"),
                                            )
                                            .child(
                                                div()
                                                    .font(editorial_serif())
                                                    .text_size(px(32.0))
                                                    .text_color(rgb(TEXT_PRIMARY))
                                                    .child("Search"),
                                            ),
                                    )
                                    .when(!q_empty, |el| el.child(neutral_chip(telemetry_text))),
                            )
                            .child(
                                div()
                                    .id("full-search-input")
                                    .cursor_text()
                                    .w_full()
                                    .h(px(50.0))
                                    .px(px(18.0))
                                    .rounded(px(10.0))
                                    .bg(rgb(BG_INPUT))
                                    .border_1()
                                    .border_color(if focused {
                                        rgba(BORDER_POSTER_HOVER)
                                    } else {
                                        rgba(BORDER_SUBTLE)
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(12.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.focused_input =
                                                Some(FocusedInput::FullSearchInput);
                                            cx.notify();
                                        }),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .flex()
                                            .items_center()
                                            .gap(px(12.0))
                                            .child(svg_icon(
                                                IconName::Search,
                                                18.0,
                                                if focused {
                                                    rgb(ACCENT)
                                                } else {
                                                    rgb(TEXT_MUTED)
                                                },
                                            ))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .truncate()
                                                    .font(ui_sans(FontWeight::MEDIUM))
                                                    .text_size(px(16.0))
                                                    .text_color(if q_empty && !focused {
                                                        rgb(TEXT_MUTED)
                                                    } else {
                                                        rgb(TEXT_PRIMARY)
                                                    })
                                                    .child(search_display),
                                            ),
                                    )
                                    .when(!q.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .id("full-search-clear")
                                                .cursor_pointer()
                                                .w(px(28.0))
                                                .h(px(28.0))
                                                .rounded(px(999.0))
                                                .hover(|s| s.bg(rgb(BG_ELEVATED)))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.full_search_query.clear();
                                                        this.palette_query.clear();
                                                        this.focused_input =
                                                            Some(FocusedInput::FullSearchInput);
                                                        this.refresh_full_search(cx);
                                                    }),
                                                )
                                                .child(svg_icon(
                                                    IconName::X,
                                                    14.0,
                                                    rgb(TEXT_SECONDARY),
                                                )),
                                        )
                                    }),
                            ),
                    )
                    .when(q_empty, |el| {
                        el.child(
                            div()
                                .p(px(36.0))
                                .rounded(px(12.0))
                                .bg(rgb(BG_SURFACE))
                                .border_1()
                                .border_color(rgba(BORDER_SUBTLE))
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .font(editorial_serif())
                                        .text_size(px(28.0))
                                        .text_color(rgb(TEXT_PRIMARY))
                                        .child("Search the entire cinema index"),
                                )
                                .child(
                                    div()
                                        .max_w(px(620.0))
                                        .text_size(px(14.0))
                                        .text_color(rgb(TEXT_SECONDARY))
                                        .child("Results deduplicate across local FTS5 metadata, installed catalogs, and TMDB by canonical IMDb/TMDB ID."),
                                ),
                        )
                    })
                    .when(!q_empty && !has_any_hits && !self.full_search_loading_tmdb, |el| {
                        el.child(
                            div()
                                .p(px(36.0))
                                .rounded(px(12.0))
                                .bg(rgb(BG_SURFACE))
                                .border_1()
                                .border_color(rgba(BORDER_SUBTLE))
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .font(editorial_serif())
                                        .text_size(px(28.0))
                                        .text_color(rgb(TEXT_PRIMARY))
                                        .child(format!("No matches found for \"{}\"", res.query)),
                                )
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .text_color(rgb(TEXT_SECONDARY))
                                        .child("Try another title spelling, actor, or director — or verify your TMDB API key in Settings for cloud multi-search."),
                                ),
                        )
                    })
                    .when_some(top_match_el, |el, card| {
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(14.0))
                                .child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(ACCENT))
                                        .child("TOP MATCH"),
                                )
                                .child(card),
                        )
                    })
                    .when(!res.movies.is_empty(), |el| {
                        el.child(self.render_search_media_section(
                            "Movies",
                            "search-movie",
                            &res.movies,
                            &library_statuses,
                            cx,
                        ))
                    })
                    .when(!res.series.is_empty(), |el| {
                        el.child(self.render_search_media_section(
                            "Series",
                            "search-series",
                            &res.series,
                            &library_statuses,
                            cx,
                        ))
                    })
                    .when(!res.people.is_empty(), |el| {
                        el.child(
                            div()
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
                                                .flex()
                                                .items_baseline()
                                                .gap(px(12.0))
                                                .child(
                                                    div()
                                                        .font(editorial_serif())
                                                        .text_size(px(28.0))
                                                        .text_color(rgb(TEXT_PRIMARY))
                                                        .child("People"),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(13.0))
                                                        .text_color(rgb(TEXT_MUTED))
                                                        .child("Cast & Directors"),
                                                ),
                                        )
                                        .child(neutral_chip(format!("{} people", res.people.len()))),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .grid()
                                        .grid_cols(4)
                                        .gap(px(16.0))
                                        .children(res.people.iter().enumerate().map(|(idx, person)| {
                                            let pk = person.person_key.clone();
                                            let pname = person.name.clone();
                                            let headshot_tex = person
                                                .headshot_url
                                                .as_deref()
                                                .and_then(|u| self.poster_cache.resolve_thumbnail_image(u));
                                            let initials = person_initials(&person.name);
                                            let known_for = if person.known_for.is_empty() {
                                                "View filmography".to_owned()
                                            } else {
                                                person.known_for.join(" · ")
                                            };

                                            div()
                                                .id(("search-person-card", idx))
                                                .cursor_pointer()
                                                .p(px(14.0))
                                                .rounded(px(10.0))
                                                .bg(rgb(BG_SURFACE))
                                                .border_1()
                                                .border_color(rgba(BORDER_SUBTLE))
                                                .hover(|s| {
                                                    s.bg(rgb(BG_ELEVATED))
                                                        .border_color(rgba(BORDER_POSTER_HOVER))
                                                })
                                                .flex()
                                                .items_center()
                                                .gap(px(14.0))
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
                                                        .w(px(56.0))
                                                        .h(px(56.0))
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
                                                                .object_fit(gpui::ObjectFit::Cover)
                                                                .into_any_element()
                                                        } else {
                                                            div()
                                                                .font(editorial_serif())
                                                                .text_size(px(20.0))
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
                                                        .gap(px(4.0))
                                                        .child(
                                                            div()
                                                                .font(ui_sans(FontWeight::SEMIBOLD))
                                                                .text_size(px(14.5))
                                                                .text_color(rgb(TEXT_PRIMARY))
                                                                .truncate()
                                                                .child(person.name.clone()),
                                                        )
                                                        .child(
                                                            div()
                                                                .font(mono_font())
                                                                .text_xs()
                                                                .text_color(rgb(ACCENT))
                                                                .child(person.department.clone()),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(rgb(TEXT_MUTED))
                                                                .truncate()
                                                                .child(known_for),
                                                        ),
                                                )
                                        })),
                                ),
                        )
                    }),
            )
    }

    fn render_search_top_match(
        &self,
        hit: &EditorialMediaHit,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open_type = hit.media_type.clone();
        let open_id = hit.canonical_id.clone();

        let metahub_bg = hit.imdb_id.as_ref().map(|imdb| {
            format!("https://images.metahub.space/background/medium/{imdb}/img")
        });
        let metahub_logo = hit.imdb_id.as_ref().map(|imdb| {
            format!("https://images.metahub.space/logo/medium/{imdb}/img")
        });

        let backdrop_tex = hit
            .backdrop_url
            .as_deref()
            .and_then(|u| {
                self.poster_cache
                    .resolve_hero_backdrop_image(u)
                    .or_else(|| self.poster_cache.resolve_widescreen_image(u))
            })
            .or_else(|| {
                metahub_bg
                    .as_deref()
                    .and_then(|u| self.poster_cache.resolve_widescreen_image(u))
            });
        let poster_tex = hit
            .poster_url
            .as_deref()
            .and_then(|u| self.poster_cache.resolve_poster_image(u));
        let logo_tex = hit
            .logo_url
            .as_deref()
            .and_then(|u| self.poster_cache.resolve_logo_image(u))
            .or_else(|| {
                metahub_logo
                    .as_deref()
                    .and_then(|u| self.poster_cache.resolve_logo_image(u))
            });

        let type_label = sentence_case_label(&hit.media_type);

        div()
            .id("search-top-match-card")
            .cursor_pointer()
            .w_full()
            .h(px(280.0))
            .rounded(px(12.0))
            .overflow_hidden()
            .relative()
            .bg(rgb(placeholder_tint_for_id(&hit.canonical_id)))
            .border_1()
            .border_color(rgba(BORDER_SUBTLE))
            .hover(|s| s.border_color(rgba(BORDER_STRONG)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.open_editorial_media_hit(open_type.clone(), open_id.clone(), None, cx);
                }),
            )
            .when_some(backdrop_tex, |el, tex| {
                el.child(
                    img(tex)
                        .size_full()
                        .absolute()
                        .inset_0()
                        .object_fit(gpui::ObjectFit::Cover)
                        .opacity(0.65),
                )
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(linear_gradient(
                        90.0,
                        linear_color_stop(rgba(0x0b0b0df2), 0.25),
                        linear_color_stop(rgba(0x0b0b0d85), 1.0),
                    )),
            )
            .child(
                div()
                    .relative()
                    .size_full()
                    .p(px(28.0))
                    .flex()
                    .items_center()
                    .gap(px(28.0))
                    .child(
                        div()
                            .w(px(148.0))
                            .h(px(222.0))
                            .flex_shrink_0()
                            .rounded(px(6.0))
                            .overflow_hidden()
                            .bg(rgb(BG_SURFACE))
                            .border_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .shadow(poster_card_shadow())
                            .child(if let Some(ptex) = poster_tex {
                                img(ptex)
                                    .size_full()
                                    .object_fit(gpui::ObjectFit::Cover)
                                    .into_any_element()
                            } else {
                                div()
                                    .size_full()
                                    .p(px(14.0))
                                    .flex()
                                    .items_end()
                                    .child(
                                        div()
                                            .font(editorial_serif())
                                            .text_size(px(20.0))
                                            .text_color(rgb(TEXT_SECONDARY))
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
                            .gap(px(10.0))
                            .child(if let Some(ltex) = logo_tex {
                                img(ltex)
                                    .h(px(72.0))
                                    .max_w(px(380.0))
                                    .object_fit(gpui::ObjectFit::Contain)
                                    .into_any_element()
                            } else {
                                div()
                                    .font(editorial_serif())
                                    .text_size(px(40.0))
                                    .line_height(gpui::relative(1.06))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .truncate()
                                    .child(hit.title.clone())
                                    .into_any_element()
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .font(mono_font())
                                    .text_xs()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(type_label)
                                    .when_some(hit.year.clone(), |el, y| {
                                        el.child(meta_divider()).child(y)
                                    })
                                    .when_some(hit.imdb_rating.clone(), |el, r| {
                                        el.child(meta_divider()).child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(4.0))
                                                .text_color(rgb(TEXT_PRIMARY))
                                                .child(svg_icon(
                                                    IconName::Star,
                                                    11.5,
                                                    rgb(ACCENT_AMBER),
                                                ))
                                                .child(r),
                                        )
                                    })
                                    .when(!hit.genres.is_empty(), |el| {
                                        el.child(meta_divider()).child(hit.genres.join(" · "))
                                    }),
                            )
                            .when_some(hit.overview.clone(), |el, desc| {
                                el.child(
                                    div()
                                        .max_w(px(640.0))
                                        .text_size(px(13.5))
                                        .text_color(rgb(TEXT_SECONDARY))
                                        .max_h(px(42.0))
                                        .overflow_hidden()
                                        .child(desc),
                                )
                            })
                            .child(
                                div()
                                    .mt(px(4.0))
                                    .h(px(36.0))
                                    .px(px(16.0))
                                    .rounded(px(999.0))
                                    .bg(rgb(ACCENT))
                                    .hover(|s| s.bg(rgb(ACCENT_HOVER)))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(px(13.0))
                                    .text_color(rgb(BG_OBSIDIAN))
                                    .child(svg_icon(IconName::Play, 13.5, rgb(BG_OBSIDIAN)))
                                    .child("Open Details"),
                            ),
                    ),
            )
    }

    pub(crate) fn render_search_media_section(
        &self,
        heading: &'static str,
        id_prefix: &'static str,
        hits: &[EditorialMediaHit],
        library_statuses: &HashMap<String, LibraryStatusTab>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
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
                            .text_size(px(28.0))
                            .text_color(rgb(TEXT_PRIMARY))
                            .child(heading),
                    )
                    .child(neutral_chip(format!("{} titles", hits.len()))),
            )
            .child(
                div()
                    .w_full()
                    .grid()
                    .grid_cols(6)
                    .gap(px(18.0))
                    .children(hits.iter().enumerate().map(|(idx, hit)| {
                        let lib_status = hit
                            .imdb_id
                            .as_deref()
                            .and_then(|imdb| library_statuses.get(imdb))
                            .or_else(|| library_statuses.get(&hit.canonical_id))
                            .copied();
                        self.render_editorial_media_card(
                            (id_prefix, idx),
                            format!("{id_prefix}-grp-{idx}"),
                            hit,
                            lib_status,
                            cx,
                        )
                    })),
            )
    }

    pub(crate) fn render_editorial_media_card(
        &self,
        element_id: impl Into<ElementId>,
        group_key: String,
        hit: &EditorialMediaHit,
        lib_status: Option<LibraryStatusTab>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let poster_tex = hit
            .poster_url
            .as_deref()
            .and_then(|u| self.poster_cache.resolve_poster_image(u));
        let open_type = hit.media_type.clone();
        let open_id = hit.canonical_id.clone();
        let grp: SharedString = group_key.into();

        div()
            .id(element_id)
            .group(grp.clone())
            .cursor_pointer()
            .w_full()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.open_editorial_media_hit(open_type.clone(), open_id.clone(), None, cx);
                }),
            )
            .child(
                div()
                    .w_full()
                    .aspect_ratio(2.0 / 3.0)
                    .rounded(px(4.0))
                    .overflow_hidden()
                    .relative()
                    .bg(rgb(placeholder_tint_for_id(&hit.canonical_id)))
                    .border_1()
                    .border_color(rgba(BORDER_SOFT))
                    .shadow(poster_card_shadow())
                    .group_hover(grp.clone(), |s| s.border_color(rgba(BORDER_POSTER_HOVER)))
                    .child(if let Some(tex) = poster_tex {
                        img(tex)
                            .size_full()
                            .object_fit(gpui::ObjectFit::Cover)
                            .into_any_element()
                    } else {
                        div()
                            .size_full()
                            .p(px(14.0))
                            .bg(rgb(BG_SURFACE))
                            .flex()
                            .items_end()
                            .child(
                                div()
                                    .font(editorial_serif())
                                    .text_size(px(20.0))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(hit.title.clone()),
                            )
                            .into_any_element()
                    })
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .opacity(0.0)
                            .group_hover(grp, |s| s.opacity(1.0))
                            .bg(linear_gradient(
                                180.0,
                                linear_color_stop(rgba(0x0b0b0d00), 0.45),
                                linear_color_stop(rgba(0x0b0b0de0), 1.0),
                            )),
                    )
                    .when_some(lib_status, |el, st| {
                        let (icon, bg_col) = match st {
                            LibraryStatusTab::Watched => (IconName::Check, STATUS_WATCHED),
                            LibraryStatusTab::Watching => (IconName::Play, STATUS_WATCHING),
                            LibraryStatusTab::Watchlist => (IconName::Bookmark, STATUS_WATCHLIST),
                        };
                        el.child(
                            div()
                                .absolute()
                                .top(px(8.0))
                                .right(px(8.0))
                                .w(px(26.0))
                                .h(px(26.0))
                                .rounded(px(999.0))
                                .bg(rgb(bg_col))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(svg_icon(icon, 12.5, rgb(BG_OBSIDIAN))),
                        )
                    })
                    .when_some(hit.imdb_rating.clone(), |el, r| {
                        el.child(
                            div()
                                .absolute()
                                .bottom(px(8.0))
                                .left(px(8.0))
                                .px(px(6.5))
                                .py(px(2.5))
                                .rounded(px(4.0))
                                .bg(rgba(0x0b0b0dc7))
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .font(mono_font())
                                .text_size(px(11.0))
                                .text_color(rgb(TEXT_PRIMARY))
                                .child(svg_icon(IconName::Star, 10.5, rgb(ACCENT_AMBER)))
                                .child(r),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_size(px(13.5))
                            .text_color(rgb(TEXT_PRIMARY))
                            .truncate()
                            .child(hit.title.clone()),
                    )
                    .when_some(hit.year.clone(), |el, yr| {
                        el.child(
                            div()
                                .font(mono_font())
                                .text_xs()
                                .text_color(rgb(TEXT_MUTED))
                                .child(yr),
                        )
                    }),
            )
    }
}
