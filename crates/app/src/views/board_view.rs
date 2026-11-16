use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
};

use gpui::{
    div, img, linear_color_stop, linear_gradient, point, prelude::*, px, rgb, rgba, AnyElement,
    Context, ElementId, FontWeight, MouseButton, MouseDownEvent, ObjectFit, ScrollDelta,
    ScrollEvent, ScrollHandle, SharedString,
};
use stremio_core_runtime::{
    stremio_core::{
        models::{
            catalog_with_filters::Selected as CatalogWithFiltersSelected, common::Loadable,
        },
        types::{
            addon::{ExtraValue, ResourceRequest},
            library::LibraryItem,
            resource::{MetaItemPreview, PosterShape, Stream, StreamSource},
        },
    },
    DesktopModel, INITIAL_BOARD_SHELVES,
};

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{
        extract_genres, extract_imdb_rating, format_movietracker_episode_code, scroll_near_bottom,
        AppRoute, BrowsingContextMenu, KbFocusableCard, LibraryStatusTab, PlaybackCandidate,
        SidebarTab,
    },
    theme::{
        editorial_serif, meta_divider, mono_font, placeholder_tint_for_id, sentence_case_label,
        ui_sans, ACCENT, ACCENT_AMBER, ACCENT_SOFT, BG_ELEVATED, BORDER_ACCENT, GLASS_BORDER,
        GLASS_BORDER_STRONG, GLASS_CARD_BG, GLASS_ROW_HOVER_BG, GLASS_SHEET_BG, GLASS_TROUGH_BG,
        RADIUS_CARD, RADIUS_CONTROL, RADIUS_PILL, RADIUS_POSTER, RADIUS_SHEET, TEXT_MUTED,
        TEXT_PRIMARY, TEXT_SECONDARY,
    },
    ui_components::{
        primary_accent_button, render_poster_card, secondary_glass_button, PosterCardProps,
    },
    virtual_grid::PosterAspect,
};

const SHELF_CARD_GAP_PX: f32 = 18.0;
const SHELF_HEADER_HEIGHT_PX: f32 = 40.0;
const SHELF_HEADER_GAP_PX: f32 = 14.0;
const SHELF_BOTTOM_GAP_PX: f32 = 36.0;
const CARD_METADATA_HEIGHT_PX: f32 = 44.0;
const VERTICAL_OVERSCAN_PX: f32 = 650.0;
const HORIZONTAL_OVERSCAN_CARDS: usize = 2;

#[derive(Clone)]
struct HomeSpotlightCandidate {
    id: String,
    r#type: String,
    name: String,
    video_id: Option<String>,
    is_resume: bool,
    eyebrow: String,
    description: Option<String>,
    year: Option<String>,
    imdb_rating: Option<String>,
    genres: Vec<String>,
    backdrop_url: Option<String>,
    logo_url: Option<String>,
    trailer_youtube_key: Option<String>,
    trailer_streams: Vec<Stream>,
}

#[derive(Clone)]
enum SpotlightTrailerTarget {
    DirectStream(Box<Stream>),
    YouTubeUrl(String),
}

struct ContinueWatchingCard {
    library_item: LibraryItem,
    notification_count: usize,
    backdrop_url: Option<String>,
    imdb_rating: Option<String>,
    preview: Option<MetaItemPreview>,
}

#[derive(Clone)]
struct BoardShelfMeta {
    raw_index: usize,
    shelf_key: String,
    title: String,
    subtitle: Option<String>,
    see_all_request: ResourceRequest,
    aspect: PosterAspect,
    total_items: usize,
    is_loading: bool,
    can_load_more: bool,
    is_pinned: bool,
}

struct BoardRowSnapshot {
    meta: BoardShelfMeta,
    visible_range: Range<usize>,
    visible_items: Vec<MetaItemPreview>,
    shelf_total_h: f32,
    is_visible_y: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HiddenShelfInfo {
    shelf_key: String,
    title: String,
    subtitle: Option<String>,
}

impl AppShell {
    pub fn render_board_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport_w = if self.viewport_size_px.0 > 100.0 && self.viewport_size_px.0.is_finite()
        {
            self.viewport_size_px.0
        } else {
            1280.0
        };
        let viewport_h = if self.viewport_size_px.1 > 100.0 && self.viewport_size_px.1.is_finite()
        {
            self.viewport_size_px.1
        } else {
            900.0
        };
        let scroll_y = (-self.board_scroll_handle.offset().y / px(1.0)).max(0.0);
        let container_w = (viewport_w.min(1440.0) - 64.0).max(360.0);

        let shelf_limit = self.visible_board_shelves.max(INITIAL_BOARD_SHELVES);
        let want_spotlight = self.preferences.show_home_spotlight;
        let pinned_prefs = self.preferences.pinned_board_shelves.clone();
        let hidden_prefs = self.preferences.hidden_board_shelves.clone();
        let shelf_scroll_x: HashMap<usize, f32> = self
            .board_shelf_scrolls
            .iter()
            .map(|(&idx, h)| (idx, (-h.offset().x / px(1.0)).max(0.0)))
            .collect();

        let library_statuses = self.cached_library_statuses();

        let (
            cw_cards,
            cw_total_count,
            catalog_rows,
            hidden_shelves,
            total_board_catalogs,
            spotlight_candidates,
            pinned_indices_to_load,
        ) = self.core.with_model(|model| {
            let cw: Vec<ContinueWatchingCard> = model
                .continue_watching_preview
                .items
                .iter()
                .take(6)
                .map(|cw_item| {
                    let li = &cw_item.library_item;
                    let preview_ref = find_preview_ref_in_model(model, &li.id);
                    ContinueWatchingCard {
                        library_item: li.clone(),
                        notification_count: cw_item.notifications,
                        backdrop_url: preview_ref
                            .and_then(|p| p.background.as_ref().map(|u| u.to_string())),
                        imdb_rating: preview_ref.and_then(extract_imdb_rating),
                        preview: preview_ref.cloned(),
                    }
                })
                .collect();
            let cw_total = model.continue_watching_preview.items.len();

            let total_catalogs = model.board.catalogs.len();
            let mut sortable_metas: Vec<(Option<usize>, BoardShelfMeta)> = Vec::new();
            let mut hidden_list: Vec<HiddenShelfInfo> = Vec::new();
            let mut seen_hidden_keys: HashSet<String> = HashSet::new();
            let mut to_load: Vec<usize> = Vec::new();

            for (idx, cat) in model.board.catalogs.iter().enumerate() {
                let Some(first_page) = cat.first() else {
                    continue;
                };
                let raw_type = first_page.request.path.r#type.as_str();
                let raw_id = first_page.request.path.id.as_str();
                let shelf_key = build_shelf_key(first_page.request.base.as_str(), raw_type, raw_id);

                let matched_mc = model
                    .ctx
                    .profile
                    .addons
                    .iter()
                    .find(|a| a.transport_url == first_page.request.base)
                    .and_then(|a| {
                        a.manifest
                            .catalogs
                            .iter()
                            .find(|mc| mc.id == raw_id && mc.r#type == raw_type)
                    });
                let addon_cat_name = matched_mc.and_then(|mc| mc.name.as_deref());
                let supports_skip = matched_mc.is_some_and(|mc| {
                    mc.extra
                        .iter()
                        .any(|extra| extra.name.eq_ignore_ascii_case("skip"))
                });

                let (section_title, section_sub) = editorialize_shelf_heading(
                    idx,
                    addon_cat_name.unwrap_or(raw_id),
                    raw_type,
                );

                if hidden_prefs.iter().any(|k| k == &shelf_key) {
                    if seen_hidden_keys.insert(shelf_key.clone()) {
                        hidden_list.push(HiddenShelfInfo {
                            shelf_key,
                            title: section_title,
                            subtitle: section_sub,
                        });
                    }
                    continue;
                }

                let pin_pos = pinned_prefs.iter().position(|k| k == &shelf_key);
                let is_pinned = pin_pos.is_some();
                if idx >= shelf_limit && !is_pinned {
                    continue;
                }

                let needs_initial_load = first_page.content.is_none();
                if is_pinned && needs_initial_load {
                    to_load.push(idx);
                }

                let mut total_items = 0_usize;
                let mut is_loading = false;
                let mut first_shape: Option<PosterShape> = None;

                for page in cat {
                    match &page.content {
                        Some(Loadable::Ready(items)) => {
                            if first_shape.is_none() {
                                if let Some(first) = items.first() {
                                    first_shape = Some(first.poster_shape.clone());
                                }
                            }
                            total_items += items.len();
                        }
                        Some(Loadable::Loading) => is_loading = true,
                        _ => {}
                    }
                }

                if total_items == 0 && !is_loading && !(is_pinned && needs_initial_load) {
                    continue;
                }

                let can_load_more = supports_skip
                    && !is_loading
                    && cat.last().is_some_and(|p| {
                        matches!(&p.content, Some(Loadable::Ready(items)) if !items.is_empty())
                    });

                let see_all_request = strip_skip_extra(&first_page.request);
                let aspect = first_shape
                    .as_ref()
                    .map(PosterAspect::from_stremio_poster_shape)
                    .unwrap_or(PosterAspect::Portrait2x3);

                sortable_metas.push((
                    pin_pos,
                    BoardShelfMeta {
                        raw_index: idx,
                        shelf_key,
                        title: section_title,
                        subtitle: section_sub,
                        see_all_request,
                        aspect,
                        total_items,
                        is_loading: is_loading || (is_pinned && needs_initial_load),
                        can_load_more,
                        is_pinned,
                    },
                ));
            }

            for key in &hidden_prefs {
                if seen_hidden_keys.insert(key.clone()) {
                    hidden_list.push(HiddenShelfInfo {
                        shelf_key: key.clone(),
                        title: fallback_hidden_shelf_title(key),
                        subtitle: None,
                    });
                }
            }

            let ordered_metas = order_board_shelves(sortable_metas);

            let mut spots: Vec<HomeSpotlightCandidate> = Vec::new();
            if want_spotlight {
                let mut seen_spot = HashSet::new();
                if let Some(first_cw) = model.continue_watching_preview.items.first() {
                    let li = &first_cw.library_item;
                    if seen_spot.insert(li.id.clone()) {
                        let cached = find_preview_ref_in_model(model, &li.id);
                        spots.push(HomeSpotlightCandidate {
                            id: li.id.clone(),
                            r#type: li.r#type.clone(),
                            name: li.name.clone(),
                            video_id: li.state.video_id.clone(),
                            is_resume: true,
                            eyebrow: "CONTINUE WATCHING".to_owned(),
                            description: cached.and_then(|c| c.description.clone()),
                            year: cached
                                .and_then(|c| c.release_info.clone())
                                .map(|s| s.trim().to_owned())
                                .filter(|s| !s.is_empty()),
                            imdb_rating: cached.and_then(extract_imdb_rating),
                            genres: cached.map(|c| extract_genres(c, 3)).unwrap_or_default(),
                            backdrop_url: cached
                                .and_then(|c| c.background.as_ref().map(|u| u.to_string())),
                            logo_url: cached.and_then(|c| c.logo.as_ref().map(|u| u.to_string())),
                            trailer_youtube_key: None,
                            trailer_streams: cached
                                .map(|c| c.trailer_streams.clone())
                                .unwrap_or_default(),
                        });
                    }
                }

                for meta in &ordered_metas {
                    if spots.len() >= 6 {
                        break;
                    }
                    if let Some(cat) = model.board.catalogs.get(meta.raw_index) {
                        for item in iter_catalog_items(cat).take(3) {
                            if spots.len() >= 6 {
                                break;
                            }
                            if seen_spot.insert(item.id.clone()) {
                                spots.push(HomeSpotlightCandidate {
                                    id: item.id.clone(),
                                    r#type: item.r#type.clone(),
                                    name: item.name.clone(),
                                    video_id: None,
                                    is_resume: false,
                                    eyebrow: format!(
                                        "FEATURED · {}",
                                        meta.title.to_ascii_uppercase()
                                    ),
                                    description: item.description.clone(),
                                    year: item
                                        .release_info
                                        .clone()
                                        .map(|s| s.trim().to_owned())
                                        .filter(|s| !s.is_empty()),
                                    imdb_rating: extract_imdb_rating(item),
                                    genres: extract_genres(item, 3),
                                    backdrop_url: item.background.as_ref().map(|u| u.to_string()),
                                    logo_url: item.logo.as_ref().map(|u| u.to_string()),
                                    trailer_youtube_key: None,
                                    trailer_streams: item.trailer_streams.clone(),
                                });
                            }
                        }
                    }
                }
            }

            let cw_cols = cw_column_count(container_w);
            let cw_rows = if cw.is_empty() {
                0
            } else {
                cw.len().div_ceil(cw_cols)
            };
            let cw_card_w = cw_card_width_px(container_w, cw_cols);
            let cw_img_h = (cw_card_w * PosterAspect::Landscape16x9.height_over_width()).ceil();
            let cw_card_h = cw_img_h + CARD_METADATA_HEIGHT_PX;
            let has_spotlight = want_spotlight && !spots.is_empty();

            let mut y_cursor = initial_shelves_top_y(has_spotlight, cw_rows, cw_card_h);
            let mut rows = Vec::with_capacity(ordered_metas.len());

            for meta in ordered_metas {
                let (card_w, _img_h, card_h) = shelf_card_dimensions(meta.aspect);
                let shelf_total_h = shelf_total_height_px(card_h);
                let shelf_top = y_cursor;
                let shelf_bottom = shelf_top + shelf_total_h;
                y_cursor = shelf_bottom;

                let is_visible_y = shelf_bottom >= (scroll_y - VERTICAL_OVERSCAN_PX)
                    && shelf_top <= (scroll_y + viewport_h + VERTICAL_OVERSCAN_PX);

                let (visible_range, visible_items) = if is_visible_y {
                    let sx = shelf_scroll_x.get(&meta.raw_index).copied().unwrap_or(0.0);
                    let range = compute_horizontal_window(
                        meta.total_items,
                        sx,
                        container_w,
                        card_w,
                        SHELF_CARD_GAP_PX,
                        HORIZONTAL_OVERSCAN_CARDS,
                    );
                    let items = model
                        .board
                        .catalogs
                        .get(meta.raw_index)
                        .map(|cat| slice_catalog_items(cat, range.clone()))
                        .unwrap_or_default();
                    (range, items)
                } else {
                    (0..0, Vec::new())
                };

                rows.push(BoardRowSnapshot {
                    meta,
                    visible_range,
                    visible_items,
                    shelf_total_h,
                    is_visible_y,
                });
            }

            (cw, cw_total, rows, hidden_list, total_catalogs, spots, to_load)
        });

        for idx in pinned_indices_to_load {
            if let std::collections::hash_map::Entry::Vacant(e) = self.board_shelf_scrolls.entry(idx)
            {
                e.insert(ScrollHandle::new());
                self.core.load_board_range(idx..idx);
            }
        }

        let mut visible_shelf_handles: HashMap<usize, ScrollHandle> = HashMap::new();
        for row in &catalog_rows {
            if row.is_visible_y {
                let handle = self
                    .board_shelf_scrolls
                    .entry(row.meta.raw_index)
                    .or_default()
                    .clone();
                visible_shelf_handles.insert(row.meta.raw_index, handle);
            }
        }

        let spotlight_el = if want_spotlight && !spotlight_candidates.is_empty() {
            let spot_count = spotlight_candidates.len();
            let active_idx = self.board_spotlight_idx % spot_count;
            let mut active = spotlight_candidates[active_idx].clone();
            self.request_background_enrichment_if_needed(&active.r#type, &active.id, cx);
            if let Some(enr) = self.core.cached_enriched_meta(&active.id) {
                if active.logo_url.is_none() {
                    active.logo_url = enr.logo_url;
                }
                if active.backdrop_url.is_none() {
                    active.backdrop_url = enr.backdrop_url;
                }
                if active.description.is_none() {
                    active.description = enr.overview;
                }
                if active.year.is_none() {
                    active.year = enr.year;
                }
                if active.imdb_rating.is_none() {
                    active.imdb_rating = enr.imdb_rating;
                }
                if active.genres.is_empty() {
                    active.genres = enr.genres;
                }
                if active.trailer_youtube_key.is_none() {
                    active.trailer_youtube_key = enr.trailer_youtube_key;
                }
            }

            if spot_count > 1 {
                let next_idx = (active_idx + 1) % spot_count;
                let next_spot = &spotlight_candidates[next_idx];
                let next_bg = next_spot
                    .backdrop_url
                    .clone()
                    .or_else(|| {
                        self.core
                            .cached_enriched_meta(&next_spot.id)
                            .and_then(|e| e.backdrop_url)
                    })
                    .or_else(|| {
                        next_spot.id.starts_with("tt").then(|| {
                            format!(
                                "https://images.metahub.space/background/medium/{}/img",
                                next_spot.id
                            )
                        })
                    });
                if let Some(next_bg_url) = next_bg.as_deref() {
                    self.poster_cache.prefetch_hero_backdrop(next_bg_url);
                }
            }

            Some(self.render_home_spotlight_banner(&active, active_idx, spot_count, cx))
        } else {
            None
        };

        let mut kb_cards: Vec<KbFocusableCard> = Vec::new();
        let cw_cols = cw_column_count(container_w);
        let cw_rows = if cw_cards.is_empty() {
            0
        } else {
            cw_cards.len().div_ceil(cw_cols)
        };

        let cw_section_el = if !cw_cards.is_empty() {
            Some(self.render_continue_watching_section(
                &cw_cards,
                cw_total_count,
                container_w,
                &library_statuses,
                &mut kb_cards,
                cx,
            ))
        } else {
            None
        };

        let total_shelves_rendered = catalog_rows.len();
        let mut mounted_shelves_count = 0_usize;
        let mut shelf_elements = Vec::with_capacity(catalog_rows.len());

        for (shelf_pos, row) in catalog_rows.into_iter().enumerate() {
            if row.is_visible_y {
                mounted_shelves_count += 1;
                let scroll_handle = visible_shelf_handles
                    .get(&row.meta.raw_index)
                    .cloned()
                    .unwrap_or_default();
                let row_coord = cw_rows + shelf_pos;
                shelf_elements.push(
                    self.render_editorial_shelf(
                        row,
                        container_w,
                        scroll_handle,
                        &library_statuses,
                        row_coord,
                        &mut kb_cards,
                        cx,
                    )
                    .into_any_element(),
                );
            } else {
                shelf_elements.push(
                    div()
                        .id(("board-shelf-spacer", row.meta.raw_index))
                        .w_full()
                        .h(px(row.shelf_total_h))
                        .into_any_element(),
                );
            }
        }

        let has_virtualized_shelves = total_shelves_rendered > mounted_shelves_count;
        let has_any_content = !cw_cards.is_empty() || total_shelves_rendered > 0;
        let has_more_shelves = shelf_limit < total_board_catalogs;
        let remaining_shelves = total_board_catalogs.saturating_sub(shelf_limit);

        self.set_kb_focus_items(kb_cards);

        div()
            .id("board-view-scroll")
            .track_scroll(&self.board_scroll_handle)
            .size_full()
            .overflow_y_scroll()
            .on_scroll(cx.listener(move |this, ev: &ScrollEvent, _, cx| {
                let down_px = match ev.delta {
                    ScrollDelta::Pixels(p) => (-p.y) / px(1.0),
                    ScrollDelta::Lines(l) => (-l.y) * 36.0,
                };
                let mut should_notify = false;
                if down_px > 0.0
                    && this.visible_board_shelves < total_board_catalogs
                    && scroll_near_bottom(&this.board_scroll_handle, down_px, 420.0)
                {
                    let old = this.visible_board_shelves.max(INITIAL_BOARD_SHELVES);
                    let next = (old + INITIAL_BOARD_SHELVES).min(total_board_catalogs);
                    if next > old {
                        this.visible_board_shelves = next;
                        this.core.load_board_range(old..next);
                        should_notify = true;
                    }
                }
                if down_px.abs() > 0.1 && has_virtualized_shelves {
                    should_notify = true;
                }
                if should_notify {
                    cx.notify();
                }
            }))
            .child(
                div()
                    .w_full()
                    .max_w(px(1440.0))
                    .mx_auto()
                    .px(px(32.0))
                    .pt(px(28.0))
                    .pb(px(72.0))
                    .flex()
                    .flex_col()
                    .when(!has_any_content, |el| {
                        el.child(Self::render_empty_board_banner(cx))
                    })
                    .when_some(spotlight_el, |el, banner| {
                        el.child(div().mb(px(40.0)).child(banner))
                    })
                    .when_some(cw_section_el, |el, cw_sec| {
                        el.child(div().mb(px(40.0)).child(cw_sec))
                    })
                    .children(shelf_elements)
                    .when(has_more_shelves, |el| {
                        el.child(
                            div()
                                .pt(px(4.0))
                                .mb(px(16.0))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(16.0))
                                .child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(format!(
                                            "Showing {} of {total_board_catalogs} shelves",
                                            shelf_limit.min(total_board_catalogs)
                                        )),
                                )
                                .child(
                                    div()
                                        .id("btn-board-show-more-shelves")
                                        .cursor_pointer()
                                        .h(px(36.0))
                                        .px(px(16.0))
                                        .rounded(px(RADIUS_PILL))
                                        .bg(rgba(GLASS_TROUGH_BG))
                                        .border_1()
                                        .border_color(rgba(GLASS_BORDER))
                                        .hover(|s| {
                                            s.bg(rgba(GLASS_ROW_HOVER_BG))
                                                .border_color(rgba(GLASS_BORDER_STRONG))
                                                .text_color(rgb(TEXT_PRIMARY))
                                        })
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .font(ui_sans(FontWeight::MEDIUM))
                                        .text_size(px(13.0))
                                        .text_color(rgb(TEXT_SECONDARY))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                let old = this
                                                    .visible_board_shelves
                                                    .max(INITIAL_BOARD_SHELVES);
                                                let next = (old + INITIAL_BOARD_SHELVES)
                                                    .min(total_board_catalogs);
                                                if next > old {
                                                    this.visible_board_shelves = next;
                                                    this.core.load_board_range(old..next);
                                                    cx.notify();
                                                }
                                            }),
                                        )
                                        .child(format!(
                                            "Show {} more shelves",
                                            remaining_shelves.min(INITIAL_BOARD_SHELVES)
                                        ))
                                        .child(svg_icon(
                                            IconName::ChevronDown,
                                            13.0,
                                            rgb(TEXT_MUTED),
                                        )),
                                ),
                        )
                    })
                    .when(!hidden_shelves.is_empty(), |el| {
                        el.child(self.render_hidden_shelves_drawer(&hidden_shelves, cx))
                    }),
            )
    }

    fn render_home_spotlight_banner(
        &self,
        spot: &HomeSpotlightCandidate,
        active_idx: usize,
        spot_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let metahub_bg = spot
            .id
            .starts_with("tt")
            .then(|| format!("https://images.metahub.space/background/medium/{}/img", spot.id));
        let metahub_logo = spot
            .id
            .starts_with("tt")
            .then(|| format!("https://images.metahub.space/logo/medium/{}/img", spot.id));

        let hero_urls: Vec<&str> = spot
            .backdrop_url
            .as_deref()
            .into_iter()
            .chain(metahub_bg.as_deref())
            .collect();
        let backdrop_tex = self.poster_cache.resolve_first_available_hero(hero_urls);

        let logo_urls: Vec<&str> = spot
            .logo_url
            .as_deref()
            .into_iter()
            .chain(metahub_logo.as_deref())
            .collect();
        let logo_tex = self.poster_cache.resolve_first_available_logo(logo_urls);

        let trailer_target = resolve_spotlight_trailer_target(spot);

        let play_type = spot.r#type.clone();
        let play_id = spot.id.clone();
        let play_vid = spot.video_id.clone();
        let is_resume = spot.is_resume;
        let det_type = spot.r#type.clone();
        let det_id = spot.id.clone();
        let det_vid = spot.video_id.clone();

        div()
            .id("home-editorial-spotlight")
            .w_full()
            .h(px(336.0))
            .rounded(px(14.0))
            .overflow_hidden()
            .relative()
            .bg(rgb(placeholder_tint_for_id(&spot.id)))
            .border_1()
            .border_color(rgba(GLASS_BORDER))
            .when_some(backdrop_tex, |el, tex| {
                el.child(
                    img(tex)
                        .size_full()
                        .absolute()
                        .inset_0()
                        .object_fit(ObjectFit::Cover),
                )
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(linear_gradient(
                        180.0,
                        linear_color_stop(rgba(0x0b0b0d2e), 0.1),
                        linear_color_stop(rgba(0x0b0b0df2), 1.0),
                    )),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(linear_gradient(
                        90.0,
                        linear_color_stop(rgba(0x0b0b0de8), 0.25),
                        linear_color_stop(rgba(0x0b0b0d1f), 0.95),
                    )),
            )
            .child(
                div()
                    .relative()
                    .size_full()
                    .p(px(32.0))
                    .flex()
                    .items_end()
                    .justify_between()
                    .gap(px(24.0))
                    .child(
                        div()
                            .max_w(px(640.0))
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(8.0))
                                    .font(mono_font())
                                    .text_xs()
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(ACCENT))
                                            .child(spot.eyebrow.clone()),
                                    )
                                    .when_some(spot.year.clone(), |el, yr| {
                                        el.child(meta_divider()).child(yr)
                                    })
                                    .when_some(spot.imdb_rating.clone(), |el, r| {
                                        el.child(meta_divider()).child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(4.0))
                                                .text_color(rgb(TEXT_PRIMARY))
                                                .child(svg_icon(
                                                    IconName::Star,
                                                    11.5,
                                                    rgb(ACCENT_AMBER),
                                                ))
                                                .child(r),
                                        )
                                    })
                                    .when(!spot.genres.is_empty(), |el| {
                                        el.child(meta_divider()).child(spot.genres.join(" · "))
                                    }),
                            )
                            .child(if let Some(ltex) = logo_tex {
                                img(ltex)
                                    .h(px(78.0))
                                    .max_w(px(380.0))
                                    .object_fit(ObjectFit::Contain)
                                    .into_any_element()
                            } else {
                                div()
                                    .font(editorial_serif())
                                    .text_size(px(46.0))
                                    .line_height(gpui::relative(1.04))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .truncate()
                                    .child(spot.name.clone())
                                    .into_any_element()
                            })
                            .when_some(spot.description.clone(), |el, desc| {
                                el.child(
                                    div()
                                        .max_w(px(560.0))
                                        .text_size(px(14.0))
                                        .text_color(rgb(TEXT_SECONDARY))
                                        .max_h(px(42.0))
                                        .overflow_hidden()
                                        .child(desc),
                                )
                            })
                            .child(
                                div()
                                    .mt(px(4.0))
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(10.0))
                                    .child(
                                        primary_accent_button(
                                            "spotlight-primary-btn",
                                            Some(IconName::Play),
                                            if is_resume { "Resume" } else { "Watch" },
                                            cx.listener(move |this, _, _, cx| {
                                                this.quick_resume_or_open(
                                                    play_type.clone(),
                                                    play_id.clone(),
                                                    play_vid.clone(),
                                                    cx,
                                                );
                                            }),
                                        )
                                        .px(px(20.0))
                                        .rounded(px(RADIUS_PILL)),
                                    )
                                    .when_some(trailer_target, |row, target| {
                                        let tr_type = spot.r#type.clone();
                                        let tr_id = spot.id.clone();
                                        let tr_name = spot.name.clone();
                                        row.child(
                                            secondary_glass_button(
                                                "spotlight-trailer-btn",
                                                Some(IconName::Clapperboard),
                                                "Trailer",
                                                false,
                                                cx.listener(move |this, _, _, cx| match &target {
                                                    SpotlightTrailerTarget::DirectStream(
                                                        stream,
                                                    ) => {
                                                        let addons = this.core.with_model(|m| {
                                                            m.ctx.profile.addons.clone()
                                                        });
                                                        let cand =
                                                            PlaybackCandidate::from_stream_and_request(
                                                                stream.as_ref().clone(),
                                                                None,
                                                                &addons,
                                                            );
                                                        this.start_playback_session(
                                                            tr_type.clone(),
                                                            tr_id.clone(),
                                                            tr_id.clone(),
                                                            format!("{tr_name} · Trailer"),
                                                            Some("Trailer".to_owned()),
                                                            None,
                                                            None,
                                                            Arc::new(vec![cand]),
                                                            0,
                                                            None,
                                                            cx,
                                                        );
                                                    }
                                                    SpotlightTrailerTarget::YouTubeUrl(url) => {
                                                        cx.open_url(url);
                                                    }
                                                }),
                                            )
                                            .rounded(px(RADIUS_PILL)),
                                        )
                                    })
                                    .child(
                                        secondary_glass_button(
                                            "spotlight-details-btn",
                                            Some(IconName::Info),
                                            "Details",
                                            false,
                                            cx.listener(move |this, _, _, cx| {
                                                this.open_meta_details(
                                                    det_type.clone(),
                                                    det_id.clone(),
                                                    det_vid.clone(),
                                                    cx,
                                                );
                                            }),
                                        )
                                        .rounded(px(RADIUS_PILL)),
                                    ),
                            ),
                    )
                    .when(spot_count > 1, |el| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .id("spotlight-prev")
                                        .cursor_pointer()
                                        .w(px(32.0))
                                        .h(px(32.0))
                                        .rounded(px(RADIUS_PILL))
                                        .bg(rgba(0x141417cc))
                                        .border_1()
                                        .border_color(rgba(GLASS_BORDER))
                                        .hover(|s| {
                                            s.bg(rgb(BG_ELEVATED))
                                                .border_color(rgba(GLASS_BORDER_STRONG))
                                        })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.board_spotlight_idx =
                                                    (this.board_spotlight_idx + spot_count - 1)
                                                        % spot_count;
                                                cx.notify();
                                            }),
                                        )
                                        .child(svg_icon(
                                            IconName::ChevronLeft,
                                            14.0,
                                            rgb(TEXT_PRIMARY),
                                        )),
                                )
                                .child(
                                    div()
                                        .px(px(6.0))
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(format!("{}/{}", active_idx + 1, spot_count)),
                                )
                                .child(
                                    div()
                                        .id("spotlight-next")
                                        .cursor_pointer()
                                        .w(px(32.0))
                                        .h(px(32.0))
                                        .rounded(px(RADIUS_PILL))
                                        .bg(rgba(0x141417cc))
                                        .border_1()
                                        .border_color(rgba(GLASS_BORDER))
                                        .hover(|s| {
                                            s.bg(rgb(BG_ELEVATED))
                                                .border_color(rgba(GLASS_BORDER_STRONG))
                                        })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.board_spotlight_idx =
                                                    (this.board_spotlight_idx + 1) % spot_count;
                                                cx.notify();
                                            }),
                                        )
                                        .child(svg_icon(
                                            IconName::ChevronRight,
                                            14.0,
                                            rgb(TEXT_PRIMARY),
                                        )),
                                ),
                        )
                    }),
            )
    }

    fn render_continue_watching_section(
        &self,
        items: &[ContinueWatchingCard],
        total_cw_count: usize,
        container_w: f32,
        library_statuses: &HashMap<String, LibraryStatusTab>,
        kb_cards: &mut Vec<KbFocusableCard>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let cols = cw_column_count(container_w);
        let card_w = cw_card_width_px(container_w, cols);
        let img_h = (card_w * PosterAspect::Landscape16x9.height_over_width()).ceil();

        let mut cw_els: Vec<AnyElement> = Vec::with_capacity(items.len());
        for (idx, cw) in items.iter().enumerate() {
            let li = &cw.library_item;
            let enr = if cw.backdrop_url.is_none() || cw.imdb_rating.is_none() {
                self.core.cached_enriched_meta(&li.id)
            } else {
                None
            };
            let metahub_bg = li.id.starts_with("tt").then(|| {
                format!(
                    "https://images.metahub.space/background/medium/{}/img",
                    li.id
                )
            });
            let wide_url = cw
                .backdrop_url
                .clone()
                .or_else(|| enr.as_ref().and_then(|e| e.backdrop_url.clone()))
                .or(metahub_bg);
            let wide_tex = self.poster_cache.resolve_widescreen_or_poster(
                wide_url.as_deref(),
                li.poster.as_ref().map(|u| u.as_str()),
            );

            let ep_code = format_movietracker_episode_code(li.state.video_id.as_deref());
            let pct = li.progress().clamp(0.0, 100.0) as f32;
            let progress_fraction = if li.state.time_offset > 0 {
                Some((pct / 100.0).max(0.02))
            } else {
                None
            };
            let subtitle =
                format_cw_subtitle(li, ep_code.as_deref(), cw.notification_count, pct);

            let lib_status = library_statuses.get(&li.id).copied();
            let in_library = !li.removed && !li.temp;
            let is_watched = li.watched();
            let imdb_rating = cw
                .imdb_rating
                .clone()
                .or_else(|| enr.as_ref().and_then(|e| e.imdb_rating.clone()))
                .map(SharedString::from);

            let kb_idx = kb_cards.len();
            let is_focused = self.kb_focus_index == Some(kb_idx);
            kb_cards.push(KbFocusableCard {
                route: AppRoute::MetaDetails {
                    type_name: li.r#type.clone(),
                    id: li.id.clone(),
                    video_id: li.state.video_id.clone(),
                },
                row: idx / cols,
                col: idx % cols,
            });

            let det_type = li.r#type.clone();
            let det_id = li.id.clone();
            let det_vid = li.state.video_id.clone();

            let play_type = li.r#type.clone();
            let play_id = li.id.clone();
            let play_vid = li.state.video_id.clone();

            let ctx_type = li.r#type.clone();
            let ctx_id = li.id.clone();
            let ctx_vid = li.state.video_id.clone();
            let ctx_title = li.name.clone();
            let ctx_preview = cw.preview.clone();

            let hover_type = li.r#type.clone();
            let hover_id = li.id.clone();
            let hover_bg = wide_url;

            cw_els.push(render_poster_card(
                PosterCardProps::new(("cw-card", idx), li.name.clone(), li.id.clone())
                    .subtitle(Some(SharedString::from(subtitle)))
                    .image(wide_tex)
                    .aspect(PosterAspect::Landscape16x9)
                    .image_height_px(Some(img_h))
                    .status_tab(lib_status)
                    .notification_count(cw.notification_count)
                    .imdb_rating(imdb_rating)
                    .progress_fraction(progress_fraction)
                    .in_library(in_library)
                    .is_focused(is_focused)
                    .on_open(cx.listener(move |this, _, _, cx| {
                        this.open_meta_details(
                            det_type.clone(),
                            det_id.clone(),
                            det_vid.clone(),
                            cx,
                        );
                    }))
                    .on_quick_play(cx.listener(move |this, _, _, cx| {
                        this.quick_resume_or_open(
                            play_type.clone(),
                            play_id.clone(),
                            play_vid.clone(),
                            cx,
                        );
                    }))
                    .on_context_menu(cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        this.open_browsing_context_menu(
                            BrowsingContextMenu::new(
                                ev.position,
                                ctx_type.clone(),
                                ctx_id.clone(),
                                ctx_title.clone(),
                            )
                            .video_id(ctx_vid.clone())
                            .preview(ctx_preview.clone())
                            .is_in_library(in_library)
                            .is_watched(is_watched)
                            .can_rewind_cw(true),
                            cx,
                        );
                    }))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.on_card_hover_prefetch(
                            &hover_type,
                            &hover_id,
                            hover_bg.as_deref(),
                            *hovered,
                            cx,
                        );
                    })),
            ));
        }

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .h(px(40.0))
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .child(
                        div()
                            .font(editorial_serif())
                            .text_size(px(32.0))
                            .text_color(rgb(TEXT_PRIMARY))
                            .child("Pick up where you left off"),
                    )
                    .child(
                        div()
                            .id("cw-open-library")
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_size(px(13.0))
                            .text_color(rgb(TEXT_SECONDARY))
                            .hover(|s| s.text_color(rgb(TEXT_PRIMARY)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.library_status_tab = LibraryStatusTab::Watching;
                                    this.navigate_tab(SidebarTab::Library, cx);
                                }),
                            )
                            .child(format!("{total_cw_count} in progress"))
                            .child(svg_icon(IconName::ChevronRight, 13.0, rgb(TEXT_MUTED))),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .grid()
                    .grid_cols(cols as u16)
                    .gap(px(SHELF_CARD_GAP_PX))
                    .children(cw_els),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_editorial_shelf(
        &self,
        row: BoardRowSnapshot,
        container_w: f32,
        shelf_scroll: ScrollHandle,
        library_statuses: &HashMap<String, LibraryStatusTab>,
        kb_row: usize,
        kb_cards: &mut Vec<KbFocusableCard>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let meta = row.meta;
        let raw_index = meta.raw_index;
        let total_items = meta.total_items;
        let can_load_more = meta.can_load_more;
        let is_loading = meta.is_loading;
        let is_pinned = meta.is_pinned;

        let (card_w, img_h, card_h) = shelf_card_dimensions(meta.aspect);
        let stride = card_w + SHELF_CARD_GAP_PX;
        let step_px = (container_w * 0.72).max(stride * 2.0);

        let vis_start = row.visible_range.start.min(total_items);
        let vis_end = row.visible_range.end.min(total_items);
        let left_pad = vis_start as f32 * stride;
        let right_pad = if total_items > vis_end {
            total_items.saturating_sub(vis_end) as f32 * stride
        } else {
            0.0
        };

        let scroll_prev = shelf_scroll.clone();
        let scroll_next = shelf_scroll.clone();
        let scroll_ev = shelf_scroll.clone();

        let pin_key = meta.shelf_key.clone();
        let hide_key = meta.shelf_key.clone();
        let see_all_req = meta.see_all_request.clone();

        let body_el = if total_items == 0 && is_loading {
            div()
                .w_full()
                .h(px(card_h + 4.0))
                .overflow_hidden()
                .flex()
                .items_start()
                .gap(px(SHELF_CARD_GAP_PX))
                .children((0..7).map(|_| {
                    div()
                        .w(px(card_w))
                        .h(px(img_h))
                        .flex_shrink_0()
                        .rounded(px(RADIUS_POSTER))
                        .bg(rgba(GLASS_CARD_BG))
                        .border_1()
                        .border_color(rgba(GLASS_BORDER))
                }))
                .into_any_element()
        } else {
            let mut card_els: Vec<AnyElement> = Vec::with_capacity(row.visible_items.len());
            for (rel_idx, item) in row.visible_items.into_iter().enumerate() {
                let item_idx = vis_start + rel_idx;
                let item_aspect = PosterAspect::from_stremio_poster_shape(&item.poster_shape);
                let lib_status = library_statuses.get(&item.id).copied();
                let in_library = lib_status.is_some();
                let is_watched = matches!(lib_status, Some(LibraryStatusTab::Watched));

                let image_tex = if item_aspect == PosterAspect::Landscape16x9 {
                    self.poster_cache.resolve_widescreen_or_poster(
                        item.background.as_ref().map(|u| u.as_str()),
                        item.poster.as_ref().map(|u| u.as_str()),
                    )
                } else {
                    self.poster_cache.resolve_poster_url(item.poster.as_ref())
                };

                let rating = extract_imdb_rating(&item).map(SharedString::from);
                let genres = extract_genres(&item, 1);
                let year = item
                    .release_info
                    .clone()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty());
                let subtitle = format_preview_subtitle(
                    &item.r#type,
                    year.as_deref(),
                    genres.first().map(String::as_str),
                );

                let kb_idx = kb_cards.len();
                let is_focused = self.kb_focus_index == Some(kb_idx);
                kb_cards.push(KbFocusableCard {
                    route: AppRoute::MetaDetails {
                        type_name: item.r#type.clone(),
                        id: item.id.clone(),
                        video_id: None,
                    },
                    row: kb_row,
                    col: item_idx,
                });

                let open_type = item.r#type.clone();
                let open_id = item.id.clone();
                let play_type = item.r#type.clone();
                let play_id = item.id.clone();
                let toggle_preview = item.clone();
                let ctx_type = item.r#type.clone();
                let ctx_id = item.id.clone();
                let ctx_title = item.name.clone();
                let ctx_preview = item.clone();
                let hover_type = item.r#type.clone();
                let hover_id = item.id.clone();
                let hover_bg = item.background.as_ref().map(|u| u.to_string());

                let mut props = PosterCardProps::new(
                    ("board-poster", raw_index * 10_000 + item_idx),
                    item.name,
                    item.id,
                )
                .subtitle(Some(SharedString::from(subtitle)))
                .image(image_tex)
                .aspect(item_aspect)
                .fixed_width_px(Some(card_w))
                .status_tab(lib_status)
                .imdb_rating(rating)
                .in_library(in_library)
                .is_focused(is_focused)
                .on_open(cx.listener(move |this, _, _, cx| {
                    this.open_meta_details(open_type.clone(), open_id.clone(), None, cx);
                }))
                .on_quick_play(cx.listener(move |this, _, _, cx| {
                    this.quick_resume_or_open(play_type.clone(), play_id.clone(), None, cx);
                }))
                .on_toggle_library(cx.listener(move |this, _, _, cx| {
                    this.core.toggle_library_item(toggle_preview.clone());
                    cx.notify();
                }))
                .on_context_menu(cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    this.open_browsing_context_menu(
                        BrowsingContextMenu::new(
                            ev.position,
                            ctx_type.clone(),
                            ctx_id.clone(),
                            ctx_title.clone(),
                        )
                        .preview(Some(ctx_preview.clone()))
                        .is_in_library(in_library)
                        .is_watched(is_watched),
                        cx,
                    );
                }))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    this.on_card_hover_prefetch(
                        &hover_type,
                        &hover_id,
                        hover_bg.as_deref(),
                        *hovered,
                        cx,
                    );
                }));

                if item_aspect != meta.aspect {
                    props = props.image_height_px(Some(img_h));
                }

                card_els.push(render_poster_card(props));
            }

            div()
                .id(("board-shelf-carousel", raw_index))
                .w_full()
                .h(px(card_h + 4.0))
                .overflow_x_scroll()
                .track_scroll(&shelf_scroll)
                .on_scroll(cx.listener(move |this, ev: &ScrollEvent, _, cx| {
                    let right_px = match ev.delta {
                        ScrollDelta::Pixels(p) => -f32::from(p.x),
                        ScrollDelta::Lines(l) => (-l.x) * 48.0,
                    };
                    if right_px.abs() < 0.1 {
                        return;
                    }
                    let max_x = (scroll_ev.max_offset().x / px(1.0)).max(0.0);
                    let cur_x = (-scroll_ev.offset().x / px(1.0)).max(0.0);
                    if right_px > 0.0
                        && can_load_more
                        && !is_loading
                        && max_x > 0.0
                        && cur_x + right_px + 320.0 >= max_x
                    {
                        this.core.load_board_next_page(raw_index);
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .h_full()
                        .flex()
                        .items_start()
                        .gap(px(SHELF_CARD_GAP_PX))
                        .pl(px(left_pad))
                        .pr(px(right_pad))
                        .children(card_els)
                        .when(is_loading && vis_end >= total_items, |row_el| {
                            row_el.child(
                                div()
                                    .w(px(card_w))
                                    .h(px(img_h))
                                    .flex_shrink_0()
                                    .rounded(px(RADIUS_POSTER))
                                    .bg(rgba(GLASS_CARD_BG))
                                    .border_1()
                                    .border_color(rgba(GLASS_BORDER)),
                            )
                        }),
                )
                .into_any_element()
        };

        div()
            .id(("board-shelf", raw_index))
            .w_full()
            .h(px(row.shelf_total_h))
            .pb(px(SHELF_BOTTOM_GAP_PX))
            .flex()
            .flex_col()
            .gap(px(SHELF_HEADER_GAP_PX))
            .child(
                div()
                    .h(px(SHELF_HEADER_HEIGHT_PX))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.0))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex()
                            .items_baseline()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .font(editorial_serif())
                                    .text_size(px(30.0))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .truncate()
                                    .child(meta.title),
                            )
                            .when_some(meta.subtitle, |el, sub| {
                                el.child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(rgb(TEXT_MUTED))
                                        .truncate()
                                        .child(sub),
                                )
                            })
                            .when(is_pinned, |el| {
                                el.child(
                                    div()
                                        .px(px(8.0))
                                        .py(px(2.0))
                                        .rounded(px(RADIUS_PILL))
                                        .bg(rgba(ACCENT_SOFT))
                                        .border_1()
                                        .border_color(rgba(BORDER_ACCENT))
                                        .font(mono_font())
                                        .text_size(px(10.5))
                                        .text_color(rgb(ACCENT))
                                        .child("Pinned"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(render_shelf_icon_button(
                                ("shelf-prev", raw_index),
                                IconName::ChevronLeft,
                                false,
                                cx.listener(move |_, _, _, cx| {
                                    let cur_x = (-scroll_prev.offset().x / px(1.0)).max(0.0);
                                    let next_x = (cur_x - step_px).max(0.0);
                                    scroll_prev.set_offset(point(px(-next_x), px(0.0)));
                                    cx.notify();
                                }),
                            ))
                            .child(render_shelf_icon_button(
                                ("shelf-next", raw_index),
                                IconName::ChevronRight,
                                false,
                                cx.listener(move |this, _, _, cx| {
                                    let max_x =
                                        (scroll_next.max_offset().x / px(1.0)).max(0.0);
                                    let cur_x = (-scroll_next.offset().x / px(1.0)).max(0.0);
                                    let next_x = (cur_x + step_px).min(max_x);
                                    scroll_next.set_offset(point(px(-next_x), px(0.0)));
                                    if can_load_more
                                        && !is_loading
                                        && (max_x <= 0.0 || next_x + 320.0 >= max_x)
                                    {
                                        this.core.load_board_next_page(raw_index);
                                    }
                                    cx.notify();
                                }),
                            ))
                            .child(render_shelf_icon_button(
                                ("shelf-pin", raw_index),
                                IconName::Pin,
                                is_pinned,
                                cx.listener(move |this, _, _, cx| {
                                    this.toggle_pin_board_shelf(pin_key.clone(), cx);
                                }),
                            ))
                            .child(render_shelf_icon_button(
                                ("shelf-hide", raw_index),
                                IconName::EyeOff,
                                false,
                                cx.listener(move |this, _, _, cx| {
                                    this.toggle_hide_board_shelf(hide_key.clone(), cx);
                                }),
                            ))
                            .child(
                                div()
                                    .id(("shelf-see-all", raw_index))
                                    .cursor_pointer()
                                    .h(px(30.0))
                                    .px(px(11.0))
                                    .ml(px(4.0))
                                    .rounded(px(RADIUS_PILL))
                                    .bg(rgba(GLASS_TROUGH_BG))
                                    .border_1()
                                    .border_color(rgba(GLASS_BORDER))
                                    .hover(|s| {
                                        s.bg(rgba(GLASS_ROW_HOVER_BG))
                                            .border_color(rgba(GLASS_BORDER_STRONG))
                                            .text_color(rgb(TEXT_PRIMARY))
                                    })
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(px(12.5))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.open_discover_catalog(
                                                CatalogWithFiltersSelected {
                                                    request: see_all_req.clone(),
                                                },
                                                cx,
                                            );
                                        }),
                                    )
                                    .child("See all")
                                    .child(svg_icon(
                                        IconName::ChevronRight,
                                        12.5,
                                        rgb(TEXT_MUTED),
                                    )),
                            ),
                    ),
            )
            .child(body_el)
    }

    fn render_hidden_shelves_drawer(
        &self,
        hidden_shelves: &[HiddenShelfInfo],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let count = hidden_shelves.len();
        let is_open = self.board_hidden_drawer_open;

        div()
            .pt(px(8.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .id("btn-board-hidden-drawer")
                            .cursor_pointer()
                            .h(px(36.0))
                            .px(px(14.0))
                            .rounded(px(RADIUS_PILL))
                            .bg(rgba(GLASS_TROUGH_BG))
                            .border_1()
                            .border_color(if is_open {
                                rgba(BORDER_ACCENT)
                            } else {
                                rgba(GLASS_BORDER)
                            })
                            .hover(|s| {
                                s.bg(rgba(GLASS_ROW_HOVER_BG))
                                    .border_color(rgba(GLASS_BORDER_STRONG))
                            })
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(px(13.0))
                            .text_color(if is_open {
                                rgb(TEXT_PRIMARY)
                            } else {
                                rgb(TEXT_SECONDARY)
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.board_hidden_drawer_open = !this.board_hidden_drawer_open;
                                    cx.notify();
                                }),
                            )
                            .child(svg_icon(
                                if is_open {
                                    IconName::Eye
                                } else {
                                    IconName::EyeOff
                                },
                                13.5,
                                rgb(TEXT_MUTED),
                            ))
                            .child(format!("Hidden shelves ({count})"))
                            .child(svg_icon(
                                IconName::ChevronDown,
                                13.0,
                                rgb(TEXT_MUTED),
                            )),
                    )
                    .when(is_open, |row| {
                        row.child(
                            secondary_glass_button(
                                "btn-board-restore-all-shelves",
                                Some(IconName::RefreshCw),
                                "Restore all",
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.restore_all_hidden_shelves(cx);
                                }),
                            )
                            .h(px(34.0))
                            .rounded(px(RADIUS_PILL)),
                        )
                    }),
            )
            .when(is_open, |el| {
                el.child(
                    div()
                        .w_full()
                        .p(px(16.0))
                        .rounded(px(RADIUS_SHEET))
                        .bg(rgba(GLASS_SHEET_BG))
                        .border_1()
                        .border_color(rgba(GLASS_BORDER))
                        .flex()
                        .flex_wrap()
                        .gap(px(10.0))
                        .children(hidden_shelves.iter().enumerate().map(|(idx, info)| {
                            let restore_key = info.shelf_key.clone();
                            let label = match &info.subtitle {
                                Some(sub) => format!("{} · {}", info.title, sub),
                                None => info.title.clone(),
                            };
                            div()
                                .id(("btn-board-restore-shelf", idx))
                                .cursor_pointer()
                                .h(px(34.0))
                                .px(px(12.0))
                                .rounded(px(RADIUS_CONTROL))
                                .bg(rgba(GLASS_CARD_BG))
                                .border_1()
                                .border_color(rgba(GLASS_BORDER))
                                .hover(|s| {
                                    s.bg(rgba(GLASS_ROW_HOVER_BG))
                                        .border_color(rgba(GLASS_BORDER_STRONG))
                                })
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .font(ui_sans(FontWeight::MEDIUM))
                                .text_size(px(12.5))
                                .text_color(rgb(TEXT_PRIMARY))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.toggle_hide_board_shelf(restore_key.clone(), cx);
                                    }),
                                )
                                .child(svg_icon(IconName::Eye, 13.0, rgb(ACCENT)))
                                .child(label)
                        })),
                )
            })
    }

    fn render_empty_board_banner(cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .p(px(36.0))
            .rounded(px(RADIUS_SHEET))
            .bg(rgba(GLASS_SHEET_BG))
            .border_1()
            .border_color(rgba(GLASS_BORDER))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .font(editorial_serif())
                    .text_size(px(34.0))
                    .text_color(rgb(TEXT_PRIMARY))
                    .child("Setting the projection room"),
            )
            .child(
                div()
                    .max_w(px(540.0))
                    .text_size(px(14.0))
                    .text_color(rgb(TEXT_SECONDARY))
                    .child(
                        "Sign in to sync your Stremio library and addons, or press Ctrl K to search any film or series immediately.",
                    ),
            )
            .child(
                div()
                    .mt(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        primary_accent_button(
                            "empty-btn-signin",
                            Some(IconName::User),
                            "Sign in",
                            cx.listener(|this, _, _, cx| {
                                this.navigate_tab(SidebarTab::Account, cx);
                            }),
                        )
                        .rounded(px(RADIUS_CARD)),
                    )
                    .child(
                        secondary_glass_button(
                            "empty-btn-search",
                            Some(IconName::Search),
                            "Search catalog",
                            false,
                            cx.listener(|this, _, _, cx| {
                                this.open_command_palette(cx);
                            }),
                        )
                        .rounded(px(RADIUS_CARD)),
                    ),
            )
    }

    pub fn render_poster_card(
        &self,
        element_id: impl Into<ElementId>,
        item: MetaItemPreview,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let lib_status = self.cached_library_statuses().get(&item.id).copied();
        let group_key = format!("poster-card-{}", item.id);
        self.render_movietracker_poster_card(element_id, group_key, item, lib_status, cx)
    }

    pub fn render_movietracker_poster_card(
        &self,
        element_id: impl Into<ElementId>,
        _group_key: String,
        item: MetaItemPreview,
        lib_status: Option<LibraryStatusTab>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let poster_tex = self.poster_cache.resolve_poster_url(item.poster.as_ref());
        let rating = extract_imdb_rating(&item).map(SharedString::from);
        let genres = extract_genres(&item, 1);
        let year = item
            .release_info
            .clone()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        let subtitle = format_preview_subtitle(
            &item.r#type,
            year.as_deref(),
            genres.first().map(String::as_str),
        );

        let open_type = item.r#type.clone();
        let open_id = item.id.clone();
        let toggle_preview = item.clone();
        let ctx_type = item.r#type.clone();
        let ctx_id = item.id.clone();
        let ctx_title = item.name.clone();
        let ctx_preview = item.clone();
        let hover_type = item.r#type.clone();
        let hover_id = item.id.clone();
        let hover_bg = item.background.as_ref().map(|u| u.to_string());
        let in_library = lib_status.is_some();
        let is_watched = matches!(lib_status, Some(LibraryStatusTab::Watched));

        render_poster_card(
            PosterCardProps::new(element_id, item.name, item.id)
                .subtitle(Some(SharedString::from(subtitle)))
                .image(poster_tex)
                .aspect(PosterAspect::Portrait2x3)
                .status_tab(lib_status)
                .imdb_rating(rating)
                .in_library(in_library)
                .on_open(cx.listener(move |this, _, _, cx| {
                    this.open_meta_details(open_type.clone(), open_id.clone(), None, cx);
                }))
                .on_toggle_library(cx.listener(move |this, _, _, cx| {
                    this.core.toggle_library_item(toggle_preview.clone());
                    cx.notify();
                }))
                .on_context_menu(cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    this.open_browsing_context_menu(
                        BrowsingContextMenu::new(
                            ev.position,
                            ctx_type.clone(),
                            ctx_id.clone(),
                            ctx_title.clone(),
                        )
                        .preview(Some(ctx_preview.clone()))
                        .is_in_library(in_library)
                        .is_watched(is_watched),
                        cx,
                    );
                }))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    this.on_card_hover_prefetch(
                        &hover_type,
                        &hover_id,
                        hover_bg.as_deref(),
                        *hovered,
                        cx,
                    );
                })),
        )
    }
}

fn render_shelf_icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .cursor_pointer()
        .w(px(30.0))
        .h(px(30.0))
        .rounded(px(RADIUS_PILL))
        .bg(if active {
            rgba(ACCENT_SOFT)
        } else {
            rgba(GLASS_TROUGH_BG)
        })
        .border_1()
        .border_color(if active {
            rgba(BORDER_ACCENT)
        } else {
            rgba(GLASS_BORDER)
        })
        .hover(|s| {
            s.bg(if active {
                rgba(ACCENT_SOFT)
            } else {
                rgba(GLASS_ROW_HOVER_BG)
            })
            .border_color(if active {
                rgb(ACCENT)
            } else {
                rgba(GLASS_BORDER_STRONG)
            })
        })
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(svg_icon(
            icon,
            13.0,
            if active {
                rgb(ACCENT)
            } else {
                rgb(TEXT_SECONDARY)
            },
        ))
}

fn find_preview_ref_in_model<'a>(
    model: &'a DesktopModel,
    route_id: &str,
) -> Option<&'a MetaItemPreview> {
    for cat in &model.board.catalogs {
        for page in cat {
            if let Some(Loadable::Ready(items)) = &page.content {
                if let Some(found) = items.iter().find(|p| p.id == route_id) {
                    return Some(found);
                }
            }
        }
    }
    for page in &model.discover.catalog {
        if let Some(Loadable::Ready(items)) = &page.content {
            if let Some(found) = items.iter().find(|p| p.id == route_id) {
                return Some(found);
            }
        }
    }
    for cat in &model.search.catalogs {
        for page in cat {
            if let Some(Loadable::Ready(items)) = &page.content {
                if let Some(found) = items.iter().find(|p| p.id == route_id) {
                    return Some(found);
                }
            }
        }
    }
    None
}

fn iter_catalog_items(
    cat: &[stremio_core_runtime::stremio_core::models::common::ResourceLoadable<Vec<MetaItemPreview>>],
) -> impl Iterator<Item = &MetaItemPreview> {
    cat.iter().flat_map(|page| match &page.content {
        Some(Loadable::Ready(items)) => items.as_slice(),
        _ => &[],
    })
}

fn slice_catalog_items(
    cat: &[stremio_core_runtime::stremio_core::models::common::ResourceLoadable<Vec<MetaItemPreview>>],
    range: Range<usize>,
) -> Vec<MetaItemPreview> {
    if range.start >= range.end {
        return Vec::new();
    }
    let count = range.end - range.start;
    iter_catalog_items(cat)
        .skip(range.start)
        .take(count)
        .cloned()
        .collect()
}

fn strip_skip_extra(req: &ResourceRequest) -> ResourceRequest {
    let mut cloned = req.clone();
    cloned.path.extra.retain(|ExtraValue { name, .. }| !name.eq_ignore_ascii_case("skip"));
    cloned
}

fn build_shelf_key(base_url: &str, raw_type: &str, raw_id: &str) -> String {
    format!("{base_url}|{raw_type}|{raw_id}")
}

fn fallback_hidden_shelf_title(shelf_key: &str) -> String {
    let mut parts = shelf_key.split('|');
    let _base = parts.next();
    let raw_type = parts.next().unwrap_or("");
    let raw_id = parts.next().unwrap_or(shelf_key);
    let clean = sentence_case_label(raw_id);
    if raw_type.is_empty() || clean.to_ascii_lowercase().contains(&raw_type.to_ascii_lowercase()) {
        clean
    } else {
        format!("{clean} {}", raw_type.to_ascii_lowercase())
    }
}

fn order_board_shelves(mut items: Vec<(Option<usize>, BoardShelfMeta)>) -> Vec<BoardShelfMeta> {
    items.sort_by(|(pin_a, meta_a), (pin_b, meta_b)| match (pin_a, pin_b) {
        (Some(pa), Some(pb)) => pa.cmp(pb),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => meta_a.raw_index.cmp(&meta_b.raw_index),
    });
    items.into_iter().map(|(_, meta)| meta).collect()
}

fn compute_horizontal_window(
    total_items: usize,
    scroll_x_px: f32,
    viewport_w_px: f32,
    card_w_px: f32,
    gap_px: f32,
    overscan: usize,
) -> Range<usize> {
    if total_items == 0
        || !scroll_x_px.is_finite()
        || !viewport_w_px.is_finite()
        || !card_w_px.is_finite()
        || viewport_w_px <= 0.0
        || card_w_px <= 0.0
    {
        return 0..0;
    }
    let stride = (card_w_px + gap_px.max(0.0)).max(1.0);
    let sx = scroll_x_px.max(0.0);
    let first_visible = (sx / stride).floor() as usize;
    let last_visible_excl = ((sx + viewport_w_px) / stride).ceil() as usize;
    let start = first_visible.saturating_sub(overscan).min(total_items);
    let end = last_visible_excl.saturating_add(overscan).min(total_items);
    start..end.max(start)
}

#[inline]
fn cw_column_count(container_w: f32) -> usize {
    if container_w < 700.0 {
        2
    } else {
        3
    }
}

#[inline]
fn cw_card_width_px(container_w: f32, cols: usize) -> f32 {
    let c = cols.max(1) as f32;
    ((container_w - SHELF_CARD_GAP_PX * (c - 1.0)) / c).max(160.0)
}

#[inline]
fn shelf_card_dimensions(aspect: PosterAspect) -> (f32, f32, f32) {
    let card_w = match aspect {
        PosterAspect::Portrait2x3 => 176.0,
        PosterAspect::Square1x1 => 196.0,
        PosterAspect::Landscape16x9 => 272.0,
    };
    let img_h = (card_w * aspect.height_over_width()).ceil();
    let card_h = img_h + CARD_METADATA_HEIGHT_PX;
    (card_w, img_h, card_h)
}

#[inline]
fn shelf_total_height_px(card_h: f32) -> f32 {
    SHELF_HEADER_HEIGHT_PX + SHELF_HEADER_GAP_PX + (card_h + 4.0) + SHELF_BOTTOM_GAP_PX
}

#[inline]
fn initial_shelves_top_y(has_spotlight: bool, cw_rows: usize, cw_card_h: f32) -> f32 {
    let mut y = 28.0_f32;
    if has_spotlight {
        y += 336.0 + 40.0;
    }
    if cw_rows > 0 {
        let cw_grid_h =
            cw_rows as f32 * cw_card_h + (cw_rows.saturating_sub(1)) as f32 * SHELF_CARD_GAP_PX;
        y += 40.0 + 16.0 + cw_grid_h + 40.0;
    }
    y
}

fn resolve_spotlight_trailer_target(spot: &HomeSpotlightCandidate) -> Option<SpotlightTrailerTarget> {
    if let Some(direct) = spot.trailer_streams.iter().find(|s| {
        matches!(
            &s.source,
            StreamSource::Url { url } if url.scheme() == "http" || url.scheme() == "https"
        )
    }) {
        return Some(SpotlightTrailerTarget::DirectStream(Box::new(direct.clone())));
    }

    let yt_key = spot
        .trailer_youtube_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            spot.trailer_streams.iter().find_map(|s| match &s.source {
                StreamSource::YouTube { yt_id } if !yt_id.trim().is_empty() => {
                    Some(yt_id.trim().to_owned())
                }
                _ => None,
            })
        })?;

    Some(SpotlightTrailerTarget::YouTubeUrl(format!(
        "https://www.youtube.com/watch?v={yt_key}"
    )))
}

fn format_preview_subtitle(raw_type: &str, year: Option<&str>, genre: Option<&str>) -> String {
    let lead = year
        .map(str::to_owned)
        .unwrap_or_else(|| sentence_case_label(raw_type));
    match genre {
        Some(g) if !g.trim().is_empty() => format!("{lead} · {g}"),
        _ => lead,
    }
}

fn format_cw_subtitle(
    li: &LibraryItem,
    ep_code: Option<&str>,
    notification_count: usize,
    pct: f32,
) -> String {
    let status = if li.state.time_offset > 0 {
        format_cw_remaining(li, pct.max(2.0))
    } else if notification_count > 0 {
        "New episode".to_owned()
    } else {
        sentence_case_label(&li.r#type)
    };

    match ep_code {
        Some(code) => format!("{code} · {status}"),
        None => status,
    }
}

fn format_cw_remaining(li: &LibraryItem, pct: f32) -> String {
    if li.state.duration > li.state.time_offset && li.state.duration > 60_000 {
        let rem_mins = ((li.state.duration - li.state.time_offset) / 60_000).max(1);
        format!("{rem_mins}m left")
    } else {
        format!("{pct:.0}% watched")
    }
}

fn editorialize_shelf_heading(
    idx: usize,
    raw_cat: &str,
    raw_type: &str,
) -> (String, Option<String>) {
    let lower = raw_cat.trim().to_ascii_lowercase();
    let is_movie = raw_type.eq_ignore_ascii_case("movie");
    let is_series = raw_type.eq_ignore_ascii_case("series");
    let type_tag = sentence_case_label(raw_type);

    if idx == 0 && (lower == "top" || lower.contains("popular")) {
        return (
            "What people are watching".to_owned(),
            Some(format!(
                "Popular {} this week",
                if is_movie { "films" } else { "shows" }
            )),
        );
    }
    if idx == 1 && (lower == "top" || lower.contains("popular")) {
        if is_series {
            return (
                "Television worth your evenings".to_owned(),
                Some("Trending series across your catalogs".to_owned()),
            );
        }
        return (
            if is_movie {
                "Popular films right now".to_owned()
            } else {
                format!("Popular {} right now", raw_type.to_ascii_lowercase())
            },
            Some(format!("Trending {type_tag} across your catalogs")),
        );
    }
    if lower.contains("featured") || lower.contains("imdb") {
        return (
            if is_movie {
                "Critically loved".to_owned()
            } else {
                "On a lot of lists".to_owned()
            },
            Some(type_tag),
        );
    }
    if lower.contains("new") || lower.contains("year") || lower.contains("cinemeta") {
        return ("Now playing".to_owned(), Some(type_tag));
    }

    let clean = sentence_case_label(raw_cat);
    if clean
        .to_ascii_lowercase()
        .contains(&raw_type.to_ascii_lowercase())
    {
        (clean, None)
    } else {
        (format!("{clean} {}", raw_type.to_ascii_lowercase()), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn editorialize_shelf_heading_never_mislabels_movies_as_television_at_idx_1() {
        let (title_movie, sub_movie) = editorialize_shelf_heading(1, "top", "movie");
        assert!(!title_movie.contains("Television"));
        assert!(!sub_movie.as_deref().unwrap_or("").contains("series"));

        let (title_pop_movie, _) = editorialize_shelf_heading(1, "Popular", "movie");
        assert!(!title_pop_movie.contains("Television"));

        let (title_series, sub_series) = editorialize_shelf_heading(1, "top", "series");
        assert_eq!(title_series, "Television worth your evenings");
        assert_eq!(
            sub_series.as_deref(),
            Some("Trending series across your catalogs")
        );
    }

    #[test]
    fn shelf_pin_and_order_puts_pinned_first_in_pin_order() {
        let dummy_req = ResourceRequest {
            base: Url::parse("https://v3-cinemeta.strem.io/manifest.json").unwrap(),
            path: stremio_core_runtime::stremio_core::types::addon::ResourcePath {
                resource: "catalog".to_owned(),
                r#type: "movie".to_owned(),
                id: "top".to_owned(),
                extra: vec![],
            },
        };
        let make_meta = |idx: usize, key: &str| BoardShelfMeta {
            raw_index: idx,
            shelf_key: key.to_owned(),
            title: key.to_owned(),
            subtitle: None,
            see_all_request: dummy_req.clone(),
            aspect: PosterAspect::Portrait2x3,
            total_items: 10,
            is_loading: false,
            can_load_more: false,
            is_pinned: false,
        };

        let ordered = order_board_shelves(vec![
            (None, make_meta(0, "a")),
            (Some(1), make_meta(1, "b")),
            (None, make_meta(2, "c")),
            (Some(0), make_meta(3, "d")),
        ]);

        let keys: Vec<&str> = ordered.iter().map(|m| m.shelf_key.as_str()).collect();
        assert_eq!(keys, vec!["d", "b", "a", "c"]);
    }

    #[test]
    fn horizontal_window_clamps_and_applies_overscan() {
        assert_eq!(
            compute_horizontal_window(0, 0.0, 1200.0, 176.0, 18.0, 2),
            0..0
        );
        // stride = 194px; at scroll_x=0, viewport=1200 -> first=0, last_excl=7 -> 0..9
        assert_eq!(
            compute_horizontal_window(50, 0.0, 1200.0, 176.0, 18.0, 2),
            0..9
        );
        // at scroll_x=1940 (item 10), first=10, last_excl=17 -> 8..19
        assert_eq!(
            compute_horizontal_window(50, 1940.0, 1200.0, 176.0, 18.0, 2),
            8..19
        );
        // near tail
        assert_eq!(
            compute_horizontal_window(20, 3500.0, 1200.0, 176.0, 18.0, 2),
            16..20
        );
    }

    #[test]
    fn spotlight_trailer_prefers_direct_http_stream_over_youtube() {
        let mut spot = HomeSpotlightCandidate {
            id: "tt15239678".to_owned(),
            r#type: "movie".to_owned(),
            name: "Dune: Part Two".to_owned(),
            video_id: None,
            is_resume: false,
            eyebrow: "FEATURED".to_owned(),
            description: None,
            year: None,
            imdb_rating: None,
            genres: vec![],
            backdrop_url: None,
            logo_url: None,
            trailer_youtube_key: Some("Way9Dexny3w".to_owned()),
            trailer_streams: vec![],
        };

        match resolve_spotlight_trailer_target(&spot) {
            Some(SpotlightTrailerTarget::YouTubeUrl(url)) => {
                assert_eq!(url, "https://www.youtube.com/watch?v=Way9Dexny3w");
            }
            _ => panic!("expected YouTube trailer URL"),
        }

        spot.trailer_streams.push(Stream {
            source: StreamSource::Url {
                url: Url::parse("https://cdn.example/trailer.mp4").unwrap(),
            },
            name: None,
            description: None,
            thumbnail: None,
            subtitles: vec![],
            behavior_hints: Default::default(),
        });

        assert!(matches!(
            resolve_spotlight_trailer_target(&spot),
            Some(SpotlightTrailerTarget::DirectStream(_))
        ));
    }
}
