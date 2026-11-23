use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
};

use gpui::{
    div, img, prelude::*, px, rgb, rgba, AnyElement, Context, ElementId, FontWeight, MouseButton,
    MouseDownEvent, SharedString,
};
use stremio_core_runtime::{EditorialMediaHit, PersonFilmography};

use crate::{
    icons::{svg_icon_weighted, IconName, IconWeight},
    shell::AppShell,
    state::{
        AppRoute, BrowsingContextMenu, KbFocusableCard, LibraryStatusTab, PersonDeptFilter,
        QuickPeekTarget,
    },
    theme::{
        active_theme, editorial_serif, mono_font, poster_card_shadow, sentence_case_label, sp,
        ui_sans, RADIUS_CARD, RADIUS_PILL, RADIUS_POSTER,
    },
    ui::{
        choice_pill, compute_app_grid_metrics, glass_segmented_bar, glass_segmented_item,
        meta_divider, neutral_chip, page_container, render_empty_state, render_page_header,
        render_poster_card, render_section_header, EmptyStateProps, PageHeader, PageWidth,
        PosterCardProps, PosterMetrics, SectionHeader,
    },
    views::search_view::{
        editorial_hit_to_meta_preview, hit_library_status, hit_target_id, person_initials,
    },
    virtual_grid::{slice_row_items, PosterAspect},
};

const BIO_COLLAPSE_CHARS: usize = 340;

fn truncate_bio_at_word_boundary(bio: &str, max_chars: usize) -> (String, bool) {
    let trimmed = bio.trim();
    if trimmed.chars().count() <= max_chars {
        return (trimmed.to_owned(), false);
    }

    let mut cut_byte = trimmed.len();
    let mut last_space_byte = None;
    let half = max_chars / 2;

    for (char_idx, (byte_idx, ch)) in trimmed.char_indices().enumerate() {
        if char_idx >= max_chars {
            cut_byte = byte_idx;
            break;
        }
        if char_idx >= half && ch.is_whitespace() {
            last_space_byte = Some(byte_idx);
        }
    }

    let safe_cut = last_space_byte.unwrap_or(cut_byte);
    let prefix = trimmed[..safe_cut].trim_end_matches(|c: char| c.is_whitespace() || c == ',' || c == ';');
    (format!("{prefix}..."), true)
}

fn is_directing_role(role: &str) -> bool {
    role.split(&[',', '/', '&'][..]).any(|part| {
        matches!(
            part.trim().to_ascii_lowercase().as_str(),
            "director" | "co-director" | "series director"
        )
    })
}

fn is_crew_job_token(part: &str) -> bool {
    matches!(
        part.trim().to_ascii_lowercase().as_str(),
        "director"
            | "co-director"
            | "series director"
            | "assistant director"
            | "writer"
            | "screenplay"
            | "story"
            | "creator"
            | "producer"
            | "executive producer"
            | "co-producer"
            | "associate producer"
            | "line producer"
            | "cinematography"
            | "director of photography"
            | "editor"
            | "film editor"
            | "composer"
            | "original music composer"
            | "music"
            | "costume design"
            | "production design"
            | "casting"
            | "art direction"
    )
}

fn is_crew_only_role(role: &str) -> bool {
    let parts: Vec<&str> = role
        .split(&[',', '/', '&'][..])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    !parts.is_empty() && parts.into_iter().all(is_crew_job_token)
}

fn format_person_credit_subtitle(hit: &EditorialMediaHit) -> Option<SharedString> {
    if let Some(raw_role) = hit
        .role_hint
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let lower = raw_role.to_ascii_lowercase();
        let formatted = if lower.starts_with("as ")
            || raw_role.eq_ignore_ascii_case("cast")
            || raw_role.eq_ignore_ascii_case("self")
            || raw_role.eq_ignore_ascii_case("himself")
            || raw_role.eq_ignore_ascii_case("herself")
            || is_crew_only_role(raw_role)
        {
            raw_role.to_owned()
        } else {
            format!("as {raw_role}")
        };
        return Some(SharedString::from(formatted));
    }

    hit.year
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|y| SharedString::from(y.to_owned()))
        .or_else(|| Some(SharedString::from(sentence_case_label(&hit.media_type))))
}

fn parse_credit_year(hit: &EditorialMediaHit) -> u32 {
    hit.year
        .as_deref()
        .map(|y| {
            y.trim()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .and_then(|digits| digits.parse::<u32>().ok())
        .unwrap_or(0)
}

fn filter_and_sort_person_credits(
    filmo: &PersonFilmography,
    dept_filter: PersonDeptFilter,
    sort_recent: bool,
) -> Vec<EditorialMediaHit> {
    let mut seen: HashSet<(String, String)> =
        HashSet::with_capacity(filmo.movies.len() + filmo.series.len());
    let mut credits: Vec<EditorialMediaHit> = Vec::new();
    let is_primary_director = filmo.department.eq_ignore_ascii_case("Directing");

    for hit in filmo.movies.iter().chain(filmo.series.iter()) {
        let key = (hit.media_type.to_ascii_lowercase(), hit_target_id(hit));
        if !seen.insert(key) {
            continue;
        }
        let matches = match dept_filter {
            PersonDeptFilter::All => true,
            PersonDeptFilter::Directing => hit.role_hint.as_deref().is_some_and(is_directing_role),
            PersonDeptFilter::Acting => match hit.role_hint.as_deref() {
                Some(role) => !is_crew_only_role(role),
                None => !is_primary_director,
            },
        };
        if matches {
            credits.push(hit.clone());
        }
    }

    credits.sort_by(|a, b| {
        if sort_recent {
            Reverse(parse_credit_year(a))
                .cmp(&Reverse(parse_credit_year(b)))
                .then_with(|| Reverse(a.score).cmp(&Reverse(b.score)))
                .then_with(|| a.title.to_ascii_lowercase().cmp(&b.title.to_ascii_lowercase()))
        } else {
            Reverse(a.score)
                .cmp(&Reverse(b.score))
                .then_with(|| Reverse(parse_credit_year(a)).cmp(&Reverse(parse_credit_year(b))))
                .then_with(|| a.title.to_ascii_lowercase().cmp(&b.title.to_ascii_lowercase()))
        }
    });

    credits
}

impl AppShell {
    #[allow(clippy::too_many_arguments)]
    fn render_person_credit_card(
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

    pub fn render_person_view(
        &mut self,
        _route_person_key: &str,
        route_name: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = active_theme();
        let accent_ink = if t.is_light { t.accent_press } else { t.accent };
        let filmo = self.active_person_filmography.clone();
        let display_name = filmo
            .as_ref()
            .filter(|f| !f.name.trim().is_empty())
            .map(|f| f.name.clone())
            .unwrap_or_else(|| route_name.to_owned());
        let department = filmo
            .as_ref()
            .map(|f| f.department.clone())
            .unwrap_or_else(|| "Acting".to_owned());
        let headshot_tex = filmo
            .as_ref()
            .and_then(|f| f.headshot_url.as_deref())
            .and_then(|u| self.poster_cache.resolve_poster_image(u));
        let initials = person_initials(&display_name);

        let library_statuses = self.cached_library_statuses();

        let has_tmdb = {
            let cfg = self.core.private_integrations();
            cfg.tmdb_enabled && cfg.has_tmdb_configured()
        };

        let bio_block = filmo
            .as_ref()
            .and_then(|f| f.biography.as_deref())
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(|bio| {
                let is_long = bio.chars().count() > BIO_COLLAPSE_CHARS;
                let text = if self.person_bio_expanded || !is_long {
                    bio.to_owned()
                } else {
                    truncate_bio_at_word_boundary(bio, BIO_COLLAPSE_CHARS).0
                };
                (text, is_long)
            });

        let total_raw_credits = filmo
            .as_ref()
            .map_or(0, |f| f.movies.len() + f.series.len());
        let credits: Vec<EditorialMediaHit> = filmo
            .as_ref()
            .map(|f| {
                filter_and_sort_person_credits(
                    f,
                    self.person_department_filter,
                    self.person_sort_recent,
                )
            })
            .unwrap_or_default();

        let total_credits = credits.len();
        let viewport_w = self.viewport_size_px.0;
        let poster = PosterMetrics::from_theme(&t);
        let grid_gap = poster.gap_px;
        let metrics = compute_app_grid_metrics(
            total_credits,
            viewport_w,
            0.0,
            PosterAspect::Portrait2x3,
            &t,
        );
        let columns = metrics.columns;
        let row_count = metrics.layout.row_count;
        let row_stride_px = metrics.row_stride_px;
        let image_height_px = metrics.image_height_px;

        let kb_items: Vec<KbFocusableCard> = credits
            .iter()
            .enumerate()
            .map(|(idx, hit)| KbFocusableCard {
                route: AppRoute::MetaDetails {
                    type_name: hit.media_type.clone(),
                    id: hit_target_id(hit),
                    video_id: None,
                },
                row: idx / columns,
                col: idx % columns,
            })
            .collect();
        self.set_kb_focus_items(kb_items);

        let credits_arc = Arc::new(credits);
        let statuses_arc = Arc::clone(&library_statuses);

        let total_grid_h = row_count as f32 * row_stride_px;
        let max_grid_h = (self.viewport_size_px.1 - 180.0).max(460.0);
        let list_height_px = total_grid_h.min(max_grid_h);

        let bio_expanded = self.person_bio_expanded;
        let dept_filter = self.person_department_filter;
        let sort_recent = self.person_sort_recent;
        let credit_count_label = if total_credits == 1 {
            "1 credit".to_owned()
        } else {
            format!("{total_credits} credits")
        };

        let raw_credit_badge = (total_raw_credits > 0).then(|| {
            SharedString::from(if total_raw_credits == 1 {
                "1 credit".to_owned()
            } else {
                format!("{total_raw_credits} credits")
            })
        });

        div()
            .id("person-view-scroll")
            .track_scroll(&self.person_scroll_handle)
            .size_full()
            .overflow_y_scroll()
            .child(
                page_container(PageWidth::Wide, viewport_w)
                    .child(render_page_header(
                        PageHeader::new(display_name.clone())
                            .back_button(
                                "person-view-back",
                                "Back",
                                cx.listener(|this, _, _, cx| {
                                    this.navigate_back(cx);
                                }),
                            )
                            .count_badge(raw_credit_badge)
                            .subtitle(Some(department.clone().into()))
                            .right_slot(
                                self.person_loading.then(|| {
                                    neutral_chip("Loading filmography...").into_any_element()
                                }),
                            ),
                    ))
                    .child(
                        div()
                            .p(sp(24.0))
                            .rounded(px(RADIUS_CARD))
                            .bg(rgba(t.glass_card_bg))
                            .border_1()
                            .border_color(rgba(t.glass_border))
                            .flex()
                            .items_start()
                            .gap(sp(24.0))
                            .child(
                                div()
                                    .w(sp(128.0))
                                    .h(sp(168.0))
                                    .flex_shrink_0()
                                    .rounded(px(RADIUS_POSTER))
                                    .overflow_hidden()
                                    .bg(rgb(t.bg_elevated))
                                    .border_1()
                                    .border_color(rgba(t.glass_border))
                                    .shadow(poster_card_shadow())
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
                                            .text_size(sp(38.0))
                                            .text_color(rgb(t.text_secondary))
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
                                    .gap(sp(10.0))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .gap(sp(10.0))
                                            .font(mono_font())
                                            .text_xs()
                                            .text_color(rgb(t.text_secondary))
                                            .child(
                                                div()
                                                    .text_color(rgb(accent_ink))
                                                    .child(department),
                                            )
                                            .when_some(
                                                filmo.as_ref().and_then(|f| f.birthday.clone()),
                                                |el, bday| {
                                                    el.child(meta_divider())
                                                        .child(format!("Born {bday}"))
                                                },
                                            )
                                            .when_some(
                                                filmo
                                                    .as_ref()
                                                    .and_then(|f| f.place_of_birth.clone()),
                                                |el, pob| el.child(meta_divider()).child(pob),
                                            ),
                                    )
                                    .when_some(bio_block, |el, (bio_text, is_long)| {
                                        el.child(
                                            div()
                                                .mt(sp(4.0))
                                                .max_w(px(780.0))
                                                .flex()
                                                .flex_col()
                                                .items_start()
                                                .gap(sp(8.0))
                                                .child(
                                                    div()
                                                        .text_size(sp(14.0))
                                                        .line_height(sp(21.0))
                                                        .text_color(rgb(t.text_secondary))
                                                        .child(bio_text),
                                                )
                                                .when(is_long, |col| {
                                                    col.child(
                                                        div()
                                                            .id("person-bio-toggle")
                                                            .cursor_pointer()
                                                            .h(sp(28.0))
                                                            .px(sp(11.0))
                                                            .rounded(px(RADIUS_PILL))
                                                            .bg(rgba(t.glass_trough_bg))
                                                            .border_1()
                                                            .border_color(rgba(t.glass_border))
                                                            .hover(move |s| {
                                                                s.bg(rgba(t.glass_row_hover_bg))
                                                                    .border_color(rgba(
                                                                        t.glass_border_strong,
                                                                    ))
                                                            })
                                                            .flex()
                                                            .items_center()
                                                            .gap(sp(5.0))
                                                            .font(ui_sans(FontWeight::MEDIUM))
                                                            .text_size(sp(12.0))
                                                            .text_color(rgb(accent_ink))
                                                            .on_mouse_down(
                                                                MouseButton::Left,
                                                                cx.listener(|this, _, _, cx| {
                                                                    cx.stop_propagation();
                                                                    this.person_bio_expanded =
                                                                        !this.person_bio_expanded;
                                                                    cx.notify();
                                                                }),
                                                            )
                                                            .child(if bio_expanded {
                                                                "Show less"
                                                            } else {
                                                                "Read more"
                                                            })
                                                            .child(svg_icon_weighted(
                                                                if bio_expanded {
                                                                    IconName::ArrowUp
                                                                } else {
                                                                    IconName::ArrowDown
                                                                },
                                                                IconWeight::Regular,
                                                                12.0 * t.ui_scale,
                                                                rgb(accent_ink),
                                                            )),
                                                    )
                                                }),
                                        )
                                    }),
                            ),
                    )
                    .when(total_raw_credits > 0, |el| {
                        let filmography_controls = div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(sp(10.0))
                            .child(glass_segmented_bar("person-dept-filter").children(
                                PersonDeptFilter::ALL.into_iter().enumerate().map(|(idx, dept)| {
                                    glass_segmented_item(
                                        ("person-dept-item", idx),
                                        dept.label(),
                                        dept_filter == dept,
                                        cx.listener(move |this, _, _, cx| {
                                            if this.person_department_filter != dept {
                                                this.person_department_filter = dept;
                                                this.kb_focus_index = None;
                                                this.person_credits_scroll.scroll_to_item(
                                                    0,
                                                    gpui::ScrollStrategy::Top,
                                                );
                                                cx.notify();
                                            }
                                        }),
                                    )
                                }),
                            ))
                            .child(
                                glass_segmented_bar("person-sort-bar")
                                    .child(glass_segmented_item(
                                        "person-sort-popularity",
                                        "Popularity",
                                        !sort_recent,
                                        cx.listener(|this, _, _, cx| {
                                            if this.person_sort_recent {
                                                this.person_sort_recent = false;
                                                this.kb_focus_index = None;
                                                this.person_credits_scroll.scroll_to_item(
                                                    0,
                                                    gpui::ScrollStrategy::Top,
                                                );
                                                cx.notify();
                                            }
                                        }),
                                    ))
                                    .child(glass_segmented_item(
                                        "person-sort-newest",
                                        "Newest",
                                        sort_recent,
                                        cx.listener(|this, _, _, cx| {
                                            if !this.person_sort_recent {
                                                this.person_sort_recent = true;
                                                this.kb_focus_index = None;
                                                this.person_credits_scroll.scroll_to_item(
                                                    0,
                                                    gpui::ScrollStrategy::Top,
                                                );
                                                cx.notify();
                                            }
                                        }),
                                    )),
                            );

                        el.child(render_section_header(
                            SectionHeader::new("Filmography")
                                .count_badge(Some(credit_count_label.into()))
                                .right_slot(Some(filmography_controls.into_any_element())),
                        ))
                    })
                    .when(total_credits > 0, |el| {
                        el.child(
                            div()
                                .w_full()
                                .h(px(list_height_px))
                                .flex()
                                .flex_col()
                                .child(
                                    gpui::uniform_list(
                                        "person-credits-grid",
                                        row_count,
                                        cx.processor(
                                            move |this, visible_rows: Range<usize>, _window, cx| {
                                                visible_rows
                                                    .map(|row_idx| {
                                                        let range = slice_row_items(
                                                            total_credits,
                                                            columns,
                                                            row_idx,
                                                        );
                                                        let empty_slots =
                                                            columns.saturating_sub(range.len());

                                                        div()
                                                            .id(("person-credit-row", row_idx))
                                                            .w_full()
                                                            .h(px(row_stride_px))
                                                            .pb(px(grid_gap))
                                                            .flex()
                                                            .items_start()
                                                            .gap(px(grid_gap))
                                                            .children(range.map(|item_idx| {
                                                                let hit = &credits_arc[item_idx];
                                                                let sub =
                                                                    format_person_credit_subtitle(
                                                                        hit,
                                                                    );
                                                                let is_focused =
                                                                    this.kb_focus_index
                                                                        == Some(item_idx);
                                                                div()
                                                                    .flex_1()
                                                                    .min_w(px(0.0))
                                                                    .h_full()
                                                                    .child(
                                                                        this.render_person_credit_card(
                                                                            ("person-credit-card", item_idx),
                                                                            hit,
                                                                            sub,
                                                                            image_height_px,
                                                                            &statuses_arc,
                                                                            is_focused,
                                                                            cx,
                                                                        ),
                                                                    )
                                                            }))
                                                            .children((0..empty_slots).map(|_| {
                                                                div().flex_1().min_w(px(0.0))
                                                            }))
                                                    })
                                                    .collect()
                                            },
                                        ),
                                    )
                                    .track_scroll(&self.person_credits_scroll)
                                    .flex_1()
                                    .w_full()
                                    .min_h(px(0.0)),
                                ),
                        )
                    })
                    .when(total_credits == 0 && total_raw_credits > 0, |el| {
                        let dept_lower = dept_filter.label().to_ascii_lowercase();
                        el.child(render_empty_state(
                            EmptyStateProps::new(
                                IconName::Sliders,
                                format!("No {dept_lower} credits"),
                                format!("No {dept_lower} credits matched for {display_name}."),
                            )
                            .action(choice_pill(
                                "person-filter-reset",
                                "Show all credits",
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.person_department_filter = PersonDeptFilter::All;
                                    this.kb_focus_index = None;
                                    this.person_credits_scroll
                                        .scroll_to_item(0, gpui::ScrollStrategy::Top);
                                    cx.notify();
                                }),
                            )),
                        ))
                    })
                    .when(!self.person_loading && total_raw_credits == 0, |el| {
                        el.child(render_empty_state(EmptyStateProps::new(
                            IconName::Film,
                            "No filmography credits",
                            if has_tmdb {
                                "No filmography credits returned for this person."
                            } else {
                                "No cached titles matched locally yet. Filmographies populate automatically from browsed catalogs, or configure a TMDB API key in Settings -> Integrations & Keys for complete credits."
                            },
                        )))
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_credit(
        id: &str,
        media_type: &str,
        title: &str,
        year: Option<&str>,
        score: i64,
        role_hint: Option<&str>,
    ) -> EditorialMediaHit {
        EditorialMediaHit {
            canonical_id: id.to_owned(),
            imdb_id: id.starts_with("tt").then(|| id.to_owned()),
            tmdb_id: None,
            media_type: media_type.to_owned(),
            title: title.to_owned(),
            year: year.map(str::to_owned),
            imdb_rating: None,
            overview: None,
            poster_url: None,
            backdrop_url: None,
            logo_url: None,
            genres: Vec::new(),
            score,
            role_hint: role_hint.map(str::to_owned),
        }
    }

    #[test]
    fn truncate_bio_never_splits_words_and_tracks_long_flag() {
        let short = "Canadian filmmaker known for Dune and Arrival.";
        let (out_short, is_long_short) = truncate_bio_at_word_boundary(short, 100);
        assert_eq!(out_short, short);
        assert!(!is_long_short);

        let long = "Denis Villeneuve is a Canadian film director and screenwriter known for sci-fi.";
        let (out_long, is_long_true) = truncate_bio_at_word_boundary(long, 32);
        assert!(is_long_true);
        assert!(out_long.ends_with("..."));
        assert!(!out_long.contains("direc..."));
    }

    #[test]
    fn person_credit_subtitle_formats_cast_and_crew_roles() {
        let acting = make_credit(
            "tt15239678",
            "movie",
            "Dune: Part Two",
            Some("2024"),
            90,
            Some("Paul Atreides"),
        );
        assert_eq!(
            format_person_credit_subtitle(&acting).as_deref(),
            Some("as Paul Atreides")
        );

        let directing = make_credit(
            "tt1160419",
            "movie",
            "Dune",
            Some("2021"),
            85,
            Some("Director"),
        );
        assert_eq!(
            format_person_credit_subtitle(&directing).as_deref(),
            Some("Director")
        );

        let fallback = make_credit("tt0111161", "movie", "Fallback", Some("1994"), 50, None);
        assert_eq!(
            format_person_credit_subtitle(&fallback).as_deref(),
            Some("1994")
        );
    }

    #[test]
    fn filter_and_sort_credits_partitions_departments_and_orders_correctly() {
        let filmo = PersonFilmography {
            person_key: "tmdb_person:1".to_owned(),
            tmdb_person_id: Some(1),
            name: "Greta Gerwig".to_owned(),
            department: "Directing".to_owned(),
            biography: None,
            birthday: None,
            place_of_birth: None,
            headshot_url: None,
            movies: vec![
                make_credit(
                    "tt1517268",
                    "movie",
                    "Barbie",
                    Some("2023"),
                    250,
                    Some("Director"),
                ),
                make_credit(
                    "tt2347569",
                    "movie",
                    "Frances Ha",
                    Some("2012"),
                    90,
                    Some("Frances Halliday"),
                ),
                make_credit(
                    "tt10919380",
                    "movie",
                    "White Noise",
                    Some("2022"),
                    120,
                    Some("Babette Gladney"),
                ),
            ],
            series: vec![make_credit(
                "tt999",
                "series",
                "Untitled Series",
                None,
                300,
                Some("Executive Producer"),
            )],
        };

        let recent_all = filter_and_sort_person_credits(&filmo, PersonDeptFilter::All, true);
        let ids_recent: Vec<&str> = recent_all.iter().map(|h| h.canonical_id.as_str()).collect();
        assert_eq!(ids_recent, vec!["tt1517268", "tt10919380", "tt2347569", "tt999"]);

        let pop_all = filter_and_sort_person_credits(&filmo, PersonDeptFilter::All, false);
        let ids_pop: Vec<&str> = pop_all.iter().map(|h| h.canonical_id.as_str()).collect();
        assert_eq!(ids_pop, vec!["tt999", "tt1517268", "tt10919380", "tt2347569"]);

        let directing =
            filter_and_sort_person_credits(&filmo, PersonDeptFilter::Directing, true);
        assert_eq!(directing.len(), 1);
        assert_eq!(directing[0].canonical_id, "tt1517268");

        let acting = filter_and_sort_person_credits(&filmo, PersonDeptFilter::Acting, true);
        let acting_ids: Vec<&str> = acting.iter().map(|h| h.canonical_id.as_str()).collect();
        assert_eq!(acting_ids, vec!["tt10919380", "tt2347569"]);
    }
}
