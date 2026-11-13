use core::ffi::{c_char, c_int, c_void, CStr};
use core::ptr;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
#[cfg(target_os = "windows")]
use std::sync::atomic::AtomicU8;
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::thread::{self, JoinHandle};

use gpui::RenderImage;
use image::{Frame, RgbaImage};
use tokio::sync::watch;

use crate::dcomp::DCompBackendMode;
#[cfg(target_os = "windows")]
use crate::dcomp::{parse_swapchain_ptr_str, WindowsDCompUnderlay};
#[cfg(target_os = "windows")]
use crate::mpv_ffi::{mpv_free, mpv_get_property, MPV_EVENT_VIDEO_RECONFIG};
use crate::mpv_ffi::{
    self, error_to_string, mpv_command_async, mpv_create, mpv_error_string, mpv_event_end_file,
    mpv_event_log_message, mpv_event_property, mpv_handle, mpv_initialize, mpv_node,
    mpv_observe_property, mpv_render_context, mpv_render_context_create, mpv_render_context_free,
    mpv_render_context_render, mpv_render_context_report_swap,
    mpv_render_context_set_update_callback, mpv_render_context_update, mpv_render_param,
    mpv_request_log_messages, mpv_set_option_string, mpv_set_property_async, mpv_terminate_destroy,
    mpv_wait_event, mpv_wakeup, MPV_END_FILE_REASON_EOF, MPV_END_FILE_REASON_ERROR,
    MPV_EVENT_COMMAND_REPLY, MPV_EVENT_END_FILE, MPV_EVENT_FILE_LOADED, MPV_EVENT_LOG_MESSAGE,
    MPV_EVENT_NONE, MPV_EVENT_PLAYBACK_RESTART, MPV_EVENT_PROPERTY_CHANGE, MPV_EVENT_SHUTDOWN,
    MPV_EVENT_START_FILE, MPV_FORMAT_DOUBLE, MPV_FORMAT_FLAG, MPV_FORMAT_INT64, MPV_FORMAT_NODE,
    MPV_FORMAT_STRING, MPV_RENDER_API_TYPE_SW, MPV_RENDER_PARAM_ADVANCED_CONTROL,
    MPV_RENDER_PARAM_API_TYPE, MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME, MPV_RENDER_PARAM_INVALID,
    MPV_RENDER_PARAM_SKIP_RENDERING, MPV_RENDER_PARAM_SW_FORMAT, MPV_RENDER_PARAM_SW_POINTER,
    MPV_RENDER_PARAM_SW_SIZE, MPV_RENDER_PARAM_SW_STRIDE, MPV_RENDER_UPDATE_FRAME,
};
#[cfg(target_os = "linux")]
use crate::mpv_ffi::{
    mpv_opengl_fbo, mpv_opengl_init_params, MPV_RENDER_API_TYPE_OPENGL, MPV_RENDER_PARAM_FLIP_Y,
    MPV_RENDER_PARAM_OPENGL_FBO, MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
    MPV_RENDER_PARAM_WL_DISPLAY,
};
use crate::shaders::{
    default_shader_cache_dir, materialize_shader_preset_in_dir, DebandConfig, HdrBackendOptions,
    ShaderPreset,
};
#[cfg(target_os = "linux")]
use crate::subsurface::{egl_get_proc_address, WaylandVideoSubsurface};
use crate::MpvError;

const SW_FORMAT_BGR0: &[u8] = b"bgr0\0";
const MAX_GLOBAL_EVICTED_FRAMES: usize = 32;

const PROP_TIME_POS: u64 = 1;
const PROP_DURATION: u64 = 2;
const PROP_PAUSE: u64 = 3;
const PROP_CORE_IDLE: u64 = 4;
const PROP_PAUSED_FOR_CACHE: u64 = 5;
const PROP_SEEKING: u64 = 6;
const PROP_DEMUXER_CACHE_DURATION: u64 = 7;
const PROP_HWDEC_CURRENT: u64 = 8;
const PROP_VIDEO_CODEC: u64 = 9;
const PROP_VIDEO_WIDTH: u64 = 10;
const PROP_VIDEO_HEIGHT: u64 = 11;
const PROP_VOLUME: u64 = 12;
const PROP_MUTE: u64 = 13;
const PROP_SUB_DELAY: u64 = 14;
const PROP_TRACK_LIST: u64 = 15;
const PROP_EOF_REACHED: u64 = 16;
const PROP_SPEED: u64 = 17;
const PROP_CACHE_SPEED: u64 = 18;
const PROP_VIDEO_FORMAT: u64 = 19;
const PROP_IDLE_ACTIVE: u64 = 20;
#[cfg(target_os = "windows")]
const PROP_DISPLAY_SWAPCHAIN: u64 = 21;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowsBackendSelection {
    backend_mode: DCompBackendMode,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    uses_libmpv_vo: bool,
    defer_media_until_attach: bool,
    hwdec: &'static str,
}

#[allow(dead_code)]
fn is_direct_composition_disabled_value(val: Option<&str>) -> bool {
    matches!(val, Some("true" | "1"))
}

#[cfg(target_os = "windows")]
fn is_direct_composition_disabled() -> bool {
    is_direct_composition_disabled_value(
        std::env::var("GPUI_DISABLE_DIRECT_COMPOSITION")
            .ok()
            .as_deref(),
    )
}

#[allow(dead_code)]
fn resolve_windows_backend_selection(
    dcomp_disabled: bool,
    extra_options: &[(&str, &str)],
) -> WindowsBackendSelection {
    let extra_vo = extra_options
        .iter()
        .rev()
        .find(|&&(k, _)| k == "vo")
        .map(|&(_, v)| v);
    match extra_vo {
        Some("libmpv") => WindowsBackendSelection {
            backend_mode: DCompBackendMode::SoftwareFallback,
            uses_libmpv_vo: true,
            defer_media_until_attach: true,
            hwdec: "d3d11va-copy,dxva2-copy,nvdec-copy,auto-copy",
        },
        Some(_) => WindowsBackendSelection {
            backend_mode: DCompBackendMode::SoftwareFallback,
            uses_libmpv_vo: false,
            defer_media_until_attach: false,
            hwdec: "d3d11va-copy,dxva2-copy,nvdec-copy,auto-copy",
        },
        None if dcomp_disabled => WindowsBackendSelection {
            backend_mode: DCompBackendMode::SoftwareFallback,
            uses_libmpv_vo: true,
            defer_media_until_attach: true,
            hwdec: "d3d11va-copy,dxva2-copy,nvdec-copy,auto-copy",
        },
        None => WindowsBackendSelection {
            backend_mode: DCompBackendMode::FlipChildHwndUnderlay,
            uses_libmpv_vo: false,
            defer_media_until_attach: true,
            hwdec: "d3d11va,nvdec,auto-safe",
        },
    }
}

#[allow(dead_code)]
fn should_probe_d3d11_composition(
    selection: WindowsBackendSelection,
    extra_options: &[(&str, &str)],
) -> bool {
    if selection.backend_mode == DCompBackendMode::SoftwareFallback {
        return false;
    }
    !extra_options.iter().any(|&(k, _)| k == "d3d11-output-mode")
}

#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct Aligned64([u8; 64]);

static GLOBAL_EVICTED_VIDEO_FRAMES: OnceLock<Mutex<Vec<Arc<RenderImage>>>> = OnceLock::new();

fn global_evicted_frames() -> &'static Mutex<Vec<Arc<RenderImage>>> {
    GLOBAL_EVICTED_VIDEO_FRAMES.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn drain_evicted_video_images() -> Vec<Arc<RenderImage>> {
    let mut guard = global_evicted_frames()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    core::mem::take(&mut *guard)
}

fn push_global_evicted_images(images: impl IntoIterator<Item = Arc<RenderImage>>) {
    let mut guard = global_evicted_frames()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for img in images {
        if guard.len() >= MAX_GLOBAL_EVICTED_FRAMES {
            guard.remove(0);
        }
        guard.push(img);
    }
}

fn strip_ascii_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

fn validate_stream_url_impl(raw: &str, allow_test_sources: bool) -> Result<&str, MpvError> {
    if raw.chars().any(|c| matches!(c, '\0' | '\r' | '\n')) {
        return Err(MpvError::Command(
            "rejected media URL containing control characters".into(),
        ));
    }

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(MpvError::Command("rejected empty media URL".into()));
    }
    if trimmed.starts_with('-') {
        return Err(MpvError::Command(
            "rejected media URL starting with '-'".into(),
        ));
    }

    if let Some(rest) = strip_ascii_prefix_ignore_case(trimmed, "https://")
        .or_else(|| strip_ascii_prefix_ignore_case(trimmed, "http://"))
    {
        if !rest.is_empty() && !rest.starts_with('/') && !rest.starts_with(char::is_whitespace) {
            return Ok(trimmed);
        }
        return Err(MpvError::Command(
            "rejected malformed http/https media URL".into(),
        ));
    }

    if allow_test_sources {
        if let Some(rest) = trimmed.strip_prefix("av://lavfi:") {
            if !rest.is_empty() {
                return Ok(trimmed);
            }
        }
        if !trimmed.contains("://") && !trimmed.contains("..") {
            let candidate = Path::new(trimmed);
            if candidate.is_absolute() && candidate.starts_with(std::env::temp_dir()) {
                return Ok(trimmed);
            }
        }
    }

    Err(MpvError::Command(
        "rejected media URL: only http:// and https:// schemes are permitted".into(),
    ))
}

pub fn validate_stream_url(raw: &str) -> Result<&str, MpvError> {
    validate_stream_url_impl(raw, cfg!(test))
}

const MAX_LOCAL_SUBTITLE_BYTES: u64 = 32 * 1024 * 1024;
const DEFAULT_SUBTITLE_COLOR_HEX: &str = "#F5F1E8";

/// Validate and canonicalize a local subtitle file path for `sub-add`.
///
/// Security invariants: rejects empty paths, control characters (`\0`, `\r`, `\n`),
/// URI scheme substrings (`://`), leading `-` option prefixes, unsupported extensions,
/// non-regular files, and files exceeding 32 MiB.
pub fn validate_local_subtitle_path(path: &Path) -> Result<PathBuf, MpvError> {
    if path.as_os_str().is_empty() {
        return Err(MpvError::Command("rejected empty local subtitle path".into()));
    }
    let raw = path
        .to_str()
        .ok_or_else(|| MpvError::Command("rejected non-UTF-8 local subtitle path".into()))?;
    if raw.trim().is_empty() {
        return Err(MpvError::Command("rejected empty local subtitle path".into()));
    }
    if raw.chars().any(|c| matches!(c, '\0' | '\r' | '\n')) {
        return Err(MpvError::Command(
            "rejected local subtitle path containing control characters".into(),
        ));
    }
    if raw.starts_with('-') || raw.trim_start().starts_with('-') {
        return Err(MpvError::Command(
            "rejected local subtitle path starting with '-'".into(),
        ));
    }
    if raw.contains("://") {
        return Err(MpvError::Command(
            "rejected local subtitle path containing URI scheme".into(),
        ));
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .ok_or_else(|| {
            MpvError::Command("rejected local subtitle path without an extension".into())
        })?;
    let supported = ext.eq_ignore_ascii_case("srt")
        || ext.eq_ignore_ascii_case("ass")
        || ext.eq_ignore_ascii_case("ssa")
        || ext.eq_ignore_ascii_case("vtt")
        || ext.eq_ignore_ascii_case("sub");
    if !supported {
        return Err(MpvError::Command(format!(
            "rejected unsupported local subtitle extension `.{ext}`"
        )));
    }

    let canonical = std::fs::canonicalize(path).map_err(|e| {
        MpvError::Command(format!("failed to canonicalize local subtitle path: {e}"))
    })?;
    let canonical_str = canonical.to_str().ok_or_else(|| {
        MpvError::Command("rejected non-UTF-8 canonical local subtitle path".into())
    })?;
    if canonical_str.chars().any(|c| matches!(c, '\0' | '\r' | '\n'))
        || canonical_str.contains("://")
        || canonical_str.starts_with('-')
    {
        return Err(MpvError::Command(
            "rejected unsafe canonical local subtitle path".into(),
        ));
    }

    let metadata = std::fs::metadata(&canonical).map_err(|e| {
        MpvError::Command(format!("failed to stat local subtitle file: {e}"))
    })?;
    if !metadata.is_file() {
        return Err(MpvError::Command(
            "rejected local subtitle path that is not a regular file".into(),
        ));
    }
    if metadata.len() > MAX_LOCAL_SUBTITLE_BYTES {
        return Err(MpvError::Command(format!(
            "rejected local subtitle file exceeding {} bytes",
            MAX_LOCAL_SUBTITLE_BYTES
        )));
    }

    Ok(canonical)
}

fn redact_log_urls(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while !rest.is_empty() {
        let lower = rest.to_ascii_lowercase();
        let next_http = lower.find("http://");
        let next_https = lower.find("https://");
        let pos = match (next_http, next_https) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let Some(idx) = pos else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..idx]);
        let url_part = &rest[idx..];
        let end_idx = url_part
            .find(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | ')' | ']' | '>'))
            .unwrap_or(url_part.len());
        let full_url = &url_part[..end_idx];
        if let Some(scheme_sep) = full_url.find("://") {
            let after_scheme = &full_url[scheme_sep + 3..];
            let host_end = after_scheme
                .find(['/', '?', '#'])
                .unwrap_or(after_scheme.len());
            out.push_str(&full_url[..scheme_sep + 3 + host_end]);
            if host_end < after_scheme.len() {
                out.push_str("/<redacted>");
            }
        } else {
            out.push_str("<redacted>");
        }
        rest = &url_part[end_idx..];
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackType {
    Video,
    Audio,
    Sub,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaTrack {
    pub id: i64,
    pub track_type: TrackType,
    pub title: Option<String>,
    pub lang: Option<String>,
    pub codec: Option<String>,
    pub selected: bool,
    pub external: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToneMappingPreset {
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

impl ToneMappingPreset {
    pub fn as_mpv_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Spline => "spline",
            Self::Bt2446a => "bt.2446a",
            Self::St2094_40 => "st2094-40",
            Self::Hable => "hable",
            Self::Mobius => "mobius",
            Self::Reinhard => "reinhard",
            Self::Clip => "clip",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpscalePreset {
    #[default]
    Bilinear,
    Spline36,
    Lanczos,
    EwaLanczos,
}

impl UpscalePreset {
    pub fn scalers(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Bilinear => ("bilinear", "bilinear", "bilinear"),
            Self::Spline36 => ("spline36", "spline36", "mitchell"),
            Self::Lanczos => ("lanczos", "lanczos", "mitchell"),
            Self::EwaLanczos => ("ewa_lanczossharp", "ewa_lanczossharp", "mitchell"),
        }
    }
}

/// Subtitle background rendering style applied live via libmpv properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubtitleBackgroundMode {
    #[default]
    DropShadow,
    TranslucentPill,
}

/// How ASS/SSA embedded styles interact with user subtitle style overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubtitleAssOverride {
    #[default]
    Scale,
    Force,
    No,
}

impl SubtitleAssOverride {
    /// Returns the libmpv `sub-ass-override` string representation.
    pub fn as_mpv_str(self) -> &'static str {
        match self {
            Self::Scale => "scale",
            Self::Force => "force",
            Self::No => "no",
        }
    }
}

/// Live HDR-safe subtitle styling configuration for libmpv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleStyle {
    pub font_size: u32,
    pub vertical_pos: u32,
    pub color_hex: String,
    pub background: SubtitleBackgroundMode,
    pub ass_override: SubtitleAssOverride,
}

impl Default for SubtitleStyle {
    fn default() -> Self {
        Self {
            font_size: 46,
            vertical_pos: 96,
            color_hex: DEFAULT_SUBTITLE_COLOR_HEX.to_string(),
            background: SubtitleBackgroundMode::DropShadow,
            ass_override: SubtitleAssOverride::Scale,
        }
    }
}

fn validate_subtitle_color_hex(raw: &str) -> String {
    if raw.chars().any(|c| c.is_control()) {
        return DEFAULT_SUBTITLE_COLOR_HEX.to_string();
    }
    let trimmed = raw.trim();
    if trimmed.starts_with('#')
        && matches!(trimmed.len(), 7 | 9)
        && trimmed[1..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        trimmed.to_owned()
    } else {
        DEFAULT_SUBTITLE_COLOR_HEX.to_string()
    }
}

impl SubtitleStyle {
    /// Clamps numeric fields and normalizes `color_hex` to a valid `#RRGGBB`/`#AARRGGBB` hex string.
    pub fn sanitized(mut self) -> Self {
        self.font_size = self.font_size.clamp(20, 96);
        self.vertical_pos = self.vertical_pos.clamp(50, 105);
        self.color_hex = validate_subtitle_color_hex(&self.color_hex);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerSnapshot {
    pub time_pos: f64,
    pub duration: f64,
    pub paused: bool,
    pub core_idle: bool,
    pub buffering: bool,
    pub demuxer_cache_duration: f64,
    pub cache_speed_bytes: i64,
    pub hwdec_current: String,
    pub video_format: String,
    pub video_codec: String,
    pub video_width: i64,
    pub video_height: i64,
    pub volume: f64,
    pub muted: bool,
    pub sub_delay: f64,
    pub tracks: Vec<MediaTrack>,
    pub eof_reached: bool,
    pub speed: f64,
}

impl Default for PlayerSnapshot {
    fn default() -> Self {
        Self {
            time_pos: 0.0,
            duration: 0.0,
            paused: false,
            core_idle: true,
            buffering: false,
            demuxer_cache_duration: 0.0,
            cache_speed_bytes: 0,
            hwdec_current: String::new(),
            video_format: String::new(),
            video_codec: String::new(),
            video_width: 0,
            video_height: 0,
            volume: 100.0,
            muted: false,
            sub_delay: 0.0,
            tracks: Vec::new(),
            eof_reached: false,
            speed: 1.0,
        }
    }
}

impl PlayerSnapshot {
    pub fn short_video_codec(&self) -> String {
        shorten_codec_label(&self.video_format, &self.video_codec)
    }
}

pub fn shorten_codec_label(video_format: &str, video_codec: &str) -> String {
    let fmt_clean = strip_parentheticals(video_format);
    if let Some(canon) = canonical_codec_token(&fmt_clean) {
        return canon.to_owned();
    }

    let codec_clean = strip_parentheticals(video_codec);
    for token in codec_clean
        .split(|c: char| c == '/' || c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        if let Some(canon) = canonical_codec_token(token) {
            return canon.to_owned();
        }
    }

    let combined_lower = format!(
        "{} {}",
        fmt_clean.to_ascii_lowercase(),
        codec_clean.to_ascii_lowercase()
    );
    if combined_lower.contains("hevc")
        || combined_lower.contains("h.265")
        || combined_lower.contains("h265")
    {
        return "HEVC".to_owned();
    }
    if combined_lower.contains("h.264")
        || combined_lower.contains("h264")
        || combined_lower.contains("avc")
    {
        return "AVC".to_owned();
    }
    if combined_lower.contains("av1") {
        return "AV1".to_owned();
    }
    if combined_lower.contains("vp9") {
        return "VP9".to_owned();
    }
    if combined_lower.contains("vp8") {
        return "VP8".to_owned();
    }
    if combined_lower.contains("prores") {
        return "ProRes".to_owned();
    }
    if combined_lower.contains("vc-1") || combined_lower.contains("vc1") {
        return "VC-1".to_owned();
    }
    if combined_lower.contains("mpeg-4") || combined_lower.contains("mpeg4") {
        return "MPEG-4".to_owned();
    }
    if combined_lower.contains("mpeg-2") || combined_lower.contains("mpeg2") {
        return "MPEG-2".to_owned();
    }
    if combined_lower.contains("mpeg-1") || combined_lower.contains("mpeg1") {
        return "MPEG-1".to_owned();
    }

    let fallback_src = if !fmt_clean.is_empty() {
        fmt_clean.as_str()
    } else {
        codec_clean.as_str()
    };
    let first_seg = fallback_src
        .split('/')
        .next()
        .unwrap_or("")
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim();
    if first_seg.is_empty() {
        return String::new();
    }
    if first_seg.len() <= 6 && first_seg.is_ascii() {
        first_seg.to_ascii_uppercase()
    } else {
        first_seg.chars().take(14).collect()
    }
}

fn canonical_codec_token(token: &str) -> Option<&'static str> {
    match token.trim().to_ascii_lowercase().as_str() {
        "hevc" | "h265" | "h.265" | "hev1" | "hvc1" => Some("HEVC"),
        "h264" | "h.264" | "avc" | "avc1" | "x264" => Some("AVC"),
        "av1" | "av01" | "dav1d" | "libdav1d" | "libaom-av1" => Some("AV1"),
        "vp9" | "vp09" => Some("VP9"),
        "vp8" | "vp08" => Some("VP8"),
        "mpeg4" | "mpeg-4" | "mp4v" | "xvid" | "divx" | "msmpeg4v3" => Some("MPEG-4"),
        "mpeg2video" | "mpeg2" | "mpeg-2" | "mpg2" => Some("MPEG-2"),
        "mpeg1video" | "mpeg1" | "mpeg-1" => Some("MPEG-1"),
        "vc1" | "vc-1" | "wmv3" => Some("VC-1"),
        "prores" | "prores_ks" | "prores_aw" | "apch" | "apcn" | "apcs" | "apco" | "ap4h"
        | "ap4x" => Some("ProRes"),
        "vvc" | "h266" | "h.266" => Some("VVC"),
        "theora" => Some("Theora"),
        "mjpeg" => Some("MJPEG"),
        "dnxhd" | "dnxhr" => Some("DNxHD"),
        _ => None,
    }
}

fn strip_parentheticals(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut depth = 0usize;
    for ch in input.chars() {
        match ch {
            '(' => depth = depth.saturating_add(1),
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    SnapshotChanged(PlayerSnapshot),
    FileLoaded,
    EndFile {
        eof: bool,
        error: Option<String>,
    },
}

pub(crate) unsafe fn parse_track_list_node(node: &mpv_node) -> Vec<MediaTrack> {
    let Some(items) = node.as_array() else {
        return Vec::new();
    };
    let mut tracks = Vec::with_capacity(items.len());
    for entry in items {
        let Some(id) = entry.map_get("id").and_then(|n| n.as_i64()) else {
            continue;
        };
        let Some(kind_str) = entry.map_get("type").and_then(|n| n.as_str()) else {
            continue;
        };
        let track_type = match kind_str {
            "video" => TrackType::Video,
            "audio" => TrackType::Audio,
            "sub" => TrackType::Sub,
            _ => continue,
        };
        let title = entry
            .map_get("title")
            .and_then(|n| n.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let lang = entry
            .map_get("lang")
            .and_then(|n| n.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let codec = entry
            .map_get("codec")
            .and_then(|n| n.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let selected = entry
            .map_get("selected")
            .and_then(|n| n.as_bool())
            .unwrap_or(false);
        let external = entry
            .map_get("external")
            .and_then(|n| n.as_bool())
            .unwrap_or(false);

        tracks.push(MediaTrack {
            id,
            track_type,
            title,
            lang,
            codec,
            selected,
            external,
        });
    }
    tracks
}

#[derive(Clone, Copy, Debug)]
struct ViewportState {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    scale_factor: f32,
    visible: bool,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
            scale_factor: 1.0,
            visible: true,
        }
    }
}

struct RenderSharedState {
    viewport: ViewportState,
    geometry_dirty: bool,
    visibility_dirty: bool,
    shutdown: bool,
}

struct RenderCoordinator {
    frame_ready: AtomicBool,
    mutex: Mutex<RenderSharedState>,
    condvar: Condvar,
}

impl RenderCoordinator {
    fn new(initial_viewport: ViewportState) -> Self {
        Self {
            frame_ready: AtomicBool::new(false),
            mutex: Mutex::new(RenderSharedState {
                viewport: initial_viewport,
                geometry_dirty: true,
                visibility_dirty: false,
                shutdown: false,
            }),
            condvar: Condvar::new(),
        }
    }

    fn notify_frame_ready(&self) {
        self.frame_ready.store(true, Ordering::Release);
        self.condvar.notify_one();
    }

    fn request_shutdown(&self) {
        if let Ok(mut guard) = self.mutex.lock() {
            guard.shutdown = true;
        }
        self.condvar.notify_all();
    }
}

unsafe extern "C" fn on_mpv_render_update(cb_ctx: *mut c_void) {
    if cb_ctx.is_null() {
        return;
    }
    let coord = &*(cb_ctx as *const RenderCoordinator);
    coord.notify_frame_ready();
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RenderTargetKey {
    #[cfg(target_os = "linux")]
    Wayland {
        wl_display: usize,
        parent_wl_surface: usize,
    },
    Software,
}

struct ActiveRenderWorker {
    target: RenderTargetKey,
    coordinator: Arc<RenderCoordinator>,
    join_handle: JoinHandle<()>,
}

impl ActiveRenderWorker {
    fn stop_and_join(self) {
        self.coordinator.request_shutdown();
        let _ = self.join_handle.join();
    }
}

#[derive(Default)]
struct SoftwareVideoState {
    latest_frame: Option<Arc<RenderImage>>,
    last_painted: Option<Arc<RenderImage>>,
    evicted: Vec<Arc<RenderImage>>,
}

fn clear_software_video_state(state: &Mutex<SoftwareVideoState>, move_to_global: bool) {
    let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
    guard.latest_frame = None;
    if let Some(painted) = guard.last_painted.take() {
        guard.evicted.push(painted);
    }
    if move_to_global && !guard.evicted.is_empty() {
        let drained = core::mem::take(&mut guard.evicted);
        drop(guard);
        push_global_evicted_images(drained);
    }
}

#[inline]
fn compute_software_canvas_size(width: i32, height: i32, scale_factor: f32) -> (u32, u32) {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let raw_w = ((width.max(2) as f32) * scale).round().max(2.0);
    let raw_h = ((height.max(2) as f32) * scale).round().max(2.0);
    let downscale = (raw_w / 1920.0_f32).max(raw_h / 1080.0_f32).max(1.0);
    let scaled_w = ((raw_w / downscale).round() as u32).clamp(2, 1920);
    let scaled_h = ((raw_h / downscale).round() as u32).clamp(2, 1080);
    ((scaled_w & !1).max(2), (scaled_h & !1).max(2))
}

struct PendingLoad {
    url: String,
    start_time_secs: Option<f64>,
    sub_delay_secs: Option<f64>,
}

enum PendingSubSource {
    RemoteUrl(String),
    LocalFile(PathBuf),
}

struct PendingSub {
    source: PendingSubSource,
    title: Option<String>,
    lang: Option<String>,
}

#[derive(Default)]
struct PendingMediaQueue {
    render_ready: bool,
    load: Option<PendingLoad>,
    subs: Vec<PendingSub>,
}

#[derive(Clone, Debug)]
struct PersistedSettings {
    volume: f64,
    muted: bool,
    speed: f64,
    sub_delay: f64,
    subtitle_style: SubtitleStyle,
    tone_mapping: ToneMappingPreset,
    upscale_mode: UpscalePreset,
    deband: DebandConfig,
    shader_preset: ShaderPreset,
    shader_path: Option<PathBuf>,
    hdr_backend: HdrBackendOptions,
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            volume: 100.0,
            muted: false,
            speed: 1.0,
            sub_delay: 0.0,
            subtitle_style: SubtitleStyle::default(),
            tone_mapping: ToneMappingPreset::default(),
            upscale_mode: UpscalePreset::default(),
            deband: DebandConfig::default(),
            shader_preset: ShaderPreset::default(),
            shader_path: None,
            hdr_backend: HdrBackendOptions::default(),
        }
    }
}

#[derive(Clone, Copy)]
struct RawMpvPtr(*mut mpv_handle);
unsafe impl Send for RawMpvPtr {}
unsafe impl Sync for RawMpvPtr {}

struct ObserverShared {
    snapshot: Arc<RwLock<PlayerSnapshot>>,
    watch_tx: watch::Sender<PlayerSnapshot>,
    event_tx: async_channel::Sender<PlayerEvent>,
    shutdown: AtomicBool,
}

static ACTIVE_MPV_INSTANCES: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_os = "windows")]
fn dcomp_mode_to_u8(mode: DCompBackendMode) -> u8 {
    match mode {
        DCompBackendMode::DirectSwapChainVisual => 1,
        DCompBackendMode::FlipChildHwndUnderlay => 2,
        DCompBackendMode::SoftwareFallback => 0,
    }
}

#[cfg(target_os = "windows")]
fn dcomp_mode_from_u8(val: u8) -> DCompBackendMode {
    match val {
        1 => DCompBackendMode::DirectSwapChainVisual,
        2 => DCompBackendMode::FlipChildHwndUnderlay,
        _ => DCompBackendMode::SoftwareFallback,
    }
}

#[cfg(target_os = "windows")]
#[derive(Default)]
struct WindowsDCompState {
    underlay: Option<WindowsDCompUnderlay>,
    last_handles: Option<gpui::WindowsVideoHandles>,
    wid_applied: Option<isize>,
    attached: bool,
}

#[cfg(target_os = "windows")]
struct WindowsDCompCoordinator {
    mode: AtomicU8,
    swapchain_ptr: AtomicUsize,
    state: Mutex<WindowsDCompState>,
}

#[cfg(target_os = "windows")]
impl WindowsDCompCoordinator {
    fn new(mode: DCompBackendMode) -> Self {
        Self {
            mode: AtomicU8::new(dcomp_mode_to_u8(mode)),
            swapchain_ptr: AtomicUsize::new(0),
            state: Mutex::new(WindowsDCompState::default()),
        }
    }

    fn mode(&self) -> DCompBackendMode {
        dcomp_mode_from_u8(self.mode.load(Ordering::Acquire))
    }

    fn set_mode(&self, mode: DCompBackendMode) {
        self.mode.store(dcomp_mode_to_u8(mode), Ordering::Release);
    }

    fn sync_from_cached(&self, viewport_cache: &Mutex<ViewportState>, observer: &ObserverShared) {
        let mode = self.mode();
        if mode == DCompBackendMode::SoftwareFallback {
            return;
        }
        let vp = *viewport_cache.lock().unwrap_or_else(|e| e.into_inner());
        let (video_w, video_h) = {
            let snap = observer.snapshot.read().unwrap_or_else(|e| e.into_inner());
            (snap.video_width, snap.video_height)
        };
        let swapchain_ptr = if mode == DCompBackendMode::DirectSwapChainVisual {
            self.swapchain_ptr.load(Ordering::Acquire)
        } else {
            0
        };
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !guard.attached {
            return;
        }
        let Some(handles) = guard.last_handles else {
            return;
        };
        if let Some(underlay) = guard.underlay.as_mut() {
            if let Err(err) = underlay.sync(
                handles,
                vp.x,
                vp.y,
                vp.width,
                vp.height,
                vp.scale_factor,
                vp.visible,
                swapchain_ptr,
                video_w,
                video_h,
            ) {
                tracing::warn!(
                    target: "gpui_mpv::dcomp",
                    "cached DirectComposition underlay sync failed: {err}"
                );
            }
        }
    }

    fn detach(&self) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.attached = false;
        guard.last_handles = None;
        guard.wid_applied = None;
        if let Some(underlay) = guard.underlay.as_mut() {
            underlay.detach();
        }
    }

    fn teardown(&self) -> Option<isize> {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.attached = false;
        guard.last_handles = None;
        let old_wid = guard.wid_applied.take();
        if let Some(mut underlay) = guard.underlay.take() {
            underlay.detach();
        }
        self.swapchain_ptr.store(0, Ordering::Release);
        old_wid
    }
}

struct MpvControllerInner {
    mpv_slot: RwLock<Option<RawMpvPtr>>,
    terminated: AtomicBool,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    uses_libmpv_vo: bool,
    defer_media_until_attach: bool,
    next_req_id: AtomicU64,
    observer: Arc<ObserverShared>,
    event_rx_template: async_channel::Receiver<PlayerEvent>,
    viewport_cache: Arc<Mutex<ViewportState>>,
    pending_media: Mutex<PendingMediaQueue>,
    persisted: Mutex<PersistedSettings>,
    failed_target: Mutex<Option<RenderTargetKey>>,
    render_worker: Mutex<Option<ActiveRenderWorker>>,
    sw_video: Arc<Mutex<SoftwareVideoState>>,
    sw_frame_tx: watch::Sender<u64>,
    sw_notifier_attached: AtomicBool,
    #[cfg(target_os = "windows")]
    dcomp: Arc<WindowsDCompCoordinator>,
    event_thread: Mutex<Option<JoinHandle<()>>>,
}

impl MpvControllerInner {
    fn teardown_now(&self) {
        self.terminated.store(true, Ordering::SeqCst);
        self.observer.event_tx.close();
        self.sw_notifier_attached.store(false, Ordering::Release);

        if let Ok(mut q) = self.pending_media.lock() {
            q.render_ready = false;
            q.load = None;
            q.subs.clear();
            q.subs.shrink_to_fit();
        }

        let old_worker = self
            .render_worker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(worker) = old_worker {
            worker.stop_and_join();
        }

        clear_software_video_state(&self.sw_video, true);

        self.observer.shutdown.store(true, Ordering::SeqCst);
        if let Some(mpv_raw) = *self.mpv_slot.read().unwrap_or_else(|e| e.into_inner()) {
            unsafe {
                mpv_wakeup(mpv_raw.0);
            }
        }
        let ev_handle = self
            .event_thread
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(handle) = ev_handle {
            let _ = handle.join();
        }

        let mpv_opt = self
            .mpv_slot
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(mpv_raw) = mpv_opt {
            unsafe {
                mpv_terminate_destroy(mpv_raw.0);
            }
            ACTIVE_MPV_INSTANCES.fetch_sub(1, Ordering::AcqRel);
        }

        #[cfg(target_os = "windows")]
        let _ = self.dcomp.teardown();

        if let Ok(mut snap) = self.observer.snapshot.write() {
            *snap = PlayerSnapshot::default();
        }
        if let Ok(mut p) = self.persisted.lock() {
            p.shader_path = None;
        }
    }
}

impl Drop for MpvControllerInner {
    fn drop(&mut self) {
        self.teardown_now();
    }
}

#[derive(Clone)]
pub struct MpvController {
    inner: Arc<MpvControllerInner>,
}

impl MpvController {
    pub fn new() -> Result<Self, MpvError> {
        Self::with_extra_options(&[])
    }

    pub fn with_extra_options(extra_options: &[(&str, &str)]) -> Result<Self, MpvError> {
        unsafe {
            mpv_ffi::ensure_mpv_loaded().map_err(MpvError::Init)?;
            mpv_ffi::set_c_numeric_locale();

            let mpv = mpv_create();
            if mpv.is_null() {
                return Err(MpvError::Init("mpv_create returned NULL".into()));
            }

            #[cfg(target_os = "windows")]
            let win_sel = resolve_windows_backend_selection(
                is_direct_composition_disabled(),
                extra_options,
            );

            #[cfg(target_os = "linux")]
            let defaults: &[(&str, &str)] = &[
                ("vo", "libmpv"),
                ("hwdec", "auto-safe"),
                ("keep-open", "yes"),
                ("ytdl", "no"),
                ("cache", "yes"),
                ("demuxer-max-bytes", "64MiB"),
                ("demuxer-max-back-bytes", "24MiB"),
                ("sub-auto", "fuzzy"),
                ("audio-client-name", "Ferrite"),
                ("audio-pitch-correction", "yes"),
                ("terminal", "no"),
            ];

            #[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
            let defaults: &[(&str, &str)] = &[
                ("vo", "libmpv"),
                ("hwdec", "d3d11va-copy,dxva2-copy,nvdec-copy,auto-copy"),
                ("keep-open", "yes"),
                ("ytdl", "no"),
                ("cache", "yes"),
                ("demuxer-max-bytes", "64MiB"),
                ("demuxer-max-back-bytes", "24MiB"),
                ("sub-auto", "fuzzy"),
                ("audio-client-name", "Ferrite"),
                ("audio-pitch-correction", "yes"),
                ("terminal", "no"),
            ];

            #[cfg(target_os = "windows")]
            let dcomp_win_defaults: &[(&str, &str)] = &[
                ("vo", "gpu"),
                ("gpu-api", "d3d11"),
                ("gpu-context", "d3d11"),
                ("d3d11-flip", "yes"),
                ("hwdec", win_sel.hwdec),
                ("osc", "no"),
                ("input-default-bindings", "no"),
                ("input-vo-keyboard", "no"),
                ("keep-open", "yes"),
                ("ytdl", "no"),
                ("cache", "yes"),
                ("demuxer-max-bytes", "64MiB"),
                ("demuxer-max-back-bytes", "24MiB"),
                ("sub-auto", "fuzzy"),
                ("audio-client-name", "Ferrite"),
                ("audio-pitch-correction", "yes"),
                ("terminal", "no"),
            ];

            #[cfg(target_os = "windows")]
            let sw_win_defaults: &[(&str, &str)] = &[
                ("vo", "libmpv"),
                ("hwdec", win_sel.hwdec),
                ("keep-open", "yes"),
                ("ytdl", "no"),
                ("cache", "yes"),
                ("demuxer-max-bytes", "64MiB"),
                ("demuxer-max-back-bytes", "24MiB"),
                ("sub-auto", "fuzzy"),
                ("audio-client-name", "Ferrite"),
                ("audio-pitch-correction", "yes"),
                ("terminal", "no"),
            ];

            #[cfg(target_os = "windows")]
            let defaults: &[(&str, &str)] = if win_sel.backend_mode == DCompBackendMode::SoftwareFallback
            {
                sw_win_defaults
            } else {
                dcomp_win_defaults
            };

            let mut active_vo = defaults
                .iter()
                .find(|&&(k, _)| k == "vo")
                .map(|&(_, v)| v)
                .unwrap_or("libmpv");
            for &(k, v) in defaults.iter().chain(extra_options.iter()) {
                if k == "vo" {
                    active_vo = v;
                }
                if let Err(err) = set_option_str(mpv, k, v) {
                    mpv_terminate_destroy(mpv);
                    return Err(err);
                }
            }

            #[cfg(target_os = "windows")]
            let backend_mode = if should_probe_d3d11_composition(win_sel, extra_options) {
                if set_option_str(mpv, "d3d11-output-mode", "composition").is_ok() {
                    DCompBackendMode::DirectSwapChainVisual
                } else {
                    DCompBackendMode::FlipChildHwndUnderlay
                }
            } else {
                win_sel.backend_mode
            };

            let _ = mpv_request_log_messages(mpv, c"warn".as_ptr());

            let init_rc = mpv_initialize(mpv);
            if init_rc < 0 {
                let msg = error_to_string(init_rc);
                mpv_terminate_destroy(mpv);
                return Err(MpvError::Init(format!("mpv_initialize failed: {msg}")));
            }

            observe_all_properties(mpv)?;

            #[cfg(target_os = "windows")]
            if backend_mode == DCompBackendMode::DirectSwapChainVisual {
                let rc = mpv_observe_property(
                    mpv,
                    PROP_DISPLAY_SWAPCHAIN,
                    c"display-swapchain".as_ptr(),
                    MPV_FORMAT_STRING,
                );
                if rc < 0 {
                    tracing::warn!(
                        target: "gpui_mpv::dcomp",
                        "failed to observe display-swapchain property: {}",
                        error_to_string(rc)
                    );
                }
            }

            let initial_snapshot = PlayerSnapshot::default();
            let snapshot = Arc::new(RwLock::new(initial_snapshot.clone()));
            let (watch_tx, _watch_rx) = watch::channel(initial_snapshot);
            let (event_tx, event_rx_template) = async_channel::bounded(256);
            let (sw_frame_tx, _sw_frame_rx) = watch::channel(0u64);

            let observer = Arc::new(ObserverShared {
                snapshot,
                watch_tx,
                event_tx,
                shutdown: AtomicBool::new(false),
            });
            let viewport_cache = Arc::new(Mutex::new(ViewportState::default()));
            #[cfg(target_os = "windows")]
            let dcomp = Arc::new(WindowsDCompCoordinator::new(backend_mode));

            let mpv_ptr = RawMpvPtr(mpv);
            let observer_clone = Arc::clone(&observer);
            #[cfg(target_os = "windows")]
            let dcomp_clone = Arc::clone(&dcomp);
            #[cfg(target_os = "windows")]
            let viewport_cache_clone = Arc::clone(&viewport_cache);

            let event_join = thread::Builder::new()
                .name("ferrite-mpv-events".into())
                .spawn(move || {
                    #[cfg(target_os = "windows")]
                    run_event_loop(mpv_ptr, observer_clone, dcomp_clone, viewport_cache_clone);
                    #[cfg(not(target_os = "windows"))]
                    run_event_loop(mpv_ptr, observer_clone);
                })
                .map_err(|e| {
                    mpv_terminate_destroy(mpv);
                    MpvError::Init(format!("failed to spawn mpv event thread: {e}"))
                })?;

            let uses_libmpv_vo = active_vo == "libmpv";
            #[cfg(target_os = "windows")]
            let defer_media_until_attach = uses_libmpv_vo || win_sel.defer_media_until_attach;
            #[cfg(not(target_os = "windows"))]
            let defer_media_until_attach = uses_libmpv_vo;

            ACTIVE_MPV_INSTANCES.fetch_add(1, Ordering::AcqRel);
            let inner = Arc::new(MpvControllerInner {
                mpv_slot: RwLock::new(Some(mpv_ptr)),
                terminated: AtomicBool::new(false),
                uses_libmpv_vo,
                defer_media_until_attach,
                next_req_id: AtomicU64::new(100),
                observer,
                event_rx_template,
                viewport_cache,
                pending_media: Mutex::new(PendingMediaQueue {
                    render_ready: !defer_media_until_attach,
                    load: None,
                    subs: Vec::new(),
                }),
                persisted: Mutex::new(PersistedSettings::default()),
                failed_target: Mutex::new(None),
                render_worker: Mutex::new(None),
                sw_video: Arc::new(Mutex::new(SoftwareVideoState::default())),
                sw_frame_tx,
                sw_notifier_attached: AtomicBool::new(false),
                #[cfg(target_os = "windows")]
                dcomp,
                event_thread: Mutex::new(Some(event_join)),
            });

            let controller = Self { inner };
            controller.apply_subtitle_style(&SubtitleStyle::default())?;
            Ok(controller)
        }
    }

    pub fn active_instance_count() -> usize {
        ACTIVE_MPV_INSTANCES.load(Ordering::Acquire)
    }

    pub fn is_terminated(&self) -> bool {
        self.inner.terminated.load(Ordering::Acquire)
    }

    pub fn shutdown_async(self) {
        self.inner.terminated.store(true, Ordering::SeqCst);
        self.inner.observer.event_tx.close();

        let res = thread::Builder::new()
            .name("ferrite-mpv-teardown".into())
            .spawn(move || {
                self.inner.teardown_now();
                drop(self);
                crate::trim_process_heap();
            });

        if res.is_err() {
            crate::trim_process_heap();
        }
    }

    pub fn shutdown_blocking(self) {
        self.inner.teardown_now();
        drop(self);
        crate::trim_process_heap();
    }

    #[cfg(target_os = "linux")]
    pub fn attach_wayland_subsurface(
        &self,
        wl_display: *mut c_void,
        parent_wl_surface: *mut c_void,
    ) -> Result<(), MpvError> {
        if self.inner.terminated.load(Ordering::Acquire) {
            return Err(MpvError::Init(
                "cannot attach Wayland subsurface to a terminated MpvController".into(),
            ));
        }
        if wl_display.is_null() || parent_wl_surface.is_null() {
            return Err(MpvError::Wayland(
                "cannot attach Wayland subsurface with null display or surface pointer".into(),
            ));
        }

        let target = RenderTargetKey::Wayland {
            wl_display: wl_display as usize,
            parent_wl_surface: parent_wl_surface as usize,
        };

        let mut worker_guard = self
            .inner
            .render_worker
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        if let Some(existing) = worker_guard.as_ref() {
            if existing.target == target {
                return Ok(());
            }
        }

        if let Ok(failed) = self.inner.failed_target.lock() {
            if *failed == Some(target) {
                return Err(MpvError::Render(
                    "Wayland subsurface attachment previously failed for this window".into(),
                ));
            }
        }

        if let Some(old_worker) = worker_guard.take() {
            if let Ok(mut q) = self.inner.pending_media.lock() {
                q.render_ready = !self.inner.defer_media_until_attach;
            }
            old_worker.stop_and_join();
            clear_software_video_state(&self.inner.sw_video, false);
            self.inner
                .sw_notifier_attached
                .store(false, Ordering::Release);
        }

        let Some(mpv_ptr) = *self
            .inner
            .mpv_slot
            .read()
            .unwrap_or_else(|e| e.into_inner())
        else {
            return Err(MpvError::Init("mpv_handle already terminated".into()));
        };
        let initial_vp = *self
            .inner
            .viewport_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let coordinator = Arc::new(RenderCoordinator::new(initial_vp));
        let coord_for_thread = Arc::clone(&coordinator);
        let wl_display_usize = wl_display as usize;
        let parent_surface_usize = parent_wl_surface as usize;

        let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<Result<(), MpvError>>(1);

        let join_handle = thread::Builder::new()
            .name("ferrite-mpv-render".into())
            .spawn(move || {
                run_wayland_render_loop(
                    mpv_ptr,
                    wl_display_usize as *mut c_void,
                    parent_surface_usize as *mut c_void,
                    coord_for_thread,
                    init_tx,
                );
            })
            .map_err(|e| MpvError::Init(format!("failed to spawn mpv render thread: {e}")))?;

        match init_rx.recv() {
            Ok(Ok(())) => {
                if let Ok(mut failed) = self.inner.failed_target.lock() {
                    *failed = None;
                }
                *worker_guard = Some(ActiveRenderWorker {
                    target,
                    coordinator,
                    join_handle,
                });
                drop(worker_guard);
                self.flush_pending_media()?;
                Ok(())
            }
            Ok(Err(err)) => {
                if let Ok(mut failed) = self.inner.failed_target.lock() {
                    *failed = Some(target);
                }
                let _ = join_handle.join();
                Err(err)
            }
            Err(_) => {
                if let Ok(mut failed) = self.inner.failed_target.lock() {
                    *failed = Some(target);
                }
                let _ = join_handle.join();
                Err(MpvError::Init(
                    "mpv render thread exited unexpectedly during initialization".into(),
                ))
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn attach_wayland_subsurface(
        &self,
        _wl_display: *mut c_void,
        _parent_wl_surface: *mut c_void,
    ) -> Result<(), MpvError> {
        Err(MpvError::Wayland(
            "Wayland subsurface is only supported on Linux".into(),
        ))
    }

    #[cfg(target_os = "windows")]
    pub fn attach_or_sync_windows_dcomp(
        &self,
        handles: gpui::WindowsVideoHandles,
    ) -> Result<(), MpvError> {
        if self.inner.terminated.load(Ordering::Acquire) {
            return Ok(());
        }
        let mode = self.inner.dcomp.mode();
        if mode == DCompBackendMode::SoftwareFallback {
            return Err(MpvError::Render(
                "DirectComposition underlay is disabled".into(),
            ));
        }
        if !handles.direct_composition_active || handles.hwnd == 0 {
            return Err(MpvError::Render(
                "DirectComposition is not active for this window".into(),
            ));
        }

        let vp = *self
            .inner
            .viewport_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (video_w, video_h) = {
            let snap = self
                .inner
                .observer
                .snapshot
                .read()
                .unwrap_or_else(|e| e.into_inner());
            (snap.video_width, snap.video_height)
        };

        let swapchain_ptr = if mode == DCompBackendMode::DirectSwapChainVisual {
            let mut cached = self.inner.dcomp.swapchain_ptr.load(Ordering::Acquire);
            if cached == 0 {
                if let Some(mpv_raw) = *self.inner.mpv_slot.read().unwrap_or_else(|e| e.into_inner())
                {
                    let queried = unsafe { read_display_swapchain_ptr(mpv_raw.0) };
                    if queried != 0 {
                        self.inner.dcomp.swapchain_ptr.store(queried, Ordering::Release);
                        cached = queried;
                    }
                }
            }
            cached
        } else {
            0
        };

        let (need_wid, need_flush) = {
            let mut guard = self
                .inner
                .dcomp
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            guard.last_handles = Some(handles);
            let underlay = guard
                .underlay
                .get_or_insert_with(|| WindowsDCompUnderlay::new(mode));
            let _diff = underlay.sync(
                handles,
                vp.x,
                vp.y,
                vp.width,
                vp.height,
                vp.scale_factor,
                vp.visible,
                swapchain_ptr,
                video_w,
                video_h,
            )?;
            let child_hwnd = if mode == DCompBackendMode::FlipChildHwndUnderlay {
                underlay.child_hwnd_isize()
            } else {
                None
            };
            let wid_update = child_hwnd.filter(|&h| guard.wid_applied != Some(h));
            if let Some(hwnd_val) = wid_update {
                guard.wid_applied = Some(hwnd_val);
            }
            let first_attach = !guard.attached;
            guard.attached = true;
            (wid_update, first_attach)
        };

        if let Some(hwnd_val) = need_wid {
            self.set_property_i64("wid", hwnd_val as i64)?;
        }
        if need_flush {
            self.flush_pending_media()?;
        }
        Ok(())
    }

    #[cfg(target_os = "windows")]
    pub fn detach_windows_dcomp(&self) {
        self.inner.dcomp.detach();
    }

    #[cfg(target_os = "windows")]
    pub fn is_windows_dcomp_active(&self) -> bool {
        self.inner.dcomp.mode() != DCompBackendMode::SoftwareFallback
    }

    pub fn attach_software_renderer(&self) -> Result<(), MpvError> {
        if self.inner.terminated.load(Ordering::Acquire) {
            return Err(MpvError::Init(
                "cannot attach software renderer to a terminated MpvController".into(),
            ));
        }

        let target = RenderTargetKey::Software;

        let mut worker_guard = self
            .inner
            .render_worker
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        if let Some(existing) = worker_guard.as_ref() {
            if existing.target == target {
                return Ok(());
            }
        }

        if let Ok(failed) = self.inner.failed_target.lock() {
            if *failed == Some(target) {
                return Err(MpvError::Render(
                    "software render context previously failed to initialize".into(),
                ));
            }
        }

        #[cfg(target_os = "windows")]
        {
            let prev_wid = self.inner.dcomp.teardown();
            self.inner
                .dcomp
                .set_mode(DCompBackendMode::SoftwareFallback);
            if prev_wid.is_some() {
                let _ = self.set_property_i64("wid", 0);
            }
            if !self.inner.uses_libmpv_vo {
                let _ = self.set_property_string(
                    "hwdec",
                    "d3d11va-copy,dxva2-copy,nvdec-copy,auto-copy",
                );
                let _ = self.set_property_string("vo", "libmpv");
            }
        }

        if let Some(old_worker) = worker_guard.take() {
            if let Ok(mut q) = self.inner.pending_media.lock() {
                q.render_ready = !self.inner.defer_media_until_attach;
            }
            old_worker.stop_and_join();
            clear_software_video_state(&self.inner.sw_video, false);
            self.inner
                .sw_notifier_attached
                .store(false, Ordering::Release);
        }

        let Some(mpv_ptr) = *self
            .inner
            .mpv_slot
            .read()
            .unwrap_or_else(|e| e.into_inner())
        else {
            return Err(MpvError::Init("mpv_handle already terminated".into()));
        };
        let initial_vp = *self
            .inner
            .viewport_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let coordinator = Arc::new(RenderCoordinator::new(initial_vp));
        let coord_for_thread = Arc::clone(&coordinator);
        let sw_video_for_thread = Arc::clone(&self.inner.sw_video);
        let sw_frame_tx = self.inner.sw_frame_tx.clone();

        let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<Result<(), MpvError>>(1);

        let join_handle = thread::Builder::new()
            .name("ferrite-mpv-sw-render".into())
            .spawn(move || {
                run_software_render_loop(
                    mpv_ptr,
                    coord_for_thread,
                    sw_video_for_thread,
                    sw_frame_tx,
                    init_tx,
                );
            })
            .map_err(|e| MpvError::Init(format!("failed to spawn mpv SW render thread: {e}")))?;

        match init_rx.recv() {
            Ok(Ok(())) => {
                if let Ok(mut failed) = self.inner.failed_target.lock() {
                    *failed = None;
                }
                *worker_guard = Some(ActiveRenderWorker {
                    target,
                    coordinator,
                    join_handle,
                });
                drop(worker_guard);
                self.flush_pending_media()?;
                Ok(())
            }
            Ok(Err(err)) => {
                if let Ok(mut failed) = self.inner.failed_target.lock() {
                    *failed = Some(target);
                }
                let _ = join_handle.join();
                Err(err)
            }
            Err(_) => {
                if let Ok(mut failed) = self.inner.failed_target.lock() {
                    *failed = Some(target);
                }
                let _ = join_handle.join();
                Err(MpvError::Init(
                    "mpv SW render thread exited unexpectedly during initialization".into(),
                ))
            }
        }
    }

    pub fn detach_wayland_subsurface(&self) {
        let mut worker_guard = self
            .inner
            .render_worker
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(worker) = worker_guard.take() {
            if let Ok(mut q) = self.inner.pending_media.lock() {
                q.render_ready = !self.inner.defer_media_until_attach;
            }
            worker.stop_and_join();
            clear_software_video_state(&self.inner.sw_video, true);
            self.inner
                .sw_notifier_attached
                .store(false, Ordering::Release);
        }
    }

    pub fn is_subsurface_attached(&self) -> bool {
        self.inner
            .render_worker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    pub fn current_video_frame(&self) -> Option<Arc<RenderImage>> {
        self.inner
            .sw_video
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .latest_frame
            .clone()
    }

    pub fn latch_video_frame_for_paint(&self) -> Option<Arc<RenderImage>> {
        let mut guard = self
            .inner
            .sw_video
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let current = guard.latest_frame.clone();
        let should_evict_prev = match (&guard.last_painted, &current) {
            (Some(prev), Some(cur)) => !Arc::ptr_eq(prev, cur),
            (Some(_), None) => true,
            (None, _) => false,
        };
        if should_evict_prev {
            if let Some(old) = guard.last_painted.take() {
                guard.evicted.push(old);
            }
        }
        guard.last_painted = current.clone();
        current
    }

    pub fn take_evicted_video_frames(&self) -> Vec<Arc<RenderImage>> {
        let mut guard = self
            .inner
            .sw_video
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        core::mem::take(&mut guard.evicted)
    }

    pub(crate) fn subscribe_software_frames(&self) -> watch::Receiver<u64> {
        self.inner.sw_frame_tx.subscribe()
    }

    pub(crate) fn mark_software_notifier_attached(&self) -> bool {
        self.inner
            .sw_notifier_attached
            .swap(true, Ordering::AcqRel)
    }

    pub fn snapshot(&self) -> PlayerSnapshot {
        self.inner
            .observer
            .snapshot
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn snapshot_handle(&self) -> Arc<RwLock<PlayerSnapshot>> {
        Arc::clone(&self.inner.observer.snapshot)
    }

    pub fn subscribe(&self) -> watch::Receiver<PlayerSnapshot> {
        self.inner.observer.watch_tx.subscribe()
    }

    pub fn subscribe_events(&self) -> async_channel::Receiver<PlayerEvent> {
        self.inner.event_rx_template.clone()
    }

    pub fn load_file(
        &self,
        url: impl AsRef<str>,
        start_time_secs: impl Into<Option<f64>>,
    ) -> Result<(), MpvError> {
        self.load_file_with_sub_delay(url.as_ref(), start_time_secs.into(), None)
    }

    fn load_file_with_sub_delay(
        &self,
        url_str: &str,
        start_secs: Option<f64>,
        sub_delay_secs: Option<f64>,
    ) -> Result<(), MpvError> {
        let clean_url = validate_stream_url(url_str)?;
        {
            let mut q = self
                .inner
                .pending_media
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !q.render_ready {
                q.load = Some(PendingLoad {
                    url: clean_url.to_owned(),
                    start_time_secs: start_secs,
                    sub_delay_secs,
                });
                q.subs.clear();
                return Ok(());
            }
        }

        self.dispatch_loadfile_now(clean_url, start_secs, sub_delay_secs)
    }

    fn dispatch_loadfile_now(
        &self,
        url: &str,
        start_time_secs: Option<f64>,
        sub_delay_secs: Option<f64>,
    ) -> Result<(), MpvError> {
        let clean_url = validate_stream_url(url)?;
        let mut per_file_opts = Vec::with_capacity(2);
        if let Some(secs) = start_time_secs.filter(|s| s.is_finite() && *s > 0.0) {
            per_file_opts.push(format!("start={secs:.3}"));
        }
        if let Some(delay) = sub_delay_secs.filter(|d| d.is_finite() && d.abs() >= 1e-4) {
            per_file_opts.push(format!("sub-delay={delay:.3}"));
        }

        if !per_file_opts.is_empty() {
            let opt = per_file_opts.join(",");
            let api_ver = unsafe { mpv_ffi::mpv_client_api_version() };
            if api_ver >= ((2 << 16) | 3) {
                self.command_async(&["loadfile", clean_url, "replace", "-1", &opt])
            } else {
                self.command_async(&["loadfile", clean_url, "replace", &opt])
            }
        } else {
            self.command_async(&["loadfile", clean_url, "replace"])
        }
    }

    fn flush_pending_media(&self) -> Result<(), MpvError> {
        let (pending_load, pending_subs) = {
            let mut q = self
                .inner
                .pending_media
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            q.render_ready = true;
            (q.load.take(), core::mem::take(&mut q.subs))
        };

        if let Some(load) = pending_load {
            self.dispatch_loadfile_now(&load.url, load.start_time_secs, load.sub_delay_secs)?;
        }
        self.drain_pending_subtitles(pending_subs)
    }

    fn drain_pending_subtitles(&self, pending_subs: Vec<PendingSub>) -> Result<(), MpvError> {
        for sub in pending_subs {
            match sub.source {
                PendingSubSource::RemoteUrl(url) => {
                    self.dispatch_add_subtitle_now(
                        &url,
                        sub.title.as_deref(),
                        sub.lang.as_deref(),
                    )?;
                }
                PendingSubSource::LocalFile(path) => {
                    self.dispatch_add_local_subtitle_now(
                        &path,
                        sub.title.as_deref(),
                        sub.lang.as_deref(),
                    )?;
                }
            }
        }
        Ok(())
    }

    pub fn stop(&self) -> Result<(), MpvError> {
        if let Ok(mut q) = self.inner.pending_media.lock() {
            q.load = None;
            q.subs.clear();
        }
        clear_software_video_state(&self.inner.sw_video, false);
        self.command_async(&["stop"])
    }

    pub fn set_paused(&self, paused: bool) -> Result<(), MpvError> {
        self.set_property_flag("pause", paused)
    }

    pub fn toggle_pause(&self) -> Result<(), MpvError> {
        self.command_async(&["cycle", "pause"])
    }

    pub fn seek_relative(&self, secs: f64) -> Result<(), MpvError> {
        if !secs.is_finite() {
            return Ok(());
        }
        let arg = format!("{secs:.3}");
        self.command_async(&["seek", &arg, "relative"])
    }

    pub fn seek_absolute(&self, secs: f64) -> Result<(), MpvError> {
        if !secs.is_finite() {
            return Ok(());
        }
        let clamped = secs.max(0.0);
        let arg = format!("{clamped:.3}");
        self.command_async(&["seek", &arg, "absolute"])
    }

    pub fn set_volume(&self, volume: f64) -> Result<(), MpvError> {
        if !volume.is_finite() {
            return Ok(());
        }
        let clamped = volume.clamp(0.0, 150.0);
        if let Ok(mut p) = self.inner.persisted.lock() {
            p.volume = clamped;
        }
        self.set_property_double("volume", clamped)
    }

    pub fn set_muted(&self, muted: bool) -> Result<(), MpvError> {
        if let Ok(mut p) = self.inner.persisted.lock() {
            p.muted = muted;
        }
        self.set_property_flag("mute", muted)
    }

    pub fn select_audio_track(&self, track_id: i64) -> Result<(), MpvError> {
        self.set_property_i64("aid", track_id)
    }

    pub fn select_subtitle_track(&self, track_id: Option<i64>) -> Result<(), MpvError> {
        match track_id {
            Some(id) => self.set_property_i64("sid", id),
            None => self.set_property_string("sid", "no"),
        }
    }

    pub fn add_subtitle(
        &self,
        url: impl AsRef<str>,
        title: Option<&str>,
        lang: Option<&str>,
    ) -> Result<(), MpvError> {
        let clean_url = validate_stream_url(url.as_ref())?;
        {
            let mut q = self
                .inner
                .pending_media
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !q.render_ready {
                q.subs.push(PendingSub {
                    source: PendingSubSource::RemoteUrl(clean_url.to_owned()),
                    title: title.map(str::to_owned),
                    lang: lang.map(str::to_owned),
                });
                return Ok(());
            }
        }
        self.dispatch_add_subtitle_now(clean_url, title, lang)
    }

    /// Safely validates and loads a local `.srt`/`.ass`/`.ssa`/`.vtt`/`.sub` subtitle file via `sub-add <path> auto`.
    pub fn add_local_subtitle_file(
        &self,
        path: &Path,
        title: Option<&str>,
        lang: Option<&str>,
    ) -> Result<(), MpvError> {
        let canonical = validate_local_subtitle_path(path)?;
        {
            let mut q = self
                .inner
                .pending_media
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !q.render_ready {
                q.subs.push(PendingSub {
                    source: PendingSubSource::LocalFile(canonical),
                    title: title.map(str::to_owned),
                    lang: lang.map(str::to_owned),
                });
                return Ok(());
            }
        }
        self.dispatch_add_local_subtitle_now(&canonical, title, lang)
    }

    fn dispatch_add_subtitle_now(
        &self,
        url: &str,
        title: Option<&str>,
        lang: Option<&str>,
    ) -> Result<(), MpvError> {
        let clean_url = validate_stream_url(url)?;
        let t = title.unwrap_or("");
        let l = lang.unwrap_or("");
        if !l.is_empty() {
            self.command_async(&["sub-add", clean_url, "select", t, l])
        } else if !t.is_empty() {
            self.command_async(&["sub-add", clean_url, "select", t])
        } else {
            self.command_async(&["sub-add", clean_url, "select"])
        }
    }

    fn dispatch_add_local_subtitle_now(
        &self,
        path: &Path,
        title: Option<&str>,
        lang: Option<&str>,
    ) -> Result<(), MpvError> {
        let canonical = validate_local_subtitle_path(path)?;
        let path_str = canonical.to_str().ok_or_else(|| {
            MpvError::Command("rejected non-UTF-8 canonical local subtitle path".into())
        })?;
        let t = title.unwrap_or("");
        let l = lang.unwrap_or("");
        if !l.is_empty() {
            self.command_async(&["sub-add", path_str, "auto", t, l])
        } else if !t.is_empty() {
            self.command_async(&["sub-add", path_str, "auto", t])
        } else {
            self.command_async(&["sub-add", path_str, "auto"])
        }
    }

    pub fn set_subtitle_delay(&self, delay_secs: f64) -> Result<(), MpvError> {
        if !delay_secs.is_finite() {
            return Ok(());
        }
        if let Ok(mut p) = self.inner.persisted.lock() {
            p.sub_delay = delay_secs;
        }
        self.set_property_double("sub-delay", delay_secs)
    }

    /// Applies and persists a sanitized HDR-safe subtitle style to the live libmpv instance.
    pub fn set_subtitle_style(&self, style: SubtitleStyle) -> Result<(), MpvError> {
        let sanitized = style.sanitized();
        if let Ok(mut p) = self.inner.persisted.lock() {
            p.subtitle_style = sanitized.clone();
        }
        self.apply_subtitle_style(&sanitized)
    }

    /// Returns the currently persisted subtitle style.
    pub fn subtitle_style(&self) -> SubtitleStyle {
        self.inner
            .persisted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .subtitle_style
            .clone()
    }

    fn apply_subtitle_style(&self, style: &SubtitleStyle) -> Result<(), MpvError> {
        self.set_property_double("sub-font-size", f64::from(style.font_size))?;
        self.set_property_double("sub-pos", f64::from(style.vertical_pos))?;
        self.set_property_string("sub-color", &style.color_hex)?;
        self.set_property_string("sub-ass-override", style.ass_override.as_mpv_str())?;

        match style.background {
            SubtitleBackgroundMode::DropShadow => {
                self.set_property_string("sub-border-style", "outline-and-shadow")?;
                self.set_property_string("sub-back-color", "#00000000")?;
                self.set_property_double("sub-border-size", 2.0_f64)?;
                self.set_property_string("sub-border-color", "#E008080A")?;
                self.set_property_double("sub-shadow-offset", 2.5_f64)?;
                self.set_property_string("sub-shadow-color", "#C0000000")?;
            }
            SubtitleBackgroundMode::TranslucentPill => {
                self.set_property_string("sub-border-style", "background-box")?;
                self.set_property_string("sub-back-color", "#B8141417")?;
                self.set_property_double("sub-border-size", 6.0_f64)?;
                self.set_property_string("sub-border-color", "#00000000")?;
                self.set_property_double("sub-shadow-offset", 0.0_f64)?;
            }
        }
        Ok(())
    }

    pub fn set_hdr_tone_mapping(&self, preset: ToneMappingPreset) -> Result<(), MpvError> {
        if let Ok(mut p) = self.inner.persisted.lock() {
            p.tone_mapping = preset;
        }
        self.set_property_string("tone-mapping", preset.as_mpv_str())
    }

    pub fn tone_mapping_preset(&self) -> ToneMappingPreset {
        self.inner
            .persisted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tone_mapping
    }

    pub fn set_upscale_mode(&self, preset: UpscalePreset) -> Result<(), MpvError> {
        if let Ok(mut p) = self.inner.persisted.lock() {
            p.upscale_mode = preset;
        }
        let (scale, cscale, dscale) = preset.scalers();
        self.set_property_string("scale", scale)?;
        self.set_property_string("cscale", cscale)?;
        self.set_property_string("dscale", dscale)
    }

    pub fn upscale_preset(&self) -> UpscalePreset {
        self.inner
            .persisted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .upscale_mode
    }

    pub fn set_speed(&self, speed: f64) -> Result<(), MpvError> {
        if !speed.is_finite() {
            return Ok(());
        }
        let clamped = speed.clamp(0.25, 4.0);
        if let Ok(mut p) = self.inner.persisted.lock() {
            p.speed = clamped;
        }
        self.set_property_double("speed", clamped)
    }

    pub fn frame_step(&self) -> Result<(), MpvError> {
        self.command_async(&["frame-step"])
    }

    pub fn frame_back_step(&self) -> Result<(), MpvError> {
        self.command_async(&["frame-back-step"])
    }

    pub fn set_deband(&self, config: DebandConfig) -> Result<(), MpvError> {
        if let Ok(mut p) = self.inner.persisted.lock() {
            p.deband = config;
        }
        self.apply_deband(config)
    }

    fn apply_deband(&self, config: DebandConfig) -> Result<(), MpvError> {
        let iterations = i64::from(config.iterations.clamp(1, 16));
        let threshold = f64::from(config.threshold.clamp(1, 4096));
        let range = f64::from(config.range.clamp(1, 64));
        let grain = f64::from(config.grain.min(4096));

        self.set_property_flag("deband", config.enabled)?;
        self.set_property_i64("deband-iterations", iterations)?;
        self.set_property_double("deband-threshold", threshold)?;
        self.set_property_double("deband-range", range)?;
        self.set_property_double("deband-grain", grain)
    }

    pub fn deband_config(&self) -> DebandConfig {
        self.inner
            .persisted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .deband
    }

    pub fn set_shader_preset(&self, preset: ShaderPreset) -> Result<(), MpvError> {
        self.set_shader_preset_in_dir(preset, &default_shader_cache_dir())
    }

    pub fn set_shader_preset_in_dir(
        &self,
        preset: ShaderPreset,
        cache_dir: &Path,
    ) -> Result<(), MpvError> {
        let materialized = materialize_shader_preset_in_dir(preset, cache_dir)?;
        if let Ok(mut p) = self.inner.persisted.lock() {
            p.shader_preset = preset;
            p.shader_path = materialized.clone();
        }
        match materialized {
            Some(path) => self.set_property_string("glsl-shaders", &path.to_string_lossy()),
            None => self.set_property_string("glsl-shaders", ""),
        }
    }

    pub fn shader_preset(&self) -> ShaderPreset {
        self.inner
            .persisted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .shader_preset
    }

    pub fn set_hdr_backend_options(&self, opts: HdrBackendOptions) -> Result<(), MpvError> {
        if let Ok(mut p) = self.inner.persisted.lock() {
            p.hdr_backend = opts;
        }
        self.apply_hdr_backend_options(opts)
    }

    fn apply_hdr_backend_options(&self, opts: HdrBackendOptions) -> Result<(), MpvError> {
        let compute_peak = if opts.hdr_compute_peak { "yes" } else { "no" };
        let colorspace_hint = if opts.target_colorspace_hint {
            "yes"
        } else {
            "no"
        };
        let placebo_opts = if opts.dolby_vision_reshape {
            "apply_dolbyvision=yes"
        } else {
            ""
        };

        self.set_property_string("hdr-compute-peak", compute_peak)?;
        self.set_property_string("target-colorspace-hint", colorspace_hint)?;
        self.set_property_string("libplacebo-opts", placebo_opts)
    }

    pub fn hdr_backend_options(&self) -> HdrBackendOptions {
        self.inner
            .persisted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .hdr_backend
    }

    pub fn switch_stream_seamless(
        &self,
        new_url: impl AsRef<str>,
        resume_time_secs: f64,
    ) -> Result<(), MpvError> {
        let saved = self
            .inner
            .persisted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        let start_opt = if resume_time_secs.is_finite() && resume_time_secs > 0.0 {
            Some(resume_time_secs)
        } else {
            None
        };

        self.load_file_with_sub_delay(new_url.as_ref(), start_opt, Some(saved.sub_delay))?;

        self.set_property_double("volume", saved.volume)?;
        self.set_property_flag("mute", saved.muted)?;
        self.set_property_double("speed", saved.speed)?;
        self.set_property_double("sub-delay", saved.sub_delay)?;
        self.apply_subtitle_style(&saved.subtitle_style)?;
        self.set_property_string("tone-mapping", saved.tone_mapping.as_mpv_str())?;

        let (scale, cscale, dscale) = saved.upscale_mode.scalers();
        self.set_property_string("scale", scale)?;
        self.set_property_string("cscale", cscale)?;
        self.set_property_string("dscale", dscale)?;

        if saved.deband != DebandConfig::default() {
            self.apply_deband(saved.deband)?;
        }

        if saved.shader_preset != ShaderPreset::Off {
            let shader_path = match saved.shader_path {
                Some(p) => Some(p),
                None => materialize_shader_preset_in_dir(
                    saved.shader_preset,
                    &default_shader_cache_dir(),
                )?,
            };
            if let Some(path) = shader_path {
                self.set_property_string("glsl-shaders", &path.to_string_lossy())?;
            }
        }

        if saved.hdr_backend != HdrBackendOptions::default() {
            self.apply_hdr_backend_options(saved.hdr_backend)?;
        }

        Ok(())
    }

    pub fn set_viewport_rect(
        &self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        scale_factor: f32,
    ) {
        let w = width.max(1);
        let h = height.max(1);
        let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };

        {
            let mut vp = self
                .inner
                .viewport_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if vp.x == x
                && vp.y == y
                && vp.width == w
                && vp.height == h
                && (vp.scale_factor - scale).abs() < 1e-4
            {
                return;
            }
            vp.x = x;
            vp.y = y;
            vp.width = w;
            vp.height = h;
            vp.scale_factor = scale;
        }

        let worker_guard = self
            .inner
            .render_worker
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(worker) = worker_guard.as_ref() {
            if let Ok(mut shared) = worker.coordinator.mutex.lock() {
                shared.viewport.x = x;
                shared.viewport.y = y;
                shared.viewport.width = w;
                shared.viewport.height = h;
                shared.viewport.scale_factor = scale;
                shared.geometry_dirty = true;
            }
            worker.coordinator.condvar.notify_one();
        }
        #[cfg(target_os = "windows")]
        {
            drop(worker_guard);
            self.inner
                .dcomp
                .sync_from_cached(&self.inner.viewport_cache, &self.inner.observer);
        }
    }

    pub fn set_visible(&self, visible: bool) {
        {
            let mut vp = self
                .inner
                .viewport_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if vp.visible == visible {
                return;
            }
            vp.visible = visible;
        }

        if !visible {
            clear_software_video_state(&self.inner.sw_video, false);
        }

        let worker_guard = self
            .inner
            .render_worker
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(worker) = worker_guard.as_ref() {
            if let Ok(mut shared) = worker.coordinator.mutex.lock() {
                shared.viewport.visible = visible;
                shared.visibility_dirty = true;
            }
            worker.coordinator.condvar.notify_one();
        }
        #[cfg(target_os = "windows")]
        {
            drop(worker_guard);
            self.inner
                .dcomp
                .sync_from_cached(&self.inner.viewport_cache, &self.inner.observer);
        }
    }

    fn next_userdata(&self) -> u64 {
        self.inner.next_req_id.fetch_add(1, Ordering::Relaxed)
    }

    fn command_async(&self, args: &[&str]) -> Result<(), MpvError> {
        if self.inner.terminated.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut c_strings = Vec::with_capacity(args.len());
        for &arg in args {
            c_strings.push(
                CString::new(arg)
                    .map_err(|_| MpvError::Command("command argument contains interior NUL".into()))?,
            );
        }
        let mut ptrs: Vec<*const c_char> = c_strings.iter().map(|s| s.as_ptr()).collect();
        ptrs.push(ptr::null());

        let mpv_guard = self
            .inner
            .mpv_slot
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let Some(mpv_raw) = *mpv_guard else {
            return Ok(());
        };
        let rc = unsafe { mpv_command_async(mpv_raw.0, self.next_userdata(), ptrs.as_ptr()) };
        drop(mpv_guard);
        if rc < 0 {
            let cmd_name = args.first().copied().unwrap_or("unknown");
            Err(MpvError::Command(format!(
                "mpv_command_async({cmd_name}) failed: {}",
                error_to_string(rc)
            )))
        } else {
            Ok(())
        }
    }

    fn set_property_flag(&self, name: &str, value: bool) -> Result<(), MpvError> {
        if self.inner.terminated.load(Ordering::Acquire) {
            return Ok(());
        }
        let c_name = CString::new(name)
            .map_err(|_| MpvError::Property("property name contains NUL".into()))?;
        let mut flag: c_int = if value { 1 } else { 0 };
        let mpv_guard = self
            .inner
            .mpv_slot
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let Some(mpv_raw) = *mpv_guard else {
            return Ok(());
        };
        let rc = unsafe {
            mpv_set_property_async(
                mpv_raw.0,
                self.next_userdata(),
                c_name.as_ptr(),
                MPV_FORMAT_FLAG,
                &mut flag as *mut c_int as *mut c_void,
            )
        };
        drop(mpv_guard);
        check_prop_rc(name, rc)
    }

    fn set_property_i64(&self, name: &str, mut value: i64) -> Result<(), MpvError> {
        if self.inner.terminated.load(Ordering::Acquire) {
            return Ok(());
        }
        let c_name = CString::new(name)
            .map_err(|_| MpvError::Property("property name contains NUL".into()))?;
        let mpv_guard = self
            .inner
            .mpv_slot
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let Some(mpv_raw) = *mpv_guard else {
            return Ok(());
        };
        let rc = unsafe {
            mpv_set_property_async(
                mpv_raw.0,
                self.next_userdata(),
                c_name.as_ptr(),
                MPV_FORMAT_INT64,
                &mut value as *mut i64 as *mut c_void,
            )
        };
        drop(mpv_guard);
        check_prop_rc(name, rc)
    }

    fn set_property_double(&self, name: &str, mut value: f64) -> Result<(), MpvError> {
        if self.inner.terminated.load(Ordering::Acquire) {
            return Ok(());
        }
        let c_name = CString::new(name)
            .map_err(|_| MpvError::Property("property name contains NUL".into()))?;
        let mpv_guard = self
            .inner
            .mpv_slot
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let Some(mpv_raw) = *mpv_guard else {
            return Ok(());
        };
        let rc = unsafe {
            mpv_set_property_async(
                mpv_raw.0,
                self.next_userdata(),
                c_name.as_ptr(),
                MPV_FORMAT_DOUBLE,
                &mut value as *mut f64 as *mut c_void,
            )
        };
        drop(mpv_guard);
        check_prop_rc(name, rc)
    }

    fn set_property_string(&self, name: &str, value: &str) -> Result<(), MpvError> {
        if self.inner.terminated.load(Ordering::Acquire) {
            return Ok(());
        }
        let c_name = CString::new(name)
            .map_err(|_| MpvError::Property("property name contains NUL".into()))?;
        let c_val = CString::new(value)
            .map_err(|_| MpvError::Property("property value contains NUL".into()))?;
        let mut val_ptr: *const c_char = c_val.as_ptr();
        let mpv_guard = self
            .inner
            .mpv_slot
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let Some(mpv_raw) = *mpv_guard else {
            return Ok(());
        };
        let rc = unsafe {
            mpv_set_property_async(
                mpv_raw.0,
                self.next_userdata(),
                c_name.as_ptr(),
                MPV_FORMAT_STRING,
                &mut val_ptr as *mut *const c_char as *mut c_void,
            )
        };
        drop(mpv_guard);
        check_prop_rc(name, rc)
    }
}

fn check_prop_rc(name: &str, rc: c_int) -> Result<(), MpvError> {
    if rc < 0 {
        Err(MpvError::Property(format!(
            "mpv_set_property_async({name}) failed: {}",
            error_to_string(rc)
        )))
    } else {
        Ok(())
    }
}

unsafe fn set_option_str(mpv: *mut mpv_handle, key: &str, val: &str) -> Result<(), MpvError> {
    let ck = CString::new(key).map_err(|_| MpvError::Init("option key contains NUL".into()))?;
    let cv = CString::new(val).map_err(|_| MpvError::Init("option val contains NUL".into()))?;
    let rc = mpv_set_option_string(mpv, ck.as_ptr(), cv.as_ptr());
    if rc < 0 {
        Err(MpvError::Init(format!(
            "mpv_set_option_string({key}) failed: {}",
            error_to_string(rc)
        )))
    } else {
        Ok(())
    }
}

unsafe fn observe_all_properties(mpv: *mut mpv_handle) -> Result<(), MpvError> {
    let specs: &[(u64, &CStr, c_int)] = &[
        (PROP_TIME_POS, c"time-pos", MPV_FORMAT_DOUBLE),
        (PROP_DURATION, c"duration", MPV_FORMAT_DOUBLE),
        (PROP_PAUSE, c"pause", MPV_FORMAT_FLAG),
        (PROP_CORE_IDLE, c"core-idle", MPV_FORMAT_FLAG),
        (PROP_PAUSED_FOR_CACHE, c"paused-for-cache", MPV_FORMAT_FLAG),
        (PROP_SEEKING, c"seeking", MPV_FORMAT_FLAG),
        (
            PROP_DEMUXER_CACHE_DURATION,
            c"demuxer-cache-duration",
            MPV_FORMAT_DOUBLE,
        ),
        (PROP_HWDEC_CURRENT, c"hwdec-current", MPV_FORMAT_STRING),
        (PROP_VIDEO_FORMAT, c"video-format", MPV_FORMAT_STRING),
        (PROP_VIDEO_CODEC, c"video-codec", MPV_FORMAT_STRING),
        (PROP_VIDEO_WIDTH, c"width", MPV_FORMAT_INT64),
        (PROP_VIDEO_HEIGHT, c"height", MPV_FORMAT_INT64),
        (PROP_VOLUME, c"volume", MPV_FORMAT_DOUBLE),
        (PROP_MUTE, c"mute", MPV_FORMAT_FLAG),
        (PROP_SUB_DELAY, c"sub-delay", MPV_FORMAT_DOUBLE),
        (PROP_TRACK_LIST, c"track-list", MPV_FORMAT_NODE),
        (PROP_EOF_REACHED, c"eof-reached", MPV_FORMAT_FLAG),
        (PROP_SPEED, c"speed", MPV_FORMAT_DOUBLE),
        (PROP_CACHE_SPEED, c"cache-speed", MPV_FORMAT_INT64),
        (PROP_IDLE_ACTIVE, c"idle-active", MPV_FORMAT_FLAG),
    ];

    for &(tag, name, fmt) in specs {
        let rc = mpv_observe_property(mpv, tag, name.as_ptr(), fmt);
        if rc < 0 {
            return Err(MpvError::Init(format!(
                "mpv_observe_property({}) failed: {}",
                name.to_string_lossy(),
                error_to_string(rc)
            )));
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
unsafe fn read_display_swapchain_ptr(mpv: *mut mpv_handle) -> usize {
    let mut out_ptr: *mut c_char = ptr::null_mut();
    let rc = mpv_get_property(
        mpv,
        c"display-swapchain".as_ptr(),
        MPV_FORMAT_STRING,
        &mut out_ptr as *mut *mut c_char as *mut c_void,
    );
    if rc >= 0 && !out_ptr.is_null() {
        let text = CStr::from_ptr(out_ptr).to_string_lossy();
        let parsed = parse_swapchain_ptr_str(text.as_ref()).unwrap_or(0);
        mpv_free(out_ptr as *mut c_void);
        parsed
    } else {
        0
    }
}

fn run_event_loop(
    mpv: RawMpvPtr,
    observer: Arc<ObserverShared>,
    #[cfg(target_os = "windows")] dcomp: Arc<WindowsDCompCoordinator>,
    #[cfg(target_os = "windows")] viewport_cache: Arc<Mutex<ViewportState>>,
) {
    let mut paused_for_cache = false;
    let mut seeking = false;

    loop {
        if observer.shutdown.load(Ordering::Acquire) {
            break;
        }

        let ev_ptr = unsafe { mpv_wait_event(mpv.0, -1.0) };
        if observer.shutdown.load(Ordering::Acquire) || ev_ptr.is_null() {
            break;
        }

        let ev = unsafe { *ev_ptr };
        match ev.event_id {
            MPV_EVENT_NONE => continue,
            MPV_EVENT_SHUTDOWN => break,
            MPV_EVENT_LOG_MESSAGE => {
                if !ev.data.is_null() {
                    unsafe {
                        let msg = &*(ev.data as *const mpv_event_log_message);
                        let prefix = if msg.prefix.is_null() {
                            "mpv"
                        } else {
                            CStr::from_ptr(msg.prefix).to_str().unwrap_or("mpv")
                        };
                        let text = if msg.text.is_null() {
                            ""
                        } else {
                            CStr::from_ptr(msg.text).to_str().unwrap_or("").trim_end()
                        };
                        if !text.is_empty() {
                            let sanitized = redact_log_urls(text);
                            tracing::debug!(target: "gpui_mpv::libmpv", "[{prefix}] {sanitized}");
                        }
                    }
                }
            }
            MPV_EVENT_COMMAND_REPLY => {
                if ev.error < 0 {
                    tracing::warn!(
                        target: "gpui_mpv::libmpv",
                        userdata = ev.reply_userdata,
                        "mpv async command returned error: {}",
                        error_to_string(ev.error)
                    );
                }
            }
            MPV_EVENT_START_FILE => {
                let mut snap = observer
                    .snapshot
                    .write()
                    .unwrap_or_else(|e| e.into_inner());
                snap.eof_reached = false;
                snap.buffering = true;
                snap.cache_speed_bytes = 0;
                snap.video_format.clear();
                let cloned = snap.clone();
                drop(snap);
                let _ = observer.watch_tx.send(cloned.clone());
                let _ = observer
                    .event_tx
                    .try_send(PlayerEvent::SnapshotChanged(cloned));
            }
            MPV_EVENT_FILE_LOADED | MPV_EVENT_PLAYBACK_RESTART => {
                {
                    let mut snap = observer
                        .snapshot
                        .write()
                        .unwrap_or_else(|e| e.into_inner());
                    let new_buf = paused_for_cache || seeking;
                    if snap.buffering != new_buf {
                        snap.buffering = new_buf;
                        let cloned = snap.clone();
                        drop(snap);
                        let _ = observer.watch_tx.send(cloned.clone());
                        let _ = observer
                            .event_tx
                            .try_send(PlayerEvent::SnapshotChanged(cloned));
                    }
                }
                if ev.event_id == MPV_EVENT_FILE_LOADED {
                    let _ = observer.event_tx.try_send(PlayerEvent::FileLoaded);
                }
            }
            #[cfg(target_os = "windows")]
            MPV_EVENT_VIDEO_RECONFIG => {
                if dcomp.mode() == DCompBackendMode::DirectSwapChainVisual {
                    let queried = unsafe { read_display_swapchain_ptr(mpv.0) };
                    if queried != 0 {
                        dcomp.swapchain_ptr.store(queried, Ordering::Release);
                    }
                    dcomp.sync_from_cached(&viewport_cache, &observer);
                }
            }
            MPV_EVENT_END_FILE => {
                if !ev.data.is_null() {
                    let end = unsafe { *(ev.data as *const mpv_event_end_file) };
                    let eof = end.reason == MPV_END_FILE_REASON_EOF;
                    let error = if end.reason == MPV_END_FILE_REASON_ERROR || end.error < 0 {
                        Some(error_to_string(end.error))
                    } else {
                        None
                    };
                    paused_for_cache = false;
                    seeking = false;
                    {
                        let mut snap = observer
                            .snapshot
                            .write()
                            .unwrap_or_else(|e| e.into_inner());
                        snap.buffering = false;
                        snap.cache_speed_bytes = 0;
                        if eof {
                            snap.eof_reached = true;
                        }
                        let cloned = snap.clone();
                        drop(snap);
                        let _ = observer.watch_tx.send(cloned.clone());
                        let _ = observer
                            .event_tx
                            .try_send(PlayerEvent::SnapshotChanged(cloned));
                    }
                    let _ = observer
                        .event_tx
                        .try_send(PlayerEvent::EndFile { eof, error });
                }
            }
            MPV_EVENT_PROPERTY_CHANGE => {
                if ev.data.is_null() {
                    continue;
                }
                let prop = unsafe { *(ev.data as *const mpv_event_property) };
                let mut changed = false;
                #[cfg(target_os = "windows")]
                let mut dcomp_resync = false;

                {
                    let mut snap = observer
                        .snapshot
                        .write()
                        .unwrap_or_else(|e| e.into_inner());

                    unsafe {
                        match ev.reply_userdata {
                            PROP_TIME_POS => {
                                let v = read_prop_f64(&prop).unwrap_or(0.0);
                                if (snap.time_pos - v).abs() >= 0.015 {
                                    snap.time_pos = v;
                                    changed = true;
                                }
                            }
                            PROP_DURATION => {
                                let v = read_prop_f64(&prop).unwrap_or(0.0);
                                if (snap.duration - v).abs() >= 0.001 {
                                    snap.duration = v;
                                    changed = true;
                                }
                            }
                            PROP_PAUSE => {
                                if let Some(v) = read_prop_bool(&prop) {
                                    if snap.paused != v {
                                        snap.paused = v;
                                        changed = true;
                                    }
                                }
                            }
                            PROP_CORE_IDLE => {
                                if let Some(v) = read_prop_bool(&prop) {
                                    if snap.core_idle != v {
                                        snap.core_idle = v;
                                        changed = true;
                                    }
                                }
                            }
                            PROP_PAUSED_FOR_CACHE => {
                                paused_for_cache = read_prop_bool(&prop).unwrap_or(false);
                                let buf = paused_for_cache || seeking;
                                if snap.buffering != buf {
                                    snap.buffering = buf;
                                    changed = true;
                                }
                            }
                            PROP_SEEKING => {
                                seeking = read_prop_bool(&prop).unwrap_or(false);
                                let buf = paused_for_cache || seeking;
                                if snap.buffering != buf {
                                    snap.buffering = buf;
                                    changed = true;
                                }
                            }
                            PROP_DEMUXER_CACHE_DURATION => {
                                let v = read_prop_f64(&prop).unwrap_or(0.0);
                                if (snap.demuxer_cache_duration - v).abs() >= 0.05 {
                                    snap.demuxer_cache_duration = v;
                                    changed = true;
                                }
                            }
                            PROP_HWDEC_CURRENT => {
                                let v = read_prop_string(&prop).unwrap_or_default();
                                if snap.hwdec_current != v {
                                    snap.hwdec_current = v;
                                    changed = true;
                                }
                            }
                            PROP_VIDEO_FORMAT => {
                                let v = read_prop_string(&prop).unwrap_or_default();
                                if snap.video_format != v {
                                    snap.video_format = v;
                                    changed = true;
                                }
                            }
                            PROP_VIDEO_CODEC => {
                                let v = read_prop_string(&prop).unwrap_or_default();
                                if snap.video_codec != v {
                                    snap.video_codec = v;
                                    changed = true;
                                }
                            }
                            PROP_VIDEO_WIDTH => {
                                let v = read_prop_i64(&prop).unwrap_or(0);
                                if snap.video_width != v {
                                    snap.video_width = v;
                                    changed = true;
                                    #[cfg(target_os = "windows")]
                                    {
                                        dcomp_resync = true;
                                    }
                                }
                            }
                            PROP_VIDEO_HEIGHT => {
                                let v = read_prop_i64(&prop).unwrap_or(0);
                                if snap.video_height != v {
                                    snap.video_height = v;
                                    changed = true;
                                    #[cfg(target_os = "windows")]
                                    {
                                        dcomp_resync = true;
                                    }
                                }
                            }
                            PROP_VOLUME => {
                                if let Some(v) = read_prop_f64(&prop) {
                                    if (snap.volume - v).abs() >= 0.05 {
                                        snap.volume = v;
                                        changed = true;
                                    }
                                }
                            }
                            PROP_MUTE => {
                                if let Some(v) = read_prop_bool(&prop) {
                                    if snap.muted != v {
                                        snap.muted = v;
                                        changed = true;
                                    }
                                }
                            }
                            PROP_SUB_DELAY => {
                                let v = read_prop_f64(&prop).unwrap_or(0.0);
                                if (snap.sub_delay - v).abs() >= 0.001 {
                                    snap.sub_delay = v;
                                    changed = true;
                                }
                            }
                            PROP_TRACK_LIST => {
                                let tracks = if prop.format == MPV_FORMAT_NODE && !prop.data.is_null()
                                {
                                    parse_track_list_node(&*(prop.data as *const mpv_node))
                                } else {
                                    Vec::new()
                                };
                                if snap.tracks != tracks {
                                    snap.tracks = tracks;
                                    changed = true;
                                }
                            }
                            PROP_EOF_REACHED => {
                                let v = read_prop_bool(&prop).unwrap_or(false);
                                if snap.eof_reached != v {
                                    snap.eof_reached = v;
                                    if v {
                                        snap.cache_speed_bytes = 0;
                                    }
                                    changed = true;
                                }
                            }
                            PROP_SPEED => {
                                if let Some(v) = read_prop_f64(&prop) {
                                    if (snap.speed - v).abs() >= 0.005 {
                                        snap.speed = v;
                                        changed = true;
                                    }
                                }
                            }
                            PROP_CACHE_SPEED => {
                                let v = read_prop_i64(&prop).unwrap_or(0).max(0);
                                let zero_transition =
                                    (v == 0 && snap.cache_speed_bytes != 0)
                                        || (snap.cache_speed_bytes == 0 && v > 0);
                                if zero_transition || (snap.cache_speed_bytes - v).abs() >= 4_096 {
                                    snap.cache_speed_bytes = v;
                                    changed = true;
                                }
                            }
                            PROP_IDLE_ACTIVE
                                if read_prop_bool(&prop) == Some(true)
                                    && snap.cache_speed_bytes != 0 =>
                            {
                                snap.cache_speed_bytes = 0;
                                changed = true;
                            }
                            #[cfg(target_os = "windows")]
                            PROP_DISPLAY_SWAPCHAIN => {
                                let ptr_val = read_prop_string(&prop)
                                    .as_deref()
                                    .and_then(parse_swapchain_ptr_str)
                                    .unwrap_or(0);
                                if dcomp.swapchain_ptr.swap(ptr_val, Ordering::AcqRel) != ptr_val {
                                    dcomp_resync = true;
                                }
                            }
                            _ => {}
                        }
                    }

                    if changed {
                        let cloned = snap.clone();
                        drop(snap);
                        let _ = observer.watch_tx.send(cloned.clone());
                        let _ = observer
                            .event_tx
                            .try_send(PlayerEvent::SnapshotChanged(cloned));
                    } else {
                        drop(snap);
                    }

                    #[cfg(target_os = "windows")]
                    if dcomp_resync && dcomp.mode() == DCompBackendMode::DirectSwapChainVisual {
                        dcomp.sync_from_cached(&viewport_cache, &observer);
                    }
                }
            }
            _ => {}
        }
    }
}

unsafe fn read_prop_f64(prop: &mpv_event_property) -> Option<f64> {
    if prop.format == MPV_FORMAT_DOUBLE && !prop.data.is_null() {
        Some(*(prop.data as *const f64))
    } else {
        None
    }
}

unsafe fn read_prop_i64(prop: &mpv_event_property) -> Option<i64> {
    if prop.format == MPV_FORMAT_INT64 && !prop.data.is_null() {
        Some(*(prop.data as *const i64))
    } else {
        None
    }
}

unsafe fn read_prop_bool(prop: &mpv_event_property) -> Option<bool> {
    if prop.format == MPV_FORMAT_FLAG && !prop.data.is_null() {
        Some(*(prop.data as *const c_int) != 0)
    } else {
        None
    }
}

unsafe fn read_prop_string(prop: &mpv_event_property) -> Option<String> {
    if prop.format == MPV_FORMAT_STRING && !prop.data.is_null() {
        let str_ptr = *(prop.data as *const *const c_char);
        if !str_ptr.is_null() {
            return Some(CStr::from_ptr(str_ptr).to_string_lossy().into_owned());
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn run_wayland_render_loop(
    mpv: RawMpvPtr,
    wl_display: *mut c_void,
    parent_wl_surface: *mut c_void,
    coordinator: Arc<RenderCoordinator>,
    init_tx: std::sync::mpsc::SyncSender<Result<(), MpvError>>,
) {
    unsafe {
        let init_vp = coordinator
            .mutex
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .viewport;

        let mut subsurface = match WaylandVideoSubsurface::new(
            wl_display,
            parent_wl_surface,
            init_vp.x,
            init_vp.y,
            init_vp.width,
            init_vp.height,
            init_vp.scale_factor,
        ) {
            Ok(s) => s,
            Err(err) => {
                let _ = init_tx.send(Err(err));
                return;
            }
        };

        let mut gl_init = mpv_opengl_init_params {
            get_proc_address: Some(egl_get_proc_address),
            get_proc_address_ctx: ptr::null_mut(),
        };
        let mut advanced_control: c_int = 1;
        let mut create_params = [
            mpv_render_param {
                type_: MPV_RENDER_PARAM_API_TYPE,
                data: MPV_RENDER_API_TYPE_OPENGL.as_ptr() as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: &mut gl_init as *mut mpv_opengl_init_params as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_WL_DISPLAY,
                data: subsurface.wl_display_ptr(),
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_ADVANCED_CONTROL,
                data: &mut advanced_control as *mut c_int as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];

        let mut render_ctx: *mut mpv_render_context = ptr::null_mut();
        let rc = mpv_render_context_create(&mut render_ctx, mpv.0, create_params.as_mut_ptr());
        if rc < 0 || render_ctx.is_null() {
            let msg = if mpv_error_string(rc).is_null() {
                format!("code {rc}")
            } else {
                error_to_string(rc)
            };
            let _ = init_tx.send(Err(MpvError::Render(format!(
                "mpv_render_context_create failed: {msg}"
            ))));
            return;
        }

        let cb_raw = Arc::into_raw(Arc::clone(&coordinator)) as *mut c_void;
        mpv_render_context_set_update_callback(render_ctx, Some(on_mpv_render_update), cb_raw);

        let _ = init_tx.send(Ok(()));

        loop {
            let (vp, geom_dirty, vis_dirty, shutdown) = {
                let mut guard = coordinator
                    .mutex
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                while !guard.shutdown
                    && !guard.geometry_dirty
                    && !guard.visibility_dirty
                    && !coordinator.frame_ready.load(Ordering::Acquire)
                {
                    guard = coordinator
                        .condvar
                        .wait(guard)
                        .unwrap_or_else(|e| e.into_inner());
                }
                let snap_vp = guard.viewport;
                let gd = core::mem::replace(&mut guard.geometry_dirty, false);
                let vd = core::mem::replace(&mut guard.visibility_dirty, false);
                let sd = guard.shutdown;
                (snap_vp, gd, vd, sd)
            };

            if shutdown {
                break;
            }

            subsurface.dispatch_pending_queue();

            let resized = if geom_dirty {
                subsurface.set_geometry(vp.x, vp.y, vp.width, vp.height, vp.scale_factor)
            } else {
                false
            };

            let had_update = coordinator.frame_ready.swap(false, Ordering::AcqRel);
            let update_flags = if had_update {
                mpv_render_context_update(render_ctx)
            } else {
                0
            };
            let wants_new_frame = (update_flags & MPV_RENDER_UPDATE_FRAME) != 0;

            if !vp.visible {
                if vis_dirty {
                    subsurface.clear_transparent_and_swap();
                }
                if wants_new_frame {
                    let mut skip: c_int = 1;
                    let mut skip_params = [
                        mpv_render_param {
                            type_: MPV_RENDER_PARAM_SKIP_RENDERING,
                            data: &mut skip as *mut c_int as *mut c_void,
                        },
                        mpv_render_param {
                            type_: MPV_RENDER_PARAM_INVALID,
                            data: ptr::null_mut(),
                        },
                    ];
                    let _ = mpv_render_context_render(render_ctx, skip_params.as_mut_ptr());
                    mpv_render_context_report_swap(render_ctx);
                }
                continue;
            }

            if wants_new_frame || resized || vis_dirty {
                let (bw, bh) = subsurface.buffer_size();
                let mut fbo = mpv_opengl_fbo {
                    fbo: 0,
                    w: bw,
                    h: bh,
                    internal_format: 0,
                };
                let mut flip_y: c_int = 1;
                let mut block_for_target: c_int = if wants_new_frame { 1 } else { 0 };
                let mut render_params = [
                    mpv_render_param {
                        type_: MPV_RENDER_PARAM_OPENGL_FBO,
                        data: &mut fbo as *mut mpv_opengl_fbo as *mut c_void,
                    },
                    mpv_render_param {
                        type_: MPV_RENDER_PARAM_FLIP_Y,
                        data: &mut flip_y as *mut c_int as *mut c_void,
                    },
                    mpv_render_param {
                        type_: MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME,
                        data: &mut block_for_target as *mut c_int as *mut c_void,
                    },
                    mpv_render_param {
                        type_: MPV_RENDER_PARAM_INVALID,
                        data: ptr::null_mut(),
                    },
                ];

                if mpv_render_context_render(render_ctx, render_params.as_mut_ptr()) >= 0 {
                    subsurface.swap_buffers();
                    mpv_render_context_report_swap(render_ctx);
                }
            }
        }

        let _ = subsurface.make_current();
        mpv_render_context_set_update_callback(render_ctx, None, ptr::null_mut());
        mpv_render_context_free(render_ctx);
        let _ = Arc::from_raw(cb_raw as *const RenderCoordinator);
        subsurface.clear_transparent_and_swap();
        drop(subsurface);
        crate::trim_process_heap();
    }
}

fn run_software_render_loop(
    mpv: RawMpvPtr,
    coordinator: Arc<RenderCoordinator>,
    sw_video: Arc<Mutex<SoftwareVideoState>>,
    sw_frame_tx: watch::Sender<u64>,
    init_tx: std::sync::mpsc::SyncSender<Result<(), MpvError>>,
) {
    unsafe {
        let mut advanced_control: c_int = 1;
        let mut create_params = [
            mpv_render_param {
                type_: MPV_RENDER_PARAM_API_TYPE,
                data: MPV_RENDER_API_TYPE_SW.as_ptr() as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_ADVANCED_CONTROL,
                data: &mut advanced_control as *mut c_int as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];

        let mut render_ctx: *mut mpv_render_context = ptr::null_mut();
        let rc = mpv_render_context_create(&mut render_ctx, mpv.0, create_params.as_mut_ptr());
        if rc < 0 || render_ctx.is_null() {
            let msg = if mpv_error_string(rc).is_null() {
                format!("code {rc}")
            } else {
                error_to_string(rc)
            };
            let _ = init_tx.send(Err(MpvError::Render(format!(
                "mpv_render_context_create(SW) failed: {msg}"
            ))));
            return;
        }

        let cb_raw = Arc::into_raw(Arc::clone(&coordinator)) as *mut c_void;
        mpv_render_context_set_update_callback(render_ctx, Some(on_mpv_render_update), cb_raw);

        let _ = init_tx.send(Ok(()));

        let mut scratch: Vec<Aligned64> = Vec::new();
        let mut has_rendered_frame = false;

        loop {
            let (vp, geom_dirty, vis_dirty, shutdown) = {
                let mut guard = coordinator
                    .mutex
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                while !guard.shutdown
                    && !guard.geometry_dirty
                    && !guard.visibility_dirty
                    && !coordinator.frame_ready.load(Ordering::Acquire)
                {
                    guard = coordinator
                        .condvar
                        .wait(guard)
                        .unwrap_or_else(|e| e.into_inner());
                }
                let snap_vp = guard.viewport;
                let gd = core::mem::replace(&mut guard.geometry_dirty, false);
                let vd = core::mem::replace(&mut guard.visibility_dirty, false);
                let sd = guard.shutdown;
                (snap_vp, gd, vd, sd)
            };

            if shutdown {
                break;
            }

            let had_update = coordinator.frame_ready.swap(false, Ordering::AcqRel);
            let update_flags = if had_update {
                mpv_render_context_update(render_ctx)
            } else {
                0
            };
            let wants_new_frame = (update_flags & MPV_RENDER_UPDATE_FRAME) != 0;

            if !vp.visible {
                if vis_dirty {
                    clear_software_video_state(&sw_video, false);
                    sw_frame_tx.send_modify(|seq| *seq = seq.wrapping_add(1));
                }
                if wants_new_frame {
                    let mut skip: c_int = 1;
                    let mut skip_params = [
                        mpv_render_param {
                            type_: MPV_RENDER_PARAM_SKIP_RENDERING,
                            data: &mut skip as *mut c_int as *mut c_void,
                        },
                        mpv_render_param {
                            type_: MPV_RENDER_PARAM_INVALID,
                            data: ptr::null_mut(),
                        },
                    ];
                    let _ = mpv_render_context_render(render_ctx, skip_params.as_mut_ptr());
                    mpv_render_context_report_swap(render_ctx);
                }
                continue;
            }

            if wants_new_frame || ((geom_dirty || vis_dirty) && has_rendered_frame) {
                let (sw_w, sw_h) =
                    compute_software_canvas_size(vp.width, vp.height, vp.scale_factor);
                let w = sw_w as usize;
                let h = sw_h as usize;
                let stride: usize = ((w * 4) + 63) & !63;
                let total_aligned_bytes = stride * h;
                let blocks = total_aligned_bytes.div_ceil(64);
                if scratch.len() < blocks {
                    scratch.resize(blocks, Aligned64([0u8; 64]));
                }

                let mut sw_size: [c_int; 2] = [sw_w as c_int, sw_h as c_int];
                let mut sw_stride: usize = stride;
                let mut block_for_target: c_int = if wants_new_frame { 1 } else { 0 };
                let scratch_ptr = scratch.as_mut_ptr() as *mut u8;

                let mut render_params = [
                    mpv_render_param {
                        type_: MPV_RENDER_PARAM_SW_SIZE,
                        data: sw_size.as_mut_ptr() as *mut c_void,
                    },
                    mpv_render_param {
                        type_: MPV_RENDER_PARAM_SW_FORMAT,
                        data: SW_FORMAT_BGR0.as_ptr() as *mut c_void,
                    },
                    mpv_render_param {
                        type_: MPV_RENDER_PARAM_SW_STRIDE,
                        data: &mut sw_stride as *mut usize as *mut c_void,
                    },
                    mpv_render_param {
                        type_: MPV_RENDER_PARAM_SW_POINTER,
                        data: scratch_ptr as *mut c_void,
                    },
                    mpv_render_param {
                        type_: MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME,
                        data: &mut block_for_target as *mut c_int as *mut c_void,
                    },
                    mpv_render_param {
                        type_: MPV_RENDER_PARAM_INVALID,
                        data: ptr::null_mut(),
                    },
                ];

                if mpv_render_context_render(render_ctx, render_params.as_mut_ptr()) >= 0 {
                    let row_bytes = w * 4;
                    let mut bgra = vec![0u8; row_bytes * h];
                    let src_slice = core::slice::from_raw_parts(scratch_ptr, total_aligned_bytes);
                    for y in 0..h {
                        let src_row = &src_slice[y * stride..y * stride + row_bytes];
                        let dst_row = &mut bgra[y * row_bytes..(y + 1) * row_bytes];
                        dst_row.copy_from_slice(src_row);
                        for px in dst_row.as_chunks_mut::<4>().0 {
                            px[3] = 0xFF;
                        }
                    }

                    if let Some(img_buf) = RgbaImage::from_raw(sw_w, sw_h, bgra) {
                        let render_image = Arc::new(RenderImage::new([Frame::new(img_buf)]));
                        sw_video
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .latest_frame = Some(render_image);
                        has_rendered_frame = true;
                        mpv_render_context_report_swap(render_ctx);
                        sw_frame_tx.send_modify(|seq| *seq = seq.wrapping_add(1));
                    } else {
                        mpv_render_context_report_swap(render_ctx);
                    }
                }
            }
        }

        mpv_render_context_set_update_callback(render_ctx, None, ptr::null_mut());
        mpv_render_context_free(render_ctx);
        let _ = Arc::from_raw(cb_raw as *const RenderCoordinator);
        drop(scratch);
        crate::trim_process_heap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mpv_ffi::{mpv_node_list, mpv_node_union, MPV_FORMAT_NODE_ARRAY, MPV_FORMAT_NODE_MAP};
    use std::time::{Duration, Instant};

    #[test]
    fn validate_stream_url_rejects_local_files_and_mpv_protocols() {
        assert_eq!(
            validate_stream_url("https://cdn.example.com/video.mkv").unwrap(),
            "https://cdn.example.com/video.mkv"
        );
        assert_eq!(
            validate_stream_url("http://127.0.0.1:11470/stream/0").unwrap(),
            "http://127.0.0.1:11470/stream/0"
        );
        assert_eq!(
            validate_stream_url("HTTPS://example.org/sub.vtt").unwrap(),
            "HTTPS://example.org/sub.vtt"
        );

        let rejected = [
            "",
            "   ",
            "file:///etc/passwd",
            "file://localhost/etc/passwd",
            "/etc/passwd",
            "C:\\Windows\\System32\\drivers\\etc\\hosts",
            "edl://!new_stream;file:///etc/passwd",
            "lavfi://[testsrc]",
            "archive:///tmp/evil.zip|/etc/shadow",
            "memory://payload",
            "fd://0",
            "--script=/tmp/evil.lua",
            "-o=/tmp/out.raw",
            "http:///missing-host",
            "https://example.com/evil\n--input-ipc-server=/tmp/pwn",
        ];
        for bad in rejected {
            assert!(
                validate_stream_url(bad).is_err(),
                "expected validate_stream_url({bad:?}) to be rejected"
            );
            assert!(
                validate_stream_url_impl(bad, false).is_err(),
                "expected production validate_stream_url_impl({bad:?}, false) to be rejected"
            );
        }

        let tmp_media = std::env::temp_dir().join("ferrite_test_clip.mp4");
        let tmp_str = tmp_media.to_string_lossy();
        assert!(validate_stream_url_impl(&tmp_str, false).is_err());
        assert!(validate_stream_url_impl("av://lavfi:color=c=red:s=64x64:d=1", false).is_err());
        assert!(validate_stream_url_impl(&tmp_str, true).is_ok());
        assert!(validate_stream_url_impl("av://lavfi:color=c=red:s=64x64:d=1", true).is_ok());

        let ctrl = MpvController::with_extra_options(&[("vo", "null"), ("ao", "null")])
            .expect("headless controller");
        assert!(ctrl.load_file("file:///etc/passwd", None).is_err());
        assert!(ctrl
            .load_file("edl://!new_stream;file:///etc/passwd", None)
            .is_err());
        assert!(ctrl
            .add_subtitle("file:///etc/passwd", Some("bad"), None)
            .is_err());
        ctrl.shutdown_blocking();
    }

    #[test]
    fn software_renderer_decodes_frames_and_evicts_superseded_atlas_images() {
        let ctrl = MpvController::with_extra_options(&[("ao", "null")])
            .expect("controller with default vo=libmpv");
        ctrl.set_viewport_rect(0, 0, 320, 180, 1.0);
        ctrl.attach_software_renderer()
            .expect("attach_software_renderer should succeed");

        let mut frame_rx = ctrl.subscribe_software_frames();
        ctrl.load_file("av://lavfi:testsrc=s=320x180:d=1.2:r=30", 0.0)
            .expect("load synthetic video into SW renderer");

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut first_latched: Option<Arc<RenderImage>> = None;
        while Instant::now() < deadline {
            if frame_rx.has_changed().unwrap_or(false) {
                let _ = frame_rx.borrow_and_update();
            }
            if let Some(img) = ctrl.latch_video_frame_for_paint() {
                first_latched = Some(img);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let first = first_latched.expect("expected first SW RenderImage frame");
        assert!(ctrl.take_evicted_video_frames().is_empty());

        let mut second_latched: Option<Arc<RenderImage>> = None;
        while Instant::now() < deadline {
            if let Some(img) = ctrl.latch_video_frame_for_paint() {
                if !Arc::ptr_eq(&first, &img) {
                    second_latched = Some(img);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(15));
        }

        let _second = second_latched.expect("expected subsequent distinct SW RenderImage frame");
        let evicted = ctrl.take_evicted_video_frames();
        assert_eq!(evicted.len(), 1);
        assert!(Arc::ptr_eq(&evicted[0], &first));
        assert!(ctrl.take_evicted_video_frames().is_empty());

        ctrl.shutdown_blocking();
        let post_shutdown_evicted = drain_evicted_video_images();
        assert!(!post_shutdown_evicted.is_empty());
    }

    #[test]
    fn mpv_controller_headless_lifecycle_and_property_observation() {
        let ctrl = MpvController::with_extra_options(&[("vo", "null"), ("ao", "null")])
            .expect("libmpv should initialize cleanly in headless mode");

        let mut rx = ctrl.subscribe();

        ctrl.set_volume(65.0).expect("set_volume");
        ctrl.set_muted(true).expect("set_muted");
        ctrl.set_paused(true).expect("set_paused");
        ctrl.set_subtitle_delay(1.25).expect("set_subtitle_delay");
        ctrl.set_hdr_tone_mapping(ToneMappingPreset::Bt2446a)
            .expect("set_hdr_tone_mapping");
        ctrl.set_upscale_mode(UpscalePreset::Spline36)
            .expect("set_upscale_mode");
        ctrl.select_subtitle_track(None)
            .expect("select_subtitle_track(None)");

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let snap = ctrl.snapshot();
            if (snap.volume - 65.0).abs() < 0.5
                && snap.muted
                && snap.paused
                && (snap.sub_delay - 1.25).abs() < 0.01
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let snap = ctrl.snapshot();
        assert!(
            (snap.volume - 65.0).abs() < 0.5,
            "expected volume ~65.0, got {}",
            snap.volume
        );
        assert!(snap.muted, "expected muted == true");
        assert!(snap.paused, "expected paused == true");
        assert!(
            (snap.sub_delay - 1.25).abs() < 0.01,
            "expected sub_delay ~1.25, got {}",
            snap.sub_delay
        );

        let watched = rx.borrow_and_update().clone();
        assert!(watched.muted);

        ctrl.toggle_pause().expect("toggle_pause");
        let deadline2 = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline2 {
            if !ctrl.snapshot().paused {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !ctrl.snapshot().paused,
            "expected paused == false after toggle"
        );

        ctrl.stop().expect("stop");
    }

    #[test]
    fn default_streaming_options_initialize_with_libmpv_2_5() {
        let api_ver = unsafe { mpv_ffi::mpv_client_api_version() };
        assert!(
            api_ver >= (2 << 16),
            "expected libmpv client API >= 2.0, got 0x{api_ver:x}"
        );

        let ctrl = MpvController::new().expect("default streaming MpvController::new() must succeed");
        ctrl.load_file("http://127.0.0.1:11470/test.mkv", 42.0)
            .expect("deferred load_file before subsurface attach");
        ctrl.add_subtitle(
            "http://127.0.0.1:11470/test.srt",
            Some("English"),
            Some("eng"),
        )
        .expect("deferred add_subtitle before subsurface attach");
        ctrl.set_viewport_rect(10, 20, 1280, 720, 1.5);
        ctrl.set_visible(false);
        ctrl.set_visible(true);
        ctrl.stop().expect("stop clears queued load");
    }

    #[test]
    fn synthetic_video_stream_plays_and_populates_tracks_and_dimensions() {
        let ctrl = MpvController::with_extra_options(&[("vo", "null"), ("ao", "null")])
            .expect("headless controller");
        let events = ctrl.subscribe_events();

        ctrl.load_file("av://lavfi:color=c=navy:s=320x180:d=0.4:r=25", 0.05)
            .expect("load synthetic lavfi video");

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut saw_file_loaded = false;
        while Instant::now() < deadline {
            while let Ok(ev) = events.try_recv() {
                if matches!(ev, PlayerEvent::FileLoaded) {
                    saw_file_loaded = true;
                }
            }
            let snap = ctrl.snapshot();
            if saw_file_loaded
                && snap.video_width == 320
                && snap.video_height == 180
                && snap.eof_reached
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }

        let snap = ctrl.snapshot();
        assert!(saw_file_loaded, "expected FileLoaded event from lavfi stream");
        assert_eq!((snap.video_width, snap.video_height), (320, 180));
        assert!(
            snap.tracks
                .iter()
                .any(|t| t.track_type == TrackType::Video && t.selected),
            "expected selected video track in snapshot: {:?}",
            snap.tracks
        );

        ctrl.seek_absolute(0.0).expect("seek_absolute");
        ctrl.seek_relative(0.1).expect("seek_relative");
        ctrl.stop().expect("stop");
    }

    #[test]
    fn track_list_mpv_node_parser_extracts_audio_sub_and_video_tracks() {
        unsafe {
            let mut k_id = c"id".to_bytes_with_nul().to_vec();
            let mut k_type = c"type".to_bytes_with_nul().to_vec();
            let mut k_title = c"title".to_bytes_with_nul().to_vec();
            let mut k_lang = c"lang".to_bytes_with_nul().to_vec();
            let mut k_codec = c"codec".to_bytes_with_nul().to_vec();
            let mut k_sel = c"selected".to_bytes_with_nul().to_vec();
            let mut k_ext = c"external".to_bytes_with_nul().to_vec();

            let mut v_audio = c"audio".to_bytes_with_nul().to_vec();
            let mut v_title = c"Surround 7.1".to_bytes_with_nul().to_vec();
            let mut v_lang = c"eng".to_bytes_with_nul().to_vec();
            let mut v_codec = c"truehd".to_bytes_with_nul().to_vec();

            let mut keys = [
                k_id.as_mut_ptr() as *mut c_char,
                k_type.as_mut_ptr() as *mut c_char,
                k_title.as_mut_ptr() as *mut c_char,
                k_lang.as_mut_ptr() as *mut c_char,
                k_codec.as_mut_ptr() as *mut c_char,
                k_sel.as_mut_ptr() as *mut c_char,
                k_ext.as_mut_ptr() as *mut c_char,
            ];
            let mut vals = [
                mpv_node {
                    u: mpv_node_union { int64: 2 },
                    format: MPV_FORMAT_INT64,
                },
                mpv_node {
                    u: mpv_node_union {
                        string: v_audio.as_mut_ptr() as *mut c_char,
                    },
                    format: MPV_FORMAT_STRING,
                },
                mpv_node {
                    u: mpv_node_union {
                        string: v_title.as_mut_ptr() as *mut c_char,
                    },
                    format: MPV_FORMAT_STRING,
                },
                mpv_node {
                    u: mpv_node_union {
                        string: v_lang.as_mut_ptr() as *mut c_char,
                    },
                    format: MPV_FORMAT_STRING,
                },
                mpv_node {
                    u: mpv_node_union {
                        string: v_codec.as_mut_ptr() as *mut c_char,
                    },
                    format: MPV_FORMAT_STRING,
                },
                mpv_node {
                    u: mpv_node_union { flag: 1 },
                    format: MPV_FORMAT_FLAG,
                },
                mpv_node {
                    u: mpv_node_union { flag: 0 },
                    format: MPV_FORMAT_FLAG,
                },
            ];

            let mut map_list = mpv_node_list {
                num: 7,
                values: vals.as_mut_ptr(),
                keys: keys.as_mut_ptr(),
            };
            let mut map_node = [mpv_node {
                u: mpv_node_union {
                    list: &mut map_list,
                },
                format: MPV_FORMAT_NODE_MAP,
            }];
            let mut root_list = mpv_node_list {
                num: 1,
                values: map_node.as_mut_ptr(),
                keys: ptr::null_mut(),
            };
            let root = mpv_node {
                u: mpv_node_union {
                    list: &mut root_list,
                },
                format: MPV_FORMAT_NODE_ARRAY,
            };

            let parsed = parse_track_list_node(&root);
            assert_eq!(
                parsed,
                vec![MediaTrack {
                    id: 2,
                    track_type: TrackType::Audio,
                    title: Some("Surround 7.1".into()),
                    lang: Some("eng".into()),
                    codec: Some("truehd".into()),
                    selected: true,
                    external: false,
                }]
            );
        }
    }

    #[test]
    fn phase_2_5_speed_frame_step_experimental_options_and_seamless_switch() {
        let ctrl = MpvController::with_extra_options(&[("vo", "null"), ("ao", "null")])
            .expect("headless controller");
        let events = ctrl.subscribe_events();

        assert_eq!(ctrl.deband_config(), DebandConfig::default());
        assert!(!ctrl.deband_config().enabled);
        assert_eq!(ctrl.shader_preset(), ShaderPreset::Off);
        assert_eq!(ctrl.hdr_backend_options(), HdrBackendOptions::default());
        assert!((ctrl.snapshot().speed - 1.0).abs() < 1e-6);

        let shader_dir =
            std::env::temp_dir().join(format!("ferrite-player-shader-{}", std::process::id()));

        ctrl.set_volume(72.0).expect("set_volume");
        ctrl.set_muted(true).expect("set_muted");
        ctrl.set_speed(2.0).expect("set_speed(2.0)");
        ctrl.set_subtitle_delay(-0.75).expect("set_subtitle_delay");
        ctrl.set_hdr_tone_mapping(ToneMappingPreset::Spline)
            .expect("set_hdr_tone_mapping");
        ctrl.set_upscale_mode(UpscalePreset::EwaLanczos)
            .expect("set_upscale_mode");
        ctrl.set_deband(DebandConfig::enabled_balanced())
            .expect("set_deband");
        for preset in ShaderPreset::ALL {
            ctrl.set_shader_preset_in_dir(preset, &shader_dir)
                .expect("set_shader_preset_in_dir");
        }
        ctrl.set_shader_preset_in_dir(ShaderPreset::Anime4kFast, &shader_dir)
            .expect("set Anime4kFast");
        ctrl.set_hdr_backend_options(HdrBackendOptions {
            dolby_vision_reshape: true,
            target_colorspace_hint: true,
            hdr_compute_peak: true,
        })
        .expect("set_hdr_backend_options(true)");

        ctrl.load_file("av://lavfi:testsrc=s=320x180:d=2.0:r=25", 0.0)
            .expect("load initial stream");

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut loaded_first = false;
        while Instant::now() < deadline {
            while let Ok(ev) = events.try_recv() {
                if matches!(ev, PlayerEvent::FileLoaded) {
                    loaded_first = true;
                }
            }
            let snap = ctrl.snapshot();
            if loaded_first
                && snap.video_width == 320
                && (snap.speed - 2.0).abs() < 0.05
                && (snap.sub_delay - (-0.75)).abs() < 0.02
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        assert!(loaded_first, "expected initial stream FileLoaded");
        assert!(
            (ctrl.snapshot().speed - 2.0).abs() < 0.05,
            "expected observed speed ~2.0, got {}",
            ctrl.snapshot().speed
        );

        ctrl.set_paused(true).expect("pause before frame-step");
        ctrl.frame_step().expect("frame_step");
        ctrl.frame_back_step().expect("frame_back_step");

        while events.try_recv().is_ok() {}
        ctrl.switch_stream_seamless("av://lavfi:testsrc=s=640x360:d=2.0:r=25", 0.40)
            .expect("switch_stream_seamless");

        let deadline2 = Instant::now() + Duration::from_secs(10);
        let mut loaded_second = false;
        while Instant::now() < deadline2 {
            while let Ok(ev) = events.try_recv() {
                if matches!(ev, PlayerEvent::FileLoaded) {
                    loaded_second = true;
                }
            }
            let snap = ctrl.snapshot();
            if loaded_second && snap.video_width == 640 && snap.video_height == 360 {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }

        std::thread::sleep(Duration::from_millis(50));
        let snap = ctrl.snapshot();
        assert!(loaded_second, "expected second stream FileLoaded");
        assert_eq!((snap.video_width, snap.video_height), (640, 360));
        assert!(
            (snap.volume - 72.0).abs() < 0.5,
            "volume preserved: {}",
            snap.volume
        );
        assert!(snap.muted, "muted preserved");
        assert!(
            (snap.speed - 2.0).abs() < 0.05,
            "speed preserved: {}",
            snap.speed
        );
        assert!(
            (snap.sub_delay - (-0.75)).abs() < 0.02,
            "sub_delay preserved across seamless stream switch: {}",
            snap.sub_delay
        );
        assert_eq!(ctrl.deband_config(), DebandConfig::enabled_balanced());
        assert_eq!(ctrl.shader_preset(), ShaderPreset::Anime4kFast);
        assert_eq!(ctrl.tone_mapping_preset(), ToneMappingPreset::Spline);
        assert_eq!(ctrl.upscale_preset(), UpscalePreset::EwaLanczos);

        ctrl.set_hdr_backend_options(HdrBackendOptions::default())
            .expect("reset hdr_backend_options");
        ctrl.set_deband(DebandConfig::default())
            .expect("reset deband");
        ctrl.set_shader_preset_in_dir(ShaderPreset::Off, &shader_dir)
            .expect("reset shader_preset");
        ctrl.set_speed(1.0).expect("reset speed");
        ctrl.stop().expect("stop");

        let _ = std::fs::remove_dir_all(&shader_dir);
    }

    #[test]
    fn shorten_codec_label_and_snapshot_telemetry_defaults() {
        let def = PlayerSnapshot::default();
        assert_eq!(def.cache_speed_bytes, 0);
        assert!(def.video_format.is_empty());
        assert!(def.short_video_codec().is_empty());

        assert_eq!(
            shorten_codec_label("hevc", "H.265 / HEVC (High Efficiency Video Coding)"),
            "HEVC"
        );
        assert_eq!(
            shorten_codec_label("", "H.265 / HEVC (High Efficiency Video Coding)"),
            "HEVC"
        );
        assert_eq!(
            shorten_codec_label("h264", "H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10"),
            "AVC"
        );
        assert_eq!(
            shorten_codec_label("", "H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10"),
            "AVC"
        );
        assert_eq!(
            shorten_codec_label("av1", "Alliance for Open Media AV1 (libdav1d)"),
            "AV1"
        );
        assert_eq!(
            shorten_codec_label("", "libdav1d (dav1d AV1 decoder)"),
            "AV1"
        );
        assert_eq!(shorten_codec_label("vp9", "Google VP9"), "VP9");
        assert_eq!(shorten_codec_label("vp8", "On2 VP8"), "VP8");
        assert_eq!(shorten_codec_label("mpeg4", "MPEG-4 part 2"), "MPEG-4");
        assert_eq!(shorten_codec_label("mpeg2video", "MPEG-2 video"), "MPEG-2");
        assert_eq!(shorten_codec_label("vc1", "SMPTE VC-1"), "VC-1");
        assert_eq!(
            shorten_codec_label("prores", "Apple ProRes (iCodec Pro)"),
            "ProRes"
        );

        let exotic = shorten_codec_label(
            "",
            "SuperVerboseExperimentalCodecIdentifier (Reference Decoder v12.4)",
        );
        assert_eq!(exotic, "SuperVerboseEx");
        assert!(exotic.chars().count() <= 14);

        let snap = PlayerSnapshot {
            video_format: "hevc".into(),
            video_codec: "H.265 / HEVC (High Efficiency Video Coding)".into(),
            cache_speed_bytes: 14_889_779,
            ..PlayerSnapshot::default()
        };
        assert_eq!(snap.short_video_codec(), "HEVC");
    }

    #[test]
    fn live_mpv_observes_video_format_and_resets_cache_speed_on_eof() {
        let ctrl = MpvController::with_extra_options(&[("vo", "null"), ("ao", "null")])
            .expect("headless controller");
        ctrl.load_file("av://lavfi:color=c=teal:s=160x90:d=0.3:r=25", 0.0)
            .expect("load lavfi stream");

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let snap = ctrl.snapshot();
            if !snap.video_format.is_empty() && snap.eof_reached {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }

        let snap = ctrl.snapshot();
        assert!(
            !snap.video_format.is_empty(),
            "expected libmpv video-format to be populated, got empty (codec={:?})",
            snap.video_codec
        );
        let short = snap.short_video_codec();
        assert!(!short.is_empty() && short.chars().count() <= 14);
        assert_eq!(
            snap.cache_speed_bytes, 0,
            "cache_speed_bytes should reset to 0 on EOF"
        );

        ctrl.stop().expect("stop");
    }

    #[test]
    fn shutdown_async_and_repeated_playback_cycles_reclaim_mpv_cleanly() {
        for cycle in 0..4 {
            let ctrl = MpvController::with_extra_options(&[("vo", "null"), ("ao", "null")])
                .expect("headless controller cycle");
            let events_rx = ctrl.subscribe_events();
            let stale_clone = ctrl.clone();

            ctrl.load_file("av://lavfi:testsrc=s=320x180:d=1.5:r=25", 0.0)
                .expect("load synthetic stream");

            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if ctrl.snapshot().video_width == 320 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(
                ctrl.snapshot().video_width,
                320,
                "cycle {cycle} should decode 320x180 video"
            );

            let t0 = Instant::now();
            ctrl.shutdown_async();
            let ui_elapsed = t0.elapsed();
            assert!(
                ui_elapsed < Duration::from_millis(500),
                "shutdown_async must be non-blocking on caller thread, took {ui_elapsed:?}"
            );
            assert!(stale_clone.is_terminated());
            assert!(events_rx.is_closed());

            stale_clone
                .set_paused(true)
                .expect("post-shutdown set_paused no-op");
            stale_clone.stop().expect("post-shutdown stop no-op");

            let td_deadline = Instant::now() + Duration::from_secs(10);
            while stale_clone
                .inner
                .mpv_slot
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .is_some()
                && Instant::now() < td_deadline
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(
                stale_clone
                    .inner
                    .mpv_slot
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_none(),
                "background teardown thread must destroy mpv_handle in cycle {cycle}"
            );
        }
    }

    #[test]
    fn windows_backend_selection_matches_dcomp_env_and_overrides() {
        assert!(!is_direct_composition_disabled_value(None));
        assert!(!is_direct_composition_disabled_value(Some("0")));
        assert!(!is_direct_composition_disabled_value(Some("false")));
        assert!(is_direct_composition_disabled_value(Some("1")));
        assert!(is_direct_composition_disabled_value(Some("true")));

        let default_win = resolve_windows_backend_selection(false, &[]);
        assert_eq!(
            default_win.backend_mode,
            DCompBackendMode::FlipChildHwndUnderlay
        );
        assert!(!default_win.uses_libmpv_vo);
        assert!(default_win.defer_media_until_attach);
        assert_eq!(default_win.hwdec, "d3d11va,nvdec,auto-safe");
        assert!(should_probe_d3d11_composition(default_win, &[]));
        assert!(!should_probe_d3d11_composition(
            default_win,
            &[("d3d11-output-mode", "window")]
        ));

        let disabled_win = resolve_windows_backend_selection(true, &[]);
        assert_eq!(
            disabled_win.backend_mode,
            DCompBackendMode::SoftwareFallback
        );
        assert!(disabled_win.uses_libmpv_vo);
        assert!(disabled_win.defer_media_until_attach);
        assert_eq!(
            disabled_win.hwdec,
            "d3d11va-copy,dxva2-copy,nvdec-copy,auto-copy"
        );
        assert!(!should_probe_d3d11_composition(disabled_win, &[]));

        let forced_libmpv = resolve_windows_backend_selection(false, &[("vo", "libmpv")]);
        assert_eq!(
            forced_libmpv.backend_mode,
            DCompBackendMode::SoftwareFallback
        );
        assert!(forced_libmpv.uses_libmpv_vo);
        assert!(forced_libmpv.defer_media_until_attach);
        assert!(!should_probe_d3d11_composition(forced_libmpv, &[]));

        let forced_null = resolve_windows_backend_selection(false, &[("vo", "null")]);
        assert_eq!(forced_null.backend_mode, DCompBackendMode::SoftwareFallback);
        assert!(!forced_null.uses_libmpv_vo);
        assert!(!forced_null.defer_media_until_attach);
        assert!(!should_probe_d3d11_composition(forced_null, &[]));
    }

    #[test]
    fn subtitle_style_defaults_validation_clamping_and_live_persistence() {
        let def = SubtitleStyle::default();
        assert_eq!(def.font_size, 46);
        assert_eq!(def.vertical_pos, 96);
        assert_eq!(def.color_hex, "#F5F1E8");
        assert_eq!(def.background, SubtitleBackgroundMode::DropShadow);
        assert_eq!(def.ass_override, SubtitleAssOverride::Scale);

        // Valid #RRGGBB and #AARRGGBB pass, invalid hex falls back to #F5F1E8, bounds clamp.
        let valid_6 = SubtitleStyle {
            font_size: 10,
            vertical_pos: 200,
            color_hex: "#E8DFC8".to_string(),
            background: SubtitleBackgroundMode::TranslucentPill,
            ass_override: SubtitleAssOverride::Force,
        }
        .sanitized();
        assert_eq!(valid_6.font_size, 20);
        assert_eq!(valid_6.vertical_pos, 105);
        assert_eq!(valid_6.color_hex, "#E8DFC8");

        let valid_8 = SubtitleStyle {
            font_size: 120,
            vertical_pos: 10,
            color_hex: "#D0F5F1E8".to_string(),
            background: SubtitleBackgroundMode::DropShadow,
            ass_override: SubtitleAssOverride::No,
        }
        .sanitized();
        assert_eq!(valid_8.font_size, 96);
        assert_eq!(valid_8.vertical_pos, 50);
        assert_eq!(valid_8.color_hex, "#D0F5F1E8");

        for bad_hex in [
            "",
            "F5F1E8",
            "#FFF",
            "#12345",
            "#1234567",
            "#123456789",
            "#GGGGGG",
            "#F5F1E8\n",
            "red",
        ] {
            let s = SubtitleStyle {
                color_hex: bad_hex.to_string(),
                ..SubtitleStyle::default()
            }
            .sanitized();
            assert_eq!(
                s.color_hex, "#F5F1E8",
                "bad color {bad_hex:?} must fall back to #F5F1E8"
            );
        }

        let ctrl = MpvController::with_extra_options(&[("vo", "null"), ("ao", "null")])
            .expect("headless controller");
        assert_eq!(ctrl.subtitle_style(), SubtitleStyle::default());

        let custom = SubtitleStyle {
            font_size: 12,
            vertical_pos: 140,
            color_hex: "#invalid".to_string(),
            background: SubtitleBackgroundMode::TranslucentPill,
            ass_override: SubtitleAssOverride::Force,
        };
        ctrl.set_subtitle_style(custom).expect("set_subtitle_style");
        let applied = ctrl.subtitle_style();
        assert_eq!(applied.font_size, 20);
        assert_eq!(applied.vertical_pos, 105);
        assert_eq!(applied.color_hex, "#F5F1E8");
        assert_eq!(applied.background, SubtitleBackgroundMode::TranslucentPill);
        assert_eq!(applied.ass_override, SubtitleAssOverride::Force);

        let drop_shadow = SubtitleStyle {
            font_size: 54,
            vertical_pos: 92,
            color_hex: "#CCFFFFFF".to_string(),
            background: SubtitleBackgroundMode::DropShadow,
            ass_override: SubtitleAssOverride::No,
        };
        ctrl.set_subtitle_style(drop_shadow.clone())
            .expect("set_subtitle_style drop shadow");
        assert_eq!(ctrl.subtitle_style(), drop_shadow);

        ctrl.switch_stream_seamless("av://lavfi:testsrc=s=64x64:d=1.0:r=25", 0.0)
            .expect("switch_stream_seamless preserves subtitle_style");
        assert_eq!(ctrl.subtitle_style(), drop_shadow);

        ctrl.shutdown_blocking();
    }

    #[test]
    fn validate_local_subtitle_path_enforces_security_and_extension_rules() {
        let base_dir = std::env::temp_dir().join(format!(
            "gpui_mpv_sub_test_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&base_dir).unwrap();

        let srt_path = base_dir.join("episode.srt");
        let ass_path = base_dir.join("episode.ASS");
        let vtt_path = base_dir.join("episode.vtt");
        let mkv_path = base_dir.join("episode.mkv");
        let mp4_path = base_dir.join("episode.mp4");
        std::fs::write(&srt_path, "1\n00:00:01,000 --> 00:00:02,000\nHello\n").unwrap();
        std::fs::write(&ass_path, "[Script Info]\nTitle: Test\n").unwrap();
        std::fs::write(&vtt_path, "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nHi\n").unwrap();
        std::fs::write(&mkv_path, b"not a subtitle").unwrap();
        std::fs::write(&mp4_path, b"not a subtitle").unwrap();

        let canon_srt = validate_local_subtitle_path(&srt_path).expect("valid .srt");
        assert!(canon_srt.is_absolute());
        assert!(validate_local_subtitle_path(&ass_path).is_ok());
        assert!(validate_local_subtitle_path(&vtt_path).is_ok());

        // Security invariant: validate_stream_url must STILL reject local file paths.
        assert!(validate_stream_url_impl(&srt_path.to_string_lossy(), false).is_err());
        assert!(validate_stream_url("file:///tmp/episode.srt").is_err());
        assert!(validate_stream_url("/etc/passwd").is_err());
        assert!(validate_stream_url("/opt/ferrite/episode.srt").is_err());

        // Reject unsupported extensions, schemes, dirs, non-existent files, leading '-', and control chars.
        assert!(validate_local_subtitle_path(&mkv_path).is_err());
        assert!(validate_local_subtitle_path(&mp4_path).is_err());
        assert!(validate_local_subtitle_path(&base_dir).is_err());
        assert!(validate_local_subtitle_path(&base_dir.join("missing.srt")).is_err());
        assert!(validate_local_subtitle_path(Path::new("")).is_err());
        assert!(validate_local_subtitle_path(Path::new("-sub.srt")).is_err());
        assert!(validate_local_subtitle_path(Path::new("edl://!new_stream.srt")).is_err());
        assert!(validate_local_subtitle_path(Path::new("lavfi://test.srt")).is_err());
        assert!(validate_local_subtitle_path(Path::new("memory://payload.srt")).is_err());
        assert!(validate_local_subtitle_path(Path::new("archive:///a.zip|b.srt")).is_err());
        assert!(validate_local_subtitle_path(Path::new("fd://0.srt")).is_err());
        assert!(validate_local_subtitle_path(Path::new("bad\npath.srt")).is_err());
        assert!(validate_local_subtitle_path(Path::new("bad\0path.srt")).is_err());

        // Verify deferred queue drain on default libmpv controller without passing local path to validate_stream_url.
        let ctrl = MpvController::new().expect("default streaming MpvController::new()");
        ctrl.add_local_subtitle_file(&srt_path, Some("English"), Some("en"))
            .expect("deferred add_local_subtitle_file before render attach");
        assert!(ctrl.add_local_subtitle_file(&mkv_path, None, None).is_err());
        ctrl.set_viewport_rect(0, 0, 320, 180, 1.0);
        ctrl.attach_software_renderer()
            .expect("attach_software_renderer drains local subtitle queue");
        ctrl.add_local_subtitle_file(&ass_path, Some("Styled"), None)
            .expect("immediate add_local_subtitle_file after render attach");
        ctrl.shutdown_blocking();

        let _ = std::fs::remove_dir_all(&base_dir);
    }
}
