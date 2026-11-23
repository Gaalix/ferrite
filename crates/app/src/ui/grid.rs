use stremio_core_runtime::stremio_core::types::resource::PosterShape;

use crate::{
    theme::ThemePalette,
    ui::{
        cards::{landscape_card_width_px, PosterMetrics},
        layout::page_gutter_px,
    },
    virtual_grid::{compute_dynamic_grid_metrics, DynamicGridMetrics, PosterAspect},
};

/// Thin canonical wrapper over `virtual_grid::compute_dynamic_grid_metrics` that applies
/// `page_gutter_px(viewport_w)` on both horizontal edges and density/scale-aware `PosterMetrics`.
pub fn compute_app_grid_metrics(
    total_items: usize,
    viewport_w: f32,
    sidebar_w: f32,
    aspect: PosterAspect,
    theme: &ThemePalette,
) -> DynamicGridMetrics {
    let poster = PosterMetrics::from_theme(theme);
    let horizontal_padding_px = page_gutter_px(viewport_w) * 2.0;

    compute_dynamic_grid_metrics(
        total_items,
        viewport_w,
        sidebar_w,
        horizontal_padding_px,
        None,
        poster.card_width_px,
        poster.gap_px,
        poster.gap_px,
        poster.metadata_height_px,
        aspect,
    )
}

/// Companion wrapper for 16:9 widescreen card grids (e.g., Library's Wide Backdrops view mode)
/// using `landscape_card_width_px`, `page_gutter_px`, and `PosterMetrics` gap/metadata height.
pub fn compute_app_landscape_grid_metrics(
    total_items: usize,
    viewport_w: f32,
    sidebar_w: f32,
    theme: &ThemePalette,
) -> DynamicGridMetrics {
    let poster = PosterMetrics::from_theme(theme);
    let horizontal_padding_px = page_gutter_px(viewport_w) * 2.0;
    let min_wide_w = landscape_card_width_px(theme.density, theme.ui_scale);

    compute_dynamic_grid_metrics(
        total_items,
        viewport_w,
        sidebar_w,
        horizontal_padding_px,
        None,
        min_wide_w,
        poster.gap_px,
        poster.gap_px,
        poster.metadata_height_px,
        PosterAspect::Landscape16x9,
    )
}

/// Infers the dominant `PosterAspect` from a sample of Stremio `PosterShape` values
/// (sampling up to the first 24 items, preferring Portrait if mixed).
pub fn infer_grid_poster_aspect<'a>(shapes: impl IntoIterator<Item = &'a PosterShape>) -> PosterAspect {
    let mut has_portrait = false;
    let mut has_square = false;
    let mut has_landscape = false;

    for shape in shapes.into_iter().take(24) {
        match shape {
            PosterShape::Poster => has_portrait = true,
            PosterShape::Square => has_square = true,
            PosterShape::Landscape => has_landscape = true,
        }
    }

    if has_portrait {
        PosterAspect::Portrait2x3
    } else if has_square && !has_landscape {
        PosterAspect::Square1x1
    } else if has_landscape && !has_square {
        PosterAspect::Landscape16x9
    } else if has_square {
        PosterAspect::Square1x1
    } else {
        PosterAspect::Portrait2x3
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{build_palette, AccentChoice, CardDensityChoice, ThemePreset, UiScaleChoice};

    #[test]
    fn compute_app_grid_metrics_applies_page_gutter_and_poster_metrics() {
        for density in CardDensityChoice::ALL {
            for scale_choice in UiScaleChoice::ALL {
                let theme = build_palette(
                    ThemePreset::EtherealDark,
                    AccentChoice::EmberCoral,
                    true,
                    scale_choice.factor(),
                    density,
                );
                let poster = PosterMetrics::from_theme(&theme);

                for &(viewport_w, sidebar_w) in &[
                    (960.0_f32, 0.0_f32),
                    (1440.0, 0.0),
                    (1440.0, 248.0 * theme.ui_scale),
                    (2560.0, 0.0),
                ] {
                    for aspect in [
                        PosterAspect::Portrait2x3,
                        PosterAspect::Landscape16x9,
                        PosterAspect::Square1x1,
                    ] {
                        let actual =
                            compute_app_grid_metrics(120, viewport_w, sidebar_w, aspect, &theme);
                        let expected = compute_dynamic_grid_metrics(
                            120,
                            viewport_w,
                            sidebar_w,
                            page_gutter_px(viewport_w) * 2.0,
                            None,
                            poster.card_width_px,
                            poster.gap_px,
                            poster.gap_px,
                            poster.metadata_height_px,
                            aspect,
                        );
                        assert_eq!(actual, expected);
                        assert!(actual.columns >= 2 && actual.columns <= 12);
                        assert!(actual.card_width_px >= poster.card_width_px);
                        assert!(
                            actual.card_height_px <= actual.row_stride_px - poster.gap_px,
                            "card clipped for density={density:?}, scale={scale_choice:?}, vw={viewport_w}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn compute_app_grid_metrics_reflects_density_ordering() {
        let compact = build_palette(
            ThemePreset::EtherealDark,
            AccentChoice::EmberCoral,
            true,
            1.0,
            CardDensityChoice::Compact,
        );
        let cinema = build_palette(
            ThemePreset::EtherealDark,
            AccentChoice::EmberCoral,
            true,
            1.0,
            CardDensityChoice::Cinema,
        );

        let m_compact =
            compute_app_grid_metrics(48, 1600.0, 0.0, PosterAspect::Portrait2x3, &compact);
        let m_cinema =
            compute_app_grid_metrics(48, 1600.0, 0.0, PosterAspect::Portrait2x3, &cinema);

        assert!(
            m_compact.columns > m_cinema.columns,
            "Compact ({}) should fit more columns than Cinema ({})",
            m_compact.columns,
            m_cinema.columns
        );
    }

    #[test]
    fn infer_grid_poster_aspect_resolves_uniform_and_mixed_shapes() {
        assert_eq!(infer_grid_poster_aspect([]), PosterAspect::Portrait2x3);
        assert_eq!(
            infer_grid_poster_aspect(&[PosterShape::Landscape, PosterShape::Landscape]),
            PosterAspect::Landscape16x9
        );
        assert_eq!(
            infer_grid_poster_aspect(&[PosterShape::Square]),
            PosterAspect::Square1x1
        );
        assert_eq!(
            infer_grid_poster_aspect(&[PosterShape::Landscape, PosterShape::Poster]),
            PosterAspect::Portrait2x3
        );
    }
}
