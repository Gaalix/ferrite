use std::borrow::Cow;

use gpui::{px, svg, AssetSource, Hsla, Result, SharedString, Styled, Svg};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum IconWeight {
    #[default]
    Regular,
    Fill,
}

impl IconWeight {
    pub const ALL: [Self; 2] = [Self::Regular, Self::Fill];
}

macro_rules! define_phosphor_icons {
    ($( $variant:ident => $slug:literal ),+ $(,)?) => {
        const ICON_VARIANT_COUNT: usize = 0 $(+ {
            let _ = stringify!($variant);
            1
        })+;

        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum IconName {
            $( $variant, )+
        }

        impl IconName {
            pub const ALL: [Self; ICON_VARIANT_COUNT] = [
                $( Self::$variant, )+
            ];

            pub const fn phosphor_name(self) -> &'static str {
                match self {
                    $( Self::$variant => $slug, )+
                }
            }

            pub const fn default_weight(self) -> IconWeight {
                match self {
                    Self::Play | Self::Pause | Self::Star | Self::Zap => IconWeight::Fill,
                    _ => IconWeight::Regular,
                }
            }

            pub const fn path_with_weight(self, weight: IconWeight) -> &'static str {
                match (self, weight) {
                    $(
                        (Self::$variant, IconWeight::Regular) => {
                            concat!("icons/phosphor/regular/", $slug, ".svg")
                        }
                        (Self::$variant, IconWeight::Fill) => {
                            concat!("icons/phosphor/fill/", $slug, "-fill.svg")
                        }
                    )+
                }
            }

            pub const fn path(self) -> &'static str {
                self.path_with_weight(self.default_weight())
            }

            pub const fn svg_bytes_with_weight(self, weight: IconWeight) -> &'static [u8] {
                match (self, weight) {
                    $(
                        (Self::$variant, IconWeight::Regular) => include_bytes!(concat!(
                            "../../../assets/icons/phosphor/regular/",
                            $slug,
                            ".svg"
                        )),
                        (Self::$variant, IconWeight::Fill) => include_bytes!(concat!(
                            "../../../assets/icons/phosphor/fill/",
                            $slug,
                            "-fill.svg"
                        )),
                    )+
                }
            }

            pub const fn svg_bytes(self) -> &'static [u8] {
                self.svg_bytes_with_weight(self.default_weight())
            }
        }
    };
}

define_phosphor_icons! {
    Home => "house",
    Compass => "compass",
    Bookmark => "bookmark-simple",
    Layers => "stack",
    Search => "magnifying-glass",
    Settings => "gear-six",
    User => "user",
    Play => "play",
    Pause => "pause",
    SkipForward => "skip-forward",
    RotateCcw => "arrow-counter-clockwise",
    RotateCw => "arrow-clockwise",
    Volume2 => "speaker-high",
    VolumeX => "speaker-slash",
    Subtitles => "subtitles",
    Sparkles => "sparkle",
    Zap => "lightning",
    Users => "users",
    HardDrive => "hard-drive",
    Cpu => "cpu",
    Maximize => "corners-out",
    Minimize => "corners-in",
    Sliders => "sliders-horizontal",
    Check => "check",
    Plus => "plus",
    X => "x",
    ChevronRight => "caret-right",
    ChevronLeft => "caret-left",
    Film => "film-strip",
    Tv => "television",
    ShieldCheck => "shield-check",
    Star => "star",
    Music => "music-note",
    RefreshCw => "arrows-clockwise",
    PanelLeft => "sidebar-simple",
    StepBack => "skip-back",
    StepForward => "skip-forward",
    Clock => "clock",
    ChevronDown => "caret-down",
    Info => "info",
    Minus => "minus",
    MaximizeWindow => "square",
    RestoreWindow => "copy",
    ArrowUp => "arrow-up",
    ArrowDown => "arrow-down",
    ArrowUpToLine => "arrow-line-up",
    ExternalLink => "arrow-square-out",
    Clipboard => "clipboard",
    Trash2 => "trash",
    FolderOpen => "folder-open",
    Replay10 => "clock-counter-clockwise",
    Forward10 => "clock-clockwise",
    ClosedCaption => "closed-captioning",
    FileText => "file-text",
    Calendar => "calendar-blank",
    Eye => "eye",
    EyeOff => "eye-slash",
    Pin => "push-pin",
    MoreHorizontal => "dots-three",
    Clapperboard => "film-slate",
    Shuffle => "shuffle",
    GridFour => "grid-four",
    Rows => "rows",
    List => "list",
    Palette => "palette",
    Sun => "sun",
    Moon => "moon",
    Fire => "fire",
    ArrowLeft => "arrow-left",
    ArrowRight => "arrow-right",
    CaretUp => "caret-up",
    Funnel => "funnel",
    Monitor => "monitor",
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EmbeddedIconAssets;

impl AssetSource for EmbeddedIconAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        for icon in IconName::ALL {
            for weight in IconWeight::ALL {
                if icon.path_with_weight(weight) == path {
                    return Ok(Some(Cow::Borrowed(icon.svg_bytes_with_weight(weight))));
                }
            }
        }
        Ok(None)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let prefix = path.trim_matches('/');
        let mut out: Vec<SharedString> = Vec::new();
        for icon in IconName::ALL {
            for weight in IconWeight::ALL {
                let candidate = icon.path_with_weight(weight);
                if (prefix.is_empty() || candidate.starts_with(prefix))
                    && !out.iter().any(|existing| existing.as_ref() == candidate)
                {
                    out.push(SharedString::new_static(candidate));
                }
            }
        }
        Ok(out)
    }
}

#[inline]
pub fn svg_icon_weighted(
    name: IconName,
    weight: IconWeight,
    size_px: f32,
    color: impl Into<Hsla>,
) -> Svg {
    svg()
        .path(name.path_with_weight(weight))
        .size(px(size_px))
        .flex_shrink_0()
        .text_color(color.into())
}

#[inline]
pub fn svg_icon(name: IconName, size_px: f32, color: impl Into<Hsla>) -> Svg {
    svg_icon_weighted(name, name.default_weight(), size_px, color)
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, sync::Arc};

    use gpui::{size, DevicePixels, SvgRenderer, SvgSize};

    use super::*;

    #[test]
    fn all_embedded_icons_load_valid_phosphor_svg_markup_for_both_weights() {
        let assets = EmbeddedIconAssets;
        let listed = assets.list("icons").expect("list icons");
        let unique_slugs: HashSet<&'static str> =
            IconName::ALL.iter().map(|i| i.phosphor_name()).collect();

        assert_eq!(IconName::ALL.len(), 73);
        assert_eq!(unique_slugs.len(), 72);
        assert_eq!(listed.len(), unique_slugs.len() * 2);
        assert_eq!(assets.list("icons/phosphor/regular").unwrap().len(), 72);
        assert_eq!(assets.list("icons/phosphor/fill").unwrap().len(), 72);

        let renderer = SvgRenderer::new(Arc::new(assets));

        for icon in IconName::ALL {
            for weight in IconWeight::ALL {
                let path = icon.path_with_weight(weight);
                let raw = assets
                    .load(path)
                    .expect("load result")
                    .unwrap_or_else(|| panic!("missing embedded icon {path}"));
                let text = std::str::from_utf8(&raw).expect("valid utf8 svg");
                assert!(
                    text.starts_with("<svg ") && text.ends_with("</svg>"),
                    "invalid svg wrapper for {path}"
                );
                assert!(
                    text.contains("viewBox=\"0 0 256 256\""),
                    "missing Phosphor viewBox for {path}"
                );
                assert!(
                    text.contains("fill=\"currentColor\""),
                    "missing currentColor fill for {path}"
                );

                let parsed = renderer
                    .parse_svg(&raw)
                    .unwrap_or_else(|err| panic!("failed to parse {path}: {err}"));
                let rendered = renderer
                    .render_parsed(&parsed, SvgSize::Size(size(DevicePixels(32), DevicePixels(32))))
                    .unwrap_or_else(|err| panic!("failed to rasterize {path}: {err}"));
                assert_eq!(rendered.size(0), size(DevicePixels(32), DevicePixels(32)));
                let bytes = rendered.as_bytes(0).expect("render image frame bytes");
                let (pixels, remainder) = bytes.as_chunks::<4>();
                assert!(remainder.is_empty(), "expected BGRA pixels for {path}");
                assert!(
                    pixels.iter().any(|bgra| bgra[3] > 0),
                    "rasterized {path} produced an empty alpha mask"
                );
            }
        }
    }

    #[test]
    fn default_weights_and_missing_assets_behave_as_expected() {
        assert_eq!(IconName::Play.default_weight(), IconWeight::Fill);
        assert_eq!(IconName::Pause.default_weight(), IconWeight::Fill);
        assert_eq!(IconName::Star.default_weight(), IconWeight::Fill);
        assert_eq!(IconName::Zap.default_weight(), IconWeight::Fill);
        assert_eq!(IconName::Home.default_weight(), IconWeight::Regular);
        assert_eq!(IconName::Shuffle.default_weight(), IconWeight::Regular);
        assert_eq!(IconWeight::default(), IconWeight::Regular);

        assert_eq!(IconName::Play.path(), "icons/phosphor/fill/play-fill.svg");
        assert_eq!(IconName::Home.path(), "icons/phosphor/regular/house.svg");

        let assets = EmbeddedIconAssets;
        assert!(assets.load("icons/nonexistent.svg").unwrap().is_none());
    }
}
