use gpui::{
    div, img, prelude::*, px, rgb, rgba, ClipboardItem, Context, FontWeight, MouseButton,
    SharedString,
};
use stremio_core_runtime::stremio_core::models::common::Loadable;

use crate::{
    icons::IconName,
    shell::AppShell,
    state::{FocusedInput, SidebarTab, ToastKind},
    theme::{
        editorial_serif, mono_font, semantic_status_pill, ui_sans, ACCENT, ACCENT_HOVER,
        ACCENT_ROSE, BG_ELEVATED, BG_INPUT, BG_OBSIDIAN, BG_SURFACE, BORDER_POSTER_HOVER,
        BORDER_STRONG, BORDER_SUBTLE, STATUS_WATCHED, STATUS_WATCHING, TEXT_MUTED, TEXT_PRIMARY,
        TEXT_SECONDARY,
    },
};

impl AppShell {
    pub fn render_auth_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

        div()
            .id("auth-view-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .max_w(px(1080.0))
                    .mx_auto()
                    .px(px(32.0))
                    .pt(px(32.0))
                    .pb(px(72.0))
                    .flex()
                    .flex_col()
                    .gap(px(28.0))
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .justify_between()
                            .gap(px(20.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .font(editorial_serif())
                                            .text_size(px(56.0))
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child("Account"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(14.5))
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(
                                                "Sync your Stremio library, watch progress, and installed addons across devices.",
                                            ),
                                    ),
                            )
                            .child(if auth_user.is_some() {
                                semantic_status_pill(
                                    IconName::Check,
                                    "Cloud sync active",
                                    STATUS_WATCHED,
                                    0x4ade804d,
                                    0x4ade801c,
                                )
                            } else {
                                semantic_status_pill(
                                    IconName::User,
                                    "Guest profile",
                                    STATUS_WATCHING,
                                    0x60a5fa4d,
                                    0x60a5fa1c,
                                )
                            }),
                    )
                    .child(
                        div()
                            .grid()
                            .grid_cols(3)
                            .gap(px(16.0))
                            .child(render_auth_stat_card(
                                "Installed addons",
                                format!("{addon_count}"),
                                "Synced via Stremio API & SQLite WAL",
                            ))
                            .child(render_auth_stat_card(
                                "Library titles",
                                format!("{library_count}"),
                                "Watched bitfields & resume offsets persisted",
                            ))
                            .child(render_auth_stat_card(
                                "Trakt scrobbling",
                                if has_trakt {
                                    "Connected".to_owned()
                                } else {
                                    "Not linked".to_owned()
                                },
                                "Automatic play & completion scrobbling",
                            )),
                    )
                    .child(if let Some((email, user_id)) = auth_user {
                        Self::render_authenticated_panel(email, user_id, cx)
                            .into_any_element()
                    } else {
                        self.render_unauthenticated_panels(link_code_state, link_data_state, cx)
                            .into_any_element()
                    }),
            )
    }

    fn render_authenticated_panel(
        email: String,
        user_id: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let initial = email
            .chars()
            .next()
            .unwrap_or('S')
            .to_ascii_uppercase()
            .to_string();

        div()
            .p(px(24.0))
            .rounded(px(10.0))
            .bg(rgb(BG_SURFACE))
            .border_1()
            .border_color(rgba(BORDER_SUBTLE))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(20.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .child(
                        div()
                            .w(px(52.0))
                            .h(px(52.0))
                            .rounded(px(999.0))
                            .bg(rgb(ACCENT))
                            .flex()
                            .items_center()
                            .justify_center()
                            .font(ui_sans(FontWeight::BOLD))
                            .text_size(px(20.0))
                            .text_color(rgb(BG_OBSIDIAN))
                            .child(initial),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(px(17.0))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .child(email),
                            )
                            .child(
                                div()
                                    .font(mono_font())
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(format!("UID {user_id}")),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .id("btn-auth-manage-addons")
                            .cursor_pointer()
                            .h(px(40.0))
                            .px(px(18.0))
                            .rounded(px(6.0))
                            .bg(rgb(BG_ELEVATED))
                            .border_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .hover(|s| s.border_color(rgba(BORDER_STRONG)))
                            .flex()
                            .items_center()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(px(13.5))
                            .text_color(rgb(TEXT_PRIMARY))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.navigate_tab(SidebarTab::Addons, cx);
                                }),
                            )
                            .child("Manage addons"),
                    )
                    .child(
                        div()
                            .id("btn-auth-logout")
                            .cursor_pointer()
                            .h(px(40.0))
                            .px(px(18.0))
                            .rounded(px(6.0))
                            .bg(rgba(0xf871711a))
                            .border_1()
                            .border_color(rgba(0xf8717155))
                            .flex()
                            .items_center()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(px(13.5))
                            .text_color(rgb(ACCENT_ROSE))
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
            .gap(px(20.0))
            .child(
                div()
                    .p(px(24.0))
                    .rounded(px(10.0))
                    .bg(rgb(BG_SURFACE))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .font(editorial_serif())
                                    .text_size(px(28.0))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .child("Sign in with email"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(
                                        "Direct authentication against api.strem.io (click a field to type or Ctrl V paste).",
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child("Email address"),
                            )
                            .child(
                                div()
                                    .id("input-auth-email")
                                    .cursor_text()
                                    .h(px(40.0))
                                    .px(px(12.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(BG_INPUT))
                                    .border_1()
                                    .border_color(if email_focused {
                                        rgba(BORDER_POSTER_HOVER)
                                    } else {
                                        rgba(BORDER_SUBTLE)
                                    })
                                    .flex()
                                    .items_center()
                                    .text_size(px(13.5))
                                    .text_color(
                                        if self.auth_email_input.is_empty() && !email_focused {
                                            rgb(TEXT_MUTED)
                                        } else {
                                            rgb(TEXT_PRIMARY)
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
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child("Password"),
                            )
                            .child(
                                div()
                                    .id("input-auth-password")
                                    .cursor_text()
                                    .h(px(40.0))
                                    .px(px(12.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(BG_INPUT))
                                    .border_1()
                                    .border_color(if pass_focused {
                                        rgba(BORDER_POSTER_HOVER)
                                    } else {
                                        rgba(BORDER_SUBTLE)
                                    })
                                    .flex()
                                    .items_center()
                                    .font(mono_font())
                                    .text_size(px(13.5))
                                    .text_color(
                                        if self.auth_password_input.is_empty() && !pass_focused {
                                            rgb(TEXT_MUTED)
                                        } else {
                                            rgb(TEXT_PRIMARY)
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
                        div()
                            .id("btn-submit-password-login")
                            .cursor_pointer()
                            .mt(px(4.0))
                            .h(px(42.0))
                            .rounded(px(6.0))
                            .bg(rgb(ACCENT))
                            .hover(|s| s.bg(rgb(ACCENT_HOVER)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_size(px(14.0))
                            .text_color(rgb(BG_OBSIDIAN))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.submit_password_login(cx);
                                }),
                            )
                            .child("Sign in & sync library"),
                    ),
            )
            .child(self.render_link_auth_card(link_code_state, link_data_state, cx))
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
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .p(px(24.0))
            .rounded(px(10.0))
            .bg(rgb(BG_SURFACE))
            .border_1()
            .border_color(rgba(BORDER_SUBTLE))
            .flex()
            .flex_col()
            .justify_between()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .font(editorial_serif())
                            .text_size(px(28.0))
                            .text_color(rgb(TEXT_PRIMARY))
                            .child("QR / link code sign-in"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(
                                "Approve login from your phone or browser at stremio.com/link.",
                            ),
                    ),
            )
            .child(match link_code_state {
                None => div()
                    .py(px(28.0))
                    .flex()
                    .justify_center()
                    .text_size(px(13.5))
                    .text_color(rgb(TEXT_SECONDARY))
                    .child("Generate a one-time 4-character code to link your account.")
                    .into_any_element(),
                Some(Loadable::Loading) => div()
                    .py(px(28.0))
                    .flex()
                    .justify_center()
                    .font(mono_font())
                    .text_xs()
                    .text_color(rgb(ACCENT))
                    .child("Requesting link code from link.stremio.com...")
                    .into_any_element(),
                Some(Loadable::Err(err)) => div()
                    .p(px(12.0))
                    .rounded(px(6.0))
                    .bg(rgba(0xf871711a))
                    .text_xs()
                    .text_color(rgb(ACCENT_ROSE))
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
                        .p(px(16.0))
                        .rounded(px(8.0))
                        .bg(rgb(BG_INPUT))
                        .border_1()
                        .border_color(rgb(ACCENT))
                        .flex()
                        .items_center()
                        .gap(px(16.0))
                        .when_some(qr_tex, |el, tex| {
                            el.child(
                                div()
                                    .w(px(96.0))
                                    .h(px(96.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(TEXT_PRIMARY))
                                    .p(px(4.0))
                                    .child(img(tex).size_full().object_fit(gpui::ObjectFit::Contain)),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(TEXT_SECONDARY))
                                        .child("Link code"),
                                )
                                .child(
                                    div()
                                        .font(mono_font())
                                        .font_weight(FontWeight::BOLD)
                                        .text_size(px(26.0))
                                        .text_color(rgb(ACCENT))
                                        .child(code_str),
                                )
                                .child(
                                    div()
                                        .id("btn-copy-link-url")
                                        .cursor_pointer()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_PRIMARY))
                                        .hover(|s| s.text_color(rgb(ACCENT)))
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
                                        .child(format!("{} (copy)", resp.link)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(TEXT_MUTED))
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
                    .gap(px(10.0))
                    .child(
                        div()
                            .id("btn-start-link-auth")
                            .cursor_pointer()
                            .flex_1()
                            .h(px(42.0))
                            .rounded(px(6.0))
                            .bg(rgb(BG_ELEVATED))
                            .border_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .hover(|s| s.border_color(rgb(ACCENT)).text_color(rgb(ACCENT)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_size(px(13.5))
                            .text_color(rgb(TEXT_PRIMARY))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.begin_link_auth(cx);
                                }),
                            )
                            .child(if self.link_poll_active {
                                "Refresh link code"
                            } else {
                                "Generate QR / link code"
                            }),
                    )
                    .when(self.link_poll_active, |el| {
                        el.child(
                            div()
                                .id("btn-poll-now")
                                .cursor_pointer()
                                .h(px(42.0))
                                .px(px(14.0))
                                .rounded(px(6.0))
                                .bg(rgb(BG_ELEVATED))
                                .border_1()
                                .border_color(rgba(BORDER_SUBTLE))
                                .flex()
                                .items_center()
                                .text_size(px(13.5))
                                .text_color(rgb(TEXT_PRIMARY))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, _cx| {
                                        this.core.poll_link_auth();
                                    }),
                                )
                                .child("Check now"),
                        )
                    }),
            )
    }
}

fn render_auth_stat_card(
    label: &'static str,
    value: String,
    caption: &'static str,
) -> impl IntoElement {
    div()
        .p(px(20.0))
        .rounded(px(10.0))
        .bg(rgb(BG_SURFACE))
        .border_1()
        .border_color(rgba(BORDER_SUBTLE))
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(div().text_xs().text_color(rgb(TEXT_SECONDARY)).child(label))
        .child(
            div()
                .font(editorial_serif())
                .text_size(px(36.0))
                .text_color(rgb(TEXT_PRIMARY))
                .child(value),
        )
        .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child(caption))
}
