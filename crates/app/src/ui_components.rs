use std::sync::Arc;

use gpui::{
    div, img, linear_color_stop, linear_gradient, prelude::*, px, relative, rgb, rgba, AnyElement,
    App, Div, ElementId, FontWeight, MouseButton, MouseDownEvent, ObjectFit, RenderImage,
    SharedString, Stateful, Window,
};

use crate::{
    icons::{svg_icon, IconName},
    state::LibraryStatusTab,
    theme::{
        editorial_serif, glass_capsule_shadow, mono_font, placeholder_tint_for_id,
        poster_card_shadow, tabular_nums, ui_sans, ACCENT, ACCENT_HOVER, ACCENT_SOFT, BG_OBSIDIAN,
        BG_SURFACE, BORDER_ACCENT, BORDER_POSTER_HOVER, CTA_ON_ACCENT, GLASS_BORDER,
        GLASS_BORDER_STRONG, GLASS_CARD_BG, GLASS_DIVIDER, GLASS_ROW_HOVER_BG,
        GLASS_ROW_SELECTED_BG, GLASS_SHEET_BG, GLASS_TROUGH_BG, RADIUS_CARD, RADIUS_CONTROL,
        RADIUS_PILL, RADIUS_POSTER, SEGMENT_ACTIVE_BG, SEGMENT_ACTIVE_TEXT, STATUS_WATCHED,
        STATUS_WATCHING, STATUS_WATCHLIST, TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY,
    },
    virtual_grid::PosterAspect,
};

/// Click callback alias compatible with GPUI `cx.listener(...)` handlers.
pub type ClickHandler = Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>;

/// Hover callback alias compatible with GPUI `cx.listener(...)` handlers.
pub type HoverHandler = Box<dyn Fn(&bool, &mut Window, &mut App) + 'static>;

/// Boxes any GPUI mouse-down listener into a `ClickHandler`.
#[inline]
pub fn click_handler(
    f: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> ClickHandler {
    Box::new(f)
}

/// Boxes any GPUI hover listener into a `HoverHandler`.
#[inline]
pub fn hover_handler(f: impl Fn(&bool, &mut Window, &mut App) + 'static) -> HoverHandler {
    Box::new(f)
}

/// Frosted segmented control trough matching the `v0.4.5` player sheet aesthetic.
pub fn glass_segmented_bar(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .p(px(3.0))
        .rounded(px(RADIUS_CONTROL))
        .bg(rgba(GLASS_TROUGH_BG))
        .border_1()
        .border_color(rgba(GLASS_BORDER))
        .flex()
        .items_center()
        .gap(px(2.0))
}

/// Individual segment button inside a `glass_segmented_bar`.
pub fn glass_segmented_item(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let base = div()
        .id(id)
        .h(px(30.0))
        .px(px(12.0))
        .rounded(px(RADIUS_POSTER))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .whitespace_nowrap()
        .text_size(px(12.5))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        });

    if active {
        base.bg(rgb(SEGMENT_ACTIVE_BG))
            .font(ui_sans(FontWeight::SEMIBOLD))
            .text_color(rgb(SEGMENT_ACTIVE_TEXT))
    } else {
        base.bg(rgba(0x00000000))
            .hover(|s| s.bg(rgba(GLASS_ROW_HOVER_BG)).text_color(rgb(TEXT_PRIMARY)))
            .font(ui_sans(FontWeight::MEDIUM))
            .text_color(rgb(TEXT_SECONDARY))
    }
    .child(label.into())
}

/// Unified `v0.4.5` 40x22 glass toggle switch with crisp white thumb.
pub fn glass_switch(
    id: impl Into<ElementId>,
    checked: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(40.0))
        .h(px(22.0))
        .flex_shrink_0()
        .rounded_full()
        .p(px(2.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .when(checked, |s| s.bg(rgb(ACCENT)).justify_end())
        .when(!checked, |s| s.bg(rgba(0xFFFFFF1E)).justify_start())
        .hover(|s| {
            if checked {
                s.bg(rgb(ACCENT_HOVER))
            } else {
                s.bg(rgba(0xFFFFFF2B))
            }
        })
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(
            div()
                .w(px(18.0))
                .h(px(18.0))
                .rounded_full()
                .bg(rgb(0xFFFFFF)),
        )
}

/// Pill-trough numeric stepper with circular 24x24 `-` / `+` buttons and `tabular_nums` reset readout.
pub fn glass_stepper(
    id_prefix: impl Into<SharedString>,
    value_label: impl Into<SharedString>,
    on_dec: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_reset: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_inc: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let prefix = id_prefix.into();
    let dec_id: SharedString = format!("{prefix}-dec").into();
    let reset_id: SharedString = format!("{prefix}-reset").into();
    let inc_id: SharedString = format!("{prefix}-inc").into();

    div()
        .h(px(34.0))
        .min_w(px(114.0))
        .rounded_full()
        .bg(rgba(GLASS_TROUGH_BG))
        .border_1()
        .border_color(rgba(GLASS_BORDER))
        .px(px(6.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(4.0))
        .child(
            div()
                .id(dec_id)
                .w(px(24.0))
                .h(px(24.0))
                .rounded_full()
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .hover(|s| s.bg(rgba(GLASS_ROW_HOVER_BG)))
                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                    cx.stop_propagation();
                    on_dec(ev, window, cx);
                })
                .child(svg_icon(IconName::Minus, 13.0, rgb(TEXT_SECONDARY))),
        )
        .child(
            div()
                .id(reset_id)
                .flex_1()
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .whitespace_nowrap()
                .hover(|s| s.bg(rgba(GLASS_ROW_HOVER_BG)))
                .font(ui_sans(FontWeight::MEDIUM))
                .font_features(tabular_nums())
                .text_size(px(12.5))
                .text_color(rgb(TEXT_PRIMARY))
                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                    cx.stop_propagation();
                    on_reset(ev, window, cx);
                })
                .child(value_label.into()),
        )
        .child(
            div()
                .id(inc_id)
                .w(px(24.0))
                .h(px(24.0))
                .rounded_full()
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .hover(|s| s.bg(rgba(GLASS_ROW_HOVER_BG)))
                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                    cx.stop_propagation();
                    on_inc(ev, window, cx);
                })
                .child(svg_icon(IconName::Plus, 13.0, rgb(TEXT_SECONDARY))),
        )
}

/// Floating frosted dropdown/context-menu panel container (`rounded(14)`, `GLASS_SHEET_BG`, `GLASS_BORDER`).
pub fn glass_dropdown_panel(id: impl Into<ElementId>, width_px: f32) -> Stateful<Div> {
    div()
        .id(id)
        .occlude()
        .w(px(width_px))
        .p(px(6.0))
        .rounded(px(14.0))
        .bg(rgba(GLASS_SHEET_BG))
        .border_1()
        .border_color(rgba(GLASS_BORDER))
        .shadow(glass_capsule_shadow())
        .flex()
        .flex_col()
        .gap(px(2.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// Selectable row inside a `glass_dropdown_panel` with optional sublabel and trailing `Check` icon.
pub fn glass_dropdown_option(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    sublabel: Option<SharedString>,
    selected: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .flex_shrink_0()
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(RADIUS_CONTROL))
        .cursor_pointer()
        .bg(if selected {
            rgba(GLASS_ROW_SELECTED_BG)
        } else {
            rgba(0x00000000)
        })
        .hover(|s| s.bg(rgba(GLASS_ROW_HOVER_BG)))
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
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .font(ui_sans(if selected {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        }))
                        .text_size(px(13.0))
                        .text_color(rgb(TEXT_PRIMARY))
                        .truncate()
                        .child(label.into()),
                )
                .when_some(sublabel, |d, sub| {
                    d.child(
                        div()
                            .font(ui_sans(FontWeight::NORMAL))
                            .text_size(px(11.5))
                            .text_color(rgb(TEXT_MUTED))
                            .truncate()
                            .child(sub),
                    )
                }),
        )
        .when(selected, |d| {
            d.child(svg_icon(IconName::Check, 14.0, rgb(ACCENT)))
        })
}

/// Pill-shaped frosted back button used across MetaDetails, Search, and Person views.
pub fn glass_back_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .h(px(34.0))
        .px(px(12.0))
        .rounded(px(RADIUS_PILL))
        .bg(rgba(GLASS_TROUGH_BG))
        .border_1()
        .border_color(rgba(GLASS_BORDER))
        .hover(|s| {
            s.bg(rgba(GLASS_ROW_HOVER_BG))
                .border_color(rgba(GLASS_BORDER_STRONG))
        })
        .flex()
        .items_center()
        .gap(px(6.0))
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(px(13.0))
        .text_color(rgb(TEXT_SECONDARY))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(svg_icon(IconName::ChevronLeft, 14.0, rgb(TEXT_SECONDARY)))
        .child(label.into())
}

/// Primary coral accent CTA button with dark `CTA_ON_ACCENT` text and optional leading icon.
pub fn primary_accent_button(
    id: impl Into<ElementId>,
    icon: Option<IconName>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .h(px(40.0))
        .px(px(18.0))
        .rounded(px(RADIUS_CONTROL))
        .bg(rgb(ACCENT))
        .hover(|s| s.bg(rgb(ACCENT_HOVER)))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .font(ui_sans(FontWeight::SEMIBOLD))
        .font_features(tabular_nums())
        .text_size(px(13.5))
        .text_color(rgb(CTA_ON_ACCENT))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .when_some(icon, |d, ic| {
            d.child(svg_icon(ic, 14.5, rgb(CTA_ON_ACCENT)))
        })
        .child(label.into())
}

/// Secondary frosted-glass action button with optional active accent highlight.
pub fn secondary_glass_button(
    id: impl Into<ElementId>,
    icon: Option<IconName>,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let fg = if active { ACCENT } else { TEXT_PRIMARY };
    div()
        .id(id)
        .cursor_pointer()
        .h(px(40.0))
        .px(px(16.0))
        .rounded(px(RADIUS_CONTROL))
        .bg(if active {
            rgba(ACCENT_SOFT)
        } else {
            rgba(GLASS_TROUGH_BG)
        })
        .border_1()
        .border_color(if active {
            rgba(BORDER_ACCENT)
        } else {
            rgba(GLASS_BORDER)
        })
        .hover(|s| {
            s.bg(if active {
                rgba(ACCENT_SOFT)
            } else {
                rgba(GLASS_ROW_HOVER_BG)
            })
            .border_color(if active {
                rgb(ACCENT)
            } else {
                rgba(GLASS_BORDER_STRONG)
            })
        })
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(px(13.5))
        .text_color(rgb(fg))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .when_some(icon, |d, ic| d.child(svg_icon(ic, 14.5, rgb(fg))))
        .child(label.into())
}

/// Props for the unified poster card shared across Home, Discover, Library, Calendar, Search, Person, and MetaDetails.
pub struct PosterCardProps {
    pub id: ElementId,
    pub title: SharedString,
    pub subtitle: Option<SharedString>,
    pub image: Option<Arc<RenderImage>>,
    pub placeholder_id: String,
    pub aspect: PosterAspect,
    pub fixed_width_px: Option<f32>,
    pub image_height_px: Option<f32>,
    pub status_tab: Option<LibraryStatusTab>,
    pub notification_count: usize,
    pub imdb_rating: Option<SharedString>,
    pub progress_fraction: Option<f32>,
    pub in_library: bool,
    pub is_focused: bool,
    pub on_open: Option<ClickHandler>,
    pub on_quick_play: Option<ClickHandler>,
    pub on_toggle_library: Option<ClickHandler>,
    pub on_context_menu: Option<ClickHandler>,
    pub on_hover: Option<HoverHandler>,
}

impl PosterCardProps {
    /// Creates a new `PosterCardProps` with portrait `2:3` aspect and default inactive flags.
    pub fn new(
        id: impl Into<ElementId>,
        title: impl Into<SharedString>,
        placeholder_id: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            image: None,
            placeholder_id: placeholder_id.into(),
            aspect: PosterAspect::Portrait2x3,
            fixed_width_px: None,
            image_height_px: None,
            status_tab: None,
            notification_count: 0,
            imdb_rating: None,
            progress_fraction: None,
            in_library: false,
            is_focused: false,
            on_open: None,
            on_quick_play: None,
            on_toggle_library: None,
            on_context_menu: None,
            on_hover: None,
        }
    }

    pub fn subtitle(mut self, subtitle: impl Into<Option<SharedString>>) -> Self {
        self.subtitle = subtitle.into();
        self
    }

    pub fn image(mut self, image: impl Into<Option<Arc<RenderImage>>>) -> Self {
        self.image = image.into();
        self
    }

    pub fn aspect(mut self, aspect: PosterAspect) -> Self {
        self.aspect = aspect;
        self
    }

    pub fn fixed_width_px(mut self, width_px: impl Into<Option<f32>>) -> Self {
        self.fixed_width_px = width_px.into();
        self
    }

    pub fn image_height_px(mut self, height_px: impl Into<Option<f32>>) -> Self {
        self.image_height_px = height_px.into();
        self
    }

    pub fn status_tab(mut self, status_tab: impl Into<Option<LibraryStatusTab>>) -> Self {
        self.status_tab = status_tab.into();
        self
    }

    pub fn notification_count(mut self, count: usize) -> Self {
        self.notification_count = count;
        self
    }

    pub fn imdb_rating(mut self, rating: impl Into<Option<SharedString>>) -> Self {
        self.imdb_rating = rating.into();
        self
    }

    pub fn progress_fraction(mut self, fraction: impl Into<Option<f32>>) -> Self {
        self.progress_fraction = fraction.into();
        self
    }

    pub fn in_library(mut self, in_library: bool) -> Self {
        self.in_library = in_library;
        self
    }

    pub fn is_focused(mut self, is_focused: bool) -> Self {
        self.is_focused = is_focused;
        self
    }

    pub fn on_open(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open = Some(Box::new(handler));
        self
    }

    pub fn on_quick_play(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_quick_play = Some(Box::new(handler));
        self
    }

    pub fn on_toggle_library(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle_library = Some(Box::new(handler));
        self
    }

    pub fn on_context_menu(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_context_menu = Some(Box::new(handler));
        self
    }

    pub fn on_hover(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Some(Box::new(handler));
        self
    }
}

/// Clamps a playback progress fraction to `0.0..=1.0`, ignoring `<= 1%` or non-finite values.
#[inline]
pub fn clamp_progress_fraction(fraction: Option<f32>) -> Option<f32> {
    fraction
        .filter(|f| f.is_finite() && *f > 0.01)
        .map(|f| f.clamp(0.0, 1.0))
}

/// Formats a notification badge pill label (`"NEW"` for 1, `"{count} NEW"` for >1).
#[inline]
pub fn format_notification_badge_label(count: usize) -> Option<SharedString> {
    match count {
        0 => None,
        1 => Some(SharedString::new_static("NEW")),
        n => Some(SharedString::from(format!("{n} NEW"))),
    }
}

/// Renders the unified v0.5.0 poster/episode card with focus ring, badges, progress bar, and hover actions.
pub fn render_poster_card(props: PosterCardProps) -> AnyElement {
    let base_id = Arc::new(props.id.clone());
    let grp: SharedString = format!("poster-card-grp-{base_id}").into();
    let quick_play_id = ElementId::NamedChild(Arc::clone(&base_id), SharedString::new_static("quick-play"));
    let toggle_lib_id = ElementId::NamedChild(Arc::clone(&base_id), SharedString::new_static("toggle-lib"));

    let notif_label = format_notification_badge_label(props.notification_count);
    let progress = clamp_progress_fraction(props.progress_fraction);
    let has_bottom_left_hover_btn = props.on_toggle_library.is_some();
    let rating_bottom_px = if progress.is_some() { 10.0 } else { 8.0 };

    let mut art_frame = div()
        .w_full()
        .rounded(px(RADIUS_POSTER))
        .overflow_hidden()
        .relative()
        .bg(rgb(placeholder_tint_for_id(&props.placeholder_id)))
        .shadow(poster_card_shadow());

    if let Some(h) = props.image_height_px {
        art_frame = art_frame.h(px(h));
    } else {
        art_frame = art_frame.aspect_ratio(props.aspect.width_over_height());
    }

    if props.is_focused {
        art_frame = art_frame.border_2().border_color(rgb(ACCENT));
    } else {
        art_frame = art_frame
            .border_1()
            .border_color(rgba(GLASS_BORDER))
            .group_hover(grp.clone(), |s| s.border_color(rgba(BORDER_POSTER_HOVER)));
    }

    art_frame = art_frame
        .child(if let Some(tex) = props.image {
            img(tex)
                .size_full()
                .object_fit(ObjectFit::Cover)
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
                        .w_full()
                        .font(editorial_serif())
                        .text_size(px(19.0))
                        .text_color(rgb(TEXT_SECONDARY))
                        .truncate()
                        .child(props.title.clone()),
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
                    linear_color_stop(rgba(0x0b0b0d00), 0.40),
                    linear_color_stop(rgba(0x0b0b0de6), 1.0),
                )),
        )
        .when_some(notif_label, |el, label| {
            el.child(
                div()
                    .absolute()
                    .top(px(8.0))
                    .left(px(8.0))
                    .h(px(20.0))
                    .px(px(7.0))
                    .rounded(px(RADIUS_PILL))
                    .bg(rgb(ACCENT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .font(mono_font())
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(10.0))
                    .text_color(rgb(CTA_ON_ACCENT))
                    .child(label),
            )
        })
        .when_some(props.status_tab, |el, st| {
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
                    .w(px(24.0))
                    .h(px(24.0))
                    .rounded(px(RADIUS_PILL))
                    .bg(rgb(bg_col))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(svg_icon(icon, 12.0, rgb(BG_OBSIDIAN))),
            )
        })
        .when_some(props.imdb_rating, |el, rating| {
            el.child(
                div()
                    .absolute()
                    .bottom(px(rating_bottom_px))
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
                    .when(has_bottom_left_hover_btn, |badge| {
                        badge.group_hover(grp.clone(), |s| s.opacity(0.0))
                    })
                    .child(svg_icon(IconName::Star, 10.5, rgb(0xfbbf24)))
                    .child(rating),
            )
        });

    if let Some(on_quick_play) = props.on_quick_play {
        art_frame = art_frame.child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .opacity(0.0)
                .group_hover(grp.clone(), |s| s.opacity(1.0))
                .child(
                    div()
                        .id(quick_play_id)
                        .cursor_pointer()
                        .w(px(44.0))
                        .h(px(44.0))
                        .rounded(px(RADIUS_PILL))
                        .bg(rgb(ACCENT))
                        .hover(|s| s.bg(rgb(ACCENT_HOVER)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                            cx.stop_propagation();
                            on_quick_play(ev, window, cx);
                        })
                        .child(svg_icon(IconName::Play, 17.0, rgb(CTA_ON_ACCENT))),
                ),
        );
    }

    if let Some(on_toggle_library) = props.on_toggle_library {
        let in_lib = props.in_library;
        art_frame = art_frame.child(
            div()
                .id(toggle_lib_id)
                .cursor_pointer()
                .absolute()
                .bottom(px(rating_bottom_px))
                .left(px(8.0))
                .opacity(0.0)
                .group_hover(grp.clone(), |s| s.opacity(1.0))
                .w(px(30.0))
                .h(px(30.0))
                .rounded(px(RADIUS_PILL))
                .bg(if in_lib {
                    rgb(STATUS_WATCHLIST)
                } else {
                    rgba(0x141417eb)
                })
                .border_1()
                .border_color(if in_lib {
                    rgb(STATUS_WATCHLIST)
                } else {
                    rgba(GLASS_BORDER_STRONG)
                })
                .hover(|s| s.bg(rgb(ACCENT)).border_color(rgb(ACCENT)))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                    cx.stop_propagation();
                    on_toggle_library(ev, window, cx);
                })
                .child(svg_icon(
                    if in_lib {
                        IconName::Check
                    } else {
                        IconName::Bookmark
                    },
                    13.0,
                    if in_lib {
                        rgb(BG_OBSIDIAN)
                    } else {
                        rgb(TEXT_PRIMARY)
                    },
                )),
        );
    }

    if let Some(frac) = progress {
        art_frame = art_frame.child(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(3.0))
                .bg(rgba(0xFFFFFF2E))
                .child(div().h_full().w(relative(frac)).bg(rgb(ACCENT))),
        );
    }

    let mut card = div()
        .id(props.id)
        .group(grp)
        .flex()
        .flex_col()
        .gap(px(8.0));

    if let Some(w) = props.fixed_width_px {
        card = card.w(px(w)).flex_shrink_0();
    } else {
        card = card.w_full().min_w(px(0.0));
    }

    if let Some(on_open) = props.on_open {
        card = card
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, on_open);
    }

    if let Some(on_context_menu) = props.on_context_menu {
        card = card.on_mouse_down(MouseButton::Right, move |ev, window, cx| {
            cx.stop_propagation();
            on_context_menu(ev, window, cx);
        });
    }

    if let Some(on_hover) = props.on_hover {
        card = card.on_hover(on_hover);
    }

    card.child(art_frame)
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
                        .text_size(px(13.5))
                        .line_height(px(18.0))
                        .text_color(rgb(TEXT_PRIMARY))
                        .truncate()
                        .child(props.title),
                )
                .when_some(props.subtitle, |col, sub| {
                    col.child(
                        div()
                            .font(ui_sans(FontWeight::NORMAL))
                            .font_features(tabular_nums())
                            .text_size(px(12.0))
                            .line_height(px(16.0))
                            .text_color(rgb(TEXT_MUTED))
                            .truncate()
                            .child(sub),
                    )
                }),
        )
        .into_any_element()
}

/// Props for the unified 3-line `v0.4.5`-styled stream source row used in `MetaDetails`.
pub struct StreamSourceRowProps {
    pub id: ElementId,
    /// Line 1 (`ui_sans(SEMIBOLD)`): e.g. `"4K · Dolby Vision · HEVC · Atmos"`.
    pub headline: SharedString,
    /// Line 2 (`tabular_nums`): e.g. `"18.4 GB · 24.5 Mbps · [RD+] Cached"`.
    pub metadata_line: SharedString,
    /// Line 3 (`mono_font`, truncated): release title.
    pub release_title: Option<SharedString>,
    pub addon_label: Option<SharedString>,
    pub is_cached: bool,
    pub is_active: bool,
    pub is_last_used: bool,
    pub is_expanded: bool,
    pub raw_filename: Option<SharedString>,
    pub info_hash: Option<SharedString>,
    pub technical_specs: Vec<SharedString>,
    pub on_play: Option<ClickHandler>,
    pub on_toggle_expand: Option<ClickHandler>,
    pub on_copy_url: Option<ClickHandler>,
    pub on_copy_hash: Option<ClickHandler>,
}

impl StreamSourceRowProps {
    pub fn new(
        id: impl Into<ElementId>,
        headline: impl Into<SharedString>,
        metadata_line: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            headline: headline.into(),
            metadata_line: metadata_line.into(),
            release_title: None,
            addon_label: None,
            is_cached: false,
            is_active: false,
            is_last_used: false,
            is_expanded: false,
            raw_filename: None,
            info_hash: None,
            technical_specs: Vec::new(),
            on_play: None,
            on_toggle_expand: None,
            on_copy_url: None,
            on_copy_hash: None,
        }
    }

    pub fn release_title(mut self, title: impl Into<Option<SharedString>>) -> Self {
        self.release_title = title.into();
        self
    }

    pub fn addon_label(mut self, addon: impl Into<Option<SharedString>>) -> Self {
        self.addon_label = addon.into();
        self
    }

    pub fn is_cached(mut self, cached: bool) -> Self {
        self.is_cached = cached;
        self
    }

    pub fn is_active(mut self, active: bool) -> Self {
        self.is_active = active;
        self
    }

    pub fn is_last_used(mut self, last_used: bool) -> Self {
        self.is_last_used = last_used;
        self
    }

    pub fn is_expanded(mut self, expanded: bool) -> Self {
        self.is_expanded = expanded;
        self
    }

    pub fn raw_filename(mut self, filename: impl Into<Option<SharedString>>) -> Self {
        self.raw_filename = filename.into();
        self
    }

    pub fn info_hash(mut self, hash: impl Into<Option<SharedString>>) -> Self {
        self.info_hash = hash.into();
        self
    }

    pub fn technical_specs(mut self, specs: Vec<SharedString>) -> Self {
        self.technical_specs = specs;
        self
    }

    pub fn on_play(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_play = Some(Box::new(handler));
        self
    }

    pub fn on_toggle_expand(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle_expand = Some(Box::new(handler));
        self
    }

    pub fn on_copy_url(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_copy_url = Some(Box::new(handler));
        self
    }

    pub fn on_copy_hash(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_copy_hash = Some(Box::new(handler));
        self
    }
}

/// Renders a unified 3-line stream source row with optional `"Last used"` badge and expandable metadata/copy drawer.
pub fn render_stream_source_row(props: StreamSourceRowProps) -> AnyElement {
    let base_id = Arc::new(props.id.clone());
    let expand_id = ElementId::NamedChild(Arc::clone(&base_id), SharedString::new_static("expand"));
    let copy_url_id = ElementId::NamedChild(Arc::clone(&base_id), SharedString::new_static("copy-url"));
    let copy_hash_id = ElementId::NamedChild(Arc::clone(&base_id), SharedString::new_static("copy-hash"));

    let has_drawer_content = props.raw_filename.is_some()
        || !props.technical_specs.is_empty()
        || props.info_hash.is_some()
        || props.on_copy_url.is_some()
        || props.on_copy_hash.is_some();

    let mut main_row = div()
        .id(props.id)
        .w_full()
        .px(px(16.0))
        .py(px(12.0))
        .when(!props.is_active, |row| {
            row.hover(|s| s.bg(rgba(GLASS_ROW_HOVER_BG)))
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(14.0));

    if let Some(on_play) = props.on_play {
        main_row = main_row
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, on_play);
    }

    let left_col = div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(3.0))
        .child(
            div()
                .w_full()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(svg_icon(IconName::Play, 12.0, rgb(ACCENT)))
                .child(
                    div()
                        .min_w(px(0.0))
                        .truncate()
                        .font(ui_sans(FontWeight::SEMIBOLD))
                        .text_size(px(13.5))
                        .text_color(rgb(TEXT_PRIMARY))
                        .child(props.headline),
                )
                .when(props.is_last_used, |row| {
                    row.child(
                        div()
                            .flex_shrink_0()
                            .px(px(7.0))
                            .py(px(2.0))
                            .rounded(px(RADIUS_PILL))
                            .bg(rgba(ACCENT_SOFT))
                            .border_1()
                            .border_color(rgba(BORDER_ACCENT))
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(px(10.5))
                            .text_color(rgb(ACCENT))
                            .child("Last used"),
                    )
                })
                .when_some(props.addon_label, |row, addon| {
                    row.child(
                        div()
                            .flex_shrink_0()
                            .font(ui_sans(FontWeight::NORMAL))
                            .text_size(px(11.5))
                            .text_color(rgb(TEXT_MUTED))
                            .child(addon),
                    )
                }),
        )
        .child(
            div()
                .w_full()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .when(props.is_cached, |row| {
                    row.child(
                        div()
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_full()
                            .flex_shrink_0()
                            .bg(rgb(STATUS_WATCHED)),
                    )
                })
                .child(
                    div()
                        .min_w(px(0.0))
                        .truncate()
                        .font(ui_sans(FontWeight::NORMAL))
                        .font_features(tabular_nums())
                        .text_size(px(12.0))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(props.metadata_line),
                ),
        )
        .when_some(props.release_title, |col, release| {
            col.child(
                div()
                    .w_full()
                    .min_w(px(0.0))
                    .font(mono_font())
                    .text_size(px(11.5))
                    .text_color(rgb(TEXT_MUTED))
                    .truncate()
                    .text_ellipsis()
                    .child(release),
            )
        });

    let mut right_col = div().flex_shrink_0().flex().items_center().gap(px(8.0));

    if props.is_active {
        right_col = right_col.child(svg_icon(IconName::Check, 15.0, rgb(ACCENT)));
    }

    if let Some(on_toggle_expand) = props.on_toggle_expand {
        let expanded = props.is_expanded;
        right_col = right_col.child(
            div()
                .id(expand_id)
                .cursor_pointer()
                .px(px(9.0))
                .py(px(4.0))
                .rounded(px(RADIUS_POSTER))
                .bg(if expanded {
                    rgba(ACCENT_SOFT)
                } else {
                    rgba(0x00000000)
                })
                .hover(|s| s.bg(rgba(GLASS_ROW_HOVER_BG)))
                .flex()
                .items_center()
                .gap(px(4.0))
                .font(ui_sans(FontWeight::MEDIUM))
                .text_size(px(11.5))
                .text_color(if expanded {
                    rgb(ACCENT)
                } else {
                    rgb(TEXT_MUTED)
                })
                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                    cx.stop_propagation();
                    on_toggle_expand(ev, window, cx);
                })
                .child("Details")
                .child(svg_icon(
                    if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    },
                    11.5,
                    if expanded {
                        rgb(ACCENT)
                    } else {
                        rgb(TEXT_MUTED)
                    },
                )),
        );
    }

    main_row = main_row.child(left_col).child(right_col);

    let mut outer = div()
        .w_full()
        .rounded(px(RADIUS_CARD))
        .bg(if props.is_active {
            rgba(GLASS_ROW_SELECTED_BG)
        } else {
            rgba(GLASS_CARD_BG)
        })
        .border_1()
        .border_color(if props.is_active {
            rgba(BORDER_ACCENT)
        } else {
            rgba(GLASS_BORDER)
        })
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(main_row);

    if props.is_expanded && has_drawer_content {
        let mut drawer = div()
            .w_full()
            .px(px(16.0))
            .py(px(12.0))
            .bg(rgba(0x0b0b0d66))
            .border_t_1()
            .border_color(rgba(GLASS_DIVIDER))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .when_some(props.raw_filename, |el, filename| {
                el.child(
                    div()
                        .font(mono_font())
                        .text_size(px(11.5))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(filename),
                )
            })
            .when(!props.technical_specs.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.0))
                        .children(props.technical_specs.into_iter().map(|spec| {
                            div()
                                .px(px(7.0))
                                .py(px(2.0))
                                .rounded(px(RADIUS_PILL))
                                .bg(rgba(GLASS_TROUGH_BG))
                                .border_1()
                                .border_color(rgba(GLASS_BORDER))
                                .font(mono_font())
                                .text_size(px(11.0))
                                .text_color(rgb(TEXT_MUTED))
                                .child(spec)
                        })),
                )
            })
            .when_some(props.info_hash, |el, hash| {
                el.child(
                    div()
                        .font(mono_font())
                        .text_size(px(11.0))
                        .text_color(rgb(TEXT_MUTED))
                        .child(format!("info_hash {hash}")),
                )
            });

        if props.on_copy_url.is_some() || props.on_copy_hash.is_some() {
            let mut actions = div().flex().flex_wrap().items_center().gap(px(8.0));
            if let Some(on_copy_url) = props.on_copy_url {
                actions = actions.child(render_drawer_copy_button(
                    copy_url_id,
                    "Copy URL",
                    on_copy_url,
                ));
            }
            if let Some(on_copy_hash) = props.on_copy_hash {
                actions = actions.child(render_drawer_copy_button(
                    copy_hash_id,
                    "Copy hash",
                    on_copy_hash,
                ));
            }
            drawer = drawer.child(actions);
        }

        outer = outer.child(drawer);
    }

    outer.into_any_element()
}

fn render_drawer_copy_button(
    id: ElementId,
    label: &'static str,
    on_click: ClickHandler,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .h(px(28.0))
        .px(px(10.0))
        .rounded(px(RADIUS_POSTER))
        .bg(rgba(GLASS_TROUGH_BG))
        .border_1()
        .border_color(rgba(GLASS_BORDER))
        .hover(|s| {
            s.bg(rgba(GLASS_ROW_HOVER_BG))
                .border_color(rgba(GLASS_BORDER_STRONG))
        })
        .flex()
        .items_center()
        .gap(px(6.0))
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(px(11.5))
        .text_color(rgb(TEXT_SECONDARY))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(svg_icon(IconName::Clipboard, 11.5, rgb(TEXT_MUTED)))
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_fraction_clamps_and_ignores_noise() {
        assert_eq!(clamp_progress_fraction(None), None);
        assert_eq!(clamp_progress_fraction(Some(0.0)), None);
        assert_eq!(clamp_progress_fraction(Some(0.005)), None);
        assert_eq!(clamp_progress_fraction(Some(f32::NAN)), None);
        assert_eq!(clamp_progress_fraction(Some(0.45)), Some(0.45));
        assert_eq!(clamp_progress_fraction(Some(1.8)), Some(1.0));
    }

    #[test]
    fn notification_badge_label_formats_zero_one_and_many() {
        assert_eq!(format_notification_badge_label(0), None);
        assert_eq!(
            format_notification_badge_label(1),
            Some(SharedString::new_static("NEW"))
        );
        assert_eq!(
            format_notification_badge_label(4),
            Some(SharedString::from("4 NEW"))
        );
    }
}
