use std::collections::HashMap;

use gpui::{div, img, prelude::*, px, rgb, rgba, AnimationExt, Context, FontWeight, MouseButton};
use stremio_core_runtime::UnifiedEditorialSearch;

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{FocusedInput, LibraryStatusTab, SidebarTab},
    theme::{
        glass_capsule_shadow, keycap, meta_divider, mono_font, obsidian_fade_in,
        placeholder_tint_for_id, sentence_case_label, tabular_nums, ui_sans, ACCENT, ACCENT_AMBER,
        ACCENT_SOFT, BG_ELEVATED, BORDER_ACCENT, GLASS_BORDER, GLASS_DIVIDER, GLASS_ROW_HOVER_BG,
        GLASS_ROW_SELECTED_BG, GLASS_SHEET_BG, GLASS_TROUGH_BG, RADIUS_CONTROL, RADIUS_PILL,
        RADIUS_POSTER, RADIUS_SHEET, STATUS_WATCHED, STATUS_WATCHING, STATUS_WATCHLIST, TEXT_MUTED,
        TEXT_PRIMARY, TEXT_SECONDARY,
    },
    views::search_view::{hit_library_status, hit_target_id, person_initials},
};

#[derive(Clone, Debug)]
pub enum PaletteEntry {
    OpenFullSearch {
        query: String,
    },
    QuickNav {
        tab: SidebarTab,
        title: &'static str,
        subtitle: &'static str,
    },
    ActionClearRecent,
    ActionReloadEngine,
    RecentQuery(String),
    PersonHit {
        person_key: String,
        name: String,
        department: String,
        headshot_url: Option<String>,
        known_for: Vec<String>,
    },
    MediaHit {
        id: String,
        type_name: String,
        name: String,
        release_info: Option<String>,
        imdb_rating: Option<String>,
        poster_url: Option<String>,
        library_status: Option<LibraryStatusTab>,
    },
}

const MAX_PALETTE_MEDIA_HITS: usize = 12;
const MAX_PALETTE_PERSON_HITS: usize = 4;

/// Builds the live search dropdown rows from a deduplicated, relevance-ranked
/// `UnifiedEditorialSearch` snapshot so the dropdown always mirrors the same
/// top match and cross-catalog ranking shown after pressing Enter.
pub fn build_palette_search_entries(
    query: &str,
    snapshot: &UnifiedEditorialSearch,
    library_statuses: &HashMap<String, LibraryStatusTab>,
) -> Vec<PaletteEntry> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let mut entries = Vec::with_capacity(20);
    let q_lower = trimmed.to_ascii_lowercase();
    entries.push(PaletteEntry::OpenFullSearch {
        query: trimmed.to_owned(),
    });

    for tab in SidebarTab::ALL {
        if tab.label().to_ascii_lowercase().contains(&q_lower) {
            entries.push(PaletteEntry::QuickNav {
                tab,
                title: tab.label(),
                subtitle: "Jump to section",
            });
        }
    }

    for hit in snapshot.ranked_media_hits(MAX_PALETTE_MEDIA_HITS) {
        let lib_status = hit_library_status(&hit, library_statuses);
        let target_id = hit_target_id(&hit);
        entries.push(PaletteEntry::MediaHit {
            id: target_id,
            type_name: hit.media_type,
            name: hit.title,
            release_info: hit.year,
            imdb_rating: hit.imdb_rating,
            poster_url: hit.poster_url,
            library_status: lib_status,
        });
    }

    for person in snapshot.people.iter().take(MAX_PALETTE_PERSON_HITS) {
        entries.push(PaletteEntry::PersonHit {
            person_key: person.person_key.clone(),
            name: person.name.clone(),
            department: person.department.clone(),
            headshot_url: person.headshot_url.clone(),
            known_for: person.known_for.clone(),
        });
    }

    entries
}

impl AppShell {
    pub fn build_palette_entries(&self) -> Vec<PaletteEntry> {
        let query = self.palette_query.trim();

        if query.is_empty() {
            let mut entries = Vec::with_capacity(20);
            let mut recent: Vec<(String, chrono::DateTime<chrono::Utc>)> =
                self.core.with_model(|m| {
                    m.ctx
                        .search_history
                        .items
                        .iter()
                        .map(|(q, ts)| (q.clone(), *ts))
                        .collect()
                });
            recent.sort_by_key(|b| std::cmp::Reverse(b.1));
            let has_recent = !recent.is_empty();
            for (q, _) in recent.into_iter().take(4) {
                entries.push(PaletteEntry::RecentQuery(q));
            }
            if has_recent {
                entries.push(PaletteEntry::ActionClearRecent);
            }

            entries.push(PaletteEntry::QuickNav {
                tab: SidebarTab::Home,
                title: "Home",
                subtitle: "Continue watching and editorial shelves",
            });
            entries.push(PaletteEntry::QuickNav {
                tab: SidebarTab::Discover,
                title: "Discover",
                subtitle: "Browse movies, shows, and genres",
            });
            entries.push(PaletteEntry::QuickNav {
                tab: SidebarTab::Calendar,
                title: "Calendar",
                subtitle: "Upcoming episodes and release schedule",
            });
            entries.push(PaletteEntry::QuickNav {
                tab: SidebarTab::Library,
                title: "Your library",
                subtitle: "Watching, watchlist, and watched collection",
            });
            entries.push(PaletteEntry::QuickNav {
                tab: SidebarTab::Addons,
                title: "Addons",
                subtitle: "Install manifest URLs or manage sources",
            });
            entries.push(PaletteEntry::QuickNav {
                tab: SidebarTab::Settings,
                title: "Settings",
                subtitle: "Storage mode, UPnP, failover, and playback",
            });
            entries.push(PaletteEntry::QuickNav {
                tab: SidebarTab::Account,
                title: "Account",
                subtitle: "Sign in with email or Stremio link code",
            });
            entries.push(PaletteEntry::ActionReloadEngine);
            return entries;
        }

        let snapshot = self.core.editorial_search_snapshot(query);
        let library_statuses = self.cached_library_statuses();
        build_palette_search_entries(query, &snapshot, library_statuses.as_ref())
    }

    pub fn execute_palette_entry(&mut self, entry: PaletteEntry, cx: &mut Context<Self>) {
        match entry {
            PaletteEntry::OpenFullSearch { query } => {
                self.close_command_palette(cx);
                self.open_full_search(query, cx);
            }
            PaletteEntry::QuickNav { tab, .. } => {
                self.close_command_palette(cx);
                self.navigate_tab(tab, cx);
            }
            PaletteEntry::ActionClearRecent => {
                self.core.clear_search_history();
                self.palette_selected_idx = 0;
                self.palette_scroll_handle.scroll_to_item(0);
                self.push_toast(
                    crate::state::ToastKind::Info,
                    "Search history cleared",
                    None,
                    cx,
                );
                cx.notify();
            }
            PaletteEntry::ActionReloadEngine => {
                self.close_command_palette(cx);
                self.core.reload_streaming_server();
                self.push_toast(
                    crate::state::ToastKind::Info,
                    "Streaming engine reloaded",
                    None,
                    cx,
                );
            }
            PaletteEntry::RecentQuery(q) => {
                self.palette_query = q.clone();
                self.input_cursors
                    .insert(FocusedInput::CommandPalette, self.palette_query.len());
                self.input_selected_all
                    .remove(&FocusedInput::CommandPalette);
                self.palette_selected_idx = 0;
                self.palette_scroll_handle.scroll_to_item(0);
                self.focused_input = Some(FocusedInput::CommandPalette);
                self.search_debounce_seq = self.search_debounce_seq.wrapping_add(1);
                self.core.search(q);
                cx.notify();
            }
            PaletteEntry::PersonHit {
                person_key, name, ..
            } => {
                self.close_command_palette(cx);
                self.open_person_details(person_key, name, cx);
            }
            PaletteEntry::MediaHit { id, type_name, .. } => {
                self.close_command_palette(cx);
                self.open_editorial_media_hit(type_name, id, None, cx);
            }
        }
    }

    pub fn render_command_palette(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entries = self.build_palette_entries();
        let selected_idx = if entries.is_empty() {
            0
        } else {
            self.palette_selected_idx.min(entries.len() - 1)
        };
        let query_non_empty = !self.palette_query.trim().is_empty();
        let query_has_chars = !self.palette_query.is_empty();
        let on_player = self.route.is_player();
        let caret_query =
            self.format_input_with_caret(FocusedInput::CommandPalette, &self.palette_query);

        let panel_base = div()
            .id("search-dropdown-panel")
            .occlude()
            .rounded(px(RADIUS_SHEET))
            .bg(rgba(GLASS_SHEET_BG))
            .border_1()
            .border_color(rgba(GLASS_BORDER))
            .shadow(glass_capsule_shadow())
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            });

        let panel_positioned = if on_player {
            panel_base
                .w(px(560.0))
                .max_w(px(560.0))
                .max_h(px(520.0))
                .child(
                    div()
                        .id("player-palette-search-header")
                        .w_full()
                        .px(px(16.0))
                        .py(px(12.0))
                        .border_b_1()
                        .border_color(rgba(GLASS_DIVIDER))
                        .bg(rgba(GLASS_TROUGH_BG))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .child(svg_icon(IconName::Search, 16.0, rgb(ACCENT)))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .flex()
                                        .items_center()
                                        .font(ui_sans(FontWeight::MEDIUM))
                                        .text_size(px(14.5))
                                        .text_color(rgb(TEXT_PRIMARY))
                                        .truncate()
                                        .child(caret_query)
                                        .when(!query_has_chars, |el| {
                                            el.child(
                                                div()
                                                    .pl(px(4.0))
                                                    .font(ui_sans(FontWeight::NORMAL))
                                                    .text_color(rgb(TEXT_MUTED))
                                                    .child(
                                                        "Search movies, shows, people, commands...",
                                                    ),
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
                                .when(query_has_chars, |row| {
                                    row.child(
                                        div()
                                            .id("player-palette-search-clear")
                                            .cursor_pointer()
                                            .p(px(4.0))
                                            .rounded(px(RADIUS_POSTER))
                                            .hover(|s| s.bg(rgba(GLASS_ROW_HOVER_BG)))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.clear_palette_query(cx);
                                                }),
                                            )
                                            .child(svg_icon(IconName::X, 13.0, rgb(TEXT_MUTED))),
                                    )
                                })
                                .child(keycap("Esc")),
                        ),
                )
        } else {
            panel_base
                .absolute()
                .top(px(60.0))
                .right(px(76.0))
                .w(px(460.0))
                .max_h(px(500.0))
        };

        let panel = panel_positioned
            .child(
                div()
                    .id("search-dropdown-results")
                    .track_scroll(&self.palette_scroll_handle)
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p(px(6.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .when(entries.is_empty(), |el| {
                        el.child(
                            div()
                                .flex_shrink_0()
                                .py(px(28.0))
                                .flex()
                                .justify_center()
                                .text_size(px(13.5))
                                .text_color(rgb(TEXT_SECONDARY))
                                .child("Searching catalogs..."),
                        )
                    })
                    .children(entries.into_iter().enumerate().map(|(idx, entry)| {
                        let is_selected = idx == selected_idx;
                        let entry_for_click = entry.clone();

                        div()
                            .id(("palette-row", idx))
                            .flex_shrink_0()
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(RADIUS_CONTROL))
                            .bg(if is_selected {
                                rgba(GLASS_ROW_SELECTED_BG)
                            } else {
                                rgba(0x00000000)
                            })
                            .border_1()
                            .border_color(if is_selected {
                                rgba(BORDER_ACCENT)
                            } else {
                                rgba(0x00000000)
                            })
                            .hover(|s| s.bg(rgba(GLASS_ROW_HOVER_BG)))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.execute_palette_entry(entry_for_click.clone(), cx);
                                }),
                            )
                            .child(self.render_palette_row_left(&entry))
                            .child(Self::render_palette_row_right(&entry))
                    })),
            )
            .when(query_non_empty, |el| {
                el.child(
                    div()
                        .id("search-dropdown-more")
                        .cursor_pointer()
                        .px(px(14.0))
                        .py(px(10.0))
                        .border_t_1()
                        .border_color(rgba(GLASS_DIVIDER))
                        .bg(rgba(ACCENT_SOFT))
                        .hover(|s| s.bg(rgb(BG_ELEVATED)))
                        .flex()
                        .items_center()
                        .justify_between()
                        .font(ui_sans(FontWeight::MEDIUM))
                        .text_size(px(12.5))
                        .text_color(rgb(ACCENT))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                let query = this.palette_query.clone();
                                this.close_command_palette(cx);
                                this.open_full_search(query, cx);
                            }),
                        )
                        .child("See all deduplicated results (Enter)")
                        .child(svg_icon(IconName::ChevronRight, 13.5, rgb(ACCENT))),
                )
            })
            .with_animation("search-dropdown-mount", obsidian_fade_in(), |el, delta| {
                el.opacity(delta)
            });

        div()
            .id("search-dropdown-backdrop")
            .absolute()
            .inset_0()
            .when(on_player, |el| {
                el.bg(rgba(0x0b0b0d80))
                    .flex()
                    .items_start()
                    .justify_center()
                    .pt(px(84.0))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_command_palette(cx);
                }),
            )
            .child(panel)
    }

    fn render_palette_row_left(&self, entry: &PaletteEntry) -> impl IntoElement {
        match entry {
            PaletteEntry::OpenFullSearch { query } => div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(svg_icon(IconName::Search, 15.5, rgb(ACCENT)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font(ui_sans(FontWeight::SEMIBOLD))
                                .text_size(px(13.5))
                                .text_color(rgb(TEXT_PRIMARY))
                                .truncate()
                                .child(format!("Search everything for \"{query}\"")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(TEXT_MUTED))
                                .truncate()
                                .child("Deduplicated Top Match, Movies, Series & People"),
                        ),
                ),
            PaletteEntry::QuickNav {
                tab,
                title,
                subtitle,
            } => div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(svg_icon(tab.icon_name(), 15.5, rgb(TEXT_SECONDARY)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font(ui_sans(FontWeight::SEMIBOLD))
                                .text_size(px(13.5))
                                .text_color(rgb(TEXT_PRIMARY))
                                .truncate()
                                .child(*title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(TEXT_MUTED))
                                .truncate()
                                .child(*subtitle),
                        ),
                ),
            PaletteEntry::ActionClearRecent => div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(svg_icon(IconName::X, 15.0, rgb(TEXT_MUTED)))
                .child(
                    div()
                        .text_size(px(13.5))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child("Clear recent"),
                ),
            PaletteEntry::ActionReloadEngine => div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(svg_icon(IconName::RefreshCw, 15.0, rgb(TEXT_SECONDARY)))
                .child(
                    div()
                        .text_size(px(13.5))
                        .text_color(rgb(TEXT_PRIMARY))
                        .child("Reload streaming engine"),
                ),
            PaletteEntry::RecentQuery(q) => div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(svg_icon(IconName::Clock, 15.0, rgb(TEXT_MUTED)))
                .child(
                    div()
                        .text_size(px(13.5))
                        .text_color(rgb(TEXT_PRIMARY))
                        .truncate()
                        .child(q.clone()),
                ),
            PaletteEntry::PersonHit {
                name,
                department,
                headshot_url,
                known_for,
                ..
            } => {
                let headshot_tex = headshot_url
                    .as_deref()
                    .and_then(|u| self.poster_cache.resolve_thumbnail_image(u));
                let initials = person_initials(name);
                let subtitle = if known_for.is_empty() {
                    department.clone()
                } else {
                    format!("{department} · {}", known_for.join(", "))
                };

                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .w(px(40.0))
                            .h(px(40.0))
                            .flex_shrink_0()
                            .rounded(px(RADIUS_PILL))
                            .overflow_hidden()
                            .bg(rgb(BG_ELEVATED))
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
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_xs()
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
                                    .text_size(px(14.0))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .truncate()
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .truncate()
                                    .child(subtitle),
                            ),
                    )
            }
            PaletteEntry::MediaHit {
                id,
                type_name,
                name,
                release_info,
                poster_url,
                ..
            } => {
                let poster_tex = poster_url
                    .as_deref()
                    .and_then(|u| self.poster_cache.resolve_thumbnail_image(u));
                let clean_type = sentence_case_label(type_name);
                let year_clean = release_info
                    .as_ref()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty());

                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .w(px(40.0))
                            .h(px(60.0))
                            .flex_shrink_0()
                            .rounded(px(3.0))
                            .overflow_hidden()
                            .bg(rgb(placeholder_tint_for_id(id)))
                            .when_some(poster_tex, |el, tex| {
                                el.child(img(tex).size_full().object_fit(gpui::ObjectFit::Cover))
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
                                    .text_size(px(14.0))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .truncate()
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(clean_type)
                                    .when_some(year_clean, |el, yr| {
                                        el.child(meta_divider()).child(
                                            div().font_features(tabular_nums()).child(yr),
                                        )
                                    }),
                            ),
                    )
            }
        }
    }

    fn render_palette_row_right(entry: &PaletteEntry) -> impl IntoElement {
        match entry {
            PaletteEntry::OpenFullSearch { .. } => div().flex_shrink_0().child(keycap("Enter")),
            PaletteEntry::QuickNav { tab, .. } => div()
                .flex_shrink_0()
                .child(keycap(format!("Alt {}", tab.shortcut_label()))),
            PaletteEntry::ActionClearRecent => div()
                .flex_shrink_0()
                .font(mono_font())
                .text_xs()
                .text_color(rgb(TEXT_MUTED))
                .child("History"),
            PaletteEntry::ActionReloadEngine => div()
                .flex_shrink_0()
                .font(mono_font())
                .text_xs()
                .text_color(rgb(TEXT_MUTED))
                .child("Action"),
            PaletteEntry::RecentQuery(_) => div()
                .flex_shrink_0()
                .font(mono_font())
                .text_xs()
                .text_color(rgb(TEXT_MUTED))
                .child("Recent"),
            PaletteEntry::PersonHit { .. } => div()
                .flex_shrink_0()
                .font(mono_font())
                .text_xs()
                .text_color(rgb(TEXT_MUTED))
                .child("Person"),
            PaletteEntry::MediaHit {
                imdb_rating,
                library_status,
                ..
            } => div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(10.0))
                .when_some(imdb_rating.clone(), |el, rating| {
                    el.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .font(mono_font())
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(svg_icon(IconName::Star, 11.0, rgb(ACCENT_AMBER)))
                            .child(rating),
                    )
                })
                .when_some(*library_status, |el, st| {
                    let (icon, col) = match st {
                        LibraryStatusTab::Watched => (IconName::Check, STATUS_WATCHED),
                        LibraryStatusTab::Watching => (IconName::Play, STATUS_WATCHING),
                        LibraryStatusTab::Watchlist => (IconName::Bookmark, STATUS_WATCHLIST),
                    };
                    el.child(svg_icon(icon, 14.0, rgb(col)))
                }),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use stremio_core_runtime::{
        build_unified_editorial_search, normalize_title_for_dedup, preview_to_editorial_hit,
        stremio_core::{
            constants::CINEMETA_URL,
            types::resource::{Link, MetaItemBehaviorHints, MetaItemPreview, PosterShape},
        },
        EditorialMediaHit,
    };
    use url::Url;

    use super::{build_palette_search_entries, PaletteEntry};
    use crate::state::LibraryStatusTab;

    fn make_preview(
        id: &str,
        media_type: &str,
        name: &str,
        year: Option<&str>,
        rating: Option<&str>,
        poster: Option<&str>,
    ) -> MetaItemPreview {
        let mut links = Vec::new();
        if let Some(r) = rating {
            links.push(Link {
                name: r.to_owned(),
                category: "imdb".to_owned(),
                url: CINEMETA_URL.clone(),
            });
        }
        MetaItemPreview {
            id: id.to_owned(),
            r#type: media_type.to_owned(),
            name: name.to_owned(),
            poster: poster.and_then(|u| Url::parse(u).ok()),
            background: None,
            logo: None,
            description: None,
            release_info: year.map(str::to_owned),
            runtime: None,
            released: None,
            poster_shape: PosterShape::Poster,
            links,
            trailer_streams: Vec::new(),
            behavior_hints: MetaItemBehaviorHints::default(),
        }
    }

    #[test]
    fn palette_search_entries_keep_game_of_thrones_series_first_across_movie_specials_and_tmdb() {
        let query = "game of thrones";
        let norm_q = normalize_title_for_dedup(query);

        // Simulate Cinemeta Movies catalog arriving first with many Game of Thrones movie specials.
        let movie_specials = [
            make_preview("tt10400001", "movie", "Game of Thrones: Aftermath", Some("2019"), None, None),
            make_preview("tt10400002", "movie", "Game of Thrones Watchathon", Some("2019"), None, None),
            make_preview(
                "tt10090796",
                "movie",
                "Game of Thrones: The Last Watch",
                Some("2019"),
                Some("7.2"),
                Some("https://images.metahub.space/poster/small/tt10090796/img"),
            ),
            make_preview(
                "tt10400003",
                "movie",
                "Game of Thrones: The Story So Far",
                Some("2017"),
                None,
                Some("https://images.metahub.space/poster/small/tt10400003/img"),
            ),
            make_preview("tt10400004", "movie", "Game of Thrones: The Story So Far", Some("2019"), None, None),
        ];

        // Series catalog arrives separately with the main show.
        let main_series = make_preview(
            "tt0944947",
            "series",
            "Game of Thrones",
            Some("2011-2019"),
            Some("9.2"),
            Some("https://images.metahub.space/poster/small/tt0944947/img"),
        );

        let addon_hits: Vec<EditorialMediaHit> = movie_specials
            .iter()
            .chain(std::iter::once(&main_series))
            .map(|p| preview_to_editorial_hit(p, &norm_q, 15))
            .collect();

        // TMDB also returns the same main series under tmdb:1399 (must deduplicate into tt0944947).
        let tmdb_hits = vec![EditorialMediaHit {
            canonical_id: "tmdb:1399".to_owned(),
            imdb_id: None,
            tmdb_id: Some(1399),
            media_type: "series".to_owned(),
            title: "Game of Thrones".to_owned(),
            year: Some("2011".to_owned()),
            imdb_rating: Some("8.4".to_owned()),
            overview: Some("Seven noble families fight for control of the mythical land of Westeros.".to_owned()),
            poster_url: Some("https://image.tmdb.org/t/p/w342/got.jpg".to_owned()),
            backdrop_url: None,
            logo_url: None,
            genres: Vec::new(),
            score: 1_080,
            role_hint: None,
        }];

        let snapshot = build_unified_editorial_search(
            query,
            Vec::new(),
            tmdb_hits,
            addon_hits,
            Vec::new(),
            50,
        );

        let mut library = HashMap::new();
        library.insert("tt0944947".to_owned(), LibraryStatusTab::Watching);

        let entries = build_palette_search_entries(query, &snapshot, &library);

        // Row 0 is OpenFullSearch, row 1 must be the main Game of Thrones Series.
        assert!(matches!(
            &entries[0],
            PaletteEntry::OpenFullSearch { query: q } if q == "game of thrones"
        ));

        let media_hits: Vec<_> = entries
            .iter()
            .filter_map(|e| match e {
                PaletteEntry::MediaHit {
                    id,
                    type_name,
                    name,
                    library_status,
                    ..
                } => Some((id.as_str(), type_name.as_str(), name.as_str(), *library_status)),
                _ => None,
            })
            .collect();

        assert_eq!(
            media_hits[0],
            (
                "tt0944947",
                "series",
                "Game of Thrones",
                Some(LibraryStatusTab::Watching)
            )
        );
        // No duplicate Game of Thrones entry after merging TMDB + Cinemeta.
        assert_eq!(
            media_hits
                .iter()
                .filter(|(_, _, name, _)| *name == "Game of Thrones")
                .count(),
            1
        );
        // Rated + postered special must outrank blank-poster unrated specials.
        let last_watch_pos = media_hits
            .iter()
            .position(|(id, _, _, _)| *id == "tt10090796")
            .unwrap();
        let aftermath_pos = media_hits
            .iter()
            .position(|(id, _, _, _)| *id == "tt10400001")
            .unwrap();
        assert!(last_watch_pos < aftermath_pos);
    }
}
