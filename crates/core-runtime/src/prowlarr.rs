use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use roxmltree::Document;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use stremio_core::types::resource::{Stream, StreamBehaviorHints, StreamSource};
use url::Url;

use crate::{
    debrid::{extract_magnet_trackers, parse_info_hash},
    private_config::{redact_secret, MAX_PROWLARR_TIMEOUT_MS, MIN_PROWLARR_TIMEOUT_MS},
    stream_parser::{format_bytes, parse_stream_with_addon, ParsedStream},
};

const TORZNAB_NS: &str = "http://torznab.com/schemas/2015/feed";
const NEWZNAB_NS: &str = "http://www.newznab.com/DTD/2010/feeds/attributes/";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProwlarrTestResult {
    pub reachable: bool,
    pub mode_detected: String,
    pub item_count: usize,
    pub latency_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RichStreamItem {
    pub stream: Stream,
    pub parsed: ParsedStream,
}

#[derive(Clone, Debug)]
pub struct ProwlarrClient {
    client: reqwest::Client,
}

impl Default for ProwlarrClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ProwlarrClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(format!("Ferrite/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(3))
            .tcp_nodelay(true)
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .pool_max_idle_per_host(6)
            .pool_idle_timeout(Duration::from_secs(90))
            .http2_adaptive_window(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client }
    }

    pub async fn test_connection(
        &self,
        raw_url: &str,
        api_key: &str,
        timeout_ms: u64,
    ) -> Result<ProwlarrTestResult, String> {
        let base = parse_prowlarr_url(raw_url)?;
        let key = api_key.trim();
        let secrets = [key];
        let timeout = clamp_timeout(timeout_ms);
        let started = Instant::now();

        let res = if looks_like_torznab_url(&base) {
            self.test_torznab_caps(&base, key, timeout, started).await
        } else {
            match self.test_rest_v1(&base, key, timeout, started).await {
                Ok(ok) => Ok(ok),
                Err(RestProbeError::Auth(msg)) => Err(msg),
                Err(RestProbeError::NotRest) => {
                    self.test_torznab_caps(&base, key, timeout, started).await
                }
                Err(RestProbeError::Transport(msg)) => Err(msg),
            }
        };

        res.map_err(|err| redact_secret(&err, &secrets))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn search(
        &self,
        raw_url: &str,
        api_key: &str,
        timeout_ms: u64,
        content_type: &str,
        video_id: &str,
        title_hint: Option<&str>,
    ) -> Result<Vec<RichStreamItem>, String> {
        let base = parse_prowlarr_url(raw_url)?;
        let key = api_key.trim();
        let secrets = [key];
        let timeout = clamp_timeout(timeout_ms);
        let query_parts = VideoQueryParts::parse(content_type, video_id, title_hint);

        let res = if looks_like_torznab_url(&base) {
            self.search_torznab(&base, key, timeout, &query_parts).await
        } else {
            match self
                .search_rest_v1(&base, key, timeout, &query_parts)
                .await
            {
                Ok(items) => Ok(items),
                Err(RestProbeError::Auth(msg)) => Err(msg),
                Err(RestProbeError::NotRest) => {
                    self.search_torznab(&base, key, timeout, &query_parts).await
                }
                Err(RestProbeError::Transport(msg)) => Err(msg),
            }
        };

        res.map_err(|err| redact_secret(&err, &secrets))
    }

    async fn test_rest_v1(
        &self,
        base: &Url,
        api_key: &str,
        timeout: Duration,
        started: Instant,
    ) -> Result<ProwlarrTestResult, RestProbeError> {
        let url = append_path(base, "api/v1/indexer")
            .map_err(|e| RestProbeError::Transport(format!("Invalid Prowlarr URL: {e}")))?;

        let mut req = self.client.get(url).timeout(timeout);
        if !api_key.is_empty() {
            req = req.header("X-Api-Key", api_key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| RestProbeError::Transport(format!("Prowlarr connection failed: {e}")))?;
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(RestProbeError::Auth(format!(
                "Prowlarr authentication failed (HTTP {status})"
            )));
        }
        if status.as_u16() == 404 || status.as_u16() == 405 {
            return Err(RestProbeError::NotRest);
        }

        let body = resp
            .text()
            .await
            .map_err(|e| RestProbeError::Transport(format!("Failed reading Prowlarr response: {e}")))?;

        if body.trim_start().starts_with('<') {
            return Err(RestProbeError::NotRest);
        }

        if !status.is_success() {
            return Err(RestProbeError::Transport(format!(
                "Prowlarr returned HTTP {status}"
            )));
        }

        let parsed: Value = serde_json::from_str(&body)
            .map_err(|_| RestProbeError::NotRest)?;
        let item_count = parsed.as_array().map(Vec::len).unwrap_or(0);

        Ok(ProwlarrTestResult {
            reachable: true,
            mode_detected: "Prowlarr REST v1".to_owned(),
            item_count,
            latency_ms: started.elapsed().as_millis() as u64,
        })
    }

    async fn test_torznab_caps(
        &self,
        base: &Url,
        api_key: &str,
        timeout: Duration,
        started: Instant,
    ) -> Result<ProwlarrTestResult, String> {
        let mut url = base.clone();
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("t", "caps");
            if !api_key.is_empty() {
                pairs.append_pair("apikey", api_key);
            }
        }

        let mut req = self.client.get(url).timeout(timeout);
        if !api_key.is_empty() {
            req = req.header("X-Api-Key", api_key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| format!("Torznab connection failed: {e}"))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("Failed reading Torznab caps response: {e}"))?;

        if !status.is_success() {
            return Err(format!("Torznab caps returned HTTP {status}"));
        }

        let doc = Document::parse(&body)
            .map_err(|e| format!("Invalid Torznab caps XML: {e}"))?;
        if let Some(err_msg) = find_torznab_xml_error(&doc) {
            return Err(format!("Torznab error: {err_msg}"));
        }

        let root_name = doc.root_element().tag_name().name();
        if !root_name.eq_ignore_ascii_case("caps") {
            return Err(format!(
                "Unexpected Torznab caps root element <{root_name}>"
            ));
        }

        let category_count = doc
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name().eq_ignore_ascii_case("category"))
            .count();

        Ok(ProwlarrTestResult {
            reachable: true,
            mode_detected: "Torznab XML".to_owned(),
            item_count: category_count,
            latency_ms: started.elapsed().as_millis() as u64,
        })
    }

    async fn search_rest_v1(
        &self,
        base: &Url,
        api_key: &str,
        timeout: Duration,
        query_parts: &VideoQueryParts,
    ) -> Result<Vec<RichStreamItem>, RestProbeError> {
        let mut url = append_path(base, "api/v1/search")
            .map_err(|e| RestProbeError::Transport(format!("Invalid Prowlarr search URL: {e}")))?;

        let query_text = query_parts.text_query();
        if query_text.is_empty() {
            return Ok(Vec::new());
        }

        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("query", &query_text);
            pairs.append_pair("type", "search");
        }

        let mut req = self.client.get(url).timeout(timeout);
        if !api_key.is_empty() {
            req = req.header("X-Api-Key", api_key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| RestProbeError::Transport(format!("Prowlarr search failed: {e}")))?;
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(RestProbeError::Auth(format!(
                "Prowlarr authentication failed (HTTP {status})"
            )));
        }
        if status.as_u16() == 404 || status.as_u16() == 405 {
            return Err(RestProbeError::NotRest);
        }

        let body = resp
            .text()
            .await
            .map_err(|e| RestProbeError::Transport(format!("Failed reading Prowlarr search response: {e}")))?;
        if body.trim_start().starts_with('<') {
            return Err(RestProbeError::NotRest);
        }
        if !status.is_success() {
            return Err(RestProbeError::Transport(format!(
                "Prowlarr search returned HTTP {status}"
            )));
        }

        let json: Value = serde_json::from_str(&body)
            .map_err(|_| RestProbeError::NotRest)?;
        let Some(arr) = json.as_array() else {
            return Err(RestProbeError::NotRest);
        };

        let mut candidates = Vec::new();
        for item in arr {
            if let Some(parsed) = parse_prowlarr_json_release(item) {
                candidates.push(parsed);
            }
        }

        Ok(finalize_rich_streams(candidates))
    }

    async fn search_torznab(
        &self,
        base: &Url,
        api_key: &str,
        timeout: Duration,
        query_parts: &VideoQueryParts,
    ) -> Result<Vec<RichStreamItem>, String> {
        let mut primary_results = Vec::new();

        if let Some(imdb_id) = query_parts.imdb_id.as_deref() {
            let mut url = base.clone();
            {
                let mut pairs = url.query_pairs_mut();
                if query_parts.is_series() {
                    pairs.append_pair("t", "tvsearch");
                    pairs.append_pair("imdbid", imdb_id);
                    if let Some(s) = query_parts.season {
                        pairs.append_pair("season", &s.to_string());
                    }
                    if let Some(e) = query_parts.episode {
                        pairs.append_pair("ep", &e.to_string());
                    }
                } else {
                    pairs.append_pair("t", "movie");
                    pairs.append_pair("imdbid", imdb_id);
                }
                if !api_key.is_empty() {
                    pairs.append_pair("apikey", api_key);
                }
            }
            primary_results = self.fetch_torznab_feed(url, api_key, timeout).await?;
        }

        if primary_results.is_empty() {
            if let Some(fallback_q) = query_parts.fallback_search_query() {
                let mut url = base.clone();
                {
                    let mut pairs = url.query_pairs_mut();
                    pairs.append_pair("t", "search");
                    pairs.append_pair("q", &fallback_q);
                    if !api_key.is_empty() {
                        pairs.append_pair("apikey", api_key);
                    }
                }
                primary_results = self.fetch_torznab_feed(url, api_key, timeout).await?;
            }
        }

        Ok(finalize_rich_streams(primary_results))
    }

    async fn fetch_torznab_feed(
        &self,
        url: Url,
        api_key: &str,
        timeout: Duration,
    ) -> Result<Vec<ExtractedTorrentRelease>, String> {
        let mut req = self.client.get(url).timeout(timeout);
        if !api_key.is_empty() {
            req = req.header("X-Api-Key", api_key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("Torznab search request failed: {e}"))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("Failed reading Torznab XML feed: {e}"))?;

        if !status.is_success() {
            return Err(format!("Torznab search returned HTTP {status}"));
        }

        parse_torznab_xml_items(&body)
    }
}

enum RestProbeError {
    Auth(String),
    NotRest,
    Transport(String),
}

#[derive(Clone, Debug)]
struct VideoQueryParts {
    content_type: String,
    imdb_id: Option<String>,
    season: Option<u32>,
    episode: Option<u32>,
    raw_video_id: String,
    title_hint: Option<String>,
}

impl VideoQueryParts {
    fn parse(content_type: &str, video_id: &str, title_hint: Option<&str>) -> Self {
        let mut parts = video_id.trim().split(':');
        let head = parts.next().unwrap_or("").trim();
        let season = parts.next().and_then(|s| s.parse::<u32>().ok());
        let episode = parts.next().and_then(|s| s.parse::<u32>().ok());
        let imdb_id = if head.starts_with("tt") && head.len() > 2 {
            Some(head.to_owned())
        } else {
            None
        };
        let title_hint = title_hint
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        Self {
            content_type: content_type.trim().to_ascii_lowercase(),
            imdb_id,
            season,
            episode,
            raw_video_id: video_id.trim().to_owned(),
            title_hint,
        }
    }

    fn is_series(&self) -> bool {
        self.content_type == "series" || (self.season.is_some() && self.episode.is_some())
    }

    fn text_query(&self) -> String {
        if let Some(title) = self.title_hint.as_deref() {
            if let (Some(s), Some(e)) = (self.season, self.episode) {
                return format!("{title} S{s:02}E{e:02}");
            }
            return title.to_owned();
        }
        self.imdb_id
            .clone()
            .unwrap_or_else(|| self.raw_video_id.clone())
    }

    fn fallback_search_query(&self) -> Option<String> {
        if let Some(title) = self.title_hint.as_deref() {
            if let (Some(s), Some(e)) = (self.season, self.episode) {
                return Some(format!("{title} S{s:02}E{e:02}"));
            }
            return Some(title.to_owned());
        }
        if self.imdb_id.is_none() && !self.raw_video_id.is_empty() {
            return Some(self.raw_video_id.clone());
        }
        None
    }
}

#[derive(Clone, Debug)]
struct ExtractedTorrentRelease {
    title: String,
    info_hash: [u8; 20],
    announce: Vec<String>,
    seeders: Option<u32>,
    size_bytes: Option<u64>,
    indexer: Option<String>,
}

fn parse_prowlarr_url(raw_url: &str) -> Result<Url, String> {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') || trimmed.chars().any(char::is_control) {
        return Err("Prowlarr URL is empty or invalid".to_owned());
    }
    let parsed = Url::parse(trimmed).map_err(|e| format!("Invalid Prowlarr URL: {e}"))?;
    match parsed.scheme().to_ascii_lowercase().as_str() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "Prowlarr URL must use http:// or https:// (got {other})"
            ));
        }
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err("Prowlarr URL is missing host".to_owned());
    }
    Ok(parsed)
}

fn clamp_timeout(timeout_ms: u64) -> Duration {
    Duration::from_millis(timeout_ms.clamp(MIN_PROWLARR_TIMEOUT_MS, MAX_PROWLARR_TIMEOUT_MS))
}

fn looks_like_torznab_url(url: &Url) -> bool {
    let lower_path = url.path().to_ascii_lowercase();
    lower_path.contains("torznab")
        || lower_path.ends_with("/api")
        || url.query_pairs().any(|(k, _)| k.eq_ignore_ascii_case("t"))
}

fn append_path(base: &Url, rel: &str) -> Result<Url, url::ParseError> {
    let mut s = base.as_str().trim_end_matches('/').to_owned();
    s.push('/');
    s.push_str(rel.trim_start_matches('/'));
    Url::parse(&s)
}

fn parse_prowlarr_json_release(item: &Value) -> Option<ExtractedTorrentRelease> {
    let title = item
        .get("title")
        .and_then(Value::as_str)
        .or_else(|| item.get("sortTitle").and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_owned();

    let magnet_candidate = item
        .get("magnetUrl")
        .and_then(Value::as_str)
        .or_else(|| {
            item.get("guid")
                .and_then(Value::as_str)
                .filter(|g| g.trim_start().to_ascii_lowercase().starts_with("magnet:"))
        })
        .or_else(|| {
            item.get("downloadUrl")
                .and_then(Value::as_str)
                .filter(|d| d.trim_start().to_ascii_lowercase().starts_with("magnet:"))
        });

    let info_hash = item
        .get("infoHash")
        .and_then(Value::as_str)
        .and_then(|h| parse_info_hash(h).ok())
        .or_else(|| magnet_candidate.and_then(|m| parse_info_hash(m).ok()))?;

    let announce = magnet_candidate
        .map(extract_magnet_trackers)
        .unwrap_or_default();

    let seeders = item
        .get("seeders")
        .and_then(Value::as_u64)
        .map(|v| v.min(u32::MAX as u64) as u32);
    let size_bytes = item
        .get("size")
        .and_then(Value::as_u64)
        .filter(|&s| s > 0);
    let indexer = item
        .get("indexer")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    Some(ExtractedTorrentRelease {
        title,
        info_hash,
        announce,
        seeders,
        size_bytes,
        indexer,
    })
}

fn find_torznab_xml_error(doc: &Document<'_>) -> Option<String> {
    let root = doc.root_element();
    if root.tag_name().name().eq_ignore_ascii_case("error") {
        let code = root.attribute("code").unwrap_or("unknown");
        let desc = root
            .attribute("description")
            .unwrap_or("Torznab API error");
        return Some(format!("{desc} (code {code})"));
    }
    None
}

fn parse_torznab_xml_items(xml_body: &str) -> Result<Vec<ExtractedTorrentRelease>, String> {
    let doc = Document::parse(xml_body)
        .map_err(|e| format!("Failed to parse Torznab XML response: {e}"))?;
    if let Some(err_msg) = find_torznab_xml_error(&doc) {
        return Err(format!("Torznab error: {err_msg}"));
    }

    let mut out = Vec::new();
    for item_node in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name().eq_ignore_ascii_case("item"))
    {
        let mut title: Option<String> = None;
        let mut size_bytes: Option<u64> = None;
        let mut seeders: Option<u32> = None;
        let mut peers: Option<u32> = None;
        let mut indexer: Option<String> = None;
        let mut infohash_attr: Option<String> = None;
        let mut magnet_attr: Option<String> = None;
        let mut link_magnet: Option<String> = None;

        for child in item_node.children().filter(|c| c.is_element()) {
            let tag = child.tag_name().name();
            let ns = child.tag_name().namespace();

            if tag.eq_ignore_ascii_case("title") {
                title = child.text().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
            } else if tag.eq_ignore_ascii_case("size") {
                if let Some(sz) = child.text().and_then(|t| t.trim().parse::<u64>().ok()) {
                    if sz > 0 {
                        size_bytes = Some(sz);
                    }
                }
            } else if tag.eq_ignore_ascii_case("prowlarrindexer")
                || tag.eq_ignore_ascii_case("jackettindexer")
            {
                if let Some(idx_name) = child.text().map(str::trim).filter(|s| !s.is_empty()) {
                    indexer = Some(idx_name.to_owned());
                }
            } else if tag.eq_ignore_ascii_case("enclosure") {
                if let Some(len) = child.attribute("length").and_then(|v| v.trim().parse::<u64>().ok()) {
                    if len > 0 && size_bytes.is_none() {
                        size_bytes = Some(len);
                    }
                }
                if let Some(u) = child.attribute("url") {
                    if u.trim_start().to_ascii_lowercase().starts_with("magnet:") {
                        link_magnet = Some(u.trim().to_owned());
                    }
                }
            } else if (tag.eq_ignore_ascii_case("link") || tag.eq_ignore_ascii_case("guid"))
                && link_magnet.is_none()
            {
                if let Some(u) = child.text().map(str::trim) {
                    if u.to_ascii_lowercase().starts_with("magnet:") {
                        link_magnet = Some(u.to_owned());
                    }
                }
            } else if tag.eq_ignore_ascii_case("attr")
                && ns.is_none_or(|uri| uri == TORZNAB_NS || uri == NEWZNAB_NS || uri.contains("torznab") || uri.contains("newznab"))
            {
                let Some(attr_name) = child.attribute("name").map(str::trim) else {
                    continue;
                };
                let Some(attr_val) = child.attribute("value").map(str::trim) else {
                    continue;
                };
                if attr_name.eq_ignore_ascii_case("infohash") && !attr_val.is_empty() {
                    infohash_attr = Some(attr_val.to_owned());
                } else if attr_name.eq_ignore_ascii_case("magneturl") && !attr_val.is_empty() {
                    magnet_attr = Some(attr_val.to_owned());
                } else if attr_name.eq_ignore_ascii_case("seeders") {
                    seeders = attr_val.parse::<u32>().ok();
                } else if attr_name.eq_ignore_ascii_case("peers") {
                    peers = attr_val.parse::<u32>().ok();
                } else if attr_name.eq_ignore_ascii_case("size") {
                    if let Ok(sz) = attr_val.parse::<u64>() {
                        if sz > 0 {
                            size_bytes = Some(sz);
                        }
                    }
                } else if attr_name.eq_ignore_ascii_case("indexer") && !attr_val.is_empty() {
                    indexer = Some(attr_val.to_owned());
                }
            }
        }

        let Some(title) = title else {
            continue;
        };
        let magnet_url = magnet_attr.or(link_magnet);
        let Some(info_hash) = infohash_attr
            .as_deref()
            .and_then(|h| parse_info_hash(h).ok())
            .or_else(|| magnet_url.as_deref().and_then(|m| parse_info_hash(m).ok()))
        else {
            continue;
        };

        let announce = magnet_url
            .as_deref()
            .map(extract_magnet_trackers)
            .unwrap_or_default();

        out.push(ExtractedTorrentRelease {
            title,
            info_hash,
            announce,
            seeders: seeders.or(peers),
            size_bytes,
            indexer,
        });
    }

    Ok(out)
}

fn finalize_rich_streams(releases: Vec<ExtractedTorrentRelease>) -> Vec<RichStreamItem> {
    let mut by_hash: HashMap<[u8; 20], ExtractedTorrentRelease> = HashMap::new();
    for rel in releases {
        match by_hash.get_mut(&rel.info_hash) {
            Some(existing) => {
                if rel.seeders.unwrap_or(0) > existing.seeders.unwrap_or(0) {
                    *existing = rel;
                }
            }
            None => {
                by_hash.insert(rel.info_hash, rel);
            }
        }
    }

    let mut items: Vec<ExtractedTorrentRelease> = by_hash.into_values().collect();
    items.sort_by(|a, b| {
        b.seeders
            .unwrap_or(0)
            .cmp(&a.seeders.unwrap_or(0))
            .then_with(|| b.size_bytes.unwrap_or(0).cmp(&a.size_bytes.unwrap_or(0)))
            .then_with(|| a.title.cmp(&b.title))
    });

    items
        .into_iter()
        .map(|rel| {
            let mut meta_parts = Vec::new();
            if let Some(seeds) = rel.seeders {
                meta_parts.push(format!("👤 {seeds}"));
            }
            if let Some(sz) = rel.size_bytes {
                meta_parts.push(format!("💾 {}", format_bytes(sz)));
            }
            if let Some(idx) = rel.indexer.as_deref() {
                meta_parts.push(format!("⚙️ {idx}"));
            }

            let description = if meta_parts.is_empty() {
                rel.title.clone()
            } else {
                format!("{}\n{}", rel.title, meta_parts.join(" "))
            };

            let name = match rel.indexer.as_deref() {
                Some(idx) if !idx.is_empty() => format!("Prowlarr | {idx}"),
                _ => "Prowlarr".to_owned(),
            };

            let stream = Stream {
                source: StreamSource::Torrent {
                    info_hash: rel.info_hash,
                    file_idx: None,
                    announce: rel.announce,
                    file_must_include: Vec::new(),
                },
                name: Some(name),
                description: Some(description),
                thumbnail: None,
                subtitles: Vec::new(),
                behavior_hints: StreamBehaviorHints {
                    filename: Some(rel.title),
                    video_size: rel.size_bytes,
                    ..Default::default()
                },
            };

            let parsed = parse_stream_with_addon(&stream, Some("Prowlarr"));
            RichStreamItem { stream, parsed }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream_parser::{HdrFormat, Resolution, VideoCodec};

    #[test]
    fn parses_torznab_xml_into_rich_stream_items_cleanly() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">
  <channel>
    <item>
      <title>Dune.Part.Two.2024.2160p.WEB-DL.DV.HDR.HEVC.Atmos-FLUX.mkv</title>
      <size>24696061952</size>
      <prowlarrindexer>TorrentGalaxy</prowlarrindexer>
      <torznab:attr name="infohash" value="0123456789ABCDEF0123456789ABCDEF01234567" />
      <torznab:attr name="magneturl" value="magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&amp;tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce" />
      <torznab:attr name="seeders" value="184" />
    </item>
  </channel>
</rss>"#;

        let releases = parse_torznab_xml_items(xml).unwrap();
        let rich = finalize_rich_streams(releases);
        assert_eq!(rich.len(), 1);
        let item = &rich[0];
        assert_eq!(item.parsed.resolution, Resolution::Uhd4k);
        assert_eq!(item.parsed.hdr, HdrFormat::DolbyVision);
        assert_eq!(item.parsed.codec, VideoCodec::HEVC);
        assert_eq!(item.parsed.seeders, Some(184));
        assert_eq!(item.parsed.size_bytes, Some(24_696_061_952));
        assert_eq!(item.parsed.indexer.as_deref(), Some("TorrentGalaxy"));
        assert_eq!(item.parsed.addon_name.as_deref(), Some("Prowlarr"));
        assert!(item.parsed.is_allowed());
        assert!(matches!(
            item.stream.source,
            StreamSource::Torrent {
                ref announce,
                ..
            } if announce.len() == 1
        ));
    }
}
