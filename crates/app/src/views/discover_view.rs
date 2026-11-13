use std::{collections::HashMap, ops::Range, sync::Arc};

use gpui::{
    deferred, div, point, prelude::*, px, rgb, rgba, Context, FontWeight, MouseButton,
    SharedString, Window,
};
use stremio_core_runtime::{
    resolve_addon_for_request_base,
    stremio_core::{
        models::{
            catalog_with_filters::Selected as CatalogWithFiltersSelected,
            common::Loadable,
        },
        runtime::msg::{Action, ActionCatalogWithFilters},
    },
    DesktopModelField,
};

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{
        classify_library_item_status, DiscoverDropdown, FocusedInput, LibraryStatusTab,
    },
    theme::{
        editorial_serif, glass_capsule_shadow, meta_divider, mono_font, sentence_case_label,
        ui_sans, ACCENT, ACCENT_SOFT, BG_ELEVATED, BG_INPUT, BG_POPOVER_GLASS, BG_SURFACE,
        BORDER_POSTER_HOVER, BORDER_SOFT, BORDER_STRONG, BORDER_SUBTLE, TEXT_MUTED, TEXT_PRIMARY,
        TEXT_SECONDARY,
    },
    virtual_grid::{compute_poster_grid_columns, slice_row_items, VirtualGridLayout},
};

impl AppShell {
    pub fn render_discover_view(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (
            types,
            catalogs,
            extras,
            has_next_page,
            items,
            is_loading,
            library_statuses,
            installed_addons,
        ) = self.core.with_model(|model| {
            let types = model.discover.selectable.types.clone();
            let catalogs = model.discover.selectable.catalogs.clone();
            let extras = model.discover.selectable.extra.clone();
            let has_next_page = model.discover.selectable.next_page.is_some();

            let mut merged = Vec::new();
            let mut loading = false;
            for page in &model.discover.catalog {
                match &page.content {
                    Some(Loadable::Ready(list)) => merged.extend(list.iter().cloned()),
                    Some(Loadable::Loading) => loading = true,
                    _ => {}
                }
            }

            let statuses: HashMap<String, LibraryStatusTab> = model
                .ctx
                .library
                .items
                .iter()
                .filter_map(|(id, li)| classify_library_item_status(li).map(|st| (id.clone(), st)))
                .collect();

            let addons = model.ctx.profile.addons.clone();

            (
                types,
                catalogs,
                extras,
                has_next_page,
                merged,
                loading,
                statuses,
                addons,
            )
        });

        let genre_extra = extras
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case("genre"))
            .cloned();

        let active_type_label = types
            .iter()
            .find(|t| t.selected)
            .or_else(|| types.first())
            .map(|t| sentence_case_label(&t.r#type))
            .unwrap_or_else(|| "All formats".to_owned());

        let enriched_catalogs: Vec<_> = catalogs
            .into_iter()
            .map(|c| {
                let title = sentence_case_label(&c.catalog);
                let addon_name = resolve_addon_for_request_base(&installed_addons, &c.request.base)
                    .map(|d| d.manifest.name.clone())
                    .or_else(|| c.request.base.host_str().map(ToOwned::to_owned));
                (c, title, addon_name)
            })
            .collect();

        let active_catalog_display = enriched_catalogs
            .iter()
            .find(|(c, _, _)| c.selected)
            .or_else(|| enriched_catalogs.first())
            .map(|(_, title, addon_opt)| match addon_opt {
                Some(addon) => format!("{title} ({addon})"),
                None => title.clone(),
            })
            .unwrap_or_else(|| "Select catalog".to_owned());

        let active_genre_label = genre_extra
            .as_ref()
            .and_then(|g| {
                g.options
                    .iter()
                    .find(|o| o.selected)
                    .map(|o| o.value.clone().unwrap_or_else(|| "All genres".to_owned()))
            })
            .unwrap_or_else(|| "All genres".to_owned());

        let open_dd = self.discover_open_dropdown.clone();
        let format_open = open_dd == Some(DiscoverDropdown::Format);
        let catalog_open = open_dd == Some(DiscoverDropdown::Catalog);
        let genre_open = open_dd == Some(DiscoverDropdown::Genre);
        let cat_query_trimmed = self.discover_catalog_search.trim().to_ascii_lowercase();
        let cat_search_focused = self.focused_input == Some(FocusedInput::DiscoverCatalogSearch);
        let total_catalogs_count = enriched_catalogs.len();
        let loaded_titles_count = items.len();
        let viewport_w = f32::from(window.viewport_size().width);
        let columns = if viewport_w > 0.0 && viewport_w.is_finite() {
            compute_poster_grid_columns(viewport_w.min(1440.0), 0.0, 64.0, 176.0, 18.0)
        } else {
            6
        };
        let items_arc = Arc::new(items);
        let statuses_arc = Arc::new(library_statuses);
        let total_items = items_arc.len();
        let row_count = VirtualGridLayout::new(total_items, columns, 336.0).row_count;

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
                            .px(px(16.0))
                            .py(px(12.0))
                            .rounded(px(10.0))
                            .bg(rgb(BG_SURFACE))
                            .border_1()
                            .border_color(rgba(BORDER_SUBTLE))
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
                                        let types_for_menu = types;
                                        bar.child(
                                            div()
                                                .relative()
                                                .child(render_discover_dropdown_trigger(
                                                    "discover-dd-format",
                                                    "Format",
                                                    active_type_label,
                                                    format_open,
                                                    false,
                                                    cx.listener(|this, _, _, cx| {
                                                        if this.discover_open_dropdown
                                                            == Some(DiscoverDropdown::Format)
                                                        {
                                                            this.discover_open_dropdown = None;
                                                        } else {
                                                            this.discover_open_dropdown =
                                                                Some(DiscoverDropdown::Format);
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
                                                .when(format_open, |wrap| {
                                                    wrap.child(
                                                        deferred(
                                                            div()
                                                                .id("discover-menu-format")
                                                                .occlude()
                                                                .absolute()
                                                                .top(px(44.0))
                                                                .left_0()
                                                                .w(px(220.0))
                                                                .max_h(px(320.0))
                                                                .overflow_y_scroll()
                                                                .p(px(6.0))
                                                                .rounded(px(10.0))
                                                                .bg(rgba(BG_POPOVER_GLASS))
                                                                .border_1()
                                                                .border_color(rgba(BORDER_SUBTLE))
                                                                .shadow(glass_capsule_shadow())
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    |_, _, cx| {
                                                                        cx.stop_propagation();
                                                                    },
                                                                )
                                                                .child(
                                                                    div()
                                                                        .w_full()
                                                                        .flex()
                                                                        .flex_col()
                                                                        .gap(px(2.0))
                                                                        .children(
                                                                            types_for_menu
                                                                                .into_iter()
                                                                                .enumerate()
                                                                                .map(|(idx, t)| {
                                                                                    let req =
                                                                                        t.request;
                                                                                    let selected =
                                                                                        t.selected;
                                                                                    let label =
                                                                                        sentence_case_label(
                                                                                            &t.r#type,
                                                                                        );
                                                                                    render_dropdown_option_row(
                                                                                        ("discover-opt-fmt", idx),
                                                                                        label,
                                                                                        None,
                                                                                        selected,
                                                                                        cx.listener(
                                                                                            move |this, _, _, cx| {
                                                                                                 this.discover_open_dropdown =
                                                                                                    None;
                                                                                                this.discover_grid_scroll_handle
                                                                                                    .0
                                                                                                    .borrow()
                                                                                                    .base_handle
                                                                                                    .set_offset(point(px(0.0), px(0.0)));
                                                                                                this.core.load_discover(
                                                                                                    Some(
                                                                                                        CatalogWithFiltersSelected {
                                                                                                            request: req.clone(),
                                                                                                        },
                                                                                                    ),
                                                                                                );
                                                                                                cx.notify();
                                                                                            },
                                                                                        ),
                                                                                    )
                                                                                }),
                                                                        ),
                                                                ),
                                                        )
                                                        .with_priority(10),
                                                    )
                                                }),
                                        )
                                    })
                                    .when(!enriched_catalogs.is_empty(), |bar| {
                                        let filtered_catalogs: Vec<_> = enriched_catalogs
                                            .into_iter()
                                            .filter(|(_, title, addon_opt)| {
                                                if cat_query_trimmed.is_empty() {
                                                    true
                                                } else {
                                                    title
                                                        .to_ascii_lowercase()
                                                        .contains(&cat_query_trimmed)
                                                        || addon_opt.as_ref().is_some_and(|a| {
                                                            a.to_ascii_lowercase()
                                                                .contains(&cat_query_trimmed)
                                                        })
                                                }
                                            })
                                            .collect();

                                        let search_display: SharedString =
                                            if cat_search_focused {
                                                if self.discover_catalog_search.is_empty() {
                                                    "Filter catalogs or addons...|".into()
                                                } else {
                                                    format!("{}|", self.discover_catalog_search)
                                                        .into()
                                                }
                                            } else if self.discover_catalog_search.is_empty() {
                                                "Filter catalogs or addons...".into()
                                            } else {
                                                self.discover_catalog_search.clone().into()
                                            };
                                        let has_cat_query = !self.discover_catalog_search.is_empty();

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
                                                            div()
                                                                .id("discover-menu-catalog")
                                                                .occlude()
                                                                .absolute()
                                                                .top(px(44.0))
                                                                .left_0()
                                                                .w(px(364.0))
                                                                .p(px(8.0))
                                                                .rounded(px(10.0))
                                                                .bg(rgba(BG_POPOVER_GLASS))
                                                                .border_1()
                                                                .border_color(rgba(BORDER_SUBTLE))
                                                                .shadow(glass_capsule_shadow())
                                                                .flex()
                                                                .flex_col()
                                                                .gap(px(6.0))
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    |_, _, cx| {
                                                                        cx.stop_propagation();
                                                                    },
                                                                )
                                                                .child(
                                                                    div()
                                                                        .id("discover-cat-search-box")
                                                                        .cursor_pointer()
                                                                        .h(px(34.0))
                                                                        .px(px(10.0))
                                                                        .rounded(px(6.0))
                                                                        .bg(rgb(BG_INPUT))
                                                                        .border_1()
                                                                        .border_color(
                                                                            if cat_search_focused {
                                                                                rgba(
                                                                                    BORDER_POSTER_HOVER,
                                                                                )
                                                                            } else {
                                                                                rgba(BORDER_SUBTLE)
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
                                                                                        .min_w(px(
                                                                                            0.0,
                                                                                        ))
                                                                                        .text_xs()
                                                                                        .text_color(
                                                                                            if has_cat_query
                                                                                            {
                                                                                                rgb(TEXT_PRIMARY)
                                                                                            } else {
                                                                                                rgb(TEXT_MUTED)
                                                                                            },
                                                                                        )
                                                                                        .truncate()
                                                                                        .child(
                                                                                            search_display,
                                                                                        ),
                                                                                ),
                                                                        )
                                                                        .when(has_cat_query, |sb| {
                                                                            sb.child(
                                                                                div()
                                                                                    .id("discover-cat-search-clear")
                                                                                    .cursor_pointer()
                                                                                    .p(px(2.0))
                                                                                    .rounded(px(4.0))
                                                                                    .hover(|s| {
                                                                                        s.bg(rgb(
                                                                                            BG_ELEVATED,
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
                                                                        .max_h(px(320.0))
                                                                        .overflow_y_scroll()
                                                                        .child(
                                                                            div()
                                                                                .w_full()
                                                                                .flex()
                                                                                .flex_col()
                                                                                .gap(px(2.0))
                                                                                .children(if filtered_catalogs.is_empty() {
                                                                                    vec![div()
                                                                                        .flex_shrink_0()
                                                                                        .px(px(10.0))
                                                                                        .py(px(12.0))
                                                                                        .text_xs()
                                                                                        .text_color(rgb(TEXT_MUTED))
                                                                                        .child("No catalogs match your filter.")
                                                                                        .into_any_element()]
                                                                                } else {
                                                                                    filtered_catalogs
                                                                                        .into_iter()
                                                                                        .enumerate()
                                                                                        .map(|(idx, (c, title, addon_opt))| {
                                                                                            let req = c.request;
                                                                                            let selected = c.selected;
                                                                                            render_dropdown_option_row(
                                                                                                ("discover-opt-cat", idx),
                                                                                                title,
                                                                                                addon_opt,
                                                                                                selected,
                                                                                                cx.listener(
                                                                                                    move |this, _, _, cx| {
                                                                                                        this.discover_open_dropdown =
                                                                                                            None;
                                                                                                        if this.focused_input
                                                                                                            == Some(
                                                                                                                FocusedInput::DiscoverCatalogSearch,
                                                                                                            )
                                                                                                        {
                                                                                                            this.focused_input =
                                                                                                                None;
                                                                                                        }
                                                                                                        this.core.load_discover(
                                                                                                            Some(
                                                                                                                CatalogWithFiltersSelected {
                                                                                                                    request: req.clone(),
                                                                                                                },
                                                                                                            ),
                                                                                                        );
                                                                                                        this.discover_grid_scroll_handle
                                                                                                            .0
                                                                                                            .borrow()
                                                                                                            .base_handle
                                                                                                            .set_offset(point(px(0.0), px(0.0)));
                                                                                                        cx.notify();
                                                                                                    },
                                                                                                ),
                                                                                            )
                                                                                            .into_any_element()
                                                                                        })
                                                                                        .collect()
                                                                                }),
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
                                                                div()
                                                                    .id("discover-menu-genre")
                                                                    .occlude()
                                                                    .absolute()
                                                                    .top(px(44.0))
                                                                    .left_0()
                                                                    .w(px(256.0))
                                                                    .max_h(px(340.0))
                                                                    .overflow_y_scroll()
                                                                    .p(px(6.0))
                                                                    .rounded(px(10.0))
                                                                    .bg(rgba(BG_POPOVER_GLASS))
                                                                    .border_1()
                                                                    .border_color(rgba(
                                                                        BORDER_SUBTLE,
                                                                    ))
                                                                    .shadow(glass_capsule_shadow())
                                                                    .on_mouse_down(
                                                                        MouseButton::Left,
                                                                        |_, _, cx| {
                                                                            cx.stop_propagation();
                                                                        },
                                                                    )
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
                                                                                        render_dropdown_option_row(
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
                                                                                                    this.discover_open_dropdown =
                                                                                                        None;
                                                                                                    this.discover_grid_scroll_handle
                                                                                                        .0
                                                                                                        .borrow()
                                                                                                        .base_handle
                                                                                                        .set_offset(point(px(0.0), px(0.0)));
                                                                                                    this.core
                                                                                                        .load_discover(
                                                                                                            Some(
                                                                                                                CatalogWithFiltersSelected {
                                                                                                                    request: req
                                                                                                                        .clone(),
                                                                                                                },
                                                                                                            ),
                                                                                                        );
                                                                                                    cx.notify();
                                                                                                },
                                                                                            ),
                                                                                        )
                                                                                    }),
                                                                            ),
                                                                    ),
                                                            )
                                                            .with_priority(10),
                                                        )
                                                    }),
                                            )
                                        },
                                    )
                                    .children(
                                        extras
                                            .into_iter()
                                            .filter(|e| {
                                                !e.name.eq_ignore_ascii_case("genre")
                                                    && !e.options.is_empty()
                                            })
                                            .enumerate()
                                            .map(
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
                                                            div()
                                                                .id((
                                                                    "discover-menu-extra",
                                                                    e_idx,
                                                                ))
                                                                .occlude()
                                                                .absolute()
                                                                .top(px(44.0))
                                                                .left_0()
                                                                .w(px(240.0))
                                                                .max_h(px(320.0))
                                                                .overflow_y_scroll()
                                                                .p(px(6.0))
                                                                .rounded(px(10.0))
                                                                .bg(rgba(BG_POPOVER_GLASS))
                                                                .border_1()
                                                                .border_color(rgba(BORDER_SUBTLE))
                                                                .shadow(glass_capsule_shadow())
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    |_, _, cx| {
                                                                        cx.stop_propagation();
                                                                    },
                                                                )
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
                                                                                    render_dropdown_option_row(
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
                                                                                                this.discover_open_dropdown =
                                                                                                    None;
                                                                                                this.discover_grid_scroll_handle
                                                                                                    .0
                                                                                                    .borrow()
                                                                                                    .base_handle
                                                                                                    .set_offset(point(px(0.0), px(0.0)));
                                                                                                this.core
                                                                                                    .load_discover(
                                                                                                        Some(
                                                                                                            CatalogWithFiltersSelected {
                                                                                                                request: req
                                                                                                                    .clone(),
                                                                                                            },
                                                                                                        ),
                                                                                                    );
                                                                                                cx.notify();
                                                                                            },
                                                                                        ),
                                                                                    )
                                                                                }),
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
                                    .child(format!("{loaded_titles_count} titles"))
                                    .when(total_catalogs_count > 0, |row| {
                                        row.child(meta_divider())
                                            .child(format!("{total_catalogs_count} catalogs"))
                                    }),
                            ),
                    )
                    .child(
                        gpui::uniform_list(
                            "discover-poster-grid",
                            row_count,
                            cx.processor(move |this, visible_rows: Range<usize>, _window, cx| {
                                if row_count > 0
                                    && visible_rows.end + 2 >= row_count
                                    && has_next_page
                                    && !is_loading
                                {
                                    this.core.dispatch(
                                        Action::CatalogWithFilters(
                                            ActionCatalogWithFilters::LoadNextPage,
                                        ),
                                        Some(DesktopModelField::Discover),
                                    );
                                }

                                visible_rows
                                    .map(|row_idx| {
                                        let range = slice_row_items(total_items, columns, row_idx);
                                        let empty_slots = columns.saturating_sub(range.len());

                                        div()
                                            .id(("discover-row", row_idx))
                                            .w_full()
                                            .h(px(336.0))
                                            .pb(px(16.0))
                                            .flex()
                                            .items_start()
                                            .gap(px(18.0))
                                            .children(range.map(|item_idx| {
                                                let item = items_arc[item_idx].clone();
                                                let st = statuses_arc.get(&item.id).copied();
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.0))
                                                    .h_full()
                                                    .overflow_hidden()
                                                    .child(this.render_movietracker_poster_card(
                                                        ("discover-card", item_idx),
                                                        format!("discover-grp-{item_idx}"),
                                                        item,
                                                        st,
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
                        .min_h(px(0.0)),
                    ),
            )
    }
}

fn render_discover_dropdown_trigger(
    id: impl Into<gpui::ElementId>,
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
        .h(px(36.0))
        .px(px(14.0))
        .max_w(px(340.0))
        .rounded(px(8.0))
        .bg(if highlighted {
            rgba(ACCENT_SOFT)
        } else {
            rgb(BG_ELEVATED)
        })
        .border_1()
        .border_color(if highlighted {
            rgb(ACCENT)
        } else {
            rgba(BORDER_SUBTLE)
        })
        .hover(|s| {
            s.border_color(if highlighted {
                rgb(ACCENT)
            } else {
                rgba(BORDER_STRONG)
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
                .bg(rgba(BORDER_SOFT)),
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
            12.0,
            if highlighted {
                rgb(ACCENT)
            } else {
                rgb(TEXT_MUTED)
            },
        ))
}

fn render_dropdown_option_row(
    id: impl Into<gpui::ElementId>,
    primary_label: impl Into<SharedString>,
    secondary_label: Option<String>,
    selected: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id.into())
        .flex_shrink_0()
        .cursor_pointer()
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(6.0))
        .bg(if selected {
            rgba(ACCENT_SOFT)
        } else {
            rgba(0x00000000)
        })
        .hover(|s| s.bg(rgb(BG_ELEVATED)))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(svg_icon(
                    IconName::Check,
                    12.5,
                    if selected {
                        rgb(ACCENT)
                    } else {
                        rgba(0x00000000)
                    },
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .font(ui_sans(if selected {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        }))
                        .text_xs()
                        .text_color(if selected {
                            rgb(ACCENT)
                        } else {
                            rgb(TEXT_PRIMARY)
                        })
                        .truncate()
                        .child(primary_label.into()),
                ),
        )
        .when_some(secondary_label, |row, sub| {
            row.child(
                div()
                    .max_w(px(136.0))
                    .font(mono_font())
                    .text_size(px(11.0))
                    .text_color(rgb(TEXT_MUTED))
                    .truncate()
                    .child(sub),
            )
        })
}
