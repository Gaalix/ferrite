use std::collections::HashMap;

use gpui::{div, img, prelude::*, px, rgb, rgba, Context, FontWeight, MouseButton};

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{classify_library_item_status, LibraryStatusTab},
    theme::{
        editorial_serif, meta_divider, mono_font, neutral_chip, poster_card_shadow, ui_sans,
        ACCENT, BG_ELEVATED, BG_SURFACE, BORDER_STRONG, BORDER_SUBTLE, TEXT_PRIMARY,
        TEXT_SECONDARY,
    },
    views::search_view::person_initials,
};

impl AppShell {
    pub fn render_person_view(
        &mut self,
        _route_person_key: &str,
        route_name: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let filmo = self.active_person_filmography.clone();
        let display_name = filmo
            .as_ref()
            .filter(|f| !f.name.trim().is_empty())
            .map(|f| f.name.clone())
            .unwrap_or_else(|| route_name.to_owned());
        let department = filmo
            .as_ref()
            .map(|f| f.department.clone())
            .unwrap_or_else(|| "Acting".to_owned());
        let headshot_tex = filmo
            .as_ref()
            .and_then(|f| f.headshot_url.as_deref())
            .and_then(|u| self.poster_cache.resolve_poster_image(u));
        let initials = person_initials(&display_name);

        let library_statuses: HashMap<String, LibraryStatusTab> = self.core.with_model(|model| {
            model
                .ctx
                .library
                .items
                .iter()
                .filter_map(|(id, li)| {
                    classify_library_item_status(li).map(|st| (id.clone(), st))
                })
                .collect()
        });

        let has_tmdb = {
            let cfg = self.core.private_integrations();
            cfg.tmdb_enabled && cfg.has_tmdb_configured()
        };

        div()
            .id("person-view-scroll")
            .track_scroll(&self.person_scroll_handle)
            .size_full()
            .overflow_y_scroll()
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
                    .gap(px(36.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .id("person-view-back")
                                    .cursor_pointer()
                                    .h(px(34.0))
                                    .px(px(12.0))
                                    .rounded(px(999.0))
                                    .bg(rgb(BG_SURFACE))
                                    .border_1()
                                    .border_color(rgba(BORDER_SUBTLE))
                                    .hover(|s| {
                                        s.bg(rgb(BG_ELEVATED))
                                            .border_color(rgba(BORDER_STRONG))
                                    })
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(px(13.0))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.navigate_back(cx);
                                        }),
                                    )
                                    .child(svg_icon(
                                        IconName::ChevronLeft,
                                        13.5,
                                        rgb(TEXT_SECONDARY),
                                    ))
                                    .child("Back"),
                            )
                            .when(self.person_loading, |el| {
                                el.child(neutral_chip("Loading filmography..."))
                            }),
                    )
                    .child(
                        div()
                            .p(px(28.0))
                            .rounded(px(12.0))
                            .bg(rgb(BG_SURFACE))
                            .border_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .flex()
                            .items_start()
                            .gap(px(28.0))
                            .child(
                                div()
                                    .w(px(128.0))
                                    .h(px(168.0))
                                    .flex_shrink_0()
                                    .rounded(px(8.0))
                                    .overflow_hidden()
                                    .bg(rgb(BG_ELEVATED))
                                    .border_1()
                                    .border_color(rgba(BORDER_SUBTLE))
                                    .shadow(poster_card_shadow())
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
                                            .font(editorial_serif())
                                            .text_size(px(40.0))
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
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .font(editorial_serif())
                                            .text_size(px(44.0))
                                            .line_height(gpui::relative(1.06))
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child(display_name),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .gap(px(10.0))
                                            .font(mono_font())
                                            .text_xs()
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(
                                                div()
                                                    .text_color(rgb(ACCENT))
                                                    .child(department),
                                            )
                                            .when_some(
                                                filmo.as_ref().and_then(|f| f.birthday.clone()),
                                                |el, bday| {
                                                    el.child(meta_divider())
                                                        .child(format!("Born {bday}"))
                                                },
                                            )
                                            .when_some(
                                                filmo
                                                    .as_ref()
                                                    .and_then(|f| f.place_of_birth.clone()),
                                                |el, pob| el.child(meta_divider()).child(pob),
                                            ),
                                    )
                                    .when_some(
                                        filmo
                                            .as_ref()
                                            .and_then(|f| f.biography.clone())
                                            .filter(|b| !b.trim().is_empty()),
                                        |el, bio| {
                                            el.child(
                                                div()
                                                    .mt(px(4.0))
                                                    .max_w(px(780.0))
                                                    .text_size(px(14.0))
                                                    .text_color(rgb(TEXT_SECONDARY))
                                                    .max_h(px(136.0))
                                                    .overflow_hidden()
                                                    .child(bio),
                                            )
                                        },
                                    ),
                            ),
                    )
                    .when_some(
                        filmo.as_ref().filter(|f| !f.movies.is_empty()),
                        |el, f| {
                            el.child(self.render_search_media_section(
                                "Movies",
                                "person-movie",
                                &f.movies,
                                &library_statuses,
                                cx,
                            ))
                        },
                    )
                    .when_some(
                        filmo.as_ref().filter(|f| !f.series.is_empty()),
                        |el, f| {
                            el.child(self.render_search_media_section(
                                "Series",
                                "person-series",
                                &f.series,
                                &library_statuses,
                                cx,
                            ))
                        },
                    )
                    .when(
                        !self.person_loading
                            && filmo
                                .as_ref()
                                .is_none_or(|f| f.movies.is_empty() && f.series.is_empty()),
                        |el| {
                            el.child(
                                div()
                                    .p(px(28.0))
                                    .rounded(px(12.0))
                                    .bg(rgb(BG_SURFACE))
                                    .border_1()
                                    .border_color(rgba(BORDER_SUBTLE))
                                    .text_size(px(14.0))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(if has_tmdb {
                                        "No filmography credits returned for this person."
                                    } else {
                                        "No cached titles matched locally yet. Filmographies populate automatically from browsed catalogs, or configure a TMDB API key in Settings -> Integrations & Keys for complete credits."
                                    }),
                            )
                        },
                    ),
            )
    }
}
