use gpui::{
    div, img, prelude::*, px, rgb, rgba, ClipboardItem, Context, FontWeight, MouseButton,
    SharedString,
};
use stremio_core_runtime::stremio_core::models::common::Loadable;

use crate::{
    icons::{svg_icon_weighted, IconName, IconWeight},
    shell::AppShell,
    state::{FocusedInput, SidebarTab, ToastKind},
    theme::{
        active_theme, editorial_serif, mono_font, semantic_status_pill, sp, tabular_nums, ui_sans,
        ThemePalette, RADIUS_CARD, RADIUS_CONTROL, RADIUS_PILL,
    },
    ui_components::{primary_accent_button, secondary_glass_button},
};

impl AppShell {
    pub fn render_auth_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = active_theme();
        let (auth_user, addon_count, library_count, has_trakt, link_code_state, link_data_state) =
            self.core.with_model(|m| {
                let user = m
                    .ctx
                    .profile
                    .auth
                    .as_ref()
                    .map(|a| (a.user.email.clone(), a.user.id.0.clone()));
                let addons = m.ctx.profile.addons.len();
                let lib_items = m
                    .ctx
                    .library
                    .items
                    .values()
                    .filter(|item| !item.removed && !item.temp)
                    .count();
                let trakt = m
                    .ctx
                    .profile
                    .auth
                    .as_ref()
                    .and_then(|a| a.user.trakt.as_ref())
                    .is_some();
                (
                    user,
                    addons,
                    lib_items,
                    trakt,
                    m.auth_link.code.clone(),
                    m.auth_link.data.clone(),
                )
            });

        let side_pad = if self.viewport_size_px.0.is_finite() && self.viewport_size_px.0 > 0.0 {
            (self.viewport_size_px.0 * 0.03).clamp(24.0, 56.0)
        } else {
            32.0
        };
        let pill_border_alpha = if t.is_light { 0x59 } else { 0x4d };
        let pill_bg_alpha = if t.is_light { 0x1f } else { 0x1c };

        div()
            .id("auth-view-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .max_w(px(1120.0))
                    .mx_auto()
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
                                            .child("Account"),
                                    )
                                    .child(
                                        div()
                                            .text_size(sp(13.5))
                                            .text_color(rgb(t.text_secondary))
                                            .child(
                                                "Sync your Stremio library, watch progress, and installed addons across devices.",
                                            ),
                                    ),
                            )
                            .child(if auth_user.is_some() {
                                semantic_status_pill(
                                    IconName::Check,
                                    "Cloud sync active",
                                    t.status_watched,
                                    (t.status_watched << 8) | pill_border_alpha,
                                    (t.status_watched << 8) | pill_bg_alpha,
                                )
                            } else {
                                semantic_status_pill(
                                    IconName::User,
                                    "Guest profile",
                                    t.status_watching,
                                    (t.status_watching << 8) | pill_border_alpha,
                                    (t.status_watching << 8) | pill_bg_alpha,
                                )
                            }),
                    )
                    .child(
                        div()
                            .grid()
                            .grid_cols(3)
                            .gap(sp(14.0))
                            .child(render_auth_stat_card(
                                IconName::Layers,
                                IconWeight::Regular,
                                "Installed addons",
                                format!("{addon_count}"),
                                "Synced via Stremio API & SQLite WAL",
                                t,
                            ))
                            .child(render_auth_stat_card(
                                IconName::Bookmark,
                                IconWeight::Fill,
                                "Library titles",
                                format!("{library_count}"),
                                "Watched bitfields & resume offsets persisted",
                                t,
                            ))
                            .child(render_auth_stat_card(
                                IconName::Sparkles,
                                if has_trakt {
                                    IconWeight::Fill
                                } else {
                                    IconWeight::Regular
                                },
                                "Trakt scrobbling",
                                if has_trakt {
                                    "Connected".to_owned()
                                } else {
                                    "Not linked".to_owned()
                                },
                                "Automatic play & completion scrobbling",
                                t,
                            )),
                    )
                    .child(if let Some((email, user_id)) = auth_user {
                        Self::render_authenticated_panel(email, user_id, t, cx).into_any_element()
                    } else {
                        self.render_unauthenticated_panels(link_code_state, link_data_state, t, cx)
                            .into_any_element()
                    }),
            )
    }

    fn render_authenticated_panel(
        email: String,
        user_id: String,
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let initial = email
            .chars()
            .next()
            .unwrap_or('S')
            .to_ascii_uppercase()
            .to_string();
        let rose_bg = (t.accent_rose << 8) | if t.is_light { 0x1a } else { 0x1c };
        let rose_hover = (t.accent_rose << 8) | if t.is_light { 0x2b } else { 0x30 };
        let rose_border = (t.accent_rose << 8) | 0x59;

        div()
            .p(sp(24.0))
            .rounded(px(RADIUS_CARD))
            .bg(rgba(t.glass_card_bg))
            .border_1()
            .border_color(rgba(t.glass_border))
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(sp(20.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(16.0))
                    .child(
                        div()
                            .w(sp(52.0))
                            .h(sp(52.0))
                            .rounded(px(RADIUS_PILL))
                            .bg(rgb(t.accent))
                            .flex()
                            .items_center()
                            .justify_center()
                            .font(ui_sans(FontWeight::BOLD))
                            .text_size(sp(20.0))
                            .text_color(rgb(t.cta_on_accent))
                            .child(initial),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(sp(4.0))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(sp(17.0))
                                    .text_color(rgb(t.text_primary))
                                    .child(email),
                            )
                            .child(
                                div()
                                    .font(mono_font())
                                    .text_xs()
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("UID {user_id}")),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(10.0))
                    .child(secondary_glass_button(
                        "btn-auth-manage-addons",
                        Some(IconName::Layers),
                        "Manage addons",
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.navigate_tab(SidebarTab::Addons, cx);
                        }),
                    ))
                    .child(
                        div()
                            .id("btn-auth-logout")
                            .cursor_pointer()
                            .h(sp(40.0))
                            .px(sp(18.0))
                            .rounded(px(RADIUS_CONTROL))
                            .bg(rgba(rose_bg))
                            .border_1()
                            .border_color(rgba(rose_border))
                            .hover(move |s| s.bg(rgba(rose_hover)))
                            .flex()
                            .items_center()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(sp(13.5))
                            .text_color(rgb(t.accent_rose))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.core.logout();
                                    this.link_poll_active = false;
                                    this.push_toast(ToastKind::Info, "Signed out", None, cx);
                                }),
                            )
                            .child("Sign out"),
                    ),
            )
    }

    fn render_unauthenticated_panels(
        &self,
        link_code_state: Option<
            Loadable<
                stremio_core_runtime::stremio_core::types::api::LinkCodeResponse,
                stremio_core_runtime::stremio_core::models::link::LinkError,
            >,
        >,
        link_data_state: Option<
            Loadable<
                stremio_core_runtime::stremio_core::types::api::LinkAuthKey,
                stremio_core_runtime::stremio_core::models::link::LinkError,
            >,
        >,
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let email_focused = self.focused_input == Some(FocusedInput::AuthEmail);
        let pass_focused = self.focused_input == Some(FocusedInput::AuthPassword);

        let email_display: SharedString = if self.auth_email_input.is_empty() {
            if email_focused {
                "|".into()
            } else {
                "you@example.com".into()
            }
        } else if email_focused {
            format!("{}|", self.auth_email_input).into()
        } else {
            self.auth_email_input.clone().into()
        };

        let masked_pass = "*".repeat(self.auth_password_input.chars().count());
        let pass_display: SharedString = if self.auth_password_input.is_empty() {
            if pass_focused {
                "|".into()
            } else {
                "************".into()
            }
        } else if pass_focused {
            format!("{masked_pass}|").into()
        } else {
            masked_pass.into()
        };

        div()
            .grid()
            .grid_cols(2)
            .gap(sp(20.0))
            .child(
                div()
                    .p(sp(24.0))
                    .rounded(px(RADIUS_CARD))
                    .bg(rgba(t.glass_card_bg))
                    .border_1()
                    .border_color(rgba(t.glass_border))
                    .flex()
                    .flex_col()
                    .gap(sp(16.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(sp(4.0))
                            .child(
                                div()
                                    .font(editorial_serif())
                                    .text_size(sp(26.0))
                                    .text_color(rgb(t.text_primary))
                                    .child("Sign in with email"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(t.text_secondary))
                                    .child(
                                        "Direct authentication against api.strem.io (click a field to type or Ctrl V paste).",
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(sp(6.0))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(t.text_secondary))
                                    .child("Email address"),
                            )
                            .child(
                                div()
                                    .id("input-auth-email")
                                    .cursor_text()
                                    .h(sp(40.0))
                                    .px(sp(12.0))
                                    .rounded(px(RADIUS_CONTROL))
                                    .bg(rgba(t.glass_trough_bg))
                                    .border_1()
                                    .border_color(if email_focused {
                                        rgba(t.border_poster_hover)
                                    } else {
                                        rgba(t.glass_border)
                                    })
                                    .flex()
                                    .items_center()
                                    .text_size(sp(13.5))
                                    .text_color(
                                        if self.auth_email_input.is_empty() && !email_focused {
                                            rgb(t.text_muted)
                                        } else {
                                            rgb(t.text_primary)
                                        },
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.focused_input = Some(FocusedInput::AuthEmail);
                                            cx.notify();
                                        }),
                                    )
                                    .child(email_display),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(sp(6.0))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(t.text_secondary))
                                    .child("Password"),
                            )
                            .child(
                                div()
                                    .id("input-auth-password")
                                    .cursor_text()
                                    .h(sp(40.0))
                                    .px(sp(12.0))
                                    .rounded(px(RADIUS_CONTROL))
                                    .bg(rgba(t.glass_trough_bg))
                                    .border_1()
                                    .border_color(if pass_focused {
                                        rgba(t.border_poster_hover)
                                    } else {
                                        rgba(t.glass_border)
                                    })
                                    .flex()
                                    .items_center()
                                    .font(mono_font())
                                    .text_size(sp(13.5))
                                    .text_color(
                                        if self.auth_password_input.is_empty() && !pass_focused {
                                            rgb(t.text_muted)
                                        } else {
                                            rgb(t.text_primary)
                                        },
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.focused_input = Some(FocusedInput::AuthPassword);
                                            cx.notify();
                                        }),
                                    )
                                    .child(pass_display),
                            ),
                    )
                    .child(
                        primary_accent_button(
                            "btn-submit-password-login",
                            Some(IconName::User),
                            "Sign in & sync library",
                            cx.listener(|this, _, _, cx| {
                                this.submit_password_login(cx);
                            }),
                        )
                        .mt(sp(4.0))
                        .w_full(),
                    ),
            )
            .child(self.render_link_auth_card(link_code_state, link_data_state, t, cx))
    }

    fn render_link_auth_card(
        &self,
        link_code_state: Option<
            Loadable<
                stremio_core_runtime::stremio_core::types::api::LinkCodeResponse,
                stremio_core_runtime::stremio_core::models::link::LinkError,
            >,
        >,
        link_data_state: Option<
            Loadable<
                stremio_core_runtime::stremio_core::types::api::LinkAuthKey,
                stremio_core_runtime::stremio_core::models::link::LinkError,
            >,
        >,
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let accent_ink = if t.is_light { t.accent_press } else { t.accent };
        let rose_bg = (t.accent_rose << 8) | if t.is_light { 0x1a } else { 0x1c };

        div()
            .p(sp(24.0))
            .rounded(px(RADIUS_CARD))
            .bg(rgba(t.glass_card_bg))
            .border_1()
            .border_color(rgba(t.glass_border))
            .flex()
            .flex_col()
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
                            .text_size(sp(26.0))
                            .text_color(rgb(t.text_primary))
                            .child("QR / link code sign-in"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(t.text_secondary))
                            .child(
                                "Approve login from your phone or browser at stremio.com/link.",
                            ),
                    ),
            )
            .child(match link_code_state {
                None => div()
                    .py(sp(28.0))
                    .flex()
                    .justify_center()
                    .text_size(sp(13.5))
                    .text_color(rgb(t.text_secondary))
                    .child("Generate a one-time 4-character code to link your account.")
                    .into_any_element(),
                Some(Loadable::Loading) => div()
                    .py(sp(28.0))
                    .flex()
                    .justify_center()
                    .font(mono_font())
                    .text_xs()
                    .text_color(rgb(accent_ink))
                    .child("Requesting link code from link.stremio.com...")
                    .into_any_element(),
                Some(Loadable::Err(err)) => div()
                    .p(sp(12.0))
                    .rounded(px(RADIUS_CONTROL))
                    .bg(rgba(rose_bg))
                    .text_xs()
                    .text_color(rgb(t.accent_rose))
                    .child(format!("Link code request failed: {err}"))
                    .into_any_element(),
                Some(Loadable::Ready(resp)) => {
                    let qr_tex = self.poster_cache.resolve_poster_image(&resp.qrcode);
                    let link_url = resp.link.clone();
                    let code_str = resp.code.clone();
                    let poll_status = match link_data_state {
                        Some(Loadable::Loading) => "Waiting for approval on stremio.com/link...",
                        Some(Loadable::Ready(_)) => "Approved, completing sign-in...",
                        Some(Loadable::Err(_)) => "Polling stremio.com/link for confirmation...",
                        None => "Enter code at stremio.com/link",
                    };

                    div()
                        .p(sp(16.0))
                        .rounded(px(RADIUS_CONTROL))
                        .bg(rgba(t.glass_trough_bg))
                        .border_1()
                        .border_color(rgba(t.border_accent))
                        .flex()
                        .items_center()
                        .gap(sp(16.0))
                        .when_some(qr_tex, |el, tex| {
                            el.child(
                                div()
                                    .w(sp(96.0))
                                    .h(sp(96.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(0xFFFFFF))
                                    .border_1()
                                    .border_color(rgba(t.glass_border))
                                    .p(sp(4.0))
                                    .child(img(tex).size_full().object_fit(gpui::ObjectFit::Contain)),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(sp(6.0))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(t.text_secondary))
                                        .child("Link code"),
                                )
                                .child(
                                    div()
                                        .font(mono_font())
                                        .font_weight(FontWeight::BOLD)
                                        .text_size(sp(26.0))
                                        .text_color(rgb(accent_ink))
                                        .child(code_str),
                                )
                                .child(
                                    div()
                                        .id("btn-copy-link-url")
                                        .cursor_pointer()
                                        .flex()
                                        .items_center()
                                        .gap(sp(5.0))
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(t.text_primary))
                                        .hover(move |s| s.text_color(rgb(accent_ink)))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    link_url.clone(),
                                                ));
                                                this.push_toast(
                                                    ToastKind::Success,
                                                    "Copied link URL",
                                                    None,
                                                    cx,
                                                );
                                            }),
                                        )
                                        .child(svg_icon_weighted(
                                            IconName::Clipboard,
                                            IconWeight::Regular,
                                            11.5 * t.ui_scale,
                                            rgb(t.text_muted),
                                        ))
                                        .child(format!("{} (copy)", resp.link)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(t.text_muted))
                                        .child(poll_status),
                                ),
                        )
                        .into_any_element()
                }
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(10.0))
                    .child(
                        secondary_glass_button(
                            "btn-start-link-auth",
                            Some(IconName::RefreshCw),
                            if self.link_poll_active {
                                "Refresh link code"
                            } else {
                                "Generate QR / link code"
                            },
                            false,
                            cx.listener(|this, _, _, cx| {
                                this.begin_link_auth(cx);
                            }),
                        )
                        .flex_1(),
                    )
                    .when(self.link_poll_active, |el| {
                        el.child(secondary_glass_button(
                            "btn-poll-now",
                            None,
                            "Check now",
                            false,
                            cx.listener(|this, _, _, _cx| {
                                this.core.poll_link_auth();
                            }),
                        ))
                    }),
            )
    }
}

fn render_auth_stat_card(
    icon: IconName,
    icon_weight: IconWeight,
    label: &'static str,
    value: String,
    caption: &'static str,
    t: ThemePalette,
) -> impl IntoElement {
    let accent_ink = if t.is_light { t.accent_press } else { t.accent };
    div()
        .p(sp(18.0))
        .rounded(px(RADIUS_CARD))
        .bg(rgba(t.glass_card_bg))
        .border_1()
        .border_color(rgba(t.glass_border))
        .flex()
        .flex_col()
        .gap(sp(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(6.0))
                .child(svg_icon_weighted(
                    icon,
                    icon_weight,
                    13.0 * t.ui_scale,
                    rgb(accent_ink),
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(t.text_secondary))
                        .child(label),
                ),
        )
        .child(
            div()
                .font(editorial_serif())
                .font_features(tabular_nums())
                .text_size(sp(34.0))
                .line_height(gpui::relative(1.05))
                .text_color(rgb(t.text_primary))
                .child(value),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(t.text_muted))
                .child(caption),
        )
}
