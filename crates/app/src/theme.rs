use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};

use gpui::{
    div, ease_out_quint, prelude::*, px, rgb, rgba, Animation, BoxShadow, Div, Font, FontFallbacks,
    FontFeatures, FontStyle, FontWeight, Rgba, SharedString,
};
use stremio_core_runtime::{
    AudioCodec, HdrFormat, ParsedStream, Resolution, StreamQuality, VideoCodec,
};

use crate::icons::{svg_icon, IconName};

pub const BG_OBSIDIAN: u32 = 0x0b0b0d;
pub const BG_OBSIDIAN_BLUR: u32 = 0x0b0b0df5;
pub const BG_TOPNAV: u32 = 0x0b0b0dc7;

pub const BG_SURFACE: u32 = 0x141417;
pub const BG_ELEVATED: u32 = 0x1c1c20;
pub const BG_SURFACE_3: u32 = 0x25252a;
pub const BG_INPUT: u32 = 0x141417;

pub const BG_SCRIM: u32 = 0x0b0b0db8;
pub const BG_OSD_GLASS: u32 = 0x141417db;
pub const BG_POPOVER_GLASS: u32 = 0x141417f7;

pub const BORDER_SUBTLE: u32 = 0x2a2a2fff;
pub const BORDER_SOFT: u32 = 0x1f1f23ff;
pub const BORDER_STRONG: u32 = 0x3a3a40ff;
pub const BORDER_ACCENT: u32 = 0xff5e3a66;
pub const BORDER_POSTER_HOVER: u32 = 0xff5e3aff;

pub const TEXT_PRIMARY: u32 = 0xf5f1e8;
pub const TEXT_SECONDARY: u32 = 0x9a9a9f;
pub const TEXT_MUTED: u32 = 0x6a6a70;

pub const ACCENT: u32 = 0xff5e3a;
pub const ACCENT_VIOLET: u32 = ACCENT;
pub const ACCENT_HOVER: u32 = 0xff7752;
pub const ACCENT_VIOLET_HOVER: u32 = ACCENT_HOVER;
pub const ACCENT_PRESS: u32 = 0xe04a28;
pub const ACCENT_SOFT: u32 = 0xff5e3a1f;

pub const STATUS_WATCHED: u32 = 0x4ade80;
pub const STATUS_WATCHING: u32 = 0x60a5fa;
pub const STATUS_WATCHLIST: u32 = 0xc084fc;
pub const ACCENT_AMBER: u32 = 0xfbbf24;
pub const ACCENT_ROSE: u32 = 0xf87171;

#[inline]
pub const fn transparent_cutout() -> Rgba {
    Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    }
}

static TABULAR_NUMS_FEATURE: LazyLock<FontFeatures> =
    LazyLock::new(|| FontFeatures(Arc::new(vec![("tnum".to_owned(), 1)])));

static DISPLAY_SERIF_FONT: LazyLock<Font> = LazyLock::new(|| Font {
    family: "Instrument Serif".into(),
    fallbacks: Some(FontFallbacks::from_fonts(vec![
        "Noto Serif Display".to_owned(),
        "Noto Serif".to_owned(),
        "Liberation Serif".to_owned(),
        "DejaVu Serif".to_owned(),
    ])),
    features: FontFeatures::default(),
    weight: FontWeight::MEDIUM,
    style: FontStyle::Italic,
});

static UI_SANS_FALLBACKS: LazyLock<FontFallbacks> = LazyLock::new(|| {
    FontFallbacks::from_fonts(vec![
        "Inter".to_owned(),
        "Noto Sans".to_owned(),
        "DejaVu Sans".to_owned(),
    ])
});

static MONO_TABULAR_FONT: LazyLock<Font> = LazyLock::new(|| Font {
    family: "Geist Mono".into(),
    fallbacks: Some(FontFallbacks::from_fonts(vec![
        "JetBrains Mono".to_owned(),
        "Noto Sans Mono".to_owned(),
        "Liberation Mono".to_owned(),
        "DejaVu Sans Mono".to_owned(),
    ])),
    features: TABULAR_NUMS_FEATURE.clone(),
    weight: FontWeight::MEDIUM,
    style: FontStyle::Normal,
});

#[inline]
pub fn tabular_nums() -> FontFeatures {
    TABULAR_NUMS_FEATURE.clone()
}

#[inline]
pub fn editorial_serif() -> Font {
    DISPLAY_SERIF_FONT.clone()
}

#[inline]
pub fn ui_sans(weight: FontWeight) -> Font {
    Font {
        family: "Geist".into(),
        fallbacks: Some(UI_SANS_FALLBACKS.clone()),
        features: FontFeatures::default(),
        weight,
        style: FontStyle::Normal,
    }
}

#[inline]
pub fn mono_font() -> Font {
    MONO_TABULAR_FONT.clone()
}

#[inline]
pub fn obsidian_fade_in() -> Animation {
    Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint())
}

pub fn glass_capsule_shadow() -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.0), px(12.0), rgba(0x00000080).into()).blur_radius(px(32.0)),
        BoxShadow::new(px(0.0), px(2.0), rgba(0x0000004d).into()).blur_radius(px(6.0)),
    ]
}

pub fn poster_card_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.0), px(1.0), rgba(0x00000066).into()).blur_radius(px(2.0))]
}

pub fn movietracker_mark(size_px: f32) -> Div {
    let sprocket_col = || {
        div()
            .h_full()
            .py(px(3.0))
            .flex()
            .flex_col()
            .justify_between()
            .children((0..4).map(|_| {
                div()
                    .w(px(2.5))
                    .h(px(2.5))
                    .rounded(px(0.75))
                    .bg(rgba(0x0b0b0d99))
            }))
    };

    div()
        .w(px(size_px))
        .h(px(size_px))
        .flex_shrink_0()
        .rounded(px(6.0))
        .bg(rgb(ACCENT))
        .px(px(3.0))
        .flex()
        .items_center()
        .justify_between()
        .child(sprocket_col())
        .child(svg_icon(IconName::Play, size_px * 0.42, rgb(TEXT_PRIMARY)))
        .child(sprocket_col())
}

pub fn meta_divider() -> Div {
    div()
        .w(px(1.0))
        .h(px(11.0))
        .flex_shrink_0()
        .bg(rgba(BORDER_SUBTLE))
}

pub fn neutral_chip(label: impl Into<SharedString>) -> Div {
    div()
        .px(px(8.0))
        .py(px(2.5))
        .rounded(px(999.0))
        .bg(rgb(BG_ELEVATED))
        .border_1()
        .border_color(rgba(BORDER_SUBTLE))
        .font(mono_font())
        .text_xs()
        .text_color(rgb(TEXT_PRIMARY))
        .child(label.into())
}

pub fn semantic_status_pill(
    icon: IconName,
    label: &'static str,
    fg_hex: u32,
    border_rgba: u32,
    bg_rgba: u32,
) -> Div {
    div()
        .h(px(26.0))
        .px(px(10.0))
        .rounded(px(999.0))
        .bg(rgba(bg_rgba))
        .border_1()
        .border_color(rgba(border_rgba))
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(fg_hex))
        .child(svg_icon(icon, 12.0, rgb(fg_hex)))
        .child(label)
}

pub fn keycap(label: impl Into<SharedString>) -> Div {
    div()
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .bg(rgb(BG_SURFACE_3))
        .border_1()
        .border_color(rgba(BORDER_SUBTLE))
        .font(mono_font())
        .text_size(px(11.0))
        .text_color(rgb(TEXT_MUTED))
        .child(label.into())
}

pub struct CleanStreamSummary {
    pub resolution: &'static str,
    pub hdr_label: Option<&'static str>,
    pub notable_audio: Option<&'static str>,
    pub availability_text: String,
    pub size_text: Option<String>,
    pub addon_label: Option<String>,
    pub technical_specs: Vec<String>,
}

pub fn summarize_stream_for_humans(parsed: &ParsedStream) -> CleanStreamSummary {
    let resolution = match parsed.resolution {
        Resolution::Uhd4k => "4K",
        Resolution::Fhd1080p => "1080p",
        Resolution::Hd720p => "720p",
        Resolution::Sd => "SD",
        Resolution::Unknown => "Stream",
    };

    let hdr_label = match parsed.hdr {
        HdrFormat::DolbyVision => Some("Dolby Vision"),
        HdrFormat::Hdr10Plus => Some("HDR10+"),
        HdrFormat::Hdr10 => Some("HDR10"),
        HdrFormat::Hlg => Some("HLG"),
        HdrFormat::Sdr => None,
    };

    let notable_audio = match parsed.audio {
        AudioCodec::Atmos => Some("Atmos"),
        AudioCodec::TrueHD => Some("TrueHD"),
        AudioCodec::DtsHdMa => Some("DTS-HD MA"),
        _ => None,
    };

    let availability_text = if parsed.is_cached {
        match parsed.debrid_provider.as_deref() {
            Some("RD+") => "[RD+] Cached".to_owned(),
            Some("TB+") => "[TB+] Cached".to_owned(),
            Some("AD+") => "[AD+] Cached".to_owned(),
            Some(p) if !p.is_empty() => format!("Cached ({p})"),
            _ => "Cached".to_owned(),
        }
    } else if let Some(seeds) = parsed.seeders {
        format!("{seeds} peers")
    } else if let Some(p) = parsed.debrid_provider.as_deref() {
        p.to_owned()
    } else {
        "Direct".to_owned()
    };

    let mut technical_specs = Vec::with_capacity(6);
    if !matches!(parsed.quality, StreamQuality::Unknown) {
        technical_specs.push(parsed.quality.label().to_owned());
    }
    if !matches!(parsed.codec, VideoCodec::Unknown) {
        technical_specs.push(parsed.codec.label().to_owned());
    }
    if !matches!(parsed.audio, AudioCodec::Unknown) {
        technical_specs.push(parsed.audio.label().to_owned());
    }
    for b in &parsed.badges {
        if b.eq_ignore_ascii_case("10-bit") || b.eq_ignore_ascii_case("10bit") {
            technical_specs.push("10-bit".to_owned());
        }
    }
    if let Some(group) = &parsed.release_group {
        if !group.is_empty() {
            technical_specs.push(format!("Group {group}"));
        }
    }
    if let Some(idx) = &parsed.indexer {
        if !idx.is_empty() {
            technical_specs.push(format!("Indexer {idx}"));
        }
    }

    CleanStreamSummary {
        resolution,
        hdr_label,
        notable_audio,
        availability_text,
        size_text: parsed.size_display.clone(),
        addon_label: parsed.addon_name.clone(),
        technical_specs,
    }
}

pub fn format_timestamp_secs(secs: f64) -> String {
    if !secs.is_finite() || secs <= 0.0 {
        return "00:00".to_owned();
    }
    let total = secs.floor() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

pub fn sentence_case_label(raw: &str) -> String {
    let cleaned = raw.trim().replace(['_', '-'], " ");
    let mut chars = cleaned.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let mut out = String::with_capacity(cleaned.len());
            for c in first.to_uppercase() {
                out.push(c);
            }
            out.push_str(&chars.as_str().to_lowercase());
            out
        }
    }
}

pub fn placeholder_tint_for_id(id: &str) -> u32 {
    let mut hash: u32 = 2166136261;
    for b in id.bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(16777619);
    }
    const PALETTE: [u32; 4] = [0x1c1c20, 0x201e20, 0x1d1f23, 0x222024];
    PALETTE[(hash as usize) % PALETTE.len()]
}
