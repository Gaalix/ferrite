use std::{collections::HashMap, ops::Range, sync::Arc};

use gpui::{
    deferred, div, point, prelude::*, px, rgb, rgba, AnyElement, Context, FontWeight, MouseButton,
    MouseDownEvent, SharedString, Window,
};
use stremio_core_runtime::stremio_core::types::{
    library::LibraryItem, resource::PosterShape,
};

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{
        classify_library_item_status, format_movietracker_episode_code, AppRoute,
        BrowsingContextMenu, FocusedInput, KbFocusableCard, LibrarySortCriterion, LibrarySortOrder,
        LibraryStatusTab, LibraryTypeFilter, MemoizedLibraryView, SidebarTab, ToastKind,
    },
    theme::{
        editorial_serif, sentence_case_label, ui_sans, ACCENT, BORDER_ACCENT, GLASS_BORDER,
        GLASS_BORDER_STRONG, GLASS_CARD_BG, GLASS_ROW_HOVER_BG, GLASS_ROW_SELECTED_BG,
        GLASS_TROUGH_BG, RADIUS_CARD, RADIUS_CONTROL, RADIUS_PILL, TEXT_MUTED, TEXT_PRIMARY,
        TEXT_SECONDARY,
    },
    ui_components::{
        click_handler, glass_dropdown_option, glass_dropdown_panel, glass_segmented_bar,
        glass_segmented_item, hover_handler, primary_accent_button, render_poster_card,
        PosterCardProps,
    },
    virtual_grid::{compute_dynamic_grid_metrics, slice_row_items, PosterAspect},
};

impl AppShell {
    pub fn render_library_view(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let memo = self.get_or_update_memoized_library();
        let notif_counts_arc = self.library_notification_counts();

        let active_tab = self.library_status_tab;
        let active_type = self.library_type_filter;
        let active_sort = self.library_sort_criterion;
        let sort_open = self.library_sort_dropdown_open;
        let search_focused = self.focused_input == Some(FocusedInput::LibrarySearch);
        let has_search_query = !self.library_search_query.is_empty();
        let search_trimmed_non_empty = !self.library_search_query.trim().is_empty();
        let search_display =
            self.format_input_with_caret(FocusedInput::LibrarySearch, &self.library_search_query);

        let filtered_arc = Arc::clone(&memo.filtered_items);
        let total_filtered = filtered_arc.len();

        let raw_viewport_w = f32::from(window.viewport_size().width);
        let viewport_w = if raw_viewport_w > 0.0 && raw_viewport_w.is_finite() {
            raw_viewport_w
        } else {
            self.viewport_size_px.0
        };

        let grid_aspect = infer_library_grid_aspect(&filtered_arc);
        let metrics = compute_dynamic_grid_metrics(
            total_filtered,
            viewport_w,
            0.0,
            64.0,
            Some(1440.0),
            176.0,
            18.0,
            18.0,
            52.0,
            grid_aspect,
        );
        let columns = metrics.columns;
        let row_count = metrics.layout.row_count;
        let row_stride_px = metrics.row_stride_px;
        let card_width_px = metrics.card_width_px;
        let image_height_px = metrics.image_height_px;

        let kb_items: Vec<KbFocusableCard> = filtered_arc
            .iter()
            .enumerate()
            .map(|(idx, (li, _))| KbFocusableCard {
                route: AppRoute::MetaDetails {
                    type_name: li.r#type.clone(),
                    id: li.id.clone(),
                    video_id: li.state.video_id.clone(),
                },
                row: idx / columns,
                col: idx % columns,
            })
            .collect();
        self.set_kb_focus_items(kb_items);

        div()
            .id("library-view-root")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    let mut changed = false;
                    if this.library_sort_dropdown_open {
                        this.library_sort_dropdown_open = false;
                        changed = true;
                    }
                    if this.focused_input == Some(FocusedInput::LibrarySearch) {
                        this.focused_input = None;
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(1440.0))
                    .mx_auto()
                    .px(px(32.0))
                    .pt(px(28.0))
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(20.0))
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .flex_wrap()
                            .items_end()
                            .justify_between()
                            .gap(px(24.0))
                            .child(
                                div()
                                    .font(editorial_serif())
                                    .text_size(px(56.0))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .child("Your library"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(32.0))
                                    .child(render_library_stat(
                                        memo.count_watching.to_string(),
                                        "Watching",
                                    ))
                                    .child(render_library_stat(
                                        memo.count_watched.to_string(),
                                        "Watched",
                                    ))
                                    .child(render_library_stat(
                                        memo.count_watchlist.to_string(),
                                        "Saved",
                                    ))
                                    .child(render_library_stat(
                                        memo.hours_logged.to_string(),
                                        "Hours",
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(14.0))
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
                                glass_segmented_bar("lib-status-bar").children(
                                    LibraryStatusTab::ALL.into_iter().enumerate().map(
                                        |(idx, tab)| {
                                            let is_active = tab == active_tab;
                                            let count = match tab {
                                                LibraryStatusTab::Watching => memo.count_watching,
                                                LibraryStatusTab::Watchlist => memo.count_watchlist,
                                                LibraryStatusTab::Watched => memo.count_watched,
                                            };
                                            let label = format!("{} ({count})", tab.label());
                                            glass_segmented_item(
                                                ("lib-status-tab", idx),
                                                label,
                                                is_active,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.library_status_tab = tab;
                                                    this.library_sort_dropdown_open = false;
                                                    this.kb_focus_index = None;
                                                    this.library_grid_scroll_handle
                                                        .0
                                                        .borrow()
                                                        .base_handle
                                                        .set_offset(point(px(0.0), px(0.0)));
                                                    cx.notify();
                                                }),
                                            )
                                        },
                                    ),
                                ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .id("lib-search-box")
                                            .cursor_pointer()
                                            .w(px(230.0))
                                            .h(px(38.0))
                                            .px(px(12.0))
                                            .rounded(px(RADIUS_CONTROL))
                                            .bg(if search_focused {
                                                rgba(GLASS_ROW_SELECTED_BG)
                                            } else {
                                                rgba(GLASS_TROUGH_BG)
                                            })
                                            .border_1()
                                            .border_color(if search_focused {
                                                rgba(BORDER_ACCENT)
                                            } else {
                                                rgba(GLASS_BORDER)
                                            })
                                            .hover(|s| {
                                                s.border_color(if search_focused {
                                                    rgba(BORDER_ACCENT)
                                                } else {
                                                    rgba(GLASS_BORDER_STRONG)
                                                })
                                            })
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap(px(8.0))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.library_sort_dropdown_open = false;
                                                    this.focused_input =
                                                        Some(FocusedInput::LibrarySearch);
                                                    cx.notify();
                                                }),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.0))
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(8.0))
                                                    .child(svg_icon(
                                                        IconName::Search,
                                                        13.5,
                                                        if search_focused {
                                                            rgb(ACCENT)
                                                        } else {
                                                            rgb(TEXT_MUTED)
                                                        },
                                                    ))
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w(px(0.0))
                                                            .flex()
                                                            .items_center()
                                                            .font(ui_sans(FontWeight::NORMAL))
                                                            .text_size(px(13.0))
                                                            .text_color(if has_search_query {
                                                                rgb(TEXT_PRIMARY)
                                                            } else {
                                                                rgb(TEXT_MUTED)
                                                            })
                                                            .truncate()
                                                            .when(
                                                                search_focused || has_search_query,
                                                                |d| d.child(search_display.clone()),
                                                            )
                                                            .when(!has_search_query, |d| {
                                                                d.child(
                                                                    div()
                                                                        .when(
                                                                            search_focused,
                                                                            |p| p.pl(px(3.0)),
                                                                        )
                                                                        .text_color(rgb(TEXT_MUTED))
                                                                        .child("Filter library..."),
                                                                )
                                                            }),
                                                    ),
                                            )
                                            .when(has_search_query, |sb| {
                                                sb.child(
                                                    div()
                                                        .id("lib-search-clear")
                                                        .cursor_pointer()
                                                        .p(px(3.0))
                                                        .rounded(px(4.0))
                                                        .hover(|s| s.bg(rgba(GLASS_ROW_HOVER_BG)))
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(|this, _, _, cx| {
                                                                cx.stop_propagation();
                                                                this.library_search_query.clear();
                                                                this.input_cursors.remove(
                                                                    &FocusedInput::LibrarySearch,
                                                                );
                                                                this.input_selected_all.remove(
                                                                    &FocusedInput::LibrarySearch,
                                                                );
                                                                this.kb_focus_index = None;
                                                                this.library_grid_scroll_handle
                                                                    .0
                                                                    .borrow()
                                                                    .base_handle
                                                                    .set_offset(point(
                                                                        px(0.0),
                                                                        px(0.0),
                                                                    ));
                                                                cx.notify();
                                                            }),
                                                        )
                                                        .child(svg_icon(
                                                            IconName::X,
                                                            11.5,
                                                            rgb(TEXT_MUTED),
                                                        )),
                                                )
                                            }),
                                    )
                                    .child(
                                        glass_segmented_bar("lib-type-bar").children(
                                            LibraryTypeFilter::ALL.into_iter().enumerate().map(
                                                |(idx, tf)| {
                                                    let is_active = tf == active_type;
                                                    let label = match tf {
                                                        LibraryTypeFilter::All => "All",
                                                        LibraryTypeFilter::Movies => "Movies",
                                                        LibraryTypeFilter::Shows => "Series",
                                                    };
                                                    glass_segmented_item(
                                                        ("lib-type-pill", idx),
                                                        label,
                                                        is_active,
                                                        cx.listener(move |this, _, _, cx| {
                                                            this.library_type_filter = tf;
                                                            this.library_sort_dropdown_open = false;
                                                            this.kb_focus_index = None;
                                                            this.library_grid_scroll_handle
                                                                .0
                                                                .borrow()
                                                                .base_handle
                                                                .set_offset(point(
                                                                    px(0.0),
                                                                    px(0.0),
                                                                ));
                                                            cx.notify();
                                                        }),
                                                    )
                                                },
                                            ),
                                        ),
                                    )
                                    .child(
                                        div()
                                            .relative()
                                            .child(
                                                div()
                                                    .id("lib-sort-pill")
                                                    .cursor_pointer()
                                                    .h(px(38.0))
                                                    .px(px(12.0))
                                                    .rounded(px(RADIUS_CONTROL))
                                                    .bg(if sort_open {
                                                        rgba(GLASS_ROW_SELECTED_BG)
                                                    } else {
                                                        rgba(GLASS_TROUGH_BG)
                                                    })
                                                    .border_1()
                                                    .border_color(if sort_open {
                                                        rgba(BORDER_ACCENT)
                                                    } else {
                                                        rgba(GLASS_BORDER)
                                                    })
                                                    .hover(|s| {
                                                        s.bg(if sort_open {
                                                            rgba(GLASS_ROW_SELECTED_BG)
                                                        } else {
                                                            rgba(GLASS_ROW_HOVER_BG)
                                                        })
                                                        .border_color(if sort_open {
                                                            rgb(ACCENT)
                                                        } else {
                                                            rgba(GLASS_BORDER_STRONG)
                                                        })
                                                    })
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(8.0))
                                                    .font(ui_sans(FontWeight::MEDIUM))
                                                    .text_size(px(13.0))
                                                    .text_color(if sort_open {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgb(TEXT_PRIMARY)
                                                    })
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            this.library_sort_dropdown_open =
                                                                !this.library_sort_dropdown_open;
                                                            if this.focused_input
                                                                == Some(FocusedInput::LibrarySearch)
                                                            {
                                                                this.focused_input = None;
                                                            }
                                                            cx.notify();
                                                        }),
                                                    )
                                                    .child(svg_icon(
                                                        IconName::Sliders,
                                                        13.0,
                                                        if sort_open {
                                                            rgb(ACCENT)
                                                        } else {
                                                            rgb(TEXT_MUTED)
                                                        },
                                                    ))
                                                    .child(active_sort.label())
                                                    .child(svg_icon(
                                                        IconName::ChevronDown,
                                                        12.5,
                                                        if sort_open {
                                                            rgb(ACCENT)
                                                        } else {
                                                            rgb(TEXT_MUTED)
                                                        },
                                                    )),
                                            )
                                            .when(sort_open, |wrap| {
                                                wrap.child(
                                                    deferred(
                                                        glass_dropdown_panel("lib-sort-menu", 216.0)
                                                            .absolute()
                                                            .top(px(44.0))
                                                            .right_0()
                                                            .children(
                                                                LibrarySortCriterion::ALL
                                                                    .into_iter()
                                                                    .enumerate()
                                                                    .map(|(idx, crit)| {
                                                                        glass_dropdown_option(
                                                                            ("lib-sort-opt", idx),
                                                                            crit.label(),
                                                                            None,
                                                                            crit == active_sort,
                                                                            cx.listener(
                                                                                move |this,
                                                                                      _,
                                                                                      _,
                                                                                      cx| {
                                                                                    this.library_sort_criterion =
                                                                                        crit;
                                                                                    this.library_sort_order = match crit {
                                                                                        LibrarySortCriterion::TitleAsc => {
                                                                                            LibrarySortOrder::Title
                                                                                        }
                                                                                        _ => {
                                                                                            LibrarySortOrder::Recent
                                                                                        }
                                                                                    };
                                                                                    this.library_sort_dropdown_open =
                                                                                        false;
                                                                                    this.kb_focus_index =
                                                                                        None;
                                                                                    this.library_grid_scroll_handle
                                                                                        .0
                                                                                        .borrow()
                                                                                        .base_handle
                                                                                        .set_offset(point(
                                                                                            px(0.0),
                                                                                            px(0.0),
                                                                                        ));
                                                                                    cx.notify();
                                                                                },
                                                                            ),
                                                                        )
                                                                    }),
                                                            ),
                                                    )
                                                    .with_priority(10),
                                                )
                                            }),
                                    ),
                            ),
                    )
                    .child(if total_filtered == 0 {
                        render_library_empty_state(
                            active_tab,
                            search_trimmed_non_empty,
                            self.library_search_query.trim(),
                            cx,
                        )
                    } else {
                        gpui::uniform_list(
                            "library-poster-grid",
                            row_count,
                            cx.processor(move |this, visible_rows: Range<usize>, _window, cx| {
                                let kb_focus_index = this.kb_focus_index;

                                visible_rows
                                    .map(|row_idx| {
                                        let range =
                                            slice_row_items(total_filtered, columns, row_idx);
                                        let empty_slots = columns.saturating_sub(range.len());

                                        div()
                                            .id(("library-row", row_idx))
                                            .w_full()
                                            .h(px(row_stride_px))
                                            .pb(px(18.0))
                                            .flex()
                                            .items_start()
                                            .gap(px(18.0))
                                            .children(range.map(|idx| {
                                                let (li, st) = &filtered_arc[idx];
                                                let notif_count = notif_counts_arc
                                                    .get(&li.id)
                                                    .copied()
                                                    .unwrap_or(0);
                                                let is_focused = kb_focus_index == Some(idx);
                                                let card_aspect =
                                                    PosterAspect::from_stremio_poster_shape(
                                                        &li.poster_shape,
                                                    );
                                                let card_image_h = if card_aspect == grid_aspect {
                                                    image_height_px
                                                } else {
                                                    (card_width_px
                                                        * card_aspect.height_over_width())
                                                    .ceil()
                                                    .min(image_height_px)
                                                };

                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.0))
                                                    .h_full()
                                                    .child(this.render_library_grid_card(
                                                        idx,
                                                        li,
                                                        *st,
                                                        notif_count,
                                                        card_aspect,
                                                        card_image_h,
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
                        .track_scroll(&self.library_grid_scroll_handle)
                        .flex_1()
                        .w_full()
                        .min_h(px(0.0))
                        .into_any_element()
                    }),
            )
    }

    fn get_or_update_memoized_library(&self) -> MemoizedLibraryView {
        let rev = self.library_state_rev;
        let status_tab = self.library_status_tab;
        let type_filter = self.library_type_filter;
        let sort_criterion = self.library_sort_criterion;
        let search_query = self.library_search_query.clone();

        {
            let guard = self.memoized_library_items.lock();
            if let Some(cached) = guard.as_ref() {
                if cached.library_rev == rev
                    && cached.status_tab == status_tab
                    && cached.type_filter == type_filter
                    && cached.sort_criterion == sort_criterion
                    && cached.search_query == search_query
                {
                    return cached.clone();
                }
            }
        }

        let query_tokens: Vec<String> = search_query
            .trim()
            .to_ascii_lowercase()
            .split_whitespace()
            .map(str::to_owned)
            .collect();

        let (
            count_watching,
            count_watchlist,
            count_watched,
            films_count,
            shows_count,
            episodes_logged,
            hours_logged,
            mut filtered,
        ) = self.core.with_model(|model| {
            let mut count_watching = 0_usize;
            let mut count_watchlist = 0_usize;
            let mut count_watched = 0_usize;
            let mut films_count = 0_usize;
            let mut shows_count = 0_usize;
            let mut episodes_logged = 0_u64;
            let mut total_watched_ms = 0_u64;
            let mut filtered = Vec::new();

            let mut process_item = |li: &LibraryItem| {
                let Some(status) = classify_library_item_status(li) else {
                    return;
                };
                match status {
                    LibraryStatusTab::Watching => count_watching += 1,
                    LibraryStatusTab::Watchlist => count_watchlist += 1,
                    LibraryStatusTab::Watched => count_watched += 1,
                }
                if li.r#type.eq_ignore_ascii_case("movie") {
                    if status == LibraryStatusTab::Watched {
                        films_count += 1;
                    }
                } else {
                    shows_count += 1;
                    episodes_logged += (li.state.times_watched as u64).max(u64::from(
                        status == LibraryStatusTab::Watching
                            || status == LibraryStatusTab::Watched,
                    ));
                }
                if li.state.overall_time_watched > 0 {
                    total_watched_ms += li.state.overall_time_watched;
                } else if li.state.time_offset > 0 {
                    total_watched_ms += li.state.time_offset;
                }

                if status == status_tab && type_filter.matches_type(&li.r#type) {
                    let matches_query = if query_tokens.is_empty() {
                        true
                    } else {
                        let lower_name = li.name.to_ascii_lowercase();
                        query_tokens.iter().all(|tok| lower_name.contains(tok))
                    };
                    if matches_query {
                        filtered.push((li.clone(), status));
                    }
                }
            };

            if !model.ctx.library.items.is_empty() {
                for li in model.ctx.library.items.values() {
                    process_item(li);
                }
            } else {
                for li in &model.library.catalog {
                    process_item(li);
                }
            }

            (
                count_watching,
                count_watchlist,
                count_watched,
                films_count,
                shows_count,
                episodes_logged,
                total_watched_ms / 3_600_000,
                filtered,
            )
        });

        match sort_criterion {
            LibrarySortCriterion::RecentlyUpdated => {
                filtered.sort_by(|a, b| {
                    b.0.mtime
                        .cmp(&a.0.mtime)
                        .then_with(|| a.0.name.to_lowercase().cmp(&b.0.name.to_lowercase()))
                });
            }
            LibrarySortCriterion::LastWatched => {
                filtered.sort_by(|a, b| {
                    b.0.state
                        .last_watched
                        .cmp(&a.0.state.last_watched)
                        .then_with(|| b.0.mtime.cmp(&a.0.mtime))
                });
            }
            LibrarySortCriterion::DateAdded => {
                filtered.sort_by(|a, b| {
                    b.0.ctime
                        .unwrap_or(b.0.mtime)
                        .cmp(&a.0.ctime.unwrap_or(a.0.mtime))
                        .then_with(|| b.0.mtime.cmp(&a.0.mtime))
                });
            }
            LibrarySortCriterion::TitleAsc => {
                filtered.sort_by(|a, b| {
                    a.0.name
                        .to_lowercase()
                        .cmp(&b.0.name.to_lowercase())
                        .then_with(|| b.0.mtime.cmp(&a.0.mtime))
                });
            }
            LibrarySortCriterion::TimesWatched => {
                filtered.sort_by(|a, b| {
                    b.0.state
                        .times_watched
                        .cmp(&a.0.state.times_watched)
                        .then_with(|| {
                            b.0.state
                                .overall_time_watched
                                .cmp(&a.0.state.overall_time_watched)
                        })
                        .then_with(|| b.0.mtime.cmp(&a.0.mtime))
                });
            }
        }

        let memo = MemoizedLibraryView {
            library_rev: rev,
            status_tab,
            type_filter,
            sort_criterion,
            search_query,
            count_watching,
            count_watchlist,
            count_watched,
            films_count,
            shows_count,
            episodes_logged,
            hours_logged,
            filtered_items: Arc::new(filtered),
        };

        let mut guard = self.memoized_library_items.lock();
        *guard = Some(memo.clone());
        memo
    }

    fn library_notification_counts(&self) -> Arc<HashMap<String, usize>> {
        let counts: HashMap<String, usize> = self.core.with_model(|model| {
            model
                .continue_watching_preview
                .items
                .iter()
                .filter(|item| item.notifications > 0)
                .map(|item| (item.library_item.id.clone(), item.notifications))
                .collect()
        });
        Arc::new(counts)
    }

    #[allow(clippy::too_many_arguments)]
    fn render_library_grid_card(
        &self,
        idx: usize,
        li: &LibraryItem,
        status: LibraryStatusTab,
        notification_count: usize,
        aspect: PosterAspect,
        image_height_px: f32,
        is_focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let poster_tex = self.poster_cache.resolve_poster_url(li.poster.as_ref());
        let type_label = sentence_case_label(&li.r#type);
        let subtitle: SharedString =
            match format_movietracker_episode_code(li.state.video_id.as_deref()) {
                Some(ep) => format!("{type_label} · {ep}").into(),
                None => type_label.into(),
            };

        let progress_fraction = if status == LibraryStatusTab::Watching
            && li.state.duration > 0
            && li.state.time_offset > 0
        {
            Some((li.state.time_offset as f32 / li.state.duration as f32).clamp(0.0, 1.0))
        } else {
            None
        };

        let open_type = li.r#type.clone();
        let open_id = li.id.clone();
        let open_vid = li.state.video_id.clone();

        let play_type = li.r#type.clone();
        let play_id = li.id.clone();
        let play_vid = li.state.video_id.clone();

        let remove_id = li.id.clone();
        let remove_name = li.name.clone();

        let ctx_type = li.r#type.clone();
        let ctx_id = li.id.clone();
        let ctx_name = li.name.clone();
        let ctx_vid = li.state.video_id.clone();
        let ctx_watched = li.watched();
        let ctx_can_rewind = li.state.time_offset > 0;

        let hover_type = li.r#type.clone();
        let hover_id = li.id.clone();

        render_poster_card(
            PosterCardProps::new(("library-card", idx), li.name.clone(), li.id.clone())
                .subtitle(Some(subtitle))
                .image(poster_tex)
                .aspect(aspect)
                .image_height_px(Some(image_height_px))
                .status_tab(Some(status))
                .notification_count(notification_count)
                .progress_fraction(progress_fraction)
                .in_library(true)
                .is_focused(is_focused)
                .on_open(click_handler(cx.listener(move |this, _, _, cx| {
                    this.open_meta_details(
                        open_type.clone(),
                        open_id.clone(),
                        open_vid.clone(),
                        cx,
                    );
                })))
                .on_quick_play(click_handler(cx.listener(move |this, _, _, cx| {
                    this.quick_resume_or_open(
                        play_type.clone(),
                        play_id.clone(),
                        play_vid.clone(),
                        cx,
                    );
                })))
                .on_toggle_library(click_handler(cx.listener(move |this, _, _, cx| {
                    this.core.remove_from_library(remove_id.clone());
                    this.push_toast(
                        ToastKind::Info,
                        "Removed from library",
                        Some(remove_name.clone()),
                        cx,
                    );
                    cx.notify();
                })))
                .on_context_menu(click_handler(cx.listener(
                    move |this, ev: &MouseDownEvent, _, cx| {
                    let menu = BrowsingContextMenu::new(
                        ev.position,
                        ctx_type.clone(),
                        ctx_id.clone(),
                        ctx_name.clone(),
                    )
                    .video_id(ctx_vid.clone())
                    .is_in_library(true)
                    .is_watched(ctx_watched)
                    .can_rewind_cw(ctx_can_rewind);
                    this.open_browsing_context_menu(menu, cx);
                })))
                .on_hover(hover_handler(cx.listener(move |this, hovered, _, cx| {
                    this.on_card_hover_prefetch(&hover_type, &hover_id, None, *hovered, cx);
                }))),
        )
    }
}

fn infer_library_grid_aspect(items: &[(LibraryItem, LibraryStatusTab)]) -> PosterAspect {
    if items.is_empty() {
        return PosterAspect::Portrait2x3;
    }
    let mut has_portrait = false;
    let mut has_square = false;
    let mut has_landscape = false;
    for (li, _) in items.iter().take(24) {
        match li.poster_shape {
            PosterShape::Poster => has_portrait = true,
            PosterShape::Square => has_square = true,
            PosterShape::Landscape => has_landscape = true,
        }
    }
    if has_portrait {
        PosterAspect::Portrait2x3
    } else if has_square && !has_landscape {
        PosterAspect::Square1x1
    } else if has_landscape && !has_square {
        PosterAspect::Landscape16x9
    } else if has_square {
        PosterAspect::Square1x1
    } else {
        PosterAspect::Portrait2x3
    }
}

fn render_library_empty_state(
    active_tab: LibraryStatusTab,
    has_search_query: bool,
    trimmed_query: &str,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let (empty_title, empty_sub): (SharedString, SharedString) = if has_search_query {
        (
            format!("No matches in {}", active_tab.label()).into(),
            format!(
                "No titles in {} matched \"{trimmed_query}\". Try another title or clear your filter.",
                active_tab.label()
            )
            .into(),
        )
    } else {
        match active_tab {
            LibraryStatusTab::Watching => (
                "Nothing in progress".into(),
                "Start any film or episode and it will stay waiting for you right here.".into(),
            ),
            LibraryStatusTab::Watchlist => (
                "Your watchlist is empty".into(),
                "Bookmark films and series across Discover to build your queue.".into(),
            ),
            LibraryStatusTab::Watched => (
                "No finished titles yet".into(),
                "Completed films and series are archived here automatically.".into(),
            ),
        }
    };

    div()
        .py(px(72.0))
        .px(px(24.0))
        .rounded(px(RADIUS_CARD))
        .border_1()
        .border_color(rgba(GLASS_BORDER))
        .bg(rgba(GLASS_CARD_BG))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(12.0))
        .child(
            div()
                .w(px(44.0))
                .h(px(44.0))
                .rounded(px(RADIUS_PILL))
                .bg(rgba(GLASS_TROUGH_BG))
                .border_1()
                .border_color(rgba(GLASS_BORDER))
                .flex()
                .items_center()
                .justify_center()
                .child(svg_icon(
                    if has_search_query {
                        IconName::Search
                    } else {
                        IconName::Bookmark
                    },
                    19.0,
                    rgb(TEXT_MUTED),
                )),
        )
        .child(
            div()
                .font(editorial_serif())
                .text_size(px(30.0))
                .text_color(rgb(TEXT_PRIMARY))
                .child(empty_title),
        )
        .child(
            div()
                .max_w(px(420.0))
                .text_size(px(14.0))
                .text_color(rgb(TEXT_SECONDARY))
                .child(empty_sub),
        )
        .child(if has_search_query {
            div()
                .pt(px(6.0))
                .child(primary_accent_button(
                    "lib-empty-clear-search",
                    Some(IconName::X),
                    "Clear search",
                    cx.listener(|this, _, _, cx| {
                        this.library_search_query.clear();
                        this.input_cursors.remove(&FocusedInput::LibrarySearch);
                        this.input_selected_all.remove(&FocusedInput::LibrarySearch);
                        this.kb_focus_index = None;
                        cx.notify();
                    }),
                ))
                .into_any_element()
        } else {
            div()
                .pt(px(6.0))
                .child(primary_accent_button(
                    "lib-empty-discover",
                    Some(IconName::Compass),
                    "Browse Discover",
                    cx.listener(|this, _, _, cx| {
                        this.navigate_tab(SidebarTab::Discover, cx);
                    }),
                ))
                .into_any_element()
        })
        .into_any_element()
}

fn render_library_stat(value: String, label: &'static str) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(2.0))
        .child(
            div()
                .font(editorial_serif())
                .text_size(px(38.0))
                .text_color(rgb(TEXT_PRIMARY))
                .child(value),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(TEXT_MUTED))
                .child(label),
        )
}
