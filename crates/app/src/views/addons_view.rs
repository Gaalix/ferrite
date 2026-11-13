use std::collections::{HashMap, HashSet};

use gpui::{
    div, prelude::*, px, rgb, rgba, ClipboardItem, Context, FontWeight, MouseButton, SharedString,
};
use stremio_core_runtime::{
    addon_configure_url, addon_origin_from_url, classify_descriptor_categories,
    curated_addon_directory, format_bytes, normalize_addon_url,
    stremio_core::{models::common::Loadable, types::addon::Descriptor},
    AddonCategory, AddonHealthStatus, AddonTelemetrySnapshot, MutedAddonEntry,
};
use url::Url;

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{addon_matches_search_query, AddonCategoryFilter, AddonSubTab, FocusedInput, ToastKind},
    theme::{
        editorial_serif, meta_divider, mono_font, neutral_chip, semantic_status_pill, ui_sans,
        ACCENT, ACCENT_AMBER, ACCENT_HOVER, ACCENT_ROSE, ACCENT_SOFT, BG_ELEVATED, BG_INPUT,
        BG_OBSIDIAN, BG_SURFACE, BG_SURFACE_3, BORDER_ACCENT, BORDER_POSTER_HOVER, BORDER_SOFT,
        BORDER_STRONG, BORDER_SUBTLE, STATUS_WATCHED, TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY,
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

        div()
            .id("addons-studio-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .max_w(px(1120.0))
                    .mx_auto()
                    .px(px(32.0))
                    .pt(px(32.0))
                    .pb(px(72.0))
                    .flex()
                    .flex_col()
                    .gap(px(24.0))
                    .child(Self::render_addons_header(total_active, &cache_stats, cx))
                    .when_some(self.clipboard_addon_suggestion.clone(), |el, clip_url| {
                        let is_update = is_clipboard_url_in_place_update(
                            &clip_url,
                            &active_addons,
                            &muted_addons,
                        );
                        el.child(self.render_clipboard_addon_bar(&clip_url, is_update, cx))
                    })
                    .child(self.render_universal_manifest_bar(cx))
                    .child(self.render_addons_toolbar(
                        total_installed_and_muted,
                        total_directory,
                        cx,
                    ))
                    .child(match self.addon_sub_tab {
                        AddonSubTab::Installed => self
                            .render_installed_priority_deck(
                                &active_addons,
                                &muted_addons,
                                &telemetry_map,
                                cx,
                            )
                            .into_any_element(),
                        AddonSubTab::Explore => self
                            .render_explore_directory_deck(
                                &directory_cards,
                                &active_addons,
                                &muted_addons,
                                remote_loading,
                                cx,
                            )
                            .into_any_element(),
                    }),
            )
    }

    fn render_addons_header(
        active_count: usize,
        cache_stats: &stremio_core_runtime::AddonCacheStats,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let has_cache = cache_stats.entries > 0 || cache_stats.hits > 0;

        div()
            .flex()
            .flex_wrap()
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
                            .child("Addons"),
                    )
                    .child(
                        div()
                            .text_size(px(14.5))
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(
                                "Priority execution order, soft-mute preserving secret config URLs, and live origin health.",
                            ),
                    ),
            )
            .child(
                div()
                    .h(px(34.0))
                    .px(px(14.0))
                    .rounded(px(999.0))
                    .bg(rgb(BG_SURFACE))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .font(mono_font())
                    .text_xs()
                    .text_color(rgb(TEXT_SECONDARY))
                    .child(
                        div()
                            .text_color(rgb(TEXT_PRIMARY))
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
                                .text_color(rgb(ACCENT))
                                .hover(|s| s.text_color(rgb(ACCENT_HOVER)))
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
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
            .px(px(16.0))
            .py(px(11.0))
            .rounded(px(8.0))
            .bg(rgb(BG_SURFACE))
            .border_1()
            .border_color(rgba(BORDER_ACCENT))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(14.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(svg_icon(IconName::Clipboard, 15.0, rgb(ACCENT)))
                    .child(
                        div()
                            .flex_shrink_0()
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_size(px(13.0))
                            .text_color(rgb(TEXT_PRIMARY))
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
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(redact_long_manifest_url(clip_url)),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("btn-clipboard-install-addon")
                            .cursor_pointer()
                            .h(px(30.0))
                            .px(px(13.0))
                            .rounded(px(6.0))
                            .bg(rgb(ACCENT))
                            .hover(|s| s.bg(rgb(ACCENT_HOVER)))
                            .flex()
                            .items_center()
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_xs()
                            .text_color(rgb(BG_OBSIDIAN))
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
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(6.0))
                            .hover(|s| s.bg(rgb(BG_ELEVATED)))
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
                            .child(svg_icon(IconName::X, 13.5, rgb(TEXT_MUTED))),
                    ),
            )
    }

    fn render_universal_manifest_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .p(px(16.0))
            .rounded(px(10.0))
            .bg(rgb(BG_SURFACE))
            .border_1()
            .border_color(rgba(BORDER_SUBTLE))
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .id("input-addon-manifest-url")
                    .cursor_text()
                    .flex_1()
                    .min_w(px(0.0))
                    .h(px(40.0))
                    .px(px(14.0))
                    .rounded(px(6.0))
                    .bg(rgb(BG_INPUT))
                    .border_1()
                    .border_color(if url_focused {
                        rgba(BORDER_POSTER_HOVER)
                    } else {
                        rgba(BORDER_SUBTLE)
                    })
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.focused_input = Some(FocusedInput::AddonManifestUrl);
                            cx.notify();
                        }),
                    )
                    .child(svg_icon(IconName::Layers, 14.0, rgb(TEXT_MUTED)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .font(mono_font())
                            .text_size(px(13.0))
                            .text_color(if self.addon_url_input.is_empty() && !url_focused {
                                rgb(TEXT_MUTED)
                            } else {
                                rgb(TEXT_PRIMARY)
                            })
                            .child(url_display),
                    ),
            )
            .child(
                div()
                    .id("btn-paste-manifest-url")
                    .cursor_pointer()
                    .h(px(40.0))
                    .px(px(14.0))
                    .rounded(px(6.0))
                    .bg(rgb(BG_ELEVATED))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .hover(|s| s.border_color(rgba(BORDER_STRONG)))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .font(ui_sans(FontWeight::MEDIUM))
                    .text_size(px(13.0))
                    .text_color(rgb(TEXT_PRIMARY))
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
                    .child(svg_icon(IconName::Clipboard, 13.5, rgb(TEXT_SECONDARY)))
                    .child("Paste"),
            )
            .child(
                div()
                    .id("btn-install-manifest-url")
                    .cursor_pointer()
                    .h(px(40.0))
                    .px(px(18.0))
                    .rounded(px(6.0))
                    .bg(rgb(ACCENT))
                    .hover(|s| s.bg(rgb(ACCENT_HOVER)))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .font(ui_sans(FontWeight::SEMIBOLD))
                    .text_size(px(13.5))
                    .text_color(rgb(BG_OBSIDIAN))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.submit_addon_manifest_lookup(cx);
                        }),
                    )
                    .child(svg_icon(IconName::Plus, 14.0, rgb(BG_OBSIDIAN)))
                    .child(if busy { "Installing..." } else { "Install" }),
            )
    }

    fn render_addons_toolbar(
        &self,
        installed_count: usize,
        directory_count: usize,
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
            .gap(px(14.0))
            .child(
                div()
                    .border_b_1()
                    .border_color(rgba(BORDER_SOFT))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(16.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(24.0))
                            .child(render_subtab_button(
                                "addons-tab-installed",
                                "Installed",
                                installed_count,
                                self.addon_sub_tab == AddonSubTab::Installed,
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
                                cx.listener(|this, _, _, cx| {
                                    this.addon_sub_tab = AddonSubTab::Explore;
                                    this.core.load_default_remote_addons();
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        div()
                            .pb(px(8.0))
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .id("input-addon-search")
                                    .cursor_text()
                                    .w(px(240.0))
                                    .h(px(34.0))
                                    .px(px(11.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(BG_SURFACE))
                                    .border_1()
                                    .border_color(if search_focused {
                                        rgba(BORDER_POSTER_HOVER)
                                    } else {
                                        rgba(BORDER_SUBTLE)
                                    })
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.focused_input = Some(FocusedInput::AddonSearch);
                                            cx.notify();
                                        }),
                                    )
                                    .child(svg_icon(IconName::Search, 13.5, rgb(TEXT_MUTED)))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .truncate()
                                            .text_size(px(12.5))
                                            .text_color(
                                                if self.addon_search_query.is_empty()
                                                    && !search_focused
                                                {
                                                    rgb(TEXT_MUTED)
                                                } else {
                                                    rgb(TEXT_PRIMARY)
                                                },
                                            )
                                            .child(search_display),
                                    )
                                    .when(!self.addon_search_query.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .id("btn-clear-addon-search")
                                                .cursor_pointer()
                                                .w(px(18.0))
                                                .h(px(18.0))
                                                .rounded(px(4.0))
                                                .hover(|s| s.bg(rgb(BG_ELEVATED)))
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
                                                .child(svg_icon(
                                                    IconName::X,
                                                    11.5,
                                                    rgb(TEXT_MUTED),
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
                    .gap(px(8.0))
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
                .p(px(36.0))
                .rounded(px(10.0))
                .bg(rgb(BG_SURFACE))
                .border_1()
                .border_color(rgba(BORDER_SUBTLE))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .font(editorial_serif())
                        .text_size(px(26.0))
                        .text_color(rgb(TEXT_PRIMARY))
                        .child("No matching installed addons"),
                )
                .child(
                    div()
                        .text_size(px(13.5))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(
                            "Clear your search filter or switch to Explore directory to install sources.",
                        ),
                );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(20.0))
            .when(!visible_active.is_empty(), |el| {
                el.child(
                    div()
                        .rounded(px(10.0))
                        .bg(rgb(BG_SURFACE))
                        .border_1()
                        .border_color(rgba(BORDER_SUBTLE))
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
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_MUTED))
                                        .child("SOFT-MUTED SOURCES (SECRET CONFIG URLS PRESERVED)"),
                                )
                                .child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(format!("{} muted", visible_muted.len())),
                                ),
                        )
                        .child(
                            div()
                                .rounded(px(10.0))
                                .bg(rgb(BG_SURFACE))
                                .border_1()
                                .border_color(rgba(BORDER_SUBTLE))
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
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
            .px(px(18.0))
            .py(px(14.0))
            .when(dom_idx > 0 && dom_idx != 10_000, |r| {
                r.border_t_1().border_color(rgba(BORDER_SOFT))
            })
            .bg(if is_active {
                rgb(BG_SURFACE)
            } else {
                rgb(BG_OBSIDIAN)
            })
            .hover(|s| s.bg(rgb(BG_ELEVATED)))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .w(px(28.0))
                            .font(mono_font())
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(14.0))
                            .text_color(if !is_active {
                                rgb(TEXT_MUTED)
                            } else if slot_idx == 0 {
                                rgb(ACCENT)
                            } else {
                                rgb(TEXT_SECONDARY)
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
                                .gap(px(3.0))
                                .child(render_reorder_icon_btn(
                                    ("addon-pin-top", dom_idx),
                                    IconName::ArrowUpToLine,
                                    can_move_up,
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
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(px(14.5))
                                    .text_color(if is_active {
                                        rgb(TEXT_PRIMARY)
                                    } else {
                                        rgb(TEXT_SECONDARY)
                                    })
                                    .child(addon.manifest.name.clone()),
                            )
                            .child(neutral_chip(format!("v{}", addon.manifest.version)))
                            .when(!is_active, |el| {
                                el.child(
                                    div()
                                        .px(px(7.0))
                                        .py(px(2.0))
                                        .rounded(px(999.0))
                                        .bg(rgb(BG_SURFACE_3))
                                        .font(mono_font())
                                        .text_size(px(11.0))
                                        .text_color(rgb(TEXT_MUTED))
                                        .child("Muted"),
                                )
                            })
                            .child(meta_divider())
                            .child(
                                div()
                                    .font(mono_font())
                                    .text_xs()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(cat_summary),
                            )
                            .when(!types_summary.is_empty(), |el| {
                                el.child(meta_divider()).child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(types_summary),
                                )
                            })
                            .when_some(telemetry, |el, snap| {
                                el.children(Self::render_telemetry_readout(
                                    dom_idx,
                                    snap,
                                    &url_str,
                                    cx,
                                ))
                            }),
                    )
                    .when_some(addon.manifest.description.clone(), |el, desc| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(rgb(TEXT_SECONDARY))
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
                    .gap(px(8.0))
                    .when_some(configure_target, |el, cfg_url| {
                        el.child(
                            div()
                                .id(("addon-configure-btn", dom_idx))
                                .cursor_pointer()
                                .h(px(30.0))
                                .px(px(10.0))
                                .rounded(px(6.0))
                                .bg(rgb(BG_ELEVATED))
                                .border_1()
                                .border_color(rgba(BORDER_SUBTLE))
                                .hover(|s| s.border_color(rgba(BORDER_STRONG)))
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .text_xs()
                                .text_color(rgb(TEXT_PRIMARY))
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
                                .child(svg_icon(IconName::ExternalLink, 12.0, rgb(TEXT_SECONDARY)))
                                .child("Configure"),
                        )
                    })
                    .child(
                        div()
                            .id(("addon-copy-url-btn", dom_idx))
                            .cursor_pointer()
                            .h(px(30.0))
                            .px(px(10.0))
                            .rounded(px(6.0))
                            .bg(rgb(BG_ELEVATED))
                            .border_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .hover(|s| s.border_color(rgba(BORDER_STRONG)))
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
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
                            .child(svg_icon(IconName::Clipboard, 12.0, rgb(TEXT_MUTED)))
                            .child("Copy URL"),
                    )
                    .child(if is_protected {
                        div()
                            .h(px(26.0))
                            .px(px(9.0))
                            .rounded(px(999.0))
                            .bg(rgb(BG_SURFACE_3))
                            .flex()
                            .items_center()
                            .font(mono_font())
                            .text_size(px(11.0))
                            .text_color(rgb(TEXT_MUTED))
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
                                .w(px(30.0))
                                .h(px(30.0))
                                .rounded(px(6.0))
                                .bg(rgba(0xf8717117))
                                .hover(|s| s.bg(rgba(0xf871712e)))
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
                                .child(svg_icon(IconName::Trash2, 13.5, rgb(ACCENT_ROSE))),
                        )
                    }),
            )
    }

    fn render_telemetry_readout(
        dom_idx: usize,
        snap: &AddonTelemetrySnapshot,
        url_str: &str,
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
                        .gap(px(8.0))
                        .child(meta_divider())
                        .child(
                            div()
                                .font(mono_font())
                                .text_xs()
                                .text_color(rgb(STATUS_WATCHED))
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
                        .gap(px(8.0))
                        .child(meta_divider())
                        .child(
                            div()
                                .font(mono_font())
                                .text_xs()
                                .text_color(rgb(ACCENT_AMBER))
                                .child(label),
                        )
                        .into_any_element(),
                )
            }
            AddonHealthStatus::Degraded => Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(meta_divider())
                    .child(
                        div()
                            .font(mono_font())
                            .text_xs()
                            .text_color(rgb(ACCENT_AMBER))
                            .child("Degraded"),
                    )
                    .into_any_element(),
            ),
            AddonHealthStatus::CircuitOpen { remaining_secs } => {
                let reset_key = url_str.to_owned();
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(meta_divider())
                        .child(
                            div()
                                .font(mono_font())
                                .text_xs()
                                .text_color(rgb(ACCENT_ROSE))
                                .child(format!("Cooldown {remaining_secs}s")),
                        )
                        .child(
                            div()
                                .id(("addon-retry-circuit", dom_idx))
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(1.5))
                                .rounded(px(4.0))
                                .bg(rgba(0xf871711f))
                                .hover(|s| s.bg(rgba(0xf8717133)))
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .font(mono_font())
                                .text_size(px(11.0))
                                .text_color(rgb(ACCENT_ROSE))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.core.reset_addon_circuit(&reset_key);
                                        this.push_toast(
                                            ToastKind::Info,
                                            "Circuit breaker reset",
                                            None,
                                            cx,
                                        );
                                        cx.notify();
                                    }),
                                )
                                .child(svg_icon(IconName::RefreshCw, 10.5, rgb(ACCENT_ROSE)))
                                .child("Retry"),
                        )
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
                .p(px(36.0))
                .rounded(px(10.0))
                .bg(rgb(BG_SURFACE))
                .border_1()
                .border_color(rgba(BORDER_SUBTLE))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .font(editorial_serif())
                        .text_size(px(26.0))
                        .text_color(rgb(TEXT_PRIMARY))
                        .child(if remote_loading {
                            "Loading addon directory..."
                        } else {
                            "No directory addons match this filter"
                        }),
                )
                .child(
                    div()
                        .text_size(px(13.5))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child("Try switching category pills or clearing the search box."),
                );
        }

        div()
            .grid()
            .grid_cols(2)
            .gap(px(16.0))
            .children(filtered.into_iter().enumerate().map(|(idx, card)| {
                self.render_directory_card(idx, card, active_addons, muted_addons, cx)
            }))
    }

    fn render_directory_card(
        &self,
        idx: usize,
        card: &DirectoryAddonCard,
        active_addons: &[Descriptor],
        muted_addons: &[MutedAddonEntry],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
            .p(px(20.0))
            .rounded(px(10.0))
            .bg(rgb(BG_SURFACE))
            .border_1()
            .border_color(rgba(BORDER_SUBTLE))
            .hover(|s| s.border_color(rgba(BORDER_STRONG)))
            .flex()
            .flex_col()
            .justify_between()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::SEMIBOLD))
                                            .text_size(px(15.5))
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .truncate()
                                            .child(card.name.clone()),
                                    )
                                    .when_some(card.tagline.clone(), |el, tagline| {
                                        el.child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(ACCENT))
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
                                .text_size(px(13.0))
                                .text_color(rgb(TEXT_SECONDARY))
                                .line_clamp(2)
                                .child(desc),
                        )
                    }),
            )
            .child(
                div()
                    .pt(px(12.0))
                    .border_t_1()
                    .border_color(rgba(BORDER_SOFT))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .font(mono_font())
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
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
                            .gap(px(8.0))
                            .when_some(cfg_url_opt, |el, cfg_url| {
                                el.child(
                                    div()
                                        .id(("explore-cfg-installed", idx))
                                        .cursor_pointer()
                                        .h(px(28.0))
                                        .px(px(10.0))
                                        .rounded(px(6.0))
                                        .bg(rgb(BG_ELEVATED))
                                        .border_1()
                                        .border_color(rgba(BORDER_SUBTLE))
                                        .hover(|s| s.border_color(rgba(BORDER_STRONG)))
                                        .flex()
                                        .items_center()
                                        .gap(px(5.0))
                                        .text_xs()
                                        .text_color(rgb(TEXT_SECONDARY))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |_, _, _, cx| {
                                                cx.open_url(&cfg_url);
                                            }),
                                        )
                                        .child(svg_icon(
                                            IconName::ExternalLink,
                                            11.5,
                                            rgb(TEXT_MUTED),
                                        ))
                                        .child("Configure"),
                                )
                            })
                            .child(semantic_status_pill(
                                IconName::Check,
                                "Installed",
                                STATUS_WATCHED,
                                0x4ade804d,
                                0x4ade801c,
                            ))
                            .into_any_element()
                    } else if let Some(muted) = muted_match {
                        let enable_url = muted.descriptor.transport_url.clone();
                        let enable_name = muted.descriptor.manifest.name.clone();
                        div()
                            .id(("explore-reenable-btn", idx))
                            .cursor_pointer()
                            .h(px(32.0))
                            .px(px(14.0))
                            .rounded(px(6.0))
                            .bg(rgba(ACCENT_SOFT))
                            .border_1()
                            .border_color(rgb(ACCENT))
                            .flex()
                            .items_center()
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_xs()
                            .text_color(rgb(ACCENT))
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
                            .h(px(32.0))
                            .px(px(13.0))
                            .rounded(px(6.0))
                            .bg(rgb(BG_ELEVATED))
                            .border_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .hover(|s| s.border_color(rgb(ACCENT)))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_xs()
                            .text_color(rgb(TEXT_PRIMARY))
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
                            .child(svg_icon(IconName::ExternalLink, 12.0, rgb(ACCENT)))
                            .child("Configure in browser")
                            .into_any_element()
                    } else {
                        let raw_install_url = card.manifest_url.clone();
                        let ready_desc = card.ready_descriptor.clone();
                        div()
                            .id(("explore-install-btn", idx))
                            .cursor_pointer()
                            .h(px(32.0))
                            .px(px(15.0))
                            .rounded(px(6.0))
                            .bg(rgb(ACCENT))
                            .hover(|s| s.bg(rgb(ACCENT_HOVER)))
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_xs()
                            .text_color(rgb(BG_OBSIDIAN))
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
                            .child(svg_icon(IconName::Plus, 12.5, rgb(BG_OBSIDIAN)))
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
    on_click: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .cursor_pointer()
        .pb(px(10.0))
        .border_b_2()
        .border_color(if active {
            rgb(ACCENT)
        } else {
            rgba(0x00000000)
        })
        .flex()
        .items_center()
        .gap(px(8.0))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            div()
                .font(ui_sans(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                }))
                .text_size(px(14.0))
                .text_color(if active {
                    rgb(TEXT_PRIMARY)
                } else {
                    rgb(TEXT_SECONDARY)
                })
                .child(label),
        )
        .child(
            div()
                .px(px(6.5))
                .py(px(1.5))
                .rounded(px(999.0))
                .bg(if active {
                    rgba(ACCENT_SOFT)
                } else {
                    rgb(BG_SURFACE)
                })
                .font(mono_font())
                .text_size(px(11.0))
                .text_color(if active {
                    rgb(ACCENT)
                } else {
                    rgb(TEXT_MUTED)
                })
                .child(format!("{count}")),
        )
}

fn render_reorder_icon_btn(
    id: impl Into<gpui::ElementId>,
    icon: IconName,
    enabled: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .when(enabled, |el| {
            el.cursor_pointer()
                .hover(|s| s.bg(rgb(BG_SURFACE_3)))
                .on_mouse_down(MouseButton::Left, on_click)
        })
        .w(px(24.0))
        .h(px(24.0))
        .rounded(px(5.0))
        .flex()
        .items_center()
        .justify_center()
        .child(svg_icon(
            icon,
            12.5,
            if enabled {
                rgb(TEXT_SECONDARY)
            } else {
                rgba(0x6a6a7055)
            },
        ))
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
}
