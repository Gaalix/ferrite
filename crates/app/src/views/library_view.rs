use std::{collections::HashMap, ops::Range, sync::Arc};

use gpui::{
    deferred, div, img, linear_color_stop, linear_gradient, point, prelude::*, px, relative, rgb,
    rgba, AnyElement, Context, FontWeight, MouseButton, MouseDownEvent, ObjectFit, SharedString,
    Window,
};
use stremio_core_runtime::stremio_core::types::{
    library::LibraryItem, resource::PosterShape,
};

use crate::{
    icons::{svg_icon, svg_icon_weighted, IconName, IconWeight},
    shell::AppShell,
    state::{
        classify_library_item_status, find_cached_preview_in_model,
        format_movietracker_episode_code, AppRoute, BrowsingContextMenu, FocusedInput,
        KbFocusableCard, LibraryFilterTab, LibrarySortCriterion, LibrarySortOrder,
        LibraryStatusTab, LibraryTypeFilter, LibraryViewMode, MemoizedLibraryView,
        QuickPeekTarget, SidebarTab, ToastKind,
    },
    theme::{
        active_theme, editorial_serif, mono_font, placeholder_tint_for_id, poster_card_shadow,
        poster_hover_glow_shadow, sentence_case_label, sp, tabular_nums, ui_sans,
        CardDensityChoice, RADIUS_CARD, RADIUS_CONTROL, RADIUS_PILL, RADIUS_POSTER,
    },
    ui_components::{
        clamp_progress_fraction, click_handler, format_notification_badge_label,
        glass_dropdown_option, glass_dropdown_panel, glass_segmented_bar, glass_segmented_item,
        hover_handler, primary_accent_button, render_poster_card, secondary_glass_button,
        PosterCardProps,
    },
    virtual_grid::{compute_dynamic_grid_metrics, slice_row_items, PosterAspect},
};

const UP_NEXT_SHELF_LIMIT: usize = 12;

struct UpNextHighlightEntry {
    item: LibraryItem,
    status: LibraryStatusTab,
    notification_count: usize,
}

impl AppShell {
    pub fn render_library_view(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = active_theme();
        let accent_ink = if theme.is_light {
            theme.accent_press
        } else {
            theme.accent
        };

        let mut memo = self.get_or_update_memoized_library();
        if self.library_filter_tab == LibraryFilterTab::NewEpisodes && memo.count_new_episodes == 0
        {
            self.library_filter_tab = LibraryFilterTab::All;
            memo = self.get_or_update_memoized_library();
        }

        let notif_counts_arc = self.library_notification_counts();
        let active_tab = self.library_filter_tab;
        let active_type = self.library_type_filter;
        let active_sort = self.library_sort_criterion;
        let active_view_mode = self.preferences.library_view_mode;
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
        let h_pad_each = compute_library_horizontal_padding_px(viewport_w);
        let h_pad_total = h_pad_each * 2.0;
        let grid_gap = theme.density.grid_gap_px() * theme.ui_scale;

        let poster_aspect = infer_library_grid_aspect(&filtered_arc);
        let poster_metrics = compute_dynamic_grid_metrics(
            total_filtered,
            viewport_w,
            0.0,
            h_pad_total,
            None,
            theme.density.target_poster_width_px() * theme.ui_scale,
            grid_gap,
            grid_gap,
            54.0 * theme.ui_scale,
            poster_aspect,
        );
        let wide_metrics = compute_dynamic_grid_metrics(
            total_filtered,
            viewport_w,
            0.0,
            h_pad_total,
            None,
            wide_card_min_width_px(theme.density, theme.ui_scale),
            grid_gap,
            grid_gap,
            56.0 * theme.ui_scale,
            PosterAspect::Landscape16x9,
        );

        let kb_columns = match active_view_mode {
            LibraryViewMode::PosterGrid => poster_metrics.columns,
            LibraryViewMode::WideBackdrops => wide_metrics.columns,
            LibraryViewMode::CompactList => 1,
        };
        let kb_items: Vec<KbFocusableCard> = filtered_arc
            .iter()
            .enumerate()
            .map(|(idx, (li, _))| KbFocusableCard {
                route: AppRoute::MetaDetails {
                    type_name: li.r#type.clone(),
                    id: li.id.clone(),
                    video_id: li.state.video_id.clone(),
                },
                row: idx / kb_columns,
                col: idx % kb_columns,
            })
            .collect();
        self.set_kb_focus_items(kb_items);

        let up_next_items = if !search_trimmed_non_empty
            && matches!(
                active_tab,
                LibraryFilterTab::All | LibraryFilterTab::Watching | LibraryFilterTab::NewEpisodes
            ) {
            self.collect_library_up_next_items(&notif_counts_arc)
        } else {
            Vec::new()
        };
        let show_up_next_shelf = !up_next_items.is_empty();

        let visible_filter_tabs: Vec<LibraryFilterTab> = LibraryFilterTab::ALL
            .into_iter()
            .filter(|tab| *tab != LibraryFilterTab::NewEpisodes || memo.count_new_episodes > 0)
            .collect();

        let content_area: AnyElement = if total_filtered == 0 {
            render_library_empty_state(
                active_tab,
                active_type,
                memo.count_all,
                search_trimmed_non_empty,
                self.library_search_query.trim(),
                cx,
            )
        } else {
            match active_view_mode {
                LibraryViewMode::PosterGrid => {
                    let columns = poster_metrics.columns;
                    let row_count = poster_metrics.layout.row_count;
                    let row_stride_px = poster_metrics.row_stride_px;
                    let card_width_px = poster_metrics.card_width_px;
                    let image_height_px = poster_metrics.image_height_px;
                    let items_for_grid = Arc::clone(&filtered_arc);
                    let notifs_for_grid = Arc::clone(&notif_counts_arc);

                    gpui::uniform_list(
                        "library-poster-grid",
                        row_count,
                        cx.processor(move |this, visible_rows: Range<usize>, _window, cx| {
                            let kb_focus_index = this.kb_focus_index;

                            visible_rows
                                .map(|row_idx| {
                                    let range = slice_row_items(total_filtered, columns, row_idx);
                                    let empty_slots = columns.saturating_sub(range.len());

                                    div()
                                        .id(("library-poster-row", row_idx))
                                        .w_full()
                                        .h(px(row_stride_px))
                                        .pb(px(grid_gap))
                                        .flex()
                                        .items_start()
                                        .gap(px(grid_gap))
                                        .children(range.map(|idx| {
                                            let (li, st) = &items_for_grid[idx];
                                            let notif_count =
                                                notifs_for_grid.get(&li.id).copied().unwrap_or(0);
                                            let is_focused = kb_focus_index == Some(idx);
                                            let card_aspect =
                                                PosterAspect::from_stremio_poster_shape(
                                                    &li.poster_shape,
                                                );
                                            let card_image_h = if card_aspect == poster_aspect {
                                                image_height_px
                                            } else {
                                                (card_width_px * card_aspect.height_over_width())
                                                    .ceil()
                                                    .min(image_height_px)
                                            };

                                            div()
                                                .flex_1()
                                                .min_w(px(0.0))
                                                .h_full()
                                                .child(this.render_library_poster_card(
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
                }
                LibraryViewMode::WideBackdrops => {
                    let columns = wide_metrics.columns;
                    let row_count = wide_metrics.layout.row_count;
                    let row_stride_px = wide_metrics.row_stride_px;
                    let image_height_px = wide_metrics.image_height_px;
                    let items_for_wide = Arc::clone(&filtered_arc);
                    let notifs_for_wide = Arc::clone(&notif_counts_arc);

                    gpui::uniform_list(
                        "library-wide-grid",
                        row_count,
                        cx.processor(move |this, visible_rows: Range<usize>, _window, cx| {
                            let kb_focus_index = this.kb_focus_index;

                            visible_rows
                                .map(|row_idx| {
                                    let range = slice_row_items(total_filtered, columns, row_idx);
                                    let empty_slots = columns.saturating_sub(range.len());

                                    div()
                                        .id(("library-wide-row", row_idx))
                                        .w_full()
                                        .h(px(row_stride_px))
                                        .pb(px(grid_gap))
                                        .flex()
                                        .items_start()
                                        .gap(px(grid_gap))
                                        .children(range.map(|idx| {
                                            let (li, st) = &items_for_wide[idx];
                                            let notif_count =
                                                notifs_for_wide.get(&li.id).copied().unwrap_or(0);
                                            let is_focused = kb_focus_index == Some(idx);

                                            div()
                                                .flex_1()
                                                .min_w(px(0.0))
                                                .h_full()
                                                .child(this.render_library_wide_card(
                                                    idx,
                                                    li,
                                                    *st,
                                                    notif_count,
                                                    image_height_px,
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
                }
                LibraryViewMode::CompactList => {
                    let row_h_px = (64.0 * theme.ui_scale).ceil();
                    let row_gap_px = (6.0 * theme.ui_scale).ceil();
                    let row_stride_px = row_h_px + row_gap_px;
                    let items_for_list = Arc::clone(&filtered_arc);
                    let notifs_for_list = Arc::clone(&notif_counts_arc);

                    gpui::uniform_list(
                        "library-compact-list",
                        total_filtered,
                        cx.processor(move |this, visible_rows: Range<usize>, _window, cx| {
                            let kb_focus_index = this.kb_focus_index;

                            visible_rows
                                .map(|idx| {
                                    let (li, st) = &items_for_list[idx];
                                    let notif_count =
                                        notifs_for_list.get(&li.id).copied().unwrap_or(0);
                                    let is_focused = kb_focus_index == Some(idx);

                                    div()
                                        .id(("library-list-wrap", idx))
                                        .w_full()
                                        .h(px(row_stride_px))
                                        .pb(px(row_gap_px))
                                        .child(this.render_library_compact_row(
                                            idx,
                                            li,
                                            *st,
                                            notif_count,
                                            row_h_px,
                                            is_focused,
                                            cx,
                                        ))
                                })
                                .collect()
                        }),
                    )
                    .track_scroll(&self.library_grid_scroll_handle)
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .into_any_element()
                }
            }
        };

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
                    .px(px(h_pad_each))
                    .pt(sp(12.0))
                    .pb(sp(12.0))
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(sp(14.0))
                    // 1. Compact Cinematic Header with frosted glass stat pills.
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap(sp(14.0))
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap(sp(12.0))
                                    .child(
                                        div()
                                            .font(editorial_serif())
                                            .text_size(sp(34.0))
                                            .line_height(sp(38.0))
                                            .text_color(rgb(theme.text_primary))
                                            .child("Library"),
                                    )
                                    .when(memo.count_all > 0, |row| {
                                        row.child(
                                            div()
                                                .px(sp(9.0))
                                                .py(sp(3.0))
                                                .rounded(px(RADIUS_PILL))
                                                .bg(rgba(theme.glass_trough_bg))
                                                .border_1()
                                                .border_color(rgba(theme.glass_border))
                                                .font(mono_font())
                                                .font_features(tabular_nums())
                                                .text_size(sp(11.5))
                                                .text_color(rgb(theme.text_muted))
                                                .child(format!(
                                                    "{} {}",
                                                    memo.count_all,
                                                    if memo.count_all == 1 {
                                                        "title"
                                                    } else {
                                                        "titles"
                                                    }
                                                )),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(sp(8.0))
                                    .child(render_library_stat_pill(
                                        "lib-stat-watching",
                                        IconName::Play,
                                        IconWeight::Fill,
                                        theme.status_watching,
                                        memo.count_watching.to_string(),
                                        "Watching",
                                        active_tab == LibraryFilterTab::Watching,
                                        Some(click_handler(cx.listener(|this, _, _, cx| {
                                            this.select_library_filter_tab(
                                                LibraryFilterTab::Watching,
                                                cx,
                                            );
                                        }))),
                                    ))
                                    .child(render_library_stat_pill(
                                        "lib-stat-saved",
                                        IconName::Bookmark,
                                        if active_tab == LibraryFilterTab::Watchlist {
                                            IconWeight::Fill
                                        } else {
                                            IconWeight::Regular
                                        },
                                        theme.status_watchlist,
                                        memo.count_watchlist.to_string(),
                                        "Saved",
                                        active_tab == LibraryFilterTab::Watchlist,
                                        Some(click_handler(cx.listener(|this, _, _, cx| {
                                            this.select_library_filter_tab(
                                                LibraryFilterTab::Watchlist,
                                                cx,
                                            );
                                        }))),
                                    ))
                                    .child(render_library_stat_pill(
                                        "lib-stat-watched",
                                        IconName::Check,
                                        IconWeight::Regular,
                                        theme.status_watched,
                                        memo.count_watched.to_string(),
                                        "Watched",
                                        active_tab == LibraryFilterTab::Watched,
                                        Some(click_handler(cx.listener(|this, _, _, cx| {
                                            this.select_library_filter_tab(
                                                LibraryFilterTab::Watched,
                                                cx,
                                            );
                                        }))),
                                    ))
                                    .child(render_library_stat_pill(
                                        "lib-stat-hours",
                                        IconName::Clock,
                                        IconWeight::Regular,
                                        theme.accent_amber,
                                        memo.hours_logged.to_string(),
                                        "Hours",
                                        false,
                                        None,
                                    )),
                            ),
                    )
                    // 2. "New Episodes & Up Next" Highlight Shelf.
                    .when(show_up_next_shelf, |col| {
                        col.child(self.render_library_up_next_shelf(
                            &up_next_items,
                            memo.count_new_episodes,
                            active_tab,
                            cx,
                        ))
                    })
                    // 3. Unified Sticky Frosted Glass Toolbar.
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(sp(12.0))
                            .py(sp(9.0))
                            .rounded(px(RADIUS_CARD))
                            .bg(rgba(theme.glass_card_bg))
                            .border_1()
                            .border_color(rgba(theme.glass_border))
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap(sp(10.0))
                            .child(
                                glass_segmented_bar("lib-filter-bar").children(
                                    visible_filter_tabs.into_iter().enumerate().map(
                                        |(idx, tab)| {
                                            let is_active = tab == active_tab;
                                            let count = match tab {
                                                LibraryFilterTab::All => memo.count_all,
                                                LibraryFilterTab::Watching => memo.count_watching,
                                                LibraryFilterTab::NewEpisodes => {
                                                    memo.count_new_episodes
                                                }
                                                LibraryFilterTab::Watchlist => memo.count_watchlist,
                                                LibraryFilterTab::Watched => memo.count_watched,
                                            };
                                            let label = format!("{} ({count})", tab.label());
                                            glass_segmented_item(
                                                ("lib-filter-tab", idx),
                                                label,
                                                is_active,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.select_library_filter_tab(tab, cx);
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
                                    .gap(sp(8.0))
                                    // Inline instant filter input.
                                    .child(
                                        div()
                                            .id("lib-search-box")
                                            .cursor_pointer()
                                            .w(sp(216.0))
                                            .h(sp(36.0))
                                            .px(sp(11.0))
                                            .rounded(px(RADIUS_CONTROL))
                                            .bg(if search_focused {
                                                rgba(theme.glass_row_selected_bg)
                                            } else {
                                                rgba(theme.glass_trough_bg)
                                            })
                                            .border_1()
                                            .border_color(if search_focused {
                                                rgba(theme.border_accent)
                                            } else {
                                                rgba(theme.glass_border)
                                            })
                                            .hover(move |s| {
                                                s.border_color(if search_focused {
                                                    rgba(theme.border_accent)
                                                } else {
                                                    rgba(theme.glass_border_strong)
                                                })
                                            })
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap(sp(8.0))
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
                                                    .gap(sp(7.0))
                                                    .child(svg_icon(
                                                        IconName::Search,
                                                        13.5 * theme.ui_scale,
                                                        if search_focused {
                                                            rgb(accent_ink)
                                                        } else {
                                                            rgb(theme.text_muted)
                                                        },
                                                    ))
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w(px(0.0))
                                                            .flex()
                                                            .items_center()
                                                            .font(ui_sans(FontWeight::NORMAL))
                                                            .text_size(sp(12.5))
                                                            .text_color(if has_search_query {
                                                                rgb(theme.text_primary)
                                                            } else {
                                                                rgb(theme.text_muted)
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
                                                                        .text_color(rgb(
                                                                            theme.text_muted,
                                                                        ))
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
                                                        .hover(move |s| {
                                                            s.bg(rgba(theme.glass_row_hover_bg))
                                                        })
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(|this, _, _, cx| {
                                                                cx.stop_propagation();
                                                                this.clear_library_search(cx);
                                                            }),
                                                        )
                                                        .child(svg_icon(
                                                            IconName::X,
                                                            11.5 * theme.ui_scale,
                                                            rgb(theme.text_muted),
                                                        )),
                                                )
                                            }),
                                    )
                                    // Type filter pills (All | Movies | Series).
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
                                                            this.reset_library_scroll();
                                                            cx.notify();
                                                        }),
                                                    )
                                                },
                                            ),
                                        ),
                                    )
                                    // Sort dropdown.
                                    .child(
                                        div()
                                            .relative()
                                            .child(
                                                div()
                                                    .id("lib-sort-pill")
                                                    .cursor_pointer()
                                                    .h(sp(36.0))
                                                    .px(sp(11.0))
                                                    .rounded(px(RADIUS_CONTROL))
                                                    .bg(if sort_open {
                                                        rgba(theme.glass_row_selected_bg)
                                                    } else {
                                                        rgba(theme.glass_trough_bg)
                                                    })
                                                    .border_1()
                                                    .border_color(if sort_open {
                                                        rgba(theme.border_accent)
                                                    } else {
                                                        rgba(theme.glass_border)
                                                    })
                                                    .hover(move |s| {
                                                        s.bg(if sort_open {
                                                            rgba(theme.glass_row_selected_bg)
                                                        } else {
                                                            rgba(theme.glass_row_hover_bg)
                                                        })
                                                        .border_color(if sort_open {
                                                            rgb(theme.accent)
                                                        } else {
                                                            rgba(theme.glass_border_strong)
                                                        })
                                                    })
                                                    .flex()
                                                    .items_center()
                                                    .gap(sp(7.0))
                                                    .font(ui_sans(FontWeight::MEDIUM))
                                                    .text_size(sp(12.5))
                                                    .text_color(if sort_open {
                                                        rgb(accent_ink)
                                                    } else {
                                                        rgb(theme.text_primary)
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
                                                        13.0 * theme.ui_scale,
                                                        if sort_open {
                                                            rgb(accent_ink)
                                                        } else {
                                                            rgb(theme.text_muted)
                                                        },
                                                    ))
                                                    .child(active_sort.label())
                                                    .child(svg_icon(
                                                        IconName::ChevronDown,
                                                        12.0 * theme.ui_scale,
                                                        if sort_open {
                                                            rgb(accent_ink)
                                                        } else {
                                                            rgb(theme.text_muted)
                                                        },
                                                    )),
                                            )
                                            .when(sort_open, |wrap| {
                                                wrap.child(
                                                    deferred(
                                                        glass_dropdown_panel("lib-sort-menu", 216.0)
                                                            .absolute()
                                                            .top(sp(42.0))
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
                                                                                    this.reset_library_scroll();
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
                                    )
                                    // 3 Persisted View Mode Switcher (PosterGrid | WideBackdrops | CompactList).
                                    .child(
                                        glass_segmented_bar("lib-view-mode-bar").children(
                                            LibraryViewMode::ALL.into_iter().enumerate().map(
                                                |(idx, mode)| {
                                                    let is_active = mode == active_view_mode;
                                                    let icon = match mode {
                                                        LibraryViewMode::PosterGrid => {
                                                            IconName::GridFour
                                                        }
                                                        LibraryViewMode::WideBackdrops => {
                                                            IconName::Rows
                                                        }
                                                        LibraryViewMode::CompactList => {
                                                            IconName::List
                                                        }
                                                    };
                                                    let weight = if is_active {
                                                        IconWeight::Fill
                                                    } else {
                                                        IconWeight::Regular
                                                    };
                                                    let icon_fg = if is_active {
                                                        theme.segment_active_text
                                                    } else {
                                                        theme.text_secondary
                                                    };
                                                    div()
                                                        .id(("lib-view-mode-btn", idx))
                                                        .w(sp(30.0))
                                                        .h(sp(30.0))
                                                        .rounded(px(RADIUS_POSTER))
                                                        .cursor_pointer()
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .when(is_active, |b| {
                                                            b.bg(rgb(theme.segment_active_bg))
                                                        })
                                                        .when(!is_active, |b| {
                                                            b.hover(move |s| {
                                                                s.bg(rgba(theme.glass_row_hover_bg))
                                                            })
                                                        })
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(move |this, _, _, cx| {
                                                                cx.stop_propagation();
                                                                if this.preferences.library_view_mode
                                                                    != mode
                                                                {
                                                                    this.preferences
                                                                        .library_view_mode = mode;
                                                                    let _ = this.preferences.save();
                                                                    this.library_sort_dropdown_open =
                                                                        false;
                                                                    this.kb_focus_index = None;
                                                                    this.reset_library_scroll();
                                                                    cx.notify();
                                                                }
                                                            }),
                                                        )
                                                        .child(svg_icon_weighted(
                                                            icon,
                                                            weight,
                                                            14.5 * theme.ui_scale,
                                                            rgb(icon_fg),
                                                        ))
                                                },
                                            ),
                                        ),
                                    ),
                            ),
                    )
                    // 4. Virtualized Content Surface (PosterGrid | WideBackdrops | CompactList).
                    .child(content_area),
            )
    }

    fn select_library_filter_tab(&mut self, tab: LibraryFilterTab, cx: &mut Context<Self>) {
        self.library_filter_tab = tab;
        match tab {
            LibraryFilterTab::Watching => self.library_status_tab = LibraryStatusTab::Watching,
            LibraryFilterTab::Watchlist => self.library_status_tab = LibraryStatusTab::Watchlist,
            LibraryFilterTab::Watched => self.library_status_tab = LibraryStatusTab::Watched,
            LibraryFilterTab::All | LibraryFilterTab::NewEpisodes => {}
        }
        self.library_sort_dropdown_open = false;
        self.kb_focus_index = None;
        self.reset_library_scroll();
        cx.notify();
    }

    fn clear_library_search(&mut self, cx: &mut Context<Self>) {
        self.library_search_query.clear();
        self.input_cursors.remove(&FocusedInput::LibrarySearch);
        self.input_selected_all.remove(&FocusedInput::LibrarySearch);
        self.kb_focus_index = None;
        self.reset_library_scroll();
        cx.notify();
    }

    fn reset_library_scroll(&self) {
        self.library_scroll_handle
            .set_offset(point(px(0.0), px(0.0)));
        self.library_grid_scroll_handle
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(0.0)));
    }

    fn resolve_library_widescreen_url(&self, li: &LibraryItem) -> Option<String> {
        if let Some(bg) = self
            .core
            .cached_enriched_meta(&li.id)
            .and_then(|e| e.backdrop_url)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
        {
            return Some(bg);
        }
        if li.id.starts_with("tt") {
            return Some(format!(
                "https://images.metahub.space/background/medium/{}/img",
                li.id
            ));
        }
        self.core.with_model(|m| {
            find_cached_preview_in_model(m, &li.id)
                .and_then(|p| p.background.map(|u| u.to_string()))
                .filter(|s| !s.trim().is_empty())
        })
    }

    fn collect_library_up_next_items(
        &self,
        notif_counts: &HashMap<String, usize>,
    ) -> Vec<UpNextHighlightEntry> {
        let mut entries: Vec<UpNextHighlightEntry> = self.core.with_model(|model| {
            let mut out = Vec::new();
            let mut visit = |li: &LibraryItem| {
                let Some(status) = classify_library_item_status(li) else {
                    return;
                };
                let notif = notif_counts.get(&li.id).copied().unwrap_or(0);
                let is_active_up_next = notif > 0
                    || (status == LibraryStatusTab::Watching
                        && (li.state.time_offset > 0
                            || (!li.r#type.eq_ignore_ascii_case("movie")
                                && li.state.last_watched.is_some())));
                if is_active_up_next {
                    out.push(UpNextHighlightEntry {
                        item: li.clone(),
                        status,
                        notification_count: notif,
                    });
                }
            };

            if !model.ctx.library.items.is_empty() {
                for li in model.ctx.library.items.values() {
                    visit(li);
                }
            } else {
                for li in &model.library.catalog {
                    visit(li);
                }
            }
            out
        });

        entries.sort_by(|a, b| {
            (b.notification_count > 0)
                .cmp(&(a.notification_count > 0))
                .then_with(|| b.notification_count.cmp(&a.notification_count))
                .then_with(|| b.item.state.last_watched.cmp(&a.item.state.last_watched))
                .then_with(|| b.item.mtime.cmp(&a.item.mtime))
                .then_with(|| {
                    a.item
                        .name
                        .to_ascii_lowercase()
                        .cmp(&b.item.name.to_ascii_lowercase())
                })
        });
        entries.truncate(UP_NEXT_SHELF_LIMIT);
        entries
    }

    fn render_library_up_next_shelf(
        &self,
        entries: &[UpNextHighlightEntry],
        new_episodes_count: usize,
        active_tab: LibraryFilterTab,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = active_theme();
        let accent_ink = if theme.is_light {
            theme.accent_press
        } else {
            theme.accent
        };
        let heading = if new_episodes_count > 0 {
            "New Episodes & Up Next"
        } else {
            "Up Next"
        };
        let heading_icon = if new_episodes_count > 0 {
            IconName::Sparkles
        } else {
            IconName::Play
        };

        div()
            .id("library-up-next-shelf")
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(sp(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sp(12.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(8.0))
                            .child(svg_icon(
                                heading_icon,
                                13.5 * theme.ui_scale,
                                rgb(accent_ink),
                            ))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(sp(13.0))
                                    .text_color(rgb(theme.text_primary))
                                    .child(heading),
                            )
                            .when(new_episodes_count > 0, |row| {
                                row.child(
                                    div()
                                        .px(sp(7.0))
                                        .py(sp(2.0))
                                        .rounded(px(RADIUS_PILL))
                                        .bg(rgba(theme.accent_soft))
                                        .border_1()
                                        .border_color(rgba(theme.border_accent))
                                        .font(mono_font())
                                        .font_features(tabular_nums())
                                        .text_size(sp(10.5))
                                        .text_color(rgb(accent_ink))
                                        .child(format!("{new_episodes_count} new")),
                                )
                            }),
                    )
                    .when(
                        new_episodes_count > 0 && active_tab != LibraryFilterTab::NewEpisodes,
                        |row| {
                            row.child(
                                div()
                                    .id("lib-up-next-view-new")
                                    .cursor_pointer()
                                    .px(sp(9.0))
                                    .py(sp(3.0))
                                    .rounded(px(RADIUS_PILL))
                                    .bg(rgba(theme.glass_trough_bg))
                                    .border_1()
                                    .border_color(rgba(theme.glass_border))
                                    .hover(move |s| {
                                        s.bg(rgba(theme.glass_row_hover_bg))
                                            .border_color(rgba(theme.glass_border_strong))
                                    })
                                    .flex()
                                    .items_center()
                                    .gap(sp(4.0))
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(sp(11.5))
                                    .text_color(rgb(theme.text_secondary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.select_library_filter_tab(
                                                LibraryFilterTab::NewEpisodes,
                                                cx,
                                            );
                                        }),
                                    )
                                    .child("Filter new episodes")
                                    .child(svg_icon(
                                        IconName::ChevronRight,
                                        11.0 * theme.ui_scale,
                                        rgb(theme.text_muted),
                                    )),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .id("library-up-next-strip")
                    .w_full()
                    .overflow_x_scroll()
                    .pb(sp(2.0))
                    .flex()
                    .items_stretch()
                    .gap(sp(12.0))
                    .children(entries.iter().enumerate().map(|(idx, entry)| {
                        self.render_library_up_next_card(idx, entry, cx)
                    })),
            )
            .into_any_element()
    }

    fn render_library_up_next_card(
        &self,
        idx: usize,
        entry: &UpNextHighlightEntry,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = active_theme();
        let li = &entry.item;
        let notif_count = entry.notification_count;
        let wide_url = self.resolve_library_widescreen_url(li);
        let wide_tex = self.poster_cache.resolve_widescreen_or_poster(
            wide_url.as_deref(),
            li.poster.as_ref().map(|u| u.as_str()),
        );
        let progress = clamp_progress_fraction(library_item_progress_fraction(li, entry.status));
        let notif_label = format_notification_badge_label(notif_count);
        let ep_code = format_movietracker_episode_code(li.state.video_id.as_deref());
        let remaining = format_library_remaining_time(li);
        let detail_line: SharedString = match (ep_code.as_deref(), remaining.as_deref()) {
            (Some(ep), Some(rem)) => format!("{ep} · {rem}").into(),
            (Some(ep), None) if notif_count > 0 => format!("{ep} · New episode").into(),
            (Some(ep), None) => ep.to_owned().into(),
            (None, Some(rem)) => rem.to_owned().into(),
            (None, None) if notif_count > 0 => "New episode available".into(),
            (None, None) => sentence_case_label(&li.r#type).into(),
        };

        let grp: SharedString = format!("lib-up-next-grp-{idx}").into();
        let hover_glow = poster_hover_glow_shadow(&theme);
        let scrim_top_transparent = theme.scrim_bottom & 0xFFFFFF00;
        let top_hairline_rgba = if theme.is_light {
            0xFFFFFF80
        } else {
            0xFFFFFF24
        };

        let open_type = li.r#type.clone();
        let open_id = li.id.clone();
        let open_vid = li.state.video_id.clone();

        let play_type = li.r#type.clone();
        let play_id = li.id.clone();
        let play_vid = li.state.video_id.clone();

        let peek_target = QuickPeekTarget {
            type_name: li.r#type.clone(),
            id: li.id.clone(),
            video_id: li.state.video_id.clone(),
            title: li.name.clone(),
            preview: None,
        };

        let ctx_type = li.r#type.clone();
        let ctx_id = li.id.clone();
        let ctx_name = li.name.clone();
        let ctx_vid = li.state.video_id.clone();
        let ctx_watched = li.watched();
        let ctx_can_rewind = li.state.time_offset > 0;

        let hover_type = li.r#type.clone();
        let hover_id = li.id.clone();
        let hover_bg = wide_url;

        div()
            .id(("lib-up-next-card", idx))
            .group(grp.clone())
            .flex_shrink_0()
            .w(sp(268.0))
            .h(sp(150.0))
            .rounded(px(RADIUS_CARD))
            .overflow_hidden()
            .relative()
            .cursor_pointer()
            .bg(rgb(placeholder_tint_for_id(&li.id)))
            .border_1()
            .border_color(rgba(theme.glass_border))
            .shadow(poster_card_shadow())
            .group_hover(grp.clone(), move |s| {
                s.border_color(rgba(theme.border_poster_hover))
                    .shadow(hover_glow)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.open_meta_details(
                        open_type.clone(),
                        open_id.clone(),
                        open_vid.clone(),
                        cx,
                    );
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
                        ctx_name.clone(),
                    )
                    .video_id(ctx_vid.clone())
                    .is_in_library(true)
                    .is_watched(ctx_watched)
                    .can_rewind_cw(ctx_can_rewind);
                    this.open_browsing_context_menu(menu, cx);
                }),
            )
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                this.on_card_hover_prefetch(
                    &hover_type,
                    &hover_id,
                    hover_bg.as_deref(),
                    *hovered,
                    cx,
                );
            }))
            // Widescreen backdrop artwork.
            .child(if let Some(tex) = wide_tex {
                img(tex)
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .into_any_element()
            } else {
                div()
                    .size_full()
                    .bg(rgb(theme.bg_surface))
                    .into_any_element()
            })
            // Luminous top glass hairline.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(1.0))
                    .bg(rgba(top_hairline_rgba)),
            )
            // Bottom cinema scrim for always-legible metadata & controls.
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(linear_gradient(
                        180.0,
                        linear_color_stop(rgba(scrim_top_transparent), 0.18),
                        linear_color_stop(rgba(theme.scrim_bottom), 1.0),
                    )),
            )
            // Top-left New Episodes badge.
            .when_some(notif_label, |el, label| {
                el.child(
                    div()
                        .absolute()
                        .top(sp(8.0))
                        .left(sp(8.0))
                        .h(sp(20.0))
                        .px(sp(7.0))
                        .rounded(px(RADIUS_PILL))
                        .bg(rgb(theme.accent))
                        .flex()
                        .items_center()
                        .justify_center()
                        .font(mono_font())
                        .font_weight(FontWeight::BOLD)
                        .text_size(sp(10.0))
                        .text_color(rgb(theme.cta_on_accent))
                        .child(label),
                )
            })
            // Top-right Quick Peek button.
            .child(
                div()
                    .id(("lib-up-next-peek", idx))
                    .cursor_pointer()
                    .absolute()
                    .top(sp(8.0))
                    .right(sp(8.0))
                    .w(sp(28.0))
                    .h(sp(28.0))
                    .rounded(px(RADIUS_PILL))
                    .bg(rgba(0x0B0D14CC))
                    .border_1()
                    .border_color(rgba(0xFFFFFF2E))
                    .opacity(0.85)
                    .hover(move |s| {
                        s.opacity(1.0)
                            .bg(rgb(theme.accent_press))
                            .border_color(rgb(theme.accent))
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.open_quick_peek(peek_target.clone(), cx);
                        }),
                    )
                    .child(svg_icon(
                        IconName::Eye,
                        13.0 * theme.ui_scale,
                        rgb(0xF5F1E8),
                    )),
            )
            // Bottom overlay: Title + Episode/Remaining + 1-click Resume CTA.
            .child(
                div()
                    .absolute()
                    .bottom(sp(10.0))
                    .left(sp(10.0))
                    .right(sp(10.0))
                    .flex()
                    .items_end()
                    .justify_between()
                    .gap(sp(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(sp(2.0))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(sp(13.0))
                                    .line_height(sp(16.0))
                                    .text_color(rgb(0xF5F1E8))
                                    .truncate()
                                    .child(li.name.clone()),
                            )
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .font_features(tabular_nums())
                                    .text_size(sp(11.5))
                                    .line_height(sp(14.0))
                                    .text_color(rgb(0xC8C8D2))
                                    .truncate()
                                    .child(detail_line),
                            ),
                    )
                    .child(
                        div()
                            .id(("lib-up-next-resume", idx))
                            .cursor_pointer()
                            .flex_shrink_0()
                            .h(sp(30.0))
                            .px(sp(10.0))
                            .rounded(px(RADIUS_PILL))
                            .bg(rgb(theme.accent))
                            .hover(move |s| s.bg(rgb(theme.accent_hover)))
                            .flex()
                            .items_center()
                            .gap(sp(5.0))
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_size(sp(11.5))
                            .text_color(rgb(theme.cta_on_accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.quick_resume_or_open(
                                        play_type.clone(),
                                        play_id.clone(),
                                        play_vid.clone(),
                                        cx,
                                    );
                                }),
                            )
                            .child(svg_icon_weighted(
                                IconName::Play,
                                IconWeight::Fill,
                                11.0 * theme.ui_scale,
                                rgb(theme.cta_on_accent),
                            ))
                            .child("Resume"),
                    ),
            )
            // Bottom edge progress bar.
            .when_some(progress, |el, frac| {
                el.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(px(3.0))
                        .bg(rgba(0xFFFFFF33))
                        .child(div().h_full().w(relative(frac)).bg(rgb(theme.accent))),
                )
            })
            .into_any_element()
    }

    fn get_or_update_memoized_library(&self) -> MemoizedLibraryView {
        let library_rev = self.library_state_rev;
        let notification_rev = self.board_state_rev;
        let filter_tab = self.library_filter_tab;
        let type_filter = self.library_type_filter;
        let sort_criterion = self.library_sort_criterion;
        let search_query = self.library_search_query.clone();

        {
            let guard = self.memoized_library_items.lock();
            if let Some(cached) = guard.as_ref() {
                if cached.library_rev == library_rev
                    && cached.notification_rev == notification_rev
                    && cached.filter_tab == filter_tab
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
            count_all,
            count_watching,
            count_new_episodes,
            count_watchlist,
            count_watched,
            films_count,
            shows_count,
            episodes_logged,
            hours_logged,
            mut filtered,
        ) = self.core.with_model(|model| {
            let notif_counts: HashMap<&str, usize> = model
                .continue_watching_preview
                .items
                .iter()
                .filter(|item| item.notifications > 0)
                .map(|item| (item.library_item.id.as_str(), item.notifications))
                .collect();

            let mut count_all = 0_usize;
            let mut count_watching = 0_usize;
            let mut count_new_episodes = 0_usize;
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
                let notif_count = notif_counts.get(li.id.as_str()).copied().unwrap_or(0);

                count_all += 1;
                match status {
                    LibraryStatusTab::Watching => count_watching += 1,
                    LibraryStatusTab::Watchlist => count_watchlist += 1,
                    LibraryStatusTab::Watched => count_watched += 1,
                }
                if notif_count > 0 {
                    count_new_episodes += 1;
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

                if filter_tab.matches(status, notif_count)
                    && type_filter.matches_type(&li.r#type)
                {
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
                count_all,
                count_watching,
                count_new_episodes,
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
            library_rev,
            notification_rev,
            filter_tab,
            type_filter,
            sort_criterion,
            search_query,
            count_all,
            count_watching,
            count_new_episodes,
            count_watchlist,
            count_watched,
            has_notifications: count_new_episodes > 0,
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

    fn build_card_toggle_watched_button(
        &self,
        id_prefix: &'static str,
        idx: usize,
        li: &LibraryItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = active_theme();
        let is_watched = li.watched();
        let watch_id = li.id.clone();
        let watch_name = li.name.clone();
        let badge_fg = if theme.is_light {
            0xFFFFFF
        } else {
            theme.bg_canvas
        };

        div()
            .id((id_prefix, idx))
            .cursor_pointer()
            .w(px(30.0))
            .h(px(30.0))
            .rounded(px(RADIUS_PILL))
            .bg(if is_watched {
                rgb(theme.status_watched)
            } else {
                rgba(0x0B0D14D9)
            })
            .border_1()
            .border_color(if is_watched {
                rgb(theme.status_watched)
            } else {
                rgba(0xFFFFFF33)
            })
            .hover(move |s| {
                s.bg(rgb(theme.accent_press))
                    .border_color(rgb(theme.accent))
            })
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let next_watched = !is_watched;
                    this.core
                        .mark_library_item_watched(watch_id.clone(), next_watched);
                    this.push_toast(
                        ToastKind::Info,
                        if next_watched {
                            "Marked as watched"
                        } else {
                            "Marked as unwatched"
                        },
                        Some(watch_name.clone()),
                        cx,
                    );
                    cx.notify();
                }),
            )
            .child(svg_icon(
                IconName::Check,
                13.0,
                if is_watched {
                    rgb(badge_fg)
                } else {
                    rgb(0xF5F1E8)
                },
            ))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_library_poster_card(
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
        let progress_fraction = library_item_progress_fraction(li, status);

        let open_type = li.r#type.clone();
        let open_id = li.id.clone();
        let open_vid = li.state.video_id.clone();

        let play_type = li.r#type.clone();
        let play_id = li.id.clone();
        let play_vid = li.state.video_id.clone();

        let remove_id = li.id.clone();
        let remove_name = li.name.clone();

        let peek_target = QuickPeekTarget {
            type_name: li.r#type.clone(),
            id: li.id.clone(),
            video_id: li.state.video_id.clone(),
            title: li.name.clone(),
            preview: None,
        };

        let ctx_type = li.r#type.clone();
        let ctx_id = li.id.clone();
        let ctx_name = li.name.clone();
        let ctx_vid = li.state.video_id.clone();
        let ctx_watched = li.watched();
        let ctx_can_rewind = li.state.time_offset > 0;

        let hover_type = li.r#type.clone();
        let hover_id = li.id.clone();

        let toggle_watched_btn =
            self.build_card_toggle_watched_button("lib-poster-watch", idx, li, cx);

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
                .on_quick_peek(click_handler(cx.listener(move |this, _, _, cx| {
                    this.open_quick_peek(peek_target.clone(), cx);
                })))
                .quick_action(Some(toggle_watched_btn))
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
                    },
                )))
                .on_hover(hover_handler(cx.listener(move |this, hovered, _, cx| {
                    this.on_card_hover_prefetch(&hover_type, &hover_id, None, *hovered, cx);
                }))),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_library_wide_card(
        &self,
        idx: usize,
        li: &LibraryItem,
        status: LibraryStatusTab,
        notification_count: usize,
        image_height_px: f32,
        is_focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wide_url = self.resolve_library_widescreen_url(li);
        let wide_tex = self.poster_cache.resolve_widescreen_or_poster(
            wide_url.as_deref(),
            li.poster.as_ref().map(|u| u.as_str()),
        );
        let subtitle = format_wide_card_subtitle(li, status, notification_count);
        let progress_fraction = library_item_progress_fraction(li, status);

        let open_type = li.r#type.clone();
        let open_id = li.id.clone();
        let open_vid = li.state.video_id.clone();

        let play_type = li.r#type.clone();
        let play_id = li.id.clone();
        let play_vid = li.state.video_id.clone();

        let remove_id = li.id.clone();
        let remove_name = li.name.clone();

        let peek_target = QuickPeekTarget {
            type_name: li.r#type.clone(),
            id: li.id.clone(),
            video_id: li.state.video_id.clone(),
            title: li.name.clone(),
            preview: None,
        };

        let ctx_type = li.r#type.clone();
        let ctx_id = li.id.clone();
        let ctx_name = li.name.clone();
        let ctx_vid = li.state.video_id.clone();
        let ctx_watched = li.watched();
        let ctx_can_rewind = li.state.time_offset > 0;

        let hover_type = li.r#type.clone();
        let hover_id = li.id.clone();
        let hover_bg = wide_url;

        let toggle_watched_btn =
            self.build_card_toggle_watched_button("lib-wide-watch", idx, li, cx);

        render_poster_card(
            PosterCardProps::new(("library-wide-card", idx), li.name.clone(), li.id.clone())
                .subtitle(Some(subtitle))
                .image(wide_tex)
                .aspect(PosterAspect::Landscape16x9)
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
                .on_quick_peek(click_handler(cx.listener(move |this, _, _, cx| {
                    this.open_quick_peek(peek_target.clone(), cx);
                })))
                .quick_action(Some(toggle_watched_btn))
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
                    },
                )))
                .on_hover(hover_handler(cx.listener(move |this, hovered, _, cx| {
                    this.on_card_hover_prefetch(
                        &hover_type,
                        &hover_id,
                        hover_bg.as_deref(),
                        *hovered,
                        cx,
                    );
                }))),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_library_compact_row(
        &self,
        idx: usize,
        li: &LibraryItem,
        status: LibraryStatusTab,
        notification_count: usize,
        row_height_px: f32,
        is_focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = active_theme();
        let thumb_tex = self.poster_cache.resolve_thumbnail_url(li.poster.as_ref());
        let type_label = sentence_case_label(&li.r#type);
        let ep_code = format_movietracker_episode_code(li.state.video_id.as_deref());
        let remaining = format_library_remaining_time(li);
        let progress = clamp_progress_fraction(library_item_progress_fraction(li, status));
        let notif_label = format_notification_badge_label(notification_count);
        let is_watched = li.watched();
        let resume_label = if status == LibraryStatusTab::Watching || li.state.time_offset > 0 {
            "Resume"
        } else {
            "Play"
        };

        let meta_detail: Option<SharedString> = match (ep_code.as_deref(), remaining.as_deref()) {
            (Some(ep), Some(rem)) => Some(format!("{ep} · {rem}").into()),
            (Some(ep), None) => Some(ep.to_owned().into()),
            (None, Some(rem)) => Some(rem.to_owned().into()),
            (None, None) if li.state.times_watched > 1 => {
                Some(format!("Watched {}×", li.state.times_watched).into())
            }
            (None, None) => None,
        };

        let (status_icon, status_color) = match status {
            LibraryStatusTab::Watching => (IconName::Play, theme.status_watching),
            LibraryStatusTab::Watchlist => (IconName::Bookmark, theme.status_watchlist),
            LibraryStatusTab::Watched => (IconName::Check, theme.status_watched),
        };
        let status_bg = (status_color << 8) | if theme.is_light { 0x1F } else { 0x1A };
        let status_border = (status_color << 8) | if theme.is_light { 0x55 } else { 0x47 };

        let open_type = li.r#type.clone();
        let open_id = li.id.clone();
        let open_vid = li.state.video_id.clone();

        let play_type = li.r#type.clone();
        let play_id = li.id.clone();
        let play_vid = li.state.video_id.clone();

        let watch_id = li.id.clone();
        let watch_name = li.name.clone();

        let peek_target = QuickPeekTarget {
            type_name: li.r#type.clone(),
            id: li.id.clone(),
            video_id: li.state.video_id.clone(),
            title: li.name.clone(),
            preview: None,
        };

        let ctx_type = li.r#type.clone();
        let ctx_id = li.id.clone();
        let ctx_name = li.name.clone();
        let ctx_vid = li.state.video_id.clone();
        let ctx_can_rewind = li.state.time_offset > 0;

        let hover_type = li.r#type.clone();
        let hover_id = li.id.clone();

        div()
            .id(("library-compact-row", idx))
            .w_full()
            .h(px(row_height_px))
            .px(sp(12.0))
            .rounded(px(RADIUS_CARD))
            .cursor_pointer()
            .bg(if is_focused {
                rgba(theme.glass_row_selected_bg)
            } else {
                rgba(theme.glass_card_bg)
            })
            .border_1()
            .border_color(if is_focused {
                rgb(theme.accent)
            } else {
                rgba(theme.glass_border)
            })
            .hover(move |s| {
                s.bg(rgba(theme.glass_row_hover_bg))
                    .border_color(if is_focused {
                        rgb(theme.accent)
                    } else {
                        rgba(theme.glass_border_strong)
                    })
            })
            .flex()
            .items_center()
            .justify_between()
            .gap(sp(12.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.open_meta_details(
                        open_type.clone(),
                        open_id.clone(),
                        open_vid.clone(),
                        cx,
                    );
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
                        ctx_name.clone(),
                    )
                    .video_id(ctx_vid.clone())
                    .is_in_library(true)
                    .is_watched(is_watched)
                    .can_rewind_cw(ctx_can_rewind);
                    this.open_browsing_context_menu(menu, cx);
                }),
            )
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                this.on_card_hover_prefetch(&hover_type, &hover_id, None, *hovered, cx);
            }))
            // Left: poster thumb + title + badges + episode/progress bar.
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .gap(sp(12.0))
                    .child(
                        div()
                            .w(sp(34.0))
                            .h(sp(48.0))
                            .flex_shrink_0()
                            .rounded(px(5.0))
                            .overflow_hidden()
                            .bg(rgb(placeholder_tint_for_id(&li.id)))
                            .border_1()
                            .border_color(rgba(theme.glass_border))
                            .child(if let Some(tex) = thumb_tex {
                                img(tex)
                                    .size_full()
                                    .object_fit(ObjectFit::Cover)
                                    .into_any_element()
                            } else {
                                div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(svg_icon(
                                        if li.r#type.eq_ignore_ascii_case("movie") {
                                            IconName::Film
                                        } else {
                                            IconName::Tv
                                        },
                                        13.0 * theme.ui_scale,
                                        rgb(theme.text_muted),
                                    ))
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
                                    .w_full()
                                    .min_w(px(0.0))
                                    .flex()
                                    .items_center()
                                    .gap(sp(8.0))
                                    .child(
                                        div()
                                            .min_w(px(0.0))
                                            .truncate()
                                            .font(ui_sans(FontWeight::SEMIBOLD))
                                            .text_size(sp(13.5))
                                            .text_color(rgb(theme.text_primary))
                                            .child(li.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .px(sp(7.0))
                                            .py(sp(1.5))
                                            .rounded(px(RADIUS_PILL))
                                            .bg(rgba(theme.glass_trough_bg))
                                            .border_1()
                                            .border_color(rgba(theme.glass_border))
                                            .font(mono_font())
                                            .text_size(sp(10.5))
                                            .text_color(rgb(theme.text_muted))
                                            .child(type_label),
                                    )
                                    .when_some(notif_label, |row, label| {
                                        row.child(
                                            div()
                                                .flex_shrink_0()
                                                .px(sp(7.0))
                                                .py(sp(1.5))
                                                .rounded(px(RADIUS_PILL))
                                                .bg(rgb(theme.accent))
                                                .font(mono_font())
                                                .font_weight(FontWeight::BOLD)
                                                .text_size(sp(10.0))
                                                .text_color(rgb(theme.cta_on_accent))
                                                .child(label),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(sp(10.0))
                                    .when_some(meta_detail, |row, detail| {
                                        row.child(
                                            div()
                                                .font(ui_sans(FontWeight::NORMAL))
                                                .font_features(tabular_nums())
                                                .text_size(sp(11.5))
                                                .text_color(rgb(theme.text_secondary))
                                                .truncate()
                                                .child(detail),
                                        )
                                    })
                                    .when_some(progress, |row, frac| {
                                        row.child(
                                            div()
                                                .w(sp(104.0))
                                                .h(px(4.0))
                                                .flex_shrink_0()
                                                .rounded(px(RADIUS_PILL))
                                                .overflow_hidden()
                                                .bg(rgba(theme.glass_trough_bg))
                                                .child(
                                                    div()
                                                        .h_full()
                                                        .w(relative(frac))
                                                        .bg(rgb(theme.accent)),
                                                ),
                                        )
                                    }),
                            ),
                    ),
            )
            // Right: status pill + inline 1-click Resume / Mark Watched / Preview actions.
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(sp(8.0))
                    .child(
                        div()
                            .h(sp(26.0))
                            .px(sp(9.0))
                            .rounded(px(RADIUS_PILL))
                            .bg(rgba(status_bg))
                            .border_1()
                            .border_color(rgba(status_border))
                            .flex()
                            .items_center()
                            .gap(sp(5.0))
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(sp(11.5))
                            .text_color(rgb(status_color))
                            .child(svg_icon(
                                status_icon,
                                11.5 * theme.ui_scale,
                                rgb(status_color),
                            ))
                            .child(status.label()),
                    )
                    // 1-click Resume / Play button.
                    .child(
                        div()
                            .id(("lib-row-resume", idx))
                            .cursor_pointer()
                            .h(sp(30.0))
                            .px(sp(11.0))
                            .rounded(px(RADIUS_CONTROL))
                            .bg(rgb(theme.accent))
                            .hover(move |s| s.bg(rgb(theme.accent_hover)))
                            .flex()
                            .items_center()
                            .gap(sp(5.0))
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_size(sp(12.0))
                            .text_color(rgb(theme.cta_on_accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.quick_resume_or_open(
                                        play_type.clone(),
                                        play_id.clone(),
                                        play_vid.clone(),
                                        cx,
                                    );
                                }),
                            )
                            .child(svg_icon_weighted(
                                IconName::Play,
                                IconWeight::Fill,
                                11.5 * theme.ui_scale,
                                rgb(theme.cta_on_accent),
                            ))
                            .child(resume_label),
                    )
                    // 1-click Mark Watched / Watched toggle button.
                    .child(
                        div()
                            .id(("lib-row-watched", idx))
                            .cursor_pointer()
                            .h(sp(30.0))
                            .px(sp(10.0))
                            .rounded(px(RADIUS_CONTROL))
                            .bg(if is_watched {
                                rgba(
                                    (theme.status_watched << 8)
                                        | if theme.is_light { 0x22 } else { 0x1F },
                                )
                            } else {
                                rgba(theme.glass_trough_bg)
                            })
                            .border_1()
                            .border_color(if is_watched {
                                rgba(
                                    (theme.status_watched << 8)
                                        | if theme.is_light { 0x66 } else { 0x55 },
                                )
                            } else {
                                rgba(theme.glass_border)
                            })
                            .hover(move |s| {
                                s.bg(rgba(theme.glass_row_hover_bg))
                                    .border_color(rgba(theme.glass_border_strong))
                            })
                            .flex()
                            .items_center()
                            .gap(sp(5.0))
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(sp(12.0))
                            .text_color(if is_watched {
                                rgb(theme.status_watched)
                            } else {
                                rgb(theme.text_secondary)
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    let next_watched = !is_watched;
                                    this.core.mark_library_item_watched(
                                        watch_id.clone(),
                                        next_watched,
                                    );
                                    this.push_toast(
                                        ToastKind::Info,
                                        if next_watched {
                                            "Marked as watched"
                                        } else {
                                            "Marked as unwatched"
                                        },
                                        Some(watch_name.clone()),
                                        cx,
                                    );
                                    cx.notify();
                                }),
                            )
                            .child(svg_icon(
                                IconName::Check,
                                12.5 * theme.ui_scale,
                                if is_watched {
                                    rgb(theme.status_watched)
                                } else {
                                    rgb(theme.text_muted)
                                },
                            ))
                            .child(if is_watched {
                                "Watched"
                            } else {
                                "Mark Watched"
                            }),
                    )
                    // 1-click Preview (Quick Peek) button.
                    .child(
                        div()
                            .id(("lib-row-preview", idx))
                            .cursor_pointer()
                            .h(sp(30.0))
                            .px(sp(10.0))
                            .rounded(px(RADIUS_CONTROL))
                            .bg(rgba(theme.glass_trough_bg))
                            .border_1()
                            .border_color(rgba(theme.glass_border))
                            .hover(move |s| {
                                s.bg(rgba(theme.glass_row_hover_bg))
                                    .border_color(rgba(theme.glass_border_strong))
                            })
                            .flex()
                            .items_center()
                            .gap(sp(5.0))
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(sp(12.0))
                            .text_color(rgb(theme.text_secondary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_quick_peek(peek_target.clone(), cx);
                                }),
                            )
                            .child(svg_icon(
                                IconName::Eye,
                                13.0 * theme.ui_scale,
                                rgb(theme.text_muted),
                            ))
                            .child("Preview"),
                    ),
            )
            .into_any_element()
    }
}

/// Computes fluid horizontal padding matching CSS `clamp(24px, 3vw, 56px)`.
pub fn compute_library_horizontal_padding_px(viewport_w: f32) -> f32 {
    if !viewport_w.is_finite() || viewport_w <= 0.0 {
        return 32.0;
    }
    (viewport_w * 0.03).clamp(24.0, 56.0)
}

/// Minimum 16:9 widescreen card width scaled by the active `CardDensityChoice` and UI scale.
pub fn wide_card_min_width_px(density: CardDensityChoice, ui_scale: f32) -> f32 {
    let base = match density {
        CardDensityChoice::Compact => 264.0,
        CardDensityChoice::Comfortable => 312.0,
        CardDensityChoice::Cinema => 368.0,
    };
    let scale = if ui_scale.is_finite() && ui_scale > 0.0 {
        ui_scale
    } else {
        1.0
    };
    base * scale
}

fn library_item_progress_fraction(li: &LibraryItem, status: LibraryStatusTab) -> Option<f32> {
    if status != LibraryStatusTab::Watching || li.state.time_offset == 0 {
        return None;
    }
    if li.state.duration > 0 {
        let raw = (li.state.time_offset as f32 / li.state.duration as f32).clamp(0.0, 1.0);
        (raw > 0.01).then_some(raw)
    } else {
        let pct = (li.progress() as f32 / 100.0).clamp(0.0, 1.0);
        (pct > 0.01).then_some(pct)
    }
}

fn format_remaining_duration_ms(time_offset_ms: u64, duration_ms: u64) -> Option<String> {
    if time_offset_ms == 0 {
        return None;
    }
    if duration_ms > time_offset_ms && duration_ms > 60_000 {
        let rem_mins = ((duration_ms - time_offset_ms) / 60_000).max(1);
        let hours = rem_mins / 60;
        let mins = rem_mins % 60;
        if hours > 0 && mins > 0 {
            Some(format!("{hours}h {mins}m left"))
        } else if hours > 0 {
            Some(format!("{hours}h left"))
        } else {
            Some(format!("{rem_mins}m left"))
        }
    } else {
        None
    }
}

fn format_library_remaining_time(li: &LibraryItem) -> Option<String> {
    if li.state.time_offset == 0 {
        return None;
    }
    if let Some(rem) = format_remaining_duration_ms(li.state.time_offset, li.state.duration) {
        return Some(rem);
    }
    let pct = li.progress().clamp(0.0, 100.0) as f32;
    if pct >= 1.0 {
        Some(format!("{pct:.0}% watched"))
    } else {
        None
    }
}

fn format_wide_card_subtitle(
    li: &LibraryItem,
    status: LibraryStatusTab,
    notification_count: usize,
) -> SharedString {
    let ep_code = format_movietracker_episode_code(li.state.video_id.as_deref());
    let remaining = if status == LibraryStatusTab::Watching {
        format_library_remaining_time(li)
    } else {
        None
    };

    let trailing = if let Some(rem) = remaining {
        rem
    } else if notification_count > 0 {
        if notification_count == 1 {
            "New episode".to_owned()
        } else {
            format!("{notification_count} new episodes")
        }
    } else if status == LibraryStatusTab::Watched {
        if li.state.times_watched > 1 {
            format!("Watched {}×", li.state.times_watched)
        } else {
            "Watched".to_owned()
        }
    } else {
        sentence_case_label(&li.r#type)
    };

    match ep_code {
        Some(ep) => format!("{ep} · {trailing}").into(),
        None => trailing.into(),
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

#[allow(clippy::too_many_arguments)]
fn render_library_stat_pill(
    id: &'static str,
    icon: IconName,
    icon_weight: IconWeight,
    icon_color_hex: u32,
    value: String,
    label: &'static str,
    active: bool,
    on_click: Option<crate::ui_components::ClickHandler>,
) -> AnyElement {
    let theme = active_theme();
    let clickable = on_click.is_some();
    let mut pill = div()
        .id(id)
        .h(sp(32.0))
        .px(sp(11.0))
        .rounded(px(RADIUS_PILL))
        .bg(if active {
            rgba(theme.glass_row_selected_bg)
        } else {
            rgba(theme.glass_card_bg)
        })
        .border_1()
        .border_color(if active {
            rgba(theme.border_accent)
        } else {
            rgba(theme.glass_border)
        })
        .flex()
        .items_center()
        .gap(sp(7.0))
        .child(svg_icon_weighted(
            icon,
            icon_weight,
            12.5 * theme.ui_scale,
            rgb(icon_color_hex),
        ))
        .child(
            div()
                .font(mono_font())
                .font_features(tabular_nums())
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(sp(12.5))
                .text_color(rgb(theme.text_primary))
                .child(value),
        )
        .child(
            div()
                .font(ui_sans(FontWeight::NORMAL))
                .text_size(sp(12.0))
                .text_color(rgb(theme.text_muted))
                .child(label),
        );

    if clickable {
        pill = pill.cursor_pointer().hover(move |s| {
            s.bg(rgba(theme.glass_row_hover_bg))
                .border_color(if active {
                    rgba(theme.border_accent)
                } else {
                    rgba(theme.glass_border_strong)
                })
        });
    }

    if let Some(handler) = on_click {
        pill = pill.on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            handler(ev, window, cx);
        });
    }

    pill.into_any_element()
}

fn render_library_empty_state(
    active_tab: LibraryFilterTab,
    active_type: LibraryTypeFilter,
    total_library_count: usize,
    has_search_query: bool,
    trimmed_query: &str,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let theme = active_theme();
    let has_type_filter = active_type != LibraryTypeFilter::All;

    let (empty_title, empty_sub): (SharedString, SharedString) = if has_search_query {
        (
            format!("No matches in {}", active_tab.label()).into(),
            format!(
                "No titles in {} matched \"{trimmed_query}\". Try another title or clear your filter.",
                active_tab.label()
            )
            .into(),
        )
    } else if has_type_filter && total_library_count > 0 {
        let type_name = match active_type {
            LibraryTypeFilter::Movies => "movies",
            LibraryTypeFilter::Shows => "series",
            LibraryTypeFilter::All => "titles",
        };
        (
            format!("No {type_name} in {}", active_tab.label()).into(),
            "Switch back to All types or explore another filter tab.".into(),
        )
    } else {
        match active_tab {
            LibraryFilterTab::All => (
                "Your vault is empty".into(),
                "Bookmark films and series across Discover or start watching to build your personal vault."
                    .into(),
            ),
            LibraryFilterTab::Watching => (
                "Nothing in progress".into(),
                "Start any film or episode and it will stay waiting for you right here.".into(),
            ),
            LibraryFilterTab::NewEpisodes => (
                "All caught up".into(),
                "New episodes for series in your library will surface here automatically.".into(),
            ),
            LibraryFilterTab::Watchlist => (
                "Your watchlist is empty".into(),
                "Bookmark films and series across Discover to build your queue.".into(),
            ),
            LibraryFilterTab::Watched => (
                "No finished titles yet".into(),
                "Completed films and series are archived here automatically.".into(),
            ),
        }
    };

    let mut actions = div()
        .pt(sp(6.0))
        .flex()
        .flex_wrap()
        .items_center()
        .justify_center()
        .gap(sp(10.0));

    if has_search_query || has_type_filter {
        actions = actions.child(primary_accent_button(
            "lib-empty-clear-filters",
            Some(IconName::X),
            if has_search_query {
                "Clear search"
            } else {
                "Reset type filter"
            },
            cx.listener(|this, _, _, cx| {
                this.library_search_query.clear();
                this.input_cursors.remove(&FocusedInput::LibrarySearch);
                this.input_selected_all.remove(&FocusedInput::LibrarySearch);
                this.library_type_filter = LibraryTypeFilter::All;
                this.kb_focus_index = None;
                this.reset_library_scroll();
                cx.notify();
            }),
        ));
    } else {
        if active_tab != LibraryFilterTab::All && total_library_count > 0 {
            actions = actions.child(secondary_glass_button(
                "lib-empty-show-all",
                Some(IconName::Bookmark),
                "View all titles",
                false,
                cx.listener(|this, _, _, cx| {
                    this.select_library_filter_tab(LibraryFilterTab::All, cx);
                }),
            ));
        }
        actions = actions.child(primary_accent_button(
            "lib-empty-discover",
            Some(IconName::Compass),
            "Browse Discover",
            cx.listener(|this, _, _, cx| {
                this.navigate_tab(SidebarTab::Discover, cx);
            }),
        ));
    }

    div()
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w_full()
                .max_w(sp(560.0))
                .py(sp(56.0))
                .px(sp(28.0))
                .rounded(px(RADIUS_CARD))
                .border_1()
                .border_color(rgba(theme.glass_border))
                .bg(rgba(theme.glass_card_bg))
                .flex()
                .flex_col()
                .items_center()
                .gap(sp(12.0))
                .child(
                    div()
                        .w(sp(44.0))
                        .h(sp(44.0))
                        .rounded(px(RADIUS_PILL))
                        .bg(rgba(theme.glass_trough_bg))
                        .border_1()
                        .border_color(rgba(theme.glass_border))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(svg_icon(
                            if has_search_query {
                                IconName::Search
                            } else {
                                IconName::Bookmark
                            },
                            19.0 * theme.ui_scale,
                            rgb(theme.text_muted),
                        )),
                )
                .child(
                    div()
                        .font(editorial_serif())
                        .text_size(sp(28.0))
                        .text_color(rgb(theme.text_primary))
                        .child(empty_title),
                )
                .child(
                    div()
                        .max_w(sp(420.0))
                        .text_size(sp(13.5))
                        .text_color(rgb(theme.text_secondary))
                        .child(empty_sub),
                )
                .child(actions),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_horizontal_padding_clamps_between_24_and_56() {
        assert_eq!(compute_library_horizontal_padding_px(600.0), 24.0);
        assert_eq!(compute_library_horizontal_padding_px(800.0), 24.0);
        assert!((compute_library_horizontal_padding_px(1200.0) - 36.0).abs() < f32::EPSILON);
        assert!((compute_library_horizontal_padding_px(1600.0) - 48.0).abs() < f32::EPSILON);
        assert_eq!(compute_library_horizontal_padding_px(2560.0), 56.0);
        assert_eq!(compute_library_horizontal_padding_px(3840.0), 56.0);
        assert_eq!(compute_library_horizontal_padding_px(0.0), 32.0);
        assert_eq!(compute_library_horizontal_padding_px(f32::NAN), 32.0);
    }

    #[test]
    fn wide_card_widths_order_by_density_and_scale() {
        let compact = wide_card_min_width_px(CardDensityChoice::Compact, 1.0);
        let comfortable = wide_card_min_width_px(CardDensityChoice::Comfortable, 1.0);
        let cinema = wide_card_min_width_px(CardDensityChoice::Cinema, 1.0);
        assert!(compact < comfortable && comfortable < cinema);
        assert!(
            (wide_card_min_width_px(CardDensityChoice::Comfortable, 1.25) - comfortable * 1.25)
                .abs()
                < f32::EPSILON
        );
        assert_eq!(
            wide_card_min_width_px(CardDensityChoice::Comfortable, f32::NAN),
            comfortable
        );
    }

    #[test]
    fn remaining_duration_formats_minutes_and_hours_cleanly() {
        assert_eq!(format_remaining_duration_ms(0, 3_600_000), None);
        assert_eq!(
            format_remaining_duration_ms(1_800_000, 3_600_000),
            Some("30m left".to_owned())
        );
        assert_eq!(
            format_remaining_duration_ms(600_000, 4_200_000),
            Some("1h left".to_owned())
        );
        assert_eq!(
            format_remaining_duration_ms(600_000, 5_100_000),
            Some("1h 15m left".to_owned())
        );
        assert_eq!(format_remaining_duration_ms(3_600_000, 3_600_000), None);
    }
}
