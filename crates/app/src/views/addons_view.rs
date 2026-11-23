use std::collections::{HashMap, HashSet};

use gpui::{
    div, prelude::*, px, rgb, rgba, AsyncApp, ClipboardItem, Context, FontWeight, MouseButton,
    SharedString, WeakEntity,
};
use stremio_core_runtime::{
    addon_configure_url, addon_origin_from_url, classify_descriptor_categories,
    curated_addon_directory, format_bytes, normalize_addon_url,
    stremio_core::{models::common::Loadable, types::addon::Descriptor},
    AddonCategory, AddonHealthStatus, AddonTelemetrySnapshot, MutedAddonEntry,
};
use url::Url;

use crate::{
    icons::{svg_icon_weighted, IconName, IconWeight},
    shell::AppShell,
    state::{addon_matches_search_query, AddonCategoryFilter, AddonSubTab, FocusedInput, ToastKind},
    theme::{
        active_theme, editorial_serif, meta_divider, mono_font, neutral_chip, semantic_status_pill,
        sp, ui_sans, ThemePalette, RADIUS_CARD, RADIUS_CONTROL, RADIUS_PILL,
    },
    views::settings_view::{render_choice_pill, render_movietracker_switch},
};

#[derive(Clone)]
struct DirectoryAddonCard {
    id: String,
    name: String,
    tagline: Option<String>,
    description: Option<String>,
    categories: Vec<AddonCategory>,
    types: Vec<String>,
    manifest_url: String,
    configure_url: Option<String>,
    requires_browser_config: bool,
    ready_descriptor: Option<Descriptor>,
}

impl AppShell {
    pub fn render_addons_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = active_theme();
        let (active_addons, remote_descriptors, remote_loading) = self.core.with_model(|model| {
            let active = model.ctx.profile.addons.clone();
            let mut remote = Vec::new();
            let mut loading = false;
            for page in &model.remote_addons.catalog {
                match &page.content {
                    Some(Loadable::Ready(list)) => remote.extend(list.iter().cloned()),
                    Some(Loadable::Loading) => loading = true,
                    _ => {}
                }
            }
            (active, remote, loading)
        });

        let muted_addons = self.core.list_muted_addons();
        let telemetry_map = self.core.addon_telemetry();
        let cache_stats = self.core.addon_cache_stats();

        let total_active = active_addons.len();
        let total_installed_and_muted = total_active + muted_addons.len();

        let directory_cards = build_deduplicated_directory(&remote_descriptors);
        let total_directory = directory_cards.len();

        let side_pad = if self.viewport_size_px.0.is_finite() && self.viewport_size_px.0 > 0.0 {
            (self.viewport_size_px.0 * 0.03).clamp(24.0, 56.0)
        } else {
            32.0
        };

        div()
            .id("addons-studio-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .max_w(px(1180.0))
                    .mx_auto()
                    .px(px(side_pad))
                    .pt(sp(16.0))
                    .pb(sp(72.0))
                    .flex()
                    .flex_col()
                    .gap(sp(22.0))
                    .child(Self::render_addons_header(
                        total_active,
                        &cache_stats,
                        t,
                        cx,
                    ))
                    .when_some(self.clipboard_addon_suggestion.clone(), |el, clip_url| {
                        let is_update = is_clipboard_url_in_place_update(
                            &clip_url,
                            &active_addons,
                            &muted_addons,
                        );
                        el.child(self.render_clipboard_addon_bar(&clip_url, is_update, t, cx))
                    })
                    .child(self.render_universal_manifest_bar(t, cx))
                    .child(self.render_addons_toolbar(
                        total_installed_and_muted,
                        total_directory,
                        t,
                        cx,
                    ))
                    .child(match self.addon_sub_tab {
                        AddonSubTab::Installed => self
                            .render_installed_priority_deck(
                                &active_addons,
                                &muted_addons,
                                &telemetry_map,
                                t,
                                cx,
                            )
                            .into_any_element(),
                        AddonSubTab::Explore => self
                            .render_explore_directory_deck(
                                &directory_cards,
                                &active_addons,
                                &muted_addons,
                                remote_loading,
                                t,
                                cx,
                            )
                            .into_any_element(),
                    }),
            )
    }

    fn render_addons_header(
        active_count: usize,
        cache_stats: &stremio_core_runtime::AddonCacheStats,
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let accent_ink = if t.is_light { t.accent_press } else { t.accent };
        let has_cache = cache_stats.entries > 0 || cache_stats.hits > 0;

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
                            .child("Addons"),
                    )
                    .child(
                        div()
                            .text_size(sp(13.5))
                            .text_color(rgb(t.text_secondary))
                            .child(
                                "Priority execution order, soft-mute preserving secret config URLs, and live origin health.",
                            ),
                    ),
            )
            .child(
                div()
                    .h(sp(34.0))
                    .px(sp(14.0))
                    .rounded(px(RADIUS_PILL))
                    .bg(rgba(t.glass_card_bg))
                    .border_1()
                    .border_color(rgba(t.glass_border))
                    .flex()
                    .items_center()
                    .gap(sp(10.0))
                    .font(mono_font())
                    .text_xs()
                    .text_color(rgb(t.text_secondary))
                    .child(
                        div()
                            .text_color(rgb(t.text_primary))
                            .child(format!("{active_count} active")),
                    )
                    .child(meta_divider())
                    .child(format!("{} cache hits", cache_stats.hits))
                    .child(meta_divider())
                    .child(format_bytes(cache_stats.total_bytes as u64))
                    .when(has_cache, |el| {
                        el.child(meta_divider()).child(
                            div()
                                .id("btn-clear-addon-http-cache")
                                .cursor_pointer()
                                .text_color(rgb(accent_ink))
                                .hover(move |s| s.text_color(rgb(t.accent_hover)))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.core.clear_addon_cache();
                                        this.push_toast(
                                            ToastKind::Info,
                                            "Cleared addon HTTP cache",
                                            None,
                                            cx,
                                        );
                                    }),
                                )
                                .child("Clear cache"),
                        )
                    }),
            )
    }

    fn render_clipboard_addon_bar(
        &self,
        clip_url: &str,
        is_update: bool,
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let accent_ink = if t.is_light { t.accent_press } else { t.accent };
        let is_installing = self.installing_addon_urls.contains(clip_url);
        let install_url = clip_url.to_owned();
        let dismiss_url = clip_url.to_owned();
        let action_label = if is_installing {
            "Installing..."
        } else if is_update {
            "Update in place"
        } else {
            "Install"
        };

        div()
            .px(sp(16.0))
            .py(sp(11.0))
            .rounded(px(RADIUS_CARD))
            .bg(rgba(t.glass_card_bg))
            .border_1()
            .border_color(rgba(t.border_accent))
            .flex()
            .items_center()
            .justify_between()
            .gap(sp(14.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .gap(sp(10.0))
                    .child(svg_icon_weighted(
                        IconName::Clipboard,
                        IconWeight::Regular,
                        15.0 * t.ui_scale,
                        rgb(accent_ink),
                    ))
                    .child(
                        div()
                            .flex_shrink_0()
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_size(sp(13.0))
                            .text_color(rgb(t.text_primary))
                            .child(if is_update {
                                "Updated addon link on clipboard"
                            } else {
                                "Addon manifest on clipboard"
                            }),
                    )
                    .child(meta_divider())
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .font(mono_font())
                            .text_xs()
                            .text_color(rgb(t.text_secondary))
                            .child(redact_long_manifest_url(clip_url)),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(sp(8.0))
                    .child(
                        div()
                            .id("btn-clipboard-install-addon")
                            .cursor_pointer()
                            .h(sp(30.0))
                            .px(sp(13.0))
                            .rounded(px(RADIUS_CONTROL))
                            .bg(rgb(t.accent))
                            .hover(move |s| s.bg(rgb(t.accent_hover)))
                            .flex()
                            .items_center()
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_xs()
                            .text_color(rgb(t.cta_on_accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.install_or_replace_addon_from_raw(
                                        &install_url,
                                        false,
                                        cx,
                                    );
                                }),
                            )
                            .child(action_label),
                    )
                    .child(
                        div()
                            .id("btn-clipboard-dismiss-addon")
                            .cursor_pointer()
                            .w(sp(28.0))
                            .h(sp(28.0))
                            .rounded(px(RADIUS_CONTROL))
                            .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.dismissed_clipboard_addon_url = Some(dismiss_url.clone());
                                    this.clipboard_addon_suggestion = None;
                                    cx.notify();
                                }),
                            )
                            .child(svg_icon_weighted(
                                IconName::X,
                                IconWeight::Regular,
                                13.5 * t.ui_scale,
                                rgb(t.text_muted),
                            )),
                    ),
            )
    }

    fn render_universal_manifest_bar(
        &self,
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let url_focused = self.focused_input == Some(FocusedInput::AddonManifestUrl);
        let url_display: SharedString = if self.addon_url_input.is_empty() {
            if url_focused {
                "|".into()
            } else {
                "Paste stremio://, https://.../manifest.json, or .../configure URL".into()
            }
        } else if url_focused {
            format!("{}|", self.addon_url_input).into()
        } else {
            self.addon_url_input.clone().into()
        };

        let busy = !self.addon_url_input.trim().is_empty()
            && normalize_addon_url(self.addon_url_input.trim())
                .ok()
                .is_some_and(|u| self.installing_addon_urls.contains(u.as_str()));

        div()
            .p(sp(14.0))
            .rounded(px(RADIUS_CARD))
            .bg(rgba(t.glass_card_bg))
            .border_1()
            .border_color(rgba(t.glass_border))
            .flex()
            .items_center()
            .gap(sp(10.0))
            .child(
                div()
                    .id("input-addon-manifest-url")
                    .cursor_text()
                    .flex_1()
                    .min_w(px(0.0))
                    .h(sp(40.0))
                    .px(sp(14.0))
                    .rounded(px(RADIUS_CONTROL))
                    .bg(rgba(t.glass_trough_bg))
                    .border_1()
                    .border_color(if url_focused {
                        rgba(t.border_poster_hover)
                    } else {
                        rgba(t.glass_border)
                    })
                    .flex()
                    .items_center()
                    .gap(sp(10.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.focused_input = Some(FocusedInput::AddonManifestUrl);
                            cx.notify();
                        }),
                    )
                    .child(svg_icon_weighted(
                        IconName::Layers,
                        IconWeight::Regular,
                        14.0 * t.ui_scale,
                        rgb(t.text_muted),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .font(mono_font())
                            .text_size(sp(13.0))
                            .text_color(if self.addon_url_input.is_empty() && !url_focused {
                                rgb(t.text_muted)
                            } else {
                                rgb(t.text_primary)
                            })
                            .child(url_display),
                    ),
            )
            .child(
                div()
                    .id("btn-paste-manifest-url")
                    .cursor_pointer()
                    .h(sp(40.0))
                    .px(sp(14.0))
                    .rounded(px(RADIUS_CONTROL))
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
                    .text_color(rgb(t.text_primary))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                                let clean: String = text
                                    .trim()
                                    .chars()
                                    .filter(|c| *c != '\n' && *c != '\r')
                                    .collect();
                                if !clean.is_empty() {
                                    this.addon_url_input = clean;
                                    this.focused_input = Some(FocusedInput::AddonManifestUrl);
                                    cx.notify();
                                }
                            }
                        }),
                    )
                    .child(svg_icon_weighted(
                        IconName::Clipboard,
                        IconWeight::Regular,
                        13.5 * t.ui_scale,
                        rgb(t.text_secondary),
                    ))
                    .child("Paste"),
            )
            .child(
                div()
                    .id("btn-install-manifest-url")
                    .cursor_pointer()
                    .h(sp(40.0))
                    .px(sp(18.0))
                    .rounded(px(RADIUS_CONTROL))
                    .bg(rgb(t.accent))
                    .hover(move |s| s.bg(rgb(t.accent_hover)))
                    .flex()
                    .items_center()
                    .gap(sp(6.0))
                    .font(ui_sans(FontWeight::SEMIBOLD))
                    .text_size(sp(13.5))
                    .text_color(rgb(t.cta_on_accent))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.submit_addon_manifest_lookup(cx);
                        }),
                    )
                    .child(svg_icon_weighted(
                        IconName::Plus,
                        IconWeight::Regular,
                        14.0 * t.ui_scale,
                        rgb(t.cta_on_accent),
                    ))
                    .child(if busy { "Installing..." } else { "Install" }),
            )
    }

    fn render_addons_toolbar(
        &self,
        installed_count: usize,
        directory_count: usize,
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let search_focused = self.focused_input == Some(FocusedInput::AddonSearch);
        let search_display: SharedString = if self.addon_search_query.is_empty() {
            if search_focused {
                "|".into()
            } else {
                "Filter addons...".into()
            }
        } else if search_focused {
            format!("{}|", self.addon_search_query).into()
        } else {
            self.addon_search_query.clone().into()
        };

        div()
            .flex()
            .flex_col()
            .gap(sp(14.0))
            .child(
                div()
                    .border_b_1()
                    .border_color(rgba(t.glass_divider))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(sp(16.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(24.0))
                            .child(render_subtab_button(
                                "addons-tab-installed",
                                "Installed",
                                installed_count,
                                self.addon_sub_tab == AddonSubTab::Installed,
                                t,
                                cx.listener(|this, _, _, cx| {
                                    this.addon_sub_tab = AddonSubTab::Installed;
                                    cx.notify();
                                }),
                            ))
                            .child(render_subtab_button(
                                "addons-tab-explore",
                                "Explore directory",
                                directory_count,
                                self.addon_sub_tab == AddonSubTab::Explore,
                                t,
                                cx.listener(|this, _, _, cx| {
                                    this.addon_sub_tab = AddonSubTab::Explore;
                                    this.core.load_default_remote_addons();
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        div()
                            .pb(sp(8.0))
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .id("input-addon-search")
                                    .cursor_text()
                                    .w(sp(240.0))
                                    .h(sp(34.0))
                                    .px(sp(11.0))
                                    .rounded(px(RADIUS_CONTROL))
                                    .bg(rgba(t.glass_trough_bg))
                                    .border_1()
                                    .border_color(if search_focused {
                                        rgba(t.border_poster_hover)
                                    } else {
                                        rgba(t.glass_border)
                                    })
                                    .flex()
                                    .items_center()
                                    .gap(sp(8.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.focused_input = Some(FocusedInput::AddonSearch);
                                            cx.notify();
                                        }),
                                    )
                                    .child(svg_icon_weighted(
                                        IconName::Search,
                                        IconWeight::Regular,
                                        13.5 * t.ui_scale,
                                        rgb(t.text_muted),
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .truncate()
                                            .text_size(sp(12.5))
                                            .text_color(
                                                if self.addon_search_query.is_empty()
                                                    && !search_focused
                                                {
                                                    rgb(t.text_muted)
                                                } else {
                                                    rgb(t.text_primary)
                                                },
                                            )
                                            .child(search_display),
                                    )
                                    .when(!self.addon_search_query.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .id("btn-clear-addon-search")
                                                .cursor_pointer()
                                                .w(sp(18.0))
                                                .h(sp(18.0))
                                                .rounded(px(4.0))
                                                .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.addon_search_query.clear();
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(svg_icon_weighted(
                                                    IconName::X,
                                                    IconWeight::Regular,
                                                    11.5 * t.ui_scale,
                                                    rgb(t.text_muted),
                                                )),
                                        )
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(sp(8.0))
                    .children(AddonCategoryFilter::ALL.into_iter().enumerate().map(
                        |(idx, pill)| {
                            let active = self.addon_category_filter == pill;
                            render_choice_pill(
                                ("addon-cat-pill", idx),
                                pill.label(),
                                active,
                                cx.listener(move |this, _, _, cx| {
                                    this.addon_category_filter = pill;
                                    cx.notify();
                                }),
                            )
                        },
                    )),
            )
    }

    fn render_installed_priority_deck(
        &self,
        active_addons: &[Descriptor],
        muted_addons: &[MutedAddonEntry],
        telemetry_map: &HashMap<String, AddonTelemetrySnapshot>,
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let total_active = active_addons.len();
        let cat_filter = self.addon_category_filter;
        let query = self.addon_search_query.trim();

        let visible_active: Vec<(usize, &Descriptor, Vec<AddonCategory>)> = active_addons
            .iter()
            .enumerate()
            .filter_map(|(orig_idx, desc)| {
                let cats = classify_descriptor_categories(desc);
                if !cat_filter.matches_addon_categories(&cats) {
                    return None;
                }
                if !addon_matches_search_query(
                    query,
                    &desc.manifest.name,
                    &desc.manifest.id,
                    desc.manifest.description.as_deref(),
                    &desc.manifest.types,
                ) {
                    return None;
                }
                Some((orig_idx, desc, cats))
            })
            .collect();

        let visible_muted: Vec<(usize, &MutedAddonEntry, Vec<AddonCategory>)> = muted_addons
            .iter()
            .enumerate()
            .filter_map(|(m_idx, entry)| {
                let desc = &entry.descriptor;
                let cats = classify_descriptor_categories(desc);
                if !cat_filter.matches_addon_categories(&cats) {
                    return None;
                }
                if !addon_matches_search_query(
                    query,
                    &desc.manifest.name,
                    &desc.manifest.id,
                    desc.manifest.description.as_deref(),
                    &desc.manifest.types,
                ) {
                    return None;
                }
                Some((m_idx, entry, cats))
            })
            .collect();

        if visible_active.is_empty() && visible_muted.is_empty() {
            return div()
                .p(sp(36.0))
                .rounded(px(RADIUS_CARD))
                .bg(rgba(t.glass_card_bg))
                .border_1()
                .border_color(rgba(t.glass_border))
                .flex()
                .flex_col()
                .items_center()
                .gap(sp(8.0))
                .child(
                    div()
                        .font(editorial_serif())
                        .text_size(sp(26.0))
                        .text_color(rgb(t.text_primary))
                        .child("No matching installed addons"),
                )
                .child(
                    div()
                        .text_size(sp(13.5))
                        .text_color(rgb(t.text_secondary))
                        .child(
                            "Clear your search filter or switch to Explore directory to install sources.",
                        ),
                );
        }

        div()
            .flex()
            .flex_col()
            .gap(sp(20.0))
            .when(!visible_active.is_empty(), |el| {
                el.child(
                    div()
                        .rounded(px(RADIUS_CARD))
                        .bg(rgba(t.glass_card_bg))
                        .border_1()
                        .border_color(rgba(t.glass_border))
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .children(visible_active.into_iter().enumerate().map(
                            |(row_pos, (orig_idx, addon, cats))| {
                                let snap = lookup_telemetry(telemetry_map, &addon.transport_url);
                                self.render_installed_addon_row(
                                    row_pos,
                                    orig_idx,
                                    total_active,
                                    addon,
                                    &cats,
                                    snap,
                                    true,
                                    t,
                                    cx,
                                )
                            },
                        )),
                )
            })
            .when(!visible_muted.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(10.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(t.text_muted))
                                        .child("SOFT-MUTED SOURCES (SECRET CONFIG URLS PRESERVED)"),
                                )
                                .child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(t.text_muted))
                                        .child(format!("{} muted", visible_muted.len())),
                                ),
                        )
                        .child(
                            div()
                                .rounded(px(RADIUS_CARD))
                                .bg(rgba(t.glass_card_bg))
                                .border_1()
                                .border_color(rgba(t.glass_border))
                                .overflow_hidden()
                                .flex()
                                .flex_col()
                                .children(visible_muted.into_iter().enumerate().map(
                                    |(row_pos, (m_idx, muted, cats))| {
                                        let snap = lookup_telemetry(
                                            telemetry_map,
                                            &muted.descriptor.transport_url,
                                        );
                                        self.render_installed_addon_row(
                                            10_000 + row_pos,
                                            m_idx,
                                            0,
                                            &muted.descriptor,
                                            &cats,
                                            snap,
                                            false,
                                            t,
                                            cx,
                                        )
                                    },
                                )),
                        ),
                )
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn render_installed_addon_row(
        &self,
        dom_idx: usize,
        slot_idx: usize,
        total_active: usize,
        addon: &Descriptor,
        categories: &[AddonCategory],
        telemetry: Option<&AddonTelemetrySnapshot>,
        is_active: bool,
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let accent_ink = if t.is_light { t.accent_press } else { t.accent };
        let rose_bg = (t.accent_rose << 8) | if t.is_light { 0x17 } else { 0x1a };
        let rose_hover = (t.accent_rose << 8) | if t.is_light { 0x2b } else { 0x2e };
        let is_protected = addon.flags.protected;
        let transport_url = addon.transport_url.clone();
        let url_str = transport_url.to_string();

        let configurable = addon.manifest.behavior_hints.configurable
            || addon.manifest.behavior_hints.configuration_required;
        let configure_target = if configurable {
            addon_configure_url(&transport_url).map(|u| u.to_string())
        } else {
            None
        };

        let cat_summary = if categories.is_empty() {
            "Addon".to_owned()
        } else {
            categories
                .iter()
                .map(|c| c.label())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let types_summary = addon.manifest.types.join(", ");

        let can_move_up = is_active && slot_idx > 0;
        let can_move_down = is_active && slot_idx + 1 < total_active;

        let url_for_top = transport_url.clone();
        let url_for_up = transport_url.clone();
        let url_for_down = transport_url.clone();
        let url_for_copy = url_str.clone();
        let url_for_switch = transport_url.clone();
        let addon_for_remove = addon.clone();
        let url_for_muted_remove = transport_url;
        let addon_name_for_switch = addon.manifest.name.clone();

        div()
            .id(("studio-addon-row", dom_idx))
            .px(sp(18.0))
            .py(sp(14.0))
            .when(dom_idx > 0 && dom_idx != 10_000, |r| {
                r.border_t_1().border_color(rgba(t.glass_divider))
            })
            .bg(if is_active {
                rgba(0x00000000)
            } else {
                rgba(t.glass_trough_bg)
            })
            .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
            .flex()
            .items_center()
            .justify_between()
            .gap(sp(16.0))
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(sp(10.0))
                    .child(
                        div()
                            .w(sp(28.0))
                            .font(mono_font())
                            .font_weight(FontWeight::BOLD)
                            .text_size(sp(14.0))
                            .text_color(if !is_active {
                                rgb(t.text_muted)
                            } else if slot_idx == 0 {
                                rgb(accent_ink)
                            } else {
                                rgb(t.text_secondary)
                            })
                            .child(if is_active {
                                format!("{:02}", slot_idx + 1)
                            } else {
                                "--".to_owned()
                            }),
                    )
                    .when(is_active && total_active > 1, |el| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(sp(3.0))
                                .child(render_reorder_icon_btn(
                                    ("addon-pin-top", dom_idx),
                                    IconName::ArrowUpToLine,
                                    can_move_up,
                                    t,
                                    cx.listener(move |this, _, _, cx| {
                                        if can_move_up && this.core.move_addon_to_top(&url_for_top)
                                        {
                                            cx.notify();
                                        }
                                    }),
                                ))
                                .child(render_reorder_icon_btn(
                                    ("addon-move-up", dom_idx),
                                    IconName::ArrowUp,
                                    can_move_up,
                                    t,
                                    cx.listener(move |this, _, _, cx| {
                                        if can_move_up && this.core.move_addon_up(&url_for_up) {
                                            cx.notify();
                                        }
                                    }),
                                ))
                                .child(render_reorder_icon_btn(
                                    ("addon-move-down", dom_idx),
                                    IconName::ArrowDown,
                                    can_move_down,
                                    t,
                                    cx.listener(move |this, _, _, cx| {
                                        if can_move_down && this.core.move_addon_down(&url_for_down)
                                        {
                                            cx.notify();
                                        }
                                    }),
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(sp(4.0))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(sp(8.0))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(sp(14.5))
                                    .text_color(if is_active {
                                        rgb(t.text_primary)
                                    } else {
                                        rgb(t.text_secondary)
                                    })
                                    .child(addon.manifest.name.clone()),
                            )
                            .child(neutral_chip(format!("v{}", addon.manifest.version)))
                            .when(!is_active, |el| {
                                el.child(
                                    div()
                                        .px(sp(7.0))
                                        .py(sp(2.0))
                                        .rounded(px(RADIUS_PILL))
                                        .bg(rgba(t.glass_trough_bg))
                                        .border_1()
                                        .border_color(rgba(t.glass_border))
                                        .font(mono_font())
                                        .text_size(sp(11.0))
                                        .text_color(rgb(t.text_muted))
                                        .child("Muted"),
                                )
                            })
                            .child(meta_divider())
                            .child(
                                div()
                                    .font(mono_font())
                                    .text_xs()
                                    .text_color(rgb(t.text_secondary))
                                    .child(cat_summary),
                            )
                            .when(!types_summary.is_empty(), |el| {
                                el.child(meta_divider()).child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(t.text_muted))
                                        .child(types_summary),
                                )
                            })
                            .when_some(telemetry, |el, snap| {
                                el.children(Self::render_telemetry_readout(
                                    dom_idx,
                                    snap,
                                    &url_str,
                                    &addon.manifest.name,
                                    t,
                                    cx,
                                ))
                            }),
                    )
                    .when_some(addon.manifest.description.clone(), |el, desc| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(rgb(t.text_secondary))
                                .truncate()
                                .max_w(px(640.0))
                                .child(desc),
                        )
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(sp(8.0))
                    .when_some(configure_target, |el, cfg_url| {
                        el.child(
                            div()
                                .id(("addon-configure-btn", dom_idx))
                                .cursor_pointer()
                                .h(sp(30.0))
                                .px(sp(10.0))
                                .rounded(px(RADIUS_CONTROL))
                                .bg(rgba(t.glass_trough_bg))
                                .border_1()
                                .border_color(rgba(t.glass_border))
                                .hover(move |s| {
                                    s.bg(rgba(t.glass_row_hover_bg))
                                        .border_color(rgba(t.glass_border_strong))
                                })
                                .flex()
                                .items_center()
                                .gap(sp(5.0))
                                .text_xs()
                                .text_color(rgb(t.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.open_url(&cfg_url);
                                        this.push_toast(
                                            ToastKind::Info,
                                            "Opened addon configuration",
                                            Some(
                                                "Copy the updated stremio:// link when finished to update in place."
                                                    .to_owned(),
                                            ),
                                            cx,
                                        );
                                    }),
                                )
                                .child(svg_icon_weighted(
                                    IconName::ExternalLink,
                                    IconWeight::Regular,
                                    12.0 * t.ui_scale,
                                    rgb(t.text_secondary),
                                ))
                                .child("Configure"),
                        )
                    })
                    .child(
                        div()
                            .id(("addon-copy-url-btn", dom_idx))
                            .cursor_pointer()
                            .h(sp(30.0))
                            .px(sp(10.0))
                            .rounded(px(RADIUS_CONTROL))
                            .bg(rgba(t.glass_trough_bg))
                            .border_1()
                            .border_color(rgba(t.glass_border))
                            .hover(move |s| {
                                s.bg(rgba(t.glass_row_hover_bg))
                                    .border_color(rgba(t.glass_border_strong))
                            })
                            .flex()
                            .items_center()
                            .gap(sp(5.0))
                            .text_xs()
                            .text_color(rgb(t.text_secondary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        url_for_copy.clone(),
                                    ));
                                    this.push_toast(
                                        ToastKind::Info,
                                        "Copied manifest URL",
                                        None,
                                        cx,
                                    );
                                }),
                            )
                            .child(svg_icon_weighted(
                                IconName::Clipboard,
                                IconWeight::Regular,
                                12.0 * t.ui_scale,
                                rgb(t.text_muted),
                            ))
                            .child("Copy URL"),
                    )
                    .child(if is_protected {
                        div()
                            .h(sp(26.0))
                            .px(sp(9.0))
                            .rounded(px(RADIUS_PILL))
                            .bg(rgba(t.glass_trough_bg))
                            .border_1()
                            .border_color(rgba(t.glass_border))
                            .flex()
                            .items_center()
                            .font(mono_font())
                            .text_size(sp(11.0))
                            .text_color(rgb(t.text_muted))
                            .child("Core")
                            .into_any_element()
                    } else {
                        render_movietracker_switch(
                            ("addon-soft-switch", dom_idx),
                            is_active,
                            cx.listener(move |this, _, _, cx| {
                                match this.core.set_addon_enabled(&url_for_switch, !is_active) {
                                    Ok(()) => {
                                        this.push_toast(
                                            ToastKind::Info,
                                            if is_active {
                                                format!("Muted {addon_name_for_switch}")
                                            } else {
                                                format!("Enabled {addon_name_for_switch}")
                                            },
                                            if is_active {
                                                Some("Config URL kept locally".to_owned())
                                            } else {
                                                None
                                            },
                                            cx,
                                        );
                                    }
                                    Err(err) => {
                                        this.push_toast(
                                            ToastKind::Error,
                                            "Could not toggle addon",
                                            Some(err),
                                            cx,
                                        );
                                    }
                                }
                                cx.notify();
                            }),
                        )
                        .into_any_element()
                    })
                    .when(!is_protected, |el| {
                        el.child(
                            div()
                                .id(("addon-remove-btn", dom_idx))
                                .cursor_pointer()
                                .w(sp(30.0))
                                .h(sp(30.0))
                                .rounded(px(RADIUS_CONTROL))
                                .bg(rgba(rose_bg))
                                .hover(move |s| s.bg(rgba(rose_hover)))
                                .flex()
                                .items_center()
                                .justify_center()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        let removed_name = addon_for_remove.manifest.name.clone();
                                        if is_active {
                                            this.core.uninstall_addon(addon_for_remove.clone());
                                        } else {
                                            this.core.remove_muted_addon(&url_for_muted_remove);
                                        }
                                        this.push_toast(
                                            ToastKind::Info,
                                            format!("Removed {removed_name}"),
                                            None,
                                            cx,
                                        );
                                        cx.notify();
                                    }),
                                )
                                .child(svg_icon_weighted(
                                    IconName::Trash2,
                                    IconWeight::Regular,
                                    13.5 * t.ui_scale,
                                    rgb(t.accent_rose),
                                )),
                        )
                    }),
            )
    }

    fn trigger_addon_health_probe(
        &mut self,
        reset_key: &str,
        probe_url: &str,
        addon_name: &str,
        cx: &mut Context<Self>,
    ) {
        self.core.reset_addon_circuit(reset_key);
        self.push_toast(
            ToastKind::Info,
            format!("Testing {addon_name}..."),
            None,
            cx,
        );
        cx.notify();

        let core = self.core.clone();
        let probe_url = probe_url.to_owned();
        let name = addon_name.to_owned();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = core.probe_addon_health_async(&probe_url).await;
            let _ = this.update(cx, |shell, cx| {
                match res {
                    Ok(snap) => {
                        let detail = snap
                            .last_latency_ms
                            .or(snap.ema_latency_ms)
                            .map(|ms| format!("Manifest responded in {ms} ms"));
                        shell.push_toast(
                            ToastKind::Success,
                            format!("{name} is healthy"),
                            detail,
                            cx,
                        );
                    }
                    Err(err) => {
                        let detail = parse_short_error_reason(&err).unwrap_or(err);
                        shell.push_toast(
                            ToastKind::Error,
                            format!("{name} probe failed"),
                            Some(detail),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_telemetry_readout(
        dom_idx: usize,
        snap: &AddonTelemetrySnapshot,
        url_str: &str,
        addon_name: &str,
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let rtt_opt = snap.ema_latency_ms.or(snap.last_latency_ms);
        match snap.status {
            AddonHealthStatus::Idle => None,
            AddonHealthStatus::Healthy => {
                let ms = rtt_opt?;
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(8.0))
                        .child(meta_divider())
                        .child(
                            div()
                                .font(mono_font())
                                .text_xs()
                                .text_color(rgb(t.status_watched))
                                .child(format!("{ms} ms")),
                        )
                        .into_any_element(),
                )
            }
            AddonHealthStatus::Slow => {
                let label = match rtt_opt {
                    Some(ms) => format!("Slow {ms} ms"),
                    None => "Slow".to_owned(),
                };
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(8.0))
                        .child(meta_divider())
                        .child(
                            div()
                                .font(mono_font())
                                .text_xs()
                                .text_color(rgb(t.accent_amber))
                                .child(label),
                        )
                        .into_any_element(),
                )
            }
            AddonHealthStatus::Degraded => {
                let reason_label = format_degraded_status_label(snap.last_error.as_deref());
                let reset_key = url_str.to_owned();
                let probe_url = url_str.to_owned();
                let name = addon_name.to_owned();
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(8.0))
                        .child(meta_divider())
                        .child(
                            div()
                                .font(mono_font())
                                .text_xs()
                                .text_color(rgb(t.accent_amber))
                                .child(reason_label),
                        )
                        .child(render_health_action_pill(
                            ("addon-test-health", dom_idx),
                            "Test",
                            t.accent_amber,
                            t,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.trigger_addon_health_probe(&reset_key, &probe_url, &name, cx);
                            }),
                        ))
                        .into_any_element(),
                )
            }
            AddonHealthStatus::CircuitOpen { remaining_secs } => {
                let reset_key = url_str.to_owned();
                let probe_url = url_str.to_owned();
                let name = addon_name.to_owned();
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(8.0))
                        .child(meta_divider())
                        .child(
                            div()
                                .font(mono_font())
                                .text_xs()
                                .text_color(rgb(t.accent_rose))
                                .child(format!("Cooldown {remaining_secs}s")),
                        )
                        .child(render_health_action_pill(
                            ("addon-retry-circuit", dom_idx),
                            "Retry",
                            t.accent_rose,
                            t,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.trigger_addon_health_probe(&reset_key, &probe_url, &name, cx);
                            }),
                        ))
                        .into_any_element(),
                )
            }
        }
    }

    fn render_explore_directory_deck(
        &self,
        directory_cards: &[DirectoryAddonCard],
        active_addons: &[Descriptor],
        muted_addons: &[MutedAddonEntry],
        remote_loading: bool,
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let cat_filter = self.addon_category_filter;
        let query = self.addon_search_query.trim();

        let filtered: Vec<&DirectoryAddonCard> = directory_cards
            .iter()
            .filter(|card| {
                cat_filter.matches_addon_categories(&card.categories)
                    && addon_matches_search_query(
                        query,
                        &card.name,
                        &card.id,
                        card.description.as_deref().or(card.tagline.as_deref()),
                        &card.types,
                    )
            })
            .collect();

        if filtered.is_empty() {
            return div()
                .p(sp(36.0))
                .rounded(px(RADIUS_CARD))
                .bg(rgba(t.glass_card_bg))
                .border_1()
                .border_color(rgba(t.glass_border))
                .flex()
                .flex_col()
                .items_center()
                .gap(sp(8.0))
                .child(
                    div()
                        .font(editorial_serif())
                        .text_size(sp(26.0))
                        .text_color(rgb(t.text_primary))
                        .child(if remote_loading {
                            "Loading addon directory..."
                        } else {
                            "No directory addons match this filter"
                        }),
                )
                .child(
                    div()
                        .text_size(sp(13.5))
                        .text_color(rgb(t.text_secondary))
                        .child("Try switching category pills or clearing the search box."),
                );
        }

        div()
            .grid()
            .grid_cols(2)
            .gap(sp(16.0))
            .children(filtered.into_iter().enumerate().map(|(idx, card)| {
                self.render_directory_card(idx, card, active_addons, muted_addons, t, cx)
            }))
    }

    fn render_directory_card(
        &self,
        idx: usize,
        card: &DirectoryAddonCard,
        active_addons: &[Descriptor],
        muted_addons: &[MutedAddonEntry],
        t: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let accent_ink = if t.is_light { t.accent_press } else { t.accent };
        let watched_border = (t.status_watched << 8) | if t.is_light { 0x59 } else { 0x4d };
        let watched_bg = (t.status_watched << 8) | if t.is_light { 0x1f } else { 0x1c };

        let norm_card_url = normalize_addon_url(&card.manifest_url)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| card.manifest_url.clone());

        let active_match = active_addons.iter().find(|a| {
            a.manifest.id.eq_ignore_ascii_case(&card.id)
                || a.transport_url.as_str().eq_ignore_ascii_case(&norm_card_url)
        });

        let muted_match = muted_addons.iter().find(|m| {
            m.descriptor.manifest.id.eq_ignore_ascii_case(&card.id)
                || m.descriptor
                    .transport_url
                    .as_str()
                    .eq_ignore_ascii_case(&norm_card_url)
        });

        let is_installing = self.installing_addon_urls.contains(&norm_card_url);
        let cat_label = card
            .categories
            .first()
            .map(|c| c.label())
            .unwrap_or("Addon");
        let types_label = card.types.join(", ");

        let cfg_url_opt = card
            .configure_url
            .clone()
            .or_else(|| {
                active_match
                    .and_then(|a| addon_configure_url(&a.transport_url))
                    .map(|u| u.to_string())
            });

        div()
            .id(("explore-addon-card", idx))
            .p(sp(20.0))
            .rounded(px(RADIUS_CARD))
            .bg(rgba(t.glass_card_bg))
            .border_1()
            .border_color(rgba(t.glass_border))
            .hover(move |s| s.border_color(rgba(t.glass_border_strong)))
            .flex()
            .flex_col()
            .justify_between()
            .gap(sp(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(8.0))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap(sp(12.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap(sp(2.0))
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::SEMIBOLD))
                                            .text_size(sp(15.5))
                                            .text_color(rgb(t.text_primary))
                                            .truncate()
                                            .child(card.name.clone()),
                                    )
                                    .when_some(card.tagline.clone(), |el, tagline| {
                                        el.child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(accent_ink))
                                                .truncate()
                                                .child(tagline),
                                        )
                                    }),
                            )
                            .child(neutral_chip(cat_label)),
                    )
                    .when_some(card.description.clone(), |el, desc| {
                        el.child(
                            div()
                                .text_size(sp(13.0))
                                .text_color(rgb(t.text_secondary))
                                .line_clamp(2)
                                .child(desc),
                        )
                    }),
            )
            .child(
                div()
                    .pt(sp(12.0))
                    .border_t_1()
                    .border_color(rgba(t.glass_divider))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sp(12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .font(mono_font())
                            .text_xs()
                            .text_color(rgb(t.text_muted))
                            .truncate()
                            .child(if types_label.is_empty() {
                                "movie, series".to_owned()
                            } else {
                                types_label
                            }),
                    )
                    .child(if active_match.is_some() {
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(8.0))
                            .when_some(cfg_url_opt, |el, cfg_url| {
                                el.child(
                                    div()
                                        .id(("explore-cfg-installed", idx))
                                        .cursor_pointer()
                                        .h(sp(28.0))
                                        .px(sp(10.0))
                                        .rounded(px(RADIUS_CONTROL))
                                        .bg(rgba(t.glass_trough_bg))
                                        .border_1()
                                        .border_color(rgba(t.glass_border))
                                        .hover(move |s| {
                                            s.bg(rgba(t.glass_row_hover_bg))
                                                .border_color(rgba(t.glass_border_strong))
                                        })
                                        .flex()
                                        .items_center()
                                        .gap(sp(5.0))
                                        .text_xs()
                                        .text_color(rgb(t.text_secondary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |_, _, _, cx| {
                                                cx.open_url(&cfg_url);
                                            }),
                                        )
                                        .child(svg_icon_weighted(
                                            IconName::ExternalLink,
                                            IconWeight::Regular,
                                            11.5 * t.ui_scale,
                                            rgb(t.text_muted),
                                        ))
                                        .child("Configure"),
                                )
                            })
                            .child(semantic_status_pill(
                                IconName::Check,
                                "Installed",
                                t.status_watched,
                                watched_border,
                                watched_bg,
                            ))
                            .into_any_element()
                    } else if let Some(muted) = muted_match {
                        let enable_url = muted.descriptor.transport_url.clone();
                        let enable_name = muted.descriptor.manifest.name.clone();
                        div()
                            .id(("explore-reenable-btn", idx))
                            .cursor_pointer()
                            .h(sp(32.0))
                            .px(sp(14.0))
                            .rounded(px(RADIUS_CONTROL))
                            .bg(rgba(t.accent_soft))
                            .border_1()
                            .border_color(rgba(t.border_accent))
                            .flex()
                            .items_center()
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_xs()
                            .text_color(rgb(accent_ink))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    if this.core.set_addon_enabled(&enable_url, true).is_ok() {
                                        this.push_toast(
                                            ToastKind::Success,
                                            format!("Re-enabled {enable_name}"),
                                            None,
                                            cx,
                                        );
                                        cx.notify();
                                    }
                                }),
                            )
                            .child("Enable (muted)")
                            .into_any_element()
                    } else if card.requires_browser_config {
                        let browser_url = cfg_url_opt.unwrap_or_else(|| card.manifest_url.clone());
                        let card_name = card.name.clone();
                        div()
                            .id(("explore-browser-cfg-btn", idx))
                            .cursor_pointer()
                            .h(sp(32.0))
                            .px(sp(13.0))
                            .rounded(px(RADIUS_CONTROL))
                            .bg(rgba(t.glass_trough_bg))
                            .border_1()
                            .border_color(rgba(t.glass_border))
                            .hover(move |s| {
                                s.bg(rgba(t.glass_row_hover_bg))
                                    .border_color(rgb(t.accent))
                            })
                            .flex()
                            .items_center()
                            .gap(sp(6.0))
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_xs()
                            .text_color(rgb(t.text_primary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.open_url(&browser_url);
                                    this.push_toast(
                                        ToastKind::Info,
                                        format!("Configuring {card_name} in browser"),
                                        Some(
                                            "Copy your personal stremio:// install link when ready."
                                                .to_owned(),
                                        ),
                                        cx,
                                    );
                                }),
                            )
                            .child(svg_icon_weighted(
                                IconName::ExternalLink,
                                IconWeight::Regular,
                                12.0 * t.ui_scale,
                                rgb(accent_ink),
                            ))
                            .child("Configure in browser")
                            .into_any_element()
                    } else {
                        let raw_install_url = card.manifest_url.clone();
                        let ready_desc = card.ready_descriptor.clone();
                        div()
                            .id(("explore-install-btn", idx))
                            .cursor_pointer()
                            .h(sp(32.0))
                            .px(sp(15.0))
                            .rounded(px(RADIUS_CONTROL))
                            .bg(rgb(t.accent))
                            .hover(move |s| s.bg(rgb(t.accent_hover)))
                            .flex()
                            .items_center()
                            .gap(sp(5.0))
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_xs()
                            .text_color(rgb(t.cta_on_accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(desc) = ready_desc.clone() {
                                        match this.core.install_or_replace_addon_descriptor(desc) {
                                            Ok(out) => {
                                                this.push_toast(
                                                    ToastKind::Success,
                                                    format!(
                                                        "Installed {}",
                                                        out.descriptor.manifest.name
                                                    ),
                                                    Some(format!(
                                                        "Priority slot {:02}",
                                                        out.slot_index + 1
                                                    )),
                                                    cx,
                                                );
                                                cx.notify();
                                            }
                                            Err(err) => {
                                                this.push_toast(
                                                    ToastKind::Error,
                                                    "Addon install failed",
                                                    Some(err),
                                                    cx,
                                                );
                                            }
                                        }
                                    } else {
                                        this.install_or_replace_addon_from_raw(
                                            &raw_install_url,
                                            false,
                                            cx,
                                        );
                                    }
                                }),
                            )
                            .child(svg_icon_weighted(
                                IconName::Plus,
                                IconWeight::Regular,
                                12.5 * t.ui_scale,
                                rgb(t.cta_on_accent),
                            ))
                            .child(if is_installing {
                                "Installing..."
                            } else {
                                "Install"
                            })
                            .into_any_element()
                    }),
            )
    }
}

fn render_subtab_button(
    id: &'static str,
    label: &'static str,
    count: usize,
    active: bool,
    t: ThemePalette,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let accent_ink = if t.is_light { t.accent_press } else { t.accent };
    div()
        .id(id)
        .cursor_pointer()
        .pb(sp(10.0))
        .border_b_2()
        .border_color(if active {
            rgb(t.accent)
        } else {
            rgba(0x00000000)
        })
        .flex()
        .items_center()
        .gap(sp(8.0))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            div()
                .font(ui_sans(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                }))
                .text_size(sp(14.0))
                .text_color(if active {
                    rgb(t.text_primary)
                } else {
                    rgb(t.text_secondary)
                })
                .child(label),
        )
        .child(
            div()
                .px(sp(6.5))
                .py(sp(1.5))
                .rounded(px(RADIUS_PILL))
                .bg(if active {
                    rgba(t.accent_soft)
                } else {
                    rgba(t.glass_trough_bg)
                })
                .font(mono_font())
                .text_size(sp(11.0))
                .text_color(if active {
                    rgb(accent_ink)
                } else {
                    rgb(t.text_muted)
                })
                .child(format!("{count}")),
        )
}

fn render_reorder_icon_btn(
    id: impl Into<gpui::ElementId>,
    icon: IconName,
    enabled: bool,
    t: ThemePalette,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let disabled_col = (t.text_muted << 8) | 0x55;
    div()
        .id(id)
        .when(enabled, |el| {
            el.cursor_pointer()
                .hover(move |s| s.bg(rgba(t.glass_row_hover_bg)))
                .on_mouse_down(MouseButton::Left, on_click)
        })
        .w(sp(24.0))
        .h(sp(24.0))
        .rounded(px(5.0))
        .flex()
        .items_center()
        .justify_center()
        .child(svg_icon_weighted(
            icon,
            IconWeight::Regular,
            12.5 * t.ui_scale,
            if enabled {
                rgb(t.text_secondary).into()
            } else {
                gpui::Hsla::from(rgba(disabled_col))
            },
        ))
}

fn render_health_action_pill(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    color_hex: u32,
    t: ThemePalette,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let pill_bg = (color_hex << 8) | 0x1f;
    let pill_hover = (color_hex << 8) | 0x33;
    let pill_border = (color_hex << 8) | 0x44;
    div()
        .id(id)
        .cursor_pointer()
        .px(sp(8.0))
        .py(sp(1.5))
        .rounded(px(RADIUS_PILL))
        .bg(rgba(pill_bg))
        .border_1()
        .border_color(rgba(pill_border))
        .hover(move |s| s.bg(rgba(pill_hover)))
        .flex()
        .items_center()
        .gap(sp(4.0))
        .font(mono_font())
        .text_size(sp(11.0))
        .text_color(rgb(color_hex))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(svg_icon_weighted(
            IconName::RefreshCw,
            IconWeight::Regular,
            10.5 * t.ui_scale,
            rgb(color_hex),
        ))
        .child(label)
}

fn parse_short_error_reason(err: &str) -> Option<String> {
    let lower = err.to_ascii_lowercase();

    for marker in ["status code ", "http "] {
        if let Some(idx) = lower.find(marker) {
            let rest = err[idx + marker.len()..].trim_start();
            let digits: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if digits.len() == 3 {
                return Some(format!("HTTP {digits}"));
            }
        }
    }

    if lower.contains("timed out") || lower.contains("timeout") {
        return Some("Timeout".to_owned());
    }
    if lower.contains("dns")
        || lower.contains("lookup address")
        || lower.contains("name or service not known")
        || lower.contains("nodename nor servname")
        || lower.contains("no such host")
        || lower.contains("failed to resolve")
    {
        return Some("DNS".to_owned());
    }
    if lower.contains("tls")
        || lower.contains("ssl")
        || lower.contains("certificate")
        || lower.contains("handshake")
        || lower.contains("rustls")
        || lower.contains("webpki")
    {
        return Some("TLS".to_owned());
    }
    if lower.contains("connection")
        || lower.contains("connect")
        || lower.contains("refused")
        || lower.contains("unreachable")
        || lower.contains("reset by peer")
    {
        return Some("Connect".to_owned());
    }

    None
}

fn format_degraded_status_label(last_error: Option<&str>) -> String {
    match last_error.and_then(parse_short_error_reason) {
        Some(reason) => format!("Degraded · {reason}"),
        None => "Degraded".to_owned(),
    }
}

fn lookup_telemetry<'a>(
    map: &'a HashMap<String, AddonTelemetrySnapshot>,
    url: &Url,
) -> Option<&'a AddonTelemetrySnapshot> {
    map.get(url.as_str()).or_else(|| {
        let origin = addon_origin_from_url(url);
        map.get(&origin)
    })
}

fn is_clipboard_url_in_place_update(
    clip_url: &str,
    active_addons: &[Descriptor],
    muted_addons: &[MutedAddonEntry],
) -> bool {
    let Ok(parsed) = Url::parse(clip_url) else {
        return false;
    };
    let clip_origin = addon_origin_from_url(&parsed);
    active_addons.iter().any(|a| {
        !a.flags.protected && addon_origin_from_url(&a.transport_url) == clip_origin
    }) || muted_addons
        .iter()
        .any(|m| addon_origin_from_url(&m.descriptor.transport_url) == clip_origin)
}

fn redact_long_manifest_url(url_str: &str) -> String {
    if url_str.chars().count() <= 72 {
        return url_str.to_owned();
    }
    if let Ok(parsed) = Url::parse(url_str) {
        let host = parsed.host_str().unwrap_or("addon");
        let scheme = parsed.scheme();
        let path = parsed.path();
        if path.len() > 36 {
            return format!("{scheme}://{host}/.../manifest.json");
        }
    }
    let head: String = url_str.chars().take(56).collect();
    format!("{head}...")
}

fn build_deduplicated_directory(remote_descriptors: &[Descriptor]) -> Vec<DirectoryAddonCard> {
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut seen_urls: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(curated_addon_directory().len() + remote_descriptors.len());

    for entry in curated_addon_directory() {
        let id_key = entry.id.to_ascii_lowercase();
        let norm_url = normalize_addon_url(entry.manifest_url)
            .map(|u| u.to_string().to_ascii_lowercase())
            .unwrap_or_else(|_| entry.manifest_url.to_ascii_lowercase());
        seen_ids.insert(id_key);
        seen_urls.insert(norm_url);

        out.push(DirectoryAddonCard {
            id: entry.id.to_owned(),
            name: entry.name.to_owned(),
            tagline: Some(entry.tagline.to_owned()),
            description: Some(entry.description.to_owned()),
            categories: vec![entry.category],
            types: entry.types.iter().map(|s| (*s).to_owned()).collect(),
            manifest_url: entry.manifest_url.to_owned(),
            configure_url: entry.configure_url.map(str::to_owned),
            requires_browser_config: entry.requires_browser_config,
            ready_descriptor: None,
        });
    }

    for desc in remote_descriptors {
        let id_key = desc.manifest.id.trim().to_ascii_lowercase();
        let url_key = desc.transport_url.as_str().to_ascii_lowercase();
        if id_key.is_empty() || !seen_ids.insert(id_key) || !seen_urls.insert(url_key) {
            continue;
        }

        let cats = classify_descriptor_categories(desc);
        let requires_cfg = desc.manifest.behavior_hints.configuration_required;
        let cfg_url = if requires_cfg || desc.manifest.behavior_hints.configurable {
            addon_configure_url(&desc.transport_url).map(|u| u.to_string())
        } else {
            None
        };

        out.push(DirectoryAddonCard {
            id: desc.manifest.id.clone(),
            name: desc.manifest.name.clone(),
            tagline: Some(format!("v{}", desc.manifest.version)),
            description: desc.manifest.description.clone(),
            categories: cats,
            types: desc.manifest.types.clone(),
            manifest_url: desc.transport_url.to_string(),
            configure_url: cfg_url,
            requires_browser_config: requires_cfg,
            ready_descriptor: if requires_cfg {
                None
            } else {
                Some(desc.clone())
            },
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curated_directory_builds_without_duplicates_and_redacts_secret_paths() {
        let cards = build_deduplicated_directory(&[]);
        assert!(cards.len() >= 10);

        let mut ids = HashSet::new();
        for c in &cards {
            assert!(ids.insert(c.id.to_ascii_lowercase()));
        }

        let secret = "https://torrentio.strem.fun/realdebrid=abcdef0123456789abcdef0123456789abcdef0123456789/manifest.json";
        let redacted = redact_long_manifest_url(secret);
        assert_eq!(redacted, "https://torrentio.strem.fun/.../manifest.json");
    }

    #[test]
    fn degraded_status_label_parses_concise_reasons_with_clean_fallback() {
        assert_eq!(format_degraded_status_label(None), "Degraded");
        assert_eq!(
            format_degraded_status_label(Some("Request timed out: operation timed out (https://a.test/...)")),
            "Degraded · Timeout"
        );
        assert_eq!(
            format_degraded_status_label(Some("Unexpected HTTP status code 502 for https://a.test/...")),
            "Degraded · HTTP 502"
        );
        assert_eq!(
            format_degraded_status_label(Some("Unexpected HTTP status code 429 for https://a.test/...")),
            "Degraded · HTTP 429"
        );
        assert_eq!(
            format_degraded_status_label(Some("DNS lookup failed: failed to lookup address information (https://a.test/...)")),
            "Degraded · DNS"
        );
        assert_eq!(
            format_degraded_status_label(Some("TLS error: invalid peer certificate (https://a.test/...)")),
            "Degraded · TLS"
        );
        assert_eq!(
            format_degraded_status_label(Some("Connection failed: Connection refused (https://a.test/...)")),
            "Degraded · Connect"
        );
        assert_eq!(
            format_degraded_status_label(Some("unexpected EOF while reading body")),
            "Degraded"
        );
    }
}
