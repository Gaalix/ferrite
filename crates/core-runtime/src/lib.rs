pub mod addon_engine;
pub mod controller;
pub mod debrid;
pub mod env;
pub mod error;
pub mod model;
pub mod prewarm;
pub mod private_config;
pub mod prowlarr;
pub mod stream_parser;
pub mod subtitles;
pub mod track_prefs;

pub use addon_engine::{
    addon_configure_url, addon_has_debrid_configured, addon_origin_from_url, addon_origin_key,
    classify_cache_ttl, classify_descriptor_categories, classify_request_deadline,
    curated_addon_directory, detect_clipboard_addon_url, has_debrid_configured_addons,
    normalize_addon_url, redact_sensitive_text, redact_url, redact_url_str,
    resolve_addon_for_request_base, AddonCacheStats, AddonCategory, AddonEngineConfig,
    AddonHealthStatus, AddonHttpEngine, AddonTelemetrySnapshot, CuratedAddonEntry,
    InstallAddonOutcome, MutedAddonEntry, ADDON_CACHE_MAX_BYTES, ADDON_CACHE_MAX_ENTRIES,
    ADDON_CACHE_MAX_SINGLE_ENTRY_BYTES, ADDON_MANIFEST_TIMEOUT, ADDON_RESOURCE_TIMEOUT,
    CIRCUIT_BREAKER_FAILURE_THRESHOLD, DEFAULT_CIRCUIT_BREAKER_COOLDOWN, LOCALHOST_TIMEOUT,
    SLOW_ADDON_LATENCY_MS, STREMIO_API_TIMEOUT, TTL_CATALOG, TTL_MANIFEST, TTL_META, TTL_STREAM,
    TTL_SUBTITLES,
};
pub use controller::{
    CoreController, DesktopRuntimeAction, DesktopRuntimeEvent, INITIAL_BOARD_SHELVES,
};
pub use debrid::{
    build_canonical_magnet, extract_magnet_trackers, parse_info_hash, select_best_video_file,
    validate_resolved_stream_url, DebridAccountStatus, DebridBaseUrls, DebridCacheStatus,
    LocalDebridResolver, ResolvedDebridStream, TorrentFileCandidate, AVAILABILITY_CACHE_TTL,
    DEFAULT_ALLDEBRID_BASE_URL, DEFAULT_REAL_DEBRID_BASE_URL, DEFAULT_TORBOX_BASE_URL,
    RESOLVED_URL_CACHE_TTL,
};
pub use env::{
    default_storage_path, legacy_storage_path, migrate_legacy_storage_file, DesktopEnv,
    DesktopEnvConfig, EnvConfig, PersistedBuckets, SqliteStorage,
};
pub use error::CoreRuntimeError;
pub use model::{clone_model_field, DesktopModel, DesktopModelField};
pub use prewarm::{
    convert_stream_to_playback_url, evaluate_next_episode_prewarm, rank_continuation_candidates,
    ActiveEpisodeMatchSpec, NextEpisodeTarget, PrewarmTriggerReason, ResolvedNextEpisode,
    ENDING_PREWARM_WINDOW_SECS, MAX_RANKED_NEXT_EPISODE_CANDIDATES,
    MIN_EPISODE_DURATION_FOR_ENDING_PREWARM_SECS, OUTRO_PREWARM_LEAD_SECS,
};
pub use private_config::{
    default_config_path as default_private_config_path, load_from_path as load_private_config,
    redact_secret, save_to_path as save_private_config, LocalDebridProvider,
    PrivateIntegrationsConfig, ProwlarrFallbackMode, SecretApiKey, DEFAULT_PROWLARR_TIMEOUT_MS,
    MAX_PROWLARR_TIMEOUT_MS, MIN_PROWLARR_TIMEOUT_MS,
};
pub use prowlarr::{ProwlarrClient, ProwlarrTestResult, RichStreamItem};
pub use stream_parser::{
    estimate_bitrate_mbps, filter_and_rank_streams, format_bitrate_mbps, format_bytes,
    is_allowed_addon_stream_url, is_allowed_stream_source, is_allowed_stream_url,
    parse_runtime_minutes, parse_stream, parse_stream_with_addon, parse_streams, AudioCodec,
    HdrFormat, ParsedStream, Resolution, StreamFilter, StreamQuality, VideoCodec,
};
pub use subtitles::{
    decode_subtitle_payload, extract_best_subtitle_from_zip, normalize_bytes_to_utf8,
    normalize_subtitle_lang_code, normalize_subtitle_lang_iso639_1,
    normalize_subtitle_lang_iso639_2, subtitle_lang_matches, FetchedNativeSubtitle, FpsHint,
    NativeSubtitleClient, NativeSubtitleEndpoints, NativeSubtitleQuery, ReleaseEdition,
    StreamingServiceTag, SubtitleAlignmentResult, SubtitleMatchRanker, SubtitleMatchTier,
    SubtitleSourceCut, SubtitleSyncAligner, VideoReleaseFingerprint,
    DEFAULT_OPENSUBTITLES_API_BASE, DEFAULT_OPENSUBTITLES_V3_BASE, DEFAULT_SUBDL_API_BASE,
    DEFAULT_SUBDL_DL_BASE, MAX_SUBTITLE_ARCHIVE_BYTES, MAX_UNCOMPRESSED_SUBTITLE_BYTES,
    NATIVE_SUBTITLE_HTTP_TIMEOUT_MS, NATIVE_SUBTITLE_POOL_IDLE_SECS,
};
pub use track_prefs::{
    extract_track_keywords, parse_lang_tag, select_preferred_tracks, MatchedTracks,
    SeriesTrackPreference, TrackCandidate, TrackKind, SERIES_TRACK_PREFS_STORAGE_KEY,
};

pub use stremio_core;
