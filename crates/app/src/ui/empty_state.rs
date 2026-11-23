use gpui::{div, prelude::*, px, relative, rgb, rgba, AnyElement, Div, SharedString};

use crate::{
    icons::{svg_icon, IconName},
    theme::{active_theme, editorial_serif, RADIUS_CARD, RADIUS_PILL},
};

/// Props for the unified centered frosted-glass empty state card shared across
/// Library, Discover, Calendar, Addons, Settings, Search, and Home.
pub struct EmptyStateProps {
    pub icon: IconName,
    pub title: SharedString,
    pub body: SharedString,
    pub actions: Vec<AnyElement>,
    pub fill_available: bool,
    pub max_width_px: f32,
}

impl EmptyStateProps {
    pub fn new(
        icon: IconName,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
    ) -> Self {
        Self {
            icon,
            title: title.into(),
            body: body.into(),
            actions: Vec::new(),
            fill_available: false,
            max_width_px: 560.0,
        }
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }

    pub fn actions(mut self, actions: impl IntoIterator<Item = AnyElement>) -> Self {
        self.actions.extend(actions);
        self
    }

    pub fn fill_available(mut self, fill: bool) -> Self {
        self.fill_available = fill;
        self
    }

    pub fn max_width_px(mut self, max_width_px: f32) -> Self {
        self.max_width_px = max_width_px;
        self
    }
}

/// Renders the canonical centered glass empty-state card (`RADIUS_CARD`, 44×44 circular icon badge,
/// `editorial_serif()` `sp(28.0)` heading, `sp(13.5)` body, and optional primary/secondary actions).
pub fn render_empty_state(props: EmptyStateProps) -> Div {
    let theme = active_theme();
    let card = div()
        .w_full()
        .max_w(theme.sp(props.max_width_px))
        .py(theme.sp(52.0))
        .px(theme.sp(28.0))
        .rounded(px(RADIUS_CARD))
        .border_1()
        .border_color(rgba(theme.glass_border))
        .bg(rgba(theme.glass_card_bg))
        .flex()
        .flex_col()
        .items_center()
        .gap(theme.sp(12.0))
        .child(
            div()
                .w(theme.sp(44.0))
                .h(theme.sp(44.0))
                .flex_shrink_0()
                .rounded(px(RADIUS_PILL))
                .bg(rgba(theme.glass_trough_bg))
                .border_1()
                .border_color(rgba(theme.glass_border))
                .flex()
                .items_center()
                .justify_center()
                .child(svg_icon(
                    props.icon,
                    19.0 * theme.ui_scale,
                    rgb(theme.text_muted),
                )),
        )
        .child(
            div()
                .font(editorial_serif())
                .text_size(theme.sp(28.0))
                .line_height(relative(1.12))
                .text_color(rgb(theme.text_primary))
                .text_center()
                .child(props.title),
        )
        .child(
            div()
                .max_w(theme.sp(420.0))
                .text_size(theme.sp(13.5))
                .text_color(rgb(theme.text_secondary))
                .text_center()
                .child(props.body),
        )
        .when(!props.actions.is_empty(), |card| {
            card.child(
                div()
                    .pt(theme.sp(6.0))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_center()
                    .gap(theme.sp(10.0))
                    .children(props.actions),
            )
        });

    let wrapper = div()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .child(card);

    if props.fill_available {
        wrapper.flex_1().min_h(px(0.0))
    } else {
        wrapper
    }
}
