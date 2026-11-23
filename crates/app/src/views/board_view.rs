use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    hash::{Hash, Hasher},
    ops::Range,
    sync::Arc,
    time::Duration,
};

use gpui::{
    div, img, linear_color_stop, linear_gradient, point, prelude::*, px, relative, rgb, rgba,
    AnyElement, AsyncApp, Context, ElementId, FontWeight, MouseButton, MouseDownEvent, ObjectFit,
    ScrollDelta, ScrollEvent, ScrollHandle, SharedString, WeakEntity,
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
    icons::{svg_icon, svg_icon_weighted, IconName, IconWeight},
    shell::AppShell,
    state::{
        extract_genres, extract_imdb_rating, format_movietracker_episode_code, scroll_near_bottom,
        AppRoute, BrowsingContextMenu, KbFocusableCard, LibraryFilterTab, LibraryStatusTab,
        PlaybackCandidate, QuickPeekTarget, SidebarTab, ToastKind,
    },
    theme::{
        active_theme, editorial_serif, glass_capsule_shadow, mono_font,
        placeholder_tint_for_id_with_preset, sentence_case_label, tabular_nums, ui_sans,
        ThemePalette, ThemePreset, RADIUS_CONTROL, RADIUS_PILL, RADIUS_POSTER, RADIUS_SHEET,
    },
    ui::{
        card_action_dispatcher, landscape_card_dimensions, page_gutter_px, primary_accent_button,
        render_empty_state, render_landscape_card, render_prepared_poster_card,
        render_section_header, secondary_glass_button, EmptyStateProps, LandscapeCardProps,
        PosterCardAction, PosterCardCapabilities, PosterMetrics, PreparedPosterCard,
        PreparedPosterRenderCtx, SectionHeader, PAGE_PADDING_BOTTOM_PX, PAGE_SECTION_GAP_PX,
    },
    views::preview_drawer::build_quick_peek_fallback_preview,
    virtual_grid::PosterAspect,
};

const SHELF_HEADER_HEIGHT_PX: f32 = 36.0;
const SHELF_HEADER_WITH_SUBTITLE_HEIGHT_PX: f32 = 52.0;
const SHELF_HEADER_GAP_PX: f32 = 14.0;
const VERTICAL_OVERSCAN_PX: f32 = 650.0;
const HORIZONTAL_OVERSCAN_CARDS: usize = 2;
const HERO_MIN_HEIGHT_PX: f32 = 460.0;
const HERO_MAX_HEIGHT_PX: f32 = 640.0;
const HERO_HOVER_DEBOUNCE_MS: u64 = 120;
const MAX_CW_FILMSTRIP_ITEMS: usize = 16;
const MAX_FEATURED_CANDIDATES: usize = 8;
const CW_FILMSTRIP_SCROLL_KEY: usize = usize::MAX;
const FEATURED_DOCK_SCROLL_KEY: usize = usize::MAX - 1;

// Crisp cinema-scrim ink tokens for the full-bleed Hero Stage (ensures contrast on Dark + LightMist).
const HERO_INK_PRIMARY: u32 = 0xF7F5F0;
const HERO_INK_SECONDARY: u32 = 0xD4D7E0;
const HERO_INK_MUTED: u32 = 0x9EA3B2;
const HERO_GLASS_BG: u32 = 0x10131CB8;
const HERO_GLASS_HOVER_BG: u32 = 0x1B202EE0;
const HERO_GLASS_TROUGH_BG: u32 = 0x0B0D14BF;
const HERO_GLASS_BORDER: u32 = 0xFFFFFF26;
const HERO_GLASS_BORDER_STRONG: u32 = 0xFFFFFF48;
const HERO_DIVIDER: u32 = 0xFFFFFF36;
const HERO_STAR_GOLD: u32 = 0xFBBF24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeroStageMode {
    ContinueWatching,
    Featured,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BoardSnapshotKey {
    board_state_rev: u64,
    library_state_rev: u64,
    shelf_limit: usize,
    pinned_hash: u64,
    hidden_hash: u64,
    want_spotlight: bool,
    theme_preset: ThemePreset,
}

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
    poster_url: Option<String>,
    trailer_youtube_key: Option<String>,
    trailer_streams: Vec<Stream>,
    preview: Option<MetaItemPreview>,
}

#[derive(Clone)]
enum SpotlightTrailerTarget {
    DirectStream(Box<Stream>),
    YouTubeUrl(String),
}

#[derive(Clone)]
struct ContinueWatchingCard {
    library_item: LibraryItem,
    notification_count: usize,
    backdrop_url: Option<String>,
    logo_url: Option<String>,
    imdb_rating: Option<String>,
    year: Option<String>,
    description: Option<String>,
    genres: Vec<String>,
    trailer_streams: Vec<Stream>,
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

struct CachedBoardShelf {
    meta: BoardShelfMeta,
    items: Arc<[MetaItemPreview]>,
    cards: Arc<[PreparedPosterCard]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HiddenShelfInfo {
    shelf_key: String,
    title: String,
    subtitle: Option<String>,
}

struct BoardViewSnapshot {
    key: BoardSnapshotKey,
    cw_cards: Arc<[ContinueWatchingCard]>,
    cw_total_count: usize,
    featured_candidates: Arc<[HomeSpotlightCandidate]>,
    shelves: Arc<[CachedBoardShelf]>,
    hidden_shelves: Arc<[HiddenShelfInfo]>,
    total_board_catalogs: usize,
    pinned_indices_to_load: Arc<[usize]>,
}

struct VisibleShelfLayout<'a> {
    shelf: &'a CachedBoardShelf,
    visible_range: Range<usize>,
    shelf_total_h: f32,
    is_visible_y: bool,
}

struct ActiveHeroDisplay {
    id: String,
    r#type: String,
    name: String,
    video_id: Option<String>,
    primary_cta_label: String,
    eyebrow: String,
    description: Option<String>,
    year: Option<String>,
    imdb_rating: Option<String>,
    genres: Vec<String>,
    backdrop_url: Option<String>,
    logo_url: Option<String>,
    poster_url: Option<String>,
    trailer_youtube_key: Option<String>,
    trailer_streams: Vec<Stream>,
    preview: Option<MetaItemPreview>,
    in_library: bool,
}

impl AppShell {
    fn cached_board_snapshot(
        &self,
        shelf_limit: usize,
        want_spotlight: bool,
        library_statuses: &HashMap<String, LibraryStatusTab>,
        theme_preset: ThemePreset,
    ) -> Arc<BoardViewSnapshot> {
        let pinned_hash = hash_string_slice(&self.preferences.pinned_board_shelves);
        let hidden_hash = hash_string_slice(&self.preferences.hidden_board_shelves);
        let key = BoardSnapshotKey {
            board_state_rev: self.board_state_rev,
            library_state_rev: self.library_state_rev,
            shelf_limit,
            pinned_hash,
            hidden_hash,
            want_spotlight,
            theme_preset,
        };

        {
            let guard = self.memoized_board_cache.lock();
            if let Some(any_arc) = guard.as_ref() {
                if let Ok(cached) = Arc::clone(any_arc).downcast::<BoardViewSnapshot>() {
                    if cached.key == key {
                        return cached;
                    }
                }
            }
        }

        let pinned_prefs = &self.preferences.pinned_board_shelves;
        let hidden_prefs = &self.preferences.hidden_board_shelves;

        let snapshot = self.core.with_model(|model| {
            build_board_view_snapshot(model, key, pinned_prefs, hidden_prefs, library_statuses)
        });
        let arc_snap = Arc::new(snapshot);

        {
            let mut guard = self.memoized_board_cache.lock();
            *guard = Some(Arc::clone(&arc_snap) as Arc<dyn std::any::Any + Send + Sync>);
        }

        arc_snap
    }

    pub fn render_board_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = active_theme();
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
        let side_pad = page_gutter_px(viewport_w);
        let container_w = (viewport_w - side_pad * 2.0).max(360.0);
        let poster_metrics = PosterMetrics::from_theme(&theme);
        let gap_px = poster_metrics.gap_px;
        let section_gap_px = PAGE_SECTION_GAP_PX * theme.ui_scale;

        let shelf_limit = self.visible_board_shelves.max(INITIAL_BOARD_SHELVES);
        let want_spotlight = self.preferences.show_home_spotlight;

        let library_statuses = self.cached_library_statuses();
        let snapshot =
            self.cached_board_snapshot(shelf_limit, want_spotlight, &library_statuses, theme.preset);

        for &idx in snapshot.pinned_indices_to_load.iter() {
            if let std::collections::hash_map::Entry::Vacant(e) = self.board_shelf_scrolls.entry(idx)
            {
                e.insert(ScrollHandle::new());
                self.core.load_board_range(idx..idx);
            }
        }

        let has_cw = !snapshot.cw_cards.is_empty();
        let has_featured = want_spotlight && !snapshot.featured_candidates.is_empty();
        let show_hero_stage = has_cw || has_featured;
        let hero_mode =
            resolve_hero_stage_mode(self.board_hero_mode_featured, has_cw, has_featured);
        let hero_height = show_hero_stage.then(|| hero_stage_height_px(viewport_h));

        let mut kb_cards: Vec<KbFocusableCard> = Vec::new();
        let hero_stage_el = if let Some(hero_h) = hero_height {
            if scroll_y <= hero_h + VERTICAL_OVERSCAN_PX {
                let cw_scroll = self
                    .board_shelf_scrolls
                    .entry(CW_FILMSTRIP_SCROLL_KEY)
                    .or_default()
                    .clone();
                let featured_scroll = self
                    .board_shelf_scrolls
                    .entry(FEATURED_DOCK_SCROLL_KEY)
                    .or_default()
                    .clone();

                Some(
                    self.render_hero_stage(
                        &snapshot,
                        hero_mode,
                        has_cw && has_featured,
                        hero_h,
                        side_pad,
                        container_w,
                        &cw_scroll,
                        &featured_scroll,
                        &library_statuses,
                        &mut kb_cards,
                        &theme,
                        cx,
                    )
                    .into_any_element(),
                )
            } else {
                Some(
                    div()
                        .id("home-hero-spacer")
                        .w_full()
                        .h(px(hero_h))
                        .into_any_element(),
                )
            }
        } else {
            None
        };

        let shelf_scroll_x: HashMap<usize, f32> = self
            .board_shelf_scrolls
            .iter()
            .filter(|(&idx, _)| idx != CW_FILMSTRIP_SCROLL_KEY && idx != FEATURED_DOCK_SCROLL_KEY)
            .map(|(&idx, h)| (idx, (-h.offset().x / px(1.0)).max(0.0)))
            .collect();

        let mut y_cursor = initial_shelves_top_y(hero_height, theme.ui_scale);
        let mut shelf_layouts: Vec<VisibleShelfLayout<'_>> =
            Vec::with_capacity(snapshot.shelves.len());

        for shelf in snapshot.shelves.iter() {
            let (card_w, _img_h, card_h) = shelf_card_dimensions(shelf.meta.aspect, &theme);
            let shelf_total_h =
                shelf_total_height_px(card_h, shelf.meta.subtitle.is_some(), theme.ui_scale);
            let shelf_top = y_cursor;
            let shelf_bottom = shelf_top + shelf_total_h;
            y_cursor = shelf_bottom + section_gap_px;

            let is_visible_y = shelf_bottom >= (scroll_y - VERTICAL_OVERSCAN_PX)
                && shelf_top <= (scroll_y + viewport_h + VERTICAL_OVERSCAN_PX);

            let visible_range = if is_visible_y {
                let sx = shelf_scroll_x
                    .get(&shelf.meta.raw_index)
                    .copied()
                    .unwrap_or(0.0);
                compute_horizontal_window(
                    shelf.meta.total_items,
                    sx,
                    container_w,
                    card_w,
                    gap_px,
                    HORIZONTAL_OVERSCAN_CARDS,
                )
            } else {
                0..0
            };

            shelf_layouts.push(VisibleShelfLayout {
                shelf,
                visible_range,
                shelf_total_h,
                is_visible_y,
            });
        }

        let mut visible_shelf_handles: HashMap<usize, ScrollHandle> = HashMap::new();
        for layout in &shelf_layouts {
            if layout.is_visible_y {
                let handle = self
                    .board_shelf_scrolls
                    .entry(layout.shelf.meta.raw_index)
                    .or_default()
                    .clone();
                visible_shelf_handles.insert(layout.shelf.meta.raw_index, handle);
            }
        }

        let kb_row_offset = usize::from(show_hero_stage);
        let total_shelves_rendered = shelf_layouts.len();
        let mut shelf_elements = Vec::with_capacity(shelf_layouts.len());

        for (shelf_pos, layout) in shelf_layouts.into_iter().enumerate() {
            if layout.is_visible_y {
                let scroll_handle = visible_shelf_handles
                    .get(&layout.shelf.meta.raw_index)
                    .cloned()
                    .unwrap_or_default();
                let row_coord = kb_row_offset + shelf_pos;
                shelf_elements.push(
                    self.render_editorial_shelf(
                        layout.shelf,
                        layout.visible_range,
                        layout.shelf_total_h,
                        container_w,
                        scroll_handle,
                        row_coord,
                        &mut kb_cards,
                        &theme,
                        cx,
                    )
                    .into_any_element(),
                );
            } else {
                shelf_elements.push(
                    div()
                        .id(("board-shelf-spacer", layout.shelf.meta.raw_index))
                        .w_full()
                        .h(px(layout.shelf_total_h))
                        .into_any_element(),
                );
            }
        }

        let has_any_content = has_cw || total_shelves_rendered > 0;
        let total_board_catalogs = snapshot.total_board_catalogs;
        let has_more_shelves = shelf_limit < total_board_catalogs;
        let remaining_shelves = total_board_catalogs.saturating_sub(shelf_limit);

        self.set_kb_focus_items(kb_cards);

        let shelves_top_pad = if show_hero_stage {
            theme.sp(PAGE_SECTION_GAP_PX)
        } else {
            theme.sp(88.0)
        };

        div()
            .id("board-view-scroll")
            .track_scroll(&self.board_scroll_handle)
            .size_full()
            .overflow_y_scroll()
            .on_scroll(cx.listener(move |this, ev: &ScrollEvent, _, cx| {
                // GPUI's ScrollHandle listener already notifies the view on scroll offset changes;
                // only notify here when we expand `visible_board_shelves` near the bottom.
                let down_px = match ev.delta {
                    ScrollDelta::Pixels(p) => (-p.y) / px(1.0),
                    ScrollDelta::Lines(l) => (-l.y) * 36.0,
                };
                if down_px > 0.0
                    && this.visible_board_shelves < total_board_catalogs
                    && scroll_near_bottom(&this.board_scroll_handle, down_px, 420.0)
                {
                    let old = this.visible_board_shelves.max(INITIAL_BOARD_SHELVES);
                    let next = (old + INITIAL_BOARD_SHELVES).min(total_board_catalogs);
                    if next > old {
                        this.visible_board_shelves = next;
                        this.core.load_board_range(old..next);
                        cx.notify();
                    }
                }
            }))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .when_some(hero_stage_el, |el, hero| el.child(hero))
                    .child(
                        div()
                            .w_full()
                            .px(px(side_pad))
                            .pt(shelves_top_pad)
                            .pb(theme.sp(PAGE_PADDING_BOTTOM_PX))
                            .flex()
                            .flex_col()
                            .gap(theme.sp(PAGE_SECTION_GAP_PX))
                            .when(!has_any_content, |el| {
                                el.child(Self::render_empty_board_banner(cx))
                            })
                            .children(shelf_elements)
                            .when(has_more_shelves, |el| {
                                el.child(
                                    div()
                                        .pt(theme.sp(4.0))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(theme.sp(16.0))
                                        .child(
                                            div()
                                                .font(mono_font())
                                                .text_xs()
                                                .text_color(rgb(theme.text_muted))
                                                .child(format!(
                                                    "Showing {} of {total_board_catalogs} shelves",
                                                    shelf_limit.min(total_board_catalogs)
                                                )),
                                        )
                                        .child(
                                            div()
                                                .id("btn-board-show-more-shelves")
                                                .cursor_pointer()
                                                .h(theme.sp(36.0))
                                                .px(theme.sp(16.0))
                                                .rounded(px(RADIUS_PILL))
                                                .bg(rgba(theme.glass_trough_bg))
                                                .border_1()
                                                .border_color(rgba(theme.glass_border))
                                                .hover(move |s| {
                                                    s.bg(rgba(theme.glass_row_hover_bg))
                                                        .border_color(rgba(
                                                            theme.glass_border_strong,
                                                        ))
                                                        .text_color(rgb(theme.text_primary))
                                                })
                                                .flex()
                                                .items_center()
                                                .gap(px(6.0))
                                                .font(ui_sans(FontWeight::MEDIUM))
                                                .text_size(theme.sp(13.0))
                                                .text_color(rgb(theme.text_secondary))
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
                                                    13.0 * theme.ui_scale,
                                                    rgb(theme.text_muted),
                                                )),
                                        ),
                                )
                            })
                            .when(!snapshot.hidden_shelves.is_empty(), |el| {
                                el.child(self.render_hidden_shelves_drawer(
                                    &snapshot.hidden_shelves,
                                    &theme,
                                    cx,
                                ))
                            }),
                    ),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_hero_stage(
        &mut self,
        snapshot: &BoardViewSnapshot,
        mode: HeroStageMode,
        show_mode_switcher: bool,
        hero_height_px: f32,
        side_pad: f32,
        container_w: f32,
        cw_scroll: &ScrollHandle,
        featured_scroll: &ScrollHandle,
        library_statuses: &HashMap<String, LibraryStatusTab>,
        kb_cards: &mut Vec<KbFocusableCard>,
        theme: &ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active_cw_idx = if snapshot.cw_cards.is_empty() {
            0
        } else {
            self.board_active_cw_idx
                .min(snapshot.cw_cards.len().saturating_sub(1))
        };
        let active_spot_idx = if snapshot.featured_candidates.is_empty() {
            0
        } else {
            self.board_spotlight_idx % snapshot.featured_candidates.len()
        };

        let mut active = match mode {
            HeroStageMode::ContinueWatching => {
                let cw = &snapshot.cw_cards[active_cw_idx];
                self.build_hero_display_from_cw(cw, library_statuses)
            }
            HeroStageMode::Featured => {
                let spot = &snapshot.featured_candidates[active_spot_idx];
                self.build_hero_display_from_spotlight(spot, library_statuses)
            }
        };

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
            if active.poster_url.is_none() {
                active.poster_url = enr.poster_url;
            }
        }

        // Prefetch the adjacent candidate's hero backdrop for instant transitions.
        match mode {
            HeroStageMode::ContinueWatching if snapshot.cw_cards.len() > 1 => {
                let next_idx = (active_cw_idx + 1) % snapshot.cw_cards.len();
                let next_cw = &snapshot.cw_cards[next_idx];
                if let Some(next_bg) = self.resolve_cw_backdrop_url(next_cw) {
                    self.poster_cache.prefetch_hero_backdrop(&next_bg);
                }
            }
            HeroStageMode::Featured if snapshot.featured_candidates.len() > 1 => {
                let next_idx = (active_spot_idx + 1) % snapshot.featured_candidates.len();
                let next_spot = &snapshot.featured_candidates[next_idx];
                if let Some(next_bg) = self.resolve_spotlight_backdrop_url(next_spot) {
                    self.poster_cache.prefetch_hero_backdrop(&next_bg);
                }
            }
            _ => {}
        }

        let metahub_bg = active.id.starts_with("tt").then(|| {
            format!(
                "https://images.metahub.space/background/medium/{}/img",
                active.id
            )
        });
        let metahub_logo = active.id.starts_with("tt").then(|| {
            format!(
                "https://images.metahub.space/logo/medium/{}/img",
                active.id
            )
        });

        let resolved_ambient_bg = active
            .backdrop_url
            .clone()
            .or_else(|| metahub_bg.clone())
            .or_else(|| active.poster_url.clone());
        self.set_ambient_hero_backdrop_url(resolved_ambient_bg);

        let hero_urls: Vec<&str> = active
            .backdrop_url
            .as_deref()
            .into_iter()
            .chain(metahub_bg.as_deref())
            .chain(active.poster_url.as_deref())
            .collect();
        let backdrop_tex = if hero_urls.len() == 1 {
            self.poster_cache.resolve_hero_backdrop_image(hero_urls[0])
        } else {
            self.poster_cache.resolve_first_available_hero(hero_urls)
        };

        let logo_urls: Vec<&str> = active
            .logo_url
            .as_deref()
            .into_iter()
            .chain(metahub_logo.as_deref())
            .collect();
        let logo_tex = self.poster_cache.resolve_first_available_logo(logo_urls);

        let trailer_target =
            resolve_trailer_target(&active.trailer_streams, active.trailer_youtube_key.as_deref());

        let play_type = active.r#type.clone();
        let play_id = active.id.clone();
        let play_vid = active.video_id.clone();

        let det_type = active.r#type.clone();
        let det_id = active.id.clone();
        let det_vid = active.video_id.clone();

        let peek_type = active.r#type.clone();
        let peek_id = active.id.clone();
        let peek_vid = active.video_id.clone();
        let peek_title = active.name.clone();
        let peek_preview = active.preview.clone();

        let lib_id = active.id.clone();
        let lib_in = active.in_library;
        let lib_preview = active.preview.clone().unwrap_or_else(|| {
            build_quick_peek_fallback_preview(
                &active.id,
                &active.r#type,
                &active.name,
                active.poster_url.as_deref(),
                active.backdrop_url.as_deref(),
                active.logo_url.as_deref(),
                active.year.as_deref(),
                None,
                active.description.as_deref(),
                active.imdb_rating.as_deref(),
                &active.genres,
            )
        });

        let tr_type = active.r#type.clone();
        let tr_id = active.id.clone();
        let tr_name = active.name.clone();

        let dock_el = match mode {
            HeroStageMode::ContinueWatching => self
                .render_hero_cw_dock(
                    &snapshot.cw_cards,
                    snapshot.cw_total_count,
                    active_cw_idx,
                    show_mode_switcher,
                    container_w,
                    cw_scroll,
                    library_statuses,
                    kb_cards,
                    theme,
                    cx,
                )
                .into_any_element(),
            HeroStageMode::Featured => self
                .render_hero_featured_dock(
                    &snapshot.featured_candidates,
                    active_spot_idx,
                    show_mode_switcher,
                    container_w,
                    featured_scroll,
                    library_statuses,
                    kb_cards,
                    theme,
                    cx,
                )
                .into_any_element(),
        };

        let scrim_layers = render_hero_scrim_layers(theme);
        let accent = theme.accent;
        let accent_hover = theme.accent_hover;
        let cta_on_accent = theme.cta_on_accent;
        let ui_scale = theme.ui_scale;
        let border_accent = theme.border_accent;
        let accent_soft = theme.accent_soft;

        div()
            .id("home-hero-stage")
            .w_full()
            .h(px(hero_height_px))
            .relative()
            .overflow_hidden()
            .bg(rgb(placeholder_tint_for_id_with_preset(
                &active.id,
                theme.preset,
            )))
            .when_some(backdrop_tex, |el, tex| {
                el.child(
                    img(tex)
                        .size_full()
                        .absolute()
                        .inset_0()
                        .object_fit(ObjectFit::Cover),
                )
            })
            .children(scrim_layers)
            .child(
                div()
                    .relative()
                    .size_full()
                    .px(px(side_pad))
                    .pt(theme.sp(84.0))
                    .pb(px(18.0))
                    .flex()
                    .flex_col()
                    .justify_between()
                    .gap(px(14.0))
                    .child(
                        // Lower-left focal content column anchored above the docked strip.
                        div()
                            .flex_1()
                            .min_h(px(0.0))
                            .w_full()
                            .flex()
                            .items_end()
                            .justify_between()
                            .gap(px(24.0))
                            .child(
                                div()
                                    .max_w(px(680.0))
                                    .flex()
                                    .flex_col()
                                    .items_start()
                                    .gap(px(10.0))
                                    // Eyebrow + metadata row.
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .gap(px(8.0))
                                            .font(mono_font())
                                            .text_size(theme.sp(11.5))
                                            .text_color(rgb(HERO_INK_SECONDARY))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(6.0))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(accent))
                                                    .child(
                                                        div()
                                                            .w(px(6.0))
                                                            .h(px(6.0))
                                                            .rounded_full()
                                                            .bg(rgb(accent)),
                                                    )
                                                    .child(active.eyebrow.clone()),
                                            )
                                            .when_some(active.year.clone(), |el, yr| {
                                                el.child(hero_meta_divider()).child(yr)
                                            })
                                            .when_some(active.imdb_rating.clone(), |el, r| {
                                                el.child(hero_meta_divider()).child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap(px(4.0))
                                                        .text_color(rgb(HERO_INK_PRIMARY))
                                                        .child(svg_icon_weighted(
                                                            IconName::Star,
                                                            IconWeight::Fill,
                                                            11.5 * ui_scale,
                                                            rgb(HERO_STAR_GOLD),
                                                        ))
                                                        .child(r),
                                                )
                                            })
                                            .when(!active.genres.is_empty(), |el| {
                                                el.child(hero_meta_divider())
                                                    .child(active.genres.join(" · "))
                                            }),
                                    )
                                    // Title logo or display typography.
                                    .child(if let Some(ltex) = logo_tex {
                                        img(ltex)
                                            .h(px(64.0))
                                            .max_w(px(360.0))
                                            .object_fit(ObjectFit::Contain)
                                            .into_any_element()
                                    } else {
                                        div()
                                            .font(editorial_serif())
                                            .text_size(theme.sp(44.0))
                                            .line_height(relative(1.04))
                                            .text_color(rgb(HERO_INK_PRIMARY))
                                            .truncate()
                                            .child(active.name.clone())
                                            .into_any_element()
                                    })
                                    // Concise 2-line synopsis.
                                    .when_some(active.description.clone(), |el, desc| {
                                        el.child(
                                            div()
                                                .max_w(px(560.0))
                                                .text_size(theme.sp(13.5))
                                                .line_height(theme.sp(19.5))
                                                .text_color(rgb(HERO_INK_SECONDARY))
                                                .max_h(theme.sp(40.0))
                                                .overflow_hidden()
                                                .child(desc),
                                        )
                                    })
                                    // Primary & secondary action pills.
                                    .child(
                                        div()
                                            .mt(px(4.0))
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .gap(px(10.0))
                                            .child(
                                                div()
                                                    .id("hero-primary-play-btn")
                                                    .cursor_pointer()
                                                    .h(theme.sp(38.0))
                                                    .px(theme.sp(20.0))
                                                    .rounded(px(RADIUS_PILL))
                                                    .bg(rgb(accent))
                                                    .hover(move |s| s.bg(rgb(accent_hover)))
                                                    .shadow(glass_capsule_shadow())
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .gap(theme.sp(8.0))
                                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                                    .font_features(tabular_nums())
                                                    .text_size(theme.sp(13.5))
                                                    .text_color(rgb(cta_on_accent))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            this.quick_resume_or_open(
                                                                play_type.clone(),
                                                                play_id.clone(),
                                                                play_vid.clone(),
                                                                cx,
                                                            );
                                                        }),
                                                    )
                                                    .child(svg_icon_weighted(
                                                        IconName::Play,
                                                        IconWeight::Fill,
                                                        14.5 * ui_scale,
                                                        rgb(cta_on_accent),
                                                    ))
                                                    .child(active.primary_cta_label),
                                            )
                                            .child(render_hero_glass_pill(
                                                "hero-details-btn",
                                                IconName::Info,
                                                IconWeight::Regular,
                                                "Details",
                                                false,
                                                accent,
                                                accent_soft,
                                                border_accent,
                                                ui_scale,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.open_meta_details(
                                                        det_type.clone(),
                                                        det_id.clone(),
                                                        det_vid.clone(),
                                                        cx,
                                                    );
                                                }),
                                            ))
                                            .when_some(trailer_target, |row, target| {
                                                row.child(render_hero_glass_pill(
                                                    "hero-trailer-btn",
                                                    IconName::Clapperboard,
                                                    IconWeight::Regular,
                                                    "Trailer",
                                                    false,
                                                    accent,
                                                    accent_soft,
                                                    border_accent,
                                                    ui_scale,
                                                    cx.listener(move |this, _, _, cx| match &target
                                                    {
                                                        SpotlightTrailerTarget::DirectStream(
                                                            stream,
                                                        ) => {
                                                            let addons = this.core.with_model(
                                                                |m| m.ctx.profile.addons.clone(),
                                                            );
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
                                                ))
                                            })
                                            .child(render_hero_glass_pill(
                                                "hero-library-btn",
                                                IconName::Bookmark,
                                                if lib_in {
                                                    IconWeight::Fill
                                                } else {
                                                    IconWeight::Regular
                                                },
                                                if lib_in { "In Library" } else { "+ Library" },
                                                lib_in,
                                                accent,
                                                accent_soft,
                                                border_accent,
                                                ui_scale,
                                                cx.listener(move |this, _, _, cx| {
                                                    if lib_in {
                                                        this.core
                                                            .remove_from_library(lib_id.clone());
                                                        this.push_toast(
                                                            ToastKind::Info,
                                                            "Removed from library",
                                                            None,
                                                            cx,
                                                        );
                                                    } else {
                                                        this.core.toggle_library_item(
                                                            lib_preview.clone(),
                                                        );
                                                        this.push_toast(
                                                            ToastKind::Success,
                                                            "Added to library",
                                                            None,
                                                            cx,
                                                        );
                                                    }
                                                    cx.notify();
                                                }),
                                            ))
                                            .child(render_hero_glass_pill(
                                                "hero-quick-peek-btn",
                                                IconName::Eye,
                                                IconWeight::Regular,
                                                "Quick Peek",
                                                false,
                                                accent,
                                                accent_soft,
                                                border_accent,
                                                ui_scale,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.open_quick_peek(
                                                        QuickPeekTarget {
                                                            type_name: peek_type.clone(),
                                                            id: peek_id.clone(),
                                                            video_id: peek_vid.clone(),
                                                            title: peek_title.clone(),
                                                            preview: peek_preview.clone(),
                                                        },
                                                        cx,
                                                    );
                                                }),
                                            )),
                                    ),
                            ),
                    )
                    .child(dock_el),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_hero_dock_card_hover(
        &mut self,
        idx: usize,
        is_featured: bool,
        item_count: usize,
        item_type: &str,
        item_id: &str,
        hover_bg: Option<&str>,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        self.on_card_hover_prefetch(item_type, item_id, hover_bg, hovered, cx);
        if hovered {
            let cur_idx = if is_featured {
                self.board_spotlight_idx % item_count.max(1)
            } else {
                self.board_active_cw_idx
            };
            if cur_idx != idx {
                self.board_hover_debounce_seq = self.board_hover_debounce_seq.wrapping_add(1);
                let seq = self.board_hover_debounce_seq;
                let prefetch_bg = hover_bg.map(str::to_owned);
                let enr_type = item_type.to_owned();
                let enr_id = item_id.to_owned();
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    cx.background_executor()
                        .timer(Duration::from_millis(HERO_HOVER_DEBOUNCE_MS))
                        .await;
                    let _ = this.update(cx, |shell, cx| {
                        if shell.board_hover_debounce_seq != seq {
                            return;
                        }
                        if is_featured {
                            shell.board_spotlight_idx = idx;
                        } else {
                            shell.board_active_cw_idx = idx;
                        }
                        if let Some(bg_url) = prefetch_bg.as_deref() {
                            shell.poster_cache.prefetch_hero_backdrop(bg_url);
                        }
                        shell.request_background_enrichment_if_needed(&enr_type, &enr_id, cx);
                        cx.notify();
                    });
                })
                .detach();
            }
        } else if self.hover_prefetch_target.is_none() {
            // Cursor left the strip onto empty space: cancel any pending switch.
            self.board_hover_debounce_seq = self.board_hover_debounce_seq.wrapping_add(1);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_hero_cw_dock(
        &self,
        cw_cards: &Arc<[ContinueWatchingCard]>,
        cw_total_count: usize,
        active_cw_idx: usize,
        show_mode_switcher: bool,
        container_w: f32,
        cw_scroll: &ScrollHandle,
        library_statuses: &HashMap<String, LibraryStatusTab>,
        kb_cards: &mut Vec<KbFocusableCard>,
        theme: &ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let dims = hero_dock_dimensions(HeroStageMode::ContinueWatching, theme);
        let step_px = (container_w * 0.65).max((dims.card_w + dims.card_gap) * 2.0);
        let scroll_prev = cw_scroll.clone();
        let scroll_next = cw_scroll.clone();
        let ui_scale = theme.ui_scale;
        let cw_count = cw_cards.len();

        let mut filmstrip_els: Vec<AnyElement> = Vec::with_capacity(cw_count);
        for (idx, cw) in cw_cards.iter().enumerate() {
            let li = &cw.library_item;
            let is_active = idx == active_cw_idx;
            let wide_url = self.resolve_cw_backdrop_url(cw);
            let wide_tex = self.poster_cache.resolve_widescreen_or_poster(
                wide_url.as_deref(),
                li.poster.as_ref().map(|u| u.as_str()),
            );

            let ep_code = format_movietracker_episode_code(li.state.video_id.as_deref());
            let pct = li.progress().clamp(0.0, 100.0) as f32;
            let progress_frac = if li.state.time_offset > 0 {
                Some((pct / 100.0).clamp(0.03, 1.0))
            } else {
                None
            };
            let status_sub = format_cw_remaining_or_status(li, cw.notification_count, pct);

            let in_library =
                library_statuses.contains_key(&li.id) || (!li.removed && !li.temp);
            let is_watched = li.watched();

            let kb_idx = kb_cards.len();
            let is_kb_focused = self.kb_focus_index == Some(kb_idx);
            kb_cards.push(KbFocusableCard {
                route: AppRoute::MetaDetails {
                    type_name: li.r#type.clone(),
                    id: li.id.clone(),
                    video_id: li.state.video_id.clone(),
                },
                row: 0,
                col: idx,
            });

            let cards_open = Arc::clone(cw_cards);
            let cards_play = Arc::clone(cw_cards);
            let cards_peek = Arc::clone(cw_cards);
            let cards_ctx = Arc::clone(cw_cards);
            let cards_hover = Arc::clone(cw_cards);
            let hover_bg = wide_url;

            let mut props = LandscapeCardProps::new(("hero-cw-card", idx), li.name.clone(), &li.id)
                .width_px(Some(dims.card_w))
                .image(wide_tex)
                .subtitle(Some(SharedString::from(status_sub)))
                .notification_count(cw.notification_count)
                .progress_fraction(progress_frac)
                .is_active(is_active)
                .is_focused(is_kb_focused)
                .on_open(cx.listener(move |this, _, _, cx| {
                    let li = &cards_open[idx].library_item;
                    this.board_active_cw_idx = idx;
                    this.open_meta_details(
                        li.r#type.clone(),
                        li.id.clone(),
                        li.state.video_id.clone(),
                        cx,
                    );
                }))
                .on_quick_play(cx.listener(move |this, _, _, cx| {
                    let li = &cards_play[idx].library_item;
                    this.quick_resume_or_open(
                        li.r#type.clone(),
                        li.id.clone(),
                        li.state.video_id.clone(),
                        cx,
                    );
                }))
                .on_quick_peek(cx.listener(move |this, _, _, cx| {
                    let cw = &cards_peek[idx];
                    let li = &cw.library_item;
                    this.open_quick_peek(
                        QuickPeekTarget {
                            type_name: li.r#type.clone(),
                            id: li.id.clone(),
                            video_id: li.state.video_id.clone(),
                            title: li.name.clone(),
                            preview: cw.preview.clone(),
                        },
                        cx,
                    );
                }))
                .on_context_menu(cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    let cw = &cards_ctx[idx];
                    let li = &cw.library_item;
                    this.open_browsing_context_menu(
                        BrowsingContextMenu::new(
                            ev.position,
                            li.r#type.clone(),
                            li.id.clone(),
                            li.name.clone(),
                        )
                        .video_id(li.state.video_id.clone())
                        .preview(cw.preview.clone())
                        .is_in_library(in_library)
                        .is_watched(is_watched)
                        .can_rewind_cw(true),
                        cx,
                    );
                }))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    let li = &cards_hover[idx].library_item;
                    this.handle_hero_dock_card_hover(
                        idx,
                        false,
                        cw_count,
                        &li.r#type,
                        &li.id,
                        hover_bg.as_deref(),
                        *hovered,
                        cx,
                    );
                }));

            if let Some(code) = ep_code {
                props = props.episode_badge(code, false);
            }

            filmstrip_els.push(render_landscape_card(props));
        }

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(dims.dock_gap))
            .child(
                div()
                    .w_full()
                    .h(px(dims.header_h))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(theme.sp(13.0))
                                    .text_color(rgb(HERO_INK_PRIMARY))
                                    .child("Continue Watching"),
                            )
                            .child(
                                div()
                                    .id("hero-cw-view-all")
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .px(px(8.0))
                                    .py(px(3.0))
                                    .rounded(px(RADIUS_PILL))
                                    .hover(|s| {
                                        s.bg(rgba(HERO_GLASS_BG))
                                            .text_color(rgb(HERO_INK_PRIMARY))
                                    })
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .font_features(tabular_nums())
                                    .text_size(theme.sp(12.0))
                                    .text_color(rgb(HERO_INK_SECONDARY))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.library_filter_tab = LibraryFilterTab::Watching;
                                            this.library_status_tab = LibraryStatusTab::Watching;
                                            this.navigate_tab(SidebarTab::Library, cx);
                                        }),
                                    )
                                    .child(format!("View all ({cw_total_count})"))
                                    .child(svg_icon(
                                        IconName::ArrowRight,
                                        12.0 * ui_scale,
                                        rgb(HERO_INK_SECONDARY),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .when(cw_count > 3, |row| {
                                row.child(render_hero_dock_chevron(
                                    "hero-cw-prev",
                                    IconName::ChevronLeft,
                                    ui_scale,
                                    cx.listener(move |_, _, _, cx| {
                                        let cur_x = (-scroll_prev.offset().x / px(1.0)).max(0.0);
                                        let next_x = (cur_x - step_px).max(0.0);
                                        scroll_prev.set_offset(point(px(-next_x), px(0.0)));
                                        cx.notify();
                                    }),
                                ))
                                .child(render_hero_dock_chevron(
                                    "hero-cw-next",
                                    IconName::ChevronRight,
                                    ui_scale,
                                    cx.listener(move |_, _, _, cx| {
                                        let max_x =
                                            (scroll_next.max_offset().x / px(1.0)).max(0.0);
                                        let cur_x = (-scroll_next.offset().x / px(1.0)).max(0.0);
                                        let next_x = (cur_x + step_px).min(max_x);
                                        scroll_next.set_offset(point(px(-next_x), px(0.0)));
                                        cx.notify();
                                    }),
                                ))
                            })
                            .when(show_mode_switcher, |row| {
                                row.child(render_hero_stage_mode_switcher(
                                    HeroStageMode::ContinueWatching,
                                    ui_scale,
                                    cx,
                                ))
                            }),
                    ),
            )
            .child(render_hero_dock_scroll_strip(
                "hero-cw-filmstrip",
                dims,
                cw_scroll,
                filmstrip_els,
            ))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_hero_featured_dock(
        &self,
        candidates: &Arc<[HomeSpotlightCandidate]>,
        active_idx: usize,
        show_mode_switcher: bool,
        container_w: f32,
        featured_scroll: &ScrollHandle,
        library_statuses: &HashMap<String, LibraryStatusTab>,
        kb_cards: &mut Vec<KbFocusableCard>,
        theme: &ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let dims = hero_dock_dimensions(HeroStageMode::Featured, theme);
        let spot_count = candidates.len();
        let scroll_prev = featured_scroll.clone();
        let scroll_next = featured_scroll.clone();
        let ui_scale = theme.ui_scale;

        let mut card_els: Vec<AnyElement> = Vec::with_capacity(spot_count);
        for (idx, spot) in candidates.iter().enumerate() {
            let is_active = idx == active_idx;
            let enriched = self.core.cached_enriched_meta(&spot.id);
            let wide_url = self.resolve_spotlight_backdrop_url(spot);
            let thumb_tex = self.poster_cache.resolve_widescreen_or_poster(
                wide_url.as_deref(),
                spot.poster_url.as_deref(),
            );

            let year = spot
                .year
                .as_deref()
                .or_else(|| enriched.as_ref().and_then(|e| e.year.as_deref()));
            let imdb_rating = spot
                .imdb_rating
                .as_deref()
                .or_else(|| enriched.as_ref().and_then(|e| e.imdb_rating.as_deref()));
            let genre = spot
                .genres
                .first()
                .map(String::as_str)
                .or_else(|| enriched.as_ref().and_then(|e| e.genres.first().map(String::as_str)));
            let clean_rating = imdb_rating
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let meta_lead = format_featured_card_meta_lead(
                year,
                genre,
                &spot.r#type,
                clean_rating.is_some(),
            );

            let lib_status = library_statuses.get(&spot.id).copied();
            let in_library = lib_status.is_some();
            let is_watched = matches!(lib_status, Some(LibraryStatusTab::Watched));

            let kb_idx = kb_cards.len();
            let is_kb_focused = self.kb_focus_index == Some(kb_idx);
            kb_cards.push(KbFocusableCard {
                route: AppRoute::MetaDetails {
                    type_name: spot.r#type.clone(),
                    id: spot.id.clone(),
                    video_id: spot.video_id.clone(),
                },
                row: 0,
                col: idx,
            });

            let cands_open = Arc::clone(candidates);
            let cands_play = Arc::clone(candidates);
            let cands_peek = Arc::clone(candidates);
            let cands_ctx = Arc::clone(candidates);
            let cands_hover = Arc::clone(candidates);
            let hover_bg = wide_url;

            let props = LandscapeCardProps::new(("hero-featured-card", idx), spot.name.clone(), &spot.id)
                .width_px(Some(dims.card_w))
                .image(thumb_tex)
                .subtitle(Some(SharedString::from(meta_lead)))
                .imdb_rating(clean_rating.map(SharedString::from))
                .is_active(is_active)
                .is_focused(is_kb_focused)
                .on_open(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let spot = &cands_open[idx];
                    this.board_spotlight_idx = idx;
                    this.open_meta_details(
                        spot.r#type.clone(),
                        spot.id.clone(),
                        spot.video_id.clone(),
                        cx,
                    );
                }))
                .on_quick_play(cx.listener(move |this, _, _, cx| {
                    let spot = &cands_play[idx];
                    this.quick_resume_or_open(
                        spot.r#type.clone(),
                        spot.id.clone(),
                        spot.video_id.clone(),
                        cx,
                    );
                }))
                .on_quick_peek(cx.listener(move |this, _, _, cx| {
                    let spot = &cands_peek[idx];
                    this.open_quick_peek(
                        QuickPeekTarget {
                            type_name: spot.r#type.clone(),
                            id: spot.id.clone(),
                            video_id: spot.video_id.clone(),
                            title: spot.name.clone(),
                            preview: spot.preview.clone(),
                        },
                        cx,
                    );
                }))
                .on_context_menu(cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    let spot = &cands_ctx[idx];
                    this.open_browsing_context_menu(
                        BrowsingContextMenu::new(
                            ev.position,
                            spot.r#type.clone(),
                            spot.id.clone(),
                            spot.name.clone(),
                        )
                        .video_id(spot.video_id.clone())
                        .preview(spot.preview.clone())
                        .is_in_library(in_library)
                        .is_watched(is_watched),
                        cx,
                    );
                }))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    let spot = &cands_hover[idx];
                    this.handle_hero_dock_card_hover(
                        idx,
                        true,
                        spot_count,
                        &spot.r#type,
                        &spot.id,
                        hover_bg.as_deref(),
                        *hovered,
                        cx,
                    );
                }));

            card_els.push(render_landscape_card(props));
        }

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(dims.dock_gap))
            .child(
                div()
                    .w_full()
                    .h(px(dims.header_h))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(theme.sp(13.0))
                                    .text_color(rgb(HERO_INK_PRIMARY))
                                    .child("Featured Picks"),
                            )
                            .when(spot_count > 1, |row| {
                                row.child(
                                    div()
                                        .font(mono_font())
                                        .text_size(px(11.0))
                                        .text_color(rgb(HERO_INK_MUTED))
                                        .child(format!("{} / {spot_count}", active_idx + 1)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .when(spot_count > 1, |row| {
                                row.child(render_hero_dock_chevron(
                                    "spotlight-prev",
                                    IconName::ChevronLeft,
                                    ui_scale,
                                    cx.listener(move |this, _, _, cx| {
                                        let next_idx = (this.board_spotlight_idx % spot_count
                                            + spot_count
                                            - 1)
                                            % spot_count;
                                        this.board_spotlight_idx = next_idx;
                                        ensure_dock_card_visible(
                                            &scroll_prev,
                                            next_idx,
                                            dims.card_w,
                                            dims.card_gap,
                                            container_w,
                                        );
                                        cx.notify();
                                    }),
                                ))
                                .child(render_hero_dock_chevron(
                                    "spotlight-next",
                                    IconName::ChevronRight,
                                    ui_scale,
                                    cx.listener(move |this, _, _, cx| {
                                        let next_idx =
                                            (this.board_spotlight_idx % spot_count + 1) % spot_count;
                                        this.board_spotlight_idx = next_idx;
                                        ensure_dock_card_visible(
                                            &scroll_next,
                                            next_idx,
                                            dims.card_w,
                                            dims.card_gap,
                                            container_w,
                                        );
                                        cx.notify();
                                    }),
                                ))
                            })
                            .when(show_mode_switcher, |row| {
                                row.child(render_hero_stage_mode_switcher(
                                    HeroStageMode::Featured,
                                    ui_scale,
                                    cx,
                                ))
                            }),
                    ),
            )
            .child(render_hero_dock_scroll_strip(
                "hero-featured-dock",
                dims,
                featured_scroll,
                card_els,
            ))
    }

    fn build_hero_display_from_cw(
        &self,
        cw: &ContinueWatchingCard,
        library_statuses: &HashMap<String, LibraryStatusTab>,
    ) -> ActiveHeroDisplay {
        let li = &cw.library_item;
        let ep_code = format_movietracker_episode_code(li.state.video_id.as_deref());
        let pct = li.progress().clamp(0.0, 100.0) as f32;
        let eyebrow = format_hero_cw_eyebrow(li, ep_code.as_deref(), cw.notification_count, pct);
        let primary_cta_label = match ep_code.as_deref() {
            Some(code) if li.state.time_offset > 0 => format!("Resume {code}"),
            Some(code) => format!("Watch {code}"),
            None if li.state.time_offset > 0 => "Resume".to_owned(),
            None => "Watch".to_owned(),
        };
        let in_library =
            library_statuses.contains_key(&li.id) || (!li.removed && !li.temp);

        ActiveHeroDisplay {
            id: li.id.clone(),
            r#type: li.r#type.clone(),
            name: li.name.clone(),
            video_id: li.state.video_id.clone(),
            primary_cta_label,
            eyebrow,
            description: cw.description.clone(),
            year: cw.year.clone(),
            imdb_rating: cw.imdb_rating.clone(),
            genres: cw.genres.clone(),
            backdrop_url: cw.backdrop_url.clone(),
            logo_url: cw.logo_url.clone(),
            poster_url: li.poster.as_ref().map(|u| u.to_string()),
            trailer_youtube_key: None,
            trailer_streams: cw.trailer_streams.clone(),
            preview: cw.preview.clone(),
            in_library,
        }
    }

    fn build_hero_display_from_spotlight(
        &self,
        spot: &HomeSpotlightCandidate,
        library_statuses: &HashMap<String, LibraryStatusTab>,
    ) -> ActiveHeroDisplay {
        let in_library = library_statuses.contains_key(&spot.id);
        ActiveHeroDisplay {
            id: spot.id.clone(),
            r#type: spot.r#type.clone(),
            name: spot.name.clone(),
            video_id: spot.video_id.clone(),
            primary_cta_label: if spot.is_resume {
                "Resume".to_owned()
            } else {
                "Watch".to_owned()
            },
            eyebrow: spot.eyebrow.clone(),
            description: spot.description.clone(),
            year: spot.year.clone(),
            imdb_rating: spot.imdb_rating.clone(),
            genres: spot.genres.clone(),
            backdrop_url: spot.backdrop_url.clone(),
            logo_url: spot.logo_url.clone(),
            poster_url: spot.poster_url.clone(),
            trailer_youtube_key: spot.trailer_youtube_key.clone(),
            trailer_streams: spot.trailer_streams.clone(),
            preview: spot.preview.clone(),
            in_library,
        }
    }

    fn resolve_cw_backdrop_url(&self, cw: &ContinueWatchingCard) -> Option<String> {
        let li = &cw.library_item;
        cw.backdrop_url
            .clone()
            .or_else(|| {
                self.core
                    .cached_enriched_meta(&li.id)
                    .and_then(|e| e.backdrop_url)
            })
            .or_else(|| {
                li.id.starts_with("tt").then(|| {
                    format!(
                        "https://images.metahub.space/background/medium/{}/img",
                        li.id
                    )
                })
            })
    }

    fn resolve_spotlight_backdrop_url(&self, spot: &HomeSpotlightCandidate) -> Option<String> {
        spot.backdrop_url
            .clone()
            .or_else(|| {
                self.core
                    .cached_enriched_meta(&spot.id)
                    .and_then(|e| e.backdrop_url)
            })
            .or_else(|| {
                spot.id.starts_with("tt").then(|| {
                    format!(
                        "https://images.metahub.space/background/medium/{}/img",
                        spot.id
                    )
                })
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn render_editorial_shelf(
        &self,
        shelf: &CachedBoardShelf,
        visible_range: Range<usize>,
        shelf_total_h: f32,
        container_w: f32,
        shelf_scroll: ScrollHandle,
        kb_row: usize,
        kb_cards: &mut Vec<KbFocusableCard>,
        theme: &ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let meta = &shelf.meta;
        let raw_index = meta.raw_index;
        let total_items = meta.total_items;
        let can_load_more = meta.can_load_more;
        let is_loading = meta.is_loading;
        let is_pinned = meta.is_pinned;
        let poster_metrics = PosterMetrics::from_theme(theme);
        let gap_px = poster_metrics.gap_px;
        let accent_ink = if theme.is_light {
            theme.accent_press
        } else {
            theme.accent
        };

        let (card_w, img_h, card_h) = shelf_card_dimensions(meta.aspect, theme);
        let stride = card_w + gap_px;
        let step_px = (container_w * 0.72).max(stride * 2.0);

        let vis_start = visible_range.start.min(shelf.items.len());
        let vis_end = visible_range.end.min(shelf.items.len());
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
                .flex_shrink_0()
                .overflow_hidden()
                .flex()
                .items_start()
                .gap(px(gap_px))
                .children((0..7).map(|_| {
                    div()
                        .w(px(card_w))
                        .h(px(img_h))
                        .flex_shrink_0()
                        .rounded(px(RADIUS_POSTER))
                        .bg(rgba(theme.glass_card_bg))
                        .border_1()
                        .border_color(rgba(theme.glass_border))
                }))
                .into_any_element()
        } else {
            let visible_cards = &shelf.cards[vis_start..vis_end];
            let items_for_dispatch = Arc::clone(&shelf.items);
            let cards_for_dispatch = Arc::clone(&shelf.cards);
            let shell_weak = cx.entity().downgrade();
            let dispatcher = card_action_dispatcher(move |item_idx, action, _window, cx| {
                let Some(shell) = shell_weak.upgrade() else {
                    return;
                };
                let Some(item) = items_for_dispatch.get(item_idx) else {
                    return;
                };
                let (in_library, is_watched) = cards_for_dispatch
                    .get(item_idx)
                    .map(|c| (c.in_library, c.watched))
                    .unwrap_or((false, false));
                let _ = shell.update(cx, |this, cx| match action {
                    PosterCardAction::Open | PosterCardAction::MiddleClick => {
                        this.open_meta_details(item.r#type.clone(), item.id.clone(), None, cx);
                    }
                    PosterCardAction::QuickPlay => {
                        this.quick_resume_or_open(item.r#type.clone(), item.id.clone(), None, cx);
                    }
                    PosterCardAction::ToggleLibrary => {
                        this.core.toggle_library_item(item.clone());
                        cx.notify();
                    }
                    PosterCardAction::QuickPeek => {
                        this.open_quick_peek(
                            QuickPeekTarget {
                                type_name: item.r#type.clone(),
                                id: item.id.clone(),
                                video_id: None,
                                title: item.name.clone(),
                                preview: Some(item.clone()),
                            },
                            cx,
                        );
                    }
                    PosterCardAction::ContextMenu(position) => {
                        this.open_browsing_context_menu(
                            BrowsingContextMenu::new(
                                position,
                                item.r#type.clone(),
                                item.id.clone(),
                                item.name.clone(),
                            )
                            .preview(Some(item.clone()))
                            .is_in_library(in_library)
                            .is_watched(is_watched),
                            cx,
                        );
                    }
                    PosterCardAction::Hover(hovered) => {
                        this.on_card_hover_prefetch(
                            &item.r#type,
                            &item.id,
                            item.background.as_ref().map(|u| u.as_str()),
                            hovered,
                            cx,
                        );
                    }
                });
            });

            let mut card_els: Vec<AnyElement> = Vec::with_capacity(visible_cards.len());
            for (rel_idx, card) in visible_cards.iter().enumerate() {
                let item_idx = vis_start + rel_idx;
                let item = &shelf.items[item_idx];

                let image_tex = if card.aspect == PosterAspect::Landscape16x9 {
                    self.poster_cache.resolve_widescreen_or_poster(
                        item.background.as_ref().map(|u| u.as_str()),
                        card.poster_url.as_deref(),
                    )
                } else {
                    card.poster_url
                        .as_deref()
                        .and_then(|u| self.poster_cache.resolve_poster_image(u))
                };

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

                let mut ctx =
                    PreparedPosterRenderCtx::new(item_idx, card, "board-poster", &dispatcher)
                        .fixed_width_px(Some(card_w))
                        .image(image_tex)
                        .is_focused(is_focused);
                if card.aspect != meta.aspect {
                    ctx = ctx.image_height_px(Some(img_h));
                }

                card_els.push(render_prepared_poster_card(ctx));
            }

            div()
                .id(("board-shelf-carousel", raw_index))
                .w_full()
                .h(px(card_h + 4.0))
                .flex_shrink_0()
                .overflow_x_scroll()
                .track_scroll(&shelf_scroll)
                .on_scroll(cx.listener(move |this, ev: &ScrollEvent, _, _cx| {
                    // GPUI's ScrollHandle listener notifies on offset change; only trigger pagination here.
                    let right_px = match ev.delta {
                        ScrollDelta::Pixels(p) => -f32::from(p.x),
                        ScrollDelta::Lines(l) => (-l.x) * 48.0,
                    };
                    if right_px <= 0.1 {
                        return;
                    }
                    let max_x = (scroll_ev.max_offset().x / px(1.0)).max(0.0);
                    let cur_x = (-scroll_ev.offset().x / px(1.0)).max(0.0);
                    if can_load_more
                        && !is_loading
                        && max_x > 0.0
                        && cur_x + right_px + 320.0 >= max_x
                    {
                        this.core.load_board_next_page(raw_index);
                    }
                }))
                .child(
                    div()
                        .h_full()
                        .flex()
                        .items_start()
                        .gap(px(gap_px))
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
                                    .bg(rgba(theme.glass_card_bg))
                                    .border_1()
                                    .border_color(rgba(theme.glass_border)),
                            )
                        }),
                )
                .into_any_element()
        };

        let glass_row_hover = theme.glass_row_hover_bg;
        let glass_border_strong = theme.glass_border_strong;
        let text_primary = theme.text_primary;

        let pinned_badge = is_pinned.then(|| {
            div()
                .flex_shrink_0()
                .px(theme.sp(8.0))
                .py(theme.sp(2.0))
                .rounded(px(RADIUS_PILL))
                .bg(rgba(theme.accent_soft))
                .border_1()
                .border_color(rgba(theme.border_accent))
                .font(mono_font())
                .text_size(theme.sp(10.5))
                .text_color(rgb(accent_ink))
                .child("Pinned")
                .into_any_element()
        });

        let shelf_controls = div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(theme.sp(6.0))
            .child(render_shelf_icon_button(
                ("shelf-prev", raw_index),
                IconName::ChevronLeft,
                IconWeight::Regular,
                false,
                theme,
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
                IconWeight::Regular,
                false,
                theme,
                cx.listener(move |this, _, _, cx| {
                    let max_x = (scroll_next.max_offset().x / px(1.0)).max(0.0);
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
                if is_pinned {
                    IconWeight::Fill
                } else {
                    IconWeight::Regular
                },
                is_pinned,
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.toggle_pin_board_shelf(pin_key.clone(), cx);
                }),
            ))
            .child(render_shelf_icon_button(
                ("shelf-hide", raw_index),
                IconName::EyeOff,
                IconWeight::Regular,
                false,
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.toggle_hide_board_shelf(hide_key.clone(), cx);
                }),
            ))
            .child(
                div()
                    .id(("shelf-see-all", raw_index))
                    .cursor_pointer()
                    .h(theme.sp(30.0))
                    .px(theme.sp(11.0))
                    .ml(theme.sp(4.0))
                    .rounded(px(RADIUS_PILL))
                    .bg(rgba(theme.glass_trough_bg))
                    .border_1()
                    .border_color(rgba(theme.glass_border))
                    .hover(move |s| {
                        s.bg(rgba(glass_row_hover))
                            .border_color(rgba(glass_border_strong))
                            .text_color(rgb(text_primary))
                    })
                    .flex()
                    .items_center()
                    .gap(theme.sp(4.0))
                    .font(ui_sans(FontWeight::MEDIUM))
                    .text_size(theme.sp(12.5))
                    .text_color(rgb(theme.text_secondary))
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
                        12.5 * theme.ui_scale,
                        rgb(theme.text_muted),
                    )),
            );

        let shelf_header = render_section_header(
            SectionHeader::new(meta.title.clone())
                .subtitle(meta.subtitle.clone().map(SharedString::from))
                .inline_badge(pinned_badge)
                .right_slot(Some(shelf_controls.into_any_element()))
                .single_line(true),
        );

        // Exact `.h(px(shelf_total_h)).overflow_hidden()` matches spacer height so
        // `max_offset_y` stays strictly invariant when shelves cross overscan boundaries.
        div()
            .id(("board-shelf", raw_index))
            .w_full()
            .h(px(shelf_total_h))
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap(theme.sp(SHELF_HEADER_GAP_PX))
            .child(shelf_header)
            .child(body_el)
    }

    fn render_hidden_shelves_drawer(
        &self,
        hidden_shelves: &[HiddenShelfInfo],
        theme: &ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let count = hidden_shelves.len();
        let is_open = self.board_hidden_drawer_open;
        let glass_row_hover = theme.glass_row_hover_bg;
        let glass_border_strong = theme.glass_border_strong;
        let accent_ink = if theme.is_light {
            theme.accent_press
        } else {
            theme.accent
        };

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
                            .h(theme.sp(36.0))
                            .px(theme.sp(14.0))
                            .rounded(px(RADIUS_PILL))
                            .bg(rgba(theme.glass_trough_bg))
                            .border_1()
                            .border_color(if is_open {
                                rgba(theme.border_accent)
                            } else {
                                rgba(theme.glass_border)
                            })
                            .hover(move |s| {
                                s.bg(rgba(glass_row_hover))
                                    .border_color(rgba(glass_border_strong))
                            })
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(theme.sp(13.0))
                            .text_color(if is_open {
                                rgb(theme.text_primary)
                            } else {
                                rgb(theme.text_secondary)
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
                                13.5 * theme.ui_scale,
                                rgb(theme.text_muted),
                            ))
                            .child(format!("Hidden shelves ({count})"))
                            .child(svg_icon(
                                if is_open {
                                    IconName::CaretUp
                                } else {
                                    IconName::ChevronDown
                                },
                                13.0 * theme.ui_scale,
                                rgb(theme.text_muted),
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
                            .h(theme.sp(34.0))
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
                        .bg(rgba(theme.glass_sheet_bg))
                        .border_1()
                        .border_color(rgba(theme.glass_border))
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
                                .h(theme.sp(34.0))
                                .px(theme.sp(12.0))
                                .rounded(px(RADIUS_CONTROL))
                                .bg(rgba(theme.glass_card_bg))
                                .border_1()
                                .border_color(rgba(theme.glass_border))
                                .hover(move |s| {
                                    s.bg(rgba(glass_row_hover))
                                        .border_color(rgba(glass_border_strong))
                                })
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .font(ui_sans(FontWeight::MEDIUM))
                                .text_size(theme.sp(12.5))
                                .text_color(rgb(theme.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.toggle_hide_board_shelf(restore_key.clone(), cx);
                                    }),
                                )
                                .child(svg_icon(
                                    IconName::Eye,
                                    13.0 * theme.ui_scale,
                                    rgb(accent_ink),
                                ))
                                .child(label)
                        })),
                )
            })
    }

    fn render_empty_board_banner(cx: &mut Context<Self>) -> impl IntoElement {
        render_empty_state(
            EmptyStateProps::new(
                IconName::Clapperboard,
                "Setting the projection room",
                "Sign in to sync your Stremio library and addons, or press Ctrl K to search any film or series immediately.",
            )
            .action(primary_accent_button(
                "empty-btn-signin",
                Some(IconName::User),
                "Sign in",
                cx.listener(|this, _, _, cx| {
                    this.navigate_tab(SidebarTab::Account, cx);
                }),
            ))
            .action(secondary_glass_button(
                "empty-btn-search",
                Some(IconName::Search),
                "Search catalog",
                false,
                cx.listener(|this, _, _, cx| {
                    this.open_command_palette(cx);
                }),
            )),
        )
    }
}

fn build_board_view_snapshot(
    model: &DesktopModel,
    key: BoardSnapshotKey,
    pinned_prefs: &[String],
    hidden_prefs: &[String],
    library_statuses: &HashMap<String, LibraryStatusTab>,
) -> BoardViewSnapshot {
    let cw_cards: Vec<ContinueWatchingCard> = model
        .continue_watching_preview
        .items
        .iter()
        .take(MAX_CW_FILMSTRIP_ITEMS)
        .map(|cw_item| {
            let li = &cw_item.library_item;
            let preview_ref = find_preview_ref_in_model(model, &li.id);
            ContinueWatchingCard {
                library_item: li.clone(),
                notification_count: cw_item.notifications,
                backdrop_url: preview_ref
                    .and_then(|p| p.background.as_ref().map(|u| u.to_string())),
                logo_url: preview_ref.and_then(|p| p.logo.as_ref().map(|u| u.to_string())),
                imdb_rating: preview_ref.and_then(extract_imdb_rating),
                year: preview_ref
                    .and_then(|p| p.release_info.clone())
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty()),
                description: preview_ref.and_then(|p| p.description.clone()),
                genres: preview_ref
                    .map(|p| extract_genres(p, 3))
                    .unwrap_or_default(),
                trailer_streams: preview_ref
                    .map(|p| p.trailer_streams.clone())
                    .unwrap_or_default(),
                preview: preview_ref.cloned(),
            }
        })
        .collect();
    let cw_total_count = model.continue_watching_preview.items.len();

    let total_board_catalogs = model.board.catalogs.len();
    let mut sortable_metas: Vec<(Option<usize>, BoardShelfMeta)> = Vec::new();
    let mut hidden_list: Vec<HiddenShelfInfo> = Vec::new();
    let mut seen_hidden_keys: HashSet<String> = HashSet::new();
    let mut pinned_to_load: Vec<usize> = Vec::new();

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

        let (section_title, section_sub) =
            editorialize_shelf_heading(idx, addon_cat_name.unwrap_or(raw_id), raw_type);

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
        if idx >= key.shelf_limit && !is_pinned {
            continue;
        }

        let needs_initial_load = first_page.content.is_none();
        if is_pinned && needs_initial_load {
            pinned_to_load.push(idx);
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

    for hidden_key in hidden_prefs {
        if seen_hidden_keys.insert(hidden_key.clone()) {
            hidden_list.push(HiddenShelfInfo {
                shelf_key: hidden_key.clone(),
                title: fallback_hidden_shelf_title(hidden_key),
                subtitle: None,
            });
        }
    }

    let ordered_metas = order_board_shelves(sortable_metas);

    let mut shelves = Vec::with_capacity(ordered_metas.len());
    for meta in ordered_metas {
        let items: Vec<MetaItemPreview> = model
            .board
            .catalogs
            .get(meta.raw_index)
            .map(|cat| iter_catalog_items(cat).cloned().collect())
            .unwrap_or_default();
        let cards: Vec<PreparedPosterCard> = items
            .iter()
            .map(|item| {
                let item_aspect = PosterAspect::from_stremio_poster_shape(&item.poster_shape);
                let lib_status = library_statuses.get(&item.id).copied();
                let in_library = lib_status.is_some();
                let watched = matches!(lib_status, Some(LibraryStatusTab::Watched));
                let tint = placeholder_tint_for_id_with_preset(&item.id, key.theme_preset);
                let rating = extract_imdb_rating(item);
                let genres = extract_genres(item, 1);
                let year = item
                    .release_info
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let subtitle = format_preview_subtitle(
                    &item.r#type,
                    year,
                    genres.first().map(String::as_str),
                );
                let type_badge = sentence_case_label(&item.r#type);
                let poster_url = item.poster.as_ref().map(|u| u.as_str());

                PreparedPosterCard::with_tint(SharedString::new(&item.name), tint)
                    .subtitle(Some(SharedString::from(subtitle)))
                    .year_str(year)
                    .imdb_rating_str(rating.as_deref())
                    .type_badge(Some(SharedString::from(type_badge)))
                    .poster_url_str(poster_url)
                    .aspect(item_aspect)
                    .status_tab(lib_status)
                    .in_library(in_library)
                    .watched(watched)
                    .capabilities(PosterCardCapabilities::STANDARD_BROWSING)
            })
            .collect();
        shelves.push(CachedBoardShelf {
            meta,
            items: Arc::from(items),
            cards: Arc::from(cards),
        });
    }

    let mut featured_candidates: Vec<HomeSpotlightCandidate> = Vec::new();
    if key.want_spotlight {
        let mut seen_spot = HashSet::new();
        for shelf in &shelves {
            if featured_candidates.len() >= MAX_FEATURED_CANDIDATES {
                break;
            }
            for item in shelf.items.iter().take(3) {
                if featured_candidates.len() >= MAX_FEATURED_CANDIDATES {
                    break;
                }
                if seen_spot.insert(item.id.clone()) {
                    featured_candidates.push(HomeSpotlightCandidate {
                        id: item.id.clone(),
                        r#type: item.r#type.clone(),
                        name: item.name.clone(),
                        video_id: None,
                        is_resume: false,
                        eyebrow: format!("FEATURED · {}", shelf.meta.title.to_ascii_uppercase()),
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
                        poster_url: item.poster.as_ref().map(|u| u.to_string()),
                        trailer_youtube_key: None,
                        trailer_streams: item.trailer_streams.clone(),
                        preview: Some(item.clone()),
                    });
                }
            }
        }
    }

    BoardViewSnapshot {
        key,
        cw_cards: Arc::from(cw_cards),
        cw_total_count,
        featured_candidates: Arc::from(featured_candidates),
        shelves: Arc::from(shelves),
        hidden_shelves: Arc::from(hidden_list),
        total_board_catalogs,
        pinned_indices_to_load: Arc::from(pinned_to_load),
    }
}

fn render_hero_scrim_layers(theme: &ThemePalette) -> Vec<AnyElement> {
    if theme.is_light {
        // Refined dark cinema scrim on LightMist so crisp white hero ink stays high-contrast,
        // with a soft bottom edge feather into the daylight canvas.
        vec![
            div()
                .absolute()
                .inset_0()
                .bg(linear_gradient(
                    180.0,
                    linear_color_stop(rgba(0x0B0D1480), 0.0),
                    linear_color_stop(rgba(0x0B0D1400), 0.30),
                ))
                .into_any_element(),
            div()
                .absolute()
                .inset_0()
                .bg(linear_gradient(
                    90.0,
                    linear_color_stop(rgba(0x0B0D14E0), 0.0),
                    linear_color_stop(rgba(0x0B0D1418), 0.78),
                ))
                .into_any_element(),
            div()
                .absolute()
                .inset_0()
                .bg(linear_gradient(
                    180.0,
                    linear_color_stop(rgba(0x0B0D1414), 0.14),
                    linear_color_stop(rgba(0x0B0D14EB), 0.90),
                ))
                .into_any_element(),
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(16.0))
                .bg(linear_gradient(
                    180.0,
                    linear_color_stop(rgba(theme.bg_canvas << 8), 0.0),
                    linear_color_stop(rgb(theme.bg_canvas), 1.0),
                ))
                .into_any_element(),
        ]
    } else {
        let c = theme.bg_canvas;
        let transparent_c = c << 8;
        vec![
            div()
                .absolute()
                .inset_0()
                .bg(linear_gradient(
                    180.0,
                    linear_color_stop(rgba((c << 8) | 0xA8), 0.0),
                    linear_color_stop(rgba(transparent_c), 0.34),
                ))
                .into_any_element(),
            div()
                .absolute()
                .inset_0()
                .bg(linear_gradient(
                    90.0,
                    linear_color_stop(rgba((c << 8) | 0xEC), 0.0),
                    linear_color_stop(rgba(transparent_c), 0.72),
                ))
                .into_any_element(),
            div()
                .absolute()
                .inset_0()
                .bg(linear_gradient(
                    180.0,
                    linear_color_stop(rgba(transparent_c), 0.14),
                    linear_color_stop(rgba((c << 8) | 0xD9), 0.74),
                ))
                .into_any_element(),
            div()
                .absolute()
                .inset_0()
                .bg(linear_gradient(
                    180.0,
                    linear_color_stop(rgba(transparent_c), 0.62),
                    linear_color_stop(rgb(c), 1.0),
                ))
                .into_any_element(),
        ]
    }
}

fn hero_meta_divider() -> gpui::Div {
    div()
        .w(px(1.0))
        .h(px(11.0))
        .flex_shrink_0()
        .bg(rgba(HERO_DIVIDER))
}

#[allow(clippy::too_many_arguments)]
fn render_hero_glass_pill(
    id: impl Into<ElementId>,
    icon: IconName,
    weight: IconWeight,
    label: impl Into<SharedString>,
    active: bool,
    accent: u32,
    accent_soft: u32,
    border_accent: u32,
    ui_scale: f32,
    on_click: impl Fn(&MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let fg = if active { accent } else { HERO_INK_PRIMARY };
    div()
        .id(id)
        .cursor_pointer()
        .h(px(38.0 * ui_scale))
        .px(px(15.0 * ui_scale))
        .rounded(px(RADIUS_PILL))
        .bg(if active {
            rgba(accent_soft)
        } else {
            rgba(HERO_GLASS_BG)
        })
        .border_1()
        .border_color(if active {
            rgba(border_accent)
        } else {
            rgba(HERO_GLASS_BORDER)
        })
        .hover(move |s| {
            s.bg(if active {
                rgba(accent_soft)
            } else {
                rgba(HERO_GLASS_HOVER_BG)
            })
            .border_color(if active {
                rgb(accent)
            } else {
                rgba(HERO_GLASS_BORDER_STRONG)
            })
        })
        .flex()
        .items_center()
        .justify_center()
        .gap(px(7.0 * ui_scale))
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(px(13.0 * ui_scale))
        .text_color(rgb(fg))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(svg_icon_weighted(icon, weight, 14.0 * ui_scale, rgb(fg)))
        .child(label.into())
}

fn render_hero_dock_chevron(
    id: impl Into<ElementId>,
    icon: IconName,
    ui_scale: f32,
    on_click: impl Fn(&MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .cursor_pointer()
        .w(px(28.0 * ui_scale))
        .h(px(28.0 * ui_scale))
        .rounded(px(RADIUS_PILL))
        .bg(rgba(HERO_GLASS_TROUGH_BG))
        .border_1()
        .border_color(rgba(HERO_GLASS_BORDER))
        .hover(|s| {
            s.bg(rgba(HERO_GLASS_HOVER_BG))
                .border_color(rgba(HERO_GLASS_BORDER_STRONG))
        })
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(svg_icon(icon, 13.0 * ui_scale, rgb(HERO_INK_PRIMARY)))
}

fn render_hero_stage_mode_switcher(
    active_mode: HeroStageMode,
    ui_scale: f32,
    cx: &mut Context<AppShell>,
) -> impl IntoElement {
    let is_cw = active_mode == HeroStageMode::ContinueWatching;
    let is_feat = active_mode == HeroStageMode::Featured;

    div()
        .id("hero-stage-mode-switcher")
        .p(px(3.0))
        .rounded(px(RADIUS_PILL))
        .bg(rgba(HERO_GLASS_TROUGH_BG))
        .border_1()
        .border_color(rgba(HERO_GLASS_BORDER))
        .flex()
        .items_center()
        .gap(px(2.0))
        .child(
            div()
                .id("hero-mode-cw")
                .cursor_pointer()
                .h(px(26.0 * ui_scale))
                .px(px(11.0 * ui_scale))
                .rounded(px(RADIUS_PILL))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.5 * ui_scale))
                .when(is_cw, |el| {
                    el.bg(rgb(0xF5F5F7))
                        .font(ui_sans(FontWeight::SEMIBOLD))
                        .text_color(rgb(0x0B0B0D))
                })
                .when(!is_cw, |el| {
                    el.font(ui_sans(FontWeight::MEDIUM))
                        .text_color(rgb(HERO_INK_SECONDARY))
                        .hover(|s| {
                            s.bg(rgba(0xFFFFFF18))
                                .text_color(rgb(HERO_INK_PRIMARY))
                        })
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        if this.board_hero_mode_featured {
                            this.board_hero_mode_featured = false;
                            cx.notify();
                        }
                    }),
                )
                .child("Continue Watching"),
        )
        .child(
            div()
                .id("hero-mode-featured")
                .cursor_pointer()
                .h(px(26.0 * ui_scale))
                .px(px(11.0 * ui_scale))
                .rounded(px(RADIUS_PILL))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.5 * ui_scale))
                .when(is_feat, |el| {
                    el.bg(rgb(0xF5F5F7))
                        .font(ui_sans(FontWeight::SEMIBOLD))
                        .text_color(rgb(0x0B0B0D))
                })
                .when(!is_feat, |el| {
                    el.font(ui_sans(FontWeight::MEDIUM))
                        .text_color(rgb(HERO_INK_SECONDARY))
                        .hover(|s| {
                            s.bg(rgba(0xFFFFFF18))
                                .text_color(rgb(HERO_INK_PRIMARY))
                        })
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        if !this.board_hero_mode_featured {
                            this.board_hero_mode_featured = true;
                            cx.notify();
                        }
                    }),
                )
                .child("Featured"),
        )
}

fn render_shelf_icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    weight: IconWeight,
    active: bool,
    theme: &ThemePalette,
    on_click: impl Fn(&MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let accent_ink = if theme.is_light {
        theme.accent_press
    } else {
        theme.accent
    };
    let accent = theme.accent;
    let accent_soft = theme.accent_soft;
    let glass_row_hover = theme.glass_row_hover_bg;
    let glass_border_strong = theme.glass_border_strong;

    div()
        .id(id)
        .cursor_pointer()
        .w(px(30.0 * theme.ui_scale))
        .h(px(30.0 * theme.ui_scale))
        .rounded(px(RADIUS_PILL))
        .bg(if active {
            rgba(accent_soft)
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
                rgba(accent_soft)
            } else {
                rgba(glass_row_hover)
            })
            .border_color(if active {
                rgb(accent)
            } else {
                rgba(glass_border_strong)
            })
        })
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(svg_icon_weighted(
            icon,
            weight,
            13.0 * theme.ui_scale,
            if active {
                rgb(accent_ink)
            } else {
                rgb(theme.text_secondary)
            },
        ))
}

#[inline]
fn hash_string_slice(items: &[String]) -> u64 {
    let mut hasher = DefaultHasher::new();
    items.len().hash(&mut hasher);
    for s in items {
        s.hash(&mut hasher);
    }
    hasher.finish()
}

#[inline]
fn hero_stage_height_px(viewport_h: f32) -> f32 {
    if !viewport_h.is_finite() || viewport_h <= 0.0 {
        return 540.0;
    }
    (viewport_h * 0.60).clamp(HERO_MIN_HEIGHT_PX, HERO_MAX_HEIGHT_PX)
}

#[inline]
fn resolve_hero_stage_mode(
    board_hero_mode_featured: bool,
    has_cw: bool,
    has_featured: bool,
) -> HeroStageMode {
    if !has_cw || (board_hero_mode_featured && has_featured) {
        HeroStageMode::Featured
    } else {
        HeroStageMode::ContinueWatching
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct HeroDockDimensions {
    card_w: f32,
    card_h: f32,
    card_gap: f32,
    scroll_h: f32,
    header_h: f32,
    dock_gap: f32,
}

#[inline]
fn hero_dock_dimensions(_mode: HeroStageMode, theme: &ThemePalette) -> HeroDockDimensions {
    let (card_w, card_h) = landscape_card_dimensions(theme);
    let card_gap = (12.0 * theme.ui_scale).ceil();
    let scroll_h = card_h + (8.0 * theme.ui_scale).ceil();
    // Matches `render_hero_stage_mode_switcher` (26px * ui_scale inner + 6px padding + 2px border).
    let header_h = (26.0 * theme.ui_scale + 8.0).ceil();
    let dock_gap = (10.0 * theme.ui_scale).ceil();
    HeroDockDimensions {
        card_w,
        card_h,
        card_gap,
        scroll_h,
        header_h,
        dock_gap,
    }
}

fn render_hero_dock_scroll_strip(
    id: impl Into<ElementId>,
    dims: HeroDockDimensions,
    scroll_handle: &ScrollHandle,
    cards: Vec<AnyElement>,
) -> impl IntoElement {
    div()
        .id(id)
        .w_full()
        .h(px(dims.scroll_h))
        .overflow_x_scroll()
        .track_scroll(scroll_handle)
        .child(
            div()
                .h_full()
                .py(px(2.0))
                .flex()
                .items_center()
                .gap(px(dims.card_gap))
                .children(cards),
        )
}

#[inline]
fn compute_dock_scroll_target_x(
    cur_x: f32,
    max_x: f32,
    viewport_w: f32,
    idx: usize,
    card_w: f32,
    card_gap: f32,
) -> Option<f32> {
    if !max_x.is_finite() || max_x <= 0.0 || !viewport_w.is_finite() || viewport_w <= 0.0 {
        return None;
    }
    let item_left = idx as f32 * (card_w + card_gap);
    let item_right = item_left + card_w;
    if item_left < cur_x {
        Some(item_left.clamp(0.0, max_x))
    } else if item_right > cur_x + viewport_w {
        Some((item_right - viewport_w).clamp(0.0, max_x))
    } else {
        None
    }
}

fn ensure_dock_card_visible(
    scroll: &ScrollHandle,
    idx: usize,
    card_w: f32,
    card_gap: f32,
    viewport_w: f32,
) {
    let max_x = (scroll.max_offset().x / px(1.0)).max(0.0);
    let cur_x = (-scroll.offset().x / px(1.0)).max(0.0);
    if let Some(target_x) =
        compute_dock_scroll_target_x(cur_x, max_x, viewport_w, idx, card_w, card_gap)
    {
        scroll.set_offset(point(px(-target_x), px(0.0)));
    }
}

fn format_featured_card_meta_lead(
    year: Option<&str>,
    genre: Option<&str>,
    raw_type: &str,
    has_rating: bool,
) -> String {
    let clean_year = year.map(str::trim).filter(|s| !s.is_empty());
    let clean_genre = genre.map(str::trim).filter(|s| !s.is_empty());
    if has_rating {
        clean_year
            .map(str::to_owned)
            .or_else(|| clean_genre.map(str::to_owned))
            .unwrap_or_else(|| sentence_case_label(raw_type))
    } else {
        format_preview_subtitle(raw_type, clean_year, clean_genre)
    }
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
    cat: &[stremio_core_runtime::stremio_core::models::common::ResourceLoadable<
        Vec<MetaItemPreview>,
    >],
) -> impl Iterator<Item = &MetaItemPreview> {
    cat.iter().flat_map(|page| match &page.content {
        Some(Loadable::Ready(items)) => items.as_slice(),
        _ => &[],
    })
}

fn strip_skip_extra(req: &ResourceRequest) -> ResourceRequest {
    let mut cloned = req.clone();
    cloned
        .path
        .extra
        .retain(|ExtraValue { name, .. }| !name.eq_ignore_ascii_case("skip"));
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
    if raw_type.is_empty()
        || clean
            .to_ascii_lowercase()
            .contains(&raw_type.to_ascii_lowercase())
    {
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
fn shelf_card_dimensions(aspect: PosterAspect, theme: &ThemePalette) -> (f32, f32, f32) {
    let metrics = PosterMetrics::from_theme(theme);
    let base_w = metrics.card_width_px;
    let card_w = match aspect {
        PosterAspect::Portrait2x3 => base_w,
        PosterAspect::Square1x1 => (base_w * 1.12).round(),
        PosterAspect::Landscape16x9 => (base_w * 1.54).round(),
    };
    let img_h = (card_w * aspect.height_over_width()).ceil();
    let card_h = img_h + metrics.metadata_height_px;
    (card_w, img_h, card_h)
}

#[inline]
fn shelf_total_height_px(card_h: f32, has_subtitle: bool, ui_scale: f32) -> f32 {
    let header_base = if has_subtitle {
        SHELF_HEADER_WITH_SUBTITLE_HEIGHT_PX
    } else {
        SHELF_HEADER_HEIGHT_PX
    };
    (header_base * ui_scale).ceil() + (SHELF_HEADER_GAP_PX * ui_scale).ceil() + (card_h + 4.0)
}

#[inline]
fn initial_shelves_top_y(hero_height: Option<f32>, ui_scale: f32) -> f32 {
    match hero_height {
        Some(h) => h + PAGE_SECTION_GAP_PX * ui_scale,
        None => 88.0 * ui_scale,
    }
}

fn resolve_trailer_target(
    trailer_streams: &[Stream],
    trailer_youtube_key: Option<&str>,
) -> Option<SpotlightTrailerTarget> {
    if let Some(direct) = trailer_streams.iter().find(|s| {
        matches!(
            &s.source,
            StreamSource::Url { url } if url.scheme() == "http" || url.scheme() == "https"
        )
    }) {
        return Some(SpotlightTrailerTarget::DirectStream(Box::new(
            direct.clone(),
        )));
    }

    let yt_key = trailer_youtube_key
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            trailer_streams.iter().find_map(|s| match &s.source {
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

fn format_cw_remaining_or_status(li: &LibraryItem, notification_count: usize, pct: f32) -> String {
    if li.state.time_offset > 0 {
        format_cw_remaining(li, pct.max(2.0))
    } else if notification_count > 0 {
        "New episode".to_owned()
    } else {
        sentence_case_label(&li.r#type)
    }
}

fn format_hero_cw_eyebrow(
    li: &LibraryItem,
    ep_code: Option<&str>,
    notification_count: usize,
    pct: f32,
) -> String {
    let mut parts: Vec<String> = vec!["CONTINUE WATCHING".to_owned()];
    if let Some(code) = ep_code {
        parts.push(code.to_owned());
    }
    if li.state.time_offset > 0 {
        parts.push(format_cw_remaining(li, pct.max(2.0)));
    } else if notification_count > 0 {
        parts.push(if notification_count == 1 {
            "NEW EPISODE".to_owned()
        } else {
            format!("{notification_count} NEW EPISODES")
        });
    }
    parts.join(" · ")
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
    use stremio_core_runtime::stremio_core::types::library::LibraryItemState;
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
        assert_eq!(
            compute_horizontal_window(50, 0.0, 1200.0, 176.0, 18.0, 2),
            0..9
        );
        assert_eq!(
            compute_horizontal_window(50, 1940.0, 1200.0, 176.0, 18.0, 2),
            8..19
        );
        assert_eq!(
            compute_horizontal_window(20, 3500.0, 1200.0, 176.0, 18.0, 2),
            16..20
        );
    }

    #[test]
    fn spotlight_trailer_prefers_direct_http_stream_over_youtube() {
        let mut streams = Vec::new();
        match resolve_trailer_target(&streams, Some("Way9Dexny3w")) {
            Some(SpotlightTrailerTarget::YouTubeUrl(url)) => {
                assert_eq!(url, "https://www.youtube.com/watch?v=Way9Dexny3w");
            }
            _ => panic!("expected YouTube trailer URL"),
        }

        streams.push(Stream {
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
            resolve_trailer_target(&streams, Some("Way9Dexny3w")),
            Some(SpotlightTrailerTarget::DirectStream(_))
        ));
    }

    #[test]
    fn hero_stage_height_and_fluid_padding_clamp_cleanly() {
        assert_eq!(hero_stage_height_px(600.0), 460.0);
        assert_eq!(hero_stage_height_px(900.0), 540.0);
        assert_eq!(hero_stage_height_px(1440.0), 640.0);
        assert_eq!(hero_stage_height_px(f32::NAN), 540.0);

        assert_eq!(page_gutter_px(640.0), 24.0);
        assert_eq!(page_gutter_px(1200.0), 36.0);
        assert_eq!(page_gutter_px(2560.0), 56.0);
        assert_eq!(page_gutter_px(f32::NAN), 32.0);
    }

    #[test]
    fn hero_stage_mode_defaults_to_featured_when_cw_is_empty() {
        assert_eq!(
            resolve_hero_stage_mode(false, false, true),
            HeroStageMode::Featured
        );
        assert_eq!(
            resolve_hero_stage_mode(false, true, true),
            HeroStageMode::ContinueWatching
        );
        assert_eq!(
            resolve_hero_stage_mode(true, true, true),
            HeroStageMode::Featured
        );
        assert_eq!(
            resolve_hero_stage_mode(true, true, false),
            HeroStageMode::ContinueWatching
        );
    }

    #[test]
    fn shelf_card_dimensions_adapt_to_card_density_and_ui_scale() {
        use crate::theme::{
            build_palette, AccentChoice, CardDensityChoice, ThemePreset, UiScaleChoice,
        };

        for scale_choice in UiScaleChoice::ALL {
            let scale = scale_choice.factor();
            let make_theme = |density| {
                build_palette(
                    ThemePreset::EtherealDark,
                    AccentChoice::EmberCoral,
                    true,
                    scale,
                    density,
                )
            };

            let t_compact = make_theme(CardDensityChoice::Compact);
            let t_comfy = make_theme(CardDensityChoice::Comfortable);
            let t_cinema = make_theme(CardDensityChoice::Cinema);

            let (w_compact, _, h_compact) =
                shelf_card_dimensions(PosterAspect::Portrait2x3, &t_compact);
            let (w_comfy, _, h_comfy) = shelf_card_dimensions(PosterAspect::Portrait2x3, &t_comfy);
            let (w_cinema, _, h_cinema) =
                shelf_card_dimensions(PosterAspect::Portrait2x3, &t_cinema);

            let m_compact = PosterMetrics::from_theme(&t_compact);
            let m_comfy = PosterMetrics::from_theme(&t_comfy);
            let m_cinema = PosterMetrics::from_theme(&t_cinema);

            assert!((w_compact - m_compact.card_width_px).abs() < 1e-4);
            assert!((w_comfy - m_comfy.card_width_px).abs() < 1e-4);
            assert!((w_cinema - m_cinema.card_width_px).abs() < 1e-4);
            assert_eq!(h_compact, m_compact.card_height_px(PosterAspect::Portrait2x3));
            assert_eq!(h_comfy, m_comfy.card_height_px(PosterAspect::Portrait2x3));
            assert_eq!(h_cinema, m_cinema.card_height_px(PosterAspect::Portrait2x3));
            assert!(w_compact < w_comfy && w_comfy < w_cinema);
        }
    }

    #[test]
    fn board_snapshot_key_and_cw_eyebrow_behave_as_expected() {
        let h1 = hash_string_slice(&["a".to_owned(), "b".to_owned()]);
        let h2 = hash_string_slice(&["a".to_owned(), "b".to_owned()]);
        let h3 = hash_string_slice(&["b".to_owned(), "a".to_owned()]);
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);

        let li = LibraryItem {
            id: "tt0903747".to_owned(),
            r#type: "series".to_owned(),
            name: "Breaking Bad".to_owned(),
            poster: None,
            poster_shape: PosterShape::Poster,
            removed: false,
            temp: false,
            ctime: None,
            mtime: Default::default(),
            state: LibraryItemState {
                time_offset: 20 * 60_000,
                duration: 50 * 60_000,
                video_id: Some("tt0903747:1:1".to_owned()),
                ..Default::default()
            },
            behavior_hints: Default::default(),
        };
        let eyebrow = format_hero_cw_eyebrow(&li, Some("S1 E1"), 0, 40.0);
        assert_eq!(eyebrow, "CONTINUE WATCHING · S1 E1 · 30m left");
    }

    #[test]
    fn hero_cw_and_featured_dock_dimensions_match_across_all_densities() {
        use crate::theme::{
            build_palette, AccentChoice, CardDensityChoice, ThemePreset, UiScaleChoice,
        };

        for scale_choice in UiScaleChoice::ALL {
            let ui_scale = scale_choice.factor();
            let mut prev_w = 0.0_f32;
            for density in CardDensityChoice::ALL {
                let theme = build_palette(
                    ThemePreset::EtherealDark,
                    AccentChoice::EmberCoral,
                    true,
                    ui_scale,
                    density,
                );
                let (expected_w, expected_h) = landscape_card_dimensions(&theme);
                let cw_dims = hero_dock_dimensions(HeroStageMode::ContinueWatching, &theme);
                let featured_dims = hero_dock_dimensions(HeroStageMode::Featured, &theme);

                assert_eq!(cw_dims, featured_dims);
                assert_eq!(cw_dims.card_w, expected_w);
                assert_eq!(cw_dims.card_h, expected_h);
                assert_eq!(cw_dims.scroll_h, cw_dims.card_h + (8.0 * ui_scale).ceil());
                assert_eq!(
                    featured_dims.scroll_h,
                    featured_dims.card_h + (8.0 * ui_scale).ceil()
                );
                assert!(cw_dims.header_h >= 26.0 * ui_scale + 8.0);
                assert_eq!(
                    cw_dims.header_h + cw_dims.dock_gap + cw_dims.scroll_h,
                    featured_dims.header_h + featured_dims.dock_gap + featured_dims.scroll_h
                );
                assert!(cw_dims.card_w > prev_w);
                prev_w = cw_dims.card_w;
            }
        }
    }

    #[test]
    fn featured_card_meta_lead_and_dock_scroll_target_behave_cleanly() {
        assert_eq!(
            format_featured_card_meta_lead(Some("2024"), Some("Sci-Fi"), "movie", true),
            "2024"
        );
        assert_eq!(
            format_featured_card_meta_lead(None, Some("Sci-Fi"), "movie", true),
            "Sci-Fi"
        );
        assert_eq!(
            format_featured_card_meta_lead(None, None, "series", true),
            "Series"
        );
        assert_eq!(
            format_featured_card_meta_lead(Some("2024"), Some("Drama"), "movie", false),
            "2024 · Drama"
        );
        assert_eq!(
            format_featured_card_meta_lead(None, None, "movie", false),
            "Movie"
        );

        // Card already visible -> no scroll needed.
        assert_eq!(
            compute_dock_scroll_target_x(0.0, 800.0, 900.0, 2, 214.0, 12.0),
            None
        );
        // Card beyond right edge -> scrolls right enough to reveal card.
        assert_eq!(
            compute_dock_scroll_target_x(0.0, 800.0, 600.0, 3, 214.0, 12.0),
            Some(292.0)
        );
        // Card to the left of viewport -> scrolls back to card start.
        assert_eq!(
            compute_dock_scroll_target_x(300.0, 800.0, 600.0, 1, 214.0, 12.0),
            Some(226.0)
        );
    }

    #[test]
    fn shelf_total_height_covers_single_line_header_gap_and_card_across_densities_and_scales() {
        use crate::theme::{build_palette, AccentChoice, CardDensityChoice, ThemePreset};

        for ui_scale in [0.9_f32, 1.0, 1.1, 1.25] {
            // Single-line SectionHeader rendered height:
            // - title line: 26.0 * 1.1 = 28.6 * ui_scale; right controls height: 30.0 * ui_scale.
            // - with subtitle: 28.6 + 2.0 (gap) + 16.0 (explicit subtitle line-height) = 46.6 * ui_scale.
            let title_only_header_h = (26.0 * 1.1_f32).max(30.0) * ui_scale;
            let subtitle_header_h = (26.0 * 1.1 + 2.0 + 16.0) * ui_scale;
            let header_gap_h = (SHELF_HEADER_GAP_PX * ui_scale).ceil();

            for density in CardDensityChoice::ALL {
                let theme = build_palette(
                    ThemePreset::EtherealDark,
                    AccentChoice::EmberCoral,
                    true,
                    ui_scale,
                    density,
                );
                let poster_metrics = PosterMetrics::from_theme(&theme);

                for aspect in [
                    PosterAspect::Portrait2x3,
                    PosterAspect::Square1x1,
                    PosterAspect::Landscape16x9,
                ] {
                    let (_card_w, img_h, card_h) = shelf_card_dimensions(aspect, &theme);
                    assert!(
                        card_h >= img_h + poster_metrics.metadata_height_px,
                        "card_h ({card_h}) must fit poster ({img_h}) + meta ({}) at scale {ui_scale}",
                        poster_metrics.metadata_height_px
                    );

                    let total_no_sub = shelf_total_height_px(card_h, false, ui_scale);
                    let required_no_sub = title_only_header_h + header_gap_h + (card_h + 4.0);
                    assert!(
                        total_no_sub >= required_no_sub,
                        "shelf height without subtitle ({total_no_sub}) < required ({required_no_sub}) at scale {ui_scale}, density {density:?}, aspect {aspect:?}"
                    );

                    let total_with_sub = shelf_total_height_px(card_h, true, ui_scale);
                    let required_with_sub = subtitle_header_h + header_gap_h + (card_h + 4.0);
                    assert!(
                        total_with_sub >= required_with_sub,
                        "shelf height with subtitle ({total_with_sub}) < required ({required_with_sub}) at scale {ui_scale}, density {density:?}, aspect {aspect:?}"
                    );
                }
            }
        }
    }
}
