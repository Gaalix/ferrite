use std::{ops::Range, sync::Arc, time::Duration};

use gpui::{
    deferred, div, point, prelude::*, px, relative, rgb, rgba, Animation, AnimationExt,
    AnyElement, Context, ElementId, FontWeight, MouseButton, MouseDownEvent, SharedString, Window,
};
use stremio_core_runtime::{
    stremio_core::{
        models::catalog_with_filters::Selected as CatalogWithFiltersSelected,
        runtime::msg::{Action, ActionCatalogWithFilters},
        types::{
            addon::ResourceRequest,
            resource::{MetaItemPreview, PosterShape},
        },
    },
    DesktopModelField,
};

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{
        extract_genres, extract_imdb_rating, AppRoute, BrowsingContextMenu, DiscoverDropdown,
        FocusedInput, KbFocusableCard, LibraryStatusTab, ToastKind,
    },
    theme::{
        editorial_serif, meta_divider, mono_font, sentence_case_label, ui_sans, ACCENT,
        ACCENT_SOFT, BORDER_ACCENT, GLASS_BORDER, GLASS_BORDER_STRONG, GLASS_CARD_BG,
        GLASS_DIVIDER, GLASS_ROW_HOVER_BG, GLASS_ROW_SELECTED_BG, GLASS_TROUGH_BG, RADIUS_CARD,
        RADIUS_CONTROL, RADIUS_PILL, RADIUS_POSTER, TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY,
    },
    ui_components::{
        click_handler, glass_dropdown_option, glass_dropdown_panel, glass_segmented_bar,
        glass_segmented_item, hover_handler, primary_accent_button, render_poster_card,
        PosterCardProps,
    },
    virtual_grid::{compute_dynamic_grid_metrics, slice_row_items, PosterAspect},
};

struct ActiveDiscoverFilterChip {
    label: String,
    reset_request: Option<ResourceRequest>,
}

impl AppShell {
    pub fn render_discover_view(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snap = self.cached_discover_snapshot();
        let statuses_arc = self.cached_library_statuses();

        let types = snap.types;
        let catalogs = snap.catalogs;
        let extras = snap.extras;
        let has_next_page = snap.has_next_page;
        let items_arc = Arc::clone(&snap.items);
        let is_loading = snap.is_loading;
        let active_catalog_display = snap.active_catalog_display;
        let active_genre_label = snap.active_genre_label;

        let genre_extra = extras
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case("genre"))
            .cloned();

        let selected_catalog_default_request = catalogs
            .iter()
            .find(|c| c.catalog.selected)
            .or_else(|| catalogs.first())
            .map(|c| c.catalog.request.clone());

        let active_extra_chips: Vec<ActiveDiscoverFilterChip> = extras
            .iter()
            .filter_map(|extra| {
                let selected_pos = extra.options.iter().position(|o| o.selected)?;
                let selected_opt = &extra.options[selected_pos];
                let val = selected_opt.value.as_ref()?;
                if extra.is_required && selected_pos == 0 {
                    return None;
                }
                let reset_request = if extra.is_required {
                    extra.options.first().map(|o| o.request.clone())
                } else {
                    extra
                        .options
                        .iter()
                        .find(|o| o.value.is_none())
                        .map(|o| o.request.clone())
                        .or_else(|| selected_catalog_default_request.clone())
                };
                Some(ActiveDiscoverFilterChip {
                    label: format!("{}: {}", sentence_case_label(&extra.name), val),
                    reset_request,
                })
            })
            .collect();

        let cat_query_trimmed = self.discover_catalog_search.trim().to_ascii_lowercase();
        let has_cat_query = !self.discover_catalog_search.is_empty();
        let has_active_filters = !active_extra_chips.is_empty() || !cat_query_trimmed.is_empty();

        let open_dd = self.discover_open_dropdown.clone();
        let catalog_open = open_dd == Some(DiscoverDropdown::Catalog);
        let genre_open = open_dd == Some(DiscoverDropdown::Genre);
        let cat_search_focused = self.focused_input == Some(FocusedInput::DiscoverCatalogSearch);
        let total_catalogs_count = catalogs.len();
        let total_items = items_arc.len();

        let raw_viewport_w = f32::from(window.viewport_size().width);
        let viewport_w = if raw_viewport_w > 0.0 && raw_viewport_w.is_finite() {
            raw_viewport_w
        } else {
            self.viewport_size_px.0
        };

        let grid_aspect = infer_discover_grid_aspect(&items_arc);
        let metrics = compute_dynamic_grid_metrics(
            total_items,
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

        let kb_items: Vec<KbFocusableCard> = items_arc
            .iter()
            .enumerate()
            .map(|(idx, item)| KbFocusableCard {
                route: AppRoute::MetaDetails {
                    type_name: item.r#type.clone(),
                    id: item.id.clone(),
                    video_id: None,
                },
                row: idx / columns,
                col: idx % columns,
            })
            .collect();
        self.set_kb_focus_items(kb_items);

        let extra_dropdowns: Vec<_> = extras
            .into_iter()
            .filter(|e| !e.name.eq_ignore_ascii_case("genre") && !e.options.is_empty())
            .collect();

        let cat_search_display = self.format_input_with_caret(
            FocusedInput::DiscoverCatalogSearch,
            &self.discover_catalog_search,
        );

        let reset_all_req = selected_catalog_default_request.clone();
        let empty_reset_req = selected_catalog_default_request;

        div()
            .id("discover-view-root")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    let mut changed = false;
                    if this.discover_open_dropdown.is_some() {
                        this.discover_open_dropdown = None;
                        changed = true;
                    }
                    if this.focused_input == Some(FocusedInput::DiscoverCatalogSearch) {
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
                            .items_end()
                            .justify_between()
                            .gap(px(16.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .font(editorial_serif())
                                            .text_size(px(56.0))
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child("Discover"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(14.5))
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(
                                                "Browse films, series, and curated shelves across your installed catalogs.",
                                            ),
                                    ),
                            )
                            .when(is_loading, |el| {
                                el.child(
                                    div()
                                        .h(px(28.0))
                                        .px(px(12.0))
                                        .rounded(px(RADIUS_PILL))
                                        .bg(rgba(ACCENT_SOFT))
                                        .border_1()
                                        .border_color(rgba(BORDER_ACCENT))
                                        .flex()
                                        .items_center()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(ACCENT))
                                        .child("Loading catalog..."),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(14.0))
                            .py(px(12.0))
                            .rounded(px(RADIUS_CARD))
                            .bg(rgba(GLASS_CARD_BG))
                            .border_1()
                            .border_color(rgba(GLASS_BORDER))
                            .flex()
                            .flex_col()
                            .gap(px(10.0))
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
                                            .items_center()
                                            .gap(px(10.0))
                                            .when(!types.is_empty(), |bar| {
                                                bar.child(
                                                    glass_segmented_bar("discover-type-bar").children(
                                                        types.into_iter().enumerate().map(|(idx, t)| {
                                                            let req = t.request;
                                                            let selected = t.selected;
                                                            let label = sentence_case_label(&t.r#type);
                                                            glass_segmented_item(
                                                                ("discover-type-tab", idx),
                                                                label,
                                                                selected,
                                                                cx.listener(move |this, _, _, cx| {
                                                                    this.apply_discover_selection(
                                                                        Some(CatalogWithFiltersSelected {
                                                                            request: req.clone(),
                                                                        }),
                                                                        cx,
                                                                    );
                                                                }),
                                                            )
                                                        }),
                                                    ),
                                                )
                                            })
                                            .when(!catalogs.is_empty(), |bar| {
                                                let filtered_catalogs: Vec<_> = catalogs
                                                    .into_iter()
                                                    .filter(|opt| {
                                                        if cat_query_trimmed.is_empty() {
                                                            true
                                                        } else {
                                                            opt.title
                                                                .to_ascii_lowercase()
                                                                .contains(&cat_query_trimmed)
                                                                || opt.addon_name.as_ref().is_some_and(|a| {
                                                                    a.to_ascii_lowercase()
                                                                        .contains(&cat_query_trimmed)
                                                                })
                                                        }
                                                    })
                                                    .collect();

                                                bar.child(
                                                    div()
                                                        .relative()
                                                        .child(render_discover_dropdown_trigger(
                                                            "discover-dd-catalog",
                                                            "Catalog",
                                                            active_catalog_display,
                                                            catalog_open,
                                                            false,
                                                            cx.listener(|this, _, _, cx| {
                                                                if this.discover_open_dropdown
                                                                    == Some(DiscoverDropdown::Catalog)
                                                                {
                                                                    this.discover_open_dropdown = None;
                                                                    if this.focused_input
                                                                        == Some(
                                                                            FocusedInput::DiscoverCatalogSearch,
                                                                        )
                                                                    {
                                                                        this.focused_input = None;
                                                                    }
                                                                } else {
                                                                    this.discover_open_dropdown =
                                                                        Some(DiscoverDropdown::Catalog);
                                                                    this.focused_input = Some(
                                                                        FocusedInput::DiscoverCatalogSearch,
                                                                    );
                                                                }
                                                                cx.notify();
                                                            }),
                                                        ))
                                                        .when(catalog_open, |wrap| {
                                                            wrap.child(
                                                                deferred(
                                                                    glass_dropdown_panel(
                                                                        "discover-menu-catalog",
                                                                        364.0,
                                                                    )
                                                                    .absolute()
                                                                    .top(px(44.0))
                                                                    .left_0()
                                                                    .gap(px(6.0))
                                                                    .child(
                                                                        div()
                                                                            .id("discover-cat-search-box")
                                                                            .cursor_pointer()
                                                                            .h(px(34.0))
                                                                            .px(px(10.0))
                                                                            .rounded(px(RADIUS_POSTER))
                                                                            .bg(if cat_search_focused {
                                                                                rgba(GLASS_ROW_SELECTED_BG)
                                                                            } else {
                                                                                rgba(GLASS_TROUGH_BG)
                                                                            })
                                                                            .border_1()
                                                                            .border_color(
                                                                                if cat_search_focused {
                                                                                    rgba(BORDER_ACCENT)
                                                                                } else {
                                                                                    rgba(GLASS_BORDER)
                                                                                },
                                                                            )
                                                                            .flex()
                                                                            .items_center()
                                                                            .justify_between()
                                                                            .gap(px(8.0))
                                                                            .on_mouse_down(
                                                                                MouseButton::Left,
                                                                                cx.listener(
                                                                                    |this, _, _, cx| {
                                                                                        cx.stop_propagation();
                                                                                        this.focused_input = Some(
                                                                                            FocusedInput::DiscoverCatalogSearch,
                                                                                        );
                                                                                        cx.notify();
                                                                                    },
                                                                                ),
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
                                                                                        13.0,
                                                                                        if cat_search_focused {
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
                                                                                            .text_xs()
                                                                                            .text_color(
                                                                                                if has_cat_query {
                                                                                                    rgb(TEXT_PRIMARY)
                                                                                                } else {
                                                                                                    rgb(TEXT_MUTED)
                                                                                                },
                                                                                            )
                                                                                            .truncate()
                                                                                            .when(
                                                                                                cat_search_focused
                                                                                                    || has_cat_query,
                                                                                                |d| {
                                                                                                    d.child(
                                                                                                        cat_search_display
                                                                                                            .clone(),
                                                                                                    )
                                                                                                },
                                                                                            )
                                                                                            .when(
                                                                                                !has_cat_query,
                                                                                                |d| {
                                                                                                    d.child(
                                                                                                        div()
                                                                                                            .when(
                                                                                                                cat_search_focused,
                                                                                                                |p| p.pl(px(3.0)),
                                                                                                            )
                                                                                                            .text_color(rgb(
                                                                                                                TEXT_MUTED,
                                                                                                            ))
                                                                                                            .child("Filter catalogs or addons..."),
                                                                                                    )
                                                                                                },
                                                                                            ),
                                                                                    ),
                                                                            )
                                                                            .when(has_cat_query, |sb| {
                                                                                sb.child(
                                                                                    div()
                                                                                        .id("discover-cat-search-clear")
                                                                                        .cursor_pointer()
                                                                                        .p(px(3.0))
                                                                                        .rounded(px(4.0))
                                                                                        .hover(|s| {
                                                                                            s.bg(rgba(
                                                                                                GLASS_ROW_HOVER_BG,
                                                                                            ))
                                                                                        })
                                                                                        .on_mouse_down(
                                                                                            MouseButton::Left,
                                                                                            cx.listener(
                                                                                                |this,
                                                                                                 _,
                                                                                                 _,
                                                                                                 cx| {
                                                                                                    cx.stop_propagation();
                                                                                                    this.discover_catalog_search
                                                                                                        .clear();
                                                                                                    this.input_cursors
                                                                                                        .remove(&FocusedInput::DiscoverCatalogSearch);
                                                                                                    this.input_selected_all
                                                                                                        .remove(&FocusedInput::DiscoverCatalogSearch);
                                                                                                    cx.notify();
                                                                                                },
                                                                                            ),
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
                                                                        div()
                                                                            .id("discover-cat-list-scroll")
                                                                            .w_full()
                                                                            .max_h(px(320.0))
                                                                            .overflow_y_scroll()
                                                                            .child(
                                                                                div()
                                                                                    .w_full()
                                                                                    .flex()
                                                                                    .flex_col()
                                                                                    .gap(px(2.0))
                                                                                    .children(
                                                                                        if filtered_catalogs
                                                                                            .is_empty()
                                                                                        {
                                                                                            vec![div()
                                                                                                .flex_shrink_0()
                                                                                                .px(px(10.0))
                                                                                                .py(px(12.0))
                                                                                                .text_xs()
                                                                                                .text_color(rgb(
                                                                                                    TEXT_MUTED,
                                                                                                ))
                                                                                                .child("No catalogs match your filter.")
                                                                                                .into_any_element()]
                                                                                        } else {
                                                                                            filtered_catalogs
                                                                                                .into_iter()
                                                                                                .enumerate()
                                                                                                .map(|(idx, opt)| {
                                                                                                    let req =
                                                                                                        opt.catalog.request;
                                                                                                    let selected =
                                                                                                        opt.catalog.selected;
                                                                                                    glass_dropdown_option(
                                                                                                        ("discover-opt-cat", idx),
                                                                                                        opt.title,
                                                                                                        opt.addon_name.map(
                                                                                                            SharedString::from,
                                                                                                        ),
                                                                                                        selected,
                                                                                                        cx.listener(
                                                                                                            move |this, _, _, cx| {
                                                                                                                this.apply_discover_selection(
                                                                                                                    Some(CatalogWithFiltersSelected {
                                                                                                                        request: req.clone(),
                                                                                                                    }),
                                                                                                                    cx,
                                                                                                                );
                                                                                                            },
                                                                                                        ),
                                                                                                    )
                                                                                                    .into_any_element()
                                                                                                })
                                                                                                .collect()
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
                                            .when_some(
                                                genre_extra.filter(|g| !g.options.is_empty()),
                                                |bar, genre| {
                                                    let genre_filtered = genre
                                                        .options
                                                        .iter()
                                                        .find(|o| o.selected)
                                                        .is_some_and(|o| o.value.is_some());
                                                    let opts = genre.options;
                                                    bar.child(
                                                        div()
                                                            .relative()
                                                            .child(render_discover_dropdown_trigger(
                                                                "discover-dd-genre",
                                                                "Genre",
                                                                active_genre_label,
                                                                genre_open,
                                                                genre_filtered,
                                                                cx.listener(|this, _, _, cx| {
                                                                    if this.discover_open_dropdown
                                                                        == Some(DiscoverDropdown::Genre)
                                                                    {
                                                                        this.discover_open_dropdown = None;
                                                                    } else {
                                                                        this.discover_open_dropdown =
                                                                            Some(DiscoverDropdown::Genre);
                                                                        if this.focused_input
                                                                            == Some(
                                                                                FocusedInput::DiscoverCatalogSearch,
                                                                            )
                                                                        {
                                                                            this.focused_input = None;
                                                                        }
                                                                    }
                                                                    cx.notify();
                                                                }),
                                                            ))
                                                            .when(genre_open, |wrap| {
                                                                wrap.child(
                                                                    deferred(
                                                                        glass_dropdown_panel(
                                                                            "discover-menu-genre",
                                                                            256.0,
                                                                        )
                                                                        .absolute()
                                                                        .top(px(44.0))
                                                                        .left_0()
                                                                        .child(
                                                                            div()
                                                                                .id("discover-genre-list-scroll")
                                                                                .w_full()
                                                                                .max_h(px(340.0))
                                                                                .overflow_y_scroll()
                                                                                .child(
                                                                                    div()
                                                                                        .w_full()
                                                                                        .flex()
                                                                                        .flex_col()
                                                                                        .gap(px(2.0))
                                                                                        .children(
                                                                                            opts.into_iter()
                                                                                                .enumerate()
                                                                                                .map(|(idx, opt)| {
                                                                                                    let req =
                                                                                                        opt.request;
                                                                                                    let selected =
                                                                                                        opt.selected;
                                                                                                    let label = opt
                                                                                                        .value
                                                                                                        .unwrap_or_else(
                                                                                                            || {
                                                                                                                "All genres"
                                                                                                                    .to_owned()
                                                                                                            },
                                                                                                        );
                                                                                                    glass_dropdown_option(
                                                                                                        (
                                                                                                            "discover-opt-genre",
                                                                                                            idx,
                                                                                                        ),
                                                                                                        label,
                                                                                                        None,
                                                                                                        selected,
                                                                                                        cx.listener(
                                                                                                            move |this,
                                                                                                                  _,
                                                                                                                  _,
                                                                                                                  cx| {
                                                                                                                this.apply_discover_selection(
                                                                                                                    Some(CatalogWithFiltersSelected {
                                                                                                                        request: req.clone(),
                                                                                                                    }),
                                                                                                                    cx,
                                                                                                                );
                                                                                                            },
                                                                                                        ),
                                                                                                    )
                                                                                                }),
                                                                                        ),
                                                                                ),
                                                                        ),
                                                                    )
                                                                    .with_priority(10),
                                                                )
                                                            }),
                                                    )
                                                },
                                            )
                                            .children(extra_dropdowns.into_iter().enumerate().map(
                                                |(e_idx, extra)| {
                                                    let extra_name = extra.name.clone();
                                                    let dd_variant =
                                                        DiscoverDropdown::Extra(extra_name.clone());
                                                    let is_open = open_dd.as_ref() == Some(&dd_variant);
                                                    let active_val = extra
                                                        .options
                                                        .iter()
                                                        .find(|o| o.selected)
                                                        .and_then(|o| o.value.clone())
                                                        .unwrap_or_else(|| "All".to_owned());
                                                    let is_filtered = extra
                                                        .options
                                                        .iter()
                                                        .find(|o| o.selected)
                                                        .is_some_and(|o| o.value.is_some());
                                                    let field_title = sentence_case_label(&extra_name);
                                                    let dd_for_click = dd_variant;

                                                    div()
                                                        .relative()
                                                        .child(render_discover_dropdown_trigger(
                                                            ("discover-dd-extra", e_idx),
                                                            field_title,
                                                            active_val,
                                                            is_open,
                                                            is_filtered,
                                                            cx.listener(move |this, _, _, cx| {
                                                                if this.discover_open_dropdown.as_ref()
                                                                    == Some(&dd_for_click)
                                                                {
                                                                    this.discover_open_dropdown = None;
                                                                } else {
                                                                    this.discover_open_dropdown =
                                                                        Some(dd_for_click.clone());
                                                                    if this.focused_input
                                                                        == Some(
                                                                            FocusedInput::DiscoverCatalogSearch,
                                                                        )
                                                                    {
                                                                        this.focused_input = None;
                                                                    }
                                                                }
                                                                cx.notify();
                                                            }),
                                                        ))
                                                        .when(is_open, |wrap| {
                                                            wrap.child(
                                                                deferred(
                                                                    glass_dropdown_panel(
                                                                        ("discover-menu-extra", e_idx),
                                                                        240.0,
                                                                    )
                                                                    .absolute()
                                                                    .top(px(44.0))
                                                                    .left_0()
                                                                    .child(
                                                                        div()
                                                                            .id((
                                                                                "discover-extra-list-scroll",
                                                                                e_idx,
                                                                            ))
                                                                            .w_full()
                                                                            .max_h(px(320.0))
                                                                            .overflow_y_scroll()
                                                                            .child(
                                                                                div()
                                                                                    .w_full()
                                                                                    .flex()
                                                                                    .flex_col()
                                                                                    .gap(px(2.0))
                                                                                    .children(
                                                                                        extra
                                                                                            .options
                                                                                            .into_iter()
                                                                                            .enumerate()
                                                                                            .map(|(o_idx, opt)| {
                                                                                                let req =
                                                                                                    opt.request;
                                                                                                let selected =
                                                                                                    opt.selected;
                                                                                                let label = opt
                                                                                                    .value
                                                                                                    .unwrap_or_else(
                                                                                                        || {
                                                                                                            "All"
                                                                                                                .to_owned()
                                                                                                        },
                                                                                                    );
                                                                                                glass_dropdown_option(
                                                                                                    (
                                                                                                        "discover-opt-extra",
                                                                                                        e_idx * 1000
                                                                                                            + o_idx,
                                                                                                    ),
                                                                                                    label,
                                                                                                    None,
                                                                                                    selected,
                                                                                                    cx.listener(
                                                                                                        move |this,
                                                                                                              _,
                                                                                                              _,
                                                                                                              cx| {
                                                                                                            this.apply_discover_selection(
                                                                                                                Some(CatalogWithFiltersSelected {
                                                                                                                    request: req.clone(),
                                                                                                                }),
                                                                                                                cx,
                                                                                                            );
                                                                                                        },
                                                                                                    ),
                                                                                                )
                                                                                            }),
                                                                                    ),
                                                                            ),
                                                                    ),
                                                                )
                                                                .with_priority(10),
                                                            )
                                                        })
                                                },
                                            )),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .font(mono_font())
                                            .text_xs()
                                            .text_color(rgb(TEXT_MUTED))
                                            .child(format!("{total_items} titles"))
                                            .when(total_catalogs_count > 0, |row| {
                                                row.child(meta_divider())
                                                    .child(format!("{total_catalogs_count} catalogs"))
                                            }),
                                    ),
                            )
                            .when(has_active_filters, |bar| {
                                bar.child(
                                    div()
                                        .pt(px(8.0))
                                        .border_t_1()
                                        .border_color(rgba(GLASS_DIVIDER))
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap(px(8.0))
                                        .children(active_extra_chips.into_iter().enumerate().map(
                                            |(idx, chip)| {
                                                let reset_req = chip.reset_request;
                                                div()
                                                    .id(("discover-active-chip", idx))
                                                    .cursor_pointer()
                                                    .h(px(28.0))
                                                    .px(px(10.0))
                                                    .rounded(px(RADIUS_PILL))
                                                    .bg(rgba(ACCENT_SOFT))
                                                    .border_1()
                                                    .border_color(rgba(BORDER_ACCENT))
                                                    .hover(|s| s.border_color(rgb(ACCENT)))
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(6.0))
                                                    .font(ui_sans(FontWeight::MEDIUM))
                                                    .text_size(px(12.0))
                                                    .text_color(rgb(ACCENT))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            this.apply_discover_selection(
                                                                reset_req.clone().map(|request| {
                                                                    CatalogWithFiltersSelected {
                                                                        request,
                                                                    }
                                                                }),
                                                                cx,
                                                            );
                                                        }),
                                                    )
                                                    .child(chip.label)
                                                    .child(svg_icon(
                                                        IconName::X,
                                                        11.5,
                                                        rgb(ACCENT),
                                                    ))
                                            },
                                        ))
                                        .when(!cat_query_trimmed.is_empty(), |row| {
                                            let query_chip = format!(
                                                "Catalog filter: \"{}\"",
                                                self.discover_catalog_search.trim()
                                            );
                                            row.child(
                                                div()
                                                    .id("discover-active-cat-query-chip")
                                                    .cursor_pointer()
                                                    .h(px(28.0))
                                                    .px(px(10.0))
                                                    .rounded(px(RADIUS_PILL))
                                                    .bg(rgba(ACCENT_SOFT))
                                                    .border_1()
                                                    .border_color(rgba(BORDER_ACCENT))
                                                    .hover(|s| s.border_color(rgb(ACCENT)))
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(6.0))
                                                    .font(ui_sans(FontWeight::MEDIUM))
                                                    .text_size(px(12.0))
                                                    .text_color(rgb(ACCENT))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            this.discover_catalog_search.clear();
                                                            this.input_cursors.remove(
                                                                &FocusedInput::DiscoverCatalogSearch,
                                                            );
                                                            this.input_selected_all.remove(
                                                                &FocusedInput::DiscoverCatalogSearch,
                                                            );
                                                            cx.notify();
                                                        }),
                                                    )
                                                    .child(query_chip)
                                                    .child(svg_icon(
                                                        IconName::X,
                                                        11.5,
                                                        rgb(ACCENT),
                                                    )),
                                            )
                                        })
                                        .child(
                                            div()
                                                .id("discover-reset-filters")
                                                .cursor_pointer()
                                                .h(px(28.0))
                                                .px(px(11.0))
                                                .rounded(px(RADIUS_PILL))
                                                .bg(rgba(GLASS_TROUGH_BG))
                                                .border_1()
                                                .border_color(rgba(GLASS_BORDER))
                                                .hover(|s| {
                                                    s.bg(rgba(GLASS_ROW_HOVER_BG))
                                                        .border_color(rgba(GLASS_BORDER_STRONG))
                                                        .text_color(rgb(TEXT_PRIMARY))
                                                })
                                                .flex()
                                                .items_center()
                                                .gap(px(6.0))
                                                .font(ui_sans(FontWeight::MEDIUM))
                                                .text_size(px(12.0))
                                                .text_color(rgb(TEXT_SECONDARY))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.discover_catalog_search.clear();
                                                        this.input_cursors.remove(
                                                            &FocusedInput::DiscoverCatalogSearch,
                                                        );
                                                        this.input_selected_all.remove(
                                                            &FocusedInput::DiscoverCatalogSearch,
                                                        );
                                                        this.apply_discover_selection(
                                                            reset_all_req.clone().map(|request| {
                                                                CatalogWithFiltersSelected { request }
                                                            }),
                                                            cx,
                                                        );
                                                    }),
                                                )
                                                .child(svg_icon(
                                                    IconName::RefreshCw,
                                                    11.5,
                                                    rgb(TEXT_SECONDARY),
                                                ))
                                                .child("Reset filters"),
                                        ),
                                )
                            }),
                    )
                    .child(if is_loading && total_items == 0 {
                        render_discover_skeleton_grid(columns, image_height_px, row_stride_px)
                    } else if total_items == 0 {
                        render_discover_empty_state(has_active_filters, empty_reset_req, cx)
                    } else {
                        gpui::uniform_list(
                            "discover-poster-grid",
                            row_count,
                            cx.processor(move |this, visible_rows: Range<usize>, _window, cx| {
                                if row_count > 0
                                    && !visible_rows.is_empty()
                                    && visible_rows.end + 2 >= row_count
                                    && has_next_page
                                    && !is_loading
                                    && this.discover_last_next_page_len.get() != Some(total_items)
                                {
                                    this.discover_last_next_page_len.set(Some(total_items));
                                    this.core.dispatch(
                                        Action::CatalogWithFilters(
                                            ActionCatalogWithFilters::LoadNextPage,
                                        ),
                                        Some(DesktopModelField::Discover),
                                    );
                                }

                                let kb_focus_index = this.kb_focus_index;

                                visible_rows
                                    .map(|row_idx| {
                                        let range = slice_row_items(total_items, columns, row_idx);
                                        let empty_slots = columns.saturating_sub(range.len());

                                        div()
                                            .id(("discover-row", row_idx))
                                            .w_full()
                                            .h(px(row_stride_px))
                                            .pb(px(18.0))
                                            .flex()
                                            .items_start()
                                            .gap(px(18.0))
                                            .children(range.map(|item_idx| {
                                                let item = items_arc[item_idx].clone();
                                                let st = statuses_arc.get(&item.id).copied();
                                                let is_focused = kb_focus_index == Some(item_idx);
                                                let card_aspect =
                                                    PosterAspect::from_stremio_poster_shape(
                                                        &item.poster_shape,
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
                                                    .child(this.render_discover_grid_card(
                                                        item_idx,
                                                        item,
                                                        st,
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
                        .track_scroll(&self.discover_grid_scroll_handle)
                        .flex_1()
                        .w_full()
                        .min_h(px(0.0))
                        .into_any_element()
                    }),
            )
    }

    fn apply_discover_selection(
        &mut self,
        selected: Option<CatalogWithFiltersSelected>,
        cx: &mut Context<Self>,
    ) {
        self.discover_open_dropdown = None;
        if self.focused_input == Some(FocusedInput::DiscoverCatalogSearch) {
            self.focused_input = None;
        }
        self.kb_focus_index = None;
        self.discover_last_next_page_len.set(None);
        self.discover_grid_scroll_handle
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(0.0)));
        self.core.load_discover(selected);
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn render_discover_grid_card(
        &self,
        item_idx: usize,
        item: MetaItemPreview,
        lib_status: Option<LibraryStatusTab>,
        aspect: PosterAspect,
        image_height_px: f32,
        is_focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let poster_tex = self
            .poster_cache
            .resolve_poster_url(item.poster.as_ref().or(item.background.as_ref()));
        let rating = extract_imdb_rating(&item).map(SharedString::from);
        let subtitle = format_discover_card_subtitle(&item);

        let open_type = item.r#type.clone();
        let open_id = item.id.clone();

        let play_type = item.r#type.clone();
        let play_id = item.id.clone();

        let toggle_preview = item.clone();
        let toggle_name = item.name.clone();
        let in_library = lib_status.is_some();

        let hover_type = item.r#type.clone();
        let hover_id = item.id.clone();
        let hover_bg = item.background.as_ref().map(|u| u.to_string());

        let ctx_preview = item.clone();
        let ctx_type = item.r#type.clone();
        let ctx_id = item.id.clone();
        let ctx_name = item.name.clone();

        render_poster_card(
            PosterCardProps::new(("discover-card", item_idx), item.name, item.id)
                .subtitle(Some(subtitle))
                .image(poster_tex)
                .aspect(aspect)
                .image_height_px(Some(image_height_px))
                .status_tab(lib_status)
                .imdb_rating(rating)
                .in_library(in_library)
                .is_focused(is_focused)
                .on_open(click_handler(cx.listener(move |this, _, _, cx| {
                    this.open_meta_details(open_type.clone(), open_id.clone(), None, cx);
                })))
                .on_quick_play(click_handler(cx.listener(move |this, _, _, cx| {
                    this.quick_resume_or_open(play_type.clone(), play_id.clone(), None, cx);
                })))
                .on_toggle_library(click_handler(cx.listener(move |this, _, _, cx| {
                    this.core.toggle_library_item(toggle_preview.clone());
                    let (kind, msg) = if in_library {
                        (ToastKind::Info, "Removed from library")
                    } else {
                        (ToastKind::Success, "Added to library")
                    };
                    this.push_toast(kind, msg, Some(toggle_name.clone()), cx);
                    cx.notify();
                })))
                .on_context_menu(click_handler(cx.listener(
                    move |this, ev: &MouseDownEvent, _, cx| {
                    let (lib_in, is_watched, can_rewind, vid) = this.core.with_model(|m| {
                        m.ctx
                            .library
                            .items
                            .get(&ctx_id)
                            .filter(|li| !li.removed)
                            .map(|li| {
                                (
                                    !li.temp,
                                    li.watched(),
                                    li.state.time_offset > 0,
                                    li.state.video_id.clone(),
                                )
                            })
                            .unwrap_or((false, false, false, None))
                    });
                    let menu = BrowsingContextMenu::new(
                        ev.position,
                        ctx_type.clone(),
                        ctx_id.clone(),
                        ctx_name.clone(),
                    )
                    .video_id(vid)
                    .preview(Some(ctx_preview.clone()))
                    .is_in_library(lib_in)
                    .is_watched(is_watched)
                    .can_rewind_cw(can_rewind);
                    this.open_browsing_context_menu(menu, cx);
                })))
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
}

fn infer_discover_grid_aspect(items: &[MetaItemPreview]) -> PosterAspect {
    if items.is_empty() {
        return PosterAspect::Portrait2x3;
    }
    let mut has_portrait = false;
    let mut has_square = false;
    let mut has_landscape = false;
    for item in items.iter().take(24) {
        match item.poster_shape {
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

fn format_discover_card_subtitle(item: &MetaItemPreview) -> SharedString {
    let primary = item
        .release_info
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| sentence_case_label(&item.r#type));
    match extract_genres(item, 1).into_iter().next() {
        Some(genre) => SharedString::from(format!("{primary} · {genre}")),
        None => SharedString::from(primary),
    }
}

fn render_discover_skeleton_grid(
    columns: usize,
    image_height_px: f32,
    row_stride_px: f32,
) -> AnyElement {
    let cols = columns.max(1);
    let shimmer_anim = Animation::new(Duration::from_millis(1300))
        .repeat_synced()
        .with_max_fps(30.0)
        .with_easing(|t| 0.45 + 0.55 * (0.5 - 0.5 * (t * std::f32::consts::TAU).cos()));

    div()
        .id("discover-skeleton-grid")
        .flex_1()
        .w_full()
        .min_h(px(0.0))
        .overflow_hidden()
        .flex()
        .flex_col()
        .children((0..3_usize).map(move |row_idx| {
            let row_anim = shimmer_anim.clone();
            div()
                .id(("discover-skel-row", row_idx))
                .w_full()
                .h(px(row_stride_px))
                .pb(px(18.0))
                .flex()
                .items_start()
                .gap(px(18.0))
                .children((0..cols).map(move |col_idx| {
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(
                            div()
                                .w_full()
                                .h(px(image_height_px))
                                .rounded(px(RADIUS_POSTER))
                                .bg(rgba(GLASS_CARD_BG))
                                .border_1()
                                .border_color(rgba(GLASS_BORDER)),
                        )
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .h(px(14.0))
                                        .w(relative(0.76))
                                        .rounded(px(4.0))
                                        .bg(rgba(GLASS_TROUGH_BG)),
                                )
                                .child(
                                    div()
                                        .h(px(11.0))
                                        .w(relative(0.46))
                                        .rounded(px(4.0))
                                        .bg(rgba(GLASS_CARD_BG)),
                                ),
                        )
                        .with_animation(
                            ("discover-skel-pulse", row_idx * 16 + col_idx),
                            row_anim.clone(),
                            |el, delta| el.opacity(delta),
                        )
                }))
        }))
        .into_any_element()
}

fn render_discover_empty_state(
    has_active_filters: bool,
    reset_req: Option<ResourceRequest>,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    div()
        .py(px(72.0))
        .px(px(24.0))
        .rounded(px(RADIUS_CARD))
        .bg(rgba(GLASS_CARD_BG))
        .border_1()
        .border_color(rgba(GLASS_BORDER))
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
                .child(svg_icon(IconName::Compass, 20.0, rgb(TEXT_MUTED))),
        )
        .child(
            div()
                .font(editorial_serif())
                .text_size(px(30.0))
                .text_color(rgb(TEXT_PRIMARY))
                .child("No titles found"),
        )
        .child(
            div()
                .max_w(px(420.0))
                .text_size(px(14.0))
                .text_color(rgb(TEXT_SECONDARY))
                .child(if has_active_filters {
                    "No catalog entries match the active filters. Reset filters or pick another genre."
                } else {
                    "This catalog did not return any titles. Try switching catalogs or addons."
                }),
        )
        .when(has_active_filters, |el| {
            el.child(div().pt(px(6.0)).child(primary_accent_button(
                "discover-empty-reset",
                Some(IconName::RefreshCw),
                "Reset filters",
                cx.listener(move |this, _, _, cx| {
                    this.discover_catalog_search.clear();
                    this.input_cursors
                        .remove(&FocusedInput::DiscoverCatalogSearch);
                    this.input_selected_all
                        .remove(&FocusedInput::DiscoverCatalogSearch);
                    this.apply_discover_selection(
                        reset_req
                            .clone()
                            .map(|request| CatalogWithFiltersSelected { request }),
                        cx,
                    );
                }),
            )))
        })
        .into_any_element()
}

fn render_discover_dropdown_trigger(
    id: impl Into<ElementId>,
    prefix_label: impl Into<SharedString>,
    value_label: impl Into<SharedString>,
    is_open: bool,
    is_filtered: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let highlighted = is_open || is_filtered;
    div()
        .id(id.into())
        .cursor_pointer()
        .h(px(38.0))
        .px(px(12.0))
        .max_w(px(340.0))
        .rounded(px(RADIUS_CONTROL))
        .bg(if highlighted {
            rgba(ACCENT_SOFT)
        } else {
            rgba(GLASS_TROUGH_BG)
        })
        .border_1()
        .border_color(if highlighted {
            rgba(BORDER_ACCENT)
        } else {
            rgba(GLASS_BORDER)
        })
        .hover(|s| {
            s.bg(if highlighted {
                rgba(ACCENT_SOFT)
            } else {
                rgba(GLASS_ROW_HOVER_BG)
            })
            .border_color(if highlighted {
                rgb(ACCENT)
            } else {
                rgba(GLASS_BORDER_STRONG)
            })
        })
        .flex()
        .items_center()
        .gap(px(8.0))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(rgb(TEXT_MUTED))
                .child(prefix_label.into()),
        )
        .child(
            div()
                .w(px(1.0))
                .h(px(14.0))
                .flex_shrink_0()
                .bg(rgba(GLASS_DIVIDER)),
        )
        .child(
            div()
                .min_w(px(0.0))
                .font(ui_sans(FontWeight::MEDIUM))
                .text_size(px(13.0))
                .text_color(if highlighted {
                    rgb(ACCENT)
                } else {
                    rgb(TEXT_PRIMARY)
                })
                .truncate()
                .child(value_label.into()),
        )
        .child(svg_icon(
            IconName::ChevronDown,
            12.5,
            if highlighted {
                rgb(ACCENT)
            } else {
                rgb(TEXT_MUTED)
            },
        ))
}
