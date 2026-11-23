use std::collections::HashSet;

use gpui::{
    deferred, div, linear_color_stop, linear_gradient, prelude::*, px, relative, rgb, rgba,
    AnyElement, BoxShadow, ClipboardItem, Context, Div, FontWeight, MouseButton, SharedString,
};
use stremio_core_runtime::{format_bytes, AddonHealthStatus, DEFAULT_PROWLARR_TIMEOUT_MS};

use crate::{
    icons::{svg_icon, svg_icon_weighted, IconName, IconWeight},
    poster_cache::MAX_DISK_CACHE_BYTES,
    shell::AppShell,
    state::{
        mask_api_key_preview, settings_row_matches_query, DebridAccessMode, FocusedInput,
        SettingsCategory, SettingsDropdown, ShaderPresetChoice, SidebarTab, StorageModeChoice,
        SubtitleBackgroundChoice, ToastKind, ToneMappingChoice, UpscaleChoice,
        DEFAULT_SUBTITLE_FONT_SIZE, DEFAULT_SUBTITLE_VERTICAL_POS,
    },
    theme::{
        active_theme, build_palette, editorial_serif, mono_font, sp, tabular_nums, ui_sans,
        AccentChoice, CardDensityChoice, ThemePalette, ThemePreset, UiScaleChoice, RADIUS_CARD,
        RADIUS_CONTROL, RADIUS_PILL, RADIUS_POSTER, RADIUS_SHEET,
    },
    ui::{
        apply_page_width, choice_pill, dropdown_trigger, glass_dropdown_option,
        glass_dropdown_panel, glass_segmented_bar, glass_segmented_item, glass_stepper,
        glass_switch, keycap, neutral_chip, page_gutter_px, primary_accent_button,
        render_empty_state, render_page_header, search_field, secondary_glass_button,
        semantic_status_pill, DropdownTriggerProps, EmptyStateProps, PageHeader, PageWidth,
        SearchFieldProps, PAGE_PADDING_BOTTOM_PX, PAGE_PADDING_TOP_PX, PAGE_SECTION_GAP_PX,
    },
    updater::{self, UpdateStatus, CURRENT_APP_VERSION},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsRowId {
    // 1. Appearance
    ThemePreset,
    AccentColor,
    AmbientArtworkBleed,
    UiScale,
    CardDensity,
    ShowHomeSpotlight,
    OsdAutohideSecs,
    // 2. Playback
    AutoPlayNextEpisode,
    PreventDisplaySleep,
    OsMediaControls,
    GpuUpscaler,
    HdrToneMapping,
    GlslShaderPreset,
    DarkSceneDebanding,
    DolbyVisionReshape,
    DynamicHdrPeak,
    HardwareZeroCopyStatus,
    // 3. Subtitles & Audio
    SubtitleStyleSummary,
    PreferredAudioLang,
    PreferredSubtitleLang,
    AutoSelectSubtitles,
    AutoMatchNativeSubtitles,
    DefaultSubtitleDelay,
    SubtitleAssScaleOverride,
    // 4. Streaming & Debrid
    PieceCacheMode,
    LocalDebridProvider,
    LocalDebridCachedOnly,
    LocalDebridDirectUnrestrict,
    DeadSwarmFailover,
    UpnpPortMapping,
    AddonDebridVisibility,
    ProwlarrFallbackMode,
    // 5. Integrations & Addons
    OpenAddonsStudio,
    ClipboardAddonDetect,
    TmdbApiKey,
    OpenSubtitlesApiKey,
    SubdlApiKey,
    RealDebridApiKey,
    TorBoxApiKey,
    AllDebridApiKey,
    ProwlarrUrl,
    ProwlarrApiKey,
    ProwlarrTimeoutAndTest,
    // 6. System & Storage
    ClearMetadataImageCache,
    SoftwareUpdateCheck,
    CheckUpdatesOnStartup,
    ShortcutsGlobal,
    ShortcutsPlayerTransport,
    ShortcutsPlayerTracksAudio,
    LocalStreamingArchitecture,
}

pub struct SettingsRowSpec {
    pub id: SettingsRowId,
    pub category: SettingsCategory,
    pub group: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub keywords: &'static [&'static str],
    pub is_advanced: bool,
}

pub const SETTINGS_ROWS: &[SettingsRowSpec] = &[
    // --- 1. Appearance ---
    SettingsRowSpec {
        id: SettingsRowId::ThemePreset,
        category: SettingsCategory::Appearance,
        group: "Theme & Color",
        label: "Theme preset",
        description: "Choose between Ethereal Dark, Pure OLED, Aurora Indigo, Warm Cinema, or Light Mist.",
        keywords: &[
            "theme", "preset", "dark", "light", "oled", "indigo", "cinema", "mist", "appearance",
        ],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::AccentColor,
        category: SettingsCategory::Appearance,
        group: "Theme & Color",
        label: "Accent color",
        description: "Customize primary highlights, focus rings, switches, and play actions.",
        keywords: &[
            "accent", "color", "swatch", "coral", "violet", "cyan", "rose", "gold", "mint",
        ],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::AmbientArtworkBleed,
        category: SettingsCategory::Appearance,
        group: "Atmosphere & Layout",
        label: "Ambient artwork bleed",
        description: "Cast a subtle blurred color glow from active cinema artwork behind the frosted interface.",
        keywords: &["ambient", "bleed", "glow", "artwork", "backdrop", "blur", "atmosphere"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::UiScale,
        category: SettingsCategory::Appearance,
        group: "Atmosphere & Layout",
        label: "UI scale",
        description: "Scale typography, spacing, and controls across the entire interface (90% to 125%).",
        keywords: &["scale", "zoom", "dpi", "size", "90%", "100%", "110%", "125%", "font"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::CardDensity,
        category: SettingsCategory::Appearance,
        group: "Atmosphere & Layout",
        label: "Card density",
        description: "Adjust poster card sizing and shelf spacing across Home, Discover, and Library.",
        keywords: &["density", "cards", "posters", "compact", "comfortable", "cinema", "grid"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::ShowHomeSpotlight,
        category: SettingsCategory::Appearance,
        group: "Atmosphere & Layout",
        label: "Featured hero on Home",
        description: "Show an editorial hero spotlight banner above your Home shelves (disable for pure poster grids).",
        keywords: &[
            "home", "spotlight", "hero", "banner", "featured", "editorial", "poster",
        ],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::OsdAutohideSecs,
        category: SettingsCategory::Appearance,
        group: "Player Overlay",
        label: "Player OSD auto-hide",
        description: "Idle duration before player controls fade away during playback (2s, 3s, or 5s).",
        keywords: &["osd", "overlay", "autohide", "timeout", "controls", "fade"],
        is_advanced: true,
    },
    // --- 2. Playback ---
    SettingsRowSpec {
        id: SettingsRowId::AutoPlayNextEpisode,
        category: SettingsCategory::Playback,
        group: "Playback Behavior",
        label: "Auto-play & pre-buffer next episode",
        description: "Pre-match and pre-buffer Episode N+1 45s before credits and auto-play at end of stream.",
        keywords: &["auto", "play", "next", "episode", "binge", "prebuffer", "credits"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::PreventDisplaySleep,
        category: SettingsCategory::Playback,
        group: "Playback Behavior",
        label: "Prevent display sleep while playing",
        description: "Keep the display awake and suppress screensaver lock during active playback.",
        keywords: &["sleep", "display", "screensaver", "wake", "inhibit", "idle"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::OsMediaControls,
        category: SettingsCategory::Playback,
        group: "Playback Behavior",
        label: "OS media controls (MPRIS2 / SMTC)",
        description: "Publish now-playing metadata and transport keys to Linux MPRIS2 and Windows SMTC.",
        keywords: &["mpris", "mpris2", "smtc", "media", "keys", "os", "transport"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::GpuUpscaler,
        category: SettingsCategory::Playback,
        group: "Video Quality",
        label: "GPU upscaler",
        description: "Scaling filter used by libmpv when upscaling lower-resolution streams (Spline36 default).",
        keywords: &["upscale", "bilinear", "spline36", "lanczos", "lanczos3", "ewa", "sharp"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::HdrToneMapping,
        category: SettingsCategory::Playback,
        group: "Video Quality",
        label: "HDR tone mapping",
        description: "Select the libmpv HDR-to-SDR tone-mapping curve for high dynamic range video.",
        keywords: &["hdr", "tone", "mapping", "spline", "bt2446a", "hable", "mobius"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::GlslShaderPreset,
        category: SettingsCategory::Playback,
        group: "Shaders & Video Pipeline",
        label: "GLSL shader preset",
        description: "Experimental libmpv post-processing shader preset (off by default).",
        keywords: &["glsl", "shader", "anime4k", "fsrcnnx", "krigbilateral", "experimental"],
        is_advanced: true,
    },
    SettingsRowSpec {
        id: SettingsRowId::DarkSceneDebanding,
        category: SettingsCategory::Playback,
        group: "Shaders & Video Pipeline",
        label: "Dark-scene debanding",
        description: "Smooth 8-bit compression banding gradients in dark scenes.",
        keywords: &["deband", "banding", "gradient", "dark", "video"],
        is_advanced: true,
    },
    SettingsRowSpec {
        id: SettingsRowId::DolbyVisionReshape,
        category: SettingsCategory::Playback,
        group: "Shaders & Video Pipeline",
        label: "Dolby Vision reshape",
        description: "Enable experimental Dolby Vision RPU metadata reshaping in libmpv.",
        keywords: &["dolby", "vision", "dv", "reshape", "hdr"],
        is_advanced: true,
    },
    SettingsRowSpec {
        id: SettingsRowId::DynamicHdrPeak,
        category: SettingsCategory::Playback,
        group: "Shaders & Video Pipeline",
        label: "Dynamic HDR peak",
        description: "Compute per-frame HDR peak luminance for dynamic tone mapping.",
        keywords: &["hdr", "peak", "dynamic", "luminance", "compute"],
        is_advanced: true,
    },
    SettingsRowSpec {
        id: SettingsRowId::HardwareZeroCopyStatus,
        category: SettingsCategory::Playback,
        group: "Shaders & Video Pipeline",
        label: "Hardware zero-copy video pipeline",
        description: "DirectComposition (Windows) / Wayland subsurface zero-copy presentation with libmpv hwdec.",
        keywords: &["hardware", "zero-copy", "directcomposition", "wayland", "hwdec", "gpu"],
        is_advanced: true,
    },
    // --- 3. Subtitles & Audio ---
    SettingsRowSpec {
        id: SettingsRowId::SubtitleStyleSummary,
        category: SettingsCategory::SubtitlesAndAudio,
        group: "Subtitle Studio",
        label: "Live subtitle preview & style",
        description: "Live cinema preview with font size, HDR-safe color swatches, backdrop style, and vertical position.",
        keywords: &[
            "subtitle", "style", "preview", "font", "size", "color", "hdr", "cream", "shadow",
            "outline", "box", "position", "reset",
        ],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::PreferredAudioLang,
        category: SettingsCategory::SubtitlesAndAudio,
        group: "Languages & Selection",
        label: "Preferred audio language",
        description: "Default ISO-639-2 audio stream preference when starting playback.",
        keywords: &["audio", "language", "eng", "jpn", "spa", "fre", "ger", "track"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::PreferredSubtitleLang,
        category: SettingsCategory::SubtitlesAndAudio,
        group: "Languages & Selection",
        label: "Preferred subtitle language",
        description: "Default subtitle language when auto-selecting tracks, or Off to disable.",
        keywords: &["subtitle", "subtitles", "language", "eng", "off", "spa", "fre", "ger"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::AutoSelectSubtitles,
        category: SettingsCategory::SubtitlesAndAudio,
        group: "Languages & Selection",
        label: "Auto-select subtitles",
        description: "Automatically enable matching subtitle tracks when playback begins.",
        keywords: &["subtitle", "auto", "select", "captions", "cc"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::AutoMatchNativeSubtitles,
        category: SettingsCategory::SubtitlesAndAudio,
        group: "Matching & Timing",
        label: "Auto-match native hash subtitles (OpenSubtitles + SubDL)",
        description: "Compute 64 KiB stream hash and auto-align matching subtitles when playback starts.",
        keywords: &["opensubtitles", "subdl", "hash", "opensubhash", "native", "subtitle", "sync"],
        is_advanced: true,
    },
    SettingsRowSpec {
        id: SettingsRowId::DefaultSubtitleDelay,
        category: SettingsCategory::SubtitlesAndAudio,
        group: "Matching & Timing",
        label: "Default subtitle delay",
        description: "Baseline subtitle timing offset (-5000 ms to +5000 ms) applied to new sessions.",
        keywords: &["subtitle", "delay", "offset", "sync", "timing", "ms"],
        is_advanced: true,
    },
    SettingsRowSpec {
        id: SettingsRowId::SubtitleAssScaleOverride,
        category: SettingsCategory::SubtitlesAndAudio,
        group: "Matching & Timing",
        label: "ASS scale override",
        description: "Apply your font sizing preferences to styled ASS/SSA anime and Blu-ray subtitles.",
        keywords: &["ass", "ssa", "scale", "override", "styled", "anime", "subtitle"],
        is_advanced: true,
    },
    // --- 4. Streaming & Debrid ---
    SettingsRowSpec {
        id: SettingsRowId::PieceCacheMode,
        category: SettingsCategory::StreamingAndDebrid,
        group: "Piece Cache Budget",
        label: "Piece cache budget",
        description: "Choose warm-resume Disk LRU caching (2, 5, 10, 20 GB) or zero-SSD-write RAM ring buffering (1, 2 GB).",
        keywords: &["storage", "cache", "piece", "disk", "lru", "ram", "ring", "budget", "gb"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::LocalDebridProvider,
        category: SettingsCategory::StreamingAndDebrid,
        group: "Local Rust Debrid",
        label: "Local Debrid provider",
        description: "Resolve cached magnet hashes directly in Rust without external proxy addons.",
        keywords: &["debrid", "real-debrid", "torbox", "alldebrid", "provider", "rust"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::LocalDebridCachedOnly,
        category: SettingsCategory::StreamingAndDebrid,
        group: "Local Rust Debrid",
        label: "Cached-only stream filter",
        description: "Hide uncached P2P torrents when a local Debrid provider is active.",
        keywords: &["cached", "filter", "debrid", "torrent", "p2p"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::LocalDebridDirectUnrestrict,
        category: SettingsCategory::StreamingAndDebrid,
        group: "Network & Resilience",
        label: "Direct ~60ms Rust unrestrict",
        description: "Keep a warm HTTP/2 connection pool for sub-100ms unrestrict and instant cache checks.",
        keywords: &["debrid", "unrestrict", "latency", "http2", "fast", "60ms"],
        is_advanced: true,
    },
    SettingsRowSpec {
        id: SettingsRowId::DeadSwarmFailover,
        category: SettingsCategory::StreamingAndDebrid,
        group: "Network & Resilience",
        label: "Dead-swarm auto-failover",
        description: "Step to the next ranked stream if a swarm stalls before first byte.",
        keywords: &["dead", "swarm", "failover", "timeout", "stalled", "torrent"],
        is_advanced: true,
    },
    SettingsRowSpec {
        id: SettingsRowId::UpnpPortMapping,
        category: SettingsCategory::StreamingAndDebrid,
        group: "Network & Resilience",
        label: "UPnP / NAT-PMP port mapping",
        description: "Automatically map inbound BitTorrent listener ports on your local router.",
        keywords: &["upnp", "nat-pmp", "port", "router", "network", "bittorrent"],
        is_advanced: true,
    },
    SettingsRowSpec {
        id: SettingsRowId::AddonDebridVisibility,
        category: SettingsCategory::StreamingAndDebrid,
        group: "Network & Resilience",
        label: "Addon Debrid visibility override",
        description: "Auto-detect Debrid from installed addon URLs or override Cached stream visibility.",
        keywords: &["addon", "debrid", "auto-detect", "visibility", "cached"],
        is_advanced: true,
    },
    SettingsRowSpec {
        id: SettingsRowId::ProwlarrFallbackMode,
        category: SettingsCategory::StreamingAndDebrid,
        group: "Network & Resilience",
        label: "Prowlarr fallback mode",
        description: "Query your local Prowlarr or Torznab endpoint on addon outage or always merge.",
        keywords: &["prowlarr", "torznab", "fallback", "outage", "indexer", "merge"],
        is_advanced: true,
    },
    // --- 5. Integrations & Addons ---
    SettingsRowSpec {
        id: SettingsRowId::OpenAddonsStudio,
        category: SettingsCategory::IntegrationsAndAddons,
        group: "Addons Studio",
        label: "Stremio Addons Studio",
        description: "Reorder execution priority, inspect live health, soft-mute sources, or browse curated addons.",
        keywords: &["addons", "studio", "stremio", "manifest", "mute", "priority", "health"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::ClipboardAddonDetect,
        category: SettingsCategory::IntegrationsAndAddons,
        group: "Addons Studio",
        label: "Clipboard addon link detection",
        description: "Detect copied stremio:// or manifest.json addon URLs when opening Addons Studio.",
        keywords: &["clipboard", "addon", "manifest", "stremio://", "paste"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::TmdbApiKey,
        category: SettingsCategory::IntegrationsAndAddons,
        group: "Metadata & Subtitle API Keys",
        label: "TMDB API key / Read Access Token",
        description: "Optional TMDB v3 API key or v4 Bearer token for native HTTP/2 logos, backdrops, cast photos, and filmographies (0600).",
        keywords: &[
            "tmdb", "metadata", "logos", "cast", "director", "search", "fts5", "0600", "api", "key",
        ],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::OpenSubtitlesApiKey,
        category: SettingsCategory::IntegrationsAndAddons,
        group: "Metadata & Subtitle API Keys",
        label: "OpenSubtitles API key",
        description: "Optional personal OpenSubtitles REST API key for higher search/download quotas (0600).",
        keywords: &["opensubtitles", "subtitles", "api", "key", "token", "0600"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::SubdlApiKey,
        category: SettingsCategory::IntegrationsAndAddons,
        group: "Metadata & Subtitle API Keys",
        label: "SubDL API key",
        description: "Optional SubDL API key for expanded hash and release subtitle matching (0600).",
        keywords: &["subdl", "subtitles", "api", "key", "token", "0600"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::RealDebridApiKey,
        category: SettingsCategory::IntegrationsAndAddons,
        group: "Debrid & Local Indexer Credentials",
        label: "Real-Debrid API key",
        description: "Private Real-Debrid API token stored locally with 0600 permissions.",
        keywords: &["real-debrid", "rd", "api", "key", "token", "0600"],
        is_advanced: true,
    },
    SettingsRowSpec {
        id: SettingsRowId::TorBoxApiKey,
        category: SettingsCategory::IntegrationsAndAddons,
        group: "Debrid & Local Indexer Credentials",
        label: "TorBox API key",
        description: "Private TorBox API token stored locally with 0600 permissions.",
        keywords: &["torbox", "tb", "api", "key", "token", "0600"],
        is_advanced: true,
    },
    SettingsRowSpec {
        id: SettingsRowId::AllDebridApiKey,
        category: SettingsCategory::IntegrationsAndAddons,
        group: "Debrid & Local Indexer Credentials",
        label: "AllDebrid API key",
        description: "Private AllDebrid API key stored locally with 0600 permissions.",
        keywords: &["alldebrid", "ad", "api", "key", "token", "0600"],
        is_advanced: true,
    },
    SettingsRowSpec {
        id: SettingsRowId::ProwlarrUrl,
        category: SettingsCategory::IntegrationsAndAddons,
        group: "Debrid & Local Indexer Credentials",
        label: "Local Prowlarr / Torznab URL",
        description: "Base URL of your local Prowlarr or Torznab indexer (e.g. http://127.0.0.1:9696).",
        keywords: &["prowlarr", "torznab", "url", "endpoint", "localhost", "indexer"],
        is_advanced: true,
    },
    SettingsRowSpec {
        id: SettingsRowId::ProwlarrApiKey,
        category: SettingsCategory::IntegrationsAndAddons,
        group: "Debrid & Local Indexer Credentials",
        label: "Prowlarr / Torznab API key",
        description: "X-Api-Key or Torznab apikey sent strictly to your local indexer endpoint (0600).",
        keywords: &["prowlarr", "torznab", "api", "key", "x-api-key", "0600"],
        is_advanced: true,
    },
    SettingsRowSpec {
        id: SettingsRowId::ProwlarrTimeoutAndTest,
        category: SettingsCategory::IntegrationsAndAddons,
        group: "Debrid & Local Indexer Credentials",
        label: "Prowlarr timeout & connection test",
        description: "Configure local indexer request timeout (1000-15000 ms) and test connectivity.",
        keywords: &["prowlarr", "timeout", "test", "indexer", "torznab", "latency"],
        is_advanced: true,
    },
    // --- 6. System & Storage ---
    SettingsRowSpec {
        id: SettingsRowId::ClearMetadataImageCache,
        category: SettingsCategory::SystemAndStorage,
        group: "Storage & Cache Telemetry",
        label: "Live storage & cache gauge",
        description: "Monitor image disk cache, torrent piece cache, and addon HTTP cache, or clear image & metadata cache.",
        keywords: &[
            "clear", "cache", "storage", "gauge", "metadata", "images", "poster", "flush",
            "cleanup", "disk", "http", "96mb",
        ],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::SoftwareUpdateCheck,
        category: SettingsCategory::SystemAndStorage,
        group: "Software Updates",
        label: "Check & install updates",
        description: "Verify GitHub Releases over HTTPS against SHA256SUMS.txt from Gaalix/ferrite.",
        keywords: &["update", "version", "sha256", "checksum", "github", "restart", "install"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::CheckUpdatesOnStartup,
        category: SettingsCategory::SystemAndStorage,
        group: "Software Updates",
        label: "Check for updates on startup",
        description: "Automatically check GitHub Releases for signed updates when Ferrite launches.",
        keywords: &["update", "startup", "github", "release", "auto"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::ShortcutsGlobal,
        category: SettingsCategory::SystemAndStorage,
        group: "Keyboard Shortcuts",
        label: "Global navigation & palette shortcuts",
        description: "Command palette (Ctrl/Cmd+K, /), tab switching (Alt+1..6), fullscreen (F11), quit (Ctrl/Cmd+Q).",
        keywords: &[
            "shortcut", "keyboard", "command palette", "ctrl+k", "alt", "f11", "quit", "escape",
        ],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::ShortcutsPlayerTransport,
        category: SettingsCategory::SystemAndStorage,
        group: "Keyboard Shortcuts",
        label: "Player transport & seeking shortcuts",
        description: "Play/Pause (Space), 2x hold (Space), Seek 5s/30s (Left/Right), Frame step (, / .), Exit (Esc).",
        keywords: &["shortcut", "player", "space", "seek", "frame", "2x", "pause", "escape"],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::ShortcutsPlayerTracksAudio,
        category: SettingsCategory::SystemAndStorage,
        group: "Keyboard Shortcuts",
        label: "Player audio, subtitles & stats shortcuts",
        description: "Volume (Up/Down), Mute (M), Fullscreen (F/F11), Audio (A), Subtitles (S), Delay (Z/X), Stats (I).",
        keywords: &[
            "shortcut", "volume", "mute", "audio", "subtitle", "delay", "stats", "fullscreen",
        ],
        is_advanced: false,
    },
    SettingsRowSpec {
        id: SettingsRowId::LocalStreamingArchitecture,
        category: SettingsCategory::SystemAndStorage,
        group: "Architecture & About",
        label: "Local streaming architecture & version",
        description: "Local Rust engine, 0600 credential isolation, loopback endpoint, and zero telemetry.",
        keywords: &[
            "about", "privacy", "local", "telemetry", "0600", "architecture", "ferrite", "version",
        ],
        is_advanced: true,
    },
];

pub fn count_matching_settings_rows(query: &str, category: Option<SettingsCategory>) -> usize {
    SETTINGS_ROWS
        .iter()
        .filter(|row| category.is_none_or(|c| row.category == c))
        .filter(|row| settings_row_matches_query(query, row.label, row.description, row.keywords))
        .count()
}

pub fn should_expand_advanced(
    category: SettingsCategory,
    advanced_open: &HashSet<SettingsCategory>,
    query: &str,
) -> bool {
    if advanced_open.contains(&category) {
        return true;
    }
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return false;
    }
    SETTINGS_ROWS.iter().any(|row| {
        row.category == category
            && row.is_advanced
            && settings_row_matches_query(trimmed, row.label, row.description, row.keywords)
    })
}

const AUDIO_LANG_OPTIONS: &[(&str, &str)] = &[
    ("eng", "ENG"),
    ("jpn", "JPN"),
    ("spa", "SPA"),
    ("fre", "FRE"),
    ("ger", "GER"),
];

const SUBTITLE_LANG_OPTIONS: &[(&str, &str)] = &[
    ("eng", "ENG"),
    ("off", "Off"),
    ("spa", "SPA"),
    ("fre", "FRE"),
    ("ger", "GER"),
];

const LOCAL_DEBRID_PROVIDERS: &[(&str, usize)] = &[
    ("Off", 0),
    ("Real-Debrid", 1),
    ("TorBox", 2),
    ("AllDebrid", 3),
];

const PROWLARR_MODES: &[(&str, usize)] = &[
    ("Off", 0),
    ("Fallback on outage", 1),
    ("Always merge", 2),
];

const SUBTITLE_SIZE_PRESETS: &[(&str, u32)] = &[("S", 38), ("M", 46), ("L", 56), ("XL", 66)];

const SUBTITLE_POSITION_PRESETS: &[(u32, &str)] = &[
    (100, "Lower"),
    (96, "Cinema"),
    (88, "Raised"),
    (80, "High"),
];

const SUBTITLE_SWATCHES: &[(&str, &str)] = &[
    ("#F5F1E8", "Warm Cream"),
    ("#FFFFFF", "Pure White"),
    ("#E5E7EB", "Soft Silver"),
    ("#FDE68A", "Cinema Amber"),
    ("#BAE6FD", "Cool Ice"),
];

#[inline]
const fn rgba_with_alpha(rgb_hex: u32, alpha: u8) -> u32 {
    (rgb_hex << 8) | (alpha as u32)
}

#[inline]
const fn accent_ink(theme: &ThemePalette) -> u32 {
    if theme.is_light {
        theme.accent_press
    } else {
        theme.accent
    }
}

fn parse_subtitle_hex_rgb(hex: &str) -> u32 {
    let clean = hex.trim().trim_start_matches('#');
    if clean.len() == 6 {
        u32::from_str_radix(clean, 16).unwrap_or(0xF5F1E8)
    } else if clean.len() == 8 {
        u32::from_str_radix(clean, 16)
            .map(|v| v & 0x00FF_FFFF)
            .unwrap_or(0xF5F1E8)
    } else {
        0xF5F1E8
    }
}

fn subtitle_color_label(hex: &str) -> String {
    let trimmed = hex.trim();
    for &(swatch_hex, name) in SUBTITLE_SWATCHES {
        if trimmed.eq_ignore_ascii_case(swatch_hex) {
            return name.to_owned();
        }
    }
    trimmed.to_ascii_uppercase()
}

fn ratio_clamped(used: u64, budget: u64) -> f32 {
    if budget == 0 {
        return 0.0;
    }
    ((used as f64) / (budget as f64)).clamp(0.0, 1.0) as f32
}

impl AppShell {
    pub fn render_settings_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.settings_search_query.trim().to_owned();
        let is_searching = !query.is_empty();
        let total_matches = if is_searching {
            count_matching_settings_rows(&query, None)
        } else {
            0
        };

        let gutter_px = page_gutter_px(self.viewport_size_px.0);

        div()
            .id("settings-two-pane-root")
            .size_full()
            .flex()
            .justify_center()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.settings_open_dropdown.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .child(
                apply_page_width(div(), PageWidth::Focused)
                    .h_full()
                    .px(px(gutter_px))
                    .pt(sp(PAGE_PADDING_TOP_PX))
                    .pb(sp(18.0))
                    .flex()
                    .gap(sp(PAGE_SECTION_GAP_PX))
                    .overflow_hidden()
                    .child(self.render_settings_sidebar_rail(
                        &query,
                        is_searching,
                        total_matches,
                        cx,
                    ))
                    .child(self.render_settings_detail_pane(
                        &query,
                        is_searching,
                        total_matches,
                        cx,
                    )),
            )
    }

    fn render_settings_sidebar_rail(
        &self,
        query: &str,
        is_searching: bool,
        total_matches: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = active_theme();
        let ink_accent = accent_ink(&theme);
        let focused = self.focused_input == Some(FocusedInput::SettingsSearch);
        let query_display = self.format_input_with_caret(
            FocusedInput::SettingsSearch,
            &self.settings_search_query,
        );

        let watched_border = rgba_with_alpha(theme.status_watched, if theme.is_light { 0x66 } else { 0x4d });
        let watched_bg = rgba_with_alpha(theme.status_watched, if theme.is_light { 0x1a } else { 0x1c });

        div()
            .w(sp(248.0))
            .h_full()
            .flex_shrink_0()
            .pb(sp(8.0))
            .child(
                div()
                    .size_full()
                    .rounded(px(RADIUS_SHEET))
                    .bg(rgba(theme.glass_sheet_bg))
                    .border_1()
                    .border_color(rgba(theme.glass_border))
                    .p(sp(14.0))
                    .flex()
                    .flex_col()
                    .gap(sp(14.0))
                    // Header
                    .child(
                        div()
                            .px(sp(6.0))
                            .flex()
                            .items_baseline()
                            .justify_between()
                            .gap(sp(8.0))
                            .child(
                                div()
                                    .font(editorial_serif())
                                    .text_size(sp(28.0))
                                    .text_color(rgb(theme.text_primary))
                                    .child("Settings"),
                            )
                            .child(
                                div()
                                    .font(mono_font())
                                    .text_size(sp(11.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(format!("v{CURRENT_APP_VERSION}")),
                            ),
                    )
                    // Compact sticky search input inside sidebar
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(sp(6.0))
                            .child(search_field(
                                SearchFieldProps::new(
                                    "input-settings-search",
                                    query_display,
                                    "Search settings...",
                                    self.settings_search_query.is_empty(),
                                    focused,
                                )
                                .clear_id("btn-clear-settings-search")
                                .full_width()
                                .on_focus(cx.listener(|this, _, _, cx| {
                                    this.focus_settings_input(FocusedInput::SettingsSearch, cx);
                                }))
                                .on_clear(cx.listener(|this, _, _, cx| {
                                    this.clear_settings_search(cx);
                                })),
                            ))
                            .when(is_searching, |el| {
                                let label = if total_matches == 1 {
                                    "1 matching setting".to_owned()
                                } else {
                                    format!("{total_matches} matching settings")
                                };
                                el.child(
                                    div()
                                        .px(sp(4.0))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .font(mono_font())
                                                .text_size(sp(11.0))
                                                .text_color(if total_matches == 0 {
                                                    rgb(theme.text_muted)
                                                } else {
                                                    rgb(ink_accent)
                                                })
                                                .child(label),
                                        ),
                                )
                            }),
                    )
                    // 6 Categories
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(sp(4.0))
                            .children(SettingsCategory::ALL.into_iter().map(|cat| {
                                let active = !is_searching && self.active_settings_category == cat;
                                let match_count = if is_searching {
                                    count_matching_settings_rows(query, Some(cat))
                                } else {
                                    0
                                };
                                let icon_weight = if active {
                                    IconWeight::Fill
                                } else {
                                    IconWeight::Regular
                                };
                                let icon_color = if active {
                                    rgb(ink_accent)
                                } else if is_searching && match_count == 0 {
                                    rgb(theme.text_muted)
                                } else {
                                    rgb(theme.text_secondary)
                                };
                                let text_color = if active {
                                    rgb(theme.text_primary)
                                } else if is_searching && match_count == 0 {
                                    rgb(theme.text_muted)
                                } else {
                                    rgb(theme.text_secondary)
                                };

                                div()
                                    .id(SharedString::from(format!("settings-cat-{}", cat.id())))
                                    .cursor_pointer()
                                    .h(sp(40.0))
                                    .px(sp(10.0))
                                    .rounded(px(RADIUS_CONTROL))
                                    .bg(if active {
                                        rgba(theme.accent_soft)
                                    } else {
                                        rgba(0x00000000)
                                    })
                                    .border_1()
                                    .border_color(if active {
                                        rgba(theme.border_accent)
                                    } else {
                                        rgba(0x00000000)
                                    })
                                    .hover(move |s| {
                                        if active {
                                            s
                                        } else {
                                            s.bg(rgba(theme.glass_row_hover_bg))
                                        }
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(sp(8.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.set_settings_category(cat, cx);
                                        }),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .flex()
                                            .items_center()
                                            .gap(sp(10.0))
                                            .child(
                                                div()
                                                    .w(px(3.0))
                                                    .h(sp(16.0))
                                                    .rounded(px(RADIUS_PILL))
                                                    .bg(if active {
                                                        rgb(theme.accent)
                                                    } else {
                                                        rgba(0x00000000)
                                                    }),
                                            )
                                            .child(svg_icon_weighted(
                                                cat.icon_name(),
                                                icon_weight,
                                                15.5 * theme.ui_scale,
                                                icon_color,
                                            ))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.0))
                                                    .truncate()
                                                    .font(ui_sans(if active {
                                                        FontWeight::SEMIBOLD
                                                    } else {
                                                        FontWeight::MEDIUM
                                                    }))
                                                    .text_size(sp(13.0))
                                                    .text_color(text_color)
                                                    .child(cat.label()),
                                            ),
                                    )
                                    .when(is_searching && match_count > 0, |el| {
                                        el.child(
                                            div()
                                                .px(sp(7.0))
                                                .py(sp(1.5))
                                                .rounded(px(RADIUS_PILL))
                                                .bg(rgba(theme.accent_soft))
                                                .border_1()
                                                .border_color(rgba(theme.border_accent))
                                                .font(mono_font())
                                                .text_size(sp(10.5))
                                                .text_color(rgb(ink_accent))
                                                .child(format!("{match_count}")),
                                        )
                                    })
                            })),
                    )
                    // Footer status
                    .child(
                        div()
                            .pt(sp(12.0))
                            .border_t_1()
                            .border_color(rgba(theme.glass_divider))
                            .px(sp(4.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(sp(8.0))
                            .child(semantic_status_pill(
                                IconName::Check,
                                "Engine online",
                                theme.status_watched,
                                watched_border,
                                watched_bg,
                            ))
                            .child(
                                div()
                                    .font(mono_font())
                                    .text_size(sp(10.5))
                                    .text_color(rgb(theme.text_muted))
                                    .truncate()
                                    .child(theme.preset.label()),
                            ),
                    ),
            )
    }

    fn render_settings_detail_pane(
        &self,
        query: &str,
        is_searching: bool,
        total_matches: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("settings-detail-scroll")
            .track_scroll(&self.settings_scroll_handle)
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .pr(sp(6.0))
                    .pb(sp(PAGE_PADDING_BOTTOM_PX))
                    .flex()
                    .flex_col()
                    .gap(sp(PAGE_SECTION_GAP_PX))
                    .child(if is_searching {
                        self.render_settings_search_results(query, total_matches, cx)
                    } else {
                        self.render_active_settings_category(query, cx)
                    }),
            )
    }

    fn render_settings_search_results(
        &self,
        query: &str,
        total_matches: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = active_theme();
        let ink_accent = accent_ink(&theme);

        if total_matches == 0 {
            return render_empty_state(
                EmptyStateProps::new(
                    IconName::Search,
                    format!("No settings match \"{query}\""),
                    "Try searching for theme, accent, hdr, subtitle, debrid, cache, prowlarr, or shortcut.",
                )
                .action(choice_pill(
                    "btn-empty-clear-search",
                    "Clear search",
                    false,
                    cx.listener(|this, _, _, cx| {
                        this.clear_settings_search(cx);
                    }),
                )),
            )
            .into_any_element();
        }

        let summary_text = if total_matches == 1 {
            "1 matching setting".to_owned()
        } else {
            format!("{total_matches} matching settings")
        };

        let header = render_page_header(
            PageHeader::new(format!("Results for \"{query}\""))
                .subtitle(Some(SharedString::from(format!(
                    "{summary_text} — Advanced sections auto-expanded where matched."
                ))))
                .right_slot(Some(
                    choice_pill(
                        "btn-search-header-clear",
                        "Clear search",
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.clear_settings_search(cx);
                        }),
                    )
                    .into_any_element(),
                ))
                .bottom_divider(true),
        );

        let mut sections: Vec<AnyElement> = vec![header.into_any_element()];

        for cat in SettingsCategory::ALL {
            let matching_essentials: Vec<&SettingsRowSpec> = SETTINGS_ROWS
                .iter()
                .filter(|r| r.category == cat && !r.is_advanced)
                .filter(|r| settings_row_matches_query(query, r.label, r.description, r.keywords))
                .collect();
            let matching_advanced: Vec<&SettingsRowSpec> = SETTINGS_ROWS
                .iter()
                .filter(|r| r.category == cat && r.is_advanced)
                .filter(|r| settings_row_matches_query(query, r.label, r.description, r.keywords))
                .collect();

            if matching_essentials.is_empty() && matching_advanced.is_empty() {
                continue;
            }

            let total_cat_matches = matching_essentials.len() + matching_advanced.len();
            let mut cat_block = div()
                .flex()
                .flex_col()
                .gap(sp(10.0))
                .child(
                    div()
                        .px(sp(4.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(sp(8.0))
                                .child(svg_icon_weighted(
                                    cat.icon_name(),
                                    IconWeight::Fill,
                                    15.0 * theme.ui_scale,
                                    rgb(ink_accent),
                                ))
                                .child(
                                    div()
                                        .font(ui_sans(FontWeight::SEMIBOLD))
                                        .text_size(sp(14.0))
                                        .text_color(rgb(theme.text_primary))
                                        .child(cat.label()),
                                ),
                        )
                        .child(
                            div()
                                .font(mono_font())
                                .text_size(sp(11.0))
                                .text_color(rgb(theme.text_muted))
                                .child(format!("{total_cat_matches} matching")),
                        ),
                );

            if !matching_essentials.is_empty() {
                cat_block = cat_block.child(self.render_rows_card(&matching_essentials, cx));
            }
            if !matching_advanced.is_empty() {
                cat_block = cat_block.child(self.render_advanced_accordion_card(
                    cat,
                    &matching_advanced,
                    true,
                    true,
                    cx,
                ));
            }

            sections.push(cat_block.into_any_element());
        }

        div()
            .flex()
            .flex_col()
            .gap(sp(20.0))
            .children(sections)
            .into_any_element()
    }

    fn render_active_settings_category(&self, query: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = active_theme();
        let cat = self.active_settings_category;

        let header = render_page_header(
            PageHeader::new(cat.label())
                .leading_icon(Some(cat.icon_name()))
                .subtitle(Some(SharedString::new_static(cat.subtitle())))
                .bottom_divider(true),
        );

        let mut body: Vec<AnyElement> = vec![header.into_any_element()];

        // Category-specific hero cards at top
        if cat == SettingsCategory::Appearance {
            body.push(self.render_live_theme_preview_card().into_any_element());
        }

        // Essentials grouped into clean frosted glass cards
        let essential_rows: Vec<&SettingsRowSpec> = SETTINGS_ROWS
            .iter()
            .filter(|r| r.category == cat && !r.is_advanced)
            .collect();

        let mut group_idx = 0;
        while group_idx < essential_rows.len() {
            let group_name = essential_rows[group_idx].group;
            let start = group_idx;
            while group_idx < essential_rows.len() && essential_rows[group_idx].group == group_name {
                group_idx += 1;
            }
            let slice = &essential_rows[start..group_idx];

            // Standalone rich cards (Subtitle Studio, Storage & Cache Gauge) render without redundant group wrapper chrome
            let is_standalone_hero = slice.len() == 1
                && matches!(
                    slice[0].id,
                    SettingsRowId::SubtitleStyleSummary | SettingsRowId::ClearMetadataImageCache
                );

            if is_standalone_hero {
                body.push(self.render_settings_row(slice[0].id, cx));
            } else {
                body.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(8.0))
                        .child(
                            div()
                                .px(sp(4.0))
                                .font(ui_sans(FontWeight::SEMIBOLD))
                                .text_size(sp(11.0))
                                .text_color(rgb(theme.text_muted))
                                .child(group_name.to_uppercase()),
                        )
                        .child(self.render_rows_card(slice, cx))
                        .into_any_element(),
                );
            }
        }

        // Security callout in Integrations & Addons before Advanced credentials
        if cat == SettingsCategory::IntegrationsAndAddons {
            body.push(Self::render_integrations_security_banner().into_any_element());
        }

        // Collapsible Advanced accordion card at bottom of every category
        let advanced_rows: Vec<&SettingsRowSpec> = SETTINGS_ROWS
            .iter()
            .filter(|r| r.category == cat && r.is_advanced)
            .collect();
        if !advanced_rows.is_empty() {
            let expanded = should_expand_advanced(cat, &self.settings_advanced_open, query);
            body.push(
                self.render_advanced_accordion_card(cat, &advanced_rows, expanded, false, cx)
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(sp(18.0))
            .children(body)
            .into_any_element()
    }

    fn render_rows_card(
        &self,
        rows: &[&SettingsRowSpec],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = active_theme();
        let last_idx = rows.len().saturating_sub(1);
        settings_glass_card(&theme).children(rows.iter().enumerate().map(|(idx, spec)| {
            let is_last = idx == last_idx;
            div()
                .px(sp(18.0))
                .when(!is_last, |el| {
                    el.border_b_1().border_color(rgba(theme.glass_divider))
                })
                .child(self.render_settings_row(spec.id, cx))
        }))
    }

    fn render_advanced_accordion_card(
        &self,
        cat: SettingsCategory,
        rows: &[&SettingsRowSpec],
        expanded: bool,
        auto_expanded_by_search: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = active_theme();
        let ink_accent = accent_ink(&theme);
        let count = rows.len();
        let count_label = if count == 1 {
            "1 setting".to_owned()
        } else {
            format!("{count} settings")
        };
        let hint = advanced_section_hint(cat);
        let toggle_id = SharedString::from(format!("settings-adv-toggle-{}", cat.id()));
        let last_idx = rows.len().saturating_sub(1);

        settings_glass_card(&theme)
            .child(
                div()
                    .id(toggle_id)
                    .cursor_pointer()
                    .px(sp(18.0))
                    .py(sp(14.0))
                    .hover(move |s| s.bg(rgba(theme.glass_row_hover_bg)))
                    .when(expanded, |el| {
                        el.border_b_1().border_color(rgba(theme.glass_divider))
                    })
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sp(12.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if !this.settings_advanced_open.remove(&cat) {
                                this.settings_advanced_open.insert(cat);
                            }
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(sp(10.0))
                            .child(
                                div()
                                    .w(sp(28.0))
                                    .h(sp(28.0))
                                    .flex_shrink_0()
                                    .rounded(px(RADIUS_POSTER))
                                    .bg(if expanded {
                                        rgba(theme.accent_soft)
                                    } else {
                                        rgba(theme.glass_trough_bg)
                                    })
                                    .border_1()
                                    .border_color(if expanded {
                                        rgba(theme.border_accent)
                                    } else {
                                        rgba(theme.glass_border)
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(svg_icon(
                                        IconName::Sliders,
                                        13.5 * theme.ui_scale,
                                        if expanded {
                                            rgb(ink_accent)
                                        } else {
                                            rgb(theme.text_secondary)
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap(sp(1.5))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(sp(8.0))
                                            .child(
                                                div()
                                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                                    .text_size(sp(13.5))
                                                    .text_color(rgb(theme.text_primary))
                                                    .child("Advanced"),
                                            )
                                            .child(
                                                div()
                                                    .px(sp(7.0))
                                                    .py(sp(1.5))
                                                    .rounded(px(RADIUS_PILL))
                                                    .bg(rgba(theme.glass_trough_bg))
                                                    .border_1()
                                                    .border_color(rgba(theme.glass_border))
                                                    .font(mono_font())
                                                    .text_size(sp(10.5))
                                                    .text_color(rgb(theme.text_muted))
                                                    .child(count_label),
                                            )
                                            .when(auto_expanded_by_search, |row| {
                                                row.child(
                                                    div()
                                                        .px(sp(7.0))
                                                        .py(sp(1.5))
                                                        .rounded(px(RADIUS_PILL))
                                                        .bg(rgba(theme.accent_soft))
                                                        .font(mono_font())
                                                        .text_size(sp(10.0))
                                                        .text_color(rgb(ink_accent))
                                                        .child("Matched"),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::NORMAL))
                                            .text_size(sp(12.0))
                                            .text_color(rgb(theme.text_muted))
                                            .truncate()
                                            .child(hint),
                                    ),
                            ),
                    )
                    .child(svg_icon(
                        if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        },
                        14.0 * theme.ui_scale,
                        rgb(theme.text_secondary),
                    )),
            )
            .when(expanded, |card| {
                card.children(rows.iter().enumerate().map(|(idx, spec)| {
                    let is_last = idx == last_idx;
                    div()
                        .px(sp(18.0))
                        .when(!is_last, |el| {
                            el.border_b_1().border_color(rgba(theme.glass_divider))
                        })
                        .child(self.render_settings_row(spec.id, cx))
                }))
            })
    }

    // --- 1. Appearance: Live Theme Preview Card ---
    fn render_live_theme_preview_card(&self) -> impl IntoElement {
        let theme = active_theme();
        let prefs = &self.preferences;
        let ink_accent = accent_ink(&theme);

        let (poster_w, poster_h, poster_gap) = match theme.density {
            CardDensityChoice::Compact => (44.0, 62.0, 8.0),
            CardDensityChoice::Comfortable => (54.0, 74.0, 11.0),
            CardDensityChoice::Cinema => (66.0, 86.0, 14.0),
        };
        let scale_f = theme.ui_scale;
        let ambient_top = rgba_with_alpha(theme.accent, if theme.is_light { 0x26 } else { 0x38 });
        let ambient_clear = theme.accent << 8;
        let hero_grad_start = rgba_with_alpha(theme.bg_elevated, 0xF0);
        let hero_grad_end = rgba_with_alpha(theme.bg_surface, 0xFA);

        settings_glass_card(&theme)
            .p(sp(16.0))
            .gap(sp(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sp(10.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(8.0))
                            .child(svg_icon_weighted(
                                IconName::Monitor,
                                IconWeight::Fill,
                                14.0 * scale_f,
                                rgb(ink_accent),
                            ))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(sp(13.0))
                                    .text_color(rgb(theme.text_primary))
                                    .child("Live theme preview"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(sp(6.0))
                            .child(neutral_chip(theme.preset.label()))
                            .child(neutral_chip(theme.accent_choice.label()))
                            .child(neutral_chip(prefs.ui_scale.label()))
                            .child(neutral_chip(theme.density.label())),
                    ),
            )
            // Mini cinema window mockup rendered live with active_theme()
            .child(
                div()
                    .w_full()
                    .rounded(px(RADIUS_CARD))
                    .bg(rgb(theme.bg_canvas))
                    .border_1()
                    .border_color(rgba(theme.glass_border_strong))
                    .overflow_hidden()
                    .relative()
                    .p(sp(14.0))
                    .flex()
                    .flex_col()
                    .gap(sp(12.0))
                    // Live ambient artwork bleed glow when enabled
                    .when(theme.ambient_bleed, |mockup| {
                        mockup.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .h(sp(90.0))
                                .bg(linear_gradient(
                                    180.0,
                                    linear_color_stop(rgba(ambient_top), 0.0),
                                    linear_color_stop(rgba(ambient_clear), 1.0),
                                )),
                        )
                    })
                    // Mini floating top nav pill
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.0))
                                    .child(
                                        div()
                                            .w(px(7.0))
                                            .h(px(7.0))
                                            .rounded_full()
                                            .bg(rgba(theme.glass_border_strong)),
                                    )
                                    .child(
                                        div()
                                            .w(px(7.0))
                                            .h(px(7.0))
                                            .rounded_full()
                                            .bg(rgba(theme.glass_border_strong)),
                                    )
                                    .child(
                                        div()
                                            .w(px(7.0))
                                            .h(px(7.0))
                                            .rounded_full()
                                            .bg(rgba(theme.glass_border_strong)),
                                    ),
                            )
                            .child(
                                div()
                                    .h(sp(24.0))
                                    .px(sp(8.0))
                                    .rounded(px(RADIUS_PILL))
                                    .bg(rgba(theme.bg_topnav_float))
                                    .border_1()
                                    .border_color(rgba(theme.glass_border))
                                    .flex()
                                    .items_center()
                                    .gap(sp(8.0))
                                    .child(
                                        div()
                                            .px(sp(7.0))
                                            .py(px(2.0))
                                            .rounded(px(RADIUS_PILL))
                                            .bg(rgba(theme.accent_soft))
                                            .font(ui_sans(FontWeight::SEMIBOLD))
                                            .text_size(sp(10.0))
                                            .text_color(rgb(ink_accent))
                                            .child("Home"),
                                    )
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::MEDIUM))
                                            .text_size(sp(10.0))
                                            .text_color(rgb(theme.text_secondary))
                                            .child("Discover"),
                                    )
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::MEDIUM))
                                            .text_size(sp(10.0))
                                            .text_color(rgb(theme.text_secondary))
                                            .child("Library"),
                                    ),
                            )
                            .child(
                                div()
                                    .w(sp(18.0))
                                    .h(sp(18.0))
                                    .rounded_full()
                                    .bg(rgba(theme.glass_trough_bg))
                                    .border_1()
                                    .border_color(rgba(theme.glass_border)),
                            ),
                    )
                    // Hero card + 3 tiny poster cards side-by-side
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .flex()
                            .items_stretch()
                            .gap(sp(14.0))
                            // Mini Hero Gradient Card
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .rounded(px(RADIUS_CONTROL))
                                    .bg(linear_gradient(
                                        135.0,
                                        linear_color_stop(rgba(hero_grad_start), 0.0),
                                        linear_color_stop(rgba(hero_grad_end), 1.0),
                                    ))
                                    .border_1()
                                    .border_color(rgba(theme.glass_border))
                                    .p(sp(14.0))
                                    .flex()
                                    .flex_col()
                                    .justify_between()
                                    .gap(sp(10.0))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(sp(3.0))
                                            .child(
                                                div()
                                                    .font(mono_font())
                                                    .text_size(sp(9.5))
                                                    .text_color(rgb(ink_accent))
                                                    .child(if prefs.show_home_spotlight {
                                                        "FEATURED CINEMA"
                                                    } else {
                                                        "GRID MODE ACTIVE"
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .font(editorial_serif())
                                                    .text_size(sp(20.0))
                                                    .line_height(sp(22.0))
                                                    .text_color(rgb(theme.text_primary))
                                                    .truncate()
                                                    .child("Dune: Part Two"),
                                            )
                                            .child(
                                                div()
                                                    .font(ui_sans(FontWeight::NORMAL))
                                                    .text_size(sp(10.5))
                                                    .text_color(rgb(theme.text_secondary))
                                                    .child("2024 · 4K Dolby Vision · Atmos"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(sp(8.0))
                                            .child(
                                                div()
                                                    .h(sp(26.0))
                                                    .px(sp(12.0))
                                                    .rounded(px(RADIUS_POSTER))
                                                    .bg(rgb(theme.accent))
                                                    .flex()
                                                    .items_center()
                                                    .gap(sp(5.0))
                                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                                    .text_size(sp(11.0))
                                                    .text_color(rgb(theme.cta_on_accent))
                                                    .child(svg_icon(
                                                        IconName::Play,
                                                        10.0 * scale_f,
                                                        rgb(theme.cta_on_accent),
                                                    ))
                                                    .child("Play"),
                                            )
                                            .child(
                                                div()
                                                    .h(sp(26.0))
                                                    .px(sp(10.0))
                                                    .rounded(px(RADIUS_POSTER))
                                                    .bg(rgba(theme.glass_trough_bg))
                                                    .border_1()
                                                    .border_color(rgba(theme.glass_border))
                                                    .flex()
                                                    .items_center()
                                                    .font(ui_sans(FontWeight::MEDIUM))
                                                    .text_size(sp(10.5))
                                                    .text_color(rgb(theme.text_secondary))
                                                    .child("Details"),
                                            ),
                                    ),
                            )
                            // 3 Tiny Poster Cards reflecting card density & scale
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_end()
                                    .gap(px(poster_gap))
                                    .children([
                                        ("Arrival", Some(0.68_f32), true),
                                        ("Interstellar", None, false),
                                        ("Severance", Some(0.32_f32), false),
                                    ].into_iter().map(move |(title, prog, hovered)| {
                                        div()
                                            .w(px(poster_w * scale_f))
                                            .flex()
                                            .flex_col()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .w_full()
                                                    .h(px(poster_h * scale_f))
                                                    .rounded(px(6.0))
                                                    .bg(rgb(if hovered {
                                                        theme.bg_surface_3
                                                    } else {
                                                        theme.bg_elevated
                                                    }))
                                                    .border_1()
                                                    .border_color(if hovered {
                                                        rgb(theme.accent)
                                                    } else {
                                                        rgba(theme.glass_border)
                                                    })
                                                    .overflow_hidden()
                                                    .relative()
                                                    .when_some(prog, |card, frac| {
                                                        card.child(
                                                            div()
                                                                .absolute()
                                                                .bottom_0()
                                                                .left_0()
                                                                .right_0()
                                                                .h(px(2.5))
                                                                .bg(rgba(theme.glass_trough_bg))
                                                                .child(
                                                                    div()
                                                                        .h_full()
                                                                        .w(relative(frac))
                                                                        .bg(rgb(theme.accent)),
                                                                ),
                                                        )
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .font(ui_sans(FontWeight::MEDIUM))
                                                    .text_size(sp(9.5))
                                                    .text_color(rgb(theme.text_secondary))
                                                    .truncate()
                                                    .child(title),
                                            )
                                    })),
                            ),
                    ),
            )
    }

    // --- 1. Appearance: Theme Preset Selector (5 Visual Swatch Cards) ---
    fn render_theme_preset_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = active_theme();
        let prefs = &self.preferences;
        let ink_accent = accent_ink(&theme);

        div()
            .py(sp(16.0))
            .flex()
            .flex_col()
            .gap(sp(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(2.0))
                    .child(
                        div()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(sp(14.0))
                            .text_color(rgb(theme.text_primary))
                            .child("Theme preset"),
                    )
                    .child(
                        div()
                            .font(ui_sans(FontWeight::NORMAL))
                            .text_size(sp(12.0))
                            .text_color(rgb(theme.text_secondary))
                            .child("Switch surface canvas, glass translucency, and contrast ink instantaneously."),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .grid()
                    .grid_cols(5)
                    .gap(sp(10.0))
                    .children(ThemePreset::ALL.into_iter().enumerate().map(|(idx, preset)| {
                        let active = prefs.theme_preset == preset;
                        let preview_pal = build_palette(
                            preset,
                            prefs.accent_color,
                            prefs.ambient_bleed_enabled,
                            1.0,
                            prefs.card_density,
                        );
                        div()
                            .id(("theme-preset-card", idx))
                            .cursor_pointer()
                            .p(sp(10.0))
                            .rounded(px(RADIUS_CARD))
                            .bg(if active {
                                rgba(theme.accent_soft)
                            } else {
                                rgba(theme.glass_trough_bg)
                            })
                            .border_1()
                            .border_color(if active {
                                rgb(theme.accent)
                            } else {
                                rgba(theme.glass_border)
                            })
                            .hover(move |s| {
                                if active {
                                    s
                                } else {
                                    s.bg(rgba(theme.glass_row_hover_bg))
                                        .border_color(rgba(theme.glass_border_strong))
                                }
                            })
                            .flex()
                            .flex_col()
                            .gap(sp(8.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_theme_preset(preset, cx);
                                }),
                            )
                            // Visual mini swatch showing preset's actual canvas/surface/text/accent
                            .child(
                                div()
                                    .w_full()
                                    .h(sp(48.0))
                                    .rounded(px(RADIUS_POSTER))
                                    .bg(rgb(preview_pal.bg_canvas))
                                    .border_1()
                                    .border_color(rgba(preview_pal.glass_border_strong))
                                    .p(px(6.0))
                                    .flex()
                                    .flex_col()
                                    .justify_between()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .w(px(28.0))
                                                    .h(px(5.0))
                                                    .rounded(px(RADIUS_PILL))
                                                    .bg(rgb(preview_pal.text_primary)),
                                            )
                                            .child(
                                                div()
                                                    .w(px(10.0))
                                                    .h(px(10.0))
                                                    .rounded_full()
                                                    .bg(rgb(preview_pal.accent)),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .h(px(18.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(preview_pal.bg_surface))
                                            .border_1()
                                            .border_color(rgba(preview_pal.glass_border))
                                            .px(px(5.0))
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .w(px(18.0))
                                                    .h(px(3.5))
                                                    .rounded(px(RADIUS_PILL))
                                                    .bg(rgb(preview_pal.text_secondary)),
                                            )
                                            .child(
                                                div()
                                                    .w(px(12.0))
                                                    .h(px(3.5))
                                                    .rounded(px(RADIUS_PILL))
                                                    .bg(rgb(preview_pal.bg_elevated)),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(sp(2.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap(sp(4.0))
                                            .child(
                                                div()
                                                    .font(ui_sans(if active {
                                                        FontWeight::SEMIBOLD
                                                    } else {
                                                        FontWeight::MEDIUM
                                                    }))
                                                    .text_size(sp(12.0))
                                                    .text_color(if active {
                                                        rgb(ink_accent)
                                                    } else {
                                                        rgb(theme.text_primary)
                                                    })
                                                    .truncate()
                                                    .child(preset.label()),
                                            )
                                            .when(active, |r| {
                                                r.child(svg_icon(
                                                    IconName::Check,
                                                    12.0 * theme.ui_scale,
                                                    rgb(ink_accent),
                                                ))
                                            }),
                                    )
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::NORMAL))
                                            .text_size(sp(10.5))
                                            .line_height(sp(14.0))
                                            .text_color(rgb(theme.text_muted))
                                            .child(preset.subtitle()),
                                    ),
                            )
                    })),
            )
            .into_any_element()
    }

    // --- 1. Appearance: Accent Color Swatches (6 Circular Buttons) ---
    fn render_accent_color_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = active_theme();
        let prefs = &self.preferences;

        div()
            .py(sp(16.0))
            .flex()
            .flex_col()
            .gap(sp(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sp(12.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(sp(2.0))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(sp(14.0))
                                    .text_color(rgb(theme.text_primary))
                                    .child("Accent color"),
                            )
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::NORMAL))
                                    .text_size(sp(12.0))
                                    .text_color(rgb(theme.text_secondary))
                                    .child("Applied across play buttons, active pills, focus rings, and progress bars."),
                            ),
                    )
                    .child(neutral_chip(prefs.accent_color.label())),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(sp(12.0))
                    .children(AccentChoice::ALL.into_iter().enumerate().map(|(idx, choice)| {
                        let active = prefs.accent_color == choice;
                        let hex = choice.hex();
                        let on_hex = choice.on_accent_hex();
                        div()
                            .id(("accent-swatch-btn", idx))
                            .cursor_pointer()
                            .px(sp(10.0))
                            .py(sp(7.0))
                            .rounded(px(RADIUS_PILL))
                            .bg(if active {
                                rgba(theme.glass_row_selected_bg)
                            } else {
                                rgba(theme.glass_trough_bg)
                            })
                            .border_1()
                            .border_color(if active {
                                rgb(hex)
                            } else {
                                rgba(theme.glass_border)
                            })
                            .hover(move |s| {
                                s.bg(rgba(theme.glass_row_hover_bg))
                                    .border_color(if active {
                                        rgb(hex)
                                    } else {
                                        rgba(theme.glass_border_strong)
                                    })
                            })
                            .flex()
                            .items_center()
                            .gap(sp(8.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_accent_color(choice, cx);
                                }),
                            )
                            .child(
                                div()
                                    .w(sp(24.0))
                                    .h(sp(24.0))
                                    .rounded_full()
                                    .p(px(2.0))
                                    .border_2()
                                    .border_color(if active {
                                        rgb(hex)
                                    } else {
                                        rgba(0x00000000)
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .size_full()
                                            .rounded_full()
                                            .bg(rgb(hex))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(active, |circle| {
                                                circle.child(svg_icon(
                                                    IconName::Check,
                                                    10.5 * theme.ui_scale,
                                                    rgb(on_hex),
                                                ))
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .font(ui_sans(if active {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::MEDIUM
                                    }))
                                    .text_size(sp(12.0))
                                    .text_color(if active {
                                        rgb(theme.text_primary)
                                    } else {
                                        rgb(theme.text_secondary)
                                    })
                                    .child(choice.label()),
                            )
                    })),
            )
            .into_any_element()
    }

    // --- 3. Subtitles & Audio: Live Subtitle Preview & Studio Card ---
    fn render_live_subtitle_studio_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = active_theme();
        let prefs = &self.preferences;
        let ink_accent = accent_ink(&theme);
        let active_bg = prefs.effective_subtitle_background();
        let active_color_rgb = parse_subtitle_hex_rgb(&prefs.subtitle_color_hex);
        let active_color_name = subtitle_color_label(&prefs.subtitle_color_hex);
        let active_size_idx = match prefs.subtitle_font_size {
            0..=41 => 0,
            42..=51 => 1,
            52..=61 => 2,
            _ => 3,
        };

        let preview_font_px = (prefs.subtitle_font_size as f32 * 0.40).clamp(11.0, 32.0);
        let bottom_pad_px =
            ((100.0 - prefs.subtitle_vertical_pos.clamp(60, 100) as f32) / 40.0) * 86.0 + 14.0;

        let subtitle_pill = {
            let base = div()
                .max_w(px(540.0))
                .px(px(14.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .font(ui_sans(FontWeight::SEMIBOLD))
                .text_size(px(preview_font_px))
                .line_height(px(preview_font_px * 1.28))
                .text_color(rgb(active_color_rgb));

            match active_bg {
                SubtitleBackgroundChoice::Shadow => base.shadow(vec![BoxShadow::new(
                    px(0.0),
                    px(2.0),
                    rgba(0x000000D9).into(),
                )
                .blur_radius(px(10.0))]),
                SubtitleBackgroundChoice::Outline => base
                    .bg(rgba(0x00000040))
                    .border_1()
                    .border_color(rgba(0x000000CC))
                    .shadow(vec![BoxShadow::new(
                        px(0.0),
                        px(1.0),
                        rgba(0x000000F2).into(),
                    )
                    .blur_radius(px(4.0))]),
                SubtitleBackgroundChoice::Box => base
                    .bg(rgba(0x000000C8))
                    .border_1()
                    .border_color(rgba(0xFFFFFF1A)),
            }
            .child("The horizon remembers every signal we leave behind.")
        };

        settings_glass_card(&theme)
            .p(sp(18.0))
            .gap(sp(16.0))
            // Header + Reset button
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sp(12.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(8.0))
                            .child(svg_icon_weighted(
                                IconName::Subtitles,
                                IconWeight::Fill,
                                15.0 * theme.ui_scale,
                                rgb(ink_accent),
                            ))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(sp(14.0))
                                    .text_color(rgb(theme.text_primary))
                                    .child("Live subtitle preview"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(8.0))
                            .child(neutral_chip(format!(
                                "{} · {} · {}px · {}%",
                                active_color_name,
                                active_bg.label(),
                                prefs.subtitle_font_size,
                                prefs.subtitle_vertical_pos
                            )))
                            .child(choice_pill(
                                "btn-reset-sub-style",
                                "Reset",
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.reset_subtitle_style_prefs(cx);
                                }),
                            )),
                    ),
            )
            // 16:9-proportioned dark cinema frame
            .child(
                div()
                    .w_full()
                    .h(sp(214.0))
                    .rounded(px(RADIUS_CARD))
                    .overflow_hidden()
                    .relative()
                    .bg(linear_gradient(
                        180.0,
                        linear_color_stop(rgb(0x131724), 0.0),
                        linear_color_stop(rgb(0x07090F), 1.0),
                    ))
                    .border_1()
                    .border_color(rgba(theme.glass_border_strong))
                    // Subtle top & bottom cinema letterbox bars
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(px(14.0))
                            .bg(rgba(0x000000A6)),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0()
                            .h(px(14.0))
                            .bg(rgba(0x000000A6)),
                    )
                    // Subtle scene horizon highlight
                    .child(
                        div()
                            .absolute()
                            .top(px(28.0))
                            .left(px(18.0))
                            .font(mono_font())
                            .text_size(px(10.5))
                            .text_color(rgba(0xFFFFFF55))
                            .child("16:9 CINEMA PREVIEW · 01:14:22"),
                    )
                    // Live positioned subtitle text
                    .child(
                        div()
                            .size_full()
                            .pb(px(bottom_pad_px))
                            .px(sp(20.0))
                            .flex()
                            .items_end()
                            .justify_center()
                            .child(subtitle_pill),
                    ),
            )
            // Inline quick controls: Font Size + Backdrop Style
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap(sp(14.0))
                    // Font size
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(sp(6.0))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(sp(12.0))
                                    .text_color(rgb(theme.text_secondary))
                                    .child("Font size"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(sp(8.0))
                                    .child(render_segmented_control(
                                        "seg-sub-size",
                                        SUBTITLE_SIZE_PRESETS.iter().enumerate().map(
                                            |(idx, &(label, size_px))| {
                                                (
                                                    label,
                                                    idx == active_size_idx,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.set_subtitle_font_size(size_px, cx);
                                                    }),
                                                )
                                            },
                                        ),
                                    ))
                                    .child(render_stepper(
                                        "stepper-sub-size",
                                        format!("{}px", prefs.subtitle_font_size),
                                        cx.listener(|this, _, _, cx| {
                                            this.step_subtitle_font_size(-2, cx);
                                        }),
                                        cx.listener(|this, _, _, cx| {
                                            this.set_subtitle_font_size(
                                                DEFAULT_SUBTITLE_FONT_SIZE,
                                                cx,
                                            );
                                        }),
                                        cx.listener(|this, _, _, cx| {
                                            this.step_subtitle_font_size(2, cx);
                                        }),
                                    )),
                            ),
                    )
                    // Backdrop style
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(sp(6.0))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(sp(12.0))
                                    .text_color(rgb(theme.text_secondary))
                                    .child("Backdrop style"),
                            )
                            .child(render_segmented_control(
                                "seg-sub-backdrop",
                                SubtitleBackgroundChoice::ALL.into_iter().map(|choice| {
                                    (
                                        choice.label(),
                                        active_bg == choice,
                                        cx.listener(move |this, _, _, cx| {
                                            this.set_subtitle_background_choice(choice, cx);
                                        }),
                                    )
                                }),
                            )),
                    ),
            )
            // Inline quick controls: Color swatches + Vertical Position
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap(sp(14.0))
                    // Color swatches
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(sp(6.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::MEDIUM))
                                            .text_size(sp(12.0))
                                            .text_color(rgb(theme.text_secondary))
                                            .child("HDR-safe color"),
                                    )
                                    .child(
                                        div()
                                            .font(mono_font())
                                            .text_size(sp(11.0))
                                            .text_color(rgb(theme.text_muted))
                                            .child(active_color_name),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(sp(8.0))
                                    .children(SUBTITLE_SWATCHES.iter().enumerate().map(
                                        |(idx, &(hex, _))| {
                                            let selected =
                                                prefs.subtitle_color_hex.eq_ignore_ascii_case(hex);
                                            let color_u32 = parse_subtitle_hex_rgb(hex);
                                            let hex_owned = hex.to_owned();
                                            div()
                                                .id(("settings-sub-swatch", idx))
                                                .cursor_pointer()
                                                .w(sp(30.0))
                                                .h(sp(30.0))
                                                .rounded_full()
                                                .p(px(2.0))
                                                .border_2()
                                                .border_color(if selected {
                                                    rgb(theme.accent)
                                                } else {
                                                    rgba(theme.glass_border)
                                                })
                                                .hover(move |s| {
                                                    if selected {
                                                        s
                                                    } else {
                                                        s.border_color(rgba(
                                                            theme.glass_border_strong,
                                                        ))
                                                    }
                                                })
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.set_subtitle_color_hex(&hex_owned, cx);
                                                    }),
                                                )
                                                .child(
                                                    div()
                                                        .size_full()
                                                        .rounded_full()
                                                        .bg(rgb(color_u32))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .when(selected, |c| {
                                                            c.child(svg_icon(
                                                                IconName::Check,
                                                                10.5 * theme.ui_scale,
                                                                rgb(0x0B0B0D),
                                                            ))
                                                        }),
                                                )
                                        },
                                    )),
                            ),
                    )
                    // Vertical position
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(sp(6.0))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(sp(12.0))
                                    .text_color(rgb(theme.text_secondary))
                                    .child("Vertical position"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(sp(8.0))
                                    .child(render_segmented_control(
                                        "seg-sub-pos",
                                        SUBTITLE_POSITION_PRESETS.iter().map(|&(pos, label)| {
                                            (
                                                label,
                                                prefs.subtitle_vertical_pos == pos,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.set_subtitle_vertical_pos(pos, cx);
                                                }),
                                            )
                                        }),
                                    ))
                                    .child(render_stepper(
                                        "stepper-sub-pos",
                                        format!("{}%", prefs.subtitle_vertical_pos),
                                        cx.listener(|this, _, _, cx| {
                                            this.step_subtitle_vertical_pos(-2, cx);
                                        }),
                                        cx.listener(|this, _, _, cx| {
                                            this.set_subtitle_vertical_pos(
                                                DEFAULT_SUBTITLE_VERTICAL_POS,
                                                cx,
                                            );
                                        }),
                                        cx.listener(|this, _, _, cx| {
                                            this.step_subtitle_vertical_pos(2, cx);
                                        }),
                                    )),
                            ),
                    ),
            )
            .into_any_element()
    }

    // --- 4. Streaming & Debrid: Piece Cache Budget Visual Pill Selector ---
    fn render_piece_cache_budget_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = active_theme();
        let prefs = &self.preferences;
        let used = self.engine.cache_usage_bytes();

        div()
            .py(sp(16.0))
            .flex()
            .flex_col()
            .gap(sp(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sp(12.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(sp(2.0))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::MEDIUM))
                                    .text_size(sp(14.0))
                                    .text_color(rgb(theme.text_primary))
                                    .child("Piece cache budget"),
                            )
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::NORMAL))
                                    .text_size(sp(12.0))
                                    .text_color(rgb(theme.text_secondary))
                                    .child("Warm-resume Disk LRU (5 GB default) or zero-SSD-write RAM ring buffer."),
                            ),
                    )
                    .child(neutral_chip(format!(
                        "{} · {}",
                        format_bytes(used),
                        prefs.storage_mode.short_badge()
                    ))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(sp(12.0))
                    // Disk LRU pills
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(6.0))
                            .child(
                                div()
                                    .font(mono_font())
                                    .text_size(sp(11.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child("DISK LRU"),
                            )
                            .children(StorageModeChoice::ALL_DISK_GB.into_iter().map(|gb| {
                                let choice = StorageModeChoice::DiskLru { budget_gb: gb };
                                let active = prefs.storage_mode == choice;
                                let label = if gb == 5 {
                                    "5 GB (Default)".to_owned()
                                } else {
                                    format!("{gb} GB")
                                };
                                choice_pill(
                                    SharedString::from(format!("pill-cache-disk-{gb}")),
                                    label,
                                    active,
                                    cx.listener(move |this, _, _, cx| {
                                        this.set_storage_mode_preference(choice, cx);
                                    }),
                                )
                            })),
                    )
                    // RAM ring pills
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(6.0))
                            .child(
                                div()
                                    .font(mono_font())
                                    .text_size(sp(11.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child("RAM RING"),
                            )
                            .children(StorageModeChoice::ALL_RAM_GB.into_iter().map(|gb| {
                                let choice = StorageModeChoice::RamOnly { budget_gb: gb };
                                let active = prefs.storage_mode == choice;
                                choice_pill(
                                    SharedString::from(format!("pill-cache-ram-{gb}")),
                                    format!("{gb} GB"),
                                    active,
                                    cx.listener(move |this, _, _, cx| {
                                        this.set_storage_mode_preference(choice, cx);
                                    }),
                                )
                            })),
                    ),
            )
            .into_any_element()
    }

    // --- 5. Integrations & Addons: Prominent Addons Studio Launch Card ---
    fn render_addons_studio_launch_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = active_theme();
        let active_count = self.core.with_model(|m| m.ctx.profile.addons.len());
        let muted_count = self.core.list_muted_addons().len();
        let telemetry = self.core.addon_telemetry();
        let cache_stats = self.core.addon_cache_stats();

        let mut degraded_count = 0usize;
        let mut slow_count = 0usize;
        let mut healthy_count = 0usize;
        for snap in telemetry.values() {
            match snap.status {
                AddonHealthStatus::Degraded | AddonHealthStatus::CircuitOpen { .. } => {
                    degraded_count += 1;
                }
                AddonHealthStatus::Slow => slow_count += 1,
                AddonHealthStatus::Healthy => healthy_count += 1,
                AddonHealthStatus::Idle => {}
            }
        }

        let (health_label, health_fg) = if degraded_count > 0 {
            ("Degraded addon", theme.accent_rose)
        } else if slow_count > 0 {
            ("Slow response", theme.accent_amber)
        } else if healthy_count > 0 {
            ("All healthy", theme.status_watched)
        } else {
            ("Ready", theme.status_watching)
        };
        let health_border = rgba_with_alpha(health_fg, if theme.is_light { 0x66 } else { 0x4d });
        let health_bg = rgba_with_alpha(health_fg, if theme.is_light { 0x1a } else { 0x1c });

        div()
            .py(sp(16.0))
            .flex()
            .items_center()
            .justify_between()
            .gap(sp(18.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(sp(6.0))
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
                                    .text_color(rgb(theme.text_primary))
                                    .child("Stremio Addons Studio"),
                            )
                            .child(neutral_chip(format!("{active_count} active")))
                            .when(muted_count > 0, |row| {
                                row.child(neutral_chip(format!("{muted_count} muted")))
                            })
                            .child(semantic_status_pill(
                                IconName::ShieldCheck,
                                health_label,
                                health_fg,
                                health_border,
                                health_bg,
                            )),
                    )
                    .child(
                        div()
                            .font(ui_sans(FontWeight::NORMAL))
                            .text_size(sp(12.5))
                            .text_color(rgb(theme.text_secondary))
                            .child(format!(
                                "Manage addon priority, soft-mute Debrid sources, or explore curated catalogs · {} HTTP cache hits.",
                                cache_stats.hits
                            )),
                    ),
            )
            .child(
                div().flex_shrink_0().child(primary_accent_button(
                    "btn-open-addons-studio",
                    Some(IconName::Layers),
                    "Open Addons Studio",
                    cx.listener(|this, _, _, cx| {
                        this.navigate_tab(SidebarTab::Addons, cx);
                    }),
                )),
            )
            .into_any_element()
    }

    // --- 5. Integrations & Addons: Local-Only 0600 Security Banner ---
    fn render_integrations_security_banner() -> impl IntoElement {
        let theme = active_theme();
        let border_col =
            rgba_with_alpha(theme.status_watched, if theme.is_light { 0x55 } else { 0x40 });
        let bg_col =
            rgba_with_alpha(theme.status_watched, if theme.is_light { 0x16 } else { 0x12 });

        div()
            .px(sp(16.0))
            .py(sp(12.0))
            .rounded(px(RADIUS_CARD))
            .bg(rgba(bg_col))
            .border_1()
            .border_color(rgba(border_col))
            .flex()
            .items_start()
            .gap(sp(12.0))
            .child(
                div().pt(px(2.0)).child(svg_icon_weighted(
                    IconName::ShieldCheck,
                    IconWeight::Fill,
                    16.0 * theme.ui_scale,
                    rgb(theme.status_watched),
                )),
            )
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
                            .text_size(sp(13.0))
                            .text_color(rgb(theme.text_primary))
                            .child("Local-only 0600 credential storage"),
                    )
                    .child(
                        div()
                            .font(ui_sans(FontWeight::NORMAL))
                            .text_size(sp(12.0))
                            .text_color(rgb(theme.text_secondary))
                            .child(
                                "API keys are written with 0600 permissions to ferrite/private_integrations.json, zeroized in memory on drop, and never synced to Stremio profiles or addons.",
                            ),
                    ),
            )
    }

    // --- 6. System & Storage: Live Storage & Cache Gauge Card ---
    fn render_storage_cache_gauge_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = active_theme();
        let prefs = &self.preferences;
        let ink_accent = accent_ink(&theme);

        // 1. Image Disk Cache (O(1) atomic)
        let img_used = self.poster_cache.disk_cache_usage_bytes();
        let img_ratio = ratio_clamped(img_used, MAX_DISK_CACHE_BYTES);
        let img_pct = (img_ratio * 100.0).round() as u32;
        let img_bar_color = if img_ratio > 0.85 {
            theme.accent_amber
        } else {
            theme.accent
        };

        // 2. Torrent Piece Cache
        let engine_snap = self.engine.engine_stats();
        let torrent_used = self.engine.cache_usage_bytes();
        let torrent_budget = prefs.storage_mode.budget_bytes();
        let torrent_ratio = ratio_clamped(torrent_used, torrent_budget);
        let torrent_sub = format!(
            "{} · {} active swarms · {} peers · {:.2} MB/s",
            prefs.storage_mode.short_badge(),
            engine_snap.active_torrents,
            engine_snap.connected_peers,
            engine_snap.download_speed_bytes / (1024.0 * 1024.0),
        );

        // 3. Addon HTTP Cache
        let addon_stats = self.core.addon_cache_stats();
        let total_lookups = addon_stats
            .hits
            .saturating_add(addon_stats.disk_hits)
            .saturating_add(addon_stats.misses);
        let hit_ratio = ratio_clamped(
            addon_stats.hits.saturating_add(addon_stats.disk_hits),
            total_lookups,
        );
        let hit_pct = (hit_ratio * 100.0).round() as u32;
        let addon_sub = format!(
            "{} entries · {} memory hits · {} disk hits ({hit_pct}% hit rate)",
            addon_stats.entries, addon_stats.hits, addon_stats.disk_hits
        );

        settings_glass_card(&theme)
            .p(sp(18.0))
            .gap(sp(16.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sp(12.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(8.0))
                            .child(svg_icon_weighted(
                                IconName::HardDrive,
                                IconWeight::Fill,
                                15.0 * theme.ui_scale,
                                rgb(ink_accent),
                            ))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(sp(14.5))
                                    .text_color(rgb(theme.text_primary))
                                    .child("Live storage & cache telemetry"),
                            ),
                    )
                    .child(secondary_glass_button(
                        "btn-clear-metadata-cache",
                        Some(IconName::Trash2),
                        "Clear Image & Metadata Cache",
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.clear_metadata_and_image_cache(cx);
                        }),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(14.0))
                    .child(render_cache_gauge_bar(
                        &theme,
                        "Image Disk Cache",
                        format!(
                            "{} / {} ({img_pct}%)",
                            format_bytes(img_used),
                            format_bytes(MAX_DISK_CACHE_BYTES)
                        ),
                        "Bounded downscaled poster, logo, and backdrop variants on disk.",
                        img_ratio,
                        img_bar_color,
                    ))
                    .child(render_cache_gauge_bar(
                        &theme,
                        "Torrent Piece Cache",
                        format!(
                            "{} / {}",
                            format_bytes(torrent_used),
                            format_bytes(torrent_budget)
                        ),
                        torrent_sub,
                        torrent_ratio,
                        theme.status_watching,
                    ))
                    .child(render_cache_gauge_bar(
                        &theme,
                        "Addon HTTP Cache",
                        format_bytes(addon_stats.total_bytes as u64),
                        addon_sub,
                        hit_ratio,
                        theme.status_watched,
                    )),
            )
            .into_any_element()
    }

    fn render_settings_row(&self, row_id: SettingsRowId, cx: &mut Context<Self>) -> AnyElement {
        let theme = active_theme();
        let prefs = &self.preferences;
        let exp = prefs.experimental_video;

        match row_id {
            // --- 1. Appearance ---
            SettingsRowId::ThemePreset => self.render_theme_preset_row(cx),
            SettingsRowId::AccentColor => self.render_accent_color_row(cx),
            SettingsRowId::AmbientArtworkBleed => render_control_row(
                "Ambient artwork bleed",
                "Cast a soft blurred color aura from active posters and backdrops behind the interface.",
                glass_switch(
                    "switch-ambient-bleed",
                    prefs.ambient_bleed_enabled,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_ambient_bleed(cx);
                    }),
                ),
            ),
            SettingsRowId::UiScale => render_control_row(
                "UI scale",
                "Adjust overall typography, control heights, and spacing across the workspace.",
                render_segmented_control(
                    "seg-ui-scale",
                    UiScaleChoice::ALL.into_iter().map(|scale| {
                        (
                            scale.label(),
                            prefs.ui_scale == scale,
                            cx.listener(move |this, _, _, cx| {
                                this.set_ui_scale(scale, cx);
                            }),
                        )
                    }),
                ),
            ),
            SettingsRowId::CardDensity => render_control_row(
                "Card density",
                "Control poster card proportions and grid density across Home, Discover, and Library.",
                render_segmented_control(
                    "seg-card-density",
                    CardDensityChoice::ALL.into_iter().map(|density| {
                        (
                            density.label(),
                            prefs.card_density == density,
                            cx.listener(move |this, _, _, cx| {
                                this.set_card_density(density, cx);
                            }),
                        )
                    }),
                ),
            ),
            SettingsRowId::ShowHomeSpotlight => render_control_row(
                "Featured hero on Home",
                "Display an editorial hero spotlight with title logo and quick Resume / Details actions on Home.",
                glass_switch(
                    "switch-home-spotlight",
                    prefs.show_home_spotlight,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_show_home_spotlight(cx);
                    }),
                ),
            ),
            SettingsRowId::OsdAutohideSecs => render_control_row(
                "Player OSD auto-hide",
                "Idle duration before player transport controls fade away during playback.",
                render_segmented_control(
                    "seg-osd-autohide",
                    [(2_u8, "2s"), (3_u8, "3s"), (5_u8, "5s")]
                        .into_iter()
                        .map(|(secs, label)| {
                            (
                                label,
                                prefs.osd_autohide_secs == secs,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_osd_autohide_secs(secs, cx);
                                }),
                            )
                        }),
                ),
            ),

            // --- 2. Playback ---
            SettingsRowId::AutoPlayNextEpisode => render_control_row(
                "Auto-play & pre-buffer next episode",
                "Pre-match and pre-buffer Episode N+1 45s before credits and auto-play at end of stream.",
                glass_switch(
                    "toggle-auto-play-next-episode",
                    prefs.auto_play_next_episode,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_auto_play_next_episode(cx);
                    }),
                ),
            ),
            SettingsRowId::PreventDisplaySleep => render_control_row(
                "Prevent display sleep while playing",
                "Keep the display awake and suppress screensaver lock during active playback.",
                glass_switch(
                    "toggle-prevent-display-sleep",
                    prefs.prevent_display_sleep,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_prevent_display_sleep(cx);
                    }),
                ),
            ),
            SettingsRowId::OsMediaControls => render_control_row(
                "OS media controls (MPRIS2 / SMTC)",
                "Publish now-playing metadata and transport keys to Linux MPRIS2 and Windows SMTC.",
                glass_switch(
                    "toggle-os-media-controls",
                    prefs.enable_os_media_controls,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_os_media_controls(cx);
                    }),
                ),
            ),
            SettingsRowId::GpuUpscaler => render_control_row(
                "GPU upscaler",
                "Scaling filter used by libmpv when upscaling lower-resolution streams (Spline36 default).",
                render_segmented_control(
                    "seg-upscaler",
                    UpscaleChoice::ALL.into_iter().map(|choice| {
                        let label = match choice {
                            UpscaleChoice::Bilinear => "Bilinear",
                            UpscaleChoice::Spline36 => "Spline36",
                            UpscaleChoice::Lanczos => "Lanczos3",
                            UpscaleChoice::EwaLanczos => "EWA LanczosSharp",
                        };
                        (
                            label,
                            prefs.upscale_mode == choice,
                            cx.listener(move |this, _, _, cx| {
                                this.set_upscale_choice(choice, cx);
                            }),
                        )
                    }),
                ),
            ),
            SettingsRowId::HdrToneMapping => {
                let open = self.settings_open_dropdown == Some(SettingsDropdown::ToneMapping);
                let items = ToneMappingChoice::ALL.into_iter().map(|choice| {
                    let active = prefs.tone_mapping == choice;
                    (
                        choice.label(),
                        active,
                        cx.listener(move |this, _, _, cx| {
                            this.settings_open_dropdown = None;
                            this.set_tone_mapping_choice(choice, cx);
                        }),
                    )
                });
                render_control_row(
                    "HDR tone mapping",
                    "Select the libmpv HDR-to-SDR tone-mapping curve for high dynamic range playback.",
                    render_settings_dropdown(
                        "settings-dd-tonemap",
                        prefs.tone_mapping.label(),
                        open,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.settings_open_dropdown = if this.settings_open_dropdown
                                == Some(SettingsDropdown::ToneMapping)
                            {
                                None
                            } else {
                                Some(SettingsDropdown::ToneMapping)
                            };
                            cx.notify();
                        }),
                        items,
                    ),
                )
            }
            SettingsRowId::GlslShaderPreset => {
                let open = self.settings_open_dropdown == Some(SettingsDropdown::ShaderPreset);
                let items = ShaderPresetChoice::ALL.into_iter().map(|preset| {
                    let active = exp.shader_preset == preset;
                    (
                        preset.label(),
                        active,
                        cx.listener(move |this, _, _, cx| {
                            this.settings_open_dropdown = None;
                            this.preferences.experimental_video.shader_preset = preset;
                            this.apply_experimental_video_to_mpv();
                            let _ = this.preferences.save();
                            cx.notify();
                        }),
                    )
                });
                render_control_row(
                    "GLSL shader preset",
                    "Experimental libmpv post-processing shader preset (off by default).",
                    render_settings_dropdown(
                        "settings-dd-shader",
                        exp.shader_preset.label(),
                        open,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.settings_open_dropdown = if this.settings_open_dropdown
                                == Some(SettingsDropdown::ShaderPreset)
                            {
                                None
                            } else {
                                Some(SettingsDropdown::ShaderPreset)
                            };
                            cx.notify();
                        }),
                        items,
                    ),
                )
            }
            SettingsRowId::DarkSceneDebanding => render_control_row(
                "Dark-scene debanding",
                "Smooth 8-bit compression banding gradients in dark scenes.",
                glass_switch(
                    "exp-toggle-deband",
                    exp.deband_enabled,
                    cx.listener(|this, _, _, cx| {
                        this.preferences.experimental_video.deband_enabled =
                            !this.preferences.experimental_video.deband_enabled;
                        this.apply_experimental_video_to_mpv();
                        let _ = this.preferences.save();
                        cx.notify();
                    }),
                ),
            ),
            SettingsRowId::DolbyVisionReshape => render_control_row(
                "Dolby Vision reshape",
                "Enable experimental Dolby Vision RPU metadata reshaping in libmpv.",
                glass_switch(
                    "exp-toggle-dv",
                    exp.dolby_vision_reshape,
                    cx.listener(|this, _, _, cx| {
                        this.preferences.experimental_video.dolby_vision_reshape =
                            !this.preferences.experimental_video.dolby_vision_reshape;
                        this.apply_experimental_video_to_mpv();
                        let _ = this.preferences.save();
                        cx.notify();
                    }),
                ),
            ),
            SettingsRowId::DynamicHdrPeak => render_control_row(
                "Dynamic HDR peak",
                "Compute per-frame HDR peak luminance for dynamic tone mapping.",
                glass_switch(
                    "exp-toggle-peak",
                    exp.hdr_compute_peak,
                    cx.listener(|this, _, _, cx| {
                        this.preferences.experimental_video.hdr_compute_peak =
                            !this.preferences.experimental_video.hdr_compute_peak;
                        this.apply_experimental_video_to_mpv();
                        let _ = this.preferences.save();
                        cx.notify();
                    }),
                ),
            ),
            SettingsRowId::HardwareZeroCopyStatus => {
                let hwdec_label = if self.player_snapshot.hwdec_current.is_empty() {
                    if cfg!(target_os = "windows") {
                        "DirectComposition"
                    } else {
                        "Wayland subsurface"
                    }
                } else {
                    self.player_snapshot.hwdec_current.as_str()
                };
                let (pill_label, fg) = if self.mpv.is_some() {
                    ("Zero-copy active", theme.status_watched)
                } else {
                    ("Headless / standby", theme.status_watching)
                };
                let border = rgba_with_alpha(fg, if theme.is_light { 0x66 } else { 0x4d });
                let bg = rgba_with_alpha(fg, if theme.is_light { 0x1a } else { 0x1c });
                render_control_row(
                    "Hardware zero-copy video pipeline",
                    "DirectComposition (Windows) / Wayland subsurface zero-copy presentation with libmpv hwdec.",
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(8.0))
                        .child(neutral_chip(hwdec_label.to_owned()))
                        .child(semantic_status_pill(
                            IconName::Cpu,
                            pill_label,
                            fg,
                            border,
                            bg,
                        )),
                )
            }

            // --- 3. Subtitles & Audio ---
            SettingsRowId::SubtitleStyleSummary => self.render_live_subtitle_studio_card(cx),
            SettingsRowId::PreferredAudioLang => render_control_row(
                "Preferred audio language",
                "Default ISO-639-2 audio stream preference when starting playback.",
                render_segmented_control(
                    "seg-audio-lang",
                    AUDIO_LANG_OPTIONS.iter().map(|&(code, label)| {
                        (
                            label,
                            prefs.preferred_audio_lang.eq_ignore_ascii_case(code),
                            cx.listener(move |this, _, _, cx| {
                                this.set_preferred_audio_lang(code, cx);
                            }),
                        )
                    }),
                ),
            ),
            SettingsRowId::PreferredSubtitleLang => render_control_row(
                "Preferred subtitle language",
                "Default subtitle language when auto-selecting tracks, or Off to disable.",
                render_segmented_control(
                    "seg-subtitle-lang",
                    SUBTITLE_LANG_OPTIONS.iter().map(|&(code, label)| {
                        (
                            label,
                            prefs.preferred_subtitle_lang.eq_ignore_ascii_case(code),
                            cx.listener(move |this, _, _, cx| {
                                this.set_preferred_subtitle_lang(code, cx);
                            }),
                        )
                    }),
                ),
            ),
            SettingsRowId::AutoSelectSubtitles => render_control_row(
                "Auto-select subtitles",
                "Automatically enable matching subtitle tracks when playback begins.",
                glass_switch(
                    "toggle-auto-select-subtitles",
                    prefs.auto_select_subtitles,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_auto_select_subtitles(cx);
                    }),
                ),
            ),
            SettingsRowId::AutoMatchNativeSubtitles => render_control_row(
                "Auto-match native hash subtitles (OpenSubtitles + SubDL)",
                "Compute 64 KiB stream hash and auto-align matching subtitles when playback starts.",
                glass_switch(
                    "toggle-auto-match-native-subs",
                    prefs.auto_match_native_subtitles,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_auto_match_native_subtitles(cx);
                    }),
                ),
            ),
            SettingsRowId::DefaultSubtitleDelay => {
                let delay = prefs.default_subtitle_delay_ms;
                let value_label = if delay > 0 {
                    format!("+{delay} ms")
                } else {
                    format!("{delay} ms")
                };
                render_control_row(
                    "Default subtitle delay",
                    "Baseline subtitle timing offset (-5000 ms to +5000 ms) applied to new sessions.",
                    render_stepper(
                        "stepper-sub-delay",
                        value_label,
                        cx.listener(|this, _, _, cx| {
                            this.step_default_subtitle_delay(-100, cx);
                        }),
                        cx.listener(|this, _, _, cx| {
                            let cur = this.preferences.default_subtitle_delay_ms;
                            if cur != 0 {
                                this.step_default_subtitle_delay(-cur, cx);
                            }
                        }),
                        cx.listener(|this, _, _, cx| {
                            this.step_default_subtitle_delay(100, cx);
                        }),
                    ),
                )
            }
            SettingsRowId::SubtitleAssScaleOverride => render_control_row(
                "ASS scale override",
                "Apply your font sizing preferences to styled ASS/SSA subtitle tracks.",
                glass_switch(
                    "toggle-ass-scale-override",
                    prefs.subtitle_ass_scale_override,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_subtitle_ass_scale_override(cx);
                    }),
                ),
            ),

            // --- 4. Streaming & Debrid ---
            SettingsRowId::PieceCacheMode => self.render_piece_cache_budget_row(cx),
            SettingsRowId::LocalDebridProvider => render_control_row(
                "Local Debrid provider",
                "Resolve cached magnet hashes directly in Rust without external proxy addons.",
                render_segmented_control(
                    "seg-local-debrid-provider",
                    LOCAL_DEBRID_PROVIDERS.iter().map(|&(label, idx)| {
                        (
                            label,
                            self.local_debrid_provider_idx == idx,
                            cx.listener(move |this, _, _, cx| {
                                this.set_local_debrid_provider(idx, cx);
                            }),
                        )
                    }),
                ),
            ),
            SettingsRowId::LocalDebridCachedOnly => render_control_row(
                "Cached-only stream filter",
                "Hide uncached P2P torrents when a local Debrid provider is active.",
                glass_switch(
                    "toggle-local-debrid-cached-only",
                    self.local_debrid_cached_only,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_local_debrid_cached_only(cx);
                    }),
                ),
            ),
            SettingsRowId::LocalDebridDirectUnrestrict => {
                let latency_badge = self
                    .last_debrid_latency_ms
                    .map(|ms| format!("{ms} ms"))
                    .unwrap_or_else(|| "~60ms target".to_owned());
                render_control_row(
                    "Direct ~60ms Rust unrestrict",
                    "Keep a warm HTTP/2 connection pool for sub-100ms unrestrict and instant cache checks.",
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(10.0))
                        .child(neutral_chip(latency_badge))
                        .child(glass_switch(
                            "toggle-local-debrid-direct",
                            self.local_debrid_direct_unrestrict,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_local_debrid_direct_unrestrict(cx);
                            }),
                        )),
                )
            }
            SettingsRowId::DeadSwarmFailover => render_control_row(
                "Dead-swarm auto-failover",
                "Step to the next ranked stream if a swarm stalls before first byte.",
                div()
                    .flex()
                    .items_center()
                    .gap(sp(10.0))
                    .child(render_segmented_control(
                        "seg-failover-timeout",
                        [6_u64, 10, 15, 20].into_iter().map(|secs| {
                            let label: SharedString = format!("{secs}s").into();
                            (
                                label,
                                prefs.dead_swarm_timeout_secs == secs,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_dead_swarm_timeout_secs(secs, cx);
                                }),
                            )
                        }),
                    ))
                    .child(glass_switch(
                        "toggle-dead-swarm",
                        prefs.dead_swarm_failover,
                        cx.listener(|this, _, _, cx| {
                            this.preferences.dead_swarm_failover =
                                !this.preferences.dead_swarm_failover;
                            let _ = this.preferences.save();
                            cx.notify();
                        }),
                    )),
            ),
            SettingsRowId::UpnpPortMapping => render_control_row(
                "UPnP / NAT-PMP port mapping",
                "Automatically map inbound BitTorrent listener ports on your local router.",
                glass_switch(
                    "toggle-upnp",
                    prefs.enable_upnp,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_upnp_preference(cx);
                    }),
                ),
            ),
            SettingsRowId::AddonDebridVisibility => render_control_row(
                "Addon Debrid visibility override",
                "Auto-detect Debrid from installed addon URLs or override Cached stream visibility.",
                render_segmented_control(
                    "seg-addon-debrid-mode",
                    DebridAccessMode::ALL.into_iter().map(|mode| {
                        let short = match mode {
                            DebridAccessMode::AutoDetect => "Auto-detect",
                            DebridAccessMode::Subscribed => "Show cached",
                            DebridAccessMode::NoneHideCached => "Hide cached",
                        };
                        (
                            short,
                            prefs.debrid_access_mode == mode,
                            cx.listener(move |this, _, _, cx| {
                                this.set_debrid_access_mode(mode, cx);
                            }),
                        )
                    }),
                ),
            ),
            SettingsRowId::ProwlarrFallbackMode => render_control_row(
                "Prowlarr fallback mode",
                "Query your local Prowlarr or Torznab endpoint on addon outage or always merge.",
                render_segmented_control(
                    "seg-prowlarr-mode",
                    PROWLARR_MODES.iter().map(|&(label, idx)| {
                        (
                            label,
                            self.prowlarr_mode_idx == idx,
                            cx.listener(move |this, _, _, cx| {
                                this.set_prowlarr_mode(idx, cx);
                            }),
                        )
                    }),
                ),
            ),

            // --- 5. Integrations & Addons ---
            SettingsRowId::OpenAddonsStudio => self.render_addons_studio_launch_row(cx),
            SettingsRowId::ClipboardAddonDetect => render_control_row(
                "Clipboard addon link detection",
                "Detect copied stremio:// or manifest.json addon URLs when opening Addons Studio.",
                glass_switch(
                    "toggle-clipboard-addon-detect",
                    prefs.clipboard_addon_detect,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_clipboard_addon_detect(cx);
                    }),
                ),
            ),
            SettingsRowId::TmdbApiKey => {
                let focused = self.focused_input == Some(FocusedInput::SettingsTmdbApiKey);
                let preview = mask_api_key_preview(&self.tmdb_key_input, self.tmdb_key_configured);
                let display: SharedString = if preview.is_empty() {
                    if focused {
                        "|".into()
                    } else {
                        "Paste TMDB v3 key or v4 token...".into()
                    }
                } else if focused {
                    format!("{preview}|").into()
                } else {
                    preview.into()
                };
                let has_any = !self.tmdb_key_input.is_empty() || self.tmdb_key_configured;
                let desc = self.tmdb_verify_status.clone().unwrap_or_else(|| {
                    "Optional personal TMDB key or Read Access Token stored locally (0600) for crisp title logos, backdrops, cast photos & filmographies.".to_owned()
                });
                render_control_row(
                    "TMDB API key / Read Access Token",
                    desc,
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(8.0))
                        .child(
                            div()
                                .id("input-tmdb-key")
                                .cursor_text()
                                .w(sp(200.0))
                                .h(sp(34.0))
                                .px(sp(10.0))
                                .rounded(px(RADIUS_POSTER))
                                .bg(rgba(theme.glass_trough_bg))
                                .border_1()
                                .border_color(if focused {
                                    rgb(theme.accent)
                                } else {
                                    rgba(theme.glass_border)
                                })
                                .flex()
                                .items_center()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.focus_settings_input(
                                            FocusedInput::SettingsTmdbApiKey,
                                            cx,
                                        );
                                    }),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .truncate()
                                        .font(mono_font())
                                        .text_size(sp(11.5))
                                        .text_color(if !has_any && !focused {
                                            rgb(theme.text_muted)
                                        } else {
                                            rgb(theme.text_primary)
                                        })
                                        .child(display),
                                ),
                        )
                        .when(has_any, |el| {
                            el.child(choice_pill(
                                "btn-clear-tmdb-key",
                                "Clear",
                                false,
                                cx.listener(|this, _, _, cx| {
                                    this.clear_tmdb_key(cx);
                                }),
                            ))
                        })
                        .child(choice_pill(
                            "btn-verify-tmdb",
                            "Verify",
                            true,
                            cx.listener(|this, _, _, cx| {
                                this.verify_tmdb_clicked(cx);
                            }),
                        )),
                )
            }
            SettingsRowId::OpenSubtitlesApiKey => self.render_masked_key_row(
                "input-opensubtitles-key",
                "btn-clear-opensubtitles-key",
                "OpenSubtitles API key",
                "Optional personal OpenSubtitles REST API key stored locally with 0600 permissions.",
                FocusedInput::SettingsOpenSubtitlesKey,
                &self.opensubtitles_key_input,
                self.opensubtitles_key_configured,
                "Paste OpenSubtitles API key...",
                cx.listener(|this, _, _, cx| {
                    this.clear_opensubtitles_key(cx);
                }),
                cx,
            ),
            SettingsRowId::SubdlApiKey => self.render_masked_key_row(
                "input-subdl-key",
                "btn-clear-subdl-key",
                "SubDL API key",
                "Optional SubDL API key stored locally with 0600 permissions.",
                FocusedInput::SettingsSubdlKey,
                &self.subdl_key_input,
                self.subdl_key_configured,
                "Paste SubDL API key...",
                cx.listener(|this, _, _, cx| {
                    this.clear_subdl_key(cx);
                }),
                cx,
            ),
            SettingsRowId::RealDebridApiKey => self.render_debrid_key_row(
                1,
                "Real-Debrid API key",
                "Private Real-Debrid API token stored locally with 0600 permissions.",
                FocusedInput::RealDebridApiKey,
                &self.real_debrid_key_input,
                self.real_debrid_key_configured,
                "Paste Real-Debrid token...",
                cx,
            ),
            SettingsRowId::TorBoxApiKey => self.render_debrid_key_row(
                2,
                "TorBox API key",
                "Private TorBox API token stored locally with 0600 permissions.",
                FocusedInput::TorBoxApiKey,
                &self.torbox_key_input,
                self.torbox_key_configured,
                "Paste TorBox API key...",
                cx,
            ),
            SettingsRowId::AllDebridApiKey => self.render_debrid_key_row(
                3,
                "AllDebrid API key",
                "Private AllDebrid API key stored locally with 0600 permissions.",
                FocusedInput::AllDebridApiKey,
                &self.alldebrid_key_input,
                self.alldebrid_key_configured,
                "Paste AllDebrid API key...",
                cx,
            ),
            SettingsRowId::ProwlarrUrl => {
                let focused = self.focused_input == Some(FocusedInput::ProwlarrUrl);
                let display: SharedString = if self.prowlarr_url_input.is_empty() {
                    if focused {
                        "|".into()
                    } else {
                        "http://127.0.0.1:9696".into()
                    }
                } else if focused {
                    format!("{}|", self.prowlarr_url_input).into()
                } else {
                    self.prowlarr_url_input.clone().into()
                };
                render_control_row(
                    "Local Prowlarr / Torznab URL",
                    "Base URL of your local Prowlarr or Torznab indexer (e.g. http://127.0.0.1:9696).",
                    div()
                        .id("input-prowlarr-url")
                        .cursor_text()
                        .w(sp(272.0))
                        .h(sp(34.0))
                        .px(sp(10.0))
                        .rounded(px(RADIUS_POSTER))
                        .bg(rgba(theme.glass_trough_bg))
                        .border_1()
                        .border_color(if focused {
                            rgb(theme.accent)
                        } else {
                            rgba(theme.glass_border)
                        })
                        .flex()
                        .items_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.focus_settings_input(FocusedInput::ProwlarrUrl, cx);
                            }),
                        )
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .font(mono_font())
                                .text_size(sp(11.5))
                                .text_color(if self.prowlarr_url_input.is_empty() && !focused {
                                    rgb(theme.text_muted)
                                } else {
                                    rgb(theme.text_primary)
                                })
                                .child(display),
                        ),
                )
            }
            SettingsRowId::ProwlarrApiKey => self.render_masked_key_row(
                "input-prowlarr-key",
                "btn-clear-prowlarr-key",
                "Prowlarr / Torznab API key",
                "X-Api-Key or Torznab apikey sent strictly to your local indexer endpoint.",
                FocusedInput::ProwlarrApiKey,
                &self.prowlarr_key_input,
                self.prowlarr_key_configured,
                "Paste Prowlarr / Torznab key...",
                cx.listener(|this, _, _, cx| {
                    this.clear_prowlarr_key(cx);
                }),
                cx,
            ),
            SettingsRowId::ProwlarrTimeoutAndTest => {
                let status_note = self.prowlarr_test_status.clone().unwrap_or_else(|| {
                    "Configure local indexer request timeout (1000-15000 ms) and test connectivity."
                        .to_owned()
                });
                render_control_row(
                    "Prowlarr timeout & connection test",
                    status_note,
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(8.0))
                        .child(render_stepper(
                            "stepper-prowlarr-timeout",
                            format!("{} ms", self.prowlarr_timeout_ms),
                            cx.listener(|this, _, _, cx| {
                                this.step_prowlarr_timeout(-500, cx);
                            }),
                            cx.listener(|this, _, _, cx| {
                                let delta = DEFAULT_PROWLARR_TIMEOUT_MS as i64
                                    - this.prowlarr_timeout_ms as i64;
                                if delta != 0 {
                                    this.step_prowlarr_timeout(delta, cx);
                                }
                            }),
                            cx.listener(|this, _, _, cx| {
                                this.step_prowlarr_timeout(500, cx);
                            }),
                        ))
                        .child(choice_pill(
                            "btn-test-prowlarr",
                            "Test Indexer",
                            true,
                            cx.listener(|this, _, _, cx| {
                                this.test_prowlarr_clicked(cx);
                            }),
                        )),
                )
            }

            // --- 6. System & Storage ---
            SettingsRowId::ClearMetadataImageCache => self.render_storage_cache_gauge_card(cx),
            SettingsRowId::SoftwareUpdateCheck => self.render_updates_row(cx),
            SettingsRowId::CheckUpdatesOnStartup => render_control_row(
                "Check for updates on startup",
                "Automatically check GitHub Releases for signed updates when Ferrite launches.",
                glass_switch(
                    "toggle-check-updates-startup",
                    prefs.check_updates_on_startup,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_check_updates_on_startup(cx);
                    }),
                ),
            ),
            SettingsRowId::ShortcutsGlobal => render_shortcut_grid_block(
                "Global navigation & palette shortcuts",
                &[
                    ("Ctrl/Cmd + K", "Open Command Palette"),
                    ("/", "Quick Command Palette"),
                    ("Alt + 1..6", "Switch primary tabs"),
                    ("F11", "Toggle fullscreen window"),
                    ("Esc", "Close menu / navigate back"),
                    ("Ctrl/Cmd + Q", "Quit Ferrite"),
                ],
            ),
            SettingsRowId::ShortcutsPlayerTransport => render_shortcut_grid_block(
                "Player transport & seeking shortcuts",
                &[
                    ("Space", "Play / Pause"),
                    ("Hold Space", "Temporary 2.0x speed"),
                    ("Left / Right", "Seek -5s / +5s"),
                    ("Shift + Left/Right", "Seek -30s / +30s"),
                    (", / .", "Step frame back / forward"),
                    ("Esc", "Close popover / exit player"),
                ],
            ),
            SettingsRowId::ShortcutsPlayerTracksAudio => render_shortcut_grid_block(
                "Player audio, subtitles & diagnostics shortcuts",
                &[
                    ("Up / Down", "Volume +5% / -5%"),
                    ("M", "Toggle mute"),
                    ("F / F11", "Toggle player fullscreen"),
                    ("A", "Cycle audio tracks"),
                    ("S", "Cycle subtitle tracks"),
                    ("Z / X", "Subtitle delay -100ms / +100ms"),
                    ("I", "Toggle Stats for Nerds"),
                ],
            ),
            SettingsRowId::LocalStreamingArchitecture => {
                let engine_url = self.engine.base_url().to_string();
                let watched_border = rgba_with_alpha(
                    theme.status_watched,
                    if theme.is_light { 0x66 } else { 0x4d },
                );
                let watched_bg = rgba_with_alpha(
                    theme.status_watched,
                    if theme.is_light { 0x1a } else { 0x1c },
                );
                div()
                    .py(sp(16.0))
                    .flex()
                    .flex_col()
                    .gap(sp(12.0))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap(sp(12.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(sp(10.0))
                                    .child(
                                        div()
                                            .font(editorial_serif())
                                            .text_size(sp(24.0))
                                            .text_color(rgb(theme.text_primary))
                                            .child(format!("Ferrite v{CURRENT_APP_VERSION}")),
                                    )
                                    .child(neutral_chip("GPUI + libmpv")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(sp(8.0))
                                    .child(semantic_status_pill(
                                        IconName::ShieldCheck,
                                        "SHA-256 verified",
                                        theme.status_watched,
                                        watched_border,
                                        watched_bg,
                                    ))
                                    .child(neutral_chip("0600 Local Keys"))
                                    .child(neutral_chip("Zero Telemetry")),
                            ),
                    )
                    .child(
                        div()
                            .font(ui_sans(FontWeight::NORMAL))
                            .text_size(sp(12.5))
                            .text_color(rgb(theme.text_secondary))
                            .child(format!(
                                "Torrents and Debrid unrestrict resolve locally in Rust over loopback ({engine_url}). Private API credentials stay isolated on disk with 0600 permissions and zero telemetry is collected."
                            )),
                    )
                    .into_any_element()
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_masked_key_row(
        &self,
        input_id: &'static str,
        clear_id: &'static str,
        label: &'static str,
        description: &'static str,
        focus_target: FocusedInput,
        input_value: &str,
        configured: bool,
        placeholder: &'static str,
        on_clear: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = active_theme();
        let focused = self.focused_input == Some(focus_target);
        let preview = mask_api_key_preview(input_value, configured);
        let display: SharedString = if preview.is_empty() {
            if focused {
                "|".into()
            } else {
                placeholder.into()
            }
        } else if focused {
            format!("{preview}|").into()
        } else {
            preview.into()
        };
        let has_any = !input_value.is_empty() || configured;

        render_control_row(
            label,
            description,
            div()
                .flex()
                .items_center()
                .gap(sp(8.0))
                .child(
                    div()
                        .id(input_id)
                        .cursor_text()
                        .w(sp(210.0))
                        .h(sp(34.0))
                        .px(sp(10.0))
                        .rounded(px(RADIUS_POSTER))
                        .bg(rgba(theme.glass_trough_bg))
                        .border_1()
                        .border_color(if focused {
                            rgb(theme.accent)
                        } else {
                            rgba(theme.glass_border)
                        })
                        .flex()
                        .items_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.focus_settings_input(focus_target, cx);
                            }),
                        )
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .font(mono_font())
                                .text_size(sp(11.5))
                                .text_color(if !has_any && !focused {
                                    rgb(theme.text_muted)
                                } else {
                                    rgb(theme.text_primary)
                                })
                                .child(display),
                        ),
                )
                .when(has_any, |el| {
                    el.child(choice_pill(clear_id, "Clear", false, on_clear))
                }),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_debrid_key_row(
        &self,
        provider_idx: usize,
        label: &'static str,
        default_desc: &'static str,
        focus_target: FocusedInput,
        input_value: &str,
        configured: bool,
        placeholder: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = active_theme();
        let focused = self.focused_input == Some(focus_target);
        let preview = mask_api_key_preview(input_value, configured);
        let display: SharedString = if preview.is_empty() {
            if focused {
                "|".into()
            } else {
                placeholder.into()
            }
        } else if focused {
            format!("{preview}|").into()
        } else {
            preview.into()
        };
        let has_any = !input_value.is_empty() || configured;
        let desc = if self.local_debrid_provider_idx == provider_idx {
            if let Some(status) = &self.debrid_verify_status {
                status.clone()
            } else {
                default_desc.to_owned()
            }
        } else {
            default_desc.to_owned()
        };

        let input_id = SharedString::from(format!("input-debrid-key-{provider_idx}"));
        let clear_id = SharedString::from(format!("btn-clear-debrid-key-{provider_idx}"));
        let verify_id = SharedString::from(format!("btn-verify-debrid-key-{provider_idx}"));

        render_control_row(
            label,
            desc,
            div()
                .flex()
                .items_center()
                .gap(sp(8.0))
                .child(
                    div()
                        .id(input_id)
                        .cursor_text()
                        .w(sp(196.0))
                        .h(sp(34.0))
                        .px(sp(10.0))
                        .rounded(px(RADIUS_POSTER))
                        .bg(rgba(theme.glass_trough_bg))
                        .border_1()
                        .border_color(if focused {
                            rgb(theme.accent)
                        } else {
                            rgba(theme.glass_border)
                        })
                        .flex()
                        .items_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.focus_settings_input(focus_target, cx);
                            }),
                        )
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .font(mono_font())
                                .text_size(sp(11.5))
                                .text_color(if !has_any && !focused {
                                    rgb(theme.text_muted)
                                } else {
                                    rgb(theme.text_primary)
                                })
                                .child(display),
                        ),
                )
                .when(has_any, |el| {
                    el.child(choice_pill(
                        clear_id,
                        "Clear",
                        false,
                        cx.listener(move |this, _, _, cx| {
                            this.clear_local_debrid_key(provider_idx, cx);
                        }),
                    ))
                })
                .child(choice_pill(
                    verify_id,
                    "Verify",
                    self.local_debrid_provider_idx == provider_idx && has_any,
                    cx.listener(move |this, _, _, cx| {
                        this.verify_local_debrid_clicked(provider_idx, cx);
                    }),
                )),
        )
    }

    fn render_updates_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = active_theme();
        let ink_accent = accent_ink(&theme);
        let status = &self.update_status;
        let busy = matches!(
            status,
            UpdateStatus::Checking | UpdateStatus::Downloading { .. }
        );

        let action_row = render_control_row(
            "Check & install updates",
            "Releases are verified over HTTPS against SHA256SUMS.txt from Gaalix/ferrite.",
            choice_pill(
                "btn-check-updates",
                if busy {
                    "Working..."
                } else {
                    "Check for updates"
                },
                false,
                cx.listener(move |this, _, _, cx| {
                    if !busy {
                        this.trigger_update_check(true, cx);
                    }
                }),
            ),
        );

        let detail_el: Option<AnyElement> = match status {
            UpdateStatus::Idle => None,
            UpdateStatus::Checking => Some(
                div()
                    .font(ui_sans(FontWeight::NORMAL))
                    .text_size(sp(12.0))
                    .text_color(rgb(theme.text_secondary))
                    .child("Checking GitHub Releases for a newer version...")
                    .into_any_element(),
            ),
            UpdateStatus::UpToDate { checked_version } => Some(
                div()
                    .font(ui_sans(FontWeight::MEDIUM))
                    .text_size(sp(12.0))
                    .text_color(rgb(theme.status_watched))
                    .child(format!(
                        "Ferrite v{checked_version} is the latest release available (SHA-256 verified)."
                    ))
                    .into_any_element(),
            ),
            UpdateStatus::Available(info) => {
                let btn_label = info.target_kind.action_button_label();
                let asset_meta = format!(
                    "{} ({})",
                    info.target_asset.name,
                    format_bytes(info.target_asset.size_bytes)
                );
                Some(
                    div()
                        .p(sp(14.0))
                        .rounded(px(RADIUS_CARD))
                        .bg(rgba(theme.glass_trough_bg))
                        .border_1()
                        .border_color(rgba(theme.glass_border))
                        .flex()
                        .flex_col()
                        .gap(sp(10.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(sp(12.0))
                                .child(
                                    div()
                                        .font(ui_sans(FontWeight::SEMIBOLD))
                                        .text_size(sp(14.0))
                                        .text_color(rgb(theme.text_primary))
                                        .child(format!("{} (v{})", info.release_name, info.version)),
                                )
                                .child(neutral_chip("SHA-256 verified")),
                        )
                        .child(
                            div()
                                .text_size(sp(12.0))
                                .text_color(rgb(theme.text_secondary))
                                .child(info.notes_summary.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(sp(12.0))
                                .child(
                                    div()
                                        .font(mono_font())
                                        .text_size(sp(11.5))
                                        .text_color(rgb(theme.text_muted))
                                        .child(asset_meta),
                                )
                                .child(primary_accent_button(
                                    "btn-install-update",
                                    Some(IconName::RefreshCw),
                                    btn_label,
                                    cx.listener(|this, _, _, cx| {
                                        this.install_available_update(cx);
                                    }),
                                )),
                        )
                        .into_any_element(),
                )
            }
            UpdateStatus::Downloading { version } => Some(
                div()
                    .font(ui_sans(FontWeight::MEDIUM))
                    .text_size(sp(12.0))
                    .text_color(rgb(ink_accent))
                    .child(format!(
                        "Downloading and verifying SHA-256 checksum for Ferrite v{version}..."
                    ))
                    .into_any_element(),
            ),
            UpdateStatus::ReadyToRestart { version } => Some(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sp(14.0))
                    .child(
                        div()
                            .font(ui_sans(FontWeight::MEDIUM))
                            .text_size(sp(12.0))
                            .text_color(rgb(theme.status_watched))
                            .child(format!(
                                "Ferrite v{version} has been verified and installed. Restart to finish updating."
                            )),
                    )
                    .child(primary_accent_button(
                        "btn-restart-update",
                        Some(IconName::RefreshCw),
                        "Restart to apply",
                        cx.listener(|this, _, _, cx| {
                            this.restart_into_updated_binary(cx);
                        }),
                    ))
                    .into_any_element(),
            ),
            UpdateStatus::DebReady {
                version,
                deb_path,
                install_cmd,
            } => {
                let cmd_for_copy = install_cmd.clone();
                let deb_for_open = deb_path.clone();
                Some(
                    div()
                        .p(sp(14.0))
                        .rounded(px(RADIUS_CARD))
                        .bg(rgba(theme.glass_trough_bg))
                        .border_1()
                        .border_color(rgba(theme.glass_border))
                        .flex()
                        .flex_col()
                        .gap(sp(10.0))
                        .child(
                            div()
                                .text_size(sp(12.0))
                                .text_color(rgb(theme.status_watched))
                                .child(format!(
                                    "Verified Ferrite v{version} Debian package saved to user cache."
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap(sp(10.0))
                                .child(
                                    div()
                                        .font(mono_font())
                                        .text_size(sp(11.5))
                                        .text_color(rgb(theme.text_primary))
                                        .child(install_cmd.clone()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(sp(8.0))
                                        .child(choice_pill(
                                            "btn-copy-deb-cmd",
                                            "Copy apt command",
                                            false,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    cmd_for_copy.clone(),
                                                ));
                                                this.push_toast(
                                                    ToastKind::Success,
                                                    "Copied apt install command",
                                                    None,
                                                    cx,
                                                );
                                            }),
                                        ))
                                        .child(choice_pill(
                                            "btn-open-deb",
                                            "Open package",
                                            true,
                                            cx.listener(move |this, _, _, cx| {
                                                if let Err(err) =
                                                    updater::open_deb_package_with_system(
                                                        &deb_for_open,
                                                    )
                                                {
                                                    this.push_toast(
                                                        ToastKind::Warning,
                                                        "Package opener unavailable",
                                                        Some(err),
                                                        cx,
                                                    );
                                                }
                                            }),
                                        )),
                                ),
                        )
                        .into_any_element(),
                )
            }
            UpdateStatus::ApplyingWindowsInstaller { version } => Some(
                div()
                    .text_size(sp(12.0))
                    .text_color(rgb(ink_accent))
                    .child(format!("Launching Ferrite v{version} setup..."))
                    .into_any_element(),
            ),
            UpdateStatus::Failed(reason) => Some(
                div()
                    .text_size(sp(12.0))
                    .text_color(rgb(theme.accent_rose))
                    .child(reason.clone())
                    .into_any_element(),
            ),
        };

        div()
            .flex()
            .flex_col()
            .child(action_row)
            .when_some(detail_el, |el, detail| {
                el.child(div().pb(sp(14.0)).child(detail))
            })
            .into_any_element()
    }
}

fn settings_glass_card(theme: &ThemePalette) -> Div {
    div()
        .w_full()
        .rounded(px(RADIUS_SHEET))
        .bg(rgba(theme.glass_card_bg))
        .border_1()
        .border_color(rgba(theme.glass_border))
        .flex()
        .flex_col()
}

const fn advanced_section_hint(cat: SettingsCategory) -> &'static str {
    match cat {
        SettingsCategory::Appearance => "Player overlay timing",
        SettingsCategory::Playback => "GLSL shaders, debanding, Dolby Vision, dynamic HDR peak & pipeline diagnostics",
        SettingsCategory::SubtitlesAndAudio => "Hash auto-matching, default delay stepper & ASS scale override",
        SettingsCategory::StreamingAndDebrid => "Direct unrestrict, dead-swarm failover, UPnP, visibility & Prowlarr fallback",
        SettingsCategory::IntegrationsAndAddons => "Local Real-Debrid, TorBox, AllDebrid keys & local Prowlarr indexer",
        SettingsCategory::SystemAndStorage => "Local streaming architecture, loopback endpoint & build metadata",
    }
}

fn render_cache_gauge_bar(
    theme: &ThemePalette,
    label: &'static str,
    value: String,
    subtitle: impl Into<SharedString>,
    ratio: f32,
    bar_color: u32,
) -> impl IntoElement {
    let clamped = ratio.clamp(0.0, 1.0);
    let min_visible_frac = if clamped > 0.0 {
        clamped.max(0.02)
    } else {
        0.0
    };

    div()
        .flex()
        .flex_col()
        .gap(sp(5.0))
        .child(
            div()
                .flex()
                .items_baseline()
                .justify_between()
                .gap(sp(10.0))
                .child(
                    div()
                        .font(ui_sans(FontWeight::MEDIUM))
                        .text_size(sp(13.0))
                        .text_color(rgb(theme.text_primary))
                        .child(label),
                )
                .child(
                    div()
                        .font(mono_font())
                        .font_features(tabular_nums())
                        .text_size(sp(12.0))
                        .text_color(rgb(theme.text_primary))
                        .child(value),
                ),
        )
        .child(
            div()
                .w_full()
                .h(px(6.0))
                .rounded(px(RADIUS_PILL))
                .bg(rgba(theme.glass_trough_bg))
                .border_1()
                .border_color(rgba(theme.glass_border))
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(relative(min_visible_frac))
                        .rounded(px(RADIUS_PILL))
                        .bg(rgb(bar_color)),
                ),
        )
        .child(
            div()
                .font(ui_sans(FontWeight::NORMAL))
                .text_size(sp(11.5))
                .text_color(rgb(theme.text_muted))
                .truncate()
                .child(subtitle.into()),
        )
}

fn render_control_row(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl IntoElement,
) -> AnyElement {
    let theme = active_theme();
    div()
        .py(sp(14.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(sp(20.0))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(sp(2.5))
                .child(
                    div()
                        .font(ui_sans(FontWeight::MEDIUM))
                        .text_size(sp(13.5))
                        .text_color(rgb(theme.text_primary))
                        .child(title.into()),
                )
                .child(
                    div()
                        .font(ui_sans(FontWeight::NORMAL))
                        .text_size(sp(12.0))
                        .text_color(rgb(theme.text_secondary))
                        .child(description.into()),
                ),
        )
        .child(div().flex_shrink_0().child(control))
        .into_any_element()
}

fn render_segmented_control<L, F>(
    id_prefix: &'static str,
    options: impl IntoIterator<Item = (L, bool, F)>,
) -> impl IntoElement
where
    L: Into<SharedString>,
    F: Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
{
    let bar_id = SharedString::from(format!("{id_prefix}-bar"));
    glass_segmented_bar(bar_id).children(options.into_iter().enumerate().map(
        |(idx, (label, active, on_click))| {
            glass_segmented_item((id_prefix, idx), label, active, on_click)
        },
    ))
}

fn render_stepper(
    id_prefix: &'static str,
    value_label: impl Into<SharedString>,
    on_dec: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    on_reset: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    on_inc: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    glass_stepper(id_prefix, value_label, on_dec, on_reset, on_inc)
}

fn render_settings_dropdown<L, F>(
    id_prefix: &'static str,
    active_label: &'static str,
    open: bool,
    on_toggle: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    items: impl IntoIterator<Item = (L, bool, F)>,
) -> impl IntoElement
where
    L: Into<SharedString>,
    F: Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
{
    let trigger_id = SharedString::from(format!("{id_prefix}-trigger"));
    let menu_id = SharedString::from(format!("{id_prefix}-menu"));

    div()
        .relative()
        .child(dropdown_trigger(
            DropdownTriggerProps::new(trigger_id, active_label, open, on_toggle)
                .min_width_px(Some(196.0)),
        ))
        .when(open, |wrap| {
            wrap.child(
                deferred(
                    glass_dropdown_panel(menu_id, 232.0)
                        .absolute()
                        .top(sp(42.0))
                        .right_0()
                        .max_h(sp(280.0))
                        .overflow_y_scroll()
                        .children(items.into_iter().enumerate().map(
                            |(idx, (label, selected, on_select))| {
                                glass_dropdown_option(
                                    (id_prefix, idx),
                                    label,
                                    None,
                                    selected,
                                    on_select,
                                )
                            },
                        )),
                )
                .with_priority(2),
            )
        })
}

fn render_shortcut_grid_block(
    title: &'static str,
    entries: &[(&'static str, &'static str)],
) -> AnyElement {
    let theme = active_theme();
    div()
        .py(sp(14.0))
        .flex()
        .flex_col()
        .gap(sp(10.0))
        .child(
            div()
                .font(ui_sans(FontWeight::MEDIUM))
                .text_size(sp(13.5))
                .text_color(rgb(theme.text_primary))
                .child(title),
        )
        .child(
            div()
                .grid()
                .grid_cols(2)
                .gap(sp(8.0))
                .children(entries.iter().map(|&(keys, action)| {
                    div()
                        .px(sp(12.0))
                        .py(sp(8.0))
                        .rounded(px(RADIUS_POSTER))
                        .bg(rgba(theme.glass_trough_bg))
                        .border_1()
                        .border_color(rgba(theme.glass_border))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(sp(10.0))
                        .child(
                            div()
                                .font(ui_sans(FontWeight::NORMAL))
                                .text_size(sp(12.0))
                                .text_color(rgb(theme.text_secondary))
                                .child(action),
                        )
                        .child(keycap(keys))
                })),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_settings_category_has_essentials_and_advanced_rows_and_search_works() {
        for cat in SettingsCategory::ALL {
            let total = count_matching_settings_rows("", Some(cat));
            assert!(
                total > 0,
                "category {:?} should have at least one row",
                cat
            );
            let essentials = SETTINGS_ROWS
                .iter()
                .filter(|r| r.category == cat && !r.is_advanced)
                .count();
            let advanced = SETTINGS_ROWS
                .iter()
                .filter(|r| r.category == cat && r.is_advanced)
                .count();
            assert!(
                essentials > 0,
                "category {:?} should have at least one Essential row",
                cat
            );
            assert!(
                advanced > 0,
                "category {:?} should have at least one Advanced row",
                cat
            );
        }

        // Cross-category search assertions
        assert!(
            count_matching_settings_rows("prowlarr", Some(SettingsCategory::StreamingAndDebrid))
                > 0
        );
        assert!(
            count_matching_settings_rows(
                "prowlarr",
                Some(SettingsCategory::IntegrationsAndAddons)
            ) > 0
        );
        assert_eq!(
            count_matching_settings_rows("prowlarr", Some(SettingsCategory::Appearance)),
            0
        );
        assert!(
            count_matching_settings_rows("0600", Some(SettingsCategory::IntegrationsAndAddons))
                >= 5
        );
        assert_eq!(
            count_matching_settings_rows("spotlight", Some(SettingsCategory::Appearance)),
            1
        );
        assert_eq!(
            count_matching_settings_rows("tmdb", Some(SettingsCategory::IntegrationsAndAddons)),
            1
        );
        assert!(
            count_matching_settings_rows(
                "subtitle delay",
                Some(SettingsCategory::SubtitlesAndAudio)
            ) >= 1
        );
        assert!(
            count_matching_settings_rows("ambient", Some(SettingsCategory::Appearance)) >= 1
        );
        assert!(
            count_matching_settings_rows("cache", Some(SettingsCategory::SystemAndStorage)) >= 1
        );
        assert_eq!(
            count_matching_settings_rows("nonexistent_xyzzy_setting", None),
            0
        );
    }

    #[test]
    fn advanced_sections_auto_expand_when_search_matches_advanced_row() {
        let mut open = HashSet::new();
        assert!(!should_expand_advanced(
            SettingsCategory::Playback,
            &open,
            ""
        ));
        // Matches an Essential row ("upscaler") -> should NOT force-open Advanced
        assert!(!should_expand_advanced(
            SettingsCategory::Playback,
            &open,
            "spline36"
        ));
        // Matches an Advanced row ("glsl" / "debanding" / "dolby") -> auto-expands!
        assert!(should_expand_advanced(
            SettingsCategory::Playback,
            &open,
            "anime4k"
        ));
        assert!(should_expand_advanced(
            SettingsCategory::Appearance,
            &open,
            "osd"
        ));
        assert!(should_expand_advanced(
            SettingsCategory::SubtitlesAndAudio,
            &open,
            "ass"
        ));
        // Explicit user toggle stays open even with empty search
        open.insert(SettingsCategory::Playback);
        assert!(should_expand_advanced(
            SettingsCategory::Playback,
            &open,
            ""
        ));
    }

    #[test]
    fn subtitle_hex_and_gauge_ratio_helpers_handle_edge_cases() {
        assert_eq!(parse_subtitle_hex_rgb("#F5F1E8"), 0xF5F1E8);
        assert_eq!(parse_subtitle_hex_rgb("fde68a"), 0xFDE68A);
        assert_eq!(parse_subtitle_hex_rgb("invalid"), 0xF5F1E8);
        assert_eq!(subtitle_color_label("#f5f1e8"), "Warm Cream");
        assert_eq!(subtitle_color_label("#123456"), "#123456");

        assert_eq!(ratio_clamped(0, 0), 0.0);
        assert!((ratio_clamped(48, 96) - 0.5).abs() < 1e-6);
        assert_eq!(ratio_clamped(200, 96), 1.0);
    }
}
