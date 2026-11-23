use std::sync::Arc;

use gpui::{
    div, prelude::*, px, rgb, rgba, App, Div, ElementId, FontWeight, MouseButton, MouseDownEvent,
    SharedString, Stateful, Window,
};

use crate::{
    icons::{svg_icon, IconName},
    theme::{active_theme, sp, ui_sans, RADIUS_CARD, RADIUS_CONTROL, RADIUS_PILL},
};

pub use crate::theme::{keycap, meta_divider, neutral_chip, semantic_status_pill};
pub use crate::ui_components::{
    click_handler, glass_back_button, glass_dropdown_option, glass_dropdown_panel,
    glass_segmented_bar, glass_segmented_item, glass_stepper, glass_switch, hover_handler,
    primary_accent_button, render_stream_source_row, secondary_glass_button, ClickHandler,
    HoverHandler, StreamSourceRowProps,
};

/// Single floating frosted-glass toolbar card (`RADIUS_CARD`, `px(12)`, `py(9)`)
/// shared across Discover, Library, and Addons.
pub fn toolbar_bar() -> Div {
    let theme = active_theme();
    div()
        .w_full()
        .flex_shrink_0()
        .px(sp(12.0))
        .py(sp(9.0))
        .rounded(px(RADIUS_CARD))
        .bg(rgba(theme.glass_card_bg))
        .border_1()
        .border_color(rgba(theme.glass_border))
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap(sp(10.0))
}

/// Props for the canonical `h(sp(36.0))`, `RADIUS_CONTROL` inline filter/search input.
pub struct SearchFieldProps {
    pub id: ElementId,
    pub clear_id: Option<ElementId>,
    pub value_display: SharedString,
    pub placeholder: SharedString,
    pub is_empty: bool,
    pub is_focused: bool,
    pub width_px: Option<f32>,
    pub height_px: f32,
    pub on_focus: Option<ClickHandler>,
    pub on_clear: Option<ClickHandler>,
}

impl SearchFieldProps {
    pub fn new(
        id: impl Into<ElementId>,
        value_display: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
        is_empty: bool,
        is_focused: bool,
    ) -> Self {
        Self {
            id: id.into(),
            clear_id: None,
            value_display: value_display.into(),
            placeholder: placeholder.into(),
            is_empty,
            is_focused,
            width_px: Some(220.0),
            height_px: 36.0,
            on_focus: None,
            on_clear: None,
        }
    }

    pub fn clear_id(mut self, id: impl Into<ElementId>) -> Self {
        self.clear_id = Some(id.into());
        self
    }

    pub fn width_px(mut self, width_px: impl Into<Option<f32>>) -> Self {
        self.width_px = width_px.into();
        self
    }

    pub fn height_px(mut self, height_px: f32) -> Self {
        self.height_px = height_px;
        self
    }

    pub fn full_width(mut self) -> Self {
        self.width_px = None;
        self
    }

    pub fn on_focus(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_focus = Some(Box::new(handler));
        self
    }

    pub fn on_clear(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_clear = Some(Box::new(handler));
        self
    }
}

/// Renders the canonical `h(sp(36.0))`, `RADIUS_CONTROL` inline filter input with leading search
/// icon and optional trailing clear (`X`) button.
pub fn search_field(props: SearchFieldProps) -> Stateful<Div> {
    let theme = active_theme();
    let accent_ink = if theme.is_light {
        theme.accent_press
    } else {
        theme.accent
    };
    let focused = props.is_focused;
    let is_empty = props.is_empty;
    let clear_id = props.clear_id.unwrap_or_else(|| {
        ElementId::NamedChild(
            Arc::new(props.id.clone()),
            SharedString::new_static("clear"),
        )
    });

    let mut box_el = div()
        .id(props.id)
        .cursor_text()
        .h(sp(props.height_px))
        .px(sp(11.0))
        .rounded(px(RADIUS_CONTROL))
        .bg(if focused {
            rgba(theme.glass_row_selected_bg)
        } else {
            rgba(theme.glass_trough_bg)
        })
        .border_1()
        .border_color(if focused {
            rgba(theme.border_accent)
        } else {
            rgba(theme.glass_border)
        })
        .hover(move |s| {
            s.border_color(if focused {
                rgba(theme.border_accent)
            } else {
                rgba(theme.glass_border_strong)
            })
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(sp(8.0));

    if let Some(w) = props.width_px {
        box_el = box_el.w(sp(w));
    } else {
        box_el = box_el.w_full();
    }

    if let Some(on_focus) = props.on_focus {
        box_el = box_el.on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_focus(ev, window, cx);
        });
    }

    box_el = box_el.child(
        div()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap(sp(7.0))
            .child(svg_icon(
                IconName::Search,
                13.5 * theme.ui_scale,
                if focused {
                    rgb(accent_ink)
                } else {
                    rgb(theme.text_muted)
                },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .font(ui_sans(FontWeight::NORMAL))
                    .text_size(sp(12.5))
                    .text_color(if is_empty {
                        rgb(theme.text_muted)
                    } else {
                        rgb(theme.text_primary)
                    })
                    .truncate()
                    .when(focused || !is_empty, |d| d.child(props.value_display))
                    .when(is_empty, |d| {
                        d.child(
                            div()
                                .when(focused, |p| p.pl(sp(3.0)))
                                .text_color(rgb(theme.text_muted))
                                .child(props.placeholder),
                        )
                    }),
            ),
    );

    if !is_empty {
        if let Some(on_clear) = props.on_clear {
            box_el = box_el.child(
                div()
                    .id(clear_id)
                    .cursor_pointer()
                    .p(sp(3.0))
                    .rounded(px(4.0))
                    .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                        cx.stop_propagation();
                        on_clear(ev, window, cx);
                    })
                    .child(svg_icon(
                        IconName::X,
                        11.5 * theme.ui_scale,
                        rgb(theme.text_muted),
                    )),
            );
        }
    }

    box_el
}

/// Props for the canonical `h(sp(36.0))`, `RADIUS_CONTROL` dropdown trigger button with chevron.
pub struct DropdownTriggerProps {
    pub id: ElementId,
    pub label: SharedString,
    pub prefix_label: Option<SharedString>,
    pub secondary_label: Option<SharedString>,
    pub leading_icon: Option<IconName>,
    pub is_open: bool,
    pub is_active: bool,
    pub min_width_px: Option<f32>,
    pub max_width_px: Option<f32>,
    pub on_click: ClickHandler,
}

impl DropdownTriggerProps {
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        is_open: bool,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            prefix_label: None,
            secondary_label: None,
            leading_icon: None,
            is_open,
            is_active: false,
            min_width_px: None,
            max_width_px: None,
            on_click: Box::new(on_click),
        }
    }

    pub fn prefix_label(mut self, prefix: impl Into<Option<SharedString>>) -> Self {
        self.prefix_label = prefix.into();
        self
    }

    pub fn secondary_label(mut self, label: impl Into<Option<SharedString>>) -> Self {
        self.secondary_label = label.into();
        self
    }

    pub fn leading_icon(mut self, icon: impl Into<Option<IconName>>) -> Self {
        self.leading_icon = icon.into();
        self
    }

    pub fn is_active(mut self, active: bool) -> Self {
        self.is_active = active;
        self
    }

    pub fn min_width_px(mut self, min_width_px: impl Into<Option<f32>>) -> Self {
        self.min_width_px = min_width_px.into();
        self
    }

    pub fn max_width_px(mut self, max_width_px: impl Into<Option<f32>>) -> Self {
        self.max_width_px = max_width_px.into();
        self
    }
}

/// Renders the canonical `h(sp(36.0))`, `RADIUS_CONTROL` dropdown trigger button with trailing chevron.
pub fn dropdown_trigger(props: DropdownTriggerProps) -> Stateful<Div> {
    let theme = active_theme();
    let accent_ink = if theme.is_light {
        theme.accent_press
    } else {
        theme.accent
    };
    let highlighted = props.is_open || props.is_active;
    let on_click = props.on_click;
    let spread_between = props.min_width_px.is_some();

    let mut btn = div()
        .id(props.id)
        .cursor_pointer()
        .h(sp(36.0))
        .px(sp(11.0))
        .rounded(px(RADIUS_CONTROL))
        .bg(if props.is_open {
            rgba(theme.glass_row_selected_bg)
        } else if props.is_active {
            rgba(theme.accent_soft)
        } else {
            rgba(theme.glass_trough_bg)
        })
        .border_1()
        .border_color(if highlighted {
            rgba(theme.border_accent)
        } else {
            rgba(theme.glass_border)
        })
        .hover(move |s| {
            s.bg(if props.is_open {
                rgba(theme.glass_row_selected_bg)
            } else if props.is_active {
                rgba(theme.accent_soft)
            } else {
                rgba(theme.glass_row_hover_bg)
            })
            .border_color(if highlighted {
                rgb(theme.accent)
            } else {
                rgba(theme.glass_border_strong)
            })
        })
        .flex()
        .items_center()
        .when(spread_between, |el| el.justify_between())
        .gap(sp(7.0))
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(sp(12.5))
        .text_color(if highlighted {
            rgb(accent_ink)
        } else {
            rgb(theme.text_primary)
        })
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        });

    if let Some(min_w) = props.min_width_px {
        btn = btn.min_w(sp(min_w));
    }
    if let Some(max_w) = props.max_width_px {
        btn = btn.max_w(sp(max_w));
    }

    let left_content = div()
        .min_w(px(0.0))
        .flex()
        .items_center()
        .gap(sp(6.0))
        .when_some(props.leading_icon, |row, icon| {
            row.child(svg_icon(
                icon,
                13.0 * theme.ui_scale,
                if highlighted {
                    rgb(accent_ink)
                } else {
                    rgb(theme.text_muted)
                },
            ))
        })
        .when_some(props.prefix_label, |row, prefix| {
            row.child(
                div()
                    .flex_shrink_0()
                    .font(ui_sans(FontWeight::NORMAL))
                    .text_size(sp(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(prefix),
            )
            .child(
                div()
                    .w(px(1.0))
                    .h(sp(12.0))
                    .flex_shrink_0()
                    .bg(rgba(theme.glass_divider)),
            )
        })
        .child(div().min_w(px(0.0)).truncate().child(props.label))
        .when_some(props.secondary_label, |row, secondary| {
            row.child(
                div()
                    .flex_shrink_0()
                    .font(ui_sans(FontWeight::NORMAL))
                    .text_size(sp(11.5))
                    .text_color(rgb(theme.text_muted))
                    .truncate()
                    .child(secondary),
            )
        });

    btn.child(left_content).child(svg_icon(
        IconName::ChevronDown,
        12.0 * theme.ui_scale,
        if highlighted {
            rgb(accent_ink)
        } else {
            rgb(theme.text_muted)
        },
    ))
}

/// Canonical pill choice/filter button (replaces view-local copies in `settings_view.rs` and `addons_view.rs`).
pub fn choice_pill(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let theme = active_theme();
    let accent_ink = if theme.is_light {
        theme.accent_press
    } else {
        theme.accent
    };

    div()
        .id(id)
        .cursor_pointer()
        .h(sp(32.0))
        .px(sp(13.0))
        .rounded(px(RADIUS_PILL))
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
        .whitespace_nowrap()
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(sp(12.5))
        .text_color(if active {
            rgb(accent_ink)
        } else {
            rgb(theme.text_secondary)
        })
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(label.into())
}
