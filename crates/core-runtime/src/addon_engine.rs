use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use http::{HeaderMap, Method};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use stremio_core::{
    constants::{
        CATALOG_RESOURCE_NAME, META_RESOURCE_NAME, STREAM_RESOURCE_NAME, SUBTITLES_RESOURCE_NAME,
    },
    runtime::EnvError,
    types::addon::{Descriptor, ManifestResource},
};
use tokio::sync::watch;
use tracing::{debug, trace};
use url::Url;

pub const ADDON_CACHE_MAX_ENTRIES: usize = 256;

pub const ADDON_CACHE_MAX_BYTES: usize = 6 * 1024 * 1024;

pub const ADDON_CACHE_MAX_SINGLE_ENTRY_BYTES: usize = 512 * 1024;

pub const CIRCUIT_BREAKER_FAILURE_THRESHOLD: u32 = 3;

pub const DEFAULT_CIRCUIT_BREAKER_COOLDOWN: Duration = Duration::from_secs(30);

pub const SLOW_ADDON_LATENCY_MS: u32 = 1_200;

pub const STREMIO_API_TIMEOUT: Duration = Duration::from_secs(15);

pub const LOCALHOST_TIMEOUT: Duration = Duration::from_secs(5);

pub const ADDON_MANIFEST_TIMEOUT: Duration = Duration::from_secs(6);

pub const ADDON_RESOURCE_TIMEOUT: Duration = Duration::from_millis(6_500);

pub const TTL_MANIFEST: Duration = Duration::from_secs(30 * 60);

pub const TTL_CATALOG: Duration = Duration::from_secs(5 * 60);

pub const TTL_META: Duration = Duration::from_secs(5 * 60);

pub const TTL_SUBTITLES: Duration = Duration::from_secs(10 * 60);

pub const TTL_STREAM: Duration = Duration::from_secs(45);

pub fn redact_url(url: &Url) -> String {
    let scheme = url.scheme().to_ascii_lowercase();
    match url.host_str().filter(|h| !h.is_empty()) {
        Some(host) => match url.port() {
            Some(port) => format!("{scheme}://{host}:{port}/..."),
            None => format!("{scheme}://{host}/..."),
        },
        None => format!("{scheme}:..."),
    }
}

pub fn redact_url_str(raw: &str) -> String {
    let trimmed = raw
        .trim()
        .trim_matches(|c| matches!(c, '"' | '\'' | '`'))
        .trim();
    if trimmed.is_empty() {
        return "<empty-url>".to_owned();
    }
    if let Ok(parsed) = Url::parse(trimmed) {
        return redact_url(&parsed);
    }
    if let Some((scheme_part, rest)) = trimmed.split_once("://") {
        let scheme = scheme_part.to_ascii_lowercase();
        if !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
            let host_port = authority
                .rsplit_once('@')
                .map(|(_, hp)| hp)
                .unwrap_or(authority);
            if !host_port.is_empty() && !host_port.chars().any(char::is_whitespace) {
                return format!("{scheme}://{host_port}/...");
            }
            return format!("{scheme}:...");
        }
    }
    if let Some((scheme_part, _)) = trimmed.split_once(':') {
        let scheme = scheme_part.to_ascii_lowercase();
        if !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            return format!("{scheme}:...");
        }
    }
    "<redacted-url>".to_owned()
}

static INLINE_URL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)\b(?:https?|stremio)://[^\s"',)\]>]+|\bmagnet:\?[^\s"',)\]>]+"#).unwrap()
});

static AUTH_KEY_TUPLE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"AuthKey\s*\(\s*"[^"]*"\s*\)"#).unwrap());

static RUST_DEBUG_SECRET_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?i)\b(password|auth_key|authkey|token|api_key|apikey|debrid_api_key|secret)\s*:\s*(?:Some\(\s*)?"[^"]*"(?:\s*\))?"#,
    )
    .unwrap()
});

static JSON_SECRET_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?i)"(password|authKey|auth_key|token|apiKey|api_key|debridApiKey|secret)"\s*:\s*"[^"]*""#,
    )
    .unwrap()
});

pub fn redact_sensitive_text(input: &str) -> String {
    let urls_redacted = INLINE_URL_RE.replace_all(input, |caps: &regex::Captures<'_>| {
        redact_url_str(&caps[0])
    });
    let auth_redacted = AUTH_KEY_TUPLE_RE.replace_all(&urls_redacted, "AuthKey(\"[REDACTED]\")");
    let debug_redacted =
        RUST_DEBUG_SECRET_RE.replace_all(&auth_redacted, "$1: \"[REDACTED]\"");
    JSON_SECRET_RE
        .replace_all(&debug_redacted, "\"$1\":\"[REDACTED]\"")
        .into_owned()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AddonHealthStatus {
    Idle,
    Healthy,
    Slow,
    Degraded,
    CircuitOpen { remaining_secs: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddonTelemetrySnapshot {
    pub origin: String,
    pub ema_latency_ms: Option<u32>,
    pub last_latency_ms: Option<u32>,
    pub total_requests: u64,
    pub cache_hits: u64,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
    pub status: AddonHealthStatus,
}

impl AddonTelemetrySnapshot {
    pub fn idle(origin: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            ema_latency_ms: None,
            last_latency_ms: None,
            total_requests: 0,
            cache_hits: 0,
            consecutive_failures: 0,
            last_error: None,
            status: AddonHealthStatus::Idle,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddonCacheStats {
    pub entries: usize,
    pub total_bytes: usize,
    pub hits: u64,
    pub misses: u64,
    pub coalesced: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutedAddonEntry {
    pub descriptor: Descriptor,
    pub preferred_index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallAddonOutcome {
    pub descriptor: Descriptor,
    pub replaced_existing: bool,
    pub was_muted: bool,
    pub slot_index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AddonCategory {
    Streams,
    Subtitles,
    Catalogs,
    Metadata,
}

impl AddonCategory {
    pub const ALL: [AddonCategory; 4] = [
        AddonCategory::Streams,
        AddonCategory::Subtitles,
        AddonCategory::Catalogs,
        AddonCategory::Metadata,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AddonCategory::Streams => "Streams",
            AddonCategory::Subtitles => "Subtitles",
            AddonCategory::Catalogs => "Catalogs",
            AddonCategory::Metadata => "Metadata",
        }
    }
}

pub fn classify_descriptor_categories(descriptor: &Descriptor) -> Vec<AddonCategory> {
    let mut categories = Vec::with_capacity(4);

    if !descriptor.manifest.catalogs.is_empty()
        || manifest_has_resource(descriptor, CATALOG_RESOURCE_NAME)
    {
        categories.push(AddonCategory::Catalogs);
    }

    if manifest_has_resource(descriptor, STREAM_RESOURCE_NAME) {
        categories.push(AddonCategory::Streams);
    }

    if manifest_has_resource(descriptor, SUBTITLES_RESOURCE_NAME) {
        categories.push(AddonCategory::Subtitles);
    }

    if manifest_has_resource(descriptor, META_RESOURCE_NAME) {
        categories.push(AddonCategory::Metadata);
    }

    categories
}

fn manifest_has_resource(descriptor: &Descriptor, resource_name: &str) -> bool {
    descriptor.manifest.resources.iter().any(|res| match res {
        ManifestResource::Short(name) => name == resource_name,
        ManifestResource::Full { name, .. } => name == resource_name,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct CuratedAddonEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub tagline: &'static str,
    pub description: &'static str,
    pub category: AddonCategory,
    pub types: &'static [&'static str],
    pub manifest_url: &'static str,
    pub configure_url: Option<&'static str>,
    pub requires_browser_config: bool,
}

static CURATED_ADDON_DIRECTORY: &[CuratedAddonEntry] = &[
    CuratedAddonEntry {
        id: "org.stremio.opensubtitlesv3",
        name: "OpenSubtitles v3",
        tagline: "Official hash-matched multi-language subtitles",
        description: "Automatic video-hash and IMDb subtitle matching across 60+ languages.",
        category: AddonCategory::Subtitles,
        types: &["movie", "series"],
        manifest_url: "https://opensubtitles-v3.strem.io/manifest.json",
        configure_url: None,
        requires_browser_config: false,
    },
    CuratedAddonEntry {
        id: "pw.ers.netflix-catalog",
        name: "Streaming Catalogs",
        tagline: "Netflix, Apple TV+, Max, Disney+ & Prime rows",
        description: "Adds curated trending movie and series rows grouped by streaming network.",
        category: AddonCategory::Catalogs,
        types: &["movie", "series"],
        manifest_url: "https://7a82163c306e-stremio-netflix-catalog-addon.baby-beamup.club/manifest.json",
        configure_url: Some("https://7a82163c306e-stremio-netflix-catalog-addon.baby-beamup.club/configure"),
        requires_browser_config: false,
    },
    CuratedAddonEntry {
        id: "com.stremio.kitsu.anime",
        name: "Anime Kitsu",
        tagline: "Seasonal, trending & top-rated anime catalog",
        description: "Comprehensive anime metadata, episode mappings, and seasonal Kitsu discovery rows.",
        category: AddonCategory::Catalogs,
        types: &["anime", "series", "movie"],
        manifest_url: "https://anime-kitsu.strem.fun/manifest.json",
        configure_url: None,
        requires_browser_config: false,
    },
    CuratedAddonEntry {
        id: "marcojoao.ml.cyberflix.catalog",
        name: "Cyberflix Catalog",
        tagline: "Multi-platform streaming, anime & kids rows",
        description: "Rich genre and streaming provider rows with customizable regional feeds.",
        category: AddonCategory::Catalogs,
        types: &["movie", "series", "anime"],
        manifest_url: "https://cyberflix.elfhosted.com/manifest.json",
        configure_url: Some("https://cyberflix.elfhosted.com/configure"),
        requires_browser_config: false,
    },
    CuratedAddonEntry {
        id: "tmdb-addon",
        name: "The Movie Database (TMDB)",
        tagline: "High-res artwork, localized synopses & cast",
        description: "Community TMDB metadata provider with localized titles, logos, and popular rows.",
        category: AddonCategory::Metadata,
        types: &["movie", "series"],
        manifest_url: "https://94c8cb9f702d-tmdb-addon.baby-beamup.club/manifest.json",
        configure_url: Some("https://94c8cb9f702d-tmdb-addon.baby-beamup.club/configure"),
        requires_browser_config: false,
    },
    CuratedAddonEntry {
        id: "org.publicdomainmovies",
        name: "Public Domain Movies",
        tagline: "Classic cinema streams ready out of the box",
        description: "Restored public-domain feature films and archival classics with instant playback.",
        category: AddonCategory::Streams,
        types: &["movie"],
        manifest_url: "https://publicdomainmovies.now.sh/manifest.json",
        configure_url: None,
        requires_browser_config: false,
    },
    CuratedAddonEntry {
        id: "com.webstreamr.addon",
        name: "WebStreamr",
        tagline: "Direct HTTPS streams with multi-audio tracks",
        description: "Resolves direct HTTPS streams without P2P swarms; optional custom hosts via configure.",
        category: AddonCategory::Streams,
        types: &["movie", "series"],
        manifest_url: "https://webstreamr.hayd.uk/manifest.json",
        configure_url: Some("https://webstreamr.hayd.uk/configure"),
        requires_browser_config: false,
    },
    CuratedAddonEntry {
        id: "stremio.comet.fast",
        name: "Comet (ElfHosted)",
        tagline: "Fast Debrid & torrent stream indexer",
        description: "Configure Real-Debrid, AllDebrid, TorBox, or Premiumize in browser, then paste your manifest URL.",
        category: AddonCategory::Streams,
        types: &["movie", "series", "anime"],
        manifest_url: "https://comet.elfhosted.com/manifest.json",
        configure_url: Some("https://comet.elfhosted.com/configure"),
        requires_browser_config: true,
    },
    CuratedAddonEntry {
        id: "stremio.addons.mediafusion",
        name: "MediaFusion (ElfHosted)",
        tagline: "Universal Debrid, TorBox & live events resolver",
        description: "Configure your Debrid provider and catalog preferences in browser to generate a personal manifest.",
        category: AddonCategory::Streams,
        types: &["movie", "series", "tv"],
        manifest_url: "https://mediafusion.elfhosted.com/manifest.json",
        configure_url: Some("https://mediafusion.elfhosted.com/configure"),
        requires_browser_config: true,
    },
    CuratedAddonEntry {
        id: "org.trakt.stremio",
        name: "Trakt TV Lists",
        tagline: "Personal watchlists & custom Traktlists",
        description: "Authenticate Trakt in browser to sync personal watchlists, history recommendations, and lists.",
        category: AddonCategory::Catalogs,
        types: &["movie", "series"],
        manifest_url: "https://2ecbbd610840-trakt.baby-beamup.club/manifest.json",
        configure_url: Some("https://2ecbbd610840-trakt.baby-beamup.club/configure"),
        requires_browser_config: true,
    },
    CuratedAddonEntry {
        id: "com.subdl.stremio",
        name: "SubDL Subtitles",
        tagline: "Season-pack & anime subtitle archive",
        description: "Configure your free SubDL API key and preferred subtitle languages in browser.",
        category: AddonCategory::Subtitles,
        types: &["movie", "series", "anime"],
        manifest_url: "https://subdl.strem.top/manifest.json",
        configure_url: Some("https://subdl.strem.top/configure"),
        requires_browser_config: true,
    },
    CuratedAddonEntry {
        id: "com.ratingposterdb.addon",
        name: "RPDB Rating Posters",
        tagline: "IMDb, Rotten Tomatoes & Metacritic poster badges",
        description: "Configure your RPDB key in browser to enrich catalog posters with critic score overlays.",
        category: AddonCategory::Metadata,
        types: &["movie", "series"],
        manifest_url: "https://api.ratingposterdb.com/stremio/manifest.json",
        configure_url: Some("https://ratingposterdb.com/integrations/stremio/"),
        requires_browser_config: true,
    },
];

pub fn curated_addon_directory() -> &'static [CuratedAddonEntry] {
    CURATED_ADDON_DIRECTORY
}

pub fn normalize_addon_url(raw: &str) -> Result<Url, String> {
    let trimmed = raw
        .trim()
        .trim_matches(|c| matches!(c, '"' | '\'' | '`'))
        .trim();

    if trimmed.is_empty() {
        return Err("Addon manifest URL cannot be empty".to_owned());
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err("Addon manifest URL must not contain spaces".to_owned());
    }

    let rewritten = if let Some(rest) = strip_ascii_case_prefix(trimmed, "stremio://") {
        if starts_with_loopback_host(rest) {
            format!("http://{rest}")
        } else {
            format!("https://{rest}")
        }
    } else if !trimmed.contains("://") {
        if starts_with_loopback_host(trimmed) {
            format!("http://{trimmed}")
        } else {
            format!("https://{trimmed}")
        }
    } else {
        trimmed.to_owned()
    };

    let mut url = Url::parse(&rewritten).map_err(|err| format!("Invalid addon URL: {err}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "Unsupported addon URL scheme '{}' (expected stremio://, https://, or http://)",
            url.scheme()
        ));
    }
    if url.host_str().unwrap_or_default().is_empty() {
        return Err("Addon URL is missing a hostname".to_owned());
    }

    url.set_fragment(None);

    let normalized_path = normalize_manifest_path(url.path());
    url.set_path(&normalized_path);

    Ok(url)
}

fn normalize_manifest_path(path: &str) -> String {
    let without_trailing_slash = path.trim_end_matches('/');
    if without_trailing_slash.is_empty() {
        return "/manifest.json".to_owned();
    }

    let lower = without_trailing_slash.to_ascii_lowercase();
    if lower.ends_with("/manifest.json") {
        return without_trailing_slash.to_owned();
    }
    if lower.ends_with("/configure") {
        let prefix = &without_trailing_slash[..without_trailing_slash.len() - "/configure".len()];
        return format!("{prefix}/manifest.json");
    }
    if lower.ends_with("/index.html") {
        let prefix = &without_trailing_slash[..without_trailing_slash.len() - "/index.html".len()];
        return format!("{prefix}/manifest.json");
    }

    format!("{without_trailing_slash}/manifest.json")
}

pub fn addon_configure_url(transport_url: &Url) -> Option<Url> {
    let mut url = if transport_url.scheme().eq_ignore_ascii_case("stremio") {
        normalize_addon_url(transport_url.as_str()).ok()?
    } else {
        transport_url.clone()
    };

    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }

    url.set_fragment(None);
    let path = url.path().trim_end_matches('/');
    let lower = path.to_ascii_lowercase();

    let next_path = if lower.ends_with("/manifest.json") {
        let prefix = &path[..path.len() - "/manifest.json".len()];
        format!("{prefix}/configure")
    } else if lower.ends_with("/configure") {
        path.to_owned()
    } else if path.is_empty() {
        "/configure".to_owned()
    } else {
        format!("{path}/configure")
    };

    url.set_path(&next_path);
    Some(url)
}

pub fn detect_clipboard_addon_url(clipboard_text: &str) -> Option<Url> {
    let trimmed = clipboard_text
        .trim()
        .trim_matches(|c| matches!(c, '"' | '\'' | '`'))
        .trim();

    if trimmed.is_empty() || trimmed.len() > 4096 || trimmed.chars().any(char::is_whitespace) {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    let is_stremio_scheme = lower.starts_with("stremio://");
    let is_manifest_http = (lower.starts_with("https://") || lower.starts_with("http://"))
        && (lower.contains("/manifest.json")
            || lower.ends_with("/manifest.json")
            || (lower.ends_with("/configure") && lower.contains("strem")));

    if !is_stremio_scheme && !is_manifest_http {
        return None;
    }

    normalize_addon_url(trimmed).ok()
}

pub fn addon_origin_from_url(url: &Url) -> String {
    let scheme = url.scheme().to_ascii_lowercase();
    let host = url.host_str().unwrap_or("unknown").to_ascii_lowercase();
    match url.port() {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    }
}

pub fn addon_origin_key(transport_url_or_origin: &str) -> Option<String> {
    let trimmed = transport_url_or_origin.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(url) = Url::parse(trimmed) {
        if url.host_str().is_some() {
            if url.scheme().eq_ignore_ascii_case("stremio") {
                if let Ok(norm) = normalize_addon_url(trimmed) {
                    return Some(addon_origin_from_url(&norm));
                }
            }
            return Some(addon_origin_from_url(&url));
        }
    }
    normalize_addon_url(trimmed)
        .ok()
        .map(|u| addon_origin_from_url(&u))
}

fn strip_ascii_case_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

fn starts_with_loopback_host(authority_and_path: &str) -> bool {
    let host_port = authority_and_path
        .split('/')
        .next()
        .unwrap_or(authority_and_path);
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or(host_port)
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };
    matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1" | "0.0.0.0"
    )
}

pub fn is_localhost_url(url: &Url) -> bool {
    match url.host_str() {
        Some(host) => matches!(
            host.trim_matches(['[', ']']).to_ascii_lowercase().as_str(),
            "localhost" | "127.0.0.1" | "::1" | "0.0.0.0"
        ),
        None => false,
    }
}

pub fn is_stremio_api_url(url: &Url) -> bool {
    match url.host_str() {
        Some(host) => matches!(
            host.to_ascii_lowercase().as_str(),
            "api.strem.io" | "link.strem.io"
        ),
        None => false,
    }
}

pub fn has_debrid_configured_addons(addons: &[Descriptor]) -> bool {
    addons.iter().any(addon_has_debrid_configured)
}

pub fn resolve_addon_for_request_base<'a>(
    addons: &'a [Descriptor],
    request_base: &Url,
) -> Option<&'a Descriptor> {
    if let Some(exact) = addons.iter().find(|a| a.transport_url == *request_base) {
        return Some(exact);
    }
    let req_str = request_base.as_str().trim_end_matches('/');
    if let Some(prefix_match) = addons.iter().find(|a| {
        let base_no_manifest = a
            .transport_url
            .as_str()
            .strip_suffix("/manifest.json")
            .unwrap_or_else(|| a.transport_url.as_str().trim_end_matches('/'));
        base_no_manifest.eq_ignore_ascii_case(req_str)
    }) {
        return Some(prefix_match);
    }
    let target_origin = addon_origin_from_url(request_base);
    addons
        .iter()
        .find(|a| addon_origin_from_url(&a.transport_url) == target_origin)
}

pub fn addon_has_debrid_configured(descriptor: &Descriptor) -> bool {
    if descriptor.flags.official
        || descriptor.flags.protected
        || descriptor.manifest.behavior_hints.configuration_required
    {
        return false;
    }

    let id_lower = descriptor.manifest.id.to_ascii_lowercase();
    if is_dedicated_debrid_manifest_id(&id_lower) {
        return true;
    }

    if manifest_name_indicates_debrid(&descriptor.manifest.name) {
        return true;
    }

    for (k, v) in descriptor.transport_url.query_pairs() {
        if kv_pair_indicates_debrid(&k, &v) || config_blob_indicates_debrid(&v) {
            return true;
        }
    }

    if let Some(segments) = descriptor.transport_url.path_segments() {
        for raw_seg in segments {
            let trimmed = raw_seg.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("manifest.json") {
                continue;
            }
            let decoded_bytes = percent_decode_bytes(trimmed);
            if let Ok(decoded_str) = std::str::from_utf8(&decoded_bytes) {
                if segment_kv_indicates_debrid(decoded_str)
                    || config_blob_indicates_debrid(decoded_str)
                {
                    return true;
                }
            }
        }
    }

    false
}

fn is_dedicated_debrid_manifest_id(id_lower: &str) -> bool {
    const DEDICATED_IDS: &[&str] = &[
        "torbox",
        "debridsearch",
        "debrid-search",
        "debrid_search",
        "stremthru",
        "easynews",
        "realdebrid",
        "real-debrid",
        "alldebrid",
        "premiumize",
        "debridlink",
        "easydebrid",
        "offcloud",
        "putio",
        "pikpak",
    ];
    DEDICATED_IDS.iter().any(|needle| id_lower.contains(needle))
}

fn manifest_name_indicates_debrid(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    const EXPLICIT_SUBSTRINGS: &[&str] = &[
        "real-debrid",
        "realdebrid",
        "alldebrid",
        "torbox",
        "premiumize",
        "debrid-link",
        "debridlink",
        "easydebrid",
        "offcloud",
        "put.io",
        "pikpak",
        "debridsearch",
        "stremthru",
        "easynews",
        "[rd",
        "[ad",
        "[tb",
        "[pm",
        "[dl",
        "[ed",
        "[oc",
        "(rd)",
        "(ad)",
        "(tb)",
        "(pm)",
        "(dl)",
    ];
    if EXPLICIT_SUBSTRINGS.iter().any(|pat| lower.contains(pat)) {
        return true;
    }

    name.split(|c: char| !c.is_ascii_alphanumeric() && c != '+')
        .any(|tok| {
            matches!(
                tok,
                "RD" | "RD+"
                    | "AD"
                    | "AD+"
                    | "TB"
                    | "TB+"
                    | "PM"
                    | "PM+"
                    | "DL"
                    | "DL+"
                    | "ED"
                    | "ED+"
                    | "OC"
                    | "OC+"
            )
        })
}

fn segment_kv_indicates_debrid(segment: &str) -> bool {
    for part in segment.split(['|', '&', ';']) {
        if let Some((raw_k, raw_v)) = part.split_once('=') {
            if kv_pair_indicates_debrid(raw_k, raw_v) || config_blob_indicates_debrid(raw_v) {
                return true;
            }
        }
    }
    false
}

fn normalize_alnum_lower(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn is_non_empty_credential_value(val: &str) -> bool {
    let v = val.trim();
    if v.is_empty() {
        return false;
    }
    let norm = normalize_alnum_lower(v);
    !norm.is_empty()
        && !matches!(
            norm.as_str(),
            "none"
                | "false"
                | "0"
                | "null"
                | "undefined"
                | "off"
                | "disabled"
                | "no"
                | "p2p"
                | "torrent"
                | "direct"
        )
}

fn is_known_debrid_service_name(val: &str) -> bool {
    let norm = normalize_alnum_lower(val);
    matches!(
        norm.as_str(),
        "realdebrid"
            | "rd"
            | "alldebrid"
            | "ad"
            | "torbox"
            | "tb"
            | "premiumize"
            | "premiumizeme"
            | "pm"
            | "debridlink"
            | "dl"
            | "easydebrid"
            | "ed"
            | "offcloud"
            | "oc"
            | "putio"
            | "pikpak"
            | "easynews"
            | "stremthru"
    )
}

fn kv_pair_indicates_debrid(raw_key: &str, raw_val: &str) -> bool {
    let key = normalize_alnum_lower(raw_key);
    if key.is_empty() {
        return false;
    }

    if matches!(
        key.as_str(),
        "realdebrid"
            | "rd"
            | "rdkey"
            | "rdtoken"
            | "realdebridapikey"
            | "alldebrid"
            | "ad"
            | "adkey"
            | "adtoken"
            | "alldebridapikey"
            | "premiumize"
            | "pm"
            | "pmkey"
            | "pmtoken"
            | "premiumizeapikey"
            | "debridlink"
            | "dl"
            | "dlkey"
            | "dltoken"
            | "debridlinkapikey"
            | "torbox"
            | "tb"
            | "tbkey"
            | "tbtoken"
            | "torboxapikey"
            | "easydebrid"
            | "ed"
            | "edkey"
            | "easydebridapikey"
            | "offcloud"
            | "oc"
            | "ockey"
            | "putio"
            | "putiokey"
            | "pikpak"
            | "easynews"
            | "debridapikey"
            | "debridkey"
            | "debridtoken"
            | "storetoken"
            | "stremthrukey"
    ) {
        return is_non_empty_credential_value(raw_val);
    }

    if matches!(
        key.as_str(),
        "debrid" | "debridservice" | "debridprovider" | "debridid" | "storeservice" | "storename"
    ) {
        return is_known_debrid_service_name(raw_val);
    }

    false
}

fn config_blob_indicates_debrid(candidate: &str) -> bool {
    let trimmed = candidate.trim();
    if trimmed.len() < 4 {
        return false;
    }

    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if json_value_indicates_debrid(&json_val) {
                return true;
            }
        }
    }

    if let Some(decoded) = decode_base64_flexible(trimmed) {
        if let Ok(decoded_str) = std::str::from_utf8(&decoded) {
            let inner = decoded_str.trim();
            if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(inner) {
                if json_value_indicates_debrid(&json_val) {
                    return true;
                }
            } else if segment_kv_indicates_debrid(inner) {
                return true;
            }
        }
    }

    false
}

fn json_value_indicates_debrid(val: &serde_json::Value) -> bool {
    match val {
        serde_json::Value::Array(items) => items.iter().any(json_value_indicates_debrid),
        serde_json::Value::Object(map) => {
            if map.get("enabled") == Some(&serde_json::Value::Bool(false)) {
                return false;
            }

            let mut saw_credential_field = false;
            let mut has_non_empty_credential = false;
            for (k, v) in map {
                let kn = normalize_alnum_lower(k);
                if matches!(
                    kn.as_str(),
                    "debridapikey"
                        | "debridkey"
                        | "debridtoken"
                        | "apikey"
                        | "key"
                        | "token"
                        | "storetoken"
                ) {
                    saw_credential_field = true;
                    if json_scalar_or_nested_non_empty(v) {
                        has_non_empty_credential = true;
                    }
                }
            }

            for (k, v) in map {
                let kn = normalize_alnum_lower(k);

                if matches!(
                    kn.as_str(),
                    "realdebrid"
                        | "alldebrid"
                        | "torbox"
                        | "premiumize"
                        | "debridlink"
                        | "easydebrid"
                        | "offcloud"
                        | "putio"
                        | "pikpak"
                        | "easynews"
                        | "debridapikey"
                        | "debridkey"
                        | "debridtoken"
                        | "rdkey"
                        | "adkey"
                        | "tbkey"
                        | "pmkey"
                        | "dlkey"
                        | "storetoken"
                ) && json_provider_entry_active(v)
                {
                    return true;
                }

                if matches!(
                    kn.as_str(),
                    "debrid"
                        | "debridservice"
                        | "debridprovider"
                        | "debridid"
                        | "service"
                        | "provider"
                        | "store"
                        | "storeservice"
                        | "id"
                ) {
                    if let Some(s) = v.as_str() {
                        if is_known_debrid_service_name(s)
                            && (!saw_credential_field || has_non_empty_credential)
                        {
                            let nested_creds_ok = map
                                .get("credentials")
                                .map(json_scalar_or_nested_non_empty)
                                .unwrap_or(true);
                            if nested_creds_ok {
                                return true;
                            }
                        }
                    } else if json_value_indicates_debrid(v) {
                        return true;
                    }
                }

                if matches!(
                    v,
                    serde_json::Value::Object(_) | serde_json::Value::Array(_)
                ) && json_value_indicates_debrid(v)
                {
                    return true;
                }
            }

            false
        }
        _ => false,
    }
}

fn json_provider_entry_active(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::String(s) => is_non_empty_credential_value(s),
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Array(arr) => arr.iter().any(json_provider_entry_active),
        serde_json::Value::Object(map) => {
            if map.get("enabled") == Some(&serde_json::Value::Bool(false)) {
                return false;
            }
            map.values().any(json_scalar_or_nested_non_empty)
        }
        _ => false,
    }
}

fn json_scalar_or_nested_non_empty(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::String(s) => is_non_empty_credential_value(s),
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(_) => true,
        serde_json::Value::Array(arr) => arr.iter().any(json_scalar_or_nested_non_empty),
        serde_json::Value::Object(map) => map.values().any(json_scalar_or_nested_non_empty),
        serde_json::Value::Null => false,
    }
}

fn percent_decode_bytes(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn decode_base64_flexible(input: &str) -> Option<Vec<u8>> {
    let trimmed = input.trim().trim_end_matches('=');
    if trimmed.len() < 4 || trimmed.len() % 4 == 1 {
        return None;
    }

    let mut out = Vec::with_capacity((trimmed.len() * 3) / 4);
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;

    for b in trimmed.bytes() {
        let sextet = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(sextet);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }

    Some(out)
}

pub fn classify_request_deadline(
    method: &Method,
    url: &Url,
    treat_loopback_as_remote: bool,
) -> Duration {
    if *method != Method::GET || is_stremio_api_url(url) {
        return STREMIO_API_TIMEOUT;
    }
    if is_localhost_url(url) && !treat_loopback_as_remote {
        return LOCALHOST_TIMEOUT;
    }
    let path_lower = url.path().to_ascii_lowercase();
    if path_lower.ends_with("/manifest.json") {
        ADDON_MANIFEST_TIMEOUT
    } else {
        ADDON_RESOURCE_TIMEOUT
    }
}

pub fn classify_cache_ttl(
    method: &Method,
    url: &Url,
    headers: &HeaderMap,
    treat_loopback_as_remote: bool,
) -> Option<Duration> {
    if *method != Method::GET {
        return None;
    }
    if is_stremio_api_url(url) {
        return None;
    }
    if is_localhost_url(url) && !treat_loopback_as_remote {
        return None;
    }
    if headers.contains_key(http::header::AUTHORIZATION)
        || headers.contains_key(http::header::COOKIE)
    {
        return None;
    }

    let path = url.path().to_ascii_lowercase();
    if path.ends_with("/manifest.json") {
        Some(TTL_MANIFEST)
    } else if path.contains("/catalog/") {
        Some(TTL_CATALOG)
    } else if path.contains("/meta/") {
        Some(TTL_META)
    } else if path.contains("/subtitles/") {
        Some(TTL_SUBTITLES)
    } else if path.contains("/stream/") {
        Some(TTL_STREAM)
    } else {
        None
    }
}

struct CachedHttpEntry {
    body: Arc<[u8]>,
    expires_at: Instant,
}

#[derive(Default)]
struct CacheInner {
    map: HashMap<String, CachedHttpEntry>,
    lru_order: VecDeque<String>,
    total_bytes: usize,
}

impl CacheInner {
    fn get_fresh(&mut self, key: &str, now: Instant) -> Option<Arc<[u8]>> {
        let is_expired = now >= self.map.get(key)?.expires_at;

        if is_expired {
            self.remove_entry(key);
            return None;
        }

        let body = Arc::clone(&self.map.get(key)?.body);
        if let Some(pos) = self.lru_order.iter().position(|k| k == key) {
            self.lru_order.remove(pos);
        }
        self.lru_order.push_back(key.to_owned());
        Some(body)
    }

    fn insert(&mut self, key: String, body: Arc<[u8]>, ttl: Duration, now: Instant) {
        let entry_bytes = body.len();
        if entry_bytes > ADDON_CACHE_MAX_SINGLE_ENTRY_BYTES {
            return;
        }

        self.evict_expired(now);
        self.remove_entry(&key);

        while (self.map.len() >= ADDON_CACHE_MAX_ENTRIES
            || self.total_bytes + entry_bytes > ADDON_CACHE_MAX_BYTES)
            && !self.lru_order.is_empty()
        {
            if let Some(oldest_key) = self.lru_order.pop_front() {
                if let Some(removed) = self.map.remove(&oldest_key) {
                    self.total_bytes = self.total_bytes.saturating_sub(removed.body.len());
                }
            }
        }

        self.total_bytes += entry_bytes;
        self.lru_order.push_back(key.clone());
        self.map.insert(
            key,
            CachedHttpEntry {
                body,
                expires_at: now + ttl,
            },
        );
    }

    fn remove_entry(&mut self, key: &str) {
        if let Some(removed) = self.map.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(removed.body.len());
        }
        if let Some(pos) = self.lru_order.iter().position(|k| k == key) {
            self.lru_order.remove(pos);
        }
    }

    fn evict_expired(&mut self, now: Instant) {
        let expired_keys: Vec<String> = self
            .map
            .iter()
            .filter(|(_, v)| now >= v.expires_at)
            .map(|(k, _)| k.clone())
            .collect();
        for k in expired_keys {
            self.remove_entry(&k);
        }
    }

    fn clear(&mut self) {
        self.map.clear();
        self.lru_order.clear();
        self.total_bytes = 0;
    }
}

#[derive(Default)]
struct OriginTelemetryState {
    ema_latency_ms: Option<f64>,
    last_latency_ms: Option<u32>,
    total_requests: u64,
    cache_hits: u64,
    consecutive_failures: u32,
    last_error: Option<String>,
    circuit_open_until: Option<Instant>,
}

impl OriginTelemetryState {
    fn to_snapshot(&self, origin: String, now: Instant) -> AddonTelemetrySnapshot {
        let ema_u32 = self.ema_latency_ms.map(|v| v.round() as u32);
        let status = if let Some(until) = self.circuit_open_until {
            if until > now {
                let remaining = until.duration_since(now).as_secs().max(1);
                AddonHealthStatus::CircuitOpen {
                    remaining_secs: remaining,
                }
            } else if self.consecutive_failures > 0 {
                AddonHealthStatus::Degraded
            } else {
                AddonHealthStatus::Healthy
            }
        } else if self.total_requests == 0 {
            AddonHealthStatus::Idle
        } else if self.consecutive_failures > 0 {
            AddonHealthStatus::Degraded
        } else if ema_u32.unwrap_or(0) > SLOW_ADDON_LATENCY_MS {
            AddonHealthStatus::Slow
        } else {
            AddonHealthStatus::Healthy
        }
        ;

        AddonTelemetrySnapshot {
            origin,
            ema_latency_ms: ema_u32,
            last_latency_ms: self.last_latency_ms,
            total_requests: self.total_requests,
            cache_hits: self.cache_hits,
            consecutive_failures: self.consecutive_failures,
            last_error: self.last_error.clone(),
            status,
        }
    }
}

type InFlightOutcome = Result<Arc<[u8]>, String>;

struct SingleflightLeaderGuard {
    key: String,
    in_flight: Arc<Mutex<HashMap<String, watch::Receiver<Option<InFlightOutcome>>>>>,
    tx: Option<watch::Sender<Option<InFlightOutcome>>>,
}

impl SingleflightLeaderGuard {
    fn complete(mut self, result: InFlightOutcome) {
        if let Ok(mut guard) = self.in_flight.lock() {
            guard.remove(&self.key);
        }
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Some(result));
        }
    }
}

impl Drop for SingleflightLeaderGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            if let Ok(mut guard) = self.in_flight.lock() {
                guard.remove(&self.key);
            }
            let _ = tx.send(Some(Err("In-flight HTTP request was cancelled".to_owned())));
        }
    }
}

#[derive(Clone, Debug)]
pub struct AddonEngineConfig {
    pub timeout_override: Option<Duration>,
    pub circuit_breaker_cooldown: Duration,
    pub treat_loopback_as_remote: bool,
    pub streaming_server_origin: Option<String>,
}

impl Default for AddonEngineConfig {
    fn default() -> Self {
        Self {
            timeout_override: None,
            circuit_breaker_cooldown: DEFAULT_CIRCUIT_BREAKER_COOLDOWN,
            treat_loopback_as_remote: false,
            streaming_server_origin: None,
        }
    }
}

pub struct AddonHttpEngine {
    config: AddonEngineConfig,
    cache: Mutex<CacheInner>,
    in_flight: Arc<Mutex<HashMap<String, watch::Receiver<Option<InFlightOutcome>>>>>,
    telemetry: Mutex<HashMap<String, OriginTelemetryState>>,
    hits: AtomicU64,
    misses: AtomicU64,
    coalesced: AtomicU64,
}

impl AddonHttpEngine {
    pub fn new(config: AddonEngineConfig) -> Self {
        Self {
            config,
            cache: Mutex::new(CacheInner::default()),
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            telemetry: Mutex::new(HashMap::new()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            coalesced: AtomicU64::new(0),
        }
    }

    pub fn clear_cache(&self) {
        if let Ok(mut guard) = self.cache.lock() {
            guard.clear();
        }
    }

    pub fn invalidate_url(&self, url: &str) {
        if let Ok(mut guard) = self.cache.lock() {
            guard.remove_entry(url);
        }
    }

    pub fn cache_stats(&self) -> AddonCacheStats {
        let now = Instant::now();
        let (entries, total_bytes) = match self.cache.lock() {
            Ok(mut guard) => {
                guard.evict_expired(now);
                (guard.map.len(), guard.total_bytes)
            }
            Err(_) => (0, 0),
        };
        AddonCacheStats {
            entries,
            total_bytes,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            coalesced: self.coalesced.load(Ordering::Relaxed),
        }
    }

    pub fn telemetry_by_origin(&self) -> HashMap<String, AddonTelemetrySnapshot> {
        let now = Instant::now();
        let Ok(guard) = self.telemetry.lock() else {
            return HashMap::new();
        };
        guard
            .iter()
            .map(|(origin, state)| (origin.clone(), state.to_snapshot(origin.clone(), now)))
            .collect()
    }

    pub fn telemetry_for_url(&self, url: &Url) -> AddonTelemetrySnapshot {
        let origin = addon_origin_from_url(url);
        let now = Instant::now();
        if let Ok(guard) = self.telemetry.lock() {
            if let Some(state) = guard.get(&origin) {
                return state.to_snapshot(origin, now);
            }
        }
        AddonTelemetrySnapshot::idle(origin)
    }

    pub fn reset_circuit(&self, transport_url_or_origin: &str) -> bool {
        let Some(origin) = addon_origin_key(transport_url_or_origin) else {
            return false;
        };
        let Ok(mut guard) = self.telemetry.lock() else {
            return false;
        };
        if let Some(state) = guard.get_mut(&origin) {
            state.circuit_open_until = None;
            state.consecutive_failures = 0;
            state.last_error = None;
            return true;
        }
        false
    }

    fn is_remote_third_party(&self, url: &Url, origin: &str) -> bool {
        if is_stremio_api_url(url) {
            return false;
        }
        if let Some(ref ss_origin) = self.config.streaming_server_origin {
            if ss_origin == origin {
                return false;
            }
        }
        !is_localhost_url(url) || self.config.treat_loopback_as_remote
    }

    fn effective_loopback_as_remote(&self, origin: &str) -> bool {
        if let Some(ref ss_origin) = self.config.streaming_server_origin {
            if ss_origin == origin {
                return false;
            }
        }
        self.config.treat_loopback_as_remote
    }

    fn check_circuit_before_request(&self, origin: &str) -> Result<(), EnvError> {
        let now = Instant::now();
        let Ok(mut guard) = self.telemetry.lock() else {
            return Ok(());
        };
        if let Some(state) = guard.get_mut(origin) {
            if let Some(until) = state.circuit_open_until {
                if until > now {
                    state.total_requests += 1;
                    let remaining = until.duration_since(now).as_secs().max(1);
                    return Err(EnvError::Fetch(format!(
                        "Addon circuit-breaker open for {origin} ({remaining}s cooldown remaining)"
                    )));
                }
            }
        }
        Ok(())
    }

    fn record_cache_hit(&self, origin: &str) {
        self.hits.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut guard) = self.telemetry.lock() {
            let entry = guard.entry(origin.to_owned()).or_default();
            entry.total_requests += 1;
            entry.cache_hits += 1;
        }
    }

    fn record_coalesced_join(&self, origin: &str) {
        self.coalesced.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut guard) = self.telemetry.lock() {
            let entry = guard.entry(origin.to_owned()).or_default();
            entry.total_requests += 1;
        }
    }

    fn record_network_success(&self, origin: &str, elapsed: Duration) {
        let sample_ms = (elapsed.as_millis() as u64).min(u32::MAX as u64) as u32;
        if let Ok(mut guard) = self.telemetry.lock() {
            let entry = guard.entry(origin.to_owned()).or_default();
            entry.total_requests += 1;
            entry.last_latency_ms = Some(sample_ms);
            entry.ema_latency_ms = Some(match entry.ema_latency_ms {
                Some(prev) => prev * 0.7 + (sample_ms as f64) * 0.3,
                None => sample_ms as f64,
            });
            entry.consecutive_failures = 0;
            entry.circuit_open_until = None;
            entry.last_error = None;
        }
    }

    fn record_network_failure(
        &self,
        origin: &str,
        error_msg: &str,
        counts_toward_circuit: bool,
        allow_circuit_breaker: bool,
    ) {
        let now = Instant::now();
        if let Ok(mut guard) = self.telemetry.lock() {
            let entry = guard.entry(origin.to_owned()).or_default();
            entry.total_requests += 1;
            entry.last_error = Some(error_msg.to_owned());

            if counts_toward_circuit {
                entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
                if allow_circuit_breaker
                    && entry.consecutive_failures >= CIRCUIT_BREAKER_FAILURE_THRESHOLD
                {
                    entry.circuit_open_until = Some(now + self.config.circuit_breaker_cooldown);
                    debug!(
                        %origin,
                        failures = entry.consecutive_failures,
                        cooldown_secs = self.config.circuit_breaker_cooldown.as_secs(),
                        "Tripped soft circuit-breaker for unresponsive addon origin"
                    );
                }
            }
        }
    }

    pub async fn execute_bytes(
        &self,
        client: &reqwest::Client,
        method: Method,
        url_str: String,
        headers: HeaderMap,
        serialized_body: Option<Vec<u8>>,
        bypass_cache: bool,
    ) -> Result<Arc<[u8]>, EnvError> {
        let parsed_url = Url::parse(&url_str).map_err(|err| {
            let redacted = redact_url_str(&url_str);
            EnvError::Fetch(format!("Invalid request URL '{redacted}': {err}"))
        })?;
        let redacted_target = redact_url(&parsed_url);
        let origin = addon_origin_from_url(&parsed_url);
        let loopback_as_remote = self.effective_loopback_as_remote(&origin);
        let allow_circuit = self.is_remote_third_party(&parsed_url, &origin);

        if allow_circuit {
            self.check_circuit_before_request(&origin)?;
        }

        let cache_ttl = if bypass_cache {
            None
        } else {
            classify_cache_ttl(&method, &parsed_url, &headers, loopback_as_remote)
        };

        if cache_ttl.is_some() {
            let cached = self
                .cache
                .lock()
                .ok()
                .and_then(|mut guard| guard.get_fresh(&url_str, Instant::now()));
            if let Some(bytes) = cached {
                trace!(url = %redacted_target, "AddonHttpCache HIT");
                self.record_cache_hit(&origin);
                return Ok(bytes);
            }
        }

        let can_coalesce = method == Method::GET
            && serialized_body.is_none()
            && !bypass_cache
            && !headers.contains_key(http::header::AUTHORIZATION)
            && !headers.contains_key(http::header::COOKIE);

        let mut leader_guard: Option<SingleflightLeaderGuard> = None;
        if can_coalesce {
            let existing_rx = {
                let mut map = self
                    .in_flight
                    .lock()
                    .map_err(|_| EnvError::Fetch("Singleflight mutex poisoned".to_owned()))?;
                if let Some(rx) = map.get(&url_str) {
                    Some(rx.clone())
                } else {
                    let (tx, rx) = watch::channel(None);
                    map.insert(url_str.clone(), rx);
                    leader_guard = Some(SingleflightLeaderGuard {
                        key: url_str.clone(),
                        in_flight: Arc::clone(&self.in_flight),
                        tx: Some(tx),
                    });
                    None
                }
            };

            if let Some(mut rx) = existing_rx {
                trace!(url = %redacted_target, "Singleflight coalesced concurrent GET");
                self.record_coalesced_join(&origin);
                let waited = rx.wait_for(|val| val.is_some()).await;
                return match waited {
                    Ok(guard_ref) => match guard_ref.as_ref().expect("checked is_some") {
                        Ok(bytes) => Ok(Arc::clone(bytes)),
                        Err(err_msg) => Err(EnvError::Fetch(err_msg.clone())),
                    },
                    Err(_) => Err(EnvError::Fetch(
                        "Singleflight leader dropped before completing".to_owned(),
                    )),
                };
            }
        }

        if cache_ttl.is_some() {
            self.misses.fetch_add(1, Ordering::Relaxed);
        }

        let deadline = self.config.timeout_override.unwrap_or_else(|| {
            classify_request_deadline(&method, &parsed_url, loopback_as_remote)
        });

        let reqwest_method = reqwest::Method::from_bytes(method.as_str().as_bytes())
            .map_err(|err| EnvError::Fetch(format!("Invalid HTTP method: {err}")))?;

        let mut req_builder = client.request(reqwest_method, &url_str).timeout(deadline);

        for (name, value) in &headers {
            req_builder = req_builder.header(name, value);
        }
        if let Some(body_bytes) = serialized_body {
            if !headers.contains_key(http::header::CONTENT_TYPE) {
                req_builder = req_builder.header(http::header::CONTENT_TYPE, "application/json");
            }
            req_builder = req_builder.body(body_bytes);
        }

        trace!(
            %method,
            url = %redacted_target,
            timeout_ms = deadline.as_millis(),
            "DesktopEnv HTTP fetch"
        );
        let started_at = Instant::now();
        let send_res = req_builder.send().await;

        let network_outcome: Result<Arc<[u8]>, (String, bool)> = match send_res {
            Ok(response) => {
                let status = response.status();
                if !status.is_success() {
                    let is_server_fault = status.is_server_error() || status.as_u16() == 408;
                    Err((
                        format!(
                            "Unexpected HTTP status code {} for {}",
                            status.as_u16(),
                            redacted_target
                        ),
                        is_server_fault,
                    ))
                } else {
                    match response.bytes().await {
                        Ok(bytes) => Ok(Arc::<[u8]>::from(bytes.as_ref())),
                        Err(err) => {
                            let clean = err.without_url();
                            Err((format!("{clean} ({redacted_target})"), true))
                        }
                    }
                }
            }
            Err(err) => {
                let clean = err.without_url();
                Err((format!("{clean} ({redacted_target})"), true))
            }
        };

        let elapsed = started_at.elapsed();

        match network_outcome {
            Ok(shared_bytes) => {
                self.record_network_success(&origin, elapsed);
                if let Some(ttl) = cache_ttl {
                    if let Ok(mut guard) = self.cache.lock() {
                        guard.insert(
                            url_str,
                            Arc::clone(&shared_bytes),
                            ttl,
                            Instant::now(),
                        );
                    }
                }
                if let Some(leader) = leader_guard {
                    leader.complete(Ok(Arc::clone(&shared_bytes)));
                }
                Ok(shared_bytes)
            }
            Err((err_msg, counts_toward_circuit)) => {
                self.record_network_failure(
                    &origin,
                    &err_msg,
                    counts_toward_circuit,
                    allow_circuit,
                );
                if let Some(leader) = leader_guard {
                    leader.complete(Err(err_msg.clone()));
                }
                Err(EnvError::Fetch(err_msg))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_stremio_and_configure_urls_cleanly() {
        assert_eq!(
            normalize_addon_url("stremio://torrentio.strem.fun/realdebrid=abc123/manifest.json")
                .unwrap()
                .as_str(),
            "https://torrentio.strem.fun/realdebrid=abc123/manifest.json"
        );
        assert_eq!(
            normalize_addon_url("  \"stremio://127.0.0.1:7000/configure\"  ")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:7000/manifest.json"
        );
        assert_eq!(
            normalize_addon_url("https://comet.elfhosted.com/eyJ0b2tlbiI6MX0/configure/")
                .unwrap()
                .as_str(),
            "https://comet.elfhosted.com/eyJ0b2tlbiI6MX0/manifest.json"
        );
        assert_eq!(
            normalize_addon_url("v3-cinemeta.strem.io")
                .unwrap()
                .as_str(),
            "https://v3-cinemeta.strem.io/manifest.json"
        );
    }

    #[test]
    fn maps_manifest_url_to_configure_preserving_secret_path() {
        let transport =
            Url::parse("https://torrentio.strem.fun/rd=secret_token/manifest.json").unwrap();
        assert_eq!(
            addon_configure_url(&transport).unwrap().as_str(),
            "https://torrentio.strem.fun/rd=secret_token/configure"
        );

        let root_manifest = Url::parse("https://cyberflix.elfhosted.com/manifest.json").unwrap();
        assert_eq!(
            addon_configure_url(&root_manifest).unwrap().as_str(),
            "https://cyberflix.elfhosted.com/configure"
        );
    }

    #[test]
    fn detects_clipboard_stremio_and_manifest_urls_without_false_positives() {
        assert_eq!(
            detect_clipboard_addon_url("stremio://anime-kitsu.strem.fun/manifest.json")
                .unwrap()
                .as_str(),
            "https://anime-kitsu.strem.fun/manifest.json"
        );
        assert_eq!(
            detect_clipboard_addon_url("https://opensubtitles-v3.strem.io/manifest.json")
                .unwrap()
                .as_str(),
            "https://opensubtitles-v3.strem.io/manifest.json"
        );
        assert!(detect_clipboard_addon_url("https://google.com/search?q=movies").is_none());
        assert!(detect_clipboard_addon_url("just some copied prose with spaces").is_none());
    }

    #[test]
    fn classifies_smart_deadlines_and_cache_ttls_accurately() {
        let empty_headers = HeaderMap::new();

        let api_url = Url::parse("https://api.strem.io/api/addonCollectionGet").unwrap();
        assert_eq!(
            classify_request_deadline(&Method::POST, &api_url, false),
            Duration::from_secs(15)
        );
        assert_eq!(
            classify_cache_ttl(&Method::GET, &api_url, &empty_headers, false),
            None
        );

        let local_url = Url::parse("http://127.0.0.1:11470/settings").unwrap();
        assert_eq!(
            classify_request_deadline(&Method::GET, &local_url, false),
            Duration::from_secs(5)
        );
        assert_eq!(
            classify_cache_ttl(&Method::GET, &local_url, &empty_headers, false),
            None
        );

        let manifest_url = Url::parse("https://comet.elfhosted.com/manifest.json").unwrap();
        assert_eq!(
            classify_request_deadline(&Method::GET, &manifest_url, false),
            Duration::from_secs(6)
        );
        assert_eq!(
            classify_cache_ttl(&Method::GET, &manifest_url, &empty_headers, false),
            Some(TTL_MANIFEST)
        );

        let stream_url =
            Url::parse("https://comet.elfhosted.com/stream/movie/tt0133093.json").unwrap();
        assert_eq!(
            classify_request_deadline(&Method::GET, &stream_url, false),
            Duration::from_millis(6_500)
        );
        assert_eq!(
            classify_cache_ttl(&Method::GET, &stream_url, &empty_headers, false),
            Some(TTL_STREAM)
        );

        let catalog_url =
            Url::parse("https://v3-cinemeta.strem.io/catalog/movie/top.json").unwrap();
        assert_eq!(
            classify_cache_ttl(&Method::GET, &catalog_url, &empty_headers, false),
            Some(TTL_CATALOG)
        );

        let meta_url =
            Url::parse("https://v3-cinemeta.strem.io/meta/series/tt0903747.json").unwrap();
        assert_eq!(
            classify_cache_ttl(&Method::GET, &meta_url, &empty_headers, false),
            Some(TTL_META)
        );

        let subs_url =
            Url::parse("https://opensubtitles-v3.strem.io/subtitles/movie/tt0133093.json").unwrap();
        assert_eq!(
            classify_cache_ttl(&Method::GET, &subs_url, &empty_headers, false),
            Some(TTL_SUBTITLES)
        );
    }

    #[test]
    fn curated_directory_covers_all_four_categories_with_valid_urls() {
        let dir = curated_addon_directory();
        assert!(dir.len() >= 10);
        for cat in AddonCategory::ALL {
            assert!(
                dir.iter().any(|entry| entry.category == cat),
                "missing curated category {cat:?}"
            );
        }
        for entry in dir {
            assert!(Url::parse(entry.manifest_url).is_ok());
            if let Some(cfg) = entry.configure_url {
                assert!(Url::parse(cfg).is_ok());
            }
        }
    }

    fn encode_base64_url_unpadded(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        let mut i = 0;
        while i < bytes.len() {
            let b0 = bytes[i];
            let b1 = if i + 1 < bytes.len() { bytes[i + 1] } else { 0 };
            let b2 = if i + 2 < bytes.len() { bytes[i + 2] } else { 0 };

            out.push(ALPHABET[(b0 >> 2) as usize] as char);
            out.push(ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
            if i + 1 < bytes.len() {
                out.push(ALPHABET[(((b1 & 0x0F) << 2) | (b2 >> 6)) as usize] as char);
            }
            if i + 2 < bytes.len() {
                out.push(ALPHABET[(b2 & 0x3F) as usize] as char);
            }
            i += 3;
        }
        out
    }

    fn make_test_descriptor(
        id: &str,
        name: &str,
        transport_url: &str,
        official: bool,
    ) -> Descriptor {
        use stremio_core::types::addon::{DescriptorFlags, Manifest, Version};
        Descriptor {
            manifest: Manifest {
                id: id.to_owned(),
                version: Version::new(1, 0, 0),
                name: name.to_owned(),
                contact_email: None,
                description: Some("Fast Debrid & torrent stream indexer".to_owned()),
                logo: None,
                background: None,
                types: vec!["movie".to_owned(), "series".to_owned()],
                resources: vec![ManifestResource::Short("stream".to_owned())],
                id_prefixes: None,
                catalogs: vec![],
                addon_catalogs: vec![],
                behavior_hints: Default::default(),
            },
            transport_url: Url::parse(transport_url).unwrap(),
            flags: DescriptorFlags {
                official,
                protected: official,
            },
        }
    }

    #[test]
    fn detects_debrid_configuration_accurately_across_torrentio_comet_jackettio_and_dedicated_addons()
    {
        let cinemeta = make_test_descriptor(
            "com.linvo.cinemeta",
            "Cinemeta",
            "https://v3-cinemeta.strem.io/manifest.json",
            true,
        );
        let opensubs = make_test_descriptor(
            "org.stremio.opensubtitlesv3",
            "OpenSubtitles v3",
            "https://opensubtitles-v3.strem.io/manifest.json",
            false,
        );
        assert!(!addon_has_debrid_configured(&cinemeta));
        assert!(!addon_has_debrid_configured(&opensubs));

        let torrentio_bare = make_test_descriptor(
            "com.stremio.torrentio.addon",
            "Torrentio",
            "https://torrentio.strem.fun/manifest.json",
            false,
        );
        let torrentio_quality_only = make_test_descriptor(
            "com.stremio.torrentio.addon",
            "Torrentio",
            "https://torrentio.strem.fun/sort=qualitysize|qualityfilter=scr,cam|debridoptions=nodownloadlinks/manifest.json",
            false,
        );
        assert!(!addon_has_debrid_configured(&torrentio_bare));
        assert!(!addon_has_debrid_configured(&torrentio_quality_only));

        let torrentio_rd = make_test_descriptor(
            "com.stremio.torrentio.addon",
            "Torrentio",
            "https://torrentio.strem.fun/sort=qualitysize|qualityfilter=cam|realdebrid=0123456789abcdef/manifest.json",
            false,
        );
        assert!(addon_has_debrid_configured(&torrentio_rd));

        let comet_p2p_b64 = encode_base64_url_unpadded(
            br#"{"maxResultsPerResolution":0,"debridService":"torrent","debridApiKey":""}"#,
        );
        let comet_p2p = make_test_descriptor(
            "stremio.comet.fast",
            "Comet",
            &format!("https://comet.elfhosted.com/{comet_p2p_b64}/manifest.json"),
            false,
        );
        let jackettio_p2p_b64 = encode_base64_url_unpadded(
            br#"{"qualities":[1080,2160],"debridId":"","debridApiKey":""}"#,
        );
        let jackettio_p2p = make_test_descriptor(
            "community.jackettio",
            "Jackettio",
            &format!("https://jackettio.elfhosted.com/{jackettio_p2p_b64}/manifest.json"),
            false,
        );
        assert!(!addon_has_debrid_configured(&comet_p2p));
        assert!(!addon_has_debrid_configured(&jackettio_p2p));

        let comet_rd_b64 = encode_base64_url_unpadded(
            br#"{"maxResultsPerResolution":0,"debridService":"realdebrid","debridApiKey":"rd_live_token_99"}"#,
        );
        let comet_rd = make_test_descriptor(
            "stremio.comet.fast",
            "Comet",
            &format!("https://comet.elfhosted.com/{comet_rd_b64}/manifest.json"),
            false,
        );
        let jackettio_tb_b64 = encode_base64_url_unpadded(
            br#"{"qualities":[1080,2160],"debridId":"torbox","debridApiKey":"tb_live_token_77"}"#,
        );
        let jackettio_tb = make_test_descriptor(
            "community.jackettio",
            "Jackettio",
            &format!("https://jackettio.elfhosted.com/{jackettio_tb_b64}/manifest.json"),
            false,
        );
        assert!(addon_has_debrid_configured(&comet_rd));
        assert!(addon_has_debrid_configured(&jackettio_tb));

        let torbox = make_test_descriptor(
            "com.torbox.stremio",
            "TorBox",
            "https://stremio.torbox.app/uuid-secret-key/manifest.json",
            false,
        );
        let debrid_search = make_test_descriptor(
            "org.stremio.debridsearch",
            "DebridSearch",
            "https://68d69db7dc40-debrid-search.baby-beamup.club/eyJ0b2tlbiI6MX0/manifest.json",
            false,
        );
        assert!(addon_has_debrid_configured(&torbox));
        assert!(addon_has_debrid_configured(&debrid_search));

        assert!(!has_debrid_configured_addons(&[
            cinemeta.clone(),
            opensubs.clone(),
            torrentio_quality_only,
            comet_p2p,
        ]));
        assert!(has_debrid_configured_addons(&[
            cinemeta,
            opensubs,
            torrentio_rd,
        ]));
    }

    #[test]
    fn cache_enforces_6mib_budget_and_512kib_single_entry_cap() {
        assert_eq!(ADDON_CACHE_MAX_BYTES, 6 * 1024 * 1024);
        assert_eq!(ADDON_CACHE_MAX_SINGLE_ENTRY_BYTES, 512 * 1024);

        let now = Instant::now();
        let mut cache = CacheInner::default();

        let oversized: Arc<[u8]> = Arc::from(vec![b'x'; ADDON_CACHE_MAX_SINGLE_ENTRY_BYTES + 1]);
        cache.insert(
            "https://example.com/catalog/movie/huge.json".to_owned(),
            oversized,
            TTL_CATALOG,
            now,
        );
        assert_eq!(cache.map.len(), 0);
        assert_eq!(cache.total_bytes, 0);

        let chunk_512k: Arc<[u8]> = Arc::from(vec![b'a'; ADDON_CACHE_MAX_SINGLE_ENTRY_BYTES]);
        for i in 0..14 {
            cache.insert(
                format!("https://example.com/catalog/movie/shelf_{i}.json"),
                Arc::clone(&chunk_512k),
                TTL_CATALOG,
                now,
            );
        }
        assert!(cache.total_bytes <= ADDON_CACHE_MAX_BYTES);
        assert_eq!(cache.map.len(), 12);
        assert!(cache
            .get_fresh("https://example.com/catalog/movie/shelf_0.json", now)
            .is_none());
        assert!(cache
            .get_fresh("https://example.com/catalog/movie/shelf_13.json", now)
            .is_some());
    }

    #[test]
    fn redacts_tokens_credentials_and_sensitive_text_from_urls_and_logs() {
        let rd_url = Url::parse(
            "https://torrentio.strem.fun/sort=qualitysize|realdebrid=RD_SUPER_SECRET_KEY_123/manifest.json?apikey=XYZ",
        )
        .unwrap();
        let redacted = redact_url(&rd_url);
        assert_eq!(redacted, "https://torrentio.strem.fun/...");
        assert!(!redacted.contains("RD_SUPER_SECRET_KEY_123"));
        assert!(!redacted.contains("XYZ"));

        let local_with_creds =
            Url::parse("http://admin:hunter2@127.0.0.1:11470/secret/token").unwrap();
        let redacted_local = redact_url(&local_with_creds);
        assert_eq!(redacted_local, "http://127.0.0.1:11470/...");
        assert!(!redacted_local.contains("hunter2"));
        assert!(!redacted_local.contains("admin"));

        let magnet = Url::parse("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567")
            .unwrap();
        assert_eq!(redact_url(&magnet), "magnet:...");

        let debug_dump = r#"Action(Ctx(Authenticate(Login { email: "u@example.com", password: "my_secret_password" }))) AuthKey("auth_live_999") transport_url: "https://comet.elfhosted.com/eyJ0b2tlbiI6InNlY3JldCJ9/manifest.json""#;
        let sanitized = redact_sensitive_text(debug_dump);
        assert!(!sanitized.contains("my_secret_password"));
        assert!(!sanitized.contains("auth_live_999"));
        assert!(!sanitized.contains("eyJ0b2tlbiI6InNlY3JldCJ9"));
        assert!(sanitized.contains("https://comet.elfhosted.com/..."));
        assert!(sanitized.contains("AuthKey(\"[REDACTED]\")"));
    }
}
