use std::{
    collections::HashMap,
    ops::Range,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gpui::{
    deferred, div, point, prelude::*, px, relative, rgb, rgba, Animation, AnimationExt,
    AnyElement, Context, ElementId, FontWeight, MouseButton, MouseDownEvent, SharedString, Window,
};
use stremio_core_runtime::{
    resolve_addon_for_request_base,
    stremio_core::{
        models::catalog_with_filters::{
            SelectableExtra, SelectableType, Selected as CatalogWithFiltersSelected,
        },
        runtime::msg::{Action, ActionCatalogWithFilters},
        types::{
            addon::{Descriptor, ResourceRequest},
            resource::MetaItemPreview,
        },
    },
    DesktopModelField,
};

use crate::{
    icons::{svg_icon, svg_icon_weighted, IconName, IconWeight},
    shell::AppShell,
    state::{
        extract_genres, extract_imdb_rating, AppRoute, BrowsingContextMenu, DiscoverCatalogOption,
        DiscoverDropdown, DiscoverViewMode, DiscoverViewSnapshot, FocusedInput, KbFocusableCard,
        LibraryStatusTab, QuickPeekTarget, ToastKind,
    },
    theme::{
        active_theme, mono_font, sentence_case_label, sp, tabular_nums, ui_sans, ThemePalette,
        RADIUS_CONTROL, RADIUS_PILL, RADIUS_POSTER,
    },
    ui::{
        choice_pill, click_handler, compute_app_grid_metrics, dropdown_trigger,
        full_height_page_container, glass_dropdown_option, glass_dropdown_panel,
        glass_segmented_bar, glass_segmented_item, glass_switch, hover_handler,
        infer_grid_poster_aspect, neutral_chip, primary_accent_button, render_empty_state,
        render_page_header, render_poster_card, search_field, toolbar_bar, DropdownTriggerProps,
        EmptyStateProps, PageHeader, PageWidth, PosterCardProps, PosterMetrics, SearchFieldProps,
    },
    virtual_grid::{slice_row_items, PosterAspect},
};

const SOURCE_RAIL_WIDTH_PX: f32 = 248.0;
const EXPANDED_ADDON_PREFIX: &str = "__exp__:";
const DROPDOWN_EXTRA_TYPES: &str = "__types__";
const DROPDOWN_FILTERS: &str = "__filters__";

#[derive(Clone, Debug, PartialEq, Eq)]
struct PartitionedDiscoverTypes {
    core: Vec<SelectableType>,
    extra: Vec<SelectableType>,
}

fn format_discover_type_label(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("movie") {
        "Movies".to_owned()
    } else if trimmed.eq_ignore_ascii_case("series") {
        "Series".to_owned()
    } else {
        sentence_case_label(trimmed)
    }
}

fn partition_discover_types(types: &[SelectableType]) -> PartitionedDiscoverTypes {
    let has_movie = types
        .iter()
        .any(|t| t.r#type.trim().eq_ignore_ascii_case("movie"));
    let has_series = types
        .iter()
        .any(|t| t.r#type.trim().eq_ignore_ascii_case("series"));

    let mut core = Vec::new();
    let mut extra = Vec::new();

    if has_movie || has_series {
        if let Some(movie) = types
            .iter()
            .find(|t| t.r#type.trim().eq_ignore_ascii_case("movie"))
        {
            core.push(movie.clone());
        }
        if let Some(series) = types
            .iter()
            .find(|t| t.r#type.trim().eq_ignore_ascii_case("series"))
        {
            core.push(series.clone());
        }

        let mut seen_movie = false;
        let mut seen_series = false;
        for t in types {
            if t.r#type.trim().eq_ignore_ascii_case("movie") && !seen_movie {
                seen_movie = true;
                continue;
            }
            if t.r#type.trim().eq_ignore_ascii_case("series") && !seen_series {
                seen_series = true;
                continue;
            }
            extra.push(t.clone());
        }
    } else {
        for (idx, t) in types.iter().enumerate() {
            if idx < 2 {
                core.push(t.clone());
            } else {
                extra.push(t.clone());
            }
        }
    }

    PartitionedDiscoverTypes { core, extra }
}

fn is_extra_filtered(extra: &SelectableExtra) -> bool {
    extra.options.iter().position(|o| o.selected).is_some_and(|pos| {
        if extra.is_required && pos == 0 {
            false
        } else {
            extra.options[pos]
                .value
                .as_deref()
                .map(str::trim)
                .is_some_and(|v| !v.is_empty())
        }
    })
}

fn active_extra_value_label(extra: &SelectableExtra) -> String {
    extra
        .options
        .iter()
        .find(|o| o.selected)
        .and_then(|o| o.value.as_deref())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("All")
        .to_owned()
}

fn collect_selectable_extra_options(
    extra: &SelectableExtra,
    default_req: Option<&ResourceRequest>,
) -> Vec<(String, bool, ResourceRequest)> {
    let has_explicit_all = extra.options.iter().any(|o| {
        o.value
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
    });
    let any_specific_selected = extra.options.iter().any(|o| {
        o.selected
            && o.value
                .as_deref()
                .map(str::trim)
                .is_some_and(|v| !v.is_empty())
    });

    let mut out = Vec::with_capacity(extra.options.len() + 1);
    if !has_explicit_all && !extra.is_required {
        if let Some(req) = default_req {
            out.push(("All".to_owned(), !any_specific_selected, req.clone()));
        }
    }
    for opt in &extra.options {
        let label = opt
            .value
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or("All")
            .to_owned();
        out.push((label, opt.selected, opt.request.clone()));
    }
    out
}

static SURPRISE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static HIDDEN_FOOTER_OPEN: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
struct AddonCatalogGroup {
    addon_name: String,
    items: Vec<DiscoverCatalogOption>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HiddenCatalogEntry {
    key: String,
    title: String,
    subtitle: Option<String>,
}

#[inline]
fn catalog_pin_key(req: &ResourceRequest) -> String {
    format!("{}::{}::{}", req.base, req.path.r#type, req.path.id)
}

#[inline]
fn normalize_addon_key(addon_name: &str) -> String {
    addon_name.trim().to_ascii_lowercase()
}

fn toggle_pinned_catalog_key(pinned: &mut Vec<String>, key: String) -> bool {
    if let Some(pos) = pinned.iter().position(|existing| existing == &key) {
        pinned.remove(pos);
        false
    } else {
        pinned.push(key);
        true
    }
}

fn move_pinned_catalog(pinned: &mut Vec<String>, key: &str, delta: isize) -> bool {
    if delta == 0 {
        return false;
    }
    let Some(idx) = pinned.iter().position(|existing| existing == key) else {
        return false;
    };
    let target = if delta < 0 {
        let step = delta.unsigned_abs();
        if idx == 0 {
            return false;
        }
        idx.saturating_sub(step)
    } else {
        let step = delta as usize;
        if idx + 1 >= pinned.len() {
            return false;
        }
        (idx + step).min(pinned.len() - 1)
    };
    if target == idx {
        return false;
    }
    let item = pinned.remove(idx);
    pinned.insert(target, item);
    true
}

fn move_visible_pinned_catalog(
    pinned: &mut Vec<String>,
    visible_keys: &[String],
    key: &str,
    delta: isize,
) -> bool {
    if delta == 0 {
        return false;
    }
    let Some(vis_idx) = visible_keys.iter().position(|existing| existing == key) else {
        return move_pinned_catalog(pinned, key, delta);
    };
    let target_vis_idx = if delta < 0 {
        let Some(prev) = vis_idx.checked_sub(1) else {
            return false;
        };
        prev
    } else {
        let next = vis_idx + 1;
        if next >= visible_keys.len() {
            return false;
        }
        next
    };

    let target_key = &visible_keys[target_vis_idx];
    let (Some(pos_a), Some(pos_b)) = (
        pinned.iter().position(|k| k == key),
        pinned.iter().position(|k| k == target_key),
    ) else {
        return false;
    };
    if pos_a == pos_b {
        return false;
    }
    pinned.swap(pos_a, pos_b);
    true
}

/// Toggles addon expansion in `collapsed_discover_addons` using inverted default-collapsed
/// semantics (`__exp__:<addon>` marks explicit expansion; bare `<addon>` marks explicit collapse
/// of the currently active addon). Returns `true` if the addon group is now expanded.
fn toggle_addon_group_expansion(
    collapsed: &mut Vec<String>,
    addon_name: &str,
    is_active_addon: bool,
) -> bool {
    let key = normalize_addon_key(addon_name);
    if key.is_empty() {
        return true;
    }
    let exp_token = format!("{EXPANDED_ADDON_PREFIX}{key}");
    let was_collapsed = is_addon_group_collapsed(collapsed, addon_name, false, is_active_addon);
    if was_collapsed {
        collapsed.retain(|existing| existing != &key);
        if !is_active_addon && !collapsed.iter().any(|existing| existing == &exp_token) {
            collapsed.push(exp_token);
        }
        true
    } else {
        collapsed.retain(|existing| existing != &exp_token);
        if is_active_addon && !collapsed.iter().any(|existing| existing == &key) {
            collapsed.push(key);
        }
        false
    }
}

fn is_addon_group_collapsed(
    collapsed: &[String],
    addon_name: &str,
    has_search_query: bool,
    is_active_addon: bool,
) -> bool {
    if has_search_query {
        return false;
    }
    let key = normalize_addon_key(addon_name);
    if key.is_empty() {
        return false;
    }
    let exp_token = format!("{EXPANDED_ADDON_PREFIX}{key}");
    if collapsed.iter().any(|existing| existing == &exp_token) {
        return false;
    }
    if is_active_addon {
        collapsed.iter().any(|existing| existing == &key)
    } else {
        true
    }
}

fn hide_catalog_key(hidden: &mut Vec<String>, key: String) -> bool {
    if hidden.iter().any(|existing| existing == &key) {
        return false;
    }
    hidden.push(key);
    true
}

fn restore_catalog_key(hidden: &mut Vec<String>, key: &str) -> bool {
    let before = hidden.len();
    hidden.retain(|existing| existing != key);
    hidden.len() != before
}

fn restore_all_catalog_keys(hidden: &mut Vec<String>) -> usize {
    let count = hidden.len();
    hidden.clear();
    count
}

fn partition_and_group_catalogs(
    catalogs: &[DiscoverCatalogOption],
    pinned_keys: &[String],
    hidden_keys: &[String],
    query_lower: &str,
) -> (Vec<DiscoverCatalogOption>, Vec<AddonCatalogGroup>) {
    let mut pinned_with_order: Vec<(usize, DiscoverCatalogOption)> = Vec::new();
    let mut groups: Vec<AddonCatalogGroup> = Vec::new();
    let mut group_indices: HashMap<String, usize> = HashMap::new();

    for opt in catalogs {
        let key = catalog_pin_key(&opt.catalog.request);
        if hidden_keys.iter().any(|hidden| hidden == &key) {
            continue;
        }

        if !query_lower.is_empty() {
            let title_match = opt.title.to_ascii_lowercase().contains(query_lower);
            let addon_match = opt
                .addon_name
                .as_deref()
                .is_some_and(|a| a.to_ascii_lowercase().contains(query_lower));
            if !title_match && !addon_match {
                continue;
            }
        }

        if let Some(pin_pos) = pinned_keys.iter().position(|k| k == &key) {
            pinned_with_order.push((pin_pos, opt.clone()));
            continue;
        }

        let addon_label = opt
            .addon_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Other Addons")
            .to_owned();
        let group_key = addon_label.to_ascii_lowercase();

        if let Some(&idx) = group_indices.get(&group_key) {
            groups[idx].items.push(opt.clone());
        } else {
            let idx = groups.len();
            group_indices.insert(group_key, idx);
            groups.push(AddonCatalogGroup {
                addon_name: addon_label,
                items: vec![opt.clone()],
            });
        }
    }

    pinned_with_order.sort_by_key(|(pos, _)| *pos);
    let pinned = pinned_with_order.into_iter().map(|(_, opt)| opt).collect();

    (pinned, groups)
}

fn resolve_hidden_catalog_entries(
    hidden_keys: &[String],
    snap_catalogs: &[DiscoverCatalogOption],
    installed_addons: &[Descriptor],
) -> Vec<HiddenCatalogEntry> {
    hidden_keys
        .iter()
        .map(|key| {
            if let Some(opt) = snap_catalogs
                .iter()
                .find(|c| catalog_pin_key(&c.catalog.request) == *key)
            {
                return HiddenCatalogEntry {
                    key: key.clone(),
                    title: opt.title.clone(),
                    subtitle: opt.addon_name.clone(),
                };
            }

            let mut parts = key.rsplitn(3, "::");
            let id_part = parts.next().unwrap_or(key.as_str());
            let type_part = parts.next();
            let base_part = parts.next();

            if let (Some(ty), Some(base_str)) = (type_part, base_part) {
                if let Ok(base_url) = url::Url::parse(base_str) {
                    if let Some(addon) = resolve_addon_for_request_base(installed_addons, &base_url)
                    {
                        let title = addon
                            .manifest
                            .catalogs
                            .iter()
                            .find(|mc| mc.id == id_part && mc.r#type == ty)
                            .and_then(|mc| mc.name.as_deref())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(sentence_case_label)
                            .unwrap_or_else(|| sentence_case_label(id_part));
                        return HiddenCatalogEntry {
                            key: key.clone(),
                            title,
                            subtitle: Some(format!(
                                "{} · {}",
                                addon.manifest.name,
                                sentence_case_label(ty)
                            )),
                        };
                    }
                }
                return HiddenCatalogEntry {
                    key: key.clone(),
                    title: sentence_case_label(id_part),
                    subtitle: Some(sentence_case_label(ty)),
                };
            }

            HiddenCatalogEntry {
                key: key.clone(),
                title: sentence_case_label(id_part),
                subtitle: None,
            }
        })
        .collect()
}

fn xorshift64(mut x: u64) -> u64 {
    if x == 0 {
        x = 0x9e37_79b9_7f4a_7c15;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

fn next_surprise_seed(len: usize) -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9_7f4a_7c15);
    let seq = SURPRISE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    xorshift64(
        nanos
            ^ seq.wrapping_mul(0x517c_c1b7_2722_0a95)
            ^ (len as u64).wrapping_mul(0x94d0_49bb_1331_11eb),
    )
}

fn pick_surprise_item_index(
    items: &[MetaItemPreview],
    statuses: &HashMap<String, LibraryStatusTab>,
    exclude_id: Option<&str>,
    seed: u64,
) -> Option<usize> {
    if items.is_empty() {
        return None;
    }

    let unwatched: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| statuses.get(&item.id).copied() != Some(LibraryStatusTab::Watched))
        .map(|(idx, _)| idx)
        .collect();

    let mut pool: Vec<usize> = if unwatched.is_empty() {
        (0..items.len()).collect()
    } else {
        unwatched
    };

    if let Some(ex) = exclude_id {
        if pool.len() > 1 {
            let filtered: Vec<usize> = pool
                .iter()
                .copied()
                .filter(|&idx| items[idx].id != ex)
                .collect();
            if !filtered.is_empty() {
                pool = filtered;
            }
        }
    }

    let chosen_slot = (xorshift64(seed) as usize) % pool.len();
    Some(pool[chosen_slot])
}

fn filter_unwatched_items(
    items: &[MetaItemPreview],
    statuses: &HashMap<String, LibraryStatusTab>,
) -> (Vec<MetaItemPreview>, usize) {
    let mut visible = Vec::with_capacity(items.len());
    let mut hidden_count = 0_usize;
    for item in items {
        if statuses.get(&item.id).copied() == Some(LibraryStatusTab::Watched) {
            hidden_count += 1;
        } else {
            visible.push(item.clone());
        }
    }
    (visible, hidden_count)
}

impl AppShell {
    pub fn render_discover_view(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.preferences.discover_view_mode != DiscoverViewMode::CatalogGrid {
            self.preferences.discover_view_mode = DiscoverViewMode::CatalogGrid;
            let _ = self.preferences.save();
        }
        if self.preferences.hidden_discover_catalogs.is_empty() {
            HIDDEN_FOOTER_OPEN.store(false, Ordering::Relaxed);
        }

        let theme = active_theme();
        let snap = self.cached_discover_snapshot();
        let statuses_arc = self.cached_library_statuses();

        let raw_viewport_w = f32::from(window.viewport_size().width);
        let viewport_w = if raw_viewport_w > 0.0 && raw_viewport_w.is_finite() {
            raw_viewport_w
        } else {
            self.viewport_size_px.0.max(1024.0)
        };

        let rail_open = self.preferences.discover_source_rail_open;
        let rail_width_px = if rail_open {
            SOURCE_RAIL_WIDTH_PX * theme.ui_scale
        } else {
            0.0
        };

        let source_rail_el = rail_open.then(|| self.render_discover_source_rail(&snap, &theme, cx));
        let main_canvas = self.render_discover_catalog_grid(
            &snap,
            &statuses_arc,
            viewport_w,
            rail_width_px,
            &theme,
            cx,
        );

        div()
            .id("discover-view-root")
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_row()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    let mut changed = false;
                    if this.discover_open_dropdown.is_some() {
                        this.discover_open_dropdown = None;
                        changed = true;
                    }
                    if this.focused_input == Some(FocusedInput::DiscoverCatalogSearch) {
                        this.focused_input = None;
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                    }
                }),
            )
            .when_some(source_rail_el, |root, rail| root.child(rail))
            .child(main_canvas)
    }

    fn render_discover_catalog_search_box(
        &self,
        box_id: &'static str,
        clear_id: &'static str,
        placeholder: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_raw_cat_query = !self.discover_catalog_search.is_empty();
        let cat_search_focused = self.focused_input == Some(FocusedInput::DiscoverCatalogSearch);
        let cat_search_display = self.format_input_with_caret(
            FocusedInput::DiscoverCatalogSearch,
            &self.discover_catalog_search,
        );

        search_field(
            SearchFieldProps::new(
                box_id,
                cat_search_display,
                placeholder,
                !has_raw_cat_query,
                cat_search_focused,
            )
            .clear_id(clear_id)
            .full_width()
            .on_focus(cx.listener(|this, _, _, cx| {
                this.focused_input = Some(FocusedInput::DiscoverCatalogSearch);
                cx.notify();
            }))
            .on_clear(cx.listener(|this, _, _, cx| {
                this.discover_catalog_search.clear();
                this.input_cursors
                    .remove(&FocusedInput::DiscoverCatalogSearch);
                this.input_selected_all
                    .remove(&FocusedInput::DiscoverCatalogSearch);
                cx.notify();
            })),
        )
        .into_any_element()
    }

    fn render_discover_source_rail(
        &self,
        snap: &DiscoverViewSnapshot,
        theme: &ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *theme;
        let cat_query_trimmed = self.discover_catalog_search.trim().to_ascii_lowercase();
        let has_cat_query = !cat_query_trimmed.is_empty();

        let active_addon_key = snap
            .catalogs
            .iter()
            .find(|c| c.catalog.selected)
            .or_else(|| snap.catalogs.first())
            .and_then(|c| c.addon_name.as_deref())
            .map(normalize_addon_key)
            .filter(|k| !k.is_empty());

        let (pinned_catalogs, addon_groups) = partition_and_group_catalogs(
            &snap.catalogs,
            &self.preferences.pinned_discover_catalogs,
            &self.preferences.hidden_discover_catalogs,
            &cat_query_trimmed,
        );

        let visible_pinned_keys: Arc<Vec<String>> = Arc::new(
            pinned_catalogs
                .iter()
                .map(|opt| catalog_pin_key(&opt.catalog.request))
                .collect(),
        );

        let fallback_next_catalog_req: Option<ResourceRequest> = snap
            .catalogs
            .iter()
            .filter(|opt| {
                let key = catalog_pin_key(&opt.catalog.request);
                !self
                    .preferences
                    .hidden_discover_catalogs
                    .iter()
                    .any(|hidden| hidden == &key)
            })
            .find(|opt| !opt.catalog.selected)
            .map(|opt| opt.catalog.request.clone());

        let search_box = self.render_discover_catalog_search_box(
            "discover-rail-search-box",
            "discover-rail-search-clear",
            "Filter catalogs...",
            cx,
        );

        let mut list_col = div().w_full().flex().flex_col().gap(sp(10.0));
        let has_any_visible_catalog = !pinned_catalogs.is_empty() || !addon_groups.is_empty();
        let pinned_len = pinned_catalogs.len();

        if !pinned_catalogs.is_empty() {
            list_col = list_col.child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(sp(3.0))
                    .child(
                        div()
                            .px(sp(6.0))
                            .pb(sp(2.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(sp(6.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(sp(5.0))
                                    .child(svg_icon_weighted(
                                        IconName::Pin,
                                        IconWeight::Fill,
                                        10.0 * theme.ui_scale,
                                        rgb(theme.text_muted),
                                    ))
                                    .child(
                                        div()
                                            .font(mono_font())
                                            .text_size(sp(10.0))
                                            .text_color(rgb(theme.text_muted))
                                            .child("PINNED"),
                                    ),
                            )
                            .child(
                                div()
                                    .font(mono_font())
                                    .font_features(tabular_nums())
                                    .text_size(sp(10.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(format!("{pinned_len}")),
                            ),
                    )
                    .children(pinned_catalogs.into_iter().enumerate().map(|(idx, opt)| {
                        self.render_source_rail_pinned_row(
                            ("discover-rail-pinned", idx),
                            opt,
                            idx,
                            pinned_len,
                            Arc::clone(&visible_pinned_keys),
                            &theme,
                            cx,
                        )
                    })),
            );
        }

        if !has_any_visible_catalog {
            let empty_msg = if snap.catalogs.is_empty() {
                "No catalogs installed yet."
            } else if has_cat_query {
                "No catalogs match your filter."
            } else {
                "All catalogs are hidden. Restore below."
            };
            list_col = list_col.child(
                div()
                    .px(sp(8.0))
                    .py(sp(12.0))
                    .text_size(sp(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(empty_msg),
            );
        } else {
            for (g_idx, group) in addon_groups.into_iter().enumerate() {
                let count = group.items.len();
                let addon_name_for_click = group.addon_name.clone();
                let is_active_addon = active_addon_key
                    .as_deref()
                    .is_some_and(|active| active == normalize_addon_key(&group.addon_name));
                let is_collapsed = is_addon_group_collapsed(
                    &self.preferences.collapsed_discover_addons,
                    &group.addon_name,
                    has_cat_query,
                    is_active_addon,
                );

                let header = div()
                    .id(("discover-rail-addon-hdr", g_idx))
                    .w_full()
                    .px(sp(6.0))
                    .py(sp(6.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sp(6.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            toggle_addon_group_expansion(
                                &mut this.preferences.collapsed_discover_addons,
                                &addon_name_for_click,
                                is_active_addon,
                            );
                            let _ = this.preferences.save();
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(sp(5.0))
                            .child(svg_icon(
                                if is_collapsed {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                },
                                11.0 * theme.ui_scale,
                                rgb(if is_active_addon || !is_collapsed {
                                    theme.text_secondary
                                } else {
                                    theme.text_muted
                                }),
                            ))
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(sp(12.0))
                                    .text_color(rgb(if is_active_addon || !is_collapsed {
                                        theme.text_primary
                                    } else {
                                        theme.text_secondary
                                    }))
                                    .child(group.addon_name),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .font(mono_font())
                            .font_features(tabular_nums())
                            .text_size(sp(10.5))
                            .text_color(rgb(theme.text_muted))
                            .child(format!("{count}")),
                    );

                let mut group_col = div().w_full().flex().flex_col().gap(sp(2.0)).child(header);
                if !is_collapsed {
                    group_col = group_col.children(group.items.into_iter().enumerate().map(
                        |(c_idx, opt)| {
                            let next_fallback = if opt.catalog.selected {
                                fallback_next_catalog_req.clone()
                            } else {
                                None
                            };
                            self.render_source_rail_addon_row(
                                ("discover-rail-cat", g_idx * 1000 + c_idx),
                                opt,
                                next_fallback,
                                &theme,
                                cx,
                            )
                        },
                    ));
                }

                list_col = list_col.child(group_col);
            }
        }

        let hidden_footer = self.render_source_rail_hidden_footer(snap, &theme, cx);

        div()
            .id("discover-source-rail")
            .w(sp(SOURCE_RAIL_WIDTH_PX))
            .h_full()
            .min_h(px(0.0))
            .flex_shrink_0()
            .bg(rgba(theme.glass_card_bg))
            .border_t_1()
            .border_r_1()
            .border_color(rgba(theme.glass_border))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_shrink_0()
                    .p(sp(12.0))
                    .border_b_1()
                    .border_color(rgba(theme.glass_divider))
                    .child(search_box),
            )
            .child(
                div()
                    .id("discover-source-rail-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_y_scroll()
                    .px(sp(8.0))
                    .py(sp(10.0))
                    .child(list_col),
            )
            .when_some(hidden_footer, |rail, footer| rail.child(footer))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_source_rail_pinned_row(
        &self,
        id: impl Into<ElementId>,
        opt: DiscoverCatalogOption,
        index: usize,
        pinned_len: usize,
        visible_pinned_keys: Arc<Vec<String>>,
        theme: &ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *theme;
        let row_id = Arc::new(id.into());
        let up_id = ElementId::NamedChild(Arc::clone(&row_id), SharedString::new_static("up"));
        let down_id = ElementId::NamedChild(Arc::clone(&row_id), SharedString::new_static("down"));
        let unpin_id =
            ElementId::NamedChild(Arc::clone(&row_id), SharedString::new_static("unpin"));
        let grp: SharedString = format!("discover-rail-pin-row-{row_id}").into();

        let selected = opt.catalog.selected;
        let req = opt.catalog.request.clone();
        let pin_key = catalog_pin_key(&req);
        let title_for_toast = opt.title.clone();

        let can_up = index > 0;
        let can_down = index + 1 < pinned_len;

        let up_keys = Arc::clone(&visible_pinned_keys);
        let up_key = pin_key.clone();
        let down_keys = visible_pinned_keys;
        let down_key = pin_key.clone();
        let unpin_key = pin_key;

        let mut hover_controls = div()
            .absolute()
            .right(sp(6.0))
            .opacity(0.0)
            .group_hover(grp.clone(), |s| s.opacity(1.0))
            .px(sp(3.0))
            .py(sp(2.0))
            .rounded(px(6.0))
            .bg(rgba(theme.glass_popover_bg))
            .border_1()
            .border_color(rgba(theme.glass_border_strong))
            .flex()
            .items_center()
            .gap(sp(2.0));

        if pinned_len > 1 {
            let mut up_btn = div()
                .id(up_id)
                .w(sp(20.0))
                .h(sp(20.0))
                .rounded(px(4.0))
                .flex()
                .items_center()
                .justify_center();

            if can_up {
                up_btn = up_btn
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if move_visible_pinned_catalog(
                                &mut this.preferences.pinned_discover_catalogs,
                                &up_keys,
                                &up_key,
                                -1,
                            ) {
                                let _ = this.preferences.save();
                                cx.notify();
                            }
                        }),
                    );
            } else {
                up_btn = up_btn.opacity(0.3).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                );
            }

            let mut down_btn = div()
                .id(down_id)
                .w(sp(20.0))
                .h(sp(20.0))
                .rounded(px(4.0))
                .flex()
                .items_center()
                .justify_center();

            if can_down {
                down_btn = down_btn
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if move_visible_pinned_catalog(
                                &mut this.preferences.pinned_discover_catalogs,
                                &down_keys,
                                &down_key,
                                1,
                            ) {
                                let _ = this.preferences.save();
                                cx.notify();
                            }
                        }),
                    );
            } else {
                down_btn = down_btn.opacity(0.3).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                );
            }

            hover_controls = hover_controls
                .child(up_btn.child(svg_icon(
                    IconName::ArrowUp,
                    11.0 * theme.ui_scale,
                    rgb(theme.text_secondary),
                )))
                .child(down_btn.child(svg_icon(
                    IconName::ArrowDown,
                    11.0 * theme.ui_scale,
                    rgb(theme.text_secondary),
                )));
        }

        hover_controls = hover_controls.child(
            div()
                .id(unpin_id)
                .cursor_pointer()
                .w(sp(20.0))
                .h(sp(20.0))
                .rounded(px(4.0))
                .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        toggle_pinned_catalog_key(
                            &mut this.preferences.pinned_discover_catalogs,
                            unpin_key.clone(),
                        );
                        let _ = this.preferences.save();
                        this.push_toast(
                            ToastKind::Info,
                            "Unpinned catalog",
                            Some(title_for_toast.clone()),
                            cx,
                        );
                        cx.notify();
                    }),
                )
                .child(svg_icon_weighted(
                    IconName::Pin,
                    IconWeight::Fill,
                    11.0 * theme.ui_scale,
                    rgb(theme.text_primary),
                )),
        );

        div()
            .id((*row_id).clone())
            .group(grp)
            .relative()
            .w_full()
            .px(sp(8.0))
            .py(sp(7.0))
            .rounded(px(RADIUS_POSTER))
            .cursor_pointer()
            .bg(if selected {
                rgba(theme.glass_row_selected_bg)
            } else {
                rgba(0x00000000)
            })
            .border_1()
            .border_color(if selected {
                rgba(theme.glass_border_strong)
            } else {
                rgba(0x00000000)
            })
            .hover(move |s| {
                s.bg(if selected {
                    rgba(theme.glass_row_selected_bg)
                } else {
                    rgba(theme.glass_row_hover_bg)
                })
                .border_color(if selected {
                    rgba(theme.glass_border_strong)
                } else {
                    rgba(theme.glass_border)
                })
            })
            .flex()
            .items_center()
            .gap(sp(6.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.apply_discover_selection(
                        Some(CatalogWithFiltersSelected {
                            request: req.clone(),
                        }),
                        cx,
                    );
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(sp(1.0))
                    .child(
                        div()
                            .w_full()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(sp(6.0))
                            .when(selected, |row| {
                                row.child(
                                    div()
                                        .w(sp(5.0))
                                        .h(sp(5.0))
                                        .rounded_full()
                                        .flex_shrink_0()
                                        .bg(rgb(theme.text_primary)),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .font(ui_sans(if selected {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::MEDIUM
                                    }))
                                    .text_size(sp(12.5))
                                    .text_color(rgb(if selected {
                                        theme.text_primary
                                    } else {
                                        theme.text_secondary
                                    }))
                                    .child(opt.title),
                            ),
                    )
                    .when_some(opt.addon_name, |col, addon| {
                        col.child(
                            div()
                                .truncate()
                                .font(ui_sans(FontWeight::NORMAL))
                                .text_size(sp(10.5))
                                .text_color(rgb(theme.text_muted))
                                .child(addon),
                        )
                    }),
            )
            .child(hover_controls)
            .into_any_element()
    }

    fn render_source_rail_addon_row(
        &self,
        id: impl Into<ElementId>,
        opt: DiscoverCatalogOption,
        fallback_next_req: Option<ResourceRequest>,
        theme: &ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *theme;
        let row_id = Arc::new(id.into());
        let pin_id = ElementId::NamedChild(Arc::clone(&row_id), SharedString::new_static("pin"));
        let hide_id = ElementId::NamedChild(Arc::clone(&row_id), SharedString::new_static("hide"));
        let grp: SharedString = format!("discover-rail-row-{row_id}").into();

        let selected = opt.catalog.selected;
        let req = opt.catalog.request.clone();
        let cat_key = catalog_pin_key(&req);
        let pin_key = cat_key.clone();
        let hide_key = cat_key;
        let pin_toast_title = opt.title.clone();
        let hide_toast_title = opt.title.clone();

        let hover_controls = div()
            .absolute()
            .right(sp(6.0))
            .opacity(0.0)
            .group_hover(grp.clone(), |s| s.opacity(1.0))
            .px(sp(3.0))
            .py(sp(2.0))
            .rounded(px(6.0))
            .bg(rgba(theme.glass_popover_bg))
            .border_1()
            .border_color(rgba(theme.glass_border_strong))
            .flex()
            .items_center()
            .gap(sp(2.0))
            .child(
                div()
                    .id(pin_id)
                    .cursor_pointer()
                    .w(sp(20.0))
                    .h(sp(20.0))
                    .rounded(px(4.0))
                    .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            let now_pinned = toggle_pinned_catalog_key(
                                &mut this.preferences.pinned_discover_catalogs,
                                pin_key.clone(),
                            );
                            let _ = this.preferences.save();
                            this.push_toast(
                                ToastKind::Info,
                                if now_pinned {
                                    "Pinned catalog"
                                } else {
                                    "Unpinned catalog"
                                },
                                Some(pin_toast_title.clone()),
                                cx,
                            );
                            cx.notify();
                        }),
                    )
                    .child(svg_icon_weighted(
                        IconName::Pin,
                        IconWeight::Regular,
                        11.0 * theme.ui_scale,
                        rgb(theme.text_secondary),
                    )),
            )
            .child(
                div()
                    .id(hide_id)
                    .cursor_pointer()
                    .w(sp(20.0))
                    .h(sp(20.0))
                    .rounded(px(4.0))
                    .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if hide_catalog_key(
                                &mut this.preferences.hidden_discover_catalogs,
                                hide_key.clone(),
                            ) {
                                let _ = this.preferences.save();
                                this.push_toast(
                                    ToastKind::Info,
                                    "Hidden catalog",
                                    Some(hide_toast_title.clone()),
                                    cx,
                                );
                                if let Some(next_req) = fallback_next_req.clone() {
                                    this.apply_discover_selection(
                                        Some(CatalogWithFiltersSelected { request: next_req }),
                                        cx,
                                    );
                                    return;
                                }
                                cx.notify();
                            }
                        }),
                    )
                    .child(svg_icon_weighted(
                        IconName::EyeOff,
                        IconWeight::Regular,
                        11.0 * theme.ui_scale,
                        rgb(theme.text_secondary),
                    )),
            );

        div()
            .id((*row_id).clone())
            .group(grp)
            .relative()
            .w_full()
            .px(sp(8.0))
            .py(sp(7.0))
            .rounded(px(RADIUS_POSTER))
            .cursor_pointer()
            .bg(if selected {
                rgba(theme.glass_row_selected_bg)
            } else {
                rgba(0x00000000)
            })
            .border_1()
            .border_color(if selected {
                rgba(theme.glass_border_strong)
            } else {
                rgba(0x00000000)
            })
            .hover(move |s| {
                s.bg(if selected {
                    rgba(theme.glass_row_selected_bg)
                } else {
                    rgba(theme.glass_row_hover_bg)
                })
                .border_color(if selected {
                    rgba(theme.glass_border_strong)
                } else {
                    rgba(theme.glass_border)
                })
            })
            .flex()
            .items_center()
            .gap(sp(6.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.apply_discover_selection(
                        Some(CatalogWithFiltersSelected {
                            request: req.clone(),
                        }),
                        cx,
                    );
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .gap(sp(6.0))
                    .when(selected, |row| {
                        row.child(
                            div()
                                .w(sp(5.0))
                                .h(sp(5.0))
                                .rounded_full()
                                .flex_shrink_0()
                                .bg(rgb(theme.text_primary)),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .font(ui_sans(if selected {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            }))
                            .text_size(sp(12.5))
                            .text_color(rgb(if selected {
                                theme.text_primary
                            } else {
                                theme.text_secondary
                            }))
                            .child(opt.title),
                    ),
            )
            .child(hover_controls)
            .into_any_element()
    }

    fn render_source_rail_hidden_footer(
        &self,
        snap: &DiscoverViewSnapshot,
        theme: &ThemePalette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.preferences.hidden_discover_catalogs.is_empty() {
            return None;
        }

        let theme = *theme;
        let is_open = HIDDEN_FOOTER_OPEN.load(Ordering::Relaxed);
        let hidden_entries = self.core.with_model(|model| {
            resolve_hidden_catalog_entries(
                &self.preferences.hidden_discover_catalogs,
                &snap.catalogs,
                &model.ctx.profile.addons,
            )
        });
        let hidden_count = hidden_entries.len();

        let toggle_row = div()
            .id("discover-hidden-footer-toggle")
            .cursor_pointer()
            .w_full()
            .px(sp(8.0))
            .py(sp(6.0))
            .rounded(px(RADIUS_POSTER))
            .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
            .flex()
            .items_center()
            .justify_between()
            .gap(sp(6.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                    let next = !HIDDEN_FOOTER_OPEN.load(Ordering::Relaxed);
                    HIDDEN_FOOTER_OPEN.store(next, Ordering::Relaxed);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(6.0))
                    .min_w(px(0.0))
                    .child(svg_icon(
                        IconName::EyeOff,
                        12.0 * theme.ui_scale,
                        rgb(theme.text_muted),
                    ))
                    .child(
                        div()
                            .truncate()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(sp(11.5))
                            .text_color(rgb(theme.text_secondary))
                            .child(format!(
                                "{hidden_count} hidden · {}",
                                if is_open { "Hide" } else { "Show" }
                            )),
                    ),
            )
            .child(svg_icon(
                if is_open {
                    IconName::CaretUp
                } else {
                    IconName::ChevronDown
                },
                11.5 * theme.ui_scale,
                rgb(theme.text_muted),
            ));

        let drawer = is_open.then(|| {
            div()
                .w_full()
                .pt(sp(6.0))
                .flex()
                .flex_col()
                .gap(sp(6.0))
                .child(
                    div()
                        .px(sp(4.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(sp(6.0))
                        .child(
                            div()
                                .font(mono_font())
                                .text_size(sp(10.0))
                                .text_color(rgb(theme.text_muted))
                                .child("HIDDEN CATALOGS"),
                        )
                        .child(
                            div()
                                .id("discover-hidden-restore-all")
                                .cursor_pointer()
                                .px(sp(7.0))
                                .py(sp(3.0))
                                .rounded(px(RADIUS_PILL))
                                .bg(rgba(theme.glass_trough_bg))
                                .border_1()
                                .border_color(rgba(theme.glass_border))
                                .hover(move |s| {
                                    s.bg(rgba(theme.glass_row_hover_bg))
                                        .border_color(rgba(theme.glass_border_strong))
                                        .text_color(rgb(theme.text_primary))
                                })
                                .font(ui_sans(FontWeight::MEDIUM))
                                .text_size(sp(10.5))
                                .text_color(rgb(theme.text_secondary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        let restored = restore_all_catalog_keys(
                                            &mut this.preferences.hidden_discover_catalogs,
                                        );
                                        HIDDEN_FOOTER_OPEN.store(false, Ordering::Relaxed);
                                        if restored > 0 {
                                            let _ = this.preferences.save();
                                            this.push_toast(
                                                ToastKind::Success,
                                                "Restored all hidden catalogs",
                                                Some(format!("{restored} restored")),
                                                cx,
                                            );
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child("Restore all"),
                        ),
                )
                .child(
                    div()
                        .id("discover-hidden-list-scroll")
                        .w_full()
                        .max_h(sp(176.0))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap(sp(3.0))
                        .children(hidden_entries.into_iter().enumerate().map(|(idx, entry)| {
                            let restore_key = entry.key;
                            let toast_title = entry.title.clone();
                            div()
                                .id(("discover-hidden-entry", idx))
                                .w_full()
                                .px(sp(6.0))
                                .py(sp(5.0))
                                .rounded(px(RADIUS_POSTER))
                                .bg(rgba(theme.glass_trough_bg))
                                .border_1()
                                .border_color(rgba(theme.glass_border))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(sp(6.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .flex()
                                        .flex_col()
                                        .gap(sp(1.0))
                                        .child(
                                            div()
                                                .truncate()
                                                .font(ui_sans(FontWeight::MEDIUM))
                                                .text_size(sp(11.5))
                                                .text_color(rgb(theme.text_primary))
                                                .child(entry.title),
                                        )
                                        .when_some(entry.subtitle, |col, sub| {
                                            col.child(
                                                div()
                                                    .truncate()
                                                    .text_size(sp(10.0))
                                                    .text_color(rgb(theme.text_muted))
                                                    .child(sub),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .id(("discover-hidden-restore", idx))
                                        .cursor_pointer()
                                        .flex_shrink_0()
                                        .px(sp(7.0))
                                        .py(sp(3.0))
                                        .rounded(px(RADIUS_PILL))
                                        .bg(rgba(theme.glass_card_bg))
                                        .border_1()
                                        .border_color(rgba(theme.glass_border))
                                        .hover(move |s| {
                                            s.bg(rgba(theme.glass_row_hover_bg))
                                                .border_color(rgba(theme.glass_border_strong))
                                                .text_color(rgb(theme.text_primary))
                                        })
                                        .font(ui_sans(FontWeight::MEDIUM))
                                        .text_size(sp(10.5))
                                        .text_color(rgb(theme.text_secondary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                if restore_catalog_key(
                                                    &mut this.preferences.hidden_discover_catalogs,
                                                    &restore_key,
                                                ) {
                                                    if this
                                                        .preferences
                                                        .hidden_discover_catalogs
                                                        .is_empty()
                                                    {
                                                        HIDDEN_FOOTER_OPEN
                                                            .store(false, Ordering::Relaxed);
                                                    }
                                                    let _ = this.preferences.save();
                                                    this.push_toast(
                                                        ToastKind::Info,
                                                        "Restored catalog",
                                                        Some(toast_title.clone()),
                                                        cx,
                                                    );
                                                    cx.notify();
                                                }
                                            }),
                                        )
                                        .child("Restore"),
                                )
                        })),
                )
        });

        Some(
            div()
                .flex_shrink_0()
                .w_full()
                .px(sp(8.0))
                .py(sp(8.0))
                .border_t_1()
                .border_color(rgba(theme.glass_divider))
                .flex()
                .flex_col()
                .child(toggle_row)
                .when_some(drawer, |el, d| el.child(d))
                .into_any_element(),
        )
    }

    fn render_discover_catalog_grid(
        &mut self,
        snap: &DiscoverViewSnapshot,
        statuses_arc: &Arc<HashMap<String, LibraryStatusTab>>,
        viewport_w: f32,
        rail_width_px: f32,
        theme: &ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *theme;
        let rail_open = self.preferences.discover_source_rail_open;
        let hide_watched = self.preferences.discover_hide_watched;

        let raw_items_arc = Arc::clone(&snap.items);
        let raw_total_items = raw_items_arc.len();
        let (items_arc, hidden_watched_count) = if hide_watched {
            let (filtered, hidden) = filter_unwatched_items(&raw_items_arc, statuses_arc);
            (Arc::new(filtered), hidden)
        } else {
            (raw_items_arc, 0_usize)
        };
        let total_items = items_arc.len();

        let genre_extra = snap
            .extras
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case("genre"))
            .cloned();

        let selected_catalog_opt = snap
            .catalogs
            .iter()
            .find(|c| c.catalog.selected)
            .or_else(|| snap.catalogs.first());
        let selected_catalog_default_request =
            selected_catalog_opt.map(|c| c.catalog.request.clone());
        let active_catalog_title = selected_catalog_opt
            .map(|c| c.title.clone())
            .unwrap_or_else(|| snap.active_catalog_display.clone());
        let active_catalog_addon = selected_catalog_opt.and_then(|c| c.addon_name.clone());

        let non_genre_extras: Vec<SelectableExtra> = snap
            .extras
            .iter()
            .filter(|e| !e.name.eq_ignore_ascii_case("genre") && !e.options.is_empty())
            .cloned()
            .collect();

        let non_genre_filtered_count = non_genre_extras
            .iter()
            .filter(|extra| is_extra_filtered(extra))
            .count();
        let has_genre_filter = genre_extra.as_ref().is_some_and(is_extra_filtered);
        let has_catalog_filters = has_genre_filter || non_genre_filtered_count > 0;
        let active_filter_count =
            usize::from(hide_watched) + usize::from(has_genre_filter) + non_genre_filtered_count;
        let has_active_filters = active_filter_count > 0;

        let grid_aspect = infer_grid_poster_aspect(items_arc.iter().map(|i| &i.poster_shape));
        let poster_metrics = PosterMetrics::from_theme(&theme);
        let gap_px = poster_metrics.gap_px;
        let metrics = compute_app_grid_metrics(
            total_items,
            viewport_w,
            rail_width_px,
            grid_aspect,
            &theme,
        );
        let columns = metrics.columns;
        let row_count = metrics.layout.row_count;
        let row_stride_px = metrics.row_stride_px;
        let card_width_px = metrics.card_width_px;
        let image_height_px = metrics.image_height_px;

        let kb_items: Vec<KbFocusableCard> = items_arc
            .iter()
            .enumerate()
            .map(|(idx, item)| KbFocusableCard {
                route: AppRoute::MetaDetails {
                    type_name: item.r#type.clone(),
                    id: item.id.clone(),
                    video_id: None,
                },
                row: idx / columns,
                col: idx % columns,
            })
            .collect();
        self.set_kb_focus_items(kb_items);

        let is_loading = snap.is_loading;
        let has_next_page = snap.has_next_page;

        let count_badge = (total_items > 0).then(|| {
            SharedString::from(format!(
                "{total_items} {}",
                if total_items == 1 { "title" } else { "titles" }
            ))
        });
        let header_subtitle: Option<SharedString> = if !active_catalog_title.is_empty() {
            Some(match active_catalog_addon.as_deref() {
                Some(addon) if !addon.is_empty() => format!("{active_catalog_title} · {addon}").into(),
                _ => active_catalog_title.clone().into(),
            })
        } else {
            None
        };
        let header_right = if is_loading {
            Some(neutral_chip("Loading…").into_any_element())
        } else if hide_watched && hidden_watched_count > 0 {
            Some(neutral_chip(format!("{hidden_watched_count} watched hidden")).into_any_element())
        } else {
            None
        };
        let page_header = render_page_header(
            PageHeader::new("Discover")
                .count_badge(count_badge)
                .subtitle(header_subtitle)
                .right_slot(header_right),
        );

        let toolbar = self.render_discover_toolbar(
            snap,
            &active_catalog_title,
            active_catalog_addon.as_deref(),
            selected_catalog_default_request.clone(),
            genre_extra.as_ref(),
            has_genre_filter,
            &non_genre_extras,
            hide_watched,
            hidden_watched_count,
            active_filter_count,
            has_catalog_filters,
            &items_arc,
            statuses_arc,
            rail_open,
            &theme,
            cx,
        );

        let statuses_for_grid = Arc::clone(statuses_arc);
        let grid_body = if is_loading && raw_total_items == 0 {
            render_discover_skeleton_grid(
                columns,
                image_height_px,
                row_stride_px,
                gap_px,
                &theme,
            )
        } else if total_items == 0 {
            render_discover_empty_state(
                has_active_filters,
                hide_watched && hidden_watched_count > 0,
                selected_catalog_default_request,
                cx,
            )
        } else {
            gpui::uniform_list(
                "discover-poster-grid",
                row_count,
                cx.processor(move |this, visible_rows: Range<usize>, _window, cx| {
                    if row_count > 0
                        && !visible_rows.is_empty()
                        && visible_rows.end + 2 >= row_count
                        && has_next_page
                        && !is_loading
                        && this.discover_last_next_page_len.get() != Some(raw_total_items)
                    {
                        this.discover_last_next_page_len.set(Some(raw_total_items));
                        this.core.dispatch(
                            Action::CatalogWithFilters(ActionCatalogWithFilters::LoadNextPage),
                            Some(DesktopModelField::Discover),
                        );
                    }

                    let kb_focus_index = this.kb_focus_index;

                    visible_rows
                        .map(|row_idx| {
                            let range = slice_row_items(total_items, columns, row_idx);
                            let empty_slots = columns.saturating_sub(range.len());

                            div()
                                .id(("discover-row", row_idx))
                                .w_full()
                                .h(px(row_stride_px))
                                .pb(px(gap_px))
                                .flex()
                                .items_start()
                                .gap(px(gap_px))
                                .children(range.map(|item_idx| {
                                    let item = items_arc[item_idx].clone();
                                    let st = statuses_for_grid.get(&item.id).copied();
                                    let is_focused = kb_focus_index == Some(item_idx);
                                    let card_aspect =
                                        PosterAspect::from_stremio_poster_shape(&item.poster_shape);
                                    let card_image_h = if card_aspect == grid_aspect {
                                        image_height_px
                                    } else {
                                        (card_width_px * card_aspect.height_over_width())
                                            .ceil()
                                            .min(image_height_px)
                                    };

                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .h_full()
                                        .child(this.render_discover_poster_card(
                                            ("discover-card", item_idx),
                                            item,
                                            st,
                                            card_aspect,
                                            None,
                                            card_image_h,
                                            is_focused,
                                            cx,
                                        ))
                                }))
                                .children(
                                    (0..empty_slots)
                                        .map(|_| div().flex_1().min_w(px(0.0))),
                                )
                        })
                        .collect()
                }),
            )
            .track_scroll(&self.discover_grid_scroll_handle)
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .into_any_element()
        };

        div()
            .id("discover-grid-canvas")
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(
                full_height_page_container(PageWidth::Wide, viewport_w)
                    .child(page_header)
                    .child(toolbar)
                    .child(grid_body),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_discover_toolbar(
        &self,
        snap: &DiscoverViewSnapshot,
        active_catalog_title: &str,
        active_catalog_addon: Option<&str>,
        default_catalog_req: Option<ResourceRequest>,
        genre_extra: Option<&SelectableExtra>,
        has_genre_filter: bool,
        non_genre_extras: &[SelectableExtra],
        hide_watched: bool,
        hidden_watched_count: usize,
        active_filter_count: usize,
        has_catalog_filters: bool,
        surprise_items: &Arc<Vec<MetaItemPreview>>,
        surprise_statuses: &Arc<HashMap<String, LibraryStatusTab>>,
        rail_open: bool,
        theme: &ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *theme;
        let type_cluster = self.render_discover_type_cluster(&snap.types, cx);
        let catalog_dd = self.render_discover_catalog_dropdown(
            snap,
            active_catalog_title,
            active_catalog_addon,
            &theme,
            cx,
        );
        let genre_dd = genre_extra
            .filter(|g| !g.options.is_empty())
            .map(|g| {
                self.render_discover_genre_dropdown(
                    g,
                    has_genre_filter,
                    default_catalog_req.clone(),
                    cx,
                )
            });
        let filters_dd = self.render_discover_filters_dropdown(
            non_genre_extras,
            default_catalog_req,
            hide_watched,
            hidden_watched_count,
            active_filter_count,
            has_catalog_filters,
            surprise_items,
            surprise_statuses,
            &theme,
            cx,
        );
        let sources_btn = self.render_source_rail_toggle_button(rail_open, &theme, cx);

        let left_and_center = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(sp(10.0))
            .min_w(px(0.0))
            .when_some(type_cluster, |row, tc| {
                row.child(tc).child(
                    div()
                        .w(px(1.0))
                        .h(sp(18.0))
                        .flex_shrink_0()
                        .bg(rgba(theme.glass_divider)),
                )
            })
            .child(catalog_dd)
            .when_some(genre_dd, |row, gd| row.child(gd));

        let right_cluster = div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(sp(8.0))
            .child(filters_dd)
            .child(sources_btn);

        toolbar_bar()
            .child(left_and_center)
            .child(right_cluster)
            .into_any_element()
    }

    fn render_discover_type_cluster(
        &self,
        types: &[SelectableType],
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let partitioned = partition_discover_types(types);
        if partitioned.core.is_empty() && partitioned.extra.is_empty() {
            return None;
        }

        let dd_types = DiscoverDropdown::Extra(DROPDOWN_EXTRA_TYPES.to_owned());
        let more_open = self.discover_open_dropdown.as_ref() == Some(&dd_types);

        let core_bar = (!partitioned.core.is_empty()).then(|| {
            glass_segmented_bar("discover-type-segmented").children(
                partitioned.core.into_iter().enumerate().map(|(idx, t)| {
                    let req = t.request;
                    let label = format_discover_type_label(&t.r#type);
                    glass_segmented_item(
                        ("discover-core-type", idx),
                        label,
                        t.selected,
                        cx.listener(move |this, _, _, cx| {
                            this.apply_discover_selection(
                                Some(CatalogWithFiltersSelected {
                                    request: req.clone(),
                                }),
                                cx,
                            );
                        }),
                    )
                }),
            )
        });

        let more_dropdown = (!partitioned.extra.is_empty()).then(|| {
            let selected_extra = partitioned.extra.iter().find(|t| t.selected);
            let more_is_active = selected_extra.is_some();
            let more_label = selected_extra
                .map(|t| format_discover_type_label(&t.r#type))
                .unwrap_or_else(|| "More".to_owned());

            div()
                .relative()
                .child(dropdown_trigger(
                    DropdownTriggerProps::new(
                        "discover-dd-more-types",
                        more_label,
                        more_open,
                        cx.listener(move |this, _, _, cx| {
                            if this.discover_open_dropdown.as_ref() == Some(&dd_types) {
                                this.discover_open_dropdown = None;
                            } else {
                                this.discover_open_dropdown = Some(dd_types.clone());
                                if this.focused_input == Some(FocusedInput::DiscoverCatalogSearch) {
                                    this.focused_input = None;
                                }
                            }
                            cx.notify();
                        }),
                    )
                    .is_active(more_is_active),
                ))
                .when(more_open, |wrap| {
                    wrap.child(
                        deferred(
                            glass_dropdown_panel("discover-menu-more-types", 210.0)
                                .absolute()
                                .top(sp(42.0))
                                .left_0()
                                .child(
                                    div()
                                        .id("discover-more-types-scroll")
                                        .w_full()
                                        .max_h(sp(280.0))
                                        .overflow_y_scroll()
                                        .flex()
                                        .flex_col()
                                        .gap(sp(2.0))
                                        .children(
                                            partitioned.extra.into_iter().enumerate().map(
                                                |(idx, t)| {
                                                    let req = t.request;
                                                    let label =
                                                        format_discover_type_label(&t.r#type);
                                                    glass_dropdown_option(
                                                        ("discover-opt-extra-type", idx),
                                                        label,
                                                        None,
                                                        t.selected,
                                                        cx.listener(move |this, _, _, cx| {
                                                            this.apply_discover_selection(
                                                                Some(CatalogWithFiltersSelected {
                                                                    request: req.clone(),
                                                                }),
                                                                cx,
                                                            );
                                                        }),
                                                    )
                                                },
                                            ),
                                        ),
                                ),
                        )
                        .with_priority(10),
                    )
                })
        });

        Some(
            div()
                .flex()
                .items_center()
                .gap(sp(6.0))
                .when_some(core_bar, |row, bar| row.child(bar))
                .when_some(more_dropdown, |row, dd| row.child(dd))
                .into_any_element(),
        )
    }

    fn render_discover_catalog_dropdown(
        &self,
        snap: &DiscoverViewSnapshot,
        active_catalog_title: &str,
        active_catalog_addon: Option<&str>,
        theme: &ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *theme;
        let is_open = self.discover_open_dropdown == Some(DiscoverDropdown::Catalog);
        let title_owned = active_catalog_title.to_owned();
        let addon_owned = active_catalog_addon.map(str::to_owned);

        let cat_query_trimmed = self.discover_catalog_search.trim().to_ascii_lowercase();
        let has_cat_query = !cat_query_trimmed.is_empty();

        let (pinned_catalogs, addon_groups) = partition_and_group_catalogs(
            &snap.catalogs,
            &self.preferences.pinned_discover_catalogs,
            &self.preferences.hidden_discover_catalogs,
            &cat_query_trimmed,
        );

        let popover = is_open.then(|| {
            let search_box = self.render_discover_catalog_search_box(
                "discover-popover-search-box",
                "discover-popover-search-clear",
                "Search catalogs...",
                cx,
            );

            let mut list_body = div().w_full().flex().flex_col().gap(sp(8.0));
            let pinned_empty = pinned_catalogs.is_empty();

            if !pinned_empty {
                list_body = list_body.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(sp(2.0))
                        .child(
                            div()
                                .px(sp(10.0))
                                .pt(sp(2.0))
                                .pb(sp(2.0))
                                .font(mono_font())
                                .text_size(sp(10.0))
                                .text_color(rgb(theme.text_muted))
                                .child("PINNED"),
                        )
                        .children(pinned_catalogs.into_iter().enumerate().map(|(idx, opt)| {
                            let req = opt.catalog.request;
                            glass_dropdown_option(
                                ("discover-pop-pin", idx),
                                opt.title,
                                opt.addon_name.map(SharedString::from),
                                opt.catalog.selected,
                                cx.listener(move |this, _, _, cx| {
                                    this.apply_discover_selection(
                                        Some(CatalogWithFiltersSelected {
                                            request: req.clone(),
                                        }),
                                        cx,
                                    );
                                }),
                            )
                        })),
                );
            }

            let render_group_section = |g_idx: usize,
                                        id_prefix: &'static str,
                                        group: AddonCatalogGroup,
                                        cx: &mut Context<Self>| {
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(sp(2.0))
                    .child(
                        div()
                            .px(sp(10.0))
                            .pt(sp(2.0))
                            .pb(sp(2.0))
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_size(sp(11.0))
                            .text_color(rgb(theme.text_muted))
                            .child(group.addon_name),
                    )
                    .children(group.items.into_iter().enumerate().map(|(c_idx, opt)| {
                        let req = opt.catalog.request;
                        glass_dropdown_option(
                            (id_prefix, g_idx * 1000 + c_idx),
                            opt.title,
                            None,
                            opt.catalog.selected,
                            cx.listener(move |this, _, _, cx| {
                                this.apply_discover_selection(
                                    Some(CatalogWithFiltersSelected {
                                        request: req.clone(),
                                    }),
                                    cx,
                                );
                            }),
                        )
                    }))
            };

            if has_cat_query {
                if pinned_empty && addon_groups.is_empty() {
                    list_body = list_body.child(
                        div()
                            .px(sp(10.0))
                            .py(sp(16.0))
                            .text_size(sp(12.5))
                            .text_color(rgb(theme.text_muted))
                            .child("No catalogs match your search."),
                    );
                } else {
                    for (g_idx, group) in addon_groups.into_iter().enumerate() {
                        list_body = list_body.child(render_group_section(
                            g_idx,
                            "discover-pop-search-cat",
                            group,
                            cx,
                        ));
                    }
                }
            } else {
                let active_addon_key = active_catalog_addon
                    .map(normalize_addon_key)
                    .filter(|k| !k.is_empty());
                let total_groups = addon_groups.len();
                let current_group_idx = active_addon_key
                    .as_deref()
                    .and_then(|active_key| {
                        addon_groups
                            .iter()
                            .position(|g| normalize_addon_key(&g.addon_name) == active_key)
                    })
                    .or((pinned_empty && total_groups > 0).then_some(0));

                let mut remaining_groups = total_groups;
                if let Some(group_idx) = current_group_idx {
                    let group = addon_groups.into_iter().nth(group_idx).unwrap();
                    remaining_groups = total_groups.saturating_sub(1);
                    list_body = list_body.child(render_group_section(
                        0,
                        "discover-pop-cur-cat",
                        group,
                        cx,
                    ));
                } else if pinned_empty {
                    list_body = list_body.child(
                        div()
                            .px(sp(10.0))
                            .py(sp(14.0))
                            .text_size(sp(12.5))
                            .text_color(rgb(theme.text_muted))
                            .child("No catalogs available."),
                    );
                }

                if remaining_groups > 0 {
                    list_body = list_body.child(
                        div()
                            .px(sp(10.0))
                            .py(sp(4.0))
                            .text_size(sp(11.0))
                            .text_color(rgb(theme.text_muted))
                            .child(format!(
                                "{remaining_groups} more {} — search above or open sources",
                                if remaining_groups == 1 { "addon" } else { "addons" }
                            )),
                    );
                }
            }

            let manage_sources_btn = div()
                .id("discover-popover-manage-sources")
                .cursor_pointer()
                .w_full()
                .h(sp(34.0))
                .px(sp(10.0))
                .rounded(px(RADIUS_CONTROL))
                .hover(move |s| {
                    s.bg(rgba(theme.glass_row_hover_bg))
                        .text_color(rgb(theme.text_primary))
                })
                .flex()
                .items_center()
                .justify_between()
                .font(ui_sans(FontWeight::MEDIUM))
                .text_size(sp(12.5))
                .text_color(rgb(theme.text_secondary))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.preferences.discover_source_rail_open = true;
                        let _ = this.preferences.save();
                        this.discover_open_dropdown = None;
                        if this.focused_input == Some(FocusedInput::DiscoverCatalogSearch) {
                            this.focused_input = None;
                        }
                        cx.notify();
                    }),
                )
                .child("Manage all sources →");

            deferred(
                glass_dropdown_panel("discover-menu-catalog", 340.0)
                    .absolute()
                    .top(sp(42.0))
                    .left_0()
                    .gap(sp(6.0))
                    .child(search_box)
                    .child(
                        div()
                            .id("discover-popover-cat-scroll")
                            .w_full()
                            .max_h(sp(300.0))
                            .overflow_y_scroll()
                            .child(list_body),
                    )
                    .child(
                        div()
                            .w_full()
                            .pt(sp(4.0))
                            .border_t_1()
                            .border_color(rgba(theme.glass_divider))
                            .child(manage_sources_btn),
                    ),
            )
            .with_priority(10)
        });

        div()
            .relative()
            .child(dropdown_trigger(
                DropdownTriggerProps::new(
                    "discover-dd-catalog",
                    title_owned,
                    is_open,
                    cx.listener(|this, _, _, cx| {
                        if this.discover_open_dropdown == Some(DiscoverDropdown::Catalog) {
                            this.discover_open_dropdown = None;
                            if this.focused_input == Some(FocusedInput::DiscoverCatalogSearch) {
                                this.focused_input = None;
                            }
                        } else {
                            this.discover_open_dropdown = Some(DiscoverDropdown::Catalog);
                            this.focused_input = Some(FocusedInput::DiscoverCatalogSearch);
                        }
                        cx.notify();
                    }),
                )
                .secondary_label(addon_owned.map(|addon| SharedString::from(format!("· {addon}"))))
                .max_width_px(Some(320.0)),
            ))
            .when_some(popover, |wrap, panel| wrap.child(panel))
            .into_any_element()
    }

    fn render_discover_genre_dropdown(
        &self,
        genre: &SelectableExtra,
        has_genre_filter: bool,
        all_genre_fallback_req: Option<ResourceRequest>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_open = self.discover_open_dropdown == Some(DiscoverDropdown::Genre);

        let active_genre_value = active_extra_value_label(genre);
        let entries = collect_selectable_extra_options(genre, all_genre_fallback_req.as_ref());

        let popover = is_open.then(|| {
            let mut rows_col = div().w_full().flex().flex_col().gap(sp(3.0));
            for (row_idx, pair) in entries.chunks(2).enumerate() {
                let mut row_el = div()
                    .id(("discover-genre-grid-row", row_idx))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(sp(4.0));

                for (col_idx, (label, selected, req)) in pair.iter().enumerate() {
                    let req_for_click = req.clone();
                    row_el = row_el.child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(glass_dropdown_option(
                                ("discover-opt-genre", row_idx * 2 + col_idx),
                                label.clone(),
                                None,
                                *selected,
                                cx.listener(move |this, _, _, cx| {
                                    this.apply_discover_selection(
                                        Some(CatalogWithFiltersSelected {
                                            request: req_for_click.clone(),
                                        }),
                                        cx,
                                    );
                                }),
                            )),
                    );
                }

                if pair.len() == 1 {
                    row_el = row_el.child(div().flex_1().min_w(px(0.0)));
                }

                rows_col = rows_col.child(row_el);
            }

            deferred(
                glass_dropdown_panel("discover-menu-genre", 340.0)
                    .absolute()
                    .top(sp(42.0))
                    .left_0()
                    .child(
                        div()
                            .id("discover-genre-list-scroll")
                            .w_full()
                            .max_h(sp(340.0))
                            .overflow_y_scroll()
                            .child(rows_col),
                    ),
            )
            .with_priority(10)
        });

        div()
            .relative()
            .child(dropdown_trigger(
                DropdownTriggerProps::new(
                    "discover-dd-genre",
                    active_genre_value,
                    is_open,
                    cx.listener(|this, _, _, cx| {
                        if this.discover_open_dropdown == Some(DiscoverDropdown::Genre) {
                            this.discover_open_dropdown = None;
                        } else {
                            this.discover_open_dropdown = Some(DiscoverDropdown::Genre);
                            if this.focused_input == Some(FocusedInput::DiscoverCatalogSearch) {
                                this.focused_input = None;
                            }
                        }
                        cx.notify();
                    }),
                )
                .prefix_label(Some(SharedString::new_static("Genre")))
                .is_active(has_genre_filter)
                .max_width_px(Some(260.0)),
            ))
            .when_some(popover, |wrap, panel| wrap.child(panel))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_discover_filters_dropdown(
        &self,
        non_genre_extras: &[SelectableExtra],
        default_catalog_req: Option<ResourceRequest>,
        hide_watched: bool,
        hidden_watched_count: usize,
        active_filter_count: usize,
        has_catalog_filters: bool,
        surprise_items: &Arc<Vec<MetaItemPreview>>,
        surprise_statuses: &Arc<HashMap<String, LibraryStatusTab>>,
        theme: &ThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *theme;
        let dd_filters = DiscoverDropdown::Extra(DROPDOWN_FILTERS.to_owned());
        let is_open = self.discover_open_dropdown.as_ref() == Some(&dd_filters);
        let has_active_filters = active_filter_count > 0;

        let surprise_items_arc = Arc::clone(surprise_items);
        let surprise_statuses_arc = Arc::clone(surprise_statuses);

        let popover = is_open.then(|| {
            let hide_watched_row = div()
                .id("discover-filters-hide-watched-row")
                .cursor_pointer()
                .w_full()
                .px(sp(10.0))
                .py(sp(8.0))
                .rounded(px(RADIUS_CONTROL))
                .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                .flex()
                .items_center()
                .justify_between()
                .gap(sp(10.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.preferences.discover_hide_watched =
                            !this.preferences.discover_hide_watched;
                        let _ = this.preferences.save();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(8.0))
                        .min_w(px(0.0))
                        .child(svg_icon_weighted(
                            IconName::EyeOff,
                            if hide_watched {
                                IconWeight::Fill
                            } else {
                                IconWeight::Regular
                            },
                            14.0 * theme.ui_scale,
                            rgb(if hide_watched {
                                theme.text_primary
                            } else {
                                theme.text_secondary
                            }),
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(sp(1.0))
                                .child(
                                    div()
                                        .font(ui_sans(FontWeight::MEDIUM))
                                        .text_size(sp(13.0))
                                        .text_color(rgb(theme.text_primary))
                                        .child("Hide watched"),
                                )
                                .when(hide_watched && hidden_watched_count > 0, |col| {
                                    col.child(
                                        div()
                                            .text_size(sp(11.0))
                                            .text_color(rgb(theme.text_muted))
                                            .child(format!("{hidden_watched_count} hidden")),
                                    )
                                }),
                        ),
                )
                .child(glass_switch(
                    "discover-filters-hide-watched-switch",
                    hide_watched,
                    cx.listener(|this, _, _, cx| {
                        this.preferences.discover_hide_watched =
                            !this.preferences.discover_hide_watched;
                        let _ = this.preferences.save();
                        cx.notify();
                    }),
                ));

            let extras_sections: Vec<AnyElement> = non_genre_extras
                .iter()
                .enumerate()
                .map(|(e_idx, extra)| {
                    let field_title = sentence_case_label(&extra.name);
                    let active_val = active_extra_value_label(extra);
                    let options =
                        collect_selectable_extra_options(extra, default_catalog_req.as_ref());
                    let options_len = options.len();
                    let chips: Vec<AnyElement> = options
                        .into_iter()
                        .enumerate()
                        .map(|(o_idx, (label, selected, req))| {
                            choice_pill(
                                ("discover-filter-chip", e_idx * 1000 + o_idx),
                                label,
                                selected,
                                cx.listener(move |this, _, _, cx| {
                                    this.apply_discover_selection(
                                        Some(CatalogWithFiltersSelected {
                                            request: req.clone(),
                                        }),
                                        cx,
                                    );
                                }),
                            )
                            .into_any_element()
                        })
                        .collect();

                    let chip_wrap = div().w_full().flex().flex_wrap().gap(sp(5.0)).children(chips);
                    let chip_container: AnyElement = if options_len > 8 {
                        div()
                            .id(("discover-extra-chips-scroll", e_idx))
                            .w_full()
                            .max_h(sp(124.0))
                            .overflow_y_scroll()
                            .child(chip_wrap)
                            .into_any_element()
                    } else {
                        chip_wrap.into_any_element()
                    };

                    div()
                        .w_full()
                        .px(sp(10.0))
                        .py(sp(6.0))
                        .flex()
                        .flex_col()
                        .gap(sp(7.0))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(sp(8.0))
                                .child(
                                    div()
                                        .font(ui_sans(FontWeight::SEMIBOLD))
                                        .text_size(sp(11.5))
                                        .text_color(rgb(theme.text_muted))
                                        .child(field_title),
                                )
                                .child(
                                    div()
                                        .text_size(sp(11.5))
                                        .text_color(rgb(theme.text_secondary))
                                        .truncate()
                                        .child(active_val),
                                ),
                        )
                        .child(chip_container)
                        .into_any_element()
                })
                .collect();

            let surprise_row = div()
                .id("discover-filters-surprise")
                .cursor_pointer()
                .w_full()
                .px(sp(10.0))
                .py(sp(8.0))
                .rounded(px(RADIUS_CONTROL))
                .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                .flex()
                .items_center()
                .justify_between()
                .gap(sp(10.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.discover_open_dropdown = None;
                        this.trigger_discover_surprise(
                            &surprise_items_arc,
                            &surprise_statuses_arc,
                            cx,
                        );
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(8.0))
                        .child(svg_icon(
                            IconName::Sparkles,
                            14.0 * theme.ui_scale,
                            rgb(theme.accent),
                        ))
                        .child(
                            div()
                                .font(ui_sans(FontWeight::MEDIUM))
                                .text_size(sp(13.0))
                                .text_color(rgb(theme.text_primary))
                                .child("Surprise me"),
                        ),
                )
                .child(
                    div()
                        .text_size(sp(11.0))
                        .text_color(rgb(theme.text_muted))
                        .child("Random pick"),
                );

            let reset_req = default_catalog_req.clone();
            let reset_row = has_active_filters.then(|| {
                div()
                    .id("discover-filters-reset")
                    .cursor_pointer()
                    .w_full()
                    .px(sp(10.0))
                    .py(sp(8.0))
                    .rounded(px(RADIUS_CONTROL))
                    .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sp(10.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.discover_open_dropdown = None;
                            if this.preferences.discover_hide_watched {
                                this.preferences.discover_hide_watched = false;
                                let _ = this.preferences.save();
                            }
                            if has_catalog_filters {
                                this.apply_discover_selection(
                                    reset_req.clone().map(|request| {
                                        CatalogWithFiltersSelected { request }
                                    }),
                                    cx,
                                );
                            } else {
                                cx.notify();
                            }
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(8.0))
                            .child(svg_icon(
                                IconName::RefreshCw,
                                13.5 * theme.ui_scale,
                                rgb(theme.text_secondary),
                            ))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(sp(13.0))
                                    .text_color(rgb(theme.text_secondary))
                                    .child("Reset filters"),
                            ),
                    )
                    .child(
                        div()
                            .font(mono_font())
                            .font_features(tabular_nums())
                            .text_size(sp(11.0))
                            .text_color(rgb(theme.text_muted))
                            .child(format!("{active_filter_count}")),
                    )
            });

            deferred(
                glass_dropdown_panel("discover-menu-filters", 324.0)
                    .absolute()
                    .top(sp(42.0))
                    .right_0()
                    .gap(sp(4.0))
                    .child(hide_watched_row)
                    .when(!extras_sections.is_empty(), |panel| {
                        panel
                            .child(
                                div()
                                    .w_full()
                                    .h(px(1.0))
                                    .my(sp(2.0))
                                    .bg(rgba(theme.glass_divider)),
                            )
                            .children(extras_sections)
                    })
                    .child(
                        div()
                            .w_full()
                            .h(px(1.0))
                            .my(sp(2.0))
                            .bg(rgba(theme.glass_divider)),
                    )
                    .child(surprise_row)
                    .when_some(reset_row, |panel, row| panel.child(row)),
            )
            .with_priority(10)
        });

        let dd_for_click = dd_filters;
        let filters_label: SharedString = if has_active_filters {
            format!("Filters ({active_filter_count})").into()
        } else {
            SharedString::new_static("Filters")
        };

        div()
            .relative()
            .child(dropdown_trigger(
                DropdownTriggerProps::new(
                    "discover-dd-filters",
                    filters_label,
                    is_open,
                    cx.listener(move |this, _, _, cx| {
                        if this.discover_open_dropdown.as_ref() == Some(&dd_for_click) {
                            this.discover_open_dropdown = None;
                        } else {
                            this.discover_open_dropdown = Some(dd_for_click.clone());
                            if this.focused_input == Some(FocusedInput::DiscoverCatalogSearch) {
                                this.focused_input = None;
                            }
                        }
                        cx.notify();
                    }),
                )
                .leading_icon(Some(IconName::Sliders))
                .is_active(has_active_filters),
            ))
            .when_some(popover, |wrap, panel| wrap.child(panel))
            .into_any_element()
    }

    fn render_source_rail_toggle_button(
        &self,
        rail_open: bool,
        theme: &ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = *theme;
        div()
            .id("discover-toggle-source-rail")
            .cursor_pointer()
            .flex_shrink_0()
            .h(sp(36.0))
            .px(sp(11.0))
            .rounded(px(RADIUS_CONTROL))
            .bg(if rail_open {
                rgba(theme.glass_row_selected_bg)
            } else {
                rgba(0x00000000)
            })
            .border_1()
            .border_color(if rail_open {
                rgba(theme.glass_border_strong)
            } else {
                rgba(theme.glass_border)
            })
            .hover(move |s| {
                s.bg(rgba(theme.glass_row_hover_bg))
                    .border_color(rgba(theme.glass_border_strong))
                    .text_color(rgb(theme.text_primary))
            })
            .flex()
            .items_center()
            .gap(sp(6.0))
            .font(ui_sans(FontWeight::MEDIUM))
            .text_size(sp(12.5))
            .text_color(rgb(if rail_open {
                theme.text_primary
            } else {
                theme.text_secondary
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.preferences.discover_source_rail_open =
                        !this.preferences.discover_source_rail_open;
                    let _ = this.preferences.save();
                    cx.notify();
                }),
            )
            .child(svg_icon_weighted(
                IconName::PanelLeft,
                if rail_open {
                    IconWeight::Fill
                } else {
                    IconWeight::Regular
                },
                14.0 * theme.ui_scale,
                rgb(if rail_open {
                    theme.text_primary
                } else {
                    theme.text_secondary
                }),
            ))
            .child("Sources")
    }

    fn apply_discover_selection(
        &mut self,
        selected: Option<CatalogWithFiltersSelected>,
        cx: &mut Context<Self>,
    ) {
        self.discover_open_dropdown = None;
        if self.focused_input == Some(FocusedInput::DiscoverCatalogSearch) {
            self.focused_input = None;
        }
        self.kb_focus_index = None;
        self.discover_last_next_page_len.set(None);
        self.discover_grid_scroll_handle
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(0.0)));
        self.core.load_discover(selected);
        cx.notify();
    }

    fn trigger_discover_surprise(
        &mut self,
        items: &[MetaItemPreview],
        statuses: &HashMap<String, LibraryStatusTab>,
        cx: &mut Context<Self>,
    ) {
        let exclude_id = self.quick_peek.as_ref().map(|p| p.id.as_str());
        let seed = next_surprise_seed(items.len());
        let Some(idx) = pick_surprise_item_index(items, statuses, exclude_id, seed) else {
            self.push_toast(
                ToastKind::Info,
                "No titles to pick from yet",
                Some("Try switching catalogs or clearing filters.".to_owned()),
                cx,
            );
            return;
        };
        let picked = items[idx].clone();
        self.open_quick_peek(
            QuickPeekTarget {
                type_name: picked.r#type.clone(),
                id: picked.id.clone(),
                video_id: None,
                title: picked.name.clone(),
                preview: Some(picked),
            },
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn render_discover_poster_card(
        &self,
        id: impl Into<ElementId>,
        item: MetaItemPreview,
        lib_status: Option<LibraryStatusTab>,
        aspect: PosterAspect,
        fixed_width_px: Option<f32>,
        image_height_px: f32,
        is_focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let base_id = Arc::new(id.into());

        let poster_tex = self
            .poster_cache
            .resolve_poster_url(item.poster.as_ref().or(item.background.as_ref()));
        let rating = extract_imdb_rating(&item).map(SharedString::from);
        let subtitle = format_discover_card_subtitle(&item);

        let peek_item = item.clone();
        let details_type = item.r#type.clone();
        let details_id = item.id.clone();

        let mid_type = item.r#type.clone();
        let mid_id = item.id.clone();

        let play_type = item.r#type.clone();
        let play_id = item.id.clone();

        let toggle_preview = item.clone();
        let toggle_name = item.name.clone();
        let in_library = lib_status.is_some();

        let hover_type = item.r#type.clone();
        let hover_id = item.id.clone();
        let hover_bg = item.background.as_ref().map(|u| u.to_string());

        let ctx_preview = item.clone();
        let ctx_type = item.r#type.clone();
        let ctx_id = item.id.clone();
        let ctx_name = item.name.clone();

        let card_el = render_poster_card(
            PosterCardProps::new((*base_id).clone(), item.name, item.id)
                .subtitle(Some(subtitle))
                .image(poster_tex)
                .aspect(aspect)
                .fixed_width_px(fixed_width_px)
                .image_height_px(Some(image_height_px))
                .status_tab(lib_status)
                .imdb_rating(rating)
                .in_library(in_library)
                .is_focused(is_focused)
                .on_open(click_handler(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.open_meta_details(details_type.clone(), details_id.clone(), None, cx);
                })))
                .on_quick_peek(click_handler(cx.listener(move |this, _, _, cx| {
                    this.open_quick_peek(
                        QuickPeekTarget {
                            type_name: peek_item.r#type.clone(),
                            id: peek_item.id.clone(),
                            video_id: None,
                            title: peek_item.name.clone(),
                            preview: Some(peek_item.clone()),
                        },
                        cx,
                    );
                })))
                .on_quick_play(click_handler(cx.listener(move |this, _, _, cx| {
                    this.quick_resume_or_open(play_type.clone(), play_id.clone(), None, cx);
                })))
                .on_toggle_library(click_handler(cx.listener(move |this, _, _, cx| {
                    this.core.toggle_library_item(toggle_preview.clone());
                    let (kind, msg) = if in_library {
                        (ToastKind::Info, "Removed from library")
                    } else {
                        (ToastKind::Success, "Added to library")
                    };
                    this.push_toast(kind, msg, Some(toggle_name.clone()), cx);
                    cx.notify();
                })))
                .on_context_menu(click_handler(cx.listener(
                    move |this, ev: &MouseDownEvent, _, cx| {
                        let (lib_in, is_watched, can_rewind, vid) = this.core.with_model(|m| {
                            m.ctx
                                .library
                                .items
                                .get(&ctx_id)
                                .filter(|li| !li.removed)
                                .map(|li| {
                                    (
                                        !li.temp,
                                        li.watched(),
                                        li.state.time_offset > 0,
                                        li.state.video_id.clone(),
                                    )
                                })
                                .unwrap_or((false, false, false, None))
                        });
                        let menu = BrowsingContextMenu::new(
                            ev.position,
                            ctx_type.clone(),
                            ctx_id.clone(),
                            ctx_name.clone(),
                        )
                        .video_id(vid)
                        .preview(Some(ctx_preview.clone()))
                        .is_in_library(lib_in)
                        .is_watched(is_watched)
                        .can_rewind_cw(can_rewind);
                        this.open_browsing_context_menu(menu, cx);
                    },
                )))
                .on_hover(hover_handler(cx.listener(move |this, hovered, _, cx| {
                    this.on_card_hover_prefetch(
                        &hover_type,
                        &hover_id,
                        hover_bg.as_deref(),
                        *hovered,
                        cx,
                    );
                }))),
        );

        let mut wrapper = div().on_mouse_down(
            MouseButton::Middle,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.open_meta_details(mid_type.clone(), mid_id.clone(), None, cx);
            }),
        );

        if fixed_width_px.is_some() {
            wrapper = wrapper.flex_shrink_0();
        } else {
            wrapper = wrapper.w_full().min_w(px(0.0));
        }

        wrapper.child(card_el).into_any_element()
    }
}

fn format_discover_card_subtitle(item: &MetaItemPreview) -> SharedString {
    let primary = item
        .release_info
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| sentence_case_label(&item.r#type));
    match extract_genres(item, 1).into_iter().next() {
        Some(genre) => SharedString::from(format!("{primary} · {genre}")),
        None => SharedString::from(primary),
    }
}

fn render_discover_skeleton_grid(
    columns: usize,
    image_height_px: f32,
    row_stride_px: f32,
    gap_px: f32,
    theme: &ThemePalette,
) -> AnyElement {
    let theme = *theme;
    let cols = columns.max(1);
    let shimmer_anim = Animation::new(Duration::from_millis(1300))
        .repeat_synced()
        .with_max_fps(30.0)
        .with_easing(|t| 0.45 + 0.55 * (0.5 - 0.5 * (t * std::f32::consts::TAU).cos()));

    div()
        .id("discover-skeleton-grid")
        .flex_1()
        .w_full()
        .min_h(px(0.0))
        .overflow_hidden()
        .flex()
        .flex_col()
        .children((0..3_usize).map(move |row_idx| {
            let row_anim = shimmer_anim.clone();
            div()
                .id(("discover-skel-row", row_idx))
                .w_full()
                .h(px(row_stride_px))
                .pb(px(gap_px))
                .flex()
                .items_start()
                .gap(px(gap_px))
                .children((0..cols).map(move |col_idx| {
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(sp(8.0))
                        .child(
                            div()
                                .w_full()
                                .h(px(image_height_px))
                                .rounded(px(RADIUS_POSTER))
                                .bg(rgba(theme.glass_card_bg))
                                .border_1()
                                .border_color(rgba(theme.glass_border)),
                        )
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .gap(sp(6.0))
                                .child(
                                    div()
                                        .h(sp(14.0))
                                        .w(relative(0.76))
                                        .rounded(px(4.0))
                                        .bg(rgba(theme.glass_trough_bg)),
                                )
                                .child(
                                    div()
                                        .h(sp(11.0))
                                        .w(relative(0.46))
                                        .rounded(px(4.0))
                                        .bg(rgba(theme.glass_card_bg)),
                                ),
                        )
                        .with_animation(
                            ("discover-skel-pulse", row_idx * 16 + col_idx),
                            row_anim.clone(),
                            |el, delta| el.opacity(delta),
                        )
                }))
        }))
        .into_any_element()
}

fn render_discover_empty_state(
    has_active_filters: bool,
    all_hidden_by_watched: bool,
    reset_req: Option<ResourceRequest>,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let icon = if all_hidden_by_watched {
        IconName::EyeOff
    } else {
        IconName::Compass
    };
    let title = if all_hidden_by_watched {
        "You've watched everything here"
    } else {
        "No titles found"
    };
    let body = if all_hidden_by_watched {
        "Every loaded title in this view is already marked as watched. Disable Hide Watched or try another genre."
    } else if has_active_filters {
        "No catalog entries match the active filters. Reset filters or pick another genre."
    } else {
        "This catalog did not return any titles. Try switching catalogs or addons."
    };

    let mut props = EmptyStateProps::new(icon, title, body).fill_available(true);
    if has_active_filters {
        props = props.action(primary_accent_button(
            "discover-empty-reset",
            Some(if all_hidden_by_watched {
                IconName::Eye
            } else {
                IconName::RefreshCw
            }),
            if all_hidden_by_watched {
                "Show watched titles"
            } else {
                "Reset filters"
            },
            cx.listener(move |this, _, _, cx| {
                if this.preferences.discover_hide_watched {
                    this.preferences.discover_hide_watched = false;
                    let _ = this.preferences.save();
                }
                if all_hidden_by_watched {
                    cx.notify();
                } else {
                    this.discover_catalog_search.clear();
                    this.input_cursors
                        .remove(&FocusedInput::DiscoverCatalogSearch);
                    this.input_selected_all
                        .remove(&FocusedInput::DiscoverCatalogSearch);
                    this.apply_discover_selection(
                        reset_req
                            .clone()
                            .map(|request| CatalogWithFiltersSelected { request }),
                        cx,
                    );
                }
            }),
        ));
    }

    render_empty_state(props).into_any_element()
}

#[cfg(test)]
mod tests {
    use semver::Version;
    use stremio_core_runtime::stremio_core::{
        models::catalog_with_filters::{SelectableCatalog, SelectableExtraOption},
        types::{
            addon::{Manifest, ManifestCatalog, ResourcePath},
            resource::{MetaItemBehaviorHints, PosterShape},
        },
    };
    use url::Url;

    use super::*;

    fn sample_req(base: &str, ty: &str, id: &str) -> ResourceRequest {
        ResourceRequest {
            base: Url::parse(base).unwrap(),
            path: ResourcePath {
                resource: "catalog".to_owned(),
                r#type: ty.to_owned(),
                id: id.to_owned(),
                extra: vec![],
            },
        }
    }

    fn sample_type(ty: &str, selected: bool) -> SelectableType {
        SelectableType {
            r#type: ty.to_owned(),
            selected,
            request: sample_req("https://v3-cinemeta.strem.io/manifest.json", ty, "top"),
        }
    }

    fn sample_preview(id: &str, name: &str) -> MetaItemPreview {
        MetaItemPreview {
            id: id.to_owned(),
            r#type: "movie".to_owned(),
            name: name.to_owned(),
            poster: None,
            background: None,
            logo: None,
            description: None,
            release_info: Some("2024".to_owned()),
            runtime: None,
            released: None,
            poster_shape: PosterShape::Poster,
            links: vec![],
            trailer_streams: vec![],
            behavior_hints: MetaItemBehaviorHints::default(),
        }
    }

    fn sample_catalog_opt(
        base: &str,
        ty: &str,
        id: &str,
        title: &str,
        addon: Option<&str>,
    ) -> DiscoverCatalogOption {
        DiscoverCatalogOption {
            catalog: SelectableCatalog {
                catalog: id.to_owned(),
                selected: false,
                request: sample_req(base, ty, id),
                is_epg_guide: false,
            },
            title: title.to_owned(),
            addon_name: addon.map(ToOwned::to_owned),
        }
    }

    fn sample_descriptor(base: &str, name: &str, catalogs: Vec<(&str, &str, &str)>) -> Descriptor {
        Descriptor {
            transport_url: Url::parse(base).unwrap(),
            manifest: Manifest {
                id: format!("org.test.{}", name.to_ascii_lowercase()),
                version: Version::new(1, 0, 0),
                name: name.to_owned(),
                contact_email: None,
                description: None,
                logo: None,
                background: None,
                types: vec!["movie".to_owned(), "series".to_owned()],
                resources: vec![],
                id_prefixes: None,
                catalogs: catalogs
                    .into_iter()
                    .map(|(ty, id, cname)| ManifestCatalog {
                        r#type: ty.to_owned(),
                        id: id.to_owned(),
                        name: Some(cname.to_owned()),
                        extra: Default::default(),
                    })
                    .collect(),
                addon_catalogs: vec![],
                behavior_hints: Default::default(),
            },
            flags: Default::default(),
        }
    }

    #[test]
    fn partition_discover_types_orders_movies_and_series_first_and_splits_extras() {
        let types = vec![
            sample_type("series", false),
            sample_type("channel", false),
            sample_type("movie", true),
            sample_type("anime", false),
            sample_type("tv", false),
        ];

        let partitioned = partition_discover_types(&types);
        let core_names: Vec<&str> = partitioned.core.iter().map(|t| t.r#type.as_str()).collect();
        let extra_names: Vec<&str> = partitioned.extra.iter().map(|t| t.r#type.as_str()).collect();

        assert_eq!(core_names, vec!["movie", "series"]);
        assert_eq!(extra_names, vec!["channel", "anime", "tv"]);
        assert_eq!(format_discover_type_label("movie"), "Movies");
        assert_eq!(format_discover_type_label("series"), "Series");
        assert_eq!(format_discover_type_label("anime"), "Anime");

        // Fallback to first 2 types when neither movie nor series exists.
        let custom_types = vec![
            sample_type("anime", true),
            sample_type("ova", false),
            sample_type("manga", false),
        ];
        let custom_part = partition_discover_types(&custom_types);
        assert_eq!(
            custom_part
                .core
                .iter()
                .map(|t| t.r#type.as_str())
                .collect::<Vec<_>>(),
            vec!["anime", "ova"]
        );
        assert_eq!(
            custom_part
                .extra
                .iter()
                .map(|t| t.r#type.as_str())
                .collect::<Vec<_>>(),
            vec!["manga"]
        );

        // When only movie & series exist, extra is empty.
        let basic = vec![sample_type("movie", true), sample_type("series", false)];
        assert!(partition_discover_types(&basic).extra.is_empty());
        assert!(partition_discover_types(&[]).core.is_empty());
    }

    #[test]
    fn is_extra_filtered_respects_required_default_and_empty_values() {
        let req = sample_req("https://cinemeta.example/m.json", "movie", "top");
        let make_opt = |value: Option<&str>, selected: bool| SelectableExtraOption {
            value: value.map(ToOwned::to_owned),
            selected,
            request: req.clone(),
        };

        let optional_unselected = SelectableExtra {
            name: "genre".to_owned(),
            is_required: false,
            options: vec![make_opt(None, true), make_opt(Some("Drama"), false)],
        };
        assert!(!is_extra_filtered(&optional_unselected));

        let optional_selected = SelectableExtra {
            name: "genre".to_owned(),
            is_required: false,
            options: vec![make_opt(None, false), make_opt(Some("Drama"), true)],
        };
        assert!(is_extra_filtered(&optional_selected));

        let required_default_first = SelectableExtra {
            name: "genre".to_owned(),
            is_required: true,
            options: vec![make_opt(Some("All"), true), make_opt(Some("Action"), false)],
        };
        assert!(!is_extra_filtered(&required_default_first));

        let required_second = SelectableExtra {
            name: "genre".to_owned(),
            is_required: true,
            options: vec![make_opt(Some("All"), false), make_opt(Some("Action"), true)],
        };
        assert!(is_extra_filtered(&required_second));
    }

    #[test]
    fn catalog_pin_key_and_toggle_preserve_order_and_toggle_cleanly() {
        let req = sample_req("https://v3-cinemeta.strem.io/manifest.json", "movie", "top");
        let key = catalog_pin_key(&req);
        assert_eq!(
            key,
            "https://v3-cinemeta.strem.io/manifest.json::movie::top"
        );

        let mut pinned = Vec::new();
        assert!(toggle_pinned_catalog_key(&mut pinned, key.clone()));
        assert_eq!(pinned, vec![key.clone()]);
        assert!(!toggle_pinned_catalog_key(&mut pinned, key));
        assert!(pinned.is_empty());
    }

    #[test]
    fn move_pinned_catalog_reorders_cleanly_and_respects_boundaries() {
        let mut pinned = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];

        assert!(!move_pinned_catalog(&mut pinned, "a", -1));
        assert!(!move_pinned_catalog(&mut pinned, "c", 1));
        assert!(!move_pinned_catalog(&mut pinned, "missing", 1));
        assert!(!move_pinned_catalog(&mut pinned, "b", 0));

        assert!(move_pinned_catalog(&mut pinned, "c", -1));
        assert_eq!(pinned, vec!["a", "c", "b"]);

        assert!(move_pinned_catalog(&mut pinned, "a", 1));
        assert_eq!(pinned, vec!["c", "a", "b"]);

        // Visible-only swap preserves interleaved keys from other formats.
        let mut mixed = vec![
            "series::x".to_owned(),
            "movie::a".to_owned(),
            "series::y".to_owned(),
            "movie::b".to_owned(),
        ];
        let visible_movies = vec!["movie::a".to_owned(), "movie::b".to_owned()];
        assert!(!move_visible_pinned_catalog(
            &mut mixed,
            &visible_movies,
            "movie::a",
            -1
        ));
        assert!(move_visible_pinned_catalog(
            &mut mixed,
            &visible_movies,
            "movie::a",
            1
        ));
        assert_eq!(
            mixed,
            vec!["series::x", "movie::b", "series::y", "movie::a"]
        );
    }

    #[test]
    fn addon_collapse_defaults_and_toggling_respect_active_and_search() {
        let mut collapsed = Vec::new();

        // By default, inactive addons are collapsed while the active addon is expanded.
        assert!(is_addon_group_collapsed(&collapsed, "TMDB", false, false));
        assert!(!is_addon_group_collapsed(&collapsed, "Cinemeta", false, true));

        // Searching always auto-expands all addon groups.
        assert!(!is_addon_group_collapsed(&collapsed, "TMDB", true, false));

        // Expanding an inactive addon records "__exp__:<lower_addon>".
        assert!(toggle_addon_group_expansion(&mut collapsed, "TMDB", false));
        assert_eq!(collapsed, vec!["__exp__:tmdb"]);
        assert!(!is_addon_group_collapsed(&collapsed, "  tmdb ", false, false));

        // Collapsing it again removes "__exp__:tmdb" and returns to default collapsed state.
        assert!(!toggle_addon_group_expansion(&mut collapsed, "TMDB", false));
        assert!(collapsed.is_empty());
        assert!(is_addon_group_collapsed(&collapsed, "TMDB", false, false));

        // Active addon can also be explicitly collapsed and re-expanded.
        assert!(!toggle_addon_group_expansion(&mut collapsed, "Cinemeta", true));
        assert_eq!(collapsed, vec!["cinemeta"]);
        assert!(is_addon_group_collapsed(&collapsed, "Cinemeta", false, true));
        assert!(toggle_addon_group_expansion(&mut collapsed, "Cinemeta", true));
        assert!(collapsed.is_empty());
        assert!(!is_addon_group_collapsed(&collapsed, "Cinemeta", false, true));
    }

    #[test]
    fn hiding_and_restoring_catalogs_filters_groups_and_resolves_entries() {
        let catalogs = vec![
            sample_catalog_opt(
                "https://cinemeta.example/m.json",
                "movie",
                "top",
                "Popular",
                Some("Cinemeta"),
            ),
            sample_catalog_opt(
                "https://tmdb.example/m.json",
                "movie",
                "trending",
                "Trending",
                Some("TMDB"),
            ),
            sample_catalog_opt(
                "https://cinemeta.example/m.json",
                "movie",
                "featured",
                "Featured",
                Some("Cinemeta"),
            ),
            sample_catalog_opt(
                "https://custom.example/m.json",
                "movie",
                "picks",
                "Picks",
                None,
            ),
        ];

        let pinned_keys = vec![catalog_pin_key(&catalogs[1].catalog.request)];
        let featured_key = catalog_pin_key(&catalogs[2].catalog.request);
        let series_hidden_key = "https://tmdb.example/m.json::series::airing".to_owned();

        let mut hidden_keys = Vec::new();
        assert!(hide_catalog_key(&mut hidden_keys, featured_key.clone()));
        assert!(!hide_catalog_key(&mut hidden_keys, featured_key.clone()));
        assert!(hide_catalog_key(&mut hidden_keys, series_hidden_key.clone()));

        let (pinned, groups) =
            partition_and_group_catalogs(&catalogs, &pinned_keys, &hidden_keys, "");
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].title, "Trending");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].addon_name, "Cinemeta");
        assert_eq!(groups[0].items.len(), 1);
        assert_eq!(groups[0].items[0].title, "Popular");
        assert_eq!(groups[1].addon_name, "Other Addons");

        let (filtered_pinned, filtered_groups) =
            partition_and_group_catalogs(&catalogs, &pinned_keys, &hidden_keys, "tmdb");
        assert_eq!(filtered_pinned.len(), 1);
        assert!(filtered_groups.is_empty());

        let addons = vec![sample_descriptor(
            "https://tmdb.example/m.json",
            "TMDB",
            vec![("series", "airing", "Airing Today")],
        )];
        let resolved = resolve_hidden_catalog_entries(&hidden_keys, &catalogs, &addons);
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].title, "Featured");
        assert_eq!(resolved[0].subtitle.as_deref(), Some("Cinemeta"));
        assert_eq!(resolved[1].title, "Airing today");
        assert_eq!(resolved[1].subtitle.as_deref(), Some("TMDB · Series"));

        assert!(restore_catalog_key(&mut hidden_keys, &featured_key));
        assert!(!restore_catalog_key(&mut hidden_keys, &featured_key));
        assert_eq!(hidden_keys, vec![series_hidden_key]);

        assert_eq!(restore_all_catalog_keys(&mut hidden_keys), 1);
        assert!(hidden_keys.is_empty());
    }

    #[test]
    fn filter_unwatched_items_hides_only_watched_entries() {
        let items = vec![
            sample_preview("tt1", "Alpha"),
            sample_preview("tt2", "Beta"),
            sample_preview("tt3", "Gamma"),
        ];
        let mut statuses = HashMap::new();
        statuses.insert("tt1".to_owned(), LibraryStatusTab::Watched);
        statuses.insert("tt2".to_owned(), LibraryStatusTab::Watching);
        statuses.insert("tt3".to_owned(), LibraryStatusTab::Watchlist);

        let (visible, hidden) = filter_unwatched_items(&items, &statuses);
        assert_eq!(hidden, 1);
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "tt2");
        assert_eq!(visible[1].id, "tt3");
    }

    #[test]
    fn surprise_picker_prefers_unwatched_and_avoids_repeat() {
        let items = vec![
            sample_preview("tt1", "Watched Movie"),
            sample_preview("tt2", "Unwatched One"),
            sample_preview("tt3", "Unwatched Two"),
        ];
        let mut statuses = HashMap::new();
        statuses.insert("tt1".to_owned(), LibraryStatusTab::Watched);

        let picked = pick_surprise_item_index(&items, &statuses, Some("tt2"), 42).unwrap();
        assert_eq!(items[picked].id, "tt3");

        // When all items are watched, it falls back gracefully to any item.
        statuses.insert("tt2".to_owned(), LibraryStatusTab::Watched);
        statuses.insert("tt3".to_owned(), LibraryStatusTab::Watched);
        assert!(pick_surprise_item_index(&items, &statuses, None, 99).is_some());
        assert_eq!(pick_surprise_item_index(&[], &statuses, None, 99), None);
    }
}
