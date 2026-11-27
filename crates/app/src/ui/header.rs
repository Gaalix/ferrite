use gpui::{
    div, prelude::*, px, relative, rgb, rgba, AnyElement, App, Div, ElementId, FontWeight,
    MouseDownEvent, SharedString, Window,
};

use crate::{
    icons::{svg_icon_weighted, IconName, IconWeight},
    theme::{
        active_theme, editorial_serif, mono_font, tabular_nums, ui_sans, ThemePalette, RADIUS_CARD,
        RADIUS_PILL,
    },
    ui_components::{glass_back_button, ClickHandler},
};

/// Leading back-button descriptor for `PageHeader` on secondary routes (Search, Person).
pub struct PageHeaderBackButton {
    pub id: ElementId,
    pub label: SharedString,
    pub on_click: ClickHandler,
}

/// Canonical top-of-page header shared across Library, Discover, Calendar, Addons, Account,
/// Settings, Search, and Person routes.
pub struct PageHeader {
    pub title: SharedString,
    pub count_badge: Option<SharedString>,
    pub subtitle: Option<SharedString>,
    pub back_button: Option<PageHeaderBackButton>,
    pub leading_icon: Option<IconName>,
    pub right_slot: Option<AnyElement>,
    pub bottom_divider: bool,
}

impl PageHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            count_badge: None,
            subtitle: None,
            back_button: None,
            leading_icon: None,
            right_slot: None,
            bottom_divider: false,
        }
    }

    pub fn count_badge(mut self, badge: impl Into<Option<SharedString>>) -> Self {
        self.count_badge = badge.into();
        self
    }

    pub fn subtitle(mut self, subtitle: impl Into<Option<SharedString>>) -> Self {
        self.subtitle = subtitle.into();
        self
    }

    pub fn back_button(
        mut self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.back_button = Some(PageHeaderBackButton {
            id: id.into(),
            label: label.into(),
            on_click: Box::new(on_click),
        });
        self
    }

    pub fn leading_icon(mut self, icon: impl Into<Option<IconName>>) -> Self {
        self.leading_icon = icon.into();
        self
    }

    pub fn right_slot(mut self, slot: impl Into<Option<AnyElement>>) -> Self {
        self.right_slot = slot.into();
        self
    }

    pub fn bottom_divider(mut self, show: bool) -> Self {
        self.bottom_divider = show;
        self
    }
}

/// Renders the canonical Ferrite page header (`editorial_serif()` `sp(36.0)`, `line_height(1.08)`).
pub fn render_page_header(props: PageHeader) -> Div {
    let theme = active_theme();
    let accent_ink = if theme.is_light {
        theme.accent_press
    } else {
        theme.accent
    };

    let title_row = div()
        .flex()
        .flex_wrap()
        .items_baseline()
        .gap(theme.sp(12.0))
        .child(
            div()
                .font(editorial_serif())
                .text_size(theme.text_hero())
                .line_height(relative(1.08))
                .text_color(rgb(theme.text_primary))
                .child(props.title),
        )
        .when_some(props.count_badge, |row, badge| {
            row.child(render_header_count_pill(badge, &theme))
        });

    let title_col = div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(theme.sp(4.0))
        .child(title_row)
        .when_some(props.subtitle, |col, sub| {
            col.child(
                div()
                    .font(ui_sans(FontWeight::NORMAL))
                    .text_size(theme.text_body())
                    .text_color(rgb(theme.text_secondary))
                    .child(sub),
            )
        });

    let left_group = div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .items_center()
        .gap(theme.sp(14.0))
        .when_some(props.back_button, |row, back| {
            row.child(glass_back_button(back.id, back.label, back.on_click))
        })
        .when_some(props.leading_icon, |row, icon| {
            row.child(
                div()
                    .w(theme.sp(42.0))
                    .h(theme.sp(42.0))
                    .flex_shrink_0()
                    .rounded(px(RADIUS_CARD))
                    .bg(rgba(theme.accent_soft))
                    .border_1()
                    .border_color(rgba(theme.border_accent))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(svg_icon_weighted(
                        icon,
                        IconWeight::Fill,
                        19.0 * theme.ui_scale,
                        rgb(accent_ink),
                    )),
            )
        })
        .child(title_col);

    div()
        .w_full()
        .flex_shrink_0()
        .when(props.bottom_divider, |el| {
            el.pb(theme.sp(14.0))
                .border_b_1()
                .border_color(rgba(theme.glass_divider))
        })
        .flex()
        .flex_wrap()
        .items_end()
        .justify_between()
        .gap(theme.sp(16.0))
        .child(left_group)
        .when_some(props.right_slot, |row, right| {
            row.child(div().flex_shrink_0().child(right))
        })
}

/// Canonical section/shelf header shared across Home shelves, Calendar sections, Search bands,
/// Library Up Next, and MetaDetails sections.
pub struct SectionHeader {
    pub title: SharedString,
    pub count_badge: Option<SharedString>,
    pub inline_badge: Option<AnyElement>,
    pub subtitle: Option<SharedString>,
    pub right_slot: Option<AnyElement>,
    /// When `true`, prevents header rows/controls from wrapping onto a second line and truncates
    /// long titles with an ellipsis so fixed-height virtualized shelves never clip.
    pub single_line: bool,
}

impl SectionHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            count_badge: None,
            inline_badge: None,
            subtitle: None,
            right_slot: None,
            single_line: false,
        }
    }

    pub fn count_badge(mut self, badge: impl Into<Option<SharedString>>) -> Self {
        self.count_badge = badge.into();
        self
    }

    pub fn inline_badge(mut self, badge: impl Into<Option<AnyElement>>) -> Self {
        self.inline_badge = badge.into();
        self
    }

    pub fn subtitle(mut self, subtitle: impl Into<Option<SharedString>>) -> Self {
        self.subtitle = subtitle.into();
        self
    }

    pub fn right_slot(mut self, slot: impl Into<Option<AnyElement>>) -> Self {
        self.right_slot = slot.into();
        self
    }

    pub fn single_line(mut self, single_line: bool) -> Self {
        self.single_line = single_line;
        self
    }
}

/// Renders the canonical Ferrite section header (`editorial_serif()` `sp(26.0)`).
pub fn render_section_header(props: SectionHeader) -> Div {
    let theme = active_theme();
    let single_line = props.single_line;

    let title_row = div()
        .w_full()
        .min_w(px(0.0))
        .flex()
        .when(!single_line, |el| el.flex_wrap())
        .items_baseline()
        .gap(theme.sp(10.0))
        .child(
            div()
                .font(editorial_serif())
                .text_size(theme.text_display())
                .line_height(relative(1.1))
                .text_color(rgb(theme.text_primary))
                .when(single_line, |el| el.min_w(px(0.0)).truncate())
                .child(props.title),
        )
        .when_some(props.count_badge, |row, badge| {
            row.child(render_header_count_pill(badge, &theme))
        })
        .when_some(props.inline_badge, |row, badge| row.child(badge));

    let left_col = div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(theme.sp(2.0))
        .child(title_row)
        .when_some(props.subtitle, |col, sub| {
            col.child(
                div()
                    .font(ui_sans(FontWeight::NORMAL))
                    .text_size(theme.text_body())
                    .line_height(theme.sp(18.0))
                    .text_color(rgb(theme.text_secondary))
                    .truncate()
                    .child(sub),
            )
        });

    div()
        .w_full()
        .flex_shrink_0()
        .flex()
        .when(!single_line, |el| el.flex_wrap())
        .items_end()
        .justify_between()
        .gap(theme.sp(12.0))
        .child(left_col)
        .when_some(props.right_slot, |row, right| {
            row.child(div().flex_shrink_0().child(right))
        })
}

fn render_header_count_pill(label: SharedString, theme: &ThemePalette) -> Div {
    div()
        .flex_shrink_0()
        .px(theme.sp(9.0))
        .py(theme.sp(2.5))
        .rounded(px(RADIUS_PILL))
        .bg(rgba(theme.glass_trough_bg))
        .border_1()
        .border_color(rgba(theme.glass_border))
        .font(mono_font())
        .font_features(tabular_nums())
        .text_size(theme.text_micro())
        .text_color(rgb(theme.text_secondary))
        .child(label)
}
