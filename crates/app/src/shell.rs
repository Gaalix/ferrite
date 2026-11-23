use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use gpui::{
    div, img, linear_color_stop, linear_gradient, point, prelude::*, px, rgb, rgba, AnimationExt,
    AsyncApp, ClipboardItem, Context, CursorStyle, Decorations, FocusHandle, Focusable, FontWeight,
    KeyDownEvent, KeyUpEvent, MouseButton, NavigationDirection, ObjectFit, PathPromptOptions,
    Render, RenderImage, ResizeEdge, ScrollDelta, ScrollHandle, SharedString, WeakEntity, Window,
    WindowBackgroundAppearance, WindowControlArea,
};
use gpui_mpv::{
    extract_audio_pcm_s16le_8khz, extract_embedded_text_subtitle_window, MpvThumbnailer,
    PlayerEvent, PlayerSnapshot, SecondaryForcedMode, TrackType,
};
use stremio_core_runtime::{
    assess_embedded_subtitles, detect_clipboard_addon_url, has_full_embedded_subtitle_for_lang,
    human_language_name, is_image_subtitle_codec, is_native_playable_stream, normalize_addon_url,
    select_alternative_embedded_full_track, select_embedded_forced_track,
    select_embedded_image_track_for_inline_forced, select_preferred_tracks,
    stremio_core::{
        constants::CINEMETA_URL,
        models::{
            catalog_with_filters::Selected as CatalogWithFiltersSelected, common::Loadable,
        },
        runtime::{
            msg::{Action, ActionCtx, Event as CoreEvent},
            RuntimeEvent,
        },
        types::{
            addon::{ExtraValue, ResourcePath, ResourceRequest},
            resource::{MetaItem, StreamSource, Video},
        },
    },
    build_unified_editorial_search, subtitle_lang_matches, AudioSpeechFingerprint,
    AudioSpeechWindow, CoreController, DebridCacheStatus, DesktopModelField, EditorialMediaHit,
    EditorialPersonHit, EmbeddedSubtitleQuality, EnrichedCollection, FetchedNativeSubtitle,
    ForeignTranslationStatus, LocalDebridProvider, NativeSubtitleQuery, PersonFilmography,
    PrivateIntegrationsConfig, ProwlarrFallbackMode, ResolvedNextEpisode, SecretApiKey,
    SeriesTrackPreference, SubtitleAlignmentResult, SubtitleMatchTier, SubtitleSearchMode,
    SubtitleSyncAligner, TrackCandidate, UnifiedEditorialSearch, INITIAL_BOARD_SHELVES,
};
use stremio_stream_engine::{opensubhash::compute_opensubhash_http, StreamEngineHandle};
use url::Url;

use crate::{
    icons::{svg_icon, IconName},
    os_integration::{
        should_inhibit_display_sleep, OsMediaCommand, OsMediaController, OsMediaMetadata,
        OsPlaybackStatus, SleepInhibitor,
    },
    poster_cache::PosterCache,
    state::{
        apply_key_to_text_buffer_with_cursor, build_subtitles_resource_path,
        classify_library_item_status, default_library_selected, detect_addon_url_candidate,
        find_cached_preview_in_model, format_core_error_message, format_text_with_caret,
        is_loopback_p2p_torrent_url, is_scrub_thumbnail_stale, merge_and_enrich_stream_candidates,
        parse_season_episode_from_video_id, pick_default_series_episode_id,
        quantize_scrub_bucket_ms, resolve_ready_meta_item, rewrite_loopback_engine_url,
        route_affected_by_fields, step_kb_focus_2d, switch_engine_storage_mode_on_tokio,
        sync_memoized_meta_streams, trim_process_heap, AddonCategoryFilter, AddonSubTab,
        AppPreferences, AppRoute, BootServices, BrowsingContextMenu, DebridAccessMode,
        DeferredStreamEngine, DiscoverDropdown, DiscoverViewMode, DiscoverViewSnapshot,
        FocusedInput, KbFocusableCard, LibraryFilterTab, LibrarySortCriterion, LibrarySortOrder,
        LibraryStatusTab, LibraryTypeFilter, LibraryViewMode, MemoizedEnrichedStreams,
        MemoizedLibraryView, MemoizedMetaStreams, NativeSubtitleMatchState,
        NativeSubtitleMatchStatus, NavHistoryEntry, PersonDeptFilter, PlaybackCandidate,
        PlaybackSession, PlayerPopover, QuickPeekTarget, SettingsCategory, SettingsDropdown,
        SidebarTab, StorageModeChoice, StreamFilterState, SubtitleBackgroundChoice, ToastKind,
        ToastMessage, ToneMappingChoice, UpscaleChoice, LIBRARY_PAGE_SIZE,
        MAX_AUTO_FAILOVER_ATTEMPTS, STREAM_PAGE_SIZE,
    },
    theme::{
        active_theme, glass_capsule_shadow, obsidian_fade_in, sp, transparent_cutout, ui_sans,
        AccentChoice, CardDensityChoice, ThemePreset, UiScaleChoice, RADIUS_CONTROL, RADIUS_PILL,
    },
    ui_components::{click_handler, glass_dropdown_panel, ClickHandler},
    updater::{self, AppliedUpdateOutcome, UpdateStatus},
};

fn shared_opensubhash_http_client() -> Option<&'static reqwest13::Client> {
    static CLIENT: OnceLock<Option<reqwest13::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest13::Client::builder()
                .timeout(Duration::from_millis(1200))
                .build()
                .ok()
        })
        .as_ref()
}

const TOAST_LIFETIME: Duration = Duration::from_millis(3600);
const PREWARM_HEAD_BYTES: u64 = 16 * 1024 * 1024;
const MAX_NAV_HISTORY: usize = 64;
const HEAP_TRIM_MIN_INTERVAL: Duration = Duration::from_secs(10);

pub struct AppShell {
    pub focus_handle: FocusHandle,
    pub tokio_handle: tokio::runtime::Handle,
    pub core: CoreController,
    pub engine: DeferredStreamEngine,
    pub poster_cache: PosterCache,
    pub preferences: AppPreferences,
    pub mpv: Option<gpui_mpv::MpvController>,
    pub thumbnailer: MpvThumbnailer,
    pub player_snapshot: PlayerSnapshot,

    pub route: AppRoute,
    pub nav_history: Vec<NavHistoryEntry>,
    pub nav_forward: Vec<NavHistoryEntry>,
    pub viewport_size_px: (f32, f32),
    pub exit_fullscreen_on_next_render: bool,
    pub window_blur_active: bool,
    pub last_window_bg_appearance: WindowBackgroundAppearance,
    pub command_palette_open: bool,
    pub avatar_menu_open: bool,
    pub context_menu: Option<BrowsingContextMenu>,
    pub quick_peek: Option<QuickPeekTarget>,
    pub quick_peek_scroll_handle: ScrollHandle,
    pub ambient_hero_backdrop_url: Option<String>,
    pub palette_query: String,
    pub palette_selected_idx: usize,
    pub palette_scroll_handle: ScrollHandle,
    pub focused_input: Option<FocusedInput>,
    pub input_cursors: HashMap<FocusedInput, usize>,
    pub input_selected_all: HashSet<FocusedInput>,
    pub kb_focus_index: Option<usize>,
    pub kb_focus_items: Vec<KbFocusableCard>,

    pub board_state_rev: u64,
    pub discover_state_rev: u64,
    pub library_state_rev: u64,
    pub calendar_state_rev: u64,
    pub meta_state_rev: u64,
    pub search_state_rev: u64,
    pub search_debounce_seq: u64,
    pub hover_prefetch_seq: u64,
    pub hover_prefetch_target: Option<String>,
    pub last_heap_trim_at: Option<Instant>,

    pub memoized_library_statuses: parking_lot::Mutex<(u64, Arc<HashMap<String, LibraryStatusTab>>)>,
    pub memoized_enriched_streams: Option<MemoizedEnrichedStreams>,
    pub memoized_discover_catalog: parking_lot::Mutex<Option<(u64, DiscoverViewSnapshot)>>,
    pub discover_last_next_page_len: Cell<Option<usize>>,
    pub memoized_library_items: parking_lot::Mutex<Option<MemoizedLibraryView>>,
    pub memoized_board_cache: parking_lot::Mutex<Option<Arc<dyn std::any::Any + Send + Sync>>>,

    pub auth_email_input: String,
    pub auth_password_input: String,
    pub addon_url_input: String,
    pub addon_sub_tab: AddonSubTab,
    pub addon_category_filter: AddonCategoryFilter,
    pub addon_search_query: String,
    pub clipboard_addon_suggestion: Option<String>,
    pub dismissed_clipboard_addon_url: Option<String>,
    pub installing_addon_urls: HashSet<String>,
    pub link_poll_active: bool,

    pub board_hero_index: usize,
    pub board_hero_mode_featured: bool,
    pub board_hover_debounce_seq: u64,
    pub board_active_cw_idx: usize,
    pub discover_loaded_once: bool,
    pub discover_hubs_scroll_handle: ScrollHandle,
    pub selected_season: Option<u32>,
    pub auto_selected_episode_for_meta: Option<(String, String)>,
    pub stream_filter: StreamFilterState,
    pub memoized_meta_streams: Option<MemoizedMetaStreams>,
    pub visible_stream_limit: usize,
    pub player_source_limit: usize,
    pub visible_board_shelves: usize,
    pub visible_library_limit: usize,
    pub meta_scroll_handle: ScrollHandle,
    pub board_scroll_handle: ScrollHandle,
    pub library_scroll_handle: ScrollHandle,
    pub calendar_scroll_handle: ScrollHandle,
    pub addons_scroll_handle: ScrollHandle,
    pub auth_scroll_handle: ScrollHandle,
    pub board_list_scroll_handle: gpui::UniformListScrollHandle,
    pub board_shelf_scrolls: HashMap<usize, ScrollHandle>,
    pub board_hidden_drawer_open: bool,
    pub discover_grid_scroll_handle: gpui::UniformListScrollHandle,
    pub library_grid_scroll_handle: gpui::UniformListScrollHandle,
    pub episode_list_scroll_handle: gpui::UniformListScrollHandle,
    pub meta_streams_scroll_handle: gpui::UniformListScrollHandle,
    pub search_movies_scroll: gpui::UniformListScrollHandle,
    pub search_series_scroll: gpui::UniformListScrollHandle,
    pub person_credits_scroll: gpui::UniformListScrollHandle,
    pub episode_list_scrolled_key: Option<(String, String)>,
    pub meta_addon_dropdown_open: bool,
    pub discover_open_dropdown: Option<DiscoverDropdown>,
    pub discover_catalog_search: String,
    pub expanded_stream_rows: HashSet<usize>,
    pub library_status_tab: LibraryStatusTab,
    pub library_filter_tab: LibraryFilterTab,
    pub library_type_filter: LibraryTypeFilter,
    pub library_sort_order: LibrarySortOrder,
    pub library_search_query: String,
    pub library_sort_dropdown_open: bool,
    pub library_sort_criterion: LibrarySortCriterion,

    pub mpv_subscriptions_attached: bool,
    pub playback_session: Option<PlaybackSession>,
    pub playback_generation_seq: u64,
    pub osd_hide_seq: u64,
    pub player_telemetry_pulse_seq: u64,
    pub player_popover: Option<PlayerPopover>,
    pub player_stats_for_nerds: bool,
    pub last_osd_activity: Instant,
    pub last_intro_visible: bool,
    pub last_next_ep_visible: bool,
    pub playback_speed: f64,
    pub hold_2x_active: bool,
    pub space_key_down: bool,
    pub is_scrubbing: bool,
    pub is_dragging_volume: bool,
    pub scrub_hover_fraction: Option<f32>,
    pub scrub_hover_metrics: Option<(f32, f32)>,
    pub hover_thumbnail_image: Option<Arc<RenderImage>>,
    pub hover_thumbnail_key: Option<(String, i64)>,
    pub in_flight_thumb_key: Option<(String, i64)>,
    pub next_episode_prewarmed: bool,
    pub prewarmed_next: Option<ResolvedNextEpisode>,
    pub prewarm_fetch_requested_vid: Option<String>,
    pub prewarmed_next_direct_url: Option<(String, String)>,
    pub native_sub_match: NativeSubtitleMatchState,
    pub pending_auto_select_sub_title: Option<String>,
    pub user_picked_subtitle: bool,
    pub secondary_forced_available: Option<(i64, bool)>,
    pub live_untranslated_upgrade_triggered: bool,
    pub pending_quick_resume: Option<(String, String, Option<String>)>,

    pub toasts: Vec<ToastMessage>,
    pub next_toast_id: u64,
    pub update_status: UpdateStatus,

    pub active_settings_category: SettingsCategory,
    pub settings_advanced_open: HashSet<SettingsCategory>,
    pub settings_search_query: String,
    pub settings_open_dropdown: Option<SettingsDropdown>,
    pub settings_scroll_handle: ScrollHandle,
    pub local_debrid_provider_idx: usize,
    pub local_debrid_direct_unrestrict: bool,
    pub local_debrid_cached_only: bool,
    pub real_debrid_key_input: String,
    pub torbox_key_input: String,
    pub alldebrid_key_input: String,
    pub real_debrid_key_configured: bool,
    pub torbox_key_configured: bool,
    pub alldebrid_key_configured: bool,
    pub debrid_verify_status: Option<String>,
    pub prowlarr_mode_idx: usize,
    pub prowlarr_url_input: String,
    pub prowlarr_key_input: String,
    pub prowlarr_key_configured: bool,
    pub prowlarr_timeout_ms: u64,
    pub prowlarr_test_status: Option<String>,
    pub opensubtitles_key_input: String,
    pub subdl_key_input: String,
    pub opensubtitles_key_configured: bool,
    pub subdl_key_configured: bool,
    pub last_debrid_latency_ms: Option<u64>,

    pub sleep_inhibitor: SleepInhibitor,
    pub os_media: OsMediaController,
    pub local_debrid_cache_map: HashMap<(LocalDebridProvider, [u8; 20]), DebridCacheStatus>,
    pub local_debrid_checked_hashes: HashSet<(LocalDebridProvider, [u8; 20])>,
    pub prowlarr_fallback_key: Option<(String, String, String)>,
    pub prowlarr_fallback_loading: bool,
    pub prowlarr_fallback_attempted: bool,
    pub prowlarr_fallback_streams: Vec<PlaybackCandidate>,
    pub prowlarr_fallback_error: Option<String>,
    pub prowlarr_request_seq: u64,

    pub tmdb_key_input: String,
    pub tmdb_key_configured: bool,
    pub tmdb_verify_status: Option<String>,
    pub full_search_query: String,
    pub full_search_results: UnifiedEditorialSearch,
    pub full_search_tmdb_hits: Vec<EditorialMediaHit>,
    pub full_search_tmdb_people: Vec<EditorialPersonHit>,
    pub full_search_loading_tmdb: bool,
    pub full_search_seq: u64,
    pub search_scroll_handle: ScrollHandle,
    pub active_person_filmography: Option<PersonFilmography>,
    pub person_loading: bool,
    pub person_request_seq: u64,
    pub person_scroll_handle: ScrollHandle,
    pub person_bio_expanded: bool,
    pub person_department_filter: PersonDeptFilter,
    pub person_sort_recent: bool,
    pub active_collection: Option<(u64, EnrichedCollection)>,
    pub loading_collection_id: Option<u64>,
    pub board_spotlight_idx: usize,
    pub enriched_attempted: HashSet<String>,
    pub synthetic_tmdb_meta: Option<(String, MetaItem)>,
}

impl Focusable for AppShell {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl AppShell {
    pub fn new(services: BootServices, window: &mut Window, cx: &mut Context<Self>) -> Self {
        services.preferences.apply_theme();
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        let mut core_rx = services.core.subscribe();
        let poster_rx = services.poster_cache.wakeup_receiver();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                match core_rx.recv().await {
                    Ok(ev) => {
                        if this
                            .update(cx, |shell, cx| {
                                shell.handle_core_runtime_event(&ev, cx);
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let _ = this.update(cx, |shell, cx| {
                            shell.sync_player_from_core_model(cx);
                            cx.notify();
                        });
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        })
        .detach();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while poster_rx.recv().await.is_ok() {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                while poster_rx.try_recv().is_ok() {}
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();

        if let Some(engine_rx) = services.engine_ready_rx {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                if let Ok(outcome) = engine_rx.recv().await {
                    let _ = this.update(cx, |shell, cx| {
                        shell.on_stream_engine_ready(outcome, cx);
                    });
                }
            })
            .detach();
        }

        let mpv_subscriptions_attached = if let Some(mpv) = &services.mpv {
            Self::configure_and_subscribe_mpv(mpv, &services.preferences, cx);
            true
        } else {
            false
        };

        #[cfg(target_os = "windows")]
        let windows_hwnd = window
            .windows_video_handles()
            .map(|h| h.hwnd)
            .filter(|&hwnd| hwnd != 0);
        #[cfg(not(target_os = "windows"))]
        let windows_hwnd: Option<isize> = None;

        let sleep_inhibitor = SleepInhibitor::new(services.tokio_handle.clone());
        let (mut os_media, os_media_rx) =
            OsMediaController::start(services.tokio_handle.clone(), windows_hwnd);
        os_media.set_enabled(services.preferences.enable_os_media_controls);

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Ok(cmd) = os_media_rx.recv().await {
                if this
                    .update(cx, |shell, cx| {
                        shell.handle_os_media_command(cmd, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let private_cfg = services.core.private_integrations();
        let last_debrid_latency_ms = services.core.last_debrid_resolve_latency_ms();

        let initial_vp = window.viewport_size();
        let viewport_size_px = (f32::from(initial_vp.width), f32::from(initial_vp.height));

        let cli_addon_url = services.initial_addon_url;
        let initial_route = if cli_addon_url.is_some() {
            services.core.load_remote_addons(None);
            AppRoute::Tab(SidebarTab::Addons)
        } else {
            AppRoute::Tab(SidebarTab::Home)
        };

        let mut shell = Self {
            focus_handle,
            tokio_handle: services.tokio_handle,
            core: services.core,
            engine: services.engine,
            poster_cache: services.poster_cache,
            preferences: services.preferences,
            mpv: services.mpv,
            thumbnailer: MpvThumbnailer::new(),
            player_snapshot: services.initial_snapshot,
            route: initial_route,
            nav_history: Vec::new(),
            nav_forward: Vec::new(),
            viewport_size_px,
            exit_fullscreen_on_next_render: false,
            window_blur_active: true,
            last_window_bg_appearance: WindowBackgroundAppearance::Blurred,
            command_palette_open: false,
            avatar_menu_open: false,
            context_menu: None,
            quick_peek: None,
            quick_peek_scroll_handle: ScrollHandle::new(),
            ambient_hero_backdrop_url: None,
            palette_query: String::new(),
            palette_selected_idx: 0,
            palette_scroll_handle: ScrollHandle::new(),
            focused_input: None,
            input_cursors: HashMap::new(),
            input_selected_all: HashSet::new(),
            kb_focus_index: None,
            kb_focus_items: Vec::new(),
            board_state_rev: 1,
            discover_state_rev: 1,
            library_state_rev: 1,
            calendar_state_rev: 1,
            meta_state_rev: 1,
            search_state_rev: 1,
            search_debounce_seq: 0,
            hover_prefetch_seq: 0,
            hover_prefetch_target: None,
            last_heap_trim_at: None,
            memoized_library_statuses: parking_lot::Mutex::new((0, Arc::new(HashMap::new()))),
            memoized_enriched_streams: None,
            memoized_discover_catalog: parking_lot::Mutex::new(None),
            discover_last_next_page_len: Cell::new(None),
            memoized_library_items: parking_lot::Mutex::new(None),
            memoized_board_cache: parking_lot::Mutex::new(None),
            auth_email_input: String::new(),
            auth_password_input: String::new(),
            addon_url_input: cli_addon_url.clone().unwrap_or_default(),
            addon_sub_tab: AddonSubTab::Installed,
            addon_category_filter: AddonCategoryFilter::All,
            addon_search_query: String::new(),
            clipboard_addon_suggestion: None,
            dismissed_clipboard_addon_url: None,
            installing_addon_urls: HashSet::new(),
            link_poll_active: false,
            board_hero_index: 0,
            board_hero_mode_featured: false,
            board_hover_debounce_seq: 0,
            board_active_cw_idx: 0,
            discover_loaded_once: false,
            discover_hubs_scroll_handle: ScrollHandle::new(),
            selected_season: None,
            auto_selected_episode_for_meta: None,
            stream_filter: StreamFilterState::default(),
            memoized_meta_streams: None,
            visible_stream_limit: STREAM_PAGE_SIZE,
            player_source_limit: STREAM_PAGE_SIZE,
            visible_board_shelves: INITIAL_BOARD_SHELVES,
            visible_library_limit: LIBRARY_PAGE_SIZE,
            meta_scroll_handle: ScrollHandle::new(),
            board_scroll_handle: ScrollHandle::new(),
            library_scroll_handle: ScrollHandle::new(),
            calendar_scroll_handle: ScrollHandle::new(),
            addons_scroll_handle: ScrollHandle::new(),
            auth_scroll_handle: ScrollHandle::new(),
            board_list_scroll_handle: gpui::UniformListScrollHandle::new(),
            board_shelf_scrolls: HashMap::new(),
            board_hidden_drawer_open: false,
            discover_grid_scroll_handle: gpui::UniformListScrollHandle::new(),
            library_grid_scroll_handle: gpui::UniformListScrollHandle::new(),
            episode_list_scroll_handle: gpui::UniformListScrollHandle::new(),
            meta_streams_scroll_handle: gpui::UniformListScrollHandle::new(),
            search_movies_scroll: gpui::UniformListScrollHandle::new(),
            search_series_scroll: gpui::UniformListScrollHandle::new(),
            person_credits_scroll: gpui::UniformListScrollHandle::new(),
            episode_list_scrolled_key: None,
            meta_addon_dropdown_open: false,
            discover_open_dropdown: None,
            discover_catalog_search: String::new(),
            expanded_stream_rows: HashSet::new(),
            library_status_tab: LibraryStatusTab::Watching,
            library_filter_tab: LibraryFilterTab::All,
            library_type_filter: LibraryTypeFilter::All,
            library_sort_order: LibrarySortOrder::Recent,
            library_search_query: String::new(),
            library_sort_dropdown_open: false,
            library_sort_criterion: LibrarySortCriterion::default(),
            mpv_subscriptions_attached,
            playback_session: None,
            playback_generation_seq: 1,
            osd_hide_seq: 0,
            player_telemetry_pulse_seq: 0,
            player_popover: None,
            player_stats_for_nerds: false,
            last_osd_activity: Instant::now(),
            last_intro_visible: false,
            last_next_ep_visible: false,
            playback_speed: 1.0,
            hold_2x_active: false,
            space_key_down: false,
            is_scrubbing: false,
            is_dragging_volume: false,
            scrub_hover_fraction: None,
            scrub_hover_metrics: None,
            hover_thumbnail_image: None,
            hover_thumbnail_key: None,
            in_flight_thumb_key: None,
            next_episode_prewarmed: false,
            prewarmed_next: None,
            prewarm_fetch_requested_vid: None,
            prewarmed_next_direct_url: None,
            native_sub_match: NativeSubtitleMatchState::default(),
            pending_auto_select_sub_title: None,
            user_picked_subtitle: false,
            secondary_forced_available: None,
            live_untranslated_upgrade_triggered: false,
            pending_quick_resume: None,
            toasts: Vec::new(),
            next_toast_id: 1,
            update_status: UpdateStatus::Idle,
            active_settings_category: SettingsCategory::default(),
            settings_advanced_open: HashSet::new(),
            settings_search_query: String::new(),
            settings_open_dropdown: None,
            settings_scroll_handle: ScrollHandle::new(),
            local_debrid_provider_idx: local_debrid_provider_to_idx(private_cfg.active_debrid),
            local_debrid_direct_unrestrict: private_cfg.direct_rust_unrestrict,
            local_debrid_cached_only: private_cfg.debrid_cached_only_filter,
            real_debrid_key_input: String::new(),
            torbox_key_input: String::new(),
            alldebrid_key_input: String::new(),
            real_debrid_key_configured: !private_cfg.real_debrid_api_key.is_empty(),
            torbox_key_configured: !private_cfg.torbox_api_key.is_empty(),
            alldebrid_key_configured: !private_cfg.alldebrid_api_key.is_empty(),
            debrid_verify_status: None,
            prowlarr_mode_idx: prowlarr_mode_to_idx(private_cfg.prowlarr_mode),
            prowlarr_url_input: private_cfg.prowlarr_url,
            prowlarr_key_input: String::new(),
            prowlarr_key_configured: !private_cfg.prowlarr_api_key.is_empty(),
            prowlarr_timeout_ms: private_cfg.prowlarr_timeout_ms,
            prowlarr_test_status: None,
            opensubtitles_key_input: String::new(),
            subdl_key_input: String::new(),
            opensubtitles_key_configured: !private_cfg.opensubtitles_api_key.is_empty(),
            subdl_key_configured: !private_cfg.subdl_api_key.is_empty(),
            last_debrid_latency_ms,
            sleep_inhibitor,
            os_media,
            local_debrid_cache_map: HashMap::new(),
            local_debrid_checked_hashes: HashSet::new(),
            prowlarr_fallback_key: None,
            prowlarr_fallback_loading: false,
            prowlarr_fallback_attempted: false,
            prowlarr_fallback_streams: Vec::new(),
            prowlarr_fallback_error: None,
            prowlarr_request_seq: 0,
            tmdb_key_input: String::new(),
            tmdb_key_configured: !private_cfg.tmdb_api_key.is_empty(),
            tmdb_verify_status: None,
            full_search_query: String::new(),
            full_search_results: UnifiedEditorialSearch::default(),
            full_search_tmdb_hits: Vec::new(),
            full_search_tmdb_people: Vec::new(),
            full_search_loading_tmdb: false,
            full_search_seq: 0,
            search_scroll_handle: ScrollHandle::new(),
            active_person_filmography: None,
            person_loading: false,
            person_request_seq: 0,
            person_scroll_handle: ScrollHandle::new(),
            person_bio_expanded: false,
            person_department_filter: PersonDeptFilter::default(),
            person_sort_recent: true,
            active_collection: None,
            loading_collection_id: None,
            board_spotlight_idx: 0,
            enriched_attempted: HashSet::new(),
            synthetic_tmdb_meta: None,
        };

        if cli_addon_url.is_some() {
            shell.submit_addon_manifest_lookup(cx);
        }
        shell
    }

    fn on_stream_engine_ready(
        &mut self,
        outcome: Result<StreamEngineHandle, String>,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(handle) => {
                let url = handle.base_url();
                handle.set_enable_upnp(self.preferences.enable_upnp);
                handle.set_first_byte_timeout(Duration::from_secs(
                    self.preferences.dead_swarm_timeout_secs.max(20),
                ));
                let desired_mode = self.preferences.storage_mode.to_engine_mode();
                let needs_mode_sync = handle.storage_mode() != desired_mode;
                self.engine.attach(handle);
                if !self.core.sync_loopback_streaming_server_url(url) {
                    self.core.reload_streaming_server();
                }
                if needs_mode_sync {
                    let engine_clone = self.engine.clone();
                    let tokio_handle = self.tokio_handle.clone();
                    cx.spawn(async move |_, _| {
                        let _ = switch_engine_storage_mode_on_tokio(
                            &tokio_handle,
                            engine_clone,
                            desired_mode,
                        )
                        .await;
                    })
                    .detach();
                }
                cx.notify();
            }
            Err(err) => {
                self.push_toast(
                    ToastKind::Error,
                    "Stream engine failed to start",
                    Some(err),
                    cx,
                );
            }
        }
    }

    pub fn schedule_debounced_heap_trim(&mut self) {
        if self
            .last_heap_trim_at
            .is_some_and(|t| t.elapsed() < HEAP_TRIM_MIN_INTERVAL)
        {
            return;
        }
        self.last_heap_trim_at = Some(Instant::now());
        drop(self.tokio_handle.spawn_blocking(trim_process_heap));
    }

    pub(crate) fn on_major_route_transition(&mut self, entering_player: bool) {
        self.poster_cache.trim_for_route_transition();
        if entering_player {
            self.poster_cache.trim_for_playback();
        }
        self.last_heap_trim_at = Some(Instant::now());
        drop(self.tokio_handle.spawn_blocking(trim_process_heap));
    }

    fn resolve_backdrop_url_for_id(
        &self,
        id: &str,
        preview: Option<&stremio_core_runtime::stremio_core::types::resource::MetaItemPreview>,
    ) -> Option<String> {
        let from_preview = preview
            .and_then(|p| p.background.as_ref())
            .map(|u| u.to_string())
            .filter(|s| !s.trim().is_empty());
        if from_preview.is_some() {
            return from_preview;
        }
        let enriched = self.core.cached_enriched_meta(id);
        if let Some(url) = enriched
            .as_ref()
            .and_then(|e| e.backdrop_url.as_ref())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
        {
            return Some(url);
        }
        let from_model = self.core.with_model(|m| {
            find_cached_preview_in_model(m, id)
                .and_then(|p| p.background.map(|u| u.to_string()))
                .filter(|s| !s.trim().is_empty())
        });
        if from_model.is_some() {
            return from_model;
        }
        let imdb_id = if id.starts_with("tt") {
            Some(id.to_owned())
        } else {
            enriched
                .and_then(|e| e.imdb_id)
                .filter(|s| s.starts_with("tt"))
        };
        imdb_id.map(|tt| format!("https://images.metahub.space/background/medium/{tt}/img"))
    }

    fn sync_ambient_backdrop_for_render(&mut self) {
        if let Some(peek) = self.quick_peek.as_ref() {
            if let Some(bg) = self.resolve_backdrop_url_for_id(&peek.id, peek.preview.as_ref()) {
                self.ambient_hero_backdrop_url = Some(bg);
            }
            return;
        }

        match &self.route {
            AppRoute::MetaDetails { id, .. } => {
                let meta_bg = self.core.with_model(|m| {
                    resolve_ready_meta_item(&m.meta_details.meta_items)
                        .filter(|(item, _)| item.preview.id == *id)
                        .and_then(|(item, _)| item.preview.background.as_ref().map(|u| u.to_string()))
                });
                let resolved = meta_bg.or_else(|| self.resolve_backdrop_url_for_id(id, None));
                if resolved.is_some() {
                    self.ambient_hero_backdrop_url = resolved;
                }
            }
            AppRoute::Tab(SidebarTab::Home) if self.ambient_hero_backdrop_url.is_none() => {
                let spotlight_id = self.core.with_model(|m| {
                    m.continue_watching_preview
                        .items
                        .first()
                        .map(|cw| cw.library_item.id.clone())
                        .or_else(|| {
                            m.board.catalogs.iter().find_map(|cat| {
                                cat.first().and_then(|p| match &p.content {
                                    Some(Loadable::Ready(items)) => {
                                        items.first().map(|i| i.id.clone())
                                    }
                                    _ => None,
                                })
                            })
                        })
                });
                if let Some(id) = spotlight_id {
                    if let Some(bg) = self.resolve_backdrop_url_for_id(&id, None) {
                        self.ambient_hero_backdrop_url = Some(bg);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn open_quick_peek(&mut self, target: QuickPeekTarget, cx: &mut Context<Self>) {
        self.avatar_menu_open = false;
        self.context_menu = None;
        self.command_palette_open = false;
        self.discover_open_dropdown = None;
        self.meta_addon_dropdown_open = false;
        self.library_sort_dropdown_open = false;
        self.board_hidden_drawer_open = false;
        self.settings_open_dropdown = None;
        self.quick_peek_scroll_handle
            .set_offset(point(px(0.0), px(0.0)));

        if let Some(bg) = self.resolve_backdrop_url_for_id(&target.id, target.preview.as_ref()) {
            self.ambient_hero_backdrop_url = Some(bg);
        }

        let target_type = target.type_name.clone();
        let target_id = target.id.clone();
        let target_vid = target.video_id.clone();
        self.quick_peek = Some(target);

        self.request_background_enrichment_if_needed(&target_type, &target_id, cx);

        let on_other_meta_details = matches!(
            &self.route,
            AppRoute::MetaDetails { id, .. } if id != &target_id
        );
        if !on_other_meta_details {
            if target_id.starts_with("tmdb:") {
                let core = self.core.clone();
                let req_type = target_type.clone();
                let req_id = target_id.clone();
                let req_vid = target_vid.clone();
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    if let Some(imdb_id) = core
                        .resolve_canonical_imdb_id(req_type.clone(), req_id.clone())
                        .await
                    {
                        let _ = this.update(cx, |shell, cx| {
                            if shell
                                .quick_peek
                                .as_ref()
                                .is_some_and(|p| p.id == req_id)
                            {
                                shell.request_background_enrichment_if_needed(
                                    &req_type,
                                    &imdb_id,
                                    cx,
                                );
                                if !matches!(shell.route, AppRoute::MetaDetails { .. }) {
                                    shell.core.load_meta_details(
                                        &req_type,
                                        &imdb_id,
                                        req_vid.as_deref(),
                                    );
                                }
                                cx.notify();
                            }
                        });
                        return;
                    }
                    if let Some(synthetic) = core
                        .fetch_synthetic_tmdb_meta(req_type, req_id.clone())
                        .await
                    {
                        let _ = this.update(cx, |shell, cx| {
                            if shell
                                .quick_peek
                                .as_ref()
                                .is_some_and(|p| p.id == req_id)
                            {
                                shell.synthetic_tmdb_meta = Some((req_id, synthetic));
                                cx.notify();
                            }
                        });
                    }
                })
                .detach();
            } else {
                let already_ready = self.core.with_model(|m| {
                    resolve_ready_meta_item(&m.meta_details.meta_items)
                        .is_some_and(|(item, _)| item.preview.id == target_id)
                });
                if !already_ready {
                    self.core
                        .load_meta_details(&target_type, &target_id, target_vid.as_deref());
                }
            }
        }

        cx.notify();
    }

    pub fn close_quick_peek(&mut self, cx: &mut Context<Self>) {
        if self.quick_peek.take().is_some() {
            if !matches!(self.route, AppRoute::MetaDetails { .. } | AppRoute::Player) {
                self.core.unload_meta_details();
            }
            cx.notify();
        }
    }

    pub fn cached_library_statuses(&self) -> Arc<HashMap<String, LibraryStatusTab>> {
        let rev = self.library_state_rev;
        {
            let guard = self.memoized_library_statuses.lock();
            if guard.0 == rev {
                return Arc::clone(&guard.1);
            }
        }
        let computed: HashMap<String, LibraryStatusTab> = self.core.with_model(|model| {
            model
                .ctx
                .library
                .items
                .iter()
                .filter_map(|(id, li)| {
                    classify_library_item_status(li).map(|st| (id.clone(), st))
                })
                .collect()
        });
        let arc = Arc::new(computed);
        let mut guard = self.memoized_library_statuses.lock();
        guard.0 = rev;
        guard.1 = Arc::clone(&arc);
        arc
    }

    pub fn cached_discover_snapshot(&self) -> DiscoverViewSnapshot {
        let rev = self.discover_state_rev;
        {
            let guard = self.memoized_discover_catalog.lock();
            if let Some((cached_rev, snap)) = guard.as_ref() {
                if *cached_rev == rev {
                    return snap.clone();
                }
            }
        }
        let snap = self.core.with_model(DiscoverViewSnapshot::from_model);
        let mut guard = self.memoized_discover_catalog.lock();
        *guard = Some((rev, snap.clone()));
        snap
    }

    pub fn toggle_pin_board_shelf(&mut self, shelf_key: String, cx: &mut Context<Self>) {
        if let Some(pos) = self
            .preferences
            .pinned_board_shelves
            .iter()
            .position(|k| k == &shelf_key)
        {
            self.preferences.pinned_board_shelves.remove(pos);
        } else {
            self.preferences
                .hidden_board_shelves
                .retain(|k| k != &shelf_key);
            self.preferences.pinned_board_shelves.push(shelf_key);
        }
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn toggle_hide_board_shelf(&mut self, shelf_key: String, cx: &mut Context<Self>) {
        if let Some(pos) = self
            .preferences
            .hidden_board_shelves
            .iter()
            .position(|k| k == &shelf_key)
        {
            self.preferences.hidden_board_shelves.remove(pos);
        } else {
            self.preferences
                .pinned_board_shelves
                .retain(|k| k != &shelf_key);
            self.preferences.hidden_board_shelves.push(shelf_key);
        }
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn restore_all_hidden_shelves(&mut self, cx: &mut Context<Self>) {
        self.preferences.hidden_board_shelves.clear();
        self.board_hidden_drawer_open = false;
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn on_card_hover_prefetch(
        &mut self,
        media_type: &str,
        id: &str,
        backdrop_url: Option<&str>,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        if !hovered {
            if self.hover_prefetch_target.as_deref() == Some(id) {
                self.hover_prefetch_target = None;
                self.hover_prefetch_seq = self.hover_prefetch_seq.wrapping_add(1);
            }
            return;
        }

        self.hover_prefetch_seq = self.hover_prefetch_seq.wrapping_add(1);
        let seq = self.hover_prefetch_seq;
        self.hover_prefetch_target = Some(id.to_owned());
        let media_type = media_type.to_owned();
        let id = id.to_owned();
        let backdrop_url = backdrop_url.map(str::to_owned);

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let _ = this.update(cx, |shell, cx| {
                if shell.hover_prefetch_seq != seq
                    || shell.hover_prefetch_target.as_deref() != Some(id.as_str())
                {
                    return;
                }
                let bg = backdrop_url.or_else(|| {
                    shell
                        .core
                        .cached_enriched_meta(&id)
                        .and_then(|e| e.backdrop_url)
                });
                if let Some(bg_url) = bg.as_deref() {
                    shell.poster_cache.prefetch_hero_backdrop(bg_url);
                }
                shell.request_background_enrichment_if_needed(&media_type, &id, cx);
            });
        })
        .detach();
    }

    pub fn request_collection_if_needed(&mut self, collection_id: u64, cx: &mut Context<Self>) {
        if self
            .active_collection
            .as_ref()
            .is_some_and(|(id, _)| *id == collection_id)
            || self.loading_collection_id == Some(collection_id)
        {
            return;
        }
        self.loading_collection_id = Some(collection_id);
        let core = self.core.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = core.fetch_tmdb_collection(collection_id).await;
            let _ = this.update(cx, |shell, cx| {
                if shell.loading_collection_id == Some(collection_id) {
                    shell.loading_collection_id = None;
                    if let Ok(collection) = res {
                        shell.active_collection = Some((collection_id, collection));
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn open_browsing_context_menu(
        &mut self,
        menu: BrowsingContextMenu,
        cx: &mut Context<Self>,
    ) {
        self.avatar_menu_open = false;
        self.command_palette_open = false;
        self.discover_open_dropdown = None;
        self.meta_addon_dropdown_open = false;
        self.library_sort_dropdown_open = false;
        self.settings_open_dropdown = None;
        self.context_menu = Some(menu);
        cx.notify();
    }

    pub fn dismiss_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    pub fn dismiss_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        let before = self.toasts.len();
        self.toasts.retain(|t| t.id != id);
        if self.toasts.len() != before {
            cx.notify();
        }
    }

    pub fn format_input_with_caret(&self, which: FocusedInput, text: &str) -> String {
        let focused = self.focused_input == Some(which);
        let cursor = self
            .input_cursors
            .get(&which)
            .copied()
            .unwrap_or(text.len());
        let selected_all = self.input_selected_all.contains(&which);
        format_text_with_caret(text, cursor, selected_all, focused)
    }

    pub fn set_kb_focus_items(&mut self, items: Vec<KbFocusableCard>) {
        self.kb_focus_items = items;
        if let Some(idx) = self.kb_focus_index {
            if self.kb_focus_items.is_empty() {
                self.kb_focus_index = None;
            } else if idx >= self.kb_focus_items.len() {
                self.kb_focus_index = Some(self.kb_focus_items.len() - 1);
            }
        }
    }

    pub fn clear_palette_query(&mut self, cx: &mut Context<Self>) {
        self.palette_query.clear();
        self.input_cursors.remove(&FocusedInput::CommandPalette);
        self.input_selected_all
            .remove(&FocusedInput::CommandPalette);
        self.palette_selected_idx = 0;
        self.palette_scroll_handle.scroll_to_item(0);
        self.search_debounce_seq = self.search_debounce_seq.wrapping_add(1);
        self.core.unload_search();
        cx.notify();
    }

    fn configure_and_subscribe_mpv(
        mpv: &gpui_mpv::MpvController,
        preferences: &AppPreferences,
        cx: &mut Context<Self>,
    ) {
        let _ = mpv.set_hdr_tone_mapping(preferences.tone_mapping.to_mpv());
        let _ = mpv.set_upscale_mode(preferences.upscale_mode.to_mpv());
        let _ = mpv.set_subtitle_style(preferences.to_mpv_subtitle_style());
        if preferences.default_subtitle_delay_ms != 0 {
            let _ = mpv.set_subtitle_delay(f64::from(preferences.default_subtitle_delay_ms) / 1000.0);
        }
        let exp = preferences.experimental_video;
        if exp != Default::default() {
            let _ = mpv.set_shader_preset(exp.shader_preset.to_mpv());
            let _ = mpv.set_deband(exp.to_deband_config());
            let _ = mpv.set_hdr_backend_options(exp.to_hdr_backend_options());
        }

        let mut watch_rx = mpv.subscribe();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while watch_rx.changed().await.is_ok() {
                let snap = watch_rx.borrow_and_update().clone();
                if this
                    .update(cx, |shell, cx| {
                        shell.on_mpv_snapshot_updated(snap, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let events_rx = mpv.subscribe_events();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Ok(ev) = events_rx.recv().await {
                if this
                    .update(cx, |shell, cx| {
                        shell.on_mpv_event(ev, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn ensure_mpv_initialized(&mut self, cx: &mut Context<Self>) -> bool {
        if self.mpv.is_some() {
            return true;
        }
        match gpui_mpv::MpvController::new() {
            Ok(ctrl) => {
                if !self.mpv_subscriptions_attached {
                    Self::configure_and_subscribe_mpv(&ctrl, &self.preferences, cx);
                    self.mpv_subscriptions_attached = true;
                }
                self.player_snapshot = ctrl.snapshot();
                self.mpv = Some(ctrl);
                true
            }
            Err(err) => {
                tracing::warn!(target: "ferrite::mpv", "lazy libmpv init failed: {err}");
                self.push_toast(
                    ToastKind::Warning,
                    "libmpv unavailable",
                    Some(format!("{err}")),
                    cx,
                );
                false
            }
        }
    }

    fn replace_hover_thumbnail(
        &mut self,
        next: Option<Arc<RenderImage>>,
        next_key: Option<(String, i64)>,
    ) {
        match (&self.hover_thumbnail_image, &next) {
            (Some(prev), Some(cur)) if Arc::ptr_eq(prev, cur) => {
                self.hover_thumbnail_key = next_key;
                return;
            }
            (Some(prev), _) => self.poster_cache.push_evicted_image(Arc::clone(prev)),
            (None, _) => {}
        }
        self.hover_thumbnail_image = next;
        self.hover_thumbnail_key = if self.hover_thumbnail_image.is_some() {
            next_key
        } else {
            None
        };
    }

    fn unload_meta_details_if_leaving(&mut self, keep_meta_loaded: bool) {
        if keep_meta_loaded {
            return;
        }
        if matches!(self.route, AppRoute::MetaDetails { .. })
            || self.memoized_meta_streams.is_some()
            || self.memoized_enriched_streams.is_some()
        {
            self.memoized_meta_streams = None;
            self.memoized_enriched_streams = None;
            self.auto_selected_episode_for_meta = None;
            self.visible_stream_limit = STREAM_PAGE_SIZE;
            self.expanded_stream_rows.clear();
            self.active_collection = None;
            self.loading_collection_id = None;
            self.core.unload_meta_details();
            self.schedule_debounced_heap_trim();
        }
    }

    pub fn has_debrid_access(&self) -> bool {
        self.preferences
            .debrid_access_mode
            .resolve_access(self.core.has_any_debrid_configured())
    }

    pub fn is_player_video_frame_ready(&self) -> bool {
        let snap = &self.player_snapshot;
        self.playback_session.as_ref().is_some_and(|s| {
            s.first_media_ready
                || (s.loaded_into_mpv && snap.video_width > 0 && snap.video_height > 0)
        })
    }

    pub fn live_player_transport_telemetry(&self) -> (f64, Option<u64>, bool) {
        let mpv_bps = self.player_snapshot.cache_speed_bytes.max(0) as f64;
        let torrent_key = self
            .playback_session
            .as_ref()
            .and_then(|s| s.active_candidate())
            .and_then(|c| c.torrent_key());
        if let Some((hash, fidx)) = torrent_key {
            if let Some(st) = self.engine.torrent_stats(&hash, fidx) {
                (st.download_speed.max(mpv_bps), Some(st.peers), true)
            } else {
                (mpv_bps, Some(0), true)
            }
        } else {
            (mpv_bps, None, false)
        }
    }

    fn ensure_player_telemetry_pulse(&mut self, cx: &mut Context<Self>) {
        let needed = self.route.is_player()
            && (self.player_stats_for_nerds || !self.is_player_video_frame_ready());
        self.player_telemetry_pulse_seq = self.player_telemetry_pulse_seq.wrapping_add(1);
        if !needed {
            return;
        }
        let seq = self.player_telemetry_pulse_seq;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep_going = this
                    .update(cx, |shell, cx| {
                        let active = shell.player_telemetry_pulse_seq == seq
                            && shell.route.is_player()
                            && (shell.player_stats_for_nerds
                                || !shell.is_player_video_frame_ready());
                        if active {
                            cx.notify();
                        }
                        active
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn osd_autohide_duration(&self) -> Duration {
        Duration::from_secs(u64::from(self.preferences.osd_autohide_secs.clamp(2, 5)))
    }

    fn schedule_osd_autohide_timer(&mut self, cx: &mut Context<Self>) {
        if !self.route.is_player() {
            return;
        }
        self.osd_hide_seq = self.osd_hide_seq.wrapping_add(1);
        let seq = self.osd_hide_seq;
        let timeout = self.osd_autohide_duration();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(timeout).await;
            let _ = this.update(cx, |shell, cx| {
                if shell.osd_hide_seq == seq
                    && shell.route.is_player()
                    && shell.is_player_video_frame_ready()
                    && !shell.player_snapshot.paused
                    && !shell.player_snapshot.buffering
                    && shell.player_popover.is_none()
                    && !shell.is_scrubbing
                    && !shell.is_dragging_volume
                    && shell.scrub_hover_fraction.is_none()
                {
                    shell.scrub_hover_metrics = None;
                    shell.replace_hover_thumbnail(None, None);
                    cx.hide_cursor();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn schedule_dead_swarm_watchdog(&self, cx: &mut Context<Self>) {
        if !self.preferences.dead_swarm_failover {
            return;
        }
        let Some(s) = &self.playback_session else {
            return;
        };
        if s.auto_failover_attempts >= MAX_AUTO_FAILOVER_ATTEMPTS
            || s.candidates.len() <= s.active_index + 1
        {
            return;
        }
        let gen_id = s.generation;
        let cand_idx = s.active_index;
        let torrent_info_hash = s.active_candidate().and_then(|c| match c.stream.source {
            StreamSource::Torrent { info_hash, .. } if !s.debrid_resolving => Some(info_hash),
            _ => None,
        });

        if let Some(ih) = torrent_info_hash {
            let initial_wait = self.preferences.dead_swarm_timeout_secs.max(18);
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let mut elapsed_secs = initial_wait;
                let mut wait_secs = initial_wait;
                let mut prev_fetched = 0_u64;
                let mut stalled_ticks = 0_u8;

                loop {
                    cx.background_executor()
                        .timer(Duration::from_secs(wait_secs))
                        .await;

                    let mut sleep_next: Option<u64> = None;
                    let update_res = this.update(cx, |shell, cx| {
                        let still_waiting = shell.route.is_player()
                            && shell.preferences.dead_swarm_failover
                            && shell.playback_session.as_ref().is_some_and(|cur| {
                                cur.generation == gen_id
                                    && cur.active_index == cand_idx
                                    && !cur.first_media_ready
                                    && cur.candidates.len() > cur.active_index + 1
                            });
                        if !still_waiting {
                            return;
                        }

                        let snapshot = shell.engine.torrent_progress_snapshot(&ih);
                        match evaluate_p2p_watchdog_tick(
                            snapshot,
                            elapsed_secs,
                            prev_fetched,
                            stalled_ticks,
                        ) {
                            P2pWatchdogDecision::Continue {
                                next_prev_fetched,
                                next_stalled_ticks,
                                sleep_secs,
                            } => {
                                prev_fetched = next_prev_fetched;
                                stalled_ticks = next_stalled_ticks;
                                elapsed_secs = elapsed_secs.saturating_add(sleep_secs);
                                sleep_next = Some(sleep_secs);
                            }
                            P2pWatchdogDecision::Failover { reason } => {
                                shell.trigger_dead_swarm_failover(&reason, cx);
                            }
                        }
                    });

                    if update_res.is_err() {
                        break;
                    }
                    match sleep_next {
                        Some(secs) => wait_secs = secs,
                        None => break,
                    }
                }
            })
            .detach();
        } else {
            let timeout_secs = self.preferences.dead_swarm_timeout_secs.max(6);
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                cx.background_executor()
                    .timer(Duration::from_secs(timeout_secs))
                    .await;
                let _ = this.update(cx, |shell, cx| {
                    let stalled = shell.route.is_player()
                        && shell.preferences.dead_swarm_failover
                        && shell.playback_session.as_ref().is_some_and(|cur| {
                            cur.generation == gen_id
                                && cur.active_index == cand_idx
                                && !cur.first_media_ready
                                && cur.candidates.len() > cur.active_index + 1
                        });
                    if stalled {
                        shell.trigger_dead_swarm_failover(
                            &format!("No video frames within {timeout_secs}s"),
                            cx,
                        );
                    }
                });
            })
            .detach();
        }
    }

    fn bump_state_revisions(&mut self, fields: &[DesktopModelField]) {
        for field in fields {
            match field {
                DesktopModelField::Board | DesktopModelField::ContinueWatchingPreview => {
                    self.board_state_rev = self.board_state_rev.wrapping_add(1);
                }
                DesktopModelField::Discover => {
                    self.discover_state_rev = self.discover_state_rev.wrapping_add(1);
                    self.discover_last_next_page_len.set(None);
                }
                DesktopModelField::Library | DesktopModelField::ContinueWatching => {
                    self.library_state_rev = self.library_state_rev.wrapping_add(1);
                    self.board_state_rev = self.board_state_rev.wrapping_add(1);
                }
                DesktopModelField::Calendar => {
                    self.calendar_state_rev = self.calendar_state_rev.wrapping_add(1);
                }
                DesktopModelField::MetaDetails => {
                    self.meta_state_rev = self.meta_state_rev.wrapping_add(1);
                }
                DesktopModelField::Search | DesktopModelField::LocalSearch => {
                    self.search_state_rev = self.search_state_rev.wrapping_add(1);
                }
                DesktopModelField::Ctx => {
                    self.board_state_rev = self.board_state_rev.wrapping_add(1);
                    self.discover_state_rev = self.discover_state_rev.wrapping_add(1);
                    self.discover_last_next_page_len.set(None);
                    self.library_state_rev = self.library_state_rev.wrapping_add(1);
                    self.calendar_state_rev = self.calendar_state_rev.wrapping_add(1);
                    self.meta_state_rev = self.meta_state_rev.wrapping_add(1);
                    self.search_state_rev = self.search_state_rev.wrapping_add(1);
                }
                DesktopModelField::InstalledAddons => {
                    self.board_state_rev = self.board_state_rev.wrapping_add(1);
                    self.discover_state_rev = self.discover_state_rev.wrapping_add(1);
                    self.discover_last_next_page_len.set(None);
                    self.meta_state_rev = self.meta_state_rev.wrapping_add(1);
                }
                _ => {}
            }
        }
    }

    fn handle_core_runtime_event(
        &mut self,
        ev: &crate::CoreRuntimeEventAlias,
        cx: &mut Context<Self>,
    ) {
        match ev {
            RuntimeEvent::NewState(fields) => {
                self.bump_state_revisions(fields);

                if fields.contains(&DesktopModelField::AuthLink) && self.link_poll_active {
                    let resolved_token = self.core.with_model(|m| match &m.auth_link.data {
                        Some(Loadable::Ready(key)) => Some(key.auth_key.clone()),
                        _ => None,
                    });
                    if let Some(token) = resolved_token {
                        self.link_poll_active = false;
                        self.core.login_with_token(token);
                        self.core
                            .dispatch(Action::Unload, Some(DesktopModelField::AuthLink));
                        self.push_toast(
                            ToastKind::Success,
                            "Link code approved",
                            Some("Syncing your Stremio profile".to_owned()),
                            cx,
                        );
                    }
                }

                if fields.contains(&DesktopModelField::MetaDetails) {
                    self.ensure_series_default_episode_streams(cx);
                    self.maybe_trigger_stream_enrichments(cx);
                    self.try_complete_pending_quick_resume(cx);
                }

                if (fields.contains(&DesktopModelField::Search)
                    || fields.contains(&DesktopModelField::LocalSearch))
                    && matches!(self.route, AppRoute::Search { .. })
                {
                    self.recompute_full_search_merged();
                }

                if fields.contains(&DesktopModelField::Player) {
                    self.sync_player_from_core_model(cx);
                }

                let player_osd_or_stats =
                    self.is_player_osd_visible() || self.player_stats_for_nerds;
                let quick_peek_affected = self.quick_peek.is_some()
                    && fields.iter().any(|f| {
                        matches!(
                            f,
                            DesktopModelField::MetaDetails
                                | DesktopModelField::Library
                                | DesktopModelField::Ctx
                        )
                    });
                if !quick_peek_affected
                    && !route_affected_by_fields(
                        &self.route,
                        self.command_palette_open,
                        player_osd_or_stats,
                        fields,
                    )
                {
                    return;
                }

                cx.notify();
            }
            RuntimeEvent::CoreEvent(core_ev) => match core_ev {
                CoreEvent::UserAuthenticated { auth_request: _ } => {
                    self.auth_password_input.clear();
                    self.input_cursors.remove(&FocusedInput::AuthPassword);
                    self.input_selected_all.remove(&FocusedInput::AuthPassword);
                    self.link_poll_active = false;
                    self.core
                        .sync_loopback_streaming_server_url(self.engine.base_url());
                    self.visible_board_shelves = INITIAL_BOARD_SHELVES;
                    self.core.load_board();
                    if self.discover_loaded_once {
                        self.core.load_discover(None);
                    }
                    self.core.load_library(default_library_selected());
                    self.push_toast(
                        ToastKind::Success,
                        "Signed in to Stremio",
                        Some("Addons and library synced".to_owned()),
                        cx,
                    );
                }
                CoreEvent::Error { error, source } => {
                    let (title, detail) = format_core_error_message(error, source);
                    self.push_toast(ToastKind::Error, title, detail, cx);
                }
                _ => {
                    cx.notify();
                }
            },
        }
    }

    fn load_url_into_active_session(&mut self, url_str: &str, cx: &mut Context<Self>) {
        let engine_base = self.engine.base_url();
        let url_str = rewrite_loopback_engine_url(url_str, &engine_base);
        let Some(session) = self.playback_session.as_mut() else {
            return;
        };
        let seamless_switch = session.seamless_switch_pending;
        let start_offset_secs = session.start_offset_secs;
        session.resolved_url = Some(url_str.clone());
        session.loaded_into_mpv = true;
        session.seamless_switch_pending = false;
        session.debrid_resolving = false;
        session.debrid_error = None;
        session.started_at = Instant::now();

        self.ensure_mpv_initialized(cx);
        if let Some(mpv) = &self.mpv {
            if seamless_switch {
                let resume_at = start_offset_secs.unwrap_or(0.0);
                let _ = mpv.switch_stream_seamless(&url_str, resume_at);
            } else {
                let _ = mpv.load_file(&url_str, start_offset_secs);
                if (self.playback_speed - 1.0).abs() > 0.01 {
                    let _ = mpv.set_speed(self.playback_speed);
                }
            }
        }
        self.sync_os_integrations();
        cx.notify();
    }

    fn collect_fallback_addon_subtitles(&self, cap: usize) -> Vec<(String, String, String)> {
        let preferred_sub_lang = self.normalized_preferred_sub_lang();
        self.core.with_model(|m| {
            let mut preferred_bucket = Vec::new();
            let mut other_bucket = Vec::new();
            let mut per_lang_counts: HashMap<String, usize> = HashMap::new();
            let mut seen_urls = HashSet::new();

            for sub_res in &m.player.subtitles {
                if let Some(Loadable::Ready(list)) = &sub_res.content {
                    for s in list {
                        let url_str = s.url.to_string();
                        if !seen_urls.insert(url_str.clone()) {
                            continue;
                        }
                        let lang_lower = s.lang.trim().to_ascii_lowercase();
                        let count = per_lang_counts.entry(lang_lower.clone()).or_insert(0);
                        if *count >= 2 {
                            continue;
                        }
                        *count += 1;

                        let human_lang = human_language_name(&s.lang);
                        let raw_label = s.label.as_deref().map(str::trim).unwrap_or("");
                        let display_title = if raw_label.is_empty()
                            || raw_label.eq_ignore_ascii_case(&s.lang)
                            || raw_label.eq_ignore_ascii_case(&human_lang)
                        {
                            format!("{human_lang} — Addon")
                        } else {
                            format!("{human_lang} — {raw_label}")
                        };
                        let entry = (url_str, display_title, s.lang.clone());
                        if subtitle_lang_matches(&s.lang, &preferred_sub_lang) {
                            preferred_bucket.push(entry);
                        } else {
                            other_bucket.push(entry);
                        }
                    }
                }
            }
            preferred_bucket.extend(other_bucket);
            preferred_bucket.truncate(cap);
            preferred_bucket
        })
    }

    fn attach_fallback_addon_subtitles_if_needed(&mut self, cx: &mut Context<Self>) {
        if !self.route.is_player() {
            return;
        }
        if self.has_usable_embedded_preferred_subtitle(&self.player_snapshot.tracks) {
            return;
        }
        let external_subs = self.collect_fallback_addon_subtitles(2);
        if external_subs.is_empty() {
            return;
        }
        let mut attached_any = false;
        if let Some(session) = self.playback_session.as_mut() {
            if session.loaded_into_mpv {
                if let Some(mpv) = &self.mpv {
                    for (sub_url, title_label, lang) in external_subs {
                        if session.attached_external_subs.insert(sub_url.clone()) {
                            let _ = mpv.add_subtitle(&sub_url, Some(&title_label), Some(&lang));
                            attached_any = true;
                        }
                    }
                }
            }
        }
        if attached_any {
            cx.notify();
        }
    }

    fn sync_player_from_core_model(&mut self, cx: &mut Context<Self>) {
        if !self.route.is_player() {
            return;
        }

        let resolved_stream_url = self.core.with_model(|m| {
            match &m.player.stream {
                Some(Loadable::Ready((urls, _))) => {
                    urls.streaming_url.as_ref().map(|u| u.to_string())
                }
                _ => None,
            }
        });

        let fallback_url_to_load = {
            let Some(session) = self.playback_session.as_ref() else {
                return;
            };
            if !session.loaded_into_mpv
                && !session.debrid_resolving
                && session.debrid_error.is_none()
            {
                let direct_fast_url = session.active_candidate().and_then(|c| {
                    self.core
                        .resolve_stream_playback_url(&c.stream)
                        .map(|u| u.to_string())
                });
                match session.active_candidate().map(|c| &c.stream.source) {
                    Some(StreamSource::Url { .. }) => direct_fast_url.or(resolved_stream_url),
                    _ => resolved_stream_url.or(direct_fast_url),
                }
            } else {
                None
            }
        };

        if let Some(url_str) = fallback_url_to_load {
            self.load_url_into_active_session(&url_str, cx);
        }

        if self.native_sub_match.file_loaded_ready
            && matches!(
                self.native_sub_match.status,
                NativeSubtitleMatchStatus::NoMatches | NativeSubtitleMatchStatus::Failed(_)
            )
        {
            self.attach_fallback_addon_subtitles_if_needed(cx);
        }
    }

    fn on_mpv_snapshot_updated(&mut self, snap: PlayerSnapshot, cx: &mut Context<Self>) {
        let prev_paused = self.player_snapshot.paused;
        let prev_buffering = self.player_snapshot.buffering;
        let prev_eof = self.player_snapshot.eof_reached;
        let prev_video_ready = self.is_player_video_frame_ready();

        let mut should_apply_series_prefs = false;
        if let Some(session) = self.playback_session.as_mut() {
            let decoded_frame_now = session.loaded_into_mpv
                && ((snap.video_width > 0 && snap.video_height > 0)
                    || (snap.time_pos > 0.5 && !snap.buffering));
            if decoded_frame_now {
                session.first_media_ready = true;
                session.auto_failover_attempts = 0;
                session.failover_exhausted_notified = false;
            }
            if !session.applied_series_track_prefs && !snap.tracks.is_empty() {
                session.applied_series_track_prefs = true;
                should_apply_series_prefs = true;
            }

            if self.route.is_player() {
                if session.last_reported_paused != Some(snap.paused) {
                    session.last_reported_paused = Some(snap.paused);
                    self.core.player_paused_changed(snap.paused);
                }

                if snap.duration > 0.0 && snap.time_pos >= 0.0 {
                    let pos_ms = (snap.time_pos * 1000.0) as u64;
                    let dur_ms = (snap.duration * 1000.0) as u64;
                    if pos_ms.abs_diff(session.last_reported_time_ms) >= 1000 {
                        session.last_reported_time_ms = pos_ms;
                        self.core
                            .player_time_changed(pos_ms, dur_ms, "ferrite");
                    }
                }

                if snap.eof_reached && !session.reported_ended {
                    session.reported_ended = true;
                    self.core.player_ended();
                }
            }
        }

        if should_apply_series_prefs {
            self.apply_saved_series_track_prefs(&snap);
        }

        if !snap.tracks.is_empty() {
            let pref_lang = self.active_preferred_sub_lang();
            let cands = Self::build_track_candidates(&snap.tracks);
            self.secondary_forced_available = select_embedded_forced_track(&cands, &pref_lang);
        } else {
            self.secondary_forced_available = None;
        }

        if let Some(pending_title) = self.pending_auto_select_sub_title.as_deref() {
            if let Some(track) = snap
                .tracks
                .iter()
                .find(|t| t.track_type == TrackType::Sub && t.title.as_deref() == Some(pending_title))
            {
                if let Some(mpv) = &self.mpv {
                    let _ = mpv.set_sub_forced_events_only(false);
                    let _ = mpv.select_subtitle_track(Some(track.id));
                }
                self.pending_auto_select_sub_title = None;
            }
        }

        if self.route.is_player()
            && !self.native_sub_match.deferred_p2p_sync_attempted
            && self.native_sub_match.file_loaded_ready
            && (snap.time_pos >= 35.0
                || (snap.time_pos >= 15.0 && snap.demuxer_cache_duration >= 25.0))
            && self.should_auto_audio_sync_now()
        {
            self.native_sub_match.deferred_p2p_sync_attempted = true;
            self.spawn_automatic_audio_subtitle_sync(false, cx);
        }

        let mut prewarm_changed = false;
        if self.route.is_player() && !self.next_episode_prewarmed {
            if let Some(target) = self
                .core
                .should_trigger_next_episode_prewarm(snap.time_pos, snap.duration)
            {
                let meta_streams_ready_for_target = self.core.with_model(|m| {
                    m.meta_details.meta_items.iter().any(|r| {
                        matches!(r.content, Some(Loadable::Ready(_)))
                    }) && m.meta_details.streams.iter().any(|r| {
                        r.request.path.id == target.video_id
                            && matches!(r.content, Some(Loadable::Ready(_)))
                    })
                });
                if self.prewarm_fetch_requested_vid.as_deref() != Some(target.video_id.as_str())
                    || meta_streams_ready_for_target
                {
                    self.prewarm_fetch_requested_vid = Some(target.video_id);
                    prewarm_changed = self.maybe_prewarm_next_episode(cx);
                }
            }
        }

        let should_auto_play_next = self.route.is_player()
            && !prev_eof
            && snap.eof_reached
            && snap.duration > 30.0
            && snap.time_pos >= snap.duration * 0.90
            && self.preferences.auto_play_next_episode
            && self.prewarmed_next.is_some();

        let (intro_visible, next_ep_visible) = if self.route.is_player() {
            self.core.with_model(|m| {
                let in_intro = m
                    .player
                    .intro_outro
                    .as_ref()
                    .and_then(|io| io.intro.as_ref())
                    .is_some_and(|seg| {
                        let from_s = (seg.from as f64) / 1000.0;
                        let to_s = (seg.to as f64) / 1000.0;
                        to_s > from_s + 2.0 && snap.time_pos >= from_s && snap.time_pos + 1.0 < to_s
                    });
                let near_end = m.player.next_video.is_some()
                    && snap.duration > 60.0
                    && (snap.duration - snap.time_pos <= 48.0 || snap.eof_reached);
                (in_intro, near_end)
            })
        } else {
            (false, false)
        };

        let prompt_flipped = intro_visible != self.last_intro_visible
            || next_ep_visible != self.last_next_ep_visible;
        self.last_intro_visible = intro_visible;
        self.last_next_ep_visible = next_ep_visible;

        self.player_snapshot = snap;
        let now_video_ready = self.is_player_video_frame_ready();
        self.sync_os_integrations();

        if should_auto_play_next {
            if let Some(next_vid) = self.prewarmed_next.as_ref().map(|r| r.next_video.clone()) {
                self.advance_to_next_episode(next_vid, cx);
                return;
            }
        }

        let state_flipped = prev_paused != self.player_snapshot.paused
            || prev_buffering != self.player_snapshot.buffering
            || prev_eof != self.player_snapshot.eof_reached
            || prev_video_ready != now_video_ready
            || prewarm_changed;

        if self.route.is_player()
            && ((prev_paused && !self.player_snapshot.paused)
                || (!prev_video_ready && now_video_ready))
        {
            self.last_osd_activity = Instant::now();
            self.schedule_osd_autohide_timer(cx);
        }

        if !self.route.is_player()
            || !now_video_ready
            || self.is_player_osd_visible()
            || self.player_stats_for_nerds
            || self.hold_2x_active
            || state_flipped
            || prompt_flipped
        {
            cx.notify();
        }
    }

    fn on_mpv_event(&mut self, ev: PlayerEvent, cx: &mut Context<Self>) {
        if !self.route.is_player() {
            return;
        }
        match ev {
            PlayerEvent::FileLoaded => {
                if let Some(mpv) = &self.mpv {
                    let fresh_snap = mpv.snapshot();
                    self.player_snapshot = fresh_snap.clone();
                    let should_apply_prefs = self
                        .playback_session
                        .as_mut()
                        .is_some_and(|session| {
                            if !session.applied_series_track_prefs && !fresh_snap.tracks.is_empty() {
                                session.applied_series_track_prefs = true;
                                true
                            } else {
                                false
                            }
                        });
                    if should_apply_prefs {
                        self.apply_saved_series_track_prefs(&fresh_snap);
                    }
                }
                self.native_sub_match.file_loaded_ready = true;
                self.evaluate_automatic_subtitles_on_file_loaded(cx);
                if self.should_auto_audio_sync_now() {
                    self.spawn_automatic_audio_subtitle_sync(false, cx);
                }
            }
            PlayerEvent::UntranslatedForeignSubtitleDetected(_) => {
                self.handle_untranslated_foreign_subtitle_detected(cx);
            }
            PlayerEvent::EndFile {
                error: Some(err_msg),
                ..
            } if self.preferences.dead_swarm_failover => {
                self.trigger_dead_swarm_failover(&format!("Stream error ({err_msg})"), cx);
            }
            _ => {}
        }
    }

    fn active_series_id(&self) -> Option<String> {
        self.core.active_player_series_id().or_else(|| {
            let s = self.playback_session.as_ref()?;
            if s.type_name.eq_ignore_ascii_case("series")
                || s.type_name.eq_ignore_ascii_case("anime")
                || s.video_id.contains(':')
            {
                Some(s.meta_id.clone())
            } else {
                None
            }
        })
    }

    fn build_track_candidates(tracks: &[gpui_mpv::MediaTrack]) -> Vec<TrackCandidate> {
        tracks
            .iter()
            .filter_map(|t| match t.track_type {
                TrackType::Audio => {
                    let mut cand =
                        TrackCandidate::audio(t.id, t.lang.as_deref(), t.title.as_deref())
                            .with_external(t.external)
                            .with_forced(t.forced)
                            .with_default(t.default);
                    if let Some(codec) = &t.codec {
                        cand = cand.with_codec(codec.clone());
                    }
                    Some(cand)
                }
                TrackType::Sub => {
                    let mut cand =
                        TrackCandidate::sub(t.id, t.lang.as_deref(), t.title.as_deref())
                            .with_external(t.external)
                            .with_forced(t.forced)
                            .with_hearing_impaired(t.hearing_impaired)
                            .with_default(t.default);
                    if let Some(codec) = &t.codec {
                        cand = cand.with_codec(codec.clone());
                    }
                    Some(cand)
                }
                TrackType::Video => None,
            })
            .collect()
    }

    fn apply_saved_series_track_prefs(&mut self, snap: &PlayerSnapshot) {
        let Some(mpv) = &self.mpv else {
            return;
        };

        let series_pref = self
            .active_series_id()
            .and_then(|series_id| self.core.get_series_track_pref(&series_id));

        let pref = if let Some(saved) = series_pref {
            saved
        } else {
            let mut global_pref = SeriesTrackPreference::new("global-default");
            let audio = self.preferences.preferred_audio_lang.trim();
            if !audio.is_empty() && !audio.eq_ignore_ascii_case("auto") {
                global_pref.audio_lang = Some(audio.to_owned());
            }
            let sub = self.preferences.preferred_subtitle_lang.trim();
            global_pref.subtitles_enabled = self.preferences.auto_select_subtitles
                && !sub.eq_ignore_ascii_case("off")
                && !sub.eq_ignore_ascii_case("none");
            if global_pref.subtitles_enabled {
                global_pref.sub_lang = Some(self.normalized_preferred_sub_lang());
            }
            global_pref.sub_delay_secs =
                f64::from(self.preferences.default_subtitle_delay_ms) / 1000.0;
            global_pref
        };

        let candidates = Self::build_track_candidates(&snap.tracks);
        let matched = select_preferred_tracks(&pref, &candidates);
        if let Some(aid) = matched.audio_id {
            let _ = mpv.select_audio_track(aid);
        }
        if let Some(sub_choice) = matched.sub_id {
            let _ = mpv.set_sub_forced_events_only(false);
            let _ = mpv.select_subtitle_track(sub_choice);
        }
        if pref.sub_delay_secs.abs() > 0.005 {
            let _ = mpv.set_subtitle_delay(pref.sub_delay_secs);
        }
    }

    fn persist_series_track_selection(
        &self,
        override_audio_id: Option<i64>,
        override_sub_choice: Option<Option<i64>>,
        override_sub_delay: Option<f64>,
    ) {
        let Some(series_id) = self.active_series_id() else {
            return;
        };
        let candidates = Self::build_track_candidates(&self.player_snapshot.tracks);

        let selected_audio_id = override_audio_id.or_else(|| {
            self.player_snapshot
                .tracks
                .iter()
                .find(|t| t.track_type == TrackType::Audio && t.selected)
                .map(|t| t.id)
        });
        let selected_sub_id = match override_sub_choice {
            Some(opt) => opt,
            None => self
                .player_snapshot
                .tracks
                .iter()
                .find(|t| t.track_type == TrackType::Sub && t.selected)
                .map(|t| t.id),
        };

        let audio_cand = selected_audio_id.and_then(|id| {
            candidates
                .iter()
                .find(|c| c.id == id && c.kind == stremio_core_runtime::TrackKind::Audio)
        });
        let sub_cand = selected_sub_id.and_then(|id| {
            candidates
                .iter()
                .find(|c| c.id == id && c.kind == stremio_core_runtime::TrackKind::Sub)
        });
        let delay = override_sub_delay.unwrap_or(self.player_snapshot.sub_delay);

        let pref = SeriesTrackPreference::from_selected_tracks(
            series_id,
            audio_cand,
            sub_cand,
            selected_sub_id.is_some(),
            delay,
        );
        self.core.save_series_track_pref(pref);
    }

    pub(crate) fn current_route_scroll_y(&self) -> f32 {
        match &self.route {
            AppRoute::Tab(SidebarTab::Home) => {
                (-self.board_scroll_handle.offset().y / px(1.0)).max(0.0)
            }
            AppRoute::Tab(SidebarTab::Discover) => match self.preferences.discover_view_mode {
                DiscoverViewMode::BrowseHubs => {
                    (-self.discover_hubs_scroll_handle.offset().y / px(1.0)).max(0.0)
                }
                DiscoverViewMode::CatalogGrid => (-self
                    .discover_grid_scroll_handle
                    .0
                    .borrow()
                    .base_handle
                    .offset()
                    .y
                    / px(1.0))
                .max(0.0),
            },
            AppRoute::Tab(SidebarTab::Calendar) => {
                (-self.calendar_scroll_handle.offset().y / px(1.0)).max(0.0)
            }
            AppRoute::Tab(SidebarTab::Library) => match self.preferences.library_view_mode {
                LibraryViewMode::PosterGrid => (-self
                    .library_grid_scroll_handle
                    .0
                    .borrow()
                    .base_handle
                    .offset()
                    .y
                    / px(1.0))
                .max(0.0),
                LibraryViewMode::WideBackdrops | LibraryViewMode::CompactList => {
                    let grid_y = (-self
                        .library_grid_scroll_handle
                        .0
                        .borrow()
                        .base_handle
                        .offset()
                        .y
                        / px(1.0))
                    .max(0.0);
                    let list_y = (-self.library_scroll_handle.offset().y / px(1.0)).max(0.0);
                    grid_y.max(list_y)
                }
            },
            AppRoute::Tab(SidebarTab::Settings) => {
                (-self.settings_scroll_handle.offset().y / px(1.0)).max(0.0)
            }
            AppRoute::MetaDetails { .. } => {
                (-self.meta_scroll_handle.offset().y / px(1.0)).max(0.0)
            }
            AppRoute::Search { .. } => (-self.search_scroll_handle.offset().y / px(1.0)).max(0.0),
            AppRoute::Person { .. } => (-self.person_scroll_handle.offset().y / px(1.0)).max(0.0),
            AppRoute::Tab(SidebarTab::Addons) => {
                (-self.addons_scroll_handle.offset().y / px(1.0)).max(0.0)
            }
            AppRoute::Tab(SidebarTab::Account) => {
                (-self.auth_scroll_handle.offset().y / px(1.0)).max(0.0)
            }
            AppRoute::Player => 0.0,
        }
    }

    fn restore_route_scroll_y(&self, route: &AppRoute, scroll_y: f32) {
        let off = point(px(0.0), px(-scroll_y.max(0.0)));
        match route {
            AppRoute::Tab(SidebarTab::Home) => self.board_scroll_handle.set_offset(off),
            AppRoute::Tab(SidebarTab::Discover) => match self.preferences.discover_view_mode {
                DiscoverViewMode::BrowseHubs => self.discover_hubs_scroll_handle.set_offset(off),
                DiscoverViewMode::CatalogGrid => self
                    .discover_grid_scroll_handle
                    .0
                    .borrow()
                    .base_handle
                    .set_offset(off),
            },
            AppRoute::Tab(SidebarTab::Calendar) => self.calendar_scroll_handle.set_offset(off),
            AppRoute::Tab(SidebarTab::Library) => {
                self.library_scroll_handle.set_offset(off);
                self.library_grid_scroll_handle
                    .0
                    .borrow()
                    .base_handle
                    .set_offset(off);
            }
            AppRoute::Tab(SidebarTab::Settings) => self.settings_scroll_handle.set_offset(off),
            AppRoute::MetaDetails { .. } => self.meta_scroll_handle.set_offset(off),
            AppRoute::Search { .. } => self.search_scroll_handle.set_offset(off),
            AppRoute::Person { .. } => self.person_scroll_handle.set_offset(off),
            AppRoute::Tab(SidebarTab::Addons) => self.addons_scroll_handle.set_offset(off),
            AppRoute::Tab(SidebarTab::Account) => self.auth_scroll_handle.set_offset(off),
            AppRoute::Player => {}
        }
    }

    fn capture_nav_entry(&self) -> NavHistoryEntry {
        NavHistoryEntry {
            route: self.route.clone(),
            selected_season: self.selected_season,
            scroll_y: self.current_route_scroll_y(),
        }
    }

    fn push_nav_history_entry(&mut self, entry: NavHistoryEntry) {
        if self.nav_history.len() >= MAX_NAV_HISTORY {
            let excess = self.nav_history.len() - MAX_NAV_HISTORY + 1;
            self.nav_history.drain(..excess);
        }
        self.nav_history.push(entry);
    }

    fn push_forward_history_entry(&mut self, entry: NavHistoryEntry) {
        if self.nav_forward.len() >= MAX_NAV_HISTORY {
            let excess = self.nav_forward.len() - MAX_NAV_HISTORY + 1;
            self.nav_forward.drain(..excess);
        }
        self.nav_forward.push(entry);
    }

    fn clear_transient_browsing_state(&mut self) {
        self.focused_input = None;
        self.command_palette_open = false;
        self.avatar_menu_open = false;
        self.context_menu = None;
        self.quick_peek = None;
        self.discover_open_dropdown = None;
        self.meta_addon_dropdown_open = false;
        self.library_sort_dropdown_open = false;
        self.board_hidden_drawer_open = false;
        self.settings_open_dropdown = None;
        self.pending_quick_resume = None;
        self.kb_focus_index = None;
        self.kb_focus_items.clear();
    }

    fn apply_nav_entry(&mut self, entry: NavHistoryEntry, cx: &mut Context<Self>) {
        let route_changed = self.route != entry.route;
        let keep_meta = matches!(
            entry.route,
            AppRoute::MetaDetails { .. } | AppRoute::Player
        );
        self.unload_meta_details_if_leaving(keep_meta);
        self.clear_transient_browsing_state();
        if route_changed {
            self.on_major_route_transition(entry.route.is_player());
        }

        self.route = entry.route.clone();
        self.selected_season = entry.selected_season;

        match entry.route.clone() {
            AppRoute::Tab(SidebarTab::Home) => {}
            AppRoute::Tab(SidebarTab::Discover) => {
                if !self.discover_loaded_once {
                    self.discover_loaded_once = true;
                    self.core.load_discover(None);
                }
            }
            AppRoute::Tab(SidebarTab::Calendar) => {
                self.core.load_calendar(None);
            }
            AppRoute::Tab(SidebarTab::Addons) => {
                self.core.load_installed_addons(None);
                self.core.load_remote_addons(None);
            }
            AppRoute::Tab(SidebarTab::Settings) => {
                self.core.reload_streaming_server();
            }
            AppRoute::Tab(SidebarTab::Library | SidebarTab::Account) => {}
            AppRoute::MetaDetails {
                type_name,
                id,
                video_id,
            } => {
                let already_loaded = self.core.with_model(|m| {
                    resolve_ready_meta_item(&m.meta_details.meta_items)
                        .is_some_and(|(item, _)| item.preview.id == id)
                });
                if !already_loaded {
                    self.memoized_meta_streams = None;
                    self.memoized_enriched_streams = None;
                    self.synthetic_tmdb_meta = None;
                    self.core
                        .load_meta_details(&type_name, &id, video_id.as_deref());
                }
                self.request_background_enrichment_if_needed(&type_name, &id, cx);
                self.maybe_trigger_stream_enrichments(cx);
            }
            AppRoute::Search { query } => {
                if self.full_search_query != query {
                    self.full_search_query = query;
                    self.refresh_full_search(cx);
                }
            }
            AppRoute::Person { person_key, name } => {
                if self
                    .active_person_filmography
                    .as_ref()
                    .is_none_or(|f| f.person_key != person_key)
                {
                    self.person_request_seq = self.person_request_seq.wrapping_add(1);
                    let seq = self.person_request_seq;
                    self.person_loading = true;
                    self.active_person_filmography = None;
                    let core = self.core.clone();
                    cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                        let filmo = core.fetch_person_filmography(person_key, name).await;
                        let _ = this.update(cx, |shell, cx| {
                            if shell.person_request_seq == seq {
                                shell.person_loading = false;
                                shell.active_person_filmography = Some(filmo);
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                }
            }
            AppRoute::Player => {}
        }

        self.restore_route_scroll_y(&entry.route, entry.scroll_y);
        cx.notify();
    }

    pub fn open_route(&mut self, route: AppRoute, cx: &mut Context<Self>) {
        match route {
            AppRoute::Tab(tab) => self.navigate_tab(tab, cx),
            AppRoute::MetaDetails {
                type_name,
                id,
                video_id,
            } => self.open_editorial_media_hit(type_name, id, video_id, cx),
            AppRoute::Search { query } => self.open_full_search(query, cx),
            AppRoute::Person { person_key, name } => self.open_person_details(person_key, name, cx),
            AppRoute::Player => {}
        }
    }

    pub fn navigate_tab(&mut self, tab: SidebarTab, cx: &mut Context<Self>) {
        let route_changed = self.route != AppRoute::Tab(tab);
        if self.route.is_player() {
            self.teardown_active_playback();
        } else if route_changed {
            let entry = self.capture_nav_entry();
            self.push_nav_history_entry(entry);
            self.nav_forward.clear();
        }

        if self.command_palette_open {
            self.core.unload_search();
        }
        self.unload_meta_details_if_leaving(false);
        self.clear_transient_browsing_state();
        if route_changed {
            self.on_major_route_transition(false);
        }

        match tab {
            SidebarTab::Home => {
                self.visible_board_shelves = INITIAL_BOARD_SHELVES;
                self.board_scroll_handle.set_offset(point(px(0.0), px(0.0)));
                self.core.load_board();
            }
            SidebarTab::Discover => {
                self.discover_loaded_once = true;
                self.discover_last_next_page_len.set(None);
                self.core.load_discover(None);
            }
            SidebarTab::Calendar => {
                self.calendar_scroll_handle
                    .set_offset(point(px(0.0), px(0.0)));
                self.core.load_calendar(None);
            }
            SidebarTab::Library => {
                self.visible_library_limit = LIBRARY_PAGE_SIZE;
                self.library_scroll_handle
                    .set_offset(point(px(0.0), px(0.0)));
                self.library_grid_scroll_handle
                    .0
                    .borrow()
                    .base_handle
                    .set_offset(point(px(0.0), px(0.0)));
                self.core.load_library(default_library_selected());
            }
            SidebarTab::Addons => {
                self.core.load_installed_addons(None);
                self.core.load_remote_addons(None);
                self.refresh_clipboard_addon_suggestion(cx);
            }
            SidebarTab::Settings => self.core.reload_streaming_server(),
            SidebarTab::Account => {}
        }

        self.route = AppRoute::Tab(tab);
        cx.notify();
    }

    pub fn open_discover_catalog(
        &mut self,
        selected: CatalogWithFiltersSelected,
        cx: &mut Context<Self>,
    ) {
        if self.route.is_player() {
            self.teardown_active_playback();
        } else {
            let entry = self.capture_nav_entry();
            self.push_nav_history_entry(entry);
            self.nav_forward.clear();
        }
        self.unload_meta_details_if_leaving(false);
        self.clear_transient_browsing_state();
        self.on_major_route_transition(false);
        self.discover_loaded_once = true;
        self.discover_last_next_page_len.set(None);
        self.discover_hubs_scroll_handle
            .set_offset(point(px(0.0), px(0.0)));
        self.discover_grid_scroll_handle
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(0.0)));
        self.route = AppRoute::Tab(SidebarTab::Discover);
        self.core.load_discover(Some(selected));
        cx.notify();
    }

    pub fn open_discover_genre(
        &mut self,
        type_name: &str,
        genre: &str,
        cx: &mut Context<Self>,
    ) {
        let req_type = type_name.trim();
        let req_genre = genre.trim();
        let request = self.core.with_model(|model| {
            let mut matched_exact: Option<ResourceRequest> = None;
            let mut matched_type_with_genre: Option<ResourceRequest> = None;
            let mut matched_type_any: Option<ResourceRequest> = None;

            for addon in &model.ctx.profile.addons {
                for mc in &addon.manifest.catalogs {
                    if !mc.r#type.eq_ignore_ascii_case(req_type) {
                        continue;
                    }
                    let Some(base_extra) = mc.default_required_extra() else {
                        continue;
                    };
                    if let Some(genre_prop) =
                        mc.extra.iter().find(|e| e.name.eq_ignore_ascii_case("genre"))
                    {
                        if let Some(opt_val) = genre_prop
                            .options
                            .iter()
                            .find(|o| o.eq_ignore_ascii_case(req_genre))
                        {
                            let mut extra = base_extra;
                            extra.retain(|ev| !ev.name.eq_ignore_ascii_case("genre"));
                            extra.push(ExtraValue {
                                name: genre_prop.name.clone(),
                                value: opt_val.clone(),
                            });
                            matched_exact = Some(ResourceRequest {
                                base: addon.transport_url.clone(),
                                path: ResourcePath {
                                    resource: "catalog".to_owned(),
                                    r#type: mc.r#type.clone(),
                                    id: mc.id.clone(),
                                    extra,
                                },
                            });
                            break;
                        } else if matched_type_with_genre.is_none() {
                            let mut extra = base_extra;
                            extra.retain(|ev| !ev.name.eq_ignore_ascii_case("genre"));
                            extra.push(ExtraValue {
                                name: genre_prop.name.clone(),
                                value: req_genre.to_owned(),
                            });
                            matched_type_with_genre = Some(ResourceRequest {
                                base: addon.transport_url.clone(),
                                path: ResourcePath {
                                    resource: "catalog".to_owned(),
                                    r#type: mc.r#type.clone(),
                                    id: mc.id.clone(),
                                    extra,
                                },
                            });
                        }
                    } else if matched_type_any.is_none() {
                        let mut extra = base_extra;
                        extra.push(ExtraValue {
                            name: "genre".to_owned(),
                            value: req_genre.to_owned(),
                        });
                        matched_type_any = Some(ResourceRequest {
                            base: addon.transport_url.clone(),
                            path: ResourcePath {
                                resource: "catalog".to_owned(),
                                r#type: mc.r#type.clone(),
                                id: mc.id.clone(),
                                extra,
                            },
                        });
                    }
                }
                if matched_exact.is_some() {
                    break;
                }
            }

            matched_exact
                .or(matched_type_with_genre)
                .or(matched_type_any)
                .unwrap_or_else(|| ResourceRequest {
                    base: CINEMETA_URL.clone(),
                    path: ResourcePath {
                        resource: "catalog".to_owned(),
                        r#type: if req_type.is_empty() {
                            "movie".to_owned()
                        } else {
                            req_type.to_owned()
                        },
                        id: "top".to_owned(),
                        extra: vec![ExtraValue {
                            name: "genre".to_owned(),
                            value: req_genre.to_owned(),
                        }],
                    },
                })
        });

        self.open_discover_catalog(CatalogWithFiltersSelected { request }, cx);
    }

    pub fn open_meta_details(
        &mut self,
        type_name: String,
        id: String,
        video_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.route.is_player() {
            self.teardown_active_playback();
        } else {
            let entry = self.capture_nav_entry();
            self.push_nav_history_entry(entry);
            self.nav_forward.clear();
        }
        let resolved_video_id = video_id
            .filter(|v| !v.trim().is_empty())
            .or_else(|| {
                self.core.with_model(|m| {
                    m.ctx
                        .library
                        .items
                        .get(&id)
                        .and_then(|li| li.state.video_id.clone())
                        .filter(|v| !v.trim().is_empty())
                })
            });

        self.clear_transient_browsing_state();
        self.on_major_route_transition(false);
        self.selected_season = None;
        self.auto_selected_episode_for_meta = resolved_video_id
            .as_ref()
            .map(|vid| (id.clone(), vid.clone()));
        self.stream_filter.reset();
        self.memoized_meta_streams = None;
        self.memoized_enriched_streams = None;
        self.synthetic_tmdb_meta = None;
        self.active_collection = None;
        self.loading_collection_id = None;
        self.visible_stream_limit = STREAM_PAGE_SIZE;
        self.expanded_stream_rows.clear();
        self.meta_scroll_handle.set_offset(point(px(0.0), px(0.0)));
        self.core
            .load_meta_details(&type_name, &id, resolved_video_id.as_deref());
        self.route = AppRoute::MetaDetails {
            type_name: type_name.clone(),
            id: id.clone(),
            video_id: resolved_video_id,
        };
        self.request_background_enrichment_if_needed(&type_name, &id, cx);
        self.ensure_series_default_episode_streams(cx);
        self.maybe_trigger_stream_enrichments(cx);
        cx.notify();
    }

    pub fn open_full_search(&mut self, query: String, cx: &mut Context<Self>) {
        let entering_search = !matches!(self.route, AppRoute::Search { .. });
        if self.route.is_player() {
            self.teardown_active_playback();
        } else if entering_search {
            let entry = self.capture_nav_entry();
            self.push_nav_history_entry(entry);
            self.nav_forward.clear();
        }
        self.clear_transient_browsing_state();
        if entering_search {
            self.on_major_route_transition(false);
        }

        let trimmed = query.trim().to_owned();
        self.full_search_query = trimmed.clone();
        self.palette_query = trimmed.clone();
        self.input_cursors
            .insert(FocusedInput::FullSearchInput, self.full_search_query.len());
        self.input_selected_all
            .remove(&FocusedInput::FullSearchInput);
        self.route = AppRoute::Search { query: trimmed };
        self.focused_input = Some(FocusedInput::FullSearchInput);
        self.search_scroll_handle.set_offset(point(px(0.0), px(0.0)));
        self.refresh_full_search(cx);
    }

    pub fn refresh_full_search(&mut self, cx: &mut Context<Self>) {
        self.search_debounce_seq = self.search_debounce_seq.wrapping_add(1);
        self.refresh_full_search_with_debounce(false, cx);
    }

    pub fn refresh_full_search_debounced(&mut self, cx: &mut Context<Self>) {
        self.refresh_full_search_with_debounce(true, cx);
    }

    fn refresh_full_search_with_debounce(&mut self, debounce: bool, cx: &mut Context<Self>) {
        self.full_search_seq = self.full_search_seq.wrapping_add(1);
        let seq = self.full_search_seq;
        self.search_debounce_seq = self.search_debounce_seq.wrapping_add(1);
        let debounce_seq = self.search_debounce_seq;

        let q = self.full_search_query.trim().to_owned();
        self.route = AppRoute::Search { query: q.clone() };
        self.full_search_tmdb_hits.clear();
        self.full_search_tmdb_people.clear();

        if q.is_empty() {
            self.full_search_results = UnifiedEditorialSearch::default();
            self.full_search_loading_tmdb = false;
            self.core.unload_search();
            cx.notify();
            return;
        }

        // Synchronous 0ms local FTS5 snapshot
        self.full_search_results = self.core.editorial_search_snapshot(&q);
        let has_tmdb = {
            let cfg = self.core.private_integrations();
            cfg.tmdb_enabled && cfg.has_tmdb_configured()
        };
        self.full_search_loading_tmdb = has_tmdb;
        if let Some(top) = self.full_search_results.top_match.clone() {
            self.request_background_enrichment_if_needed(&top.media_type, &top.canonical_id, cx);
        }
        cx.notify();

        if debounce {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                cx.background_executor()
                    .timer(Duration::from_millis(180))
                    .await;
                let _ = this.update(cx, |shell, cx| {
                    if shell.search_debounce_seq != debounce_seq || shell.full_search_seq != seq {
                        return;
                    }
                    shell.dispatch_network_search_and_tmdb(q, seq, has_tmdb, cx);
                });
            })
            .detach();
        } else {
            self.dispatch_network_search_and_tmdb(q, seq, has_tmdb, cx);
        }
    }

    fn dispatch_network_search_and_tmdb(
        &mut self,
        q: String,
        seq: u64,
        has_tmdb: bool,
        cx: &mut Context<Self>,
    ) {
        self.core.search(q.clone());
        if has_tmdb {
            let core = self.core.clone();
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let res = core.editorial_search_live_tmdb(q).await;
                let _ = this.update(cx, |shell, cx| {
                    if shell.full_search_seq != seq {
                        return;
                    }
                    shell.full_search_loading_tmdb = false;
                    if let Ok((tmdb_hits, tmdb_people)) = res {
                        shell.full_search_tmdb_hits = tmdb_hits;
                        shell.full_search_tmdb_people = tmdb_people;
                        shell.recompute_full_search_merged();
                        if let Some(top) = shell.full_search_results.top_match.clone() {
                            shell.request_background_enrichment_if_needed(
                                &top.media_type,
                                &top.canonical_id,
                                cx,
                            );
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    pub fn recompute_full_search_merged(&mut self) {
        let snap = self.core.editorial_search_snapshot(&self.full_search_query);
        if self.full_search_tmdb_hits.is_empty() && self.full_search_tmdb_people.is_empty() {
            self.full_search_results = snap;
            return;
        }
        let local_hits: Vec<EditorialMediaHit> = snap
            .movies
            .into_iter()
            .chain(snap.series)
            .collect();
        let merged_people: Vec<EditorialPersonHit> = self
            .full_search_tmdb_people
            .iter()
            .cloned()
            .chain(snap.people)
            .collect();
        self.full_search_results = build_unified_editorial_search(
            &self.full_search_query,
            local_hits,
            self.full_search_tmdb_hits.clone(),
            Vec::new(),
            merged_people,
            snap.elapsed_us,
        );
    }

    pub fn open_person_details(
        &mut self,
        person_key: String,
        name: String,
        cx: &mut Context<Self>,
    ) {
        if self.route.is_player() {
            self.teardown_active_playback();
        } else {
            let entry = self.capture_nav_entry();
            self.push_nav_history_entry(entry);
            self.nav_forward.clear();
        }
        self.clear_transient_browsing_state();
        self.on_major_route_transition(false);

        self.person_request_seq = self.person_request_seq.wrapping_add(1);
        let seq = self.person_request_seq;
        self.person_loading = true;
        self.active_person_filmography = None;
        self.person_bio_expanded = false;
        self.person_department_filter = PersonDeptFilter::All;
        self.person_sort_recent = true;
        self.person_scroll_handle.set_offset(point(px(0.0), px(0.0)));
        self.route = AppRoute::Person {
            person_key: person_key.clone(),
            name: name.clone(),
        };
        cx.notify();

        let core = self.core.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let filmo = core.fetch_person_filmography(person_key, name).await;
            let _ = this.update(cx, |shell, cx| {
                if shell.person_request_seq == seq {
                    shell.person_loading = false;
                    shell.active_person_filmography = Some(filmo);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn open_editorial_media_hit(
        &mut self,
        type_name: String,
        id: String,
        video_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !id.starts_with("tmdb:") {
            self.open_meta_details(type_name, id, video_id, cx);
            return;
        }

        self.open_meta_details(type_name.clone(), id.clone(), video_id.clone(), cx);
        let core = self.core.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if let Some(imdb_id) = core
                .resolve_canonical_imdb_id(type_name.clone(), id.clone())
                .await
            {
                if imdb_id != id {
                    let _ = this.update(cx, |shell, cx| {
                        let still_on_tmdb_route = matches!(
                            &shell.route,
                            AppRoute::MetaDetails { id: cur_id, .. } if cur_id == &id
                        );
                        if !still_on_tmdb_route {
                            return;
                        }
                        let resolved_vid = video_id.clone().filter(|v| !v.trim().is_empty()).or_else(|| {
                            shell.core.with_model(|m| {
                                m.ctx
                                    .library
                                    .items
                                    .get(&imdb_id)
                                    .and_then(|li| li.state.video_id.clone())
                                    .filter(|v| !v.trim().is_empty())
                            })
                        });
                        shell.auto_selected_episode_for_meta = resolved_vid
                            .as_ref()
                            .map(|vid| (imdb_id.clone(), vid.clone()));
                        shell.memoized_meta_streams = None;
                        shell.memoized_enriched_streams = None;
                        shell.synthetic_tmdb_meta = None;
                        shell.core.load_meta_details(
                            &type_name,
                            &imdb_id,
                            resolved_vid.as_deref(),
                        );
                        shell.route = AppRoute::MetaDetails {
                            type_name: type_name.clone(),
                            id: imdb_id.clone(),
                            video_id: resolved_vid,
                        };
                        shell.request_background_enrichment_if_needed(&type_name, &imdb_id, cx);
                        shell.ensure_series_default_episode_streams(cx);
                        shell.maybe_trigger_stream_enrichments(cx);
                        cx.notify();
                    });
                    return;
                }
            }

            if let Some(synthetic) = core
                .fetch_synthetic_tmdb_meta(type_name.clone(), id.clone())
                .await
            {
                let _ = this.update(cx, |shell, cx| {
                    let still_on_tmdb_route = matches!(
                        &shell.route,
                        AppRoute::MetaDetails { id: cur_id, .. } if cur_id == &id
                    );
                    if still_on_tmdb_route {
                        shell.synthetic_tmdb_meta = Some((id, synthetic));
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    pub fn request_background_enrichment_if_needed(
        &mut self,
        media_type: &str,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let trimmed = id.trim();
        if trimmed.is_empty() || self.core.cached_enriched_meta(trimmed).is_some() {
            return;
        }
        let has_tmdb = {
            let cfg = self.core.private_integrations();
            cfg.tmdb_enabled && cfg.has_tmdb_configured()
        };
        if !has_tmdb || !self.enriched_attempted.insert(trimmed.to_owned()) {
            return;
        }
        let core = self.core.clone();
        let mt = media_type.to_owned();
        let req_id = trimmed.to_owned();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if core.ensure_title_enrichment(mt, req_id).await.is_some() {
                let _ = this.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
    }

    pub fn toggle_stream_row_details(&mut self, row_idx: usize, cx: &mut Context<Self>) {
        if !self.expanded_stream_rows.remove(&row_idx) {
            self.expanded_stream_rows.insert(row_idx);
        }
        cx.notify();
    }

    pub fn toggle_player_stats_for_nerds(&mut self, cx: &mut Context<Self>) {
        self.last_osd_activity = Instant::now();
        self.player_stats_for_nerds = !self.player_stats_for_nerds;
        self.schedule_osd_autohide_timer(cx);
        self.ensure_player_telemetry_pulse(cx);
        cx.notify();
    }

    pub fn select_episode_for_streams(
        &mut self,
        type_name: String,
        id: String,
        video_id: String,
        cx: &mut Context<Self>,
    ) {
        self.meta_addon_dropdown_open = false;
        self.auto_selected_episode_for_meta = Some((id.clone(), video_id.clone()));
        self.memoized_meta_streams = None;
        self.memoized_enriched_streams = None;
        self.visible_stream_limit = STREAM_PAGE_SIZE;
        self.expanded_stream_rows.clear();
        self.core
            .load_meta_details(&type_name, &id, Some(&video_id));
        self.route = AppRoute::MetaDetails {
            type_name,
            id,
            video_id: Some(video_id),
        };
        self.maybe_trigger_stream_enrichments(cx);
        cx.notify();
    }

    pub fn ensure_series_default_episode_streams(&mut self, _cx: &mut Context<Self>) {
        let AppRoute::MetaDetails {
            type_name,
            id,
            video_id: route_vid,
        } = self.route.clone()
        else {
            return;
        };

        let target_vid = self.core.with_model(|model| {
            if model
                .meta_details
                .selected
                .as_ref()
                .and_then(|s| s.stream_path.as_ref())
                .is_some()
            {
                return None;
            }
            let (meta, _) = resolve_ready_meta_item(&model.meta_details.meta_items)?;
            if meta.preview.id != id || meta.videos.is_empty() {
                return None;
            }
            let lib_vid = model
                .meta_details
                .library_item
                .as_ref()
                .and_then(|li| li.state.video_id.as_deref())
                .or_else(|| {
                    model
                        .ctx
                        .library
                        .items
                        .get(&id)
                        .and_then(|li| li.state.video_id.as_deref())
                });
            pick_default_series_episode_id(meta, route_vid.as_deref(), lib_vid)
        });

        let Some(default_vid) = target_vid else {
            return;
        };

        if self.auto_selected_episode_for_meta.as_ref() == Some(&(id.clone(), default_vid.clone()))
        {
            return;
        }

        self.auto_selected_episode_for_meta = Some((id.clone(), default_vid.clone()));
        if self
            .pending_quick_resume
            .as_ref()
            .is_some_and(|(_, q_id, q_vid)| q_id == &id && q_vid.is_none())
        {
            self.pending_quick_resume =
                Some((type_name.clone(), id.clone(), Some(default_vid.clone())));
        }
        self.memoized_meta_streams = None;
        self.memoized_enriched_streams = None;
        self.route = AppRoute::MetaDetails {
            type_name: type_name.clone(),
            id: id.clone(),
            video_id: Some(default_vid.clone()),
        };
        self.core
            .load_meta_details(&type_name, &id, Some(&default_vid));
    }

    pub fn quick_resume_or_open(
        &mut self,
        type_name: String,
        id: String,
        video_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.pending_quick_resume = Some((type_name.clone(), id.clone(), video_id.clone()));
        self.open_meta_details(type_name, id, video_id, cx);
        self.try_complete_pending_quick_resume(cx);
    }

    fn try_complete_pending_quick_resume(&mut self, cx: &mut Context<Self>) {
        let Some((req_type, req_id, req_vid)) = self.pending_quick_resume.clone() else {
            return;
        };
        let has_debrid = self.has_debrid_access();
        let memo_slot = &mut self.memoized_meta_streams;

        let ready = self.core.with_model(|m| {
            let (meta, meta_req) = resolve_ready_meta_item(&m.meta_details.meta_items)
                .map(|(item, r)| (item.clone(), r))?;
            if meta.preview.id != req_id {
                return None;
            }

            let li = m
                .meta_details
                .library_item
                .clone()
                .or_else(|| m.ctx.library.items.get(&req_id).cloned());
            let vid = pick_default_series_episode_id(
                &meta,
                req_vid.as_deref(),
                li.as_ref().and_then(|i| i.state.video_id.as_deref()),
            )
            .or_else(|| req_vid.clone())
            .or_else(|| li.as_ref().and_then(|i| i.state.video_id.clone()))
            .unwrap_or_else(|| meta.preview.id.clone());

            let (candidates, _, _, _) = sync_memoized_meta_streams(
                memo_slot,
                &req_type,
                &req_id,
                &vid,
                has_debrid,
                m.meta_details
                    .meta_streams
                    .iter()
                    .chain(m.meta_details.streams.iter()),
                &m.ctx.profile.addons,
            );
            Some((meta, meta_req, vid, candidates, li))
        });

        let Some((meta, meta_req, vid, base_candidates, li)) = ready else {
            return;
        };

        let cfg = self.core.private_integrations();
        let prowlarr_slice = if self.prowlarr_fallback_key.as_ref()
            == Some(&(req_type.clone(), req_id.clone(), vid.clone()))
        {
            self.prowlarr_fallback_streams.as_slice()
        } else {
            &[]
        };
        let (candidates, _) = merge_and_enrich_stream_candidates(
            &base_candidates,
            prowlarr_slice,
            cfg.active_debrid,
            &self.local_debrid_cache_map,
            cfg.has_local_debrid_configured() && cfg.debrid_cached_only_filter,
        );
        if candidates.is_empty() {
            return;
        }

        self.pending_quick_resume = None;
        let resume_secs = crate::state::resume_offset_for_library_item(li.as_ref(), &vid);
        let ep_label = crate::state::format_movietracker_episode_code(Some(&vid));
        let subs_path = Some(build_subtitles_resource_path(&req_type, &vid));

        if matches!(self.route, AppRoute::MetaDetails { .. }) {
            if let Some(origin_entry) = self.nav_history.pop() {
                self.route = origin_entry.route;
                self.selected_season = origin_entry.selected_season;
            }
        }

        self.start_playback_session(
            req_type,
            req_id,
            vid,
            meta.preview.name,
            ep_label,
            Some(meta_req),
            subs_path,
            candidates,
            0,
            resume_secs,
            cx,
        );

        self.memoized_meta_streams = None;
        self.memoized_enriched_streams = None;
        self.auto_selected_episode_for_meta = None;
        self.core.unload_meta_details();
        self.schedule_debounced_heap_trim();
    }

    pub fn navigate_back(&mut self, cx: &mut Context<Self>) {
        if self.route.is_player() {
            self.exit_player(cx);
            return;
        }
        if let Some(prev_entry) = self.nav_history.pop() {
            let cur_entry = self.capture_nav_entry();
            self.push_forward_history_entry(cur_entry);
            self.apply_nav_entry(prev_entry, cx);
        } else if self.route != AppRoute::Tab(SidebarTab::Home) {
            let cur_entry = self.capture_nav_entry();
            self.push_forward_history_entry(cur_entry);
            self.apply_nav_entry(
                NavHistoryEntry {
                    route: AppRoute::Tab(SidebarTab::Home),
                    selected_season: None,
                    scroll_y: 0.0,
                },
                cx,
            );
        }
    }

    pub fn navigate_forward(&mut self, cx: &mut Context<Self>) {
        if self.route.is_player() {
            return;
        }
        let Some(next_entry) = self.nav_forward.pop() else {
            return;
        };
        let cur_entry = self.capture_nav_entry();
        self.push_nav_history_entry(cur_entry);
        self.apply_nav_entry(next_entry, cx);
    }

    pub fn move_kb_focus(&mut self, dr: isize, dc: isize, cx: &mut Context<Self>) -> bool {
        let Some(next_idx) = step_kb_focus_2d(&self.kb_focus_items, self.kb_focus_index, dr, dc)
        else {
            return false;
        };
        if self.kb_focus_index == Some(next_idx) {
            return false;
        }
        self.kb_focus_index = Some(next_idx);
        if let Some(card) = self.kb_focus_items.get(next_idx) {
            match self.route {
                AppRoute::Tab(SidebarTab::Discover) => {
                    self.discover_grid_scroll_handle
                        .scroll_to_item(card.row, gpui::ScrollStrategy::Nearest);
                }
                AppRoute::Tab(SidebarTab::Library) => {
                    self.library_grid_scroll_handle
                        .scroll_to_item(card.row, gpui::ScrollStrategy::Nearest);
                }
                _ => {}
            }
        }
        cx.notify();
        true
    }

    fn scroll_handle_for_page_key(
        handle: &ScrollHandle,
        key: &str,
        viewport_height_px: f32,
    ) -> bool {
        let max_scroll = (handle.max_offset().y / px(1.0)).max(0.0);
        let cur_scrolled = (-handle.offset().y / px(1.0)).max(0.0);
        let page_step = (viewport_height_px - 120.0).max(280.0) * 0.8;
        let next = match key {
            "pageup" => (cur_scrolled - page_step).max(0.0),
            "pagedown" => (cur_scrolled + page_step).min(max_scroll),
            "home" => 0.0,
            "end" => max_scroll,
            _ => return false,
        };
        handle.set_offset(point(px(0.0), px(-next)));
        true
    }

    pub fn scroll_active_view_key(&self, key: &str) -> bool {
        let vh = self.viewport_size_px.1;
        match &self.route {
            AppRoute::Tab(SidebarTab::Home) => {
                Self::scroll_handle_for_page_key(&self.board_scroll_handle, key, vh)
            }
            AppRoute::Tab(SidebarTab::Discover) => match self.preferences.discover_view_mode {
                DiscoverViewMode::BrowseHubs => {
                    Self::scroll_handle_for_page_key(&self.discover_hubs_scroll_handle, key, vh)
                }
                DiscoverViewMode::CatalogGrid => {
                    let guard = self.discover_grid_scroll_handle.0.borrow();
                    Self::scroll_handle_for_page_key(&guard.base_handle, key, vh)
                }
            },
            AppRoute::Tab(SidebarTab::Calendar) => {
                Self::scroll_handle_for_page_key(&self.calendar_scroll_handle, key, vh)
            }
            AppRoute::Tab(SidebarTab::Library) => match self.preferences.library_view_mode {
                LibraryViewMode::PosterGrid => {
                    let guard = self.library_grid_scroll_handle.0.borrow();
                    Self::scroll_handle_for_page_key(&guard.base_handle, key, vh)
                }
                LibraryViewMode::WideBackdrops | LibraryViewMode::CompactList => {
                    Self::scroll_handle_for_page_key(&self.library_scroll_handle, key, vh)
                }
            },
            AppRoute::Tab(SidebarTab::Settings) => {
                Self::scroll_handle_for_page_key(&self.settings_scroll_handle, key, vh)
            }
            AppRoute::MetaDetails { .. } => {
                Self::scroll_handle_for_page_key(&self.meta_scroll_handle, key, vh)
            }
            AppRoute::Search { .. } => {
                Self::scroll_handle_for_page_key(&self.search_scroll_handle, key, vh)
            }
            AppRoute::Person { .. } => {
                Self::scroll_handle_for_page_key(&self.person_scroll_handle, key, vh)
            }
            AppRoute::Tab(SidebarTab::Addons) => {
                Self::scroll_handle_for_page_key(&self.addons_scroll_handle, key, vh)
            }
            AppRoute::Tab(SidebarTab::Account) => {
                Self::scroll_handle_for_page_key(&self.auth_scroll_handle, key, vh)
            }
            AppRoute::Player => false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_playback_session(
        &mut self,
        type_name: String,
        meta_id: String,
        video_id: String,
        title: String,
        episode_label: Option<String>,
        meta_request: Option<ResourceRequest>,
        subtitles_path: Option<ResourcePath>,
        candidates: Arc<Vec<PlaybackCandidate>>,
        active_index: usize,
        start_offset_secs: Option<f64>,
        cx: &mut Context<Self>,
    ) {
        self.start_playback_session_inner(
            type_name,
            meta_id,
            video_id,
            title,
            episode_label,
            meta_request,
            subtitles_path,
            candidates,
            active_index,
            start_offset_secs,
            None,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn start_playback_session_inner(
        &mut self,
        type_name: String,
        meta_id: String,
        video_id: String,
        title: String,
        episode_label: Option<String>,
        meta_request: Option<ResourceRequest>,
        subtitles_path: Option<ResourcePath>,
        candidates: Arc<Vec<PlaybackCandidate>>,
        active_index: usize,
        start_offset_secs: Option<f64>,
        prewarmed_direct_url: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if candidates.is_empty() {
            return;
        }

        if !self.route.is_player() {
            let entry = self.capture_nav_entry();
            self.push_nav_history_entry(entry);
            self.nav_forward.clear();
        }

        self.on_major_route_transition(true);
        self.ensure_mpv_initialized(cx);

        self.playback_generation_seq = self.playback_generation_seq.wrapping_add(1);
        let session = PlaybackSession {
            generation: self.playback_generation_seq,
            type_name,
            meta_id,
            video_id,
            title,
            episode_label,
            meta_request,
            subtitles_path,
            candidates,
            active_index: active_index.min(usize::MAX - 1),
            start_offset_secs,
            seamless_switch_pending: false,
            applied_series_track_prefs: false,
            debrid_resolving: false,
            debrid_error: None,
            resolved_url: None,
            loaded_into_mpv: false,
            started_at: Instant::now(),
            first_media_ready: false,
            auto_failover_attempts: 0,
            failover_exhausted_notified: false,
            attached_external_subs: HashSet::new(),
            last_reported_time_ms: 0,
            last_reported_paused: None,
            reported_ended: false,
        };

        if let Some(selected) = session.build_player_selected() {
            self.core.load_player(selected);
        }

        self.playback_session = Some(session);
        self.player_popover = None;
        self.player_source_limit = STREAM_PAGE_SIZE;
        self.meta_addon_dropdown_open = false;
        self.discover_open_dropdown = None;
        self.avatar_menu_open = false;
        self.context_menu = None;
        self.quick_peek = None;
        self.command_palette_open = false;
        self.kb_focus_index = None;
        self.kb_focus_items.clear();
        self.expanded_stream_rows.clear();
        self.next_episode_prewarmed = false;
        self.prewarmed_next = None;
        self.prewarm_fetch_requested_vid = None;
        self.prewarmed_next_direct_url = None;
        self.native_sub_match.reset();
        self.pending_auto_select_sub_title = None;
        self.user_picked_subtitle = false;
        self.secondary_forced_available = None;
        self.live_untranslated_upgrade_triggered = false;
        self.is_scrubbing = false;
        self.is_dragging_volume = false;
        self.scrub_hover_fraction = None;
        self.scrub_hover_metrics = None;
        self.replace_hover_thumbnail(None, None);
        self.in_flight_thumb_key = None;
        self.last_intro_visible = false;
        self.last_next_ep_visible = false;
        self.last_osd_activity = Instant::now();
        self.route = AppRoute::Player;
        self.schedule_osd_autohide_timer(cx);
        self.schedule_dead_swarm_watchdog(cx);
        self.ensure_player_telemetry_pulse(cx);
        self.sync_os_integrations();
        if let Some(direct_url) = prewarmed_direct_url {
            self.load_url_into_active_session(&direct_url, cx);
            self.sync_player_from_core_model(cx);
        } else {
            self.begin_active_candidate_resolution(cx);
        }
        cx.notify();
    }

    pub fn switch_active_player_stream(
        &mut self,
        new_index: usize,
        is_auto_failover: bool,
        cx: &mut Context<Self>,
    ) {
        let preserve_pos = if self.player_snapshot.time_pos > 1.5 {
            Some(self.player_snapshot.time_pos)
        } else {
            self.playback_session
                .as_ref()
                .and_then(|s| s.start_offset_secs)
        };
        let was_loaded = self
            .playback_session
            .as_ref()
            .is_some_and(|s| s.loaded_into_mpv);

        let Some(session) = self.playback_session.as_mut() else {
            return;
        };
        if new_index >= session.candidates.len() {
            return;
        }

        session.active_index = new_index;
        session.start_offset_secs = preserve_pos;
        session.seamless_switch_pending = was_loaded;
        session.debrid_resolving = false;
        session.debrid_error = None;
        session.resolved_url = None;
        session.loaded_into_mpv = false;
        session.first_media_ready = false;
        session.started_at = Instant::now();
        session.attached_external_subs.clear();
        if !is_auto_failover {
            session.auto_failover_attempts = 0;
            session.failover_exhausted_notified = false;
        }

        let badge = session
            .active_candidate()
            .map(|c| c.summary_badge())
            .unwrap_or_default();

        if let Some(selected) = session.build_player_selected() {
            self.core.load_player(selected);
        }

        self.native_sub_match.reset();
        self.pending_auto_select_sub_title = None;
        self.secondary_forced_available = None;
        self.live_untranslated_upgrade_triggered = false;

        self.schedule_dead_swarm_watchdog(cx);
        self.ensure_player_telemetry_pulse(cx);

        if !is_auto_failover {
            self.player_popover = None;
            self.push_toast(
                ToastKind::Info,
                format!("Switched to stream #{}", new_index + 1),
                Some(badge),
                cx,
            );
        }
        self.begin_active_candidate_resolution(cx);
        cx.notify();
    }

    fn begin_active_candidate_resolution(&mut self, cx: &mut Context<Self>) {
        let cfg = self.core.private_integrations();
        let torrent_params = self.playback_session.as_ref().and_then(|session| {
            let cand = session.active_candidate()?;
            match &cand.stream.source {
                StreamSource::Torrent {
                    info_hash,
                    file_idx,
                    announce,
                    ..
                } if cfg.direct_rust_unrestrict && cfg.has_local_debrid_configured() => Some((
                    session.generation,
                    session.active_index,
                    *info_hash,
                    file_idx.map(u32::from),
                    announce.clone(),
                    parse_season_episode_from_video_id(Some(&session.video_id)),
                )),
                _ => None,
            }
        });

        let Some((gen_id, cand_idx, info_hash, file_idx, announce, season_episode)) =
            torrent_params
        else {
            self.sync_player_from_core_model(cx);
            return;
        };

        if let Some(session) = self.playback_session.as_mut() {
            session.debrid_resolving = true;
            session.debrid_error = None;
        }

        let core = self.core.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = core
                .resolve_torrent_via_local_debrid(info_hash, file_idx, announce, season_episode)
                .await;
            let _ = this.update(cx, |shell, cx| {
                shell.last_debrid_latency_ms = shell.core.last_debrid_resolve_latency_ms();
                let still_active = shell.route.is_player()
                    && shell.playback_session.as_ref().is_some_and(|s| {
                        s.generation == gen_id && s.active_index == cand_idx
                    });
                if !still_active {
                    return;
                }

                match result {
                    Ok(resolved) => {
                        shell.load_url_into_active_session(resolved.url.as_str(), cx);
                        shell.sync_player_from_core_model(cx);
                    }
                    Err(err) => {
                        let cfg_now = shell.core.private_integrations();
                        let fallback_p2p = if is_uncached_debrid_error(&err)
                            && !cfg_now.debrid_cached_only_filter
                        {
                            shell.playback_session.as_ref().and_then(|s| {
                                s.active_candidate().and_then(|c| {
                                    shell.core.resolve_stream_playback_url(&c.stream)
                                })
                            })
                        } else {
                            None
                        };

                        if let Some(session) = shell.playback_session.as_mut() {
                            session.debrid_resolving = false;
                        }

                        if let Some(p2p_url) = fallback_p2p {
                            shell.push_toast(
                                ToastKind::Info,
                                "Uncached on Debrid — falling back to P2P swarm",
                                Some(err),
                                cx,
                            );
                            shell.load_url_into_active_session(p2p_url.as_str(), cx);
                            shell.sync_player_from_core_model(cx);
                        } else {
                            if let Some(session) = shell.playback_session.as_mut() {
                                session.debrid_error = Some(err.clone());
                            }
                            shell.push_toast(
                                ToastKind::Error,
                                if cfg_now.debrid_cached_only_filter {
                                    "Stream uncached (Cached-only filter enabled)"
                                } else {
                                    "Local Debrid resolve failed"
                                },
                                Some(err),
                                cx,
                            );
                            cx.notify();
                        }
                    }
                }
            });
        })
        .detach();
    }

    fn trigger_dead_swarm_failover(&mut self, reason: &str, cx: &mut Context<Self>) {
        enum FailoverStep {
            Switch {
                next_idx: usize,
                next_badge: String,
                attempt: usize,
            },
            Exhausted {
                notify_error: bool,
            },
        }

        let step = {
            let Some(session) = self.playback_session.as_mut() else {
                return;
            };
            if session.auto_failover_attempts >= MAX_AUTO_FAILOVER_ATTEMPTS {
                session.debrid_error =
                    Some(format!("{reason} — automatic backup streams exhausted"));
                let notify_error = !session.failover_exhausted_notified;
                session.failover_exhausted_notified = true;
                FailoverStep::Exhausted { notify_error }
            } else {
                let next_playable = ((session.active_index + 1)..session.candidates.len()).find(
                    |&idx| {
                        session
                            .candidates
                            .get(idx)
                            .is_some_and(|c| is_native_playable_stream(&c.stream))
                    },
                );
                if let Some(next_idx) = next_playable {
                    session.auto_failover_attempts += 1;
                    let attempt = session.auto_failover_attempts;
                    let next_badge = session.candidates[next_idx].summary_badge();
                    FailoverStep::Switch {
                        next_idx,
                        next_badge,
                        attempt,
                    }
                } else {
                    session.debrid_error =
                        Some(format!("{reason} — no playable backup streams remaining"));
                    let notify_error = !session.failover_exhausted_notified;
                    session.failover_exhausted_notified = true;
                    FailoverStep::Exhausted { notify_error }
                }
            }
        };

        match step {
            FailoverStep::Switch {
                next_idx,
                next_badge,
                attempt,
            } => {
                self.switch_active_player_stream(next_idx, true, cx);
                self.push_toast(
                    ToastKind::Warning,
                    format!(
                        "Switched to backup stream #{} ({attempt}/{MAX_AUTO_FAILOVER_ATTEMPTS})",
                        next_idx + 1
                    ),
                    Some(format!("{reason} - trying {next_badge}")),
                    cx,
                );
            }
            FailoverStep::Exhausted { notify_error } => {
                if notify_error {
                    self.push_toast(
                        ToastKind::Error,
                        "Playback failed",
                        Some(format!("{reason} — use Switch source to pick another stream")),
                        cx,
                    );
                }
                cx.notify();
            }
        }
    }

    pub fn advance_to_next_episode(&mut self, next_vid: Video, cx: &mut Context<Self>) {
        let Some(cur) = self.playback_session.clone() else {
            return;
        };

        let prewarmed_direct_url = self
            .prewarmed_next_direct_url
            .take()
            .and_then(|(vid, url)| (vid == next_vid.id).then_some(url));

        let resolved_opt = self
            .prewarmed_next
            .take()
            .filter(|r| r.next_video.id == next_vid.id)
            .or_else(|| self.core.resolve_next_episode_playback_url());

        if let Some(resolved) = resolved_opt {
            let installed_addons = self.core.with_model(|m| m.ctx.profile.addons.clone());
            let ep_label = resolved
                .next_video
                .series_info
                .as_ref()
                .map(|s| format!("S{} E{} - {}", s.season, s.episode, resolved.next_video.title));
            let subs = resolved.player_selected.subtitles_path.clone().or_else(|| {
                Some(build_subtitles_resource_path(
                    &cur.type_name,
                    &resolved.next_video.id,
                ))
            });
            let stream_req = resolved.player_selected.stream_request.clone();
            let meta_req = resolved.player_selected.meta_request.or(cur.meta_request);
            let mut candidates = Vec::new();
            candidates.push(PlaybackCandidate::from_stream_and_request(
                resolved.stream.clone(),
                stream_req,
                &installed_addons,
            ));
            for (cand_stream, cand_req) in resolved.ranked_candidates {
                if cand_stream == resolved.stream {
                    continue;
                }
                candidates.push(PlaybackCandidate::from_stream_and_request(
                    cand_stream,
                    cand_req,
                    &installed_addons,
                ));
            }

            self.start_playback_session_inner(
                cur.type_name,
                cur.meta_id,
                resolved.next_video.id,
                cur.title,
                ep_label,
                meta_req,
                subs,
                Arc::new(candidates),
                0,
                None,
                prewarmed_direct_url,
                cx,
            );
        } else {
            let t = cur.type_name;
            let m = cur.meta_id;
            self.exit_player(cx);
            self.open_meta_details(t, m, Some(next_vid.id), cx);
        }
    }

    fn maybe_prewarm_next_episode(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(resolved) = self.core.resolve_next_episode_playback_url() else {
            return false;
        };
        let url_to_warm = resolved.url.clone();
        let next_vid_id = resolved.next_video.id.clone();
        let next_stream = resolved.stream.clone();
        self.next_episode_prewarmed = true;
        self.prewarmed_next = Some(resolved);

        let gen_id = self
            .playback_session
            .as_ref()
            .map(|s| s.generation)
            .unwrap_or(0);
        let meta_id = self
            .playback_session
            .as_ref()
            .map(|s| s.meta_id.clone())
            .unwrap_or_default();
        let type_name = self
            .playback_session
            .as_ref()
            .map(|s| s.type_name.clone())
            .unwrap_or_else(|| "series".to_owned());

        let cfg = self.core.private_integrations();
        let engine = self.engine.clone();
        let core = self.core.clone();
        let tokio_handle = self.tokio_handle.clone();

        match &next_stream.source {
            StreamSource::Torrent {
                info_hash,
                file_idx,
                announce,
                ..
            } if cfg.direct_rust_unrestrict && cfg.has_local_debrid_configured() => {
                let hash = *info_hash;
                let file_idx_u32 = file_idx.map(u32::from);
                let file_idx_usize = file_idx.map(usize::from);
                let trackers = announce.clone();
                let season_ep = parse_season_episode_from_video_id(Some(&next_vid_id));
                let cached_only = cfg.debrid_cached_only_filter;
                let next_vid_for_state = next_vid_id.clone();

                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    let debrid_res = core
                        .resolve_torrent_via_local_debrid(hash, file_idx_u32, trackers.clone(), season_ep)
                        .await;
                    match debrid_res {
                        Ok(debrid) => {
                            let warm_url = debrid.url.clone();
                            let warm_url_str = debrid.url.to_string();
                            let engine_clone = engine.clone();
                            let _ = tokio_handle
                                .spawn(async move {
                                    let _ = engine_clone
                                        .prewarm_stream(&warm_url, PREWARM_HEAD_BYTES)
                                        .await;
                                })
                                .await;
                            let _ = this.update(cx, |shell, cx| {
                                if shell.route.is_player()
                                    && shell
                                        .playback_session
                                        .as_ref()
                                        .is_some_and(|s| s.generation == gen_id)
                                {
                                    shell.prewarmed_next_direct_url =
                                        Some((next_vid_for_state, warm_url_str));
                                    cx.notify();
                                }
                            });
                        }
                        Err(_) if !cached_only => {
                            let hash_hex = hex::encode(hash);
                            let _ = tokio_handle
                                .spawn(async move {
                                    let _ = engine
                                        .prebuffer_episode(&hash_hex, file_idx_usize, trackers)
                                        .await;
                                })
                                .await;
                        }
                        Err(_) => {}
                    }
                })
                .detach();
            }
            StreamSource::Torrent {
                info_hash,
                file_idx,
                announce,
                ..
            } => {
                let hash_hex = hex::encode(info_hash);
                let file_idx_usize = file_idx.map(usize::from);
                let trackers = announce.clone();
                self.tokio_handle.spawn(async move {
                    let _ = engine
                        .prebuffer_episode(&hash_hex, file_idx_usize, trackers)
                        .await;
                });
            }
            _ => {
                self.tokio_handle.spawn(async move {
                    let _ = engine
                        .prewarm_stream(&url_to_warm, PREWARM_HEAD_BYTES)
                        .await;
                });
            }
        }

        // Pre-warm Episode N+1 native subtitles into the local cache
        if self.preferences.auto_match_native_subtitles && cfg.native_subtitles_enabled && !meta_id.is_empty() {
            let (season, episode) = parse_season_episode_from_video_id(Some(&next_vid_id))
                .map(|(s, e)| (Some(s), Some(e)))
                .unwrap_or((None, None));
            let pref_lang = self.normalized_preferred_sub_lang();
            let sub_query = NativeSubtitleQuery {
                imdb_id: meta_id,
                media_type: type_name,
                season,
                episode,
                opensubhash: None,
                video_byte_size: next_stream.behavior_hints.video_size,
                video_filename: next_stream
                    .behavior_hints
                    .filename
                    .clone()
                    .or_else(|| next_stream.name.clone())
                    .or_else(|| next_stream.description.clone()),
                preferred_langs: vec![pref_lang],
                search_mode: SubtitleSearchMode::Full,
            };
            let core_subs = self.core.clone();
            self.tokio_handle.spawn(async move {
                let _ = core_subs.search_and_fetch_native_subtitles(sub_query, 2).await;
            });
        }
        true
    }

    pub fn exit_player(&mut self, cx: &mut Context<Self>) {
        self.exit_fullscreen_on_next_render = true;
        self.teardown_active_playback();
        let next_entry = self.nav_history.pop().unwrap_or(NavHistoryEntry {
            route: AppRoute::Tab(SidebarTab::Home),
            selected_season: None,
            scroll_y: 0.0,
        });
        self.apply_nav_entry(next_entry, cx);
    }

    fn teardown_active_playback(&mut self) {
        self.exit_fullscreen_on_next_render = true;
        if let Some(mpv) = self.mpv.take() {
            self.mpv_subscriptions_attached = false;
            mpv.shutdown_async();
        }
        self.player_snapshot = PlayerSnapshot::default();
        self.core.unload_player();
        self.engine.release_playback_sessions_bg();
        self.thumbnailer.clear_and_suspend();
        self.player_telemetry_pulse_seq = self.player_telemetry_pulse_seq.wrapping_add(1);
        self.playback_session = None;
        self.player_popover = None;
        self.player_source_limit = STREAM_PAGE_SIZE;
        self.hold_2x_active = false;
        self.space_key_down = false;
        self.is_scrubbing = false;
        self.is_dragging_volume = false;
        self.scrub_hover_fraction = None;
        self.scrub_hover_metrics = None;
        self.replace_hover_thumbnail(None, None);
        self.in_flight_thumb_key = None;
        self.next_episode_prewarmed = false;
        self.prewarmed_next = None;
        self.prewarm_fetch_requested_vid = None;
        self.prewarmed_next_direct_url = None;
        self.native_sub_match.reset();
        self.pending_auto_select_sub_title = None;
        self.secondary_forced_available = None;
        self.live_untranslated_upgrade_triggered = false;
        self.user_picked_subtitle = false;
        self.sync_os_integrations();
        self.schedule_debounced_heap_trim();
    }

    pub fn is_player_osd_visible(&self) -> bool {
        !self.is_player_video_frame_ready()
            || self.player_snapshot.paused
            || self.player_snapshot.buffering
            || self.player_popover.is_some()
            || self.is_scrubbing
            || self.is_dragging_volume
            || self.scrub_hover_fraction.is_some()
            || self.last_osd_activity.elapsed() < self.osd_autohide_duration()
    }

    pub fn bump_player_osd_activity(&mut self, cx: &mut Context<Self>) {
        let was_hidden = !self.is_player_osd_visible();
        self.last_osd_activity = Instant::now();
        self.schedule_osd_autohide_timer(cx);
        if was_hidden {
            cx.notify();
        }
    }

    pub fn clear_scrub_hover(&mut self, cx: &mut Context<Self>) {
        let had_hover =
            self.scrub_hover_fraction.take().is_some() || self.scrub_hover_metrics.take().is_some();
        if had_hover {
            self.replace_hover_thumbnail(None, None);
            self.in_flight_thumb_key = None;
            self.last_osd_activity = Instant::now();
            self.schedule_osd_autohide_timer(cx);
            cx.notify();
        }
    }

    pub fn end_player_drag_interactions(&mut self, cx: &mut Context<Self>) {
        if self.is_scrubbing || self.is_dragging_volume {
            self.is_scrubbing = false;
            self.is_dragging_volume = false;
            self.last_osd_activity = Instant::now();
            self.schedule_osd_autohide_timer(cx);
            cx.notify();
        }
    }

    pub fn set_player_volume_user(
        &mut self,
        volume: f64,
        unmute_if_positive: bool,
        cx: &mut Context<Self>,
    ) {
        self.bump_player_osd_activity(cx);
        let clamped = volume.clamp(0.0, 100.0);
        self.player_snapshot.volume = clamped;
        if let Some(mpv) = &self.mpv {
            let _ = mpv.set_volume(clamped);
            if unmute_if_positive && self.player_snapshot.muted && clamped > 0.0 {
                self.player_snapshot.muted = false;
                let _ = mpv.set_muted(false);
            }
        }
        cx.notify();
    }

    pub fn adjust_player_volume_step(&mut self, delta: f64, cx: &mut Context<Self>) {
        let next = (self.player_snapshot.volume + delta).clamp(0.0, 100.0);
        self.set_player_volume_user(next, delta > 0.0, cx);
    }

    pub fn adjust_player_volume_by_wheel(&mut self, delta: ScrollDelta, cx: &mut Context<Self>) {
        let up_amount = match delta {
            ScrollDelta::Pixels(p) => p.y / px(1.0),
            ScrollDelta::Lines(l) => l.y * 36.0,
        };
        if up_amount > 0.1 {
            self.adjust_player_volume_step(5.0, cx);
        } else if up_amount < -0.1 {
            self.adjust_player_volume_step(-5.0, cx);
        }
    }

    pub fn toggle_player_mute_user(&mut self, cx: &mut Context<Self>) {
        self.bump_player_osd_activity(cx);
        let next_muted = !self.player_snapshot.muted;
        self.player_snapshot.muted = next_muted;
        if let Some(mpv) = &self.mpv {
            let _ = mpv.set_muted(next_muted);
        }
        cx.notify();
    }

    pub fn on_scrub_bar_hover(
        &mut self,
        fraction: f32,
        metrics: Option<(f32, f32)>,
        cx: &mut Context<Self>,
    ) {
        self.last_osd_activity = Instant::now();
        self.schedule_osd_autohide_timer(cx);
        let clamped = fraction.clamp(0.0, 1.0);
        self.scrub_hover_fraction = Some(clamped);
        if metrics.is_some() {
            self.scrub_hover_metrics = metrics;
        }

        let duration = self.player_snapshot.duration;
        let url_opt = self
            .playback_session
            .as_ref()
            .and_then(|s| s.resolved_url.clone());

        if duration > 1.0 {
            if let Some(url) = url_opt {
                let hover_secs = (clamped as f64) * duration;
                let bucket_ms = quantize_scrub_bucket_ms(hover_secs);
                self.ensure_resting_scrub_thumbnail(url, hover_secs, bucket_ms, cx);
                return;
            }
        }
        cx.notify();
    }

    fn ensure_resting_scrub_thumbnail(
        &mut self,
        url: String,
        hover_secs: f64,
        bucket_ms: i64,
        cx: &mut Context<Self>,
    ) {
        if let Some(cached) = self.thumbnailer.get_cached_thumbnail(&url, hover_secs) {
            self.replace_hover_thumbnail(Some(cached.render_image), Some((url, bucket_ms)));
            cx.notify();
            return;
        }

        if is_scrub_thumbnail_stale(self.hover_thumbnail_key.as_ref(), &url, bucket_ms) {
            self.replace_hover_thumbnail(None, None);
        }

        let req_key = (url.clone(), bucket_ms);
        if self.in_flight_thumb_key.as_ref() != Some(&req_key) {
            self.in_flight_thumb_key = Some(req_key.clone());
            let thumb = self.thumbnailer.clone();
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let res = thumb.request_thumbnail(&url, hover_secs).await;
                let _ = this.update(cx, |shell, cx| {
                    let was_latest_in_flight = shell.in_flight_thumb_key.as_ref() == Some(&req_key);
                    if was_latest_in_flight {
                        shell.in_flight_thumb_key = None;
                    }

                    let current_target = shell.scrub_hover_fraction.and_then(|cur_frac| {
                        let cur_dur = shell.player_snapshot.duration;
                        let cur_url = shell
                            .playback_session
                            .as_ref()
                            .and_then(|s| s.resolved_url.clone())?;
                        if cur_dur > 1.0 {
                            let cur_secs = (cur_frac as f64) * cur_dur;
                            let cur_ms = quantize_scrub_bucket_ms(cur_secs);
                            Some((cur_url, cur_secs, cur_ms))
                        } else {
                            None
                        }
                    });

                    match (res, current_target) {
                        (Some(frame), Some((cur_url, cur_secs, cur_ms))) => {
                            if !is_scrub_thumbnail_stale(Some(&req_key), &cur_url, cur_ms) {
                                shell.replace_hover_thumbnail(
                                    Some(frame.render_image),
                                    Some(req_key.clone()),
                                );
                                cx.notify();
                            } else {
                                shell.poster_cache.push_evicted_image(frame.render_image);
                            }

                            if (cur_url.as_str(), cur_ms) != (req_key.0.as_str(), req_key.1) {
                                shell.ensure_resting_scrub_thumbnail(
                                    cur_url, cur_secs, cur_ms, cx,
                                );
                            }
                        }
                        (Some(frame), None) => {
                            shell.poster_cache.push_evicted_image(frame.render_image);
                        }
                        (None, Some((cur_url, cur_secs, cur_ms))) => {
                            let cur_key = (cur_url.clone(), cur_ms);
                            let moved_to_other_bucket = cur_key != req_key;
                            let superseded_same_bucket =
                                !was_latest_in_flight && shell.in_flight_thumb_key.is_none();
                            if (moved_to_other_bucket || superseded_same_bucket)
                                && shell.in_flight_thumb_key.as_ref() != Some(&cur_key)
                            {
                                shell.ensure_resting_scrub_thumbnail(
                                    cur_url, cur_secs, cur_ms, cx,
                                );
                            }
                        }
                        (None, None) => {}
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    pub fn seek_player_relative(&mut self, delta_secs: f64, cx: &mut Context<Self>) {
        self.bump_player_osd_activity(cx);
        if let Some(mpv) = &self.mpv {
            let _ = mpv.seek_relative(delta_secs);
        }
        let target = (self.player_snapshot.time_pos + delta_secs)
            .clamp(0.0, self.player_snapshot.duration.max(0.0));
        let dur_ms = (self.player_snapshot.duration.max(0.0) * 1000.0) as u64;
        self.core
            .player_seek((target * 1000.0) as u64, dur_ms, "ferrite");
        self.os_media.notify_seeked(target);
    }

    pub fn seek_player_absolute(&mut self, target_secs: f64, cx: &mut Context<Self>) {
        self.bump_player_osd_activity(cx);
        let clamped = target_secs.clamp(0.0, self.player_snapshot.duration.max(0.0));
        if let Some(mpv) = &self.mpv {
            let _ = mpv.seek_absolute(clamped);
        }
        let dur_ms = (self.player_snapshot.duration.max(0.0) * 1000.0) as u64;
        self.core
            .player_seek((clamped * 1000.0) as u64, dur_ms, "ferrite");
        self.os_media.notify_seeked(clamped);
    }

    pub fn step_player_frame(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.bump_player_osd_activity(cx);
        if let Some(mpv) = &self.mpv {
            if forward {
                let _ = mpv.frame_step();
            } else {
                let _ = mpv.frame_back_step();
            }
        }
        cx.notify();
    }

    pub fn toggle_player_popover(&mut self, pop: PlayerPopover, cx: &mut Context<Self>) {
        self.last_osd_activity = Instant::now();
        let same_family = self.player_popover.is_some_and(|cur| {
            cur == pop
                || (pop == PlayerPopover::AudioAndSubtitles && cur.is_audio_subs_family())
                || (pop == PlayerPopover::PictureAndPlayback && cur.is_picture_playback_family())
        });
        if same_family {
            self.player_popover = None;
            self.schedule_osd_autohide_timer(cx);
        } else {
            if pop == PlayerPopover::SwitchStream {
                self.player_source_limit = STREAM_PAGE_SIZE;
            }
            self.player_popover = Some(pop);
        }
        cx.notify();
    }

    pub fn user_select_audio_track(&mut self, track_id: i64, cx: &mut Context<Self>) {
        if let Some(mpv) = &self.mpv {
            let _ = mpv.select_audio_track(track_id);
        }
        self.persist_series_track_selection(Some(track_id), None, None);
        cx.notify();
    }

    pub fn user_select_subtitle_track(&mut self, track_id: Option<i64>, cx: &mut Context<Self>) {
        self.user_picked_subtitle = true;
        self.pending_auto_select_sub_title = None;
        if let Some(mpv) = &self.mpv {
            let _ = mpv.set_sub_forced_events_only(false);
            let _ = mpv.set_secondary_subtitle_track(None, SecondaryForcedMode::Off);
            let _ = mpv.select_subtitle_track(track_id);
        }
        self.persist_series_track_selection(None, Some(track_id), None);
        cx.notify();
    }

    pub fn cycle_audio_tracks(&mut self, cx: &mut Context<Self>) {
        let audio: Vec<i64> = self
            .player_snapshot
            .tracks
            .iter()
            .filter(|t| t.track_type == TrackType::Audio)
            .map(|t| t.id)
            .collect();
        if audio.is_empty() {
            return;
        }
        let cur_idx = self
            .player_snapshot
            .tracks
            .iter()
            .filter(|t| t.track_type == TrackType::Audio)
            .position(|t| t.selected)
            .unwrap_or(0);
        let next_id = audio[(cur_idx + 1) % audio.len()];
        self.user_select_audio_track(next_id, cx);
        self.push_toast(ToastKind::Info, format!("Audio track #{next_id}"), None, cx);
    }

    pub fn cycle_subtitle_tracks(&mut self, cx: &mut Context<Self>) {
        let subs: Vec<i64> = self
            .player_snapshot
            .tracks
            .iter()
            .filter(|t| t.track_type == TrackType::Sub)
            .map(|t| t.id)
            .collect();
        if subs.is_empty() {
            self.user_select_subtitle_track(None, cx);
            return;
        }
        let selected_pos = self
            .player_snapshot
            .tracks
            .iter()
            .filter(|t| t.track_type == TrackType::Sub)
            .position(|t| t.selected);

        match selected_pos {
            None => {
                let first = subs[0];
                self.user_select_subtitle_track(Some(first), cx);
                self.push_toast(ToastKind::Info, format!("Subtitles #{first}"), None, cx);
            }
            Some(idx) if idx + 1 < subs.len() => {
                let next = subs[idx + 1];
                self.user_select_subtitle_track(Some(next), cx);
                self.push_toast(ToastKind::Info, format!("Subtitles #{next}"), None, cx);
            }
            Some(_) => {
                self.user_select_subtitle_track(None, cx);
                self.push_toast(ToastKind::Info, "Subtitles off", None, cx);
            }
        }
    }

    pub fn adjust_subtitle_delay(&mut self, delta_secs: f64, cx: &mut Context<Self>) {
        self.bump_player_osd_activity(cx);
        let next = (((self.player_snapshot.sub_delay + delta_secs) * 10.0).round() / 10.0)
            .clamp(-30.0, 30.0);
        self.player_snapshot.sub_delay = next;
        if let Some(mpv) = &self.mpv {
            let _ = mpv.set_subtitle_delay(next);
        }
        self.persist_series_track_selection(None, None, Some(next));
        cx.notify();
    }

    pub fn set_playback_speed_user(&mut self, speed: f64, cx: &mut Context<Self>) {
        self.bump_player_osd_activity(cx);
        self.playback_speed = speed.clamp(0.25, 4.0);
        if let Some(mpv) = &self.mpv {
            let _ = mpv.set_speed(self.playback_speed);
        }
        cx.notify();
    }

    pub fn set_tone_mapping_choice(&mut self, choice: ToneMappingChoice, cx: &mut Context<Self>) {
        self.preferences.tone_mapping = choice;
        if let Some(mpv) = &self.mpv {
            let _ = mpv.set_hdr_tone_mapping(choice.to_mpv());
        }
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn set_upscale_choice(&mut self, choice: UpscaleChoice, cx: &mut Context<Self>) {
        self.preferences.upscale_mode = choice;
        if let Some(mpv) = &self.mpv {
            let _ = mpv.set_upscale_mode(choice.to_mpv());
        }
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn apply_experimental_video_to_mpv(&self) {
        if let Some(mpv) = &self.mpv {
            let exp = self.preferences.experimental_video;
            let _ = mpv.set_shader_preset(exp.shader_preset.to_mpv());
            let _ = mpv.set_deband(exp.to_deband_config());
            let _ = mpv.set_hdr_backend_options(exp.to_hdr_backend_options());
        }
    }

    pub fn set_storage_mode_preference(
        &mut self,
        choice: StorageModeChoice,
        cx: &mut Context<Self>,
    ) {
        self.preferences.storage_mode = choice;
        let _ = self.preferences.save();
        let engine = self.engine.clone();
        let tokio_handle = self.tokio_handle.clone();
        let engine_mode = choice.to_engine_mode();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = switch_engine_storage_mode_on_tokio(&tokio_handle, engine, engine_mode).await;
            let _ = this.update(cx, |shell, cx| match res {
                Ok(()) => {
                    shell.push_toast(
                        ToastKind::Success,
                        format!("Storage mode: {}", choice.short_badge()),
                        None,
                        cx,
                    );
                }
                Err(err) => {
                    shell.push_toast(
                        ToastKind::Error,
                        "Storage mode switch failed",
                        Some(format!("{err}")),
                        cx,
                    );
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub fn toggle_upnp_preference(&mut self, cx: &mut Context<Self>) {
        self.preferences.enable_upnp = !self.preferences.enable_upnp;
        self.engine.set_enable_upnp(self.preferences.enable_upnp);
        let _ = self.preferences.save();
        self.push_toast(
            ToastKind::Info,
            if self.preferences.enable_upnp {
                "UPnP / NAT-PMP enabled"
            } else {
                "UPnP / NAT-PMP disabled"
            },
            None,
            cx,
        );
    }

    pub fn set_dead_swarm_timeout_secs(&mut self, secs: u64, cx: &mut Context<Self>) {
        self.preferences.dead_swarm_timeout_secs = secs;
        self.engine
            .set_first_byte_timeout(Duration::from_secs(secs.max(20)));
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn set_debrid_access_mode(&mut self, mode: DebridAccessMode, cx: &mut Context<Self>) {
        self.preferences.debrid_access_mode = mode;
        if !self.has_debrid_access() {
            self.stream_filter.cached_only = false;
        }
        self.memoized_meta_streams = None;
        self.memoized_enriched_streams = None;
        self.visible_stream_limit = STREAM_PAGE_SIZE;
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn set_settings_category(
        &mut self,
        category: SettingsCategory,
        cx: &mut Context<Self>,
    ) {
        self.active_settings_category = category;
        self.settings_open_dropdown = None;
        self.settings_scroll_handle
            .set_offset(point(px(0.0), px(0.0)));
        if !self.settings_search_query.is_empty() {
            self.settings_search_query.clear();
            self.input_cursors.remove(&FocusedInput::SettingsSearch);
            self.input_selected_all.remove(&FocusedInput::SettingsSearch);
        }
        cx.notify();
    }

    pub fn clear_settings_search(&mut self, cx: &mut Context<Self>) {
        self.settings_search_query.clear();
        self.input_cursors.remove(&FocusedInput::SettingsSearch);
        self.input_selected_all.remove(&FocusedInput::SettingsSearch);
        self.settings_open_dropdown = None;
        if self.focused_input == Some(FocusedInput::SettingsSearch) {
            self.focused_input = None;
        }
        cx.notify();
    }

    pub fn toggle_check_updates_on_startup(&mut self, cx: &mut Context<Self>) {
        self.preferences.check_updates_on_startup = !self.preferences.check_updates_on_startup;
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn toggle_clipboard_addon_detect(&mut self, cx: &mut Context<Self>) {
        self.preferences.clipboard_addon_detect = !self.preferences.clipboard_addon_detect;
        if !self.preferences.clipboard_addon_detect {
            self.clipboard_addon_suggestion = None;
        }
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn toggle_show_home_spotlight(&mut self, cx: &mut Context<Self>) {
        self.preferences.show_home_spotlight = !self.preferences.show_home_spotlight;
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn set_theme_preset(&mut self, preset: ThemePreset, cx: &mut Context<Self>) {
        self.preferences.theme_preset = preset;
        self.preferences.apply_theme();
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn set_accent_color(&mut self, accent: AccentChoice, cx: &mut Context<Self>) {
        self.preferences.accent_color = accent;
        self.preferences.apply_theme();
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn toggle_ambient_bleed(&mut self, cx: &mut Context<Self>) {
        self.preferences.ambient_bleed_enabled = !self.preferences.ambient_bleed_enabled;
        self.preferences.apply_theme();
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn set_ui_scale(&mut self, scale: UiScaleChoice, cx: &mut Context<Self>) {
        self.preferences.ui_scale = scale;
        self.preferences.apply_theme();
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn set_card_density(&mut self, density: CardDensityChoice, cx: &mut Context<Self>) {
        self.preferences.card_density = density;
        self.preferences.apply_theme();
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn set_osd_autohide_secs(&mut self, secs: u8, cx: &mut Context<Self>) {
        self.preferences.osd_autohide_secs = secs.clamp(2, 5);
        let _ = self.preferences.save();
        if self.route.is_player() {
            self.schedule_osd_autohide_timer(cx);
        }
        cx.notify();
    }

    pub fn toggle_prevent_display_sleep(&mut self, cx: &mut Context<Self>) {
        self.preferences.prevent_display_sleep = !self.preferences.prevent_display_sleep;
        let _ = self.preferences.save();
        self.sync_os_integrations();
        cx.notify();
    }

    pub fn toggle_os_media_controls(&mut self, cx: &mut Context<Self>) {
        self.preferences.enable_os_media_controls = !self.preferences.enable_os_media_controls;
        let _ = self.preferences.save();
        self.os_media
            .set_enabled(self.preferences.enable_os_media_controls);
        self.sync_os_integrations();
        cx.notify();
    }

    pub fn set_preferred_audio_lang(&mut self, lang: &str, cx: &mut Context<Self>) {
        self.preferences.preferred_audio_lang = lang.trim().to_ascii_lowercase();
        let _ = self.preferences.save();
        if self.route.is_player() && !self.player_snapshot.tracks.is_empty() {
            let snap = self.player_snapshot.clone();
            self.apply_saved_series_track_prefs(&snap);
        }
        cx.notify();
    }

    pub fn set_preferred_subtitle_lang(&mut self, lang: &str, cx: &mut Context<Self>) {
        self.preferences.preferred_subtitle_lang = lang.trim().to_ascii_lowercase();
        let _ = self.preferences.save();
        if self.route.is_player() && !self.player_snapshot.tracks.is_empty() {
            let snap = self.player_snapshot.clone();
            self.apply_saved_series_track_prefs(&snap);
        }
        cx.notify();
    }

    pub fn toggle_auto_select_subtitles(&mut self, cx: &mut Context<Self>) {
        self.preferences.auto_select_subtitles = !self.preferences.auto_select_subtitles;
        let _ = self.preferences.save();
        if self.route.is_player() && !self.player_snapshot.tracks.is_empty() {
            let snap = self.player_snapshot.clone();
            self.apply_saved_series_track_prefs(&snap);
        }
        cx.notify();
    }

    pub fn step_default_subtitle_delay(&mut self, delta_ms: i32, cx: &mut Context<Self>) {
        let next = (self.preferences.default_subtitle_delay_ms + delta_ms).clamp(-5000, 5000);
        self.preferences.default_subtitle_delay_ms = next;
        let _ = self.preferences.save();
        if let Some(mpv) = &self.mpv {
            let _ = mpv.set_subtitle_delay(f64::from(next) / 1000.0);
        }
        cx.notify();
    }

    pub fn update_subtitle_style_prefs(&mut self, cx: &mut Context<Self>) {
        let _ = self.preferences.save();
        if let Some(mpv) = &self.mpv {
            let _ = mpv.set_subtitle_style(self.preferences.to_mpv_subtitle_style());
        }
        cx.notify();
    }

    pub fn set_subtitle_color_hex(&mut self, hex: &str, cx: &mut Context<Self>) {
        self.preferences.subtitle_color_hex = hex.trim().to_ascii_uppercase();
        self.update_subtitle_style_prefs(cx);
    }

    pub fn set_subtitle_background_choice(
        &mut self,
        choice: SubtitleBackgroundChoice,
        cx: &mut Context<Self>,
    ) {
        self.preferences.subtitle_background = Some(choice);
        self.preferences.subtitle_background_pill = choice == SubtitleBackgroundChoice::Box;
        self.update_subtitle_style_prefs(cx);
    }

    pub fn set_subtitle_background_pill(&mut self, pill: bool, cx: &mut Context<Self>) {
        let choice = if pill {
            SubtitleBackgroundChoice::Box
        } else {
            SubtitleBackgroundChoice::Shadow
        };
        self.set_subtitle_background_choice(choice, cx);
    }

    pub fn set_subtitle_font_size(&mut self, size: u32, cx: &mut Context<Self>) {
        self.preferences.subtitle_font_size = size.clamp(22, 96);
        self.update_subtitle_style_prefs(cx);
    }

    pub fn step_subtitle_font_size(&mut self, delta: i32, cx: &mut Context<Self>) {
        let next = i32::try_from(self.preferences.subtitle_font_size)
            .unwrap_or(46)
            .saturating_add(delta)
            .clamp(22, 96) as u32;
        self.set_subtitle_font_size(next, cx);
    }

    pub fn set_subtitle_vertical_pos(&mut self, pos: u32, cx: &mut Context<Self>) {
        self.preferences.subtitle_vertical_pos = pos.clamp(60, 100);
        self.update_subtitle_style_prefs(cx);
    }

    pub fn step_subtitle_vertical_pos(&mut self, delta: i32, cx: &mut Context<Self>) {
        let next = i32::try_from(self.preferences.subtitle_vertical_pos)
            .unwrap_or(96)
            .saturating_add(delta)
            .clamp(60, 100) as u32;
        self.set_subtitle_vertical_pos(next, cx);
    }

    pub fn toggle_subtitle_ass_scale_override(&mut self, cx: &mut Context<Self>) {
        self.preferences.subtitle_ass_scale_override = !self.preferences.subtitle_ass_scale_override;
        self.update_subtitle_style_prefs(cx);
    }

    pub fn reset_subtitle_style_prefs(&mut self, cx: &mut Context<Self>) {
        self.preferences.reset_subtitle_style();
        self.update_subtitle_style_prefs(cx);
    }

    pub fn toggle_auto_match_native_subtitles(&mut self, cx: &mut Context<Self>) {
        self.preferences.auto_match_native_subtitles = !self.preferences.auto_match_native_subtitles;
        let _ = self.preferences.save();
        self.persist_private_integrations_draft(cx);
    }

    pub fn toggle_auto_play_next_episode(&mut self, cx: &mut Context<Self>) {
        self.preferences.auto_play_next_episode = !self.preferences.auto_play_next_episode;
        let _ = self.preferences.save();
        cx.notify();
    }

    fn normalized_preferred_sub_lang(&self) -> String {
        let pref = self.preferences.preferred_subtitle_lang.trim();
        if pref.is_empty()
            || pref.eq_ignore_ascii_case("auto")
            || pref.eq_ignore_ascii_case("off")
            || pref.eq_ignore_ascii_case("none")
        {
            "eng".to_owned()
        } else {
            pref.to_ascii_lowercase()
        }
    }

    fn clean_subtitle_provider_name(raw: &str) -> &str {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            "Native"
        } else if trimmed.eq_ignore_ascii_case("OpenSubtitles v3")
            || trimmed.eq_ignore_ascii_case("OpenSubtitles API")
            || trimmed.eq_ignore_ascii_case("OpenSubtitles.org")
        {
            "OpenSubtitles"
        } else {
            trimmed
        }
    }

    fn active_preferred_sub_lang(&self) -> String {
        self.active_series_id()
            .and_then(|series_id| self.core.get_series_track_pref(&series_id))
            .and_then(|pref| pref.sub_lang)
            .unwrap_or_else(|| self.normalized_preferred_sub_lang())
    }

    fn has_usable_embedded_preferred_subtitle(&self, tracks: &[gpui_mpv::MediaTrack]) -> bool {
        let preferred = self.active_preferred_sub_lang();
        let candidates = Self::build_track_candidates(tracks);
        has_full_embedded_subtitle_for_lang(&candidates, &preferred)
    }

    fn evaluate_automatic_subtitles_on_file_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.route.is_player() {
            return;
        }
        let series_pref = self
            .active_series_id()
            .and_then(|series_id| self.core.get_series_track_pref(&series_id));
        let raw_pref = self.preferences.preferred_subtitle_lang.trim();
        let subtitles_wanted = series_pref
            .as_ref()
            .map(|p| p.subtitles_enabled)
            .unwrap_or_else(|| {
                self.preferences.auto_select_subtitles
                    && !raw_pref.eq_ignore_ascii_case("off")
                    && !raw_pref.eq_ignore_ascii_case("none")
            });
        let preferred = self.active_preferred_sub_lang();
        let candidates = Self::build_track_candidates(&self.player_snapshot.tracks);
        let forced_cand = select_embedded_forced_track(&candidates, &preferred);
        self.secondary_forced_available = forced_cand;

        if !subtitles_wanted || series_pref.as_ref().is_some_and(|p| p.prefer_forced_only) {
            if let Some((forced_id, _)) = forced_cand {
                if let Some(mpv) = &self.mpv {
                    let _ = mpv.set_secondary_subtitle_track(None, SecondaryForcedMode::Off);
                    let _ = mpv.set_sub_forced_events_only(false);
                    let _ = mpv.select_subtitle_track(Some(forced_id));
                }
                if !matches!(
                    self.native_sub_match.status,
                    NativeSubtitleMatchStatus::LocalFile { .. }
                        | NativeSubtitleMatchStatus::Searching
                        | NativeSubtitleMatchStatus::AudioSyncing
                        | NativeSubtitleMatchStatus::AudioSynced { .. }
                ) {
                    self.native_sub_match.status = NativeSubtitleMatchStatus::EmbeddedReady;
                }
                cx.notify();
            } else if let Some(pgs_id) =
                select_embedded_image_track_for_inline_forced(&candidates, &preferred)
            {
                if let Some(mpv) = &self.mpv {
                    let _ = mpv.set_secondary_subtitle_track(None, SecondaryForcedMode::Off);
                    let _ = mpv.set_sub_forced_events_only(true);
                    let _ = mpv.select_subtitle_track(Some(pgs_id));
                }
                if !matches!(
                    self.native_sub_match.status,
                    NativeSubtitleMatchStatus::LocalFile { .. }
                        | NativeSubtitleMatchStatus::Searching
                        | NativeSubtitleMatchStatus::AudioSyncing
                        | NativeSubtitleMatchStatus::AudioSynced { .. }
                ) {
                    self.native_sub_match.status = NativeSubtitleMatchStatus::EmbeddedReady;
                }
                cx.notify();
            } else if self.preferences.auto_match_native_subtitles
                && matches!(self.native_sub_match.status, NativeSubtitleMatchStatus::Idle)
            {
                self.trigger_native_subtitle_search(
                    SubtitleSearchMode::ForcedOnly,
                    false,
                    false,
                    cx,
                );
            }
            return;
        }

        if let Some(mpv) = &self.mpv {
            let _ = mpv.set_sub_forced_events_only(false);
        }

        let quality = assess_embedded_subtitles(&candidates, &preferred);
        match quality {
            EmbeddedSubtitleQuality::UsableFull {
                full_id,
                full_is_text,
                forced_id,
                forced_is_text,
                ..
            } => {
                if let Some(mpv) = &self.mpv {
                    if !self.user_picked_subtitle {
                        let currently_full = self
                            .player_snapshot
                            .tracks
                            .iter()
                            .any(|t| t.track_type == TrackType::Sub && t.id == full_id && t.selected);
                        if !currently_full {
                            let _ = mpv.select_subtitle_track(Some(full_id));
                        }
                    }
                    if full_is_text && forced_is_text {
                        if let Some(fid) = forced_id {
                            let _ = mpv.set_secondary_subtitle_track(
                                Some(fid),
                                SecondaryForcedMode::SmartFallback,
                            );
                        } else {
                            let _ = mpv.set_secondary_subtitle_track(None, SecondaryForcedMode::Off);
                        }
                    } else {
                        let _ = mpv.set_secondary_subtitle_track(None, SecondaryForcedMode::Off);
                    }
                }
                if !matches!(
                    self.native_sub_match.status,
                    NativeSubtitleMatchStatus::LocalFile { .. }
                        | NativeSubtitleMatchStatus::Searching
                        | NativeSubtitleMatchStatus::AudioSyncing
                        | NativeSubtitleMatchStatus::AudioSynced { .. }
                ) {
                    self.native_sub_match.status = NativeSubtitleMatchStatus::EmbeddedReady;
                }
                cx.notify();
            }
            EmbeddedSubtitleQuality::ForcedOnly { .. } | EmbeddedSubtitleQuality::None => {
                if let Some(mpv) = &self.mpv {
                    let _ = mpv.set_secondary_subtitle_track(None, SecondaryForcedMode::Off);
                }
                if self.preferences.auto_match_native_subtitles
                    && matches!(self.native_sub_match.status, NativeSubtitleMatchStatus::Idle)
                {
                    self.trigger_native_subtitle_search(SubtitleSearchMode::Full, false, false, cx);
                } else {
                    self.attach_fallback_addon_subtitles_if_needed(cx);
                }
            }
        }
    }

    fn handle_untranslated_foreign_subtitle_detected(&mut self, cx: &mut Context<Self>) {
        if !self.route.is_player()
            || self.user_picked_subtitle
            || self.live_untranslated_upgrade_triggered
        {
            return;
        }
        self.live_untranslated_upgrade_triggered = true;

        let active_embedded_sub = self
            .player_snapshot
            .tracks
            .iter()
            .find(|t| t.track_type == TrackType::Sub && t.selected && !t.external)
            .map(|t| (t.id, !is_image_subtitle_codec(t.codec.as_deref())));

        if let Some((active_id, true)) = active_embedded_sub {
            if let Some((forced_id, true)) = self.secondary_forced_available {
                if forced_id != active_id {
                    if let Some(mpv) = &self.mpv {
                        let _ = mpv.set_secondary_subtitle_track(
                            Some(forced_id),
                            SecondaryForcedMode::SmartFallback,
                        );
                    }
                    cx.notify();
                    return;
                }
            }
        }

        if let Some((active_id, _)) = active_embedded_sub {
            let preferred = self.active_preferred_sub_lang();
            let candidates = Self::build_track_candidates(&self.player_snapshot.tracks);
            if let Some(alt_id) =
                select_alternative_embedded_full_track(&candidates, &preferred, active_id)
            {
                if let Some(mpv) = &self.mpv {
                    let _ = mpv.set_secondary_subtitle_track(None, SecondaryForcedMode::Off);
                    let _ = mpv.set_sub_forced_events_only(false);
                    let _ = mpv.select_subtitle_track(Some(alt_id));
                }
                cx.notify();
                return;
            }
        } else if let Some(best_translated) = self
            .native_sub_match
            .fetched_subs
            .iter()
            .find(|s| {
                s.match_tier.is_trusted_sync()
                    && s.content_profile.foreign_status == ForeignTranslationStatus::Translated
                    && s.content_profile.foreign_placeholder_count == 0
            })
            .cloned()
        {
            let label = Self::format_native_sub_label(&best_translated, None);
            if let Some(track) = self
                .player_snapshot
                .tracks
                .iter()
                .find(|t| t.track_type == TrackType::Sub && t.title.as_deref() == Some(&label))
                .map(|t| t.id)
            {
                if let Some(mpv) = &self.mpv {
                    let _ = mpv.set_secondary_subtitle_track(None, SecondaryForcedMode::Off);
                    let _ = mpv.set_sub_forced_events_only(false);
                    let _ = mpv.select_subtitle_track(Some(track));
                }
                self.push_toast(
                    ToastKind::Info,
                    "Upgraded subtitles",
                    Some("Foreign dialogue translated".to_owned()),
                    cx,
                );
                cx.notify();
                return;
            }
        }

        if self.preferences.auto_match_native_subtitles
            && !matches!(
                self.native_sub_match.status,
                NativeSubtitleMatchStatus::Searching
            )
        {
            self.trigger_native_subtitle_search(SubtitleSearchMode::Full, false, true, cx);
        }
    }

    fn format_native_sub_label(
        sub: &FetchedNativeSubtitle,
        aligned: Option<&SubtitleAlignmentResult>,
    ) -> String {
        let base = Self::native_sub_base_label(sub);
        let mut extras: Vec<String> = Vec::new();
        match sub.content_profile.foreign_status {
            ForeignTranslationStatus::Translated => extras.push("Foreign translated".to_owned()),
            ForeignTranslationStatus::MissingForeign => extras.push("Missing foreign".to_owned()),
            ForeignTranslationStatus::Unknown => {}
        }
        if let Some(align) = aligned {
            extras.push(Self::format_alignment_badge(align));
        } else if sub.match_tier == SubtitleMatchTier::ExactHash {
            extras.push("hash match".to_owned());
        }
        if extras.is_empty() {
            base
        } else {
            format!("{base} — {}", extras.join(" · "))
        }
    }

    fn format_alignment_badge(align: &SubtitleAlignmentResult) -> String {
        let secs = align.offset_ms as f64 / 1000.0;
        if (align.drift_ratio - 1.0).abs() > 0.01 {
            format!("Synced ({secs:+.1}s · FPS)")
        } else if align.offset_ms.abs() < 80 {
            "Synced (0.0s)".to_owned()
        } else {
            format!("Synced ({secs:+.1}s)")
        }
    }

    fn native_sub_base_label(sub: &FetchedNativeSubtitle) -> String {
        let base_lang = human_language_name(&sub.lang);
        let is_sdh = sub.hearing_impaired
            || sub.title.to_ascii_uppercase().contains("SDH")
            || sub.release_name.to_ascii_uppercase().contains("SDH");
        let mut lang_title = base_lang;
        if sub.forced {
            lang_title.push_str(" Forced");
        } else if is_sdh {
            lang_title.push_str(" SDH");
        }
        let provider = Self::clean_subtitle_provider_name(&sub.provider);
        format!("{lang_title} — {provider}")
    }

    pub fn trigger_native_subtitle_auto_match(&mut self, force_rematch: bool, cx: &mut Context<Self>) {
        self.trigger_native_subtitle_search(SubtitleSearchMode::Full, force_rematch, false, cx);
    }

    pub fn trigger_native_subtitle_search(
        &mut self,
        search_mode: SubtitleSearchMode,
        force_rematch: bool,
        is_live_placeholder_rescue: bool,
        cx: &mut Context<Self>,
    ) {
        if !force_rematch && !self.preferences.auto_match_native_subtitles {
            return;
        }
        if force_rematch {
            let mut cfg = self.core.private_integrations();
            if !cfg.native_subtitles_enabled {
                cfg.native_subtitles_enabled = true;
                let _ = self.core.update_private_integrations(cfg);
            }
        }
        let Some(session) = self.playback_session.as_ref() else {
            return;
        };

        let gen_id = session.generation;
        let imdb_id = session.meta_id.clone();
        let media_type = session.type_name.clone();
        let (season, episode) = parse_season_episode_from_video_id(Some(&session.video_id))
            .map(|(s, e)| (Some(s), Some(e)))
            .unwrap_or((None, None));
        let resolved_url = session.resolved_url.clone();
        let active_cand = session.active_candidate();
        let hint_video_hash = active_cand
            .and_then(|c| c.stream.behavior_hints.video_hash.clone())
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| s.len() == 16);
        let video_byte_size = active_cand.and_then(|c| c.stream.behavior_hints.video_size);
        let video_filename = active_cand
            .and_then(|c| {
                c.stream
                    .behavior_hints
                    .filename
                    .clone()
                    .or_else(|| Some(c.parsed.display_title.clone()))
                    .or_else(|| c.stream.name.clone())
            })
            .or_else(|| {
                resolved_url.as_deref().and_then(|u| {
                    Url::parse(u).ok().and_then(|parsed| {
                        parsed
                            .path_segments()
                            .and_then(|mut segs| segs.next_back())
                            .map(str::to_owned)
                            .filter(|s| !s.is_empty())
                    })
                })
            });
        let preferred_lang = self.active_preferred_sub_lang();
        let local_target_to_align = if force_rematch
            && matches!(
                self.native_sub_match.status,
                NativeSubtitleMatchStatus::LocalFile { .. }
            ) {
            self.native_sub_match.active_local_sub_path.clone()
        } else {
            None
        };
        let align_local_mode = local_target_to_align.is_some();

        let previous_status = self.native_sub_match.status.clone();
        let keep_embedded_label_quiet =
            !force_rematch && matches!(previous_status, NativeSubtitleMatchStatus::EmbeddedReady);
        if !keep_embedded_label_quiet {
            self.native_sub_match.status = NativeSubtitleMatchStatus::Searching;
        }
        if force_rematch {
            self.native_sub_match.last_alignment_badge = None;
        }
        cx.notify();

        let core = self.core.clone();
        let tokio_handle = self.tokio_handle.clone();
        let playhead = self.player_snapshot.time_pos.max(0.0);
        let active_embedded_text_sub_ordinal = if is_live_placeholder_rescue {
            self.player_snapshot
                .tracks
                .iter()
                .filter(|t| t.track_type == TrackType::Sub && !t.external)
                .enumerate()
                .find_map(|(ordinal, t)| {
                    (t.selected && !is_image_subtitle_codec(t.codec.as_deref())).then_some(ordinal)
                })
        } else {
            None
        };
        let max_fetch = match search_mode {
            SubtitleSearchMode::Full => 4,
            SubtitleSearchMode::ForcedOnly => 2,
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let outcome = tokio_handle
                .spawn(async move {
                    let hash_res = if let Some(url_str) = resolved_url.as_deref() {
                        if !is_loopback_p2p_torrent_url(url_str) {
                            if let Ok(parsed) = Url::parse(url_str) {
                                if parsed.scheme() == "http" || parsed.scheme() == "https" {
                                    if let Some(client) = shared_opensubhash_http_client() {
                                        tokio::time::timeout(
                                            Duration::from_millis(1200),
                                            compute_opensubhash_http(client, &parsed),
                                        )
                                        .await
                                        .ok()
                                        .and_then(Result::ok)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    let query = NativeSubtitleQuery {
                        imdb_id,
                        media_type,
                        season,
                        episode,
                        opensubhash: hash_res.as_ref().map(|r| r.hash.clone()).or(hint_video_hash),
                        video_byte_size: hash_res.as_ref().map(|r| r.size).or(video_byte_size),
                        video_filename,
                        preferred_langs: vec![preferred_lang],
                        search_mode,
                    };
                    let mut fetched = core.search_and_fetch_native_subtitles(query, max_fetch).await;
                    let exact_ref = fetched
                        .iter()
                        .find(|s| s.match_tier == SubtitleMatchTier::ExactHash)
                        .cloned();

                    let mut auto_aligned = if let (Some(top), Some(ref_sub)) =
                        (fetched.first(), exact_ref.as_ref())
                    {
                        if let Some(local_path) = local_target_to_align.as_ref() {
                            core.align_subtitle_to_reference(local_path, &ref_sub.local_path)
                                .ok()
                        } else if top.match_tier != SubtitleMatchTier::ExactHash
                            && top.local_path != ref_sub.local_path
                        {
                            core.align_subtitle_to_reference(&top.local_path, &ref_sub.local_path)
                                .ok()
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    if is_live_placeholder_rescue
                        && auto_aligned.is_none()
                        && fetched
                            .first()
                            .is_some_and(|top| top.match_tier != SubtitleMatchTier::ExactHash)
                    {
                        if let (Some(sub_ordinal), Some(stream_url)) = (
                            active_embedded_text_sub_ordinal,
                            resolved_url
                                .as_deref()
                                .filter(|u| {
                                    !is_loopback_p2p_torrent_url(u)
                                        && Url::parse(u).ok().is_some_and(|p| {
                                            p.scheme() == "http" || p.scheme() == "https"
                                        })
                                })
                                .map(str::to_owned),
                        ) {
                            let mut align_candidates: Vec<(usize, PathBuf)> = Vec::new();
                            for (idx, sub) in fetched.iter().enumerate() {
                                if !sub.forced
                                    && sub.content_profile.foreign_placeholder_count == 0
                                    && sub.content_profile.foreign_status
                                        == ForeignTranslationStatus::Translated
                                {
                                    align_candidates.push((idx, sub.local_path.clone()));
                                }
                            }
                            for (idx, sub) in fetched.iter().enumerate() {
                                if !sub.forced
                                    && sub.content_profile.foreign_placeholder_count == 0
                                    && sub.content_profile.foreign_status
                                        != ForeignTranslationStatus::MissingForeign
                                    && !align_candidates.iter().any(|(i, _)| *i == idx)
                                {
                                    align_candidates.push((idx, sub.local_path.clone()));
                                }
                            }
                            if !align_candidates.is_empty() {
                                let window_start = (playhead - 45.0).max(0.0);
                                let embedded_aligned = tokio::task::spawn_blocking(move || {
                                    let ref_srt = extract_embedded_text_subtitle_window(
                                        &stream_url,
                                        sub_ordinal,
                                        window_start,
                                        120.0,
                                    )
                                    .ok()?;
                                    if ref_srt.trim().is_empty() {
                                        return None;
                                    }
                                    for (cand_idx, cand_path) in align_candidates {
                                        let ext = cand_path
                                            .extension()
                                            .and_then(|e| e.to_str())
                                            .unwrap_or("srt");
                                        let stem = cand_path
                                            .file_stem()
                                            .and_then(|s| s.to_str())
                                            .unwrap_or("sub");
                                        let out_path = cand_path
                                            .parent()
                                            .unwrap_or_else(|| std::path::Path::new("."))
                                            .join(format!("{stem}_embref_aligned.{ext}"));
                                        if let Ok(alignment) =
                                            SubtitleSyncAligner::align_subtitle_file_to_reference_text(
                                                &cand_path,
                                                &ref_srt,
                                                &out_path,
                                            )
                                        {
                                            if alignment.matched_onsets >= 10
                                                && alignment.confidence >= 0.40
                                            {
                                                return Some((cand_idx, out_path, alignment));
                                            }
                                        }
                                    }
                                    None
                                })
                                .await
                                .ok()
                                .flatten();

                                if let Some((cand_idx, out_path, alignment)) = embedded_aligned {
                                    if cand_idx > 0 && cand_idx < fetched.len() {
                                        let chosen = fetched.remove(cand_idx);
                                        fetched.insert(0, chosen);
                                    }
                                    auto_aligned = Some((out_path, alignment));
                                }
                            }
                        }
                    }

                    (fetched, exact_ref.map(|r| r.local_path), auto_aligned)
                })
                .await;

            let _ = this.update(cx, |shell, cx| {
                let still_active = shell.route.is_player()
                    && shell
                        .playback_session
                        .as_ref()
                        .is_some_and(|s| s.generation == gen_id);
                if !still_active {
                    return;
                }

                let Ok((fetched, exact_ref_path, auto_aligned)) = outcome else {
                    if !keep_embedded_label_quiet {
                        shell.native_sub_match.status = NativeSubtitleMatchStatus::Failed(
                            "Subtitle search task failed".to_owned(),
                        );
                    }
                    cx.notify();
                    return;
                };

                shell.native_sub_match.reference_sub_path = exact_ref_path;
                shell.native_sub_match.fetched_subs = fetched.clone();

                if fetched.is_empty() {
                    if keep_embedded_label_quiet {
                        shell.native_sub_match.status = NativeSubtitleMatchStatus::EmbeddedReady;
                    } else if !matches!(
                        shell.native_sub_match.status,
                        NativeSubtitleMatchStatus::LocalFile { .. }
                    ) {
                        shell.native_sub_match.status = if search_mode == SubtitleSearchMode::ForcedOnly {
                            NativeSubtitleMatchStatus::Idle
                        } else {
                            NativeSubtitleMatchStatus::NoMatches
                        };
                        if search_mode == SubtitleSearchMode::Full {
                            shell.attach_fallback_addon_subtitles_if_needed(cx);
                        }
                    } else if shell.native_sub_match.file_loaded_ready
                        && shell.should_auto_audio_sync_now()
                    {
                        shell.spawn_automatic_audio_subtitle_sync(false, cx);
                    }
                    if force_rematch {
                        shell.push_toast(
                            ToastKind::Info,
                            "No native subtitles found",
                            Some("Use Load file or drop an .srt/.ass file onto the player".to_owned()),
                            cx,
                        );
                    }
                    cx.notify();
                    return;
                }

                let preferred = shell.active_preferred_sub_lang();
                let track_cands = Self::build_track_candidates(&shell.player_snapshot.tracks);
                let embedded_quality = assess_embedded_subtitles(&track_cands, &preferred);
                let top_sub = &fetched[0];
                let aligned_to_reference = auto_aligned.is_some();
                let should_select_top = should_hot_swap_to_online(
                    embedded_quality,
                    top_sub,
                    search_mode,
                    force_rematch,
                    shell.user_picked_subtitle,
                    is_live_placeholder_rescue,
                    aligned_to_reference,
                ) && (search_mode == SubtitleSearchMode::ForcedOnly
                    || shell.preferences.auto_select_subtitles
                    || force_rematch);
                let had_embedded_before = !matches!(
                    embedded_quality,
                    EmbeddedSubtitleQuality::None | EmbeddedSubtitleQuality::ForcedOnly { .. }
                );
                let mut chosen_title: Option<String> = None;

                if let Some((aligned_path, alignment)) = auto_aligned {
                    let badge = Self::format_alignment_badge(&alignment);
                    let label = Self::format_native_sub_label(top_sub, Some(&alignment));
                    if let Some(mpv) = &shell.mpv {
                        if should_select_top {
                            let _ = mpv.set_sub_forced_events_only(false);
                            let _ =
                                mpv.set_secondary_subtitle_track(None, SecondaryForcedMode::Off);
                        }
                        let _ = mpv.add_local_subtitle_file_selected(
                            &aligned_path,
                            Some(&label),
                            Some(&top_sub.lang),
                            should_select_top,
                        );
                        if should_select_top {
                            let _ = mpv.set_subtitle_delay(0.0);
                        }
                    }
                    if should_select_top {
                        shell.native_sub_match.active_local_sub_path = Some(aligned_path);
                        shell.native_sub_match.synced_track_title = Some(label.clone());
                        shell.native_sub_match.last_alignment_badge = Some(badge.clone());
                        if !align_local_mode {
                            shell.native_sub_match.status = NativeSubtitleMatchStatus::Matched {
                                tier: top_sub.match_tier,
                                elapsed_ms: top_sub.lookup_elapsed_ms,
                            };
                        }
                    }
                    chosen_title = Some(label);
                    if force_rematch {
                        shell.push_toast(ToastKind::Success, "Subtitles auto-synced", Some(badge), cx);
                    }
                }

                for (idx, sub) in fetched.iter().enumerate() {
                    let label = Self::format_native_sub_label(sub, None);
                    if idx == 0 && chosen_title.is_some() {
                        continue;
                    }
                    let select_this = should_select_top && idx == 0;
                    if let Some(mpv) = &shell.mpv {
                        if select_this {
                            let _ = mpv.set_sub_forced_events_only(false);
                            let _ =
                                mpv.set_secondary_subtitle_track(None, SecondaryForcedMode::Off);
                        }
                        let _ = mpv.add_local_subtitle_file_selected(
                            &sub.local_path,
                            Some(&label),
                            Some(&sub.lang),
                            select_this,
                        );
                        if select_this {
                            let _ = mpv.set_subtitle_delay(0.0);
                        }
                    }
                    if idx == 0 && chosen_title.is_none() {
                        if should_select_top {
                            shell.native_sub_match.active_local_sub_path =
                                Some(sub.local_path.clone());
                            shell.native_sub_match.status = NativeSubtitleMatchStatus::Matched {
                                tier: sub.match_tier,
                                elapsed_ms: sub.lookup_elapsed_ms,
                            };
                        }
                        chosen_title = Some(label);
                    }
                }

                if should_select_top {
                    if !align_local_mode {
                        shell.native_sub_match.local_source_sub_path = None;
                    }
                    if let Some(mpv) = &shell.mpv {
                        let _ = mpv.set_sub_forced_events_only(false);
                        let _ = mpv.set_secondary_subtitle_track(None, SecondaryForcedMode::Off);
                    }
                    if let Some(title) = chosen_title {
                        if let Some(track) = shell.player_snapshot.tracks.iter().find(|t| {
                            t.track_type == TrackType::Sub && t.title.as_deref() == Some(&title)
                        }) {
                            if let Some(mpv) = &shell.mpv {
                                let _ = mpv.select_subtitle_track(Some(track.id));
                            }
                            shell.pending_auto_select_sub_title = None;
                        } else {
                            shell.pending_auto_select_sub_title = Some(title);
                        }
                    }
                    if !force_rematch && had_embedded_before && search_mode == SubtitleSearchMode::Full {
                        let reason = if top_sub.content_profile.foreign_status
                            == ForeignTranslationStatus::Translated
                        {
                            "Foreign dialogue translated"
                        } else {
                            "Synced clean replacement"
                        };
                        shell.push_toast(
                            ToastKind::Info,
                            "Upgraded subtitles",
                            Some(reason.to_owned()),
                            cx,
                        );
                    }
                } else if keep_embedded_label_quiet {
                    shell.native_sub_match.status = NativeSubtitleMatchStatus::EmbeddedReady;
                }

                if force_rematch && !align_local_mode {
                    shell.push_toast(
                        ToastKind::Success,
                        format!("Matched {} subtitles", fetched.len()),
                        shell.native_sub_match.status_badge(),
                        cx,
                    );
                }

                if should_select_top
                    && !aligned_to_reference
                    && shell.native_sub_match.reference_sub_path.is_none()
                    && shell.native_sub_match.file_loaded_ready
                    && shell.should_auto_audio_sync_now()
                {
                    shell.spawn_automatic_audio_subtitle_sync(false, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn should_auto_audio_sync_now(&self) -> bool {
        if !self.preferences.auto_match_native_subtitles
            || self.native_sub_match.audio_sync_in_flight
            || self.native_sub_match.last_alignment_badge.is_some()
            || matches!(
                self.native_sub_match.status,
                NativeSubtitleMatchStatus::AudioSynced { .. }
                    | NativeSubtitleMatchStatus::Searching
                    | NativeSubtitleMatchStatus::EmbeddedReady
            )
        {
            return false;
        }
        let is_local_file = matches!(
            self.native_sub_match.status,
            NativeSubtitleMatchStatus::LocalFile { .. }
        );
        if is_local_file {
            return self.native_sub_match.active_local_sub_path.is_some();
        }
        if self.native_sub_match.reference_sub_path.is_some()
            || self.native_sub_match.fetched_subs.is_empty()
            || self.native_sub_match.active_local_sub_path.is_none()
        {
            return false;
        }
        let has_selected_embedded = self
            .player_snapshot
            .tracks
            .iter()
            .any(|t| t.track_type == TrackType::Sub && !t.external && t.selected);
        !(has_selected_embedded && self.pending_auto_select_sub_title.is_none())
    }

    fn spawn_automatic_audio_subtitle_sync(
        &mut self,
        user_triggered: bool,
        cx: &mut Context<Self>,
    ) {
        if self.native_sub_match.audio_sync_in_flight {
            return;
        }
        let Some(session) = self.playback_session.as_ref() else {
            return;
        };
        let Some(stream_url) = session.resolved_url.clone() else {
            if user_triggered {
                self.push_toast(
                    ToastKind::Info,
                    "Waiting for media stream before audio sync",
                    None,
                    cx,
                );
            }
            return;
        };
        let is_p2p_loopback = is_loopback_p2p_torrent_url(&stream_url);
        if is_p2p_loopback
            && !user_triggered
            && self.player_snapshot.time_pos < 35.0
            && self.player_snapshot.demuxer_cache_duration < 25.0
        {
            // Wait until dialogue and P2P piece buffer are established before running audio probe.
            return;
        }
        let gen_id = session.generation;
        let preferred_lang = self.normalized_preferred_sub_lang();
        let local_src = self.native_sub_match.local_source_sub_path.clone().or_else(|| {
            matches!(
                self.native_sub_match.status,
                NativeSubtitleMatchStatus::LocalFile { .. }
            )
            .then(|| self.native_sub_match.active_local_sub_path.clone())
            .flatten()
        });
        let is_local_mode = local_src.is_some();

        let mut candidates: Vec<(PathBuf, String, String, Option<String>)> = Vec::new();
        if let Some(path) = local_src {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Local")
                .to_owned();
            candidates.push((
                path,
                preferred_lang,
                format!("{stem} — Local"),
                Some(format!("{stem} — Local file")),
            ));
        } else {
            let mut lang_matched: Vec<&FetchedNativeSubtitle> = self
                .native_sub_match
                .fetched_subs
                .iter()
                .filter(|s| subtitle_lang_matches(&s.lang, &preferred_lang))
                .collect();
            if lang_matched.is_empty() {
                lang_matched = self.native_sub_match.fetched_subs.iter().collect();
            }
            for sub in lang_matched.into_iter().take(3) {
                candidates.push((
                    sub.local_path.clone(),
                    sub.lang.clone(),
                    Self::native_sub_base_label(sub),
                    Some(Self::format_native_sub_label(sub, None)),
                ));
            }
        }

        if candidates.is_empty() {
            if user_triggered {
                self.trigger_native_subtitle_auto_match(true, cx);
            }
            return;
        }

        let playhead = self.player_snapshot.time_pos.max(0.0);
        let duration = self.player_snapshot.duration.max(0.0);

        self.native_sub_match.audio_sync_in_flight = true;
        self.native_sub_match.status = NativeSubtitleMatchStatus::AudioSyncing;
        cx.notify();

        let core = self.core.clone();
        let tokio_handle = self.tokio_handle.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let task_res = tokio_handle
                .spawn(async move {
                    tokio::task::spawn_blocking(move || {
                        let cues = SubtitleSyncAligner::parse_spoken_dialogue_file(
                            &candidates[0].0,
                        )
                        .unwrap_or_default();
                        let probe_windows = if is_p2p_loopback {
                            vec![((playhead - 25.0).max(5.0), 50.0)]
                        } else if user_triggered && playhead > 60.0 {
                            let primary_start = (playhead - 30.0).max(5.0);
                            let mut w = vec![(primary_start, 65.0)];
                            if let Some((s2, d2)) =
                                SubtitleSyncAligner::select_dialogue_probe_windows(
                                    &cues, 0.0, duration,
                                )
                                .into_iter()
                                .filter(|&(s2, _)| (s2 - primary_start).abs() >= 360.0)
                                .max_by(|a, b| {
                                    (a.0 - primary_start)
                                        .abs()
                                        .total_cmp(&(b.0 - primary_start).abs())
                                })
                            {
                                w.push((s2, d2));
                            }
                            w
                        } else {
                            SubtitleSyncAligner::select_dialogue_probe_windows(
                                &cues, playhead, duration,
                            )
                        };

                        let candidate_paths: Vec<PathBuf> =
                            candidates.iter().map(|c| c.0.clone()).collect();
                        let mut windows = Vec::new();
                        for (start, dur) in probe_windows.into_iter().take(2) {
                            if let Ok(pcm) = extract_audio_pcm_s16le_8khz(&stream_url, start, dur) {
                                let w = AudioSpeechWindow::from_pcm_8khz(start, &pcm);
                                if !w.vad.is_empty() {
                                    windows.push(w);
                                }
                            }
                        }
                        let fp = AudioSpeechFingerprint { windows };

                        if fp.is_empty() {
                            return (
                                Err("Could not extract dialogue audio from stream".to_owned()),
                                candidates,
                            );
                        }

                        let aligned = core.align_best_subtitle_to_audio(&candidate_paths, &fp);

                        (aligned, candidates)
                    })
                    .await
                })
                .await;

            let _ = this.update(cx, |shell, cx| {
                shell.native_sub_match.audio_sync_in_flight = false;
                let still_active = shell.route.is_player()
                    && shell
                        .playback_session
                        .as_ref()
                        .is_some_and(|s| s.generation == gen_id);
                if !still_active {
                    return;
                }

                let Ok(Ok((aligned_res, candidates))) = task_res else {
                    if user_triggered {
                        shell.push_toast(
                            ToastKind::Warning,
                            "Audio sync task interrupted",
                            None,
                            cx,
                        );
                    }
                    cx.notify();
                    return;
                };

                match aligned_res {
                    Ok((best_idx, aligned_path, alignment)) => {
                        if !user_triggered
                            && (alignment.confidence < 0.24 || alignment.matched_onsets < 5)
                        {
                            if let Some(first_sub) = shell.native_sub_match.fetched_subs.first() {
                                shell.native_sub_match.status = NativeSubtitleMatchStatus::Matched {
                                    tier: first_sub.match_tier,
                                    elapsed_ms: first_sub.lookup_elapsed_ms,
                                };
                            } else if is_local_mode {
                                let filename = candidates
                                    .first()
                                    .and_then(|c| c.0.file_name())
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("Local")
                                    .to_owned();
                                shell.native_sub_match.status =
                                    NativeSubtitleMatchStatus::LocalFile { filename };
                            }
                            cx.notify();
                            return;
                        }

                        let badge = Self::format_alignment_badge(&alignment);
                        let cand = &candidates[best_idx];
                        let synced_label = format!("{} · {badge}", cand.2);
                        let should_select = user_triggered
                            || !shell.user_picked_subtitle
                            || is_local_mode
                            || matches!(
                                shell.native_sub_match.status,
                                NativeSubtitleMatchStatus::AudioSyncing
                            );

                        if let Some(mpv) = &shell.mpv {
                            let prev_synced = shell.native_sub_match.synced_track_title.as_deref();
                            let unaligned = cand.3.as_deref();
                            for tr in &shell.player_snapshot.tracks {
                                if tr.track_type == TrackType::Sub
                                    && tr.external
                                    && (tr.title.as_deref() == prev_synced
                                        || (unaligned.is_some()
                                            && tr.title.as_deref() == unaligned))
                                {
                                    let _ = mpv.remove_subtitle_track(tr.id);
                                }
                            }
                            if should_select {
                                let _ = mpv.set_sub_forced_events_only(false);
                                let _ = mpv.set_secondary_subtitle_track(
                                    None,
                                    SecondaryForcedMode::Off,
                                );
                            }
                            let _ = mpv.add_local_subtitle_file_selected(
                                &aligned_path,
                                Some(&synced_label),
                                Some(&cand.1),
                                should_select,
                            );
                            if should_select {
                                let _ = mpv.set_subtitle_delay(0.0);
                            }
                        }

                        if should_select {
                            shell.pending_auto_select_sub_title = Some(synced_label.clone());
                        }
                        shell.native_sub_match.synced_track_title = Some(synced_label);
                        shell.native_sub_match.active_local_sub_path = Some(aligned_path);
                        shell.native_sub_match.last_alignment_badge = Some(badge.clone());
                        shell.native_sub_match.status = NativeSubtitleMatchStatus::AudioSynced {
                            offset_ms: alignment.offset_ms,
                            fps_adjusted: (alignment.drift_ratio - 1.0).abs() > 0.01,
                        };
                        if user_triggered {
                            shell.push_toast(
                                ToastKind::Success,
                                "Subtitles synced to audio",
                                Some(badge),
                                cx,
                            );
                        }
                    }
                    Err(err) => {
                        if let Some(first_sub) = shell.native_sub_match.fetched_subs.first() {
                            shell.native_sub_match.status = NativeSubtitleMatchStatus::Matched {
                                tier: first_sub.match_tier,
                                elapsed_ms: first_sub.lookup_elapsed_ms,
                            };
                        } else {
                            shell.native_sub_match.status =
                                NativeSubtitleMatchStatus::NoMatches;
                        }
                        if user_triggered {
                            shell.push_toast(
                                ToastKind::Warning,
                                "Could not sync subtitles to audio",
                                Some(err),
                                cx,
                            );
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn auto_sync_active_subtitle(&mut self, cx: &mut Context<Self>) {
        self.spawn_automatic_audio_subtitle_sync(true, cx);
    }

    pub fn prompt_and_load_local_subtitle(&mut self, cx: &mut Context<Self>) {
        if !self.route.is_player() {
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select Subtitle File".into()),
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if !paths.is_empty() {
                    let _ = this.update(cx, |shell, cx| {
                        shell.handle_dropped_subtitle_paths(&paths, cx);
                    });
                }
            }
        })
        .detach();
    }

    pub fn handle_dropped_subtitle_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        if !self.route.is_player() || paths.is_empty() {
            return;
        }

        let mut loaded_count = 0usize;
        let mut last_loaded: Option<(PathBuf, String, String)> = None;
        let mut last_err: Option<String> = None;

        for path in paths {
            match gpui_mpv::validate_local_subtitle_path(path) {
                Ok(canonical) => {
                    let filename = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Local Subtitle")
                        .to_owned();
                    let stem = path
                        .file_stem()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Local")
                        .to_owned();
                    let label = format!("{stem} — Local file");
                    if let Some(mpv) = &self.mpv {
                        if let Some(prev_title) =
                            self.native_sub_match.synced_track_title.as_deref()
                        {
                            for tr in &self.player_snapshot.tracks {
                                if tr.track_type == TrackType::Sub
                                    && tr.external
                                    && tr.title.as_deref() == Some(prev_title)
                                {
                                    let _ = mpv.remove_subtitle_track(tr.id);
                                }
                            }
                        }
                        let _ = mpv.set_sub_forced_events_only(false);
                        let _ = mpv.set_secondary_subtitle_track(None, SecondaryForcedMode::Off);
                        if let Err(err) = mpv.add_local_subtitle_file_selected(
                            &canonical,
                            Some(&label),
                            None,
                            true,
                        ) {
                            last_err = Some(err.to_string());
                            continue;
                        }
                        let _ = mpv.set_subtitle_delay(0.0);
                    }
                    loaded_count += 1;
                    last_loaded = Some((canonical, filename, label.clone()));
                    self.pending_auto_select_sub_title = Some(label);
                }
                Err(err) => {
                    last_err = Some(err.to_string());
                }
            }
        }

        if let Some((canonical, filename, label)) = last_loaded {
            self.user_picked_subtitle = true;
            self.native_sub_match.local_source_sub_path = Some(canonical.clone());
            self.native_sub_match.active_local_sub_path = Some(canonical);
            self.native_sub_match.synced_track_title = Some(label);
            self.native_sub_match.status = NativeSubtitleMatchStatus::LocalFile {
                filename: filename.clone(),
            };
            self.native_sub_match.last_alignment_badge = None;
            let title = if loaded_count == 1 {
                "Loaded local subtitle".to_owned()
            } else {
                format!("Loaded {loaded_count} local subtitles")
            };
            self.push_toast(ToastKind::Success, title, Some(filename), cx);
            if self.native_sub_match.file_loaded_ready
                && self.preferences.auto_match_native_subtitles
            {
                self.spawn_automatic_audio_subtitle_sync(false, cx);
            }
        } else {
            self.push_toast(
                ToastKind::Warning,
                "Unsupported subtitle file",
                last_err.or_else(|| {
                    Some("Select or drop a .srt, .ass, .ssa, .vtt, or .sub file".to_owned())
                }),
                cx,
            );
        }
        cx.notify();
    }

    pub fn set_local_debrid_provider(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.local_debrid_provider_idx = idx.min(3);
        self.debrid_verify_status = None;
        self.persist_private_integrations_draft(cx);
    }

    pub fn toggle_local_debrid_direct_unrestrict(&mut self, cx: &mut Context<Self>) {
        self.local_debrid_direct_unrestrict = !self.local_debrid_direct_unrestrict;
        self.persist_private_integrations_draft(cx);
    }

    pub fn toggle_local_debrid_cached_only(&mut self, cx: &mut Context<Self>) {
        self.local_debrid_cached_only = !self.local_debrid_cached_only;
        self.persist_private_integrations_draft(cx);
    }

    pub fn verify_local_debrid_clicked(&mut self, provider_idx: usize, cx: &mut Context<Self>) {
        self.persist_private_integrations_draft(cx);
        let provider = idx_to_local_debrid_provider(provider_idx);
        let (label, override_key, has_key) = match provider {
            LocalDebridProvider::RealDebrid => (
                provider.label(),
                (!self.real_debrid_key_input.trim().is_empty())
                    .then(|| self.real_debrid_key_input.trim().to_owned()),
                !self.real_debrid_key_input.trim().is_empty() || self.real_debrid_key_configured,
            ),
            LocalDebridProvider::TorBox => (
                provider.label(),
                (!self.torbox_key_input.trim().is_empty())
                    .then(|| self.torbox_key_input.trim().to_owned()),
                !self.torbox_key_input.trim().is_empty() || self.torbox_key_configured,
            ),
            LocalDebridProvider::AllDebrid => (
                provider.label(),
                (!self.alldebrid_key_input.trim().is_empty())
                    .then(|| self.alldebrid_key_input.trim().to_owned()),
                !self.alldebrid_key_input.trim().is_empty() || self.alldebrid_key_configured,
            ),
            LocalDebridProvider::None => ("Local Debrid", None, false),
        };
        if !provider.is_enabled() || !has_key {
            let msg = format!("Enter a {label} API key before verifying.");
            self.debrid_verify_status = Some(msg.clone());
            self.push_toast(ToastKind::Warning, msg, None, cx);
            return;
        }

        self.debrid_verify_status = Some(format!("Verifying {label} account..."));
        cx.notify();

        let core = self.core.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = core.verify_local_debrid(provider, override_key).await;
            let _ = this.update(cx, |shell, cx| match res {
                Ok(status) => {
                    shell.last_debrid_latency_ms = Some(status.latency_ms);
                    let tier = if status.premium { "Premium" } else { "Free" };
                    let exp = status
                        .expiration
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .map(|s| format!(" · exp {s}"))
                        .unwrap_or_default();
                    let summary = format!(
                        "{} ({tier}) · {} · {} ms{exp}",
                        status.username,
                        status.provider.label(),
                        status.latency_ms
                    );
                    shell.debrid_verify_status = Some(summary.clone());
                    shell.push_toast(
                        ToastKind::Success,
                        format!("{} verified", status.provider.label()),
                        Some(summary),
                        cx,
                    );
                }
                Err(err) => {
                    shell.debrid_verify_status = Some(format!("Verification failed: {err}"));
                    shell.push_toast(
                        ToastKind::Error,
                        format!("{label} verification failed"),
                        Some(err),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    pub fn clear_local_debrid_key(&mut self, provider_idx: usize, cx: &mut Context<Self>) {
        match provider_idx {
            1 => {
                self.real_debrid_key_input.clear();
                self.real_debrid_key_configured = false;
                self.input_cursors.remove(&FocusedInput::RealDebridApiKey);
                self.input_selected_all
                    .remove(&FocusedInput::RealDebridApiKey);
            }
            2 => {
                self.torbox_key_input.clear();
                self.torbox_key_configured = false;
                self.input_cursors.remove(&FocusedInput::TorBoxApiKey);
                self.input_selected_all.remove(&FocusedInput::TorBoxApiKey);
            }
            3 => {
                self.alldebrid_key_input.clear();
                self.alldebrid_key_configured = false;
                self.input_cursors.remove(&FocusedInput::AllDebridApiKey);
                self.input_selected_all
                    .remove(&FocusedInput::AllDebridApiKey);
            }
            _ => {}
        }
        self.debrid_verify_status = None;
        self.persist_private_integrations_draft(cx);
    }

    pub fn set_prowlarr_mode(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.prowlarr_mode_idx = idx.min(2);
        self.persist_private_integrations_draft(cx);
    }

    pub fn step_prowlarr_timeout(&mut self, delta_ms: i64, cx: &mut Context<Self>) {
        let next = (self.prowlarr_timeout_ms as i64 + delta_ms).clamp(1000, 15000) as u64;
        self.prowlarr_timeout_ms = next;
        self.persist_private_integrations_draft(cx);
    }

    pub fn test_prowlarr_clicked(&mut self, cx: &mut Context<Self>) {
        self.persist_private_integrations_draft(cx);
        let url = self.prowlarr_url_input.trim().to_owned();
        if url.is_empty() {
            let msg = "Enter a local Prowlarr or Torznab URL first.".to_owned();
            self.prowlarr_test_status = Some(msg.clone());
            self.push_toast(ToastKind::Warning, msg, None, cx);
            return;
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            let msg = "Prowlarr URL must start with http:// or https://".to_owned();
            self.prowlarr_test_status = Some(msg.clone());
            self.push_toast(ToastKind::Warning, msg, None, cx);
            return;
        }

        let override_key = (!self.prowlarr_key_input.trim().is_empty())
            .then(|| self.prowlarr_key_input.trim().to_owned());
        self.prowlarr_test_status = Some(format!("Testing {url}..."));
        cx.notify();

        let core = self.core.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = core.test_prowlarr(Some(url.clone()), override_key).await;
            let _ = this.update(cx, |shell, cx| match res {
                Ok(test_res) => {
                    let summary = format!(
                        "Connected ({}) · {} items · {} ms",
                        test_res.mode_detected, test_res.item_count, test_res.latency_ms
                    );
                    shell.prowlarr_test_status = Some(summary.clone());
                    shell.push_toast(
                        ToastKind::Success,
                        "Prowlarr / Torznab connected",
                        Some(summary),
                        cx,
                    );
                }
                Err(err) => {
                    shell.prowlarr_test_status = Some(format!("Connection failed: {err}"));
                    shell.push_toast(
                        ToastKind::Error,
                        "Prowlarr test failed",
                        Some(err),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    pub fn clear_prowlarr_key(&mut self, cx: &mut Context<Self>) {
        self.prowlarr_key_input.clear();
        self.prowlarr_key_configured = false;
        self.input_cursors.remove(&FocusedInput::ProwlarrApiKey);
        self.input_selected_all
            .remove(&FocusedInput::ProwlarrApiKey);
        self.prowlarr_test_status = None;
        self.persist_private_integrations_draft(cx);
    }

    pub fn clear_opensubtitles_key(&mut self, cx: &mut Context<Self>) {
        self.opensubtitles_key_input.clear();
        self.opensubtitles_key_configured = false;
        self.input_cursors
            .remove(&FocusedInput::SettingsOpenSubtitlesKey);
        self.input_selected_all
            .remove(&FocusedInput::SettingsOpenSubtitlesKey);
        self.persist_private_integrations_draft(cx);
    }

    pub fn clear_subdl_key(&mut self, cx: &mut Context<Self>) {
        self.subdl_key_input.clear();
        self.subdl_key_configured = false;
        self.input_cursors.remove(&FocusedInput::SettingsSubdlKey);
        self.input_selected_all
            .remove(&FocusedInput::SettingsSubdlKey);
        self.persist_private_integrations_draft(cx);
    }

    pub fn clear_tmdb_key(&mut self, cx: &mut Context<Self>) {
        self.tmdb_key_input.clear();
        self.tmdb_key_configured = false;
        self.input_cursors.remove(&FocusedInput::SettingsTmdbApiKey);
        self.input_selected_all
            .remove(&FocusedInput::SettingsTmdbApiKey);
        self.tmdb_verify_status = None;
        self.persist_private_integrations_draft(cx);
    }

    pub fn verify_tmdb_clicked(&mut self, cx: &mut Context<Self>) {
        self.persist_private_integrations_draft(cx);
        let override_key = (!self.tmdb_key_input.trim().is_empty())
            .then(|| self.tmdb_key_input.trim().to_owned());
        if override_key.is_none() && !self.tmdb_key_configured {
            let msg = "Enter a TMDB v3 API key or v4 Read Access Token first.".to_owned();
            self.tmdb_verify_status = Some(msg.clone());
            self.push_toast(ToastKind::Warning, msg, None, cx);
            return;
        }
        self.tmdb_verify_status = Some("Verifying TMDB credentials over HTTP/2...".to_owned());
        cx.notify();

        let core = self.core.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = core.verify_tmdb(override_key).await;
            let _ = this.update(cx, |shell, cx| match res {
                Ok(status) => {
                    let msg = format!("Verified ({}) · Image CDN ready", status.auth_mode);
                    shell.tmdb_verify_status = Some(msg.clone());
                    shell.enriched_attempted.clear();
                    shell.push_toast(ToastKind::Success, "TMDB API verified", Some(msg), cx);
                }
                Err(err) => {
                    shell.tmdb_verify_status = Some(format!("Verification failed: {err}"));
                    shell.push_toast(ToastKind::Error, "TMDB verification failed", Some(err), cx);
                }
            });
        })
        .detach();
    }

    pub fn focus_settings_input(&mut self, input: FocusedInput, cx: &mut Context<Self>) {
        self.settings_open_dropdown = None;
        self.focused_input = Some(input);
        cx.notify();
    }

    pub fn persist_private_integrations_draft(&mut self, cx: &mut Context<Self>) {
        let existing = self.core.private_integrations();
        let next = PrivateIntegrationsConfig {
            tmdb_enabled: true,
            tmdb_api_key: merge_secret_key_draft(
                &self.tmdb_key_input,
                &mut self.tmdb_key_configured,
                existing.tmdb_api_key,
            ),
            active_debrid: idx_to_local_debrid_provider(self.local_debrid_provider_idx),
            direct_rust_unrestrict: self.local_debrid_direct_unrestrict,
            debrid_cached_only_filter: self.local_debrid_cached_only,
            real_debrid_api_key: merge_secret_key_draft(
                &self.real_debrid_key_input,
                &mut self.real_debrid_key_configured,
                existing.real_debrid_api_key,
            ),
            torbox_api_key: merge_secret_key_draft(
                &self.torbox_key_input,
                &mut self.torbox_key_configured,
                existing.torbox_api_key,
            ),
            alldebrid_api_key: merge_secret_key_draft(
                &self.alldebrid_key_input,
                &mut self.alldebrid_key_configured,
                existing.alldebrid_api_key,
            ),
            prowlarr_mode: idx_to_prowlarr_mode(self.prowlarr_mode_idx),
            prowlarr_url: self.prowlarr_url_input.trim().to_owned(),
            prowlarr_api_key: merge_secret_key_draft(
                &self.prowlarr_key_input,
                &mut self.prowlarr_key_configured,
                existing.prowlarr_api_key,
            ),
            prowlarr_timeout_ms: self.prowlarr_timeout_ms.clamp(1000, 15000),
            native_subtitles_enabled: true,
            opensubtitles_api_key: merge_secret_key_draft(
                &self.opensubtitles_key_input,
                &mut self.opensubtitles_key_configured,
                existing.opensubtitles_api_key,
            ),
            subdl_api_key: merge_secret_key_draft(
                &self.subdl_key_input,
                &mut self.subdl_key_configured,
                existing.subdl_api_key,
            ),
        };

        if let Err(err) = self.core.update_private_integrations(next) {
            self.push_toast(
                ToastKind::Error,
                "Failed to save private integrations",
                Some(err),
                cx,
            );
        } else {
            self.memoized_meta_streams = None;
            self.memoized_enriched_streams = None;
            self.prowlarr_fallback_key = None;
            self.prowlarr_fallback_attempted = false;
            self.enriched_attempted.clear();
        }
        cx.notify();
    }

    pub fn clear_metadata_and_image_cache(&mut self, cx: &mut Context<Self>) {
        self.core.clear_addon_cache();
        self.enriched_attempted.clear();
        self.synthetic_tmdb_meta = None;
        self.active_collection = None;
        self.loading_collection_id = None;
        self.poster_cache.clear_failures();
        self.poster_cache.trim_for_playback();
        self.poster_cache.clear_disk_cache();
        self.schedule_debounced_heap_trim();
        self.push_toast(
            ToastKind::Success,
            "Metadata & image cache cleared",
            None,
            cx,
        );
    }

    pub fn open_command_palette(&mut self, cx: &mut Context<Self>) {
        if self.route.is_player() && !self.player_snapshot.paused {
            if let Some(mpv) = &self.mpv {
                let _ = mpv.set_paused(true);
            }
        }
        self.avatar_menu_open = false;
        self.context_menu = None;
        self.quick_peek = None;
        self.discover_open_dropdown = None;
        self.meta_addon_dropdown_open = false;
        self.library_sort_dropdown_open = false;
        self.command_palette_open = true;
        self.palette_selected_idx = 0;
        self.palette_scroll_handle.scroll_to_item(0);
        self.focused_input = Some(FocusedInput::CommandPalette);
        self.input_cursors
            .insert(FocusedInput::CommandPalette, self.palette_query.len());
        self.input_selected_all
            .remove(&FocusedInput::CommandPalette);
        if !self.palette_query.trim().is_empty() {
            self.core.search(self.palette_query.clone());
        }
        cx.notify();
    }

    pub fn close_command_palette(&mut self, cx: &mut Context<Self>) {
        self.command_palette_open = false;
        if self.focused_input == Some(FocusedInput::CommandPalette) {
            self.focused_input = None;
        }
        self.core.unload_search();
        self.schedule_debounced_heap_trim();
        cx.notify();
    }

    pub fn submit_password_login(&mut self, cx: &mut Context<Self>) {
        let email = self.auth_email_input.trim().to_owned();
        let password = self.auth_password_input.clone();
        if email.is_empty() || password.is_empty() {
            self.push_toast(
                ToastKind::Warning,
                "Email and password required",
                None,
                cx,
            );
            return;
        }
        self.focused_input = None;
        self.core.login_with_password(email.clone(), password);
        self.push_toast(ToastKind::Info, format!("Signing in as {email}"), None, cx);
    }

    pub fn begin_link_auth(&mut self, cx: &mut Context<Self>) {
        let already_polling = self.link_poll_active;
        self.link_poll_active = true;
        self.core.start_link_auth();

        if !already_polling {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                    let keep_going = this
                        .update(cx, |shell, _cx| {
                            if !shell.link_poll_active {
                                return false;
                            }
                            let signed_in =
                                shell.core.with_model(|m| m.ctx.profile.auth.is_some());
                            if signed_in {
                                shell.link_poll_active = false;
                                false
                            } else {
                                shell.core.poll_link_auth();
                                true
                            }
                        })
                        .unwrap_or(false);
                    if !keep_going {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.notify();
    }

    pub fn refresh_clipboard_addon_suggestion(&mut self, cx: &mut Context<Self>) {
        if !self.preferences.clipboard_addon_detect {
            self.clipboard_addon_suggestion = None;
            return;
        }
        let clip_text = cx.read_from_clipboard().and_then(|c| c.text());
        let candidate = clip_text.as_deref().and_then(|txt| {
            detect_clipboard_addon_url(txt)
                .map(|u| u.to_string())
                .or_else(|| detect_addon_url_candidate(txt))
        });
        let Some(raw_url) = candidate else {
            self.clipboard_addon_suggestion = None;
            return;
        };
        let normalized_check = normalize_addon_url(&raw_url)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| raw_url.clone());

        if self.dismissed_clipboard_addon_url.as_deref() == Some(raw_url.as_str())
            || self.dismissed_clipboard_addon_url.as_deref() == Some(normalized_check.as_str())
        {
            self.clipboard_addon_suggestion = None;
            return;
        }

        let already_installed_verbatim = self.core.with_model(|m| {
            m.ctx.profile.addons.iter().any(|a| {
                let s = a.transport_url.as_str();
                s.eq_ignore_ascii_case(&raw_url) || s.eq_ignore_ascii_case(&normalized_check)
            })
        });
        if already_installed_verbatim {
            self.clipboard_addon_suggestion = None;
        } else {
            self.clipboard_addon_suggestion = Some(normalized_check);
        }
    }

    pub fn install_or_replace_addon_from_raw(
        &mut self,
        raw_url: &str,
        clear_url_input: bool,
        cx: &mut Context<Self>,
    ) {
        let trimmed = raw_url.trim().to_owned();
        if trimmed.is_empty() {
            return;
        }
        let normalized_key = match normalize_addon_url(&trimmed) {
            Ok(url) => url.to_string(),
            Err(err) => {
                self.push_toast(ToastKind::Error, "Invalid addon URL", Some(err), cx);
                return;
            }
        };

        if !self.installing_addon_urls.insert(normalized_key.clone()) {
            return;
        }
        if self.focused_input == Some(FocusedInput::AddonManifestUrl) {
            self.focused_input = None;
        }
        cx.notify();

        let core = self.core.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let outcome = core.install_or_replace_addon_by_url(&trimmed).await;
            let _ = this.update(cx, |shell, cx| {
                shell.installing_addon_urls.remove(&normalized_key);
                match outcome {
                    Ok(res) => {
                        if clear_url_input {
                            shell.addon_url_input.clear();
                            shell.input_cursors.remove(&FocusedInput::AddonManifestUrl);
                            shell
                                .input_selected_all
                                .remove(&FocusedInput::AddonManifestUrl);
                        }
                        if shell
                            .clipboard_addon_suggestion
                            .as_deref()
                            .is_some_and(|c| c.eq_ignore_ascii_case(&normalized_key))
                        {
                            shell.clipboard_addon_suggestion = None;
                        }
                        let name = res.descriptor.manifest.name;
                        let slot_label = format!("Priority slot {:02}", res.slot_index + 1);
                        let title = if res.was_muted {
                            format!("Re-enabled {name}")
                        } else if res.replaced_existing {
                            format!("Updated {name} in place")
                        } else {
                            format!("Installed {name}")
                        };
                        shell.push_toast(ToastKind::Success, title, Some(slot_label), cx);
                    }
                    Err(err) => {
                        shell.push_toast(
                            ToastKind::Error,
                            "Addon install failed",
                            Some(err),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn submit_addon_manifest_lookup(&mut self, cx: &mut Context<Self>) {
        let raw = self.addon_url_input.trim().to_owned();
        if raw.is_empty() {
            return;
        }
        self.install_or_replace_addon_from_raw(&raw, true, cx);
    }

    pub fn schedule_startup_update_check(&mut self, cx: &mut Context<Self>) {
        if !self.preferences.check_updates_on_startup {
            return;
        }
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let _ = this.update(cx, |shell, cx| {
                if matches!(shell.update_status, UpdateStatus::Idle) {
                    shell.trigger_update_check(false, cx);
                }
            });
        })
        .detach();
    }

    pub fn trigger_update_check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if matches!(
            self.update_status,
            UpdateStatus::Checking | UpdateStatus::Downloading { .. }
        ) {
            return;
        }
        self.update_status = UpdateStatus::Checking;
        cx.notify();

        let tokio_handle = self.tokio_handle.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = tokio_handle
                .spawn(async move { updater::check_for_updates().await })
                .await
                .map_err(|e| format!("Update check task aborted: {e}"))
                .and_then(|inner| inner);

            let _ = this.update(cx, |shell, cx| {
                match res {
                    Ok(Some(info)) => {
                        let ver = info.version.clone();
                        shell.update_status = UpdateStatus::Available(Box::new(info));
                        shell.push_toast(
                            ToastKind::Info,
                            format!("Ferrite v{ver} is available"),
                            Some("Open Settings to review and install".to_owned()),
                            cx,
                        );
                    }
                    Ok(None) => {
                        shell.update_status = UpdateStatus::UpToDate {
                            checked_version: updater::CURRENT_APP_VERSION.to_owned(),
                        };
                        if manual {
                            shell.push_toast(
                                ToastKind::Success,
                                format!(
                                    "Ferrite v{} is up to date",
                                    updater::CURRENT_APP_VERSION
                                ),
                                None,
                                cx,
                            );
                        }
                    }
                    Err(err) => {
                        if manual {
                            shell.update_status = UpdateStatus::Failed(err.clone());
                            shell.push_toast(
                                ToastKind::Warning,
                                "Update check failed",
                                Some(err),
                                cx,
                            );
                        } else {
                            shell.update_status = UpdateStatus::Idle;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn install_available_update(&mut self, cx: &mut Context<Self>) {
        let UpdateStatus::Available(info_box) = &self.update_status else {
            return;
        };
        let info = (**info_box).clone();
        self.update_status = UpdateStatus::Downloading {
            version: info.version.clone(),
        };
        cx.notify();

        let tokio_handle = self.tokio_handle.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let outcome = tokio_handle
                .spawn(async move { updater::download_and_apply_update(&info).await })
                .await
                .map_err(|e| format!("Update installer task aborted: {e}"))
                .and_then(|inner| inner);

            let _ = this.update(cx, |shell, cx| {
                match outcome {
                    Ok(AppliedUpdateOutcome::ReadyToRestart { version }) => {
                        shell.update_status = UpdateStatus::ReadyToRestart {
                            version: version.clone(),
                        };
                        shell.push_toast(
                            ToastKind::Success,
                            format!("Ferrite v{version} installed"),
                            Some("Click Restart to apply".to_owned()),
                            cx,
                        );
                    }
                    Ok(AppliedUpdateOutcome::DebDownloaded {
                        version,
                        deb_path,
                        install_cmd,
                    }) => {
                        let _ = updater::open_deb_package_with_system(&deb_path);
                        shell.update_status = UpdateStatus::DebReady {
                            version: version.clone(),
                            deb_path,
                            install_cmd,
                        };
                        shell.push_toast(
                            ToastKind::Success,
                            format!("Verified Ferrite v{version} .deb downloaded"),
                            Some("Install via Package Installer or apt command".to_owned()),
                            cx,
                        );
                    }
                    Ok(AppliedUpdateOutcome::WindowsInstallerPrepared {
                        version,
                        installer_path,
                    }) => match updater::launch_windows_silent_installer(&installer_path) {
                        Ok(()) => {
                            shell.update_status = UpdateStatus::ApplyingWindowsInstaller {
                                version,
                            };
                            cx.quit();
                        }
                        Err(err) => {
                            shell.update_status = UpdateStatus::Failed(err.clone());
                            shell.push_toast(
                                ToastKind::Error,
                                "Installer launch failed",
                                Some(err),
                                cx,
                            );
                        }
                    },
                    Err(err) => {
                        shell.update_status = UpdateStatus::Failed(err.clone());
                        shell.push_toast(
                            ToastKind::Error,
                            "Update verification failed",
                            Some(err),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn restart_into_updated_binary(&mut self, cx: &mut Context<Self>) {
        match updater::spawn_restarted_executable() {
            Ok(()) => cx.quit(),
            Err(err) => {
                self.push_toast(ToastKind::Error, "Restart failed", Some(err), cx);
            }
        }
    }

    pub fn push_toast(
        &mut self,
        kind: ToastKind,
        title: impl Into<String>,
        detail: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_toast_id;
        self.next_toast_id = self.next_toast_id.wrapping_add(1);
        self.toasts.push(ToastMessage {
            id,
            kind,
            title: title.into(),
            detail,
            created_at: Instant::now(),
        });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor()
                .timer(TOAST_LIFETIME + Duration::from_millis(20))
                .await;
            let _ = this.update(cx, |shell, cx| {
                let before = shell.toasts.len();
                shell.toasts.retain(|t| t.id != id && t.created_at.elapsed() < TOAST_LIFETIME);
                if shell.toasts.len() != before {
                    cx.notify();
                }
            });
        })
        .detach();

        cx.notify();
    }

    fn apply_focused_text_edit(
        &mut self,
        target: FocusedInput,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> crate::state::TextEditResult {
        let clip = cx.read_from_clipboard().and_then(|c| c.text());
        let is_secret = target.is_secret();
        let selected_all_entry = &mut self.input_selected_all;
        let cursors = &mut self.input_cursors;

        let buf = match target {
            FocusedInput::CommandPalette => &mut self.palette_query,
            FocusedInput::FullSearchInput => &mut self.full_search_query,
            FocusedInput::AuthEmail => &mut self.auth_email_input,
            FocusedInput::AuthPassword => &mut self.auth_password_input,
            FocusedInput::AddonManifestUrl => &mut self.addon_url_input,
            FocusedInput::AddonSearch => &mut self.addon_search_query,
            FocusedInput::DiscoverCatalogSearch => &mut self.discover_catalog_search,
            FocusedInput::LibrarySearch => &mut self.library_search_query,
            FocusedInput::SettingsSearch => &mut self.settings_search_query,
            FocusedInput::RealDebridApiKey => &mut self.real_debrid_key_input,
            FocusedInput::TorBoxApiKey => &mut self.torbox_key_input,
            FocusedInput::AllDebridApiKey => &mut self.alldebrid_key_input,
            FocusedInput::ProwlarrUrl => &mut self.prowlarr_url_input,
            FocusedInput::ProwlarrApiKey => &mut self.prowlarr_key_input,
            FocusedInput::SettingsOpenSubtitlesKey => &mut self.opensubtitles_key_input,
            FocusedInput::SettingsSubdlKey => &mut self.subdl_key_input,
            FocusedInput::SettingsTmdbApiKey => &mut self.tmdb_key_input,
        };

        let cursor = cursors.entry(target).or_insert_with(|| buf.len());
        let mut selected_all = selected_all_entry.contains(&target);
        let res = apply_key_to_text_buffer_with_cursor(
            buf,
            cursor,
            &mut selected_all,
            is_secret,
            event,
            clip,
        );
        if selected_all {
            selected_all_entry.insert(target);
        } else {
            selected_all_entry.remove(&target);
        }
        if let Some(copied) = &res.copied_text {
            cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
        }
        res
    }

    fn schedule_palette_search_debounced(&mut self, cx: &mut Context<Self>) {
        self.search_debounce_seq = self.search_debounce_seq.wrapping_add(1);
        let debounce_seq = self.search_debounce_seq;
        let query = self.palette_query.clone();
        if query.trim().is_empty() {
            self.core.unload_search();
            return;
        }
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor()
                .timer(Duration::from_millis(180))
                .await;
            let _ = this.update(cx, |shell, _cx| {
                if shell.search_debounce_seq == debounce_seq && shell.command_palette_open {
                    shell.core.search(query);
                }
            });
        })
        .detach();
    }

    fn on_global_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        let ctrl_or_cmd = ks.modifiers.control || ks.modifiers.platform;

        if ctrl_or_cmd && ks.key == "q" && self.focused_input.is_none() && !self.command_palette_open
        {
            cx.quit();
            return;
        }

        if ctrl_or_cmd && ks.key == "k" {
            if self.command_palette_open {
                self.close_command_palette(cx);
            } else {
                self.open_command_palette(cx);
            }
            return;
        }

        if self.command_palette_open {
            match ks.key.as_str() {
                "escape" => {
                    self.close_command_palette(cx);
                    return;
                }
                "up" => {
                    self.palette_selected_idx = self.palette_selected_idx.saturating_sub(1);
                    self.palette_scroll_handle
                        .scroll_to_item(self.palette_selected_idx);
                    cx.notify();
                    return;
                }
                "down" => {
                    let count = self.build_palette_entries().len();
                    if count > 0 {
                        self.palette_selected_idx = (self.palette_selected_idx + 1).min(count - 1);
                        self.palette_scroll_handle
                            .scroll_to_item(self.palette_selected_idx);
                    }
                    cx.notify();
                    return;
                }
                "enter" => {
                    let entries = self.build_palette_entries();
                    if !entries.is_empty() {
                        let idx = self.palette_selected_idx.min(entries.len() - 1);
                        let chosen = entries[idx].clone();
                        self.execute_palette_entry(chosen, cx);
                    }
                    return;
                }
                _ => {
                    let edit = self.apply_focused_text_edit(FocusedInput::CommandPalette, event, cx);
                    if edit.text_changed {
                        self.palette_selected_idx = 0;
                        self.palette_scroll_handle.scroll_to_item(0);
                        self.schedule_palette_search_debounced(cx);
                        cx.notify();
                    } else if edit.handled {
                        cx.notify();
                    }
                    return;
                }
            }
        }

        if let Some(target) = self.focused_input {
            match ks.key.as_str() {
                "escape" => {
                    if target == FocusedInput::SettingsSearch
                        && !self.settings_search_query.is_empty()
                    {
                        self.settings_search_query.clear();
                        self.input_cursors.remove(&FocusedInput::SettingsSearch);
                        self.input_selected_all
                            .remove(&FocusedInput::SettingsSearch);
                        cx.notify();
                        return;
                    }
                    if target == FocusedInput::LibrarySearch && !self.library_search_query.is_empty()
                    {
                        self.library_search_query.clear();
                        self.input_cursors.remove(&FocusedInput::LibrarySearch);
                        self.input_selected_all.remove(&FocusedInput::LibrarySearch);
                        cx.notify();
                        return;
                    }
                    self.focused_input = None;
                    self.input_selected_all.remove(&target);
                    if target == FocusedInput::DiscoverCatalogSearch {
                        self.discover_open_dropdown = None;
                    }
                    cx.notify();
                    return;
                }
                "tab" => {
                    self.input_selected_all.remove(&target);
                    let next_target = match target {
                        FocusedInput::AuthEmail => FocusedInput::AuthPassword,
                        FocusedInput::AuthPassword => FocusedInput::AuthEmail,
                        FocusedInput::AddonManifestUrl => FocusedInput::AddonSearch,
                        FocusedInput::AddonSearch => FocusedInput::AddonManifestUrl,
                        FocusedInput::RealDebridApiKey => FocusedInput::TorBoxApiKey,
                        FocusedInput::TorBoxApiKey => FocusedInput::AllDebridApiKey,
                        FocusedInput::AllDebridApiKey => FocusedInput::ProwlarrUrl,
                        FocusedInput::ProwlarrUrl => FocusedInput::ProwlarrApiKey,
                        FocusedInput::ProwlarrApiKey => FocusedInput::SettingsOpenSubtitlesKey,
                        FocusedInput::SettingsOpenSubtitlesKey => FocusedInput::SettingsSubdlKey,
                        FocusedInput::SettingsSubdlKey => FocusedInput::SettingsTmdbApiKey,
                        FocusedInput::SettingsTmdbApiKey => FocusedInput::RealDebridApiKey,
                        other => other,
                    };
                    self.focused_input = Some(next_target);
                    cx.notify();
                    return;
                }
                "enter" => {
                    self.input_selected_all.remove(&target);
                    match target {
                        FocusedInput::AuthEmail | FocusedInput::AuthPassword => {
                            self.submit_password_login(cx);
                        }
                        FocusedInput::AddonManifestUrl => {
                            self.submit_addon_manifest_lookup(cx);
                        }
                        FocusedInput::FullSearchInput => {
                            self.refresh_full_search(cx);
                        }
                        FocusedInput::AddonSearch
                        | FocusedInput::DiscoverCatalogSearch
                        | FocusedInput::LibrarySearch
                        | FocusedInput::SettingsSearch => {
                            self.focused_input = None;
                            cx.notify();
                        }
                        FocusedInput::RealDebridApiKey
                        | FocusedInput::TorBoxApiKey
                        | FocusedInput::AllDebridApiKey
                        | FocusedInput::ProwlarrUrl
                        | FocusedInput::ProwlarrApiKey
                        | FocusedInput::SettingsOpenSubtitlesKey
                        | FocusedInput::SettingsSubdlKey
                        | FocusedInput::SettingsTmdbApiKey => {
                            self.focused_input = None;
                            self.persist_private_integrations_draft(cx);
                        }
                        FocusedInput::CommandPalette => {}
                    }
                    return;
                }
                _ => {
                    let edit = self.apply_focused_text_edit(target, event, cx);
                    if !edit.text_changed {
                        if edit.handled {
                            cx.notify();
                        }
                        return;
                    }
                    let mut integration_edited = false;
                    match target {
                        FocusedInput::RealDebridApiKey => {
                            self.real_debrid_key_configured =
                                !self.real_debrid_key_input.trim().is_empty();
                            integration_edited = true;
                        }
                        FocusedInput::TorBoxApiKey => {
                            self.torbox_key_configured = !self.torbox_key_input.trim().is_empty();
                            integration_edited = true;
                        }
                        FocusedInput::AllDebridApiKey => {
                            self.alldebrid_key_configured =
                                !self.alldebrid_key_input.trim().is_empty();
                            integration_edited = true;
                        }
                        FocusedInput::ProwlarrUrl => {
                            integration_edited = true;
                        }
                        FocusedInput::ProwlarrApiKey => {
                            self.prowlarr_key_configured =
                                !self.prowlarr_key_input.trim().is_empty();
                            integration_edited = true;
                        }
                        FocusedInput::SettingsOpenSubtitlesKey => {
                            self.opensubtitles_key_configured =
                                !self.opensubtitles_key_input.trim().is_empty();
                            integration_edited = true;
                        }
                        FocusedInput::SettingsSubdlKey => {
                            self.subdl_key_configured = !self.subdl_key_input.trim().is_empty();
                            integration_edited = true;
                        }
                        FocusedInput::SettingsTmdbApiKey => {
                            self.tmdb_key_configured = !self.tmdb_key_input.trim().is_empty();
                            integration_edited = true;
                        }
                        FocusedInput::FullSearchInput => {
                            self.palette_query = self.full_search_query.clone();
                            self.refresh_full_search_debounced(cx);
                            return;
                        }
                        _ => {}
                    }
                    if integration_edited {
                        self.persist_private_integrations_draft(cx);
                    } else {
                        cx.notify();
                    }
                    return;
                }
            }
        }

        if self.route.is_player() {
            self.bump_player_osd_activity(cx);
            match ks.key.as_str() {
                "space" => {
                    if event.is_held {
                        if !self.hold_2x_active {
                            self.hold_2x_active = true;
                            if let Some(mpv) = &self.mpv {
                                let _ = mpv.set_paused(false);
                                let _ = mpv.set_speed(2.0);
                            }
                            cx.notify();
                        }
                    } else {
                        self.space_key_down = true;
                    }
                }
                "," | "<" => self.step_player_frame(false, cx),
                "." | ">" => self.step_player_frame(true, cx),
                "left" => {
                    let step = if ks.modifiers.shift { -30.0 } else { -5.0 };
                    self.seek_player_relative(step, cx);
                }
                "right" => {
                    let step = if ks.modifiers.shift { 30.0 } else { 5.0 };
                    self.seek_player_relative(step, cx);
                }
                "up" => self.adjust_player_volume_step(5.0, cx),
                "down" => self.adjust_player_volume_step(-5.0, cx),
                "m" => self.toggle_player_mute_user(cx),
                "f" | "f11" => {
                    window.toggle_fullscreen();
                    cx.notify();
                }
                "s" | "v" => self.cycle_subtitle_tracks(cx),
                "a" => self.cycle_audio_tracks(cx),
                "i" => self.toggle_player_stats_for_nerds(cx),
                "z" => self.adjust_subtitle_delay(-0.1, cx),
                "x" => self.adjust_subtitle_delay(0.1, cx),
                "escape" => {
                    if let Some(cur) = self.player_popover {
                        self.player_popover = cur.parent_sheet();
                        if self.player_popover.is_none() {
                            self.schedule_osd_autohide_timer(cx);
                        }
                        cx.notify();
                    } else if window.is_fullscreen() {
                        window.toggle_fullscreen();
                        cx.notify();
                    } else {
                        self.exit_player(cx);
                    }
                }
                _ => {}
            }
            return;
        }

        if ks.key == "f11" {
            window.toggle_fullscreen();
            cx.notify();
            return;
        }
        if ks.key == "/" && !ctrl_or_cmd {
            self.open_command_palette(cx);
            return;
        }
        if ks.key == "escape" {
            if self.context_menu.is_some() {
                self.dismiss_context_menu(cx);
            } else if self.avatar_menu_open {
                self.avatar_menu_open = false;
                cx.notify();
            } else if self.quick_peek.is_some() {
                self.close_quick_peek(cx);
            } else if self.discover_open_dropdown.is_some() {
                self.discover_open_dropdown = None;
                cx.notify();
            } else if self.meta_addon_dropdown_open {
                self.meta_addon_dropdown_open = false;
                cx.notify();
            } else if self.library_sort_dropdown_open {
                self.library_sort_dropdown_open = false;
                cx.notify();
            } else if self.board_hidden_drawer_open {
                self.board_hidden_drawer_open = false;
                cx.notify();
            } else if self.settings_open_dropdown.is_some() {
                self.settings_open_dropdown = None;
                cx.notify();
            } else if self.kb_focus_index.take().is_some() {
                cx.notify();
            } else if window.is_fullscreen() {
                window.toggle_fullscreen();
                cx.notify();
            } else {
                self.navigate_back(cx);
            }
            return;
        }
        if ks.modifiers.alt {
            match ks.key.as_str() {
                "left" => {
                    self.navigate_back(cx);
                    return;
                }
                "right" => {
                    self.navigate_forward(cx);
                    return;
                }
                _ => {}
            }
        }
        if ks.modifiers.alt || ctrl_or_cmd {
            let tab = match ks.key.as_str() {
                "1" => Some(SidebarTab::Home),
                "2" => Some(SidebarTab::Discover),
                "3" => Some(SidebarTab::Calendar),
                "4" => Some(SidebarTab::Library),
                "5" => Some(SidebarTab::Addons),
                "6" => Some(SidebarTab::Settings),
                "7" => Some(SidebarTab::Account),
                _ => None,
            };
            if let Some(t) = tab {
                self.navigate_tab(t, cx);
                return;
            }
        }

        if self.context_menu.is_none()
            && !self.avatar_menu_open
            && self.quick_peek.is_none()
            && !ks.modifiers.alt
            && !ctrl_or_cmd
        {
            match ks.key.as_str() {
                "left" => {
                    self.move_kb_focus(0, -1, cx);
                }
                "right" => {
                    self.move_kb_focus(0, 1, cx);
                }
                "up" => {
                    self.move_kb_focus(-1, 0, cx);
                }
                "down" => {
                    self.move_kb_focus(1, 0, cx);
                }
                "enter" => {
                    if let Some(idx) = self.kb_focus_index {
                        if let Some(card) = self.kb_focus_items.get(idx).cloned() {
                            self.open_route(card.route, cx);
                        }
                    }
                }
                "pageup" | "pagedown" | "home" | "end"
                    if self.scroll_active_view_key(ks.key.as_str()) =>
                {
                    cx.notify();
                }
                _ => {}
            }
        }
    }

    fn on_global_key_up(
        &mut self,
        event: &KeyUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.route.is_player() && event.keystroke.key == "space" {
            if self.hold_2x_active {
                self.hold_2x_active = false;
                self.space_key_down = false;
                if let Some(mpv) = &self.mpv {
                    let _ = mpv.set_speed(self.playback_speed);
                }
                cx.notify();
            } else if self.space_key_down {
                self.space_key_down = false;
                if let Some(mpv) = &self.mpv {
                    let _ = mpv.toggle_pause();
                }
            }
        }
    }

    pub(crate) fn render_csd_window_controls(
        window: &Window,
        id_prefix: &'static str,
        in_glass_capsule: bool,
    ) -> Option<gpui::Div> {
        let is_windows = cfg!(target_os = "windows");
        if !should_render_custom_window_controls(
            is_windows,
            window.window_decorations(),
            window.is_fullscreen(),
        ) {
            return None;
        }

        let theme = active_theme();
        let is_player_chrome = id_prefix == "player";
        let fg_idle = if is_player_chrome {
            0xA0A0A8
        } else {
            theme.text_secondary
        };
        let fg_hover = if is_player_chrome {
            0xF5F1E8
        } else {
            theme.text_primary
        };
        let hover_bg = if is_player_chrome {
            0xFFFFFF18
        } else {
            theme.glass_row_hover_bg
        };
        let capsule_bg = if is_player_chrome {
            0x141417DB
        } else {
            theme.bg_topnav_float
        };
        let capsule_border = if is_player_chrome {
            0xFFFFFF1E
        } else {
            theme.glass_border
        };
        let close_hover_fg = if is_player_chrome {
            0xf87171
        } else {
            theme.accent_rose
        };

        let use_native_nc_chrome = uses_native_windows_nc_chrome(is_windows);
        let controls = window.window_controls();
        let is_maximized = window.is_maximized();
        let min_id = SharedString::from(format!("{id_prefix}-csd-min"));
        let max_id = SharedString::from(format!("{id_prefix}-csd-max"));
        let close_id = SharedString::from(format!("{id_prefix}-csd-close"));
        let min_grp = SharedString::from(format!("{id_prefix}-grp-min"));
        let max_grp = SharedString::from(format!("{id_prefix}-grp-max"));
        let close_grp = SharedString::from(format!("{id_prefix}-grp-close"));

        // Occlude the container and each button so the parent titlebar's
        // `WindowControlArea::Drag` hitbox (which GPUI registers first and
        // checks in paint order) cannot shadow `Min`/`Max`/`Close` on Windows
        // or trigger parent titlebar click handlers.
        let base = div()
            .occlude()
            .flex()
            .items_center()
            .gap(px(2.0))
            .when(in_glass_capsule, |el| {
                el.h(sp(36.0))
                    .px(sp(4.0))
                    .rounded(px(RADIUS_PILL))
                    .bg(rgba(capsule_bg))
                    .border_1()
                    .border_color(rgba(capsule_border))
            });

        Some(
            base.when(controls.minimize, |el| {
                el.child(
                    div()
                        .id(min_id)
                        .group(min_grp.clone())
                        .occlude()
                        .window_control_area(WindowControlArea::Min)
                        .cursor_pointer()
                        .w(sp(30.0))
                        .h(sp(30.0))
                        .rounded(px(RADIUS_PILL))
                        .hover(move |s| s.bg(rgba(hover_bg)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(!use_native_nc_chrome, |btn| {
                            btn.on_mouse_down(MouseButton::Left, |_, window, cx| {
                                cx.stop_propagation();
                                window.minimize_window();
                            })
                        })
                        .child(
                            svg_icon(IconName::Minus, 13.5 * theme.ui_scale, rgb(fg_idle))
                                .group_hover(min_grp, move |s| s.text_color(rgb(fg_hover))),
                        ),
                )
            })
            .when(controls.maximize, |el| {
                el.child(
                    div()
                        .id(max_id)
                        .group(max_grp.clone())
                        .occlude()
                        .window_control_area(WindowControlArea::Max)
                        .cursor_pointer()
                        .w(sp(30.0))
                        .h(sp(30.0))
                        .rounded(px(RADIUS_PILL))
                        .hover(move |s| s.bg(rgba(hover_bg)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(!use_native_nc_chrome, |btn| {
                            btn.on_mouse_down(MouseButton::Left, |_, window, cx| {
                                cx.stop_propagation();
                                window.zoom_window();
                            })
                        })
                        .child(
                            svg_icon(
                                if is_maximized {
                                    IconName::RestoreWindow
                                } else {
                                    IconName::MaximizeWindow
                                },
                                13.0 * theme.ui_scale,
                                rgb(fg_idle),
                            )
                            .group_hover(max_grp, move |s| s.text_color(rgb(fg_hover))),
                        ),
                )
            })
            .child(
                div()
                    .id(close_id)
                    .group(close_grp.clone())
                    .occlude()
                    .window_control_area(WindowControlArea::Close)
                    .cursor_pointer()
                    .w(sp(30.0))
                    .h(sp(30.0))
                    .rounded(px(RADIUS_PILL))
                    .hover(|s| s.bg(rgba(0xf8717124)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(!use_native_nc_chrome, |btn| {
                        btn.on_mouse_down(MouseButton::Left, |_, window, cx| {
                            cx.stop_propagation();
                            window.remove_window();
                        })
                    })
                    .child(
                        svg_icon(IconName::X, 14.0 * theme.ui_scale, rgb(fg_idle))
                            .group_hover(close_grp, move |s| s.text_color(rgb(close_hover_fg))),
                    ),
            ),
        )
    }

    fn render_csd_resize_handles(window: &Window) -> Option<gpui::Div> {
        let Decorations::Client { tiling } = window.window_decorations() else {
            return None;
        };
        if window.is_maximized() || window.is_fullscreen() {
            return None;
        }

        Some(
            div()
                .absolute()
                .inset_0()
                .bg(transparent_cutout())
                .when(!tiling.top, |el| {
                    el.child(
                        div()
                            .id("csd-resize-top")
                            .absolute()
                            .top_0()
                            .left(px(10.0))
                            .right(px(10.0))
                            .h(px(5.0))
                            .bg(transparent_cutout())
                            .cursor(CursorStyle::ResizeUpDown)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                cx.stop_propagation();
                                window.start_window_resize(ResizeEdge::Top);
                            }),
                    )
                })
                .when(!tiling.bottom, |el| {
                    el.child(
                        div()
                            .id("csd-resize-bottom")
                            .absolute()
                            .bottom_0()
                            .left(px(10.0))
                            .right(px(10.0))
                            .h(px(5.0))
                            .bg(transparent_cutout())
                            .cursor(CursorStyle::ResizeUpDown)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                cx.stop_propagation();
                                window.start_window_resize(ResizeEdge::Bottom);
                            }),
                    )
                })
                .when(!tiling.left, |el| {
                    el.child(
                        div()
                            .id("csd-resize-left")
                            .absolute()
                            .left_0()
                            .top(px(10.0))
                            .bottom(px(10.0))
                            .w(px(5.0))
                            .bg(transparent_cutout())
                            .cursor(CursorStyle::ResizeLeftRight)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                cx.stop_propagation();
                                window.start_window_resize(ResizeEdge::Left);
                            }),
                    )
                })
                .when(!tiling.right, |el| {
                    el.child(
                        div()
                            .id("csd-resize-right")
                            .absolute()
                            .right_0()
                            .top(px(10.0))
                            .bottom(px(10.0))
                            .w(px(5.0))
                            .bg(transparent_cutout())
                            .cursor(CursorStyle::ResizeLeftRight)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                cx.stop_propagation();
                                window.start_window_resize(ResizeEdge::Right);
                            }),
                    )
                })
                .when(!tiling.top && !tiling.left, |el| {
                    el.child(
                        div()
                            .id("csd-resize-top-left")
                            .absolute()
                            .top_0()
                            .left_0()
                            .w(px(10.0))
                            .h(px(10.0))
                            .bg(transparent_cutout())
                            .cursor(CursorStyle::ResizeUpLeftDownRight)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                cx.stop_propagation();
                                window.start_window_resize(ResizeEdge::TopLeft);
                            }),
                    )
                })
                .when(!tiling.top && !tiling.right, |el| {
                    el.child(
                        div()
                            .id("csd-resize-top-right")
                            .absolute()
                            .top_0()
                            .right_0()
                            .w(px(10.0))
                            .h(px(10.0))
                            .bg(transparent_cutout())
                            .cursor(CursorStyle::ResizeUpRightDownLeft)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                cx.stop_propagation();
                                window.start_window_resize(ResizeEdge::TopRight);
                            }),
                    )
                })
                .when(!tiling.bottom && !tiling.left, |el| {
                    el.child(
                        div()
                            .id("csd-resize-bottom-left")
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .w(px(10.0))
                            .h(px(10.0))
                            .bg(transparent_cutout())
                            .cursor(CursorStyle::ResizeUpRightDownLeft)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                cx.stop_propagation();
                                window.start_window_resize(ResizeEdge::BottomLeft);
                            }),
                    )
                })
                .when(!tiling.bottom && !tiling.right, |el| {
                    el.child(
                        div()
                            .id("csd-resize-bottom-right")
                            .absolute()
                            .bottom_0()
                            .right_0()
                            .w(px(10.0))
                            .h(px(10.0))
                            .bg(transparent_cutout())
                            .cursor(CursorStyle::ResizeUpLeftDownRight)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                cx.stop_propagation();
                                window.start_window_resize(ResizeEdge::BottomRight);
                            }),
                    )
                }),
        )
    }

    fn render_browsing_context_menu(
        &self,
        menu: &BrowsingContextMenu,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = active_theme();
        let accent_ink = if theme.is_light {
            theme.accent_press
        } else {
            theme.accent
        };
        let menu_w = 248.0_f32;
        let est_h = 296.0_f32;
        let x = f32::from(menu.position.x)
            .clamp(12.0, (self.viewport_size_px.0 - menu_w - 12.0).max(12.0));
        let y = f32::from(menu.position.y)
            .clamp(12.0, (self.viewport_size_px.1 - est_h - 12.0).max(12.0));

        let play_type = menu.type_name.clone();
        let play_id = menu.id.clone();
        let play_vid = menu.video_id.clone();
        let play_label = if menu.can_rewind_cw {
            "Resume"
        } else {
            "Play"
        };

        let peek_target = QuickPeekTarget {
            type_name: menu.type_name.clone(),
            id: menu.id.clone(),
            video_id: menu.video_id.clone(),
            title: menu.title.to_string(),
            preview: menu.preview.clone(),
        };

        let details_type = menu.type_name.clone();
        let details_id = menu.id.clone();
        let details_vid = menu.video_id.clone();

        let lib_id = menu.id.clone();
        let lib_in = menu.is_in_library;
        let lib_preview = menu.preview.clone();

        let watch_id = menu.id.clone();
        let watch_in_lib = menu.is_in_library;
        let watch_is_watched = menu.is_watched;
        let watch_preview = menu.preview.clone();
        let watch_video = menu.video.clone();

        let rewind_id = menu.id.clone();
        let can_rewind = menu.can_rewind_cw;

        let hover_bg = theme.glass_row_hover_bg;
        let ui_scale = theme.ui_scale;
        let menu_row = |id: &'static str,
                        icon: IconName,
                        label: &'static str,
                        col: u32,
                        on_click: ClickHandler| {
            div()
                .id(id)
                .w_full()
                .cursor_pointer()
                .px(sp(10.0))
                .py(sp(8.0))
                .rounded(px(RADIUS_CONTROL))
                .hover(move |s| s.bg(rgba(hover_bg)))
                .flex()
                .items_center()
                .gap(sp(10.0))
                .font(ui_sans(FontWeight::MEDIUM))
                .text_size(sp(13.0))
                .text_color(rgb(col))
                .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                    cx.stop_propagation();
                    on_click(ev, window, cx);
                })
                .child(svg_icon(icon, 14.0 * ui_scale, rgb(col)))
                .child(label)
        };

        div()
            .id("browsing-context-menu-backdrop")
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dismiss_context_menu(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.dismiss_context_menu(cx);
                }),
            )
            .child(
                glass_dropdown_panel("browsing-context-menu", menu_w)
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .child(
                        div()
                            .px(sp(10.0))
                            .py(sp(6.0))
                            .border_b_1()
                            .border_color(rgba(theme.glass_divider))
                            .mb(sp(2.0))
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_size(sp(12.5))
                            .text_color(rgb(theme.text_primary))
                            .truncate()
                            .child(menu.title.clone()),
                    )
                    .child(menu_row(
                        "ctx-menu-play",
                        IconName::Play,
                        play_label,
                        accent_ink,
                        click_handler(cx.listener(move |this, _, _, cx| {
                            this.dismiss_context_menu(cx);
                            this.quick_resume_or_open(
                                play_type.clone(),
                                play_id.clone(),
                                play_vid.clone(),
                                cx,
                            );
                        })),
                    ))
                    .child(menu_row(
                        "ctx-menu-quick-peek",
                        IconName::Eye,
                        "Quick Peek",
                        theme.text_primary,
                        click_handler(cx.listener(move |this, _, _, cx| {
                            this.dismiss_context_menu(cx);
                            this.open_quick_peek(peek_target.clone(), cx);
                        })),
                    ))
                    .child(menu_row(
                        "ctx-menu-details",
                        IconName::Info,
                        "Open details",
                        theme.text_primary,
                        click_handler(cx.listener(move |this, _, _, cx| {
                            this.dismiss_context_menu(cx);
                            this.open_meta_details(
                                details_type.clone(),
                                details_id.clone(),
                                details_vid.clone(),
                                cx,
                            );
                        })),
                    ))
                    .when(lib_in || lib_preview.is_some(), |panel| {
                        panel.child(menu_row(
                            "ctx-menu-library",
                            IconName::Bookmark,
                            if lib_in {
                                "Remove from library"
                            } else {
                                "Add to library"
                            },
                            theme.text_primary,
                            click_handler(cx.listener(move |this, _, _, cx| {
                                this.dismiss_context_menu(cx);
                                if lib_in {
                                    this.core.remove_from_library(lib_id.clone());
                                    this.push_toast(
                                        ToastKind::Info,
                                        "Removed from library",
                                        None,
                                        cx,
                                    );
                                } else if let Some(preview) = lib_preview.clone() {
                                    this.core.toggle_library_item(preview);
                                    this.push_toast(
                                        ToastKind::Success,
                                        "Added to library",
                                        None,
                                        cx,
                                    );
                                }
                            })),
                        ))
                    })
                    .when(
                        watch_video.is_some() || watch_in_lib || watch_preview.is_some(),
                        |panel| {
                            let label = if let Some((_, vid_watched)) = &watch_video {
                                if *vid_watched {
                                    "Mark episode unwatched"
                                } else {
                                    "Mark episode watched"
                                }
                            } else if watch_is_watched {
                                "Mark as unwatched"
                            } else {
                                "Mark as watched"
                            };
                            panel.child(menu_row(
                                "ctx-menu-watched",
                                IconName::Check,
                                label,
                                theme.text_primary,
                                click_handler(cx.listener(move |this, _, _, cx| {
                                    this.dismiss_context_menu(cx);
                                    if let Some((video, vid_watched)) = watch_video.clone() {
                                        this.core.mark_video_watched(video, !vid_watched);
                                    } else if watch_in_lib {
                                        this.core.mark_library_item_watched(
                                            watch_id.clone(),
                                            !watch_is_watched,
                                        );
                                    } else if let Some(preview) = watch_preview.clone() {
                                        this.core.dispatch_action(Action::Ctx(
                                            ActionCtx::MetaItemMarkAsWatched {
                                                meta_item: preview,
                                                is_watched: !watch_is_watched,
                                            },
                                        ));
                                    }
                                    cx.notify();
                                })),
                            ))
                        },
                    )
                    .when(can_rewind, |panel| {
                        panel.child(menu_row(
                            "ctx-menu-rewind",
                            IconName::X,
                            "Remove from Continue Watching",
                            theme.text_secondary,
                            click_handler(cx.listener(move |this, _, _, cx| {
                                this.dismiss_context_menu(cx);
                                this.core.rewind_library_item(rewind_id.clone());
                                this.push_toast(
                                    ToastKind::Info,
                                    "Removed from Continue Watching",
                                    None,
                                    cx,
                                );
                            })),
                        ))
                    })
                    .with_animation("ctx-menu-fade", obsidian_fade_in(), |el, delta| {
                        el.opacity(delta)
                    }),
            )
    }

    fn render_toast_stack(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = active_theme();
        let accent_ink = if theme.is_light {
            theme.accent_press
        } else {
            theme.accent
        };
        div()
            .absolute()
            .bottom(if self.route.is_player() {
                px(124.0)
            } else {
                px(24.0)
            })
            .left_0()
            .right_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(sp(8.0))
            .children(self.toasts.iter().map(|toast| {
                let toast_id = toast.id;
                let (icon_col, icon) = match toast.kind {
                    ToastKind::Info => (accent_ink, IconName::Info),
                    ToastKind::Success => (theme.status_watched, IconName::Check),
                    ToastKind::Warning => (theme.accent_amber, IconName::RefreshCw),
                    ToastKind::Error => (theme.accent_rose, IconName::X),
                };
                div()
                    .px(sp(14.0))
                    .py(sp(9.0))
                    .rounded(px(RADIUS_PILL))
                    .bg(rgba(theme.glass_popover_bg))
                    .border_1()
                    .border_color(rgba(theme.glass_border))
                    .shadow(glass_capsule_shadow())
                    .flex()
                    .items_center()
                    .gap(sp(10.0))
                    .child(svg_icon(icon, 14.0 * theme.ui_scale, rgb(icon_col)))
                    .child(
                        div()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(sp(13.0))
                            .text_color(rgb(theme.text_primary))
                            .child(toast.title.clone()),
                    )
                    .when_some(toast.detail.clone(), |el, d| {
                        el.child(
                            div()
                                .text_size(sp(12.5))
                                .text_color(rgb(theme.text_secondary))
                                .child(d),
                        )
                    })
                    .child(
                        div()
                            .id(("toast-dismiss", toast_id as usize))
                            .cursor_pointer()
                            .w(sp(22.0))
                            .h(sp(22.0))
                            .rounded(px(RADIUS_PILL))
                            .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.dismiss_toast(toast_id, cx);
                                }),
                            )
                            .child(svg_icon(
                                IconName::X,
                                12.0 * theme.ui_scale,
                                rgb(theme.text_muted),
                            )),
                    )
                    .with_animation(
                        ("toast-fade", toast_id as usize),
                        obsidian_fade_in(),
                        |el, delta| el.opacity(delta),
                    )
            }))
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vp = window.viewport_size();
        self.viewport_size_px = (f32::from(vp.width), f32::from(vp.height));

        if self.exit_fullscreen_on_next_render {
            self.exit_fullscreen_on_next_render = false;
            if window.is_fullscreen() {
                window.toggle_fullscreen();
            }
        }

        self.poster_cache.begin_render_frame();

        let mut evicted_images = self.poster_cache.drain_evicted_images();
        evicted_images.extend(gpui_mpv::drain_evicted_video_images());
        let evicted_count = evicted_images.len();
        if evicted_count > 0 {
            for img in evicted_images {
                let _ = window.drop_image(img);
            }
            if evicted_count >= 16 {
                self.schedule_debounced_heap_trim();
            }
        }

        let theme = active_theme();
        let is_player = self.route.is_player();
        let is_hero_route = matches!(
            self.route,
            AppRoute::Tab(SidebarTab::Home) | AppRoute::MetaDetails { .. }
        );
        let has_custom_window_controls = should_render_custom_window_controls(
            cfg!(target_os = "windows"),
            window.window_decorations(),
            window.is_fullscreen(),
        );

        let target_appearance = if is_player {
            WindowBackgroundAppearance::Transparent
        } else if theme.is_light || theme.preset == ThemePreset::PureOled {
            WindowBackgroundAppearance::Opaque
        } else {
            WindowBackgroundAppearance::Blurred
        };
        if self.last_window_bg_appearance != target_appearance {
            window.set_background_appearance(target_appearance);
            self.last_window_bg_appearance = target_appearance;
            self.window_blur_active = target_appearance == WindowBackgroundAppearance::Blurred;
        }

        let root_bg = if is_player {
            transparent_cutout()
        } else {
            rgb(theme.bg_canvas)
        };

        let transition_key = SharedString::from(self.route.transition_id());
        let resize_handles_el = Self::render_csd_resize_handles(window);

        let active_content = match self.route.clone() {
            AppRoute::Player => self.render_player_view(window, cx).into_any_element(),
            AppRoute::Tab(SidebarTab::Home) => self.render_board_view(cx).into_any_element(),
            AppRoute::Tab(SidebarTab::Discover) => {
                self.render_discover_view(window, cx).into_any_element()
            }
            AppRoute::Tab(SidebarTab::Calendar) => {
                self.render_calendar_view(cx).into_any_element()
            }
            AppRoute::Tab(SidebarTab::Library) => {
                self.render_library_view(window, cx).into_any_element()
            }
            AppRoute::Tab(SidebarTab::Addons) => self.render_addons_view(cx).into_any_element(),
            AppRoute::Tab(SidebarTab::Settings) => self.render_settings_view(cx).into_any_element(),
            AppRoute::Tab(SidebarTab::Account) => self.render_auth_view(cx).into_any_element(),
            AppRoute::MetaDetails {
                type_name,
                id,
                video_id,
            } => self
                .render_meta_details_view(&type_name, &id, video_id.as_deref(), cx)
                .into_any_element(),
            AppRoute::Search { query } => self.render_search_view(&query, cx).into_any_element(),
            AppRoute::Person { person_key, name } => self
                .render_person_view(&person_key, &name, cx)
                .into_any_element(),
        };

        let ambient_layer = if !is_player && theme.ambient_bleed {
            self.sync_ambient_backdrop_for_render();
            self.ambient_hero_backdrop_url
                .as_deref()
                .and_then(|u| self.poster_cache.resolve_ambient_bleed(u))
                .map(|tex| {
                    let bleed_opacity = if theme.is_light { 0.16 } else { 0.21 };
                    let top_fade = (theme.bg_canvas << 8) | 0x26;
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(460.0))
                        .overflow_hidden()
                        .child(
                            img(tex)
                                .size_full()
                                .absolute()
                                .inset_0()
                                .object_fit(ObjectFit::Cover)
                                .opacity(bleed_opacity),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .bg(linear_gradient(
                                    180.0,
                                    linear_color_stop(rgba(top_fade), 0.0),
                                    linear_color_stop(rgb(theme.bg_canvas), 1.0),
                                )),
                        )
                })
        } else {
            None
        };

        let topnav_el = (!is_player).then(|| self.render_floating_topnav(window, cx));
        let quick_peek_el = (!is_player)
            .then(|| self.render_quick_peek_drawer(cx))
            .flatten();

        div()
            .id("ferrite-root")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_global_key_down))
            .on_key_up(cx.listener(Self::on_global_key_up))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.navigate_back(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.navigate_forward(cx);
                }),
            )
            .size_full()
            .bg(root_bg)
            .font(ui_sans(FontWeight::NORMAL))
            .text_color(rgb(theme.text_primary))
            .relative()
            .flex()
            .flex_col()
            .when_some(ambient_layer, |el, bleed| el.child(bleed))
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .bg(transparent_cutout())
                    .child(if is_player {
                        active_content
                    } else {
                        div()
                            .size_full()
                            .flex()
                            .flex_col()
                            .min_h(px(0.0))
                            .when(!is_hero_route, |el| el.pt(sp(76.0)))
                            .child(
                                div()
                                    .flex_1()
                                    .w_full()
                                    .min_h(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .child(active_content),
                            )
                            .with_animation(transition_key, obsidian_fade_in(), |el, delta| {
                                el.opacity(delta)
                            })
                            .into_any_element()
                    }),
            )
            .when_some(topnav_el, |el, bar| el.child(bar))
            .when_some(quick_peek_el, |el, drawer| el.child(drawer))
            .when(self.command_palette_open, |el| {
                el.child(self.render_command_palette(cx))
            })
            .when(self.avatar_menu_open, |el| {
                el.child(self.render_avatar_menu(has_custom_window_controls, cx))
            })
            .when_some(self.context_menu.as_ref(), |el, menu| {
                el.child(self.render_browsing_context_menu(menu, cx))
            })
            .when(!self.toasts.is_empty(), |el| {
                el.child(self.render_toast_stack(cx))
            })
            .when_some(resize_handles_el, |el, handles| el.child(handles))
    }
}

impl AppShell {
    fn sync_os_integrations(&mut self) {
        let in_player = self.route.is_player() && self.playback_session.is_some();
        let video_ready = self.is_player_video_frame_ready();
        let inhibit = should_inhibit_display_sleep(
            self.preferences.prevent_display_sleep,
            in_player,
            video_ready,
            self.player_snapshot.paused,
            self.player_snapshot.eof_reached,
        );
        self.sleep_inhibitor.set_inhibited(inhibit, "Playing video");

        let metadata = if !in_player {
            OsMediaMetadata::default()
        } else if let Some(session) = &self.playback_session {
            let status = if self.player_snapshot.paused || self.player_snapshot.eof_reached {
                OsPlaybackStatus::Paused
            } else {
                OsPlaybackStatus::Playing
            };
            let (can_go_next, poster_url) = self.core.with_model(|m| {
                let has_next = m.player.next_video.is_some();
                let poster = m
                    .meta_details
                    .meta_items
                    .iter()
                    .find_map(|res| res.content.as_ref().and_then(|c| c.ready()))
                    .filter(|item| item.preview.id == session.meta_id)
                    .and_then(|item| item.preview.poster.as_ref().map(|u| u.to_string()))
                    .or_else(|| {
                        m.ctx
                            .library
                            .items
                            .get(&session.meta_id)
                            .and_then(|li| li.poster.as_ref().map(|u| u.to_string()))
                    });
                (has_next, poster)
            });
            OsMediaMetadata {
                status,
                title: session.title.clone(),
                subtitle: session.episode_label.clone().unwrap_or_default(),
                poster_url,
                duration_secs: self.player_snapshot.duration.max(0.0),
                position_secs: self.player_snapshot.time_pos.max(0.0),
                can_seek: self.player_snapshot.duration > 0.0,
                can_go_next,
                can_go_previous: self.player_snapshot.time_pos > 5.0,
            }
        } else {
            OsMediaMetadata::default()
        };

        self.os_media.sync_state(metadata);
    }

    fn handle_os_media_command(&mut self, cmd: OsMediaCommand, cx: &mut Context<Self>) {
        if !self.route.is_player() {
            return;
        }
        match cmd {
            OsMediaCommand::Play => {
                if let Some(mpv) = &self.mpv {
                    let _ = mpv.set_paused(false);
                }
            }
            OsMediaCommand::Pause => {
                if let Some(mpv) = &self.mpv {
                    let _ = mpv.set_paused(true);
                }
            }
            OsMediaCommand::PlayPause => {
                if let Some(mpv) = &self.mpv {
                    let _ = mpv.toggle_pause();
                }
            }
            OsMediaCommand::Stop => self.exit_player(cx),
            OsMediaCommand::Next => {
                let next_vid = self.core.with_model(|m| m.player.next_video.clone());
                if let Some(vid) = next_vid {
                    self.advance_to_next_episode(vid, cx);
                }
            }
            OsMediaCommand::Previous => self.seek_player_absolute(0.0, cx),
            OsMediaCommand::SeekRelativeSecs(delta) => self.seek_player_relative(delta, cx),
            OsMediaCommand::SetPositionSecs(pos) => self.seek_player_absolute(pos, cx),
        }
    }

    pub fn maybe_trigger_stream_enrichments(&mut self, cx: &mut Context<Self>) {
        let AppRoute::MetaDetails {
            type_name,
            id,
            video_id: route_vid,
        } = self.route.clone()
        else {
            return;
        };

        let has_debrid = self.has_debrid_access();
        let memo_slot = &mut self.memoized_meta_streams;

        let snap = self.core.with_model(|model| {
            let (meta, _) = resolve_ready_meta_item(&model.meta_details.meta_items)?;
            if meta.preview.id != id {
                return None;
            }
            let selected_stream_vid = model
                .meta_details
                .selected
                .as_ref()
                .and_then(|s| s.stream_path.as_ref())
                .map(|p| p.id.clone());
            let library_item = model
                .meta_details
                .library_item
                .as_ref()
                .or_else(|| model.ctx.library.items.get(&id));
            let active_video_id = pick_default_series_episode_id(
                meta,
                selected_stream_vid.as_deref().or(route_vid.as_deref()),
                library_item.and_then(|li| li.state.video_id.as_deref()),
            )
            .unwrap_or_else(|| meta.preview.id.clone());

            let (candidates, _, addons_ready, addons_total) = sync_memoized_meta_streams(
                memo_slot,
                &type_name,
                &id,
                &active_video_id,
                has_debrid,
                model
                    .meta_details
                    .meta_streams
                    .iter()
                    .chain(model.meta_details.streams.iter()),
                &model.ctx.profile.addons,
            );

            Some((
                meta.preview.name.clone(),
                active_video_id,
                candidates,
                addons_ready,
                addons_total,
            ))
        });

        let Some((title_hint, active_video_id, addon_candidates, addons_ready, addons_total)) = snap
        else {
            return;
        };

        let key = (type_name, id, active_video_id);
        let key_changed = self.prowlarr_fallback_key.as_ref() != Some(&key);
        if key_changed {
            self.prowlarr_fallback_key = Some(key.clone());
            self.prowlarr_fallback_loading = false;
            self.prowlarr_fallback_attempted = false;
            self.prowlarr_fallback_streams.clear();
            self.prowlarr_fallback_error = None;
            self.prowlarr_request_seq = self.prowlarr_request_seq.wrapping_add(1);
        }

        let cfg = self.core.private_integrations();

        if cfg.has_local_debrid_configured() {
            let hashes: Vec<[u8; 20]> = addon_candidates
                .iter()
                .chain(self.prowlarr_fallback_streams.iter())
                .filter_map(|c| match c.stream.source {
                    StreamSource::Torrent { info_hash, .. } => Some(info_hash),
                    _ => None,
                })
                .collect();
            self.trigger_debrid_cache_check_for_hashes(cfg.active_debrid, hashes, cx);
        }

        if !self.prowlarr_fallback_attempted
            && !self.prowlarr_fallback_loading
            && !cfg.prowlarr_url.trim().is_empty()
            && cfg.prowlarr_mode != ProwlarrFallbackMode::Off
        {
            let all_addons_done = addons_total == 0 || addons_ready >= addons_total;
            let should_trigger_now = match cfg.prowlarr_mode {
                ProwlarrFallbackMode::Off => false,
                ProwlarrFallbackMode::AlwaysMerge => true,
                ProwlarrFallbackMode::FallbackOnOutage => {
                    all_addons_done && addon_candidates.is_empty()
                }
            };

            if should_trigger_now {
                self.trigger_prowlarr_fetch(key, title_hint, cx);
            } else if key_changed
                && cfg.prowlarr_mode == ProwlarrFallbackMode::FallbackOnOutage
                && addon_candidates.is_empty()
            {
                let watch_seq = self.prowlarr_request_seq;
                let watch_key = key;
                let watch_title = title_hint;
                let wait_ms = cfg.prowlarr_timeout_ms.clamp(2500, 6000);
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    cx.background_executor()
                        .timer(Duration::from_millis(wait_ms))
                        .await;
                    let _ = this.update(cx, |shell, cx| {
                        if shell.prowlarr_request_seq != watch_seq
                            || shell.prowlarr_fallback_key.as_ref() != Some(&watch_key)
                            || shell.prowlarr_fallback_attempted
                            || shell.prowlarr_fallback_loading
                        {
                            return;
                        }
                        let still_empty = shell.core.with_model(|m| {
                            m.meta_details
                                .meta_streams
                                .iter()
                                .chain(m.meta_details.streams.iter())
                                .all(|res| match &res.content {
                                    Some(Loadable::Ready(list)) => list.is_empty(),
                                    _ => true,
                                })
                        });
                        if still_empty {
                            shell.trigger_prowlarr_fetch(watch_key, watch_title, cx);
                        }
                    });
                })
                .detach();
            }
        }
    }

    fn trigger_prowlarr_fetch(
        &mut self,
        key: (String, String, String),
        title_hint: String,
        cx: &mut Context<Self>,
    ) {
        if self.prowlarr_fallback_loading || self.prowlarr_fallback_attempted {
            return;
        }
        self.prowlarr_fallback_loading = true;
        self.prowlarr_fallback_attempted = true;
        let req_seq = self.prowlarr_request_seq;
        let core = self.core.clone();
        let (req_type, _req_meta_id, req_vid) = key.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = core
                .fetch_prowlarr_streams(&req_type, &req_vid, Some(&title_hint))
                .await;
            let _ = this.update(cx, |shell, cx| {
                if shell.prowlarr_request_seq != req_seq
                    || shell.prowlarr_fallback_key.as_ref() != Some(&key)
                {
                    return;
                }
                shell.prowlarr_fallback_loading = false;
                match res {
                    Ok(items) => {
                        let cands: Vec<PlaybackCandidate> = items
                            .into_iter()
                            .map(PlaybackCandidate::from_rich_stream_item)
                            .collect();
                        let count = cands.len();
                        let hashes: Vec<[u8; 20]> = cands
                            .iter()
                            .filter_map(|c| match c.stream.source {
                                StreamSource::Torrent { info_hash, .. } => Some(info_hash),
                                _ => None,
                            })
                            .collect();
                        shell.prowlarr_fallback_streams = cands;
                        shell.prowlarr_fallback_error = None;

                        let cfg = shell.core.private_integrations();
                        if cfg.has_local_debrid_configured() && !hashes.is_empty() {
                            shell.trigger_debrid_cache_check_for_hashes(
                                cfg.active_debrid,
                                hashes,
                                cx,
                            );
                        }
                        if count > 0
                            && matches!(cfg.prowlarr_mode, ProwlarrFallbackMode::FallbackOnOutage)
                        {
                            shell.push_toast(
                                ToastKind::Info,
                                format!("Prowlarr fallback found {count} streams"),
                                None,
                                cx,
                            );
                        }
                        cx.notify();
                    }
                    Err(err) => {
                        shell.prowlarr_fallback_error = Some(err);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn trigger_debrid_cache_check_for_hashes(
        &mut self,
        provider: LocalDebridProvider,
        hashes: Vec<[u8; 20]>,
        cx: &mut Context<Self>,
    ) {
        if !provider.is_enabled() || hashes.is_empty() {
            return;
        }
        let mut unchecked = Vec::new();
        for hash in hashes {
            if self.local_debrid_checked_hashes.insert((provider, hash)) {
                unchecked.push(hash);
            }
        }
        if unchecked.is_empty() {
            return;
        }

        let core = self.core.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let map = core.check_local_debrid_cache(unchecked).await;
            if map.is_empty() {
                return;
            }
            let _ = this.update(cx, |shell, cx| {
                let mut changed = false;
                for (hash, status) in map {
                    let prev = shell
                        .local_debrid_cache_map
                        .insert((provider, hash), status);
                    if prev != Some(status) {
                        changed = true;
                    }
                }
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

fn local_debrid_provider_to_idx(provider: LocalDebridProvider) -> usize {
    match provider {
        LocalDebridProvider::None => 0,
        LocalDebridProvider::RealDebrid => 1,
        LocalDebridProvider::TorBox => 2,
        LocalDebridProvider::AllDebrid => 3,
    }
}

fn idx_to_local_debrid_provider(idx: usize) -> LocalDebridProvider {
    match idx {
        1 => LocalDebridProvider::RealDebrid,
        2 => LocalDebridProvider::TorBox,
        3 => LocalDebridProvider::AllDebrid,
        _ => LocalDebridProvider::None,
    }
}

fn prowlarr_mode_to_idx(mode: ProwlarrFallbackMode) -> usize {
    match mode {
        ProwlarrFallbackMode::Off => 0,
        ProwlarrFallbackMode::FallbackOnOutage => 1,
        ProwlarrFallbackMode::AlwaysMerge => 2,
    }
}

fn idx_to_prowlarr_mode(idx: usize) -> ProwlarrFallbackMode {
    match idx {
        0 => ProwlarrFallbackMode::Off,
        2 => ProwlarrFallbackMode::AlwaysMerge,
        _ => ProwlarrFallbackMode::FallbackOnOutage,
    }
}

fn merge_secret_key_draft(
    input: &str,
    configured: &mut bool,
    existing: SecretApiKey,
) -> SecretApiKey {
    let trimmed = input.trim();
    if !trimmed.is_empty() {
        *configured = true;
        SecretApiKey::new(trimmed)
    } else if *configured {
        existing
    } else {
        SecretApiKey::default()
    }
}

fn is_uncached_debrid_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("not cached")
        || lower.contains("not instantly cached")
        || lower.contains("uncached")
}

/// Whether Ferrite should render its own minimize / maximize / close window controls.
///
/// On Linux, GPUI reports `Decorations::Client` when client-side decorations are active.
/// On Windows, `TitlebarOptions { appears_transparent: true, .. }` hides the native Win32
/// titlebar (`gpui_windows` removes it in `WM_NCCALCSIZE`), while `WindowsWindow` inherits
/// `PlatformWindow::window_decorations()`'s default of `Decorations::Server`. Because the
/// native caption buttons are hidden on Windows, Ferrite must render custom controls whenever
/// the window is not fullscreen.
/// On macOS, `appears_transparent: true` preserves native traffic lights, so custom controls
/// remain off when `is_windows` is false and decorations are `Decorations::Server`.
fn should_render_custom_window_controls(
    is_windows: bool,
    decorations: Decorations,
    is_fullscreen: bool,
) -> bool {
    if is_fullscreen {
        return false;
    }
    is_windows || matches!(decorations, Decorations::Client { .. })
}

/// Whether caption button clicks and titlebar drag / double-click / system-menu interactions
/// should defer to GPUI's native Windows non-client message path (`WM_NCHITTEST` +
/// `WM_NCLBUTTONDOWN`/`WM_NCLBUTTONUP` + `DefWindowProcW`) rather than manual `on_mouse_down`
/// handlers.
///
/// On Windows, `Window::zoom_window()` unconditionally calls `ShowWindowAsync(_, SW_MAXIMIZE)`
/// and cannot restore a maximized window, whereas `gpui_windows`'s `WM_NCLBUTTONUP` handler
/// for `HTMAXBUTTON` properly toggles `SW_NORMAL`/`SW_MAXIMIZE` and `WM_NCHITTEST` returning
/// `HTMAXBUTTON` enables Windows 11 Snap Layouts.
pub(crate) fn uses_native_windows_nc_chrome(is_windows: bool) -> bool {
    is_windows
}

const P2P_WATCHDOG_POLL_SECS: u64 = 15;
const P2P_WATCHDOG_HARD_CAP_SECS: u64 = 75;
const P2P_WATCHDOG_ACTIVE_SPEED_BPS: f64 = 1024.0;

#[derive(Debug, PartialEq, Eq)]
enum P2pWatchdogDecision {
    Continue {
        next_prev_fetched: u64,
        next_stalled_ticks: u8,
        sleep_secs: u64,
    },
    Failover {
        reason: String,
    },
}

fn evaluate_p2p_watchdog_tick(
    snapshot: Option<(u64, u32, f64)>,
    elapsed_secs: u64,
    prev_fetched: u64,
    stalled_ticks: u8,
) -> P2pWatchdogDecision {
    let Some((fetched_bytes, live_peers, speed_bps)) = snapshot else {
        return P2pWatchdogDecision::Failover {
            reason: format!("No video frames within {elapsed_secs}s"),
        };
    };

    if live_peers == 0 && fetched_bytes == 0 {
        return P2pWatchdogDecision::Failover {
            reason: format!("Dead swarm: 0 peers after {elapsed_secs}s"),
        };
    }

    if elapsed_secs >= P2P_WATCHDOG_HARD_CAP_SECS {
        return P2pWatchdogDecision::Failover {
            reason: format!("No video frames within {elapsed_secs}s"),
        };
    }

    if fetched_bytes > prev_fetched || speed_bps >= P2P_WATCHDOG_ACTIVE_SPEED_BPS {
        return P2pWatchdogDecision::Continue {
            next_prev_fetched: fetched_bytes,
            next_stalled_ticks: 0,
            sleep_secs: P2P_WATCHDOG_POLL_SECS,
        };
    }

    if live_peers > 0 && stalled_ticks == 0 {
        return P2pWatchdogDecision::Continue {
            next_prev_fetched: fetched_bytes,
            next_stalled_ticks: 1,
            sleep_secs: P2P_WATCHDOG_POLL_SECS,
        };
    }

    if live_peers == 0 {
        P2pWatchdogDecision::Failover {
            reason: format!("Dead swarm: 0 peers after {elapsed_secs}s"),
        }
    } else {
        P2pWatchdogDecision::Failover {
            reason: format!(
                "Swarm stalled ({live_peers} peers, no new bytes) after {elapsed_secs}s"
            ),
        }
    }
}

#[cfg(test)]
mod p2p_watchdog_tests {
    use super::{evaluate_p2p_watchdog_tick, P2pWatchdogDecision};

    #[test]
    fn dead_swarm_zero_peers_zero_bytes_fails_over_immediately() {
        assert_eq!(
            evaluate_p2p_watchdog_tick(Some((0, 0, 0.0)), 18, 0, 0),
            P2pWatchdogDecision::Failover {
                reason: "Dead swarm: 0 peers after 18s".to_owned(),
            }
        );
    }

    #[test]
    fn active_swarm_extends_with_15s_polls_until_hard_cap() {
        assert_eq!(
            evaluate_p2p_watchdog_tick(Some((1_048_576, 12, 256_000.0)), 18, 0, 0),
            P2pWatchdogDecision::Continue {
                next_prev_fetched: 1_048_576,
                next_stalled_ticks: 0,
                sleep_secs: 15,
            }
        );
        assert_eq!(
            evaluate_p2p_watchdog_tick(Some((1_048_576, 12, 2_048.0)), 33, 1_048_576, 1),
            P2pWatchdogDecision::Continue {
                next_prev_fetched: 1_048_576,
                next_stalled_ticks: 0,
                sleep_secs: 15,
            }
        );
        assert_eq!(
            evaluate_p2p_watchdog_tick(Some((8_388_608, 12, 512_000.0)), 78, 4_194_304, 0),
            P2pWatchdogDecision::Failover {
                reason: "No video frames within 78s".to_owned(),
            }
        );
    }

    #[test]
    fn unchoking_peers_get_one_grace_tick_then_failover_on_second_stall() {
        assert_eq!(
            evaluate_p2p_watchdog_tick(Some((0, 6, 0.0)), 18, 0, 0),
            P2pWatchdogDecision::Continue {
                next_prev_fetched: 0,
                next_stalled_ticks: 1,
                sleep_secs: 15,
            }
        );
        assert_eq!(
            evaluate_p2p_watchdog_tick(Some((0, 6, 0.0)), 33, 0, 1),
            P2pWatchdogDecision::Failover {
                reason: "Swarm stalled (6 peers, no new bytes) after 33s".to_owned(),
            }
        );
    }

    #[test]
    fn missing_snapshot_fails_over_at_elapsed_time() {
        assert_eq!(
            evaluate_p2p_watchdog_tick(None, 18, 0, 0),
            P2pWatchdogDecision::Failover {
                reason: "No video frames within 18s".to_owned(),
            }
        );
    }
}

pub fn should_hot_swap_to_online(
    embedded_quality: EmbeddedSubtitleQuality,
    top_online: &FetchedNativeSubtitle,
    search_mode: SubtitleSearchMode,
    force_rematch: bool,
    user_picked_subtitle: bool,
    is_live_placeholder_rescue: bool,
    aligned_to_reference: bool,
) -> bool {
    if user_picked_subtitle && !force_rematch {
        return false;
    }
    if force_rematch {
        return true;
    }
    if search_mode == SubtitleSearchMode::ForcedOnly {
        return top_online.forced
            && (top_online.match_tier.is_compatible_sync() || aligned_to_reference);
    }
    match embedded_quality {
        EmbeddedSubtitleQuality::None | EmbeddedSubtitleQuality::ForcedOnly { .. } => true,
        EmbeddedSubtitleQuality::UsableFull { .. } => {
            is_live_placeholder_rescue
                && (top_online.match_tier.is_trusted_sync() || aligned_to_reference)
                && top_online.content_profile.foreign_placeholder_count == 0
                && top_online.content_profile.foreign_status
                    != ForeignTranslationStatus::MissingForeign
        }
    }
}

#[cfg(test)]
mod window_chrome_tests {
    use gpui::{Decorations, Tiling};

    use super::{should_render_custom_window_controls, uses_native_windows_nc_chrome};

    #[test]
    fn windows_renders_custom_controls_even_with_server_decorations() {
        assert!(should_render_custom_window_controls(
            true,
            Decorations::Server,
            false
        ));
        assert!(should_render_custom_window_controls(
            true,
            Decorations::Client {
                tiling: Tiling::default(),
            },
            false
        ));
    }

    #[test]
    fn fullscreen_hides_custom_controls_on_all_platforms() {
        assert!(!should_render_custom_window_controls(
            true,
            Decorations::Server,
            true
        ));
        assert!(!should_render_custom_window_controls(
            false,
            Decorations::Client {
                tiling: Tiling::default(),
            },
            true
        ));
    }

    #[test]
    fn non_windows_follows_client_decorations_only() {
        // Linux CSD renders custom controls
        assert!(should_render_custom_window_controls(
            false,
            Decorations::Client {
                tiling: Tiling::default(),
            },
            false
        ));
        // macOS (native traffic lights) and Linux SSD do not render custom controls
        assert!(!should_render_custom_window_controls(
            false,
            Decorations::Server,
            false
        ));
    }

    #[test]
    fn native_windows_nc_chrome_is_windows_only() {
        assert!(uses_native_windows_nc_chrome(true));
        assert!(!uses_native_windows_nc_chrome(false));
    }
}

#[cfg(test)]
mod subtitle_upgrade_tests {
    use std::path::PathBuf;

    use stremio_core_runtime::{
        EmbeddedSubtitleQuality, FetchedNativeSubtitle, ForeignTranslationStatus,
        SubtitleContentProfile, SubtitleMatchTier, SubtitleSearchMode,
    };

    use super::should_hot_swap_to_online;

    fn online_sub(
        tier: SubtitleMatchTier,
        hi: bool,
        forced: bool,
        foreign_status: ForeignTranslationStatus,
        placeholder_count: usize,
    ) -> FetchedNativeSubtitle {
        FetchedNativeSubtitle {
            provider: "OpenSubtitles".to_owned(),
            lang: "eng".to_owned(),
            title: "Sub".to_owned(),
            release_name: "Show.S01E01.BluRay".to_owned(),
            match_tier: tier,
            score: 4_000_000,
            local_path: PathBuf::from("/tmp/sub.srt"),
            lookup_elapsed_ms: 120,
            hearing_impaired: hi,
            forced,
            content_profile: SubtitleContentProfile {
                cue_count: 500,
                span_ms: 3_000_000,
                foreign_placeholder_count: placeholder_count,
                sdh_cue_count: 0,
                sdh_ratio_pct: if hi { 30 } else { 0 },
                foreign_status,
            },
        }
    }

    #[test]
    fn respects_user_manual_pick_unless_force_rematch() {
        let translated = online_sub(
            SubtitleMatchTier::ExactHash,
            false,
            false,
            ForeignTranslationStatus::Translated,
            0,
        );
        assert!(!should_hot_swap_to_online(
            EmbeddedSubtitleQuality::None,
            &translated,
            SubtitleSearchMode::Full,
            false,
            true,
            false,
            false,
        ));
        assert!(should_hot_swap_to_online(
            EmbeddedSubtitleQuality::None,
            &translated,
            SubtitleSearchMode::Full,
            true,
            true,
            false,
            false,
        ));
    }

    #[test]
    fn usable_embedded_never_replaced_on_normal_load() {
        let exact_translated = online_sub(
            SubtitleMatchTier::ExactHash,
            false,
            false,
            ForeignTranslationStatus::Translated,
            0,
        );
        let fallback_translated = online_sub(
            SubtitleMatchTier::Fallback,
            false,
            false,
            ForeignTranslationStatus::Translated,
            0,
        );

        let embedded_variants = [
            // Clean SRT/ASS embedded track
            EmbeddedSubtitleQuality::UsableFull {
                full_id: 1,
                full_is_text: true,
                forced_id: None,
                forced_is_text: false,
                is_sdh: false,
            },
            // Blu-ray PGS embedded track
            EmbeddedSubtitleQuality::UsableFull {
                full_id: 2,
                full_is_text: false,
                forced_id: None,
                forced_is_text: false,
                is_sdh: false,
            },
            // Embedded SDH text track
            EmbeddedSubtitleQuality::UsableFull {
                full_id: 3,
                full_is_text: true,
                forced_id: None,
                forced_is_text: false,
                is_sdh: true,
            },
            // Embedded SDH Blu-ray PGS track
            EmbeddedSubtitleQuality::UsableFull {
                full_id: 4,
                full_is_text: false,
                forced_id: Some(5),
                forced_is_text: false,
                is_sdh: true,
            },
        ];

        for quality in embedded_variants {
            assert!(!should_hot_swap_to_online(
                quality,
                &exact_translated,
                SubtitleSearchMode::Full,
                false,
                false,
                false,
                true,
            ));
            assert!(!should_hot_swap_to_online(
                quality,
                &fallback_translated,
                SubtitleSearchMode::Full,
                false,
                false,
                false,
                false,
            ));
        }
    }

    #[test]
    fn live_placeholder_rescue_enforces_strict_sync_and_clean_content() {
        let usable = EmbeddedSubtitleQuality::UsableFull {
            full_id: 1,
            full_is_text: true,
            forced_id: None,
            forced_is_text: false,
            is_sdh: false,
        };

        // Unaligned off-sync or merely cut-compatible tiers are REJECTED
        for tier in [
            SubtitleMatchTier::Fallback,
            SubtitleMatchTier::Conflicting,
            SubtitleMatchTier::CompatibleCut,
            SubtitleMatchTier::SameServiceAndCut,
        ] {
            let sub = online_sub(tier, false, false, ForeignTranslationStatus::Translated, 0);
            assert!(
                !should_hot_swap_to_online(
                    usable,
                    &sub,
                    SubtitleSearchMode::Full,
                    false,
                    false,
                    true,
                    false,
                ),
                "unaligned {tier:?} should be rejected during live rescue"
            );
        }

        // ExactHash, ExactRelease, or reference-aligned clean tracks are ACCEPTED
        for (tier, aligned) in [
            (SubtitleMatchTier::ExactHash, false),
            (SubtitleMatchTier::ExactRelease, false),
            (SubtitleMatchTier::CompatibleCut, true),
            (SubtitleMatchTier::Fallback, true),
        ] {
            let clean_translated =
                online_sub(tier, false, false, ForeignTranslationStatus::Translated, 0);
            assert!(
                should_hot_swap_to_online(
                    usable,
                    &clean_translated,
                    SubtitleSearchMode::Full,
                    false,
                    false,
                    true,
                    aligned,
                ),
                "{tier:?} (aligned={aligned}) clean track should be accepted during live rescue"
            );
        }

        // Tracks with MissingForeign or placeholder_count > 0 are REJECTED even if ExactHash/aligned
        let missing_foreign = online_sub(
            SubtitleMatchTier::ExactHash,
            false,
            false,
            ForeignTranslationStatus::MissingForeign,
            1,
        );
        assert!(!should_hot_swap_to_online(
            usable,
            &missing_foreign,
            SubtitleSearchMode::Full,
            false,
            false,
            true,
            true,
        ));

        let with_placeholders = online_sub(
            SubtitleMatchTier::ExactHash,
            false,
            false,
            ForeignTranslationStatus::Unknown,
            2,
        );
        assert!(!should_hot_swap_to_online(
            usable,
            &with_placeholders,
            SubtitleSearchMode::Full,
            false,
            false,
            true,
            true,
        ));
    }

    #[test]
    fn forced_only_mode_requires_forced_flag_and_compatible_or_aligned_sync() {
        let non_forced = online_sub(
            SubtitleMatchTier::ExactHash,
            false,
            false,
            ForeignTranslationStatus::Unknown,
            0,
        );
        let forced_compatible = online_sub(
            SubtitleMatchTier::CompatibleCut,
            false,
            true,
            ForeignTranslationStatus::Unknown,
            0,
        );
        let forced_fallback = online_sub(
            SubtitleMatchTier::Fallback,
            false,
            true,
            ForeignTranslationStatus::Unknown,
            0,
        );
        let forced_conflicting = online_sub(
            SubtitleMatchTier::Conflicting,
            false,
            true,
            ForeignTranslationStatus::Unknown,
            0,
        );

        assert!(!should_hot_swap_to_online(
            EmbeddedSubtitleQuality::None,
            &non_forced,
            SubtitleSearchMode::ForcedOnly,
            false,
            false,
            false,
            false,
        ));
        assert!(should_hot_swap_to_online(
            EmbeddedSubtitleQuality::None,
            &forced_compatible,
            SubtitleSearchMode::ForcedOnly,
            false,
            false,
            false,
            false,
        ));
        assert!(!should_hot_swap_to_online(
            EmbeddedSubtitleQuality::None,
            &forced_fallback,
            SubtitleSearchMode::ForcedOnly,
            false,
            false,
            false,
            false,
        ));
        assert!(!should_hot_swap_to_online(
            EmbeddedSubtitleQuality::None,
            &forced_conflicting,
            SubtitleSearchMode::ForcedOnly,
            false,
            false,
            false,
            false,
        ));
        // Aligned forced track is accepted even from Fallback tier
        assert!(should_hot_swap_to_online(
            EmbeddedSubtitleQuality::None,
            &forced_fallback,
            SubtitleSearchMode::ForcedOnly,
            false,
            false,
            false,
            true,
        ));
    }

    #[test]
    fn none_or_forced_only_embedded_selects_online_full_track() {
        let fallback_full = online_sub(
            SubtitleMatchTier::Fallback,
            false,
            false,
            ForeignTranslationStatus::Unknown,
            0,
        );
        let conflicting_full = online_sub(
            SubtitleMatchTier::Conflicting,
            false,
            false,
            ForeignTranslationStatus::Unknown,
            0,
        );

        for quality in [
            EmbeddedSubtitleQuality::None,
            EmbeddedSubtitleQuality::ForcedOnly {
                forced_id: 1,
                forced_is_text: true,
            },
        ] {
            assert!(should_hot_swap_to_online(
                quality,
                &fallback_full,
                SubtitleSearchMode::Full,
                false,
                false,
                false,
                false,
            ));
            assert!(should_hot_swap_to_online(
                quality,
                &conflicting_full,
                SubtitleSearchMode::Full,
                false,
                false,
                false,
                false,
            ));
        }
    }
}
