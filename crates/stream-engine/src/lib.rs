pub mod failover;
pub mod opensubhash;
pub mod range;
mod routes;
pub mod scheduler;
pub mod storage_mode;

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use futures::StreamExt;
use librqbit::{
    api::TorrentIdOrHash, dht::Id20, torrent_from_bytes, AddTorrent, AddTorrentOptions,
    DhtSessionConfig, ListenerMode, ListenerOptions, ManagedTorrent, PeerConnectionOptions,
    Session, SessionOptions, SessionPersistenceConfig,
};
use parking_lot::{Mutex, RwLock};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};
use stremio_core::types::{
    streaming_server::{
        DeviceInfo, File as StatFile, Growler, NetworkInfo, Options as StatOptions, PeerSearch,
        Settings, SettingsOption, SettingsOptionSelection, SettingsOptionValue, SettingsResponse,
        Source as StatSource, Statistics, SwarmCap,
    },
    torrent::InfoHash,
};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};
use url::Url;

pub use failover::{
    FailedCandidate, ResolvedStream, StreamCandidate, StreamCandidateKind,
    DEFAULT_FIRST_BYTE_TIMEOUT,
};
pub use opensubhash::{
    compute_opensubhash_bytes, compute_opensubhash_from_chunks, compute_opensubhash_http,
    compute_opensubhash_path, compute_opensubhash_url_or_path, OpensubHashError,
    OpensubHashResponse, OpensubHashResult, OPENSUB_HASH_CHUNK_SIZE,
};
pub use range::{
    content_type_for_path, is_video_path, parse_http_byte_range, RangeParseOutcome,
    ResolvedByteRange, VIDEO_EXTENSIONS,
};
pub use scheduler::{
    PiecePriority, PlayheadScheduler, PriorityPlan, RangeCursorGuard, DEFAULT_LOOKAHEAD_BYTES,
    HEAD_CRITICAL_BYTES, LOOKAHEAD_TARGET_SECONDS, MAX_LOOKAHEAD_BYTES, MIN_LOOKAHEAD_BYTES,
    TAIL_CRITICAL_BYTES,
};
pub use storage_mode::{
    allocate_ram_cache_dir_path, allocated_directory_bytes, CacheStateTracker, LruIndexFile,
    LruTorrentEntry, StorageMode, TorrentStorageLocation, DEFAULT_DISK_LRU_MAX_BYTES,
    DEFAULT_RAM_ONLY_MAX_BYTES, LRU_INDEX_FILENAME,
};

pub const DEFAULT_BIND_PORT: u16 = 11470;
pub const SERVER_VERSION: &str = "4.21.1";

pub const DEFAULT_OPENTRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://open.demonii.com:1337/announce",
];

pub const CURATED_OPENTRACKERS: &[&str] = DEFAULT_OPENTRACKERS;

pub fn redact_url(url: &Url) -> String {
    if let Some(host) = url.host_str() {
        let port_suffix = url.port().map(|p| format!(":{p}")).unwrap_or_default();
        format!("{}://{host}{port_suffix}/...", url.scheme())
    } else {
        format!("{}:...", url.scheme())
    }
}

pub fn redact_url_str(raw: &str) -> String {
    Url::parse(raw.trim())
        .map(|u| redact_url(&u))
        .unwrap_or_else(|_| "<redacted-url>".to_owned())
}

pub fn validate_info_hash_str(raw: &str) -> Result<[u8; 20], String> {
    if raw.contains('/')
        || raw.contains('\\')
        || raw.contains('\0')
        || raw.contains("..")
        || raw.bytes().any(|b| !b.is_ascii())
    {
        return Err("Invalid BitTorrent info_hash".to_owned());
    }
    let trimmed = raw.trim();
    if trimmed.len() == 40 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        let mut out = [0u8; 20];
        hex::decode_to_slice(trimmed, &mut out)
            .map_err(|_| "Invalid hex info_hash".to_owned())?;
        return Ok(out);
    }
    if trimmed.len() == 32
        && trimmed
            .bytes()
            .all(|b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'2'..=b'7'))
    {
        let upper = trimmed.to_ascii_uppercase();
        if let Ok(decoded) = data_encoding::BASE32.decode(upper.as_bytes()) {
            if decoded.len() == 20 {
                let mut out = [0u8; 20];
                out.copy_from_slice(&decoded);
                return Ok(out);
            }
        }
    }
    Err("Invalid BitTorrent info_hash".to_owned())
}

pub(crate) fn is_safe_path_no_traversal(path: &Path) -> bool {
    let raw = path.as_os_str().to_string_lossy();
    if raw.is_empty() || raw.contains('\0') {
        return false;
    }
    !path
        .components()
        .any(|c| matches!(c, Component::ParentDir))
}

pub(crate) fn validate_outbound_http_url(url: &Url) -> Result<(), String> {
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(format!("Unsupported URL scheme `{}`", url.scheme()));
    }
    if url.host_str().is_none_or(|h| h.is_empty()) {
        return Err("URL missing host".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Embedded URL credentials are not permitted".to_owned());
    }
    Ok(())
}

pub(crate) fn is_allowed_tracker_url(raw: &str) -> bool {
    let Ok(parsed) = Url::parse(raw) else {
        return false;
    };
    matches!(parsed.scheme(), "udp" | "http" | "https" | "ws" | "wss")
        && parsed.host_str().is_some_and(|h| !h.is_empty())
        && parsed.username().is_empty()
        && parsed.password().is_none()
}

#[cfg(unix)]
pub(crate) async fn ensure_dir_0700(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::create_dir_all(path).await?;
    let _ = tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) async fn ensure_dir_0700(path: &Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(path).await
}

#[cfg(unix)]
pub(crate) fn ensure_dir_0700_sync(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path)?;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn ensure_dir_0700_sync(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

#[cfg(unix)]
pub(crate) async fn set_file_0600(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await;
}

#[cfg(not(unix))]
pub(crate) async fn set_file_0600(_path: &Path) {}

#[cfg(unix)]
pub(crate) fn set_file_0600_sync(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
pub(crate) fn set_file_0600_sync(_path: &Path) {}

#[derive(Debug, Error)]
pub enum StreamEngineError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("URL error: {0}")]
    Url(#[from] url::ParseError),
    #[error("BitTorrent session error: {0}")]
    Torrent(String),
    #[error("Metadata resolution timed out after {0:?}")]
    MetadataTimeout(Duration),
    #[error("Initial playhead byte timed out after {0:?}")]
    FirstByteTimeout(Duration),
    #[error("All {} stream candidate(s) failed probing/failover", .attempts.len())]
    AllCandidatesFailed { attempts: Vec<FailedCandidate> },
}

#[derive(Debug, Clone)]
pub struct StreamEngineConfig {
    pub bind_addr: SocketAddr,
    pub cache_dir: PathBuf,
    pub enable_dht: bool,
    pub enable_trackers: bool,
    pub enable_lsd: bool,
    pub enable_peer_listener: bool,
    pub enable_upnp: bool,
    pub listen_port: Option<u16>,
    pub storage_mode: StorageMode,
    pub metadata_timeout: Duration,
    pub first_byte_timeout: Duration,
    pub extra_trackers: Vec<String>,
    pub cache_size: Option<f64>,
}

impl Default for StreamEngineConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_BIND_PORT),
            cache_dir: default_cache_dir(),
            enable_dht: true,
            enable_trackers: true,
            enable_lsd: true,
            enable_peer_listener: true,
            enable_upnp: true,
            listen_port: None,
            storage_mode: StorageMode::default(),
            metadata_timeout: Duration::from_secs(30),
            first_byte_timeout: DEFAULT_FIRST_BYTE_TIMEOUT,
            extra_trackers: DEFAULT_OPENTRACKERS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            cache_size: Some(DEFAULT_DISK_LRU_MAX_BYTES as f64),
        }
    }
}

impl StreamEngineConfig {
    pub fn for_testing(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            cache_dir: cache_dir.into(),
            enable_dht: false,
            enable_trackers: false,
            enable_lsd: false,
            enable_peer_listener: true,
            enable_upnp: false,
            listen_port: None,
            storage_mode: StorageMode::default(),
            metadata_timeout: Duration::from_secs(5),
            first_byte_timeout: Duration::from_secs(5),
            extra_trackers: DEFAULT_OPENTRACKERS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            cache_size: Some(DEFAULT_DISK_LRU_MAX_BYTES as f64),
        }
    }
}

pub fn default_cache_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            let trimmed = local_app_data.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join("ferrite").join("torrents");
            }
        }
        if let Ok(user_profile) = std::env::var("USERPROFILE") {
            let trimmed = user_profile.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed)
                    .join("AppData")
                    .join("Local")
                    .join("ferrite")
                    .join("torrents");
            }
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        let trimmed = xdg.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("ferrite").join("torrents");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed)
                .join(".cache")
                .join("ferrite")
                .join("torrents");
        }
    }
    std::env::temp_dir().join("ferrite").join("torrents")
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatsSnapshot {
    pub active_torrents: usize,
    pub download_speed_bytes: f64,
    pub upload_speed_bytes: f64,
    pub downloaded_bytes: u64,
    pub uploaded_bytes: u64,
    pub connected_peers: u64,
}

struct RegisteredMagnet {
    trackers: RwLock<Vec<String>>,
    started_at: DateTime<Utc>,
    resolve_lock: Arc<tokio::sync::Mutex<()>>,
}

#[inline]
fn trim_process_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        let _ = malloc_trim(0);
    }
}

pub(crate) struct EngineInner {
    pub config: StreamEngineConfig,
    pub runtime_handle: tokio::runtime::Handle,
    pub base_url: Url,
    pub local_addr: SocketAddr,
    pub session: Arc<Session>,
    pub scheduler: Arc<PlayheadScheduler>,
    pub cache_tracker: Arc<CacheStateTracker>,
    pub settings: RwLock<Settings>,
    pub upnp_enabled: AtomicBool,
    pub first_byte_timeout_lock: RwLock<Duration>,
    pub eviction_lock: tokio::sync::Mutex<()>,
    pub http_client: reqwest::Client,
    pub shutdown_token: CancellationToken,
    registered_magnets: DashMap<[u8; 20], Arc<RegisteredMagnet>>,
}

impl Drop for EngineInner {
    fn drop(&mut self) {
        self.scheduler.shutdown();
        self.shutdown_token.cancel();
        self.session.cancellation_token().cancel();
        self.cache_tracker.persist_disk_lru_index_sync();
        let ram_dir = self.cache_tracker.ram_cache_dir();
        if ram_dir.exists() && is_safe_path_no_traversal(ram_dir) {
            let _ = std::fs::remove_dir_all(ram_dir);
        }
    }
}

impl EngineInner {
    pub fn first_byte_timeout(&self) -> Duration {
        *self.first_byte_timeout_lock.read()
    }

    pub fn set_first_byte_timeout(&self, timeout: Duration) {
        *self.first_byte_timeout_lock.write() = timeout;
    }

    pub fn enable_upnp(&self) -> bool {
        self.upnp_enabled.load(Ordering::Relaxed)
    }

    pub fn set_enable_upnp(&self, enabled: bool) {
        self.upnp_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn merge_with_curated_trackers(&self, incoming: Vec<String>) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut merged = Vec::new();
        for tr in incoming {
            let clean = tr.trim().to_owned();
            if is_allowed_tracker_url(&clean) && seen.insert(clean.clone()) {
                merged.push(clean);
            }
        }
        if merged.len() < 5 {
            for extra in &self.config.extra_trackers {
                let clean = extra.trim().to_owned();
                if is_allowed_tracker_url(&clean) && seen.insert(clean.clone()) {
                    merged.push(clean);
                }
            }
        }
        merged
    }

    fn get_or_insert_magnet_entry(
        &self,
        info_hash: [u8; 20],
        new_trackers: Vec<String>,
    ) -> Arc<RegisteredMagnet> {
        let entry = self
            .registered_magnets
            .entry(info_hash)
            .or_insert_with(|| {
                Arc::new(RegisteredMagnet {
                    trackers: RwLock::new(Vec::new()),
                    started_at: Utc::now(),
                    resolve_lock: Arc::new(tokio::sync::Mutex::new(())),
                })
            })
            .clone();

        {
            let mut tr_guard = entry.trackers.write();
            let mut combined = tr_guard.clone();
            combined.extend(new_trackers);
            *tr_guard = self.merge_with_curated_trackers(combined);
        }
        entry
    }

    pub fn get_managed_torrent(&self, info_hash: &[u8; 20]) -> Option<Arc<ManagedTorrent>> {
        self.session
            .get(TorrentIdOrHash::Hash(Id20::new(*info_hash)))
    }

    pub fn register_magnet_and_prefetch(
        self: &Arc<Self>,
        info_hash: [u8; 20],
        trackers: Vec<String>,
    ) {
        let _ = self.get_or_insert_magnet_entry(info_hash, trackers);
        if self.get_managed_torrent(&info_hash).is_some() {
            return;
        }
        let this = Arc::clone(self);
        let cancel = self.shutdown_token.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => {}
                res = this.ensure_torrent_resolved(info_hash, Vec::new()) => {
                    if let Err(err) = res {
                        debug!(
                            info_hash = %hex::encode(info_hash),
                            status = %err.status,
                            message = %err.message,
                            "background magnet prefetch did not complete yet"
                        );
                    }
                }
            }
        });
    }

    async fn prepare_output_folder_for_torrent(
        &self,
        info_hash: &[u8; 20],
    ) -> Result<(PathBuf, TorrentStorageLocation), std::io::Error> {
        if let (Some(loc), Some(existing_folder)) = (
            self.cache_tracker.location_of(info_hash),
            self.cache_tracker.output_folder_of(info_hash),
        ) {
            if is_safe_path_no_traversal(&existing_folder) {
                ensure_dir_0700(&existing_folder).await?;
                return Ok((existing_folder, loc));
            }
        }

        let (folder, loc) = self.cache_tracker.output_folder_for_new_torrent(info_hash);
        ensure_dir_0700(&folder).await?;
        Ok((folder, loc))
    }

    async fn persist_torrent_metadata_backup(&self, info_hash: &[u8; 20], handle: &ManagedTorrent) {
        if !self.cache_tracker.mode().is_disk_lru() {
            return;
        }
        if self.cache_tracker.location_of(info_hash) == Some(TorrentStorageLocation::Ram) {
            return;
        }
        let Ok(raw_bytes) = handle.with_metadata(|m| m.torrent_bytes.clone()) else {
            return;
        };
        if raw_bytes.is_empty() {
            return;
        }
        let backup_dir = self.cache_tracker.disk_cache_dir().join(".torrents");
        if ensure_dir_0700(&backup_dir).await.is_ok() {
            let path = backup_dir.join(format!("{}.torrent", hex::encode(info_hash)));
            if tokio::fs::write(&path, &raw_bytes).await.is_ok() {
                set_file_0600(&path).await;
            }
        }
    }

    async fn backup_torrent_bitv(&self, info_hash: &[u8; 20]) {
        if !self.cache_tracker.mode().is_disk_lru() {
            return;
        }
        if self.cache_tracker.location_of(info_hash) == Some(TorrentStorageLocation::Ram) {
            return;
        }
        let hex_ih = hex::encode(info_hash);
        let rqbit_bitv = self
            .cache_tracker
            .disk_cache_dir()
            .join(".rqbit_session")
            .join(format!("{hex_ih}.bitv"));
        if !rqbit_bitv.exists() {
            return;
        }
        let backup_dir = self.cache_tracker.disk_cache_dir().join(".torrents");
        if ensure_dir_0700(&backup_dir).await.is_ok() {
            let dst = backup_dir.join(format!("{hex_ih}.bitv"));
            if tokio::fs::copy(&rqbit_bitv, &dst).await.is_ok() {
                set_file_0600(&dst).await;
            }
        }
    }

    async fn restore_torrent_bitv_if_needed(&self, info_hash: &[u8; 20]) {
        if !self.cache_tracker.mode().is_disk_lru() {
            return;
        }
        if self.cache_tracker.location_of(info_hash) == Some(TorrentStorageLocation::Ram) {
            return;
        }
        let hex_ih = hex::encode(info_hash);
        let backup = self
            .cache_tracker
            .disk_cache_dir()
            .join(".torrents")
            .join(format!("{hex_ih}.bitv"));
        if !backup.exists() {
            return;
        }
        let persist_dir = self.cache_tracker.disk_cache_dir().join(".rqbit_session");
        let dst = persist_dir.join(format!("{hex_ih}.bitv"));
        if !dst.exists()
            && ensure_dir_0700(&persist_dir).await.is_ok()
            && tokio::fs::copy(&backup, &dst).await.is_ok()
        {
            set_file_0600(&dst).await;
        }
    }

    async fn wait_handle_initialized(
        &self,
        info_hash: [u8; 20],
        handle: Arc<ManagedTorrent>,
    ) -> Result<Arc<ManagedTorrent>, routes::ApiHttpError> {
        let timeout_dur = self.config.metadata_timeout;
        tokio::time::timeout(timeout_dur, handle.wait_until_initialized())
            .await
            .map_err(|_| {
                routes::ApiHttpError::timeout(format!(
                    "Timed out after {:?} waiting for torrent metadata ({})",
                    timeout_dur,
                    hex::encode(info_hash)
                ))
            })?
            .map_err(|e| map_torrent_init_err(&info_hash, e))?;
        Ok(handle)
    }

    pub async fn ensure_torrent_resolved(
        &self,
        info_hash: [u8; 20],
        extra_trackers: Vec<String>,
    ) -> Result<Arc<ManagedTorrent>, routes::ApiHttpError> {
        let entry = self.get_or_insert_magnet_entry(info_hash, extra_trackers);
        if let Some(existing) = self.get_managed_torrent(&info_hash) {
            return self.wait_handle_initialized(info_hash, existing).await;
        }

        let timeout_dur = self.config.metadata_timeout;

        let resolve_fut = async {
            let _guard = entry.resolve_lock.lock().await;
            if let Some(existing) = self.get_managed_torrent(&info_hash) {
                existing
                    .wait_until_initialized()
                    .await
                    .map_err(|e| map_torrent_init_err(&info_hash, e))?;
                return Ok(existing);
            }

            let trackers = entry.trackers.read().clone();
            let (out_folder, loc) = self
                .prepare_output_folder_for_torrent(&info_hash)
                .await
                .map_err(|e| {
                    routes::ApiHttpError::internal(format!("Failed creating cache folder: {e}"))
                })?;

            if self.cache_tracker.mode().is_disk_lru() && loc == TorrentStorageLocation::Disk {
                let backup_torrent_path = self
                    .cache_tracker
                    .disk_cache_dir()
                    .join(".torrents")
                    .join(format!("{}.torrent", hex::encode(info_hash)));
                if let Ok(raw_bytes) = tokio::fs::read(&backup_torrent_path).await {
                    if !raw_bytes.is_empty() {
                        self.restore_torrent_bitv_if_needed(&info_hash).await;
                        let warm_opts = AddTorrentOptions {
                            paused: true,
                            overwrite: true,
                            output_folder: Some(out_folder.to_string_lossy().into_owned()),
                            trackers: Some(trackers.clone()),
                            peer_opts: Some(PeerConnectionOptions {
                                connect_timeout: Some(Duration::from_secs(8)),
                                read_write_timeout: Some(Duration::from_secs(15)),
                                ..Default::default()
                            }),
                            ..Default::default()
                        };
                        if let Ok(resp) = self
                            .session
                            .add_torrent(AddTorrent::from_bytes(raw_bytes), Some(warm_opts))
                            .await
                        {
                            if let Some(handle) = resp.into_handle() {
                                let _ = handle.wait_until_initialized().await;
                                let stats = handle.stats();
                                self.cache_tracker.record_or_touch(
                                    info_hash,
                                    loc,
                                    out_folder,
                                    handle.name(),
                                    stats.progress_bytes,
                                    self.scheduler.selected_file(&info_hash),
                                );
                                self.cache_tracker.persist_disk_lru_index().await;
                                debug!(
                                    info_hash = %hex::encode(info_hash),
                                    "warm-restored hibernated DiskLru torrent offline from .torrents backup"
                                );
                                return Ok(handle);
                            }
                        }
                    }
                }
            }

            let magnet_uri = build_magnet_with_trackers(&info_hash, &trackers);
            let opts = AddTorrentOptions {
                overwrite: true,
                output_folder: Some(out_folder.to_string_lossy().into_owned()),
                trackers: Some(trackers),
                peer_opts: Some(PeerConnectionOptions {
                    connect_timeout: Some(Duration::from_secs(8)),
                    read_write_timeout: Some(Duration::from_secs(15)),
                    ..Default::default()
                }),
                ..Default::default()
            };

            let response = self
                .session
                .add_torrent(AddTorrent::from_url(magnet_uri), Some(opts))
                .await
                .map_err(|e| {
                    routes::ApiHttpError::bad_gateway(format!(
                        "Failed resolving torrent {}: {e:#}",
                        hex::encode(info_hash)
                    ))
                })?;

            let handle = response.into_handle().ok_or_else(|| {
                routes::ApiHttpError::internal("librqbit returned ListOnly handle unexpectedly")
            })?;

            handle
                .wait_until_initialized()
                .await
                .map_err(|e| map_torrent_init_err(&info_hash, e))?;

            let stats = handle.stats();
            self.cache_tracker.record_or_touch(
                info_hash,
                loc,
                out_folder,
                handle.name(),
                stats.progress_bytes,
                None,
            );
            self.persist_torrent_metadata_backup(&info_hash, &handle)
                .await;
            self.cache_tracker.persist_disk_lru_index().await;

            Ok(handle)
        };

        tokio::time::timeout(timeout_dur, resolve_fut)
            .await
            .map_err(|_| {
                routes::ApiHttpError::timeout(format!(
                    "Timed out after {:?} waiting for torrent metadata ({})",
                    timeout_dur,
                    hex::encode(info_hash)
                ))
            })?
    }

    pub async fn add_torrent_bytes(
        &self,
        torrent_bytes: Vec<u8>,
        trackers: Vec<String>,
        initial_peers: Option<Vec<SocketAddr>>,
    ) -> Result<[u8; 20], StreamEngineError> {
        let parsed = torrent_from_bytes(&torrent_bytes)
            .map_err(|e| StreamEngineError::Torrent(format!("Invalid torrent bytes: {e:#}")))?;
        let expected_hash = parsed.info_hash.0;

        if let Some(existing) = self.get_managed_torrent(&expected_hash) {
            let merged_trackers = self.merge_with_curated_trackers(trackers);
            let _ = self.get_or_insert_magnet_entry(expected_hash, merged_trackers);
            let loc = self.cache_tracker.location_of(&expected_hash).unwrap_or_else(|| {
                if self.cache_tracker.mode().is_ram_only() {
                    TorrentStorageLocation::Ram
                } else {
                    TorrentStorageLocation::Disk
                }
            });
            let folder = self
                .cache_tracker
                .output_folder_of(&expected_hash)
                .unwrap_or_else(|| {
                    self.cache_tracker
                        .output_folder_for_new_torrent(&expected_hash)
                        .0
                });
            self.cache_tracker.record_or_touch(
                expected_hash,
                loc,
                folder,
                existing.name(),
                existing.stats().progress_bytes,
                self.scheduler.selected_file(&expected_hash),
            );
            return Ok(expected_hash);
        }

        let (out_folder, loc) = self.prepare_output_folder_for_torrent(&expected_hash).await?;
        self.restore_torrent_bitv_if_needed(&expected_hash).await;
        let merged_trackers = self.merge_with_curated_trackers(trackers);
        let opts = AddTorrentOptions {
            overwrite: true,
            output_folder: Some(out_folder.to_string_lossy().into_owned()),
            trackers: Some(merged_trackers.clone()),
            initial_peers,
            ..Default::default()
        };

        let resp = self
            .session
            .add_torrent(AddTorrent::from_bytes(torrent_bytes), Some(opts))
            .await
            .map_err(|e| StreamEngineError::Torrent(format!("{e:#}")))?;

        let handle = resp.into_handle().ok_or_else(|| {
            StreamEngineError::Torrent("librqbit add_torrent did not return a handle".to_owned())
        })?;

        let info_hash = handle.info_hash().0;
        let _ = self.get_or_insert_magnet_entry(info_hash, merged_trackers);

        tokio::time::timeout(self.config.metadata_timeout, handle.wait_until_initialized())
            .await
            .map_err(|_| StreamEngineError::MetadataTimeout(self.config.metadata_timeout))?
            .map_err(|e| StreamEngineError::Torrent(format!("{e:#}")))?;

        let stats = handle.stats();
        self.cache_tracker.record_or_touch(
            info_hash,
            loc,
            out_folder,
            handle.name(),
            stats.progress_bytes,
            None,
        );
        self.persist_torrent_metadata_backup(&info_hash, &handle)
            .await;
        self.cache_tracker.persist_disk_lru_index().await;
        let _ = self.enforce_cache_quota_excluding(Some(info_hash)).await;

        Ok(info_hash)
    }

    pub async fn activate_and_touch_episode(
        &self,
        handle: &Arc<ManagedTorrent>,
        info_hash: [u8; 20],
        resolved_idx: usize,
    ) -> Result<PriorityPlan, StreamEngineError> {
        handle
            .wait_until_initialized()
            .await
            .map_err(|e| StreamEngineError::Torrent(format!("torrent init failed: {e:#}")))?;

        let file_len = handle
            .with_metadata(|m| m.file_infos.get(resolved_idx).map(|f| f.len).unwrap_or(0))
            .unwrap_or(0);
        let tstats = handle.stats();
        let file_have = tstats
            .file_progress
            .get(resolved_idx)
            .copied()
            .unwrap_or(0);
        if handle.is_paused() && file_have < file_len {
            let _ = self.session.unpause(handle).await;
        }

        let plan = self
            .scheduler
            .activate_episode(
                &self.session,
                handle,
                info_hash,
                resolved_idx,
                &self.shutdown_token,
            )
            .await?;

        let default_loc = if self.cache_tracker.mode().is_ram_only() {
            TorrentStorageLocation::Ram
        } else {
            TorrentStorageLocation::Disk
        };
        let loc = self
            .cache_tracker
            .location_of(&info_hash)
            .unwrap_or(default_loc);
        let out_folder = self
            .cache_tracker
            .output_folder_of(&info_hash)
            .unwrap_or_else(|| {
                self.cache_tracker
                    .output_folder_for_new_torrent(&info_hash)
                    .0
            });

        self.cache_tracker.record_or_touch(
            info_hash,
            loc,
            out_folder,
            handle.name(),
            tstats.progress_bytes,
            Some(resolved_idx),
        );
        self.persist_torrent_metadata_backup(&info_hash, handle)
            .await;

        let mut other_inactive: HashSet<[u8; 20]> = self.session.with_torrents(|it| {
            it.map(|(_, h)| h.info_hash().0)
                .filter(|ih| {
                    *ih != info_hash && !self.scheduler.has_active_streams_for_torrent(ih)
                })
                .collect()
        });
        for entry in self.registered_magnets.iter() {
            let ih = *entry.key();
            if ih != info_hash && !self.scheduler.has_active_streams_for_torrent(&ih) {
                other_inactive.insert(ih);
            }
        }
        if !other_inactive.is_empty() {
            for other_ih in other_inactive {
                self.hibernate_inactive_torrent(other_ih).await;
            }
            trim_process_heap();
        }

        let _ = self.enforce_cache_quota_excluding(Some(info_hash)).await;
        self.cache_tracker.persist_disk_lru_index().await;

        Ok(plan)
    }

    pub fn update_torrent_cache_size_sync(&self, info_hash: &[u8; 20]) {
        if let Some(handle) = self.get_managed_torrent(info_hash) {
            let stats = handle.stats();
            self.cache_tracker
                .update_size_bytes(info_hash, stats.progress_bytes, handle.name());
        }
    }

    pub async fn refresh_torrent_cache_accounting(&self, info_hash: &[u8; 20]) {
        self.update_torrent_cache_size_sync(info_hash);
        let _ = self.enforce_cache_quota_excluding(None).await;
        self.cache_tracker.persist_disk_lru_index().await;
    }

    pub async fn hibernate_inactive_torrent(&self, info_hash: [u8; 20]) {
        if self.cache_tracker.mode().is_ram_only()
            || self.cache_tracker.location_of(&info_hash) == Some(TorrentStorageLocation::Ram)
        {
            self.evict_torrent_completely(info_hash).await;
            return;
        }

        self.scheduler.remove_torrent(&info_hash);
        self.registered_magnets.remove(&info_hash);

        if let Some(handle) = self.get_managed_torrent(&info_hash) {
            self.cache_tracker.update_size_bytes(
                &info_hash,
                handle.stats().progress_bytes,
                handle.name(),
            );
            self.persist_torrent_metadata_backup(&info_hash, &handle)
                .await;
            if !handle.is_paused() {
                let _ = self.session.pause(&handle).await;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            self.backup_torrent_bitv(&info_hash).await;
            if handle.with_metadata(|_| ()).is_ok() {
                let _ = self
                    .session
                    .delete(TorrentIdOrHash::Hash(Id20::new(info_hash)), false)
                    .await;
            }
        }
    }

    pub async fn release_playback_sessions(&self) {
        self.scheduler.clear_all();

        let mut all_hashes: HashSet<[u8; 20]> = self
            .session
            .with_torrents(|it| it.map(|(_, h)| h.info_hash().0).collect());
        for entry in self.registered_magnets.iter() {
            all_hashes.insert(*entry.key());
        }

        for ih in all_hashes {
            self.hibernate_inactive_torrent(ih).await;
        }
        self.registered_magnets.clear();

        if self.cache_tracker.mode().is_disk_lru() {
            self.cache_tracker.persist_disk_lru_index().await;
        }

        trim_process_heap();
    }

    fn is_deletable_torrent_subfolder(&self, folder: &Path) -> bool {
        if !is_safe_path_no_traversal(folder) {
            return false;
        }
        let disk_root = self.cache_tracker.disk_cache_dir();
        let ram_root = self.cache_tracker.ram_cache_dir();
        (folder.starts_with(disk_root) && folder != disk_root)
            || (folder.starts_with(ram_root) && folder != ram_root)
    }

    pub async fn evict_torrent_completely(&self, info_hash: [u8; 20]) {
        self.scheduler.remove_torrent(&info_hash);
        self.registered_magnets.remove(&info_hash);
        let removed_meta = self.cache_tracker.remove_item(&info_hash);

        if let Some(h) = self.get_managed_torrent(&info_hash) {
            if h.with_metadata(|_| ()).is_ok() {
                let _ = self
                    .session
                    .delete(TorrentIdOrHash::Hash(Id20::new(info_hash)), true)
                    .await;
            } else {
                let _ = self.session.pause(&h).await;
            }
        }

        if let Some((loc, folder)) = removed_meta {
            if folder.exists() && self.is_deletable_torrent_subfolder(&folder) {
                let _ = tokio::fs::remove_dir_all(&folder).await;
            }
            if loc == TorrentStorageLocation::Disk {
                let hex_ih = hex::encode(info_hash);
                let disk_root = self.cache_tracker.disk_cache_dir();
                let _ = tokio::fs::remove_file(
                    disk_root.join(".torrents").join(format!("{hex_ih}.torrent")),
                )
                .await;
                let _ = tokio::fs::remove_file(
                    disk_root.join(".torrents").join(format!("{hex_ih}.bitv")),
                )
                .await;
                let _ = tokio::fs::remove_file(
                    disk_root
                        .join(".rqbit_session")
                        .join(format!("{hex_ih}.bitv")),
                )
                .await;
            }
        }
    }

    pub async fn evict_inactive_ram_torrents_except(&self, keep: Option<[u8; 20]>) -> usize {
        let _guard = self.eviction_lock.lock().await;
        let ram_hashes = self
            .cache_tracker
            .tracked_hashes_by_location(TorrentStorageLocation::Ram);
        let mut evicted = 0usize;
        for ih in ram_hashes {
            if keep == Some(ih) {
                continue;
            }
            if self.scheduler.has_active_streams_for_torrent(&ih) {
                continue;
            }
            self.evict_torrent_completely(ih).await;
            evicted += 1;
        }
        evicted
    }

    pub async fn enforce_cache_quota_excluding(&self, protected_extra: Option<[u8; 20]>) -> usize {
        let _guard = self.eviction_lock.lock().await;

        let live_snapshots: Vec<([u8; 20], u64, Option<String>)> = self.session.with_torrents(|it| {
            it.map(|(_, h)| (h.info_hash().0, h.stats().progress_bytes, h.name()))
                .collect()
        });
        for (ih, bytes, name) in live_snapshots {
            self.cache_tracker.update_size_bytes(&ih, bytes, name);
        }

        let mode = self.cache_tracker.mode();
        let (loc, max_bytes) = match mode {
            StorageMode::DiskLru { max_bytes } => (TorrentStorageLocation::Disk, max_bytes),
            StorageMode::RamOnly { max_bytes } => (TorrentStorageLocation::Ram, max_bytes),
        };

        let victims = self.cache_tracker.select_eviction_victims(
            loc,
            max_bytes,
            &|ih| {
                protected_extra.is_some_and(|p| p == *ih)
                    || self.scheduler.has_active_streams_for_torrent(ih)
            },
        );

        let count = victims.len();
        for (ih, _folder) in victims {
            info!(
                info_hash = %hex::encode(ih),
                ?mode,
                "evicting oldest inactive cached torrent to respect storage quota"
            );
            self.evict_torrent_completely(ih).await;
        }

        if count > 0 && loc == TorrentStorageLocation::Disk {
            self.cache_tracker.persist_disk_lru_index().await;
        }
        count
    }

    pub async fn switch_storage_mode(&self, next_mode: StorageMode) -> Result<(), StreamEngineError> {
        let prev_mode = self.cache_tracker.mode();
        self.cache_tracker.set_mode(next_mode);

        match next_mode {
            StorageMode::RamOnly { max_bytes } => {
                ensure_dir_0700(self.cache_tracker.ram_cache_dir()).await?;
                {
                    let mut settings = self.settings.write();
                    settings.cache_size = Some(max_bytes as f64);
                    settings.cache_root = self
                        .cache_tracker
                        .ram_cache_dir()
                        .to_string_lossy()
                        .into_owned();
                }
                if prev_mode.is_disk_lru() {
                    let disk_hashes = self
                        .cache_tracker
                        .tracked_hashes_by_location(TorrentStorageLocation::Disk);
                    for ih in disk_hashes {
                        if !self.scheduler.has_active_streams_for_torrent(&ih) {
                            if let Some(h) = self.get_managed_torrent(&ih) {
                                let _ = self.session.pause(&h).await;
                            }
                        }
                    }
                }
                let _ = self.enforce_cache_quota_excluding(None).await;
            }
            StorageMode::DiskLru { max_bytes } => {
                ensure_dir_0700(self.cache_tracker.disk_cache_dir()).await?;
                {
                    let mut settings = self.settings.write();
                    settings.cache_size = Some(max_bytes as f64);
                    settings.cache_root = self
                        .cache_tracker
                        .disk_cache_dir()
                        .to_string_lossy()
                        .into_owned();
                }
                if prev_mode.is_ram_only() {
                    self.evict_inactive_ram_torrents_except(None).await;
                }
                let _ = self.enforce_cache_quota_excluding(None).await;
                self.cache_tracker.persist_disk_lru_index().await;
            }
        }
        Ok(())
    }

    pub fn resolve_file_idx_from_handle(
        &self,
        info_hash: &[u8; 20],
        requested_idx: i32,
        file_must_include: &[String],
    ) -> Option<usize> {
        let handle = self.get_managed_torrent(info_hash)?;
        handle
            .with_metadata(|meta| {
                routes::select_best_file_index(&meta.file_infos, requested_idx, file_must_include)
                    .ok()
            })
            .ok()
            .flatten()
    }

    pub fn settings_response(&self) -> SettingsResponse {
        let values = self.settings.read().clone();
        let options = vec![
            SettingsOption {
                id: "cacheSize".to_owned(),
                label: "CACHING".to_owned(),
                supports_directory: None,
                selections: vec![
                    SettingsOptionSelection {
                        name: "no caching".to_owned(),
                        val: SettingsOptionValue::Number(0.0),
                    },
                    SettingsOptionSelection {
                        name: "2GB".to_owned(),
                        val: SettingsOptionValue::Number(2_147_483_648.0),
                    },
                    SettingsOptionSelection {
                        name: "5GB".to_owned(),
                        val: SettingsOptionValue::Number(5_368_709_120.0),
                    },
                    SettingsOptionSelection {
                        name: "10GB".to_owned(),
                        val: SettingsOptionValue::Number(10_737_418_240.0),
                    },
                    SettingsOptionSelection {
                        name: "∞".to_owned(),
                        val: SettingsOptionValue::Null,
                    },
                ],
            },
            SettingsOption {
                id: "cacheRoot".to_owned(),
                label: "SETTINGS_CACHING_DRIVE".to_owned(),
                supports_directory: Some(true),
                selections: vec![SettingsOptionSelection {
                    name: "Default".to_owned(),
                    val: SettingsOptionValue::String(values.cache_root.clone()),
                }],
            },
        ];

        SettingsResponse {
            base_url: self.base_url.clone(),
            values,
            options,
        }
    }

    pub async fn apply_settings_patch(
        &self,
        patch: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        let validated_cache_root = if let Some(val) = patch.get("cacheRoot") {
            let Some(root_str) = val.as_str() else {
                return Err("`cacheRoot` must be a string path".to_owned());
            };
            let trimmed = root_str.trim();
            if trimmed.is_empty() {
                return Err("`cacheRoot` cannot be empty".to_owned());
            }
            let root_path = PathBuf::from(trimmed);
            if !root_path.is_absolute() || !is_safe_path_no_traversal(&root_path) {
                return Err(
                    "`cacheRoot` must be an absolute path without parent traversal".to_owned(),
                );
            }
            ensure_dir_0700(&root_path)
                .await
                .map_err(|e| format!("Failed creating cacheRoot: {e}"))?;
            Some(trimmed.to_owned())
        } else {
            None
        };

        let mut settings = self.settings.write();
        if let Some(root) = validated_cache_root {
            settings.cache_root = root;
        }
        if let Some(val) = patch.get("cacheSize") {
            if val.is_null() {
                settings.cache_size = None;
            } else if let Some(num) = val.as_f64() {
                settings.cache_size = Some(num);
            }
        }
        if let Some(v) = patch.get("btMaxConnections").and_then(|v| v.as_u64()) {
            settings.bt_max_connections = v;
        }
        if let Some(v) = patch.get("btHandshakeTimeout").and_then(|v| v.as_u64()) {
            settings.bt_handshake_timeout = v;
        }
        if let Some(v) = patch.get("btRequestTimeout").and_then(|v| v.as_u64()) {
            settings.bt_request_timeout = v;
        }
        if let Some(v) = patch
            .get("btDownloadSpeedSoftLimit")
            .and_then(|v| v.as_f64())
        {
            settings.bt_download_speed_soft_limit = v;
        }
        if let Some(v) = patch
            .get("btDownloadSpeedHardLimit")
            .and_then(|v| v.as_f64())
        {
            settings.bt_download_speed_hard_limit = v;
        }
        if let Some(v) = patch.get("btMinPeersForStable").and_then(|v| v.as_u64()) {
            settings.bt_min_peers_for_stable = v;
        }
        if let Some(val) = patch.get("remoteHttps") {
            settings.remote_https = val.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        }
        if let Some(v) = patch.get("proxyStreamsEnabled").and_then(|v| v.as_bool()) {
            settings.proxy_streams_enabled = v;
        }
        if let Some(val) = patch.get("transcodeProfile") {
            settings.transcode_profile = val.as_str().map(str::to_owned);
        }

        Ok(())
    }

    pub fn device_info(&self) -> DeviceInfo {
        let mut accels = Vec::new();
        #[cfg(target_os = "windows")]
        {
            accels.push("d3d11va".to_owned());
            accels.push("vulkan".to_owned());
        }
        #[cfg(not(target_os = "windows"))]
        {
            if Path::new("/dev/dri/renderD128").exists() || Path::new("/dev/dri").exists() {
                accels.push("vaapi".to_owned());
                accels.push("vulkan".to_owned());
            }
        }
        DeviceInfo {
            available_hardware_accelerations: accels,
        }
    }

    pub fn network_info(&self) -> NetworkInfo {
        let mut interfaces = Vec::new();
        if let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
            if sock.connect("198.51.100.1:80").is_ok() {
                if let Ok(addr) = sock.local_addr() {
                    let ip = addr.ip().to_string();
                    if !ip.is_empty() && ip != "0.0.0.0" {
                        interfaces.push(ip);
                    }
                }
            }
        }
        if !interfaces.iter().any(|ip| ip == "127.0.0.1") {
            interfaces.push("127.0.0.1".to_owned());
        }
        NetworkInfo {
            available_interfaces: interfaces,
        }
    }

    pub fn build_statistics(
        &self,
        info_hash: &[u8; 20],
        file_idx: Option<usize>,
    ) -> Option<Statistics> {
        let info_hash_hex = hex::encode(info_hash);
        let st_info_hash = InfoHash::new(*info_hash);
        let settings = self.settings.read().clone();

        let registered = self.registered_magnets.get(info_hash).map(|r| r.clone());
        let handle = self.get_managed_torrent(info_hash);

        if handle.is_none() && registered.is_none() {
            return None;
        }

        let started_iso = registered
            .as_ref()
            .map(|r| r.started_at.to_rfc3339())
            .unwrap_or_else(|| Utc::now().to_rfc3339());

        let tracker_list: Vec<String> = if let Some(ref reg) = registered {
            reg.trackers.read().clone()
        } else if let Some(ref h) = handle {
            h.shared()
                .trackers
                .iter()
                .map(ToString::to_string)
                .collect()
        } else {
            Vec::new()
        };

        let mut sources = Vec::new();
        if self.config.enable_dht {
            if let Ok(dht_url) = Url::parse(&format!("dht:{info_hash_hex}")) {
                sources.push(StatSource {
                    last_started: started_iso.clone(),
                    num_found: 0,
                    num_found_uniq: 0,
                    num_requests: 1,
                    url: dht_url,
                });
            }
        }
        for tr in &tracker_list {
            let prefixed = if tr.starts_with("tracker:") {
                tr.clone()
            } else {
                format!("tracker:{tr}")
            };
            if let Ok(url) = Url::parse(&prefixed) {
                sources.push(StatSource {
                    last_started: started_iso.clone(),
                    num_found: 0,
                    num_found_uniq: 0,
                    num_requests: 1,
                    url,
                });
            }
        }

        let peer_search_sources: Vec<String> = tracker_list
            .iter()
            .map(|tr| {
                if tr.starts_with("tracker:") || tr.starts_with("dht:") {
                    tr.clone()
                } else {
                    format!("tracker:{tr}")
                }
            })
            .collect();

        let opts = StatOptions {
            connections: Some(settings.bt_max_connections),
            dht: self.config.enable_dht,
            growler: Growler {
                flood: 0,
                pulse: Some(settings.bt_download_speed_hard_limit as u64),
            },
            handshake_timeout: Some(settings.bt_handshake_timeout),
            path: PathBuf::from(&settings.cache_root)
                .join(&info_hash_hex)
                .to_string_lossy()
                .into_owned(),
            peer_search: PeerSearch::new(40, 200, st_info_hash, peer_search_sources),
            swarm_cap: SwarmCap {
                max_speed: Some(settings.bt_download_speed_soft_limit),
                min_peers: Some(settings.bt_min_peers_for_stable),
            },
            timeout: Some(settings.bt_request_timeout),
            tracker: self.config.enable_trackers,
            r#virtual: true,
        };

        let Some(handle) = handle else {
            return Some(Statistics {
                name: info_hash_hex.clone(),
                info_hash: info_hash_hex,
                files: Vec::new(),
                sources,
                opts,
                download_speed: 0.0,
                upload_speed: 0.0,
                downloaded: 0,
                uploaded: 0,
                unchoked: 0,
                peers: 0,
                queued: 0,
                unique: 0,
                connection_tries: 0,
                peer_search_running: true,
                stream_len: None,
                stream_name: None,
                stream_progress: None,
                swarm_connections: 0,
                swarm_paused: false,
                swarm_size: 0,
            });
        };

        let tstats = handle.stats();
        let files: Vec<StatFile> = handle
            .with_metadata(|meta| {
                meta.file_infos
                    .iter()
                    .map(|fi| {
                        let rel = fi.relative_filename.to_string_lossy().into_owned();
                        let name = fi
                            .relative_filename
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| rel.clone());
                        StatFile {
                            name,
                            path: rel,
                            length: fi.len,
                            offset: fi.offset_in_torrent,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let (
            download_speed,
            upload_speed,
            peers,
            queued,
            unique,
            connection_tries,
            swarm_connections,
            swarm_size,
        ) = if let Some(ref live) = tstats.live {
            let ps = &live.snapshot.peer_stats;
            let down_bps = live.download_speed.mbps * 1024.0 * 1024.0;
            let up_bps = live.upload_speed.mbps * 1024.0 * 1024.0;
            let live_peers = ps.live as u64;
            let queued_peers = ps.queued as u64;
            let seen_peers = ps.seen as u64;
            let tries = (ps.connecting + ps.live + ps.dead) as u64;
            let conns = (ps.live + ps.connecting) as u64;
            let size = seen_peers.max(conns);
            (
                down_bps,
                up_bps,
                live_peers,
                queued_peers,
                seen_peers,
                tries,
                conns,
                size,
            )
        } else {
            (0.0, 0.0, 0, 0, 0, 0, 0, 0)
        };

        let (stream_len, stream_name, stream_progress) = file_idx
            .and_then(|idx| {
                files.get(idx).map(|f| {
                    let have_bytes = tstats.file_progress.get(idx).copied().unwrap_or(0);
                    let progress = if f.length == 0 {
                        1.0
                    } else {
                        (have_bytes as f64 / f.length as f64).clamp(0.0, 1.0)
                    };
                    (Some(f.length), Some(f.name.clone()), Some(progress))
                })
            })
            .unwrap_or_default();

        let paused = handle.is_paused();
        let name = handle
            .name()
            .or_else(|| files.first().map(|f| f.name.clone()))
            .unwrap_or_else(|| info_hash_hex.clone());

        Some(Statistics {
            name,
            info_hash: info_hash_hex,
            files,
            sources,
            opts,
            download_speed,
            upload_speed,
            downloaded: tstats.progress_bytes,
            uploaded: tstats.uploaded_bytes,
            unchoked: peers,
            peers,
            queued,
            unique,
            connection_tries,
            peer_search_running: !tstats.finished && !paused,
            stream_len,
            stream_name,
            stream_progress,
            swarm_connections,
            swarm_paused: paused,
            swarm_size,
        })
    }
}

fn build_magnet_with_trackers(info_hash: &[u8; 20], trackers: &[String]) -> String {
    let mut uri = format!("magnet:?xt=urn:btih:{}", hex::encode(info_hash));
    for tr in trackers {
        uri.push_str("&tr=");
        uri.push_str(&utf8_percent_encode(tr, NON_ALPHANUMERIC).to_string());
    }
    uri
}

pub struct StreamEngine;

impl StreamEngine {
    pub async fn start(config: StreamEngineConfig) -> Result<StreamEngineHandle, StreamEngineError> {
        let ram_cache_dir = allocate_ram_cache_dir_path();
        let session_root = match config.storage_mode {
            StorageMode::DiskLru { .. } => {
                ensure_dir_0700(&config.cache_dir).await?;
                config.cache_dir.clone()
            }
            StorageMode::RamOnly { .. } => {
                ensure_dir_0700(&ram_cache_dir).await?;
                ram_cache_dir.clone()
            }
        };

        let persistence_cfg = match config.storage_mode {
            StorageMode::DiskLru { .. } => {
                let persist_dir = config.cache_dir.join(".rqbit_session");
                ensure_dir_0700(&persist_dir).await?;
                Some(SessionPersistenceConfig::Json {
                    folder: Some(persist_dir),
                })
            }
            StorageMode::RamOnly { .. } => None,
        };

        let shutdown_token = CancellationToken::new();

        let mut session_trackers = HashSet::new();
        for tr in &config.extra_trackers {
            if is_allowed_tracker_url(tr) {
                if let Ok(parsed) = Url::parse(tr) {
                    session_trackers.insert(parsed);
                }
            }
        }

        let listen_opts = config.enable_peer_listener.then(|| ListenerOptions {
            mode: ListenerMode::TcpAndUtp,
            listen_addr: SocketAddr::new(
                IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                config.listen_port.unwrap_or(0),
            ),
            enable_upnp_port_forwarding: config.enable_upnp,
            ipv4_only: true,
            ..Default::default()
        });

        let dht_opts = config.enable_dht.then_some(DhtSessionConfig {
            bootstrap_addrs: None,
            port: None,
            persistence: None,
        });

        let session_opts = SessionOptions {
            dht: dht_opts,
            disable_trackers: !config.enable_trackers,
            disable_local_service_discovery: !config.enable_lsd,
            fastresume: config.storage_mode.is_disk_lru(),
            persistence: persistence_cfg,
            listen: listen_opts,
            trackers: session_trackers,
            peer_limit: Some(55),
            runtime_worker_threads: Some(8),
            concurrent_init_limit: Some(4),
            cancellation_token: Some(shutdown_token.child_token()),
            ..Default::default()
        };

        let session = Session::new_with_opts(session_root.clone(), session_opts)
            .await
            .map_err(|e| StreamEngineError::Torrent(format!("Failed starting librqbit: {e:#}")))?;

        let cache_tracker = Arc::new(CacheStateTracker::new(
            config.storage_mode,
            config.cache_dir.clone(),
            ram_cache_dir,
        ));

        if config.storage_mode.is_disk_lru() {
            let restored_handles: Vec<Arc<ManagedTorrent>> = session
                .with_torrents(|it| it.map(|(_, h)| h.clone()).collect());
            if !restored_handles.is_empty() {
                let backup_dir = config.cache_dir.join(".torrents");
                let _ = ensure_dir_0700(&backup_dir).await;
                for h in &restored_handles {
                    let _ = h.wait_until_initialized().await;
                    if !h.is_paused() {
                        let _ = session.pause(h).await;
                    }
                    let ih = h.info_hash().0;
                    let hex_ih = hex::encode(ih);
                    if cache_tracker.location_of(&ih).is_none() {
                        cache_tracker.record_or_touch(
                            ih,
                            TorrentStorageLocation::Disk,
                            config.cache_dir.join(&hex_ih),
                            h.name(),
                            h.stats().progress_bytes,
                            h.only_files().and_then(|of| of.first().copied()),
                        );
                    }
                    if let Ok(raw_bytes) = h.with_metadata(|m| m.torrent_bytes.clone()) {
                        if !raw_bytes.is_empty() {
                            let torrent_file = backup_dir.join(format!("{hex_ih}.torrent"));
                            if tokio::fs::write(&torrent_file, &raw_bytes).await.is_ok() {
                                set_file_0600(&torrent_file).await;
                            }
                        }
                    }
                    let rqbit_bitv = config
                        .cache_dir
                        .join(".rqbit_session")
                        .join(format!("{hex_ih}.bitv"));
                    if rqbit_bitv.exists() {
                        let dst_bitv = backup_dir.join(format!("{hex_ih}.bitv"));
                        if tokio::fs::copy(&rqbit_bitv, &dst_bitv).await.is_ok() {
                            set_file_0600(&dst_bitv).await;
                        }
                    }
                    if h.with_metadata(|_| ()).is_ok() {
                        let _ = session
                            .delete(TorrentIdOrHash::Hash(Id20::new(ih)), false)
                            .await;
                    }
                }
                cache_tracker.persist_disk_lru_index().await;
            }
        }

        let listener = TcpListener::bind(config.bind_addr).await?;
        let local_addr = listener.local_addr()?;
        let base_url = Url::parse(&format!("http://{local_addr}/"))?;

        let cache_root_str = session_root.to_string_lossy().into_owned();
        let reported_cache_size =
            Some(config.storage_mode.max_bytes() as f64).or(config.cache_size);
        let initial_settings = Settings {
            app_path: cache_root_str.clone(),
            cache_root: cache_root_str,
            server_version: SERVER_VERSION.to_owned(),
            remote_https: None,
            transcode_profile: None,
            cache_size: reported_cache_size,
            proxy_streams_enabled: false,
            bt_max_connections: 55,
            bt_handshake_timeout: 20_000,
            bt_request_timeout: 4_000,
            bt_download_speed_soft_limit: 2_621_440.0,
            bt_download_speed_hard_limit: 39_321_600.0,
            bt_min_peers_for_stable: 5,
        };

        let http_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| StreamEngineError::Torrent(format!("Failed building HTTP client: {e}")))?;

        let scheduler = Arc::new(PlayheadScheduler::new());
        let upnp_flag = config.enable_upnp;
        let first_byte_to = config.first_byte_timeout;
        let runtime_handle = tokio::runtime::Handle::current();

        let inner = Arc::new(EngineInner {
            config,
            runtime_handle,
            base_url,
            local_addr,
            session,
            scheduler,
            cache_tracker,
            settings: RwLock::new(initial_settings),
            upnp_enabled: AtomicBool::new(upnp_flag),
            first_byte_timeout_lock: RwLock::new(first_byte_to),
            eviction_lock: tokio::sync::Mutex::new(()),
            http_client,
            shutdown_token: shutdown_token.clone(),
            registered_magnets: DashMap::new(),
        });

        let _ = inner.enforce_cache_quota_excluding(None).await;

        {
            let maint_inner = Arc::clone(&inner);
            let maint_cancel = shutdown_token.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_millis(400));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        _ = maint_cancel.cancelled() => break,
                        _ = maint_inner.scheduler.cursor_closed().notified() => {
                            let _ = maint_inner.enforce_cache_quota_excluding(None).await;
                        }
                        _ = ticker.tick() => {
                            let _ = maint_inner.enforce_cache_quota_excluding(None).await;
                        }
                    }
                }
            });
        }

        let router = routes::build_router(Arc::clone(&inner));
        let server_cancel = shutdown_token.clone();
        let server_task = tokio::spawn(async move {
            let res = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    server_cancel.cancelled().await;
                })
                .await;
            if let Err(err) = res {
                tracing::error!("StreamEngine HTTP server error: {err}");
            }
        });

        info!(
            base_url = %inner.base_url,
            storage_mode = ?inner.cache_tracker.mode(),
            upnp = inner.enable_upnp(),
            peer_listen_addr = ?inner.session.listen_addr(),
            "StreamEngine started"
        );

        Ok(StreamEngineHandle {
            inner,
            server_task: Arc::new(Mutex::new(Some(server_task))),
        })
    }
}

fn map_torrent_init_err(info_hash: &[u8; 20], e: impl std::fmt::Display) -> routes::ApiHttpError {
    routes::ApiHttpError::bad_gateway(format!(
        "Failed initializing torrent {}: {e:#}",
        hex::encode(info_hash)
    ))
}

#[derive(Clone)]
pub struct StreamEngineHandle {
    inner: Arc<EngineInner>,
    server_task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl StreamEngineHandle {
    pub fn base_url(&self) -> Url {
        self.inner.base_url.clone()
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.inner.local_addr
    }

    pub fn listen_addr(&self) -> SocketAddr {
        self.inner.local_addr
    }

    pub fn peer_listen_addr(&self) -> Option<SocketAddr> {
        self.inner.session.listen_addr()
    }

    pub fn enable_upnp(&self) -> bool {
        self.inner.enable_upnp()
    }

    pub fn set_enable_upnp(&self, enabled: bool) {
        self.inner.set_enable_upnp(enabled);
    }

    pub fn storage_mode(&self) -> StorageMode {
        self.inner.cache_tracker.mode()
    }

    pub async fn set_storage_mode(&self, mode: StorageMode) -> Result<(), StreamEngineError> {
        self.inner.switch_storage_mode(mode).await
    }

    pub fn cache_usage_bytes(&self) -> u64 {
        let live_snapshots: Vec<([u8; 20], u64, Option<String>)> =
            self.inner.session.with_torrents(|it| {
                it.map(|(_, h)| (h.info_hash().0, h.stats().progress_bytes, h.name()))
                    .collect()
            });
        for (ih, bytes, name) in live_snapshots {
            self.inner.cache_tracker.update_size_bytes(&ih, bytes, name);
        }
        self.inner.cache_tracker.active_mode_usage_bytes()
    }

    pub async fn enforce_cache_quota(&self) -> usize {
        self.inner.enforce_cache_quota_excluding(None).await
    }

    pub fn cache_dir(&self) -> PathBuf {
        PathBuf::from(&self.inner.settings.read().cache_root)
    }

    pub fn ram_cache_dir(&self) -> PathBuf {
        self.inner.cache_tracker.ram_cache_dir().to_path_buf()
    }

    pub fn first_byte_timeout(&self) -> Duration {
        self.inner.first_byte_timeout()
    }

    pub fn set_first_byte_timeout(&self, timeout: Duration) {
        self.inner.set_first_byte_timeout(timeout);
    }

    pub async fn probe_or_failover_streams(
        &self,
        candidates: &[StreamCandidate],
        per_candidate_timeout: Duration,
    ) -> Result<ResolvedStream, StreamEngineError> {
        failover::execute_probe_or_failover(&self.inner, candidates, per_candidate_timeout).await
    }

    pub async fn prewarm_stream(
        &self,
        stream_url: &Url,
        bytes_to_warm: u64,
    ) -> Result<u64, StreamEngineError> {
        validate_outbound_http_url(stream_url).map_err(StreamEngineError::Torrent)?;
        if bytes_to_warm == 0 {
            return Ok(0);
        }
        let redacted = redact_url(stream_url);
        let end_inclusive = bytes_to_warm.saturating_sub(1);
        let resp = self
            .inner
            .http_client
            .get(stream_url.as_str())
            .header(http::header::RANGE, format!("bytes=0-{end_inclusive}"))
            .send()
            .await
            .map_err(|e| {
                StreamEngineError::Torrent(format!(
                    "Prewarm request to {redacted} failed: {}",
                    e.without_url()
                ))
            })?;

        if !resp.status().is_success() {
            return Err(StreamEngineError::Torrent(format!(
                "Prewarm {redacted} returned HTTP {}",
                resp.status()
            )));
        }

        let mut total = 0u64;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                StreamEngineError::Torrent(format!(
                    "Prewarm read from {redacted} failed: {}",
                    e.without_url()
                ))
            })?;
            total = total.saturating_add(chunk.len() as u64);
            if total >= bytes_to_warm {
                break;
            }
        }
        Ok(total)
    }

    /// Pre-buffers a single episode or season-pack file without interrupting an active stream.
    pub async fn prebuffer_episode(
        &self,
        info_hash_hex: &str,
        file_idx: Option<usize>,
        trackers: Vec<String>,
    ) -> Result<String, StreamEngineError> {
        let info_hash =
            validate_info_hash_str(info_hash_hex).map_err(StreamEngineError::Torrent)?;
        let normalized_trackers = routes::extract_trackers_from_sources(&trackers);
        let handle = self
            .inner
            .ensure_torrent_resolved(info_hash, normalized_trackers)
            .await
            .map_err(|e| {
                if e.status == axum::http::StatusCode::GATEWAY_TIMEOUT {
                    StreamEngineError::MetadataTimeout(self.inner.config.metadata_timeout)
                } else {
                    StreamEngineError::Torrent(e.message)
                }
            })?;

        tokio::time::timeout(
            self.inner.config.metadata_timeout,
            handle.wait_until_initialized(),
        )
        .await
        .map_err(|_| StreamEngineError::MetadataTimeout(self.inner.config.metadata_timeout))?
        .map_err(|e| StreamEngineError::Torrent(format!("torrent init failed: {e:#}")))?;

        let requested_idx = match file_idx {
            Some(idx) => i32::try_from(idx).map_err(|_| {
                StreamEngineError::Torrent(format!("file_idx {idx} exceeds supported range"))
            })?,
            None => -1,
        };
        let (resolved_idx, file_len) = handle
            .with_metadata(|meta| {
                let idx = routes::select_best_file_index(&meta.file_infos, requested_idx, &[])
                    .map_err(|e| StreamEngineError::Torrent(e.message))?;
                Ok::<_, StreamEngineError>((idx, meta.file_infos[idx].len))
            })
            .map_err(|e| StreamEngineError::Torrent(format!("{e:#}")))??;

        let tstats = handle.stats();
        let file_have = tstats.file_progress.get(resolved_idx).copied().unwrap_or(0);
        if handle.is_paused() && file_have < file_len {
            let _ = self.inner.session.unpause(&handle).await;
        }

        self.inner
            .scheduler
            .prebuffer_episode(&self.inner.session, info_hash, &handle, resolved_idx)
            .await?;

        let default_loc = if self.inner.cache_tracker.mode().is_ram_only() {
            TorrentStorageLocation::Ram
        } else {
            TorrentStorageLocation::Disk
        };
        let loc = self
            .inner
            .cache_tracker
            .location_of(&info_hash)
            .unwrap_or(default_loc);
        let out_folder = self
            .inner
            .cache_tracker
            .output_folder_of(&info_hash)
            .unwrap_or_else(|| {
                self.inner
                    .cache_tracker
                    .output_folder_for_new_torrent(&info_hash)
                    .0
            });
        let tracked_idx = self
            .inner
            .scheduler
            .selected_file(&info_hash)
            .or(Some(resolved_idx));
        self.inner.cache_tracker.record_or_touch(
            info_hash,
            loc,
            out_folder,
            handle.name(),
            tstats.progress_bytes,
            tracked_idx,
        );
        self.inner
            .persist_torrent_metadata_backup(&info_hash, &handle)
            .await;

        let stream_url = self
            .inner
            .base_url
            .join(&format!("{}/{resolved_idx}", hex::encode(info_hash)))?;
        Ok(stream_url.into())
    }

    pub fn session(&self) -> &Arc<Session> {
        &self.inner.session
    }

    pub fn scheduler(&self) -> &Arc<PlayheadScheduler> {
        &self.inner.scheduler
    }

    pub fn torrent_stats(&self, info_hash: &str, file_idx: Option<usize>) -> Option<Statistics> {
        let parsed = validate_info_hash_str(info_hash).ok()?;
        self.inner.build_statistics(&parsed, file_idx)
    }

    pub fn active_torrents(&self) -> Vec<Statistics> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let session_hashes: Vec<[u8; 20]> = self
            .inner
            .session
            .with_torrents(|iter| iter.map(|(_, handle)| handle.info_hash().0).collect());
        for ih in session_hashes {
            if seen.insert(ih) {
                let selected_file = self.inner.scheduler.selected_file(&ih);
                if let Some(stats) = self.inner.build_statistics(&ih, selected_file) {
                    out.push(stats);
                }
            }
        }
        for entry in self.inner.registered_magnets.iter() {
            let ih = *entry.key();
            if seen.insert(ih) {
                if let Some(stats) = self.inner.build_statistics(&ih, None) {
                    out.push(stats);
                }
            }
        }
        out
    }

    pub fn engine_stats(&self) -> EngineStatsSnapshot {
        let torrents = self.active_torrents();
        let mut snap = EngineStatsSnapshot {
            active_torrents: torrents.len(),
            ..Default::default()
        };
        for t in torrents {
            snap.download_speed_bytes += t.download_speed;
            snap.upload_speed_bytes += t.upload_speed;
            snap.downloaded_bytes = snap.downloaded_bytes.saturating_add(t.downloaded);
            snap.uploaded_bytes = snap.uploaded_bytes.saturating_add(t.uploaded);
            snap.connected_peers = snap.connected_peers.saturating_add(t.peers);
        }
        snap
    }

    pub fn stats(&self) -> EngineStatsSnapshot {
        self.engine_stats()
    }

    pub async fn release_playback_sessions(&self) {
        self.inner.release_playback_sessions().await;
    }

    pub fn release_playback_sessions_bg(&self) {
        let inner = Arc::clone(&self.inner);
        self.inner.runtime_handle.spawn(async move {
            inner.release_playback_sessions().await;
        });
    }

    pub async fn release_playback_resources(&self) {
        self.release_playback_sessions().await;
    }

    pub fn release_playback_resources_bg(&self) {
        self.release_playback_sessions_bg();
    }

    pub async fn add_torrent_bytes(
        &self,
        torrent_bytes: Vec<u8>,
        trackers: Vec<String>,
        initial_peers: Option<Vec<SocketAddr>>,
    ) -> Result<InfoHash, StreamEngineError> {
        let ih = self
            .inner
            .add_torrent_bytes(torrent_bytes, trackers, initial_peers)
            .await?;
        Ok(InfoHash::new(ih))
    }

    pub async fn stop(&self) {
        self.inner.scheduler.shutdown();
        self.inner.shutdown_token.cancel();

        let ram_hashes = self
            .inner
            .cache_tracker
            .tracked_hashes_by_location(TorrentStorageLocation::Ram);
        for ih in ram_hashes {
            self.inner.evict_torrent_completely(ih).await;
        }

        let handles: Vec<Arc<ManagedTorrent>> = self
            .inner
            .session
            .with_torrents(|iter| iter.map(|(_, h)| h.clone()).collect());
        for h in handles {
            let ih = h.info_hash().0;
            self.inner
                .cache_tracker
                .update_size_bytes(&ih, h.stats().progress_bytes, h.name());
            let _ = self.inner.session.pause(&h).await;
        }

        if self.inner.cache_tracker.mode().is_disk_lru() {
            self.inner.cache_tracker.persist_disk_lru_index().await;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        self.inner.session.cancellation_token().cancel();

        let ram_dir = self.inner.cache_tracker.ram_cache_dir();
        if ram_dir.exists() && is_safe_path_no_traversal(ram_dir) {
            let _ = tokio::fs::remove_dir_all(ram_dir).await;
        }

        let task = self.server_task.lock().take();
        if let Some(join_handle) = task {
            let _ = tokio::time::timeout(Duration::from_secs(3), join_handle).await;
        }
    }

    pub async fn shutdown(self) {
        self.stop().await;
    }
}
