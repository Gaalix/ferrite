use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    hash::{Hash, Hasher},
    ops::Range,
    rc::Rc,
    sync::Arc,
};

use gpui::{
    deferred, div, img, point, prelude::*, px, relative, rgb, rgba, AnyElement, App, Context,
    FontWeight, MouseButton, MouseDownEvent, ObjectFit, Render, SharedString, StyleRefinement,
    WeakEntity, Window,
};
use stremio_core_runtime::stremio_core::types::library::LibraryItem;

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
        active_theme, active_theme_version, mono_font, placeholder_tint_for_id,
        placeholder_tint_for_id_with_preset, sentence_case_label, sp, tabular_nums, ui_sans,
        ThemePalette, ThemePreset, RADIUS_CARD, RADIUS_CONTROL, RADIUS_PILL, RADIUS_POSTER,
    },
    ui::{
        card_action_dispatcher, clamp_progress_fraction, compute_app_grid_metrics,
        compute_app_landscape_grid_metrics, dropdown_trigger, format_notification_badge_label,
        full_height_page_container, glass_dropdown_option, glass_dropdown_panel,
        glass_segmented_bar, glass_segmented_item, infer_grid_poster_aspect, primary_accent_button,
        render_empty_state, render_landscape_card, render_page_header,
        render_prepared_poster_card, render_section_header, search_field, secondary_glass_button,
        toolbar_bar, CardActionDispatcher, ClickHandler, DropdownTriggerProps, EmptyStateProps,
        LandscapeCardProps, PageHeader, PageWidth, PosterCardAction, PosterCardCapabilities,
        PosterMetrics, PreparedPosterCard, PreparedPosterRenderCtx, SearchFieldProps,
        SectionHeader, PAGE_SECTION_GAP_PX,
    },
    virtual_grid::{slice_row_items, PosterAspect},
};

const UP_NEXT_SHELF_LIMIT: usize = 12;

struct UpNextHighlightEntry {
    item: LibraryItem,
    status: LibraryStatusTab,
    notification_count: usize,
}

struct CachedLibraryAux {
    library_rev: u64,
    notification_rev: u64,
    notif_counts: Arc<HashMap<String, usize>>,
    up_next_items: Arc<Vec<UpNextHighlightEntry>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LibraryCardSlabKey {
    library_rev: u64,
    notification_rev: u64,
    filter_tab: LibraryFilterTab,
    type_filter: LibraryTypeFilter,
    sort_criterion: LibrarySortCriterion,
    view_mode: LibraryViewMode,
    aspect: PosterAspect,
    theme_preset: ThemePreset,
    search_query: String,
}

struct CachedLibraryCardSlab {
    key: LibraryCardSlabKey,
    cards: Arc<[PreparedPosterCard]>,
    wide_urls: Arc<[Option<SharedString>]>,
}

type ToggleWatchedDispatcher = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// Cached child view boundary for Library's PageHeader + optional Up Next shelf + Toolbar.
pub struct LibraryChromeView {
    shell: WeakEntity<AppShell>,
}

impl LibraryChromeView {
    pub fn new(shell: WeakEntity<AppShell>) -> Self {
        Self { shell }
    }
}

impl Render for LibraryChromeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(shell) = self.shell.upgrade() else {
            return div().w_full().into_any_element();
        };
        shell.update(cx, |shell, shell_cx| {
            shell.render_library_chrome_inner(shell_cx)
        })
    }
}

/// Uncached child view boundary owning Library's virtualized `uniform_list`.
///
/// Scrolling the list marks ONLY `LibraryGridView -> ActiveScrollHostView -> AppShell`
/// dirty, skipping render/layout/prepaint/paint of `LibraryChromeView` (including the
/// 12-card Up Next shelf), `TopNavView`, `AmbientBackdropView`, and `BackToTopView`.
pub struct LibraryGridView {
    shell: WeakEntity<AppShell>,
}

impl LibraryGridView {
    pub fn new(shell: WeakEntity<AppShell>) -> Self {
        Self { shell }
    }
}

impl Render for LibraryGridView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(shell) = self.shell.upgrade() else {
            return div().flex_1().w_full().min_h(px(0.0)).into_any_element();
        };
        shell.update(cx, |shell, shell_cx| {
            shell.render_library_grid_inner(window, shell_cx)
        })
    }
}

impl AppShell {
    pub fn render_library_view(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let memo = self.get_or_update_memoized_library();
        if self.library_filter_tab == LibraryFilterTab::NewEpisodes && memo.count_new_episodes == 0
        {
            self.library_filter_tab = LibraryFilterTab::All;
        }

        let raw_viewport_w = f32::from(window.viewport_size().width);
        let viewport_w = if raw_viewport_w > 0.0 && raw_viewport_w.is_finite() {
            raw_viewport_w
        } else {
            self.viewport_size_px.0
        };

        let mut layout_sig_hasher = DefaultHasher::new();
        (
            viewport_w.to_bits(),
            active_theme_version(),
            self.library_state_rev,
            self.board_state_rev,
            std::mem::discriminant(&self.library_filter_tab),
            self.library_search_query.trim().is_empty(),
            self.library_sort_dropdown_open,
        )
            .hash(&mut layout_sig_hasher);
        let chrome_layout_sig = layout_sig_hasher.finish();

        // Keep LibraryChromeView uncached while the sort dropdown is open or on the 1st
        // measure frame of a new layout signature; reuse its cache on all scroll frames.
        let cached_chrome_h = if !self.library_sort_dropdown_open {
            self.library_chrome_height
                .get()
                .and_then(|(sig, h)| (sig == chrome_layout_sig && h > 1.0).then_some(h))
        } else {
            None
        };
        let chrome_el: AnyElement = match cached_chrome_h {
            Some(h) => self
                .library_chrome_view
                .clone()
                .cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(px(h))
                        .flex_shrink_0(),
                )
                .into_any_element(),
            None => self.library_chrome_view.clone().into_any_element(),
        };
        let height_cell = Rc::clone(&self.library_chrome_height);
        let chrome_slot = div()
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(b) = bounds.first() {
                    let h = f32::from(b.size.height);
                    if h > 1.0 && height_cell.get() != Some((chrome_layout_sig, h)) {
                        height_cell.set(Some((chrome_layout_sig, h)));
                    }
                }
            })
            .id("library-chrome-slot")
            .w_full()
            .flex_shrink_0()
            .child(chrome_el);

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
                full_height_page_container(PageWidth::Wide, viewport_w)
                    .child(chrome_slot)
                    .child(self.library_grid_view.clone()),
            )
    }

    fn render_library_chrome_inner(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = active_theme();
        let mut memo = self.get_or_update_memoized_library();
        if self.library_filter_tab == LibraryFilterTab::NewEpisodes && memo.count_new_episodes == 0
        {
            self.library_filter_tab = LibraryFilterTab::All;
            memo = self.get_or_update_memoized_library();
        }
        let aux = self.get_or_update_library_aux();
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

        let show_up_next_shelf = !search_trimmed_non_empty
            && matches!(
                active_tab,
                LibraryFilterTab::All | LibraryFilterTab::Watching | LibraryFilterTab::NewEpisodes
            )
            && !aux.up_next_items.is_empty();
        let up_next_arc = Arc::clone(&aux.up_next_items);
        if !show_up_next_shelf {
            self.library_up_next_pending_images.set(false);
        }

        let visible_filter_tabs: Vec<LibraryFilterTab> = LibraryFilterTab::ALL
            .into_iter()
            .filter(|tab| *tab != LibraryFilterTab::NewEpisodes || memo.count_new_episodes > 0)
            .collect();

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.sp(PAGE_SECTION_GAP_PX))
            // 1. Canonical Page Header with unique watch-time pill (tab bar below owns counts).
            .child(render_page_header(
                PageHeader::new("Library").right_slot(Some(
                    render_library_stat_pill(
                        "lib-stat-hours",
                        IconName::Clock,
                        IconWeight::Regular,
                        theme.accent_amber,
                        memo.hours_logged.to_string(),
                        "Hours watched",
                        false,
                        None,
                    )
                    .into_any_element(),
                )),
            ))
            // 2. "New Episodes & Up Next" Highlight Shelf.
            .when(show_up_next_shelf, |col| {
                col.child(self.render_library_up_next_shelf(
                    &up_next_arc,
                    memo.count_new_episodes,
                    active_tab,
                    cx,
                ))
            })
            // 3. Unified Floating Frosted Glass Toolbar.
            .child(
                toolbar_bar()
                    .child(
                        glass_segmented_bar("lib-filter-bar").children(
                            visible_filter_tabs.into_iter().enumerate().map(|(idx, tab)| {
                                let is_active = tab == active_tab;
                                let count = match tab {
                                    LibraryFilterTab::All => memo.count_all,
                                    LibraryFilterTab::Watching => memo.count_watching,
                                    LibraryFilterTab::NewEpisodes => memo.count_new_episodes,
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
                            }),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(sp(8.0))
                            // Inline instant filter input.
                            .child(search_field(
                                SearchFieldProps::new(
                                    "lib-search-box",
                                    search_display.clone(),
                                    "Filter library...",
                                    !has_search_query,
                                    search_focused,
                                )
                                .clear_id("lib-search-clear")
                                .width_px(Some(216.0))
                                .on_focus(cx.listener(|this, _, _, cx| {
                                    this.library_sort_dropdown_open = false;
                                    this.focused_input = Some(FocusedInput::LibrarySearch);
                                    cx.notify();
                                }))
                                .on_clear(cx.listener(|this, _, _, cx| {
                                    this.clear_library_search(cx);
                                })),
                            ))
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
                                    .child(dropdown_trigger(
                                        DropdownTriggerProps::new(
                                            "lib-sort-pill",
                                            active_sort.label(),
                                            sort_open,
                                            cx.listener(|this, _, _, cx| {
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
                                        .leading_icon(Some(IconName::Sliders)),
                                    ))
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
                                                                        move |this, _, _, cx| {
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
                                                LibraryViewMode::PosterGrid => IconName::GridFour,
                                                LibraryViewMode::WideBackdrops => IconName::Rows,
                                                LibraryViewMode::CompactList => IconName::List,
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
                                                            this.preferences.library_view_mode =
                                                                mode;
                                                            let _ = this.preferences.save();
                                                            this.library_sort_dropdown_open = false;
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
            .into_any_element()
    }

    fn render_library_grid_inner(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = active_theme();
        let mut memo = self.get_or_update_memoized_library();
        if self.library_filter_tab == LibraryFilterTab::NewEpisodes && memo.count_new_episodes == 0
        {
            self.library_filter_tab = LibraryFilterTab::All;
            memo = self.get_or_update_memoized_library();
        }
        let aux = self.get_or_update_library_aux();
        let notif_counts_arc = Arc::clone(&aux.notif_counts);
        let active_tab = self.library_filter_tab;
        let active_type = self.library_type_filter;
        let active_sort = self.library_sort_criterion;
        let active_view_mode = self.preferences.library_view_mode;
        let search_trimmed_non_empty = !self.library_search_query.trim().is_empty();

        let filtered_arc = Arc::clone(&memo.filtered_items);
        let total_filtered = filtered_arc.len();

        let raw_viewport_w = f32::from(window.viewport_size().width);
        let viewport_w = if raw_viewport_w > 0.0 && raw_viewport_w.is_finite() {
            raw_viewport_w
        } else {
            self.viewport_size_px.0
        };
        let poster_dims = PosterMetrics::from_theme(&theme);
        let grid_gap = poster_dims.gap_px;

        let poster_aspect =
            infer_grid_poster_aspect(filtered_arc.iter().map(|(li, _)| &li.poster_shape));
        let poster_metrics =
            compute_app_grid_metrics(total_filtered, viewport_w, 0.0, poster_aspect, &theme);
        let wide_metrics =
            compute_app_landscape_grid_metrics(total_filtered, viewport_w, 0.0, &theme);

        let kb_columns = match active_view_mode {
            LibraryViewMode::PosterGrid => poster_metrics.columns,
            LibraryViewMode::WideBackdrops => wide_metrics.columns,
            LibraryViewMode::CompactList => 1,
        };
        let mut kb_hasher = DefaultHasher::new();
        (
            "library",
            memo.library_rev,
            memo.notification_rev,
            std::mem::discriminant(&active_tab),
            std::mem::discriminant(&active_type),
            std::mem::discriminant(&active_sort),
            std::mem::discriminant(&active_view_mode),
            kb_columns,
            memo.search_query.as_str(),
        )
            .hash(&mut kb_hasher);
        let kb_key = kb_hasher.finish();
        self.sync_kb_focus_items_cached(kb_key, || {
            filtered_arc
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
                .collect()
        });

        if total_filtered == 0 {
            self.active_scroll_suppressed.set(true);
            return render_library_empty_state(
                active_tab,
                active_type,
                memo.count_all,
                search_trimmed_non_empty,
                self.library_search_query.trim(),
                cx,
            );
        }

        match active_view_mode {
            LibraryViewMode::PosterGrid => {
                let columns = poster_metrics.columns;
                let row_count = poster_metrics.layout.row_count;
                let row_stride_px = poster_metrics.row_stride_px;
                let card_width_px = poster_metrics.card_width_px;
                let image_height_px = poster_metrics.image_height_px;
                let items_for_grid = Arc::clone(&filtered_arc);
                let slab = self.get_or_update_library_card_slab(
                    &memo,
                    &aux,
                    LibraryViewMode::PosterGrid,
                    poster_aspect,
                    theme.preset,
                );
                let cards_for_grid = Arc::clone(&slab.cards);

                gpui::uniform_list(
                    "library-poster-grid",
                    row_count,
                    cx.processor(move |this, visible_rows: Range<usize>, _window, cx| {
                        let kb_focus_index = this.kb_focus_index;
                        let render_theme = active_theme();
                        let dispatcher =
                            build_library_card_dispatcher(&items_for_grid, None, cx);
                        let toggle_watched = build_library_toggle_watched_dispatcher(
                            &items_for_grid,
                            &cards_for_grid,
                            cx,
                        );

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
                                        let card = &cards_for_grid[idx];
                                        let poster_tex = card.poster_url.as_deref().and_then(|u| {
                                            this.poster_cache.resolve_poster_image(u)
                                        });
                                        let is_focused = kb_focus_index == Some(idx);
                                        let card_image_h = if card.aspect == poster_aspect {
                                            image_height_px
                                        } else {
                                            (card_width_px * card.aspect.height_over_width())
                                                .ceil()
                                                .min(image_height_px)
                                        };
                                        let toggle_watched_btn = build_card_toggle_watched_button(
                                            "lib-poster-watch",
                                            idx,
                                            card.watched,
                                            &render_theme,
                                            &toggle_watched,
                                        );

                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .h_full()
                                            .child(render_prepared_poster_card(
                                                PreparedPosterRenderCtx::new(
                                                    idx,
                                                    card,
                                                    "library-card",
                                                    &dispatcher,
                                                )
                                                .image(poster_tex)
                                                .image_height_px(Some(card_image_h))
                                                .is_focused(is_focused)
                                                .quick_action(Some(toggle_watched_btn)),
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
                let slab = self.get_or_update_library_card_slab(
                    &memo,
                    &aux,
                    LibraryViewMode::WideBackdrops,
                    PosterAspect::Landscape16x9,
                    theme.preset,
                );
                let cards_for_wide = Arc::clone(&slab.cards);
                let wide_urls_for_grid = Arc::clone(&slab.wide_urls);

                gpui::uniform_list(
                    "library-wide-grid",
                    row_count,
                    cx.processor(move |this, visible_rows: Range<usize>, _window, cx| {
                        let kb_focus_index = this.kb_focus_index;
                        let render_theme = active_theme();
                        let dispatcher = build_library_card_dispatcher(
                            &items_for_wide,
                            Some(Arc::clone(&wide_urls_for_grid)),
                            cx,
                        );
                        let toggle_watched = build_library_toggle_watched_dispatcher(
                            &items_for_wide,
                            &cards_for_wide,
                            cx,
                        );

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
                                        let card = &cards_for_wide[idx];
                                        let wide_url = wide_urls_for_grid
                                            .get(idx)
                                            .and_then(|u| u.as_deref());
                                        let wide_tex =
                                            this.poster_cache.resolve_widescreen_or_poster(
                                                wide_url,
                                                card.poster_url.as_deref(),
                                            );
                                        let is_focused = kb_focus_index == Some(idx);
                                        let toggle_watched_btn = build_card_toggle_watched_button(
                                            "lib-wide-watch",
                                            idx,
                                            card.watched,
                                            &render_theme,
                                            &toggle_watched,
                                        );

                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .h_full()
                                            .child(render_prepared_poster_card(
                                                PreparedPosterRenderCtx::new(
                                                    idx,
                                                    card,
                                                    "library-wide-card",
                                                    &dispatcher,
                                                )
                                                .image(wide_tex)
                                                .image_height_px(Some(image_height_px))
                                                .is_focused(is_focused)
                                                .quick_action(Some(toggle_watched_btn)),
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
                                        &items_for_list,
                                        idx,
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

    fn get_or_update_library_aux(&self) -> Arc<CachedLibraryAux> {
        let library_rev = self.library_state_rev;
        let notification_rev = self.board_state_rev;

        {
            let guard = self.memoized_library_aux_cache.lock();
            if let Some(any_arc) = guard.as_ref() {
                if let Ok(cached) = Arc::clone(any_arc).downcast::<CachedLibraryAux>() {
                    if cached.library_rev == library_rev
                        && cached.notification_rev == notification_rev
                    {
                        return cached;
                    }
                }
            }
        }

        let (notif_counts, mut entries) = self.core.with_model(|model| {
            let counts: HashMap<String, usize> = model
                .continue_watching_preview
                .items
                .iter()
                .filter(|item| item.notifications > 0)
                .map(|item| (item.library_item.id.clone(), item.notifications))
                .collect();

            let mut out = Vec::new();
            let mut visit = |li: &LibraryItem| {
                let Some(status) = classify_library_item_status(li) else {
                    return;
                };
                let notif = counts.get(&li.id).copied().unwrap_or(0);
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
            (counts, out)
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

        let arc = Arc::new(CachedLibraryAux {
            library_rev,
            notification_rev,
            notif_counts: Arc::new(notif_counts),
            up_next_items: Arc::new(entries),
        });
        *self.memoized_library_aux_cache.lock() =
            Some(Arc::clone(&arc) as Arc<dyn std::any::Any + Send + Sync>);
        arc
    }

    fn render_library_up_next_shelf(
        &self,
        entries: &Arc<Vec<UpNextHighlightEntry>>,
        new_episodes_count: usize,
        active_tab: LibraryFilterTab,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = active_theme();
        let heading = if new_episodes_count > 0 {
            "New Episodes & Up Next"
        } else {
            "Up Next"
        };

        let right_action = (new_episodes_count > 0 && active_tab != LibraryFilterTab::NewEpisodes)
            .then(|| {
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
                            this.select_library_filter_tab(LibraryFilterTab::NewEpisodes, cx);
                        }),
                    )
                    .child("Filter new episodes")
                    .child(svg_icon(
                        IconName::ChevronRight,
                        11.0 * theme.ui_scale,
                        rgb(theme.text_muted),
                    ))
                    .into_any_element()
            });

        let mut any_pending_images = false;
        let cards: Vec<AnyElement> = (0..entries.len())
            .map(|idx| {
                let (el, pending) = self.render_library_up_next_card(entries, idx, cx);
                if pending {
                    any_pending_images = true;
                }
                el
            })
            .collect();
        self.library_up_next_pending_images.set(any_pending_images);

        div()
            .id("library-up-next-shelf")
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(sp(12.0))
            .child(render_section_header(
                SectionHeader::new(heading)
                    .count_badge(
                        (new_episodes_count > 0)
                            .then(|| SharedString::from(format!("{new_episodes_count} new"))),
                    )
                    .right_slot(right_action),
            ))
            .child(
                div()
                    .id("library-up-next-strip")
                    .w_full()
                    .overflow_x_scroll()
                    .pb(sp(2.0))
                    .flex()
                    .items_stretch()
                    .gap(sp(12.0))
                    .children(cards),
            )
            .into_any_element()
    }

    fn render_library_up_next_card(
        &self,
        entries: &Arc<Vec<UpNextHighlightEntry>>,
        idx: usize,
        cx: &mut Context<Self>,
    ) -> (AnyElement, bool) {
        let entry = &entries[idx];
        let li = &entry.item;
        let notif_count = entry.notification_count;
        let wide_url = self.resolve_library_widescreen_url(li);
        let wide_tex = self.poster_cache.resolve_widescreen_or_poster(
            wide_url.as_deref(),
            li.poster.as_ref().map(|u| u.as_str()),
        );
        let is_pending = wide_tex.is_none() && (wide_url.is_some() || li.poster.is_some());
        let progress = clamp_progress_fraction(library_item_progress_fraction(li, entry.status));
        let ep_code = format_movietracker_episode_code(li.state.video_id.as_deref());
        let remaining = format_library_remaining_time(li);
        let subtitle: Option<SharedString> = match (ep_code.is_some(), remaining) {
            (_, Some(rem)) => Some(rem.into()),
            (true, None) if notif_count > 0 => Some("New episode".into()),
            (true, None) => None,
            (false, None) if notif_count > 0 => Some("New episode available".into()),
            (false, None) => Some(sentence_case_label(&li.r#type).into()),
        };

        let open_entries = Arc::clone(entries);
        let play_entries = Arc::clone(entries);
        let peek_entries = Arc::clone(entries);
        let ctx_entries = Arc::clone(entries);
        let hover_entries = Arc::clone(entries);
        let hover_bg = wide_url;

        let mut props = LandscapeCardProps::new(("lib-up-next-card", idx), li.name.clone(), &li.id)
            .image(wide_tex)
            .subtitle(subtitle)
            .notification_count(notif_count)
            .progress_fraction(progress)
            .play_label(Some(SharedString::new_static("Resume")))
            .on_open(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                let li = &open_entries[idx].item;
                this.open_meta_details(
                    li.r#type.clone(),
                    li.id.clone(),
                    li.state.video_id.clone(),
                    cx,
                );
            }))
            .on_quick_play(cx.listener(move |this, _, _, cx| {
                let li = &play_entries[idx].item;
                this.quick_resume_or_open(
                    li.r#type.clone(),
                    li.id.clone(),
                    li.state.video_id.clone(),
                    cx,
                );
            }))
            .on_quick_peek(cx.listener(move |this, _, _, cx| {
                let li = &peek_entries[idx].item;
                this.open_quick_peek(
                    QuickPeekTarget {
                        type_name: li.r#type.clone(),
                        id: li.id.clone(),
                        video_id: li.state.video_id.clone(),
                        title: li.name.clone(),
                        preview: None,
                    },
                    cx,
                );
            }))
            .on_context_menu(cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                let li = &ctx_entries[idx].item;
                let menu = BrowsingContextMenu::new(
                    ev.position,
                    li.r#type.clone(),
                    li.id.clone(),
                    li.name.clone(),
                )
                .video_id(li.state.video_id.clone())
                .is_in_library(true)
                .is_watched(li.watched())
                .can_rewind_cw(li.state.time_offset > 0);
                this.open_browsing_context_menu(menu, cx);
            }))
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                let li = &hover_entries[idx].item;
                this.on_card_hover_prefetch(
                    &li.r#type,
                    &li.id,
                    hover_bg.as_deref(),
                    *hovered,
                    cx,
                );
            }));

        if let Some(code) = ep_code {
            props = props.episode_badge(code, false);
        }

        (render_landscape_card(props), is_pending)
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

            let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut process_item = |li: &LibraryItem, forced_status: Option<LibraryStatusTab>| {
                let Some(status) = forced_status.or_else(|| classify_library_item_status(li))
                else {
                    return;
                };
                if !seen_ids.insert(li.id.clone()) {
                    return;
                }
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
                    process_item(li, None);
                }
            } else {
                for li in &model.library.catalog {
                    process_item(li, None);
                }
            }

            // Unify Library › Watching with Home Continue Watching so in-progress titles
            // watched without explicitly saving to Library are still reflected in Watching.
            for cw in &model.continue_watching_preview.items {
                process_item(&cw.library_item, Some(LibraryStatusTab::Watching));
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

    fn get_or_update_library_card_slab(
        &self,
        memo: &MemoizedLibraryView,
        aux: &CachedLibraryAux,
        view_mode: LibraryViewMode,
        aspect: PosterAspect,
        theme_preset: ThemePreset,
    ) -> Arc<CachedLibraryCardSlab> {
        let key = LibraryCardSlabKey {
            library_rev: memo.library_rev,
            notification_rev: memo.notification_rev,
            filter_tab: memo.filter_tab,
            type_filter: memo.type_filter,
            sort_criterion: memo.sort_criterion,
            view_mode,
            aspect,
            theme_preset,
            search_query: memo.search_query.clone(),
        };

        {
            let guard = self.memoized_library_cards.lock();
            if let Some(any_arc) = guard.as_ref() {
                if let Ok(cached) = Arc::clone(any_arc).downcast::<CachedLibraryCardSlab>() {
                    if cached.key == key {
                        return cached;
                    }
                }
            }
        }

        let items = &memo.filtered_items;
        let is_wide = view_mode == LibraryViewMode::WideBackdrops;
        let mut cards = Vec::with_capacity(items.len());
        let mut wide_urls = if is_wide {
            Vec::with_capacity(items.len())
        } else {
            Vec::new()
        };

        for (li, status) in items.iter() {
            let notif_count = aux.notif_counts.get(&li.id).copied().unwrap_or(0);
            let tint = placeholder_tint_for_id_with_preset(&li.id, theme_preset);
            let type_label = sentence_case_label(&li.r#type);
            let progress_fraction = library_item_progress_fraction(li, *status);
            let poster_url = li.poster.as_ref().map(|u| u.as_str());

            let (card_aspect, subtitle) = if is_wide {
                wide_urls.push(
                    self.resolve_library_widescreen_url(li)
                        .map(SharedString::from),
                );
                (
                    PosterAspect::Landscape16x9,
                    format_wide_card_subtitle(li, *status, notif_count),
                )
            } else {
                let sub: SharedString =
                    match format_movietracker_episode_code(li.state.video_id.as_deref()) {
                        Some(ep) => format!("{type_label} · {ep}").into(),
                        None => SharedString::from(type_label.clone()),
                    };
                (PosterAspect::from_stremio_poster_shape(&li.poster_shape), sub)
            };

            // Show corner status glyphs only when they add information (avoid putting a purple
            // Watchlist dot on every single saved poster).
            let visible_status_badge = match (self.library_filter_tab, *status) {
                (LibraryFilterTab::Watchlist, LibraryStatusTab::Watchlist) => None,
                (LibraryFilterTab::Watched, LibraryStatusTab::Watched) => None,
                (LibraryFilterTab::Watching, LibraryStatusTab::Watching) => None,
                (LibraryFilterTab::All, LibraryStatusTab::Watchlist) => None,
                (_, st) => Some(st),
            };

            cards.push(
                PreparedPosterCard::with_tint(SharedString::new(&li.name), tint)
                    .subtitle(Some(subtitle))
                    .type_badge(Some(SharedString::from(type_label)))
                    .poster_url_str(poster_url)
                    .aspect(card_aspect)
                    .status_tab(visible_status_badge)
                    .notification_count(notif_count)
                    .progress_fraction(progress_fraction)
                    .in_library(!li.removed && !li.temp)
                    .watched(li.watched())
                    .capabilities(PosterCardCapabilities::STANDARD_BROWSING),
            );
        }

        let slab = Arc::new(CachedLibraryCardSlab {
            key,
            cards: Arc::from(cards),
            wide_urls: Arc::from(wide_urls),
        });
        *self.memoized_library_cards.lock() =
            Some(Arc::clone(&slab) as Arc<dyn std::any::Any + Send + Sync>);
        slab
    }

    #[allow(clippy::too_many_arguments)]
    fn render_library_compact_row(
        &self,
        items: &Arc<Vec<(LibraryItem, LibraryStatusTab)>>,
        idx: usize,
        status: LibraryStatusTab,
        notification_count: usize,
        row_height_px: f32,
        is_focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let li = &items[idx].0;
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

        let open_items = Arc::clone(items);
        let ctx_items = Arc::clone(items);
        let hover_items = Arc::clone(items);
        let play_items = Arc::clone(items);
        let watch_items = Arc::clone(items);
        let peek_items = Arc::clone(items);

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
                    let li = &open_items[idx].0;
                    this.open_meta_details(
                        li.r#type.clone(),
                        li.id.clone(),
                        li.state.video_id.clone(),
                        cx,
                    );
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let li = &ctx_items[idx].0;
                    let menu = BrowsingContextMenu::new(
                        ev.position,
                        li.r#type.clone(),
                        li.id.clone(),
                        li.name.clone(),
                    )
                    .video_id(li.state.video_id.clone())
                    .is_in_library(true)
                    .is_watched(is_watched)
                    .can_rewind_cw(li.state.time_offset > 0);
                    this.open_browsing_context_menu(menu, cx);
                }),
            )
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                let li = &hover_items[idx].0;
                this.on_card_hover_prefetch(&li.r#type, &li.id, None, *hovered, cx);
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
                                    let li = &play_items[idx].0;
                                    this.quick_resume_or_open(
                                        li.r#type.clone(),
                                        li.id.clone(),
                                        li.state.video_id.clone(),
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
                                    let li = &watch_items[idx].0;
                                    let next_watched = !is_watched;
                                    this.core.mark_library_item_watched(
                                        li.id.clone(),
                                        next_watched,
                                    );
                                    this.push_toast(
                                        ToastKind::Info,
                                        if next_watched {
                                            "Marked as watched"
                                        } else {
                                            "Marked as unwatched"
                                        },
                                        Some(li.name.clone()),
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
                                    let li = &peek_items[idx].0;
                                    this.open_quick_peek(
                                        QuickPeekTarget {
                                            type_name: li.r#type.clone(),
                                            id: li.id.clone(),
                                            video_id: li.state.video_id.clone(),
                                            title: li.name.clone(),
                                            preview: None,
                                        },
                                        cx,
                                    );
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

fn build_library_card_dispatcher(
    items: &Arc<Vec<(LibraryItem, LibraryStatusTab)>>,
    wide_urls: Option<Arc<[Option<SharedString>]>>,
    cx: &mut Context<AppShell>,
) -> CardActionDispatcher {
    let items_for_dispatch = Arc::clone(items);
    let shell_weak = cx.entity().downgrade();
    card_action_dispatcher(move |idx, action, _window, cx| {
        let Some(shell) = shell_weak.upgrade() else {
            return;
        };
        let Some((li, _)) = items_for_dispatch.get(idx) else {
            return;
        };
        let hover_bg = wide_urls
            .as_ref()
            .and_then(|urls| urls.get(idx))
            .and_then(|u| u.as_deref());
        shell.update(cx, |this, cx| match action {
            PosterCardAction::Open | PosterCardAction::MiddleClick => {
                this.open_meta_details(
                    li.r#type.clone(),
                    li.id.clone(),
                    li.state.video_id.clone(),
                    cx,
                );
            }
            PosterCardAction::QuickPlay => {
                this.quick_resume_or_open(
                    li.r#type.clone(),
                    li.id.clone(),
                    li.state.video_id.clone(),
                    cx,
                );
            }
            PosterCardAction::ToggleLibrary => {
                this.core.remove_from_library(li.id.clone());
                this.push_toast(
                    ToastKind::Info,
                    "Removed from library",
                    Some(li.name.clone()),
                    cx,
                );
                cx.notify();
            }
            PosterCardAction::QuickPeek => {
                this.open_quick_peek(
                    QuickPeekTarget {
                        type_name: li.r#type.clone(),
                        id: li.id.clone(),
                        video_id: li.state.video_id.clone(),
                        title: li.name.clone(),
                        preview: None,
                    },
                    cx,
                );
            }
            PosterCardAction::ContextMenu(position) => {
                let menu = BrowsingContextMenu::new(
                    position,
                    li.r#type.clone(),
                    li.id.clone(),
                    li.name.clone(),
                )
                .video_id(li.state.video_id.clone())
                .is_in_library(true)
                .is_watched(li.watched())
                .can_rewind_cw(li.state.time_offset > 0);
                this.open_browsing_context_menu(menu, cx);
            }
            PosterCardAction::Hover(hovered) => {
                this.on_card_hover_prefetch(&li.r#type, &li.id, hover_bg, hovered, cx);
            }
        });
    })
}

fn build_library_toggle_watched_dispatcher(
    items: &Arc<Vec<(LibraryItem, LibraryStatusTab)>>,
    cards: &Arc<[PreparedPosterCard]>,
    cx: &mut Context<AppShell>,
) -> ToggleWatchedDispatcher {
    let items_for_watch = Arc::clone(items);
    let cards_for_watch = Arc::clone(cards);
    let shell_weak = cx.entity().downgrade();
    Rc::new(move |idx, _window, cx| {
        let Some(shell) = shell_weak.upgrade() else {
            return;
        };
        let Some((li, _)) = items_for_watch.get(idx) else {
            return;
        };
        let is_watched = cards_for_watch.get(idx).is_some_and(|c| c.watched);
        let next_watched = !is_watched;
        shell.update(cx, |this, cx| {
            this.core
                .mark_library_item_watched(li.id.clone(), next_watched);
            this.push_toast(
                ToastKind::Info,
                if next_watched {
                    "Marked as watched"
                } else {
                    "Marked as unwatched"
                },
                Some(li.name.clone()),
                cx,
            );
            cx.notify();
        });
    })
}

fn build_card_toggle_watched_button(
    id_prefix: &'static str,
    idx: usize,
    is_watched: bool,
    theme: &ThemePalette,
    dispatcher: &ToggleWatchedDispatcher,
) -> AnyElement {
    let theme = *theme;
    let badge_fg = if theme.is_light {
        0xFFFFFF
    } else {
        theme.bg_canvas
    };
    let dispatch = Rc::clone(dispatcher);

    div()
        .id((id_prefix, idx))
        .cursor_pointer()
        .w(sp(30.0))
        .h(sp(30.0))
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
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            dispatch(idx, window, cx);
        })
        .child(svg_icon(
            IconName::Check,
            13.0 * theme.ui_scale,
            if is_watched {
                rgb(badge_fg)
            } else {
                rgb(0xF5F1E8)
            },
        ))
        .into_any_element()
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

#[allow(clippy::too_many_arguments)]
fn render_library_stat_pill(
    id: &'static str,
    icon: IconName,
    icon_weight: IconWeight,
    icon_color_hex: u32,
    value: String,
    label: &'static str,
    active: bool,
    on_click: Option<ClickHandler>,
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

    let mut actions: Vec<AnyElement> = Vec::new();
    if has_search_query || has_type_filter {
        actions.push(
            primary_accent_button(
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
            )
            .into_any_element(),
        );
    } else {
        if active_tab != LibraryFilterTab::All && total_library_count > 0 {
            actions.push(
                secondary_glass_button(
                    "lib-empty-show-all",
                    Some(IconName::Bookmark),
                    "View all titles",
                    false,
                    cx.listener(|this, _, _, cx| {
                        this.select_library_filter_tab(LibraryFilterTab::All, cx);
                    }),
                )
                .into_any_element(),
            );
        }
        actions.push(
            primary_accent_button(
                "lib-empty-discover",
                Some(IconName::Compass),
                "Browse Discover",
                cx.listener(|this, _, _, cx| {
                    this.navigate_tab(SidebarTab::Discover, cx);
                }),
            )
            .into_any_element(),
        );
    }

    render_empty_state(
        EmptyStateProps::new(
            if has_search_query {
                IconName::Search
            } else {
                IconName::Bookmark
            },
            empty_title,
            empty_sub,
        )
        .fill_available(true)
        .actions(actions),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

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
