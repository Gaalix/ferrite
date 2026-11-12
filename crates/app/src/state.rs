use std::{
    collections::{BTreeSet, HashSet},
    hash::{Hash, Hasher},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{px, ClipboardItem, KeyDownEvent, ScrollHandle};
use gpui_mpv::{
    DebandConfig, HdrBackendOptions, MpvController, PlayerSnapshot, ShaderPreset,
    ToneMappingPreset, UpscalePreset,
};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use stremio_core_runtime::{
    parse_stream_with_addon, resolve_addon_for_request_base,
    stremio_core::{
        constants::{META_RESOURCE_NAME, STREAM_RESOURCE_NAME, SUBTITLES_RESOURCE_NAME},
        models::{
            common::{Loadable, ResourceLoadable},
            library_with_filters::{
                LibraryRequest, LibraryRequestPage, Selected as LibraryWithFiltersSelected,
                Sort as LibrarySort,
            },
            player::Selected as PlayerSelected,
        },
        types::{
            addon::{Descriptor, ResourcePath, ResourceRequest},
            library::LibraryItem,
            resource::{MetaItem, MetaItemPreview, Stream, StreamSource},
        },
    },
    AddonCategory, CoreController, DesktopModel, ParsedStream, Resolution, StreamFilter,
};
use stremio_stream_engine::{
    StorageMode, StreamEngine, StreamEngineConfig, StreamEngineError, StreamEngineHandle,
    DEFAULT_BIND_PORT,
};
use url::Url;

use crate::{icons::IconName, poster_cache::PosterCache};

pub const STREAM_PAGE_SIZE: usize = 30;

pub const LIBRARY_PAGE_SIZE: usize = 48;

#[inline]
pub fn trim_process_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        unsafe {
            let _ = malloc_trim(0);
        }
    }
}

#[inline]
pub fn scroll_near_bottom(handle: &ScrollHandle, extra_down_px: f32, threshold_px: f32) -> bool {
    let max_y = handle.max_offset().y / px(1.0);
    if !max_y.is_finite() || max_y <= 0.0 {
        return false;
    }
    let scrolled_y = (-handle.offset().y) / px(1.0);
    let projected_y = (scrolled_y + extra_down_px.max(0.0)).max(0.0);
    projected_y + threshold_px >= max_y
}

pub fn default_library_selected() -> LibraryWithFiltersSelected {
    LibraryWithFiltersSelected {
        request: LibraryRequest {
            r#type: None,
            sort: LibrarySort::default(),
            page: LibraryRequestPage::default(),
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SidebarTab {
    Home,
    Discover,
    Library,
    Addons,
    Settings,
    Account,
}

impl SidebarTab {
    pub const ALL: [Self; 6] = [
        Self::Home,
        Self::Discover,
        Self::Library,
        Self::Addons,
        Self::Settings,
        Self::Account,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Discover => "Discover",
            Self::Library => "Library",
            Self::Addons => "Addons",
            Self::Settings => "Settings",
            Self::Account => "Account",
        }
    }

    pub const fn icon_name(self) -> IconName {
        match self {
            Self::Home => IconName::Home,
            Self::Discover => IconName::Compass,
            Self::Library => IconName::Bookmark,
            Self::Addons => IconName::Layers,
            Self::Settings => IconName::Settings,
            Self::Account => IconName::User,
        }
    }

    pub const fn shortcut_label(self) -> &'static str {
        match self {
            Self::Home => "1",
            Self::Discover => "2",
            Self::Library => "3",
            Self::Addons => "4",
            Self::Settings => "5",
            Self::Account => "6",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppRoute {
    Tab(SidebarTab),
    MetaDetails {
        type_name: String,
        id: String,
        video_id: Option<String>,
    },
    Player,
}

impl AppRoute {
    pub fn active_sidebar_tab(&self) -> Option<SidebarTab> {
        match self {
            Self::Tab(tab) => Some(*tab),
            Self::MetaDetails { .. } | Self::Player => None,
        }
    }

    pub fn is_player(&self) -> bool {
        matches!(self, Self::Player)
    }

    pub fn transition_id(&self) -> String {
        match self {
            Self::Tab(tab) => format!("route-tab-{}", tab.label()),
            Self::MetaDetails { id, video_id, .. } => {
                format!("route-meta-{id}-{}", video_id.as_deref().unwrap_or("root"))
            }
            Self::Player => "route-player".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum StorageModeChoice {
    DiskLru { budget_gb: u64 },
    RamOnly { budget_gb: u64 },
}

impl Default for StorageModeChoice {
    fn default() -> Self {
        Self::DiskLru { budget_gb: 20 }
    }
}

impl StorageModeChoice {
    pub fn budget_bytes(self) -> u64 {
        match self {
            Self::DiskLru { budget_gb } | Self::RamOnly { budget_gb } => {
                budget_gb.saturating_mul(1024 * 1024 * 1024)
            }
        }
    }

    pub fn short_badge(self) -> String {
        match self {
            Self::DiskLru { budget_gb } => format!("Disk LRU ({budget_gb} GB)"),
            Self::RamOnly { budget_gb } => format!("RAM ({budget_gb} GB)"),
        }
    }

    pub fn to_engine_mode(self) -> StorageMode {
        match self {
            Self::DiskLru { .. } => StorageMode::disk_lru(self.budget_bytes()),
            Self::RamOnly { .. } => StorageMode::ram_only(self.budget_bytes()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToneMappingChoice {
    #[default]
    Auto,
    Spline,
    Bt2446a,
    St2094_40,
    Hable,
    Mobius,
    Reinhard,
    Clip,
}

impl ToneMappingChoice {
    pub const ALL: [Self; 8] = [
        Self::Auto,
        Self::Spline,
        Self::Bt2446a,
        Self::St2094_40,
        Self::Hable,
        Self::Mobius,
        Self::Reinhard,
        Self::Clip,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Spline => "Spline (BT.2408)",
            Self::Bt2446a => "BT.2446a",
            Self::St2094_40 => "ST2094-40 (HDR10+)",
            Self::Hable => "Hable Filmic",
            Self::Mobius => "Mobius",
            Self::Reinhard => "Reinhard",
            Self::Clip => "Hard Clip",
        }
    }

    pub const fn to_mpv(self) -> ToneMappingPreset {
        match self {
            Self::Auto => ToneMappingPreset::Auto,
            Self::Spline => ToneMappingPreset::Spline,
            Self::Bt2446a => ToneMappingPreset::Bt2446a,
            Self::St2094_40 => ToneMappingPreset::St2094_40,
            Self::Hable => ToneMappingPreset::Hable,
            Self::Mobius => ToneMappingPreset::Mobius,
            Self::Reinhard => ToneMappingPreset::Reinhard,
            Self::Clip => ToneMappingPreset::Clip,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpscaleChoice {
    #[default]
    Bilinear,
    Spline36,
    Lanczos,
    EwaLanczos,
}

impl UpscaleChoice {
    pub const ALL: [Self; 4] = [
        Self::Bilinear,
        Self::Spline36,
        Self::Lanczos,
        Self::EwaLanczos,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Bilinear => "Bilinear (Fast)",
            Self::Spline36 => "Spline36",
            Self::Lanczos => "Lanczos3",
            Self::EwaLanczos => "EWA LanczosSharp",
        }
    }

    pub const fn to_mpv(self) -> UpscalePreset {
        match self {
            Self::Bilinear => UpscalePreset::Bilinear,
            Self::Spline36 => UpscalePreset::Spline36,
            Self::Lanczos => UpscalePreset::Lanczos,
            Self::EwaLanczos => UpscalePreset::EwaLanczos,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShaderPresetChoice {
    #[default]
    Off,
    KrigBilateral,
    FsrcnnxLine,
    Anime4kFast,
    Anime4kHq,
}

impl ShaderPresetChoice {
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::KrigBilateral,
        Self::FsrcnnxLine,
        Self::Anime4kFast,
        Self::Anime4kHq,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off (Native)",
            Self::KrigBilateral => "KrigBilateral Chroma",
            Self::FsrcnnxLine => "FSRCNNX Line Sharpen",
            Self::Anime4kFast => "Anime4K Fast",
            Self::Anime4kHq => "Anime4K HQ",
        }
    }

    pub const fn to_mpv(self) -> ShaderPreset {
        match self {
            Self::Off => ShaderPreset::Off,
            Self::KrigBilateral => ShaderPreset::KrigBilateral,
            Self::FsrcnnxLine => ShaderPreset::FsrcnnxLine,
            Self::Anime4kFast => ShaderPreset::Anime4kFast,
            Self::Anime4kHq => ShaderPreset::Anime4kHq,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExperimentalVideoPrefs {
    pub shader_preset: ShaderPresetChoice,
    pub deband_enabled: bool,
    pub dolby_vision_reshape: bool,
    pub target_colorspace_hint: bool,
    pub hdr_compute_peak: bool,
}

impl ExperimentalVideoPrefs {
    pub const fn to_deband_config(self) -> DebandConfig {
        if self.deband_enabled {
            DebandConfig::enabled_balanced()
        } else {
            DebandConfig {
                enabled: false,
                iterations: 1,
                threshold: 48,
                range: 16,
                grain: 32,
            }
        }
    }

    pub const fn to_hdr_backend_options(self) -> HdrBackendOptions {
        HdrBackendOptions {
            dolby_vision_reshape: self.dolby_vision_reshape,
            target_colorspace_hint: self.target_colorspace_hint,
            hdr_compute_peak: self.hdr_compute_peak,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebridAccessMode {
    #[default]
    AutoDetect,
    Subscribed,
    NoneHideCached,
}

impl DebridAccessMode {
    pub const ALL: [Self; 3] = [Self::AutoDetect, Self::Subscribed, Self::NoneHideCached];

    pub const fn label(self) -> &'static str {
        match self {
            Self::AutoDetect => "Auto-detect",
            Self::Subscribed => "Subscribed (show cached)",
            Self::NoneHideCached => "No Debrid (hide cached)",
        }
    }

    pub const fn resolve_access(self, auto_detected_debrid: bool) -> bool {
        match self {
            Self::AutoDetect => auto_detected_debrid,
            Self::Subscribed => true,
            Self::NoneHideCached => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppPreferences {
    pub storage_mode: StorageModeChoice,
    pub enable_upnp: bool,
    pub dead_swarm_failover: bool,
    pub dead_swarm_timeout_secs: u64,
    pub debrid_access_mode: DebridAccessMode,
    pub sidebar_collapsed: bool,
    pub tone_mapping: ToneMappingChoice,
    pub upscale_mode: UpscaleChoice,
    pub experimental_video: ExperimentalVideoPrefs,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            storage_mode: StorageModeChoice::default(),
            enable_upnp: true,
            dead_swarm_failover: true,
            dead_swarm_timeout_secs: 10,
            debrid_access_mode: DebridAccessMode::AutoDetect,
            sidebar_collapsed: false,
            tone_mapping: ToneMappingChoice::Auto,
            upscale_mode: UpscaleChoice::Bilinear,
            experimental_video: ExperimentalVideoPrefs::default(),
        }
    }
}

impl AppPreferences {
    pub fn default_path() -> PathBuf {
        Self::config_path_for_app_dir("ferrite")
    }

    pub fn legacy_default_path() -> PathBuf {
        Self::config_path_for_app_dir("stremio-gpui")
    }

    fn config_path_for_app_dir(app_dir: &str) -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            let trimmed = xdg.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed)
                    .join(app_dir)
                    .join("preferences.json");
            }
        }
        if let Some(cfg) = dirs::config_dir() {
            return cfg.join(app_dir).join("preferences.json");
        }
        std::env::temp_dir().join(app_dir).join("preferences.json")
    }

    pub fn load_or_default() -> Self {
        let primary = Self::default_path();
        if let Some(loaded) = std::fs::read_to_string(&primary)
            .ok()
            .and_then(|raw| serde_json::from_str::<Self>(&raw).ok())
        {
            return loaded;
        }

        let legacy = Self::legacy_default_path();
        if let Some(migrated) = std::fs::read_to_string(&legacy)
            .ok()
            .and_then(|raw| serde_json::from_str::<Self>(&raw).ok())
        {
            let _ = migrated.save_to(&primary);
            return migrated;
        }

        Self::default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::default_path();
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }
        let encoded = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&tmp)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(encoded.as_bytes())?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

pub async fn start_stream_engine_with_fallback(
    prefs: &AppPreferences,
) -> Result<StreamEngineHandle, StreamEngineError> {
    let mut primary_cfg = build_engine_config_from_prefs(prefs);
    primary_cfg.bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_BIND_PORT);

    match StreamEngine::start(primary_cfg).await {
        Ok(handle) => Ok(handle),
        Err(err) => {
            tracing::warn!(
                target: "ferrite::boot",
                "127.0.0.1:{DEFAULT_BIND_PORT} unavailable ({err}); falling back to ephemeral loopback port"
            );
            let mut fallback_cfg = build_engine_config_from_prefs(prefs);
            fallback_cfg.bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
            StreamEngine::start(fallback_cfg).await
        }
    }
}

pub fn build_engine_config_from_prefs(prefs: &AppPreferences) -> StreamEngineConfig {
    let mut cfg = StreamEngineConfig {
        enable_upnp: prefs.enable_upnp,
        storage_mode: prefs.storage_mode.to_engine_mode(),
        cache_size: Some(prefs.storage_mode.budget_bytes() as f64),
        ..StreamEngineConfig::default()
    };
    let timeout_dur = Duration::from_secs(prefs.dead_swarm_timeout_secs.max(4));
    if prefs.dead_swarm_failover {
        cfg.metadata_timeout = timeout_dur;
        cfg.first_byte_timeout = timeout_dur;
    }
    cfg
}

pub async fn switch_engine_storage_mode_on_tokio(
    tokio_handle: &tokio::runtime::Handle,
    engine: StreamEngineHandle,
    mode: StorageMode,
) -> anyhow::Result<()> {
    let res = tokio_handle
        .spawn(async move { engine.set_storage_mode(mode).await })
        .await
        .map_err(|join_err| anyhow::anyhow!("storage mode switch task cancelled: {join_err}"))?;
    res.map_err(|err| anyhow::anyhow!("{err}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StreamResolutionChoice {
    Uhd4k,
    Fhd1080p,
    Hd720p,
}

impl StreamResolutionChoice {
    pub const ALL: [Self; 3] = [Self::Uhd4k, Self::Fhd1080p, Self::Hd720p];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Uhd4k => "4K",
            Self::Fhd1080p => "1080p",
            Self::Hd720p => "720p",
        }
    }

    pub const fn to_core_resolution(self) -> Resolution {
        match self {
            Self::Uhd4k => Resolution::Uhd4k,
            Self::Fhd1080p => Resolution::Fhd1080p,
            Self::Hd720p => Resolution::Hd720p,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamFilterState {
    pub selected_addons: BTreeSet<String>,
    pub resolutions: BTreeSet<StreamResolutionChoice>,
    pub hdr_only: bool,
    pub cached_only: bool,
}

impl StreamFilterState {
    pub fn is_unfiltered(&self, has_debrid_access: bool) -> bool {
        self.selected_addons.is_empty()
            && self.resolutions.is_empty()
            && !self.hdr_only
            && !(has_debrid_access && self.cached_only)
    }

    pub fn reset(&mut self) {
        self.selected_addons.clear();
        self.resolutions.clear();
        self.hdr_only = false;
        self.cached_only = false;
    }

    pub fn toggle_addon(&mut self, addon_key: &str) {
        let normalized = addon_key.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return;
        }
        if !self.selected_addons.remove(&normalized) {
            self.selected_addons.insert(normalized);
        }
    }

    pub fn is_addon_selected(&self, addon_key: &str) -> bool {
        let normalized = addon_key.trim().to_ascii_lowercase();
        self.selected_addons.contains(&normalized)
    }

    pub fn toggle_resolution(&mut self, choice: StreamResolutionChoice) {
        if !self.resolutions.remove(&choice) {
            self.resolutions.insert(choice);
        }
    }

    pub fn addon_dropdown_label(&self, available_addons: &[(String, String, usize)]) -> String {
        if self.selected_addons.is_empty() {
            return "All addons".to_owned();
        }
        let mut names: Vec<String> = self
            .selected_addons
            .iter()
            .map(|key| {
                available_addons
                    .iter()
                    .find(|(k, _, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, name, _)| name.clone())
                    .unwrap_or_else(|| key.clone())
            })
            .collect();
        names.sort_by_key(|a| a.to_ascii_lowercase());

        match names.as_slice() {
            [] => "All addons".to_owned(),
            [single] => format!("Addon: {single}"),
            [first, rest @ ..] => format!("Addon: {first} (+{})", rest.len()),
        }
    }

    pub fn to_core_filter(&self, has_debrid_access: bool) -> StreamFilter {
        let mut filter = StreamFilter::new().with_exclude_debrid(!has_debrid_access);
        for addon in &self.selected_addons {
            filter = filter.with_addon_source(addon.clone());
        }
        for res in &self.resolutions {
            filter = filter.with_resolution(res.to_core_resolution());
        }
        if self.hdr_only {
            filter = filter.only_hdr();
        }
        if has_debrid_access && self.cached_only {
            filter = filter.only_cached();
        }
        filter
    }

    pub fn matches_candidate(&self, cand: &PlaybackCandidate, has_debrid_access: bool) -> bool {
        if !has_debrid_access && cand.parsed.is_debrid_stream() {
            return false;
        }
        if has_debrid_access && self.cached_only && !cand.parsed.is_cached {
            return false;
        }
        if self.hdr_only && !cand.parsed.hdr.is_hdr() {
            return false;
        }
        if !self.selected_addons.is_empty() {
            let addon_hit = self.selected_addons.iter().any(|sel| {
                cand.addon_key.eq_ignore_ascii_case(sel) || cand.parsed.matches_addon_source(sel)
            });
            if !addon_hit {
                return false;
            }
        }
        if !self.resolutions.is_empty() {
            let res_hit = self
                .resolutions
                .iter()
                .any(|r| r.to_core_resolution() == cand.parsed.resolution);
            if !res_hit {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoverDropdown {
    Format,
    Catalog,
    Genre,
    Extra(String),
}

#[inline]
pub fn compute_scrub_preview_left(cursor_x: f32, track_width: f32, card_width: f32) -> f32 {
    if !cursor_x.is_finite()
        || !track_width.is_finite()
        || !card_width.is_finite()
        || track_width <= card_width
        || track_width <= 0.0
    {
        return 0.0;
    }
    (cursor_x - card_width * 0.5).clamp(0.0, (track_width - card_width).max(0.0))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibraryStatusTab {
    #[default]
    Watching,
    Watchlist,
    Watched,
}

impl LibraryStatusTab {
    pub const ALL: [Self; 3] = [Self::Watching, Self::Watchlist, Self::Watched];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Watching => "Watching",
            Self::Watchlist => "Watchlist",
            Self::Watched => "Watched",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibraryTypeFilter {
    #[default]
    All,
    Movies,
    Shows,
}

impl LibraryTypeFilter {
    pub const ALL: [Self; 3] = [Self::All, Self::Movies, Self::Shows];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Movies => "Movies",
            Self::Shows => "Shows",
        }
    }

    pub fn matches_type(self, raw_type: &str) -> bool {
        match self {
            Self::All => true,
            Self::Movies => raw_type.eq_ignore_ascii_case("movie"),
            Self::Shows => !raw_type.eq_ignore_ascii_case("movie"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibrarySortOrder {
    #[default]
    Recent,
    Title,
}

impl LibrarySortOrder {
    pub const fn next(self) -> Self {
        match self {
            Self::Recent => Self::Title,
            Self::Title => Self::Recent,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Recent => "Sort: Recent",
            Self::Title => "Sort: Title",
        }
    }
}

pub fn classify_library_item_status(li: &LibraryItem) -> Option<LibraryStatusTab> {
    if li.removed && li.state.time_offset == 0 && li.state.times_watched == 0 {
        return None;
    }
    if li.watched() || li.progress() >= 95.0 {
        Some(LibraryStatusTab::Watched)
    } else if li.state.time_offset > 0 || li.progress() > 0.0 || li.state.times_watched > 0 {
        Some(LibraryStatusTab::Watching)
    } else if !li.removed && !li.temp {
        Some(LibraryStatusTab::Watchlist)
    } else {
        None
    }
}

#[derive(Clone, Debug)]
pub struct PlaybackCandidate {
    pub stream: Stream,
    pub parsed: ParsedStream,
    pub stream_request: Option<ResourceRequest>,
    pub addon_key: String,
    pub addon_name: String,
}

impl PlaybackCandidate {
    pub fn from_stream_and_request(
        stream: Stream,
        stream_request: Option<ResourceRequest>,
        installed_addons: &[Descriptor],
    ) -> Self {
        let matched_desc = stream_request
            .as_ref()
            .and_then(|req| resolve_addon_for_request_base(installed_addons, &req.base));

        let manifest_id = matched_desc
            .map(|d| d.manifest.id.trim())
            .filter(|s| !s.is_empty());
        let manifest_name = matched_desc
            .map(|d| d.manifest.name.trim())
            .filter(|s| !s.is_empty());
        let transport_str = matched_desc
            .map(|d| d.transport_url.as_str())
            .or_else(|| stream_request.as_ref().map(|r| r.base.as_str()));

        let mut parsed = parse_stream_with_addon(&stream, manifest_name).with_addon_origin(
            manifest_id,
            manifest_name,
            transport_str,
        );

        let addon_name = manifest_name
            .map(str::to_owned)
            .or_else(|| parsed.addon_name.clone().filter(|s| !s.trim().is_empty()))
            .or_else(|| {
                stream_request
                    .as_ref()
                    .and_then(|r| r.base.host_str().map(str::to_owned))
            })
            .unwrap_or_else(|| "Direct".to_owned());

        if parsed
            .addon_name
            .as_deref()
            .is_none_or(|s| s.trim().is_empty())
        {
            parsed.addon_name = Some(addon_name.clone());
        }

        let addon_key = manifest_id
            .map(|id| id.to_ascii_lowercase())
            .unwrap_or_else(|| addon_name.to_ascii_lowercase());

        Self {
            stream,
            parsed,
            stream_request,
            addon_key,
            addon_name,
        }
    }

    pub fn summary_badge(&self) -> String {
        let mut parts = Vec::new();
        if !matches!(self.parsed.resolution, Resolution::Unknown) {
            parts.push(self.parsed.resolution.label().to_owned());
        }
        if self.parsed.hdr.is_hdr() {
            parts.push(self.parsed.hdr.label().to_owned());
        }
        if self.parsed.is_cached {
            parts.push("Cached".to_owned());
        } else if let Some(seeds) = self.parsed.seeders {
            parts.push(format!("{seeds} peers"));
        }
        if parts.is_empty() {
            "Stream".to_owned()
        } else {
            parts.join(" ")
        }
    }

    pub fn info_hash_hex(&self) -> Option<String> {
        match &self.stream.source {
            StreamSource::Torrent { info_hash, .. } => Some(hex::encode(info_hash)),
            _ => None,
        }
    }

    pub fn raw_release_filename(&self) -> String {
        if let Some(fname) = self
            .stream
            .behavior_hints
            .filename
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            return fname.trim().to_owned();
        }
        self.parsed.display_title.clone()
    }

    pub fn torrent_key(&self) -> Option<(String, Option<usize>)> {
        match &self.stream.source {
            StreamSource::Torrent {
                info_hash,
                file_idx,
                ..
            } => Some((hex::encode(info_hash), file_idx.map(usize::from))),
            _ => None,
        }
    }
}

pub fn sort_stream_candidates(candidates: &mut [PlaybackCandidate]) {
    candidates.sort_by(|ca, cb| {
        let a = &ca.parsed;
        let b = &cb.parsed;
        b.is_cached
            .cmp(&a.is_cached)
            .then_with(|| b.resolution.cmp(&a.resolution))
            .then_with(|| b.quality.cmp(&a.quality))
            .then_with(|| b.hdr.cmp(&a.hdr))
            .then_with(|| b.audio.cmp(&a.audio))
            .then_with(|| b.seeders.unwrap_or(0).cmp(&a.seeders.unwrap_or(0)))
            .then_with(|| b.size_bytes.unwrap_or(0).cmp(&a.size_bytes.unwrap_or(0)))
    });
}

pub fn build_ranked_stream_candidates(
    raw_streams: &[(Stream, Option<ResourceRequest>)],
    installed_addons: &[Descriptor],
    has_debrid_access: bool,
) -> Vec<PlaybackCandidate> {
    let mut candidates: Vec<PlaybackCandidate> = raw_streams
        .iter()
        .map(|(stream, req)| {
            PlaybackCandidate::from_stream_and_request(
                stream.clone(),
                req.clone(),
                installed_addons,
            )
        })
        .filter(|c| has_debrid_access || !c.parsed.is_debrid_stream())
        .collect();

    sort_stream_candidates(&mut candidates);
    candidates
}

fn hash_installed_addons(addons: &[Descriptor]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    addons.len().hash(&mut hasher);
    for desc in addons {
        desc.manifest.id.hash(&mut hasher);
        desc.manifest.name.hash(&mut hasher);
        desc.transport_url.as_str().hash(&mut hasher);
    }
    hasher.finish()
}

#[derive(Clone, Debug)]
pub struct AddonStreamSlotMemo {
    pub request_base: String,
    pub request_id: String,
    pub ready_len: Option<usize>,
    pub parsed_candidates: Vec<PlaybackCandidate>,
}

#[derive(Clone, Debug)]
pub struct MemoizedMetaStreams {
    pub type_name: String,
    pub meta_id: String,
    pub video_id: String,
    pub has_debrid_access: bool,
    pub addons_hash: u64,
    pub slot_memos: Vec<AddonStreamSlotMemo>,
    pub ranked_candidates: Arc<Vec<PlaybackCandidate>>,
    pub available_addons: Arc<Vec<(String, String, usize)>>,
    pub total_streams_parsed: usize,
}

pub type SyncedMetaStreams = (
    Arc<Vec<PlaybackCandidate>>,
    Arc<Vec<(String, String, usize)>>,
    usize,
    usize,
);

pub fn sync_memoized_meta_streams<'a>(
    memo_slot: &mut Option<MemoizedMetaStreams>,
    type_name: &str,
    meta_id: &str,
    video_id: &str,
    has_debrid_access: bool,
    slots_iter: impl Iterator<Item = &'a ResourceLoadable<Vec<Stream>>>,
    installed_addons: &[Descriptor],
) -> SyncedMetaStreams {
    let slots: SmallVec<[&'a ResourceLoadable<Vec<Stream>>; 24]> = slots_iter.collect();
    let addons_total = slots.len();
    let addons_ready = slots
        .iter()
        .filter(|res| matches!(res.content, Some(Loadable::Ready(_)) | Some(Loadable::Err(_))))
        .count();
    let addons_hash = hash_installed_addons(installed_addons);

    if let Some(existing) = memo_slot.as_ref() {
        let same_key = existing.type_name == type_name
            && existing.meta_id == meta_id
            && existing.video_id == video_id
            && existing.has_debrid_access == has_debrid_access
            && existing.addons_hash == addons_hash
            && existing.slot_memos.len() == slots.len();
        if same_key
            && existing
                .slot_memos
                .iter()
                .zip(slots.iter())
                .all(|(m, res)| {
                    let cur_len = match &res.content {
                        Some(Loadable::Ready(list)) => Some(list.len()),
                        _ => None,
                    };
                    m.ready_len == cur_len
                        && m.request_base == res.request.base.as_str()
                        && m.request_id == res.request.path.id
                })
        {
            return (
                Arc::clone(&existing.ranked_candidates),
                Arc::clone(&existing.available_addons),
                addons_ready,
                addons_total,
            );
        }
    }

    let (prev_slots, mut total_streams_parsed) = match memo_slot.take() {
        Some(prev)
            if prev.type_name == type_name
                && prev.meta_id == meta_id
                && prev.video_id == video_id
                && prev.has_debrid_access == has_debrid_access
                && prev.addons_hash == addons_hash =>
        {
            (prev.slot_memos, prev.total_streams_parsed)
        }
        _ => (Vec::new(), 0),
    };

    let mut new_slot_memos = Vec::with_capacity(slots.len());
    let mut total_candidates_cap = 0_usize;

    for (idx, res) in slots.iter().enumerate() {
        let base_str = res.request.base.as_str();
        let req_id = res.request.path.id.as_str();
        let ready_slice = match &res.content {
            Some(Loadable::Ready(list)) => Some(list.as_slice()),
            _ => None,
        };
        let ready_len = ready_slice.map(|s| s.len());

        let reusable = prev_slots
            .get(idx)
            .filter(|m| {
                m.ready_len == ready_len && m.request_base == base_str && m.request_id == req_id
            })
            .or_else(|| {
                prev_slots.iter().find(|m| {
                    m.ready_len == ready_len && m.request_base == base_str && m.request_id == req_id
                })
            });

        let parsed_candidates = if let Some(hit) = reusable {
            hit.parsed_candidates.clone()
        } else if let Some(list) = ready_slice {
            total_streams_parsed += list.len();
            let req_opt = Some(res.request.clone());
            list.iter()
                .map(|stream| {
                    PlaybackCandidate::from_stream_and_request(
                        stream.clone(),
                        req_opt.clone(),
                        installed_addons,
                    )
                })
                .filter(|c| has_debrid_access || !c.parsed.is_debrid_stream())
                .collect()
        } else {
            Vec::new()
        };

        total_candidates_cap += parsed_candidates.len();
        new_slot_memos.push(AddonStreamSlotMemo {
            request_base: base_str.to_owned(),
            request_id: req_id.to_owned(),
            ready_len,
            parsed_candidates,
        });
    }

    let mut merged = Vec::with_capacity(total_candidates_cap);
    for slot in &new_slot_memos {
        merged.extend(slot.parsed_candidates.iter().cloned());
    }
    sort_stream_candidates(&mut merged);

    let available_addons = Arc::new(summarize_available_stream_addons(&merged));
    let ranked_candidates = Arc::new(merged);

    *memo_slot = Some(MemoizedMetaStreams {
        type_name: type_name.to_owned(),
        meta_id: meta_id.to_owned(),
        video_id: video_id.to_owned(),
        has_debrid_access,
        addons_hash,
        slot_memos: new_slot_memos,
        ranked_candidates: Arc::clone(&ranked_candidates),
        available_addons: Arc::clone(&available_addons),
        total_streams_parsed,
    });

    (
        ranked_candidates,
        available_addons,
        addons_ready,
        addons_total,
    )
}

pub fn summarize_available_stream_addons(
    candidates: &[PlaybackCandidate],
) -> Vec<(String, String, usize)> {
    let mut map: std::collections::BTreeMap<String, (String, usize)> =
        std::collections::BTreeMap::new();
    for cand in candidates {
        let entry = map
            .entry(cand.addon_key.clone())
            .or_insert_with(|| (cand.addon_name.clone(), 0));
        entry.1 += 1;
    }
    let mut list: Vec<(String, String, usize)> = map
        .into_iter()
        .map(|(k, (name, count))| (k, name, count))
        .collect();
    list.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| a.1.to_ascii_lowercase().cmp(&b.1.to_ascii_lowercase()))
    });
    list
}

#[derive(Clone, Debug)]
pub struct PlaybackSession {
    pub generation: u64,
    pub type_name: String,
    pub meta_id: String,
    pub video_id: String,
    pub title: String,
    pub episode_label: Option<String>,
    pub meta_request: Option<ResourceRequest>,
    pub subtitles_path: Option<ResourcePath>,
    pub candidates: Arc<Vec<PlaybackCandidate>>,
    pub active_index: usize,
    pub start_offset_secs: Option<f64>,
    pub seamless_switch_pending: bool,
    pub applied_series_track_prefs: bool,
    pub resolved_url: Option<String>,
    pub loaded_into_mpv: bool,
    pub started_at: Instant,
    pub first_media_ready: bool,
    pub attached_external_subs: HashSet<String>,
    pub last_reported_time_ms: u64,
    pub last_reported_paused: Option<bool>,
    pub reported_ended: bool,
}

impl PlaybackSession {
    pub fn active_candidate(&self) -> Option<&PlaybackCandidate> {
        self.candidates.get(self.active_index)
    }

    pub fn build_player_selected(&self) -> Option<PlayerSelected> {
        let cand = self.active_candidate()?;
        Some(PlayerSelected {
            stream: cand.stream.clone(),
            stream_request: cand.stream_request.clone(),
            meta_request: self.meta_request.clone(),
            subtitles_path: self.subtitles_path.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerPopover {
    Volume,
    AudioTracks,
    Subtitles,
    VideoShaders,
    PlaybackSpeed,
    SwitchStream,
}

#[inline]
pub fn quantize_scrub_bucket_ms(timestamp_secs: f64) -> i64 {
    gpui_mpv::MpvThumbnailer::quantize_bucket_ms(timestamp_secs)
}

#[inline]
pub fn is_scrub_thumbnail_stale(
    current_key: Option<&(String, i64)>,
    target_url: &str,
    target_bucket_ms: i64,
) -> bool {
    current_key.is_some_and(|(prev_url, prev_ms)| {
        prev_url != target_url || (prev_ms - target_bucket_ms).abs() > 2_000
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct ToastMessage {
    pub id: u64,
    pub kind: ToastKind,
    pub title: String,
    pub detail: Option<String>,
    pub created_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedInput {
    CommandPalette,
    AuthEmail,
    AuthPassword,
    AddonManifestUrl,
    AddonSearch,
    DiscoverCatalogSearch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AddonSubTab {
    #[default]
    Installed,
    Explore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AddonCategoryFilter {
    #[default]
    All,
    Streams,
    Subtitles,
    Catalogs,
    Metadata,
}

impl AddonCategoryFilter {
    pub const ALL: [Self; 5] = [
        Self::All,
        Self::Streams,
        Self::Subtitles,
        Self::Catalogs,
        Self::Metadata,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Streams => "Streams",
            Self::Subtitles => "Subtitles",
            Self::Catalogs => "Catalogs",
            Self::Metadata => "Metadata",
        }
    }

    pub fn matches_resource_names<'a>(
        self,
        mut resource_names: impl Iterator<Item = &'a str>,
    ) -> bool {
        match self {
            Self::All => true,
            Self::Streams => resource_names.any(|r| r.eq_ignore_ascii_case("stream") || r.eq_ignore_ascii_case("streams")),
            Self::Subtitles => resource_names.any(|r| r.eq_ignore_ascii_case("subtitles") || r.eq_ignore_ascii_case("subtitle")),
            Self::Catalogs => resource_names.any(|r| r.eq_ignore_ascii_case("catalog") || r.eq_ignore_ascii_case("catalogs")),
            Self::Metadata => resource_names.any(|r| r.eq_ignore_ascii_case("meta") || r.eq_ignore_ascii_case("metadata")),
        }
    }

    pub const fn matches_addon_category(self, category: AddonCategory) -> bool {
        match self {
            Self::All => true,
            Self::Streams => matches!(category, AddonCategory::Streams),
            Self::Subtitles => matches!(category, AddonCategory::Subtitles),
            Self::Catalogs => matches!(category, AddonCategory::Catalogs),
            Self::Metadata => matches!(category, AddonCategory::Metadata),
        }
    }

    pub fn matches_addon_categories(self, categories: &[AddonCategory]) -> bool {
        match self {
            Self::All => true,
            _ => categories
                .iter()
                .copied()
                .any(|c| self.matches_addon_category(c)),
        }
    }
}

pub fn addon_matches_search_query(
    query: &str,
    name: &str,
    addon_id: &str,
    description: Option<&str>,
    media_types: &[String],
) -> bool {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    name.to_ascii_lowercase().contains(&q)
        || addon_id.to_ascii_lowercase().contains(&q)
        || description.is_some_and(|d| d.to_ascii_lowercase().contains(&q))
        || media_types.iter().any(|t| t.to_ascii_lowercase().contains(&q))
}

pub fn detect_addon_url_candidate(raw: &str) -> Option<String> {
    let trimmed = raw
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '<' || c == '>');
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("stremio://") && trimmed.len() > "stremio://".len() + 3 {
        return Some(trimmed.to_owned());
    }
    if (lower.starts_with("https://") || lower.starts_with("http://"))
        && (lower.ends_with("/manifest.json") || lower.contains("/manifest.json?"))
    {
        return Some(trimmed.to_owned());
    }
    None
}

pub fn extract_cli_addon_arg(args: impl IntoIterator<Item = String>) -> Option<String> {
    args.into_iter()
        .find_map(|arg| detect_addon_url_candidate(&arg))
}

pub fn apply_key_to_text_buffer(
    buffer: &mut String,
    event: &KeyDownEvent,
    clipboard_text: Option<String>,
) -> bool {
    let ks = &event.keystroke;
    let ctrl_or_cmd = ks.modifiers.control || ks.modifiers.platform;

    if ctrl_or_cmd {
        match ks.key.as_str() {
            "v" => {
                if let Some(paste) = clipboard_text {
                    let clean: String =
                        paste.chars().filter(|c| *c != '\n' && *c != '\r').collect();
                    if !clean.is_empty() {
                        buffer.push_str(&clean);
                        return true;
                    }
                }
                return false;
            }
            "u" | "backspace" => {
                if !buffer.is_empty() {
                    buffer.clear();
                    return true;
                }
                return false;
            }
            _ => return false,
        }
    }

    if ks.modifiers.alt {
        return false;
    }

    if ks.key == "backspace" {
        return buffer.pop().is_some();
    }

    if ks.key == "space" {
        buffer.push(' ');
        return true;
    }

    if let Some(ch_str) = &ks.key_char {
        let mut mutated = false;
        for ch in ch_str.chars() {
            if !ch.is_control() {
                buffer.push(ch);
                mutated = true;
            }
        }
        return mutated;
    }

    if ks.key.chars().count() == 1 {
        if let Some(ch) = ks.key.chars().next() {
            if !ch.is_control() {
                let c = if ks.modifiers.shift {
                    ch.to_ascii_uppercase()
                } else {
                    ch
                };
                buffer.push(c);
                return true;
            }
        }
    }

    false
}

pub fn parse_episode_badge_from_video_id(video_id: Option<&str>) -> Option<String> {
    let raw = video_id?.trim();
    let mut parts = raw.rsplitn(3, ':');
    let ep_str = parts.next()?;
    let season_str = parts.next()?;
    let _imdb_prefix = parts.next()?;
    let season = season_str.parse::<u32>().ok()?;
    let episode = ep_str.parse::<u32>().ok()?;
    Some(format!("S{season:02}E{episode:02}"))
}

pub fn format_movietracker_episode_code(video_id: Option<&str>) -> Option<String> {
    let raw = video_id?.trim();
    let mut parts = raw.rsplitn(3, ':');
    let ep_str = parts.next()?;
    let season_str = parts.next()?;
    let _imdb_prefix = parts.next()?;
    let season = season_str.parse::<u32>().ok()?;
    let episode = ep_str.parse::<u32>().ok()?;
    Some(format!("S{season} E{episode}"))
}

pub fn extract_imdb_rating(preview: &MetaItemPreview) -> Option<String> {
    preview
        .links
        .iter()
        .find(|l| l.category.eq_ignore_ascii_case("imdb"))
        .map(|l| l.name.trim().to_owned())
        .filter(|s| !s.is_empty())
}

pub fn extract_genres(preview: &MetaItemPreview, limit: usize) -> Vec<String> {
    preview
        .links
        .iter()
        .filter(|l| l.category.eq_ignore_ascii_case("genres"))
        .map(|l| l.name.trim().to_owned())
        .filter(|s| !s.is_empty())
        .take(limit)
        .collect()
}

pub fn build_subtitles_resource_path(type_name: &str, video_id: &str) -> ResourcePath {
    ResourcePath {
        resource: SUBTITLES_RESOURCE_NAME.to_owned(),
        r#type: type_name.to_owned(),
        id: video_id.to_owned(),
        extra: vec![],
    }
}

pub fn pick_default_series_episode_id(
    meta: &MetaItem,
    route_video_id: Option<&str>,
    library_video_id: Option<&str>,
) -> Option<String> {
    if meta.videos.is_empty() {
        return None;
    }
    if let Some(vid) = route_video_id.map(str::trim).filter(|v| !v.is_empty()) {
        if meta.videos.iter().any(|v| v.id == vid) {
            return Some(vid.to_owned());
        }
    }
    if let Some(vid) = library_video_id.map(str::trim).filter(|v| !v.is_empty()) {
        if meta.videos.iter().any(|v| v.id == vid) {
            return Some(vid.to_owned());
        }
    }
    if let Some((best_vid, _)) = meta
        .videos
        .iter()
        .filter_map(|v| {
            v.series_info
                .as_ref()
                .filter(|s| s.season > 0)
                .map(|s| (&v.id, (s.season, s.episode)))
        })
        .min_by_key(|(_, key)| *key)
    {
        return Some(best_vid.clone());
    }
    meta.videos.first().map(|v| v.id.clone())
}

#[derive(Clone, Debug)]
pub struct CachedTitlePreview {
    pub id: String,
    pub r#type: String,
    pub name: String,
    pub poster: Option<Url>,
    pub background: Option<Url>,
    pub release_info: Option<String>,
    pub runtime: Option<String>,
    pub description: Option<String>,
    pub imdb_rating: Option<String>,
    pub genres: Vec<String>,
}

impl CachedTitlePreview {
    fn from_preview(p: &MetaItemPreview) -> Self {
        Self {
            id: p.id.clone(),
            r#type: p.r#type.clone(),
            name: p.name.clone(),
            poster: p.poster.clone(),
            background: p.background.clone(),
            release_info: p.release_info.clone(),
            runtime: p.runtime.clone(),
            description: p.description.clone(),
            imdb_rating: extract_imdb_rating(p),
            genres: extract_genres(p, 3),
        }
    }
}

pub fn find_cached_preview_in_model(
    model: &DesktopModel,
    route_id: &str,
) -> Option<CachedTitlePreview> {
    for cat in &model.board.catalogs {
        for page in cat {
            if let Some(Loadable::Ready(items)) = &page.content {
                if let Some(found) = items.iter().find(|p| p.id == route_id) {
                    return Some(CachedTitlePreview::from_preview(found));
                }
            }
        }
    }
    for page in &model.discover.catalog {
        if let Some(Loadable::Ready(items)) = &page.content {
            if let Some(found) = items.iter().find(|p| p.id == route_id) {
                return Some(CachedTitlePreview::from_preview(found));
            }
        }
    }
    for cat in &model.search.catalogs {
        for page in cat {
            if let Some(Loadable::Ready(items)) = &page.content {
                if let Some(found) = items.iter().find(|p| p.id == route_id) {
                    return Some(CachedTitlePreview::from_preview(found));
                }
            }
        }
    }
    if let Some(li) = model.ctx.library.items.get(route_id) {
        return Some(CachedTitlePreview {
            id: li.id.clone(),
            r#type: li.r#type.clone(),
            name: li.name.clone(),
            poster: li.poster.clone(),
            background: None,
            release_info: None,
            runtime: None,
            description: None,
            imdb_rating: None,
            genres: Vec::new(),
        });
    }
    None
}

pub fn resolve_ready_meta_item(
    meta_items: &[stremio_core_runtime::stremio_core::models::common::ResourceLoadable<MetaItem>],
) -> Option<(&MetaItem, ResourceRequest)> {
    let mut fallback: Option<(&MetaItem, ResourceRequest)> = None;
    for loadable in meta_items {
        if let Some(Loadable::Ready(item)) = &loadable.content {
            if !item.videos.is_empty() || item.preview.description.is_some() {
                return Some((item, loadable.request.clone()));
            }
            if fallback.is_none() {
                fallback = Some((item, loadable.request.clone()));
            }
        }
    }
    fallback
}

pub fn build_default_meta_request(base: &Url, type_name: &str, id: &str) -> ResourceRequest {
    ResourceRequest {
        base: base.clone(),
        path: ResourcePath {
            resource: META_RESOURCE_NAME.to_owned(),
            r#type: type_name.to_owned(),
            id: id.to_owned(),
            extra: vec![],
        },
    }
}

pub fn build_default_stream_request(base: &Url, type_name: &str, video_id: &str) -> ResourceRequest {
    ResourceRequest {
        base: base.clone(),
        path: ResourcePath {
            resource: STREAM_RESOURCE_NAME.to_owned(),
            r#type: type_name.to_owned(),
            id: video_id.to_owned(),
            extra: vec![],
        },
    }
}

pub fn resume_offset_for_library_item(item: Option<&LibraryItem>, video_id: &str) -> Option<f64> {
    let li = item?;
    if li.state.time_offset == 0 {
        return None;
    }
    if let Some(saved_vid) = li.state.video_id.as_deref() {
        if saved_vid != video_id {
            return None;
        }
    }
    let secs = (li.state.time_offset as f64) / 1000.0;
    if secs > 3.0 {
        Some(secs)
    } else {
        None
    }
}

pub struct BootServices {
    pub tokio_handle: tokio::runtime::Handle,
    pub core: CoreController,
    pub engine: StreamEngineHandle,
    pub poster_cache: PosterCache,
    pub preferences: AppPreferences,
    pub mpv: Option<MpvController>,
    pub initial_snapshot: PlayerSnapshot,
    pub clipboard_seed: Option<ClipboardItem>,
    pub initial_addon_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Keystroke, Modifiers};

    #[test]
    fn parses_episode_badge_from_series_video_ids() {
        assert_eq!(
            parse_episode_badge_from_video_id(Some("tt0903747:5:14")),
            Some("S05E14".to_owned())
        );
        assert_eq!(
            parse_episode_badge_from_video_id(Some("kitsu:1234:1:2")),
            Some("S01E02".to_owned())
        );
        assert_eq!(parse_episode_badge_from_video_id(Some("tt0133093")), None);
        assert_eq!(parse_episode_badge_from_video_id(None), None);
    }

    #[test]
    fn preferences_roundtrip_and_experimental_video_defaults_off() {
        let def = AppPreferences::default();
        assert_eq!(def.experimental_video.shader_preset, ShaderPresetChoice::Off);
        assert!(!def.experimental_video.deband_enabled);
        assert!(!def.experimental_video.dolby_vision_reshape);
        assert!(!def.experimental_video.target_colorspace_hint);
        assert!(!def.experimental_video.hdr_compute_peak);

        let dir = tempfile::tempdir().unwrap();
        let pref_path = dir.path().join("prefs.json");
        let prefs = AppPreferences {
            storage_mode: StorageModeChoice::RamOnly { budget_gb: 2 },
            enable_upnp: false,
            dead_swarm_failover: true,
            dead_swarm_timeout_secs: 12,
            debrid_access_mode: DebridAccessMode::Subscribed,
            sidebar_collapsed: true,
            tone_mapping: ToneMappingChoice::Spline,
            upscale_mode: UpscaleChoice::EwaLanczos,
            experimental_video: ExperimentalVideoPrefs::default(),
        };
        prefs.save_to(&pref_path).unwrap();
        let loaded: AppPreferences =
            serde_json::from_str(&std::fs::read_to_string(&pref_path).unwrap()).unwrap();
        assert_eq!(loaded, prefs);
        assert_eq!(prefs.storage_mode.budget_bytes(), 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn text_buffer_handles_typing_paste_and_backspace() {
        let mut buf = String::new();
        let type_a = KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers::default(),
                key: "a".to_owned(),
                key_char: Some("a".to_owned()),
            },
            is_held: false,
            prefer_character_input: true,
        };
        assert!(apply_key_to_text_buffer(&mut buf, &type_a, None));
        assert_eq!(buf, "a");

        let paste_ev = KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
                key: "v".to_owned(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        };
        assert!(apply_key_to_text_buffer(
            &mut buf,
            &paste_ev,
            Some("https://addon.example/manifest.json\n".to_owned())
        ));
        assert_eq!(buf, "ahttps://addon.example/manifest.json");

        let backspace_ev = KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers::default(),
                key: "backspace".to_owned(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        };
        assert!(apply_key_to_text_buffer(&mut buf, &backspace_ev, None));
        assert_eq!(buf, "ahttps://addon.example/manifest.jso");
    }

    #[tokio::test]
    async fn stream_engine_fallback_binds_ephemeral_when_port_occupied() {
        let occupy = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let prefs = AppPreferences {
            enable_upnp: false,
            ..AppPreferences::default()
        };
        let mut cfg = build_engine_config_from_prefs(&prefs);
        cfg.bind_addr = occupy.local_addr().unwrap();
        assert!(StreamEngine::start(cfg).await.is_err());

        let mut fallback_cfg = build_engine_config_from_prefs(&prefs);
        fallback_cfg.bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let handle = StreamEngine::start(fallback_cfg).await.unwrap();
        assert_ne!(handle.local_addr().port(), 0);
        handle.shutdown().await;
    }

    #[test]
    fn switching_storage_mode_from_non_tokio_thread_via_handle_succeeds_without_reactor_panic() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let rt_handle = runtime.handle().clone();

        let temp_cache = tempfile::tempdir().unwrap();
        let cache_path = temp_cache.path().join("engine-cache");

        let handle = runtime.block_on(async {
            let cfg = StreamEngineConfig {
                bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
                enable_upnp: false,
                cache_dir: cache_path.clone(),
                ..StreamEngineConfig::default()
            };
            StreamEngine::start(cfg).await.unwrap()
        });

        assert!(tokio::runtime::Handle::try_current().is_err());

        futures::executor::block_on(switch_engine_storage_mode_on_tokio(
            &rt_handle,
            handle.clone(),
            StorageModeChoice::RamOnly { budget_gb: 2 }.to_engine_mode(),
        ))
        .expect("RamOnly switch via tokio_handle must succeed from non-Tokio thread");

        futures::executor::block_on(switch_engine_storage_mode_on_tokio(
            &rt_handle,
            handle.clone(),
            StorageModeChoice::DiskLru { budget_gb: 10 }.to_engine_mode(),
        ))
        .expect("DiskLru switch via tokio_handle must succeed from non-Tokio thread");

        runtime.block_on(handle.shutdown());
    }

    #[test]
    fn detects_clipboard_and_cli_addon_urls_and_filters_categories() {
        assert_eq!(
            detect_addon_url_candidate("  \"stremio://torrentio.strem.fun/manifest.json\" \n"),
            Some("stremio://torrentio.strem.fun/manifest.json".to_owned())
        );
        assert_eq!(
            detect_addon_url_candidate("https://v3-cinemeta.strem.io/manifest.json"),
            Some("https://v3-cinemeta.strem.io/manifest.json".to_owned())
        );
        assert_eq!(detect_addon_url_candidate("https://example.com/index.html"), None);
        assert_eq!(detect_addon_url_candidate("not a url with spaces"), None);

        let cli = vec![
            "--verbose".to_owned(),
            "stremio://opensubtitles-v3.strem.io/manifest.json".to_owned(),
        ];
        assert_eq!(
            extract_cli_addon_arg(cli),
            Some("stremio://opensubtitles-v3.strem.io/manifest.json".to_owned())
        );

        let resources = ["catalog", "meta", "stream"];
        assert!(AddonCategoryFilter::Streams.matches_resource_names(resources.iter().copied()));
        assert!(AddonCategoryFilter::Catalogs.matches_resource_names(resources.iter().copied()));
        assert!(AddonCategoryFilter::Metadata.matches_resource_names(resources.iter().copied()));
        assert!(!AddonCategoryFilter::Subtitles.matches_resource_names(resources.iter().copied()));

        let types = vec!["movie".to_owned(), "series".to_owned()];
        assert!(addon_matches_search_query(
            "cinemeta",
            "Cinemeta",
            "com.linvo.cinemeta",
            Some("Official catalog"),
            &types,
        ));
        assert!(!addon_matches_search_query(
            "kitsu",
            "Cinemeta",
            "com.linvo.cinemeta",
            Some("Official catalog"),
            &types,
        ));
    }

    #[test]
    fn scrub_preview_clamps_to_track_edges_and_debrid_mode_resolves() {
        assert_eq!(compute_scrub_preview_left(12.0, 600.0, 208.0), 0.0);
        assert!((compute_scrub_preview_left(300.0, 600.0, 208.0) - 196.0).abs() < 0.01);
        assert!((compute_scrub_preview_left(588.0, 600.0, 208.0) - 392.0).abs() < 0.01);
        assert_eq!(compute_scrub_preview_left(40.0, 150.0, 208.0), 0.0);

        assert!(!DebridAccessMode::AutoDetect.resolve_access(false));
        assert!(DebridAccessMode::AutoDetect.resolve_access(true));
        assert!(DebridAccessMode::Subscribed.resolve_access(false));
        assert!(!DebridAccessMode::NoneHideCached.resolve_access(true));

        assert_eq!(quantize_scrub_bucket_ms(-3.0), 0);
        assert_eq!(quantize_scrub_bucket_ms(0.9), 0);
        assert_eq!(quantize_scrub_bucket_ms(1.1), 2_000);
        assert_eq!(quantize_scrub_bucket_ms(115.2), 116_000);
        assert_eq!(quantize_scrub_bucket_ms(720.0), 720_000);

        let key_116s = ("http://127.0.0.1:11470/a.mkv".to_owned(), 116_000_i64);
        assert!(!is_scrub_thumbnail_stale(
            Some(&key_116s),
            "http://127.0.0.1:11470/a.mkv",
            116_000
        ));
        assert!(!is_scrub_thumbnail_stale(
            Some(&key_116s),
            "http://127.0.0.1:11470/a.mkv",
            118_000
        ));
        assert!(is_scrub_thumbnail_stale(
            Some(&key_116s),
            "http://127.0.0.1:11470/a.mkv",
            720_000
        ));
        assert!(is_scrub_thumbnail_stale(
            Some(&key_116s),
            "http://127.0.0.1:11470/other.mkv",
            116_000
        ));
    }

    #[test]
    fn composable_stream_filter_and_addon_origin_combine_correctly() {
        let addons: Vec<Descriptor> = serde_json::from_value(serde_json::json!([
            {
                "manifest": {
                    "id": "com.stremio.torrentio.addon",
                    "version": "0.0.14",
                    "name": "Torrentio",
                    "resources": ["stream"],
                    "types": ["movie", "series"],
                    "catalogs": []
                },
                "transportUrl": "https://torrentio.strem.fun/realdebrid=tok/manifest.json"
            },
            {
                "manifest": {
                    "id": "com.webstreamr.addon",
                    "version": "1.0.0",
                    "name": "WebStreamr",
                    "resources": ["stream"],
                    "types": ["movie", "series"],
                    "catalogs": []
                },
                "transportUrl": "https://webstreamr.hayd.uk/manifest.json"
            }
        ]))
        .expect("valid addon descriptors");

        let s_tio_4k_cached: Stream = serde_json::from_value(serde_json::json!({
            "url": "https://real-debrid.example/1.mkv",
            "name": "[RD+] Torrentio 4k",
            "description": "Dune.Part.Two.2024.2160p.BluRay.REMUX.DV.HEVC"
        }))
        .unwrap();
        let s_web_1080_hdr: Stream = serde_json::from_value(serde_json::json!({
            "url": "https://cdn.webstreamr.example/2.mkv",
            "name": "1080p",
            "description": "Dune.Part.Two.2024.1080p.WEB-DL.HDR10.HEVC"
        }))
        .unwrap();
        let s_tio_1080_p2p: Stream = serde_json::from_value(serde_json::json!({
            "infoHash": "0123456789abcdef0123456789abcdef01234567",
            "name": "Torrentio 1080p",
            "description": "Dune.Part.Two.2024.1080p.WEB-DL.x264\nSeeds: 84"
        }))
        .unwrap();

        let req_tio = build_default_stream_request(
            &Url::parse("https://torrentio.strem.fun/realdebrid=tok/manifest.json").unwrap(),
            "movie",
            "tt15239678",
        );
        let req_web = build_default_stream_request(
            &Url::parse("https://webstreamr.hayd.uk/manifest.json").unwrap(),
            "movie",
            "tt15239678",
        );

        let raw = vec![
            (s_tio_4k_cached, Some(req_tio.clone())),
            (s_web_1080_hdr, Some(req_web)),
            (s_tio_1080_p2p, Some(req_tio)),
        ];

        let ranked_debrid = build_ranked_stream_candidates(&raw, &addons, true);
        assert_eq!(ranked_debrid.len(), 3);
        let avail = summarize_available_stream_addons(&ranked_debrid);
        assert_eq!(avail.len(), 2);
        assert_eq!(avail[0], ("com.stremio.torrentio.addon".to_owned(), "Torrentio".to_owned(), 2));
        assert_eq!(avail[1], ("com.webstreamr.addon".to_owned(), "WebStreamr".to_owned(), 1));

        let mut ui_filter = StreamFilterState::default();
        assert!(ui_filter.is_unfiltered(true));
        ui_filter.toggle_addon("com.stremio.torrentio.addon");
        ui_filter.toggle_resolution(StreamResolutionChoice::Uhd4k);
        ui_filter.hdr_only = true;
        assert!(!ui_filter.is_unfiltered(true));
        assert_eq!(ui_filter.addon_dropdown_label(&avail), "Addon: Torrentio");

        let matched: Vec<_> = ranked_debrid
            .iter()
            .filter(|c| ui_filter.matches_candidate(c, true))
            .collect();
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].addon_name, "Torrentio");

        ui_filter.reset();
        ui_filter.toggle_resolution(StreamResolutionChoice::Uhd4k);
        ui_filter.toggle_resolution(StreamResolutionChoice::Fhd1080p);
        ui_filter.hdr_only = true;
        let matched_hdr: Vec<_> = ranked_debrid
            .iter()
            .filter(|c| ui_filter.matches_candidate(c, true))
            .collect();
        assert_eq!(matched_hdr.len(), 2);

        let ranked_no_debrid = build_ranked_stream_candidates(&raw, &addons, false);
        assert_eq!(ranked_no_debrid.len(), 2);
        assert!(ranked_no_debrid.iter().all(|c| !c.parsed.is_debrid_stream()));
    }

    #[test]
    fn memoized_meta_streams_parses_incrementally_and_paginates_1000_streams() {
        let addons: Vec<Descriptor> = serde_json::from_value(serde_json::json!([
            {
                "manifest": {
                    "id": "com.stremio.torrentio.addon",
                    "version": "0.0.14",
                    "name": "Torrentio",
                    "resources": ["stream"],
                    "types": ["movie", "series"],
                    "catalogs": []
                },
                "transportUrl": "https://torrentio.strem.fun/realdebrid=tok/manifest.json"
            },
            {
                "manifest": {
                    "id": "com.comet.addon",
                    "version": "1.0.0",
                    "name": "Comet",
                    "resources": ["stream"],
                    "types": ["movie", "series"],
                    "catalogs": []
                },
                "transportUrl": "https://comet.elfhosted.com/manifest.json"
            }
        ]))
        .expect("valid addon descriptors");

        let req_tio = build_default_stream_request(
            &Url::parse("https://torrentio.strem.fun/realdebrid=tok/manifest.json").unwrap(),
            "movie",
            "tt15239678",
        );
        let req_comet = build_default_stream_request(
            &Url::parse("https://comet.elfhosted.com/manifest.json").unwrap(),
            "movie",
            "tt15239678",
        );

        let tio_streams: Vec<Stream> = (0..700)
            .map(|i| {
                serde_json::from_value(serde_json::json!({
                    "url": format!("https://real-debrid.example/{i}.mkv"),
                    "name": if i % 2 == 0 { "[RD+] Torrentio 4k" } else { "[RD+] Torrentio 1080p" },
                    "description": format!("Dune.Part.Two.2024.{}.HDR10.HEVC.idx{i}", if i % 2 == 0 { "2160p" } else { "1080p" })
                }))
                .unwrap()
            })
            .collect();
        let comet_streams: Vec<Stream> = (0..300)
            .map(|i| {
                serde_json::from_value(serde_json::json!({
                    "url": format!("https://comet-cdn.example/{i}.mkv"),
                    "name": "Comet 1080p",
                    "description": format!("Dune.Part.Two.2024.1080p.WEB-DL.x265.idx{i}")
                }))
                .unwrap()
            })
            .collect();

        let mut slots = [
            ResourceLoadable {
                request: req_tio,
                content: Some(Loadable::Ready(tio_streams)),
            },
            ResourceLoadable {
                request: req_comet.clone(),
                content: Some(Loadable::Loading),
            },
        ];

        let mut memo: Option<MemoizedMetaStreams> = None;

        let (ranked_1, avail_1, ready_1, total_1) = sync_memoized_meta_streams(
            &mut memo,
            "movie",
            "tt15239678",
            "tt15239678",
            true,
            slots.iter(),
            &addons,
        );
        assert_eq!((ready_1, total_1), (1, 2));
        assert_eq!(ranked_1.len(), 700);
        assert_eq!(avail_1.len(), 1);
        assert_eq!(memo.as_ref().unwrap().total_streams_parsed, 700);

        let (ranked_2, avail_2, _, _) = sync_memoized_meta_streams(
            &mut memo,
            "movie",
            "tt15239678",
            "tt15239678",
            true,
            slots.iter(),
            &addons,
        );
        assert!(Arc::ptr_eq(&ranked_1, &ranked_2));
        assert!(Arc::ptr_eq(&avail_1, &avail_2));
        assert_eq!(memo.as_ref().unwrap().total_streams_parsed, 700);

        slots[1] = ResourceLoadable {
            request: req_comet,
            content: Some(Loadable::Ready(comet_streams)),
        };
        let (ranked_3, avail_3, ready_3, total_3) = sync_memoized_meta_streams(
            &mut memo,
            "movie",
            "tt15239678",
            "tt15239678",
            true,
            slots.iter(),
            &addons,
        );
        assert_eq!((ready_3, total_3), (2, 2));
        assert_eq!(ranked_3.len(), 1_000);
        assert_eq!(avail_3.len(), 2);
        assert_eq!(memo.as_ref().unwrap().total_streams_parsed, 1_000);

        let page_1_indices: Vec<usize> = (0..ranked_3.len()).take(STREAM_PAGE_SIZE).collect();
        assert_eq!(page_1_indices.len(), 30);
        let page_2_indices: Vec<usize> = (0..ranked_3.len()).take(STREAM_PAGE_SIZE * 2).collect();
        assert_eq!(page_2_indices.len(), 60);
    }

    #[test]
    fn series_default_episode_selection_picks_s1e1_or_saved_resume_episode() {
        let meta: MetaItem = serde_json::from_value(serde_json::json!({
            "id": "tt27995113",
            "type": "series",
            "name": "Black Doves",
            "videos": [
                {
                    "id": "tt27995113:0:1",
                    "title": "Behind the Scenes",
                    "released": "2024-12-01T00:00:00.000Z",
                    "season": 0,
                    "episode": 1
                },
                {
                    "id": "tt27995113:1:2",
                    "title": "A Little Black Dove",
                    "released": "2024-12-05T00:00:00.000Z",
                    "season": 1,
                    "episode": 2
                },
                {
                    "id": "tt27995113:1:1",
                    "title": "To Love Then",
                    "released": "2024-12-05T00:00:00.000Z",
                    "season": 1,
                    "episode": 1
                }
            ]
        }))
        .expect("deserialize Black Doves series MetaItem");

        assert_eq!(
            pick_default_series_episode_id(&meta, None, None),
            Some("tt27995113:1:1".to_owned())
        );

        assert_eq!(
            pick_default_series_episode_id(&meta, None, Some("tt27995113:1:2")),
            Some("tt27995113:1:2".to_owned())
        );

        assert_eq!(
            pick_default_series_episode_id(&meta, Some("tt27995113:0:1"), Some("tt27995113:1:2")),
            Some("tt27995113:0:1".to_owned())
        );
    }
}
