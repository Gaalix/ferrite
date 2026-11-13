use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use gpui::{
    div, point, prelude::*, px, rgb, rgba, AnimationExt, AsyncApp, Context, CursorStyle,
    Decorations, FocusHandle, Focusable, FontWeight, KeyDownEvent, KeyUpEvent, MouseButton,
    PathPromptOptions, Render, RenderImage, ResizeEdge, ScrollDelta, ScrollHandle, SharedString,
    WeakEntity, Window, WindowBackgroundAppearance, WindowControlArea,
};
use gpui_mpv::{
    extract_audio_pcm_s16le_8khz, MpvThumbnailer, PlayerEvent, PlayerSnapshot, TrackType,
};
use stremio_core_runtime::{
    detect_clipboard_addon_url, has_full_embedded_subtitle_for_lang, human_language_name,
    is_native_playable_stream, normalize_addon_url, select_preferred_tracks,
    stremio_core::{
        models::common::Loadable,
        runtime::{
            msg::{Action, Event as CoreEvent},
            RuntimeEvent,
        },
        types::{
            addon::{ResourcePath, ResourceRequest},
            resource::{MetaItem, StreamSource, Video},
        },
    },
    build_unified_editorial_search, subtitle_lang_matches, AudioSpeechFingerprint,
    AudioSpeechWindow, CoreController, DebridCacheStatus, DesktopModelField, EditorialMediaHit,
    EditorialPersonHit, FetchedNativeSubtitle, LocalDebridProvider, NativeSubtitleQuery,
    PersonFilmography, PrivateIntegrationsConfig, ProwlarrFallbackMode, ResolvedNextEpisode,
    SecretApiKey, SeriesTrackPreference, SubtitleAlignmentResult, SubtitleMatchTier,
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
        apply_key_to_text_buffer, build_subtitles_resource_path, default_library_selected,
        detect_addon_url_candidate, is_loopback_p2p_torrent_url, is_scrub_thumbnail_stale,
        merge_and_enrich_stream_candidates, parse_season_episode_from_video_id,
        pick_default_series_episode_id, quantize_scrub_bucket_ms, resolve_ready_meta_item,
        rewrite_loopback_engine_url, switch_engine_storage_mode_on_tokio,
        sync_memoized_meta_streams, trim_process_heap, AddonCategoryFilter, AddonSubTab,
        AppPreferences, AppRoute, BootServices, DebridAccessMode, DiscoverDropdown, FocusedInput,
        LibrarySortOrder, LibraryStatusTab, LibraryTypeFilter, MemoizedMetaStreams,
        NativeSubtitleMatchState, NativeSubtitleMatchStatus, PlaybackCandidate, PlaybackSession,
        PlayerPopover, SettingsCategory, SettingsDropdown, SidebarTab, StorageModeChoice,
        StreamFilterState, ToastKind, ToastMessage, ToneMappingChoice, UpscaleChoice,
        LIBRARY_PAGE_SIZE, MAX_AUTO_FAILOVER_ATTEMPTS, STREAM_PAGE_SIZE,
    },
    theme::{
        editorial_serif, glass_capsule_shadow, keycap, movietracker_mark, obsidian_fade_in,
        transparent_cutout, ui_sans, ACCENT, ACCENT_AMBER, ACCENT_ROSE, ACCENT_SOFT, BG_ELEVATED,
        BG_INPUT, BG_OBSIDIAN, BG_POPOVER_GLASS, BG_SURFACE, BG_TOPNAV, BORDER_POSTER_HOVER,
        BORDER_SOFT, BORDER_STRONG, BORDER_SUBTLE, STATUS_WATCHED, TEXT_MUTED, TEXT_PRIMARY,
        TEXT_SECONDARY,
    },
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

pub struct AppShell {
    pub focus_handle: FocusHandle,
    pub tokio_handle: tokio::runtime::Handle,
    pub core: CoreController,
    pub engine: StreamEngineHandle,
    pub poster_cache: PosterCache,
    pub preferences: AppPreferences,
    pub mpv: Option<gpui_mpv::MpvController>,
    pub thumbnailer: MpvThumbnailer,
    pub player_snapshot: PlayerSnapshot,

    pub route: AppRoute,
    pub nav_history: Vec<AppRoute>,
    pub exit_fullscreen_on_next_render: bool,
    pub window_blur_active: bool,
    pub command_palette_open: bool,
    pub avatar_menu_open: bool,
    pub palette_query: String,
    pub palette_selected_idx: usize,
    pub focused_input: Option<FocusedInput>,

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
    pub discover_loaded_once: bool,
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
    pub discover_grid_scroll_handle: gpui::UniformListScrollHandle,
    pub library_grid_scroll_handle: gpui::UniformListScrollHandle,
    pub episode_list_scroll_handle: gpui::UniformListScrollHandle,
    pub episode_list_scrolled_key: Option<(String, String)>,
    pub meta_addon_dropdown_open: bool,
    pub discover_open_dropdown: Option<DiscoverDropdown>,
    pub discover_catalog_search: String,
    pub expanded_stream_rows: HashSet<usize>,
    pub library_status_tab: LibraryStatusTab,
    pub library_type_filter: LibraryTypeFilter,
    pub library_sort_order: LibrarySortOrder,

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
    pub pending_quick_resume: Option<(String, String, Option<String>)>,

    pub toasts: Vec<ToastMessage>,
    pub next_toast_id: u64,
    pub update_status: UpdateStatus,

    pub active_settings_category: SettingsCategory,
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
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();

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
            exit_fullscreen_on_next_render: false,
            window_blur_active: true,
            command_palette_open: false,
            avatar_menu_open: false,
            palette_query: String::new(),
            palette_selected_idx: 0,
            focused_input: None,
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
            discover_loaded_once: false,
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
            discover_grid_scroll_handle: gpui::UniformListScrollHandle::new(),
            library_grid_scroll_handle: gpui::UniformListScrollHandle::new(),
            episode_list_scroll_handle: gpui::UniformListScrollHandle::new(),
            episode_list_scrolled_key: None,
            meta_addon_dropdown_open: false,
            discover_open_dropdown: None,
            discover_catalog_search: String::new(),
            expanded_stream_rows: HashSet::new(),
            library_status_tab: LibraryStatusTab::Watching,
            library_type_filter: LibraryTypeFilter::All,
            library_sort_order: LibrarySortOrder::Recent,
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
            pending_quick_resume: None,
            toasts: Vec::new(),
            next_toast_id: 1,
            update_status: UpdateStatus::Idle,
            active_settings_category: SettingsCategory::default(),
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
            board_spotlight_idx: 0,
            enriched_attempted: HashSet::new(),
            synthetic_tmdb_meta: None,
        };

        if cli_addon_url.is_some() {
            shell.submit_addon_manifest_lookup(cx);
        }
        shell
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
        {
            self.memoized_meta_streams = None;
            self.auto_selected_episode_for_meta = None;
            self.visible_stream_limit = STREAM_PAGE_SIZE;
            self.expanded_stream_rows.clear();
            self.core.unload_meta_details();
            trim_process_heap();
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
        let is_p2p_torrent = s
            .active_candidate()
            .is_some_and(|c| matches!(c.stream.source, StreamSource::Torrent { .. }))
            && !s.debrid_resolving;
        let timeout_secs = if is_p2p_torrent {
            self.preferences.dead_swarm_timeout_secs.max(18)
        } else {
            self.preferences.dead_swarm_timeout_secs.max(6)
        };

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

    fn handle_core_runtime_event(
        &mut self,
        ev: &crate::CoreRuntimeEventAlias,
        cx: &mut Context<Self>,
    ) {
        match ev {
            RuntimeEvent::NewState(fields) => {
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

                if self.route.is_player()
                    && fields.len() == 1
                    && fields[0] == DesktopModelField::Player
                    && !self.is_player_osd_visible()
                    && !self.player_stats_for_nerds
                {
                    return;
                }

                cx.notify();
            }
            RuntimeEvent::CoreEvent(core_ev) => match core_ev {
                CoreEvent::UserAuthenticated { auth_request: _ } => {
                    self.auth_password_input.clear();
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
                CoreEvent::Error { error, .. } => {
                    self.push_toast(
                        ToastKind::Error,
                        "Stremio notice",
                        Some(format!("{error:?}")),
                        cx,
                    );
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

        if let Some(pending_title) = self.pending_auto_select_sub_title.as_deref() {
            if let Some(track) = snap
                .tracks
                .iter()
                .find(|t| t.track_type == TrackType::Sub && t.title.as_deref() == Some(pending_title))
            {
                if let Some(mpv) = &self.mpv {
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

    pub fn navigate_tab(&mut self, tab: SidebarTab, cx: &mut Context<Self>) {
        if self.route.is_player() {
            self.teardown_active_playback();
        }
        if self.command_palette_open {
            self.core.unload_search();
        }
        self.unload_meta_details_if_leaving(false);

        self.focused_input = None;
        self.command_palette_open = false;
        self.avatar_menu_open = false;
        self.discover_open_dropdown = None;
        self.meta_addon_dropdown_open = false;
        self.settings_open_dropdown = None;
        self.pending_quick_resume = None;

        match tab {
            SidebarTab::Home => {
                self.visible_board_shelves = INITIAL_BOARD_SHELVES;
                self.board_scroll_handle.set_offset(point(px(0.0), px(0.0)));
                self.core.load_board();
            }
            SidebarTab::Discover => {
                self.discover_loaded_once = true;
                self.core.load_discover(None);
            }
            SidebarTab::Library => {
                self.visible_library_limit = LIBRARY_PAGE_SIZE;
                self.library_scroll_handle
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

        self.nav_history.clear();
        self.route = AppRoute::Tab(tab);
        cx.notify();
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
        }
        if !matches!(self.route, AppRoute::Player) {
            self.nav_history.push(self.route.clone());
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

        self.avatar_menu_open = false;
        self.discover_open_dropdown = None;
        self.meta_addon_dropdown_open = false;
        self.selected_season = None;
        self.auto_selected_episode_for_meta = resolved_video_id
            .as_ref()
            .map(|vid| (id.clone(), vid.clone()));
        self.stream_filter.reset();
        self.memoized_meta_streams = None;
        self.synthetic_tmdb_meta = None;
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
        if self.route.is_player() {
            self.teardown_active_playback();
        }
        if !matches!(self.route, AppRoute::Player | AppRoute::Search { .. }) {
            self.nav_history.push(self.route.clone());
        }
        self.avatar_menu_open = false;
        self.command_palette_open = false;
        self.discover_open_dropdown = None;
        self.meta_addon_dropdown_open = false;
        self.settings_open_dropdown = None;

        let trimmed = query.trim().to_owned();
        self.full_search_query = trimmed.clone();
        self.palette_query = trimmed.clone();
        self.route = AppRoute::Search { query: trimmed };
        self.focused_input = Some(FocusedInput::FullSearchInput);
        self.search_scroll_handle.set_offset(point(px(0.0), px(0.0)));
        self.refresh_full_search(cx);
    }

    pub fn refresh_full_search(&mut self, cx: &mut Context<Self>) {
        self.full_search_seq = self.full_search_seq.wrapping_add(1);
        let seq = self.full_search_seq;
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

        self.full_search_results = self.core.editorial_search_snapshot(&q);
        self.core.search(q.clone());

        let has_tmdb = {
            let cfg = self.core.private_integrations();
            cfg.tmdb_enabled && cfg.has_tmdb_configured()
        };
        self.full_search_loading_tmdb = has_tmdb;
        if let Some(top) = self.full_search_results.top_match.clone() {
            self.request_background_enrichment_if_needed(&top.media_type, &top.canonical_id, cx);
        }
        cx.notify();

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
        }
        if !matches!(self.route, AppRoute::Player) {
            self.nav_history.push(self.route.clone());
        }
        self.avatar_menu_open = false;
        self.command_palette_open = false;
        self.discover_open_dropdown = None;
        self.meta_addon_dropdown_open = false;
        self.settings_open_dropdown = None;

        self.person_request_seq = self.person_request_seq.wrapping_add(1);
        let seq = self.person_request_seq;
        self.person_loading = true;
        self.active_person_filmography = None;
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
            if let Some(origin_route) = self.nav_history.pop() {
                self.route = origin_route;
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
        self.auto_selected_episode_for_meta = None;
        self.core.unload_meta_details();
        trim_process_heap();
    }

    pub fn navigate_back(&mut self, cx: &mut Context<Self>) {
        if self.route.is_player() {
            self.exit_player(cx);
            return;
        }
        self.discover_open_dropdown = None;
        self.meta_addon_dropdown_open = false;
        self.pending_quick_resume = None;
        let next_route = self
            .nav_history
            .pop()
            .unwrap_or(AppRoute::Tab(SidebarTab::Home));
        let keep_meta = matches!(
            next_route,
            AppRoute::MetaDetails { .. } | AppRoute::Player
        );
        self.unload_meta_details_if_leaving(keep_meta);
        self.route = next_route.clone();
        if let AppRoute::Person { person_key, name } = next_route {
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
        cx.notify();
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
            self.nav_history.push(self.route.clone());
        }

        self.poster_cache.trim_for_playback();
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
        self.command_palette_open = false;
        self.expanded_stream_rows.clear();
        self.next_episode_prewarmed = false;
        self.prewarmed_next = None;
        self.prewarm_fetch_requested_vid = None;
        self.prewarmed_next_direct_url = None;
        self.native_sub_match.reset();
        self.pending_auto_select_sub_title = None;
        self.user_picked_subtitle = false;
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
        let next_route = self
            .nav_history
            .pop()
            .unwrap_or(AppRoute::Tab(SidebarTab::Home));
        if !matches!(next_route, AppRoute::MetaDetails { .. }) {
            self.memoized_meta_streams = None;
            self.auto_selected_episode_for_meta = None;
            self.visible_stream_limit = STREAM_PAGE_SIZE;
            self.core.unload_meta_details();
            trim_process_heap();
        }
        self.route = next_route;
        cx.notify();
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
        self.user_picked_subtitle = false;
        self.sync_os_integrations();
        trim_process_heap();
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
        if self.player_popover == Some(pop) {
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
        self.playback_speed = speed.clamp(0.25, 4.0);
        if let Some(mpv) = &self.mpv {
            let _ = mpv.set_speed(self.playback_speed);
        }
        self.player_popover = None;
        self.schedule_osd_autohide_timer(cx);
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
            .set_first_byte_timeout(Duration::from_secs(secs.max(12)));
        let _ = self.preferences.save();
        cx.notify();
    }

    pub fn set_debrid_access_mode(&mut self, mode: DebridAccessMode, cx: &mut Context<Self>) {
        self.preferences.debrid_access_mode = mode;
        if !self.has_debrid_access() {
            self.stream_filter.cached_only = false;
        }
        self.memoized_meta_streams = None;
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
        }
        cx.notify();
    }

    pub fn clear_settings_search(&mut self, cx: &mut Context<Self>) {
        self.settings_search_query.clear();
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

    pub fn set_subtitle_background_pill(&mut self, pill: bool, cx: &mut Context<Self>) {
        self.preferences.subtitle_background_pill = pill;
        self.update_subtitle_style_prefs(cx);
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

    fn has_usable_embedded_preferred_subtitle(&self, tracks: &[gpui_mpv::MediaTrack]) -> bool {
        let preferred = self
            .active_series_id()
            .and_then(|series_id| self.core.get_series_track_pref(&series_id))
            .and_then(|pref| pref.sub_lang)
            .unwrap_or_else(|| self.normalized_preferred_sub_lang());
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
        if !subtitles_wanted {
            return;
        }

        if self.has_usable_embedded_preferred_subtitle(&self.player_snapshot.tracks) {
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
            return;
        }

        if self.preferences.auto_match_native_subtitles
            && matches!(self.native_sub_match.status, NativeSubtitleMatchStatus::Idle)
        {
            self.trigger_native_subtitle_auto_match(false, cx);
        } else {
            self.attach_fallback_addon_subtitles_if_needed(cx);
        }
    }

    fn format_native_sub_label(sub: &FetchedNativeSubtitle, aligned: Option<&SubtitleAlignmentResult>) -> String {
        let base = Self::native_sub_base_label(sub);
        if let Some(align) = aligned {
            format!("{base} · {}", Self::format_alignment_badge(align))
        } else if sub.match_tier == SubtitleMatchTier::ExactHash {
            format!("{base} · hash match")
        } else {
            base
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
        let is_sdh = sub.title.to_ascii_uppercase().contains("SDH")
            || sub.release_name.to_ascii_uppercase().contains("SDH");
        let lang_title = if is_sdh {
            format!("{base_lang} SDH")
        } else {
            base_lang
        };
        let provider = Self::clean_subtitle_provider_name(&sub.provider);
        format!("{lang_title} — {provider}")
    }

    pub fn trigger_native_subtitle_auto_match(&mut self, force_rematch: bool, cx: &mut Context<Self>) {
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
        let preferred_lang = self.normalized_preferred_sub_lang();
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

        self.native_sub_match.status = NativeSubtitleMatchStatus::Searching;
        if force_rematch {
            self.native_sub_match.last_alignment_badge = None;
        }
        cx.notify();

        let core = self.core.clone();
        let tokio_handle = self.tokio_handle.clone();
        let max_fetch = if force_rematch { 3 } else { 2 };
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
                    };
                    let fetched = core.search_and_fetch_native_subtitles(query, max_fetch).await;
                    let exact_ref = fetched
                        .iter()
                        .find(|s| s.match_tier == SubtitleMatchTier::ExactHash)
                        .cloned();

                    let auto_aligned = if let (Some(top), Some(ref_sub)) =
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
                    shell.native_sub_match.status =
                        NativeSubtitleMatchStatus::Failed("Subtitle search task failed".to_owned());
                    cx.notify();
                    return;
                };

                shell.native_sub_match.reference_sub_path = exact_ref_path;
                shell.native_sub_match.fetched_subs = fetched.clone();

                if fetched.is_empty() {
                    if !matches!(
                        shell.native_sub_match.status,
                        NativeSubtitleMatchStatus::LocalFile { .. }
                    ) {
                        shell.native_sub_match.status = NativeSubtitleMatchStatus::NoMatches;
                        shell.attach_fallback_addon_subtitles_if_needed(cx);
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

                let has_embedded_preferred =
                    shell.has_usable_embedded_preferred_subtitle(&shell.player_snapshot.tracks);
                let top_sub = &fetched[0];
                let should_select_top = (force_rematch
                    || (!shell.user_picked_subtitle && !has_embedded_preferred))
                    && shell.preferences.auto_select_subtitles;
                let mut chosen_title: Option<String> = None;

                if let Some((aligned_path, alignment)) = auto_aligned {
                    let badge = Self::format_alignment_badge(&alignment);
                    let label = Self::format_native_sub_label(top_sub, Some(&alignment));
                    if let Some(mpv) = &shell.mpv {
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
                    shell.native_sub_match.active_local_sub_path = Some(aligned_path);
                    shell.native_sub_match.synced_track_title = Some(label.clone());
                    shell.native_sub_match.last_alignment_badge = Some(badge.clone());
                    if !align_local_mode {
                        shell.native_sub_match.status = NativeSubtitleMatchStatus::Matched {
                            tier: top_sub.match_tier,
                            elapsed_ms: top_sub.lookup_elapsed_ms,
                        };
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
                        shell.native_sub_match.active_local_sub_path = Some(sub.local_path.clone());
                        shell.native_sub_match.status = NativeSubtitleMatchStatus::Matched {
                            tier: sub.match_tier,
                            elapsed_ms: sub.lookup_elapsed_ms,
                        };
                        chosen_title = Some(label);
                    }
                }

                if should_select_top {
                    if !align_local_mode {
                        shell.native_sub_match.local_source_sub_path = None;
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
                }
                if force_rematch && !align_local_mode {
                    shell.push_toast(
                        ToastKind::Success,
                        format!("Matched {} subtitles", fetched.len()),
                        shell.native_sub_match.status_badge(),
                        cx,
                    );
                }

                if shell.native_sub_match.reference_sub_path.is_none()
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
            || matches!(
                self.native_sub_match.status,
                NativeSubtitleMatchStatus::AudioSynced { .. }
                    | NativeSubtitleMatchStatus::Searching
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
            || self.has_usable_embedded_preferred_subtitle(&self.player_snapshot.tracks)
        {
            return false;
        }
        let has_selected_embedded = self
            .player_snapshot
            .tracks
            .iter()
            .any(|t| t.track_type == TrackType::Sub && !t.external && t.selected);
        !has_selected_embedded
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
            }
            2 => {
                self.torbox_key_input.clear();
                self.torbox_key_configured = false;
            }
            3 => {
                self.alldebrid_key_input.clear();
                self.alldebrid_key_configured = false;
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
        self.prowlarr_test_status = None;
        self.persist_private_integrations_draft(cx);
    }

    pub fn clear_opensubtitles_key(&mut self, cx: &mut Context<Self>) {
        self.opensubtitles_key_input.clear();
        self.opensubtitles_key_configured = false;
        self.persist_private_integrations_draft(cx);
    }

    pub fn clear_subdl_key(&mut self, cx: &mut Context<Self>) {
        self.subdl_key_input.clear();
        self.subdl_key_configured = false;
        self.persist_private_integrations_draft(cx);
    }

    pub fn clear_tmdb_key(&mut self, cx: &mut Context<Self>) {
        self.tmdb_key_input.clear();
        self.tmdb_key_configured = false;
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
        self.poster_cache.clear_failures();
        self.poster_cache.trim_for_playback();
        let cache_root = self.poster_cache.cache_root().to_path_buf();
        if let Ok(entries) = std::fs::read_dir(&cache_root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let _ = std::fs::remove_file(path);
                } else if path.is_dir() {
                    let _ = std::fs::remove_dir_all(path);
                }
            }
        }
        trim_process_heap();
        self.push_toast(
            ToastKind::Success,
            "Metadata & image cache cleared",
            None,
            cx,
        );
    }

    pub fn open_command_palette(&mut self, cx: &mut Context<Self>) {
        self.avatar_menu_open = false;
        self.discover_open_dropdown = None;
        self.meta_addon_dropdown_open = false;
        self.command_palette_open = true;
        self.palette_selected_idx = 0;
        self.focused_input = Some(FocusedInput::CommandPalette);
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
        trim_process_heap();
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
                    cx.notify();
                    return;
                }
                "down" => {
                    let count = self.build_palette_entries().len();
                    if count > 0 {
                        self.palette_selected_idx = (self.palette_selected_idx + 1).min(count - 1);
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
                    let clip = cx.read_from_clipboard().and_then(|c| c.text());
                    if apply_key_to_text_buffer(&mut self.palette_query, event, clip) {
                        self.palette_selected_idx = 0;
                        self.core.search(self.palette_query.clone());
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
                        cx.notify();
                        return;
                    }
                    self.focused_input = None;
                    if target == FocusedInput::DiscoverCatalogSearch {
                        self.discover_open_dropdown = None;
                    }
                    cx.notify();
                    return;
                }
                "tab" => {
                    self.focused_input = match target {
                        FocusedInput::AuthEmail => Some(FocusedInput::AuthPassword),
                        FocusedInput::AuthPassword => Some(FocusedInput::AuthEmail),
                        FocusedInput::AddonManifestUrl => Some(FocusedInput::AddonSearch),
                        FocusedInput::AddonSearch => Some(FocusedInput::AddonManifestUrl),
                        FocusedInput::RealDebridApiKey => Some(FocusedInput::TorBoxApiKey),
                        FocusedInput::TorBoxApiKey => Some(FocusedInput::AllDebridApiKey),
                        FocusedInput::AllDebridApiKey => Some(FocusedInput::ProwlarrUrl),
                        FocusedInput::ProwlarrUrl => Some(FocusedInput::ProwlarrApiKey),
                        FocusedInput::ProwlarrApiKey => Some(FocusedInput::SettingsOpenSubtitlesKey),
                        FocusedInput::SettingsOpenSubtitlesKey => {
                            Some(FocusedInput::SettingsSubdlKey)
                        }
                        FocusedInput::SettingsSubdlKey => {
                            Some(FocusedInput::SettingsTmdbApiKey)
                        }
                        FocusedInput::SettingsTmdbApiKey => {
                            Some(FocusedInput::RealDebridApiKey)
                        }
                        other => Some(other),
                    };
                    cx.notify();
                    return;
                }
                "enter" => {
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
                    let clip = cx.read_from_clipboard().and_then(|c| c.text());
                    let mut integration_edited = false;
                    let changed = match target {
                        FocusedInput::AuthEmail => {
                            apply_key_to_text_buffer(&mut self.auth_email_input, event, clip)
                        }
                        FocusedInput::AuthPassword => {
                            apply_key_to_text_buffer(&mut self.auth_password_input, event, clip)
                        }
                        FocusedInput::AddonManifestUrl => {
                            apply_key_to_text_buffer(&mut self.addon_url_input, event, clip)
                        }
                        FocusedInput::AddonSearch => {
                            apply_key_to_text_buffer(&mut self.addon_search_query, event, clip)
                        }
                        FocusedInput::DiscoverCatalogSearch => {
                            apply_key_to_text_buffer(&mut self.discover_catalog_search, event, clip)
                        }
                        FocusedInput::SettingsSearch => {
                            apply_key_to_text_buffer(&mut self.settings_search_query, event, clip)
                        }
                        FocusedInput::RealDebridApiKey => {
                            let c = apply_key_to_text_buffer(
                                &mut self.real_debrid_key_input,
                                event,
                                clip,
                            );
                            if c {
                                self.real_debrid_key_configured =
                                    !self.real_debrid_key_input.trim().is_empty();
                                integration_edited = true;
                            }
                            c
                        }
                        FocusedInput::TorBoxApiKey => {
                            let c =
                                apply_key_to_text_buffer(&mut self.torbox_key_input, event, clip);
                            if c {
                                self.torbox_key_configured =
                                    !self.torbox_key_input.trim().is_empty();
                                integration_edited = true;
                            }
                            c
                        }
                        FocusedInput::AllDebridApiKey => {
                            let c = apply_key_to_text_buffer(
                                &mut self.alldebrid_key_input,
                                event,
                                clip,
                            );
                            if c {
                                self.alldebrid_key_configured =
                                    !self.alldebrid_key_input.trim().is_empty();
                                integration_edited = true;
                            }
                            c
                        }
                        FocusedInput::ProwlarrUrl => {
                            let c =
                                apply_key_to_text_buffer(&mut self.prowlarr_url_input, event, clip);
                            if c {
                                integration_edited = true;
                            }
                            c
                        }
                        FocusedInput::ProwlarrApiKey => {
                            let c =
                                apply_key_to_text_buffer(&mut self.prowlarr_key_input, event, clip);
                            if c {
                                self.prowlarr_key_configured =
                                    !self.prowlarr_key_input.trim().is_empty();
                                integration_edited = true;
                            }
                            c
                        }
                        FocusedInput::SettingsOpenSubtitlesKey => {
                            let c = apply_key_to_text_buffer(
                                &mut self.opensubtitles_key_input,
                                event,
                                clip,
                            );
                            if c {
                                self.opensubtitles_key_configured =
                                    !self.opensubtitles_key_input.trim().is_empty();
                                integration_edited = true;
                            }
                            c
                        }
                        FocusedInput::SettingsSubdlKey => {
                            let c =
                                apply_key_to_text_buffer(&mut self.subdl_key_input, event, clip);
                            if c {
                                self.subdl_key_configured =
                                    !self.subdl_key_input.trim().is_empty();
                                integration_edited = true;
                            }
                            c
                        }
                        FocusedInput::SettingsTmdbApiKey => {
                            let c =
                                apply_key_to_text_buffer(&mut self.tmdb_key_input, event, clip);
                            if c {
                                self.tmdb_key_configured = !self.tmdb_key_input.trim().is_empty();
                                integration_edited = true;
                            }
                            c
                        }
                        FocusedInput::FullSearchInput => {
                            let c = apply_key_to_text_buffer(
                                &mut self.full_search_query,
                                event,
                                clip,
                            );
                            if c {
                                self.palette_query = self.full_search_query.clone();
                                self.refresh_full_search(cx);
                            }
                            false
                        }
                        FocusedInput::CommandPalette => false,
                    };
                    if integration_edited {
                        self.persist_private_integrations_draft(cx);
                    } else if changed {
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
                    if self.player_popover.is_some() {
                        self.player_popover = None;
                        self.schedule_osd_autohide_timer(cx);
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
            if self.avatar_menu_open {
                self.avatar_menu_open = false;
                cx.notify();
            } else if self.discover_open_dropdown.is_some() {
                self.discover_open_dropdown = None;
                cx.notify();
            } else if self.meta_addon_dropdown_open {
                self.meta_addon_dropdown_open = false;
                cx.notify();
            } else if self.settings_open_dropdown.is_some() {
                self.settings_open_dropdown = None;
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
            let tab = match ks.key.as_str() {
                "1" => Some(SidebarTab::Home),
                "2" => Some(SidebarTab::Discover),
                "3" => Some(SidebarTab::Library),
                "4" => Some(SidebarTab::Addons),
                "5" => Some(SidebarTab::Settings),
                "6" => Some(SidebarTab::Account),
                _ => None,
            };
            if let Some(t) = tab {
                self.navigate_tab(t, cx);
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
        if !matches!(window.window_decorations(), Decorations::Client { .. })
            || window.is_fullscreen()
        {
            return None;
        }

        let controls = window.window_controls();
        let is_maximized = window.is_maximized();
        let min_id = SharedString::from(format!("{id_prefix}-csd-min"));
        let max_id = SharedString::from(format!("{id_prefix}-csd-max"));
        let close_id = SharedString::from(format!("{id_prefix}-csd-close"));
        let min_grp = SharedString::from(format!("{id_prefix}-grp-min"));
        let max_grp = SharedString::from(format!("{id_prefix}-grp-max"));
        let close_grp = SharedString::from(format!("{id_prefix}-grp-close"));

        let base = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .when(in_glass_capsule, |el| {
                el.h(px(36.0))
                    .px(px(4.0))
                    .rounded(px(999.0))
                    .bg(rgba(crate::theme::BG_OSD_GLASS))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
            });

        Some(
            base.when(controls.minimize, |el| {
                el.child(
                    div()
                        .id(min_id)
                        .group(min_grp.clone())
                        .window_control_area(WindowControlArea::Min)
                        .cursor_pointer()
                        .w(px(32.0))
                        .h(px(32.0))
                        .rounded(px(6.0))
                        .hover(|s| s.bg(rgb(BG_ELEVATED)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            cx.stop_propagation();
                            window.minimize_window();
                        })
                        .child(
                            svg_icon(IconName::Minus, 14.0, rgb(TEXT_SECONDARY))
                                .group_hover(min_grp, |s| s.text_color(rgb(TEXT_PRIMARY))),
                        ),
                )
            })
            .when(controls.maximize, |el| {
                el.child(
                    div()
                        .id(max_id)
                        .group(max_grp.clone())
                        .window_control_area(WindowControlArea::Max)
                        .cursor_pointer()
                        .w(px(32.0))
                        .h(px(32.0))
                        .rounded(px(6.0))
                        .hover(|s| s.bg(rgb(BG_ELEVATED)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            cx.stop_propagation();
                            window.zoom_window();
                        })
                        .child(
                            svg_icon(
                                if is_maximized {
                                    IconName::RestoreWindow
                                } else {
                                    IconName::MaximizeWindow
                                },
                                13.5,
                                rgb(TEXT_SECONDARY),
                            )
                            .group_hover(max_grp, |s| s.text_color(rgb(TEXT_PRIMARY))),
                        ),
                )
            })
            .child(
                div()
                    .id(close_id)
                    .group(close_grp.clone())
                    .window_control_area(WindowControlArea::Close)
                    .cursor_pointer()
                    .w(px(32.0))
                    .h(px(32.0))
                    .rounded(px(6.0))
                    .hover(|s| s.bg(rgba(0xf8717124)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        cx.stop_propagation();
                        window.remove_window();
                    })
                    .child(
                        svg_icon(IconName::X, 14.5, rgb(TEXT_SECONDARY))
                            .group_hover(close_grp, |s| s.text_color(rgb(ACCENT_ROSE))),
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

    fn render_topnav_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active_tab = self.route.active_sidebar_tab();
        let is_fullscreen = window.is_fullscreen();
        let csd_controls = Self::render_csd_window_controls(window, "topnav", false);
        let (signed_in_email, avatar_initial) = self.core.with_model(|m| {
            let email = m.ctx.profile.auth.as_ref().map(|a| a.user.email.clone());
            let init = email
                .as_deref()
                .and_then(|e| e.chars().next())
                .map(|c| c.to_ascii_uppercase().to_string());
            (email, init)
        });

        let nav_tabs = SidebarTab::TITLEBAR_TABS.into_iter().map(|t| (t, t.label()));

        let query_empty = self.palette_query.is_empty();
        let search_text: SharedString = if self.command_palette_open {
            if query_empty {
                "Search movies, shows, commands...|".into()
            } else {
                format!("{}|", self.palette_query).into()
            }
        } else if query_empty {
            "Search movies, shows...".into()
        } else {
            self.palette_query.clone().into()
        };

        div()
            .id("topnav-titlebar")
            .window_control_area(WindowControlArea::Drag)
            .w_full()
            .h(px(64.0))
            .flex_shrink_0()
            .pl(px(32.0))
            .pr(if csd_controls.is_some() {
                px(16.0)
            } else {
                px(32.0)
            })
            .bg(rgba(BG_TOPNAV))
            .border_b_1()
            .border_color(rgba(BORDER_SOFT))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(24.0))
            .on_mouse_down(MouseButton::Left, |event, window, _cx| {
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(28.0))
                    .child(
                        div()
                            .id("topnav-brand")
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.navigate_tab(SidebarTab::Home, cx);
                                }),
                            )
                            .child(movietracker_mark(28.0))
                            .child(
                                div()
                                    .font(editorial_serif())
                                    .text_size(px(22.0))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .child("Ferrite"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .children(nav_tabs.into_iter().enumerate().map(|(idx, (tab, label))| {
                                let is_active = active_tab == Some(tab);
                                div()
                                    .id(("topnav-link", idx))
                                    .cursor_pointer()
                                    .h(px(34.0))
                                    .px(px(12.0))
                                    .rounded(px(6.0))
                                    .when(is_active, |el| el.bg(rgb(BG_ELEVATED)))
                                    .when(!is_active, |el| {
                                        el.hover(|s| s.text_color(rgb(TEXT_PRIMARY)))
                                    })
                                    .flex()
                                    .items_center()
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(px(14.0))
                                    .text_color(if is_active {
                                        rgb(TEXT_PRIMARY)
                                    } else {
                                        rgb(TEXT_SECONDARY)
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.navigate_tab(tab, cx);
                                        }),
                                    )
                                    .child(label)
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .id("topnav-search-box")
                            .cursor_pointer()
                            .w(px(340.0))
                            .h(px(36.0))
                            .px(px(12.0))
                            .rounded(px(8.0))
                            .bg(if self.command_palette_open {
                                rgb(BG_ELEVATED)
                            } else {
                                rgb(BG_INPUT)
                            })
                            .border_1()
                            .border_color(if self.command_palette_open {
                                rgba(BORDER_POSTER_HOVER)
                            } else {
                                rgba(BORDER_SUBTLE)
                            })
                            .hover(|s| {
                                s.border_color(if self.command_palette_open {
                                    rgba(BORDER_POSTER_HOVER)
                                } else {
                                    rgba(BORDER_STRONG)
                                })
                            })
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
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
                                    .gap(px(9.0))
                                    .child(svg_icon(
                                        IconName::Search,
                                        14.5,
                                        if self.command_palette_open {
                                            rgb(ACCENT)
                                        } else {
                                            rgb(TEXT_MUTED)
                                        },
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .font(ui_sans(FontWeight::NORMAL))
                                            .text_size(px(13.5))
                                            .text_color(if query_empty {
                                                rgb(TEXT_MUTED)
                                            } else {
                                                rgb(TEXT_PRIMARY)
                                            })
                                            .truncate()
                                            .child(search_text),
                                    ),
                            )
                            .child(if !query_empty {
                                div()
                                    .id("topnav-search-clear")
                                    .cursor_pointer()
                                    .p(px(3.0))
                                    .rounded(px(4.0))
                                    .hover(|s| s.bg(rgba(0xffffff10)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.palette_query.clear();
                                            this.palette_selected_idx = 0;
                                            cx.notify();
                                        }),
                                    )
                                    .child(svg_icon(IconName::X, 12.0, rgb(TEXT_MUTED)))
                                    .into_any_element()
                            } else {
                                keycap("Ctrl K").into_any_element()
                            }),
                    )
                    .when_some(
                        self.update_status
                            .available_version()
                            .map(|v| (v.to_owned(), self.update_status.is_ready_to_restart())),
                        |row, (ver, ready_restart)| {
                            row.child(
                                div()
                                    .id("topnav-update-badge")
                                    .cursor_pointer()
                                    .h(px(30.0))
                                    .px(px(11.0))
                                    .rounded(px(999.0))
                                    .bg(rgba(ACCENT_SOFT))
                                    .border_1()
                                    .border_color(rgb(ACCENT))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(px(12.0))
                                    .text_color(rgb(ACCENT))
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
                                    .child(svg_icon(IconName::RefreshCw, 12.0, rgb(ACCENT)))
                                    .child(if ready_restart {
                                        format!("Restart v{ver}")
                                    } else {
                                        format!("Update v{ver}")
                                    }),
                            )
                        },
                    )
                    .child(
                        div()
                            .id("topnav-avatar-btn")
                            .cursor_pointer()
                            .w(px(32.0))
                            .h(px(32.0))
                            .rounded(px(999.0))
                            .bg(if signed_in_email.is_some() {
                                rgb(ACCENT)
                            } else {
                                rgb(BG_ELEVATED)
                            })
                            .border_1()
                            .border_color(if self.avatar_menu_open {
                                rgba(BORDER_POSTER_HOVER)
                            } else {
                                rgba(BORDER_SUBTLE)
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.command_palette_open = false;
                                    this.avatar_menu_open = !this.avatar_menu_open;
                                    cx.notify();
                                }),
                            )
                            .child(if let Some(ch) = avatar_initial {
                                div()
                                    .font(ui_sans(FontWeight::BOLD))
                                    .text_size(px(13.0))
                                    .text_color(rgb(BG_OBSIDIAN))
                                    .child(ch)
                                    .into_any_element()
                            } else {
                                svg_icon(IconName::User, 15.0, rgb(TEXT_SECONDARY))
                                    .into_any_element()
                            }),
                    )
                    .when(is_fullscreen, |row| {
                        row.child(
                            div()
                                .id("topnav-btn-exit-fullscreen")
                                .cursor_pointer()
                                .h(px(34.0))
                                .px(px(12.0))
                                .rounded(px(999.0))
                                .bg(rgb(BG_ELEVATED))
                                .border_1()
                                .border_color(rgba(BORDER_SUBTLE))
                                .hover(|s| s.border_color(rgba(BORDER_STRONG)))
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .font(ui_sans(FontWeight::MEDIUM))
                                .text_size(px(12.5))
                                .text_color(rgb(TEXT_PRIMARY))
                                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                    cx.stop_propagation();
                                    window.toggle_fullscreen();
                                })
                                .child(svg_icon(IconName::Minimize, 13.0, rgb(ACCENT)))
                                .child("Exit fullscreen"),
                        )
                    })
                    .when_some(csd_controls, |row, controls_el| {
                        row.child(
                            div()
                                .w(px(1.0))
                                .h(px(18.0))
                                .flex_shrink_0()
                                .bg(rgba(BORDER_SOFT)),
                        )
                        .child(controls_el)
                    }),
            )
    }

    fn render_avatar_menu(&self, is_csd: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let email_opt = self.core.with_model(|m| {
            m.ctx.profile.auth.as_ref().map(|a| a.user.email.clone())
        });

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
                    .absolute()
                    .top(px(60.0))
                    .right(if is_csd { px(136.0) } else { px(32.0) })
                    .w(px(236.0))
                    .p(px(6.0))
                    .rounded(px(12.0))
                    .bg(rgba(BG_POPOVER_GLASS))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .shadow(glass_capsule_shadow())
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px(px(10.0))
                            .py(px(8.0))
                            .border_b_1()
                            .border_color(rgba(BORDER_SOFT))
                            .mb(px(4.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(TEXT_PRIMARY))
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
                                    .text_color(rgb(TEXT_MUTED))
                                    .truncate()
                                    .child(
                                        email_opt
                                            .clone()
                                            .unwrap_or_else(|| "Sign in to sync library".to_owned()),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("avatar-menu-account")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(6.0))
                            .hover(|s| s.bg(rgb(BG_ELEVATED)).text_color(rgb(TEXT_PRIMARY)))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .text_size(px(13.5))
                            .text_color(rgb(TEXT_SECONDARY))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.navigate_tab(SidebarTab::Account, cx);
                                }),
                            )
                            .child(svg_icon(IconName::User, 14.5, rgb(TEXT_SECONDARY)))
                            .child(if email_opt.is_some() {
                                "Account & sync"
                            } else {
                                "Sign in"
                            }),
                    )
                    .child(
                        div()
                            .id("avatar-menu-addons")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(6.0))
                            .hover(|s| s.bg(rgb(BG_ELEVATED)).text_color(rgb(TEXT_PRIMARY)))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .text_size(px(13.5))
                            .text_color(rgb(TEXT_SECONDARY))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.navigate_tab(SidebarTab::Addons, cx);
                                }),
                            )
                            .child(svg_icon(IconName::Layers, 14.5, rgb(TEXT_SECONDARY)))
                            .child("Addons"),
                    )
                    .child(
                        div()
                            .id("avatar-menu-settings")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(6.0))
                            .hover(|s| s.bg(rgb(BG_ELEVATED)).text_color(rgb(TEXT_PRIMARY)))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .text_size(px(13.5))
                            .text_color(rgb(TEXT_SECONDARY))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.navigate_tab(SidebarTab::Settings, cx);
                                }),
                            )
                            .child(svg_icon(IconName::Settings, 14.5, rgb(TEXT_SECONDARY)))
                            .child("Settings"),
                    )
                    .when(email_opt.is_some(), |el| {
                        el.child(
                            div()
                                .id("avatar-menu-logout")
                                .cursor_pointer()
                                .mt(px(4.0))
                                .px(px(10.0))
                                .py(px(8.0))
                                .rounded(px(6.0))
                                .hover(|s| s.bg(rgba(0xf871711a)))
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .text_size(px(13.5))
                                .text_color(rgb(ACCENT_ROSE))
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
                                .child(svg_icon(IconName::X, 14.0, rgb(ACCENT_ROSE)))
                                .child("Sign out"),
                        )
                    })
                    .with_animation("avatar-menu-fade", obsidian_fade_in(), |el, delta| {
                        el.opacity(delta)
                    }),
            )
    }

    fn render_toast_stack(&self) -> impl IntoElement {
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
            .gap(px(8.0))
            .children(self.toasts.iter().map(|toast| {
                let (icon_col, icon) = match toast.kind {
                    ToastKind::Info => (ACCENT, IconName::Info),
                    ToastKind::Success => (STATUS_WATCHED, IconName::Check),
                    ToastKind::Warning => (ACCENT_AMBER, IconName::RefreshCw),
                    ToastKind::Error => (ACCENT_ROSE, IconName::X),
                };
                div()
                    .px(px(16.0))
                    .py(px(10.0))
                    .rounded(px(999.0))
                    .bg(rgb(BG_SURFACE))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .shadow(glass_capsule_shadow())
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(svg_icon(icon, 14.0, rgb(icon_col)))
                    .child(
                        div()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(px(13.0))
                            .text_color(rgb(TEXT_PRIMARY))
                            .child(toast.title.clone()),
                    )
                    .when_some(toast.detail.clone(), |el, d| {
                        el.child(
                            div()
                                .text_size(px(12.5))
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(d),
                        )
                    })
                    .with_animation(
                        ("toast-fade", toast.id as usize),
                        obsidian_fade_in(),
                        |el, delta| el.opacity(delta),
                    )
            }))
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.exit_fullscreen_on_next_render {
            self.exit_fullscreen_on_next_render = false;
            if window.is_fullscreen() {
                window.toggle_fullscreen();
            }
        }

        self.poster_cache.begin_render_frame();

        let mut evicted_images = self.poster_cache.drain_evicted_images();
        evicted_images.extend(gpui_mpv::drain_evicted_video_images());
        if !evicted_images.is_empty() {
            for img in evicted_images {
                let _ = window.drop_image(img);
            }
            trim_process_heap();
        }

        let is_player = self.route.is_player();
        let is_csd = matches!(window.window_decorations(), Decorations::Client { .. })
            && !window.is_fullscreen();

        if is_player && self.window_blur_active {
            window.set_background_appearance(WindowBackgroundAppearance::Transparent);
            self.window_blur_active = false;
        } else if !is_player && !self.window_blur_active {
            window.set_background_appearance(WindowBackgroundAppearance::Blurred);
            self.window_blur_active = true;
        }

        let root_bg = if is_player {
            transparent_cutout()
        } else {
            rgb(BG_OBSIDIAN)
        };

        let transition_key = SharedString::from(self.route.transition_id());
        let topnav_el = if !is_player {
            Some(self.render_topnav_bar(window, cx))
        } else {
            None
        };
        let resize_handles_el = Self::render_csd_resize_handles(window);

        let active_content = match self.route.clone() {
            AppRoute::Player => self.render_player_view(window, cx).into_any_element(),
            AppRoute::Tab(SidebarTab::Home) => self.render_board_view(cx).into_any_element(),
            AppRoute::Tab(SidebarTab::Discover) => {
                self.render_discover_view(window, cx).into_any_element()
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

        div()
            .id("ferrite-root")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_global_key_down))
            .on_key_up(cx.listener(Self::on_global_key_up))
            .size_full()
            .bg(root_bg)
            .font(ui_sans(FontWeight::NORMAL))
            .text_color(rgb(TEXT_PRIMARY))
            .relative()
            .flex()
            .flex_col()
            .when_some(topnav_el, |el, bar| el.child(bar))
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
                            .child(active_content)
                            .with_animation(transition_key, obsidian_fade_in(), |el, delta| {
                                el.opacity(delta)
                            })
                            .into_any_element()
                    }),
            )
            .when(self.command_palette_open, |el| {
                el.child(self.render_command_palette(cx))
            })
            .when(self.avatar_menu_open, |el| {
                el.child(self.render_avatar_menu(is_csd, cx))
            })
            .when(!self.toasts.is_empty(), |el| {
                el.child(self.render_toast_stack())
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
