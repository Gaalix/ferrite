use std::{
    collections::{HashMap, HashSet},
    fmt,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
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
pub const MAX_SUBTITLE_CACHE_FILES: usize = 48;
pub const MAX_SUBTITLE_CACHE_BYTES: u64 = 6 * 1024 * 1024;

const ALIGN_MAX_OFFSET_MS: i64 = 45_000;
const ALIGN_BIN_MS: i64 = 50;
const ALIGN_BIN_COUNT: usize = ((ALIGN_MAX_OFFSET_MS * 2) / ALIGN_BIN_MS) as usize + 3;
const ALIGN_ONSET_TOLERANCE_MS: i64 = 250;
const ALIGN_FINE_WINDOW_MS: i64 = 150;
const ALIGN_DRIFT_CANDIDATES: [f64; 5] = [
    1.0,
    24.0 / 23.976,
    23.976 / 24.0,
    25.0 / 23.976,
    23.976 / 25.0,
];
pub const AUDIO_SAMPLE_RATE_HZ: usize = 8_000;
pub const AUDIO_ENV_FRAME_MS: i64 = 20;
const AUDIO_ENV_SAMPLES_PER_FRAME: usize = 160; // 20ms at 8 kHz
const AUDIO_ALIGN_MAX_OFFSET_MS: i64 = 35_000;

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

/// Converts a raw ISO-639-1/2/regional subtitle tag (`"eng"`, `"pob"`, `"slv"`, `"pt-BR"`) into a clean human name.
pub fn human_language_name(code: &str) -> String {
    let cleaned = code.trim().to_ascii_lowercase().replace('_', "-");
    if cleaned.is_empty() || cleaned == "und" || cleaned == "unknown" {
        return "Unknown".to_owned();
    }
    match cleaned.as_str() {
        "pob" | "pt-br" | "ptbr" | "pb" => return "Portuguese (BR)".to_owned(),
        "pt-pt" => return "Portuguese (PT)".to_owned(),
        "zh-tw" | "cht" | "zht" => return "Chinese (Trad.)".to_owned(),
        "zh-cn" | "chs" | "zhs" => return "Chinese (Simp.)".to_owned(),
        "es-la" | "es-419" | "spl" => return "Spanish (LatAm)".to_owned(),
        _ => {}
    }
    match normalize_subtitle_lang_iso639_1(&cleaned) {
        Some("en") => "English".to_owned(),
        Some("sl") => "Slovenian".to_owned(),
        Some("es") => "Spanish".to_owned(),
        Some("fr") => "French".to_owned(),
        Some("de") => "German".to_owned(),
        Some("it") => "Italian".to_owned(),
        Some("pt") => "Portuguese".to_owned(),
        Some("ja") => "Japanese".to_owned(),
        Some("ko") => "Korean".to_owned(),
        Some("zh") => "Chinese".to_owned(),
        Some("ru") => "Russian".to_owned(),
        Some("nl") => "Dutch".to_owned(),
        Some("pl") => "Polish".to_owned(),
        Some("tr") => "Turkish".to_owned(),
        Some("ar") => "Arabic".to_owned(),
        Some("he") => "Hebrew".to_owned(),
        Some("hi") => "Hindi".to_owned(),
        Some("sv") => "Swedish".to_owned(),
        Some("no") => "Norwegian".to_owned(),
        Some("da") => "Danish".to_owned(),
        Some("fi") => "Finnish".to_owned(),
        Some("cs") => "Czech".to_owned(),
        Some("hu") => "Hungarian".to_owned(),
        Some("ro") => "Romanian".to_owned(),
        Some("el") => "Greek".to_owned(),
        Some("uk") => "Ukrainian".to_owned(),
        Some("bg") => "Bulgarian".to_owned(),
        Some("hr") => "Croatian".to_owned(),
        Some("sr") => "Serbian".to_owned(),
        Some("th") => "Thai".to_owned(),
        Some("vi") => "Vietnamese".to_owned(),
        Some("id") => "Indonesian".to_owned(),
        Some("ms") => "Malay".to_owned(),
        _ => code.trim().to_ascii_uppercase(),
    }
}

/// Formats a compact badge like `"EN"`, `"PT-BR"`, or `"SL"` for bottom player pills.
pub fn short_language_badge(code: &str) -> String {
    let cleaned = code.trim().to_ascii_lowercase().replace('_', "-");
    if matches!(cleaned.as_str(), "pob" | "pt-br" | "ptbr" | "pb") {
        return "PT-BR".to_owned();
    }
    if let Some(iso1) = normalize_subtitle_lang_iso639_1(&cleaned) {
        return iso1.to_ascii_uppercase();
    }
    let upper = code.trim().to_ascii_uppercase();
    if upper.is_empty() {
        "SUB".to_owned()
    } else {
        upper.chars().take(5).collect()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubtitleSearchMode {
    #[default]
    Full,
    ForcedOnly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForeignTranslationStatus {
    #[default]
    Unknown,
    Translated,
    MissingForeign,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubtitleContentProfile {
    pub cue_count: usize,
    pub span_ms: i64,
    pub foreign_placeholder_count: usize,
    pub sdh_cue_count: usize,
    pub sdh_ratio_pct: u8,
    pub foreign_status: ForeignTranslationStatus,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnalyzedSubtitleContent {
    pub profile: SubtitleContentProfile,
    pub spoken_cues: Vec<(i64, i64)>,
    pub foreign_placeholder_timestamps_ms: Vec<i64>,
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
    #[serde(default)]
    pub search_mode: SubtitleSearchMode,
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
    Conflicting,
}

impl SubtitleMatchTier {
    pub fn display_label(self) -> &'static str {
        match self {
            Self::ExactHash => "Hash-Matched · 0ms",
            Self::ExactRelease => "Release-Matched",
            Self::SameServiceAndCut | Self::CompatibleCut => "Cut-Matched",
            Self::Fallback => "Fallback",
            Self::Conflicting => "Mismatched cut",
        }
    }

    pub fn is_trusted_sync(self) -> bool {
        matches!(self, Self::ExactHash | Self::ExactRelease)
    }

    pub fn is_compatible_sync(self) -> bool {
        matches!(
            self,
            Self::ExactHash | Self::ExactRelease | Self::SameServiceAndCut | Self::CompatibleCut
        )
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

fn strip_trailing_subtitle_suffixes(base_name: &str) -> &str {
    const STRIPPABLE_SUFFIXES: &[&str] = &[
        "srt", "vtt", "ass", "ssa", "sub", "eng", "en", "hi", "sdh", "cc", "forced", "foreign",
    ];
    let mut cur = base_name.trim();
    loop {
        let mut stripped_any = false;
        if let Some(sep_idx) = cur.rfind(['.', '_']) {
            let suffix = &cur[sep_idx + 1..];
            if STRIPPABLE_SUFFIXES
                .iter()
                .any(|s| suffix.eq_ignore_ascii_case(s))
            {
                cur = cur[..sep_idx].trim_end();
                stripped_any = true;
            }
        }
        if !stripped_any {
            break;
        }
    }
    cur
}

fn extract_subtitle_release_group(raw: &str) -> Option<String> {
    let base_name = Path::new(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(raw)
        .trim();
    let cleaned = strip_trailing_subtitle_suffixes(base_name);
    let group = RE_SUB_RELEASE_GROUP.captures(cleaned)?.get(1)?.as_str();
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

        let tier = if has_hard_conflict {
            SubtitleMatchTier::Conflicting
        } else if release_group_match {
            score += 4_000_000;
            SubtitleMatchTier::ExactRelease
        } else if same_service_and_cut {
            score += 1_800_000;
            SubtitleMatchTier::SameServiceAndCut
        } else if compatible_cut {
            score += 600_000;
            SubtitleMatchTier::CompatibleCut
        } else {
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
    #[serde(default)]
    pub hearing_impaired: bool,
    #[serde(default)]
    pub forced: bool,
    #[serde(default)]
    pub content_profile: SubtitleContentProfile,
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
    hearing_impaired: bool,
    forced: bool,
    download: ProviderDownloadSpec,
}

#[derive(Clone, Debug)]
struct DownloadedSubtitleAnalysis {
    sub: FetchedNativeSubtitle,
    analysis: AnalyzedSubtitleContent,
}

pub fn has_explicit_forced_marker(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("unforced") {
        return false;
    }
    if lower.contains("forced")
        || lower.contains("foreign parts")
        || lower.contains("foreign-parts")
        || lower.contains("foreign_parts")
        || lower.contains("foreign only")
        || lower.contains("foreign-only")
        || lower.contains("foreign dialogue")
        || lower.contains("non-english")
        || lower.contains("non english")
        || lower.contains("non_english")
        || lower.contains("signs & songs")
        || lower.contains("signs and songs")
        || lower.contains("signs/songs")
        || lower.contains("s&s")
    {
        return true;
    }
    lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|tok| tok == "foreign")
}

fn json_opt_u32(val: Option<&Value>) -> Option<u32> {
    let v = val?;
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u32>().ok()))
}

fn fps_hint_from_milli(val: Option<&Value>) -> Option<FpsHint> {
    let milli = json_opt_u32(val)?;
    match milli {
        23_970..=23_985 => Some(FpsHint::Fps23976),
        23_990..=24_010 => Some(FpsHint::Fps24000),
        24_990..=25_010 => Some(FpsHint::Fps25000),
        29_960..=29_980 => Some(FpsHint::Fps29970),
        _ => None,
    }
}

fn merge_fingerprint(
    mut base: VideoReleaseFingerprint,
    extra: VideoReleaseFingerprint,
) -> VideoReleaseFingerprint {
    if base.release_group.is_none() {
        base.release_group = extra.release_group;
    }
    if base.streaming_service.is_none() {
        base.streaming_service = extra.streaming_service;
    }
    if base.source_cut == SubtitleSourceCut::Unknown {
        base.source_cut = extra.source_cut;
    }
    if base.edition.is_none() {
        base.edition = extra.edition;
    }
    if base.fps_hint.is_none() {
        base.fps_hint = extra.fps_hint;
    }
    base
}

fn is_sdh_metadata_title(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("hearing impaired") || lower.contains("closed caption") {
        return true;
    }
    lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|tok| matches!(tok, "sdh" | "cc" | "hi"))
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
            a.match_tier
                .cmp(&b.match_tier)
                .then_with(|| b.score.cmp(&a.score))
                .then_with(|| a.provider.cmp(&b.provider))
                .then_with(|| a.release_name.cmp(&b.release_name))
        });

        let mut seen_downloads = HashSet::new();
        candidates.retain(|cand| {
            let key = match &cand.download {
                ProviderDownloadSpec::DirectUrl(u) => u.clone(),
                ProviderDownloadSpec::OpenSubtitlesFileId { file_id, .. } => {
                    format!("os:{file_id}")
                }
            };
            seen_downloads.insert(key)
        });

        let attempt_limit = max_downloads.saturating_mul(2).clamp(4, 8);
        let mut to_download: Vec<RankedSubtitleCandidate> = Vec::with_capacity(attempt_limit + 1);
        match query.search_mode {
            SubtitleSearchMode::Full => {
                for cand in candidates.iter().filter(|c| !c.forced).take(attempt_limit) {
                    to_download.push(cand.clone());
                }
                if let Some(oracle) = candidates.iter().find(|c| c.forced) {
                    to_download.push(oracle.clone());
                }
                if to_download.is_empty() {
                    to_download.extend(candidates.into_iter().take(attempt_limit));
                }
            }
            SubtitleSearchMode::ForcedOnly => {
                to_download.extend(candidates.into_iter().filter(|c| c.forced).take(attempt_limit));
            }
        }

        let fetch_futs = to_download.into_iter().map(|cand| {
            let endpoints_clone = endpoints.clone();
            async move { self.download_and_cache(cand, &endpoints_clone, query).await }
        });

        let downloaded: Vec<DownloadedSubtitleAnalysis> =
            futures::future::join_all(fetch_futs)
                .await
                .into_iter()
                .flatten()
                .collect();
        let elapsed_ms = started.elapsed().as_millis() as u64;

        let mut out =
            rerank_downloaded_subtitles(downloaded, query.search_mode, max_downloads, &self.cache_dir);
        for item in &mut out {
            item.lookup_elapsed_ms = elapsed_ms;
        }
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
        let has_hash = query
            .opensubhash
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(hash) = has_hash {
            extra_parts.push(format!("videoHash={}", urlencoding::encode(hash)));
        }
        if let Some(size) = query.video_byte_size.filter(|&b| b > 0) {
            extra_parts.push(format!("videoSize={size}"));
        }
        if let Some(fname) = query
            .video_filename
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            extra_parts.push(format!("filename={}", urlencoding::encode(fname)));
        }

        let plain_url = format!("{base}/subtitles/{media_type}/{stremio_id}.json");
        let primary_url = if extra_parts.is_empty() {
            plain_url.clone()
        } else {
            format!(
                "{base}/subtitles/{media_type}/{stremio_id}/{}.json",
                extra_parts.join("&")
            )
        };

        let fetch_subtitles_json = |target_url: String| async move {
            let resp = match self.client.get(&target_url).send().await {
                Ok(r) if r.status().is_success() => r,
                Ok(r) => {
                    debug!("OpenSubtitles v3 returned status {}", r.status());
                    return None;
                }
                Err(err) => {
                    debug!("OpenSubtitles v3 lookup failed: {err}");
                    return None;
                }
            };
            match resp.json::<Value>().await {
                Ok(v) => v.get("subtitles").and_then(Value::as_array).cloned(),
                Err(err) => {
                    debug!("OpenSubtitles v3 invalid JSON: {err}");
                    None
                }
            }
        };

        let Some(items) = fetch_subtitles_json(primary_url.clone()).await else {
            return Vec::new();
        };

        let primary_has_explicit_hash_metadata = items.iter().any(|item| {
            item.get("m")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.trim().is_empty())
                || item.get("hashMatch").and_then(Value::as_bool).is_some()
                || item
                    .get("MovieHash")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty())
        });

        // Only fetch `plain_url` lazily when the primary response lacks explicit hash metadata
        // (`m`, `hashMatch`, `MovieHash`) on all items.
        let plain_items_opt = if has_hash.is_some()
            && primary_url != plain_url
            && !primary_has_explicit_hash_metadata
        {
            fetch_subtitles_json(plain_url).await
        } else {
            None
        };

        let mut plain_ranks_by_lang: HashMap<String, HashMap<String, usize>> = HashMap::new();
        let plain_available = plain_items_opt
            .as_ref()
            .is_some_and(|list| !list.is_empty());
        if let Some(plain_items) = plain_items_opt.as_ref() {
            for pitem in plain_items {
                let lang = normalize_subtitle_lang_code(
                    pitem.get("lang").and_then(Value::as_str).unwrap_or("und"),
                );
                let per_lang = plain_ranks_by_lang.entry(lang).or_default();
                let next_rank = per_lang.len();
                if let Some(id) = pitem.get("id").and_then(Value::as_str).map(str::trim) {
                    per_lang.entry(format!("id:{id}")).or_insert(next_rank);
                }
                if let Some(u) = pitem.get("url").and_then(Value::as_str).map(str::trim) {
                    per_lang.entry(format!("url:{u}")).or_insert(next_rank);
                }
            }
        }

        let mut hash_rank_by_lang: HashMap<String, usize> = HashMap::new();
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let Some(sub_url) = item.get("url").and_then(Value::as_str).map(str::trim) else {
                continue;
            };
            if !is_allowed_addon_stream_url(sub_url) {
                continue;
            }
            if let (Some(req_s), Some(item_s)) = (query.season, json_opt_u32(item.get("season"))) {
                if item_s != req_s {
                    continue;
                }
            }
            if let (Some(req_e), Some(item_e)) = (query.episode, json_opt_u32(item.get("episode")))
            {
                if item_e != req_e {
                    continue;
                }
            }

            let lang_raw = item
                .get("lang")
                .and_then(Value::as_str)
                .unwrap_or("und");
            let lang = normalize_subtitle_lang_code(lang_raw);
            let cur_hash_rank = {
                let entry = hash_rank_by_lang.entry(lang.clone()).or_insert(0);
                let r = *entry;
                *entry += 1;
                r
            };
            if !query.preferred_langs.is_empty()
                && !query
                    .preferred_langs
                    .iter()
                    .any(|p| subtitle_lang_matches(p, &lang))
            {
                continue;
            }

            let subtitle_file_name = item
                .get("subtitleFileName")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let movie_release_name = item
                .get("movieReleaseName")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());

            let release_name = subtitle_file_name
                .or(movie_release_name)
                .or_else(|| {
                    item.get("SubFileName")
                        .or_else(|| item.get("release"))
                        .or_else(|| item.get("title"))
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                })
                .unwrap_or("OpenSubtitles v3")
                .to_owned();
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&release_name)
                .to_owned();

            let item_id = item.get("id").and_then(Value::as_str).map(str::trim);
            let plain_diff_match = if has_hash.is_some() && plain_available {
                let per_lang_plain = plain_ranks_by_lang.get(&lang);
                match per_lang_plain {
                    None => true,
                    Some(map) => {
                        let rank_in_plain = item_id
                            .and_then(|id| map.get(&format!("id:{id}")).copied())
                            .or_else(|| map.get(&format!("url:{sub_url}")).copied());
                        match rank_in_plain {
                            None => true,
                            Some(plain_rank) => cur_hash_rank == 0 && plain_rank >= 2,
                        }
                    }
                }
            } else {
                false
            };

            let explicit_m = item
                .get("m")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let hash_match = if let Some(m_tag) = explicit_m {
                m_tag.eq_ignore_ascii_case("h")
            } else if let Some(hm) = item.get("hashMatch").and_then(Value::as_bool) {
                hm
            } else if let (Some(mh), Some(qh)) = (
                item.get("MovieHash")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty()),
                has_hash,
            ) {
                mh.eq_ignore_ascii_case(qh)
            } else {
                plain_diff_match
            };

            let forced = item.get("forced").and_then(Value::as_bool).unwrap_or(false)
                || has_explicit_forced_marker(&release_name)
                || movie_release_name.is_some_and(has_explicit_forced_marker)
                || has_explicit_forced_marker(&title)
                || has_explicit_forced_marker(sub_url);
            if query.search_mode == SubtitleSearchMode::ForcedOnly && !forced {
                continue;
            }
            let hearing_impaired = item
                .get("hearingImpaired")
                .or_else(|| item.get("hi"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || is_sdh_metadata_title(&release_name)
                || movie_release_name.is_some_and(is_sdh_metadata_title)
                || is_sdh_metadata_title(&title);

            let mut cand_fp = match (subtitle_file_name, movie_release_name) {
                (Some(sub_fn), Some(mov_rel)) => merge_fingerprint(
                    VideoReleaseFingerprint::parse(sub_fn),
                    VideoReleaseFingerprint::parse(mov_rel),
                ),
                (Some(sub_fn), None) => VideoReleaseFingerprint::parse(sub_fn),
                (None, Some(mov_rel)) => VideoReleaseFingerprint::parse(mov_rel),
                (None, None) => VideoReleaseFingerprint::parse(&release_name),
            };
            if cand_fp.release_group.is_none() {
                if let Some(group) = item
                    .get("releaseGroup")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    cand_fp.release_group = Some(group.to_owned());
                }
            }
            if let Some(fmt) = item
                .get("releaseFormat")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                cand_fp = merge_fingerprint(cand_fp, VideoReleaseFingerprint::parse(fmt));
            }
            if cand_fp.fps_hint.is_none() {
                cand_fp.fps_hint = fps_hint_from_milli(item.get("fpsMilli"));
            }

            let (tier, score) = SubtitleMatchRanker::score_candidate(
                video_fp,
                &cand_fp,
                hash_match,
                &lang,
                &query.preferred_langs,
                None,
                hearing_impaired,
            );

            out.push(RankedSubtitleCandidate {
                provider: "OpenSubtitles v3".to_owned(),
                lang,
                title,
                release_name,
                match_tier: tier,
                score,
                hearing_impaired,
                forced,
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

        let Ok(mut url) = Url::parse(&format!("{base}/subtitles")) else {
            return Vec::new();
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
            pairs.append_pair(
                "foreign_parts_only",
                if query.search_mode == SubtitleSearchMode::ForcedOnly {
                    "only"
                } else {
                    "include"
                },
            );
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
                .unwrap_or(false)
                || is_sdh_metadata_title(&effective_release)
                || is_sdh_metadata_title(&title);
            let forced = attrs
                .get("foreign_parts_only")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || has_explicit_forced_marker(&effective_release)
                || has_explicit_forced_marker(&title);
            if query.search_mode == SubtitleSearchMode::ForcedOnly && !forced {
                continue;
            }

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
                hearing_impaired: hi,
                forced,
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

        let Ok(mut url) = Url::parse(&format!("{base}/subtitles")) else {
            return Vec::new();
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
            let comment = item
                .get("comment")
                .and_then(Value::as_str)
                .unwrap_or("");
            let hi = item.get("hi").and_then(Value::as_bool).unwrap_or(false)
                || is_sdh_metadata_title(&release_name)
                || is_sdh_metadata_title(&title)
                || is_sdh_metadata_title(comment);
            let forced = item.get("forced").and_then(Value::as_bool).unwrap_or(false)
                || has_explicit_forced_marker(&release_name)
                || has_explicit_forced_marker(&title)
                || has_explicit_forced_marker(comment);
            if query.search_mode == SubtitleSearchMode::ForcedOnly && !forced {
                continue;
            }

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
                hearing_impaired: hi,
                forced,
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
    ) -> Option<DownloadedSubtitleAnalysis> {
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

        let prefer_forced_entry =
            candidate.forced || query.search_mode == SubtitleSearchMode::ForcedOnly;
        let (utf8_text, detected_ext) = match decode_subtitle_payload_with_mode(
            &raw_bytes,
            query.season,
            query.episode,
            prefer_forced_entry,
        ) {
            Ok(decoded) => decoded,
            Err(err) => {
                warn!("Failed to decode subtitle payload from {}: {err}", candidate.provider);
                return None;
            }
        };

        let analysis = analyze_subtitle_content(&utf8_text);
        let hearing_impaired =
            candidate.hearing_impaired || analysis.profile.sdh_ratio_pct >= 25;

        let local_path = match write_cached_subtitle(&self.cache_dir, &utf8_text, detected_ext) {
            Ok(path) => path,
            Err(err) => {
                warn!("Failed to cache decoded subtitle: {err}");
                return None;
            }
        };

        let sub = FetchedNativeSubtitle {
            provider: candidate.provider,
            lang: candidate.lang,
            title: candidate.title,
            release_name: candidate.release_name,
            match_tier: candidate.match_tier,
            score: candidate.score,
            local_path,
            lookup_elapsed_ms: 0,
            hearing_impaired,
            forced: candidate.forced,
            content_profile: analysis.profile.clone(),
        };
        Some(DownloadedSubtitleAnalysis { sub, analysis })
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
    decode_subtitle_payload_with_mode(raw, season, episode, false)
}

fn decode_subtitle_payload_with_mode(
    raw: &[u8],
    season: Option<u32>,
    episode: Option<u32>,
    prefer_forced: bool,
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
        let (entry_bytes, ext) =
            extract_best_subtitle_from_zip_with_mode(raw, season, episode, prefer_forced)?;
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
    prefer_forced: bool,
) -> i32 {
    let mut score: i32 = match ext {
        "ass" | "ssa" => 300,
        "srt" => 250,
        "vtt" => 150,
        _ => 0,
    };

    let lower = name.to_ascii_lowercase();
    if has_explicit_forced_marker(&lower) {
        score += if prefer_forced { 500 } else { -100 };
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
    extract_best_subtitle_from_zip_with_mode(archive, season, episode, false)
}

fn extract_best_subtitle_from_zip_with_mode(
    archive: &[u8],
    season: Option<u32>,
    episode: Option<u32>,
    prefer_forced: bool,
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

        let score = score_zip_entry(&name, ext, season, episode, prefer_forced);
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

fn head_ascii_lowercase(content: &str, max_bytes: usize) -> String {
    let mut end = content.len().min(max_bytes);
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    content[..end].to_ascii_lowercase()
}

fn detect_subtitle_extension(content: &str) -> &'static str {
    let lower = head_ascii_lowercase(content, 1024);
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

    let seq = TEMP_SUB_SEQ.fetch_add(1, Ordering::Relaxed);
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

pub fn prune_subtitle_cache_dir(cache_dir: &Path) -> usize {
    prune_subtitle_cache_dir_with_limits(
        cache_dir,
        MAX_SUBTITLE_CACHE_FILES,
        MAX_SUBTITLE_CACHE_BYTES,
        None,
    )
}

fn prune_subtitle_cache_dir_with_limits(
    cache_dir: &Path,
    max_files: usize,
    max_bytes: u64,
    keep_path: Option<&Path>,
) -> usize {
    let Ok(read_dir) = std::fs::read_dir(cache_dir) else {
        return 0;
    };

    let mut entries = Vec::new();
    let mut total_bytes: u64 = 0;
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let len = meta.len();
        total_bytes = total_bytes.saturating_add(len);
        let modified = meta
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        entries.push((modified, len, path));
    }

    let mut total_files = entries.len();
    if total_files <= max_files && total_bytes <= max_bytes {
        return 0;
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));

    let mut removed = 0usize;
    for (_, len, path) in entries {
        if total_files <= max_files && total_bytes <= max_bytes {
            break;
        }
        if keep_path.is_some_and(|keep| keep == path.as_path()) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            total_files = total_files.saturating_sub(1);
            total_bytes = total_bytes.saturating_sub(len);
            removed += 1;
        }
    }
    removed
}

fn write_cached_subtitle(cache_dir: &Path, utf8_content: &str, ext: &str) -> Result<PathBuf, String> {
    ensure_private_subtitle_dir(cache_dir)?;
    let mut hasher = Sha256::new();
    hasher.update(utf8_content.as_bytes());
    let digest_hex = hex::encode(hasher.finalize());
    let prefix = &digest_hex[..24];
    let out_path = cache_dir.join(format!("{prefix}.{ext}"));
    write_private_file(&out_path, utf8_content)?;
    let _ = prune_subtitle_cache_dir_with_limits(
        cache_dir,
        MAX_SUBTITLE_CACHE_FILES,
        MAX_SUBTITLE_CACHE_BYTES,
        Some(&out_path),
    );
    Ok(out_path)
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubtitleAlignmentResult {
    pub offset_ms: i64,
    pub drift_ratio: f64,
    pub matched_onsets: usize,
    pub confidence: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioSpeechWindow {
    pub start_ms: i64,
    pub frame_ms: i64,
    pub vad: Vec<f32>,
    pub onset: Vec<f32>,
}

impl AudioSpeechWindow {
    /// Extracts a noise- and music-resistant `[0.0..1.0]` speech activity + onset envelope from `8 kHz` mono PCM.
    pub fn from_pcm_8khz(start_secs: f64, pcm: &[i16]) -> Self {
        let start_ms = if start_secs.is_finite() {
            (start_secs.max(0.0) * 1000.0).round() as i64
        } else {
            0
        };
        let frame_count = pcm.len() / AUDIO_ENV_SAMPLES_PER_FRAME;
        if frame_count == 0 {
            return Self {
                start_ms,
                frame_ms: AUDIO_ENV_FRAME_MS,
                vad: Vec::new(),
                onset: Vec::new(),
            };
        }

        // Pre-emphasis + cascaded 300 Hz HP + 3200 Hz LP to isolate vocal formants from sustained bass/music.
        let mut prev_raw = 0.0_f32;
        let mut hp1_in = 0.0_f32;
        let mut hp1_out = 0.0_f32;
        let mut hp2_in = 0.0_f32;
        let mut hp2_out = 0.0_f32;
        let mut lp_out = 0.0_f32;

        let mut log_energies = Vec::with_capacity(frame_count);
        let (chunks, _) = pcm.as_chunks::<AUDIO_ENV_SAMPLES_PER_FRAME>();
        for chunk in chunks {
            let mut sq_sum = 0.0_f64;
            for &sample in chunk {
                let x = f32::from(sample) / 32768.0;
                let pre = x - 0.95 * prev_raw;
                prev_raw = x;

                hp1_out = 0.79 * (hp1_out + pre - hp1_in);
                hp1_in = pre;

                hp2_out = 0.79 * (hp2_out + hp1_out - hp2_in);
                hp2_in = hp1_out;

                lp_out = 0.65 * hp2_out + 0.35 * lp_out;
                sq_sum += f64::from(lp_out) * f64::from(lp_out);
            }
            let rms = (sq_sum / AUDIO_ENV_SAMPLES_PER_FRAME as f64).sqrt();
            log_energies.push((rms + 1e-6).ln() as f32);
        }

        // Subtract local 1.6s (80-frame) 25th-percentile floor so steady score/music beds fade to zero.
        let mut speech_mod = vec![0.0_f32; frame_count];
        let mut win_buf = Vec::with_capacity(81);
        for k in 0..frame_count {
            let lo = k.saturating_sub(40);
            let hi = (k + 41).min(frame_count);
            win_buf.clear();
            win_buf.extend_from_slice(&log_energies[lo..hi]);
            let pct_idx = win_buf.len() / 4;
            let (_, floor_val, _) =
                win_buf.select_nth_unstable_by(pct_idx, |a, b| a.total_cmp(b));
            speech_mod[k] = (log_energies[k] - *floor_val).max(0.0);
        }

        // Reuse log_energies as scratch buffer to find the 96th-percentile peak without extra allocation.
        log_energies.clear();
        log_energies.extend_from_slice(&speech_mod);
        let norm_idx = ((log_energies.len() * 96) / 100).min(log_energies.len().saturating_sub(1));
        let (_, peak_ref, _) =
            log_energies.select_nth_unstable_by(norm_idx, |a, b| a.total_cmp(b));
        let scale = peak_ref.max(0.25);

        for val in &mut speech_mod {
            // Suppress low residual floor below 12% of peak modulation.
            *val = (((*val / scale) - 0.12).max(0.0) / 0.88).min(1.0);
        }
        let vad = speech_mod;

        let mut onset = vec![0.0_f32; frame_count];
        for k in 1..frame_count {
            let prev_min = if k >= 2 {
                vad[k - 1].min(vad[k - 2])
            } else {
                vad[k - 1]
            };
            onset[k] = (vad[k] - prev_min).max(0.0);
        }

        Self {
            start_ms,
            frame_ms: AUDIO_ENV_FRAME_MS,
            vad,
            onset,
        }
    }

    pub fn end_ms(&self) -> i64 {
        self.start_ms + (self.vad.len() as i64 * self.frame_ms)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioSpeechFingerprint {
    pub windows: Vec<AudioSpeechWindow>,
}

impl AudioSpeechFingerprint {
    pub fn is_empty(&self) -> bool {
        self.windows.iter().all(|w| w.vad.is_empty())
    }
}

pub struct SubtitleSyncAligner;

impl SubtitleSyncAligner {
    /// Parses `(start_ms, end_ms)` cue intervals from `.srt`, `.vtt`, `.ass`, or `.ssa` text,
    /// stripping formatting tags and ignoring common uploader watermark cues.
    pub fn parse_subtitle_cues(content: &str) -> Vec<(i64, i64)> {
        let trimmed = content.strip_prefix('\u{feff}').unwrap_or(content);
        let lower_head = head_ascii_lowercase(trimmed, 1024);
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

    /// Parses spoken dialogue cues, filtering out pure lyrics (`♪`/`♫`) and bracketed SFX tags.
    pub fn parse_spoken_dialogue_cues(content: &str) -> Vec<(i64, i64)> {
        let trimmed = content.strip_prefix('\u{feff}').unwrap_or(content);
        let lower_head = head_ascii_lowercase(trimmed, 1024);
        if lower_head.contains("[events]") || lower_head.contains("dialogue:") {
            parse_ass_cues_filtered(trimmed, true)
        } else {
            parse_srt_or_vtt_cues_filtered(trimmed, true)
        }
    }

    /// Reads a subtitle file and returns spoken dialogue cues suited for audio-to-subtitle alignment.
    pub fn parse_spoken_dialogue_file(path: &Path) -> Result<Vec<(i64, i64)>, String> {
        let bytes = std::fs::read(path).map_err(|err| match err.kind() {
            ErrorKind::NotFound => format!("Subtitle file not found: {}", path.display()),
            _ => format!("Failed to read subtitle file {}: {err}", path.display()),
        })?;
        let text = normalize_bytes_to_utf8(&bytes)?;
        let spoken = Self::parse_spoken_dialogue_cues(&text);
        if spoken.len() >= 8 {
            Ok(spoken)
        } else {
            Ok(Self::parse_subtitle_cues(&text))
        }
    }

    /// Selects 1 or 2 high-density `(start_secs, duration_secs)` windows for fast audio extraction.
    pub fn select_dialogue_probe_windows(
        cues: &[(i64, i64)],
        playhead_secs: f64,
        duration_secs: f64,
    ) -> Vec<(f64, f64)> {
        let mut valid: Vec<(i64, i64)> = cues
            .iter()
            .copied()
            .filter(|&(s, e)| s >= 0 && e > s + 150)
            .collect();
        valid.sort_unstable_by_key(|&(s, _)| s);
        if valid.is_empty() {
            let start = if playhead_secs.is_finite() && playhead_secs > 45.0 {
                (playhead_secs - 25.0).max(0.0)
            } else {
                90.0
            };
            return vec![(start, 60.0)];
        }

        let max_cue_secs = valid
            .iter()
            .map(|&(_, e)| e as f64 / 1000.0)
            .fold(0.0_f64, f64::max);
        let media_end_secs = if duration_secs.is_finite() && duration_secs > 120.0 {
            duration_secs
        } else {
            max_cue_secs + 60.0
        };

        let pick_best_center = |min_secs: f64, max_secs: f64, anchor_secs: Option<f64>| -> f64 {
            let lo_ms = (min_secs * 1000.0) as i64;
            let hi_ms = (max_secs * 1000.0) as i64;
            let mut best_center = (min_secs + max_secs) * 0.5;
            let mut best_score = f64::NEG_INFINITY;

            for &(cs, _) in &valid {
                if cs < lo_ms || cs > hi_ms {
                    continue;
                }
                let mut cluster_score = 0.0_f64;
                let mut prev_end: Option<i64> = None;
                for &(s, e) in &valid {
                    if s >= cs - 25_000 && s <= cs + 25_000 && (e - s) <= 8_500 {
                        cluster_score += 1.0;
                        if prev_end.is_none_or(|pe| s - pe >= 320) {
                            cluster_score += 0.35;
                        }
                        prev_end = Some(e);
                    }
                }
                let center_s = cs as f64 / 1000.0;
                let dist_penalty = anchor_secs
                    .map(|a| ((center_s - a).abs() / 600.0).min(3.0))
                    .unwrap_or(0.0);
                let score = cluster_score - dist_penalty;
                if score > best_score {
                    best_score = score;
                    best_center = center_s;
                }
            }
            best_center
        };

        let (min_e, max_e, anchor) = if playhead_secs.is_finite() && playhead_secs > 50.0 {
            (
                (playhead_secs - 180.0).max(30.0),
                (playhead_secs + 240.0).min((media_end_secs - 70.0).max(40.0)),
                Some(playhead_secs),
            )
        } else if media_end_secs >= 720.0 {
            let mut min_early = (media_end_secs * 0.12).clamp(180.0, 420.0);
            let max_early = (media_end_secs * 0.42)
                .max(min_early + 120.0)
                .min(media_end_secs - 75.0);
            let min_early_ms = (min_early * 1000.0) as i64;
            let max_early_ms = (max_early * 1000.0) as i64;
            let early_cue_count = valid
                .iter()
                .filter(|&&(s, _)| s >= min_early_ms && s <= max_early_ms)
                .count();
            if early_cue_count < 5 {
                min_early = 45.0;
            }
            let anchor_early = Some(min_early + (max_early - min_early) * 0.35);
            (min_early, max_early, anchor_early)
        } else {
            (25.0, (media_end_secs - 65.0).max(40.0), None)
        };

        let c1 = if max_e > min_e + 20.0 {
            pick_best_center(min_e, max_e, anchor)
        } else {
            (media_end_secs * 0.35).max(30.0)
        };
        let w1_start = (c1 - 30.0).clamp(10.0, (media_end_secs - 70.0).max(10.0));
        let mut windows = vec![(w1_start, 60.0)];

        if media_end_secs >= 720.0 {
            if w1_start < media_end_secs * 0.50 {
                let min_2 = (w1_start + 390.0)
                    .max(media_end_secs * 0.58)
                    .min(media_end_secs - 130.0);
                let max_2 = (media_end_secs * 0.86)
                    .max(min_2 + 50.0)
                    .min(media_end_secs - 70.0);
                if max_2 > min_2 + 20.0 {
                    let c2 = pick_best_center(min_2, max_2, None);
                    let w2_start = (c2 - 30.0).clamp(min_2, (media_end_secs - 65.0).max(min_2));
                    windows.push((w2_start, 60.0));
                }
            } else {
                let min_2 = (media_end_secs * 0.12).clamp(120.0, 300.0);
                let max_2 = (w1_start - 390.0).max(min_2 + 50.0);
                if max_2 > min_2 + 20.0 && max_2 < w1_start - 180.0 {
                    let c2 = pick_best_center(min_2, max_2, None);
                    let w2_start = (c2 - 30.0).clamp(10.0, max_2);
                    windows.push((w2_start, 60.0));
                }
            }
        }
        windows
    }

    /// Correlates subtitle speech intervals and onsets against extracted `AudioSpeechFingerprint` windows.
    pub fn align_cues_to_audio(
        cues: &[(i64, i64)],
        fingerprint: &AudioSpeechFingerprint,
    ) -> SubtitleAlignmentResult {
        if cues.is_empty() || fingerprint.is_empty() {
            return SubtitleAlignmentResult {
                offset_ms: 0,
                drift_ratio: 1.0,
                matched_onsets: 0,
                confidence: 0.0,
            };
        }

        let filtered: Vec<(i64, i64)> = {
            let mut v: Vec<(i64, i64)> = cues
                .iter()
                .copied()
                .filter(|&(s, e)| e > s + 150 && (e - s) <= 12_000)
                .collect();
            if v.len() < 4 {
                v = cues.iter().copied().filter(|&(s, e)| e > s).collect();
            }
            v.sort_unstable_by_key(|&(s, _)| s);
            v
        };
        if filtered.is_empty() {
            return SubtitleAlignmentResult {
                offset_ms: 0,
                drift_ratio: 1.0,
                matched_onsets: 0,
                confidence: 0.0,
            };
        }

        #[derive(Clone, Copy)]
        struct CueMeta {
            start_ms: i64,
            box_dur_ms: i64,
            is_utterance_start: bool,
        }

        let cue_metas: Vec<CueMeta> = filtered
            .iter()
            .enumerate()
            .map(|(i, &(cs, ce))| {
                let is_utterance_start = i == 0 || (cs - filtered[i - 1].1) >= 320;
                let gap_after = if i + 1 < filtered.len() {
                    filtered[i + 1].0 - ce
                } else {
                    1_000
                };
                let raw_dur = ce - cs;
                let box_dur_ms = if gap_after >= 250 && raw_dur > 900 {
                    let tail_trim = ((raw_dur - 900) * 35 / 100).min(480);
                    (raw_dur - tail_trim).max(400)
                } else {
                    raw_dur
                };
                CueMeta {
                    start_ms: cs,
                    box_dur_ms,
                    is_utterance_start,
                }
            })
            .collect();

        struct PrecomputedWin<'a> {
            win: &'a AudioSpeechWindow,
            prefix_vad: Vec<f64>,
            sum_vad: f64,
            var_vad: f64,
        }

        let precomputed: Vec<PrecomputedWin<'_>> = fingerprint
            .windows
            .iter()
            .filter(|w| w.vad.len() >= 25)
            .map(|w| {
                let n = w.vad.len() as f64;
                let mut prefix_vad = Vec::with_capacity(w.vad.len() + 1);
                prefix_vad.push(0.0);
                let mut sum_v = 0.0_f64;
                let mut sum_v2 = 0.0_f64;
                for &val in &w.vad {
                    let fv = f64::from(val);
                    sum_v += fv;
                    sum_v2 += fv * fv;
                    prefix_vad.push(sum_v);
                }
                let var_vad = (sum_v2 - (sum_v * sum_v) / n).max(1e-9);
                PrecomputedWin {
                    win: w,
                    prefix_vad,
                    sum_vad: sum_v,
                    var_vad,
                }
            })
            .collect();

        if precomputed.is_empty() {
            return SubtitleAlignmentResult {
                offset_ms: 0,
                drift_ratio: 1.0,
                matched_onsets: 0,
                confidence: 0.0,
            };
        }

        let eval_candidate = |drift: f64, offset_ms: i64| -> (f64, usize) {
            let mut total_score = 0.0_f64;
            let mut total_weight = 0.0_f64;
            let mut matched_onsets = 0usize;

            for pw in &precomputed {
                let w = pw.win;
                let n = w.vad.len();
                let nf = n as f64;
                let w_start = w.start_ms;
                let w_end = w.end_ms();

                // Collect & merge shifted cue frame intervals [a, b) intersecting this window.
                let mut intervals: Vec<(usize, usize)> = Vec::new();
                let mut attack_sum = 0.0_f64;
                let mut attack_weight = 0.0_f64;
                let mut cues_in_win = 0usize;

                for cue in &cue_metas {
                    let ts = (cue.start_ms as f64 * drift).round() as i64 + offset_ms;
                    if ts > w_end {
                        break;
                    }
                    let te = ts + (cue.box_dur_ms as f64 * drift).round() as i64;
                    if te <= w_start {
                        continue;
                    }
                    let rel_s = ((ts - w_start).max(0) / w.frame_ms) as usize;
                    let rel_e = (((te - w_start + w.frame_ms - 1) / w.frame_ms) as usize).min(n);
                    if rel_e <= rel_s {
                        continue;
                    }

                    if ts >= w_start && ts < w_end {
                        cues_in_win += 1;
                        let center = (((ts - w_start) / w.frame_ms) as usize).min(n - 1);
                        let k_lo = center.saturating_sub(4);
                        let k_hi = (center + 4).min(n - 1);
                        let mut weighted_onset = 0.0_f64;
                        let mut raw_peak_onset = 0.0_f32;
                        for (k, &on_val) in w.onset.iter().enumerate().take(k_hi + 1).skip(k_lo) {
                            let dist = center.abs_diff(k);
                            let kernel = 1.0 - (dist as f64) * 0.18;
                            weighted_onset = weighted_onset.max(f64::from(on_val) * kernel);
                            if dist <= 3 {
                                raw_peak_onset = raw_peak_onset.max(on_val);
                            }
                        }

                        if cue.is_utterance_start && center >= 6 && center + 8 <= n {
                            let pre_lo = center.saturating_sub(10);
                            let pre_hi = center.saturating_sub(1);
                            let pre_mean = (pw.prefix_vad[pre_hi] - pw.prefix_vad[pre_lo])
                                / (pre_hi - pre_lo).max(1) as f64;
                            let post_hi = (center + 10).min(n);
                            let post_mean = (pw.prefix_vad[post_hi] - pw.prefix_vad[center])
                                / (post_hi - center).max(1) as f64;
                            let step = (post_mean - 0.70 * pre_mean).max(0.0);
                            attack_sum += 0.55 * weighted_onset + 0.45 * step;
                            attack_weight += 1.0;
                        } else {
                            attack_sum += 0.4 * (0.35 * weighted_onset);
                            attack_weight += 0.4;
                        }

                        if f64::from(raw_peak_onset) >= 0.12 || f64::from(w.vad[center]) >= 0.22 {
                            matched_onsets += 1;
                        }
                    }

                    if let Some(last) = intervals.last_mut() {
                        if rel_s <= last.1 {
                            last.1 = last.1.max(rel_e);
                            continue;
                        }
                    }
                    intervals.push((rel_s, rel_e));
                }

                if cues_in_win < 2 || intervals.is_empty() {
                    continue;
                }

                let mut dot = 0.0_f64;
                let mut ones = 0usize;
                for (a, b) in intervals {
                    dot += pw.prefix_vad[b] - pw.prefix_vad[a];
                    ones += b - a;
                }
                if ones == 0 || ones >= n {
                    continue;
                }
                let ones_f = ones as f64;
                let cov = dot - ones_f * (pw.sum_vad / nf);
                let var_m = ones_f - (ones_f * ones_f) / nf;
                if var_m <= 1e-9 {
                    continue;
                }
                let zncc = cov / (var_m * pw.var_vad).sqrt();
                let attack_mean = if attack_weight > 0.0 {
                    attack_sum / attack_weight
                } else {
                    0.0
                };
                let win_score = 0.58 * zncc + 0.62 * attack_mean;
                let weight = cues_in_win as f64;
                total_score += win_score * weight;
                total_weight += weight;
            }

            if total_weight <= 0.0 {
                return (-10.0, 0);
            }
            let drift_prior = if (drift - 1.0).abs() < 1e-6 {
                0.012
            } else {
                0.0
            };
            let shift_prior = -0.00025 * (offset_ms.abs() as f64 / 1000.0);
            (
                (total_score / total_weight) + drift_prior + shift_prior,
                matched_onsets,
            )
        };

        // Disable drift stretching if all audio windows fall inside a short range where 25/23.976 drift
        // would merely look identical to a constant shift.
        let min_win_ms = precomputed
            .iter()
            .map(|p| p.win.start_ms)
            .min()
            .unwrap_or(0);
        let max_win_ms = precomputed
            .iter()
            .map(|p| p.win.end_ms())
            .max()
            .unwrap_or(0);
        let drifts_to_test: &[f64] = if max_win_ms - min_win_ms >= 360_000 {
            &ALIGN_DRIFT_CANDIDATES
        } else {
            &ALIGN_DRIFT_CANDIDATES[..1]
        };

        let mut best_drift = 1.0_f64;
        let mut best_offset_ms = 0_i64;
        let mut best_score = f64::NEG_INFINITY;
        let mut best_matches = 0usize;

        for &drift in drifts_to_test {
            let mut top_coarse: Vec<(f64, i64)> = Vec::new();
            let mut off = -AUDIO_ALIGN_MAX_OFFSET_MS;
            while off <= AUDIO_ALIGN_MAX_OFFSET_MS {
                let (score, _) = eval_candidate(drift, off);
                if let Some(existing) = top_coarse
                    .iter_mut()
                    .find(|&&mut (_, prev_off)| (prev_off - off).abs() < 320)
                {
                    if score > existing.0 {
                        *existing = (score, off);
                        top_coarse.sort_by(|a, b| b.0.total_cmp(&a.0));
                    }
                } else if top_coarse.len() < 8 {
                    top_coarse.push((score, off));
                    top_coarse.sort_by(|a, b| b.0.total_cmp(&a.0));
                } else if score > top_coarse[7].0 {
                    top_coarse[7] = (score, off);
                    top_coarse.sort_by(|a, b| b.0.total_cmp(&a.0));
                }
                off += 40;
            }

            let mut local_best_score = f64::NEG_INFINITY;
            let mut local_best_off = 0_i64;
            let mut local_best_matches = 0usize;
            for &(_, coarse_off) in &top_coarse {
                let mut fine_off = (coarse_off - 160).max(-AUDIO_ALIGN_MAX_OFFSET_MS);
                let fine_max = (coarse_off + 160).min(AUDIO_ALIGN_MAX_OFFSET_MS);
                while fine_off <= fine_max {
                    let (score, matches) = eval_candidate(drift, fine_off);
                    if score > local_best_score {
                        local_best_score = score;
                        local_best_off = fine_off;
                        local_best_matches = matches;
                    }
                    fine_off += AUDIO_ENV_FRAME_MS;
                }
            }

            // 3-point parabolic refinement around local_best_off on the 20ms grid.
            let (s_left, _) = eval_candidate(drift, local_best_off - AUDIO_ENV_FRAME_MS);
            let (s_right, _) = eval_candidate(drift, local_best_off + AUDIO_ENV_FRAME_MS);
            let denom = 2.0 * (s_left - 2.0 * local_best_score + s_right);
            if denom.abs() > 1e-6 && denom < 0.0 {
                let frac = ((s_left - s_right) / denom).clamp(-0.5, 0.5);
                let sub_off =
                    local_best_off + (frac * AUDIO_ENV_FRAME_MS as f64).round() as i64;
                let (sub_score, sub_matches) = eval_candidate(drift, sub_off);
                if sub_score >= local_best_score {
                    local_best_score = sub_score;
                    local_best_off = sub_off;
                    local_best_matches = sub_matches;
                }
            }

            if local_best_score > best_score {
                best_score = local_best_score;
                best_drift = drift;
                best_offset_ms = local_best_off;
                best_matches = local_best_matches;
            }
        }

        let confidence = (best_score / 0.65).clamp(0.0, 1.0);
        SubtitleAlignmentResult {
            offset_ms: best_offset_ms,
            drift_ratio: best_drift,
            matched_onsets: best_matches,
            confidence,
        }
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

    /// Parses cues from `reference_srt_text`, aligns `target_path` onto the reference timeline
    /// requiring `matched_onsets >= 10` and `confidence >= 0.40`, and writes the aligned file.
    pub fn align_subtitle_file_to_reference_text(
        target_path: &Path,
        reference_srt_text: &str,
        output_path: &Path,
    ) -> Result<SubtitleAlignmentResult, String> {
        let target_bytes = std::fs::read(target_path).map_err(|err| match err.kind() {
            ErrorKind::NotFound => format!("Subtitle file not found: {}", target_path.display()),
            _ => format!("Failed to read subtitle file {}: {err}", target_path.display()),
        })?;
        let target_text = normalize_bytes_to_utf8(&target_bytes)?;
        let target_cues = {
            let spoken = Self::parse_spoken_dialogue_cues(&target_text);
            if spoken.len() >= 10 {
                spoken
            } else {
                Self::parse_subtitle_cues(&target_text)
            }
        };
        let reference_cues = {
            let spoken = Self::parse_spoken_dialogue_cues(reference_srt_text);
            if spoken.len() >= 10 {
                spoken
            } else {
                Self::parse_subtitle_cues(reference_srt_text)
            }
        };
        if target_cues.len() < 10 || reference_cues.len() < 10 {
            return Err(format!(
                "Insufficient subtitle cues for alignment (target={}, reference={})",
                target_cues.len(),
                reference_cues.len()
            ));
        }

        let alignment = Self::solve_subtitle_alignment(&target_cues, &reference_cues);
        if alignment.matched_onsets < 10 || alignment.confidence < 0.40 {
            return Err(format!(
                "Subtitle alignment below confidence threshold (matched_onsets={}, confidence={:.2})",
                alignment.matched_onsets, alignment.confidence
            ));
        }

        Self::write_aligned_subtitle_file(
            target_path,
            output_path,
            alignment.offset_ms,
            alignment.drift_ratio,
        )?;
        Ok(alignment)
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
    let no_ass = if raw.contains('{') {
        RE_ASS_OVERRIDE.replace_all(raw, " ")
    } else {
        std::borrow::Cow::Borrowed(raw)
    };
    let no_html = if no_ass.contains('<') {
        RE_HTML_TAG.replace_all(&no_ass, " ")
    } else {
        no_ass
    };
    let normalized = if no_html.contains('\\') {
        std::borrow::Cow::Owned(
            no_html
                .replace("\\N", " ")
                .replace("\\n", " ")
                .replace("\\h", " "),
        )
    } else {
        no_html
    };
    let mut out = String::with_capacity(normalized.len());
    for word in normalized.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
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

fn clean_cue_text_preserving_lines(raw: &str) -> String {
    let no_ass = if raw.contains('{') {
        RE_ASS_OVERRIDE.replace_all(raw, " ")
    } else {
        std::borrow::Cow::Borrowed(raw)
    };
    let no_html = if no_ass.contains('<') {
        RE_HTML_TAG.replace_all(&no_ass, " ")
    } else {
        no_ass
    };
    no_html
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace("\\N", "\n")
        .replace("\\n", "\n")
        .replace("\\h", " ")
}

fn is_single_line_foreign_placeholder(line: &str) -> bool {
    let mut s = line.trim();
    if let Some(rest) = s.strip_prefix('-').or_else(|| s.strip_prefix('–')) {
        s = rest.trim();
    }
    let inner = if let Some(rest) = s.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        rest.trim()
    } else if let Some(rest) = s.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        rest.trim()
    } else {
        return false;
    };

    if inner.is_empty()
        || inner.contains('[')
        || inner.contains(']')
        || inner.contains('(')
        || inner.contains(')')
    {
        return false;
    }

    let normalized = inner
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if normalized.is_empty() {
        return false;
    }

    const EXPLICIT_FOREIGN_PHRASES: &[&str] = &[
        "foreign language",
        "foreign dialogue",
        "foreign dialog",
        "foreign speech",
        "foreign tongue",
        "non-english",
        "non english",
        "native tongue",
        "native language",
        "alien language",
        "unknown language",
        "another language",
        "other language",
    ];
    if EXPLICIT_FOREIGN_PHRASES
        .iter()
        .any(|phrase| normalized.contains(phrase))
    {
        return !normalized.contains("in english");
    }

    for prefix in ["speaking ", "speaks "] {
        if let Some(rest) = normalized.strip_prefix(prefix) {
            return is_plausible_foreign_target(rest, true);
        }
    }

    const VERB_IN_PREFIXES: &[&str] = &[
        "in ",
        "talking in ",
        "conversing in ",
        "dialogue in ",
        "dialog in ",
        "chanting in ",
        "singing in ",
        "shouting in ",
        "yelling in ",
        "screaming in ",
        "whispering in ",
        "muttering in ",
        "mutters in ",
        "arguing in ",
        "spoken in ",
    ];
    for prefix in VERB_IN_PREFIXES {
        if let Some(rest) = normalized.strip_prefix(prefix) {
            return is_plausible_foreign_target(rest, false);
        }
    }

    false
}

fn is_plausible_foreign_target(raw_target: &str, allow_in_prefix: bool) -> bool {
    let mut target = raw_target
        .trim_matches(|c: char| !c.is_alphanumeric() && c != ' ' && c != '-')
        .trim();
    if allow_in_prefix {
        if let Some(after_in) = target.strip_prefix("in ") {
            target = after_in.trim();
        }
    }
    if target.is_empty() {
        return false;
    }

    let words: Vec<&str> = target.split_whitespace().collect();
    if words.is_empty() || words.len() > 3 {
        return false;
    }

    const EXCLUDED_PHRASES: &[&str] = &[
        "english",
        "unison",
        "background",
        "the background",
        "distance",
        "the distance",
        "pain",
        "tears",
        "shock",
        "fear",
        "panic",
        "awe",
        "wonder",
        "anger",
        "silence",
        "whisper",
        "a whisper",
        "chorus",
        "harmony",
        "tongues",
        "riddles",
        "code",
    ];
    if EXCLUDED_PHRASES.contains(&target) {
        return false;
    }

    const EXCLUDED_FIRST_WORDS: &[&str] = &[
        "softly",
        "quietly",
        "loudly",
        "slowly",
        "quickly",
        "fast",
        "calmly",
        "angrily",
        "sadly",
        "gently",
        "coldly",
        "firmly",
        "urgently",
        "nervously",
        "weakly",
        "faintly",
        "hoarsely",
        "harshly",
        "brightly",
        "excitedly",
        "rapidly",
        "briefly",
        "indistinctly",
        "inaudibly",
        "overlapping",
        "simultaneously",
        "into",
        "on",
        "over",
        "through",
        "to",
        "with",
        "from",
        "at",
        "for",
        "out",
        "off",
        "clearly",
        "directly",
    ];
    if EXCLUDED_FIRST_WORDS.contains(&words[0]) {
        return false;
    }

    if target.contains("phone")
        || target.contains("radio")
        || target.contains("speaker")
        || target.contains("mic")
        || target.contains("intercom")
        || target.contains("walkie")
        || target.contains("voiceover")
        || target.contains("offscreen")
        || target.contains("off-screen")
    {
        return false;
    }

    true
}

/// Returns true when the entire subtitle cue is an untranslated foreign-speech placeholder
/// such as `[speaking Dothraki]`, `(in Japanese)`, or `(in foreign language)`.
pub fn is_untranslated_foreign_placeholder(raw_or_cleaned: &str) -> bool {
    let cleaned = clean_cue_text_preserving_lines(raw_or_cleaned);
    let non_empty: Vec<&str> = cleaned
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if non_empty.is_empty() {
        return false;
    }
    non_empty
        .iter()
        .all(|line| is_single_line_foreign_placeholder(line))
}

fn is_sdh_line_or_cue(cleaned_text: &str) -> bool {
    if is_non_speech_cue(cleaned_text) {
        return true;
    }
    let trimmed = cleaned_text.trim();
    if trimmed.contains('♪') || trimmed.contains('♫') {
        return true;
    }
    if (trimmed.starts_with('[') && trimmed.contains(']'))
        || (trimmed.starts_with('(') && trimmed.contains(')'))
    {
        return true;
    }
    false
}

/// Single-pass content analyzer for downloaded `.srt`/`.vtt`/`.ass` subtitle files (< 1 ms).
pub fn analyze_subtitle_content(content: &str) -> AnalyzedSubtitleContent {
    let entries = extract_subtitle_cue_entries(content);
    if entries.is_empty() {
        return AnalyzedSubtitleContent::default();
    }

    let mut cue_count = 0usize;
    let mut foreign_placeholder_count = 0usize;
    let mut sdh_cue_count = 0usize;
    let mut min_start = i64::MAX;
    let mut max_end = i64::MIN;
    let mut spoken_cues = Vec::with_capacity(entries.len());
    let mut foreign_placeholder_timestamps_ms = Vec::new();

    for (start_ms, end_ms, raw_body, cleaned) in entries {
        cue_count += 1;
        min_start = min_start.min(start_ms);
        max_end = max_end.max(end_ms);

        if is_untranslated_foreign_placeholder(&raw_body) {
            foreign_placeholder_count += 1;
            if foreign_placeholder_timestamps_ms.len() < 80 {
                foreign_placeholder_timestamps_ms.push(start_ms);
            }
        } else if is_non_speech_cue(&cleaned) {
            sdh_cue_count += 1;
        } else {
            if is_sdh_line_or_cue(&cleaned) {
                sdh_cue_count += 1;
            }
            spoken_cues.push((start_ms, end_ms));
        }
    }

    spoken_cues.sort_unstable();
    let span_ms = if max_end > min_start {
        max_end - min_start
    } else {
        0
    };
    let sdh_ratio_pct = ((sdh_cue_count * 100) / cue_count.max(1)).min(100) as u8;

    AnalyzedSubtitleContent {
        profile: SubtitleContentProfile {
            cue_count,
            span_ms,
            foreign_placeholder_count,
            sdh_cue_count,
            sdh_ratio_pct,
            foreign_status: if foreign_placeholder_count > 0 {
                ForeignTranslationStatus::MissingForeign
            } else {
                ForeignTranslationStatus::Unknown
            },
        },
        spoken_cues,
        foreign_placeholder_timestamps_ms,
    }
}

fn is_timestamp_covered_by_spoken_cues(sorted_cues: &[(i64, i64)], t: i64, tol_ms: i64) -> bool {
    let idx = sorted_cues.partition_point(|&(s, _)| s <= t + tol_ms);
    sorted_cues[..idx]
        .iter()
        .rev()
        .take_while(|&&(s, _)| s >= t - 60_000)
        .any(|&(_, e)| e >= t - tol_ms)
}

fn forced_oracle_shares_timeline(
    oracle: &FetchedNativeSubtitle,
    candidate: &FetchedNativeSubtitle,
) -> bool {
    if oracle.match_tier == SubtitleMatchTier::ExactHash
        && candidate.match_tier == SubtitleMatchTier::ExactHash
    {
        return true;
    }
    if oracle.match_tier == SubtitleMatchTier::ExactRelease
        && candidate.match_tier == SubtitleMatchTier::ExactRelease
    {
        if let (Some(og), Some(cg)) = (
            extract_subtitle_release_group(&oracle.release_name),
            extract_subtitle_release_group(&candidate.release_name),
        ) {
            return og.eq_ignore_ascii_case(&cg);
        }
    }
    false
}

fn rerank_downloaded_subtitles(
    mut downloaded: Vec<DownloadedSubtitleAnalysis>,
    mode: SubtitleSearchMode,
    max_downloads: usize,
    cache_dir: &Path,
) -> Vec<FetchedNativeSubtitle> {
    if downloaded.is_empty() || max_downloads == 0 {
        return Vec::new();
    }

    if mode == SubtitleSearchMode::ForcedOnly {
        let mut forced_out: Vec<FetchedNativeSubtitle> = downloaded
            .into_iter()
            .filter(|item| {
                item.sub.forced
                    && item.analysis.profile.cue_count > 0
                    && item.analysis.profile.cue_count <= 400
            })
            .map(|item| item.sub)
            .collect();
        forced_out.sort_by(|a, b| {
            a.match_tier
                .cmp(&b.match_tier)
                .then_with(|| b.score.cmp(&a.score))
                .then_with(|| a.provider.cmp(&b.provider))
        });
        forced_out.truncate(max_downloads);
        return forced_out;
    }

    let non_forced_indices: Vec<usize> = downloaded
        .iter()
        .enumerate()
        .filter(|(_, item)| !item.sub.forced)
        .map(|(idx, _)| idx)
        .collect();

    if non_forced_indices.is_empty() {
        return Vec::new();
    }

    let max_non_forced_cues = non_forced_indices
        .iter()
        .map(|&i| downloaded[i].analysis.profile.cue_count)
        .max()
        .unwrap_or(0);

    // Collect foreign-speech oracle timestamps from (a) placeholder captions in any full track
    // and (b) explicit Forced/foreign-parts tracks downloaded in the same parallel batch.
    let mut oracle_sources: Vec<(usize, bool)> = Vec::new();
    for (idx, item) in downloaded.iter().enumerate() {
        if !item.analysis.foreign_placeholder_timestamps_ms.is_empty() {
            oracle_sources.push((idx, false));
        } else if item.sub.forced
            && !item.analysis.spoken_cues.is_empty()
            && item.analysis.spoken_cues.len() <= 400
        {
            oracle_sources.push((idx, true));
        }
    }

    let mut computed_statuses = Vec::with_capacity(non_forced_indices.len());
    for &idx in &non_forced_indices {
        let item = &downloaded[idx];
        let status = if item.analysis.profile.foreign_placeholder_count > 0 {
            ForeignTranslationStatus::MissingForeign
        } else if !oracle_sources.is_empty() && item.analysis.profile.cue_count > 0 {
            let mut total_checks = 0usize;
            let mut total_covered = 0usize;

            for &(o_idx, is_forced_oracle) in &oracle_sources {
                if o_idx == idx {
                    continue;
                }
                let oracle = &downloaded[o_idx];
                if !is_forced_oracle {
                    if item.analysis.spoken_cues.len() < 10
                        || oracle.analysis.spoken_cues.len() < 10
                    {
                        continue;
                    }
                    let align = SubtitleSyncAligner::solve_subtitle_alignment(
                        &oracle.analysis.spoken_cues,
                        &item.analysis.spoken_cues,
                    );
                    if align.matched_onsets < 10 || align.confidence < 0.35 {
                        continue;
                    }

                    for &ts in oracle
                        .analysis
                        .foreign_placeholder_timestamps_ms
                        .iter()
                        .take(40)
                    {
                        let mapped = transform_ms(ts, align.offset_ms, align.drift_ratio);
                        total_checks += 1;
                        if is_timestamp_covered_by_spoken_cues(
                            &item.analysis.spoken_cues,
                            mapped,
                            1_200,
                        ) {
                            total_covered += 1;
                        }
                    }
                } else {
                    if !forced_oracle_shares_timeline(&oracle.sub, &item.sub) {
                        continue;
                    }
                    for &(ts, _) in oracle.analysis.spoken_cues.iter().take(60) {
                        total_checks += 1;
                        if is_timestamp_covered_by_spoken_cues(
                            &item.analysis.spoken_cues,
                            ts,
                            1_800,
                        ) {
                            total_covered += 1;
                        }
                    }
                }
            }

            if total_checks >= 1 && total_covered * 100 >= total_checks * 50 {
                ForeignTranslationStatus::Translated
            } else if (total_checks >= 2 && total_covered * 100 < total_checks * 30)
                || (total_checks == 1 && total_covered == 0)
            {
                ForeignTranslationStatus::MissingForeign
            } else {
                ForeignTranslationStatus::Unknown
            }
        } else {
            ForeignTranslationStatus::Unknown
        };
        computed_statuses.push((idx, status));
    }

    for (idx, status) in computed_statuses {
        let item = &mut downloaded[idx];
        item.sub.content_profile.foreign_status = status;

        if max_non_forced_cues >= 150 && item.analysis.profile.cue_count < 30 {
            item.sub.match_tier = SubtitleMatchTier::Conflicting;
            item.sub.score -= 15_000_000;
        }
        match status {
            ForeignTranslationStatus::Translated => {
                item.sub.score += 150_000;
            }
            ForeignTranslationStatus::MissingForeign => {
                item.sub.score -= 150_000;
            }
            ForeignTranslationStatus::Unknown => {}
        }
        if !item.sub.hearing_impaired && item.analysis.profile.sdh_ratio_pct < 18 {
            item.sub.score += 40_000;
        }
    }

    let mut non_forced_items: Vec<DownloadedSubtitleAnalysis> = downloaded
        .into_iter()
        .filter(|item| !item.sub.forced)
        .collect();
    non_forced_items.sort_by(|a, b| {
        a.sub
            .match_tier
            .cmp(&b.sub.match_tier)
            .then_with(|| b.sub.score.cmp(&a.sub.score))
            .then_with(|| a.sub.provider.cmp(&b.sub.provider))
    });

    // Verified cross-tier alignment: if the #1 high-trust timing reference (ExactHash / ExactRelease)
    // is MissingForeign, promote another non-forced downloaded track ONLY if it has zero placeholders,
    // aligns tightly to #1's spoken timeline (matched_onsets >= 15, confidence >= 0.45), and covers
    // >= 50% of #1's foreign placeholder timestamps after alignment.
    if non_forced_items.len() >= 2
        && non_forced_items[0].sub.match_tier.is_trusted_sync()
        && non_forced_items[0].sub.content_profile.foreign_status
            == ForeignTranslationStatus::MissingForeign
        && !non_forced_items[0]
            .analysis
            .foreign_placeholder_timestamps_ms
            .is_empty()
        && non_forced_items[0].analysis.spoken_cues.len() >= 15
    {
        let ref_spoken = &non_forced_items[0].analysis.spoken_cues;
        let ref_placeholders: Vec<i64> = non_forced_items[0]
            .analysis
            .foreign_placeholder_timestamps_ms
            .iter()
            .copied()
            .take(40)
            .collect();

        let mut best_match: Option<(usize, SubtitleAlignmentResult, bool, usize)> = None;
        for (idx, cand) in non_forced_items.iter().enumerate().skip(1) {
            if cand.analysis.profile.foreign_placeholder_count > 0 {
                continue;
            }
            if max_non_forced_cues >= 150 && cand.analysis.profile.cue_count < 30 {
                continue;
            }
            if cand.analysis.spoken_cues.len() < 15 {
                continue;
            }
            let align = SubtitleSyncAligner::solve_subtitle_alignment(
                &cand.analysis.spoken_cues,
                ref_spoken,
            );
            if align.matched_onsets < 15 || align.confidence < 0.45 {
                continue;
            }

            let mut aligned_cues: Vec<(i64, i64)> = cand
                .analysis
                .spoken_cues
                .iter()
                .map(|&(s, e)| {
                    let ns = transform_ms(s, align.offset_ms, align.drift_ratio);
                    let ne = transform_ms(e, align.offset_ms, align.drift_ratio).max(ns);
                    (ns, ne)
                })
                .collect();
            aligned_cues.sort_unstable();

            let covered = ref_placeholders
                .iter()
                .filter(|&&ts| is_timestamp_covered_by_spoken_cues(&aligned_cues, ts, 1_200))
                .count();
            if covered * 100 < ref_placeholders.len() * 50 {
                continue;
            }

            let is_clean = !cand.sub.hearing_impaired && cand.analysis.profile.sdh_ratio_pct < 18;
            let is_better = match &best_match {
                None => true,
                Some((prev_idx, prev_align, prev_clean, prev_cov)) => {
                    let prev_sub = &non_forced_items[*prev_idx].sub;
                    (is_clean, covered, align.matched_onsets)
                        > (*prev_clean, *prev_cov, prev_align.matched_onsets)
                        || ((is_clean, covered, align.matched_onsets)
                            == (*prev_clean, *prev_cov, prev_align.matched_onsets)
                            && (align.confidence > prev_align.confidence
                                || ((align.confidence - prev_align.confidence).abs() < 1e-9
                                    && (cand.sub.match_tier < prev_sub.match_tier
                                        || (cand.sub.match_tier == prev_sub.match_tier
                                            && cand.sub.score > prev_sub.score)))))
                }
            };
            if is_better {
                best_match = Some((idx, align, is_clean, covered));
            }
        }

        if let Some((best_idx, best_align, _, _)) = best_match {
            let src_path = non_forced_items[best_idx].sub.local_path.clone();
            let ext = src_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .filter(|e| matches!(e.as_str(), "srt" | "ass" | "ssa" | "vtt"))
                .unwrap_or_else(|| "srt".to_owned());
            let mut hasher = Sha256::new();
            hasher.update(src_path.to_string_lossy().as_bytes());
            hasher.update(best_align.offset_ms.to_le_bytes());
            hasher.update(best_align.drift_ratio.to_bits().to_le_bytes());
            let prefix = &hex::encode(hasher.finalize())[..24];
            let aligned_path = cache_dir.join(format!("{prefix}.aligned.{ext}"));

            if SubtitleSyncAligner::write_aligned_subtitle_file(
                &src_path,
                &aligned_path,
                best_align.offset_ms,
                best_align.drift_ratio,
            )
            .is_ok()
            {
                let _ = prune_subtitle_cache_dir_with_limits(
                    cache_dir,
                    MAX_SUBTITLE_CACHE_FILES,
                    MAX_SUBTITLE_CACHE_BYTES,
                    Some(&aligned_path),
                );
                let ref_tier = non_forced_items[0].sub.match_tier;
                let mut promoted = non_forced_items.remove(best_idx);
                promoted.sub.local_path = aligned_path;
                promoted.sub.content_profile.foreign_status = ForeignTranslationStatus::Translated;
                promoted.sub.match_tier = ref_tier;
                non_forced_items.insert(0, promoted);
            }
        }
    }

    let mut non_forced_out: Vec<FetchedNativeSubtitle> = non_forced_items
        .into_iter()
        .map(|item| item.sub)
        .collect();
    non_forced_out.truncate(max_downloads);
    non_forced_out
}

fn is_non_speech_cue(cleaned_text: &str) -> bool {
    let trimmed = cleaned_text.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.contains('♪') || trimmed.contains('♫') {
        return true;
    }
    (trimmed.starts_with('[') && trimmed.ends_with(']'))
        || (trimmed.starts_with('(') && trimmed.ends_with(')'))
}

fn extract_subtitle_cue_entries(content: &str) -> Vec<(i64, i64, String, String)> {
    let trimmed = content.strip_prefix('\u{feff}').unwrap_or(content);
    let lower_head = head_ascii_lowercase(trimmed, 1024);
    if lower_head.contains("[events]") || lower_head.contains("dialogue:") {
        extract_ass_cue_entries(trimmed)
    } else {
        extract_srt_or_vtt_cue_entries(trimmed)
    }
}

fn extract_srt_or_vtt_cue_entries(content: &str) -> Vec<(i64, i64, String, String)> {
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
        let body = lines[ts_idx + 1..].join("\n");
        let cleaned = clean_cue_text(&body);
        if is_watermark_cue(&cleaned) {
            continue;
        }
        cues.push((start_ms, end_ms, body, cleaned));
    }
    cues
}

fn extract_ass_cue_entries(content: &str) -> Vec<(i64, i64, String, String)> {
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
            let raw_body = fields[text_col].to_owned();
            let cleaned = clean_cue_text(&raw_body);
            if is_watermark_cue(&cleaned) {
                continue;
            }
            cues.push((start_ms, end_ms, raw_body, cleaned));
        }
    }
    cues
}

fn parse_srt_or_vtt_cues(content: &str) -> Vec<(i64, i64)> {
    parse_srt_or_vtt_cues_filtered(content, false)
}

fn parse_srt_or_vtt_cues_filtered(content: &str, spoken_only: bool) -> Vec<(i64, i64)> {
    extract_srt_or_vtt_cue_entries(content)
        .into_iter()
        .filter(|(_, _, _, cleaned)| !spoken_only || !is_non_speech_cue(cleaned))
        .map(|(s, e, _, _)| (s, e))
        .collect()
}

fn parse_ass_cues(content: &str) -> Vec<(i64, i64)> {
    parse_ass_cues_filtered(content, false)
}

fn parse_ass_cues_filtered(content: &str, spoken_only: bool) -> Vec<(i64, i64)> {
    extract_ass_cue_entries(content)
        .into_iter()
        .filter(|(_, _, _, cleaned)| !spoken_only || !is_non_speech_cue(cleaned))
        .map(|(s, e, _, _)| (s, e))
        .collect()
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

        // NF vs AMZN cross-service penalty (-900_000) -> Hard Sync Gate Conflicting
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
        assert_eq!(tier_cross_svc, SubtitleMatchTier::Conflicting);
        assert_eq!(tier_cross_svc.display_label(), "Mismatched cut");
        assert!(!tier_cross_svc.is_trusted_sync());
        assert!(!tier_cross_svc.is_compatible_sync());
        assert!(score_cross_svc < 0);

        // Even matching release group MUST stay Conflicting when cut/service conflicts (no +4M bonus)
        let amzn_same_grp = VideoReleaseFingerprint::parse("Show.S01E01.1080p.AMZN.WEB-DL-GRP");
        let (tier_conflict_grp, score_conflict_grp) = SubtitleMatchRanker::score_candidate(
            Some(&nf_video),
            &amzn_same_grp,
            false,
            "en",
            &["en".to_owned()],
            None,
            false,
        );
        assert_eq!(tier_conflict_grp, SubtitleMatchTier::Conflicting);
        assert!(score_conflict_grp < 0);

        // BluRay vs WEB-DL penalty (-1_500_000) -> Conflicting
        let bluray_sub =
            VideoReleaseFingerprint::parse("Severance.S02E01.1080p.BluRay.x264-ROVERS");
        let (tier_bluray, score_bluray) = SubtitleMatchRanker::score_candidate(
            Some(&video),
            &bluray_sub,
            false,
            "en",
            &["en".to_owned()],
            None,
            false,
        );
        assert_eq!(tier_bluray, SubtitleMatchTier::Conflicting);
        assert!(score_bluray < -900_000);

        // Edition mismatch penalty (-2_000_000) -> Conflicting
        let dc_video =
            VideoReleaseFingerprint::parse("Movie.2024.Directors.Cut.2160p.BluRay-GROUP");
        let theatrical_sub = VideoReleaseFingerprint::parse("Movie.2024.2160p.BluRay-OTHER");
        let (tier_ed, score_ed) = SubtitleMatchRanker::score_candidate(
            Some(&dc_video),
            &theatrical_sub,
            false,
            "en",
            &["en".to_owned()],
            None,
            false,
        );
        assert_eq!(tier_ed, SubtitleMatchTier::Conflicting);
        assert!(score_ed < -1_400_000);

        // 25fps vs 23.976fps penalty (-2_500_000) -> Conflicting
        let ntsc_video =
            VideoReleaseFingerprint::parse("Movie.2024.1080p.BluRay.23.976fps-GRP");
        let pal_sub = VideoReleaseFingerprint::parse("Movie.2024.1080p.BluRay.25fps-PALGRP");
        let (tier_fps, score_fps) = SubtitleMatchRanker::score_candidate(
            Some(&ntsc_video),
            &pal_sub,
            false,
            "en",
            &["en".to_owned()],
            None,
            false,
        );
        assert_eq!(tier_fps, SubtitleMatchTier::Conflicting);
        assert!(score_fps < -1_900_000);

        // Helper methods on SubtitleMatchTier
        assert!(SubtitleMatchTier::ExactHash.is_trusted_sync());
        assert!(SubtitleMatchTier::ExactRelease.is_trusted_sync());
        assert!(!SubtitleMatchTier::SameServiceAndCut.is_trusted_sync());
        assert!(SubtitleMatchTier::SameServiceAndCut.is_compatible_sync());
        assert!(SubtitleMatchTier::CompatibleCut.is_compatible_sync());
        assert!(!SubtitleMatchTier::Fallback.is_compatible_sync());
        assert!(!SubtitleMatchTier::Conflicting.is_compatible_sync());
        assert!(SubtitleMatchTier::Fallback < SubtitleMatchTier::Conflicting);
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
            ..Default::default()
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

    fn synth_speech_with_music_pcm(
        window_start_secs: f64,
        window_dur_secs: f64,
        speech_intervals_ms: &[(i64, i64)],
    ) -> Vec<i16> {
        let total_samples = (window_dur_secs * AUDIO_SAMPLE_RATE_HZ as f64).round() as usize;
        let win_start_ms = (window_start_secs * 1000.0).round() as i64;
        let mut out = Vec::with_capacity(total_samples);
        for i in 0..total_samples {
            let t_sec = i as f64 / AUDIO_SAMPLE_RATE_HZ as f64;
            let abs_ms = win_start_ms + ((i as i64 * 1000) / AUDIO_SAMPLE_RATE_HZ as i64);
            let speaking = speech_intervals_ms
                .iter()
                .any(|&(s, e)| abs_ms >= s && abs_ms <= e);
            // Continuous 180 Hz + 440 Hz background score bed across whole window
            let music = 0.22 * (2.0 * std::f64::consts::PI * 180.0 * t_sec).sin()
                + 0.14 * (2.0 * std::f64::consts::PI * 440.0 * t_sec).sin();
            let speech = if speaking {
                // Syllabic 5 Hz AM modulated vocal formants (700 Hz + 1450 Hz)
                let env = 0.55 + 0.45 * (2.0 * std::f64::consts::PI * 5.0 * t_sec).sin();
                env * (0.55 * (2.0 * std::f64::consts::PI * 700.0 * t_sec).sin()
                    + 0.35 * (2.0 * std::f64::consts::PI * 1450.0 * t_sec).sin())
            } else {
                0.0
            };
            let s = ((music + speech) * 14_000.0).clamp(-32767.0, 32767.0) as i16;
            out.push(s);
        }
        out
    }

    #[test]
    fn human_language_names_and_badges_format_cleanly() {
        assert_eq!(human_language_name("eng"), "English");
        assert_eq!(human_language_name("en"), "English");
        assert_eq!(human_language_name("pob"), "Portuguese (BR)");
        assert_eq!(human_language_name("slv"), "Slovenian");
        assert_eq!(human_language_name("jpn"), "Japanese");
        assert_eq!(short_language_badge("eng"), "EN");
        assert_eq!(short_language_badge("pob"), "PT-BR");
        assert_eq!(short_language_badge("sl"), "SL");
    }

    #[test]
    fn audio_aligner_recovers_exact_offset_through_background_music_and_rejects_decoy() {
        // True dialogue cadence in [10s..70s]
        let true_speech: Vec<(i64, i64)> = vec![
            (114_000, 116_200),
            (118_500, 121_000),
            (124_000, 125_800),
            (129_200, 132_500),
            (136_000, 138_900),
            (143_000, 145_400),
            (150_500, 153_200),
            (157_000, 159_800),
            (164_000, 166_500),
        ];
        let pcm = synth_speech_with_music_pcm(100.0, 80.0, &true_speech);
        let fp = AudioSpeechFingerprint {
            windows: vec![AudioSpeechWindow::from_pcm_8khz(100.0, &pcm)],
        };

        // Shifted subtitle cues (+4,260ms too late -> true shift needed is -4,260ms)
        let shifted_cues: Vec<(i64, i64)> =
            true_speech.iter().map(|&(s, e)| (s + 4_260, e + 4_260)).collect();
        // Wrong cut decoy cues with completely different spacing
        let decoy_cues: Vec<(i64, i64)> = (0..9)
            .map(|k| (112_000 + k * 7_300, 113_100 + k * 7_300))
            .collect();

        let good_res = SubtitleSyncAligner::align_cues_to_audio(&shifted_cues, &fp);
        let decoy_res = SubtitleSyncAligner::align_cues_to_audio(&decoy_cues, &fp);

        assert!(
            (good_res.offset_ms - (-4_260)).abs() <= 45,
            "expected offset near -4260ms, got {:?}",
            good_res
        );
        assert!(
            good_res.confidence > decoy_res.confidence + 0.15,
            "good={:?} vs decoy={:?}",
            good_res,
            decoy_res
        );
    }

    #[test]
    fn audio_aligner_recovers_pal_drift_and_offset_across_two_windows() {
        let pal_ratio = 25.0_f64 / 23.976_f64;
        let offset_true = 1_600_i64;
        let mut raw_cues = Vec::new();
        // Early cluster (~130s..180s) & Late cluster (~1450s..1500s)
        for idx in 0..8 {
            let s1 = 132_000 + idx * 5_600;
            raw_cues.push((s1, s1 + 2_100));
        }
        for idx in 0..8 {
            let s2 = 1_452_000 + idx * 5_800;
            raw_cues.push((s2, s2 + 2_200));
        }

        // Video audio is spoken at transformed timestamps `raw * pal_ratio + offset_true`
        let audio_speech: Vec<(i64, i64)> = raw_cues
            .iter()
            .map(|&(s, e)| {
                (
                    (s as f64 * pal_ratio).round() as i64 + offset_true,
                    (e as f64 * pal_ratio).round() as i64 + offset_true,
                )
            })
            .collect();

        let pcm1 = synth_speech_with_music_pcm(120.0, 75.0, &audio_speech);
        let pcm2 = synth_speech_with_music_pcm(1_495.0, 75.0, &audio_speech);
        let fp = AudioSpeechFingerprint {
            windows: vec![
                AudioSpeechWindow::from_pcm_8khz(120.0, &pcm1),
                AudioSpeechWindow::from_pcm_8khz(1_495.0, &pcm2),
            ],
        };

        let aligned = SubtitleSyncAligner::align_cues_to_audio(&raw_cues, &fp);
        assert!(
            (aligned.drift_ratio - pal_ratio).abs() < 0.005,
            "expected PAL drift {pal_ratio:.5}, got {:?}",
            aligned
        );
        assert!(
            (aligned.offset_ms - offset_true).abs() <= 60,
            "expected offset ~{offset_true}ms, got {:?}",
            aligned
        );
    }

    #[tokio::test]
    async fn opensubtitles_v3_hash_vs_plain_diff_detects_exact_hash_match() {
        let dir = tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");

        let server = tokio::spawn(async move {
            for _ in 0..4 {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let base_clone = base_url.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let (status, ctype, body) = if path.contains("videoHash=") {
                        // Hash response prepends "hash-only-1" which wasn't in plain IMDb list
                        let json = serde_json::json!({
                            "subtitles": [
                                {"id": "hash-only-1", "url": format!("{base_clone}/h.srt"), "lang": "eng"},
                                {"id": "plain-1", "url": format!("{base_clone}/p1.srt"), "lang": "eng"}
                            ]
                        });
                        ("200 OK", "application/json", json.to_string())
                    } else if path == "/osv3/subtitles/movie/tt0443536.json" {
                        let json = serde_json::json!({
                            "subtitles": [
                                {"id": "plain-1", "url": format!("{base_clone}/p1.srt"), "lang": "eng"},
                                {"id": "plain-2", "url": format!("{base_clone}/p2.srt"), "lang": "eng"},
                                {"id": "plain-3", "url": format!("{base_clone}/p3.srt"), "lang": "eng"}
                            ]
                        });
                        ("200 OK", "application/json", json.to_string())
                    } else {
                        (
                            "200 OK",
                            "text/plain",
                            "1\n00:00:05,000 --> 00:00:07,000\nHello\n".to_owned(),
                        )
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });

        let endpoints = NativeSubtitleEndpoints {
            opensubtitles_v3_base: format!("http://{addr}/osv3"),
            ..Default::default()
        };
        let client = NativeSubtitleClient::with_endpoints(dir.path(), endpoints.clone());
        let query = NativeSubtitleQuery {
            imdb_id: "tt0443536".to_owned(),
            media_type: "movie".to_owned(),
            opensubhash: Some("deadbeef12345678".to_owned()),
            video_byte_size: Some(1_500_000_000),
            preferred_langs: vec!["eng".to_owned()],
            ..Default::default()
        };
        let cands = client
            .query_opensubtitles_v3(&endpoints.opensubtitles_v3_base, &query, None)
            .await;
        server.abort();

        assert_eq!(cands.len(), 2);
        assert_eq!(cands[0].match_tier, SubtitleMatchTier::ExactHash);
        assert_eq!(cands[1].match_tier, SubtitleMatchTier::Fallback);
    }

    #[test]
    fn prunes_subtitle_cache_dir_by_file_count_and_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        for idx in 0..8usize {
            let path = dir.path().join(format!("sub-{idx:02}.srt"));
            std::fs::write(&path, format!("cue-{idx:02}-payload-0123456789")).expect("write");
        }

        let removed = prune_subtitle_cache_dir_with_limits(dir.path(), 3, 128, None);
        assert_eq!(removed, 5);

        let remaining_count = std::fs::read_dir(dir.path()).expect("read_dir").count();
        assert_eq!(remaining_count, 3);
    }

    #[test]
    fn audio_aligner_stays_accurate_with_reading_speed_tail_padding_and_24fps_pulldown_drift() {
        let pulldown_ratio = 23.976_f64 / 24.0_f64;
        let offset_true = 2_340_i64;
        let mut raw_cues = Vec::new();
        let mut spoken_speech = Vec::new();
        // Two separated clusters (~180s and ~1520s) where .srt cues hold +650ms after speech stops
        for idx in 0..8 {
            let s1 = 182_000 + idx * 5_500;
            raw_cues.push((s1, s1 + 1_950));
            spoken_speech.push((
                (s1 as f64 * pulldown_ratio).round() as i64 + offset_true,
                ((s1 + 1_300) as f64 * pulldown_ratio).round() as i64 + offset_true,
            ));
        }
        for idx in 0..8 {
            let s2 = 1_522_000 + idx * 5_600;
            raw_cues.push((s2, s2 + 1_950));
            spoken_speech.push((
                (s2 as f64 * pulldown_ratio).round() as i64 + offset_true,
                ((s2 + 1_300) as f64 * pulldown_ratio).round() as i64 + offset_true,
            ));
        }

        let pcm1 = synth_speech_with_music_pcm(175.0, 65.0, &spoken_speech);
        let pcm2 = synth_speech_with_music_pcm(1_515.0, 65.0, &spoken_speech);
        let fp = AudioSpeechFingerprint {
            windows: vec![
                AudioSpeechWindow::from_pcm_8khz(175.0, &pcm1),
                AudioSpeechWindow::from_pcm_8khz(1_515.0, &pcm2),
            ],
        };

        let aligned = SubtitleSyncAligner::align_cues_to_audio(&raw_cues, &fp);
        assert!(
            (aligned.drift_ratio - pulldown_ratio).abs() < 0.0005,
            "expected 23.976/24.0 drift {pulldown_ratio:.5}, got {:?}",
            aligned
        );
        assert!(
            (aligned.offset_ms - offset_true).abs() <= 50,
            "expected offset within 50ms of {offset_true}ms, got {:?}",
            aligned
        );
    }

    #[test]
    fn dialogue_probe_windows_skip_intro_recap_and_provide_two_separated_windows() {
        let mut cues = Vec::new();
        // Opening recap / credits noise in [15s..120s]
        for s in (15_000..120_000).step_by(6_000) {
            cues.push((s, s + 2_200));
        }
        // Main episode/movie dialogue across [180s..2500s]
        for s in (180_000..2_500_000).step_by(5_000) {
            cues.push((s, s + 2_000));
        }

        let windows = SubtitleSyncAligner::select_dialogue_probe_windows(&cues, 0.0, 0.0);
        assert_eq!(windows.len(), 2, "expected 2 windows, got {windows:?}");
        assert!(
            windows[0].0 >= 150.0,
            "expected first window to skip intro/recap (>=150s), got {:?}",
            windows
        );
        assert!(
            (windows[1].0 + windows[1].1) - windows[0].0 >= 390.0,
            "expected >=390s separation across windows, got {:?}",
            windows
        );
    }

    #[test]
    fn universal_foreign_placeholder_analyzer_handles_multiple_shows_and_movies() {
        let srt = "\
1\n00:00:05,000 --> 00:00:07,000\nWelcome to Winterfell.\n\n\
2\n00:00:10,000 --> 00:00:12,000\n[speaking Dothraki]\n\n\
3\n00:00:15,000 --> 00:00:17,000\n(in Japanese)\n\n\
4\n00:00:20,000 --> 00:00:22,000\n[chanting in Latin]\n\n\
5\n00:00:25,000 --> 00:00:27,000\n(speaking foreign language)\n\n\
6\n00:00:30,000 --> 00:00:32,000\n[door creaks]\n\n\
7\n00:00:35,000 --> 00:00:37,000\n[speaking softly]\nDon't wake the guards.\n\n\
8\n00:00:40,000 --> 00:00:42,000\n[speaking Spanish]\nMove back!\n";

        let analyzed = analyze_subtitle_content(srt);
        assert_eq!(analyzed.profile.cue_count, 8);
        assert_eq!(analyzed.profile.foreign_placeholder_count, 4);
        assert_eq!(
            analyzed.foreign_placeholder_timestamps_ms,
            vec![10_000, 15_000, 20_000, 25_000]
        );
        assert_eq!(
            analyzed.profile.foreign_status,
            ForeignTranslationStatus::MissingForeign
        );
        // Cues 1, 7 (mixed), 8 (mixed) are kept as spoken dialogue.
        assert_eq!(analyzed.spoken_cues.len(), 3);
    }

    #[tokio::test]
    async fn search_and_fetch_promotes_foreign_translating_clean_track_over_placeholder_exact_hash() {
        let dir = tempdir().unwrap();
        let cache_dir = dir.path().join("native_subtitles");

        let mut base_lines = String::new();
        let mut shifted_lines = String::new();
        for idx in 0..20 {
            let start_ms = 5_000 + (idx as i64) * 5_000;
            let end_ms = start_ms + 2_000;
            base_lines.push_str(&format!(
                "{}\n{} --> {}\nRegular line {idx}.\n\n",
                idx + 1,
                format_srt_vtt_timestamp(start_ms, false),
                format_srt_vtt_timestamp(end_ms, false),
            ));
            // Shifted +3,200ms relative to the ExactHash reference (simulating BluRay vs WEB-DL offset)
            shifted_lines.push_str(&format!(
                "{}\n{} --> {}\nRegular line {idx}.\n\n",
                idx + 1,
                format_srt_vtt_timestamp(start_ms + 3_200, false),
                format_srt_vtt_timestamp(end_ms + 3_200, false),
            ));
        }

        // Hash match candidate leaves foreign speech untranslated and has SDH cues.
        let srt_hash_placeholder = format!(
            "{base_lines}21\n00:02:00,000 --> 00:02:03,000\n[speaking Valyrian]\n\n22\n00:02:20,000 --> 00:02:23,000\n(in foreign language)\n\n23\n00:02:40,000 --> 00:02:43,000\n[door slams]\n"
        );
        // Candidate 2 leaves blank gaps at 02:00 & 02:20.
        let srt_blank_gaps = base_lines.clone();
        // Candidate 3 translates the foreign scenes cleanly at +3,200ms shifted timestamps.
        let srt_translated = format!(
            "{shifted_lines}21\n00:02:03,200 --> 00:02:06,200\nBend the knee or burn.\n\n22\n00:02:23,200 --> 00:02:26,200\nThe ships sail at dawn.\n"
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");

        let server_task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let base_clone = base_url.clone();
                let h_body = srt_hash_placeholder.clone();
                let b_body = srt_blank_gaps.clone();
                let t_body = srt_translated.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");

                    let (status, ctype, body) = if path.starts_with("/osv3/subtitles/") {
                        let json = serde_json::json!({
                            "subtitles": [
                                {
                                    "id": "hash-ph",
                                    "url": format!("{base_clone}/files/hash_ph.srt"),
                                    "lang": "eng",
                                    "hashMatch": true,
                                    "SubFileName": "Show.S01E01.1080p.WEB-DL-GRP.srt"
                                },
                                {
                                    "id": "blank",
                                    "url": format!("{base_clone}/files/blank.srt"),
                                    "lang": "eng",
                                    "SubFileName": "Show.S01E01.1080p.WEB-DL-OTHER.srt"
                                },
                                {
                                    "id": "translated",
                                    "url": format!("{base_clone}/files/translated.srt"),
                                    "lang": "eng",
                                    "SubFileName": "Show.S01E01.1080p.BluRay-GOOD.srt"
                                }
                            ]
                        });
                        ("200 OK", "application/json", json.to_string())
                    } else if path == "/files/hash_ph.srt" {
                        ("200 OK", "text/plain", h_body)
                    } else if path == "/files/blank.srt" {
                        ("200 OK", "text/plain", b_body)
                    } else if path == "/files/translated.srt" {
                        ("200 OK", "text/plain", t_body)
                    } else {
                        ("404 Not Found", "text/plain", "not found".to_owned())
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });

        let endpoints = NativeSubtitleEndpoints {
            opensubtitles_v3_base: format!("http://{addr}/osv3"),
            ..Default::default()
        };
        let client = NativeSubtitleClient::with_endpoints(&cache_dir, endpoints);
        let query = NativeSubtitleQuery {
            imdb_id: "tt0944947".to_owned(),
            media_type: "series".to_owned(),
            season: Some(1),
            episode: Some(1),
            opensubhash: Some("abcdef1234567890".to_owned()),
            video_filename: Some("Show.S01E01.1080p.WEB-DL-GRP.mkv".to_owned()),
            preferred_langs: vec!["eng".to_owned()],
            search_mode: SubtitleSearchMode::Full,
            ..Default::default()
        };

        let fetched = client.search_and_fetch(&query, None, None, 4).await;
        server_task.abort();

        assert_eq!(fetched.len(), 3);
        // Verified cross-tier alignment writes an aligned copy of BluRay-GOOD, promotes its tier to ExactHash,
        // marks it Translated, and places it at index 0 locked to the ExactHash reference's 0ms timeline.
        assert_eq!(fetched[0].release_name, "Show.S01E01.1080p.BluRay-GOOD.srt");
        assert_eq!(fetched[0].match_tier, SubtitleMatchTier::ExactHash);
        assert_eq!(
            fetched[0].content_profile.foreign_status,
            ForeignTranslationStatus::Translated
        );
        let aligned_txt = std::fs::read_to_string(&fetched[0].local_path).unwrap();
        assert!(
            aligned_txt.contains("00:02:00,000 --> 00:02:03,000"),
            "expected aligned timestamps locked to ExactHash timeline, got:\n{aligned_txt}"
        );
        // Original ExactHash timing reference remains retained at index 1.
        assert_eq!(fetched[1].release_name, "Show.S01E01.1080p.WEB-DL-GRP.srt");
        assert_eq!(fetched[1].match_tier, SubtitleMatchTier::ExactHash);
        // Blank-gap track must be detected as MissingForeign.
        let blank = fetched
            .iter()
            .find(|s| s.release_name == "Show.S01E01.1080p.WEB-DL-OTHER.srt")
            .unwrap();
        assert_eq!(
            blank.content_profile.foreign_status,
            ForeignTranslationStatus::MissingForeign
        );
    }

    #[tokio::test]
    async fn forced_oracle_detects_missing_foreign_gaps_and_forced_only_mode_ignores_all_english() {
        let dir = tempdir().unwrap();
        let cache_dir = dir.path().join("native_subtitles");

        let srt_blank = "1\n00:00:05,000 --> 00:00:08,000\nEnglish line.\n";
        let srt_translated = "1\n00:00:05,000 --> 00:00:08,000\nEnglish line.\n\n2\n00:05:00,000 --> 00:05:03,000\nForeign speech translated!\n";
        let srt_forced = "1\n00:05:00,500 --> 00:05:03,500\nForeign speech translated!\n";

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");

        let server_task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let base_clone = base_url.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");

                    let (status, ctype, body) = if path.starts_with("/osapi_none/subtitles") {
                        // All-English movie with zero foreign_parts_only tracks.
                        let json = serde_json::json!({
                            "data": [{
                                "attributes": {
                                    "language": "en",
                                    "release": "Comedy.2025.1080p.WEB-DL",
                                    "foreign_parts_only": false,
                                    "hearing_impaired": false,
                                    "files": [{"file_id": 1001, "file_name": "full.srt"}]
                                }
                            }]
                        });
                        ("200 OK", "application/json", json.to_string())
                    } else if path.starts_with("/osapi/subtitles") {
                        let json = serde_json::json!({
                            "data": [
                                {
                                    "attributes": {
                                        "language": "en",
                                        "release": "Epic.2025.WEB-DL-BLANK",
                                        "moviehash_match": true,
                                        "foreign_parts_only": false,
                                        "download_count": 900,
                                        "files": [{"file_id": 2001, "file_name": "blank.srt"}]
                                    }
                                },
                                {
                                    "attributes": {
                                        "language": "en",
                                        "release": "Epic.2025.WEB-DL-FULL",
                                        "moviehash_match": true,
                                        "foreign_parts_only": false,
                                        "download_count": 200,
                                        "files": [{"file_id": 2002, "file_name": "translated.srt"}]
                                    }
                                },
                                {
                                    "attributes": {
                                        "language": "en",
                                        "release": "Epic.2025.Forced",
                                        "moviehash_match": true,
                                        "foreign_parts_only": true,
                                        "files": [{"file_id": 2003, "file_name": "forced.srt"}]
                                    }
                                }
                            ]
                        });
                        ("200 OK", "application/json", json.to_string())
                    } else if path == "/osapi/download" {
                        let req_str = req.as_ref();
                        let file_id = if req_str.contains("2001") {
                            "blank"
                        } else if req_str.contains("2002") {
                            "translated"
                        } else {
                            "forced"
                        };
                        let json = serde_json::json!({
                            "link": format!("{base_clone}/files/{file_id}.srt")
                        });
                        ("200 OK", "application/json", json.to_string())
                    } else if path == "/files/blank.srt" {
                        ("200 OK", "text/plain", srt_blank.to_owned())
                    } else if path == "/files/translated.srt" {
                        ("200 OK", "text/plain", srt_translated.to_owned())
                    } else if path == "/files/forced.srt" {
                        ("200 OK", "text/plain", srt_forced.to_owned())
                    } else {
                        ("404 Not Found", "text/plain", "not found".to_owned())
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });

        let client = NativeSubtitleClient::with_endpoints(
            &cache_dir,
            NativeSubtitleEndpoints {
                opensubtitles_api_base: format!("http://{addr}/osapi_none"),
                ..Default::default()
            },
        );
        // 1. ForcedOnly on an all-English movie must return empty (never auto-enable full subs).
        let forced_none = client
            .search_and_fetch(
                &NativeSubtitleQuery {
                    imdb_id: "tt9999991".to_owned(),
                    media_type: "movie".to_owned(),
                    preferred_langs: vec!["eng".to_owned()],
                    search_mode: SubtitleSearchMode::ForcedOnly,
                    ..Default::default()
                },
                Some("k"),
                None,
                3,
            )
            .await;
        assert!(forced_none.is_empty());

        // 2. Full mode uses the same-timeline ExactHash Forced oracle to break ties within ExactHash.
        client.set_endpoints(NativeSubtitleEndpoints {
            opensubtitles_api_base: format!("http://{addr}/osapi"),
            ..Default::default()
        });
        let full_res = client
            .search_and_fetch(
                &NativeSubtitleQuery {
                    imdb_id: "tt9999992".to_owned(),
                    media_type: "movie".to_owned(),
                    preferred_langs: vec!["eng".to_owned()],
                    search_mode: SubtitleSearchMode::Full,
                    ..Default::default()
                },
                Some("k"),
                None,
                4,
            )
            .await;
        assert_eq!(full_res.len(), 2);
        assert_eq!(full_res[0].release_name, "Epic.2025.WEB-DL-FULL");
        assert_eq!(
            full_res[0].content_profile.foreign_status,
            ForeignTranslationStatus::Translated
        );
        assert_eq!(
            full_res[1].content_profile.foreign_status,
            ForeignTranslationStatus::MissingForeign
        );

        // 3. ForcedOnly on the movie with foreign_parts_only=true returns only the forced track.
        let forced_hit = client
            .search_and_fetch(
                &NativeSubtitleQuery {
                    imdb_id: "tt9999992".to_owned(),
                    media_type: "movie".to_owned(),
                    preferred_langs: vec!["eng".to_owned()],
                    search_mode: SubtitleSearchMode::ForcedOnly,
                    ..Default::default()
                },
                Some("k"),
                None,
                3,
            )
            .await;
        server_task.abort();
        assert_eq!(forced_hit.len(), 1);
        assert!(forced_hit[0].forced);
        assert_eq!(forced_hit[0].release_name, "Epic.2025.Forced");
    }

    #[test]
    fn extracts_release_group_after_stripping_trailing_subtitle_suffixes() {
        assert_eq!(
            extract_subtitle_release_group(
                "Game.of.Thrones.S01E01.720p.HDTV.DD5.1.x264-EbP.HI.srt"
            )
            .as_deref(),
            Some("EbP")
        );
        assert_eq!(
            extract_subtitle_release_group(
                "Game.of.Thrones.S01E01.1080p.BluRay.x264-ROVERS.eng.sdh.forced.vtt"
            )
            .as_deref(),
            Some("ROVERS")
        );
        assert_eq!(
            extract_subtitle_release_group(
                "Show.S02E03.1080p.WEB-DL.DDP5.1.H.265-NTb.en.cc.ass"
            )
            .as_deref(),
            Some("NTb")
        );
        assert_eq!(
            extract_subtitle_release_group("Movie.2024.1080p.BluRay-HI.srt"),
            None
        );
    }

    #[tokio::test]
    async fn opensubtitles_v3_camelcase_got_bluray_ranks_first_and_never_promotes_m_i() {
        use std::sync::atomic::AtomicUsize;

        let dir = tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");
        let plain_hits = Arc::new(AtomicUsize::new(0));
        let plain_hits_srv = Arc::clone(&plain_hits);

        let server = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let base_clone = base_url.clone();
                let plain_counter = Arc::clone(&plain_hits_srv);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");

                    let (status, ctype, body) = if path.contains("videoHash=") {
                        // Real-world OpenSubtitles v3 camelCase payload:
                        // #1-#3 are HDTV with m="i" (IMDb fallback, NOT hash match),
                        // #4 is BluRay with m="i", #5 is wrong episode S01E02!
                        let json = serde_json::json!({
                            "subtitles": [
                                {
                                    "id": "got-hdtv-1",
                                    "url": format!("{base_clone}/hdtv1.srt"),
                                    "lang": "eng",
                                    "subtitleFileName": "Game.of.Thrones.S01E01.720p.HDTV.DD5.1.x264-EbP.HI.srt",
                                    "movieReleaseName": "Game.of.Thrones.S01E01.720p.HDTV.DD5.1.x264-EbP",
                                    "releaseGroup": "EbP",
                                    "releaseFormat": "HDTV",
                                    "fpsMilli": 23976,
                                    "season": 1,
                                    "episode": 1,
                                    "m": "i"
                                },
                                {
                                    "id": "got-hdtv-2",
                                    "url": format!("{base_clone}/hdtv2.srt"),
                                    "lang": "eng",
                                    "subtitleFileName": "Game.of.Thrones.1x01.srt",
                                    "releaseGroup": "CTU",
                                    "releaseFormat": "HDTV",
                                    "fpsMilli": 25000,
                                    "season": 1,
                                    "episode": 1,
                                    "m": "i"
                                },
                                {
                                    "id": "got-hdtv-3",
                                    "url": format!("{base_clone}/hdtv3.srt"),
                                    "lang": "eng",
                                    "movieReleaseName": "Game.of.Thrones.S01E01.HDTV.XviD-FEVER",
                                    "releaseFormat": "HDTV",
                                    "fpsMilli": 23976,
                                    "season": 1,
                                    "episode": 1,
                                    "m": "i"
                                },
                                {
                                    "id": "got-bluray-4",
                                    "url": format!("{base_clone}/bluray4.srt"),
                                    "lang": "eng",
                                    "subtitleFileName": "Game.of.Thrones.S01E01.1080p.BluRay.x264-EbP.srt",
                                    "movieReleaseName": "Game.of.Thrones.S01E01.1080p.BluRay.x264-EbP",
                                    "releaseGroup": "EbP",
                                    "releaseFormat": "BluRay",
                                    "fpsMilli": 23976,
                                    "season": 1,
                                    "episode": 1,
                                    "m": "i"
                                },
                                {
                                    "id": "got-wrong-ep",
                                    "url": format!("{base_clone}/wrong_ep.srt"),
                                    "lang": "eng",
                                    "subtitleFileName": "Game.of.Thrones.S01E02.1080p.BluRay.x264-EbP.srt",
                                    "releaseFormat": "BluRay",
                                    "fpsMilli": 23976,
                                    "season": 1,
                                    "episode": 2,
                                    "m": "h"
                                }
                            ]
                        });
                        ("200 OK", "application/json", json.to_string())
                    } else if path.ends_with(".json") {
                        plain_counter.fetch_add(1, Ordering::Relaxed);
                        // Even if plain_url existed with different IDs, m="i" must NOT trigger a plain fetch!
                        ("200 OK", "application/json", r#"{"subtitles":[]}"#.to_owned())
                    } else {
                        (
                            "200 OK",
                            "text/plain",
                            "1\n00:00:05,000 --> 00:00:07,000\nWinter is coming.\n".to_owned(),
                        )
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });

        let endpoints = NativeSubtitleEndpoints {
            opensubtitles_v3_base: format!("http://{addr}/osv3"),
            ..Default::default()
        };
        let client = NativeSubtitleClient::with_endpoints(dir.path(), endpoints);
        let query = NativeSubtitleQuery {
            imdb_id: "tt0944947".to_owned(),
            media_type: "series".to_owned(),
            season: Some(1),
            episode: Some(1),
            opensubhash: Some("0123456789abcdef".to_owned()),
            video_byte_size: Some(4_200_000_000),
            video_filename: Some(
                "Game.of.Thrones.S01E01.1080p.BluRay.23.976fps.x264-ROVERS.mkv".to_owned(),
            ),
            preferred_langs: vec!["eng".to_owned()],
            ..Default::default()
        };

        let fetched = client.search_and_fetch(&query, None, None, 5).await;
        server.abort();

        // Plain URL must NOT have been fetched because primary response carried explicit `"m"`.
        assert_eq!(plain_hits.load(Ordering::Relaxed), 0);
        // Wrong episode S01E02 must be skipped even though it had m="h".
        assert_eq!(fetched.len(), 4);
        assert!(!fetched
            .iter()
            .any(|s| s.release_name.contains("S01E02")));

        // #1 MUST be the BluRay candidate (CompatibleCut), NEVER promoted to ExactHash since m="i"!
        assert_eq!(
            fetched[0].release_name,
            "Game.of.Thrones.S01E01.1080p.BluRay.x264-EbP.srt"
        );
        assert_eq!(fetched[0].match_tier, SubtitleMatchTier::CompatibleCut);

        // All three HDTV candidates MUST be Conflicting (never ExactHash despite m="i" and hash query).
        for hdtv in &fetched[1..] {
            assert_eq!(
                hdtv.match_tier,
                SubtitleMatchTier::Conflicting,
                "expected HDTV track {} to be Conflicting",
                hdtv.release_name
            );
        }
    }

    #[test]
    fn strict_sync_first_prevents_translated_conflicting_or_lower_tier_from_outranking_higher_tier()
    {
        let dir = tempdir().unwrap();

        let mut base_srt = String::new();
        for idx in 0..20 {
            let s = 10_000 + (idx as i64) * 5_000;
            base_srt.push_str(&format!(
                "{}\n{} --> {}\nSpoken line {idx}\n\n",
                idx + 1,
                format_srt_vtt_timestamp(s, false),
                format_srt_vtt_timestamp(s + 2_000, false),
            ));
        }

        // CompatibleCut track has a placeholder (and is NOT trusted sync, so cross-tier alignment doesn't run)
        let compat_srt = format!(
            "{base_srt}21\n00:03:00,000 --> 00:03:03,000\n[speaking Dothraki]\n"
        );
        // Conflicting HDTV track translates 03:00 cleanly AND matches onsets -> gets Translated status
        let conflict_srt = format!(
            "{base_srt}21\n00:03:00,000 --> 00:03:03,000\nThe stallion who mounts the world.\n"
        );

        let compat_path = write_cached_subtitle(dir.path(), &compat_srt, "srt").unwrap();
        let conflict_path = write_cached_subtitle(dir.path(), &conflict_srt, "srt").unwrap();

        let compat_analysis = analyze_subtitle_content(&compat_srt);
        let conflict_analysis = analyze_subtitle_content(&conflict_srt);

        let downloaded = vec![
            DownloadedSubtitleAnalysis {
                sub: FetchedNativeSubtitle {
                    provider: "OpenSubtitles v3".to_owned(),
                    lang: "en".to_owned(),
                    title: "BluRay".to_owned(),
                    release_name: "Show.S01E01.1080p.BluRay-A.srt".to_owned(),
                    match_tier: SubtitleMatchTier::CompatibleCut,
                    score: 1_100_000,
                    local_path: compat_path,
                    lookup_elapsed_ms: 0,
                    hearing_impaired: false,
                    forced: false,
                    content_profile: compat_analysis.profile.clone(),
                },
                analysis: compat_analysis,
            },
            DownloadedSubtitleAnalysis {
                sub: FetchedNativeSubtitle {
                    provider: "OpenSubtitles v3".to_owned(),
                    lang: "en".to_owned(),
                    title: "HDTV".to_owned(),
                    release_name: "Show.S01E01.720p.HDTV-B.srt".to_owned(),
                    match_tier: SubtitleMatchTier::Conflicting,
                    score: -1_000_000,
                    local_path: conflict_path,
                    lookup_elapsed_ms: 0,
                    hearing_impaired: false,
                    forced: false,
                    content_profile: conflict_analysis.profile.clone(),
                },
                analysis: conflict_analysis,
            },
        ];

        let ranked = rerank_downloaded_subtitles(
            downloaded,
            SubtitleSearchMode::Full,
            4,
            dir.path(),
        );
        assert_eq!(ranked.len(), 2);
        // CompatibleCut MUST stay #1 even though Conflicting HDTV is Translated!
        assert_eq!(ranked[0].match_tier, SubtitleMatchTier::CompatibleCut);
        assert_eq!(ranked[1].match_tier, SubtitleMatchTier::Conflicting);
        assert_eq!(
            ranked[1].content_profile.foreign_status,
            ForeignTranslationStatus::Translated
        );
    }

    #[test]
    fn shifted_unaligned_track_never_false_positives_as_translated_against_oracles() {
        let dir = tempdir().unwrap();

        // Placeholder oracle with 20 cues at 10s, 15s, ... + placeholder at 03:00
        let mut oracle_srt = String::new();
        for idx in 0..20 {
            let s = 10_000 + (idx as i64) * 5_000;
            oracle_srt.push_str(&format!(
                "{}\n{} --> {}\nOracle line {idx}\n\n",
                idx + 1,
                format_srt_vtt_timestamp(s, false),
                format_srt_vtt_timestamp(s + 2_000, false),
            ));
        }
        oracle_srt.push_str("21\n00:03:00,000 --> 00:03:03,000\n[speaking Valyrian]\n");

        // Unrelated/mismatched-cut candidate with completely different cue spacing
        // that happens to have a random English cue right at 03:00:00!
        let mut unrelated_srt = String::new();
        for idx in 0..20 {
            let s = 7_000 + (idx as i64) * 8_300;
            unrelated_srt.push_str(&format!(
                "{}\n{} --> {}\nUnrelated cut line {idx}\n\n",
                idx + 1,
                format_srt_vtt_timestamp(s, false),
                format_srt_vtt_timestamp(s + 2_000, false),
            ));
        }
        unrelated_srt.push_str("21\n00:03:00,200 --> 00:03:02,800\nRandom English line.\n");

        // Unaligned Fallback Forced track with a cue at 03:00:00
        let forced_srt = "1\n00:03:00,000 --> 00:03:03,000\nForeign forced line.\n";

        let oracle_path = write_cached_subtitle(dir.path(), &oracle_srt, "srt").unwrap();
        let unrelated_path = write_cached_subtitle(dir.path(), &unrelated_srt, "srt").unwrap();
        let forced_path = write_cached_subtitle(dir.path(), forced_srt, "srt").unwrap();

        let oracle_analysis = analyze_subtitle_content(&oracle_srt);
        let unrelated_analysis = analyze_subtitle_content(&unrelated_srt);
        let forced_analysis = analyze_subtitle_content(forced_srt);

        let downloaded = vec![
            DownloadedSubtitleAnalysis {
                sub: FetchedNativeSubtitle {
                    provider: "OSv3".to_owned(),
                    lang: "en".to_owned(),
                    title: "ExactHash".to_owned(),
                    release_name: "Show.S01E01.WEB-DL-REF.srt".to_owned(),
                    match_tier: SubtitleMatchTier::ExactHash,
                    score: 10_500_000,
                    local_path: oracle_path,
                    lookup_elapsed_ms: 0,
                    hearing_impaired: false,
                    forced: false,
                    content_profile: oracle_analysis.profile.clone(),
                },
                analysis: oracle_analysis,
            },
            DownloadedSubtitleAnalysis {
                sub: FetchedNativeSubtitle {
                    provider: "OSv3".to_owned(),
                    lang: "en".to_owned(),
                    title: "Unrelated".to_owned(),
                    release_name: "Show.S01E01.HDTV-BAD.srt".to_owned(),
                    match_tier: SubtitleMatchTier::Conflicting,
                    score: -1_000_000,
                    local_path: unrelated_path,
                    lookup_elapsed_ms: 0,
                    hearing_impaired: false,
                    forced: false,
                    content_profile: unrelated_analysis.profile.clone(),
                },
                analysis: unrelated_analysis,
            },
            DownloadedSubtitleAnalysis {
                sub: FetchedNativeSubtitle {
                    provider: "OSv3".to_owned(),
                    lang: "en".to_owned(),
                    title: "Forced".to_owned(),
                    release_name: "Show.S01E01.Forced.srt".to_owned(),
                    match_tier: SubtitleMatchTier::Fallback,
                    score: 500_000,
                    local_path: forced_path,
                    lookup_elapsed_ms: 0,
                    hearing_impaired: false,
                    forced: true,
                    content_profile: forced_analysis.profile.clone(),
                },
                analysis: forced_analysis,
            },
        ];

        let ranked = rerank_downloaded_subtitles(
            downloaded,
            SubtitleSearchMode::Full,
            4,
            dir.path(),
        );
        assert_eq!(ranked.len(), 2);
        // ExactHash stays #1 because unrelated track failed both oracle alignment AND cross-tier alignment!
        assert_eq!(ranked[0].release_name, "Show.S01E01.WEB-DL-REF.srt");
        assert_eq!(ranked[0].match_tier, SubtitleMatchTier::ExactHash);
        // Unrelated track MUST remain Unknown (never falsely marked Translated!)
        assert_eq!(ranked[1].release_name, "Show.S01E01.HDTV-BAD.srt");
        assert_eq!(
            ranked[1].content_profile.foreign_status,
            ForeignTranslationStatus::Unknown
        );
    }

    #[test]
    fn align_subtitle_file_to_reference_text_aligns_valid_and_rejects_low_confidence() {
        let dir = tempdir().unwrap();

        let mut ref_srt = String::new();
        let mut target_srt = String::new();
        let mut decoy_srt = String::new();
        for idx in 0..16 {
            let s = 12_000 + (idx as i64) * 4_500;
            ref_srt.push_str(&format!(
                "{}\n{} --> {}\nReference line {idx}\n\n",
                idx + 1,
                format_srt_vtt_timestamp(s, false),
                format_srt_vtt_timestamp(s + 2_000, false),
            ));
            target_srt.push_str(&format!(
                "{}\n{} --> {}\nTarget line {idx}\n\n",
                idx + 1,
                format_srt_vtt_timestamp(s + 2_750, false),
                format_srt_vtt_timestamp(s + 4_750, false),
            ));
            let decoy_s = 3_000 + (idx as i64) * 9_100;
            decoy_srt.push_str(&format!(
                "{}\n{} --> {}\nDecoy line {idx}\n\n",
                idx + 1,
                format_srt_vtt_timestamp(decoy_s, false),
                format_srt_vtt_timestamp(decoy_s + 1_500, false),
            ));
        }

        let target_path = dir.path().join("target.srt");
        let decoy_path = dir.path().join("decoy.srt");
        let out_path = dir.path().join("aligned.srt");
        std::fs::write(&target_path, &target_srt).unwrap();
        std::fs::write(&decoy_path, &decoy_srt).unwrap();

        let res = SubtitleSyncAligner::align_subtitle_file_to_reference_text(
            &target_path,
            &ref_srt,
            &out_path,
        )
        .expect("high-confidence alignment should succeed");
        assert!(res.matched_onsets >= 10);
        assert!(res.confidence >= 0.40);
        assert!((res.offset_ms - (-2_750)).abs() <= 5);
        let rewritten = std::fs::read_to_string(&out_path).unwrap();
        assert!(rewritten.contains("00:00:12,000 --> 00:00:14,000"));

        let decoy_out = dir.path().join("decoy_aligned.srt");
        let err = SubtitleSyncAligner::align_subtitle_file_to_reference_text(
            &decoy_path,
            &ref_srt,
            &decoy_out,
        )
        .expect_err("unrelated cut must be rejected");
        assert!(err.contains("confidence") || err.contains("Insufficient"));
        assert!(!decoy_out.exists());
    }
}
