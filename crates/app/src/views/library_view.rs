use gpui::{
    div, img, point, prelude::*, px, relative, rgb, rgba, Context, FontWeight, MouseButton,
    ScrollDelta, ScrollEvent, SharedString,
};
use stremio_core_runtime::stremio_core::types::library::LibraryItem;

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{
        classify_library_item_status, format_movietracker_episode_code, scroll_near_bottom,
        LibrarySortOrder, LibraryStatusTab, LibraryTypeFilter, SidebarTab, LIBRARY_PAGE_SIZE,
    },
    theme::{
        editorial_serif, meta_divider, mono_font, placeholder_tint_for_id, poster_card_shadow,
        sentence_case_label, tabular_nums, ui_sans, ACCENT, ACCENT_HOVER, ACCENT_SOFT, BG_ELEVATED,
        BG_OBSIDIAN, BG_SURFACE, BORDER_POSTER_HOVER, BORDER_SOFT, BORDER_STRONG, BORDER_SUBTLE,
        STATUS_WATCHED, STATUS_WATCHING, STATUS_WATCHLIST, TEXT_MUTED, TEXT_PRIMARY,
        TEXT_SECONDARY,
    },
};

impl AppShell {
    pub fn render_library_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let all_items: Vec<(LibraryItem, LibraryStatusTab)> = self.core.with_model(|model| {
            let source: Vec<LibraryItem> = if !model.ctx.library.items.is_empty() {
                model.ctx.library.items.values().cloned().collect()
            } else {
                model.library.catalog.clone()
            };
            source
                .into_iter()
                .filter_map(|li| classify_library_item_status(&li).map(|st| (li, st)))
                .collect()
        });

        let mut films_watched_or_saved = 0_usize;
        let mut shows_count = 0_usize;
        let mut episodes_logged = 0_u64;
        let mut total_watched_ms = 0_u64;

        let mut count_watching = 0_usize;
        let mut count_watchlist = 0_usize;
        let mut count_watched = 0_usize;

        for (li, status) in &all_items {
            match status {
                LibraryStatusTab::Watching => count_watching += 1,
                LibraryStatusTab::Watchlist => count_watchlist += 1,
                LibraryStatusTab::Watched => count_watched += 1,
            }
            if li.r#type.eq_ignore_ascii_case("movie") {
                films_watched_or_saved += 1;
            } else {
                shows_count += 1;
                episodes_logged += (li.state.times_watched as u64).max(u64::from(
                    *status == LibraryStatusTab::Watching || *status == LibraryStatusTab::Watched,
                ));
            }
            if li.state.overall_time_watched > 0 {
                total_watched_ms += li.state.overall_time_watched;
            } else if li.state.time_offset > 0 {
                total_watched_ms += li.state.time_offset;
            }
        }
        let hours_logged = total_watched_ms / 3_600_000;

        let active_tab = self.library_status_tab;
        let active_type = self.library_type_filter;
        let active_sort = self.library_sort_order;

        let mut filtered: Vec<(LibraryItem, LibraryStatusTab)> = all_items
            .into_iter()
            .filter(|(li, st)| *st == active_tab && active_type.matches_type(&li.r#type))
            .collect();

        match active_sort {
            LibrarySortOrder::Recent => {
                filtered.sort_by_key(|b| std::cmp::Reverse(b.0.mtime))
            }
            LibrarySortOrder::Title => {
                filtered.sort_by_key(|a| a.0.name.to_lowercase())
            }
        }

        let total_filtered = filtered.len();
        let lib_limit = self.visible_library_limit.max(LIBRARY_PAGE_SIZE);
        let visible_count = total_filtered.min(lib_limit);
        let remaining_lib = total_filtered.saturating_sub(visible_count);

        div()
            .id("library-view-scroll")
            .track_scroll(&self.library_scroll_handle)
            .size_full()
            .overflow_y_scroll()
            .on_scroll(cx.listener(move |this, ev: &ScrollEvent, _, cx| {
                if this.visible_library_limit >= total_filtered {
                    return;
                }
                let down_px = match ev.delta {
                    ScrollDelta::Pixels(p) => (-p.y) / px(1.0),
                    ScrollDelta::Lines(l) => (-l.y) * 36.0,
                };
                if down_px > 0.0
                    && scroll_near_bottom(&this.library_scroll_handle, down_px, 380.0)
                {
                    let next = (this.visible_library_limit.max(LIBRARY_PAGE_SIZE)
                        + LIBRARY_PAGE_SIZE)
                        .min(total_filtered);
                    if next > this.visible_library_limit {
                        this.visible_library_limit = next;
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
                    .pt(px(32.0))
                    .pb(px(72.0))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .mb(px(28.0))
                            .flex()
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
                                    .child(render_library_stat(films_watched_or_saved.to_string(), "Films"))
                                    .child(render_library_stat(shows_count.to_string(), "Shows"))
                                    .child(render_library_stat(episodes_logged.to_string(), "Episodes"))
                                    .child(render_library_stat(hours_logged.to_string(), "Hours")),
                            ),
                    )
                    .child(
                        div()
                            .mb(px(28.0))
                            .border_b_1()
                            .border_color(rgba(BORDER_SOFT))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(16.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .children(LibraryStatusTab::ALL.into_iter().enumerate().map(
                                        |(idx, tab)| {
                                            let is_active = tab == active_tab;
                                            let count = match tab {
                                                LibraryStatusTab::Watching => count_watching,
                                                LibraryStatusTab::Watchlist => count_watchlist,
                                                LibraryStatusTab::Watched => count_watched,
                                            };
                                            div()
                                                .id(("lib-status-tab", idx))
                                                .cursor_pointer()
                                                .px(px(14.0))
                                                .py(px(12.0))
                                                .border_b_2()
                                                .border_color(if is_active {
                                                    rgb(ACCENT)
                                                } else {
                                                    rgba(0x00000000)
                                                })
                                                .flex()
                                                .items_center()
                                                .gap(px(8.0))
                                                .font(ui_sans(FontWeight::MEDIUM))
                                                .text_size(px(14.5))
                                                .text_color(if is_active {
                                                    rgb(TEXT_PRIMARY)
                                                } else {
                                                    rgb(TEXT_SECONDARY)
                                                })
                                                .hover(|s| s.text_color(rgb(TEXT_PRIMARY)))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.library_status_tab = tab;
                                                        this.visible_library_limit =
                                                            LIBRARY_PAGE_SIZE;
                                                        this.library_scroll_handle
                                                            .set_offset(point(px(0.0), px(0.0)));
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(tab.label())
                                                .child(
                                                    div()
                                                        .px(px(7.0))
                                                        .py(px(1.5))
                                                        .rounded(px(999.0))
                                                        .bg(rgb(BG_ELEVATED))
                                                        .font(mono_font())
                                                        .text_size(px(11.5))
                                                        .text_color(if is_active {
                                                            rgb(ACCENT)
                                                        } else {
                                                            rgb(TEXT_MUTED)
                                                        })
                                                        .child(count.to_string()),
                                                )
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .pb(px(8.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .children(LibraryTypeFilter::ALL.into_iter().enumerate().map(
                                        |(idx, tf)| {
                                            let is_active = tf == active_type;
                                            div()
                                                .id(("lib-type-pill", idx))
                                                .cursor_pointer()
                                                .h(px(32.0))
                                                .px(px(13.0))
                                                .rounded(px(999.0))
                                                .bg(if is_active {
                                                    rgba(ACCENT_SOFT)
                                                } else {
                                                    rgb(BG_SURFACE)
                                                })
                                                .border_1()
                                                .border_color(if is_active {
                                                    rgb(ACCENT)
                                                } else {
                                                    rgba(BORDER_SUBTLE)
                                                })
                                                .hover(|s| {
                                                    s.border_color(if is_active {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgba(BORDER_STRONG)
                                                    })
                                                })
                                                .flex()
                                                .items_center()
                                                .font(ui_sans(FontWeight::MEDIUM))
                                                .text_size(px(13.0))
                                                .text_color(if is_active {
                                                    rgb(ACCENT)
                                                } else {
                                                    rgb(TEXT_SECONDARY)
                                                })
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.library_type_filter = tf;
                                                        this.visible_library_limit =
                                                            LIBRARY_PAGE_SIZE;
                                                        this.library_scroll_handle
                                                            .set_offset(point(px(0.0), px(0.0)));
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(tf.label())
                                        },
                                    ))
                                    .child(
                                        div()
                                            .id("lib-sort-pill")
                                            .cursor_pointer()
                                            .h(px(32.0))
                                            .px(px(13.0))
                                            .rounded(px(999.0))
                                            .bg(rgb(BG_SURFACE))
                                            .border_1()
                                            .border_color(rgba(BORDER_SUBTLE))
                                            .hover(|s| s.border_color(rgba(BORDER_STRONG)))
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .font(ui_sans(FontWeight::MEDIUM))
                                            .text_size(px(13.0))
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.library_sort_order =
                                                        this.library_sort_order.next();
                                                    cx.notify();
                                                }),
                                            )
                                            .child(active_sort.label())
                                            .child(svg_icon(
                                                IconName::ChevronDown,
                                                12.5,
                                                rgb(TEXT_MUTED),
                                            )),
                                    ),
                            ),
                    )
                    .child(if filtered.is_empty() {
                        let (empty_title, empty_sub) = match active_tab {
                            LibraryStatusTab::Watching => (
                                "Nothing in progress",
                                "Start any film or episode and it will stay waiting for you right here.",
                            ),
                            LibraryStatusTab::Watchlist => (
                                "Your watchlist is empty",
                                "Bookmark films and series across Discover to build your queue.",
                            ),
                            LibraryStatusTab::Watched => (
                                "No finished titles yet",
                                "Completed films and series are archived here automatically.",
                            ),
                        };
                        div()
                            .py(px(72.0))
                            .px(px(24.0))
                            .rounded(px(10.0))
                            .border_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .bg(rgb(BG_SURFACE))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(10.0))
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
                            .child(
                                div()
                                    .id("lib-empty-discover")
                                    .cursor_pointer()
                                    .mt(px(8.0))
                                    .h(px(40.0))
                                    .px(px(20.0))
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
                                            this.navigate_tab(SidebarTab::Discover, cx);
                                        }),
                                    )
                                    .child("Browse Discover"),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(28.0))
                            .child(
                                div()
                                    .w_full()
                                    .grid()
                                    .grid_cols(6)
                                    .gap(px(18.0))
                                    .children(
                                        filtered.into_iter().take(lib_limit).enumerate().map(
                                            |(idx, (li, st))| {
                                                let poster_tex = self
                                                    .poster_cache
                                                    .resolve_poster_url(li.poster.as_ref());
                                                let ep_code = format_movietracker_episode_code(
                                                    li.state.video_id.as_deref(),
                                                );
                                                let pct = li.progress().clamp(0.0, 100.0) as f32;
                                                let (badge_icon, badge_col) = match st {
                                                    LibraryStatusTab::Watched => {
                                                        (IconName::Check, STATUS_WATCHED)
                                                    }
                                                    LibraryStatusTab::Watching => {
                                                        (IconName::Play, STATUS_WATCHING)
                                                    }
                                                    LibraryStatusTab::Watchlist => {
                                                        (IconName::Bookmark, STATUS_WATCHLIST)
                                                    }
                                                };

                                                let open_type = li.r#type.clone();
                                                let open_id = li.id.clone();
                                                let open_vid = li.state.video_id.clone();
                                                let grp: SharedString =
                                                    format!("lib-poster-grp-{idx}").into();

                                                div()
                                                    .id(("library-card", idx))
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
                                                            this.open_meta_details(
                                                                open_type.clone(),
                                                                open_id.clone(),
                                                                open_vid.clone(),
                                                                cx,
                                                            );
                                                        }),
                                                    )
                                                    .child(
                                                        div()
                                                            .w_full()
                                                            .aspect_ratio(2.0 / 3.0)
                                                            .rounded(px(4.0))
                                                            .overflow_hidden()
                                                            .relative()
                                                            .bg(rgb(placeholder_tint_for_id(
                                                                &li.id,
                                                            )))
                                                            .border_1()
                                                            .border_color(rgba(BORDER_SOFT))
                                                            .shadow(poster_card_shadow())
                                                            .group_hover(grp, |s| {
                                                                s.border_color(rgba(
                                                                    BORDER_POSTER_HOVER,
                                                                ))
                                                            })
                                                            .when_some(poster_tex, |el, tex| {
                                                                el.child(
                                                                    img(tex).size_full().object_fit(
                                                                        gpui::ObjectFit::Cover,
                                                                    ),
                                                                )
                                                            })
                                                            .child(
                                                                div()
                                                                    .absolute()
                                                                    .top(px(8.0))
                                                                    .right(px(8.0))
                                                                    .w(px(26.0))
                                                                    .h(px(26.0))
                                                                    .rounded(px(999.0))
                                                                    .bg(rgb(badge_col))
                                                                    .flex()
                                                                    .items_center()
                                                                    .justify_center()
                                                                    .child(svg_icon(
                                                                        badge_icon,
                                                                        12.5,
                                                                        rgb(BG_OBSIDIAN),
                                                                    )),
                                                            )
                                                            .when(
                                                                pct > 1.0
                                                                    && st
                                                                        == LibraryStatusTab::Watching,
                                                                |el| {
                                                                    el.child(
                                                                        div()
                                                                            .absolute()
                                                                            .bottom_0()
                                                                            .left_0()
                                                                            .right_0()
                                                                            .h(px(3.0))
                                                                            .bg(rgba(0xffffff2e))
                                                                            .child(
                                                                                div()
                                                                                    .h_full()
                                                                                    .w(relative(
                                                                                        pct / 100.0,
                                                                                    ))
                                                                                    .bg(rgb(ACCENT)),
                                                                            ),
                                                                    )
                                                                },
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
                                                                    .font(ui_sans(
                                                                        FontWeight::MEDIUM,
                                                                    ))
                                                                    .text_size(px(14.0))
                                                                    .text_color(rgb(TEXT_PRIMARY))
                                                                    .truncate()
                                                                    .child(li.name),
                                                            )
                                                            .child(
                                                                div()
                                                                    .flex()
                                                                    .items_center()
                                                                    .gap(px(7.0))
                                                                    .text_xs()
                                                                    .font_features(tabular_nums())
                                                                    .text_color(rgb(TEXT_MUTED))
                                                                    .child(sentence_case_label(
                                                                        &li.r#type,
                                                                    ))
                                                                    .when_some(
                                                                        ep_code,
                                                                        |el, ep| {
                                                                            el.child(meta_divider())
                                                                                .child(
                                                                                    div()
                                                                                        .font(mono_font())
                                                                                        .text_color(rgb(ACCENT))
                                                                                        .child(ep),
                                                                                )
                                                                        },
                                                                    ),
                                                            ),
                                                    )
                                            },
                                        ),
                                    ),
                            )
                            .when(remaining_lib > 0, |col| {
                                col.child(
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
                                                    "Showing {visible_count} of {total_filtered} titles"
                                                )),
                                        )
                                        .child(
                                            div()
                                                .id("btn-library-show-more")
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
                                                        this.visible_library_limit = (this
                                                            .visible_library_limit
                                                            .max(LIBRARY_PAGE_SIZE)
                                                            + LIBRARY_PAGE_SIZE)
                                                            .min(total_filtered);
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(format!(
                                                    "Show {} more",
                                                    remaining_lib.min(LIBRARY_PAGE_SIZE)
                                                ))
                                                .child(svg_icon(
                                                    IconName::ChevronDown,
                                                    13.0,
                                                    rgb(TEXT_MUTED),
                                                )),
                                        ),
                                )
                            })
                            .into_any_element()
                    }),
            )
    }
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
