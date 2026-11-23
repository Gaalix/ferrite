use std::collections::HashMap;

use chrono::{Datelike, Local, Months, NaiveDate};
use gpui::{
    div, img, linear_color_stop, linear_gradient, prelude::*, px, rgb, rgba, AnyElement, Context,
    FontWeight, MouseButton, MouseDownEvent, ObjectFit, SharedString,
};
use stremio_core_runtime::stremio_core::{
    models::{calendar::YearMonthDate, common::Loadable},
    types::resource::{MetaItemPreview, Video},
};
use url::Url;

use crate::{
    icons::{svg_icon_weighted, IconName, IconWeight},
    shell::AppShell,
    state::{
        format_movietracker_episode_code, AppRoute, BrowsingContextMenu, KbFocusableCard,
        SidebarTab,
    },
    theme::{
        active_theme, editorial_serif, mono_font, placeholder_tint_for_id, sp, tabular_nums,
        ui_sans, ThemePalette, RADIUS_CARD, RADIUS_PILL, RADIUS_POSTER, RADIUS_SHEET,
    },
    ui_components::{
        format_notification_badge_label, glass_back_button, glass_segmented_bar,
        glass_segmented_item, primary_accent_button, secondary_glass_button,
    },
};

const WEEKDAYS_MON_FIRST: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MAX_EPISODE_PILLS_PER_CELL: usize = 4;

#[derive(Clone)]
struct CalendarEpisodeEntry {
    meta_id: String,
    meta_type: String,
    series_name: String,
    video_id: String,
    episode_badge_spaced: String,
    episode_badge_compact: String,
    episode_title: String,
    air_date: Option<NaiveDate>,
    thumbnail_url: Option<String>,
    backdrop_url: Option<String>,
    poster_url: Option<Url>,
    preview: MetaItemPreview,
    in_library: bool,
    is_item_watched: bool,
    is_episode_watched: bool,
    notification_count: usize,
}

#[derive(Clone)]
struct CwNotificationAlert {
    id: String,
    r#type: String,
    name: String,
    video_id: Option<String>,
    notifications: usize,
}

impl AppShell {
    pub fn render_calendar_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let local_today = Local::now().date_naive();

        let (
            selected,
            prev_ym,
            next_ym,
            today_day,
            days_in_month,
            first_weekday,
            is_loading,
            days_map,
            up_next_entries,
            total_episodes,
            active_days_count,
            cw_alerts,
        ) = self.core.with_model(|model| {
            let selected = model
                .calendar
                .selected
                .clone()
                .unwrap_or_else(|| YearMonthDate::from(local_today));

            let fallback_first = NaiveDate::from_ymd_opt(selected.year, selected.month, 1)
                .unwrap_or(local_today);

            let (prev_ym, next_ym) = if model.calendar.selected.is_some() {
                (
                    model.calendar.selectable.prev.clone(),
                    model.calendar.selectable.next.clone(),
                )
            } else {
                (
                    YearMonthDate::from(fallback_first - Months::new(1)),
                    YearMonthDate::from(fallback_first + Months::new(1)),
                )
            };

            let (today_day, days_in_month, first_weekday) = if model.calendar.month_info.days > 0 {
                (
                    model.calendar.month_info.today,
                    model.calendar.month_info.days,
                    model.calendar.month_info.first_weekday,
                )
            } else {
                let first_next = fallback_first + Months::new(1);
                let last_day = first_next.pred_opt().unwrap_or(fallback_first);
                let today_opt =
                    if local_today.year() == selected.year && local_today.month() == selected.month
                    {
                        Some(local_today.day())
                    } else {
                        None
                    };
                (
                    today_opt,
                    last_day.day(),
                    fallback_first.weekday().num_days_from_monday(),
                )
            };

            let is_loading = model
                .calendar
                .meta_items
                .iter()
                .any(|res| matches!(res.content, Some(Loadable::Loading)));

            let mut notif_by_id: HashMap<String, usize> = HashMap::new();
            let mut alerts: Vec<CwNotificationAlert> = Vec::new();
            for cw in &model.continue_watching_preview.items {
                if cw.notifications > 0 {
                    notif_by_id.insert(cw.library_item.id.clone(), cw.notifications);
                    alerts.push(CwNotificationAlert {
                        id: cw.library_item.id.clone(),
                        r#type: cw.library_item.r#type.clone(),
                        name: cw.library_item.name.clone(),
                        video_id: cw.library_item.state.video_id.clone(),
                        notifications: cw.notifications,
                    });
                }
            }

            let mut per_day: HashMap<u32, Vec<CalendarEpisodeEntry>> = HashMap::new();
            let mut this_week: Vec<CalendarEpisodeEntry> = Vec::new();
            let mut later_upcoming: Vec<CalendarEpisodeEntry> = Vec::new();
            let mut ep_count = 0_usize;
            let mut non_empty_days = 0_usize;

            for day_item in &model.calendar.items {
                if day_item.items.is_empty() {
                    continue;
                }
                non_empty_days += 1;
                let air_date = NaiveDate::from_ymd_opt(
                    day_item.date.year,
                    day_item.date.month,
                    day_item.date.day,
                );
                let delta_days = air_date.map(|d| (d - local_today).num_days());

                let mut day_entries = Vec::with_capacity(day_item.items.len());
                for content in &day_item.items {
                    ep_count += 1;
                    let meta = &content.meta_item;
                    let video = &content.video;
                    let lib_item = model.ctx.library.items.get(&meta.preview.id);
                    let in_library = lib_item.is_some_and(|li| !li.removed);
                    let is_item_watched = lib_item.is_some_and(|li| li.watched());
                    let is_episode_watched = lib_item.is_some_and(|li| {
                        li.state
                            .watched_bitfield(&meta.videos)
                            .get_video(&video.id)
                    });
                    let notification_count = notif_by_id
                        .get(&meta.preview.id)
                        .copied()
                        .unwrap_or(0);

                    let (badge_spaced, badge_compact) = format_episode_badges(video);
                    let ep_title = if video.title.trim().is_empty() {
                        badge_spaced.clone()
                    } else {
                        video.title.trim().to_owned()
                    };

                    let entry = CalendarEpisodeEntry {
                        meta_id: meta.preview.id.clone(),
                        meta_type: meta.preview.r#type.clone(),
                        series_name: meta.preview.name.clone(),
                        video_id: video.id.clone(),
                        episode_badge_spaced: badge_spaced,
                        episode_badge_compact: badge_compact,
                        episode_title: ep_title,
                        air_date,
                        thumbnail_url: video
                            .thumbnail
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned),
                        backdrop_url: meta
                            .preview
                            .background
                            .as_ref()
                            .map(|u| u.to_string()),
                        poster_url: meta.preview.poster.clone(),
                        preview: meta.preview.clone(),
                        in_library,
                        is_item_watched,
                        is_episode_watched,
                        notification_count,
                    };

                    if let Some(delta) = delta_days {
                        if (0..=7).contains(&delta) {
                            this_week.push(entry.clone());
                        } else if delta > 7 && later_upcoming.len() < 12 {
                            later_upcoming.push(entry.clone());
                        }
                    }

                    day_entries.push(entry);
                }

                per_day.insert(day_item.date.day, day_entries);
            }

            let strip_entries = if !this_week.is_empty() {
                this_week
            } else {
                later_upcoming
            };

            (
                selected,
                prev_ym,
                next_ym,
                today_day,
                days_in_month,
                first_weekday,
                is_loading,
                per_day,
                strip_entries,
                ep_count,
                non_empty_days,
                alerts,
            )
        });

        let t = active_theme();
        let side_pad = if self.viewport_size_px.0.is_finite() && self.viewport_size_px.0 > 0.0 {
            (self.viewport_size_px.0 * 0.03).clamp(24.0, 56.0)
        } else {
            32.0
        };

        let is_current_month =
            selected.year == local_today.year() && selected.month == local_today.month();
        let has_any_episodes = total_episodes > 0;
        let grid_cells = compute_month_grid_cells(days_in_month, first_weekday);
        let mut kb_cards: Vec<KbFocusableCard> = Vec::new();

        let prev_for_click = prev_ym.clone();
        let next_for_click = next_ym.clone();

        let cw_alerts_el = if cw_alerts.is_empty() {
            None
        } else {
            Some(
                self.render_cw_notification_strip(&cw_alerts, t, cx)
                    .into_any_element(),
            )
        };

        let up_next_el = if up_next_entries.is_empty() {
            None
        } else {
            Some(
                self.render_up_next_strip(&up_next_entries, local_today, &mut kb_cards, t, cx)
                    .into_any_element(),
            )
        };

        let body_el = if !has_any_episodes && !is_loading {
            self.render_calendar_empty_state(t, cx).into_any_element()
        } else {
            self.render_monthly_release_grid(
                &grid_cells,
                today_day,
                &days_map,
                &mut kb_cards,
                t,
                cx,
            )
            .into_any_element()
        };

        self.set_kb_focus_items(kb_cards);

        div()
            .id("calendar-view-scroll")
            .track_scroll(&self.calendar_scroll_handle)
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .px(px(side_pad))
                    .pt(sp(16.0))
                    .pb(sp(72.0))
                    .flex()
                    .flex_col()
                    .gap(sp(24.0))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_end()
                            .justify_between()
                            .gap(sp(16.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(sp(4.0))
                                    .child(
                                        div()
                                            .font(editorial_serif())
                                            .text_size(sp(38.0))
                                            .line_height(gpui::relative(1.06))
                                            .text_color(rgb(t.text_primary))
                                            .child("Release Calendar"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(sp(10.0))
                                            .text_size(sp(13.5))
                                            .text_color(rgb(t.text_secondary))
                                            .child(if is_loading {
                                                "Updating episode release dates from your library..."
                                                    .to_owned()
                                            } else if has_any_episodes {
                                                let ep_word = if total_episodes == 1 {
                                                    "episode"
                                                } else {
                                                    "episodes"
                                                };
                                                let day_word = if active_days_count == 1 {
                                                    "day"
                                                } else {
                                                    "days"
                                                };
                                                format!(
                                                    "{} {} · {total_episodes} {ep_word} across {active_days_count} {day_word}",
                                                    month_name(selected.month),
                                                    selected.year
                                                )
                                            } else {
                                                format!(
                                                    "{} {} · No tracked episode air dates",
                                                    month_name(selected.month),
                                                    selected.year
                                                )
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(sp(10.0))
                                    .child(glass_back_button(
                                        "cal-prev-month",
                                        format!(
                                            "{} {}",
                                            month_short_name(prev_ym.month),
                                            prev_ym.year
                                        ),
                                        cx.listener(move |this, _, _, cx| {
                                            this.core.load_calendar(Some(prev_for_click.clone()));
                                            cx.notify();
                                        }),
                                    ))
                                    .child(
                                        glass_segmented_bar("cal-month-bar")
                                            .child(
                                                div()
                                                    .px(sp(14.0))
                                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                                    .font_features(tabular_nums())
                                                    .text_size(sp(13.0))
                                                    .text_color(rgb(t.text_primary))
                                                    .whitespace_nowrap()
                                                    .child(format!(
                                                        "{} {}",
                                                        month_name(selected.month),
                                                        selected.year
                                                    )),
                                            )
                                            .child(glass_segmented_item(
                                                "cal-today-btn",
                                                "Today",
                                                is_current_month,
                                                cx.listener(|this, _, _, cx| {
                                                    this.core.load_calendar(None);
                                                    cx.notify();
                                                }),
                                            )),
                                    )
                                    .child(
                                        div()
                                            .id("cal-next-month")
                                            .cursor_pointer()
                                            .h(sp(34.0))
                                            .px(sp(12.0))
                                            .rounded(px(RADIUS_PILL))
                                            .bg(rgba(t.glass_trough_bg))
                                            .border_1()
                                            .border_color(rgba(t.glass_border))
                                            .hover(move |s| {
                                                s.bg(rgba(t.glass_row_hover_bg))
                                                    .border_color(rgba(t.glass_border_strong))
                                            })
                                            .flex()
                                            .items_center()
                                            .gap(sp(6.0))
                                            .font(ui_sans(FontWeight::MEDIUM))
                                            .text_size(sp(13.0))
                                            .text_color(rgb(t.text_secondary))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.core
                                                        .load_calendar(Some(next_for_click.clone()));
                                                    cx.notify();
                                                }),
                                            )
                                            .child(format!(
                                                "{} {}",
                                                month_short_name(next_ym.month),
                                                next_ym.year
                                            ))
                                            .child(svg_icon_weighted(
                                                IconName::ChevronRight,
                                                IconWeight::Regular,
                                                14.0 * t.ui_scale,
                                                rgb(t.text_secondary),
                                            )),
                                    ),
                            ),
                    )
                    .when_some(cw_alerts_el, |root, el| root.child(el))
                    .when_some(up_next_el, |root, el| root.child(el))
                    .child(body_el),
            )
    }

    fn render_cw_notification_strip(
        &self,
        alerts: &[CwNotificationAlert],
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let accent_ink = if t.is_light { t.accent_press } else { t.accent };
        let mut alert_chips: Vec<AnyElement> = Vec::with_capacity(alerts.len());
        for (idx, alert) in alerts.iter().enumerate() {
            let open_type = alert.r#type.clone();
            let open_id = alert.id.clone();
            let open_vid = alert.video_id.clone();
            let dismiss_id = alert.id.clone();
            let badge = format_notification_badge_label(alert.notifications)
                .unwrap_or_else(|| SharedString::new_static("NEW"));

            alert_chips.push(
                div()
                    .id(("cal-notif-chip", idx))
                    .cursor_pointer()
                    .h(sp(30.0))
                    .pl(sp(10.0))
                    .pr(sp(6.0))
                    .rounded(px(RADIUS_PILL))
                    .bg(rgba(t.glass_trough_bg))
                    .border_1()
                    .border_color(rgba(t.glass_border))
                    .hover(move |s| {
                        s.bg(rgba(t.glass_row_hover_bg))
                            .border_color(rgba(t.glass_border_strong))
                    })
                    .flex()
                    .items_center()
                    .gap(sp(7.0))
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
                            .px(sp(6.0))
                            .py(sp(1.5))
                            .rounded(px(RADIUS_PILL))
                            .bg(rgb(t.accent))
                            .font(mono_font())
                            .font_weight(FontWeight::BOLD)
                            .text_size(sp(9.5))
                            .text_color(rgb(t.cta_on_accent))
                            .child(badge),
                    )
                    .child(
                        div()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(sp(12.5))
                            .text_color(rgb(t.text_primary))
                            .child(alert.name.clone()),
                    )
                    .child(
                        div()
                            .id(("cal-notif-dismiss", idx))
                            .cursor_pointer()
                            .w(sp(20.0))
                            .h(sp(20.0))
                            .rounded(px(RADIUS_PILL))
                            .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.core.dismiss_notification_item(dismiss_id.clone());
                                    cx.notify();
                                }),
                            )
                            .child(svg_icon_weighted(
                                IconName::X,
                                IconWeight::Regular,
                                11.0 * t.ui_scale,
                                rgb(t.text_muted),
                            )),
                    )
                    .into_any_element(),
            );
        }

        div()
            .p(sp(14.0))
            .rounded(px(RADIUS_CARD))
            .bg(rgba(t.glass_card_bg))
            .border_1()
            .border_color(rgba(t.glass_border))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(sp(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(6.0))
                    .font(mono_font())
                    .text_xs()
                    .text_color(rgb(accent_ink))
                    .child(svg_icon_weighted(
                        IconName::Sparkles,
                        IconWeight::Fill,
                        12.5 * t.ui_scale,
                        rgb(accent_ink),
                    ))
                    .child("NEW EPISODE ALERTS"),
            )
            .children(alert_chips)
    }

    fn render_up_next_strip(
        &self,
        entries: &[CalendarEpisodeEntry],
        local_today: NaiveDate,
        kb_cards: &mut Vec<KbFocusableCard>,
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut cards: Vec<AnyElement> = Vec::with_capacity(entries.len());
        for (idx, entry) in entries.iter().enumerate() {
            let card_tex = if let Some(thumb) = entry.thumbnail_url.as_deref() {
                self.poster_cache.resolve_episode_still_image(thumb)
            } else if let Some(bg) = entry.backdrop_url.as_deref() {
                self.poster_cache.resolve_episode_still_image(bg)
            } else {
                self.poster_cache
                    .resolve_poster_url(entry.poster_url.as_ref())
            };

            let (date_label, is_today_pill) = entry
                .air_date
                .map(|d| classify_air_pill((d - local_today).num_days(), d))
                .unwrap_or_else(|| ("UPCOMING".to_owned(), false));

            let notif_badge = format_notification_badge_label(entry.notification_count);

            let kb_idx = kb_cards.len();
            let is_focused = self.kb_focus_index == Some(kb_idx);
            kb_cards.push(KbFocusableCard {
                route: AppRoute::MetaDetails {
                    type_name: entry.meta_type.clone(),
                    id: entry.meta_id.clone(),
                    video_id: Some(entry.video_id.clone()),
                },
                row: 0,
                col: idx,
            });

            let open_type = entry.meta_type.clone();
            let open_id = entry.meta_id.clone();
            let open_vid = entry.video_id.clone();

            let ctx_type = entry.meta_type.clone();
            let ctx_id = entry.meta_id.clone();
            let ctx_vid = entry.video_id.clone();
            let ctx_title = format!("{} · {}", entry.series_name, entry.episode_badge_spaced);
            let ctx_preview = entry.preview.clone();
            let ctx_in_lib = entry.in_library;
            let ctx_watched = entry.is_item_watched;

            let hover_type = entry.meta_type.clone();
            let hover_id = entry.meta_id.clone();
            let hover_bg = entry.backdrop_url.clone();

            cards.push(
                div()
                    .id(("cal-upnext-card", idx))
                    .cursor_pointer()
                    .w(sp(276.0))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(sp(8.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.open_meta_details(
                                open_type.clone(),
                                open_id.clone(),
                                Some(open_vid.clone()),
                                cx,
                            );
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_browsing_context_menu(
                                BrowsingContextMenu::new(
                                    ev.position,
                                    ctx_type.clone(),
                                    ctx_id.clone(),
                                    ctx_title.clone(),
                                )
                                .video_id(Some(ctx_vid.clone()))
                                .preview(Some(ctx_preview.clone()))
                                .is_in_library(ctx_in_lib)
                                .is_watched(ctx_watched),
                                cx,
                            );
                        }),
                    )
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.on_card_hover_prefetch(
                            &hover_type,
                            &hover_id,
                            hover_bg.as_deref(),
                            *hovered,
                            cx,
                        );
                    }))
                    .child(
                        div()
                            .w_full()
                            .h(sp(155.0))
                            .rounded(px(RADIUS_POSTER))
                            .overflow_hidden()
                            .relative()
                            .bg(rgb(placeholder_tint_for_id(&entry.meta_id)))
                            .border_1()
                            .border_color(if is_focused {
                                rgb(t.accent)
                            } else if is_today_pill {
                                rgba(t.border_accent)
                            } else {
                                rgba(t.glass_border)
                            })
                            .hover(move |s| s.border_color(rgba(t.border_poster_hover)))
                            .child(if let Some(tex) = card_tex {
                                img(tex)
                                    .size_full()
                                    .object_fit(ObjectFit::Cover)
                                    .into_any_element()
                            } else {
                                div()
                                    .size_full()
                                    .p(sp(14.0))
                                    .bg(rgb(t.bg_surface))
                                    .flex()
                                    .items_end()
                                    .child(
                                        div()
                                            .w_full()
                                            .font(editorial_serif())
                                            .text_size(sp(20.0))
                                            .text_color(rgb(t.text_secondary))
                                            .truncate()
                                            .child(entry.series_name.clone()),
                                    )
                                    .into_any_element()
                            })
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .bg(linear_gradient(
                                        180.0,
                                        linear_color_stop(rgba(0x0B0D1414), 0.2),
                                        linear_color_stop(rgba(0x0B0D14E0), 1.0),
                                    )),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(8.0))
                                    .left(px(8.0))
                                    .h(sp(22.0))
                                    .px(sp(8.0))
                                    .rounded(px(RADIUS_PILL))
                                    .bg(if is_today_pill {
                                        rgb(t.accent)
                                    } else {
                                        rgba(0x0B0D14D9)
                                    })
                                    .border_1()
                                    .border_color(if is_today_pill {
                                        rgb(t.accent)
                                    } else {
                                        rgba(0xFFFFFF26)
                                    })
                                    .flex()
                                    .items_center()
                                    .font(mono_font())
                                    .font_weight(FontWeight::BOLD)
                                    .text_size(sp(10.0))
                                    .text_color(if is_today_pill {
                                        rgb(t.cta_on_accent)
                                    } else {
                                        rgb(0xF5F1E8)
                                    })
                                    .child(date_label),
                            )
                            .when_some(notif_badge, |art, badge| {
                                art.child(
                                    div()
                                        .absolute()
                                        .top(px(8.0))
                                        .right(px(8.0))
                                        .h(sp(20.0))
                                        .px(sp(7.0))
                                        .rounded(px(RADIUS_PILL))
                                        .bg(rgb(t.accent))
                                        .flex()
                                        .items_center()
                                        .font(mono_font())
                                        .font_weight(FontWeight::BOLD)
                                        .text_size(sp(10.0))
                                        .text_color(rgb(t.cta_on_accent))
                                        .child(badge),
                                )
                            })
                            .child(
                                div()
                                    .absolute()
                                    .bottom(px(8.0))
                                    .left(px(8.0))
                                    .px(sp(7.0))
                                    .py(sp(2.5))
                                    .rounded(px(6.0))
                                    .bg(rgba(0x0B0D14D9))
                                    .border_1()
                                    .border_color(rgba(0xFFFFFF26))
                                    .font(mono_font())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(sp(10.5))
                                    .text_color(if entry.is_episode_watched {
                                        rgb(0x4ADE80)
                                    } else {
                                        rgb(t.accent)
                                    })
                                    .child(entry.episode_badge_spaced.clone()),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(sp(2.0))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(sp(13.5))
                                    .text_color(rgb(t.text_primary))
                                    .truncate()
                                    .child(entry.series_name.clone()),
                            )
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::NORMAL))
                                    .text_size(sp(12.0))
                                    .text_color(rgb(t.text_secondary))
                                    .truncate()
                                    .child(entry.episode_title.clone()),
                            ),
                    )
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(sp(14.0))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .gap(sp(12.0))
                    .child(
                        div()
                            .font(editorial_serif())
                            .text_size(sp(26.0))
                            .text_color(rgb(t.text_primary))
                            .child("Airing This Week & Up Next"),
                    )
                    .child(
                        div()
                            .font(mono_font())
                            .text_xs()
                            .text_color(rgb(t.text_muted))
                            .child(format!("{} upcoming", entries.len())),
                    ),
            )
            .child(
                div()
                    .id("cal-upnext-strip")
                    .w_full()
                    .overflow_x_scroll()
                    .child(div().flex().items_start().gap(sp(16.0)).children(cards)),
            )
    }

    fn render_monthly_release_grid(
        &self,
        grid_cells: &[Option<u32>],
        today_day: Option<u32>,
        days_map: &HashMap<u32, Vec<CalendarEpisodeEntry>>,
        kb_cards: &mut Vec<KbFocusableCard>,
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let accent_ink = if t.is_light { t.accent_press } else { t.accent };
        let empty_cell_bg = if t.is_light { 0x0F172A06 } else { 0xFFFFFF03 };
        let mut week_rows: Vec<AnyElement> = Vec::new();

        for (week_idx, week) in grid_cells.chunks(7).enumerate() {
            let kb_row = week_idx + 1;
            let mut row_col = 0_usize;
            let mut day_cells: Vec<AnyElement> = Vec::with_capacity(7);

            for (col_idx, cell_day) in week.iter().enumerate() {
                let Some(day) = *cell_day else {
                    day_cells.push(
                        div()
                            .id(("cal-pad-cell", week_idx * 7 + col_idx))
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(sp(120.0))
                            .rounded(px(RADIUS_CARD))
                            .bg(rgba(empty_cell_bg))
                            .border_1()
                            .border_color(rgba(t.glass_divider))
                            .into_any_element(),
                    );
                    continue;
                };

                let is_today = today_day == Some(day);
                let entries = days_map.get(&day).map(Vec::as_slice).unwrap_or(&[]);
                let hidden_count = entries.len().saturating_sub(MAX_EPISODE_PILLS_PER_CELL);

                let mut ep_pills: Vec<AnyElement> =
                    Vec::with_capacity(entries.len().min(MAX_EPISODE_PILLS_PER_CELL));
                for (ep_idx, entry) in entries
                    .iter()
                    .take(MAX_EPISODE_PILLS_PER_CELL)
                    .enumerate()
                {
                    let kb_idx = kb_cards.len();
                    let is_focused = self.kb_focus_index == Some(kb_idx);
                    kb_cards.push(KbFocusableCard {
                        route: AppRoute::MetaDetails {
                            type_name: entry.meta_type.clone(),
                            id: entry.meta_id.clone(),
                            video_id: Some(entry.video_id.clone()),
                        },
                        row: kb_row,
                        col: row_col,
                    });
                    row_col += 1;

                    let open_type = entry.meta_type.clone();
                    let open_id = entry.meta_id.clone();
                    let open_vid = entry.video_id.clone();

                    let ctx_type = entry.meta_type.clone();
                    let ctx_id = entry.meta_id.clone();
                    let ctx_vid = entry.video_id.clone();
                    let ctx_title =
                        format!("{} · {}", entry.series_name, entry.episode_badge_spaced);
                    let ctx_preview = entry.preview.clone();
                    let ctx_in_lib = entry.in_library;
                    let ctx_watched = entry.is_item_watched;

                    ep_pills.push(
                        div()
                            .id(("cal-day-ep", (day as usize) * 1000 + ep_idx))
                            .cursor_pointer()
                            .w_full()
                            .min_w(px(0.0))
                            .px(sp(7.0))
                            .py(sp(4.0))
                            .rounded(px(6.0))
                            .bg(rgba(t.glass_trough_bg))
                            .border_1()
                            .border_color(if is_focused {
                                rgb(t.accent)
                            } else {
                                rgba(t.glass_border)
                            })
                            .hover(move |s| {
                                s.bg(rgba(t.glass_row_hover_bg))
                                    .border_color(rgba(t.border_poster_hover))
                            })
                            .flex()
                            .items_center()
                            .gap(sp(5.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_meta_details(
                                        open_type.clone(),
                                        open_id.clone(),
                                        Some(open_vid.clone()),
                                        cx,
                                    );
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.open_browsing_context_menu(
                                        BrowsingContextMenu::new(
                                            ev.position,
                                            ctx_type.clone(),
                                            ctx_id.clone(),
                                            ctx_title.clone(),
                                        )
                                        .video_id(Some(ctx_vid.clone()))
                                        .preview(Some(ctx_preview.clone()))
                                        .is_in_library(ctx_in_lib)
                                        .is_watched(ctx_watched),
                                        cx,
                                    );
                                }),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .font(mono_font())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(sp(10.0))
                                    .text_color(if entry.is_episode_watched {
                                        rgb(t.status_watched)
                                    } else {
                                        rgb(accent_ink)
                                    })
                                    .child(entry.episode_badge_compact.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(sp(11.5))
                                    .text_color(rgb(t.text_primary))
                                    .truncate()
                                    .child(entry.series_name.clone()),
                            )
                            .into_any_element(),
                    );
                }

                day_cells.push(
                    div()
                        .id(("cal-day-cell", day as usize))
                        .flex_1()
                        .min_w(px(0.0))
                        .min_h(sp(120.0))
                        .p(sp(10.0))
                        .rounded(px(RADIUS_CARD))
                        .bg(if is_today {
                            rgba(t.accent_soft)
                        } else {
                            rgba(t.glass_card_bg)
                        })
                        .border_1()
                        .border_color(if is_today {
                            rgb(t.accent)
                        } else {
                            rgba(t.glass_border)
                        })
                        .flex()
                        .flex_col()
                        .gap(sp(6.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(sp(6.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(sp(6.0))
                                        .child(
                                            div()
                                                .font(ui_sans(if is_today {
                                                    FontWeight::BOLD
                                                } else {
                                                    FontWeight::SEMIBOLD
                                                }))
                                                .font_features(tabular_nums())
                                                .text_size(sp(13.0))
                                                .text_color(if is_today {
                                                    rgb(accent_ink)
                                                } else if !entries.is_empty() {
                                                    rgb(t.text_primary)
                                                } else {
                                                    rgb(t.text_muted)
                                                })
                                                .child(day.to_string()),
                                        )
                                        .when(is_today, |hdr| {
                                            hdr.child(
                                                div()
                                                    .px(sp(6.0))
                                                    .py(sp(1.5))
                                                    .rounded(px(RADIUS_PILL))
                                                    .bg(rgb(t.accent))
                                                    .font(mono_font())
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_size(sp(9.0))
                                                    .text_color(rgb(t.cta_on_accent))
                                                    .child("TODAY"),
                                            )
                                        }),
                                )
                                .when(!entries.is_empty(), |hdr| {
                                    hdr.child(
                                        div()
                                            .px(sp(6.0))
                                            .py(sp(1.0))
                                            .rounded(px(RADIUS_PILL))
                                            .bg(rgba(t.glass_trough_bg))
                                            .font(mono_font())
                                            .text_size(sp(10.0))
                                            .text_color(rgb(t.text_secondary))
                                            .child(entries.len().to_string()),
                                    )
                                }),
                        )
                        .children(ep_pills)
                        .when(hidden_count > 0, |cell| {
                            cell.child(
                                div()
                                    .px(sp(4.0))
                                    .font(mono_font())
                                    .text_size(sp(10.5))
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("+{hidden_count} more")),
                            )
                        })
                        .into_any_element(),
                );
            }

            week_rows.push(
                div()
                    .w_full()
                    .flex()
                    .items_stretch()
                    .gap(sp(8.0))
                    .children(day_cells)
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(sp(8.0))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(sp(8.0))
                    .children(WEEKDAYS_MON_FIRST.into_iter().map(|day_name| {
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .px(sp(10.0))
                            .py(sp(6.0))
                            .font(mono_font())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(sp(11.5))
                            .text_color(rgb(t.text_muted))
                            .child(day_name)
                    })),
            )
            .children(week_rows)
    }

    fn render_calendar_empty_state(&self, t: ThemePalette, cx: &mut Context<Self>) -> impl IntoElement {
        let accent_ink = if t.is_light { t.accent_press } else { t.accent };
        div()
            .w_full()
            .p(sp(36.0))
            .rounded(px(RADIUS_SHEET))
            .bg(rgba(t.glass_card_bg))
            .border_1()
            .border_color(rgba(t.glass_border))
            .flex()
            .flex_col()
            .items_start()
            .gap(sp(12.0))
            .child(
                div()
                    .w(sp(42.0))
                    .h(sp(42.0))
                    .rounded(px(RADIUS_CARD))
                    .bg(rgba(t.accent_soft))
                    .border_1()
                    .border_color(rgba(t.border_accent))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(svg_icon_weighted(
                        IconName::Calendar,
                        IconWeight::Fill,
                        20.0 * t.ui_scale,
                        rgb(accent_ink),
                    )),
            )
            .child(
                div()
                    .font(editorial_serif())
                    .text_size(sp(30.0))
                    .text_color(rgb(t.text_primary))
                    .child("No releases scheduled this month"),
            )
            .child(
                div()
                    .max_w(px(540.0))
                    .text_size(sp(14.0))
                    .text_color(rgb(t.text_secondary))
                    .child(
                        "Add ongoing series to your library to track upcoming episode air dates here.",
                    ),
            )
            .child(
                div()
                    .mt(sp(6.0))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(sp(10.0))
                    .child(primary_accent_button(
                        "cal-empty-discover",
                        Some(IconName::Compass),
                        "Browse Discover",
                        cx.listener(|this, _, _, cx| {
                            this.navigate_tab(SidebarTab::Discover, cx);
                        }),
                    ))
                    .child(secondary_glass_button(
                        "cal-empty-library",
                        Some(IconName::Bookmark),
                        "Open Library",
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.navigate_tab(SidebarTab::Library, cx);
                        }),
                    )),
            )
    }
}

fn compute_month_grid_cells(days: u32, first_weekday: u32) -> Vec<Option<u32>> {
    if days == 0 {
        return Vec::new();
    }
    // `stremio-core` sets `first_weekday = first_day.weekday().num_days_from_monday()`,
    // so 0 = Monday .. 6 = Sunday.
    let leading = (first_weekday as usize) % 7;
    let total_so_far = leading + days as usize;
    let trailing = (7 - (total_so_far % 7)) % 7;
    let mut cells = Vec::with_capacity(total_so_far + trailing);
    cells.extend(std::iter::repeat_n(None, leading));
    for d in 1..=days {
        cells.push(Some(d));
    }
    cells.extend(std::iter::repeat_n(None, trailing));
    cells
}

fn format_episode_badges(video: &Video) -> (String, String) {
    if let Some(si) = &video.series_info {
        (
            format!("S{} · E{}", si.season, si.episode),
            format!("S{}E{}", si.season, si.episode),
        )
    } else if let Some(spaced) = format_movietracker_episode_code(Some(&video.id)) {
        let compact = spaced.replace(' ', "");
        (spaced, compact)
    } else {
        ("EP".to_owned(), "EP".to_owned())
    }
}

fn classify_air_pill(delta_days: i64, date: NaiveDate) -> (String, bool) {
    match delta_days {
        0 => ("TODAY".to_owned(), true),
        1 => ("TOMORROW".to_owned(), false),
        _ => (
            format!("{} {}", month_short_name(date.month()), date.day()),
            false,
        ),
    }
}

const fn month_name(month: u32) -> &'static str {
    match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "Month",
    }
}

const fn month_short_name(month: u32) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "Mon",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_grid_aligns_monday_first_and_pads_to_full_weeks() {
        // Monday start (first_weekday = 0), 30 days -> 0 leading, 5 trailing = 35 cells
        let mon = compute_month_grid_cells(30, 0);
        assert_eq!(mon.len(), 35);
        assert_eq!(mon[0], Some(1));
        assert_eq!(mon[29], Some(30));
        assert_eq!(mon[30], None);

        // Sunday start (first_weekday = 6, e.g. Nov 2026), 30 days -> 6 leading, 6 trailing = 42
        let sun = compute_month_grid_cells(30, 6);
        assert_eq!(sun.len(), 42);
        assert!(sun[..6].iter().all(Option::is_none));
        assert_eq!(sun[6], Some(1));
        assert_eq!(sun[35], Some(30));
        assert!(sun[36..].iter().all(Option::is_none));
    }

    #[test]
    fn air_pill_classifies_today_tomorrow_and_future_dates() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
        assert_eq!(classify_air_pill(0, d), ("TODAY".to_owned(), true));
        assert_eq!(classify_air_pill(1, d), ("TOMORROW".to_owned(), false));
        assert_eq!(classify_air_pill(3, d), ("Nov 16".to_owned(), false));
    }
}
