use gpui::{div, img, prelude::*, px, rgb, rgba, AnimationExt, Context, FontWeight, MouseButton};
use stremio_core_runtime::stremio_core::models::common::Loadable;

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{
        classify_library_item_status, FocusedInput, LibraryStatusTab, SidebarTab,
    },
    theme::{
        glass_capsule_shadow, keycap, meta_divider, mono_font, obsidian_fade_in,
        placeholder_tint_for_id, sentence_case_label, tabular_nums, ui_sans, ACCENT, ACCENT_AMBER,
        ACCENT_SOFT, BG_ELEVATED, BG_POPOVER_GLASS, BORDER_POSTER_HOVER, BORDER_SOFT,
        BORDER_SUBTLE, STATUS_WATCHED, STATUS_WATCHING, STATUS_WATCHLIST, TEXT_MUTED, TEXT_PRIMARY,
        TEXT_SECONDARY,
    },
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
    ActionReloadEngine,
    RecentQuery(String),
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

impl AppShell {
    pub fn build_palette_entries(&self) -> Vec<PaletteEntry> {
        let query = self.palette_query.trim();
        let mut entries = Vec::with_capacity(16);

        if query.is_empty() {
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
            for (q, _) in recent.into_iter().take(4) {
                entries.push(PaletteEntry::RecentQuery(q));
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

        let q_lower = query.to_ascii_lowercase();
        entries.push(PaletteEntry::OpenFullSearch {
            query: query.to_owned(),
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

        let (local_fts_hits, _local_people) = self.core.metadata_engine().search_local_fts(query, 6);

        let media_hits = self.core.with_model(|model| {
            let mut seen = std::collections::HashSet::new();
            let mut hits = Vec::new();

            for fts in local_fts_hits {
                let canonical = fts.imdb_id.as_deref().unwrap_or(&fts.canonical_id);
                if seen.insert(canonical.to_owned())
                    && (canonical == fts.canonical_id || seen.insert(fts.canonical_id.clone()))
                {
                    let lib_status = model
                        .ctx
                        .library
                        .items
                        .get(canonical)
                        .or_else(|| model.ctx.library.items.get(&fts.canonical_id))
                        .and_then(classify_library_item_status);
                    hits.push(PaletteEntry::MediaHit {
                        id: fts.canonical_id,
                        type_name: fts.media_type,
                        name: fts.title,
                        release_info: fts.year,
                        imdb_rating: fts.imdb_rating,
                        poster_url: fts.poster_url,
                        library_status: lib_status,
                    });
                }
            }

            for item in model.local_search.search_results.iter().take(10) {
                if !item.name.is_empty() && seen.insert(item.id.clone()) {
                    let lib_status = model
                        .ctx
                        .library
                        .items
                        .get(&item.id)
                        .and_then(classify_library_item_status);
                    hits.push(PaletteEntry::MediaHit {
                        id: item.id.clone(),
                        type_name: item.r#type.clone(),
                        name: item.name.clone(),
                        release_info: item.release_info.clone(),
                        imdb_rating: item.imdb_rating.map(|r| format!("{:.1}", r.to_f64())),
                        poster_url: item.poster.as_ref().map(|u| u.to_string()),
                        library_status: lib_status,
                    });
                }
            }

            for cat in &model.search.catalogs {
                if let Some(page) = cat.first() {
                    if let Some(Loadable::Ready(items)) = &page.content {
                        for item in items.iter().take(10) {
                            if seen.insert(item.id.clone()) {
                                let rating = crate::state::extract_imdb_rating(item);
                                let lib_status = model
                                    .ctx
                                    .library
                                    .items
                                    .get(&item.id)
                                    .and_then(classify_library_item_status);
                                hits.push(PaletteEntry::MediaHit {
                                    id: item.id.clone(),
                                    type_name: item.r#type.clone(),
                                    name: item.name.clone(),
                                    release_info: item.release_info.clone(),
                                    imdb_rating: rating,
                                    poster_url: item.poster.as_ref().map(|u| u.to_string()),
                                    library_status: lib_status,
                                });
                            }
                            if hits.len() >= 14 {
                                break;
                            }
                        }
                    }
                }
                if hits.len() >= 14 {
                    break;
                }
            }
            hits
        });

        entries.extend(media_hits);
        entries
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
                self.palette_selected_idx = 0;
                self.focused_input = Some(FocusedInput::CommandPalette);
                self.core.search(q);
                cx.notify();
            }
            PaletteEntry::MediaHit {
                id,
                type_name,
                ..
            } => {
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
        let on_player = self.route.is_player();

        div()
            .id("search-dropdown-backdrop")
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_command_palette(cx);
                }),
            )
            .child(
                div()
                    .id("search-dropdown-panel")
                    .absolute()
                    .top(if on_player { px(72.0) } else { px(60.0) })
                    .right(if on_player { px(64.0) } else { px(76.0) })
                    .w(px(440.0))
                    .max_h(px(480.0))
                    .rounded(px(12.0))
                    .bg(rgba(BG_POPOVER_GLASS))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .shadow(glass_capsule_shadow())
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .id("search-dropdown-results")
                            .flex_1()
                            .overflow_y_scroll()
                            .p(px(6.0))
                            .child(
                                div()
                                    .w_full()
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
                                            .rounded(px(8.0))
                                            .bg(if is_selected {
                                                rgb(BG_ELEVATED)
                                            } else {
                                                rgba(0x00000000)
                                            })
                                            .border_1()
                                            .border_color(if is_selected {
                                                rgba(BORDER_POSTER_HOVER)
                                            } else {
                                                rgba(0x00000000)
                                            })
                                            .hover(|s| s.bg(rgb(BG_ELEVATED)))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap(px(12.0))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.execute_palette_entry(
                                                        entry_for_click.clone(),
                                                        cx,
                                                    );
                                                }),
                                            )
                                            .child(self.render_palette_row_left(&entry))
                                            .child(Self::render_palette_row_right(&entry))
                                    })),
                            ),
                    )
                    .when(query_non_empty, |el| {
                        el.child(
                            div()
                                .id("search-dropdown-more")
                                .cursor_pointer()
                                .px(px(14.0))
                                .py(px(10.0))
                                .border_t_1()
                                .border_color(rgba(BORDER_SOFT))
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
                    }),
            )
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
            PaletteEntry::OpenFullSearch { .. } => div()
                .flex_shrink_0()
                .child(keycap("Enter")),
            PaletteEntry::QuickNav { tab, .. } => div()
                .flex_shrink_0()
                .child(keycap(format!("Alt {}", tab.shortcut_label()))),
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
