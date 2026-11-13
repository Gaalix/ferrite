use gpui::{
    deferred, div, prelude::*, px, rgb, rgba, AnyElement, ClipboardItem, Context, FontWeight,
    MouseButton, SharedString,
};
use stremio_core_runtime::format_bytes;

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{
        mask_api_key_preview, settings_row_matches_query, DebridAccessMode, FocusedInput,
        SettingsCategory, SettingsDropdown, ShaderPresetChoice, SidebarTab, StorageModeChoice,
        ToastKind, ToneMappingChoice, UpscaleChoice,
    },
    theme::{
        editorial_serif, glass_capsule_shadow, keycap, mono_font, neutral_chip,
        semantic_status_pill, ui_sans, ACCENT, ACCENT_HOVER, ACCENT_ROSE, ACCENT_SOFT, BG_ELEVATED,
        BG_INPUT, BG_OBSIDIAN, BG_POPOVER_GLASS, BG_SURFACE, BG_SURFACE_3, BORDER_POSTER_HOVER,
        BORDER_SOFT, BORDER_STRONG, BORDER_SUBTLE, STATUS_WATCHED, STATUS_WATCHING, TEXT_MUTED,
        TEXT_PRIMARY, TEXT_SECONDARY,
    },
    updater::{self, UpdateStatus, CURRENT_APP_VERSION},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsRowId {
    CheckUpdatesOnStartup,
    ClipboardAddonDetect,
    OsdAutohideSecs,
    HardwareZeroCopyStatus,
    HdrToneMapping,
    GpuUpscaler,
    GlslShaderPreset,
    DarkSceneDebanding,
    DolbyVisionReshape,
    DynamicHdrPeak,
    PreventDisplaySleep,
    OsMediaControls,
    AutoPlayNextEpisode,
    PreferredAudioLang,
    PreferredSubtitleLang,
    AutoSelectSubtitles,
    AutoMatchNativeSubtitles,
    SubtitleStyleSummary,
    DefaultSubtitleDelay,
    LocalDebridProvider,
    LocalDebridDirectUnrestrict,
    LocalDebridCachedOnly,
    ProwlarrFallbackMode,
    DeadSwarmFailover,
    UpnpPortMapping,
    AddonDebridVisibility,
    RealDebridApiKey,
    TorBoxApiKey,
    AllDebridApiKey,
    ProwlarrUrl,
    ProwlarrApiKey,
    ProwlarrTimeoutAndTest,
    OpenSubtitlesApiKey,
    SubdlApiKey,
    OpenAddonsStudio,
    PieceCacheMode,
    ClearMetadataImageCache,
    ShortcutsGlobal,
    ShortcutsPlayerTransport,
    ShortcutsPlayerTracksAudio,
    SoftwareUpdateCheck,
    LocalStreamingArchitecture,
}

pub struct SettingsRowSpec {
    pub id: SettingsRowId,
    pub category: SettingsCategory,
    pub group: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub keywords: &'static [&'static str],
}

pub const SETTINGS_ROWS: &[SettingsRowSpec] = &[
    SettingsRowSpec {
        id: SettingsRowId::CheckUpdatesOnStartup,
        category: SettingsCategory::GeneralAndAppearance,
        group: "Startup & Clipboard",
        label: "Check for updates on startup",
        description: "Automatically check GitHub Releases for signed updates when Ferrite launches.",
        keywords: &["update", "startup", "github", "release", "auto"],
    },
    SettingsRowSpec {
        id: SettingsRowId::ClipboardAddonDetect,
        category: SettingsCategory::GeneralAndAppearance,
        group: "Startup & Clipboard",
        label: "Clipboard addon link detection",
        description: "Detect copied stremio:// or manifest.json addon URLs when opening Addons Studio.",
        keywords: &["clipboard", "addon", "manifest", "stremio://", "paste"],
    },
    SettingsRowSpec {
        id: SettingsRowId::OsdAutohideSecs,
        category: SettingsCategory::GeneralAndAppearance,
        group: "Interface",
        label: "Player OSD auto-hide",
        description: "Idle duration before player controls fade away during playback (2s, 3s, or 5s).",
        keywords: &["osd", "overlay", "autohide", "timeout", "controls", "fade"],
    },
    SettingsRowSpec {
        id: SettingsRowId::HardwareZeroCopyStatus,
        category: SettingsCategory::PlaybackAndVideo,
        group: "Video Pipeline",
        label: "Hardware zero-copy video output",
        description: "DirectComposition (Windows) / Wayland subsurface zero-copy presentation with libmpv hwdec.",
        keywords: &["hardware", "zero-copy", "directcomposition", "wayland", "hwdec", "gpu"],
    },
    SettingsRowSpec {
        id: SettingsRowId::HdrToneMapping,
        category: SettingsCategory::PlaybackAndVideo,
        group: "Video Pipeline",
        label: "HDR tone mapping",
        description: "Select the libmpv HDR-to-SDR tone-mapping curve for high dynamic range video.",
        keywords: &["hdr", "tone", "mapping", "spline", "bt2446a", "hable", "mobius"],
    },
    SettingsRowSpec {
        id: SettingsRowId::GpuUpscaler,
        category: SettingsCategory::PlaybackAndVideo,
        group: "Video Pipeline",
        label: "GPU upscaler",
        description: "Scaling filter used by libmpv when upscaling lower-resolution streams.",
        keywords: &["upscale", "bilinear", "spline36", "lanczos", "ewa", "sharp"],
    },
    SettingsRowSpec {
        id: SettingsRowId::GlslShaderPreset,
        category: SettingsCategory::PlaybackAndVideo,
        group: "Experimental Shaders & HDR",
        label: "GLSL shader preset",
        description: "Experimental libmpv shader preset (off by default).",
        keywords: &["glsl", "shader", "anime4k", "fsrcnnx", "krigbilateral", "experimental"],
    },
    SettingsRowSpec {
        id: SettingsRowId::DarkSceneDebanding,
        category: SettingsCategory::PlaybackAndVideo,
        group: "Experimental Shaders & HDR",
        label: "Dark-scene debanding",
        description: "Smooth 8-bit compression banding gradients in dark scenes.",
        keywords: &["deband", "banding", "gradient", "dark", "video"],
    },
    SettingsRowSpec {
        id: SettingsRowId::DolbyVisionReshape,
        category: SettingsCategory::PlaybackAndVideo,
        group: "Experimental Shaders & HDR",
        label: "Dolby Vision reshape",
        description: "Enable experimental Dolby Vision RPU metadata reshaping in libmpv.",
        keywords: &["dolby", "vision", "dv", "reshape", "hdr"],
    },
    SettingsRowSpec {
        id: SettingsRowId::DynamicHdrPeak,
        category: SettingsCategory::PlaybackAndVideo,
        group: "Experimental Shaders & HDR",
        label: "Dynamic HDR peak",
        description: "Compute per-frame HDR peak luminance for dynamic tone mapping.",
        keywords: &["hdr", "peak", "dynamic", "luminance", "compute"],
    },
    SettingsRowSpec {
        id: SettingsRowId::PreventDisplaySleep,
        category: SettingsCategory::PlaybackAndVideo,
        group: "System Integration",
        label: "Prevent display sleep while playing",
        description: "Keep the display awake and suppress screensaver lock during active playback.",
        keywords: &["sleep", "display", "screensaver", "wake", "inhibit", "idle"],
    },
    SettingsRowSpec {
        id: SettingsRowId::OsMediaControls,
        category: SettingsCategory::PlaybackAndVideo,
        group: "System Integration",
        label: "OS media controls (MPRIS2 / SMTC)",
        description: "Publish now-playing metadata and transport keys to Linux MPRIS2 and Windows SMTC.",
        keywords: &["mpris", "mpris2", "smtc", "media", "keys", "os", "transport"],
    },
    SettingsRowSpec {
        id: SettingsRowId::AutoPlayNextEpisode,
        category: SettingsCategory::PlaybackAndVideo,
        group: "Binge & Continuation",
        label: "Auto-play & pre-buffer next episode",
        description: "Pre-match and pre-buffer Episode N+1 45s before credits and auto-play at end of stream.",
        keywords: &["auto", "play", "next", "episode", "binge", "prebuffer", "credits"],
    },
    SettingsRowSpec {
        id: SettingsRowId::PreferredAudioLang,
        category: SettingsCategory::AudioAndSubtitles,
        group: "Language Preferences",
        label: "Preferred audio language",
        description: "Default ISO-639-2 audio stream preference when starting playback.",
        keywords: &["audio", "language", "eng", "jpn", "spa", "fre", "ger", "track"],
    },
    SettingsRowSpec {
        id: SettingsRowId::PreferredSubtitleLang,
        category: SettingsCategory::AudioAndSubtitles,
        group: "Language Preferences",
        label: "Preferred subtitle language",
        description: "Default subtitle language when auto-selecting tracks, or Off to disable.",
        keywords: &["subtitle", "subtitles", "language", "eng", "off", "spa", "fre", "ger"],
    },
    SettingsRowSpec {
        id: SettingsRowId::AutoSelectSubtitles,
        category: SettingsCategory::AudioAndSubtitles,
        group: "Subtitle Behavior",
        label: "Auto-select subtitles",
        description: "Automatically enable matching subtitle tracks when playback begins.",
        keywords: &["subtitle", "auto", "select", "captions", "cc"],
    },
    SettingsRowSpec {
        id: SettingsRowId::AutoMatchNativeSubtitles,
        category: SettingsCategory::AudioAndSubtitles,
        group: "Subtitle Behavior",
        label: "Auto-match native hash subtitles (OpenSubtitles + SubDL)",
        description: "Compute 64 KiB stream hash and auto-align matching subtitles when playback starts.",
        keywords: &["opensubtitles", "subdl", "hash", "opensubhash", "native", "subtitle", "sync"],
    },
    SettingsRowSpec {
        id: SettingsRowId::SubtitleStyleSummary,
        category: SettingsCategory::AudioAndSubtitles,
        group: "Subtitle Behavior",
        label: "Subtitle style",
        description: "Current Subtitle Studio typography, HDR-safe color, backdrop, and vertical position.",
        keywords: &["subtitle", "style", "font", "size", "color", "hdr", "cream", "pill", "reset"],
    },
    SettingsRowSpec {
        id: SettingsRowId::DefaultSubtitleDelay,
        category: SettingsCategory::AudioAndSubtitles,
        group: "Subtitle Behavior",
        label: "Default subtitle delay",
        description: "Baseline subtitle timing offset (-5000 ms to +5000 ms) applied to new sessions.",
        keywords: &["subtitle", "delay", "offset", "sync", "timing", "ms"],
    },
    SettingsRowSpec {
        id: SettingsRowId::LocalDebridProvider,
        category: SettingsCategory::DebridAndStreaming,
        group: "Local Rust Debrid",
        label: "Local Rust Debrid provider",
        description: "Resolve cached magnet hashes directly in Rust without external proxy addons.",
        keywords: &["debrid", "real-debrid", "torbox", "alldebrid", "provider", "rust"],
    },
    SettingsRowSpec {
        id: SettingsRowId::LocalDebridDirectUnrestrict,
        category: SettingsCategory::DebridAndStreaming,
        group: "Local Rust Debrid",
        label: "Direct ~60ms Rust unrestrict",
        description: "Keep a warm HTTP/2 connection pool for sub-100ms unrestrict and instant cache checks.",
        keywords: &["debrid", "unrestrict", "latency", "http2", "fast", "60ms"],
    },
    SettingsRowSpec {
        id: SettingsRowId::LocalDebridCachedOnly,
        category: SettingsCategory::DebridAndStreaming,
        group: "Local Rust Debrid",
        label: "Cached-only stream filter",
        description: "Hide uncached P2P torrents when a local Debrid provider is active.",
        keywords: &["cached", "filter", "debrid", "torrent", "p2p"],
    },
    SettingsRowSpec {
        id: SettingsRowId::ProwlarrFallbackMode,
        category: SettingsCategory::DebridAndStreaming,
        group: "Indexer & Failover",
        label: "Prowlarr fallback mode",
        description: "Query your local Prowlarr or Torznab endpoint on addon outage or always merge.",
        keywords: &["prowlarr", "torznab", "fallback", "outage", "indexer", "merge"],
    },
    SettingsRowSpec {
        id: SettingsRowId::DeadSwarmFailover,
        category: SettingsCategory::DebridAndStreaming,
        group: "Indexer & Failover",
        label: "Dead-swarm auto-failover",
        description: "Step to the next ranked stream if a swarm stalls before first byte.",
        keywords: &["dead", "swarm", "failover", "timeout", "stalled", "torrent"],
    },
    SettingsRowSpec {
        id: SettingsRowId::UpnpPortMapping,
        category: SettingsCategory::DebridAndStreaming,
        group: "Network",
        label: "UPnP / NAT-PMP port mapping",
        description: "Automatically map inbound BitTorrent listener ports on your local router.",
        keywords: &["upnp", "nat-pmp", "port", "router", "network", "bittorrent"],
    },
    SettingsRowSpec {
        id: SettingsRowId::AddonDebridVisibility,
        category: SettingsCategory::DebridAndStreaming,
        group: "Network",
        label: "Addon Debrid visibility override",
        description: "Auto-detect Debrid from installed addon URLs or override Cached stream visibility.",
        keywords: &["addon", "debrid", "auto-detect", "visibility", "cached"],
    },
    SettingsRowSpec {
        id: SettingsRowId::RealDebridApiKey,
        category: SettingsCategory::IntegrationsAndKeys,
        group: "Debrid API Keys",
        label: "Real-Debrid API key",
        description: "Private Real-Debrid API token stored locally with 0600 permissions.",
        keywords: &["real-debrid", "rd", "api", "key", "token", "0600"],
    },
    SettingsRowSpec {
        id: SettingsRowId::TorBoxApiKey,
        category: SettingsCategory::IntegrationsAndKeys,
        group: "Debrid API Keys",
        label: "TorBox API key",
        description: "Private TorBox API token stored locally with 0600 permissions.",
        keywords: &["torbox", "tb", "api", "key", "token", "0600"],
    },
    SettingsRowSpec {
        id: SettingsRowId::AllDebridApiKey,
        category: SettingsCategory::IntegrationsAndKeys,
        group: "Debrid API Keys",
        label: "AllDebrid API key",
        description: "Private AllDebrid API key stored locally with 0600 permissions.",
        keywords: &["alldebrid", "ad", "api", "key", "token", "0600"],
    },
    SettingsRowSpec {
        id: SettingsRowId::ProwlarrUrl,
        category: SettingsCategory::IntegrationsAndKeys,
        group: "Local Prowlarr / Torznab",
        label: "Local Prowlarr / Torznab URL",
        description: "Base URL of your local Prowlarr or Torznab indexer (e.g. http://127.0.0.1:9696).",
        keywords: &["prowlarr", "torznab", "url", "endpoint", "localhost", "indexer"],
    },
    SettingsRowSpec {
        id: SettingsRowId::ProwlarrApiKey,
        category: SettingsCategory::IntegrationsAndKeys,
        group: "Local Prowlarr / Torznab",
        label: "Prowlarr / Torznab API key",
        description: "X-Api-Key or Torznab apikey sent strictly to your local indexer endpoint.",
        keywords: &["prowlarr", "torznab", "api", "key", "x-api-key", "0600"],
    },
    SettingsRowSpec {
        id: SettingsRowId::ProwlarrTimeoutAndTest,
        category: SettingsCategory::IntegrationsAndKeys,
        group: "Local Prowlarr / Torznab",
        label: "Prowlarr timeout & connection test",
        description: "Configure local indexer request timeout (1000-15000 ms) and test connectivity.",
        keywords: &["prowlarr", "timeout", "test", "indexer", "torznab", "latency"],
    },
    SettingsRowSpec {
        id: SettingsRowId::OpenSubtitlesApiKey,
        category: SettingsCategory::IntegrationsAndKeys,
        group: "Native Subtitle API Keys",
        label: "OpenSubtitles API key",
        description: "Optional personal OpenSubtitles REST API key for higher search/download quotas.",
        keywords: &["opensubtitles", "subtitles", "api", "key", "token", "0600"],
    },
    SettingsRowSpec {
        id: SettingsRowId::SubdlApiKey,
        category: SettingsCategory::IntegrationsAndKeys,
        group: "Native Subtitle API Keys",
        label: "SubDL API key",
        description: "Optional SubDL API key for expanded hash and release subtitle matching.",
        keywords: &["subdl", "subtitles", "api", "key", "token", "0600"],
    },
    SettingsRowSpec {
        id: SettingsRowId::OpenAddonsStudio,
        category: SettingsCategory::IntegrationsAndKeys,
        group: "Stremio Addons",
        label: "Stremio Addons Studio",
        description: "Reorder execution priority, soft-mute Debrid sources, or browse curated addons.",
        keywords: &["addons", "studio", "stremio", "manifest", "mute", "priority"],
    },
    SettingsRowSpec {
        id: SettingsRowId::PieceCacheMode,
        category: SettingsCategory::StorageAndCache,
        group: "Torrent Piece Cache",
        label: "Torrent piece cache mode",
        description: "Choose warm-resume Disk LRU caching (10/20/50 GB) or zero-SSD-write RAM-Only (2/4 GB).",
        keywords: &["storage", "cache", "disk", "lru", "ram", "budget", "gb"],
    },
    SettingsRowSpec {
        id: SettingsRowId::ClearMetadataImageCache,
        category: SettingsCategory::StorageAndCache,
        group: "Cleanup",
        label: "Clear metadata & image cache",
        description: "Flush cached poster artwork and HTTP addon responses to reclaim disk and memory.",
        keywords: &["clear", "cache", "metadata", "images", "poster", "flush", "cleanup"],
    },
    SettingsRowSpec {
        id: SettingsRowId::ShortcutsGlobal,
        category: SettingsCategory::Shortcuts,
        group: "Global Navigation",
        label: "Global navigation & palette shortcuts",
        description: "Command palette (Ctrl/Cmd+K, /), tab switching (Alt+1..6), fullscreen (F11), quit (Ctrl/Cmd+Q).",
        keywords: &["shortcut", "keyboard", "command palette", "ctrl+k", "alt", "f11", "quit", "escape"],
    },
    SettingsRowSpec {
        id: SettingsRowId::ShortcutsPlayerTransport,
        category: SettingsCategory::Shortcuts,
        group: "Video Player",
        label: "Player transport & seeking shortcuts",
        description: "Play/Pause (Space), 2x hold (Space), Seek 5s/30s (Left/Right), Frame step (, / .), Exit (Esc).",
        keywords: &["shortcut", "player", "space", "seek", "frame", "2x", "pause", "escape"],
    },
    SettingsRowSpec {
        id: SettingsRowId::ShortcutsPlayerTracksAudio,
        category: SettingsCategory::Shortcuts,
        group: "Video Player",
        label: "Player audio, subtitles & stats shortcuts",
        description: "Volume (Up/Down), Mute (M), Fullscreen (F/F11), Audio (A), Subtitles (S), Delay (Z/X), Stats (I).",
        keywords: &["shortcut", "volume", "mute", "audio", "subtitle", "delay", "stats", "fullscreen"],
    },
    SettingsRowSpec {
        id: SettingsRowId::SoftwareUpdateCheck,
        category: SettingsCategory::UpdatesAndAbout,
        group: "Releases",
        label: "Check & install updates",
        description: "Verify GitHub Releases over HTTPS against SHA256SUMS.txt from Gaalix/ferrite.",
        keywords: &["update", "version", "sha256", "checksum", "github", "restart", "install"],
    },
    SettingsRowSpec {
        id: SettingsRowId::LocalStreamingArchitecture,
        category: SettingsCategory::UpdatesAndAbout,
        group: "Architecture & Privacy",
        label: "Local streaming & privacy architecture",
        description: "Local Rust engine, 0600 credential isolation, and zero telemetry.",
        keywords: &["about", "privacy", "local", "telemetry", "0600", "architecture", "ferrite"],
    },
];

pub fn count_matching_settings_rows(query: &str, category: Option<SettingsCategory>) -> usize {
    SETTINGS_ROWS
        .iter()
        .filter(|row| category.is_none_or(|c| row.category == c))
        .filter(|row| settings_row_matches_query(query, row.label, row.description, row.keywords))
        .count()
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

impl AppShell {
    pub fn render_settings_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.settings_search_query.trim().to_owned();
        let is_searching = !query.is_empty();
        let total_matches = if is_searching {
            count_matching_settings_rows(&query, None)
        } else {
            0
        };

        div()
            .id("settings-two-pane-root")
            .size_full()
            .flex()
            .overflow_hidden()
            .bg(rgb(BG_OBSIDIAN))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.settings_open_dropdown.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .child(self.render_settings_sidebar_rail(&query, is_searching, cx))
            .child(self.render_settings_detail_pane(&query, is_searching, total_matches, cx))
    }

    fn render_settings_sidebar_rail(
        &self,
        query: &str,
        is_searching: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .w(px(248.0))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(BG_SURFACE))
            .border_r_1()
            .border_color(rgba(BORDER_SOFT))
            .px(px(14.0))
            .py(px(22.0))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(
                div()
                    .px(px(8.0))
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .font(editorial_serif())
                            .text_size(px(30.0))
                            .text_color(rgb(TEXT_PRIMARY))
                            .child("Settings"),
                    )
                    .child(
                        div()
                            .font(mono_font())
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
                            .child(format!("v{CURRENT_APP_VERSION}")),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .children(SettingsCategory::ALL.into_iter().map(|cat| {
                        let active = !is_searching && self.active_settings_category == cat;
                        let match_count = if is_searching {
                            count_matching_settings_rows(query, Some(cat))
                        } else {
                            0
                        };
                        let icon_color = if active {
                            rgb(ACCENT)
                        } else {
                            rgb(TEXT_SECONDARY)
                        };
                        let text_color = if active {
                            rgb(TEXT_PRIMARY)
                        } else if is_searching && match_count == 0 {
                            rgb(TEXT_MUTED)
                        } else {
                            rgb(TEXT_SECONDARY)
                        };

                        div()
                            .id(SharedString::from(format!("settings-cat-{}", cat.id())))
                            .cursor_pointer()
                            .h(px(38.0))
                            .px(px(10.0))
                            .rounded(px(8.0))
                            .bg(if active {
                                rgba(ACCENT_SOFT)
                            } else {
                                rgba(0x00000000)
                            })
                            .border_1()
                            .border_color(if active {
                                rgba(0xff5e3a44)
                            } else {
                                rgba(0x00000000)
                            })
                            .hover(|s| {
                                if active {
                                    s
                                } else {
                                    s.bg(rgb(BG_ELEVATED))
                                }
                            })
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
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
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .w(px(3.0))
                                            .h(px(16.0))
                                            .rounded(px(999.0))
                                            .bg(if active {
                                                rgb(ACCENT)
                                            } else {
                                                rgba(0x00000000)
                                            }),
                                    )
                                    .child(svg_icon(cat.icon_name(), 15.0, icon_color))
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
                                            .text_size(px(13.0))
                                            .text_color(text_color)
                                            .child(cat.label()),
                                    ),
                            )
                            .when(is_searching && match_count > 0, |el| {
                                el.child(
                                    div()
                                        .px(px(7.0))
                                        .py(px(1.5))
                                        .rounded(px(999.0))
                                        .bg(rgba(ACCENT_SOFT))
                                        .font(mono_font())
                                        .text_size(px(11.0))
                                        .text_color(rgb(ACCENT))
                                        .child(format!("{match_count}")),
                                )
                            })
                    })),
            )
            .child(
                div()
                    .pt(px(12.0))
                    .border_t_1()
                    .border_color(rgba(BORDER_SOFT))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(semantic_status_pill(
                        IconName::Check,
                        "Engine online",
                        STATUS_WATCHED,
                        0x4ade804d,
                        0x4ade801c,
                    )),
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
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(self.render_settings_search_header(is_searching, total_matches, cx))
            .child(
                div()
                    .id("settings-detail-scroll")
                    .track_scroll(&self.settings_scroll_handle)
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(860.0))
                            .px(px(36.0))
                            .pt(px(26.0))
                            .pb(px(72.0))
                            .flex()
                            .flex_col()
                            .gap(px(22.0))
                            .child(if is_searching {
                                self.render_settings_search_results(query, total_matches, cx)
                            } else {
                                self.render_active_settings_category(cx)
                            }),
                    ),
            )
    }

    fn render_settings_search_header(
        &self,
        is_searching: bool,
        total_matches: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let focused = self.focused_input == Some(FocusedInput::SettingsSearch);
        let query_display: SharedString = if self.settings_search_query.is_empty() {
            if focused {
                "|".into()
            } else {
                "Search all settings, shortcuts, Debrid, Prowlarr...".into()
            }
        } else if focused {
            format!("{}|", self.settings_search_query).into()
        } else {
            self.settings_search_query.clone().into()
        };

        let summary_text = if total_matches == 1 {
            "1 match".to_owned()
        } else {
            format!("{total_matches} matches")
        };

        div()
            .w_full()
            .px(px(36.0))
            .py(px(16.0))
            .bg(rgb(BG_OBSIDIAN))
            .border_b_1()
            .border_color(rgba(BORDER_SOFT))
            .child(
                div()
                    .w_full()
                    .max_w(px(788.0))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .id("input-settings-search")
                            .cursor_text()
                            .flex_1()
                            .min_w(px(0.0))
                            .h(px(38.0))
                            .px(px(12.0))
                            .rounded(px(8.0))
                            .bg(rgb(BG_SURFACE))
                            .border_1()
                            .border_color(if focused {
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
                                    cx.stop_propagation();
                                    this.focus_settings_input(FocusedInput::SettingsSearch, cx);
                                }),
                            )
                            .child(svg_icon(
                                IconName::Search,
                                14.5,
                                if focused {
                                    rgb(ACCENT)
                                } else {
                                    rgb(TEXT_MUTED)
                                },
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_size(px(13.0))
                                    .text_color(
                                        if self.settings_search_query.is_empty() && !focused {
                                            rgb(TEXT_MUTED)
                                        } else {
                                            rgb(TEXT_PRIMARY)
                                        },
                                    )
                                    .child(query_display),
                            )
                            .when(!self.settings_search_query.is_empty(), |el| {
                                el.child(
                                    div()
                                        .id("btn-clear-settings-search")
                                        .cursor_pointer()
                                        .w(px(22.0))
                                        .h(px(22.0))
                                        .rounded(px(999.0))
                                        .bg(rgb(BG_ELEVATED))
                                        .hover(|s| s.bg(rgb(BG_SURFACE_3)))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.clear_settings_search(cx);
                                            }),
                                        )
                                        .child(svg_icon(IconName::X, 12.0, rgb(TEXT_SECONDARY))),
                                )
                            }),
                    )
                    .when(is_searching, |el| {
                        el.child(
                            div()
                                .px(px(10.0))
                                .py(px(6.0))
                                .rounded(px(999.0))
                                .bg(rgb(BG_SURFACE))
                                .border_1()
                                .border_color(rgba(BORDER_SUBTLE))
                                .font(mono_font())
                                .text_xs()
                                .text_color(if total_matches == 0 {
                                    rgb(TEXT_MUTED)
                                } else {
                                    rgb(TEXT_SECONDARY)
                                })
                                .child(summary_text),
                        )
                    }),
            )
    }

    fn render_settings_search_results(
        &self,
        query: &str,
        total_matches: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if total_matches == 0 {
            return div()
                .py(px(56.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .w(px(44.0))
                        .h(px(44.0))
                        .rounded(px(999.0))
                        .bg(rgb(BG_SURFACE))
                        .border_1()
                        .border_color(rgba(BORDER_SUBTLE))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(svg_icon(IconName::Search, 18.0, rgb(TEXT_MUTED))),
                )
                .child(
                    div()
                        .font(ui_sans(FontWeight::SEMIBOLD))
                        .text_size(px(15.0))
                        .text_color(rgb(TEXT_PRIMARY))
                        .child(format!("No settings match \"{query}\"")),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child("Try searching for hdr, debrid, prowlarr, delay, osd, or shortcut."),
                )
                .child(render_choice_pill(
                    "btn-empty-clear-search",
                    "Clear search",
                    false,
                    cx.listener(|this, _, _, cx| {
                        this.clear_settings_search(cx);
                    }),
                ))
                .into_any_element();
        }

        let mut sections: Vec<AnyElement> = Vec::new();
        for cat in SettingsCategory::ALL {
            let matching: Vec<&SettingsRowSpec> = SETTINGS_ROWS
                .iter()
                .filter(|r| r.category == cat)
                .filter(|r| settings_row_matches_query(query, r.label, r.description, r.keywords))
                .collect();
            if matching.is_empty() {
                continue;
            }
            sections.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .pb(px(6.0))
                            .border_b_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(svg_icon(cat.icon_name(), 14.0, rgb(ACCENT)))
                                    .child(
                                        div()
                                            .font(ui_sans(FontWeight::SEMIBOLD))
                                            .text_size(px(13.5))
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child(cat.label()),
                                    ),
                            )
                            .child(
                                div()
                                    .font(mono_font())
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(format!("{} matching", matching.len())),
                            ),
                    )
                    .children(matching.into_iter().map(|spec| self.render_settings_row(spec.id, cx)))
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(26.0))
            .children(sections)
            .into_any_element()
    }

    fn render_active_settings_category(&self, cx: &mut Context<Self>) -> AnyElement {
        let cat = self.active_settings_category;
        let header = div()
            .pb(px(14.0))
            .border_b_1()
            .border_color(rgba(BORDER_SOFT))
            .flex()
            .items_end()
            .justify_between()
            .gap(px(16.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .font(editorial_serif())
                            .text_size(px(36.0))
                            .text_color(rgb(TEXT_PRIMARY))
                            .child(cat.label()),
                    )
                    .child(
                        div()
                            .text_size(px(13.5))
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(cat.subtitle()),
                    ),
            );

        let mut body_children: Vec<AnyElement> = vec![header.into_any_element()];

        match cat {
            SettingsCategory::IntegrationsAndKeys => {
                body_children.push(self.render_integrations_security_banner().into_any_element());
            }
            SettingsCategory::StorageAndCache => {
                body_children.push(self.render_storage_telemetry_strip().into_any_element());
            }
            SettingsCategory::UpdatesAndAbout => {
                body_children.push(self.render_about_version_strip().into_any_element());
            }
            _ => {}
        }

        let cat_rows: Vec<&SettingsRowSpec> = SETTINGS_ROWS
            .iter()
            .filter(|r| r.category == cat)
            .collect();

        let mut last_group = "";
        for spec in cat_rows {
            if spec.group != last_group {
                last_group = spec.group;
                body_children.push(
                    div()
                        .pt(px(8.0))
                        .pb(px(2.0))
                        .font(ui_sans(FontWeight::SEMIBOLD))
                        .text_size(px(11.5))
                        .text_color(rgb(TEXT_MUTED))
                        .child(spec.group.to_uppercase())
                        .into_any_element(),
                );
            }
            body_children.push(self.render_settings_row(spec.id, cx));
        }

        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .children(body_children)
            .into_any_element()
    }

    fn render_integrations_security_banner(&self) -> impl IntoElement {
        div()
            .my(px(4.0))
            .px(px(14.0))
            .py(px(12.0))
            .rounded(px(8.0))
            .bg(rgb(BG_SURFACE))
            .border_1()
            .border_color(rgba(0x4ade8040))
            .flex()
            .items_start()
            .gap(px(12.0))
            .child(
                div()
                    .pt(px(2.0))
                    .child(svg_icon(IconName::ShieldCheck, 16.0, rgb(STATUS_WATCHED))),
            )
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
                            .text_size(px(13.0))
                            .text_color(rgb(TEXT_PRIMARY))
                            .child("Local-only 0600 credential storage"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(
                                "API keys are written with 0600 permissions to ferrite/private_integrations.json, zeroized in memory on drop, and never synced to Stremio profiles or addons.",
                            ),
                    ),
            )
    }

    fn render_storage_telemetry_strip(&self) -> impl IntoElement {
        let engine_snap = self.engine.engine_stats();
        let cache_used = self.engine.cache_usage_bytes();
        let engine_url = self.engine.base_url().to_string();
        let prefs = &self.preferences;

        div()
            .my(px(4.0))
            .px(px(16.0))
            .py(px(12.0))
            .rounded(px(8.0))
            .bg(rgb(BG_SURFACE))
            .border_1()
            .border_color(rgba(BORDER_SUBTLE))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(14.0))
            .child(render_telemetry_cell(
                "Active swarms",
                format!("{}", engine_snap.active_torrents),
                format!("{} peers", engine_snap.connected_peers),
            ))
            .child(div().w(px(1.0)).h(px(30.0)).bg(rgba(BORDER_SOFT)))
            .child(render_telemetry_cell(
                "Throughput",
                format!(
                    "{:.2} MB/s",
                    engine_snap.download_speed_bytes / (1024.0 * 1024.0)
                ),
                format!("{} rx", format_bytes(engine_snap.downloaded_bytes)),
            ))
            .child(div().w(px(1.0)).h(px(30.0)).bg(rgba(BORDER_SOFT)))
            .child(render_telemetry_cell(
                "Cache usage",
                format_bytes(cache_used),
                prefs.storage_mode.short_badge(),
            ))
            .child(div().w(px(1.0)).h(px(30.0)).bg(rgba(BORDER_SOFT)))
            .child(render_telemetry_cell(
                "Local endpoint",
                engine_url,
                "Loopback HTTP".to_owned(),
            ))
    }

    fn render_about_version_strip(&self) -> impl IntoElement {
        div()
            .my(px(4.0))
            .px(px(16.0))
            .py(px(14.0))
            .rounded(px(8.0))
            .bg(rgb(BG_SURFACE))
            .border_1()
            .border_color(rgba(BORDER_SUBTLE))
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
                            .font(editorial_serif())
                            .text_size(px(26.0))
                            .text_color(rgb(TEXT_PRIMARY))
                            .child(format!("Ferrite v{CURRENT_APP_VERSION}")),
                    )
                    .child(neutral_chip(format!("Build v{CURRENT_APP_VERSION}"))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(semantic_status_pill(
                        IconName::ShieldCheck,
                        "SHA-256 verified",
                        STATUS_WATCHED,
                        0x4ade804d,
                        0x4ade801c,
                    ))
                    .child(neutral_chip("GPUI + libmpv")),
            )
    }

    fn render_settings_row(&self, row_id: SettingsRowId, cx: &mut Context<Self>) -> AnyElement {
        let prefs = &self.preferences;
        let exp = prefs.experimental_video;

        match row_id {
            SettingsRowId::CheckUpdatesOnStartup => render_control_row(
                "Check for updates on startup",
                "Automatically check GitHub Releases for signed updates when Ferrite launches.",
                render_movietracker_switch(
                    "toggle-check-updates-startup",
                    prefs.check_updates_on_startup,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_check_updates_on_startup(cx);
                    }),
                ),
            ),
            SettingsRowId::ClipboardAddonDetect => render_control_row(
                "Clipboard addon link detection",
                "Detect copied stremio:// or manifest.json addon URLs when opening Addons Studio.",
                render_movietracker_switch(
                    "toggle-clipboard-addon-detect",
                    prefs.clipboard_addon_detect,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_clipboard_addon_detect(cx);
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
                let (pill_label, fg, border, bg) = if self.mpv.is_some() {
                    ("Zero-copy active", STATUS_WATCHED, 0x4ade804d, 0x4ade801c)
                } else {
                    ("Headless / standby", STATUS_WATCHING, 0x60a5fa4d, 0x60a5fa1c)
                };
                render_control_row(
                    "Hardware zero-copy video output",
                    "DirectComposition (Windows) / Wayland subsurface zero-copy presentation with libmpv hwdec.",
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
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
            SettingsRowId::GpuUpscaler => render_control_row(
                "GPU upscaler",
                "Scaling filter used by libmpv when upscaling lower-resolution streams.",
                render_segmented_control(
                    "seg-upscaler",
                    UpscaleChoice::ALL.into_iter().map(|choice| {
                        let short = match choice {
                            UpscaleChoice::Bilinear => "Bilinear",
                            UpscaleChoice::Spline36 => "Spline36",
                            UpscaleChoice::Lanczos => "Lanczos3",
                            UpscaleChoice::EwaLanczos => "EWA Sharp",
                        };
                        (
                            short,
                            prefs.upscale_mode == choice,
                            cx.listener(move |this, _, _, cx| {
                                this.set_upscale_choice(choice, cx);
                            }),
                        )
                    }),
                ),
            ),
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
                render_movietracker_switch(
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
                render_movietracker_switch(
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
                render_movietracker_switch(
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
            SettingsRowId::PreventDisplaySleep => render_control_row(
                "Prevent display sleep while playing",
                "Keep the display awake and suppress screensaver lock during active playback.",
                render_movietracker_switch(
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
                render_movietracker_switch(
                    "toggle-os-media-controls",
                    prefs.enable_os_media_controls,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_os_media_controls(cx);
                    }),
                ),
            ),
            SettingsRowId::AutoPlayNextEpisode => render_control_row(
                "Auto-play & pre-buffer next episode",
                "Pre-match and pre-buffer Episode N+1 45s before credits and auto-play at end of stream.",
                render_movietracker_switch(
                    "toggle-auto-play-next-episode",
                    prefs.auto_play_next_episode,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_auto_play_next_episode(cx);
                    }),
                ),
            ),
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
                render_movietracker_switch(
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
                render_movietracker_switch(
                    "toggle-auto-match-native-subs",
                    prefs.auto_match_native_subtitles,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_auto_match_native_subtitles(cx);
                    }),
                ),
            ),
            SettingsRowId::SubtitleStyleSummary => {
                let color_upper = prefs.subtitle_color_hex.to_ascii_uppercase();
                let color_name = match color_upper.as_str() {
                    "#F5F1E8" => "Warm Cream #F5F1E8",
                    "#E5E7EB" => "Soft Silver #E5E7EB",
                    "#FDE68A" => "Cinema Amber #FDE68A",
                    "#BAE6FD" => "Cool Ice #BAE6FD",
                    "#FFFFFF" => "Pure White #FFFFFF",
                    other => other,
                };
                let backdrop = if prefs.subtitle_background_pill {
                    "Translucent Pill"
                } else {
                    "Drop Shadow"
                };
                let summary = format!(
                    "{color_name} · {backdrop} · {}px · {}%",
                    prefs.subtitle_font_size, prefs.subtitle_vertical_pos
                );
                render_control_row(
                    "Subtitle style",
                    "Current Subtitle Studio typography, HDR-safe color, backdrop, and vertical position.",
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(neutral_chip(summary))
                        .child(render_choice_pill(
                            "btn-reset-sub-style",
                            "Reset default",
                            false,
                            cx.listener(|this, _, _, cx| {
                                this.reset_subtitle_style_prefs(cx);
                            }),
                        )),
                )
            }
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
                        delay <= -5000,
                        delay >= 5000,
                        cx.listener(|this, _, _, cx| {
                            this.step_default_subtitle_delay(-100, cx);
                        }),
                        cx.listener(|this, _, _, cx| {
                            this.step_default_subtitle_delay(100, cx);
                        }),
                    ),
                )
            }
            SettingsRowId::LocalDebridProvider => render_control_row(
                "Local Rust Debrid provider",
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
                        .gap(px(10.0))
                        .child(neutral_chip(latency_badge))
                        .child(render_movietracker_switch(
                            "toggle-local-debrid-direct",
                            self.local_debrid_direct_unrestrict,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_local_debrid_direct_unrestrict(cx);
                            }),
                        )),
                )
            }
            SettingsRowId::LocalDebridCachedOnly => render_control_row(
                "Cached-only stream filter",
                "Hide uncached P2P torrents when a local Debrid provider is active.",
                render_movietracker_switch(
                    "toggle-local-debrid-cached-only",
                    self.local_debrid_cached_only,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_local_debrid_cached_only(cx);
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
            SettingsRowId::DeadSwarmFailover => render_control_row(
                "Dead-swarm auto-failover",
                "Step to the next ranked stream if a swarm stalls before first byte.",
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
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
                    .child(render_movietracker_switch(
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
                render_movietracker_switch(
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
                        .w(px(280.0))
                        .h(px(34.0))
                        .px(px(10.0))
                        .rounded(px(6.0))
                        .bg(rgb(BG_INPUT))
                        .border_1()
                        .border_color(if focused {
                            rgba(BORDER_POSTER_HOVER)
                        } else {
                            rgba(BORDER_SUBTLE)
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
                                .text_xs()
                                .text_color(if self.prowlarr_url_input.is_empty() && !focused {
                                    rgb(TEXT_MUTED)
                                } else {
                                    rgb(TEXT_PRIMARY)
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
                        .gap(px(8.0))
                        .child(render_stepper(
                            "stepper-prowlarr-timeout",
                            format!("{} ms", self.prowlarr_timeout_ms),
                            self.prowlarr_timeout_ms <= 1000,
                            self.prowlarr_timeout_ms >= 15000,
                            cx.listener(|this, _, _, cx| {
                                this.step_prowlarr_timeout(-500, cx);
                            }),
                            cx.listener(|this, _, _, cx| {
                                this.step_prowlarr_timeout(500, cx);
                            }),
                        ))
                        .child(render_choice_pill(
                            "btn-test-prowlarr",
                            "Test Indexer",
                            true,
                            cx.listener(|this, _, _, cx| {
                                this.test_prowlarr_clicked(cx);
                            }),
                        )),
                )
            }
            SettingsRowId::OpenAddonsStudio => {
                let active_count = self.core.with_model(|m| m.ctx.profile.addons.len());
                let muted_count = self.core.list_muted_addons().len();
                let cache_stats = self.core.addon_cache_stats();
                render_control_row(
                    "Stremio Addons Studio",
                    format!(
                        "{active_count} active · {muted_count} muted · {} HTTP cache hits",
                        cache_stats.hits
                    ),
                    div()
                        .id("btn-open-addons-studio")
                        .cursor_pointer()
                        .h(px(34.0))
                        .px(px(14.0))
                        .rounded(px(6.0))
                        .bg(rgb(ACCENT))
                        .hover(|s| s.bg(rgb(ACCENT_HOVER)))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .font(ui_sans(FontWeight::SEMIBOLD))
                        .text_size(px(12.5))
                        .text_color(rgb(BG_OBSIDIAN))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.navigate_tab(SidebarTab::Addons, cx);
                            }),
                        )
                        .child(svg_icon(IconName::Layers, 13.5, rgb(BG_OBSIDIAN)))
                        .child("Open Addons Studio"),
                )
            }
            SettingsRowId::PieceCacheMode => render_control_row(
                "Torrent piece cache mode",
                "Choose warm-resume Disk LRU caching or zero-SSD-write RAM ring buffering.",
                render_segmented_control(
                    "seg-storage-mode",
                    [
                        (StorageModeChoice::DiskLru { budget_gb: 10 }, "Disk 10 GB"),
                        (StorageModeChoice::DiskLru { budget_gb: 20 }, "Disk 20 GB"),
                        (StorageModeChoice::DiskLru { budget_gb: 50 }, "Disk 50 GB"),
                        (StorageModeChoice::RamOnly { budget_gb: 2 }, "RAM 2 GB"),
                        (StorageModeChoice::RamOnly { budget_gb: 4 }, "RAM 4 GB"),
                    ]
                    .into_iter()
                    .map(|(choice, label)| {
                        (
                            label,
                            prefs.storage_mode == choice,
                            cx.listener(move |this, _, _, cx| {
                                this.set_storage_mode_preference(choice, cx);
                            }),
                        )
                    }),
                ),
            ),
            SettingsRowId::ClearMetadataImageCache => render_control_row(
                "Clear metadata & image cache",
                "Flush cached poster artwork and HTTP addon responses to reclaim disk and memory.",
                div()
                    .id("btn-clear-metadata-cache")
                    .cursor_pointer()
                    .h(px(34.0))
                    .px(px(14.0))
                    .rounded(px(6.0))
                    .bg(rgb(BG_ELEVATED))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .hover(|s| s.border_color(rgba(BORDER_STRONG)))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .font(ui_sans(FontWeight::MEDIUM))
                    .text_size(px(12.5))
                    .text_color(rgb(TEXT_PRIMARY))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.clear_metadata_and_image_cache(cx);
                        }),
                    )
                    .child(svg_icon(IconName::Trash2, 13.5, rgb(TEXT_SECONDARY)))
                    .child("Clear cache"),
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
            SettingsRowId::SoftwareUpdateCheck => self.render_updates_row(cx),
            SettingsRowId::LocalStreamingArchitecture => render_control_row(
                "Local streaming & privacy architecture",
                "Torrents and Debrid unrestrict resolve locally in Rust; 0600 keys never leave your machine and zero telemetry is collected.",
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(neutral_chip("0600 Local Keys"))
                    .child(neutral_chip("Zero Telemetry")),
            ),
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
                .gap(px(8.0))
                .child(
                    div()
                        .id(input_id)
                        .cursor_text()
                        .w(px(210.0))
                        .h(px(34.0))
                        .px(px(10.0))
                        .rounded(px(6.0))
                        .bg(rgb(BG_INPUT))
                        .border_1()
                        .border_color(if focused {
                            rgba(BORDER_POSTER_HOVER)
                        } else {
                            rgba(BORDER_SUBTLE)
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
                                .text_xs()
                                .text_color(if !has_any && !focused {
                                    rgb(TEXT_MUTED)
                                } else {
                                    rgb(TEXT_PRIMARY)
                                })
                                .child(display),
                        ),
                )
                .when(has_any, |el| {
                    el.child(render_choice_pill(clear_id, "Clear", false, on_clear))
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
                .gap(px(8.0))
                .child(
                    div()
                        .id(input_id)
                        .cursor_text()
                        .w(px(196.0))
                        .h(px(34.0))
                        .px(px(10.0))
                        .rounded(px(6.0))
                        .bg(rgb(BG_INPUT))
                        .border_1()
                        .border_color(if focused {
                            rgba(BORDER_POSTER_HOVER)
                        } else {
                            rgba(BORDER_SUBTLE)
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
                                .text_xs()
                                .text_color(if !has_any && !focused {
                                    rgb(TEXT_MUTED)
                                } else {
                                    rgb(TEXT_PRIMARY)
                                })
                                .child(display),
                        ),
                )
                .when(has_any, |el| {
                    el.child(render_choice_pill(
                        clear_id,
                        "Clear",
                        false,
                        cx.listener(move |this, _, _, cx| {
                            this.clear_local_debrid_key(provider_idx, cx);
                        }),
                    ))
                })
                .child(render_choice_pill(
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
        let status = &self.update_status;
        let busy = matches!(
            status,
            UpdateStatus::Checking | UpdateStatus::Downloading { .. }
        );

        let action_row = render_control_row(
            "Check & install updates",
            "Releases are verified over HTTPS against SHA256SUMS.txt from Gaalix/ferrite.",
            render_choice_pill(
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
                    .text_xs()
                    .text_color(rgb(TEXT_SECONDARY))
                    .child("Checking GitHub Releases for a newer version...")
                    .into_any_element(),
            ),
            UpdateStatus::UpToDate { checked_version } => Some(
                div()
                    .text_xs()
                    .text_color(rgb(STATUS_WATCHED))
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
                        .p(px(14.0))
                        .rounded(px(8.0))
                        .bg(rgb(BG_SURFACE))
                        .border_1()
                        .border_color(rgba(BORDER_SUBTLE))
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .font(ui_sans(FontWeight::SEMIBOLD))
                                        .text_size(px(14.0))
                                        .text_color(rgb(TEXT_PRIMARY))
                                        .child(format!("{} (v{})", info.release_name, info.version)),
                                )
                                .child(neutral_chip("SHA-256 verified")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(info.notes_summary.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_MUTED))
                                        .child(asset_meta),
                                )
                                .child(
                                    div()
                                        .id("btn-install-update")
                                        .cursor_pointer()
                                        .h(px(34.0))
                                        .px(px(14.0))
                                        .rounded(px(6.0))
                                        .bg(rgb(ACCENT))
                                        .hover(|s| s.bg(rgb(ACCENT_HOVER)))
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .font(ui_sans(FontWeight::SEMIBOLD))
                                        .text_size(px(12.5))
                                        .text_color(rgb(BG_OBSIDIAN))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.install_available_update(cx);
                                            }),
                                        )
                                        .child(svg_icon(
                                            IconName::RefreshCw,
                                            13.0,
                                            rgb(BG_OBSIDIAN),
                                        ))
                                        .child(btn_label),
                                ),
                        )
                        .into_any_element(),
                )
            }
            UpdateStatus::Downloading { version } => Some(
                div()
                    .text_xs()
                    .text_color(rgb(ACCENT))
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
                    .gap(px(14.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(STATUS_WATCHED))
                            .child(format!(
                                "Ferrite v{version} has been verified and installed. Restart to finish updating."
                            )),
                    )
                    .child(
                        div()
                            .id("btn-restart-update")
                            .cursor_pointer()
                            .h(px(34.0))
                            .px(px(14.0))
                            .rounded(px(6.0))
                            .bg(rgb(ACCENT))
                            .hover(|s| s.bg(rgb(ACCENT_HOVER)))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .font(ui_sans(FontWeight::SEMIBOLD))
                            .text_size(px(12.5))
                            .text_color(rgb(BG_OBSIDIAN))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.restart_into_updated_binary(cx);
                                }),
                            )
                            .child("Restart to apply"),
                    )
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
                        .p(px(14.0))
                        .rounded(px(8.0))
                        .bg(rgb(BG_SURFACE))
                        .border_1()
                        .border_color(rgba(BORDER_SUBTLE))
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(STATUS_WATCHED))
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
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .font(mono_font())
                                        .text_xs()
                                        .text_color(rgb(TEXT_PRIMARY))
                                        .child(install_cmd.clone()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .child(render_choice_pill(
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
                                        .child(render_choice_pill(
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
                    .text_xs()
                    .text_color(rgb(ACCENT))
                    .child(format!("Launching Ferrite v{version} setup..."))
                    .into_any_element(),
            ),
            UpdateStatus::Failed(reason) => Some(
                div()
                    .text_xs()
                    .text_color(rgb(ACCENT_ROSE))
                    .child(reason.clone())
                    .into_any_element(),
            ),
        };

        div()
            .flex()
            .flex_col()
            .child(action_row)
            .when_some(detail_el, |el, detail| {
                el.child(div().pb(px(12.0)).child(detail))
            })
            .into_any_element()
    }
}

fn render_control_row(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: impl IntoElement,
) -> AnyElement {
    div()
        .py(px(14.0))
        .border_b_1()
        .border_color(rgba(BORDER_SOFT))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(20.0))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .font(ui_sans(FontWeight::MEDIUM))
                        .text_size(px(14.0))
                        .text_color(rgb(TEXT_PRIMARY))
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(description.into()),
                ),
        )
        .child(div().flex_shrink_0().child(control))
        .into_any_element()
}

fn render_telemetry_cell(label: &'static str, value: String, sub: String) -> impl IntoElement {
    div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child(label))
        .child(
            div()
                .font(mono_font())
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(14.0))
                .text_color(rgb(TEXT_PRIMARY))
                .truncate()
                .child(value),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(TEXT_SECONDARY))
                .truncate()
                .child(sub),
        )
}

fn render_segmented_control<L, F>(
    id_prefix: &'static str,
    options: impl IntoIterator<Item = (L, bool, F)>,
) -> impl IntoElement
where
    L: Into<SharedString>,
    F: Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
{
    div()
        .p(px(3.0))
        .rounded(px(8.0))
        .bg(rgb(BG_SURFACE))
        .border_1()
        .border_color(rgba(BORDER_SUBTLE))
        .flex()
        .items_center()
        .gap(px(2.0))
        .children(options.into_iter().enumerate().map(|(idx, (label, active, on_click))| {
            div()
                .id((id_prefix, idx))
                .cursor_pointer()
                .h(px(28.0))
                .px(px(10.0))
                .rounded(px(6.0))
                .bg(if active {
                    rgba(ACCENT_SOFT)
                } else {
                    rgba(0x00000000)
                })
                .border_1()
                .border_color(if active {
                    rgba(0xff5e3a55)
                } else {
                    rgba(0x00000000)
                })
                .hover(|s| {
                    if active {
                        s
                    } else {
                        s.bg(rgb(BG_ELEVATED))
                    }
                })
                .flex()
                .items_center()
                .font(ui_sans(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                }))
                .text_size(px(12.0))
                .text_color(if active {
                    rgb(ACCENT)
                } else {
                    rgb(TEXT_SECONDARY)
                })
                .on_mouse_down(MouseButton::Left, on_click)
                .child(label.into())
        }))
}

fn render_stepper(
    id_prefix: &'static str,
    value_label: impl Into<SharedString>,
    at_min: bool,
    at_max: bool,
    on_dec: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    on_inc: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let dec_id = SharedString::from(format!("{id_prefix}-dec"));
    let inc_id = SharedString::from(format!("{id_prefix}-inc"));

    div()
        .h(px(34.0))
        .px(px(4.0))
        .rounded(px(8.0))
        .bg(rgb(BG_SURFACE))
        .border_1()
        .border_color(rgba(BORDER_SUBTLE))
        .flex()
        .items_center()
        .gap(px(4.0))
        .child(
            div()
                .id(dec_id)
                .cursor_pointer()
                .w(px(26.0))
                .h(px(26.0))
                .rounded(px(6.0))
                .bg(rgb(BG_ELEVATED))
                .hover(|s| s.bg(rgb(BG_SURFACE_3)))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, on_dec)
                .child(svg_icon(
                    IconName::Minus,
                    12.0,
                    if at_min {
                        rgb(TEXT_MUTED)
                    } else {
                        rgb(TEXT_PRIMARY)
                    },
                )),
        )
        .child(
            div()
                .min_w(px(74.0))
                .px(px(6.0))
                .flex()
                .items_center()
                .justify_center()
                .font(mono_font())
                .text_xs()
                .text_color(rgb(TEXT_PRIMARY))
                .child(value_label.into()),
        )
        .child(
            div()
                .id(inc_id)
                .cursor_pointer()
                .w(px(26.0))
                .h(px(26.0))
                .rounded(px(6.0))
                .bg(rgb(BG_ELEVATED))
                .hover(|s| s.bg(rgb(BG_SURFACE_3)))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, on_inc)
                .child(svg_icon(
                    IconName::Plus,
                    12.0,
                    if at_max {
                        rgb(TEXT_MUTED)
                    } else {
                        rgb(TEXT_PRIMARY)
                    },
                )),
        )
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
        .child(
            div()
                .id(trigger_id)
                .cursor_pointer()
                .h(px(34.0))
                .min_w(px(190.0))
                .px(px(12.0))
                .rounded(px(8.0))
                .bg(rgb(BG_SURFACE))
                .border_1()
                .border_color(if open {
                    rgb(ACCENT)
                } else {
                    rgba(BORDER_SUBTLE)
                })
                .hover(|s| s.border_color(rgba(BORDER_STRONG)))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .on_mouse_down(MouseButton::Left, on_toggle)
                .child(
                    div()
                        .text_xs()
                        .font(ui_sans(FontWeight::MEDIUM))
                        .text_color(rgb(TEXT_PRIMARY))
                        .child(active_label),
                )
                .child(svg_icon(IconName::ChevronDown, 13.0, rgb(TEXT_SECONDARY))),
        )
        .when(open, |wrap| {
            wrap.child(
                deferred(
                    div()
                        .id(menu_id)
                        .occlude()
                        .absolute()
                        .top(px(38.0))
                        .right_0()
                        .w(px(230.0))
                        .max_h(px(280.0))
                        .overflow_y_scroll()
                        .p(px(5.0))
                        .rounded(px(8.0))
                        .bg(rgba(BG_POPOVER_GLASS))
                        .border_1()
                        .border_color(rgba(BORDER_SUBTLE))
                        .shadow(glass_capsule_shadow())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .children(items.into_iter().enumerate().map(
                                    |(idx, (label, selected, on_select))| {
                                        div()
                                            .id((id_prefix, idx))
                                            .cursor_pointer()
                                            .px(px(10.0))
                                            .py(px(7.0))
                                            .rounded(px(6.0))
                                            .bg(if selected {
                                                rgba(ACCENT_SOFT)
                                            } else {
                                                rgba(0x00000000)
                                            })
                                            .hover(|s| {
                                                if selected {
                                                    s
                                                } else {
                                                    s.bg(rgb(BG_ELEVATED))
                                                }
                                            })
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap(px(8.0))
                                            .on_mouse_down(MouseButton::Left, on_select)
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font(ui_sans(if selected {
                                                        FontWeight::SEMIBOLD
                                                    } else {
                                                        FontWeight::NORMAL
                                                    }))
                                                    .text_color(if selected {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgb(TEXT_PRIMARY)
                                                    })
                                                    .child(label.into()),
                                            )
                                            .when(selected, |el| {
                                                el.child(svg_icon(
                                                    IconName::Check,
                                                    12.0,
                                                    rgb(ACCENT),
                                                ))
                                            })
                                    },
                                )),
                        ),
                )
                .with_priority(2),
            )
        })
}

fn render_shortcut_grid_block(
    title: &'static str,
    entries: &[(&'static str, &'static str)],
) -> AnyElement {
    div()
        .py(px(14.0))
        .border_b_1()
        .border_color(rgba(BORDER_SOFT))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .font(ui_sans(FontWeight::MEDIUM))
                .text_size(px(13.5))
                .text_color(rgb(TEXT_PRIMARY))
                .child(title),
        )
        .child(
            div()
                .grid()
                .grid_cols(2)
                .gap(px(8.0))
                .children(entries.iter().map(|&(keys, action)| {
                    div()
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(6.0))
                        .bg(rgb(BG_SURFACE))
                        .border_1()
                        .border_color(rgba(BORDER_SOFT))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(action),
                        )
                        .child(keycap(keys))
                })),
        )
        .into_any_element()
}

pub(crate) fn render_choice_pill(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .cursor_pointer()
        .h(px(32.0))
        .px(px(13.0))
        .rounded(px(999.0))
        .bg(if active {
            rgba(ACCENT_SOFT)
        } else {
            rgb(BG_ELEVATED)
        })
        .border_1()
        .border_color(if active {
            rgb(ACCENT)
        } else {
            rgba(BORDER_SUBTLE)
        })
        .hover(|s| {
            s.border_color(if active {
                rgb(ACCENT)
            } else {
                rgba(BORDER_STRONG)
            })
        })
        .flex()
        .items_center()
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(px(12.5))
        .text_color(if active {
            rgb(ACCENT)
        } else {
            rgb(TEXT_SECONDARY)
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.into())
}

pub(crate) fn render_movietracker_switch(
    id: impl Into<gpui::ElementId>,
    enabled: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .cursor_pointer()
        .w(px(40.0))
        .h(px(22.0))
        .p(px(3.0))
        .rounded(px(999.0))
        .bg(if enabled {
            rgb(ACCENT)
        } else {
            rgb(BG_SURFACE_3)
        })
        .flex()
        .items_center()
        .when(enabled, |el| el.justify_end())
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            div()
                .w(px(16.0))
                .h(px(16.0))
                .rounded(px(999.0))
                .bg(if enabled {
                    rgb(BG_OBSIDIAN)
                } else {
                    rgb(TEXT_PRIMARY)
                }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_settings_category_has_searchable_rows_and_cross_category_search_works() {
        for cat in SettingsCategory::ALL {
            assert!(
                count_matching_settings_rows("", Some(cat)) > 0,
                "category {:?} should have at least one row",
                cat
            );
        }

        assert!(count_matching_settings_rows("prowlarr", Some(SettingsCategory::DebridAndStreaming)) > 0);
        assert!(count_matching_settings_rows("prowlarr", Some(SettingsCategory::IntegrationsAndKeys)) > 0);
        assert_eq!(
            count_matching_settings_rows("prowlarr", Some(SettingsCategory::GeneralAndAppearance)),
            0
        );
        assert!(count_matching_settings_rows("0600", Some(SettingsCategory::IntegrationsAndKeys)) >= 3);
        assert!(count_matching_settings_rows("subtitle delay", Some(SettingsCategory::AudioAndSubtitles)) >= 1);
        assert_eq!(count_matching_settings_rows("nonexistent_xyzzy_setting", None), 0);
    }
}
