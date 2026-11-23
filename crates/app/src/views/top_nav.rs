use gpui::{
    div, prelude::*, px, rgb, rgba, AnimationExt, Context, FontWeight, MouseButton, SharedString,
    Window, WindowControlArea,
};

use crate::{
    icons::{svg_icon, svg_icon_weighted, IconName, IconWeight},
    shell::{uses_native_windows_nc_chrome, AppShell},
    state::{FocusedInput, SidebarTab, ToastKind},
    theme::{
        active_theme, editorial_serif, glass_capsule_shadow, movietracker_mark, obsidian_fade_in,
        sp, ui_sans, RADIUS_CONTROL, RADIUS_PILL,
    },
    ui::{glass_dropdown_option, is_compact_viewport, keycap, topnav_gutter_px},
};

pub const TOPNAV_SCROLLED_THRESHOLD_PX: f32 = 24.0;

pub const PRIMARY_NAV_TABS: [SidebarTab; 4] = [
    SidebarTab::Home,
    SidebarTab::Discover,
    SidebarTab::Library,
    SidebarTab::Calendar,
];

#[inline]
pub fn is_topnav_scrolled(scroll_y: f32) -> bool {
    scroll_y.is_finite() && scroll_y >= TOPNAV_SCROLLED_THRESHOLD_PX
}

impl AppShell {
    pub fn render_floating_topnav(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = active_theme();
        let active_tab = self.route.active_sidebar_tab();
        let is_fullscreen = window.is_fullscreen();
        let csd_controls = Self::render_csd_window_controls(window, "topnav", false);
        let use_native_nc_chrome = uses_native_windows_nc_chrome(cfg!(target_os = "windows"));

        let scroll_y = self.current_route_scroll_y();
        let is_scrolled = is_topnav_scrolled(scroll_y);
        let viewport_w = self.viewport_size_px.0;
        let compact_width = is_compact_viewport(viewport_w);
        let nav_gutter_px = topnav_gutter_px(viewport_w);

        let (signed_in_email, avatar_initial) = self.core.with_model(|m| {
            let email = m.ctx.profile.auth.as_ref().map(|a| a.user.email.clone());
            let init = email
                .as_deref()
                .and_then(|e| e.chars().next())
                .map(|c| c.to_ascii_uppercase().to_string());
            (email, init)
        });

        let can_back = !self.nav_history.is_empty();
        let can_forward = !self.nav_forward.is_empty();
        let on_account_tab = active_tab == Some(SidebarTab::Account);
        let on_addons_tab = active_tab == Some(SidebarTab::Addons);
        let on_settings_tab = active_tab == Some(SidebarTab::Settings);
        let has_clipboard_addon = self.clipboard_addon_suggestion.is_some();

        let accent_ink = if theme.is_light {
            theme.accent_press
        } else {
            theme.accent
        };

        let query_empty = self.palette_query.is_empty();
        let search_text: SharedString = if self.command_palette_open {
            if query_empty {
                "Search...|".into()
            } else {
                self.format_input_with_caret(FocusedInput::CommandPalette, &self.palette_query)
                    .into()
            }
        } else if query_empty {
            "Search".into()
        } else {
            self.palette_query.clone().into()
        };

        // Left zone: Ferrite brand mark + Back/Forward history capsule.
        let left_inner = div()
            .flex()
            .items_center()
            .gap(sp(8.0))
            .child(
                div()
                    .id("topnav-brand")
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(sp(8.0))
                    .px(sp(4.0))
                    .py(sp(3.0))
                    .rounded(px(RADIUS_PILL))
                    .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.navigate_tab(SidebarTab::Home, cx);
                        }),
                    )
                    .child(movietracker_mark(24.0 * theme.ui_scale))
                    .child(
                        div()
                            .font(editorial_serif())
                            .text_size(sp(19.5))
                            .text_color(rgb(theme.text_primary))
                            .child("Ferrite"),
                    ),
            )
            .child(
                div()
                    .w(px(1.0))
                    .h(sp(16.0))
                    .flex_shrink_0()
                    .bg(rgba(theme.glass_divider)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.0))
                    .child(
                        div()
                            .id("topnav-nav-back")
                            .w(sp(26.0))
                            .h(sp(26.0))
                            .rounded(px(RADIUS_PILL))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(can_back, |el| {
                                el.cursor_pointer()
                                    .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.navigate_back(cx);
                                        }),
                                    )
                            })
                            .child(svg_icon(
                                IconName::ChevronLeft,
                                14.0 * theme.ui_scale,
                                if can_back {
                                    rgb(theme.text_primary)
                                } else {
                                    rgb(theme.text_muted)
                                },
                            )),
                    )
                    .child(
                        div()
                            .id("topnav-nav-forward")
                            .w(sp(26.0))
                            .h(sp(26.0))
                            .rounded(px(RADIUS_PILL))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(can_forward, |el| {
                                el.cursor_pointer()
                                    .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.navigate_forward(cx);
                                        }),
                                    )
                            })
                            .child(svg_icon(
                                IconName::ChevronRight,
                                14.0 * theme.ui_scale,
                                if can_forward {
                                    rgb(theme.text_primary)
                                } else {
                                    rgb(theme.text_muted)
                                },
                            )),
                    ),
            );

        let left_zone = div()
            .occlude()
            .flex_shrink_0()
            .h(sp(42.0))
            .px(sp(8.0))
            .rounded(px(RADIUS_PILL))
            .flex()
            .items_center()
            .when(!is_scrolled, |el| {
                el.bg(rgba(theme.bg_topnav_float))
                    .border_1()
                    .border_color(rgba(theme.glass_border))
                    .shadow(glass_capsule_shadow())
            })
            .child(left_inner);

        // Center zone: Primary 4-tab frosted capsule (Home, Discover, Library, Calendar).
        let center_zone = div()
            .occlude()
            .flex_shrink_0()
            .h(sp(42.0))
            .p(sp(4.0))
            .rounded(px(RADIUS_PILL))
            .bg(rgba(if is_scrolled {
                theme.glass_trough_bg
            } else {
                theme.bg_topnav_float
            }))
            .border_1()
            .border_color(rgba(theme.glass_border))
            .when(!is_scrolled, |el| el.shadow(glass_capsule_shadow()))
            .flex()
            .items_center()
            .gap(sp(2.0))
            .children(PRIMARY_NAV_TABS.into_iter().enumerate().map(|(idx, tab)| {
                let is_active = active_tab == Some(tab);
                let weight = if is_active {
                    IconWeight::Fill
                } else {
                    IconWeight::Regular
                };
                let icon_color = if is_active {
                    rgb(theme.segment_active_text)
                } else {
                    rgb(theme.text_secondary)
                };

                let base = div()
                    .id(("topnav-link", idx))
                    .h(sp(32.0))
                    .px(sp(if compact_width { 10.0 } else { 13.0 }))
                    .rounded(px(RADIUS_PILL))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(sp(6.0))
                    .whitespace_nowrap()
                    .text_size(sp(13.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.navigate_tab(tab, cx);
                        }),
                    );

                if is_active {
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
                .child(svg_icon_weighted(
                    tab.icon_name(),
                    weight,
                    14.5 * theme.ui_scale,
                    icon_color,
                ))
                .child(tab.label())
            }));

        // Right zone: Compact Search pill + 1-click Addons + 1-click Settings + Avatar + CSD controls.
        let search_pill = div()
            .id("topnav-search-box")
            .cursor_pointer()
            .h(sp(32.0))
            .min_w(sp(if compact_width { 98.0 } else { 140.0 }))
            .max_w(sp(230.0))
            .px(sp(10.0))
            .rounded(px(RADIUS_PILL))
            .bg(if self.command_palette_open {
                rgba(theme.glass_row_selected_bg)
            } else {
                rgba(theme.glass_trough_bg)
            })
            .border_1()
            .border_color(if self.command_palette_open {
                rgba(theme.border_accent)
            } else {
                rgba(theme.glass_border)
            })
            .hover(move |s| {
                s.border_color(if self.command_palette_open {
                    rgba(theme.border_accent)
                } else {
                    rgba(theme.glass_border_strong)
                })
                .bg(rgba(theme.glass_row_hover_bg))
            })
            .flex()
            .items_center()
            .justify_between()
            .gap(sp(8.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.open_command_palette(cx);
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .gap(sp(7.0))
                    .child(svg_icon_weighted(
                        IconName::Search,
                        if self.command_palette_open {
                            IconWeight::Fill
                        } else {
                            IconWeight::Regular
                        },
                        14.0 * theme.ui_scale,
                        if self.command_palette_open {
                            rgb(accent_ink)
                        } else {
                            rgb(theme.text_secondary)
                        },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(sp(12.5))
                            .text_color(if query_empty {
                                rgb(theme.text_secondary)
                            } else {
                                rgb(theme.text_primary)
                            })
                            .truncate()
                            .child(search_text),
                    ),
            )
            .when(!query_empty, |pill| {
                pill.child(
                    div()
                        .id("topnav-search-clear")
                        .cursor_pointer()
                        .p(sp(2.5))
                        .rounded(px(RADIUS_PILL))
                        .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.clear_palette_query(cx);
                            }),
                        )
                        .child(svg_icon(
                            IconName::X,
                            11.5 * theme.ui_scale,
                            rgb(theme.text_muted),
                        )),
                )
            })
            .when(query_empty && !compact_width, |pill| {
                pill.child(keycap("Ctrl K"))
            });

        let addons_btn = div()
            .id("topnav-addons-btn")
            .relative()
            .flex_shrink_0()
            .cursor_pointer()
            .w(sp(32.0))
            .h(sp(32.0))
            .rounded(px(RADIUS_PILL))
            .bg(if on_addons_tab {
                rgba(theme.accent_soft)
            } else {
                rgba(0x00000000)
            })
            .border_1()
            .border_color(if on_addons_tab {
                rgba(theme.border_accent)
            } else {
                rgba(0x00000000)
            })
            .hover(move |s| {
                s.bg(if on_addons_tab {
                    rgba(theme.accent_soft)
                } else {
                    rgba(theme.glass_row_hover_bg)
                })
                .border_color(rgba(theme.glass_border))
            })
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.navigate_tab(SidebarTab::Addons, cx);
                }),
            )
            .child(svg_icon_weighted(
                IconName::Layers,
                if on_addons_tab {
                    IconWeight::Fill
                } else {
                    IconWeight::Regular
                },
                15.0 * theme.ui_scale,
                if on_addons_tab {
                    rgb(accent_ink)
                } else {
                    rgb(theme.text_secondary)
                },
            ))
            .when(has_clipboard_addon, |btn| {
                btn.child(
                    div()
                        .absolute()
                        .top(sp(4.0))
                        .right(sp(4.0))
                        .w(sp(7.0))
                        .h(sp(7.0))
                        .rounded_full()
                        .bg(rgb(theme.accent))
                        .border_1()
                        .border_color(rgb(theme.bg_canvas)),
                )
            });

        let settings_btn = div()
            .id("topnav-settings-btn")
            .flex_shrink_0()
            .cursor_pointer()
            .w(sp(32.0))
            .h(sp(32.0))
            .rounded(px(RADIUS_PILL))
            .bg(if on_settings_tab {
                rgba(theme.accent_soft)
            } else {
                rgba(0x00000000)
            })
            .border_1()
            .border_color(if on_settings_tab {
                rgba(theme.border_accent)
            } else {
                rgba(0x00000000)
            })
            .hover(move |s| {
                s.bg(if on_settings_tab {
                    rgba(theme.accent_soft)
                } else {
                    rgba(theme.glass_row_hover_bg)
                })
                .border_color(rgba(theme.glass_border))
            })
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.navigate_tab(SidebarTab::Settings, cx);
                }),
            )
            .child(svg_icon_weighted(
                IconName::Settings,
                if on_settings_tab {
                    IconWeight::Fill
                } else {
                    IconWeight::Regular
                },
                15.0 * theme.ui_scale,
                if on_settings_tab {
                    rgb(accent_ink)
                } else {
                    rgb(theme.text_secondary)
                },
            ));

        let avatar_btn = div()
            .id("topnav-avatar-btn")
            .flex_shrink_0()
            .cursor_pointer()
            .w(sp(32.0))
            .h(sp(32.0))
            .rounded(px(RADIUS_PILL))
            .bg(if signed_in_email.is_some() {
                rgb(theme.accent)
            } else if on_account_tab || self.avatar_menu_open {
                rgba(theme.accent_soft)
            } else {
                rgba(theme.glass_trough_bg)
            })
            .border_1()
            .border_color(if on_account_tab || self.avatar_menu_open {
                rgb(theme.accent)
            } else {
                rgba(theme.glass_border)
            })
            .hover(move |s| s.border_color(rgba(theme.glass_border_strong)))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.command_palette_open = false;
                    this.context_menu = None;
                    this.avatar_menu_open = !this.avatar_menu_open;
                    cx.notify();
                }),
            )
            .child(if let Some(ch) = avatar_initial {
                div()
                    .font(ui_sans(FontWeight::BOLD))
                    .text_size(sp(12.5))
                    .text_color(rgb(theme.cta_on_accent))
                    .child(ch)
                    .into_any_element()
            } else {
                svg_icon_weighted(
                    IconName::User,
                    if on_account_tab || self.avatar_menu_open {
                        IconWeight::Fill
                    } else {
                        IconWeight::Regular
                    },
                    14.5 * theme.ui_scale,
                    if on_account_tab || self.avatar_menu_open {
                        rgb(accent_ink)
                    } else {
                        rgb(theme.text_secondary)
                    },
                )
                .into_any_element()
            });

        let right_zone = div()
            .occlude()
            .flex_shrink_0()
            .h(sp(42.0))
            .px(sp(6.0))
            .rounded(px(RADIUS_PILL))
            .flex()
            .items_center()
            .gap(sp(6.0))
            .when(!is_scrolled, |el| {
                el.bg(rgba(theme.bg_topnav_float))
                    .border_1()
                    .border_color(rgba(theme.glass_border))
                    .shadow(glass_capsule_shadow())
            })
            .child(search_pill)
            .when_some(
                self.update_status
                    .available_version()
                    .map(|v| (v.to_owned(), self.update_status.is_ready_to_restart())),
                |row, (ver, ready_restart)| {
                    row.child(
                        div()
                            .id("topnav-update-badge")
                            .flex_shrink_0()
                            .cursor_pointer()
                            .h(sp(28.0))
                            .px(sp(10.0))
                            .rounded(px(RADIUS_PILL))
                            .bg(rgba(theme.accent_soft))
                            .border_1()
                            .border_color(rgb(theme.accent))
                            .flex()
                            .items_center()
                            .gap(sp(5.0))
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_size(sp(11.5))
                            .text_color(rgb(accent_ink))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if ready_restart {
                                        this.restart_into_updated_binary(cx);
                                    } else {
                                        this.navigate_tab(SidebarTab::Settings, cx);
                                    }
                                }),
                            )
                            .child(svg_icon(
                                IconName::RefreshCw,
                                11.5 * theme.ui_scale,
                                rgb(accent_ink),
                            ))
                            .child(if ready_restart {
                                format!("Restart v{ver}")
                            } else {
                                format!("v{ver}")
                            }),
                    )
                },
            )
            .child(addons_btn)
            .child(settings_btn)
            .child(avatar_btn)
            .when(is_fullscreen, |row| {
                row.child(
                    div()
                        .id("topnav-btn-exit-fullscreen")
                        .flex_shrink_0()
                        .cursor_pointer()
                        .h(sp(30.0))
                        .px(sp(10.0))
                        .rounded(px(RADIUS_PILL))
                        .bg(rgba(theme.glass_trough_bg))
                        .border_1()
                        .border_color(rgba(theme.glass_border))
                        .hover(move |s| s.border_color(rgba(theme.glass_border_strong)))
                        .flex()
                        .items_center()
                        .gap(sp(5.0))
                        .font(ui_sans(FontWeight::MEDIUM))
                        .text_size(sp(12.0))
                        .text_color(rgb(theme.text_primary))
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            cx.stop_propagation();
                            window.toggle_fullscreen();
                        })
                        .child(svg_icon(
                            IconName::Minimize,
                            12.5 * theme.ui_scale,
                            rgb(accent_ink),
                        ))
                        .child("Exit fullscreen"),
                )
            })
            .when_some(csd_controls, |row, controls_el| {
                row.child(
                    div()
                        .w(px(1.0))
                        .h(sp(16.0))
                        .flex_shrink_0()
                        .bg(rgba(theme.glass_divider)),
                )
                .child(controls_el)
            });

        div()
            .id("topnav-titlebar")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .when(!is_fullscreen, |el| {
                el.window_control_area(WindowControlArea::Drag)
            })
            .h(if is_scrolled { sp(60.0) } else { sp(68.0) })
            .px(px(nav_gutter_px))
            .when(!is_scrolled, |el| el.pt(sp(8.0)))
            .when(is_scrolled, |el| {
                el.bg(rgba(theme.bg_topnav_scrolled))
                    .border_b_1()
                    .border_color(rgba(theme.glass_divider))
                    .shadow(glass_capsule_shadow())
            })
            .flex()
            .items_center()
            .justify_between()
            .gap(sp(10.0))
            .when(!use_native_nc_chrome, |el| {
                el.on_mouse_down(MouseButton::Left, |event, window, _cx| {
                    if event.click_count >= 2 {
                        window.zoom_window();
                    } else {
                        window.start_window_move();
                    }
                })
                .on_mouse_down(MouseButton::Right, |event, window, cx| {
                    cx.stop_propagation();
                    window.show_window_menu(event.position);
                })
            })
            .child(left_zone)
            .child(center_zone)
            .child(right_zone)
    }

    pub(crate) fn render_avatar_menu(
        &self,
        is_csd: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = active_theme();
        let email_opt = self
            .core
            .with_model(|m| m.ctx.profile.auth.as_ref().map(|a| a.user.email.clone()));
        let active_tab = self.route.active_sidebar_tab();

        let nav_gutter_px = topnav_gutter_px(self.viewport_size_px.0);

        div()
            .id("avatar-menu-backdrop")
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.avatar_menu_open = false;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("avatar-menu-popover")
                    .occlude()
                    .absolute()
                    .top(sp(62.0))
                    .right(if is_csd {
                        px(nav_gutter_px + 108.0 * theme.ui_scale)
                    } else {
                        px(nav_gutter_px)
                    })
                    .w(sp(248.0))
                    .p(sp(6.0))
                    .rounded(px(14.0))
                    .bg(rgba(theme.glass_popover_bg))
                    .border_1()
                    .border_color(rgba(theme.glass_border))
                    .shadow(glass_capsule_shadow())
                    .flex()
                    .flex_col()
                    .gap(sp(2.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px(sp(10.0))
                            .py(sp(8.0))
                            .border_b_1()
                            .border_color(rgba(theme.glass_divider))
                            .mb(sp(4.0))
                            .flex()
                            .flex_col()
                            .gap(sp(2.0))
                            .child(
                                div()
                                    .text_size(sp(13.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme.text_primary))
                                    .truncate()
                                    .child(if email_opt.is_some() {
                                        "Stremio Account"
                                    } else {
                                        "Guest Session"
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .truncate()
                                    .child(
                                        email_opt
                                            .clone()
                                            .unwrap_or_else(|| "Sign in to sync library".to_owned()),
                                    ),
                            ),
                    )
                    .child(glass_dropdown_option(
                        "avatar-menu-account",
                        if email_opt.is_some() {
                            "Account & sync"
                        } else {
                            "Sign in"
                        },
                        None,
                        active_tab == Some(SidebarTab::Account),
                        cx.listener(|this, _, _, cx| {
                            this.navigate_tab(SidebarTab::Account, cx);
                        }),
                    ))
                    .child(glass_dropdown_option(
                        "avatar-menu-addons",
                        "Addons",
                        None,
                        active_tab == Some(SidebarTab::Addons),
                        cx.listener(|this, _, _, cx| {
                            this.navigate_tab(SidebarTab::Addons, cx);
                        }),
                    ))
                    .child(glass_dropdown_option(
                        "avatar-menu-settings",
                        "Settings",
                        None,
                        active_tab == Some(SidebarTab::Settings),
                        cx.listener(|this, _, _, cx| {
                            this.navigate_tab(SidebarTab::Settings, cx);
                        }),
                    ))
                    .when(email_opt.is_some(), |el| {
                        el.child(
                            div()
                                .id("avatar-menu-logout")
                                .cursor_pointer()
                                .mt(sp(4.0))
                                .px(sp(12.0))
                                .py(sp(8.0))
                                .rounded(px(RADIUS_CONTROL))
                                .hover(|s| s.bg(rgba(0xf871711a)))
                                .flex()
                                .items_center()
                                .gap(sp(10.0))
                                .text_size(sp(13.0))
                                .font(ui_sans(FontWeight::MEDIUM))
                                .text_color(rgb(theme.accent_rose))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.avatar_menu_open = false;
                                        this.core.logout();
                                        this.push_toast(
                                            ToastKind::Info,
                                            "Signed out of Stremio",
                                            None,
                                            cx,
                                        );
                                    }),
                                )
                                .child(svg_icon(
                                    IconName::X,
                                    14.0 * theme.ui_scale,
                                    rgb(theme.accent_rose),
                                ))
                                .child("Sign out"),
                        )
                    })
                    .with_animation("avatar-menu-fade", obsidian_fade_in(), |el, delta| {
                        el.opacity(delta)
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topnav_scroll_transition_threshold_and_primary_tabs_order() {
        assert!(!is_topnav_scrolled(0.0));
        assert!(!is_topnav_scrolled(23.9));
        assert!(!is_topnav_scrolled(f32::NAN));
        assert!(is_topnav_scrolled(24.0));
        assert!(is_topnav_scrolled(140.0));

        assert_eq!(
            PRIMARY_NAV_TABS,
            [
                SidebarTab::Home,
                SidebarTab::Discover,
                SidebarTab::Library,
                SidebarTab::Calendar,
            ]
        );
    }
}
