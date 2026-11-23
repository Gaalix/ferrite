use std::{
    ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign},
    rc::Rc,
    sync::Arc,
};

use gpui::{
    div, img, linear_color_stop, linear_gradient, prelude::*, px, relative, rgb, rgba, AnyElement,
    App, Div, ElementId, FontWeight, MouseButton, MouseDownEvent, ObjectFit, Pixels, Point,
    RenderImage, SharedString, Stateful, Window,
};

use crate::{
    icons::{svg_icon, IconName},
    state::LibraryStatusTab,
    theme::{
        active_theme, editorial_serif, glass_capsule_shadow, mono_font, placeholder_tint_for_id,
        poster_card_shadow, poster_hover_glow_shadow, tabular_nums, ui_sans, ThemePalette,
        RADIUS_CARD, RADIUS_CONTROL, RADIUS_PILL, RADIUS_POSTER,
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

/// Frosted segmented control trough styled from the active Ethereal `ThemePalette`.
pub fn glass_segmented_bar(id: impl Into<ElementId>) -> Stateful<Div> {
    let theme = active_theme();
    div()
        .id(id)
        .p(theme.sp(3.0))
        .rounded(px(RADIUS_CONTROL))
        .bg(rgba(theme.glass_trough_bg))
        .border_1()
        .border_color(rgba(theme.glass_border))
        .flex()
        .items_center()
        .gap(theme.sp(2.0))
}

/// Individual segment button inside a `glass_segmented_bar`.
pub fn glass_segmented_item(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let theme = active_theme();
    let base = div()
        .id(id)
        .h(theme.sp(30.0))
        .px(theme.sp(12.0))
        .rounded(px(RADIUS_POSTER))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .whitespace_nowrap()
        .text_size(theme.sp(12.5))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        });

    if active {
        base.bg(rgb(theme.segment_active_bg))
            .font(ui_sans(FontWeight::SEMIBOLD))
            .text_color(rgb(theme.segment_active_text))
    } else {
        base.bg(rgba(0x00000000))
            .hover(move |s| {
                s.bg(rgba(theme.glass_row_hover_bg))
                    .text_color(rgb(theme.text_primary))
            })
            .font(ui_sans(FontWeight::MEDIUM))
            .text_color(rgb(theme.text_secondary))
    }
    .child(label.into())
}

/// Unified 40x22 glass toggle switch with crisp white thumb, adapting across dark and light presets.
pub fn glass_switch(
    id: impl Into<ElementId>,
    checked: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let theme = active_theme();
    let unchecked_bg = if theme.is_light { 0x0F172A29 } else { 0xFFFFFF1E };
    let unchecked_hover_bg = if theme.is_light { 0x0F172A3D } else { 0xFFFFFF2B };

    div()
        .id(id)
        .w(theme.sp(40.0))
        .h(theme.sp(22.0))
        .flex_shrink_0()
        .rounded_full()
        .p(theme.sp(2.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .when(checked, |s| s.bg(rgb(theme.accent)).justify_end())
        .when(!checked, |s| s.bg(rgba(unchecked_bg)).justify_start())
        .hover(move |s| {
            if checked {
                s.bg(rgb(theme.accent_hover))
            } else {
                s.bg(rgba(unchecked_hover_bg))
            }
        })
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(
            div()
                .w(theme.sp(18.0))
                .h(theme.sp(18.0))
                .rounded_full()
                .bg(rgb(0xFFFFFF))
                .when(theme.is_light, |thumb| {
                    thumb.border_1().border_color(rgba(0x0000001A))
                }),
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
    let theme = active_theme();
    let prefix = id_prefix.into();
    let dec_id: SharedString = format!("{prefix}-dec").into();
    let reset_id: SharedString = format!("{prefix}-reset").into();
    let inc_id: SharedString = format!("{prefix}-inc").into();
    let icon_sz = 13.0 * theme.ui_scale;

    div()
        .h(theme.sp(34.0))
        .min_w(theme.sp(114.0))
        .rounded_full()
        .bg(rgba(theme.glass_trough_bg))
        .border_1()
        .border_color(rgba(theme.glass_border))
        .px(theme.sp(6.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(theme.sp(4.0))
        .child(
            div()
                .id(dec_id)
                .w(theme.sp(24.0))
                .h(theme.sp(24.0))
                .rounded_full()
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                    cx.stop_propagation();
                    on_dec(ev, window, cx);
                })
                .child(svg_icon(
                    IconName::Minus,
                    icon_sz,
                    rgb(theme.text_secondary),
                )),
        )
        .child(
            div()
                .id(reset_id)
                .flex_1()
                .px(theme.sp(6.0))
                .py(theme.sp(2.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .whitespace_nowrap()
                .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                .font(ui_sans(FontWeight::MEDIUM))
                .font_features(tabular_nums())
                .text_size(theme.sp(12.5))
                .text_color(rgb(theme.text_primary))
                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                    cx.stop_propagation();
                    on_reset(ev, window, cx);
                })
                .child(value_label.into()),
        )
        .child(
            div()
                .id(inc_id)
                .w(theme.sp(24.0))
                .h(theme.sp(24.0))
                .rounded_full()
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                    cx.stop_propagation();
                    on_inc(ev, window, cx);
                })
                .child(svg_icon(
                    IconName::Plus,
                    icon_sz,
                    rgb(theme.text_secondary),
                )),
        )
}

/// Floating frosted dropdown/context-menu panel container styled from `active_theme()`.
pub fn glass_dropdown_panel(id: impl Into<ElementId>, width_px: f32) -> Stateful<Div> {
    let theme = active_theme();
    div()
        .id(id)
        .occlude()
        .w(theme.sp(width_px))
        .p(theme.sp(6.0))
        .rounded(px(14.0))
        .bg(rgba(theme.glass_sheet_bg))
        .border_1()
        .border_color(rgba(theme.glass_border))
        .shadow(glass_capsule_shadow())
        .flex()
        .flex_col()
        .gap(theme.sp(2.0))
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
    let theme = active_theme();
    let check_color = if theme.is_light {
        theme.accent_press
    } else {
        theme.accent
    };
    div()
        .id(id)
        .w_full()
        .flex_shrink_0()
        .px(theme.sp(12.0))
        .py(theme.sp(8.0))
        .rounded(px(RADIUS_CONTROL))
        .cursor_pointer()
        .bg(if selected {
            rgba(theme.glass_row_selected_bg)
        } else {
            rgba(0x00000000)
        })
        .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
        .flex()
        .items_center()
        .justify_between()
        .gap(theme.sp(10.0))
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
                .gap(theme.sp(2.0))
                .child(
                    div()
                        .font(ui_sans(if selected {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        }))
                        .text_size(theme.sp(13.0))
                        .text_color(rgb(theme.text_primary))
                        .truncate()
                        .child(label.into()),
                )
                .when_some(sublabel, |d, sub| {
                    d.child(
                        div()
                            .font(ui_sans(FontWeight::NORMAL))
                            .text_size(theme.sp(11.5))
                            .text_color(rgb(theme.text_muted))
                            .truncate()
                            .child(sub),
                    )
                }),
        )
        .when(selected, |d| {
            d.child(svg_icon(
                IconName::Check,
                14.0 * theme.ui_scale,
                rgb(check_color),
            ))
        })
}

/// Pill-shaped frosted back button used across MetaDetails, Search, and Person views.
pub fn glass_back_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let theme = active_theme();
    div()
        .id(id)
        .cursor_pointer()
        .h(theme.sp(34.0))
        .px(theme.sp(12.0))
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
        .gap(theme.sp(6.0))
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(theme.sp(13.0))
        .text_color(rgb(theme.text_secondary))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(svg_icon(
            IconName::ChevronLeft,
            14.0 * theme.ui_scale,
            rgb(theme.text_secondary),
        ))
        .child(label.into())
}

/// Primary accent CTA button with `cta_on_accent` ink and optional leading icon.
pub fn primary_accent_button(
    id: impl Into<ElementId>,
    icon: Option<IconName>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let theme = active_theme();
    div()
        .id(id)
        .cursor_pointer()
        .h(theme.sp(40.0))
        .px(theme.sp(18.0))
        .rounded(px(RADIUS_CONTROL))
        .bg(rgb(theme.accent))
        .hover(move |s| s.bg(rgb(theme.accent_hover)))
        .flex()
        .items_center()
        .justify_center()
        .gap(theme.sp(8.0))
        .font(ui_sans(FontWeight::SEMIBOLD))
        .font_features(tabular_nums())
        .text_size(theme.sp(13.5))
        .text_color(rgb(theme.cta_on_accent))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .when_some(icon, |d, ic| {
            d.child(svg_icon(
                ic,
                14.5 * theme.ui_scale,
                rgb(theme.cta_on_accent),
            ))
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
    let theme = active_theme();
    let fg = if active {
        if theme.is_light {
            theme.accent_press
        } else {
            theme.accent
        }
    } else {
        theme.text_primary
    };
    div()
        .id(id)
        .cursor_pointer()
        .h(theme.sp(40.0))
        .px(theme.sp(16.0))
        .rounded(px(RADIUS_CONTROL))
        .bg(if active {
            rgba(theme.accent_soft)
        } else {
            rgba(theme.glass_trough_bg)
        })
        .border_1()
        .border_color(if active {
            rgba(theme.border_accent)
        } else {
            rgba(theme.glass_border)
        })
        .hover(move |s| {
            s.bg(if active {
                rgba(theme.accent_soft)
            } else {
                rgba(theme.glass_row_hover_bg)
            })
            .border_color(if active {
                rgb(theme.accent)
            } else {
                rgba(theme.glass_border_strong)
            })
        })
        .flex()
        .items_center()
        .justify_center()
        .gap(theme.sp(8.0))
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(theme.sp(13.5))
        .text_color(rgb(fg))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .when_some(icon, |d, ic| {
            d.child(svg_icon(ic, 14.5 * theme.ui_scale, rgb(fg)))
        })
        .child(label.into())
}

/// Props for the unified poster card shared across Home, Discover, Library, Calendar, Search, Person, and MetaDetails.
pub struct PosterCardProps {
    pub id: ElementId,
    pub title: SharedString,
    pub subtitle: Option<SharedString>,
    pub image: Option<Arc<RenderImage>>,
    pub placeholder_tint: u32,
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
    pub on_quick_peek: Option<ClickHandler>,
    pub quick_action: Option<AnyElement>,
    pub on_context_menu: Option<ClickHandler>,
    pub on_hover: Option<HoverHandler>,
}

impl PosterCardProps {
    /// Creates a new `PosterCardProps` with portrait `2:3` aspect and default inactive flags.
    pub fn new(
        id: impl Into<ElementId>,
        title: impl Into<SharedString>,
        placeholder_id: impl AsRef<str>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            image: None,
            placeholder_tint: placeholder_tint_for_id(placeholder_id.as_ref()),
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
            on_quick_peek: None,
            quick_action: None,
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

    pub fn on_open_handler(mut self, handler: ClickHandler) -> Self {
        self.on_open = Some(handler);
        self
    }

    pub fn on_quick_play(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_quick_play = Some(Box::new(handler));
        self
    }

    pub fn on_quick_play_handler(mut self, handler: ClickHandler) -> Self {
        self.on_quick_play = Some(handler);
        self
    }

    pub fn on_toggle_library(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle_library = Some(Box::new(handler));
        self
    }

    pub fn on_toggle_library_handler(mut self, handler: ClickHandler) -> Self {
        self.on_toggle_library = Some(handler);
        self
    }

    pub fn on_quick_peek(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_quick_peek = Some(Box::new(handler));
        self
    }

    pub fn on_quick_peek_handler(mut self, handler: ClickHandler) -> Self {
        self.on_quick_peek = Some(handler);
        self
    }

    pub fn quick_action(mut self, action: impl Into<Option<AnyElement>>) -> Self {
        self.quick_action = action.into();
        self
    }

    pub fn on_context_menu(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_context_menu = Some(Box::new(handler));
        self
    }

    pub fn on_context_menu_handler(mut self, handler: ClickHandler) -> Self {
        self.on_context_menu = Some(handler);
        self
    }

    pub fn on_hover(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Some(Box::new(handler));
        self
    }

    pub fn on_hover_handler(mut self, handler: HoverHandler) -> Self {
        self.on_hover = Some(handler);
        self
    }
}

/// Static GPUI hover group name shared by every poster card.
///
/// GPUI's `GroupHitboxes` is stack-scoped (pushed on `.group(...)` entry and popped after
/// painting children), so a single static group name scopes hover styles strictly to each
/// individual card subtree with zero heap `format!` allocations.
const POSTER_CARD_GROUP: SharedString = SharedString::new_static("poster-card");

/// Delegated interaction event emitted by [`render_prepared_poster_card`] to a shared
/// [`CardActionDispatcher`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PosterCardAction {
    Open,
    MiddleClick,
    QuickPlay,
    QuickPeek,
    ToggleLibrary,
    ContextMenu(Point<Pixels>),
    Hover(bool),
}

/// Compact 1-byte bitflags declaring which interactive affordances a [`PreparedPosterCard`]
/// enables. Only enabled capabilities build hover buttons or attach mouse/hover listeners.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PosterCardCapabilities(u8);

impl PosterCardCapabilities {
    pub const NONE: Self = Self(0);
    pub const CAN_OPEN: Self = Self(1 << 0);
    pub const CAN_MIDDLE_CLICK: Self = Self(1 << 1);
    pub const CAN_QUICK_PLAY: Self = Self(1 << 2);
    pub const CAN_QUICK_PEEK: Self = Self(1 << 3);
    pub const CAN_TOGGLE_LIBRARY: Self = Self(1 << 4);
    pub const CAN_CONTEXT_MENU: Self = Self(1 << 5);
    pub const CAN_HOVER: Self = Self(1 << 6);

    /// Standard browsing affordances (open, quick play, quick peek, library toggle, context menu, hover).
    pub const STANDARD_BROWSING: Self = Self(
        Self::CAN_OPEN.0
            | Self::CAN_QUICK_PLAY.0
            | Self::CAN_QUICK_PEEK.0
            | Self::CAN_TOGGLE_LIBRARY.0
            | Self::CAN_CONTEXT_MENU.0
            | Self::CAN_HOVER.0,
    );

    /// All interactive affordances enabled, including middle-click.
    pub const ALL: Self = Self(Self::STANDARD_BROWSING.0 | Self::CAN_MIDDLE_CLICK.0);

    #[inline]
    pub const fn empty() -> Self {
        Self::NONE
    }

    #[inline]
    pub const fn bits(self) -> u8 {
        self.0
    }

    #[inline]
    pub const fn from_bits_truncate(bits: u8) -> Self {
        Self(bits & Self::ALL.0)
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline]
    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    #[inline]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[inline]
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

impl BitOr for PosterCardCapabilities {
    type Output = Self;

    #[inline]
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for PosterCardCapabilities {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for PosterCardCapabilities {
    type Output = Self;

    #[inline]
    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for PosterCardCapabilities {
    #[inline]
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

/// Single shared delegated callback per grid or shelf (`1 Rc` instead of `6×N` heap closures).
pub type CardActionDispatcher = Rc<dyn Fn(usize, PosterCardAction, &mut Window, &mut App)>;

/// Wraps a card-index action handler into a shared [`CardActionDispatcher`].
#[inline]
pub fn card_action_dispatcher(
    f: impl Fn(usize, PosterCardAction, &mut Window, &mut App) + 'static,
) -> CardActionDispatcher {
    Rc::new(f)
}

#[inline]
fn opt_shared_from_str(value: Option<&str>) -> Option<SharedString> {
    value.filter(|s| !s.is_empty()).map(SharedString::new)
}

/// Flat, `'static`, pre-computed poster card descriptor stored in contiguous
/// `Arc<[PreparedPosterCard]>` slabs built once per `state_rev`.
///
/// Rendering a card from a slab performs zero `String`/`format!` allocations for titles,
/// subtitles, ratings, notification pills, placeholder tints, or element/group IDs.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedPosterCard {
    pub title: SharedString,
    pub subtitle: Option<SharedString>,
    pub year: Option<SharedString>,
    pub imdb_rating: Option<SharedString>,
    pub type_badge: Option<SharedString>,
    pub poster_url: Option<SharedString>,
    pub notification_label: Option<SharedString>,
    pub placeholder_tint: u32,
    pub aspect: PosterAspect,
    pub status_tab: Option<LibraryStatusTab>,
    pub progress_fraction: Option<f32>,
    pub in_library: bool,
    pub watched: bool,
    pub notification_count: usize,
    pub is_new: bool,
    pub capabilities: PosterCardCapabilities,
}

impl PreparedPosterCard {
    /// Creates a new `'static` poster card entry with portrait `2:3` aspect and default flags.
    pub fn new(title: impl Into<SharedString>, placeholder_id: &str) -> Self {
        Self::with_tint(title, placeholder_tint_for_id(placeholder_id))
    }

    /// Creates a new `'static` poster card entry from a borrowed `&str` title without an
    /// intermediate `String` allocation (inline `SmolStr` for titles `<= 23` bytes).
    #[inline]
    pub fn from_ref(title: &str, placeholder_id: &str) -> Self {
        Self::with_tint(SharedString::new(title), placeholder_tint_for_id(placeholder_id))
    }

    /// Creates a new `'static` poster card entry with a pre-computed `placeholder_tint`.
    #[inline]
    pub fn with_tint(title: impl Into<SharedString>, placeholder_tint: u32) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            year: None,
            imdb_rating: None,
            type_badge: None,
            poster_url: None,
            notification_label: None,
            placeholder_tint,
            aspect: PosterAspect::Portrait2x3,
            status_tab: None,
            progress_fraction: None,
            in_library: false,
            watched: false,
            notification_count: 0,
            is_new: false,
            capabilities: PosterCardCapabilities::NONE,
        }
    }

    #[inline]
    pub fn subtitle(mut self, subtitle: impl Into<Option<SharedString>>) -> Self {
        self.subtitle = subtitle.into();
        self
    }

    #[inline]
    pub fn subtitle_str(mut self, subtitle: Option<&str>) -> Self {
        self.subtitle = opt_shared_from_str(subtitle);
        self
    }

    #[inline]
    pub fn year(mut self, year: impl Into<Option<SharedString>>) -> Self {
        self.year = year.into();
        self
    }

    #[inline]
    pub fn year_str(mut self, year: Option<&str>) -> Self {
        self.year = opt_shared_from_str(year);
        self
    }

    #[inline]
    pub fn imdb_rating(mut self, rating: impl Into<Option<SharedString>>) -> Self {
        self.imdb_rating = rating.into();
        self
    }

    #[inline]
    pub fn imdb_rating_str(mut self, rating: Option<&str>) -> Self {
        self.imdb_rating = opt_shared_from_str(rating);
        self
    }

    #[inline]
    pub fn type_badge(mut self, badge: impl Into<Option<SharedString>>) -> Self {
        self.type_badge = badge.into();
        self
    }

    #[inline]
    pub fn type_badge_str(mut self, badge: Option<&str>) -> Self {
        self.type_badge = opt_shared_from_str(badge);
        self
    }

    #[inline]
    pub fn poster_url(mut self, url: impl Into<Option<SharedString>>) -> Self {
        self.poster_url = url.into();
        self
    }

    #[inline]
    pub fn poster_url_str(mut self, url: Option<&str>) -> Self {
        self.poster_url = opt_shared_from_str(url);
        self
    }

    #[inline]
    pub fn placeholder_tint(mut self, tint: u32) -> Self {
        self.placeholder_tint = tint;
        self
    }

    #[inline]
    pub fn aspect(mut self, aspect: PosterAspect) -> Self {
        self.aspect = aspect;
        self
    }

    #[inline]
    pub fn status_tab(mut self, status_tab: impl Into<Option<LibraryStatusTab>>) -> Self {
        self.status_tab = status_tab.into();
        self
    }

    #[inline]
    pub fn progress_fraction(mut self, fraction: impl Into<Option<f32>>) -> Self {
        self.progress_fraction = clamp_progress_fraction(fraction.into());
        self
    }

    #[inline]
    pub fn in_library(mut self, in_library: bool) -> Self {
        self.in_library = in_library;
        self
    }

    #[inline]
    pub fn watched(mut self, watched: bool) -> Self {
        self.watched = watched;
        self
    }

    /// Sets `notification_count`, pre-formats `notification_label` once, and updates `is_new`.
    #[inline]
    pub fn notification_count(mut self, count: usize) -> Self {
        self.notification_count = count;
        self.notification_label = format_notification_badge_label(count);
        self.is_new = count > 0;
        self
    }

    /// Marks the card as new (setting a static `"NEW"` label when no counted label is present).
    #[inline]
    pub fn is_new(mut self, is_new: bool) -> Self {
        self.is_new = is_new;
        if is_new && self.notification_label.is_none() {
            self.notification_label = Some(SharedString::new_static("NEW"));
        } else if !is_new && self.notification_count == 0 {
            self.notification_label = None;
        }
        self
    }

    #[inline]
    pub fn capabilities(mut self, capabilities: PosterCardCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }
}

/// Lightweight stack-only render context for [`render_prepared_poster_card`].
///
/// Pairs a slab entry ([`PreparedPosterCard`]) with per-frame dynamic state (resolved GPU
/// texture, focus ring, sizing, optional overlays, and the shared [`CardActionDispatcher`]).
pub struct PreparedPosterRenderCtx<'a> {
    pub card_index: usize,
    pub card: &'a PreparedPosterCard,
    pub id_prefix: &'static str,
    /// Optional fixed card width in pixels (`None` fills grid cell via `w_full().min_w(0)`).
    pub fixed_width_px: Option<f32>,
    /// Optional explicit artwork height in pixels (`None` uses `card.aspect` ratio).
    pub image_height_px: Option<f32>,
    pub image: Option<Arc<RenderImage>>,
    pub is_focused: bool,
    pub extra_top_left_overlay: Option<AnyElement>,
    pub quick_action: Option<AnyElement>,
    pub dispatcher: &'a CardActionDispatcher,
}

impl<'a> PreparedPosterRenderCtx<'a> {
    #[inline]
    pub fn new(
        card_index: usize,
        card: &'a PreparedPosterCard,
        id_prefix: &'static str,
        dispatcher: &'a CardActionDispatcher,
    ) -> Self {
        Self {
            card_index,
            card,
            id_prefix,
            fixed_width_px: None,
            image_height_px: None,
            image: None,
            is_focused: false,
            extra_top_left_overlay: None,
            quick_action: None,
            dispatcher,
        }
    }

    #[inline]
    pub fn fixed_width_px(mut self, width_px: impl Into<Option<f32>>) -> Self {
        self.fixed_width_px = width_px.into();
        self
    }

    #[inline]
    pub fn image_height_px(mut self, height_px: impl Into<Option<f32>>) -> Self {
        self.image_height_px = height_px.into();
        self
    }

    #[inline]
    pub fn image(mut self, image: impl Into<Option<Arc<RenderImage>>>) -> Self {
        self.image = image.into();
        self
    }

    #[inline]
    pub fn is_focused(mut self, focused: bool) -> Self {
        self.is_focused = focused;
        self
    }

    #[inline]
    pub fn extra_top_left_overlay(mut self, overlay: impl Into<Option<AnyElement>>) -> Self {
        self.extra_top_left_overlay = overlay.into();
        self
    }

    #[inline]
    pub fn quick_action(mut self, action: impl Into<Option<AnyElement>>) -> Self {
        self.quick_action = action.into();
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

/// Formats a notification badge pill label (`"NEW"` for 1, `"{count} NEW"` for >1) without
/// heap allocation (stack-formatted into `SharedString`'s inline `SmolStr` buffer).
#[inline]
pub fn format_notification_badge_label(count: usize) -> Option<SharedString> {
    match count {
        0 => None,
        1 => Some(SharedString::new_static("NEW")),
        n => {
            let mut buf = [0u8; 32];
            let mut cursor = std::io::Cursor::new(&mut buf[..]);
            if std::io::Write::write_fmt(&mut cursor, format_args!("{n} NEW")).is_ok() {
                let len = cursor.position() as usize;
                if let Ok(s) = std::str::from_utf8(&buf[..len]) {
                    return Some(SharedString::new(s));
                }
            }
            Some(SharedString::from(format!("{n} NEW")))
        }
    }
}

struct PosterArtFrameSpec<'a> {
    title: &'a SharedString,
    image: Option<Arc<RenderImage>>,
    placeholder_tint: u32,
    aspect: PosterAspect,
    image_height_px: Option<f32>,
    is_focused: bool,
    notif_label: Option<SharedString>,
    extra_top_left_overlay: Option<AnyElement>,
    status_tab: Option<LibraryStatusTab>,
    imdb_rating: Option<SharedString>,
    has_bottom_left_hover_btn: bool,
    rating_bottom_px: f32,
    badge_fg_on_status: u32,
}

fn build_poster_notif_pill(label: SharedString, theme: &ThemePalette) -> Div {
    div()
        .h(theme.sp(20.0))
        .px(theme.sp(7.0))
        .rounded(px(RADIUS_PILL))
        .bg(rgb(theme.accent))
        .flex()
        .items_center()
        .justify_center()
        .font(mono_font())
        .font_weight(FontWeight::BOLD)
        .text_size(theme.sp(10.0))
        .text_color(rgb(theme.cta_on_accent))
        .child(label)
}

fn build_poster_art_frame_base(spec: PosterArtFrameSpec<'_>, theme: &ThemePalette) -> Div {
    let theme = *theme;
    let hover_glow = poster_hover_glow_shadow(&theme);
    let scrim_top_transparent = theme.scrim_bottom & 0xFFFFFF00;
    let top_hairline_rgba = if theme.is_light {
        0xFFFFFF80
    } else {
        0xFFFFFF24
    };

    let mut art_frame = div()
        .w_full()
        .rounded(px(RADIUS_POSTER))
        .overflow_hidden()
        .relative()
        .bg(rgb(spec.placeholder_tint))
        .when(theme.is_light, |el| el.shadow(poster_card_shadow()));

    if let Some(h) = spec.image_height_px {
        art_frame = art_frame.h(px(h));
    } else {
        art_frame = art_frame.aspect_ratio(spec.aspect.width_over_height());
    }

    if spec.is_focused {
        art_frame = art_frame
            .border_2()
            .border_color(rgb(theme.accent))
            .shadow(hover_glow);
    } else {
        art_frame = art_frame
            .border_1()
            .border_color(rgba(theme.glass_border))
            .group_hover(POSTER_CARD_GROUP, move |s| {
                s.border_color(rgba(theme.border_poster_hover))
                    .shadow(hover_glow)
            });
    }

    art_frame = art_frame
        .child(if let Some(tex) = spec.image {
            img(tex)
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element()
        } else {
            div()
                .size_full()
                .p(theme.sp(14.0))
                .bg(rgb(theme.bg_surface))
                .flex()
                .items_end()
                .child(
                    div()
                        .w_full()
                        .font(editorial_serif())
                        .text_size(theme.sp(19.0))
                        .text_color(rgb(theme.text_secondary))
                        .truncate()
                        .child(spec.title.clone()),
                )
                .into_any_element()
        })
        // Luminous inner top glass edge highlight.
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(1.0))
                .bg(rgba(top_hairline_rgba)),
        )
        // Hover scrim: `invisible()` hits GPUI's Visibility::Hidden early-out (zero GPU quads at rest).
        .child(
            div()
                .absolute()
                .inset_0()
                .invisible()
                .group_hover(POSTER_CARD_GROUP, |s| s.visible())
                .bg(linear_gradient(
                    180.0,
                    linear_color_stop(rgba(scrim_top_transparent), 0.40),
                    linear_color_stop(rgba(theme.scrim_bottom), 1.0),
                )),
        );

    match (spec.notif_label, spec.extra_top_left_overlay) {
        (Some(label), Some(extra)) => {
            art_frame = art_frame.child(
                div()
                    .absolute()
                    .top(theme.sp(8.0))
                    .left(theme.sp(8.0))
                    .flex()
                    .items_center()
                    .gap(theme.sp(4.0))
                    .child(build_poster_notif_pill(label, &theme))
                    .child(extra),
            );
        }
        (Some(label), None) => {
            art_frame = art_frame.child(
                build_poster_notif_pill(label, &theme)
                    .absolute()
                    .top(theme.sp(8.0))
                    .left(theme.sp(8.0)),
            );
        }
        (None, Some(extra)) => {
            art_frame = art_frame.child(
                div()
                    .absolute()
                    .top(theme.sp(8.0))
                    .left(theme.sp(8.0))
                    .child(extra),
            );
        }
        (None, None) => {}
    }

    art_frame
        .when_some(spec.status_tab, |el, st| {
            let (icon, bg_col) = match st {
                LibraryStatusTab::Watched => (IconName::Check, theme.status_watched),
                LibraryStatusTab::Watching => (IconName::Play, theme.status_watching),
                LibraryStatusTab::Watchlist => (IconName::Bookmark, theme.status_watchlist),
            };
            el.child(
                div()
                    .absolute()
                    .top(theme.sp(8.0))
                    .right(theme.sp(8.0))
                    .w(theme.sp(24.0))
                    .h(theme.sp(24.0))
                    .rounded(px(RADIUS_PILL))
                    .bg(rgb(bg_col))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(svg_icon(
                        icon,
                        12.0 * theme.ui_scale,
                        rgb(spec.badge_fg_on_status),
                    )),
            )
        })
        .when_some(spec.imdb_rating, |el, rating| {
            el.child(
                div()
                    .absolute()
                    .bottom(theme.sp(spec.rating_bottom_px))
                    .left(theme.sp(8.0))
                    .px(theme.sp(6.5))
                    .py(theme.sp(2.5))
                    .rounded(px(4.0))
                    .bg(rgba(0x0B0D14D4))
                    .border_1()
                    .border_color(rgba(0xFFFFFF22))
                    .flex()
                    .items_center()
                    .gap(theme.sp(4.0))
                    .font(mono_font())
                    .text_size(theme.sp(11.0))
                    .text_color(rgb(0xF5F1E8))
                    .when(spec.has_bottom_left_hover_btn, |badge| {
                        badge.group_hover(POSTER_CARD_GROUP, |s| s.invisible())
                    })
                    .child(svg_icon(
                        IconName::Star,
                        10.5 * theme.ui_scale,
                        rgb(0xfbbf24),
                    ))
                    .child(rating),
            )
        })
}

fn build_poster_quick_play_overlay(
    id: impl Into<ElementId>,
    theme: &ThemePalette,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let theme = *theme;
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .invisible()
        .group_hover(POSTER_CARD_GROUP, |s| s.visible())
        .child(
            div()
                .id(id)
                .cursor_pointer()
                .w(theme.sp(44.0))
                .h(theme.sp(44.0))
                .rounded(px(RADIUS_PILL))
                .bg(rgb(theme.accent))
                .border_1()
                .border_color(rgba(0xFFFFFF38))
                .hover(move |s| s.bg(rgb(theme.accent_hover)))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                    cx.stop_propagation();
                    on_click(ev, window, cx);
                })
                .child(svg_icon(
                    IconName::Play,
                    17.0 * theme.ui_scale,
                    rgb(theme.cta_on_accent),
                )),
        )
}

fn build_poster_toggle_library_button(
    id: impl Into<ElementId>,
    in_library: bool,
    rating_bottom_px: f32,
    badge_fg_on_status: u32,
    theme: &ThemePalette,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let theme = *theme;
    div()
        .id(id)
        .cursor_pointer()
        .absolute()
        .bottom(theme.sp(rating_bottom_px))
        .left(theme.sp(8.0))
        .invisible()
        .group_hover(POSTER_CARD_GROUP, |s| s.visible())
        .w(theme.sp(30.0))
        .h(theme.sp(30.0))
        .rounded(px(RADIUS_PILL))
        .bg(if in_library {
            rgb(theme.status_watchlist)
        } else {
            rgba(0x0B0D14D9)
        })
        .border_1()
        .border_color(if in_library {
            rgb(theme.status_watchlist)
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
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(svg_icon(
            if in_library {
                IconName::Check
            } else {
                IconName::Bookmark
            },
            13.0 * theme.ui_scale,
            if in_library {
                rgb(badge_fg_on_status)
            } else {
                rgb(0xF5F1E8)
            },
        ))
}

fn build_poster_quick_peek_button(
    id: impl Into<ElementId>,
    theme: &ThemePalette,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let theme = *theme;
    div()
        .id(id)
        .cursor_pointer()
        .w(theme.sp(30.0))
        .h(theme.sp(30.0))
        .rounded(px(RADIUS_PILL))
        .bg(rgba(0x0B0D14D9))
        .border_1()
        .border_color(rgba(0xFFFFFF33))
        .hover(move |s| {
            s.bg(rgb(theme.accent_press))
                .border_color(rgb(theme.accent))
        })
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(svg_icon(
            IconName::Eye,
            13.5 * theme.ui_scale,
            rgb(0xF5F1E8),
        ))
}

fn build_poster_right_actions_cluster(
    rating_bottom_px: f32,
    quick_action: Option<AnyElement>,
    quick_peek_btn: Option<Stateful<Div>>,
    theme: &ThemePalette,
) -> Div {
    let mut right_actions = div()
        .absolute()
        .bottom(theme.sp(rating_bottom_px))
        .right(theme.sp(8.0))
        .invisible()
        .group_hover(POSTER_CARD_GROUP, |s| s.visible())
        .flex()
        .items_center()
        .gap(theme.sp(6.0));

    if let Some(slot) = quick_action {
        right_actions = right_actions.child(slot);
    }
    if let Some(peek_btn) = quick_peek_btn {
        right_actions = right_actions.child(peek_btn);
    }
    right_actions
}

fn build_poster_progress_bar(frac: f32, theme: &ThemePalette) -> Div {
    div()
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .h(theme.sp(3.0))
        .bg(rgba(0xFFFFFF33))
        .child(div().h_full().w(relative(frac)).bg(rgb(theme.accent)))
}

fn build_poster_card_shell(
    id: impl Into<ElementId>,
    fixed_width_px: Option<f32>,
    theme: &ThemePalette,
) -> Stateful<Div> {
    let card = div()
        .id(id)
        .group(POSTER_CARD_GROUP)
        .flex()
        .flex_col()
        .gap(theme.sp(8.0));

    if let Some(w) = fixed_width_px {
        card.w(px(w)).flex_shrink_0()
    } else {
        card.w_full().min_w(px(0.0))
    }
}

fn build_poster_metadata_block(
    title: SharedString,
    subtitle: Option<SharedString>,
    fallback_type_badge: Option<SharedString>,
    fallback_year: Option<SharedString>,
    theme: &ThemePalette,
) -> Div {
    let col = div()
        .w_full()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(theme.sp(2.0))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_size(theme.sp(13.5))
                .line_height(theme.sp(18.0))
                .text_color(rgb(theme.text_primary))
                .truncate()
                .child(title),
        );

    if let Some(sub) = subtitle {
        col.child(
            div()
                .font_weight(FontWeight::NORMAL)
                .font_features(tabular_nums())
                .text_size(theme.sp(12.0))
                .line_height(theme.sp(16.0))
                .text_color(rgb(theme.text_muted))
                .truncate()
                .child(sub),
        )
    } else if fallback_type_badge.is_some() || fallback_year.is_some() {
        let mut row = div()
            .w_full()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap(theme.sp(4.0))
            .font_weight(FontWeight::NORMAL)
            .font_features(tabular_nums())
            .text_size(theme.sp(12.0))
            .line_height(theme.sp(16.0))
            .text_color(rgb(theme.text_muted))
            .overflow_hidden();

        let has_both = fallback_type_badge.is_some() && fallback_year.is_some();
        if let Some(badge) = fallback_type_badge {
            row = row.child(div().flex_shrink_0().child(badge));
        }
        if has_both {
            row = row.child(div().flex_shrink_0().child(SharedString::new_static("·")));
        }
        if let Some(year) = fallback_year {
            row = row.child(div().min_w(px(0.0)).truncate().child(year));
        }
        col.child(row)
    } else {
        col
    }
}

/// Renders a flat `'static` [`PreparedPosterCard`] using a single shared [`CardActionDispatcher`].
///
/// Zero `format!` / `String` allocations per card: root uses `(id_prefix, card_index)`
/// (`ElementId::NamedInteger`), hover group uses static `POSTER_CARD_GROUP`, and child buttons use
/// static `ElementId`s scoped beneath the card's `GlobalElementId` stack.
pub fn render_prepared_poster_card(ctx: PreparedPosterRenderCtx<'_>) -> AnyElement {
    let theme = active_theme();
    let card_data = ctx.card;
    let caps = card_data.capabilities;
    let card_index = ctx.card_index;
    let dispatcher = ctx.dispatcher;

    let notif_label = card_data
        .notification_label
        .clone()
        .or_else(|| card_data.is_new.then(|| SharedString::new_static("NEW")));
    let progress = clamp_progress_fraction(card_data.progress_fraction);
    let status_tab = card_data
        .status_tab
        .or(card_data.watched.then_some(LibraryStatusTab::Watched));
    let has_toggle_library = caps.contains(PosterCardCapabilities::CAN_TOGGLE_LIBRARY);
    let rating_bottom_px = if progress.is_some() { 10.0 } else { 8.0 };
    let badge_fg_on_status = if theme.is_light {
        0xFFFFFF
    } else {
        theme.bg_canvas
    };

    let mut art_frame = build_poster_art_frame_base(
        PosterArtFrameSpec {
            title: &card_data.title,
            image: ctx.image,
            placeholder_tint: card_data.placeholder_tint,
            aspect: card_data.aspect,
            image_height_px: ctx.image_height_px,
            is_focused: ctx.is_focused,
            notif_label,
            extra_top_left_overlay: ctx.extra_top_left_overlay,
            status_tab,
            imdb_rating: card_data.imdb_rating.clone(),
            has_bottom_left_hover_btn: has_toggle_library,
            rating_bottom_px,
            badge_fg_on_status,
        },
        &theme,
    );

    if caps.contains(PosterCardCapabilities::CAN_QUICK_PLAY) {
        let dispatch = Rc::clone(dispatcher);
        art_frame = art_frame.child(build_poster_quick_play_overlay(
            "quick-play",
            &theme,
            move |_, window, cx| dispatch(card_index, PosterCardAction::QuickPlay, window, cx),
        ));
    }

    if has_toggle_library {
        let dispatch = Rc::clone(dispatcher);
        art_frame = art_frame.child(build_poster_toggle_library_button(
            "toggle-lib",
            card_data.in_library,
            rating_bottom_px,
            badge_fg_on_status,
            &theme,
            move |_, window, cx| dispatch(card_index, PosterCardAction::ToggleLibrary, window, cx),
        ));
    }

    let has_quick_peek = caps.contains(PosterCardCapabilities::CAN_QUICK_PEEK);
    if ctx.quick_action.is_some() || has_quick_peek {
        let peek_btn = has_quick_peek.then(|| {
            let dispatch = Rc::clone(dispatcher);
            build_poster_quick_peek_button("quick-peek", &theme, move |_, window, cx| {
                dispatch(card_index, PosterCardAction::QuickPeek, window, cx)
            })
        });
        art_frame = art_frame.child(build_poster_right_actions_cluster(
            rating_bottom_px,
            ctx.quick_action,
            peek_btn,
            &theme,
        ));
    }

    if let Some(frac) = progress {
        art_frame = art_frame.child(build_poster_progress_bar(frac, &theme));
    }

    let mut card = build_poster_card_shell((ctx.id_prefix, card_index), ctx.fixed_width_px, &theme);

    if caps.contains(PosterCardCapabilities::CAN_OPEN) {
        let dispatch = Rc::clone(dispatcher);
        card = card
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                dispatch(card_index, PosterCardAction::Open, window, cx);
            });
    }

    if caps.contains(PosterCardCapabilities::CAN_MIDDLE_CLICK) {
        let dispatch = Rc::clone(dispatcher);
        card = card.on_mouse_down(MouseButton::Middle, move |_, window, cx| {
            cx.stop_propagation();
            dispatch(card_index, PosterCardAction::MiddleClick, window, cx);
        });
    }

    if caps.contains(PosterCardCapabilities::CAN_CONTEXT_MENU) {
        let dispatch = Rc::clone(dispatcher);
        card = card.on_mouse_down(MouseButton::Right, move |ev, window, cx| {
            cx.stop_propagation();
            dispatch(
                card_index,
                PosterCardAction::ContextMenu(ev.position),
                window,
                cx,
            );
        });
    }

    if caps.contains(PosterCardCapabilities::CAN_HOVER) {
        let dispatch = Rc::clone(dispatcher);
        card = card.on_hover(move |hovered, window, cx| {
            dispatch(card_index, PosterCardAction::Hover(*hovered), window, cx);
        });
    }

    card.child(art_frame)
        .child(build_poster_metadata_block(
            card_data.title.clone(),
            card_data.subtitle.clone(),
            card_data.type_badge.clone(),
            card_data.year.clone(),
            &theme,
        ))
        .into_any_element()
}

/// Renders the unified Ethereal poster/episode card with luminous hairline border, ambient hover glow,
/// focus ring, badges, progress bar, quick-peek drawer button, and optional quick-action slot.
pub fn render_poster_card(props: PosterCardProps) -> AnyElement {
    let theme = active_theme();
    let notif_label = format_notification_badge_label(props.notification_count);
    let progress = clamp_progress_fraction(props.progress_fraction);
    let has_bottom_left_hover_btn = props.on_toggle_library.is_some();
    let rating_bottom_px = if progress.is_some() { 10.0 } else { 8.0 };
    let badge_fg_on_status = if theme.is_light {
        0xFFFFFF
    } else {
        theme.bg_canvas
    };

    let mut art_frame = build_poster_art_frame_base(
        PosterArtFrameSpec {
            title: &props.title,
            image: props.image,
            placeholder_tint: props.placeholder_tint,
            aspect: props.aspect,
            image_height_px: props.image_height_px,
            is_focused: props.is_focused,
            notif_label,
            extra_top_left_overlay: None,
            status_tab: props.status_tab,
            imdb_rating: props.imdb_rating,
            has_bottom_left_hover_btn,
            rating_bottom_px,
            badge_fg_on_status,
        },
        &theme,
    );

    if let Some(on_quick_play) = props.on_quick_play {
        art_frame = art_frame.child(build_poster_quick_play_overlay(
            "quick-play",
            &theme,
            on_quick_play,
        ));
    }

    if let Some(on_toggle_library) = props.on_toggle_library {
        art_frame = art_frame.child(build_poster_toggle_library_button(
            "toggle-lib",
            props.in_library,
            rating_bottom_px,
            badge_fg_on_status,
            &theme,
            on_toggle_library,
        ));
    }

    if props.quick_action.is_some() || props.on_quick_peek.is_some() {
        let peek_btn = props
            .on_quick_peek
            .map(|on_quick_peek| build_poster_quick_peek_button("quick-peek", &theme, on_quick_peek));
        art_frame = art_frame.child(build_poster_right_actions_cluster(
            rating_bottom_px,
            props.quick_action,
            peek_btn,
            &theme,
        ));
    }

    if let Some(frac) = progress {
        art_frame = art_frame.child(build_poster_progress_bar(frac, &theme));
    }

    let mut card = build_poster_card_shell(props.id, props.fixed_width_px, &theme);

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
        .child(build_poster_metadata_block(
            props.title,
            props.subtitle,
            None,
            None,
            &theme,
        ))
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
    let theme = active_theme();
    let accent_ink = if theme.is_light {
        theme.accent_press
    } else {
        theme.accent
    };
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
        .px(theme.sp(16.0))
        .py(theme.sp(12.0))
        .when(!props.is_active, |row| {
            row.hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(theme.sp(14.0));

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
        .gap(theme.sp(3.0))
        .child(
            div()
                .w_full()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(theme.sp(8.0))
                .child(svg_icon(
                    IconName::Play,
                    12.0 * theme.ui_scale,
                    rgb(accent_ink),
                ))
                .child(
                    div()
                        .min_w(px(0.0))
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(theme.sp(13.5))
                        .text_color(rgb(theme.text_primary))
                        .child(props.headline),
                )
                .when(props.is_last_used, |row| {
                    row.child(
                        div()
                            .flex_shrink_0()
                            .px(theme.sp(7.0))
                            .py(theme.sp(2.0))
                            .rounded(px(RADIUS_PILL))
                            .bg(rgba(theme.accent_soft))
                            .border_1()
                            .border_color(rgba(theme.border_accent))
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(theme.sp(10.5))
                            .text_color(rgb(accent_ink))
                            .child("Last used"),
                    )
                })
                .when_some(props.addon_label, |row, addon| {
                    row.child(
                        div()
                            .flex_shrink_0()
                            .font_weight(FontWeight::NORMAL)
                            .text_size(theme.sp(11.5))
                            .text_color(rgb(theme.text_muted))
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
                .gap(theme.sp(6.0))
                .when(props.is_cached, |row| {
                    row.child(
                        div()
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_full()
                            .flex_shrink_0()
                            .bg(rgb(theme.status_watched)),
                    )
                })
                .child(
                    div()
                        .min_w(px(0.0))
                        .truncate()
                        .font_weight(FontWeight::NORMAL)
                        .font_features(tabular_nums())
                        .text_size(theme.sp(12.0))
                        .text_color(rgb(theme.text_secondary))
                        .child(props.metadata_line),
                ),
        )
        .when_some(props.release_title, |col, release| {
            col.child(
                div()
                    .w_full()
                    .min_w(px(0.0))
                    .font(mono_font())
                    .text_size(theme.sp(11.5))
                    .text_color(rgb(theme.text_muted))
                    .truncate()
                    .text_ellipsis()
                    .child(release),
            )
        });

    let mut right_col = div().flex_shrink_0().flex().items_center().gap(theme.sp(8.0));

    if props.is_active {
        right_col = right_col.child(svg_icon(
            IconName::Check,
            15.0 * theme.ui_scale,
            rgb(accent_ink),
        ));
    }

    if let Some(on_toggle_expand) = props.on_toggle_expand {
        let expanded = props.is_expanded;
        right_col = right_col.child(
            div()
                .id(expand_id)
                .cursor_pointer()
                .px(theme.sp(9.0))
                .py(theme.sp(4.0))
                .rounded(px(RADIUS_POSTER))
                .bg(if expanded {
                    rgba(theme.accent_soft)
                } else {
                    rgba(0x00000000)
                })
                .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                .flex()
                .items_center()
                .gap(theme.sp(4.0))
                .font_weight(FontWeight::MEDIUM)
                .text_size(theme.sp(11.5))
                .text_color(if expanded {
                    rgb(accent_ink)
                } else {
                    rgb(theme.text_muted)
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
                    11.5 * theme.ui_scale,
                    if expanded {
                        rgb(accent_ink)
                    } else {
                        rgb(theme.text_muted)
                    },
                )),
        );
    }

    main_row = main_row.child(left_col).child(right_col);

    let mut outer = div()
        .w_full()
        .rounded(px(RADIUS_CARD))
        .bg(if props.is_active {
            rgba(theme.glass_row_selected_bg)
        } else {
            rgba(theme.glass_card_bg)
        })
        .border_1()
        .border_color(if props.is_active {
            rgba(theme.border_accent)
        } else {
            rgba(theme.glass_border)
        })
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(main_row);

    if props.is_expanded && has_drawer_content {
        let mut drawer = div()
            .w_full()
            .px(theme.sp(16.0))
            .py(theme.sp(12.0))
            .bg(rgba(theme.glass_trough_bg))
            .border_t_1()
            .border_color(rgba(theme.glass_divider))
            .flex()
            .flex_col()
            .gap(theme.sp(8.0))
            .when_some(props.raw_filename, |el, filename| {
                el.child(
                    div()
                        .font(mono_font())
                        .text_size(theme.sp(11.5))
                        .text_color(rgb(theme.text_secondary))
                        .child(filename),
                )
            })
            .when(!props.technical_specs.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(theme.sp(6.0))
                        .children(props.technical_specs.into_iter().map(|spec| {
                            div()
                                .px(theme.sp(7.0))
                                .py(theme.sp(2.0))
                                .rounded(px(RADIUS_PILL))
                                .bg(rgba(theme.glass_trough_bg))
                                .border_1()
                                .border_color(rgba(theme.glass_border))
                                .font(mono_font())
                                .text_size(theme.sp(11.0))
                                .text_color(rgb(theme.text_muted))
                                .child(spec)
                        })),
                )
            })
            .when_some(props.info_hash, |el, hash| {
                el.child(
                    div()
                        .font(mono_font())
                        .text_size(theme.sp(11.0))
                        .text_color(rgb(theme.text_muted))
                        .child(format!("info_hash {hash}")),
                )
            });

        if props.on_copy_url.is_some() || props.on_copy_hash.is_some() {
            let mut actions = div().flex().flex_wrap().items_center().gap(theme.sp(8.0));
            if let Some(on_copy_url) = props.on_copy_url {
                actions = actions.child(render_drawer_copy_button(
                    copy_url_id,
                    "Copy URL",
                    &theme,
                    on_copy_url,
                ));
            }
            if let Some(on_copy_hash) = props.on_copy_hash {
                actions = actions.child(render_drawer_copy_button(
                    copy_hash_id,
                    "Copy hash",
                    &theme,
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
    theme: &crate::theme::ThemePalette,
    on_click: ClickHandler,
) -> Stateful<Div> {
    let theme = *theme;
    div()
        .id(id)
        .cursor_pointer()
        .h(theme.sp(28.0))
        .px(theme.sp(10.0))
        .rounded(px(RADIUS_POSTER))
        .bg(rgba(theme.glass_trough_bg))
        .border_1()
        .border_color(rgba(theme.glass_border))
        .hover(move |s| {
            s.bg(rgba(theme.glass_row_hover_bg))
                .border_color(rgba(theme.glass_border_strong))
        })
        .flex()
        .items_center()
        .gap(theme.sp(6.0))
        .font_weight(FontWeight::MEDIUM)
        .text_size(theme.sp(11.5))
        .text_color(rgb(theme.text_secondary))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(svg_icon(
            IconName::Clipboard,
            11.5 * theme.ui_scale,
            rgb(theme.text_muted),
        ))
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

    #[test]
    fn poster_card_props_builders_set_quick_peek_and_slot() {
        let props = PosterCardProps::new("card-1", "Dune", "tt1160419")
            .on_quick_peek(|_, _, _| {})
            .quick_action(div().into_any_element());
        assert!(props.on_quick_peek.is_some());
        assert!(props.quick_action.is_some());

        let default_props = PosterCardProps::new("card-2", "Arrival", "tt2543164");
        assert!(default_props.on_quick_peek.is_none());
        assert!(default_props.quick_action.is_none());
    }

    #[test]
    fn poster_card_capabilities_bitflags_compose_and_filter_cleanly() {
        assert_eq!(std::mem::size_of::<PosterCardCapabilities>(), 1);
        assert!(PosterCardCapabilities::NONE.is_empty());
        assert_eq!(PosterCardCapabilities::empty(), PosterCardCapabilities::NONE);

        let mut caps = PosterCardCapabilities::CAN_OPEN | PosterCardCapabilities::CAN_QUICK_PEEK;
        assert!(caps.contains(PosterCardCapabilities::CAN_OPEN));
        assert!(caps.contains(PosterCardCapabilities::CAN_QUICK_PEEK));
        assert!(!caps.contains(PosterCardCapabilities::CAN_QUICK_PLAY));
        assert!(caps.intersects(PosterCardCapabilities::STANDARD_BROWSING));

        caps |= PosterCardCapabilities::CAN_MIDDLE_CLICK;
        assert!(caps.contains(PosterCardCapabilities::CAN_MIDDLE_CLICK));
        assert!(!PosterCardCapabilities::STANDARD_BROWSING.contains(PosterCardCapabilities::CAN_MIDDLE_CLICK));
        assert!(PosterCardCapabilities::ALL.contains(PosterCardCapabilities::CAN_MIDDLE_CLICK));
        assert!(PosterCardCapabilities::ALL.contains(PosterCardCapabilities::STANDARD_BROWSING));

        let trimmed = caps.without(PosterCardCapabilities::CAN_QUICK_PEEK);
        assert!(trimmed.contains(PosterCardCapabilities::CAN_OPEN));
        assert!(!trimmed.contains(PosterCardCapabilities::CAN_QUICK_PEEK));

        let intersected = PosterCardCapabilities::ALL & PosterCardCapabilities::CAN_HOVER;
        assert_eq!(intersected, PosterCardCapabilities::CAN_HOVER);

        let truncated = PosterCardCapabilities::from_bits_truncate(0xFF);
        assert_eq!(truncated, PosterCardCapabilities::ALL);
    }

    #[test]
    fn prepared_poster_card_precomputes_tint_notifications_and_fields() {
        let card = PreparedPosterCard::from_ref("Dune: Part Two", "tt15239678")
            .subtitle_str(Some("Movie · 2024"))
            .year_str(Some("2024"))
            .imdb_rating_str(Some("8.6"))
            .type_badge_str(Some("Movie"))
            .poster_url_str(Some("https://example.com/dune2.jpg"))
            .aspect(PosterAspect::Portrait2x3)
            .status_tab(Some(LibraryStatusTab::Watching))
            .progress_fraction(Some(1.4))
            .in_library(true)
            .watched(false)
            .notification_count(3)
            .capabilities(PosterCardCapabilities::ALL);

        assert_eq!(card.title.as_ref(), "Dune: Part Two");
        assert_eq!(card.subtitle.as_deref(), Some("Movie · 2024"));
        assert_eq!(card.year.as_deref(), Some("2024"));
        assert_eq!(card.imdb_rating.as_deref(), Some("8.6"));
        assert_eq!(card.type_badge.as_deref(), Some("Movie"));
        assert_eq!(card.poster_url.as_deref(), Some("https://example.com/dune2.jpg"));
        assert_eq!(card.placeholder_tint, placeholder_tint_for_id("tt15239678"));
        assert_eq!(card.progress_fraction, Some(1.0));
        assert_eq!(card.notification_count, 3);
        assert_eq!(card.notification_label.as_deref(), Some("3 NEW"));
        assert!(card.is_new);
        assert!(card.in_library);
        assert!(!card.watched);
        assert_eq!(card.status_tab, Some(LibraryStatusTab::Watching));
        assert_eq!(card.capabilities, PosterCardCapabilities::ALL);

        // Slab-friendly: Arc<[PreparedPosterCard]> is `'static` and cheaply cloneable.
        let slab: Arc<[PreparedPosterCard]> = Arc::from(vec![card.clone()]);
        assert_eq!(slab.len(), 1);
        assert_eq!(slab[0], card);
    }

    #[test]
    fn prepared_poster_card_notification_and_empty_str_handling() {
        let single_new = PreparedPosterCard::new("Severance", "tt11280740").notification_count(1);
        assert_eq!(single_new.notification_label, Some(SharedString::new_static("NEW")));
        assert!(single_new.is_new);

        let cleared = single_new.notification_count(0);
        assert_eq!(cleared.notification_label, None);
        assert!(!cleared.is_new);

        let flagged_new = PreparedPosterCard::new("Silo", "tt14688458")
            .subtitle_str(Some(""))
            .year_str(None)
            .progress_fraction(Some(0.005))
            .is_new(true);
        assert_eq!(flagged_new.subtitle, None);
        assert_eq!(flagged_new.year, None);
        assert_eq!(flagged_new.progress_fraction, None);
        assert_eq!(flagged_new.notification_label, Some(SharedString::new_static("NEW")));
        assert!(flagged_new.is_new);

        let unflagged = flagged_new.is_new(false);
        assert_eq!(unflagged.notification_label, None);
        assert!(!unflagged.is_new);
    }

    #[test]
    fn prepared_poster_card_with_tint_and_dispatcher_smoke() {
        let card = PreparedPosterCard::with_tint("Arrival", 0x112233)
            .capabilities(PosterCardCapabilities::STANDARD_BROWSING);
        assert_eq!(card.placeholder_tint, 0x112233);

        let dispatcher = card_action_dispatcher(|_idx, _action, _window, _cx| {});
        let ctx = PreparedPosterRenderCtx::new(7, &card, "discover-card", &dispatcher)
            .fixed_width_px(Some(180.0))
            .image_height_px(Some(270.0))
            .is_focused(true);
        assert_eq!(ctx.card_index, 7);
        assert_eq!(ctx.id_prefix, "discover-card");
        assert_eq!(ctx.fixed_width_px, Some(180.0));
        assert_eq!(ctx.image_height_px, Some(270.0));
        assert!(ctx.is_focused);

        // Verify `render_prepared_poster_card` builds an element cleanly without panic.
        let _el = render_prepared_poster_card(ctx);
    }
}
