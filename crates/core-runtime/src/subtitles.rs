use std::{
    cmp::Ordering,
    fmt,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering as AtomicOrdering},
        Arc, RwLock,
    },
    time::{Duration, Instant},
};

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use flate2::read::{DeflateDecoder, GzDecoder};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::{debug, warn};
use url::Url;

use crate::{private_config::redact_secret, stream_parser::is_allowed_addon_stream_url};

pub const DEFAULT_OPENSUBTITLES_V3_BASE: &str = "https://opensubtitles-v3.strem.io";
pub const DEFAULT_OPENSUBTITLES_API_BASE: &str = "https://api.opensubtitles.com/api/v1";
pub const DEFAULT_SUBDL_API_BASE: &str = "https://api.subdl.com/api/v1";
pub const DEFAULT_SUBDL_DL_BASE: &str = "https://dl.subdl.com";

pub const NATIVE_SUBTITLE_HTTP_TIMEOUT_MS: u64 = 2_800;
pub const NATIVE_SUBTITLE_POOL_IDLE_SECS: u64 = 90;
pub const MAX_SUBTITLE_ARCHIVE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_UNCOMPRESSED_SUBTITLE_BYTES: usize = 16 * 1024 * 1024;

const ALIGN_MAX_OFFSET_MS: i64 = 45_000;
const ALIGN_BIN_MS: i64 = 50;
const ALIGN_BIN_COUNT: usize = ((ALIGN_MAX_OFFSET_MS * 2) / ALIGN_BIN_MS) as usize + 3;
const ALIGN_ONSET_TOLERANCE_MS: i64 = 250;
const ALIGN_FINE_WINDOW_MS: i64 = 150;
const ALIGN_DRIFT_CANDIDATES: [f64; 4] = [
    1.0,
    25.0 / 23.976,
    23.976 / 25.0,
    24.0 / 23.976,
];

static TEMP_SUB_SEQ: AtomicU64 = AtomicU64::new(0);

static RE_SUB_RELEASE_GROUP: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"-([A-Za-z0-9]{2,20})(?:\[(?:rarbg|eztv|yts|tgx|ettv)\])?(?:\.[a-zA-Z0-9]{2,4})?$")
        .unwrap()
});
static RE_EPISODE_TAG: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:s(\d{1,2})e(\d{1,3})|(\d{1,2})x(\d{1,3}))").unwrap());
static RE_HTML_TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());
static RE_ASS_OVERRIDE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\{[^}]*\}").unwrap());

/// Maps ISO-639-1/2/name subtitle language tags into `(iso639_1, iso639_2)` pairs.
const LANG_MAP: &[(&str, &str, &[&str])] = &[
    ("en", "eng", &["en", "eng", "english"]),
    ("sl", "slv", &["sl", "slv", "slovenian", "slovene"]),
    ("es", "spa", &["es", "spa", "spanish", "espanol", "castellano"]),
    ("fr", "fre", &["fr", "fre", "fra", "french", "francais"]),
    ("de", "ger", &["de", "ger", "deu", "german", "deutsch"]),
    ("it", "ita", &["it", "ita", "italian", "italiano"]),
    ("pt", "por", &["pt", "por", "pob", "portuguese", "portugues", "pt-br", "pt-pt"]),
    ("ja", "jpn", &["ja", "jpn", "japanese"]),
    ("ko", "kor", &["ko", "kor", "korean"]),
    ("zh", "zho", &["zh", "zho", "chi", "chinese", "zh-cn", "zh-tw", "chs", "cht"]),
    ("ru", "rus", &["ru", "rus", "russian"]),
    ("nl", "nld", &["nl", "nld", "dut", "dutch"]),
    ("pl", "pol", &["pl", "pol", "polish"]),
    ("tr", "tur", &["tr", "tur", "turkish"]),
    ("ar", "ara", &["ar", "ara", "arabic"]),
    ("he", "heb", &["he", "heb", "iw", "hebrew"]),
    ("hi", "hin", &["hi", "hin", "hindi"]),
    ("sv", "swe", &["sv", "swe", "swedish"]),
    ("no", "nor", &["no", "nor", "nob", "nno", "norwegian"]),
    ("da", "dan", &["da", "dan", "danish"]),
    ("fi", "fin", &["fi", "fin", "finnish"]),
    ("cs", "ces", &["cs", "ces", "cze", "czech"]),
    ("hu", "hun", &["hu", "hun", "hungarian"]),
    ("ro", "ron", &["ro", "ron", "rum", "romanian"]),
    ("el", "ell", &["el", "ell", "gre", "greek"]),
    ("uk", "ukr", &["uk", "ukr", "ukrainian"]),
    ("bg", "bul", &["bg", "bul", "bulgarian"]),
    ("hr", "hrv", &["hr", "hrv", "croatian"]),
    ("sr", "srp", &["sr", "srp", "scc", "serbian"]),
    ("th", "tha", &["th", "tha", "thai"]),
    ("vi", "vie", &["vi", "vie", "vietnamese"]),
    ("id", "ind", &["id", "ind", "indonesian"]),
    ("ms", "msa", &["ms", "msa", "may", "malay"]),
];

/// Converts a 2-letter, 3-letter, or English language name to its canonical ISO-639-1 2-letter code.
pub fn normalize_subtitle_lang_iso639_1(code: &str) -> Option<&'static str> {
    let cleaned = code.trim().to_ascii_lowercase().replace('_', "-");
    if cleaned.is_empty() {
        return None;
    }
    let primary = cleaned.split('-').next().unwrap_or(&cleaned);
    for &(iso1, _iso2, aliases) in LANG_MAP {
        if aliases.contains(&cleaned.as_str()) || aliases.contains(&primary) {
            return Some(iso1);
        }
    }
    None
}

/// Converts a 2-letter, 3-letter, or English language name to its canonical ISO-639-2 3-letter code.
pub fn normalize_subtitle_lang_iso639_2(code: &str) -> Option<&'static str> {
    let cleaned = code.trim().to_ascii_lowercase().replace('_', "-");
    if cleaned.is_empty() {
        return None;
    }
    let primary = cleaned.split('-').next().unwrap_or(&cleaned);
    for &(_iso1, iso2, aliases) in LANG_MAP {
        if aliases.contains(&cleaned.as_str()) || aliases.contains(&primary) {
            return Some(iso2);
        }
    }
    None
}

/// Normalizes a subtitle language string to lowercase ISO-639-1 if known, otherwise trimmed lowercase.
pub fn normalize_subtitle_lang_code(code: &str) -> String {
    if let Some(iso1) = normalize_subtitle_lang_iso639_1(code) {
        return iso1.to_owned();
    }
    code.trim()
        .to_ascii_lowercase()
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_owned()
}

/// Returns true if two subtitle language tags refer to the same language (e.g. `"eng"` and `"en"`).
pub fn subtitle_lang_matches(a: &str, b: &str) -> bool {
    let na = normalize_subtitle_lang_code(a);
    let nb = normalize_subtitle_lang_code(b);
    !na.is_empty() && na == nb
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSubtitleQuery {
    pub imdb_id: String,
    pub media_type: String,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub opensubhash: Option<String>,
    pub video_byte_size: Option<u64>,
    pub video_filename: Option<String>,
    pub preferred_langs: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SubtitleSourceCut {
    #[default]
    Unknown,
    Hdtv,
    WebRip,
    WebDl,
    BdRip,
    BluRay,
    Remux,
}

impl SubtitleSourceCut {
    fn family(self) -> Option<CutFamily> {
        match self {
            Self::Unknown => None,
            Self::WebDl | Self::WebRip => Some(CutFamily::Web),
            Self::BluRay | Self::Remux | Self::BdRip => Some(CutFamily::Optical),
            Self::Hdtv => Some(CutFamily::Broadcast),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CutFamily {
    Web,
    Optical,
    Broadcast,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StreamingServiceTag {
    Netflix,
    Amazon,
    AppleTv,
    DisneyPlus,
    Max,
    Hulu,
    Peacock,
    Crunchyroll,
    Bbc,
    Itv,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReleaseEdition {
    Extended,
    DirectorsCut,
    Uncut,
    Unrated,
    Imax,
    Remastered,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FpsHint {
    Fps23976,
    Fps24000,
    Fps25000,
    Fps29970,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SubtitleMatchTier {
    ExactHash,
    ExactRelease,
    SameServiceAndCut,
    CompatibleCut,
    Fallback,
}

impl SubtitleMatchTier {
    pub fn display_label(self) -> &'static str {
        match self {
            Self::ExactHash => "Hash-Matched · 0ms",
            Self::ExactRelease => "Release-Matched",
            Self::SameServiceAndCut | Self::CompatibleCut => "Cut-Matched",
            Self::Fallback => "Fallback",
        }
    }
}

impl fmt::Display for SubtitleMatchTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_label())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoReleaseFingerprint {
    pub release_group: Option<String>,
    pub streaming_service: Option<StreamingServiceTag>,
    pub source_cut: SubtitleSourceCut,
    pub edition: Option<ReleaseEdition>,
    pub fps_hint: Option<FpsHint>,
}

impl VideoReleaseFingerprint {
    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Self::default();
        }
        let lower = trimmed.to_ascii_lowercase();
        let tokens: Vec<String> = trimmed
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '+')
            .filter(|t| !t.is_empty())
            .map(|t| t.to_ascii_lowercase())
            .collect();

        let release_group = extract_subtitle_release_group(trimmed);
        let streaming_service = detect_streaming_service(&tokens, &lower);
        let source_cut = detect_source_cut(&tokens, &lower);
        let edition = detect_edition(&tokens, &lower);
        let fps_hint = detect_fps_hint(&tokens, &lower);

        Self {
            release_group,
            streaming_service,
            source_cut,
            edition,
            fps_hint,
        }
    }
}

fn extract_subtitle_release_group(raw: &str) -> Option<String> {
    let base_name = Path::new(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(raw)
        .trim();
    let group = RE_SUB_RELEASE_GROUP.captures(base_name)?.get(1)?.as_str();
    let lower = group.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "dl" | "rip"
            | "hd"
            | "ma"
            | "264"
            | "265"
            | "x264"
            | "x265"
            | "h264"
            | "h265"
            | "10bit"
            | "hi"
            | "sdh"
            | "forced"
            | "1"
            | "2"
            | "3"
    ) {
        return None;
    }
    Some(group.to_owned())
}

fn has_token(tokens: &[String], needle: &str) -> bool {
    tokens.iter().any(|t| t == needle)
}

fn detect_streaming_service(tokens: &[String], lower: &str) -> Option<StreamingServiceTag> {
    if has_token(tokens, "nf") || has_token(tokens, "netflix") {
        Some(StreamingServiceTag::Netflix)
    } else if has_token(tokens, "amzn") || has_token(tokens, "amazon") {
        Some(StreamingServiceTag::Amazon)
    } else if has_token(tokens, "atvp") || lower.contains("appletv") || lower.contains("apple.tv") {
        Some(StreamingServiceTag::AppleTv)
    } else if has_token(tokens, "dsnp") || has_token(tokens, "disneyplus") || lower.contains("disney+") {
        Some(StreamingServiceTag::DisneyPlus)
    } else if has_token(tokens, "hmax") || has_token(tokens, "hbomax") || has_token(tokens, "max") {
        Some(StreamingServiceTag::Max)
    } else if has_token(tokens, "hulu") {
        Some(StreamingServiceTag::Hulu)
    } else if has_token(tokens, "pcok") || has_token(tokens, "peacock") {
        Some(StreamingServiceTag::Peacock)
    } else if has_token(tokens, "cr") || has_token(tokens, "crunchyroll") {
        Some(StreamingServiceTag::Crunchyroll)
    } else if has_token(tokens, "bbc") {
        Some(StreamingServiceTag::Bbc)
    } else if has_token(tokens, "itv") {
        Some(StreamingServiceTag::Itv)
    } else {
        None
    }
}

fn detect_source_cut(tokens: &[String], lower: &str) -> SubtitleSourceCut {
    if has_token(tokens, "remux") || has_token(tokens, "bdremux") || has_token(tokens, "uhdremux") {
        SubtitleSourceCut::Remux
    } else if has_token(tokens, "bdrip") || has_token(tokens, "brrip") {
        SubtitleSourceCut::BdRip
    } else if has_token(tokens, "bluray")
        || has_token(tokens, "bdmux")
        || lower.contains("blu-ray")
        || lower.contains("blu.ray")
        || lower.contains("blu_ray")
    {
        SubtitleSourceCut::BluRay
    } else if has_token(tokens, "webdl")
        || lower.contains("web-dl")
        || lower.contains("web.dl")
        || lower.contains("web_dl")
    {
        SubtitleSourceCut::WebDl
    } else if has_token(tokens, "webrip")
        || has_token(tokens, "webmux")
        || lower.contains("web-rip")
        || lower.contains("web.rip")
        || lower.contains("web_rip")
    {
        SubtitleSourceCut::WebRip
    } else if has_token(tokens, "hdtv") || has_token(tokens, "pdtv") || has_token(tokens, "tvrip") {
        SubtitleSourceCut::Hdtv
    } else if has_token(tokens, "web") {
        SubtitleSourceCut::WebDl
    } else {
        SubtitleSourceCut::Unknown
    }
}

fn detect_edition(tokens: &[String], lower: &str) -> Option<ReleaseEdition> {
    if has_token(tokens, "extended") {
        Some(ReleaseEdition::Extended)
    } else if lower.contains("directors.cut")
        || lower.contains("directors-cut")
        || lower.contains("directors_cut")
        || lower.contains("directors cut")
        || lower.contains("director's cut")
        || has_token(tokens, "dc")
    {
        Some(ReleaseEdition::DirectorsCut)
    } else if has_token(tokens, "uncut") {
        Some(ReleaseEdition::Uncut)
    } else if has_token(tokens, "unrated") {
        Some(ReleaseEdition::Unrated)
    } else if has_token(tokens, "imax") {
        Some(ReleaseEdition::Imax)
    } else if has_token(tokens, "remastered") {
        Some(ReleaseEdition::Remastered)
    } else {
        None
    }
}

fn detect_fps_hint(tokens: &[String], lower: &str) -> Option<FpsHint> {
    if lower.contains("23.976") || lower.contains("23.98") || has_token(tokens, "23976") {
        Some(FpsHint::Fps23976)
    } else if lower.contains("25.000")
        || lower.contains("25.00")
        || lower.contains("25fps")
        || has_token(tokens, "25000")
        || has_token(tokens, "pal")
    {
        Some(FpsHint::Fps25000)
    } else if lower.contains("29.970")
        || lower.contains("29.97")
        || has_token(tokens, "29970")
        || has_token(tokens, "ntsc")
    {
        Some(FpsHint::Fps29970)
    } else if lower.contains("24.000")
        || lower.contains("24fps")
        || has_token(tokens, "24000")
    {
        Some(FpsHint::Fps24000)
    } else {
        None
    }
}

#[derive(Clone, Debug, Default)]
pub struct SubtitleMatchRanker;

impl SubtitleMatchRanker {
    pub fn score_candidate(
        video: Option<&VideoReleaseFingerprint>,
        candidate: &VideoReleaseFingerprint,
        exact_hash_match: bool,
        candidate_lang: &str,
        preferred_langs: &[String],
        download_count: Option<u64>,
        hearing_impaired: bool,
    ) -> (SubtitleMatchTier, i64) {
        let mut score: i64 = 0;

        // Language preference boost
        if let Some(pos) = preferred_langs
            .iter()
            .position(|pref| subtitle_lang_matches(pref, candidate_lang))
        {
            if pos == 0 {
                score += 500_000;
            } else {
                score += 250_000;
            }
        }

        if let Some(downloads) = download_count {
            score += downloads.min(50_000) as i64;
        }
        if hearing_impaired {
            score -= 5_000;
        }

        if exact_hash_match {
            score += 10_000_000;
            return (SubtitleMatchTier::ExactHash, score);
        }

        let Some(video) = video else {
            return (SubtitleMatchTier::Fallback, score);
        };

        // Anti-desync penalties
        let mut has_hard_conflict = false;

        if video.edition != candidate.edition
            && (video.edition.is_some() || candidate.edition.is_some())
        {
            score -= 2_000_000;
            has_hard_conflict = true;
        }

        if let (Some(v_fps), Some(c_fps)) = (video.fps_hint, candidate.fps_hint) {
            if v_fps != c_fps {
                score -= 2_500_000;
                has_hard_conflict = true;
            }
        }

        let v_family = video.source_cut.family();
        let c_family = candidate.source_cut.family();
        if let (Some(vf), Some(cf)) = (v_family, c_family) {
            if vf != cf {
                score -= 1_500_000;
                has_hard_conflict = true;
            }
        }

        if let (Some(vs), Some(cs)) = (video.streaming_service, candidate.streaming_service) {
            if vs != cs {
                score -= 900_000;
                has_hard_conflict = true;
            }
        }

        let release_group_match = match (
            video.release_group.as_deref(),
            candidate.release_group.as_deref(),
        ) {
            (Some(vg), Some(cg)) => vg.eq_ignore_ascii_case(cg),
            _ => false,
        };

        let same_service_and_cut = video.streaming_service.is_some()
            && video.streaming_service == candidate.streaming_service
            && v_family.is_some()
            && v_family == c_family;

        let compatible_cut = v_family.is_some() && v_family == c_family;

        let tier = if release_group_match && !has_hard_conflict {
            score += 4_000_000;
            SubtitleMatchTier::ExactRelease
        } else if same_service_and_cut && !has_hard_conflict {
            score += 1_800_000;
            SubtitleMatchTier::SameServiceAndCut
        } else if compatible_cut && !has_hard_conflict {
            score += 600_000;
            SubtitleMatchTier::CompatibleCut
        } else {
            if release_group_match {
                score += 4_000_000;
            } else if same_service_and_cut {
                score += 1_800_000;
            }
            SubtitleMatchTier::Fallback
        };

        (tier, score)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeSubtitleEndpoints {
    pub opensubtitles_v3_base: String,
    pub opensubtitles_api_base: String,
    pub subdl_api_base: String,
    pub subdl_dl_base: String,
}

impl Default for NativeSubtitleEndpoints {
    fn default() -> Self {
        Self {
            opensubtitles_v3_base: DEFAULT_OPENSUBTITLES_V3_BASE.to_owned(),
            opensubtitles_api_base: DEFAULT_OPENSUBTITLES_API_BASE.to_owned(),
            subdl_api_base: DEFAULT_SUBDL_API_BASE.to_owned(),
            subdl_dl_base: DEFAULT_SUBDL_DL_BASE.to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchedNativeSubtitle {
    pub provider: String,
    pub lang: String,
    pub title: String,
    pub release_name: String,
    pub match_tier: SubtitleMatchTier,
    pub score: i64,
    pub local_path: PathBuf,
    pub lookup_elapsed_ms: u64,
}

#[derive(Clone, Debug)]
enum ProviderDownloadSpec {
    DirectUrl(String),
    OpenSubtitlesFileId { file_id: u64, api_key: String },
}

#[derive(Clone, Debug)]
struct RankedSubtitleCandidate {
    provider: String,
    lang: String,
    title: String,
    release_name: String,
    match_tier: SubtitleMatchTier,
    score: i64,
    download: ProviderDownloadSpec,
}

#[derive(Clone, Debug)]
pub struct NativeSubtitleClient {
    client: reqwest::Client,
    cache_dir: PathBuf,
    endpoints: Arc<RwLock<NativeSubtitleEndpoints>>,
}

impl NativeSubtitleClient {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self::with_endpoints(cache_dir, NativeSubtitleEndpoints::default())
    }

    pub fn with_endpoints(
        cache_dir: impl Into<PathBuf>,
        endpoints: NativeSubtitleEndpoints,
    ) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(format!("Ferrite/{}", env!("CARGO_PKG_VERSION")))
            .tcp_nodelay(true)
            .pool_idle_timeout(Duration::from_secs(NATIVE_SUBTITLE_POOL_IDLE_SECS))
            .timeout(Duration::from_millis(NATIVE_SUBTITLE_HTTP_TIMEOUT_MS))
            .http2_adaptive_window(true)
            .gzip(true)
            .deflate(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            cache_dir: cache_dir.into(),
            endpoints: Arc::new(RwLock::new(endpoints)),
        }
    }

    pub fn set_endpoints(&self, endpoints: NativeSubtitleEndpoints) {
        if let Ok(mut guard) = self.endpoints.write() {
            *guard = endpoints;
        }
    }

    pub fn endpoints(&self) -> NativeSubtitleEndpoints {
        self.endpoints
            .read()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    pub async fn search_and_fetch(
        &self,
        query: &NativeSubtitleQuery,
        opensubtitles_api_key: Option<&str>,
        subdl_api_key: Option<&str>,
        max_downloads: usize,
    ) -> Vec<FetchedNativeSubtitle> {
        if query.imdb_id.trim().is_empty() || max_downloads == 0 {
            return Vec::new();
        }

        let started = Instant::now();
        let endpoints = self.endpoints();
        let os_key = opensubtitles_api_key
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let sdl_key = subdl_api_key.map(str::trim).filter(|s| !s.is_empty());

        let video_fp = query
            .video_filename
            .as_deref()
            .map(VideoReleaseFingerprint::parse);

        let (os_v3_res, os_rest_res, subdl_res) = tokio::join!(
            self.query_opensubtitles_v3(&endpoints.opensubtitles_v3_base, query, video_fp.as_ref()),
            self.query_opensubtitles_rest(
                &endpoints.opensubtitles_api_base,
                os_key,
                query,
                video_fp.as_ref()
            ),
            self.query_subdl(
                &endpoints.subdl_api_base,
                &endpoints.subdl_dl_base,
                sdl_key,
                query,
                video_fp.as_ref()
            ),
        );

        let mut candidates = Vec::new();
        candidates.extend(os_v3_res);
        candidates.extend(os_rest_res);
        candidates.extend(subdl_res);

        if candidates.is_empty() {
            return Vec::new();
        }

        candidates.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.match_tier.cmp(&b.match_tier))
                .then_with(|| a.provider.cmp(&b.provider))
                .then_with(|| a.release_name.cmp(&b.release_name))
        });

        let attempt_limit = max_downloads.saturating_mul(2).max(max_downloads);
        let to_fetch: Vec<RankedSubtitleCandidate> =
            candidates.into_iter().take(attempt_limit).collect();

        let fetch_futs = to_fetch.into_iter().map(|cand| {
            let endpoints_clone = endpoints.clone();
            async move { self.download_and_cache(cand, &endpoints_clone, query).await }
        });

        let downloaded = futures::future::join_all(fetch_futs).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;

        let mut out: Vec<FetchedNativeSubtitle> = downloaded
            .into_iter()
            .flatten()
            .map(|mut item| {
                item.lookup_elapsed_ms = elapsed_ms;
                item
            })
            .collect();

        out.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.match_tier.cmp(&b.match_tier))
                .then_with(|| a.provider.cmp(&b.provider))
        });
        out.truncate(max_downloads);
        out
    }

    async fn query_opensubtitles_v3(
        &self,
        base_url: &str,
        query: &NativeSubtitleQuery,
        video_fp: Option<&VideoReleaseFingerprint>,
    ) -> Vec<RankedSubtitleCandidate> {
        let base = base_url.trim_end_matches('/');
        if base.is_empty() {
            return Vec::new();
        }
        let media_type = if query.media_type.trim().is_empty() {
            if query.season.is_some() && query.episode.is_some() {
                "series"
            } else {
                "movie"
            }
        } else {
            query.media_type.trim()
        };

        let stremio_id = match (query.season, query.episode) {
            (Some(s), Some(e)) => format!("{}:{}:{}", query.imdb_id.trim(), s, e),
            _ => query.imdb_id.trim().to_owned(),
        };

        let mut extra_parts = Vec::new();
        if let Some(hash) = query
            .opensubhash
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            extra_parts.push(format!("videoHash={}", urlencoding::encode(hash)));
        }
        if let Some(size) = query.video_byte_size.filter(|&b| b > 0) {
            extra_parts.push(format!("videoSize={size}"));
        }

        let url = if extra_parts.is_empty() {
            format!("{base}/subtitles/{media_type}/{stremio_id}.json")
        } else {
            format!(
                "{base}/subtitles/{media_type}/{stremio_id}/{}.json",
                extra_parts.join("&")
            )
        };

        let resp = match self.client.get(&url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                debug!("OpenSubtitles v3 returned status {}", r.status());
                return Vec::new();
            }
            Err(err) => {
                debug!("OpenSubtitles v3 lookup failed: {err}");
                return Vec::new();
            }
        };

        let json: Value = match resp.json().await {
            Ok(v) => v,
            Err(err) => {
                debug!("OpenSubtitles v3 invalid JSON: {err}");
                return Vec::new();
            }
        };

        let Some(items) = json.get("subtitles").and_then(Value::as_array) else {
            return Vec::new();
        };

        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let Some(sub_url) = item.get("url").and_then(Value::as_str).map(str::trim) else {
                continue;
            };
            if !is_allowed_addon_stream_url(sub_url) {
                continue;
            }
            let lang_raw = item
                .get("lang")
                .and_then(Value::as_str)
                .unwrap_or("und");
            let lang = normalize_subtitle_lang_code(lang_raw);
            if !query.preferred_langs.is_empty()
                && !query
                    .preferred_langs
                    .iter()
                    .any(|p| subtitle_lang_matches(p, &lang))
            {
                continue;
            }

            let release_name = item
                .get("SubFileName")
                .or_else(|| item.get("release"))
                .or_else(|| item.get("title"))
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("OpenSubtitles v3")
                .trim()
                .to_owned();
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&release_name)
                .to_owned();
            let hash_match = item
                .get("hashMatch")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || (query.opensubhash.is_some()
                    && item
                        .get("MovieHash")
                        .and_then(Value::as_str)
                        .zip(query.opensubhash.as_deref())
                        .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b)));

            let cand_fp = VideoReleaseFingerprint::parse(&release_name);
            let (tier, score) = SubtitleMatchRanker::score_candidate(
                video_fp,
                &cand_fp,
                hash_match,
                &lang,
                &query.preferred_langs,
                None,
                false,
            );

            out.push(RankedSubtitleCandidate {
                provider: "OpenSubtitles v3".to_owned(),
                lang,
                title,
                release_name,
                match_tier: tier,
                score,
                download: ProviderDownloadSpec::DirectUrl(sub_url.to_owned()),
            });
        }
        out
    }

    async fn query_opensubtitles_rest(
        &self,
        base_url: &str,
        api_key: Option<&str>,
        query: &NativeSubtitleQuery,
        video_fp: Option<&VideoReleaseFingerprint>,
    ) -> Vec<RankedSubtitleCandidate> {
        let Some(key) = api_key else {
            return Vec::new();
        };
        let base = base_url.trim_end_matches('/');
        if base.is_empty() {
            return Vec::new();
        }
        let secrets = [key];

        let mut url = match Url::parse(&format!("{base}/subtitles")) {
            Ok(u) => u,
            Err(_) => return Vec::new(),
        };

        let imdb_numeric = query.imdb_id.trim().trim_start_matches("tt");
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("imdb_id", imdb_numeric);
            if let Some(season) = query.season {
                pairs.append_pair("season_number", &season.to_string());
            }
            if let Some(episode) = query.episode {
                pairs.append_pair("episode_number", &episode.to_string());
            }
            if let Some(hash) = query
                .opensubhash
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                pairs.append_pair("moviehash", hash);
            }
            let langs_csv: Vec<&str> = query
                .preferred_langs
                .iter()
                .filter_map(|l| normalize_subtitle_lang_iso639_1(l))
                .collect();
            if !langs_csv.is_empty() {
                pairs.append_pair("languages", &langs_csv.join(","));
            }
        }

        let resp = match self
            .client
            .get(url)
            .header("Api-Key", key)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                debug!("OpenSubtitles REST returned status {}", r.status());
                return Vec::new();
            }
            Err(err) => {
                debug!(
                    "OpenSubtitles REST lookup failed: {}",
                    redact_secret(&err.to_string(), &secrets)
                );
                return Vec::new();
            }
        };

        let json: Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let Some(data) = json.get("data").and_then(Value::as_array) else {
            return Vec::new();
        };

        let mut out = Vec::with_capacity(data.len());
        for item in data {
            let Some(attrs) = item.get("attributes") else {
                continue;
            };
            let lang = normalize_subtitle_lang_code(
                attrs
                    .get("language")
                    .and_then(Value::as_str)
                    .unwrap_or("und"),
            );
            let release_name = attrs
                .get("release")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let first_file = attrs
                .get("files")
                .and_then(Value::as_array)
                .and_then(|files| files.first());
            let Some(file_id) = first_file
                .and_then(|f| f.get("file_id"))
                .and_then(Value::as_u64)
            else {
                continue;
            };
            let file_name = first_file
                .and_then(|f| f.get("file_name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());

            let effective_release = release_name
                .or_else(|| file_name.map(str::to_owned))
                .unwrap_or_else(|| format!("OpenSubtitles #{file_id}"));
            let title = file_name
                .map(str::to_owned)
                .unwrap_or_else(|| effective_release.clone());

            let hash_match = attrs
                .get("moviehash_match")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let downloads = attrs.get("download_count").and_then(Value::as_u64);
            let hi = attrs
                .get("hearing_impaired")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            let cand_fp = VideoReleaseFingerprint::parse(&effective_release);
            let (tier, score) = SubtitleMatchRanker::score_candidate(
                video_fp,
                &cand_fp,
                hash_match,
                &lang,
                &query.preferred_langs,
                downloads,
                hi,
            );

            out.push(RankedSubtitleCandidate {
                provider: "OpenSubtitles API".to_owned(),
                lang,
                title,
                release_name: effective_release,
                match_tier: tier,
                score,
                download: ProviderDownloadSpec::OpenSubtitlesFileId {
                    file_id,
                    api_key: key.to_owned(),
                },
            });
        }
        out
    }

    async fn query_subdl(
        &self,
        api_base: &str,
        dl_base: &str,
        api_key: Option<&str>,
        query: &NativeSubtitleQuery,
        video_fp: Option<&VideoReleaseFingerprint>,
    ) -> Vec<RankedSubtitleCandidate> {
        let Some(key) = api_key else {
            return Vec::new();
        };
        let base = api_base.trim_end_matches('/');
        if base.is_empty() {
            return Vec::new();
        }
        let secrets = [key];

        let mut url = match Url::parse(&format!("{base}/subtitles")) {
            Ok(u) => u,
            Err(_) => return Vec::new(),
        };

        let subdl_type = match query.media_type.trim().to_ascii_lowercase().as_str() {
            "series" | "tv" => "tv",
            _ => "movie",
        };
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("api_key", key);
            pairs.append_pair("imdb_id", query.imdb_id.trim());
            pairs.append_pair("type", subdl_type);
            if let Some(season) = query.season {
                pairs.append_pair("season_number", &season.to_string());
            }
            if let Some(episode) = query.episode {
                pairs.append_pair("episode_number", &episode.to_string());
            }
            let langs: Vec<String> = query
                .preferred_langs
                .iter()
                .filter_map(|l| normalize_subtitle_lang_iso639_1(l))
                .map(|c| c.to_ascii_uppercase())
                .collect();
            if !langs.is_empty() {
                pairs.append_pair("languages", &langs.join(","));
            }
            pairs.append_pair("subs_per_page", "30");
        }

        let resp = match self.client.get(url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                debug!("SubDL returned status {}", r.status());
                return Vec::new();
            }
            Err(err) => {
                debug!(
                    "SubDL lookup failed: {}",
                    redact_secret(&err.to_string(), &secrets)
                );
                return Vec::new();
            }
        };

        let json: Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let Some(items) = json.get("subtitles").and_then(Value::as_array) else {
            return Vec::new();
        };

        let dl_base_trimmed = dl_base.trim_end_matches('/');
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let Some(raw_url) = item.get("url").and_then(Value::as_str).map(str::trim) else {
                continue;
            };
            let full_url = if raw_url.starts_with("http://") || raw_url.starts_with("https://") {
                raw_url.to_owned()
            } else if raw_url.starts_with('/') {
                format!("{dl_base_trimmed}{raw_url}")
            } else {
                format!("{dl_base_trimmed}/{raw_url}")
            };
            if !is_allowed_addon_stream_url(&full_url) {
                continue;
            }

            let lang_raw = item
                .get("language")
                .or_else(|| item.get("lang"))
                .and_then(Value::as_str)
                .unwrap_or("und");
            let lang = normalize_subtitle_lang_code(lang_raw);
            let release_name = item
                .get("release_name")
                .or_else(|| item.get("name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("SubDL Release")
                .to_owned();
            let title = item
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&release_name)
                .to_owned();
            let hi = item.get("hi").and_then(Value::as_bool).unwrap_or(false);

            let cand_fp = VideoReleaseFingerprint::parse(&release_name);
            let (tier, score) = SubtitleMatchRanker::score_candidate(
                video_fp,
                &cand_fp,
                false,
                &lang,
                &query.preferred_langs,
                None,
                hi,
            );

            out.push(RankedSubtitleCandidate {
                provider: "SubDL".to_owned(),
                lang,
                title,
                release_name,
                match_tier: tier,
                score,
                download: ProviderDownloadSpec::DirectUrl(full_url),
            });
        }
        out
    }

    async fn download_and_cache(
        &self,
        candidate: RankedSubtitleCandidate,
        endpoints: &NativeSubtitleEndpoints,
        query: &NativeSubtitleQuery,
    ) -> Option<FetchedNativeSubtitle> {
        let raw_bytes = match &candidate.download {
            ProviderDownloadSpec::DirectUrl(url) => self.fetch_bytes_bounded(url).await?,
            ProviderDownloadSpec::OpenSubtitlesFileId { file_id, api_key } => {
                let secrets = [api_key.as_str()];
                let dl_endpoint = format!(
                    "{}/download",
                    endpoints.opensubtitles_api_base.trim_end_matches('/')
                );
                let resp = match self
                    .client
                    .post(&dl_endpoint)
                    .header("Api-Key", api_key)
                    .header("Accept", "application/json")
                    .json(&serde_json::json!({ "file_id": file_id }))
                    .send()
                    .await
                {
                    Ok(r) if r.status().is_success() => r,
                    Ok(r) => {
                        debug!("OpenSubtitles download endpoint returned {}", r.status());
                        return None;
                    }
                    Err(err) => {
                        debug!(
                            "OpenSubtitles download link error: {}",
                            redact_secret(&err.to_string(), &secrets)
                        );
                        return None;
                    }
                };
                let json: Value = resp.json().await.ok()?;
                let link = json.get("link").and_then(Value::as_str)?.trim();
                if !is_allowed_addon_stream_url(link) {
                    return None;
                }
                self.fetch_bytes_bounded(link).await?
            }
        };

        let (utf8_text, detected_ext) = match decode_subtitle_payload(
            &raw_bytes,
            query.season,
            query.episode,
        ) {
            Ok(decoded) => decoded,
            Err(err) => {
                warn!("Failed to decode subtitle payload from {}: {err}", candidate.provider);
                return None;
            }
        };

        let local_path = match write_cached_subtitle(&self.cache_dir, &utf8_text, detected_ext) {
            Ok(path) => path,
            Err(err) => {
                warn!("Failed to cache decoded subtitle: {err}");
                return None;
            }
        };

        Some(FetchedNativeSubtitle {
            provider: candidate.provider,
            lang: candidate.lang,
            title: candidate.title,
            release_name: candidate.release_name,
            match_tier: candidate.match_tier,
            score: candidate.score,
            local_path,
            lookup_elapsed_ms: 0,
        })
    }

    async fn fetch_bytes_bounded(&self, url: &str) -> Option<Vec<u8>> {
        if !is_allowed_addon_stream_url(url) {
            return None;
        }
        let resp = self.client.get(url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        if let Some(len) = resp.content_length() {
            if len as usize > MAX_SUBTITLE_ARCHIVE_BYTES {
                return None;
            }
        }
        let bytes = resp.bytes().await.ok()?;
        if bytes.is_empty() || bytes.len() > MAX_SUBTITLE_ARCHIVE_BYTES {
            return None;
        }
        Some(bytes.to_vec())
    }
}

/// Decompresses `.gz` or `.zip` payloads (or reads raw subtitle bytes) and normalizes encoding to UTF-8.
pub fn decode_subtitle_payload(
    raw: &[u8],
    season: Option<u32>,
    episode: Option<u32>,
) -> Result<(String, &'static str), String> {
    if raw.is_empty() {
        return Err("Empty subtitle payload".to_owned());
    }
    if raw.len() > MAX_SUBTITLE_ARCHIVE_BYTES {
        return Err(format!(
            "Subtitle payload exceeds {MAX_SUBTITLE_ARCHIVE_BYTES} byte limit"
        ));
    }

    let (plain_bytes, hint_ext): (Vec<u8>, Option<&'static str>) = if raw.starts_with(&[0x1f, 0x8b]) {
        (decompress_gzip_bounded(raw)?, None)
    } else if raw.starts_with(&[0x50, 0x4b, 0x03, 0x04]) {
        let (entry_bytes, ext) = extract_best_subtitle_from_zip(raw, season, episode)?;
        (entry_bytes, Some(ext))
    } else {
        (raw.to_vec(), None)
    };

    let utf8 = normalize_bytes_to_utf8(&plain_bytes)?;
    let ext = hint_ext.unwrap_or_else(|| detect_subtitle_extension(&utf8));
    Ok((utf8, ext))
}

fn decompress_gzip_bounded(raw: &[u8]) -> Result<Vec<u8>, String> {
    let decoder = GzDecoder::new(raw);
    let mut out = Vec::new();
    decoder
        .take(MAX_UNCOMPRESSED_SUBTITLE_BYTES as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|err| format!("Failed to decompress gzip subtitle: {err}"))?;
    if out.len() > MAX_UNCOMPRESSED_SUBTITLE_BYTES {
        return Err(format!(
            "Decompressed gzip subtitle exceeds {MAX_UNCOMPRESSED_SUBTITLE_BYTES} byte limit"
        ));
    }
    Ok(out)
}

fn read_u16_le(buf: &[u8], offset: usize) -> Option<u16> {
    let slice = buf.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(buf: &[u8], offset: usize) -> Option<u32> {
    let slice = buf.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn is_safe_zip_entry_name(name: &str) -> bool {
    if name.is_empty()
        || name.contains('\0')
        || name.starts_with('/')
        || name.starts_with('\\')
        || name.as_bytes().get(1) == Some(&b':')
    {
        return false;
    }
    !name.split(['/', '\\']).any(|seg| seg == "..")
}

fn subtitle_ext_for_filename(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".ass") {
        Some("ass")
    } else if lower.ends_with(".ssa") {
        Some("ssa")
    } else if lower.ends_with(".srt") {
        Some("srt")
    } else if lower.ends_with(".vtt") {
        Some("vtt")
    } else {
        None
    }
}

fn score_zip_entry(
    name: &str,
    ext: &str,
    season: Option<u32>,
    episode: Option<u32>,
) -> i32 {
    let mut score: i32 = match ext {
        "ass" | "ssa" => 300,
        "srt" => 250,
        "vtt" => 150,
        _ => 0,
    };

    let lower = name.to_ascii_lowercase();
    if lower.contains("forced") {
        score -= 100;
    }
    if lower.contains("sdh") || lower.contains("hearing") {
        score -= 20;
    }

    if let (Some(target_s), Some(target_e)) = (season, episode) {
        if let Some(caps) = RE_EPISODE_TAG.captures(&lower) {
            let parsed = if let (Some(s), Some(e)) = (caps.get(1), caps.get(2)) {
                s.as_str()
                    .parse::<u32>()
                    .ok()
                    .zip(e.as_str().parse::<u32>().ok())
            } else if let (Some(s), Some(e)) = (caps.get(3), caps.get(4)) {
                s.as_str()
                    .parse::<u32>()
                    .ok()
                    .zip(e.as_str().parse::<u32>().ok())
            } else {
                None
            };
            if let Some((s, e)) = parsed {
                if s == target_s && e == target_e {
                    score += 5_000;
                } else {
                    score -= 5_000;
                }
            }
        } else {
            let ep_token_a = format!("e{target_e:02}");
            let ep_token_b = format!("ep{target_e:02}");
            let ep_token_c = format!("episode {target_e}");
            if lower.contains(&ep_token_a)
                || lower.contains(&ep_token_b)
                || lower.contains(&ep_token_c)
            {
                score += 2_000;
            }
        }
    }

    score
}

/// Bounds-checked pure-Rust ZIP extractor supporting Store (0) and Deflate (8).
pub fn extract_best_subtitle_from_zip(
    archive: &[u8],
    season: Option<u32>,
    episode: Option<u32>,
) -> Result<(Vec<u8>, &'static str), String> {
    if archive.len() > MAX_SUBTITLE_ARCHIVE_BYTES {
        return Err(format!(
            "ZIP archive exceeds {MAX_SUBTITLE_ARCHIVE_BYTES} byte limit"
        ));
    }
    if archive.len() < 22 {
        return Err("Truncated ZIP archive".to_owned());
    }

    // Locate End of Central Directory (0x06054b50)
    let search_start = archive.len().saturating_sub(65_535 + 22);
    let mut eocd_pos = None;
    for pos in (search_start..=archive.len() - 22).rev() {
        if archive[pos..pos + 4] == [0x50, 0x4b, 0x05, 0x06] {
            let comment_len = read_u16_le(archive, pos + 20).unwrap_or(0) as usize;
            if pos + 22 + comment_len == archive.len() {
                eocd_pos = Some(pos);
                break;
            }
        }
    }

    let eocd = eocd_pos.ok_or_else(|| "Missing ZIP End of Central Directory".to_owned())?;
    let total_entries = read_u16_le(archive, eocd + 10).unwrap_or(0) as usize;
    let cd_size = read_u32_le(archive, eocd + 12).unwrap_or(0) as usize;
    let cd_offset = read_u32_le(archive, eocd + 16).unwrap_or(0) as usize;

    let cd_end = cd_offset
        .checked_add(cd_size)
        .ok_or_else(|| "ZIP central directory overflow".to_owned())?;
    if cd_end > archive.len() {
        return Err("ZIP central directory out of bounds".to_owned());
    }

    #[derive(Clone)]
    struct ZipCandidate {
        ext: &'static str,
        method: u16,
        compressed_size: usize,
        uncompressed_size: usize,
        local_header_offset: usize,
        score: i32,
    }

    let mut best: Option<ZipCandidate> = None;
    let mut cursor = cd_offset;
    for _ in 0..total_entries {
        if cursor + 46 > cd_end {
            return Err("Truncated ZIP central directory header".to_owned());
        }
        if archive[cursor..cursor + 4] != [0x50, 0x4b, 0x01, 0x02] {
            return Err("Invalid ZIP central directory signature".to_owned());
        }

        let method = read_u16_le(archive, cursor + 10).unwrap_or(u16::MAX);
        let compressed_size = read_u32_le(archive, cursor + 20).unwrap_or(0) as usize;
        let uncompressed_size = read_u32_le(archive, cursor + 24).unwrap_or(0) as usize;
        let name_len = read_u16_le(archive, cursor + 28).unwrap_or(0) as usize;
        let extra_len = read_u16_le(archive, cursor + 30).unwrap_or(0) as usize;
        let comment_len = read_u16_le(archive, cursor + 32).unwrap_or(0) as usize;
        let local_header_offset = read_u32_le(archive, cursor + 42).unwrap_or(0) as usize;

        let name_start = cursor + 46;
        let name_end = name_start
            .checked_add(name_len)
            .ok_or_else(|| "ZIP entry name overflow".to_owned())?;
        let next_cursor = name_end
            .checked_add(extra_len)
            .and_then(|v| v.checked_add(comment_len))
            .ok_or_else(|| "ZIP entry metadata overflow".to_owned())?;
        if next_cursor > cd_end {
            return Err("ZIP central directory entry out of bounds".to_owned());
        }

        let name = String::from_utf8_lossy(&archive[name_start..name_end]);
        if !is_safe_zip_entry_name(&name) {
            return Err(format!("ZIP entry rejected due to unsafe path: {name}"));
        }

        cursor = next_cursor;

        if name.ends_with('/') || name.starts_with("__MACOSX/") || name.contains("/._") {
            continue;
        }
        let Some(ext) = subtitle_ext_for_filename(&name) else {
            continue;
        };
        if uncompressed_size > MAX_UNCOMPRESSED_SUBTITLE_BYTES {
            return Err(format!(
                "ZIP entry '{name}' exceeds {MAX_UNCOMPRESSED_SUBTITLE_BYTES} byte limit"
            ));
        }
        if !matches!(method, 0 | 8) {
            continue;
        }

        let score = score_zip_entry(&name, ext, season, episode);
        let candidate = ZipCandidate {
            ext,
            method,
            compressed_size,
            uncompressed_size,
            local_header_offset,
            score,
        };
        if best.as_ref().is_none_or(|b| candidate.score > b.score) {
            best = Some(candidate);
        }
    }

    let chosen = best.ok_or_else(|| "No supported subtitle file found in ZIP archive".to_owned())?;

    let lfh = chosen.local_header_offset;
    if lfh + 30 > archive.len() || archive[lfh..lfh + 4] != [0x50, 0x4b, 0x03, 0x04] {
        return Err("Invalid ZIP local file header".to_owned());
    }
    let lfh_name_len = read_u16_le(archive, lfh + 26).unwrap_or(0) as usize;
    let lfh_extra_len = read_u16_le(archive, lfh + 28).unwrap_or(0) as usize;
    let data_start = lfh
        .checked_add(30)
        .and_then(|v| v.checked_add(lfh_name_len))
        .and_then(|v| v.checked_add(lfh_extra_len))
        .ok_or_else(|| "ZIP data offset overflow".to_owned())?;
    let data_end = data_start
        .checked_add(chosen.compressed_size)
        .ok_or_else(|| "ZIP compressed data overflow".to_owned())?;
    if data_end > archive.len() {
        return Err("ZIP compressed data out of bounds".to_owned());
    }

    let compressed = &archive[data_start..data_end];
    let decompressed = match chosen.method {
        0 => {
            if compressed.len() > MAX_UNCOMPRESSED_SUBTITLE_BYTES {
                return Err("ZIP stored entry exceeds uncompressed byte limit".to_owned());
            }
            compressed.to_vec()
        }
        8 => {
            let mut out = Vec::with_capacity(chosen.uncompressed_size.min(65_536));
            DeflateDecoder::new(compressed)
                .take(MAX_UNCOMPRESSED_SUBTITLE_BYTES as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|err| format!("Failed to inflate ZIP subtitle entry: {err}"))?;
            if out.len() > MAX_UNCOMPRESSED_SUBTITLE_BYTES {
                return Err(format!(
                    "Inflated ZIP entry exceeds {MAX_UNCOMPRESSED_SUBTITLE_BYTES} byte limit"
                ));
            }
            out
        }
        other => return Err(format!("Unsupported ZIP compression method: {other}")),
    };

    Ok((decompressed, chosen.ext))
}

/// Normalizes UTF-8 (with/without BOM), UTF-16LE/BE, or legacy 8-bit encodings (e.g. CP1252) to UTF-8.
pub fn normalize_bytes_to_utf8(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > MAX_UNCOMPRESSED_SUBTITLE_BYTES {
        return Err(format!(
            "Subtitle text exceeds {MAX_UNCOMPRESSED_SUBTITLE_BYTES} byte limit"
        ));
    }

    let text = if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        if let Ok(s) = std::str::from_utf8(rest) {
            s.to_owned()
        } else {
            decode_with_chardetng(rest)
        }
    } else if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let (cow, _) = encoding_rs::UTF_16LE.decode_without_bom_handling(rest);
        cow.into_owned()
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let (cow, _) = encoding_rs::UTF_16BE.decode_without_bom_handling(rest);
        cow.into_owned()
    } else if let Ok(s) = std::str::from_utf8(bytes) {
        s.to_owned()
    } else {
        decode_with_chardetng(bytes)
    };

    Ok(text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned())
}

fn decode_with_chardetng(bytes: &[u8]) -> String {
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    let encoding = detector.guess(None, Utf8Detection::Allow);
    let (cow, _, _) = encoding.decode(bytes);
    cow.into_owned()
}

fn detect_subtitle_extension(content: &str) -> &'static str {
    let head: String = content.chars().take(1024).collect();
    let lower = head.to_ascii_lowercase();
    if lower.contains("[v4+ styles]") || (lower.contains("[script info]") && lower.contains("dialogue:"))
    {
        "ass"
    } else if lower.contains("[v4 styles]") {
        "ssa"
    } else if lower.trim_start().starts_with("webvtt") {
        "vtt"
    } else {
        "srt"
    }
}

fn ensure_private_subtitle_dir(dir: &Path) -> Result<(), String> {
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        builder.mode(0o700);
        builder
            .create(dir)
            .map_err(|err| format!("Failed to create subtitle cache dir: {err}"))?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|err| format!("Failed to secure subtitle cache dir: {err}"))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("Failed to create subtitle cache dir: {err}"))
    }
}

fn write_private_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        ensure_private_subtitle_dir(parent)?;
    }

    let seq = TEMP_SUB_SEQ.fetch_add(1, AtomicOrdering::Relaxed);
    let fname = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("subtitle.srt");
    let tmp_path = path.with_file_name(format!(".{fname}.{}.{}.tmp", std::process::id(), seq));

    let res: std::io::Result<()> = (|| {
        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp_path)?;
            std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600))?;
            f
        };
        #[cfg(not(unix))]
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;

        file.write_all(content.as_bytes())?;
        file.flush()?;
        drop(file);
        std::fs::rename(&tmp_path, path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    })();

    if let Err(err) = res {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("Failed to write subtitle file: {err}"));
    }

    Ok(())
}

fn write_cached_subtitle(cache_dir: &Path, utf8_content: &str, ext: &str) -> Result<PathBuf, String> {
    ensure_private_subtitle_dir(cache_dir)?;
    let mut hasher = Sha256::new();
    hasher.update(utf8_content.as_bytes());
    let digest_hex = hex::encode(hasher.finalize());
    let prefix = &digest_hex[..24];
    let out_path = cache_dir.join(format!("{prefix}.{ext}"));
    write_private_file(&out_path, utf8_content)?;
    Ok(out_path)
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubtitleAlignmentResult {
    pub offset_ms: i64,
    pub drift_ratio: f64,
    pub matched_onsets: usize,
    pub confidence: f64,
}

pub struct SubtitleSyncAligner;

impl SubtitleSyncAligner {
    /// Parses `(start_ms, end_ms)` cue intervals from `.srt`, `.vtt`, `.ass`, or `.ssa` text,
    /// stripping formatting tags and ignoring common uploader watermark cues.
    pub fn parse_subtitle_cues(content: &str) -> Vec<(i64, i64)> {
        let trimmed = content.strip_prefix('\u{feff}').unwrap_or(content);
        let lower_head: String = trimmed.chars().take(1024).collect::<String>().to_ascii_lowercase();
        if lower_head.contains("[events]") || lower_head.contains("dialogue:") {
            parse_ass_cues(trimmed)
        } else {
            parse_srt_or_vtt_cues(trimmed)
        }
    }

    /// Reads a subtitle file from disk (supporting UTF-8/UTF-16/CP1252) and parses its cues.
    pub fn parse_subtitle_file(path: &Path) -> Result<Vec<(i64, i64)>, String> {
        let bytes = std::fs::read(path).map_err(|err| match err.kind() {
            ErrorKind::NotFound => format!("Subtitle file not found: {}", path.display()),
            _ => format!("Failed to read subtitle file {}: {err}", path.display()),
        })?;
        let text = normalize_bytes_to_utf8(&bytes)?;
        Ok(Self::parse_subtitle_cues(&text))
    }

    /// Solves optimal constant offset and PAL/NTSC drift ratio aligning `target_cues` to `reference_cues`.
    pub fn solve_subtitle_alignment(
        target_cues: &[(i64, i64)],
        reference_cues: &[(i64, i64)],
    ) -> SubtitleAlignmentResult {
        if target_cues.is_empty() || reference_cues.is_empty() {
            return SubtitleAlignmentResult {
                offset_ms: 0,
                drift_ratio: 1.0,
                matched_onsets: 0,
                confidence: 0.0,
            };
        }

        let mut target_starts: Vec<i64> = target_cues.iter().map(|(s, _)| *s).collect();
        let mut ref_starts: Vec<i64> = reference_cues.iter().map(|(s, _)| *s).collect();
        target_starts.sort_unstable();
        ref_starts.sort_unstable();

        let mut best_drift = 1.0_f64;
        let mut best_bin_idx = 0usize;
        let mut best_window_score: f64 = -1.0;

        let mut hist = vec![0u32; ALIGN_BIN_COUNT];

        for &drift in &ALIGN_DRIFT_CANDIDATES {
            hist.fill(0);
            let mut j_lo = 0usize;

            for &r in &ref_starts {
                while j_lo < target_starts.len() {
                    let t_scaled = (target_starts[j_lo] as f64 * drift).round() as i64;
                    if r - t_scaled > ALIGN_MAX_OFFSET_MS {
                        j_lo += 1;
                    } else {
                        break;
                    }
                }
                let mut j = j_lo;
                while j < target_starts.len() {
                    let t_scaled = (target_starts[j] as f64 * drift).round() as i64;
                    let delta = r - t_scaled;
                    if delta < -ALIGN_MAX_OFFSET_MS {
                        break;
                    }
                    let bin = ((delta + ALIGN_MAX_OFFSET_MS + (ALIGN_BIN_MS / 2)) / ALIGN_BIN_MS)
                        as usize;
                    if bin < ALIGN_BIN_COUNT {
                        hist[bin] += 1;
                    }
                    j += 1;
                }
            }

            let mut local_best_bin = 0usize;
            let mut local_best_count = 0u32;
            for b in 0..ALIGN_BIN_COUNT {
                let prev = if b > 0 { hist[b - 1] } else { 0 };
                let next = if b + 1 < ALIGN_BIN_COUNT { hist[b + 1] } else { 0 };
                let sum = prev + hist[b] + next;
                if sum > local_best_count {
                    local_best_count = sum;
                    local_best_bin = b;
                }
            }

            // Slight preference for 1.0x when counts are tied within 1 cue
            let adjusted_score = if (drift - 1.0).abs() < 1e-6 {
                local_best_count as f64 + 1.5
            } else {
                local_best_count as f64
            };

            if adjusted_score > best_window_score {
                best_window_score = adjusted_score;
                best_drift = drift;
                best_bin_idx = local_best_bin;
            }
        }

        let coarse_offset_ms = (best_bin_idx as i64 * ALIGN_BIN_MS) - ALIGN_MAX_OFFSET_MS;

        // Fine refinement: collect exact deltas within ±ALIGN_FINE_WINDOW_MS of coarse peak and take median
        let mut fine_deltas: Vec<i64> = Vec::new();
        let mut j_lo = 0usize;
        for &r in &ref_starts {
            while j_lo < target_starts.len() {
                let t_scaled = (target_starts[j_lo] as f64 * best_drift).round() as i64;
                if r - t_scaled > coarse_offset_ms + ALIGN_FINE_WINDOW_MS {
                    j_lo += 1;
                } else {
                    break;
                }
            }
            let mut j = j_lo;
            while j < target_starts.len() {
                let t_scaled = (target_starts[j] as f64 * best_drift).round() as i64;
                let delta = r - t_scaled;
                if delta < coarse_offset_ms - ALIGN_FINE_WINDOW_MS {
                    break;
                }
                if (delta - coarse_offset_ms).abs() <= ALIGN_FINE_WINDOW_MS {
                    fine_deltas.push(delta);
                }
                j += 1;
            }
        }

        let offset_ms = if fine_deltas.is_empty() {
            coarse_offset_ms
        } else {
            fine_deltas.sort_unstable();
            fine_deltas[fine_deltas.len() / 2]
        };

        // Count matched onsets via two-pointer scan within ±ALIGN_ONSET_TOLERANCE_MS
        let mut matched_onsets = 0usize;
        let mut ti = 0usize;
        for &r in &ref_starts {
            while ti < target_starts.len() {
                let aligned = (target_starts[ti] as f64 * best_drift).round() as i64 + offset_ms;
                if aligned < r - ALIGN_ONSET_TOLERANCE_MS {
                    ti += 1;
                } else {
                    break;
                }
            }
            if ti < target_starts.len() {
                let aligned = (target_starts[ti] as f64 * best_drift).round() as i64 + offset_ms;
                if (aligned - r).abs() <= ALIGN_ONSET_TOLERANCE_MS {
                    matched_onsets += 1;
                    ti += 1;
                }
            }
        }

        let denom = ref_starts.len().min(target_starts.len()).max(1) as f64;
        let confidence = (matched_onsets as f64 / denom).clamp(0.0, 1.0);

        SubtitleAlignmentResult {
            offset_ms,
            drift_ratio: best_drift,
            matched_onsets,
            confidence,
        }
    }

    /// Rewrites `source_path` with `(offset_ms, drift_ratio)` applied to every cue timestamp,
    /// preserving original styles/formatting and enforcing `0600` permissions on Unix.
    pub fn write_aligned_subtitle_file(
        source_path: &Path,
        output_path: &Path,
        offset_ms: i64,
        drift_ratio: f64,
    ) -> Result<(), String> {
        let bytes = std::fs::read(source_path)
            .map_err(|err| format!("Failed to read source subtitle {}: {err}", source_path.display()))?;
        let text = normalize_bytes_to_utf8(&bytes)?;
        let ext = detect_subtitle_extension(&text);

        let rewritten = match ext {
            "ass" | "ssa" => rewrite_ass_timestamps(&text, offset_ms, drift_ratio),
            "vtt" => rewrite_srt_or_vtt_timestamps(&text, offset_ms, drift_ratio, true),
            _ => rewrite_srt_or_vtt_timestamps(&text, offset_ms, drift_ratio, false),
        };

        write_private_file(output_path, &rewritten)
    }
}

fn transform_ms(ms: i64, offset_ms: i64, drift_ratio: f64) -> i64 {
    let ratio = if drift_ratio.is_finite() && drift_ratio > 0.0 {
        drift_ratio
    } else {
        1.0
    };
    ((ms as f64 * ratio).round() as i64 + offset_ms).max(0)
}

fn clean_cue_text(raw: &str) -> String {
    let no_ass = RE_ASS_OVERRIDE.replace_all(raw, " ");
    let no_html = RE_HTML_TAG.replace_all(&no_ass, " ");
    no_html
        .replace("\\N", " ")
        .replace("\\n", " ")
        .replace("\\h", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_watermark_cue(cleaned_text: &str) -> bool {
    let lower = cleaned_text.to_ascii_lowercase();
    if lower.is_empty() {
        return true;
    }
    const WATERMARKS: &[&str] = &[
        "opensubtitles",
        "subscene",
        "addic7ed",
        "podnapisi",
        "subdl",
        "yify",
        "yts.mx",
        "synced and corrected by",
        "subtitles by",
        "captions by",
        "encoded by",
        "ripped by",
        "downloaded from",
        "advertise your product",
        "become vip",
        "www.",
        "http://",
        "https://",
    ];
    WATERMARKS.iter().any(|w| lower.contains(w))
}

fn parse_srt_vtt_timestamp(raw: &str) -> Option<i64> {
    let token = raw.split_whitespace().next()?;
    let normalized = token.replace(',', ".");
    let (hms, frac) = normalized.split_once('.')?;
    let ms_digits: String = frac.chars().take(3).collect();
    if ms_digits.is_empty() || !ms_digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut ms = ms_digits.parse::<i64>().ok()?;
    match ms_digits.len() {
        1 => ms *= 100,
        2 => ms *= 10,
        _ => {}
    }

    let parts: Vec<&str> = hms.split(':').collect();
    let (hours, mins, secs) = match parts.as_slice() {
        [h, m, s] => (
            h.parse::<i64>().ok()?,
            m.parse::<i64>().ok()?,
            s.parse::<i64>().ok()?,
        ),
        [m, s] => (0, m.parse::<i64>().ok()?, s.parse::<i64>().ok()?),
        _ => return None,
    };
    Some((((hours * 60 + mins) * 60) + secs) * 1000 + ms)
}

fn parse_ass_timestamp(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    let (hms, cs_str) = trimmed.split_once('.')?;
    let cs_digits: String = cs_str.chars().take(2).collect();
    let mut cs = cs_digits.parse::<i64>().ok()?;
    if cs_digits.len() == 1 {
        cs *= 10;
    }
    let parts: Vec<&str> = hms.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let hours = parts[0].parse::<i64>().ok()?;
    let mins = parts[1].parse::<i64>().ok()?;
    let secs = parts[2].parse::<i64>().ok()?;
    Some((((hours * 60 + mins) * 60) + secs) * 1000 + cs * 10)
}

fn format_srt_vtt_timestamp(ms: i64, use_dot: bool) -> String {
    let clamped = ms.max(0);
    let total_secs = clamped / 1000;
    let millis = clamped % 1000;
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    let sep = if use_dot { '.' } else { ',' };
    format!("{hours:02}:{mins:02}:{secs:02}{sep}{millis:03}")
}

fn format_ass_timestamp(ms: i64) -> String {
    let clamped = ms.max(0);
    let total_cs = (clamped + 5) / 10;
    let cs = total_cs % 100;
    let total_secs = total_cs / 100;
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hours = total_secs / 3600;
    format!("{hours}:{mins:02}:{secs:02}.{cs:02}")
}

fn parse_srt_or_vtt_cues(content: &str) -> Vec<(i64, i64)> {
    let mut cues = Vec::new();
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    for block in normalized.split("\n\n") {
        let lines: Vec<&str> = block.lines().collect();
        let Some(ts_idx) = lines.iter().position(|line| line.contains("-->")) else {
            continue;
        };
        let ts_line = lines[ts_idx];
        let Some((left, right)) = ts_line.split_once("-->") else {
            continue;
        };
        let Some(start_ms) = parse_srt_vtt_timestamp(left) else {
            continue;
        };
        let Some(end_ms) = parse_srt_vtt_timestamp(right) else {
            continue;
        };
        if end_ms <= start_ms {
            continue;
        }
        let body = lines[ts_idx + 1..].join(" ");
        let cleaned = clean_cue_text(&body);
        if is_watermark_cue(&cleaned) {
            continue;
        }
        cues.push((start_ms, end_ms));
    }
    cues
}

fn parse_ass_cues(content: &str) -> Vec<(i64, i64)> {
    let mut cues = Vec::new();
    let mut start_col = 1usize;
    let mut end_col = 2usize;
    let mut text_col = 9usize;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("Format:") {
            let cols: Vec<&str> = rest.split(',').map(str::trim).collect();
            if let Some(idx) = cols.iter().position(|c| c.eq_ignore_ascii_case("Start")) {
                start_col = idx;
            }
            if let Some(idx) = cols.iter().position(|c| c.eq_ignore_ascii_case("End")) {
                end_col = idx;
            }
            if let Some(idx) = cols.iter().position(|c| c.eq_ignore_ascii_case("Text")) {
                text_col = idx;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("Dialogue:") {
            let fields: Vec<&str> = rest.splitn(text_col + 1, ',').collect();
            if fields.len() <= start_col.max(end_col).max(text_col) {
                continue;
            }
            let Some(start_ms) = parse_ass_timestamp(fields[start_col]) else {
                continue;
            };
            let Some(end_ms) = parse_ass_timestamp(fields[end_col]) else {
                continue;
            };
            if end_ms <= start_ms {
                continue;
            }
            let cleaned = clean_cue_text(fields[text_col]);
            if is_watermark_cue(&cleaned) {
                continue;
            }
            cues.push((start_ms, end_ms));
        }
    }
    cues
}

fn rewrite_srt_or_vtt_timestamps(
    content: &str,
    offset_ms: i64,
    drift_ratio: f64,
    is_vtt: bool,
) -> String {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let mut out_lines = Vec::new();
    for line in normalized.split('\n') {
        if let Some((left, right)) = line.split_once("-->") {
            let right_trimmed = right.trim_start();
            let (end_token, trailing) = match right_trimmed.find(char::is_whitespace) {
                Some(idx) => (&right_trimmed[..idx], &right_trimmed[idx..]),
                None => (right_trimmed, ""),
            };
            if let (Some(start_ms), Some(end_ms)) = (
                parse_srt_vtt_timestamp(left),
                parse_srt_vtt_timestamp(end_token),
            ) {
                let new_start = transform_ms(start_ms, offset_ms, drift_ratio);
                let new_end = transform_ms(end_ms, offset_ms, drift_ratio).max(new_start);
                out_lines.push(format!(
                    "{} --> {}{}",
                    format_srt_vtt_timestamp(new_start, is_vtt),
                    format_srt_vtt_timestamp(new_end, is_vtt),
                    trailing
                ));
                continue;
            }
        }
        out_lines.push(line.to_owned());
    }
    out_lines.join("\n")
}

fn rewrite_ass_timestamps(content: &str, offset_ms: i64, drift_ratio: f64) -> String {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let mut start_col = 1usize;
    let mut end_col = 2usize;
    let mut in_events = false;
    let mut out_lines = Vec::new();

    for line in normalized.split('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_events = trimmed.eq_ignore_ascii_case("[Events]");
            out_lines.push(line.to_owned());
            continue;
        }
        if in_events {
            if let Some(rest) = trimmed.strip_prefix("Format:") {
                let cols: Vec<&str> = rest.split(',').map(str::trim).collect();
                if let Some(idx) = cols.iter().position(|c| c.eq_ignore_ascii_case("Start")) {
                    start_col = idx;
                }
                if let Some(idx) = cols.iter().position(|c| c.eq_ignore_ascii_case("End")) {
                    end_col = idx;
                }
                out_lines.push(line.to_owned());
                continue;
            }
            if let Some(rest) = line.strip_prefix("Dialogue:") {
                let split_count = start_col.max(end_col) + 2;
                let mut fields: Vec<String> = rest.splitn(split_count, ',').map(str::to_owned).collect();
                if fields.len() > start_col.max(end_col) {
                    if let (Some(start_ms), Some(end_ms)) = (
                        parse_ass_timestamp(&fields[start_col]),
                        parse_ass_timestamp(&fields[end_col]),
                    ) {
                        let new_start = transform_ms(start_ms, offset_ms, drift_ratio);
                        let new_end = transform_ms(end_ms, offset_ms, drift_ratio).max(new_start);
                        fields[start_col] = format_ass_timestamp(new_start);
                        fields[end_col] = format_ass_timestamp(new_end);
                        out_lines.push(format!("Dialogue:{}", fields.join(",")));
                        continue;
                    }
                }
            }
        }
        out_lines.push(line.to_owned());
    }
    out_lines.join("\n")
}

#[allow(dead_code)]
fn _compare_f64(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::DeflateEncoder, write::GzEncoder, Compression};
    use tempfile::tempdir;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn build_test_zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut archive = Vec::new();
        struct CdMeta {
            name: Vec<u8>,
            method: u16,
            comp_size: u32,
            uncomp_size: u32,
            local_offset: u32,
        }
        let mut cd_entries = Vec::new();

        for &(name, data, deflate) in entries {
            let local_offset = archive.len() as u32;
            let (method, payload) = if deflate {
                let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
                enc.write_all(data).unwrap();
                (8u16, enc.finish().unwrap())
            } else {
                (0u16, data.to_vec())
            };
            let name_bytes = name.as_bytes();
            // Local file header
            archive.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
            archive.extend_from_slice(&20u16.to_le_bytes()); // version needed
            archive.extend_from_slice(&0u16.to_le_bytes()); // flags
            archive.extend_from_slice(&method.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes()); // mod time
            archive.extend_from_slice(&0u16.to_le_bytes()); // mod date
            archive.extend_from_slice(&0u32.to_le_bytes()); // crc32
            archive.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            archive.extend_from_slice(&(data.len() as u32).to_le_bytes());
            archive.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes()); // extra len
            archive.extend_from_slice(name_bytes);
            archive.extend_from_slice(&payload);

            cd_entries.push(CdMeta {
                name: name_bytes.to_vec(),
                method,
                comp_size: payload.len() as u32,
                uncomp_size: data.len() as u32,
                local_offset,
            });
        }

        let cd_offset = archive.len() as u32;
        for entry in &cd_entries {
            archive.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
            archive.extend_from_slice(&20u16.to_le_bytes()); // version made by
            archive.extend_from_slice(&20u16.to_le_bytes()); // version needed
            archive.extend_from_slice(&0u16.to_le_bytes()); // flags
            archive.extend_from_slice(&entry.method.to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes()); // time
            archive.extend_from_slice(&0u16.to_le_bytes()); // date
            archive.extend_from_slice(&0u32.to_le_bytes()); // crc32
            archive.extend_from_slice(&entry.comp_size.to_le_bytes());
            archive.extend_from_slice(&entry.uncomp_size.to_le_bytes());
            archive.extend_from_slice(&(entry.name.len() as u16).to_le_bytes());
            archive.extend_from_slice(&0u16.to_le_bytes()); // extra len
            archive.extend_from_slice(&0u16.to_le_bytes()); // comment len
            archive.extend_from_slice(&0u16.to_le_bytes()); // disk start
            archive.extend_from_slice(&0u16.to_le_bytes()); // int attr
            archive.extend_from_slice(&0u32.to_le_bytes()); // ext attr
            archive.extend_from_slice(&entry.local_offset.to_le_bytes());
            archive.extend_from_slice(&entry.name);
        }
        let cd_size = (archive.len() as u32) - cd_offset;

        // EOCD
        archive.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        archive.extend_from_slice(&0u16.to_le_bytes()); // disk
        archive.extend_from_slice(&0u16.to_le_bytes()); // cd disk
        archive.extend_from_slice(&(cd_entries.len() as u16).to_le_bytes());
        archive.extend_from_slice(&(cd_entries.len() as u16).to_le_bytes());
        archive.extend_from_slice(&cd_size.to_le_bytes());
        archive.extend_from_slice(&cd_offset.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes()); // comment len
        archive
    }

    #[test]
    fn maps_iso_language_codes_cleanly() {
        assert_eq!(normalize_subtitle_lang_iso639_1("eng"), Some("en"));
        assert_eq!(normalize_subtitle_lang_iso639_1("slv"), Some("sl"));
        assert_eq!(normalize_subtitle_lang_iso639_1("fre"), Some("fr"));
        assert_eq!(normalize_subtitle_lang_iso639_1("ger"), Some("de"));
        assert_eq!(normalize_subtitle_lang_iso639_1("zho"), Some("zh"));
        assert_eq!(normalize_subtitle_lang_iso639_2("en"), Some("eng"));
        assert_eq!(normalize_subtitle_lang_iso639_2("sl"), Some("slv"));
        assert!(subtitle_lang_matches("eng", "en-US"));
        assert!(subtitle_lang_matches("slv", "Slovenian"));
        assert!(!subtitle_lang_matches("eng", "slv"));
    }

    #[test]
    fn anti_desync_ranker_enforces_penalties_and_tier_hierarchy() {
        let video = VideoReleaseFingerprint::parse(
            "Severance.S02E01.1080p.ATVP.WEB-DL.DDP5.1.H.265-NTb.mkv",
        );
        assert_eq!(video.release_group.as_deref(), Some("NTb"));
        assert_eq!(video.streaming_service, Some(StreamingServiceTag::AppleTv));
        assert_eq!(video.source_cut, SubtitleSourceCut::WebDl);

        let hash_fp = VideoReleaseFingerprint::parse("Generic.Sub");
        let (tier_hash, score_hash) = SubtitleMatchRanker::score_candidate(
            Some(&video),
            &hash_fp,
            true,
            "eng",
            &["en".to_owned()],
            Some(10),
            false,
        );
        assert_eq!(tier_hash, SubtitleMatchTier::ExactHash);
        assert_eq!(tier_hash.display_label(), "Hash-Matched · 0ms");

        let exact_rel = VideoReleaseFingerprint::parse(
            "Severance.S02E01.1080p.ATVP.WEB-DL.DDP5.1.H.265-NTb",
        );
        let (tier_rel, score_rel) = SubtitleMatchRanker::score_candidate(
            Some(&video),
            &exact_rel,
            false,
            "eng",
            &["en".to_owned()],
            Some(10),
            false,
        );
        assert_eq!(tier_rel, SubtitleMatchTier::ExactRelease);
        assert_eq!(tier_rel.display_label(), "Release-Matched");
        assert!(score_hash > score_rel);

        let same_svc_cut =
            VideoReleaseFingerprint::parse("Severance.S02E01.720p.ATVP.WEB-DL-FLUX");
        let (tier_svc, score_svc) = SubtitleMatchRanker::score_candidate(
            Some(&video),
            &same_svc_cut,
            false,
            "eng",
            &["en".to_owned()],
            Some(10),
            false,
        );
        assert_eq!(tier_svc, SubtitleMatchTier::SameServiceAndCut);
        assert_eq!(tier_svc.display_label(), "Cut-Matched");
        assert!(score_rel > score_svc);

        // NF vs AMZN cross-service penalty (-900_000)
        let nf_video = VideoReleaseFingerprint::parse("Show.S01E01.1080p.NF.WEB-DL-GRP");
        let amzn_sub = VideoReleaseFingerprint::parse("Show.S01E01.1080p.AMZN.WEB-DL-OTHER");
        let (tier_cross_svc, score_cross_svc) = SubtitleMatchRanker::score_candidate(
            Some(&nf_video),
            &amzn_sub,
            false,
            "en",
            &["en".to_owned()],
            None,
            false,
        );
        assert_eq!(tier_cross_svc, SubtitleMatchTier::Fallback);
        assert!(score_cross_svc < 0);

        // BluRay vs WEB-DL penalty (-1_500_000)
        let bluray_sub =
            VideoReleaseFingerprint::parse("Severance.S02E01.1080p.BluRay.x264-ROVERS");
        let (_, score_bluray) = SubtitleMatchRanker::score_candidate(
            Some(&video),
            &bluray_sub,
            false,
            "en",
            &["en".to_owned()],
            None,
            false,
        );
        assert!(score_bluray < -900_000);

        // Edition mismatch penalty (-2_000_000)
        let dc_video =
            VideoReleaseFingerprint::parse("Movie.2024.Directors.Cut.2160p.BluRay-GROUP");
        let theatrical_sub = VideoReleaseFingerprint::parse("Movie.2024.2160p.BluRay-OTHER");
        let (_, score_ed) = SubtitleMatchRanker::score_candidate(
            Some(&dc_video),
            &theatrical_sub,
            false,
            "en",
            &["en".to_owned()],
            None,
            false,
        );
        assert!(score_ed < -1_400_000);

        // 25fps vs 23.976fps penalty (-2_500_000)
        let ntsc_video =
            VideoReleaseFingerprint::parse("Movie.2024.1080p.BluRay.23.976fps-GRP");
        let pal_sub = VideoReleaseFingerprint::parse("Movie.2024.1080p.BluRay.25fps-PALGRP");
        let (_, score_fps) = SubtitleMatchRanker::score_candidate(
            Some(&ntsc_video),
            &pal_sub,
            false,
            "en",
            &["en".to_owned()],
            None,
            false,
        );
        assert!(score_fps < -1_900_000);
    }

    #[test]
    fn extracts_gz_and_season_pack_zip_and_rejects_traversal_and_converts_cp1252() {
        // 1. Gzip + Windows-1252 bytes -> UTF-8
        let cp1252_srt: Vec<u8> = b"1\r\n00:00:01,000 --> 00:00:03,500\r\nCaf\xe9 \x93Bonjour\x94\r\n"
            .to_vec();
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&cp1252_srt).unwrap();
        let gz_bytes = gz.finish().unwrap();
        let (decoded, ext) = decode_subtitle_payload(&gz_bytes, None, None).unwrap();
        assert_eq!(ext, "srt");
        assert!(decoded.contains("Café"));
        assert!(decoded.contains("“Bonjour”"));

        // 2. ZIP season pack with Store + Deflate picks matching S01E02 .ass
        let ep1_srt = b"1\n00:00:01,000 --> 00:00:02,000\nWrong episode\n";
        let ep2_ass = b"[Script Info]\nTitle: Ep2\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:02.00,0:00:04.50,Default,,0,0,0,,Right episode!\n";
        let zip_bytes = build_test_zip(&[
            ("Show.S01E01.srt", ep1_srt, false),
            ("Subs/Show.S01E02.ass", ep2_ass, true),
        ]);
        let (picked, picked_ext) = decode_subtitle_payload(&zip_bytes, Some(1), Some(2)).unwrap();
        assert_eq!(picked_ext, "ass");
        assert!(picked.contains("Right episode!"));

        // 3. Path traversal rejection
        let evil_zip = build_test_zip(&[("../../etc/evil.srt", ep1_srt, false)]);
        let err = decode_subtitle_payload(&evil_zip, None, None).unwrap_err();
        assert!(err.contains("unsafe path"));
    }

    #[test]
    fn aligns_constant_offset_and_pal_fps_drift_in_under_5ms() {
        // Generate 800 synthetic reference cues
        let mut ref_cues = Vec::with_capacity(800);
        for i in 0..800 {
            let start = 10_000 + (i as i64) * 3_200;
            ref_cues.push((start, start + 1_800));
        }

        // Case A: Constant offset (-3_420ms -> target is +3_420ms later than reference)
        let target_offset_cues: Vec<(i64, i64)> = ref_cues
            .iter()
            .map(|&(s, e)| (s + 3_420, e + 3_420))
            .collect();
        let started = Instant::now();
        let res_const =
            SubtitleSyncAligner::solve_subtitle_alignment(&target_offset_cues, &ref_cues);
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(5),
            "alignment took {:?}, expected <5ms",
            elapsed
        );
        assert!((res_const.offset_ms - (-3_420)).abs() <= 5);
        assert!((res_const.drift_ratio - 1.0).abs() < 1e-4);
        assert!(res_const.confidence > 0.95);

        // Case B: PAL 25fps target vs 23.976fps reference + 1200ms offset
        // ref = target * (25.0 / 23.976) - 1200 => target = (ref + 1200) * (23.976 / 25.0)
        let target_pal_cues: Vec<(i64, i64)> = ref_cues
            .iter()
            .map(|&(s, e)| {
                let ts = ((s + 1_200) as f64 * (23.976 / 25.0)).round() as i64;
                let te = ((e + 1_200) as f64 * (23.976 / 25.0)).round() as i64;
                (ts, te)
            })
            .collect();
        let started_pal = Instant::now();
        let res_pal = SubtitleSyncAligner::solve_subtitle_alignment(&target_pal_cues, &ref_cues);
        let elapsed_pal = started_pal.elapsed();
        assert!(
            elapsed_pal < Duration::from_millis(5),
            "PAL alignment took {:?}, expected <5ms",
            elapsed_pal
        );
        assert!((res_pal.drift_ratio - (25.0 / 23.976)).abs() < 1e-4);
        assert!((res_pal.offset_ms - (-1_200)).abs() <= 5);
        assert!(res_pal.confidence > 0.95);

        // Verify ASS style preservation + 0600 file rewrite
        let dir = tempdir().unwrap();
        let src_ass = dir.path().join("src.ass");
        let out_ass = dir.path().join("aligned.ass");
        std::fs::write(
            &src_ass,
            "[Script Info]\nScriptType: v4.00+\n[V4+ Styles]\nStyle: TopTitle,Arial,24,&H00FFFFFF\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:10.00,0:00:12.50,TopTitle,,0,0,0,,{\\an8}Hello, world!\n",
        )
        .unwrap();
        SubtitleSyncAligner::write_aligned_subtitle_file(&src_ass, &out_ass, -2_000, 1.0).unwrap();
        let rewritten = std::fs::read_to_string(&out_ass).unwrap();
        assert!(rewritten.contains("Style: TopTitle,Arial,24,&H00FFFFFF"));
        assert!(rewritten.contains("Dialogue: 0,0:00:08.00,0:00:10.50,TopTitle,,0,0,0,,{\\an8}Hello, world!"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&out_ass).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[tokio::test]
    async fn concurrent_native_subtitle_client_queries_mock_providers_and_caches_0600() {
        let dir = tempdir().unwrap();
        let cache_dir = dir.path().join("native_subtitles");

        let srt_v3 = "1\n00:00:01,000 --> 00:00:03,000\nFrom OSv3\n";
        let srt_os_api = b"1\r\n00:00:01,000 --> 00:00:03,000\r\nFrom OS API Caf\xe9\r\n";
        let zip_subdl = build_test_zip(&[(
            "Severance.S02E01.1080p.ATVP.WEB-DL-NTb.srt",
            b"1\n00:00:01,000 --> 00:00:03,000\nFrom SubDL NTb\n",
            true,
        )]);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");

        let server_task = tokio::spawn(async move {
            for _ in 0..10 {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let zip_clone = zip_subdl.clone();
                let base_clone = base_url.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let first_line = req.lines().next().unwrap_or("");
                    let path = first_line.split_whitespace().nth(1).unwrap_or("/");

                    let (status, content_type, body): (&str, &str, Vec<u8>) =
                        if path.starts_with("/osv3/subtitles/") {
                            let json = serde_json::json!({
                                "subtitles": [{
                                    "id": "osv3-1",
                                    "url": format!("{base_clone}/files/osv3.srt"),
                                    "lang": "eng",
                                    "SubFileName": "Severance.S02E01.1080p.ATVP.WEB-DL-OTHER.srt"
                                }]
                            });
                            ("200 OK", "application/json", json.to_string().into_bytes())
                        } else if path == "/files/osv3.srt" {
                            ("200 OK", "text/plain", srt_v3.as_bytes().to_vec())
                        } else if path.starts_with("/osapi/subtitles") {
                            let json = serde_json::json!({
                                "data": [{
                                    "attributes": {
                                        "language": "en",
                                        "release": "Severance.S02E01.1080p.NF.WEB-DL-GRP",
                                        "moviehash_match": true,
                                        "download_count": 500,
                                        "hearing_impaired": false,
                                        "files": [{"file_id": 4242, "file_name": "os_api.srt"}]
                                    }
                                }]
                            });
                            ("200 OK", "application/json", json.to_string().into_bytes())
                        } else if path == "/osapi/download" {
                            let json = serde_json::json!({
                                "link": format!("{base_clone}/files/osapi.srt")
                            });
                            ("200 OK", "application/json", json.to_string().into_bytes())
                        } else if path == "/files/osapi.srt" {
                            ("200 OK", "text/plain", srt_os_api.to_vec())
                        } else if path.starts_with("/subdl/subtitles") {
                            let json = serde_json::json!({
                                "status": true,
                                "subtitles": [{
                                    "release_name": "Severance.S02E01.1080p.ATVP.WEB-DL-NTb",
                                    "name": "Severance S02E01 NTb",
                                    "language": "EN",
                                    "url": "/subtitle/999.zip",
                                    "hi": false
                                }]
                            });
                            ("200 OK", "application/json", json.to_string().into_bytes())
                        } else if path == "/subdl-dl/subtitle/999.zip" {
                            ("200 OK", "application/zip", zip_clone)
                        } else {
                            ("404 Not Found", "text/plain", b"not found".to_vec())
                        };

                    let header = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });

        let endpoints = NativeSubtitleEndpoints {
            opensubtitles_v3_base: format!("http://{addr}/osv3"),
            opensubtitles_api_base: format!("http://{addr}/osapi"),
            subdl_api_base: format!("http://{addr}/subdl"),
            subdl_dl_base: format!("http://{addr}/subdl-dl"),
        };
        let client = NativeSubtitleClient::with_endpoints(&cache_dir, endpoints);

        let query = NativeSubtitleQuery {
            imdb_id: "tt11280740".to_owned(),
            media_type: "series".to_owned(),
            season: Some(2),
            episode: Some(1),
            opensubhash: Some("8e245d9679d31e12".to_owned()),
            video_byte_size: Some(3_500_000_000),
            video_filename: Some(
                "Severance.S02E01.1080p.ATVP.WEB-DL.DDP5.1.H.265-NTb.mkv".to_owned(),
            ),
            preferred_langs: vec!["eng".to_owned()],
        };

        let fetched = client
            .search_and_fetch(&query, Some("test_os_key"), Some("test_subdl_key"), 5)
            .await;
        server_task.abort();

        assert_eq!(fetched.len(), 3);
        // #1 should be ExactHash from OpenSubtitles API
        assert_eq!(fetched[0].provider, "OpenSubtitles API");
        assert_eq!(fetched[0].match_tier, SubtitleMatchTier::ExactHash);
        let os_api_text = std::fs::read_to_string(&fetched[0].local_path).unwrap();
        assert!(os_api_text.contains("Café"));

        // #2 should be ExactRelease (NTb) from SubDL ZIP
        assert_eq!(fetched[1].provider, "SubDL");
        assert_eq!(fetched[1].match_tier, SubtitleMatchTier::ExactRelease);
        let subdl_text = std::fs::read_to_string(&fetched[1].local_path).unwrap();
        assert!(subdl_text.contains("From SubDL NTb"));

        // #3 should be SameServiceAndCut (ATVP WEB-DL) from OpenSubtitles v3
        assert_eq!(fetched[2].provider, "OpenSubtitles v3");
        assert_eq!(fetched[2].match_tier, SubtitleMatchTier::SameServiceAndCut);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir_mode = std::fs::metadata(&cache_dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(dir_mode, 0o700);
            for item in &fetched {
                let file_mode = std::fs::metadata(&item.local_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(file_mode, 0o600);
            }
        }
    }
}
