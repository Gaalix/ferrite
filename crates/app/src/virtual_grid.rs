use std::ops::Range;

use stremio_core_runtime::stremio_core::types::resource::PosterShape;

/// Aspect ratio presets for poster and episode cards in dynamic grids and carousels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PosterAspect {
    #[default]
    Portrait2x3,
    Landscape16x9,
    Square1x1,
}

impl PosterAspect {
    /// Returns `height / width` for computing card image height from width.
    pub const fn height_over_width(self) -> f32 {
        match self {
            Self::Portrait2x3 => 1.5,
            Self::Landscape16x9 => 9.0 / 16.0,
            Self::Square1x1 => 1.0,
        }
    }

    /// Returns `width / height` for GPUI `.aspect_ratio(...)` calls.
    pub const fn width_over_height(self) -> f32 {
        match self {
            Self::Portrait2x3 => 2.0 / 3.0,
            Self::Landscape16x9 => 16.0 / 9.0,
            Self::Square1x1 => 1.0,
        }
    }

    /// Maps a Stremio catalog `PosterShape` into the matching `PosterAspect`.
    pub fn from_stremio_poster_shape(shape: &PosterShape) -> Self {
        match shape {
            PosterShape::Poster => Self::Portrait2x3,
            PosterShape::Landscape => Self::Landscape16x9,
            PosterShape::Square => Self::Square1x1,
        }
    }
}

/// Fluid grid dimensions and corresponding virtual row layout, ensuring cards never clip vertically.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DynamicGridMetrics {
    pub columns: usize,
    pub card_width_px: f32,
    pub image_height_px: f32,
    pub card_height_px: f32,
    pub row_stride_px: f32,
    pub layout: VirtualGridLayout,
}

/// Precomputed virtual grid geometry for a fixed-row-stride poster grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VirtualGridLayout {
    pub columns: usize,
    pub row_count: usize,
    pub row_stride_px: f32,
}

impl VirtualGridLayout {
    /// Builds a virtual grid layout for `total_items` distributed across `columns`.
    pub fn new(total_items: usize, columns: usize, row_stride_px: f32) -> Self {
        let clamped_cols = columns.max(1);
        let row_count = if total_items == 0 || columns == 0 {
            0
        } else {
            total_items.div_ceil(clamped_cols)
        };
        Self {
            columns: clamped_cols,
            row_count,
            row_stride_px: if row_stride_px.is_finite() && row_stride_px > 0.0 {
                row_stride_px
            } else {
                0.0
            },
        }
    }

    /// Computes the visible row range for the given scroll offset and viewport height.
    pub fn visible_row_range(
        &self,
        total_items: usize,
        scroll_offset_y_px: f32,
        viewport_height_px: f32,
        overscan_rows: usize,
    ) -> Range<usize> {
        compute_visible_row_range(
            total_items,
            self.columns,
            scroll_offset_y_px,
            viewport_height_px,
            self.row_stride_px,
            overscan_rows,
        )
    }
}

/// Computes the responsive column count for a poster grid, clamped to `2..=12`.
pub fn compute_poster_grid_columns(
    viewport_width_px: f32,
    sidebar_width_px: f32,
    horizontal_padding_px: f32,
    min_card_width_px: f32,
    gap_px: f32,
) -> usize {
    if !viewport_width_px.is_finite()
        || !sidebar_width_px.is_finite()
        || !horizontal_padding_px.is_finite()
        || !min_card_width_px.is_finite()
        || !gap_px.is_finite()
        || min_card_width_px <= 0.0
    {
        return 2;
    }

    let gap = gap_px.max(0.0);
    let available = (viewport_width_px - sidebar_width_px.max(0.0) - horizontal_padding_px.max(0.0))
        .max(0.0);
    let unit = min_card_width_px + gap;
    if unit <= 0.0 {
        return 2;
    }

    let raw = ((available + gap) / unit).floor();
    if !raw.is_finite() || raw < 2.0 {
        2
    } else {
        (raw as usize).clamp(2, 12)
    }
}

/// Computes responsive column counts, fluid card width/height, and virtual row stride so grid rows
/// never clip card posters or metadata across any viewport width.
#[allow(clippy::too_many_arguments)]
pub fn compute_dynamic_grid_metrics(
    total_items: usize,
    viewport_width_px: f32,
    sidebar_width_px: f32,
    horizontal_padding_px: f32,
    max_container_width_px: Option<f32>,
    min_card_width_px: f32,
    col_gap_px: f32,
    row_gap_px: f32,
    metadata_height_px: f32,
    aspect: PosterAspect,
) -> DynamicGridMetrics {
    let min_card_w = if min_card_width_px.is_finite() && min_card_width_px > 0.0 {
        min_card_width_px
    } else {
        176.0
    };
    let sidebar = if sidebar_width_px.is_finite() {
        sidebar_width_px.max(0.0)
    } else {
        0.0
    };
    let h_pad = if horizontal_padding_px.is_finite() {
        horizontal_padding_px.max(0.0)
    } else {
        0.0
    };
    let col_gap = if col_gap_px.is_finite() {
        col_gap_px.max(0.0)
    } else {
        0.0
    };
    let row_gap = if row_gap_px.is_finite() {
        row_gap_px.max(0.0)
    } else {
        0.0
    };
    let meta_h = if metadata_height_px.is_finite() {
        metadata_height_px.max(0.0)
    } else {
        0.0
    };

    let bounded_viewport = if viewport_width_px.is_finite() && viewport_width_px > 0.0 {
        match max_container_width_px {
            Some(max_w) if max_w.is_finite() => viewport_width_px.min(max_w.max(0.0)),
            _ => viewport_width_px,
        }
    } else {
        0.0
    };

    let effective_available = (bounded_viewport - sidebar - h_pad).max(0.0);
    let columns = compute_poster_grid_columns(bounded_viewport, sidebar, h_pad, min_card_w, col_gap);

    let card_width_px = if columns > 0 {
        ((effective_available - col_gap * (columns as f32 - 1.0)) / columns as f32).max(min_card_w)
    } else {
        min_card_w
    };
    let image_height_px = (card_width_px * aspect.height_over_width()).ceil();
    let card_height_px = image_height_px + meta_h;
    let mut row_stride_px = card_height_px + row_gap;
    if row_stride_px - row_gap < card_height_px {
        row_stride_px = f32::from_bits(row_stride_px.to_bits() + 1);
    }
    let layout = VirtualGridLayout::new(total_items, columns, row_stride_px);

    DynamicGridMetrics {
        columns,
        card_width_px,
        image_height_px,
        card_height_px,
        row_stride_px,
        layout,
    }
}

/// Computes the O(1) visible row range (inclusive of overscan rows above and below).
pub fn compute_visible_row_range(
    total_items: usize,
    columns: usize,
    scroll_offset_y_px: f32,
    viewport_height_px: f32,
    row_stride_px: f32,
    overscan_rows: usize,
) -> Range<usize> {
    if total_items == 0
        || columns == 0
        || !scroll_offset_y_px.is_finite()
        || !viewport_height_px.is_finite()
        || !row_stride_px.is_finite()
        || viewport_height_px <= 0.0
        || row_stride_px <= 0.0
    {
        return 0..0;
    }

    let row_count = total_items.div_ceil(columns);
    let scroll_top = scroll_offset_y_px.max(0.0);
    let first_visible = (scroll_top / row_stride_px).floor() as usize;
    let last_visible_excl = ((scroll_top + viewport_height_px) / row_stride_px).ceil() as usize;

    let start = first_visible.saturating_sub(overscan_rows).min(row_count);
    let end = last_visible_excl.saturating_add(overscan_rows).min(row_count);

    start..end.max(start)
}

/// Returns the half-open item index range belonging to `row_idx`.
pub fn slice_row_items(total_items: usize, columns: usize, row_idx: usize) -> Range<usize> {
    if total_items == 0 || columns == 0 {
        return 0..0;
    }

    let start = row_idx.saturating_mul(columns);
    if start >= total_items {
        return 0..0;
    }

    let end = start.saturating_add(columns).min(total_items);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hint::black_box;
    use std::time::Instant;

    #[test]
    fn poster_grid_columns_scales_and_clamps_to_2_through_12() {
        assert_eq!(
            compute_poster_grid_columns(320.0, 0.0, 64.0, 176.0, 18.0),
            2
        );
        assert_eq!(
            compute_poster_grid_columns(0.0, 0.0, 64.0, 176.0, 18.0),
            2
        );
        assert_eq!(
            compute_poster_grid_columns(f32::NAN, 0.0, 64.0, 176.0, 18.0),
            2
        );
        assert_eq!(
            compute_poster_grid_columns(1280.0, 0.0, 64.0, 176.0, 18.0),
            6
        );
        assert_eq!(
            compute_poster_grid_columns(1440.0, 0.0, 64.0, 176.0, 18.0),
            7
        );
        assert_eq!(
            compute_poster_grid_columns(3840.0, 240.0, 64.0, 176.0, 18.0),
            12
        );
    }

    #[test]
    fn slice_row_items_handles_full_tail_and_out_of_bounds_rows() {
        assert_eq!(slice_row_items(0, 6, 0), 0..0);
        assert_eq!(slice_row_items(14, 0, 0), 0..0);
        assert_eq!(slice_row_items(14, 6, 0), 0..6);
        assert_eq!(slice_row_items(14, 6, 1), 6..12);
        assert_eq!(slice_row_items(14, 6, 2), 12..14);
        assert_eq!(slice_row_items(14, 6, 3), 0..0);
    }

    #[test]
    fn constant_mounted_element_count_across_100_5000_50000_items() {
        let columns = 6;
        let row_stride_px = 336.0;
        let viewport_height_px = 900.0;
        let overscan_rows = 2;
        // Max rows mounted = ceil(900 / 336) + 1 (partial straddle) + 2*overscan = 3 + 1 + 4 = 8
        let max_expected_rows = 8;
        let max_expected_items = max_expected_rows * columns;

        let mut mounted_counts = Vec::new();
        for &total_items in &[100usize, 5_000, 50_000] {
            let layout = VirtualGridLayout::new(total_items, columns, row_stride_px);
            assert_eq!(layout.row_count, total_items.div_ceil(columns));

            let mid_scroll_px = (layout.row_count / 2) as f32 * row_stride_px + 120.0;
            for &scroll_y in &[0.0_f32, mid_scroll_px] {
                let rows = layout.visible_row_range(
                    total_items,
                    scroll_y,
                    viewport_height_px,
                    overscan_rows,
                );
                assert!(
                    rows.len() <= max_expected_rows,
                    "mounted rows {} exceeded bound {max_expected_rows} for {total_items} items",
                    rows.len()
                );

                let mounted_items: usize = rows
                    .clone()
                    .map(|row_idx| slice_row_items(total_items, columns, row_idx).len())
                    .sum();
                assert!(
                    mounted_items <= max_expected_items,
                    "mounted items {mounted_items} exceeded bound {max_expected_items} for {total_items} items"
                );
                if scroll_y > 0.0 {
                    mounted_counts.push((rows.len(), mounted_items));
                }
            }
        }

        // Mid-viewport mounted count must be identical across 100, 5_000, and 50_000 items.
        assert_eq!(mounted_counts[0], mounted_counts[1]);
        assert_eq!(mounted_counts[1], mounted_counts[2]);
    }

    #[test]
    fn layout_and_windowing_pass_over_5000_plus_items_is_under_300_microseconds() {
        const ITERATIONS: u32 = 2_000;
        let total_items = 50_000usize;
        let stride = 336.0_f32;
        let viewport_h = 1080.0_f32;
        let overscan = 2usize;

        let start = Instant::now();
        for i in 0..ITERATIONS {
            let viewport_w = 1024.0 + ((i % 10) as f32) * 96.0;
            let cols = compute_poster_grid_columns(
                black_box(viewport_w),
                black_box(0.0),
                black_box(64.0),
                black_box(176.0),
                black_box(18.0),
            );
            let layout = VirtualGridLayout::new(black_box(total_items), cols, stride);
            let scroll_y = (i as f32 * 417.0) % (layout.row_count as f32 * stride);
            let rows = layout.visible_row_range(total_items, scroll_y, viewport_h, overscan);
            let mut count = 0usize;
            for r in rows {
                count += slice_row_items(total_items, cols, r).len();
            }
            black_box(count);
        }
        let elapsed = start.elapsed();
        let avg_micros = elapsed.as_secs_f64() * 1_000_000.0 / f64::from(ITERATIONS);
        eprintln!(
            "virtual_grid 50k windowing pass average: {avg_micros:.4} us over {ITERATIONS} iterations"
        );
        assert!(
            avg_micros < 300.0,
            "average layout/windowing pass took {avg_micros:.2} us, expected < 300 us"
        );
    }

    #[test]
    fn poster_aspect_ratios_and_shape_mapping_are_exact() {
        assert_eq!(PosterAspect::Portrait2x3.height_over_width(), 1.5);
        assert_eq!(PosterAspect::Landscape16x9.height_over_width(), 9.0 / 16.0);
        assert_eq!(PosterAspect::Square1x1.height_over_width(), 1.0);

        assert_eq!(
            PosterAspect::from_stremio_poster_shape(&PosterShape::Poster),
            PosterAspect::Portrait2x3
        );
        assert_eq!(
            PosterAspect::from_stremio_poster_shape(&PosterShape::Landscape),
            PosterAspect::Landscape16x9
        );
        assert_eq!(
            PosterAspect::from_stremio_poster_shape(&PosterShape::Square),
            PosterAspect::Square1x1
        );
    }

    #[test]
    fn dynamic_grid_metrics_never_clip_cards_across_600px_to_3840px() {
        let min_card_w = 176.0_f32;
        let col_gap = 18.0_f32;
        let row_gap = 16.0_f32;
        let metadata_h = 52.0_f32;
        let h_pad = 64.0_f32;

        for aspect in [
            PosterAspect::Portrait2x3,
            PosterAspect::Landscape16x9,
            PosterAspect::Square1x1,
        ] {
            for max_container in [Some(1440.0_f32), None] {
                for sidebar_w in [0.0_f32, 240.0_f32] {
                    for width in (600..=3840).step_by(4) {
                        let vw = width as f32;
                        let metrics = compute_dynamic_grid_metrics(
                            120,
                            vw,
                            sidebar_w,
                            h_pad,
                            max_container,
                            min_card_w,
                            col_gap,
                            row_gap,
                            metadata_h,
                            aspect,
                        );

                        assert!(
                            metrics.card_height_px <= metrics.row_stride_px - row_gap,
                            "card clipped at vw={vw}, aspect={aspect:?}, sidebar={sidebar_w}: card_h={} > available_h={}",
                            metrics.card_height_px,
                            metrics.row_stride_px - row_gap
                        );
                        assert_eq!(metrics.layout.columns, metrics.columns);
                        assert_eq!(metrics.layout.row_stride_px, metrics.row_stride_px);

                        let container_w = match max_container {
                            Some(max_w) => vw.min(max_w),
                            None => vw,
                        };
                        let effective_available = (container_w - sidebar_w - h_pad).max(0.0);
                        if effective_available >= 2.0 * min_card_w + col_gap {
                            assert!(
                                metrics.card_width_px >= min_card_w,
                                "card width {} below min {} at vw={vw}",
                                metrics.card_width_px,
                                min_card_w
                            );
                            let row_used = metrics.columns as f32 * metrics.card_width_px
                                + (metrics.columns as f32 - 1.0) * col_gap;
                            assert!(
                                row_used <= effective_available + 0.05,
                                "row width {row_used} exceeded available {effective_available} at vw={vw}"
                            );
                        } else {
                            assert!(metrics.card_width_px >= min_card_w);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn dynamic_grid_metrics_handles_empty_and_non_finite_inputs() {
        let zero_items = compute_dynamic_grid_metrics(
            0,
            1440.0,
            0.0,
            64.0,
            Some(1440.0),
            176.0,
            18.0,
            16.0,
            52.0,
            PosterAspect::Portrait2x3,
        );
        assert_eq!(zero_items.layout.row_count, 0);
        assert!(zero_items.card_height_px <= zero_items.row_stride_px - 16.0);

        let nan_vw = compute_dynamic_grid_metrics(
            24,
            f32::NAN,
            0.0,
            64.0,
            Some(1440.0),
            176.0,
            18.0,
            16.0,
            52.3,
            PosterAspect::Portrait2x3,
        );
        assert_eq!(nan_vw.columns, 2);
        assert!(nan_vw.card_width_px >= 176.0);
        assert!(nan_vw.card_height_px <= nan_vw.row_stride_px - 16.0);
    }
}
