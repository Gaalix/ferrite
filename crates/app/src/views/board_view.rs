use std::collections::HashMap;

use gpui::{
    div, img, linear_color_stop, linear_gradient, prelude::*, px, relative, rgb, rgba, Context,
    ElementId, FontWeight, MouseButton, ScrollDelta, ScrollEvent, SharedString,
};
use stremio_core_runtime::{
    stremio_core::{
        models::common::Loadable,
        types::{library::LibraryItem, resource::MetaItemPreview},
    },
    INITIAL_BOARD_SHELVES,
};

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{
        classify_library_item_status, extract_genres, extract_imdb_rating,
        format_movietracker_episode_code, scroll_near_bottom, LibraryStatusTab, SidebarTab,
    },
    theme::{
        editorial_serif, meta_divider, mono_font, placeholder_tint_for_id, poster_card_shadow,
        sentence_case_label, tabular_nums, ui_sans, ACCENT, ACCENT_AMBER, ACCENT_HOVER,
        BG_ELEVATED, BG_OBSIDIAN, BG_SURFACE, BORDER_POSTER_HOVER, BORDER_SOFT, BORDER_STRONG,
        BORDER_SUBTLE, STATUS_WATCHED, STATUS_WATCHING, STATUS_WATCHLIST, TEXT_MUTED, TEXT_PRIMARY,
        TEXT_SECONDARY,
    },
};

struct BoardRowSnapshot {
    index: usize,
    title: String,
    subtitle: Option<String>,
    items: Vec<MetaItemPreview>,
    is_loading: bool,
}

impl AppShell {
    pub fn render_board_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shelf_limit = self.visible_board_shelves.max(INITIAL_BOARD_SHELVES);
        let (cw_items, catalog_rows, total_board_catalogs, library_statuses) =
            self.core.with_model(|model| {
                let cw: Vec<LibraryItem> = model
                    .continue_watching_preview
                    .items
                    .iter()
                    .map(|i| i.library_item.clone())
                    .collect();

                let total_catalogs = model.board.catalogs.len();
                let mut rows = Vec::with_capacity(shelf_limit.min(total_catalogs));
                for (idx, cat) in model.board.catalogs.iter().enumerate().take(shelf_limit) {
                    let Some(first_page) = cat.first() else {
                        continue;
                    };
                    let raw_type = first_page.request.path.r#type.as_str();
                    let raw_id = first_page.request.path.id.as_str();

                    let addon_cat_name = model
                        .ctx
                        .profile
                        .addons
                        .iter()
                        .find(|a| a.transport_url == first_page.request.base)
                        .and_then(|a| {
                            a.manifest
                                .catalogs
                                .iter()
                                .find(|mc| mc.id == raw_id && mc.r#type == raw_type)
                                .and_then(|mc| mc.name.clone())
                        });

                    let (section_title, section_sub) = editorialize_shelf_heading(
                        idx,
                        addon_cat_name.as_deref().unwrap_or(raw_id),
                        raw_type,
                    );

                    let mut merged_items = Vec::new();
                    let mut is_loading = false;
                    for page in cat {
                        match &page.content {
                            Some(Loadable::Ready(items)) => {
                                merged_items.extend(items.iter().take(12).cloned())
                            }
                            Some(Loadable::Loading) => is_loading = true,
                            _ => {}
                        }
                        if merged_items.len() >= 12 {
                            break;
                        }
                    }

                    if !merged_items.is_empty() || is_loading {
                        rows.push(BoardRowSnapshot {
                            index: idx,
                            title: section_title,
                            subtitle: section_sub,
                            items: merged_items,
                            is_loading,
                        });
                    }
                }

                let statuses: HashMap<String, LibraryStatusTab> = model
                    .ctx
                    .library
                    .items
                    .iter()
                    .filter_map(|(id, li)| {
                        classify_library_item_status(li).map(|st| (id.clone(), st))
                    })
                    .collect();

                (cw, rows, total_catalogs, statuses)
            });

        let has_any_content = !cw_items.is_empty() || !catalog_rows.is_empty();
        let has_more_shelves = shelf_limit < total_board_catalogs;
        let remaining_shelves = total_board_catalogs.saturating_sub(shelf_limit);

        div()
            .id("board-view-scroll")
            .track_scroll(&self.board_scroll_handle)
            .size_full()
            .overflow_y_scroll()
            .on_scroll(cx.listener(move |this, ev: &ScrollEvent, _, cx| {
                if this.visible_board_shelves >= total_board_catalogs {
                    return;
                }
                let down_px = match ev.delta {
                    ScrollDelta::Pixels(p) => (-p.y) / px(1.0),
                    ScrollDelta::Lines(l) => (-l.y) * 36.0,
                };
                if down_px > 0.0
                    && scroll_near_bottom(&this.board_scroll_handle, down_px, 420.0)
                {
                    let old = this.visible_board_shelves.max(INITIAL_BOARD_SHELVES);
                    let next = (old + INITIAL_BOARD_SHELVES).min(total_board_catalogs);
                    if next > old {
                        this.visible_board_shelves = next;
                        this.core.load_board_range(old..next);
                        cx.notify();
                    }
                }
            }))
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
                    .gap(px(44.0))
                    .when(!has_any_content, |el| {
                        el.child(Self::render_empty_board_banner(cx))
                    })
                    .when(!cw_items.is_empty(), |el| {
                        el.child(self.render_continue_watching_section(&cw_items, cx))
                    })
                    .children(catalog_rows.into_iter().map(|row| {
                        self.render_editorial_shelf(row, &library_statuses, cx)
                    }))
                    .when(has_more_shelves, |el| {
                        el.child(
                            div()
                                .pt(px(4.0))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(16.0))
                                .child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(format!(
                                            "Showing {} of {total_board_catalogs} shelves",
                                            shelf_limit.min(total_board_catalogs)
                                        )),
                                )
                                .child(
                                    div()
                                        .id("btn-board-show-more-shelves")
                                        .cursor_pointer()
                                        .h(px(36.0))
                                        .px(px(16.0))
                                        .rounded(px(999.0))
                                        .bg(rgb(BG_SURFACE))
                                        .border_1()
                                        .border_color(rgba(BORDER_SUBTLE))
                                        .hover(|s| {
                                            s.bg(rgb(BG_ELEVATED))
                                                .border_color(rgba(BORDER_STRONG))
                                                .text_color(rgb(TEXT_PRIMARY))
                                        })
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .font(ui_sans(FontWeight::MEDIUM))
                                        .text_size(px(13.0))
                                        .text_color(rgb(TEXT_SECONDARY))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                let old = this
                                                    .visible_board_shelves
                                                    .max(INITIAL_BOARD_SHELVES);
                                                let next = (old + INITIAL_BOARD_SHELVES)
                                                    .min(total_board_catalogs);
                                                if next > old {
                                                    this.visible_board_shelves = next;
                                                    this.core.load_board_range(old..next);
                                                    cx.notify();
                                                }
                                            }),
                                        )
                                        .child(format!(
                                            "Show {} more shelves",
                                            remaining_shelves.min(INITIAL_BOARD_SHELVES)
                                        ))
                                        .child(svg_icon(
                                            IconName::ChevronDown,
                                            13.0,
                                            rgb(TEXT_MUTED),
                                        )),
                                ),
                        )
                    }),
            )
    }

    fn render_continue_watching_section(
        &self,
        items: &[LibraryItem],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .child(
                        div()
                            .font(editorial_serif())
                            .text_size(px(32.0))
                            .text_color(rgb(TEXT_PRIMARY))
                            .child("Pick up where you left off"),
                    )
                    .child(
                        div()
                            .id("cw-open-library")
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_size(px(13.0))
                            .text_color(rgb(TEXT_SECONDARY))
                            .hover(|s| s.text_color(rgb(TEXT_PRIMARY)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.library_status_tab = LibraryStatusTab::Watching;
                                    this.navigate_tab(SidebarTab::Library, cx);
                                }),
                            )
                            .child(format!("{} in progress", items.len()))
                            .child(svg_icon(IconName::ChevronRight, 13.0, rgb(TEXT_MUTED))),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .grid()
                    .grid_cols(3)
                    .gap(px(18.0))
                    .children(items.iter().take(6).enumerate().map(|(idx, li)| {
                        let metahub_bg = format!(
                            "https://images.metahub.space/background/medium/{}/img",
                            li.id
                        );
                        let wide_tex = self
                            .poster_cache
                            .resolve_widescreen_image(&metahub_bg)
                            .or_else(|| self.poster_cache.resolve_poster_url(li.poster.as_ref()));
                        let ep_code =
                            format_movietracker_episode_code(li.state.video_id.as_deref());
                        let pct = li.progress().clamp(2.0, 100.0) as f32;
                        let remaining_caption = format_cw_remaining(li, pct);

                        let resume_type = li.r#type.clone();
                        let resume_id = li.id.clone();
                        let resume_vid = li.state.video_id.clone();
                        let ctx_type = li.r#type.clone();
                        let ctx_id = li.id.clone();
                        let ctx_vid = li.state.video_id.clone();
                        let btn_type = li.r#type.clone();
                        let btn_id = li.id.clone();
                        let btn_vid = li.state.video_id.clone();
                        let title_type = li.r#type.clone();
                        let title_id = li.id.clone();
                        let title_vid = li.state.video_id.clone();
                        let group_name: SharedString = format!("cw-card-grp-{idx}").into();

                        div()
                            .id(("cw-card", idx))
                            .group(group_name.clone())
                            .cursor_pointer()
                            .w_full()
                            .aspect_ratio(16.0 / 9.0)
                            .rounded(px(8.0))
                            .overflow_hidden()
                            .relative()
                            .bg(rgb(placeholder_tint_for_id(&li.id)))
                            .border_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .hover(|s| s.border_color(rgba(BORDER_STRONG)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.quick_resume_or_open(
                                        resume_type.clone(),
                                        resume_id.clone(),
                                        resume_vid.clone(),
                                        cx,
                                    );
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_meta_details(
                                        ctx_type.clone(),
                                        ctx_id.clone(),
                                        ctx_vid.clone(),
                                        cx,
                                    );
                                }),
                            )
                            .when_some(wide_tex, |el, tex| {
                                el.child(img(tex).size_full().object_fit(gpui::ObjectFit::Cover))
                            })
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .bg(linear_gradient(
                                        180.0,
                                        linear_color_stop(rgba(0x0b0b0d14), 0.15),
                                        linear_color_stop(rgba(0x0b0b0deb), 1.0),
                                    )),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .opacity(0.0)
                                    .group_hover(group_name.clone(), |s| s.opacity(1.0))
                                    .child(
                                        div()
                                            .w(px(52.0))
                                            .h(px(52.0))
                                            .rounded(px(999.0))
                                            .bg(rgb(ACCENT))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(svg_icon(
                                                IconName::Play,
                                                20.0,
                                                rgb(BG_OBSIDIAN),
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .id(("cw-card-details", idx))
                                    .cursor_pointer()
                                    .absolute()
                                    .top(px(10.0))
                                    .right(px(10.0))
                                    .h(px(28.0))
                                    .px(px(10.0))
                                    .rounded(px(999.0))
                                    .bg(rgba(0x0b0b0dd8))
                                    .border_1()
                                    .border_color(rgba(BORDER_SUBTLE))
                                    .opacity(0.86)
                                    .group_hover(group_name, |s| s.opacity(1.0))
                                    .hover(|s| {
                                        s.bg(rgb(BG_ELEVATED))
                                            .border_color(rgba(BORDER_POSTER_HOVER))
                                            .text_color(rgb(TEXT_PRIMARY))
                                    })
                                    .flex()
                                    .items_center()
                                    .gap(px(5.0))
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_xs()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.open_meta_details(
                                                btn_type.clone(),
                                                btn_id.clone(),
                                                btn_vid.clone(),
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(svg_icon(IconName::Info, 12.5, rgb(TEXT_SECONDARY)))
                                    .child("Details"),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .bottom(px(3.0))
                                    .p(px(16.0))
                                    .flex()
                                    .flex_col()
                                    .items_start()
                                    .gap(px(3.0))
                                    .when_some(ep_code, |el, code| {
                                        el.child(
                                            div()
                                                .font(mono_font())
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_xs()
                                                .text_color(rgb(ACCENT))
                                                .child(code),
                                        )
                                    })
                                    .child(
                                        div()
                                            .id(("cw-card-title", idx))
                                            .cursor_pointer()
                                            .max_w_full()
                                            .font(ui_sans(FontWeight::BOLD))
                                            .text_size(px(16.0))
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .hover(|s| s.text_color(rgb(ACCENT)))
                                            .truncate()
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.open_meta_details(
                                                        title_type.clone(),
                                                        title_id.clone(),
                                                        title_vid.clone(),
                                                        cx,
                                                    );
                                                }),
                                            )
                                            .child(li.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .font(mono_font())
                                            .text_xs()
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(remaining_caption),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .bottom_0()
                                    .h(px(3.0))
                                    .bg(rgba(0xffffff2e))
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(pct / 100.0))
                                            .bg(rgb(ACCENT)),
                                    ),
                            )
                    })),
            )
    }

    fn render_editorial_shelf(
        &self,
        row: BoardRowSnapshot,
        library_statuses: &HashMap<String, LibraryStatusTab>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(14.0))
                            .child(
                                div()
                                    .font(editorial_serif())
                                    .text_size(px(30.0))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .child(row.title),
                            )
                            .when_some(row.subtitle, |el, sub| {
                                el.child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(sub),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id(("shelf-see-all", row.index))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_size(px(13.0))
                            .text_color(rgb(TEXT_SECONDARY))
                            .hover(|s| s.text_color(rgb(TEXT_PRIMARY)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.navigate_tab(SidebarTab::Discover, cx);
                                }),
                            )
                            .child("See all")
                            .child(svg_icon(IconName::ChevronRight, 13.0, rgb(TEXT_MUTED))),
                    ),
            )
            .child(if row.items.is_empty() && row.is_loading {
                div()
                    .w_full()
                    .grid()
                    .grid_cols(6)
                    .gap(px(18.0))
                    .children((0..6).map(|_| {
                        div()
                            .w_full()
                            .aspect_ratio(2.0 / 3.0)
                            .rounded(px(4.0))
                            .bg(rgb(BG_SURFACE))
                            .border_1()
                            .border_color(rgba(BORDER_SOFT))
                    }))
                    .into_any_element()
            } else {
                div()
                    .w_full()
                    .grid()
                    .grid_cols(6)
                    .gap(px(18.0))
                    .children(row.items.into_iter().take(12).enumerate().map(
                        |(item_idx, item)| {
                            let lib_status = library_statuses.get(&item.id).copied();
                            self.render_movietracker_poster_card(
                                ("board-poster", row.index * 1000 + item_idx),
                                format!("board-grp-{}-{item_idx}", row.index),
                                item,
                                lib_status,
                                cx,
                            )
                        },
                    ))
                    .into_any_element()
            })
    }

    fn render_empty_board_banner(cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .p(px(36.0))
            .rounded(px(10.0))
            .bg(rgb(BG_SURFACE))
            .border_1()
            .border_color(rgba(BORDER_SUBTLE))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .font(editorial_serif())
                    .text_size(px(34.0))
                    .text_color(rgb(TEXT_PRIMARY))
                    .child("Setting the projection room"),
            )
            .child(
                div()
                    .max_w(px(540.0))
                    .text_size(px(14.0))
                    .text_color(rgb(TEXT_SECONDARY))
                    .child(
                        "Sign in to sync your Stremio library and addons, or press Ctrl K to search any film or series immediately.",
                    ),
            )
            .child(
                div()
                    .mt(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .id("empty-btn-signin")
                            .cursor_pointer()
                            .h(px(40.0))
                            .px(px(18.0))
                            .rounded(px(6.0))
                            .bg(rgb(ACCENT))
                            .hover(|s| s.bg(rgb(ACCENT_HOVER)))
                            .flex()
                            .items_center()
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_size(px(13.5))
                            .text_color(rgb(BG_OBSIDIAN))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.navigate_tab(SidebarTab::Account, cx);
                                }),
                            )
                            .child("Sign in"),
                    )
                    .child(
                        div()
                            .id("empty-btn-search")
                            .cursor_pointer()
                            .h(px(40.0))
                            .px(px(18.0))
                            .rounded(px(6.0))
                            .bg(rgb(BG_ELEVATED))
                            .border_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .hover(|s| s.border_color(rgba(BORDER_STRONG)))
                            .flex()
                            .items_center()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(px(13.5))
                            .text_color(rgb(TEXT_PRIMARY))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.open_command_palette(cx);
                                }),
                            )
                            .child("Search catalog"),
                    ),
            )
    }

    pub fn render_poster_card(
        &self,
        element_id: impl Into<ElementId>,
        item: MetaItemPreview,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let lib_status = self.core.with_model(|m| {
            m.ctx
                .library
                .items
                .get(&item.id)
                .and_then(classify_library_item_status)
        });
        let group_key = format!("poster-card-{}", item.id);
        self.render_movietracker_poster_card(element_id, group_key, item, lib_status, cx)
    }

    pub fn render_movietracker_poster_card(
        &self,
        element_id: impl Into<ElementId>,
        group_key: String,
        item: MetaItemPreview,
        lib_status: Option<LibraryStatusTab>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let poster_tex = self.poster_cache.resolve_poster_url(item.poster.as_ref());
        let rating = extract_imdb_rating(&item);
        let genres = extract_genres(&item, 1);
        let year = item
            .release_info
            .clone()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());

        let open_type = item.r#type.clone();
        let open_id = item.id.clone();
        let toggle_preview = item.clone();
        let in_library = lib_status.is_some();
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
                    this.open_meta_details(open_type.clone(), open_id.clone(), None, cx);
                }),
            )
            .child(
                div()
                    .w_full()
                    .aspect_ratio(2.0 / 3.0)
                    .rounded(px(4.0))
                    .overflow_hidden()
                    .relative()
                    .bg(rgb(placeholder_tint_for_id(&item.id)))
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
                                    .child(item.name.clone()),
                            )
                            .into_any_element()
                    })
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .opacity(0.0)
                            .group_hover(grp.clone(), |s| s.opacity(1.0))
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
                    .when_some(rating, |el, r| {
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
                                .group_hover(grp.clone(), |s| s.opacity(0.0))
                                .child(svg_icon(IconName::Star, 10.5, rgb(ACCENT_AMBER)))
                                .child(r),
                        )
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("poster-quickadd-{}", item.id)))
                            .absolute()
                            .bottom(px(8.0))
                            .left(px(8.0))
                            .opacity(0.0)
                            .group_hover(grp, |s| s.opacity(1.0))
                            .w(px(30.0))
                            .h(px(30.0))
                            .rounded(px(999.0))
                            .bg(if in_library {
                                rgb(STATUS_WATCHLIST)
                            } else {
                                rgba(0x141417eb)
                            })
                            .border_1()
                            .border_color(if in_library {
                                rgb(STATUS_WATCHLIST)
                            } else {
                                rgba(BORDER_STRONG)
                            })
                            .hover(|s| s.bg(rgb(ACCENT)).border_color(rgb(ACCENT)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.core.toggle_library_item(toggle_preview.clone());
                                    cx.notify();
                                }),
                            )
                            .child(svg_icon(
                                if in_library {
                                    IconName::Check
                                } else {
                                    IconName::Bookmark
                                },
                                13.0,
                                if in_library {
                                    rgb(BG_OBSIDIAN)
                                } else {
                                    rgb(TEXT_PRIMARY)
                                },
                            )),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(px(14.0))
                            .text_color(rgb(TEXT_PRIMARY))
                            .truncate()
                            .child(item.name),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.0))
                            .text_xs()
                            .font_features(tabular_nums())
                            .text_color(rgb(TEXT_MUTED))
                            .child(
                                year.unwrap_or_else(|| sentence_case_label(&item.r#type)),
                            )
                            .when_some(genres.into_iter().next(), |el, g| {
                                el.child(meta_divider())
                                    .child(div().truncate().child(g))
                            }),
                    ),
            )
    }
}

fn format_cw_remaining(li: &LibraryItem, pct: f32) -> String {
    if li.state.duration > li.state.time_offset && li.state.duration > 60_000 {
        let rem_mins = ((li.state.duration - li.state.time_offset) / 60_000).max(1);
        format!("{rem_mins}m left")
    } else {
        format!("{pct:.0}% watched")
    }
}

fn editorialize_shelf_heading(
    idx: usize,
    raw_cat: &str,
    raw_type: &str,
) -> (String, Option<String>) {
    let lower = raw_cat.trim().to_ascii_lowercase();
    let is_movie = raw_type.eq_ignore_ascii_case("movie");
    let type_tag = sentence_case_label(raw_type);

    if idx == 0 && (lower == "top" || lower.contains("popular")) {
        return (
            "What people are watching".to_owned(),
            Some(format!("Popular {} this week", if is_movie { "films" } else { "shows" })),
        );
    }
    if idx == 1 && (lower == "top" || lower.contains("popular")) {
        return (
            "Television worth your evenings".to_owned(),
            Some("Trending series across your catalogs".to_owned()),
        );
    }
    if lower.contains("featured") || lower.contains("imdb") {
        return (
            if is_movie {
                "Critically loved".to_owned()
            } else {
                "On a lot of lists".to_owned()
            },
            Some(type_tag),
        );
    }
    if lower.contains("new") || lower.contains("year") || lower.contains("cinemeta") {
        return ("Now playing".to_owned(), Some(type_tag));
    }

    let clean = sentence_case_label(raw_cat);
    if clean.to_ascii_lowercase().contains(&raw_type.to_ascii_lowercase()) {
        (clean, None)
    } else {
        (format!("{clean} {}", raw_type.to_ascii_lowercase()), None)
    }
}
