use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, RwLock,
    },
    time::{Duration, Instant},
};

use chrono::DateTime;
use data_encoding::BASE32_NOPAD;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::{
    private_config::{redact_secret, LocalDebridProvider},
    stream_parser::is_allowed_stream_url,
};

pub const DEFAULT_REAL_DEBRID_BASE_URL: &str = "https://api.real-debrid.com/rest/1.0";
pub const DEFAULT_TORBOX_BASE_URL: &str = "https://api.torbox.app/v1/api";
pub const DEFAULT_ALLDEBRID_BASE_URL: &str = "https://api.alldebrid.com";

pub const RESOLVED_URL_CACHE_TTL: Duration = Duration::from_secs(15 * 60);
pub const AVAILABILITY_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const DEBRID_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const DEBRID_CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const DEBRID_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "m4v", "ts", "m2ts", "webm", "flv", "mpg", "mpeg",
];

static RE_SAMPLE_WORD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[\s._\-\[\]()/])(?:sample|trailer|featurette|rarbg\.com)(?:$|[\s._\-\[\]()/])")
        .unwrap()
});

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebridAccountStatus {
    pub provider: LocalDebridProvider,
    pub username: String,
    pub premium: bool,
    pub expiration: Option<String>,
    pub latency_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DebridCacheStatus {
    CachedInstant,
    UncachedOrUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedDebridStream {
    pub url: Url,
    pub filename: Option<String>,
    pub provider: LocalDebridProvider,
    pub latency_ms: u64,
    pub from_memory_cache: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebridBaseUrls {
    pub real_debrid: Url,
    pub torbox: Url,
    pub alldebrid: Url,
}

impl Default for DebridBaseUrls {
    fn default() -> Self {
        Self {
            real_debrid: Url::parse(DEFAULT_REAL_DEBRID_BASE_URL)
                .expect("default Real-Debrid URL is valid"),
            torbox: Url::parse(DEFAULT_TORBOX_BASE_URL).expect("default TorBox URL is valid"),
            alldebrid: Url::parse(DEFAULT_ALLDEBRID_BASE_URL)
                .expect("default AllDebrid URL is valid"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TorrentFileCandidate {
    pub id: String,
    pub index: Option<usize>,
    pub path: String,
    pub size_bytes: u64,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ResolveCacheKey {
    provider: LocalDebridProvider,
    info_hash: [u8; 20],
    file_idx: Option<u32>,
    season_episode: Option<(u32, u32)>,
}

#[derive(Clone, Debug)]
struct CachedResolvedEntry {
    stream: ResolvedDebridStream,
    expires_at: Instant,
}

#[derive(Clone, Debug)]
struct CachedAvailabilityEntry {
    status: DebridCacheStatus,
    expires_at: Instant,
}

pub struct LocalDebridResolver {
    client: reqwest::Client,
    base_urls: RwLock<DebridBaseUrls>,
    resolved_cache: Mutex<HashMap<ResolveCacheKey, CachedResolvedEntry>>,
    availability_cache: Mutex<HashMap<(LocalDebridProvider, [u8; 20]), CachedAvailabilityEntry>>,
    last_resolve_latency_ms: AtomicU64,
}

impl Default for LocalDebridResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for LocalDebridResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalDebridResolver")
            .field("last_resolve_latency_ms", &self.last_resolve_latency_ms())
            .finish()
    }
}

impl LocalDebridResolver {
    pub fn new() -> Self {
        Self::with_base_urls(DebridBaseUrls::default())
    }

    pub fn with_base_urls(base_urls: DebridBaseUrls) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(format!("Ferrite/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(DEBRID_CONNECT_TIMEOUT)
            .timeout(DEBRID_REQUEST_TIMEOUT)
            .tcp_nodelay(true)
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .pool_max_idle_per_host(8)
            .pool_idle_timeout(DEBRID_POOL_IDLE_TIMEOUT)
            .http2_adaptive_window(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            base_urls: RwLock::new(base_urls),
            resolved_cache: Mutex::new(HashMap::new()),
            availability_cache: Mutex::new(HashMap::new()),
            last_resolve_latency_ms: AtomicU64::new(u64::MAX),
        }
    }

    pub fn set_base_urls(&self, base_urls: DebridBaseUrls) {
        if let Ok(mut guard) = self.base_urls.write() {
            *guard = base_urls;
        }
        self.clear_caches();
    }

    pub fn base_urls(&self) -> DebridBaseUrls {
        self.base_urls
            .read()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    pub fn clear_caches(&self) {
        if let Ok(mut guard) = self.resolved_cache.lock() {
            guard.clear();
        }
        if let Ok(mut guard) = self.availability_cache.lock() {
            guard.clear();
        }
    }

    pub fn last_resolve_latency_ms(&self) -> Option<u64> {
        let raw = self.last_resolve_latency_ms.load(Ordering::Relaxed);
        (raw != u64::MAX).then_some(raw)
    }

    fn record_resolve_latency(&self, latency_ms: u64) {
        self.last_resolve_latency_ms
            .store(latency_ms, Ordering::Relaxed);
    }

    pub async fn verify_account(
        &self,
        provider: LocalDebridProvider,
        api_key: &str,
    ) -> Result<DebridAccountStatus, String> {
        let key = api_key.trim();
        if !provider.is_enabled() {
            return Err("Select a local Debrid provider first".to_owned());
        }
        if key.is_empty() {
            return Err(format!("{} API key is empty", provider.label()));
        }

        let secrets = [key];
        let bases = self.base_urls();
        let started = Instant::now();

        let res = match provider {
            LocalDebridProvider::None => unreachable!(),
            LocalDebridProvider::RealDebrid => {
                self.verify_real_debrid(&bases.real_debrid, key, started)
                    .await
            }
            LocalDebridProvider::TorBox => self.verify_torbox(&bases.torbox, key, started).await,
            LocalDebridProvider::AllDebrid => {
                self.verify_alldebrid(&bases.alldebrid, key, started).await
            }
        };

        res.map_err(|err| redact_secret(&err, &secrets))
    }

    pub async fn check_cache(
        &self,
        provider: LocalDebridProvider,
        api_key: &str,
        info_hashes: &[[u8; 20]],
    ) -> HashMap<[u8; 20], DebridCacheStatus> {
        let mut out = HashMap::with_capacity(info_hashes.len());
        let key = api_key.trim();
        if !provider.is_enabled() || key.is_empty() || info_hashes.is_empty() {
            return out;
        }

        let now = Instant::now();
        let mut missing = Vec::new();

        if let Ok(mut guard) = self.availability_cache.lock() {
            guard.retain(|_, v| v.expires_at > now);
            for hash in info_hashes {
                if let Some(entry) = guard.get(&(provider, *hash)) {
                    out.insert(*hash, entry.status);
                } else if !missing.contains(hash) {
                    missing.push(*hash);
                }
            }
        } else {
            missing.extend_from_slice(info_hashes);
        }

        if missing.is_empty() {
            return out;
        }

        let bases = self.base_urls();
        let fetched = match provider {
            LocalDebridProvider::None => HashMap::new(),
            LocalDebridProvider::RealDebrid => {
                self.check_real_debrid_cache(&bases.real_debrid, key, &missing)
                    .await
            }
            LocalDebridProvider::TorBox => {
                self.check_torbox_cache(&bases.torbox, key, &missing).await
            }
            LocalDebridProvider::AllDebrid => {
                self.check_alldebrid_cache(&bases.alldebrid, key, &missing)
                    .await
            }
        };

        let expires_at = Instant::now() + AVAILABILITY_CACHE_TTL;
        if let Ok(mut guard) = self.availability_cache.lock() {
            for hash in &missing {
                let status = fetched
                    .get(hash)
                    .copied()
                    .unwrap_or(DebridCacheStatus::UncachedOrUnknown);
                guard.insert((provider, *hash), CachedAvailabilityEntry { status, expires_at });
                out.insert(*hash, status);
            }
        } else {
            for hash in missing {
                let status = fetched
                    .get(&hash)
                    .copied()
                    .unwrap_or(DebridCacheStatus::UncachedOrUnknown);
                out.insert(hash, status);
            }
        }

        out
    }

    pub async fn resolve_torrent(
        &self,
        provider: LocalDebridProvider,
        api_key: &str,
        info_hash: [u8; 20],
        file_idx: Option<u32>,
        announce: &[String],
        season_episode: Option<(u32, u32)>,
    ) -> Result<ResolvedDebridStream, String> {
        let key = api_key.trim();
        if !provider.is_enabled() {
            return Err("No local Debrid provider is enabled".to_owned());
        }
        if key.is_empty() {
            return Err(format!("{} API key is not configured", provider.label()));
        }

        let cache_key = ResolveCacheKey {
            provider,
            info_hash,
            file_idx,
            season_episode,
        };

        if let Ok(mut guard) = self.resolved_cache.lock() {
            let now = Instant::now();
            guard.retain(|_, v| v.expires_at > now);
            if let Some(hit) = guard.get(&cache_key) {
                let mut stream = hit.stream.clone();
                stream.from_memory_cache = true;
                stream.latency_ms = 0;
                return Ok(stream);
            }
        }

        let secrets = [key];
        let magnet = build_canonical_magnet(info_hash, announce, None);
        let bases = self.base_urls();
        let started = Instant::now();

        let resolved = match provider {
            LocalDebridProvider::None => unreachable!(),
            LocalDebridProvider::RealDebrid => {
                self.resolve_real_debrid(
                    &bases.real_debrid,
                    key,
                    &magnet,
                    file_idx,
                    season_episode,
                    started,
                )
                .await
            }
            LocalDebridProvider::TorBox => {
                self.resolve_torbox(
                    &bases.torbox,
                    key,
                    &magnet,
                    info_hash,
                    file_idx,
                    season_episode,
                    started,
                )
                .await
            }
            LocalDebridProvider::AllDebrid => {
                self.resolve_alldebrid(
                    &bases.alldebrid,
                    key,
                    &magnet,
                    file_idx,
                    season_episode,
                    started,
                )
                .await
            }
        }
        .map_err(|err| redact_secret(&err, &secrets))?;

        self.record_resolve_latency(resolved.latency_ms);

        if let Ok(mut guard) = self.resolved_cache.lock() {
            guard.insert(
                cache_key,
                CachedResolvedEntry {
                    stream: resolved.clone(),
                    expires_at: Instant::now() + RESOLVED_URL_CACHE_TTL,
                },
            );
        }

        if let Ok(mut guard) = self.availability_cache.lock() {
            guard.insert(
                (provider, info_hash),
                CachedAvailabilityEntry {
                    status: DebridCacheStatus::CachedInstant,
                    expires_at: Instant::now() + AVAILABILITY_CACHE_TTL,
                },
            );
        }

        Ok(resolved)
    }

    async fn verify_real_debrid(
        &self,
        base: &Url,
        api_key: &str,
        started: Instant,
    ) -> Result<DebridAccountStatus, String> {
        let url = join_api_path(base, "user")?;
        let resp = self
            .client
            .get(url)
            .bearer_auth(api_key)
            .send()
            .await
            .map_err(|e| format!("Real-Debrid account request failed: {e}"))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("Real-Debrid account response invalid (HTTP {status}): {e}"))?;

        if !status.is_success() {
            let msg = extract_api_error(&body).unwrap_or_else(|| format!("HTTP {status}"));
            return Err(format!("Real-Debrid verification failed: {msg}"));
        }

        let username = body
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or("Real-Debrid User")
            .to_owned();
        let account_type = body.get("type").and_then(Value::as_str).unwrap_or("");
        let premium_secs = body.get("premium").and_then(Value::as_u64).unwrap_or(0);
        let premium = account_type.eq_ignore_ascii_case("premium") || premium_secs > 0;
        let expiration = body
            .get("expiration")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned);

        Ok(DebridAccountStatus {
            provider: LocalDebridProvider::RealDebrid,
            username,
            premium,
            expiration,
            latency_ms: started.elapsed().as_millis() as u64,
        })
    }

    async fn verify_torbox(
        &self,
        base: &Url,
        api_key: &str,
        started: Instant,
    ) -> Result<DebridAccountStatus, String> {
        let url = join_api_path(base, "user/me")?;
        let resp = self
            .client
            .get(url)
            .bearer_auth(api_key)
            .send()
            .await
            .map_err(|e| format!("TorBox account request failed: {e}"))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("TorBox account response invalid (HTTP {status}): {e}"))?;

        if !status.is_success() || body.get("success").and_then(Value::as_bool) == Some(false) {
            let msg = extract_api_error(&body).unwrap_or_else(|| format!("HTTP {status}"));
            return Err(format!("TorBox verification failed: {msg}"));
        }

        let data = body.get("data").unwrap_or(&body);
        let username = data
            .get("email")
            .and_then(Value::as_str)
            .or_else(|| data.get("username").and_then(Value::as_str))
            .map(str::to_owned)
            .unwrap_or_else(|| "TorBox User".to_owned());
        let is_subscribed = data
            .get("is_subscribed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let plan = data.get("plan").and_then(Value::as_u64).unwrap_or(0);
        let premium = is_subscribed || plan > 0;
        let expiration = data
            .get("premium_expires_at")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned);

        Ok(DebridAccountStatus {
            provider: LocalDebridProvider::TorBox,
            username,
            premium,
            expiration,
            latency_ms: started.elapsed().as_millis() as u64,
        })
    }

    async fn verify_alldebrid(
        &self,
        base: &Url,
        api_key: &str,
        started: Instant,
    ) -> Result<DebridAccountStatus, String> {
        let mut url = join_api_path(base, "v4/user")?;
        url.query_pairs_mut().append_pair("agent", "Ferrite");
        let resp = self
            .client
            .get(url)
            .bearer_auth(api_key)
            .send()
            .await
            .map_err(|e| format!("AllDebrid account request failed: {e}"))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("AllDebrid account response invalid (HTTP {status}): {e}"))?;

        if !status.is_success()
            || body.get("status").and_then(Value::as_str) == Some("error")
        {
            let msg = extract_api_error(&body).unwrap_or_else(|| format!("HTTP {status}"));
            return Err(format!("AllDebrid verification failed: {msg}"));
        }

        let user = body
            .get("data")
            .and_then(|d| d.get("user"))
            .unwrap_or(&body);
        let username = user
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or("AllDebrid User")
            .to_owned();
        let premium = user
            .get("isPremium")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let expiration = user
            .get("premiumUntil")
            .and_then(Value::as_i64)
            .filter(|&ts| ts > 0)
            .and_then(|ts| DateTime::from_timestamp(ts, 0))
            .map(|dt| dt.to_rfc3339());

        Ok(DebridAccountStatus {
            provider: LocalDebridProvider::AllDebrid,
            username,
            premium,
            expiration,
            latency_ms: started.elapsed().as_millis() as u64,
        })
    }

    async fn check_real_debrid_cache(
        &self,
        base: &Url,
        api_key: &str,
        hashes: &[[u8; 20]],
    ) -> HashMap<[u8; 20], DebridCacheStatus> {
        let mut out = HashMap::new();
        for chunk in hashes.chunks(40) {
            let joined = chunk
                .iter()
                .map(hex::encode)
                .collect::<Vec<_>>()
                .join("/");
            let Ok(url) = join_api_path(base, &format!("torrents/instantAvailability/{joined}"))
            else {
                continue;
            };
            let Ok(resp) = self.client.get(url).bearer_auth(api_key).send().await else {
                continue;
            };
            if !resp.status().is_success() {
                // Graceful fallback on 403 / disabled endpoint.
                continue;
            }
            let Ok(body) = resp.json::<Value>().await else {
                continue;
            };
            if let Some(map) = body.as_object() {
                for (hash_hex, hosters) in map {
                    let Ok(hash) = parse_info_hash(hash_hex) else {
                        continue;
                    };
                    let cached = match hosters.as_object() {
                        Some(hoster_map) => hoster_map.values().any(|arr| {
                            arr.as_array().is_some_and(|variants| !variants.is_empty())
                        }),
                        None => false,
                    };
                    out.insert(
                        hash,
                        if cached {
                            DebridCacheStatus::CachedInstant
                        } else {
                            DebridCacheStatus::UncachedOrUnknown
                        },
                    );
                }
            }
        }
        out
    }

    async fn check_torbox_cache(
        &self,
        base: &Url,
        api_key: &str,
        hashes: &[[u8; 20]],
    ) -> HashMap<[u8; 20], DebridCacheStatus> {
        let mut out = HashMap::new();
        for chunk in hashes.chunks(50) {
            let joined = chunk
                .iter()
                .map(hex::encode)
                .collect::<Vec<_>>()
                .join(",");
            let Ok(mut url) = join_api_path(base, "torrents/checkcached") else {
                continue;
            };
            url.query_pairs_mut()
                .append_pair("hash", &joined)
                .append_pair("format", "object");
            let Ok(resp) = self.client.get(url).bearer_auth(api_key).send().await else {
                continue;
            };
            if !resp.status().is_success() {
                continue;
            }
            let Ok(body) = resp.json::<Value>().await else {
                continue;
            };
            if body.get("success").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            let Some(data) = body.get("data") else {
                continue;
            };
            if let Some(map) = data.as_object() {
                for (hash_hex, entry) in map {
                    if entry.is_null() || entry.as_bool() == Some(false) {
                        continue;
                    }
                    if let Ok(hash) = parse_info_hash(hash_hex) {
                        out.insert(hash, DebridCacheStatus::CachedInstant);
                    }
                }
            } else if let Some(arr) = data.as_array() {
                for item in arr {
                    if let Some(hash_hex) = item.get("hash").and_then(Value::as_str) {
                        if let Ok(hash) = parse_info_hash(hash_hex) {
                            out.insert(hash, DebridCacheStatus::CachedInstant);
                        }
                    }
                }
            }
        }
        out
    }

    async fn check_alldebrid_cache(
        &self,
        base: &Url,
        api_key: &str,
        hashes: &[[u8; 20]],
    ) -> HashMap<[u8; 20], DebridCacheStatus> {
        let mut out = HashMap::new();
        for chunk in hashes.chunks(40) {
            let Ok(mut url) = join_api_path(base, "v4/magnet/instant") else {
                continue;
            };
            {
                let mut pairs = url.query_pairs_mut();
                pairs.append_pair("agent", "Ferrite");
                for h in chunk {
                    pairs.append_pair("magnets[]", &hex::encode(h));
                }
            }
            let Ok(resp) = self.client.get(url).bearer_auth(api_key).send().await else {
                continue;
            };
            if !resp.status().is_success() {
                continue;
            }
            let Ok(body) = resp.json::<Value>().await else {
                continue;
            };
            if body.get("status").and_then(Value::as_str) == Some("error") {
                continue;
            }
            if let Some(magnets) = body
                .get("data")
                .and_then(|d| d.get("magnets"))
                .and_then(Value::as_array)
            {
                for item in magnets {
                    let instant = item
                        .get("instant")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let hash_str = item
                        .get("hash")
                        .and_then(Value::as_str)
                        .or_else(|| item.get("magnet").and_then(Value::as_str));
                    if let Some(raw_hash) = hash_str {
                        if let Ok(hash) = parse_info_hash(raw_hash) {
                            out.insert(
                                hash,
                                if instant {
                                    DebridCacheStatus::CachedInstant
                                } else {
                                    DebridCacheStatus::UncachedOrUnknown
                                },
                            );
                        }
                    }
                }
            }
        }
        out
    }

    async fn resolve_real_debrid(
        &self,
        base: &Url,
        api_key: &str,
        magnet: &str,
        file_idx: Option<u32>,
        season_episode: Option<(u32, u32)>,
        started: Instant,
    ) -> Result<ResolvedDebridStream, String> {
        let add_url = join_api_path(base, "torrents/addMagnet")?;
        let add_resp = self
            .client
            .post(add_url)
            .bearer_auth(api_key)
            .form(&[("magnet", magnet)])
            .send()
            .await
            .map_err(|e| format!("Real-Debrid addMagnet failed: {e}"))?;
        let add_status = add_resp.status();
        let add_body: Value = add_resp
            .json()
            .await
            .map_err(|e| format!("Real-Debrid addMagnet response invalid: {e}"))?;
        if !add_status.is_success() {
            let msg = extract_api_error(&add_body).unwrap_or_else(|| format!("HTTP {add_status}"));
            return Err(format!("Real-Debrid addMagnet error: {msg}"));
        }

        let torrent_id = add_body
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                add_body
                    .get("id")
                    .and_then(Value::as_u64)
                    .map(|n| n.to_string())
            })
            .ok_or_else(|| "Real-Debrid addMagnet missing torrent id".to_owned())?;

        let info_url = join_api_path(base, &format!("torrents/info/{torrent_id}"))?;
        let info_body = self.rd_get_torrent_info(&info_url, api_key).await?;

        let status_str = info_body
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();

        if matches!(
            status_str.as_str(),
            "magnet_error" | "error" | "virus" | "dead" | "queued" | "downloading"
        ) {
            self.rd_delete_torrent(base, api_key, &torrent_id).await;
            return Err(format!(
                "Torrent is not instantly cached on Real-Debrid (status: {status_str})"
            ));
        }

        let files = parse_rd_files(&info_body);
        let selected_candidate = select_best_video_file(&files, file_idx, season_episode)
            .cloned();

        let final_info = if status_str == "waiting_files_selection"
            || (info_body
                .get("links")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty)
                && !files.is_empty())
        {
            let chosen = selected_candidate.as_ref().ok_or_else(|| {
                "No playable video file found in torrent on Real-Debrid".to_owned()
            })?;

            let select_url = join_api_path(base, &format!("torrents/selectFiles/{torrent_id}"))?;
            let select_resp = self
                .client
                .post(select_url)
                .bearer_auth(api_key)
                .form(&[("files", chosen.id.as_str())])
                .send()
                .await
                .map_err(|e| format!("Real-Debrid selectFiles failed: {e}"))?;
            if !select_resp.status().is_success() {
                let st = select_resp.status();
                self.rd_delete_torrent(base, api_key, &torrent_id).await;
                return Err(format!("Real-Debrid selectFiles failed (HTTP {st})"));
            }

            self.rd_get_torrent_info(&info_url, api_key).await?
        } else {
            info_body
        };

        let final_status = final_info
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let links = final_info
            .get("links")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if final_status != "downloaded" || links.is_empty() {
            self.rd_delete_torrent(base, api_key, &torrent_id).await;
            let state_desc = if final_status.is_empty() {
                "uncached"
            } else {
                final_status.as_str()
            };
            return Err(format!(
                "Torrent is not cached on Real-Debrid (status: {state_desc})"
            ));
        }

        let target_link = pick_rd_link(&final_info, &links, selected_candidate.as_ref())
            .ok_or_else(|| "Real-Debrid returned no downloadable link".to_owned())?;

        let unrestrict_url = join_api_path(base, "unrestrict/link")?;
        let unrestrict_resp = self
            .client
            .post(unrestrict_url)
            .bearer_auth(api_key)
            .form(&[("link", target_link.as_str())])
            .send()
            .await
            .map_err(|e| format!("Real-Debrid unrestrict failed: {e}"))?;
        let unrestrict_status = unrestrict_resp.status();
        let unrestrict_body: Value = unrestrict_resp
            .json()
            .await
            .map_err(|e| format!("Real-Debrid unrestrict response invalid: {e}"))?;
        if !unrestrict_status.is_success() {
            let msg = extract_api_error(&unrestrict_body)
                .unwrap_or_else(|| format!("HTTP {unrestrict_status}"));
            return Err(format!("Real-Debrid unrestrict error: {msg}"));
        }

        let download_raw = unrestrict_body
            .get("download")
            .and_then(Value::as_str)
            .ok_or_else(|| "Real-Debrid unrestrict response missing 'download' URL".to_owned())?;
        let validated_url = validate_resolved_stream_url(download_raw)?;
        let filename = unrestrict_body
            .get("filename")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| {
                selected_candidate
                    .as_ref()
                    .and_then(|c| basename_owned(&c.path))
            });

        Ok(ResolvedDebridStream {
            url: validated_url,
            filename,
            provider: LocalDebridProvider::RealDebrid,
            latency_ms: started.elapsed().as_millis() as u64,
            from_memory_cache: false,
        })
    }

    async fn rd_get_torrent_info(&self, info_url: &Url, api_key: &str) -> Result<Value, String> {
        let resp = self
            .client
            .get(info_url.clone())
            .bearer_auth(api_key)
            .send()
            .await
            .map_err(|e| format!("Real-Debrid torrent info failed: {e}"))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("Real-Debrid torrent info invalid: {e}"))?;
        if !status.is_success() {
            let msg = extract_api_error(&body).unwrap_or_else(|| format!("HTTP {status}"));
            return Err(format!("Real-Debrid torrent info error: {msg}"));
        }
        Ok(body)
    }

    async fn rd_delete_torrent(&self, base: &Url, api_key: &str, torrent_id: &str) {
        if let Ok(url) = join_api_path(base, &format!("torrents/delete/{torrent_id}")) {
            let _ = self.client.delete(url).bearer_auth(api_key).send().await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn resolve_torbox(
        &self,
        base: &Url,
        api_key: &str,
        magnet: &str,
        info_hash: [u8; 20],
        file_idx: Option<u32>,
        season_episode: Option<(u32, u32)>,
        started: Instant,
    ) -> Result<ResolvedDebridStream, String> {
        let create_url = join_api_path(base, "torrents/createtorrent")?;
        let create_resp = self
            .client
            .post(create_url)
            .bearer_auth(api_key)
            .form(&[("magnet", magnet), ("add_only_if_cached", "true")])
            .send()
            .await
            .map_err(|e| format!("TorBox createtorrent failed: {e}"))?;
        let create_status = create_resp.status();
        let create_body: Value = create_resp
            .json()
            .await
            .map_err(|e| format!("TorBox createtorrent response invalid: {e}"))?;

        if !create_status.is_success()
            || create_body.get("success").and_then(Value::as_bool) == Some(false)
        {
            let msg = extract_api_error(&create_body)
                .unwrap_or_else(|| format!("HTTP {create_status}"));
            return Err(format!(
                "Torrent is not cached on TorBox (or create failed): {msg}"
            ));
        }

        let data = create_body.get("data").unwrap_or(&create_body);
        let torrent_id = data
            .get("torrent_id")
            .and_then(value_to_id_string)
            .or_else(|| data.get("id").and_then(value_to_id_string))
            .ok_or_else(|| "Torrent is not cached on TorBox (missing torrent_id)".to_owned())?;

        let mut list_url = join_api_path(base, "torrents/mylist")?;
        list_url.query_pairs_mut().append_pair("id", &torrent_id);
        let list_resp = self
            .client
            .get(list_url)
            .bearer_auth(api_key)
            .send()
            .await
            .map_err(|e| format!("TorBox mylist failed: {e}"))?;
        let list_status = list_resp.status();
        let list_body: Value = list_resp
            .json()
            .await
            .map_err(|e| format!("TorBox mylist response invalid: {e}"))?;

        if !list_status.is_success()
            || list_body.get("success").and_then(Value::as_bool) == Some(false)
        {
            let msg =
                extract_api_error(&list_body).unwrap_or_else(|| format!("HTTP {list_status}"));
            return Err(format!("TorBox mylist error: {msg}"));
        }

        let target_hex = hex::encode(info_hash);
        let torrent_obj = find_torbox_torrent(&list_body, &torrent_id, &target_hex)
            .ok_or_else(|| "Torrent not found in TorBox library".to_owned())?;

        let files = parse_torbox_files(torrent_obj);
        let chosen = select_best_video_file(&files, file_idx, season_episode)
            .ok_or_else(|| "No playable video file found in TorBox torrent".to_owned())?;

        let mut dl_url = join_api_path(base, "torrents/requestdl")?;
        dl_url
            .query_pairs_mut()
            .append_pair("token", api_key)
            .append_pair("torrent_id", &torrent_id)
            .append_pair("file_id", &chosen.id)
            .append_pair("zip_link", "false");

        let dl_resp = self
            .client
            .get(dl_url)
            .bearer_auth(api_key)
            .send()
            .await
            .map_err(|e| format!("TorBox requestdl failed: {e}"))?;
        let dl_status = dl_resp.status();
        let dl_body: Value = dl_resp
            .json()
            .await
            .map_err(|e| format!("TorBox requestdl response invalid: {e}"))?;

        if !dl_status.is_success()
            || dl_body.get("success").and_then(Value::as_bool) == Some(false)
        {
            let msg = extract_api_error(&dl_body).unwrap_or_else(|| format!("HTTP {dl_status}"));
            return Err(format!("TorBox requestdl error: {msg}"));
        }

        let download_raw = dl_body
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| "TorBox requestdl response missing download URL".to_owned())?;
        let validated_url = validate_resolved_stream_url(download_raw)?;

        Ok(ResolvedDebridStream {
            url: validated_url,
            filename: basename_owned(&chosen.path),
            provider: LocalDebridProvider::TorBox,
            latency_ms: started.elapsed().as_millis() as u64,
            from_memory_cache: false,
        })
    }

    async fn resolve_alldebrid(
        &self,
        base: &Url,
        api_key: &str,
        magnet: &str,
        file_idx: Option<u32>,
        season_episode: Option<(u32, u32)>,
        started: Instant,
    ) -> Result<ResolvedDebridStream, String> {
        let mut upload_url = join_api_path(base, "v4/magnet/upload")?;
        upload_url.query_pairs_mut().append_pair("agent", "Ferrite");
        let upload_resp = self
            .client
            .post(upload_url)
            .bearer_auth(api_key)
            .form(&[("magnets[]", magnet)])
            .send()
            .await
            .map_err(|e| format!("AllDebrid magnet/upload failed: {e}"))?;
        let upload_status = upload_resp.status();
        let upload_body: Value = upload_resp
            .json()
            .await
            .map_err(|e| format!("AllDebrid magnet/upload response invalid: {e}"))?;

        if !upload_status.is_success()
            || upload_body.get("status").and_then(Value::as_str) == Some("error")
        {
            let msg = extract_api_error(&upload_body)
                .unwrap_or_else(|| format!("HTTP {upload_status}"));
            return Err(format!("AllDebrid magnet upload failed: {msg}"));
        }

        let magnet_entry = upload_body
            .get("data")
            .and_then(|d| d.get("magnets"))
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .ok_or_else(|| "AllDebrid magnet/upload returned empty magnets list".to_owned())?;

        if let Some(err_obj) = magnet_entry.get("error") {
            let msg = extract_api_error(err_obj).unwrap_or_else(|| "upload error".to_owned());
            return Err(format!("AllDebrid magnet upload rejected: {msg}"));
        }

        let magnet_id = magnet_entry
            .get("id")
            .and_then(value_to_id_string)
            .ok_or_else(|| "AllDebrid magnet/upload missing magnet id".to_owned())?;

        let mut status_url = join_api_path(base, "v4.1/magnet/status")?;
        status_url
            .query_pairs_mut()
            .append_pair("agent", "Ferrite")
            .append_pair("id", &magnet_id);

        let status_resp = self
            .client
            .get(status_url)
            .bearer_auth(api_key)
            .send()
            .await
            .map_err(|e| format!("AllDebrid magnet/status failed: {e}"))?;
        let status_http = status_resp.status();
        let status_body: Value = status_resp
            .json()
            .await
            .map_err(|e| format!("AllDebrid magnet/status response invalid: {e}"))?;

        if !status_http.is_success()
            || status_body.get("status").and_then(Value::as_str) == Some("error")
        {
            self.ad_delete_magnet(base, api_key, &magnet_id).await;
            let msg =
                extract_api_error(&status_body).unwrap_or_else(|| format!("HTTP {status_http}"));
            return Err(format!("AllDebrid magnet status error: {msg}"));
        }

        let magnet_status_obj = status_body
            .get("data")
            .and_then(|d| d.get("magnets"))
            .and_then(|m| {
                if m.is_array() {
                    m.as_array().and_then(|a| a.first())
                } else {
                    Some(m)
                }
            })
            .ok_or_else(|| "AllDebrid magnet/status missing magnet payload".to_owned())?;

        let status_code = magnet_status_obj
            .get("statusCode")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if status_code != 4 {
            self.ad_delete_magnet(base, api_key, &magnet_id).await;
            let status_label = magnet_status_obj
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("Uncached");
            return Err(format!(
                "Torrent is not cached on AllDebrid (status: {status_label}, code {status_code})"
            ));
        }

        let (files, link_by_id) = parse_alldebrid_files(magnet_status_obj);
        let chosen = select_best_video_file(&files, file_idx, season_episode)
            .ok_or_else(|| "No playable video file found in AllDebrid magnet".to_owned())?;
        let hoster_link = link_by_id
            .get(&chosen.id)
            .ok_or_else(|| "Selected AllDebrid file is missing a hoster link".to_owned())?;

        let mut unlock_url = join_api_path(base, "v4/link/unlock")?;
        unlock_url
            .query_pairs_mut()
            .append_pair("agent", "Ferrite")
            .append_pair("link", hoster_link);

        let unlock_resp = self
            .client
            .get(unlock_url)
            .bearer_auth(api_key)
            .send()
            .await
            .map_err(|e| format!("AllDebrid link/unlock failed: {e}"))?;
        let unlock_http = unlock_resp.status();
        let unlock_body: Value = unlock_resp
            .json()
            .await
            .map_err(|e| format!("AllDebrid link/unlock response invalid: {e}"))?;

        if !unlock_http.is_success()
            || unlock_body.get("status").and_then(Value::as_str) == Some("error")
        {
            let msg =
                extract_api_error(&unlock_body).unwrap_or_else(|| format!("HTTP {unlock_http}"));
            return Err(format!("AllDebrid link/unlock error: {msg}"));
        }

        let data = unlock_body.get("data").unwrap_or(&unlock_body);
        let download_raw = data
            .get("link")
            .and_then(Value::as_str)
            .ok_or_else(|| "AllDebrid link/unlock response missing 'link' URL".to_owned())?;
        let validated_url = validate_resolved_stream_url(download_raw)?;
        let filename = data
            .get("filename")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| basename_owned(&chosen.path));

        Ok(ResolvedDebridStream {
            url: validated_url,
            filename,
            provider: LocalDebridProvider::AllDebrid,
            latency_ms: started.elapsed().as_millis() as u64,
            from_memory_cache: false,
        })
    }

    async fn ad_delete_magnet(&self, base: &Url, api_key: &str, magnet_id: &str) {
        if let Ok(mut url) = join_api_path(base, "v4/magnet/delete") {
            url.query_pairs_mut()
                .append_pair("agent", "Ferrite")
                .append_pair("id", magnet_id);
            let _ = self.client.get(url).bearer_auth(api_key).send().await;
        }
    }
}

/// Parses a 40-char hex hash, 32-char base32 hash, or `magnet:?xt=urn:btih:...` URI into 20 bytes.
pub fn parse_info_hash(raw: &str) -> Result<[u8; 20], String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Empty infohash or magnet URI".to_owned());
    }

    if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("magnet:") {
        let parsed = Url::parse(trimmed).map_err(|e| format!("Invalid magnet URI: {e}"))?;
        for (k, v) in parsed.query_pairs() {
            if k.eq_ignore_ascii_case("xt") {
                if let Some(hash_part) = strip_prefix_ignore_ascii_case(v.as_ref(), "urn:btih:") {
                    return decode_hash_token(hash_part);
                }
            }
        }
        return Err("Magnet URI does not contain a btih infohash".to_owned());
    }

    if let Some(hash_part) = strip_prefix_ignore_ascii_case(trimmed, "urn:btih:") {
        return decode_hash_token(hash_part);
    }

    decode_hash_token(trimmed)
}

/// Extracts `tr=` tracker announce URLs from a magnet URI.
pub fn extract_magnet_trackers(magnet: &str) -> Vec<String> {
    let Ok(parsed) = Url::parse(magnet.trim()) else {
        return Vec::new();
    };
    let mut trackers = Vec::new();
    let mut seen = HashSet::new();
    for (k, v) in parsed.query_pairs() {
        if k.eq_ignore_ascii_case("tr") {
            let tr = v.trim();
            if !tr.is_empty() && !tr.chars().any(char::is_control) && seen.insert(tr.to_owned()) {
                trackers.push(tr.to_owned());
            }
        }
    }
    trackers
}

/// Builds a canonical `magnet:?xt=urn:btih:<hex>` URI with optional display name and trackers.
pub fn build_canonical_magnet(
    info_hash: [u8; 20],
    announce: &[String],
    display_name: Option<&str>,
) -> String {
    let mut out = format!("magnet:?xt=urn:btih:{}", hex::encode(info_hash));
    if let Some(name) = display_name.map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str("&dn=");
        out.push_str(&urlencoding::encode(name));
    }
    let mut seen = HashSet::new();
    for tr in announce {
        let cleaned = tr.trim();
        if cleaned.is_empty() || cleaned.chars().any(char::is_control) {
            continue;
        }
        if seen.insert(cleaned.to_owned()) {
            out.push_str("&tr=");
            out.push_str(&urlencoding::encode(cleaned));
        }
    }
    out
}

/// Selects the best video file from a torrent's file list using explicit `file_idx`,
/// `SxxExx` episode matching, sample exclusion, and largest-file fallback.
pub fn select_best_video_file(
    files: &[TorrentFileCandidate],
    file_idx: Option<u32>,
    season_episode: Option<(u32, u32)>,
) -> Option<&TorrentFileCandidate> {
    if files.is_empty() {
        return None;
    }

    if let Some(idx) = file_idx {
        let idx_usize = idx as usize;
        if let Some(by_index) = files
            .iter()
            .find(|f| f.index == Some(idx_usize) && is_video_filename(&f.path))
        {
            return Some(by_index);
        }
        if let Some(by_id) = files
            .iter()
            .find(|f| f.id == idx.to_string() && is_video_filename(&f.path))
        {
            return Some(by_id);
        }
    }

    let video_files: Vec<&TorrentFileCandidate> = files
        .iter()
        .filter(|f| is_video_filename(&f.path))
        .collect();

    if video_files.is_empty() {
        if let Some(idx) = file_idx {
            if let Some(by_index) = files.iter().find(|f| f.index == Some(idx as usize)) {
                return Some(by_index);
            }
        }
        return files.iter().max_by_key(|f| f.size_bytes);
    }

    let non_sample: Vec<&TorrentFileCandidate> = video_files
        .iter()
        .copied()
        .filter(|f| !is_sample_or_extra(&f.path))
        .collect();
    let pool = if non_sample.is_empty() {
        video_files
    } else {
        non_sample
    };

    if let Some((season, episode)) = season_episode {
        if let Some(best_ep) = pool
            .iter()
            .copied()
            .filter(|f| matches_season_episode(&f.path, season, episode))
            .max_by_key(|f| f.size_bytes)
        {
            return Some(best_ep);
        }
    }

    pool.into_iter().max_by_key(|f| f.size_bytes)
}

/// Validates a resolved Debrid playback URL (`https://` in production; loopback `http://` allowed for tests).
pub fn validate_resolved_stream_url(raw: &str) -> Result<Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') || trimmed.chars().any(char::is_control) {
        return Err("Debrid provider returned an unsafe or empty stream URL".to_owned());
    }
    let parsed =
        Url::parse(trimmed).map_err(|e| format!("Debrid provider returned invalid URL: {e}"))?;
    if !is_allowed_stream_url(&parsed) {
        return Err("Debrid provider returned a disallowed stream URL".to_owned());
    }
    match parsed.scheme().to_ascii_lowercase().as_str() {
        "https" => Ok(parsed),
        "http" if is_loopback_host(&parsed) => Ok(parsed),
        other => Err(format!(
            "Debrid provider returned non-HTTPS stream scheme '{other}'"
        )),
    }
}

fn is_loopback_host(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let clean = host.trim_start_matches('[').trim_end_matches(']');
    clean
        .parse::<IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

fn decode_hash_token(token: &str) -> Result<[u8; 20], String> {
    let clean = token.trim();
    if clean.len() == 40 && clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        let bytes = hex::decode(clean).map_err(|e| format!("Invalid hex infohash: {e}"))?;
        let arr: [u8; 20] = bytes
            .try_into()
            .map_err(|_| "Hex infohash did not decode to 20 bytes".to_owned())?;
        return Ok(arr);
    }
    if clean.len() == 32 {
        let upper = clean.to_ascii_uppercase();
        if let Ok(bytes) = BASE32_NOPAD.decode(upper.as_bytes()) {
            if let Ok(arr) = <[u8; 20]>::try_from(bytes.as_slice()) {
                return Ok(arr);
            }
        }
    }
    Err("Infohash must be 40-char hex or 32-char base32".to_owned())
}

fn strip_prefix_ignore_ascii_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

fn join_api_path(base: &Url, rel: &str) -> Result<Url, String> {
    let mut base_str = base.as_str().trim_end_matches('/').to_owned();
    base_str.push('/');
    base_str.push_str(rel.trim_start_matches('/'));
    Url::parse(&base_str).map_err(|e| format!("Invalid API endpoint URL: {e}"))
}

fn is_video_filename(path: &str) -> bool {
    let ext = path
        .rsplit('.')
        .next()
        .map(|s| s.trim().to_ascii_lowercase());
    ext.is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.as_str()))
}

fn is_sample_or_extra(path: &str) -> bool {
    RE_SAMPLE_WORD.is_match(path)
}

fn matches_season_episode(path: &str, season: u32, episode: u32) -> bool {
    let pattern = format!(
        r"(?i)(?:^|[^a-z0-9])(?:s0*{season}\s*[._\-]?\s*e0*{episode}|{season}x0*{episode}|season\s*0*{season}\s*episode\s*0*{episode})(?:$|[^0-9])"
    );
    Regex::new(&pattern).is_ok_and(|re| re.is_match(path))
}

fn basename_owned(path: &str) -> Option<String> {
    path.rsplit(['/', '\\'])
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn extract_api_error(val: &Value) -> Option<String> {
    for key in ["error", "detail", "message"] {
        if let Some(s) = val.get(key).and_then(Value::as_str) {
            if !s.trim().is_empty() {
                return Some(s.trim().to_owned());
            }
        }
        if let Some(msg) = val
            .get(key)
            .and_then(|obj| obj.get("message"))
            .and_then(Value::as_str)
        {
            if !msg.trim().is_empty() {
                return Some(msg.trim().to_owned());
            }
        }
    }
    None
}

fn value_to_id_string(val: &Value) -> Option<String> {
    val.as_str()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| val.as_u64().map(|n| n.to_string()))
        .or_else(|| val.as_i64().map(|n| n.to_string()))
}

fn parse_rd_files(info: &Value) -> Vec<TorrentFileCandidate> {
    let Some(arr) = info.get("files").and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .enumerate()
        .filter_map(|(pos, item)| {
            let id = item.get("id").and_then(value_to_id_string)?;
            let path = item
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let size_bytes = item.get("bytes").and_then(Value::as_u64).unwrap_or(0);
            let selected = item.get("selected").and_then(Value::as_u64).unwrap_or(0) == 1;
            Some(TorrentFileCandidate {
                id,
                index: Some(pos),
                path,
                size_bytes,
                selected,
            })
        })
        .collect()
}

fn pick_rd_link(
    info: &Value,
    links: &[String],
    selected: Option<&TorrentFileCandidate>,
) -> Option<String> {
    if links.len() == 1 {
        return links.first().cloned();
    }
    if let Some(chosen) = selected {
        let files = parse_rd_files(info);
        let selected_files: Vec<&TorrentFileCandidate> =
            files.iter().filter(|f| f.selected).collect();
        if let Some(pos) = selected_files.iter().position(|f| f.id == chosen.id) {
            if let Some(link) = links.get(pos) {
                return Some(link.clone());
            }
        }
    }
    links.first().cloned()
}

fn find_torbox_torrent<'a>(
    list_body: &'a Value,
    torrent_id: &str,
    target_hex: &str,
) -> Option<&'a Value> {
    let data = list_body.get("data")?;
    if let Some(arr) = data.as_array() {
        arr.iter()
            .find(|item| {
                item.get("id")
                    .and_then(value_to_id_string)
                    .is_some_and(|id| id == torrent_id)
            })
            .or_else(|| {
                arr.iter().find(|item| {
                    item.get("hash")
                        .and_then(Value::as_str)
                        .is_some_and(|h| h.eq_ignore_ascii_case(target_hex))
                })
            })
            .or_else(|| arr.first())
    } else if data.is_object() {
        Some(data)
    } else {
        None
    }
}

fn parse_torbox_files(torrent_obj: &Value) -> Vec<TorrentFileCandidate> {
    let Some(arr) = torrent_obj.get("files").and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .enumerate()
        .map(|(pos, item)| {
            let id = item
                .get("id")
                .and_then(value_to_id_string)
                .unwrap_or_else(|| pos.to_string());
            let path = item
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| item.get("short_name").and_then(Value::as_str))
                .unwrap_or("")
                .to_owned();
            let size_bytes = item.get("size").and_then(Value::as_u64).unwrap_or(0);
            TorrentFileCandidate {
                id,
                index: Some(pos),
                path,
                size_bytes,
                selected: true,
            }
        })
        .collect()
}

fn parse_alldebrid_files(
    magnet_obj: &Value,
) -> (Vec<TorrentFileCandidate>, HashMap<String, String>) {
    let mut candidates = Vec::new();
    let mut link_map = HashMap::new();

    fn walk_nodes(
        nodes: &[Value],
        prefix: &str,
        candidates: &mut Vec<TorrentFileCandidate>,
        link_map: &mut HashMap<String, String>,
    ) {
        for node in nodes {
            let name = node
                .get("n")
                .and_then(Value::as_str)
                .or_else(|| node.get("filename").and_then(Value::as_str))
                .unwrap_or("");
            let full_path = if prefix.is_empty() {
                name.to_owned()
            } else if name.is_empty() {
                prefix.to_owned()
            } else {
                format!("{prefix}/{name}")
            };

            if let Some(children) = node.get("e").and_then(Value::as_array) {
                walk_nodes(children, &full_path, candidates, link_map);
            }

            if let Some(link) = node
                .get("l")
                .and_then(Value::as_str)
                .or_else(|| node.get("link").and_then(Value::as_str))
            {
                let idx = candidates.len();
                let id = idx.to_string();
                let size_bytes = node
                    .get("s")
                    .and_then(Value::as_u64)
                    .or_else(|| node.get("size").and_then(Value::as_u64))
                    .unwrap_or(0);
                candidates.push(TorrentFileCandidate {
                    id: id.clone(),
                    index: Some(idx),
                    path: full_path,
                    size_bytes,
                    selected: true,
                });
                link_map.insert(id, link.to_owned());
            }
        }
    }

    if let Some(files) = magnet_obj.get("files").and_then(Value::as_array) {
        walk_nodes(files, "", &mut candidates, &mut link_map);
    }
    if candidates.is_empty() {
        if let Some(links) = magnet_obj.get("links").and_then(Value::as_array) {
            walk_nodes(links, "", &mut candidates, &mut link_map);
        }
    }

    (candidates, link_map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_base32_and_magnet_hashes_and_builds_canonical_magnet() {
        let hex_hash = "0123456789abcdef0123456789abcdef01234567";
        let bytes = parse_info_hash(hex_hash).unwrap();
        assert_eq!(hex::encode(bytes), hex_hash);

        let b32 = BASE32_NOPAD.encode(&bytes);
        assert_eq!(b32.len(), 32);
        assert_eq!(parse_info_hash(&b32.to_ascii_lowercase()).unwrap(), bytes);

        let magnet = build_canonical_magnet(
            bytes,
            &[
                "udp://tracker.opentrackr.org:1337/announce".to_owned(),
                "udp://tracker.opentrackr.org:1337/announce".to_owned(),
            ],
            Some("Test Movie 2160p"),
        );
        assert!(magnet.starts_with(&format!("magnet:?xt=urn:btih:{hex_hash}")));
        assert_eq!(parse_info_hash(&magnet).unwrap(), bytes);
        assert_eq!(extract_magnet_trackers(&magnet).len(), 1);
    }

    #[test]
    fn selects_best_video_file_with_episode_and_sample_filtering() {
        let files = vec![
            TorrentFileCandidate {
                id: "1".to_owned(),
                index: Some(0),
                path: "/Show/Sample/Show.S01E02.sample.mkv".to_owned(),
                size_bytes: 50_000_000,
                selected: false,
            },
            TorrentFileCandidate {
                id: "2".to_owned(),
                index: Some(1),
                path: "/Show/Show.S01E01.1080p.mkv".to_owned(),
                size_bytes: 2_500_000_000,
                selected: false,
            },
            TorrentFileCandidate {
                id: "3".to_owned(),
                index: Some(2),
                path: "/Show/Show.S01E02.1080p.mkv".to_owned(),
                size_bytes: 1_900_000_000,
                selected: false,
            },
            TorrentFileCandidate {
                id: "4".to_owned(),
                index: Some(3),
                path: "/Show/Show.S01E02.srt".to_owned(),
                size_bytes: 90_000,
                selected: false,
            },
        ];

        let ep2 = select_best_video_file(&files, None, Some((1, 2))).unwrap();
        assert_eq!(ep2.id, "3");

        let by_idx = select_best_video_file(&files, Some(1), Some((1, 2))).unwrap();
        assert_eq!(by_idx.id, "2");

        let largest = select_best_video_file(&files, None, None).unwrap();
        assert_eq!(largest.id, "2");
    }

    #[test]
    fn validates_https_and_loopback_urls_only() {
        assert!(validate_resolved_stream_url("https://cdn.debrid.example/dl/movie.mkv").is_ok());
        assert!(validate_resolved_stream_url("http://127.0.0.1:8080/dl/movie.mkv").is_ok());
        assert!(validate_resolved_stream_url("http://cdn.debrid.example/dl/movie.mkv").is_err());
        assert!(validate_resolved_stream_url("file:///etc/passwd").is_err());
    }
}
