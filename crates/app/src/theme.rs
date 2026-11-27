use std::{
    cell::Cell,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc, LazyLock,
    },
    time::Duration,
};

use gpui::{
    div, ease_out_quint, prelude::*, px, rgb, rgba, Animation, BoxShadow, Div, Font, FontFallbacks,
    FontFeatures, FontStyle, FontWeight, Pixels, Rgba, SharedString,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use stremio_core_runtime::{
    AudioCodec, HdrFormat, ParsedStream, Resolution, StreamQuality, VideoCodec,
};

use crate::icons::{svg_icon, IconName};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreset {
    #[default]
    EtherealDark,
    PureOled,
    AuroraIndigo,
    WarmCinema,
    LightMist,
}

impl ThemePreset {
    pub const ALL: [Self; 5] = [
        Self::EtherealDark,
        Self::PureOled,
        Self::AuroraIndigo,
        Self::WarmCinema,
        Self::LightMist,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::EtherealDark => "Ethereal Dark",
            Self::PureOled => "Pure OLED",
            Self::AuroraIndigo => "Aurora Indigo",
            Self::WarmCinema => "Warm Cinema",
            Self::LightMist => "Light Mist",
        }
    }

    pub const fn subtitle(self) -> &'static str {
        match self {
            Self::EtherealDark => "Velvet obsidian glass with warm neutral ink",
            Self::PureOled => "True #000000 black canvas for OLED panels",
            Self::AuroraIndigo => "Deep nocturnal midnight indigo atmosphere",
            Self::WarmCinema => "Smoky theater espresso with warm tungsten highlights",
            Self::LightMist => "Frosted alabaster daylight canvas with crisp dark ink",
        }
    }

    pub const fn is_light(self) -> bool {
        matches!(self, Self::LightMist)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccentChoice {
    #[default]
    EmberCoral,
    ElectricViolet,
    AuroraCyan,
    RoseQuartz,
    CinemaGold,
    EmeraldMint,
}

impl AccentChoice {
    pub const ALL: [Self; 6] = [
        Self::EmberCoral,
        Self::ElectricViolet,
        Self::AuroraCyan,
        Self::RoseQuartz,
        Self::CinemaGold,
        Self::EmeraldMint,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::EmberCoral => "Ember Coral",
            Self::ElectricViolet => "Electric Violet",
            Self::AuroraCyan => "Aurora Cyan",
            Self::RoseQuartz => "Rose Quartz",
            Self::CinemaGold => "Cinema Gold",
            Self::EmeraldMint => "Emerald Mint",
        }
    }

    pub const fn hex(self) -> u32 {
        match self {
            Self::EmberCoral => 0xff5e3a,
            Self::ElectricViolet => 0x8b5cf6,
            Self::AuroraCyan => 0x22d3ee,
            Self::RoseQuartz => 0xf472b6,
            Self::CinemaGold => 0xfbbf24,
            Self::EmeraldMint => 0x34d399,
        }
    }

    pub const fn hover_hex(self) -> u32 {
        match self {
            Self::EmberCoral => 0xff7752,
            Self::ElectricViolet => 0xa78bfa,
            Self::AuroraCyan => 0x67e8f9,
            Self::RoseQuartz => 0xf9a8d4,
            Self::CinemaGold => 0xfcd34d,
            Self::EmeraldMint => 0x6ee7b7,
        }
    }

    pub const fn press_hex(self) -> u32 {
        match self {
            Self::EmberCoral => 0xe04a28,
            Self::ElectricViolet => 0x7c3aed,
            Self::AuroraCyan => 0x06b6d4,
            Self::RoseQuartz => 0xec4899,
            Self::CinemaGold => 0xd97706,
            Self::EmeraldMint => 0x10b981,
        }
    }

    pub const fn on_accent_hex(self) -> u32 {
        match self {
            Self::ElectricViolet => 0xffffff,
            Self::EmberCoral
            | Self::AuroraCyan
            | Self::RoseQuartz
            | Self::CinemaGold
            | Self::EmeraldMint => 0x0b0b0d,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiScaleChoice {
    S90,
    #[default]
    S100,
    S110,
    S125,
}

impl UiScaleChoice {
    pub const ALL: [Self; 4] = [Self::S90, Self::S100, Self::S110, Self::S125];

    pub const fn label(self) -> &'static str {
        match self {
            Self::S90 => "90%",
            Self::S100 => "100%",
            Self::S110 => "110%",
            Self::S125 => "125%",
        }
    }

    pub const fn factor(self) -> f32 {
        match self {
            Self::S90 => 0.90,
            Self::S100 => 1.00,
            Self::S110 => 1.10,
            Self::S125 => 1.25,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardDensityChoice {
    Compact,
    #[default]
    Comfortable,
    Cinema,
}

impl CardDensityChoice {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Comfortable, Self::Cinema];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Comfortable => "Comfortable",
            Self::Cinema => "Cinema",
        }
    }

    pub const fn target_poster_width_px(self) -> f32 {
        match self {
            Self::Compact => 176.0,
            Self::Comfortable => 216.0,
            Self::Cinema => 256.0,
        }
    }

    pub const fn grid_gap_px(self) -> f32 {
        match self {
            Self::Compact => 16.0,
            Self::Comfortable => 20.0,
            Self::Cinema => 24.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ThemePalette {
    pub preset: ThemePreset,
    pub accent_choice: AccentChoice,
    pub is_light: bool,
    pub bg_canvas: u32,
    pub bg_surface: u32,
    pub bg_elevated: u32,
    pub bg_surface_3: u32,
    pub bg_topnav_float: u32,
    pub bg_topnav_scrolled: u32,
    pub glass_sheet_bg: u32,
    pub glass_popover_bg: u32,
    pub glass_card_bg: u32,
    pub glass_border: u32,
    pub glass_border_strong: u32,
    pub glass_divider: u32,
    pub glass_trough_bg: u32,
    pub glass_row_hover_bg: u32,
    pub glass_row_selected_bg: u32,
    pub segment_active_bg: u32,
    pub segment_active_text: u32,
    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_muted: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub accent_press: u32,
    pub accent_soft: u32,
    pub border_accent: u32,
    pub border_poster_hover: u32,
    pub cta_on_accent: u32,
    pub scrim_top: u32,
    pub scrim_bottom: u32,
    pub scrim_left: u32,
    pub status_watched: u32,
    pub status_watching: u32,
    pub status_watchlist: u32,
    pub accent_amber: u32,
    pub accent_rose: u32,
    pub ambient_bleed: bool,
    pub ui_scale: f32,
    pub density: CardDensityChoice,
}

pub const fn build_palette(
    preset: ThemePreset,
    accent_choice: AccentChoice,
    ambient_bleed: bool,
    ui_scale: f32,
    density: CardDensityChoice,
) -> ThemePalette {
    let accent = accent_choice.hex();
    let accent_hover = accent_choice.hover_hex();
    let accent_press = accent_choice.press_hex();
    let cta_on_accent = accent_choice.on_accent_hex();
    let is_light = preset.is_light();

    let accent_soft = (accent << 8) | if is_light { 0x26 } else { 0x1f };
    let border_accent = (accent << 8) | if is_light { 0x73 } else { 0x66 };
    let border_poster_hover = (accent << 8) | 0xff;
    let clamped_scale = if ui_scale >= 0.5 && ui_scale <= 2.0 {
        ui_scale
    } else {
        1.0
    };

    match preset {
        ThemePreset::EtherealDark => ThemePalette {
            preset,
            accent_choice,
            is_light: false,
            bg_canvas: 0x0b0b0d,
            bg_surface: 0x141417,
            bg_elevated: 0x1c1c20,
            bg_surface_3: 0x25252a,
            bg_topnav_float: 0x0b0b0d99,
            bg_topnav_scrolled: 0x0b0b0de6,
            glass_sheet_bg: 0x141416F2,
            glass_popover_bg: 0x141417F5,
            glass_card_bg: 0xFFFFFF0A,
            glass_border: 0xFFFFFF1C,
            glass_border_strong: 0xFFFFFF30,
            glass_divider: 0xFFFFFF14,
            glass_trough_bg: 0xFFFFFF0E,
            glass_row_hover_bg: 0xFFFFFF0D,
            glass_row_selected_bg: 0xFFFFFF16,
            segment_active_bg: 0xF5F5F7,
            segment_active_text: 0x0B0B0D,
            text_primary: 0xF5F1E8,
            text_secondary: 0xB6B6C0,
            text_muted: 0x90909C,
            accent,
            accent_hover,
            accent_press,
            accent_soft,
            border_accent,
            border_poster_hover,
            cta_on_accent,
            scrim_top: 0x0B0B0D99,
            scrim_bottom: 0x0B0B0DF2,
            scrim_left: 0x0B0B0DD9,
            status_watched: 0x4ade80,
            status_watching: 0x60a5fa,
            status_watchlist: 0xc084fc,
            accent_amber: 0xfbbf24,
            accent_rose: 0xf87171,
            ambient_bleed,
            ui_scale: clamped_scale,
            density,
        },
        ThemePreset::PureOled => ThemePalette {
            preset,
            accent_choice,
            is_light: false,
            bg_canvas: 0x000000,
            bg_surface: 0x0E0E12,
            bg_elevated: 0x17171D,
            bg_surface_3: 0x222229,
            bg_topnav_float: 0x000000B8,
            bg_topnav_scrolled: 0x000000F2,
            glass_sheet_bg: 0x0E0E12F5,
            glass_popover_bg: 0x121217F7,
            glass_card_bg: 0xFFFFFF0B,
            glass_border: 0xFFFFFF22,
            glass_border_strong: 0xFFFFFF38,
            glass_divider: 0xFFFFFF18,
            glass_trough_bg: 0xFFFFFF10,
            glass_row_hover_bg: 0xFFFFFF0F,
            glass_row_selected_bg: 0xFFFFFF1A,
            segment_active_bg: 0xFFFFFF,
            segment_active_text: 0x000000,
            text_primary: 0xF7F7FA,
            text_secondary: 0xBABAC6,
            text_muted: 0x9292A0,
            accent,
            accent_hover,
            accent_press,
            accent_soft,
            border_accent,
            border_poster_hover,
            cta_on_accent,
            scrim_top: 0x000000A6,
            scrim_bottom: 0x000000F5,
            scrim_left: 0x000000E0,
            status_watched: 0x4ade80,
            status_watching: 0x60a5fa,
            status_watchlist: 0xc084fc,
            accent_amber: 0xfbbf24,
            accent_rose: 0xf87171,
            ambient_bleed,
            ui_scale: clamped_scale,
            density,
        },
        ThemePreset::AuroraIndigo => ThemePalette {
            preset,
            accent_choice,
            is_light: false,
            bg_canvas: 0x0B0E1A,
            bg_surface: 0x121629,
            bg_elevated: 0x1A2038,
            bg_surface_3: 0x232B48,
            bg_topnav_float: 0x0B0E1AA3,
            bg_topnav_scrolled: 0x0B0E1AEB,
            glass_sheet_bg: 0x111528F2,
            glass_popover_bg: 0x141930F7,
            glass_card_bg: 0xD8E2FF0A,
            glass_border: 0xCBD8FF1A,
            glass_border_strong: 0xCBD8FF30,
            glass_divider: 0xCBD8FF14,
            glass_trough_bg: 0xCBD8FF0F,
            glass_row_hover_bg: 0xD8E2FF0D,
            glass_row_selected_bg: 0xD8E2FF18,
            segment_active_bg: 0xEEF2FF,
            segment_active_text: 0x0B0E1A,
            text_primary: 0xEEF2FF,
            text_secondary: 0xB0BCDE,
            text_muted: 0x8693BA,
            accent,
            accent_hover,
            accent_press,
            accent_soft,
            border_accent,
            border_poster_hover,
            cta_on_accent,
            scrim_top: 0x0B0E1A99,
            scrim_bottom: 0x0B0E1AF2,
            scrim_left: 0x0B0E1AD9,
            status_watched: 0x4ade80,
            status_watching: 0x60a5fa,
            status_watchlist: 0xc084fc,
            accent_amber: 0xfbbf24,
            accent_rose: 0xf87171,
            ambient_bleed,
            ui_scale: clamped_scale,
            density,
        },
        ThemePreset::WarmCinema => ThemePalette {
            preset,
            accent_choice,
            is_light: false,
            bg_canvas: 0x110E0D,
            bg_surface: 0x1A1513,
            bg_elevated: 0x241D1A,
            bg_surface_3: 0x302723,
            bg_topnav_float: 0x110E0DA3,
            bg_topnav_scrolled: 0x110E0DEB,
            glass_sheet_bg: 0x181311F2,
            glass_popover_bg: 0x1D1715F7,
            glass_card_bg: 0xFFF2E609,
            glass_border: 0xFFE8D616,
            glass_border_strong: 0xFFE8D62A,
            glass_divider: 0xFFE8D612,
            glass_trough_bg: 0xFFE8D60E,
            glass_row_hover_bg: 0xFFF2E60C,
            glass_row_selected_bg: 0xFFF2E616,
            segment_active_bg: 0xF7EFE6,
            segment_active_text: 0x110E0D,
            text_primary: 0xF7EFE6,
            text_secondary: 0xC8BBB0,
            text_muted: 0x9E9187,
            accent,
            accent_hover,
            accent_press,
            accent_soft,
            border_accent,
            border_poster_hover,
            cta_on_accent,
            scrim_top: 0x110E0D99,
            scrim_bottom: 0x110E0DF2,
            scrim_left: 0x110E0DD9,
            status_watched: 0x4ade80,
            status_watching: 0x60a5fa,
            status_watchlist: 0xc084fc,
            accent_amber: 0xfbbf24,
            accent_rose: 0xf87171,
            ambient_bleed,
            ui_scale: clamped_scale,
            density,
        },
        ThemePreset::LightMist => ThemePalette {
            preset,
            accent_choice,
            is_light: true,
            bg_canvas: 0xF2F4F8,
            bg_surface: 0xE7EBF2,
            bg_elevated: 0xFFFFFF,
            bg_surface_3: 0xDCE2EC,
            bg_topnav_float: 0xF2F4F8BF,
            bg_topnav_scrolled: 0xF2F4F8EB,
            glass_sheet_bg: 0xFFFFFFF2,
            glass_popover_bg: 0xFFFFFFFA,
            glass_card_bg: 0xFFFFFFB8,
            glass_border: 0x00000018,
            glass_border_strong: 0x0000002B,
            glass_divider: 0x00000012,
            glass_trough_bg: 0x0F172A0D,
            glass_row_hover_bg: 0x0F172A0A,
            glass_row_selected_bg: 0x0F172A14,
            segment_active_bg: 0x12141A,
            segment_active_text: 0xFFFFFF,
            text_primary: 0x12141A,
            text_secondary: 0x4A5162,
            text_muted: 0x6B7280,
            accent,
            accent_hover,
            accent_press,
            accent_soft,
            border_accent,
            border_poster_hover,
            cta_on_accent,
            scrim_top: 0x0B0D1480,
            scrim_bottom: 0x0B0D14D9,
            scrim_left: 0x0B0D14B8,
            status_watched: 0x16A34A,
            status_watching: 0x2563EB,
            status_watchlist: 0x9333EA,
            accent_amber: 0xD97706,
            accent_rose: 0xDC2626,
            ambient_bleed,
            ui_scale: clamped_scale,
            density,
        },
    }
}

/// Canonical 6-step typography scale (px at 100% UI scale).
/// - `TYPE_MICRO` (12px): badges, eyebrows, keycaps, micro-metadata
/// - `TYPE_BODY` (14px): floor for readable prose, card titles, buttons, inputs, settings
/// - `TYPE_SUBHEAD` (16px): card headings, sheet titles, lead metadata
/// - `TYPE_TITLE` (20px): sub-section headings, hero subheads
/// - `TYPE_DISPLAY` (28px): section headers, shelf titles
/// - `TYPE_HERO` (38px): page headers, hero display titles
pub const TYPE_MICRO: f32 = 12.0;
pub const TYPE_BODY: f32 = 14.0;
pub const TYPE_SUBHEAD: f32 = 16.0;
pub const TYPE_TITLE: f32 = 20.0;
pub const TYPE_DISPLAY: f32 = 28.0;
pub const TYPE_HERO: f32 = 38.0;

impl ThemePalette {
    /// Scales a base UI measurement (px) by this palette's UI scale factor without global lookup.
    #[inline]
    pub fn sp(&self, val: f32) -> Pixels {
        px(val * self.ui_scale)
    }

    #[inline]
    pub fn text_micro(&self) -> Pixels {
        self.sp(TYPE_MICRO)
    }

    #[inline]
    pub fn text_body(&self) -> Pixels {
        self.sp(TYPE_BODY)
    }

    #[inline]
    pub fn text_subhead(&self) -> Pixels {
        self.sp(TYPE_SUBHEAD)
    }

    #[inline]
    pub fn text_title(&self) -> Pixels {
        self.sp(TYPE_TITLE)
    }

    #[inline]
    pub fn text_display(&self) -> Pixels {
        self.sp(TYPE_DISPLAY)
    }

    #[inline]
    pub fn text_hero(&self) -> Pixels {
        self.sp(TYPE_HERO)
    }
}

const DEFAULT_THEME_PALETTE: ThemePalette = build_palette(
    ThemePreset::EtherealDark,
    AccentChoice::EmberCoral,
    true,
    1.0,
    CardDensityChoice::Comfortable,
);

static ACTIVE_THEME: RwLock<ThemePalette> = RwLock::new(DEFAULT_THEME_PALETTE);
static ACTIVE_THEME_VERSION: AtomicU64 = AtomicU64::new(0);
static ACTIVE_UI_SCALE_BITS: AtomicU32 = AtomicU32::new(1.0_f32.to_bits());

thread_local! {
    static THREAD_LOCAL_THEME: Cell<(u64, ThemePalette)> = const {
        Cell::new((0, DEFAULT_THEME_PALETTE))
    };
}

/// Returns the monotonic theme version counter incremented whenever theme,
/// accent, ambient bleed, UI scale, or card density changes.
#[inline]
pub fn active_theme_version() -> u64 {
    ACTIVE_THEME_VERSION.load(Ordering::Acquire)
}

/// Returns the active `ThemePalette` via a lock-free thread-local cache synchronized
/// with an atomic version counter (zero `RwLock` contention on the UI thread hot path).
#[inline]
pub fn active_theme() -> ThemePalette {
    let ver = ACTIVE_THEME_VERSION.load(Ordering::Acquire);
    THREAD_LOCAL_THEME.with(|cell| {
        let (cached_ver, cached_palette) = cell.get();
        if cached_ver == ver {
            cached_palette
        } else {
            let guard = ACTIVE_THEME.read();
            let fresh = *guard;
            let synced_ver = ACTIVE_THEME_VERSION.load(Ordering::Acquire);
            drop(guard);
            cell.set((synced_ver, fresh));
            fresh
        }
    })
}

pub fn set_active_theme(
    preset: ThemePreset,
    accent_choice: AccentChoice,
    ambient_bleed: bool,
    ui_scale: f32,
    density: CardDensityChoice,
) {
    let palette = build_palette(preset, accent_choice, ambient_bleed, ui_scale, density);
    let mut guard = ACTIVE_THEME.write();
    *guard = palette;
    ACTIVE_UI_SCALE_BITS.store(palette.ui_scale.to_bits(), Ordering::Relaxed);
    ACTIVE_THEME_VERSION.fetch_add(1, Ordering::Release);
}

/// Scales a base UI measurement (px) by the active theme's UI scale factor
/// via a single lock-free relaxed atomic load.
#[inline]
pub fn sp(val: f32) -> Pixels {
    px(val * f32::from_bits(ACTIVE_UI_SCALE_BITS.load(Ordering::Relaxed)))
}

#[inline]
pub fn type_micro() -> Pixels {
    sp(TYPE_MICRO)
}

#[inline]
pub fn type_body() -> Pixels {
    sp(TYPE_BODY)
}

#[inline]
pub fn type_subhead() -> Pixels {
    sp(TYPE_SUBHEAD)
}

#[inline]
pub fn type_title() -> Pixels {
    sp(TYPE_TITLE)
}

#[inline]
pub fn type_display() -> Pixels {
    sp(TYPE_DISPLAY)
}

#[inline]
pub fn type_hero() -> Pixels {
    sp(TYPE_HERO)
}

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

// Unified v0.4.5 glass surface & interaction tokens.
pub const GLASS_SHEET_BG: u32 = 0x141416F2;
pub const GLASS_POPOVER_BG: u32 = 0x141417F5;
pub const GLASS_CARD_BG: u32 = 0xFFFFFF06;
pub const GLASS_BORDER: u32 = 0xFFFFFF14;
pub const GLASS_BORDER_STRONG: u32 = 0xFFFFFF26;
pub const GLASS_DIVIDER: u32 = 0xFFFFFF10;
pub const GLASS_TROUGH_BG: u32 = 0xFFFFFF0B;
pub const GLASS_ROW_HOVER_BG: u32 = 0xFFFFFF08;
pub const GLASS_ROW_SELECTED_BG: u32 = 0xFFFFFF0F;
pub const SEGMENT_ACTIVE_BG: u32 = 0xF5F5F7;
pub const SEGMENT_ACTIVE_TEXT: u32 = 0x0B0B0D;
pub const CTA_ON_ACCENT: u32 = 0x0B0B0D;

pub const RADIUS_POSTER: f32 = 8.0;
pub const RADIUS_CARD: f32 = 12.0;
pub const RADIUS_CONTROL: f32 = 10.0;
pub const RADIUS_SHEET: f32 = 16.0;
pub const RADIUS_PILL: f32 = 999.0;

pub const BORDER_SUBTLE: u32 = 0x2a2a2fff;
pub const BORDER_SOFT: u32 = 0x1f1f23ff;
pub const BORDER_STRONG: u32 = 0x3a3a40ff;
pub const BORDER_ACCENT: u32 = 0xff5e3a66;
pub const BORDER_POSTER_HOVER: u32 = 0xff5e3aff;

pub const TEXT_PRIMARY: u32 = 0xf5f1e8;
pub const TEXT_SECONDARY: u32 = 0x9a9a9f;
pub const TEXT_MUTED: u32 = 0x6a6a70;

pub const ACCENT: u32 = 0xff5e3a;
pub const ACCENT_HOVER: u32 = 0xff7752;
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

fn build_ui_sans(weight: FontWeight) -> Font {
    Font {
        family: "Geist".into(),
        fallbacks: Some(UI_SANS_FALLBACKS.clone()),
        features: FontFeatures::default(),
        weight,
        style: FontStyle::Normal,
    }
}

static UI_SANS_NORMAL: LazyLock<Font> = LazyLock::new(|| build_ui_sans(FontWeight::NORMAL));
static UI_SANS_MEDIUM: LazyLock<Font> = LazyLock::new(|| build_ui_sans(FontWeight::MEDIUM));
static UI_SANS_SEMIBOLD: LazyLock<Font> = LazyLock::new(|| build_ui_sans(FontWeight::SEMIBOLD));
static UI_SANS_BOLD: LazyLock<Font> = LazyLock::new(|| build_ui_sans(FontWeight::BOLD));

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
    if weight == FontWeight::NORMAL {
        UI_SANS_NORMAL.clone()
    } else if weight == FontWeight::MEDIUM {
        UI_SANS_MEDIUM.clone()
    } else if weight == FontWeight::SEMIBOLD {
        UI_SANS_SEMIBOLD.clone()
    } else if weight == FontWeight::BOLD {
        UI_SANS_BOLD.clone()
    } else {
        build_ui_sans(weight)
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
    if active_theme().is_light {
        vec![
            BoxShadow::new(px(0.0), px(12.0), rgba(0x0f172a24).into()).blur_radius(px(28.0)),
            BoxShadow::new(px(0.0), px(2.0), rgba(0x0f172a14).into()).blur_radius(px(6.0)),
        ]
    } else {
        vec![
            BoxShadow::new(px(0.0), px(12.0), rgba(0x00000080).into()).blur_radius(px(32.0)),
            BoxShadow::new(px(0.0), px(2.0), rgba(0x0000004d).into()).blur_radius(px(6.0)),
        ]
    }
}

/// Lightweight single-layer shadow (`1px`/`6px` blur) for scrolled bars and floating pills,
/// avoiding the heavy full-width 32px Gaussian blur fill-rate cost of `glass_capsule_shadow()`.
pub fn subtle_bar_shadow() -> Vec<BoxShadow> {
    if active_theme().is_light {
        vec![BoxShadow::new(px(0.0), px(1.0), rgba(0x0f172a1c).into()).blur_radius(px(6.0))]
    } else {
        vec![BoxShadow::new(px(0.0), px(1.0), rgba(0x00000052).into()).blur_radius(px(6.0))]
    }
}

/// Lightweight elevation shadow for the scrolled full-width TopNav bar.
#[inline]
pub fn topnav_scrolled_shadow() -> Vec<BoxShadow> {
    subtle_bar_shadow()
}

/// Resting card elevation shadow: subtle shadow on `LightMist`, empty on dark presets where
/// dark drop shadows over obsidian canvases are imperceptible and waste GPU fragment fill-rate.
pub fn poster_card_shadow() -> Vec<BoxShadow> {
    if active_theme().is_light {
        vec![BoxShadow::new(px(0.0), px(2.0), rgba(0x0f172a1a).into()).blur_radius(px(6.0))]
    } else {
        Vec::new()
    }
}

pub fn poster_hover_glow_shadow(palette: &ThemePalette) -> Vec<BoxShadow> {
    let drop_rgba = if palette.is_light {
        0x0f172a26
    } else {
        0x00000088
    };
    let glow_alpha = if palette.is_light { 0x38 } else { 0x48 };
    vec![
        BoxShadow::new(px(0.0), px(6.0), rgba(drop_rgba).into()).blur_radius(px(14.0)),
        BoxShadow::new(
            px(0.0),
            px(0.0),
            rgba((palette.accent << 8) | glow_alpha).into(),
        )
        .blur_radius(px(12.0))
        .spread_radius(px(1.0)),
    ]
}

pub fn movietracker_mark(size_px: f32) -> Div {
    let theme = active_theme();
    let sprocket_rgba = if theme.cta_on_accent == 0xffffff {
        0x0000004d
    } else {
        0x0b0b0d99
    };
    let sprocket_col = move || {
        div()
            .h_full()
            .py(px(3.0))
            .flex()
            .flex_col()
            .justify_between()
            .children((0..4).map(move |_| {
                div()
                    .w(px(2.5))
                    .h(px(2.5))
                    .rounded(px(0.75))
                    .bg(rgba(sprocket_rgba))
            }))
    };

    div()
        .w(px(size_px))
        .h(px(size_px))
        .flex_shrink_0()
        .rounded(px(6.0))
        .bg(rgb(theme.accent))
        .px(px(3.0))
        .flex()
        .items_center()
        .justify_between()
        .child(sprocket_col())
        .child(svg_icon(
            IconName::Play,
            size_px * 0.42,
            rgb(theme.cta_on_accent),
        ))
        .child(sprocket_col())
}

pub fn meta_divider() -> Div {
    let theme = active_theme();
    div()
        .w(px(1.0))
        .h(px(11.0))
        .flex_shrink_0()
        .bg(rgba(theme.glass_divider))
}

pub fn neutral_chip(label: impl Into<SharedString>) -> Div {
    let theme = active_theme();
    div()
        .px(theme.sp(9.0))
        .py(theme.sp(3.0))
        .rounded(px(999.0))
        .bg(rgb(theme.bg_elevated))
        .border_1()
        .border_color(rgba(theme.glass_border))
        .font(mono_font())
        .text_size(theme.text_micro())
        .text_color(rgb(theme.text_primary))
        .child(label.into())
}

pub fn semantic_status_pill(
    icon: IconName,
    label: &'static str,
    fg_hex: u32,
    border_rgba: u32,
    bg_rgba: u32,
) -> Div {
    let theme = active_theme();
    div()
        .h(theme.sp(28.0))
        .px(theme.sp(11.0))
        .rounded(px(999.0))
        .bg(rgba(bg_rgba))
        .border_1()
        .border_color(rgba(border_rgba))
        .flex()
        .items_center()
        .gap(theme.sp(6.0))
        .text_size(theme.text_micro())
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(fg_hex))
        .child(svg_icon(icon, 13.0 * theme.ui_scale, rgb(fg_hex)))
        .child(label)
}

pub fn keycap(label: impl Into<SharedString>) -> Div {
    let theme = active_theme();
    div()
        .px(theme.sp(6.0))
        .py(theme.sp(2.0))
        .rounded(px(4.0))
        .bg(rgb(theme.bg_surface_3))
        .border_1()
        .border_color(rgba(theme.glass_border))
        .font(mono_font())
        .text_size(theme.text_micro())
        .text_color(rgb(theme.text_secondary))
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
        Resolution::Sd => "480p",
        Resolution::Unknown => "Unknown quality",
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

pub fn compact_hdr_badge(hdr: HdrFormat) -> Option<&'static str> {
    match hdr {
        HdrFormat::DolbyVision => Some("DV"),
        HdrFormat::Hdr10Plus => Some("HDR10+"),
        HdrFormat::Hdr10 | HdrFormat::Hlg => Some("HDR"),
        HdrFormat::Sdr => None,
    }
}

pub fn compact_audio_badge(audio: AudioCodec) -> Option<&'static str> {
    match audio {
        AudioCodec::Atmos => Some("Atmos"),
        AudioCodec::TrueHD => Some("TrueHD"),
        AudioCodec::DtsHdMa => Some("DTS-HD"),
        AudioCodec::Dts | AudioCodec::DdPlus | AudioCodec::Ac3 => Some("5.1"),
        AudioCodec::Flac => Some("FLAC"),
        AudioCodec::Opus => Some("Opus"),
        AudioCodec::Aac => Some("AAC"),
        AudioCodec::Mp3 | AudioCodec::Unknown => None,
    }
}

pub fn compact_codec_badge(codec: VideoCodec) -> Option<&'static str> {
    match codec {
        VideoCodec::AV1 => Some("AV1"),
        VideoCodec::HEVC => Some("HEVC"),
        VideoCodec::AVC => Some("AVC"),
        VideoCodec::VC1 => Some("VC-1"),
        VideoCodec::Xvid => Some("XviD"),
        VideoCodec::Mpeg2 => Some("MPEG-2"),
        VideoCodec::Unknown => None,
    }
}

pub fn parse_runtime_minutes(raw: &str) -> Option<u32> {
    let bytes = raw.trim().as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let mut total_mins: u32 = 0;
    let mut current_num: u32 = 0;
    let mut has_num = false;
    let mut found_unit = false;
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_digit() {
            current_num = current_num
                .checked_mul(10)?
                .checked_add(u32::from(b - b'0'))?;
            has_num = true;
            i += 1;
            continue;
        }
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if has_num {
            let unit_start = b.to_ascii_lowercase();
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            if unit_start == b'h' {
                total_mins = total_mins.saturating_add(current_num.saturating_mul(60));
                found_unit = true;
            } else if unit_start == b'm' {
                total_mins = total_mins.saturating_add(current_num);
                found_unit = true;
            }
            current_num = 0;
            has_num = false;
        } else {
            i += 1;
        }
    }

    if has_num && !found_unit {
        total_mins = total_mins.saturating_add(current_num);
    }

    (total_mins > 0).then_some(total_mins)
}

pub fn estimate_bitrate_mbps(size_bytes: Option<u64>, duration_minutes: Option<u32>) -> Option<f64> {
    let bytes = size_bytes?;
    let mins = duration_minutes?;
    if bytes == 0 || mins == 0 {
        return None;
    }
    let mbps = (bytes as f64 * 8.0) / (f64::from(mins) * 60.0 * 1_000_000.0);
    (mbps.is_finite() && mbps > 0.0).then_some(mbps)
}

pub fn format_bitrate_mbps(mbps: f64) -> String {
    if !mbps.is_finite() || mbps <= 0.0 {
        return "~0.0 Mbps".to_owned();
    }
    format!("~{mbps:.1} Mbps")
}

pub fn format_episode_air_date(dt: chrono::DateTime<chrono::Utc>) -> String {
    dt.format("%b %-d, %Y").to_string()
}

#[inline]
pub fn placeholder_tint_for_id_with_preset(id: &str, preset: ThemePreset) -> u32 {
    let mut hash: u32 = 2166136261;
    for b in id.bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(16777619);
    }
    let palette = match preset {
        ThemePreset::LightMist => [0xe3e8f0, 0xe7e5ec, 0xe0e7ed, 0xe8e4e6],
        ThemePreset::PureOled => [0x0c0c0f, 0x100f12, 0x0e1014, 0x121116],
        ThemePreset::AuroraIndigo => [0x14192e, 0x171c34, 0x151b30, 0x1a1e38],
        ThemePreset::WarmCinema => [0x1c1614, 0x201916, 0x1d1715, 0x231b18],
        ThemePreset::EtherealDark => [0x1c1c20, 0x201e20, 0x1d1f23, 0x222024],
    };
    palette[(hash as usize) % palette.len()]
}

#[inline]
pub fn placeholder_tint_for_id(id: &str) -> u32 {
    placeholder_tint_for_id_with_preset(id, active_theme().preset)
}

/// Deterministic editorial gradient pair `(top_rgb, bottom_rgb)` for styled fallback posters
/// when title artwork is unavailable.
pub fn placeholder_gradient_for_seed(seed: u32, is_light: bool) -> (u32, u32) {
    let dark_pairs: [(u32, u32); 6] = [
        (0x1E293B, 0x0B1120), // Midnight slate
        (0x2E1A3B, 0x110B1A), // Deep plum
        (0x132A2E, 0x091317), // Abyssal teal
        (0x321D1A, 0x140B0A), // Warm cinema umber
        (0x1B233A, 0x0B0E1A), // Nocturnal indigo
        (0x182B24, 0x0A1310), // Forest obsidian
    ];
    let light_pairs: [(u32, u32); 6] = [
        (0xE2E8F0, 0xCBD5E1),
        (0xEDE9FE, 0xDDD6FE),
        (0xCCFBF1, 0x99F6E4),
        (0xFFEDD5, 0xFED7AA),
        (0xE0E7FF, 0xC7D2FE),
        (0xD1FAE5, 0xA7F3D0),
    ];
    let idx = (seed as usize) % dark_pairs.len();
    if is_light {
        light_pairs[idx]
    } else {
        dark_pairs[idx]
    }
}

/// Normalizes ongoing series release ranges like `"2024–"` or `"2024-"` into `"2024 – present"`.
pub fn format_release_info(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(prefix) = trimmed
        .strip_suffix('–')
        .or_else(|| trimmed.strip_suffix('-'))
        .or_else(|| trimmed.strip_suffix('—'))
    {
        let year = prefix.trim();
        if year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit()) {
            return format!("{year} – present");
        }
    }
    trimmed.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_and_bitrate_helpers_parse_and_estimate_cleanly() {
        assert_eq!(parse_runtime_minutes("52 min"), Some(52));
        assert_eq!(parse_runtime_minutes("1h 42m"), Some(102));
        assert_eq!(parse_runtime_minutes(" 1H  42MIN "), Some(102));
        assert_eq!(parse_runtime_minutes("2h"), Some(120));
        assert_eq!(parse_runtime_minutes("102"), Some(102));
        assert_eq!(parse_runtime_minutes("0 min"), None);
        assert_eq!(parse_runtime_minutes("999999999999999999m"), None);
        assert_eq!(parse_runtime_minutes(""), None);

        let bytes_18_4_gb = (18.4_f64 * 1_000_000_000.0) as u64;
        let mbps = estimate_bitrate_mbps(Some(bytes_18_4_gb), Some(100)).unwrap();
        assert!((mbps - 24.533).abs() < 0.1);
        assert_eq!(format_bitrate_mbps(mbps), "~24.5 Mbps");
        assert_eq!(estimate_bitrate_mbps(None, Some(100)), None);
        assert_eq!(estimate_bitrate_mbps(Some(1024), None), None);
    }

    #[test]
    fn theme_presets_and_accents_build_valid_contrasting_palettes() {
        assert_eq!(ThemePreset::ALL.len(), 5);
        assert_eq!(AccentChoice::ALL.len(), 6);
        assert_eq!(ThemePreset::default(), ThemePreset::EtherealDark);
        assert_eq!(AccentChoice::default(), AccentChoice::EmberCoral);

        for preset in ThemePreset::ALL {
            assert!(!preset.label().is_empty());
            assert!(!preset.subtitle().is_empty());

            for accent in AccentChoice::ALL {
                assert!(!accent.label().is_empty());
                let p = build_palette(
                    preset,
                    accent,
                    true,
                    UiScaleChoice::S100.factor(),
                    CardDensityChoice::Comfortable,
                );
                assert_eq!(p.preset, preset);
                assert_eq!(p.accent_choice, accent);
                assert_eq!(p.is_light, preset == ThemePreset::LightMist);
                assert_eq!(p.accent, accent.hex());
                assert_eq!(p.accent_hover, accent.hover_hex());
                assert_eq!(p.accent_press, accent.press_hex());
                assert_eq!(p.cta_on_accent, accent.on_accent_hex());
                assert_ne!(p.bg_canvas, p.text_primary);
                assert_ne!(p.segment_active_bg, p.segment_active_text);
                assert_eq!(p.accent_soft >> 8, accent.hex());
                assert_eq!(p.border_accent >> 8, accent.hex());
                assert_eq!(p.border_poster_hover, (accent.hex() << 8) | 0xff);
                assert!(p.ambient_bleed);
                assert!((p.ui_scale - 1.0).abs() < f32::EPSILON);
                assert_eq!(p.density, CardDensityChoice::Comfortable);

                if preset == ThemePreset::PureOled {
                    assert_eq!(p.bg_canvas, 0x000000);
                }
                if preset == ThemePreset::LightMist {
                    assert_eq!(p.text_primary, 0x12141A);
                    assert_eq!(p.glass_card_bg, 0xFFFFFFB8);
                    assert_eq!(p.glass_border, 0x00000018);
                }
            }
        }
    }

    #[test]
    fn ui_scale_and_card_density_helpers_are_ordered_and_roundtrip_serde() {
        assert_eq!(UiScaleChoice::default(), UiScaleChoice::S100);
        assert_eq!(UiScaleChoice::ALL.len(), 4);
        assert!((UiScaleChoice::S100.factor() - 1.0).abs() < f32::EPSILON);

        let factors: Vec<f32> = UiScaleChoice::ALL.iter().map(|s| s.factor()).collect();
        assert!(factors.windows(2).all(|w| w[0] < w[1]));
        for scale in UiScaleChoice::ALL {
            assert!(!scale.label().is_empty());
            let json = serde_json::to_string(&scale).unwrap();
            let decoded: UiScaleChoice = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, scale);
        }

        assert_eq!(CardDensityChoice::default(), CardDensityChoice::Comfortable);
        assert_eq!(CardDensityChoice::ALL.len(), 3);
        let widths: Vec<f32> = CardDensityChoice::ALL
            .iter()
            .map(|d| d.target_poster_width_px())
            .collect();
        let gaps: Vec<f32> = CardDensityChoice::ALL
            .iter()
            .map(|d| d.grid_gap_px())
            .collect();
        assert!(widths.windows(2).all(|w| w[0] < w[1]));
        assert!(gaps.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(CardDensityChoice::Comfortable.target_poster_width_px(), 216.0);
        assert_eq!(CardDensityChoice::Comfortable.grid_gap_px(), 20.0);
        for density in CardDensityChoice::ALL {
            assert!(!density.label().is_empty());
            let json = serde_json::to_string(&density).unwrap();
            let decoded: CardDensityChoice = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, density);
        }
    }

    #[test]
    fn active_theme_rwlock_updates_and_scales_sp_pixels() {
        let initial = active_theme();
        let v0 = active_theme_version();
        assert_eq!(initial.preset, ThemePreset::EtherealDark);
        assert_eq!(initial.accent_choice, AccentChoice::EmberCoral);

        set_active_theme(
            ThemePreset::LightMist,
            AccentChoice::ElectricViolet,
            false,
            UiScaleChoice::S125.factor(),
            CardDensityChoice::Cinema,
        );
        let v1 = active_theme_version();
        assert!(v1 > v0);
        let updated = active_theme();
        assert_eq!(updated.preset, ThemePreset::LightMist);
        assert!(updated.is_light);
        assert_eq!(updated.accent_choice, AccentChoice::ElectricViolet);
        assert!(!updated.ambient_bleed);
        assert_eq!(updated.density, CardDensityChoice::Cinema);
        assert_eq!(sp(16.0), px(20.0));
        assert_eq!(updated.sp(16.0), px(20.0));
        assert_eq!(poster_card_shadow().len(), 1);
        assert_eq!(subtle_bar_shadow().len(), 1);
        assert_eq!(topnav_scrolled_shadow().len(), 1);

        // Invalid scale falls back safely to 1.0.
        let sanitized = build_palette(
            ThemePreset::AuroraIndigo,
            AccentChoice::AuroraCyan,
            true,
            f32::NAN,
            CardDensityChoice::Compact,
        );
        assert!((sanitized.ui_scale - 1.0).abs() < f32::EPSILON);

        // Restore default active theme so parallel UI tests see default state.
        set_active_theme(
            ThemePreset::EtherealDark,
            AccentChoice::EmberCoral,
            true,
            1.0,
            CardDensityChoice::Comfortable,
        );
        assert!(active_theme_version() > v1);
        assert_eq!(sp(16.0), px(16.0));
        assert_eq!(active_theme().sp(16.0), px(16.0));
        assert!(poster_card_shadow().is_empty());
        assert_eq!(subtle_bar_shadow().len(), 1);
    }

    #[test]
    fn type_scale_and_release_info_formatting_behave_consistently() {
        let p = build_palette(
            ThemePreset::PureOled,
            AccentChoice::AuroraCyan,
            true,
            1.0,
            CardDensityChoice::Comfortable,
        );
        assert_eq!(p.text_micro(), px(12.0));
        assert_eq!(p.text_body(), px(14.0));
        assert_eq!(p.text_subhead(), px(16.0));
        assert_eq!(p.text_title(), px(20.0));
        assert_eq!(p.text_display(), px(28.0));
        assert_eq!(p.text_hero(), px(38.0));
        assert_eq!(format_release_info("2024–"), "2024 – present");
        assert_eq!(format_release_info("2024-"), "2024 – present");
        assert_eq!(format_release_info("2011–2019"), "2011–2019");
        assert_eq!(format_release_info("2026"), "2026");
    }
}
