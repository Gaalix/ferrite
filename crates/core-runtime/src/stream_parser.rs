use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use stremio_core::types::resource::{Stream, StreamSource};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Resolution {
    Unknown,
    Sd,
    Hd720p,
    Fhd1080p,
    Uhd4k,
}

impl Resolution {
    pub fn label(self) -> &'static str {
        match self {
            Self::Uhd4k => "4K",
            Self::Fhd1080p => "1080p",
            Self::Hd720p => "720p",
            Self::Sd => "SD",
            Self::Unknown => "Unknown",
        }
    }
}

impl std::fmt::Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum HdrFormat {
    Sdr,
    Hlg,
    Hdr10,
    Hdr10Plus,
    DolbyVision,
}

impl HdrFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::DolbyVision => "DV",
            Self::Hdr10Plus => "HDR10+",
            Self::Hdr10 => "HDR10",
            Self::Hlg => "HLG",
            Self::Sdr => "SDR",
        }
    }

    pub fn is_hdr(self) -> bool {
        !matches!(self, Self::Sdr)
    }
}

impl std::fmt::Display for HdrFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum StreamQuality {
    Unknown,
    Cam,
    Dvd,
    Hdtv,
    WebRip,
    WebDl,
    BluRay,
    Remux,
}

impl StreamQuality {
    pub fn label(self) -> &'static str {
        match self {
            Self::Remux => "REMUX",
            Self::BluRay => "BluRay",
            Self::WebDl => "WEB-DL",
            Self::WebRip => "WEBRip",
            Self::Hdtv => "HDTV",
            Self::Dvd => "DVD",
            Self::Cam => "CAM",
            Self::Unknown => "Unknown",
        }
    }
}

impl std::fmt::Display for StreamQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum VideoCodec {
    Unknown,
    Mpeg2,
    Xvid,
    VC1,
    AVC,
    HEVC,
    AV1,
}

impl VideoCodec {
    pub fn label(self) -> &'static str {
        match self {
            Self::AV1 => "AV1",
            Self::HEVC => "HEVC",
            Self::AVC => "AVC",
            Self::VC1 => "VC-1",
            Self::Xvid => "XviD",
            Self::Mpeg2 => "MPEG-2",
            Self::Unknown => "Unknown",
        }
    }
}

impl std::fmt::Display for VideoCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AudioCodec {
    Unknown,
    Mp3,
    Aac,
    Opus,
    Ac3,
    DdPlus,
    Flac,
    Dts,
    DtsHdMa,
    TrueHD,
    Atmos,
}

impl AudioCodec {
    pub fn label(self) -> &'static str {
        match self {
            Self::Atmos => "Atmos",
            Self::TrueHD => "TrueHD",
            Self::DtsHdMa => "DTS-HD MA",
            Self::Dts => "DTS",
            Self::Flac => "FLAC",
            Self::DdPlus => "DD+",
            Self::Ac3 => "AC3",
            Self::Opus => "Opus",
            Self::Aac => "AAC",
            Self::Mp3 => "MP3",
            Self::Unknown => "Unknown",
        }
    }
}

impl std::fmt::Display for AudioCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

fn default_allowed_stream_source() -> bool {
    true
}

pub fn is_allowed_stream_url(url: &Url) -> bool {
    let raw = url.as_str();
    if raw.is_empty()
        || raw.starts_with('-')
        || raw.trim_start().starts_with('-')
        || raw.chars().any(char::is_control)
    {
        return false;
    }

    match url.scheme().to_ascii_lowercase().as_str() {
        "http" | "https" => url
            .host_str()
            .is_some_and(|host| !host.is_empty() && !host.starts_with('-')),
        "magnet" => !url.path().starts_with('-') && url.query().is_some_and(|q| !q.is_empty()),
        _ => false,
    }
}

pub fn is_allowed_addon_stream_url(raw_url: &str) -> bool {
    if raw_url.is_empty()
        || raw_url.starts_with('-')
        || raw_url.trim_start().starts_with('-')
        || raw_url.chars().any(char::is_control)
    {
        return false;
    }

    let trimmed = raw_url.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return false;
    }

    match Url::parse(trimmed) {
        Ok(parsed) => is_allowed_stream_url(&parsed),
        Err(_) => false,
    }
}

pub fn is_allowed_stream_source(stream: &Stream) -> bool {
    match &stream.source {
        StreamSource::Url { url } => is_allowed_stream_url(url),
        StreamSource::Torrent { announce, .. } => {
            announce.iter().all(|tr| !tr.chars().any(char::is_control))
        }
        StreamSource::YouTube { yt_id } => {
            !yt_id.is_empty() && !yt_id.starts_with('-') && !yt_id.chars().any(char::is_control)
        }
        StreamSource::PlayerFrame { player_frame_url } => {
            matches!(player_frame_url.scheme(), "http" | "https")
                && is_allowed_stream_url(player_frame_url)
        }
        StreamSource::External {
            external_url,
            android_tv_url,
            tizen_url,
            webos_url,
        } => {
            external_url.as_ref().is_some_and(is_allowed_stream_url)
                && android_tv_url.as_ref().is_none_or(is_allowed_stream_url)
                && tizen_url
                    .as_deref()
                    .is_none_or(is_allowed_addon_stream_url)
                && webos_url
                    .as_deref()
                    .is_none_or(is_allowed_addon_stream_url)
        }
        StreamSource::Rar { urls, .. }
        | StreamSource::Zip { urls, .. }
        | StreamSource::Zip7 { urls, .. }
        | StreamSource::Tgz { urls, .. }
        | StreamSource::Tar { urls, .. } => {
            !urls.is_empty()
                && urls.iter().all(|entry| {
                    matches!(entry.url.scheme(), "http" | "https")
                        && is_allowed_stream_url(&entry.url)
                })
        }
        StreamSource::Nzb { url, urls, servers } => {
            let has_nzb_source = url.is_some() || !urls.is_empty();
            has_nzb_source
                && url.as_ref().is_none_or(|u| {
                    matches!(u.scheme(), "http" | "https") && is_allowed_stream_url(u)
                })
                && urls.iter().all(|u| {
                    matches!(u.scheme(), "http" | "https") && is_allowed_stream_url(u)
                })
                && servers.iter().all(|u| {
                    matches!(u.scheme(), "http" | "https" | "nntp" | "nntps")
                        && !u.as_str().starts_with('-')
                        && !u.as_str().chars().any(char::is_control)
                })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedStream {
    pub resolution: Resolution,
    pub hdr: HdrFormat,
    pub quality: StreamQuality,
    pub codec: VideoCodec,
    pub audio: AudioCodec,
    pub seeders: Option<u32>,
    pub size_bytes: Option<u64>,
    pub size_display: Option<String>,
    pub debrid_provider: Option<String>,
    pub is_cached: bool,
    pub display_title: String,
    pub release_group: Option<String>,
    pub indexer: Option<String>,
    pub addon_name: Option<String>,
    #[serde(default)]
    pub addon_id: Option<String>,
    #[serde(default)]
    pub addon_transport_url: Option<String>,
    #[serde(default = "default_allowed_stream_source")]
    pub is_allowed_source: bool,
    pub badges: Vec<String>,
}

impl ParsedStream {
    pub fn with_addon_origin(
        mut self,
        addon_id: Option<&str>,
        addon_name: Option<&str>,
        addon_transport_url: Option<&str>,
    ) -> Self {
        if let Some(id) = addon_id.map(str::trim).filter(|s| !s.is_empty()) {
            self.addon_id = Some(id.to_owned());
        }
        if let Some(name) = addon_name.map(str::trim).filter(|s| !s.is_empty()) {
            self.addon_name = Some(name.to_owned());
        }
        if let Some(url) = addon_transport_url.map(str::trim).filter(|s| !s.is_empty()) {
            self.addon_transport_url = Some(url.to_owned());
        }
        self
    }

    pub fn is_allowed(&self) -> bool {
        self.is_allowed_source
    }

    pub fn is_debrid_stream(&self) -> bool {
        self.is_cached || self.debrid_provider.is_some()
    }

    pub fn matches_addon_source(&self, query: &str) -> bool {
        let q = query.trim();
        if q.is_empty() {
            return true;
        }
        if self
            .addon_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case(q))
        {
            return true;
        }
        if self
            .addon_transport_url
            .as_deref()
            .is_some_and(|u| u.eq_ignore_ascii_case(q))
        {
            return true;
        }
        if let Some(name) = self.addon_name.as_deref() {
            if name.eq_ignore_ascii_case(q) {
                return true;
            }
            let name_lower = name.to_ascii_lowercase();
            let q_lower = q.to_ascii_lowercase();
            if let Some(rest) = name_lower.strip_prefix(&q_lower) {
                if rest.is_empty() || rest.starts_with(|c: char| !c.is_ascii_alphanumeric()) {
                    return true;
                }
            }
        }
        false
    }

    pub fn matches(&self, filter: &StreamFilter) -> bool {
        if !self.is_allowed_source {
            return false;
        }
        if filter.exclude_debrid_streams && self.is_debrid_stream() {
            return false;
        }
        if filter.cached_only && !self.is_cached {
            return false;
        }
        if filter.hdr_only && !self.hdr.is_hdr() {
            return false;
        }
        if !filter.addon_sources.is_empty()
            && !filter
                .addon_sources
                .iter()
                .any(|src| self.matches_addon_source(src))
        {
            return false;
        }
        if !filter.resolutions.is_empty() && !filter.resolutions.contains(&self.resolution) {
            return false;
        }
        if !filter.codecs.is_empty() && !filter.codecs.contains(&self.codec) {
            return false;
        }
        if let Some(min_seeders) = filter.min_seeders {
            if !self.is_cached && self.seeders.unwrap_or(0) < min_seeders {
                return false;
            }
        }
        if let Some(max_bytes) = filter.max_size_bytes {
            if let Some(bytes) = self.size_bytes {
                if bytes > max_bytes {
                    return false;
                }
            }
        }
        true
    }

    pub fn debrid_provider_full_name(&self) -> Option<&'static str> {
        match self.debrid_provider.as_deref()? {
            "RD" => Some("Real-Debrid"),
            "AD" => Some("AllDebrid"),
            "TB" => Some("TorBox"),
            "PM" => Some("Premiumize"),
            "DL" => Some("Debrid-Link"),
            "ED" => Some("EasyDebrid"),
            "OC" => Some("Offcloud"),
            "PK" => Some("PikPak"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamFilter {
    #[serde(default)]
    pub addon_sources: Vec<String>,
    pub resolutions: Vec<Resolution>,
    pub codecs: Vec<VideoCodec>,
    pub cached_only: bool,
    #[serde(default)]
    pub exclude_debrid_streams: bool,
    pub hdr_only: bool,
    pub min_seeders: Option<u32>,
    pub max_size_bytes: Option<u64>,
}

impl StreamFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_unfiltered(&self) -> bool {
        self.addon_sources.is_empty()
            && self.resolutions.is_empty()
            && self.codecs.is_empty()
            && !self.cached_only
            && !self.hdr_only
            && self.min_seeders.is_none()
            && self.max_size_bytes.is_none()
    }

    pub fn with_addon_source(mut self, addon_source: impl Into<String>) -> Self {
        let s = addon_source.into();
        let trimmed = s.trim();
        if !trimmed.is_empty()
            && !self
                .addon_sources
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(trimmed))
        {
            self.addon_sources.push(trimmed.to_owned());
        }
        self
    }

    pub fn with_addon_sources<I, S>(mut self, sources: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for src in sources {
            self = self.with_addon_source(src);
        }
        self
    }

    pub fn only_cached(mut self) -> Self {
        self.cached_only = true;
        self
    }

    pub fn without_debrid(mut self) -> Self {
        self.exclude_debrid_streams = true;
        self
    }

    pub fn with_exclude_debrid(mut self, exclude: bool) -> Self {
        self.exclude_debrid_streams = exclude;
        self
    }

    pub fn only_hdr(mut self) -> Self {
        self.hdr_only = true;
        self
    }

    pub fn with_resolution(mut self, resolution: Resolution) -> Self {
        if !self.resolutions.contains(&resolution) {
            self.resolutions.push(resolution);
        }
        self
    }

    pub fn with_codec(mut self, codec: VideoCodec) -> Self {
        if !self.codecs.contains(&codec) {
            self.codecs.push(codec);
        }
        self
    }

    pub fn with_min_seeders(mut self, min_seeders: u32) -> Self {
        self.min_seeders = Some(min_seeders);
        self
    }

    pub fn with_max_size_bytes(mut self, max_size_bytes: u64) -> Self {
        self.max_size_bytes = Some(max_size_bytes);
        self
    }
}

static RE_RES_4K: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])(?:2160[pi]|4k|uhd)(?:$|[^a-z0-9])").unwrap());
static RE_RES_1080P: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])(?:1080[pi]|fhd)(?:$|[^a-z0-9])").unwrap());
static RE_RES_720P: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])720[pi](?:$|[^a-z0-9])").unwrap());
static RE_RES_SD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:576[pi]|480[pi]|360p|240p|sd|dvdrip|dvdscr)(?:$|[^a-z0-9])")
        .unwrap()
});

static RE_HDR_DV: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:dolby[\s._-]*vision|dovi|dv)(?:$|[^a-z0-9])").unwrap()
});
static RE_HDR10_PLUS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])hdr10(?:\+|plus)(?:$|[^a-z0-9])").unwrap());
static RE_HDR10: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])hdr(?:10)?(?:$|[^a-z0-9+])").unwrap());
static RE_HLG: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])hlg(?:$|[^a-z0-9])").unwrap());

static RE_QUAL_REMUX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])(?:bd|uhd)?remux(?:$|[^a-z0-9])").unwrap());
static RE_QUAL_BLURAY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:blu[\s._-]?ray|bdrip|brrip|bdmux)(?:$|[^a-z0-9])").unwrap()
});
static RE_QUAL_WEBDL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:web[\s._-]?dl|webdl)(?:$|[^a-z0-9])").unwrap()
});
static RE_QUAL_WEBRIP: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:web[\s._-]?rip|webmux)(?:$|[^a-z0-9])").unwrap()
});
static RE_QUAL_WEB_STANDALONE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])web(?:$|[^a-z0-9-])").unwrap());
static RE_QUAL_HDTV: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:hdtv|pdtv|dsr|tvrip)(?:$|[^a-z0-9])").unwrap()
});
static RE_QUAL_CAM: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:hdcam|camrip|cam|hdts|telesync|telecine|dvdscr|screener|workprint)(?:$|[^a-z0-9])").unwrap()
});
static RE_QUAL_DVD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:dvdrip|dvd9|dvd5|dvd)(?:$|[^a-z0-9])").unwrap()
});

static RE_CODEC_AV1: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])av1(?:$|[^a-z0-9])").unwrap());
static RE_CODEC_HEVC: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:hevc|x265|h[\s._-]?265)(?:$|[^a-z0-9])").unwrap()
});
static RE_CODEC_AVC: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:avc|x264|h[\s._-]?264)(?:$|[^a-z0-9])").unwrap()
});
static RE_CODEC_VC1: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])vc[\s._-]?1(?:$|[^a-z0-9])").unwrap());
static RE_CODEC_XVID: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])(?:xvid|divx)(?:$|[^a-z0-9])").unwrap());
static RE_CODEC_MPEG2: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])mpeg[\s._-]?2(?:$|[^a-z0-9])").unwrap());

static RE_AUDIO_ATMOS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])atmos(?:$|[^a-z0-9])").unwrap());
static RE_AUDIO_TRUEHD: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])true[\s._-]?hd(?:$|[^a-z0-9]|\d)").unwrap());
static RE_AUDIO_DTS_HD_MA: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])dts[\s._-]?(?:hd(?:[\s._-]?ma)?|x)(?:$|[^a-z0-9]|\d)").unwrap()
});
static RE_AUDIO_DTS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])dts(?:$|[^a-z0-9]|\d)").unwrap());
static RE_AUDIO_DDP: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(?:^|[^a-z0-9])(?:ddp|dd\+|e[\s._-]?ac[\s._-]?3|dolby[\s._-]*digital[\s._-]*plus)(?:$|[^a-z0-9]|\d)",
    )
    .unwrap()
});
static RE_AUDIO_AC3: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(?:^|[^a-z0-9])(?:ac[\s._-]?3|dd[\s._-]*[257]\.[01]|dolby[\s._-]*digital)(?:$|[^a-z0-9]|\d)",
    )
    .unwrap()
});
static RE_AUDIO_FLAC: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])flac(?:$|[^a-z0-9]|\d)").unwrap());
static RE_AUDIO_OPUS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])opus(?:$|[^a-z0-9]|\d)").unwrap());
static RE_AUDIO_AAC: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])aac(?:$|[^a-z0-9]|\d)").unwrap());
static RE_AUDIO_MP3: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])mp3(?:$|[^a-z0-9]|\d)").unwrap());

static RE_SEEDERS_EMOJI: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:👤|👥|🌱)\s*([\d,]+)").unwrap());
static RE_SEEDERS_TEXT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|[^a-z])(?:seeders|seeds)\s*[:=]?\s*([\d,]+)").unwrap());

static RE_SIZE_EMOJI: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)💾\s*([\d]+(?:[.,][\d]+)?)\s*(TB|TiB|GB|GiB|MB|MiB|KB|KiB)").unwrap()
});
static RE_SIZE_GENERAL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[^\w.])([\d]+(?:\.[\d]+)?)\s*(TB|TiB|GB|GiB|MB|MiB)(?:$|[^a-zA-Z])")
        .unwrap()
});

static RE_DEBRID_BRACKET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\[\s*(RD|AD|TB|PM|DL|ED|OC|PK)\s*(\+|⚡|⚡️|download|⬇️|⬇|⏳)?\s*\]").unwrap()
});
static RE_DEBRID_INLINE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(?:^|[\s|/•\-])(Real-?Debrid|AllDebrid|TorBox|Premiumize|Debrid-?Link|EasyDebrid|Offcloud|RD|AD|TB|PM|DL|ED|OC)\s*(\+|⚡|⚡️|⏳|⬇️|⬇|download)?(?:$|[\s|/•\-\n])",
    )
    .unwrap()
});

static RE_INDEXER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:⚙️|⚙|🔎)\s*([^\n|💾👤🔊🎥🌐]+)").unwrap());
static RE_RELEASE_GROUP: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"-([A-Za-z0-9]+)(?:\.[a-zA-Z0-9]{2,4})?$").unwrap()
});
static RE_VIDEO_EXT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\.(mkv|mp4|avi|mov|wmv|m4v|ts|webm)$").unwrap());
static RE_LEADING_BRACKET_PREFIX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*\[[^\]]+\]\s*").unwrap());

pub fn parse_stream(stream: &Stream) -> ParsedStream {
    parse_stream_with_addon(stream, None)
}

pub fn parse_stream_with_addon(stream: &Stream, addon_name_override: Option<&str>) -> ParsedStream {
    let name_raw = stream.name.as_deref().unwrap_or("");
    let desc_raw = stream.description.as_deref().unwrap_or("");
    let filename_hint = stream.behavior_hints.filename.as_deref().unwrap_or("");
    let binge_group = stream.behavior_hints.binge_group.as_deref().unwrap_or("");

    let combined = format!("{name_raw}\n{filename_hint}\n{desc_raw}\n{binge_group}");

    let resolution = detect_resolution(&combined);
    let (hdr, has_hdr10_plus_secondary, has_hdr10_secondary) = detect_hdr(&combined);
    let quality = detect_quality(&combined);
    let codec = detect_codec(&combined);
    let audio = detect_audio(&combined);
    let seeders = detect_seeders(desc_raw).or_else(|| detect_seeders(name_raw));
    let (size_bytes, size_display) =
        detect_size(stream.behavior_hints.video_size, desc_raw, name_raw);
    let (debrid_provider, is_cached) = detect_debrid(name_raw, desc_raw);
    let indexer = detect_indexer(desc_raw);
    let addon_name = addon_name_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| extract_addon_name(name_raw));
    let display_title = extract_display_title(filename_hint, desc_raw, name_raw);
    let release_group = extract_release_group(if !filename_hint.is_empty() {
        filename_hint
    } else {
        &display_title
    });
    let is_allowed_source = is_allowed_stream_source(stream);

    let badges = build_badges(
        resolution,
        hdr,
        has_hdr10_plus_secondary,
        has_hdr10_secondary,
        quality,
        codec,
        audio,
        debrid_provider.as_deref(),
        is_cached,
    );

    ParsedStream {
        resolution,
        hdr,
        quality,
        codec,
        audio,
        seeders,
        size_bytes,
        size_display,
        debrid_provider,
        is_cached,
        display_title,
        release_group,
        indexer,
        addon_name,
        addon_id: None,
        addon_transport_url: None,
        is_allowed_source,
        badges,
    }
}

pub fn parse_streams(streams: &[Stream]) -> Vec<ParsedStream> {
    streams.iter().map(parse_stream).collect()
}

pub fn filter_and_rank_streams<'a>(
    streams: &'a [Stream],
    filter: &StreamFilter,
) -> Vec<(&'a Stream, ParsedStream)> {
    let mut matched: Vec<(&'a Stream, ParsedStream)> = streams
        .iter()
        .map(|s| (s, parse_stream(s)))
        .filter(|(_, parsed)| parsed.matches(filter))
        .collect();

    matched.sort_by(|(_, a), (_, b)| {
        b.is_cached
            .cmp(&a.is_cached)
            .then_with(|| b.resolution.cmp(&a.resolution))
            .then_with(|| b.quality.cmp(&a.quality))
            .then_with(|| b.hdr.cmp(&a.hdr))
            .then_with(|| b.audio.cmp(&a.audio))
            .then_with(|| b.seeders.unwrap_or(0).cmp(&a.seeders.unwrap_or(0)))
            .then_with(|| b.size_bytes.unwrap_or(0).cmp(&a.size_bytes.unwrap_or(0)))
    });

    matched
}

fn detect_resolution(text: &str) -> Resolution {
    if RE_RES_4K.is_match(text) {
        Resolution::Uhd4k
    } else if RE_RES_1080P.is_match(text) {
        Resolution::Fhd1080p
    } else if RE_RES_720P.is_match(text) {
        Resolution::Hd720p
    } else if RE_RES_SD.is_match(text) {
        Resolution::Sd
    } else {
        Resolution::Unknown
    }
}

fn detect_hdr(text: &str) -> (HdrFormat, bool, bool) {
    let has_dv = RE_HDR_DV.is_match(text);
    let has_hdr10_plus = RE_HDR10_PLUS.is_match(text);
    let has_hdr10 = RE_HDR10.is_match(text);
    let has_hlg = RE_HLG.is_match(text);

    if has_dv {
        (
            HdrFormat::DolbyVision,
            has_hdr10_plus,
            !has_hdr10_plus && has_hdr10,
        )
    } else if has_hdr10_plus {
        (HdrFormat::Hdr10Plus, false, false)
    } else if has_hdr10 {
        (HdrFormat::Hdr10, false, false)
    } else if has_hlg {
        (HdrFormat::Hlg, false, false)
    } else {
        (HdrFormat::Sdr, false, false)
    }
}

fn detect_quality(text: &str) -> StreamQuality {
    if RE_QUAL_REMUX.is_match(text) {
        StreamQuality::Remux
    } else if RE_QUAL_BLURAY.is_match(text) {
        StreamQuality::BluRay
    } else if RE_QUAL_WEBDL.is_match(text) {
        StreamQuality::WebDl
    } else if RE_QUAL_WEBRIP.is_match(text) {
        StreamQuality::WebRip
    } else if RE_QUAL_HDTV.is_match(text) {
        StreamQuality::Hdtv
    } else if RE_QUAL_CAM.is_match(text) {
        StreamQuality::Cam
    } else if RE_QUAL_DVD.is_match(text) {
        StreamQuality::Dvd
    } else if RE_QUAL_WEB_STANDALONE.is_match(text) {
        StreamQuality::WebDl
    } else {
        StreamQuality::Unknown
    }
}

fn detect_codec(text: &str) -> VideoCodec {
    if RE_CODEC_AV1.is_match(text) {
        VideoCodec::AV1
    } else if RE_CODEC_HEVC.is_match(text) {
        VideoCodec::HEVC
    } else if RE_CODEC_AVC.is_match(text) {
        VideoCodec::AVC
    } else if RE_CODEC_VC1.is_match(text) {
        VideoCodec::VC1
    } else if RE_CODEC_XVID.is_match(text) {
        VideoCodec::Xvid
    } else if RE_CODEC_MPEG2.is_match(text) {
        VideoCodec::Mpeg2
    } else {
        VideoCodec::Unknown
    }
}

fn detect_audio(text: &str) -> AudioCodec {
    if RE_AUDIO_ATMOS.is_match(text) {
        AudioCodec::Atmos
    } else if RE_AUDIO_TRUEHD.is_match(text) {
        AudioCodec::TrueHD
    } else if RE_AUDIO_DTS_HD_MA.is_match(text) {
        AudioCodec::DtsHdMa
    } else if RE_AUDIO_DTS.is_match(text) {
        AudioCodec::Dts
    } else if RE_AUDIO_DDP.is_match(text) {
        AudioCodec::DdPlus
    } else if RE_AUDIO_AC3.is_match(text) {
        AudioCodec::Ac3
    } else if RE_AUDIO_FLAC.is_match(text) {
        AudioCodec::Flac
    } else if RE_AUDIO_OPUS.is_match(text) {
        AudioCodec::Opus
    } else if RE_AUDIO_AAC.is_match(text) {
        AudioCodec::Aac
    } else if RE_AUDIO_MP3.is_match(text) {
        AudioCodec::Mp3
    } else {
        AudioCodec::Unknown
    }
}

fn detect_seeders(text: &str) -> Option<u32> {
    let raw = RE_SEEDERS_EMOJI
        .captures(text)
        .or_else(|| RE_SEEDERS_TEXT.captures(text))
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str())?;

    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse::<u32>().ok()
}

fn detect_size(
    hint_bytes: Option<u64>,
    description: &str,
    name: &str,
) -> (Option<u64>, Option<String>) {
    let text_parsed = parse_size_from_text(description).or_else(|| parse_size_from_text(name));

    if let Some(bytes) = hint_bytes.filter(|&b| b > 0) {
        let display = text_parsed
            .map(|(_, disp)| disp)
            .unwrap_or_else(|| format_bytes(bytes));
        return (Some(bytes), Some(display));
    }

    match text_parsed {
        Some((bytes, disp)) => (Some(bytes), Some(disp)),
        None => (None, None),
    }
}

fn parse_size_from_text(text: &str) -> Option<(u64, String)> {
    let caps = RE_SIZE_EMOJI
        .captures(text)
        .or_else(|| RE_SIZE_GENERAL.captures(text))?;
    let num_str = caps.get(1)?.as_str().replace(',', ".");
    let unit_raw = caps.get(2)?.as_str();
    let value = num_str.parse::<f64>().ok()?;
    if !value.is_finite() || value <= 0.0 {
        return None;
    }

    let (multiplier, canonical_unit) = match unit_raw.to_ascii_uppercase().as_str() {
        "TB" | "TIB" => ((1_u64 << 40) as f64, "TB"),
        "GB" | "GIB" => ((1_u64 << 30) as f64, "GB"),
        "MB" | "MIB" => ((1_u64 << 20) as f64, "MB"),
        "KB" | "KIB" => ((1_u64 << 10) as f64, "KB"),
        _ => return None,
    };

    let bytes = (value * multiplier).round() as u64;
    let display = format!("{num_str} {canonical_unit}");
    Some((bytes, display))
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;

    let b = bytes as f64;
    let (val, unit) = if b >= TB {
        (b / TB, "TB")
    } else if b >= GB {
        (b / GB, "GB")
    } else if b >= MB {
        (b / MB, "MB")
    } else if b >= KB {
        (b / KB, "KB")
    } else {
        return format!("{bytes} B");
    };

    let formatted = format!("{val:.2}");
    let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
    format!("{trimmed} {unit}")
}

fn normalize_debrid_code(raw: &str) -> String {
    let upper = raw.to_ascii_uppercase();
    match upper.as_str() {
        "RD" | "REAL-DEBRID" | "REALDEBRID" => "RD".to_owned(),
        "AD" | "ALLDEBRID" => "AD".to_owned(),
        "TB" | "TORBOX" => "TB".to_owned(),
        "PM" | "PREMIUMIZE" => "PM".to_owned(),
        "DL" | "DEBRID-LINK" | "DEBRIDLINK" => "DL".to_owned(),
        "ED" | "EASYDEBRID" => "ED".to_owned(),
        "OC" | "OFFCLOUD" => "OC".to_owned(),
        "PK" | "PIKPAK" => "PK".to_owned(),
        _ => upper,
    }
}

fn detect_debrid(name: &str, description: &str) -> (Option<String>, bool) {
    let combined = format!("{name}\n{description}");
    let has_lightning = combined.contains('⚡');
    let has_explicit_uncached = combined.contains('⏳')
        || combined.contains('⬇')
        || combined.to_ascii_lowercase().contains("download]");

    if let Some(caps) = RE_DEBRID_BRACKET.captures(&combined) {
        let provider = normalize_debrid_code(&caps[1]);
        let marker = caps.get(2).map(|m| m.as_str().to_ascii_lowercase());
        let cached = match marker.as_deref() {
            Some("+") => true,
            Some(m) if m.contains('⚡') => true,
            Some("download") => false,
            Some(m) if m.contains('⬇') || m.contains('⏳') => false,
            _ => has_lightning && !has_explicit_uncached,
        };
        return (Some(provider), cached);
    }

    if let Some(caps) = RE_DEBRID_INLINE.captures(name) {
        let provider = normalize_debrid_code(&caps[1]);
        let marker = caps.get(2).map(|m| m.as_str().to_ascii_lowercase());
        let cached = match marker.as_deref() {
            Some("+") => true,
            Some(m) if m.contains('⚡') => true,
            Some("download") => false,
            Some(m) if m.contains('⬇') || m.contains('⏳') => false,
            _ => has_lightning && !has_explicit_uncached,
        };
        return (Some(provider), cached);
    }

    (None, has_lightning && !has_explicit_uncached)
}

fn detect_indexer(description: &str) -> Option<String> {
    let raw = RE_INDEXER.captures(description)?.get(1)?.as_str().trim();
    if raw.is_empty() {
        None
    } else {
        Some(raw.to_owned())
    }
}

fn extract_addon_name(name: &str) -> Option<String> {
    let first_line = name.lines().next()?.trim();
    if first_line.is_empty() {
        return None;
    }
    let without_bracket = RE_LEADING_BRACKET_PREFIX.replace(first_line, "");
    let cleaned = without_bracket
        .split(['|', '\n'])
        .next()
        .unwrap_or(&without_bracket)
        .replace(['⚡', '\u{fe0f}'], "");
    let cleaned = RE_RES_4K.replace(&cleaned, " ");
    let cleaned = RE_RES_1080P.replace(&cleaned, " ");
    let cleaned = RE_RES_720P.replace(&cleaned, " ");
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn clean_title_candidate(raw: &str) -> String {
    let stripped = raw
        .trim()
        .trim_start_matches(['📄', '📂', '🎬', '📁', '🎞', '•', '-', ' '])
        .trim_start_matches('\u{fe0f}')
        .trim();
    RE_VIDEO_EXT.replace(stripped, "").trim().to_owned()
}

fn is_metadata_clutter_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return true;
    }
    let clutter_prefixes = [
        '👤', '👥', '🌱', '💾', '⚙', '🎥', '📺', '🔊', '🌐', '🔎', '🇬', '🇺', '🇫', '🇪', '🇩', '🇮',
        '🇯', '🇰', '🇧', '🇷', '🇨', '🇲', '🇵',
    ];
    if trimmed.starts_with(clutter_prefixes) {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("seeds:")
        || lower.starts_with("seeders:")
        || lower.starts_with("multi subs")
        || lower.starts_with("subtitles:")
        || lower.starts_with("size:")
}

fn extract_display_title(filename_hint: &str, description: &str, name: &str) -> String {
    if !filename_hint.trim().is_empty() {
        let cleaned = clean_title_candidate(filename_hint);
        if !cleaned.is_empty() {
            return cleaned;
        }
    }

    let content_lines: Vec<&str> = description
        .lines()
        .map(str::trim)
        .filter(|line| !is_metadata_clutter_line(line))
        .collect();

    if let Some(video_line) = content_lines
        .iter()
        .copied()
        .find(|line| RE_VIDEO_EXT.is_match(line))
    {
        let cleaned = clean_title_candidate(video_line);
        if !cleaned.is_empty() {
            return cleaned;
        }
    }

    if let Some(first_line) = content_lines.first() {
        let cleaned = clean_title_candidate(first_line);
        if !cleaned.is_empty() {
            return cleaned;
        }
    }

    let fallback = RE_LEADING_BRACKET_PREFIX
        .replace(name.lines().next().unwrap_or(name), "")
        .replace(['⚡', '\u{fe0f}'], "")
        .trim()
        .to_owned();

    if fallback.is_empty() {
        "Direct Stream".to_owned()
    } else {
        fallback
    }
}

fn extract_release_group(title_or_filename: &str) -> Option<String> {
    let group = RE_RELEASE_GROUP
        .captures(title_or_filename.trim())?
        .get(1)?
        .as_str();
    let lower = group.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "dl" | "rip" | "hd" | "ma" | "264" | "265" | "1" | "2" | "3" | "x264" | "x265" | "10bit"
    ) {
        return None;
    }
    Some(group.to_owned())
}

#[allow(clippy::too_many_arguments)]
fn build_badges(
    resolution: Resolution,
    hdr: HdrFormat,
    has_hdr10_plus_secondary: bool,
    has_hdr10_secondary: bool,
    quality: StreamQuality,
    codec: VideoCodec,
    audio: AudioCodec,
    debrid_provider: Option<&str>,
    is_cached: bool,
) -> Vec<String> {
    let mut badges = Vec::with_capacity(8);

    match (debrid_provider, is_cached) {
        (Some(provider), true) => badges.push(format!("{provider}+")),
        (Some(provider), false) => badges.push(provider.to_owned()),
        (None, true) => badges.push("Cached ⚡".to_owned()),
        (None, false) => {}
    }

    if resolution != Resolution::Unknown {
        badges.push(resolution.label().to_owned());
    }

    if hdr != HdrFormat::Sdr {
        badges.push(hdr.label().to_owned());
        if has_hdr10_plus_secondary {
            badges.push(HdrFormat::Hdr10Plus.label().to_owned());
        } else if has_hdr10_secondary {
            badges.push(HdrFormat::Hdr10.label().to_owned());
        }
    }

    if quality != StreamQuality::Unknown {
        badges.push(quality.label().to_owned());
    }

    if codec != VideoCodec::Unknown {
        badges.push(codec.label().to_owned());
    }

    if audio != AudioCodec::Unknown {
        badges.push(audio.label().to_owned());
    }

    badges
}

#[cfg(test)]
mod tests {
    use super::*;
    use stremio_core::types::resource::StreamBehaviorHints;

    fn make_stream_with_url(
        url: &str,
        name: Option<&str>,
        description: Option<&str>,
        filename: Option<&str>,
        video_size: Option<u64>,
        binge_group: Option<&str>,
    ) -> Stream {
        Stream {
            source: StreamSource::Url {
                url: url.parse().unwrap(),
            },
            name: name.map(ToOwned::to_owned),
            description: description.map(ToOwned::to_owned),
            thumbnail: None,
            subtitles: vec![],
            behavior_hints: StreamBehaviorHints {
                filename: filename.map(ToOwned::to_owned),
                video_size,
                binge_group: binge_group.map(ToOwned::to_owned),
                ..Default::default()
            },
        }
    }

    fn make_stream(
        name: Option<&str>,
        description: Option<&str>,
        filename: Option<&str>,
        video_size: Option<u64>,
        binge_group: Option<&str>,
    ) -> Stream {
        make_stream_with_url(
            "https://example.com/video.mkv",
            name,
            description,
            filename,
            video_size,
            binge_group,
        )
    }

    #[test]
    fn parses_torrentio_rd_cached_4k_dv_hdr10plus_remux_atmos() {
        let stream = make_stream(
            Some("[RD+] Torrentio\n4k DV | HDR10+"),
            Some(
                "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR10Plus.TrueHD.7.1.Atmos.HEVC-FraMeSToR\n\
                 👤 342 💾 68.42 GB ⚙️ RARBG\n\
                 Multi Subs /🇬🇧 /🇫🇷",
            ),
            Some(
                "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR10Plus.TrueHD.7.1.Atmos.HEVC-FraMeSToR.mkv",
            ),
            Some(73_465_413_632),
            Some("torrentio|4k|REMUX|hevc|DV|HDR10+"),
        );

        let parsed = parse_stream(&stream);
        assert!(parsed.is_allowed());
        assert_eq!(parsed.resolution, Resolution::Uhd4k);
        assert_eq!(parsed.hdr, HdrFormat::DolbyVision);
        assert_eq!(parsed.quality, StreamQuality::Remux);
        assert_eq!(parsed.codec, VideoCodec::HEVC);
        assert_eq!(parsed.audio, AudioCodec::Atmos);
        assert_eq!(parsed.seeders, Some(342));
        assert_eq!(parsed.size_bytes, Some(73_465_413_632));
        assert_eq!(parsed.size_display.as_deref(), Some("68.42 GB"));
        assert_eq!(parsed.debrid_provider.as_deref(), Some("RD"));
        assert_eq!(parsed.debrid_provider_full_name(), Some("Real-Debrid"));
        assert!(parsed.is_cached);
        assert_eq!(parsed.addon_name.as_deref(), Some("Torrentio"));
        assert_eq!(parsed.indexer.as_deref(), Some("RARBG"));
        assert_eq!(parsed.release_group.as_deref(), Some("FraMeSToR"));
        assert_eq!(
            parsed.display_title,
            "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR10Plus.TrueHD.7.1.Atmos.HEVC-FraMeSToR"
        );
        assert_eq!(
            parsed.badges,
            vec!["RD+", "4K", "DV", "HDR10+", "REMUX", "HEVC", "Atmos"]
        );
    }

    #[test]
    fn parses_comet_alldebrid_lightning_bluray_dts_hd_ma() {
        let stream = make_stream(
            Some("[AD⚡] Comet 2160p"),
            Some(
                "📄 Oppenheimer.2023.2160p.BluRay.x265.10bit.HDR10.DTS-HD.MA.5.1-SWTYBLZ.mkv\n\
                 💾 24.5 GB | 👤 87 | 🔎 BitSearch\n\
                 🎥 HEVC | 📺 BluRay | 🔊 DTS-HD MA",
            ),
            None,
            None,
            None,
        );

        let parsed = parse_stream(&stream);
        assert_eq!(parsed.resolution, Resolution::Uhd4k);
        assert_eq!(parsed.hdr, HdrFormat::Hdr10);
        assert_eq!(parsed.quality, StreamQuality::BluRay);
        assert_eq!(parsed.codec, VideoCodec::HEVC);
        assert_eq!(parsed.audio, AudioCodec::DtsHdMa);
        assert_eq!(parsed.seeders, Some(87));
        assert_eq!(parsed.size_display.as_deref(), Some("24.5 GB"));
        assert!(parsed.size_bytes.unwrap() > 26_000_000_000);
        assert_eq!(parsed.debrid_provider.as_deref(), Some("AD"));
        assert!(parsed.is_cached);
        assert_eq!(parsed.addon_name.as_deref(), Some("Comet"));
        assert_eq!(parsed.indexer.as_deref(), Some("BitSearch"));
        assert_eq!(parsed.release_group.as_deref(), Some("SWTYBLZ"));
        assert_eq!(
            parsed.display_title,
            "Oppenheimer.2023.2160p.BluRay.x265.10bit.HDR10.DTS-HD.MA.5.1-SWTYBLZ"
        );
    }

    #[test]
    fn parses_mediafusion_season_pack_episode_webdl_ddp_avc() {
        let stream = make_stream(
            Some("MediaFusion | TB ⚡\n1080p"),
            Some(
                "📂 Severance.S02.1080p.ATVP.WEB-DL.DDP5.1.H.264-NTb\n\
                 📄 Severance.S02E01.Hello.Ms.Cobel.1080p.ATVP.WEB-DL.DDP5.1.H.264-NTb.mkv\n\
                 💾 3.45 GB 👤 1,240\n\
                 🌐 English",
            ),
            None,
            None,
            None,
        );

        let parsed = parse_stream(&stream);
        assert_eq!(parsed.resolution, Resolution::Fhd1080p);
        assert_eq!(parsed.hdr, HdrFormat::Sdr);
        assert_eq!(parsed.quality, StreamQuality::WebDl);
        assert_eq!(parsed.codec, VideoCodec::AVC);
        assert_eq!(parsed.audio, AudioCodec::DdPlus);
        assert_eq!(parsed.seeders, Some(1240));
        assert_eq!(parsed.size_display.as_deref(), Some("3.45 GB"));
        assert_eq!(parsed.debrid_provider.as_deref(), Some("TB"));
        assert!(parsed.is_cached);
        assert_eq!(parsed.addon_name.as_deref(), Some("MediaFusion"));
        assert_eq!(parsed.release_group.as_deref(), Some("NTb"));
        assert_eq!(
            parsed.display_title,
            "Severance.S02E01.Hello.Ms.Cobel.1080p.ATVP.WEB-DL.DDP5.1.H.264-NTb"
        );
    }

    #[test]
    fn parses_uncached_debrid_and_p2p_streams_accurately() {
        let rd_uncached = make_stream(
            Some("[RD download] Torrentio\n1080p"),
            Some("The.Substance.2024.1080p.WEBRip.AV1.AAC2.0-GROUP\n👤 54 💾 2.1 GB"),
            None,
            None,
            None,
        );
        let parsed_uncached = parse_stream(&rd_uncached);
        assert_eq!(parsed_uncached.debrid_provider.as_deref(), Some("RD"));
        assert!(!parsed_uncached.is_cached);
        assert_eq!(parsed_uncached.quality, StreamQuality::WebRip);
        assert_eq!(parsed_uncached.codec, VideoCodec::AV1);
        assert_eq!(parsed_uncached.audio, AudioCodec::Aac);

        let p2p_stream = make_stream(
            Some("TPB+\n720p"),
            Some("The_Matrix_1999_720p_HDTV_x264_AC3-SCENE\nSeeds: 142 Peers: 19 | 1.85 GiB"),
            None,
            None,
            None,
        );
        let parsed_p2p = parse_stream(&p2p_stream);
        assert_eq!(parsed_p2p.debrid_provider, None);
        assert!(!parsed_p2p.is_cached);
        assert_eq!(parsed_p2p.resolution, Resolution::Hd720p);
        assert_eq!(parsed_p2p.quality, StreamQuality::Hdtv);
        assert_eq!(parsed_p2p.codec, VideoCodec::AVC);
        assert_eq!(parsed_p2p.audio, AudioCodec::Ac3);
        assert_eq!(parsed_p2p.seeders, Some(142));
        assert_eq!(parsed_p2p.size_display.as_deref(), Some("1.85 GB"));
    }

    #[test]
    fn parses_pm_dl_dvdrip_hlg_truehd_dts_and_cam() {
        let pm_stream = make_stream(
            Some("[PM+] Jackettio 4K"),
            Some("Planet.Earth.III.2160p.UHD.HLG.TrueHD.7.1.HEVC.mkv\n💾 18.4 GB"),
            None,
            None,
            None,
        );
        let p_pm = parse_stream(&pm_stream);
        assert_eq!(p_pm.debrid_provider.as_deref(), Some("PM"));
        assert!(p_pm.is_cached);
        assert_eq!(p_pm.hdr, HdrFormat::Hlg);
        assert_eq!(p_pm.audio, AudioCodec::TrueHD);

        let dl_stream = make_stream(
            Some("[DL+] Annatar"),
            Some("Classic.Film.1975.480p.DVDRip.XviD.DTS-OLD.avi\nSeeds: 12 Size: 700 MB"),
            None,
            None,
            None,
        );
        let p_dl = parse_stream(&dl_stream);
        assert_eq!(p_dl.debrid_provider.as_deref(), Some("DL"));
        assert!(p_dl.is_cached);
        assert_eq!(p_dl.resolution, Resolution::Sd);
        assert_eq!(p_dl.hdr, HdrFormat::Sdr);
        assert_eq!(p_dl.quality, StreamQuality::Dvd);
        assert_eq!(p_dl.codec, VideoCodec::Xvid);
        assert_eq!(p_dl.audio, AudioCodec::Dts);

        let cam_stream = make_stream(
            Some("Torrentio"),
            Some("Blockbuster.2026.HDCAM.x264-TS"),
            None,
            None,
            None,
        );
        let p_cam = parse_stream(&cam_stream);
        assert_eq!(p_cam.quality, StreamQuality::Cam);
    }

    #[test]
    fn filters_and_ranks_streams() {
        let s_p2p_1080 = make_stream(
            Some("Torrentio 1080p"),
            Some("Movie.2024.1080p.WEB-DL.H264\n👤 200 💾 4.0 GB"),
            None,
            None,
            None,
        );
        let s_cached_4k_dv = make_stream(
            Some("[RD+] Torrentio 4k"),
            Some("Movie.2024.2160p.REMUX.DV.HEVC.Atmos\n👤 50 💾 55.0 GB"),
            None,
            None,
            None,
        );
        let s_cached_1080 = make_stream(
            Some("[RD+] Torrentio 1080p"),
            Some("Movie.2024.1080p.BluRay.HEVC\n👤 80 💾 10.0 GB"),
            None,
            None,
            None,
        );

        let streams = vec![s_p2p_1080, s_cached_1080, s_cached_4k_dv];
        let ranked = filter_and_rank_streams(&streams, &StreamFilter::new());
        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[0].1.resolution, Resolution::Uhd4k);
        assert!(ranked[0].1.is_cached);
        assert_eq!(ranked[1].1.resolution, Resolution::Fhd1080p);
        assert!(ranked[1].1.is_cached);
        assert!(!ranked[2].1.is_cached);

        let cached_hdr_only = StreamFilter::new().only_cached().only_hdr();
        let filtered = filter_and_rank_streams(&streams, &cached_hdr_only);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1.hdr, HdrFormat::DolbyVision);

        let s_uncached_debrid = make_stream(
            Some("[RD download] Torrentio 1080p"),
            Some("Movie.2024.1080p.WEB-DL.H264\n👤 40 💾 4.5 GB"),
            None,
            None,
            None,
        );
        let mixed = vec![
            streams[0].clone(),
            streams[1].clone(),
            streams[2].clone(),
            s_uncached_debrid,
        ];
        assert!(parse_stream(&mixed[1]).is_debrid_stream());
        assert!(parse_stream(&mixed[3]).is_debrid_stream());
        assert!(!parse_stream(&mixed[0]).is_debrid_stream());

        let non_debrid_filter = StreamFilter {
            exclude_debrid_streams: true,
            ..Default::default()
        };
        let p2p_only = filter_and_rank_streams(&mixed, &non_debrid_filter);
        assert_eq!(p2p_only.len(), 1);
        assert!(!p2p_only[0].1.is_debrid_stream());
        assert_eq!(p2p_only[0].1.seeders, Some(200));
    }

    #[test]
    fn composable_multi_filters_and_addon_source_matching_work() {
        let torrentio_4k_hdr = parse_stream_with_addon(
            &make_stream(
                Some("[RD+] Torrentio 4k"),
                Some("Dune.Part.Two.2024.2160p.WEB-DL.DV.HDR10.HEVC.Atmos"),
                None,
                None,
                None,
            ),
            Some("Torrentio"),
        )
        .with_addon_origin(
            Some("com.stremio.torrentio.addon"),
            Some("Torrentio"),
            Some("https://torrentio.strem.fun/realdebrid=abc/manifest.json"),
        );

        let comet_4k_hdr = parse_stream_with_addon(
            &make_stream(
                Some("[RD+] 2160p"),
                Some("Dune.Part.Two.2024.2160p.BluRay.HDR10.HEVC"),
                None,
                None,
                None,
            ),
            Some("Comet"),
        )
        .with_addon_origin(
            Some("stremio.comet.fast"),
            Some("Comet"),
            Some("https://comet.elfhosted.com/cfg/manifest.json"),
        );

        let webstreamr_1080_hdr = parse_stream_with_addon(
            &make_stream(
                Some("1080p"),
                Some("Dune.Part.Two.2024.1080p.WEB-DL.HDR10.HEVC"),
                None,
                None,
                None,
            ),
            Some("WebStreamr"),
        )
        .with_addon_origin(
            Some("com.webstreamr.addon"),
            Some("WebStreamr"),
            Some("https://webstreamr.hayd.uk/manifest.json"),
        );
        assert_eq!(webstreamr_1080_hdr.addon_name.as_deref(), Some("WebStreamr"));

        let torrentio_1080_sdr = parse_stream_with_addon(
            &make_stream(
                Some("Torrentio 1080p"),
                Some("Dune.Part.Two.2024.1080p.WEB-DL.x264"),
                None,
                None,
                None,
            ),
            Some("Torrentio"),
        )
        .with_addon_origin(
            Some("com.stremio.torrentio.addon"),
            Some("Torrentio"),
            Some("https://torrentio.strem.fun/realdebrid=abc/manifest.json"),
        );

        let all = [
            &torrentio_4k_hdr,
            &comet_4k_hdr,
            &webstreamr_1080_hdr,
            &torrentio_1080_sdr,
        ];

        let f_4k_hdr_torrentio = StreamFilter::new()
            .with_resolution(Resolution::Uhd4k)
            .only_hdr()
            .with_addon_source("com.stremio.torrentio.addon");
        assert!(!f_4k_hdr_torrentio.is_unfiltered());
        let m1: Vec<_> = all
            .iter()
            .copied()
            .filter(|s| s.matches(&f_4k_hdr_torrentio))
            .collect();
        assert_eq!(m1.len(), 1);
        assert_eq!(m1[0].addon_name.as_deref(), Some("Torrentio"));
        assert_eq!(m1[0].resolution, Resolution::Uhd4k);

        let f_4k_or_1080_hdr = StreamFilter::new()
            .with_resolution(Resolution::Uhd4k)
            .with_resolution(Resolution::Fhd1080p)
            .only_hdr();
        let m2: Vec<_> = all
            .iter()
            .copied()
            .filter(|s| s.matches(&f_4k_or_1080_hdr))
            .collect();
        assert_eq!(
            m2.len(),
            3,
            "should match Torrentio 4K HDR, Comet 4K HDR, and WebStreamr 1080p HDR, excluding 1080p SDR"
        );

        let f_multi_addon = f_4k_or_1080_hdr.with_addon_sources(["comet", "WebStreamr"]);
        let m3: Vec<_> = all
            .iter()
            .copied()
            .filter(|s| s.matches(&f_multi_addon))
            .collect();
        assert_eq!(m3.len(), 2);
    }

    #[test]
    fn rejects_untrusted_and_malicious_stream_url_schemes() {
        assert!(is_allowed_addon_stream_url("https://cdn.example.com/movie.mkv"));
        assert!(is_allowed_addon_stream_url("http://127.0.0.1:11470/0123456789abcdef0123456789abcdef01234567/0"));
        assert!(is_allowed_addon_stream_url(
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Movie"
        ));

        let blocked_raw = [
            "file:///etc/passwd",
            "file://localhost/etc/shadow",
            "edl://!new_stream;!no_clip;file:///etc/passwd",
            "lavfi://[testsrc]",
            "av://lavfi:testsrc=duration=5",
            "fd://0",
            "archive:///tmp/payload.zip|/video.mkv",
            "memory://injected_payload",
            "ftp://ftp.example.com/video.mkv",
            "javascript:alert(1)",
            "--script=/tmp/evil.lua",
            "  -o=/tmp/overwrite",
            "https://cdn.example.com/movie.mkv\n--script=/tmp/evil.lua",
            "https://-badhost/video.mkv",
            "",
        ];

        for raw in blocked_raw {
            assert!(
                !is_allowed_addon_stream_url(raw),
                "expected malicious URL to be rejected: {raw:?}"
            );
        }

        let malicious_streams = vec![
            make_stream_with_url("file:///etc/passwd", Some("[RD+] 4K"), Some("2160p"), None, None, None),
            make_stream_with_url("edl://0", Some("[RD+] 4K"), Some("2160p"), None, None, None),
            make_stream_with_url("lavfi://testsrc", Some("[RD+] 4K"), Some("2160p"), None, None, None),
            make_stream_with_url("av://lavfi/testsrc", Some("[RD+] 4K"), Some("2160p"), None, None, None),
            make_stream_with_url("av:lavfi:testsrc", Some("[RD+] 4K"), Some("2160p"), None, None, None),
            make_stream_with_url("fd://3", Some("[RD+] 4K"), Some("2160p"), None, None, None),
            make_stream_with_url("archive:///tmp/a.rar|/b.mkv", Some("[RD+] 4K"), Some("2160p"), None, None, None),
            make_stream_with_url("memory://raw", Some("[RD+] 4K"), Some("2160p"), None, None, None),
        ];

        for bad in &malicious_streams {
            assert!(!is_allowed_stream_source(bad));
            let parsed = parse_stream(bad);
            assert!(!parsed.is_allowed());
            assert!(!parsed.matches(&StreamFilter::default()));
        }

        let mut combined = malicious_streams;
        combined.push(make_stream_with_url(
            "https://cdn.example.com/legit.mkv",
            Some("[RD+] Legit 1080p"),
            Some("Movie.2024.1080p.WEB-DL.mkv"),
            None,
            None,
            None,
        ));

        let ranked = filter_and_rank_streams(&combined, &StreamFilter::default());
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].1.resolution, Resolution::Fhd1080p);
    }
}
