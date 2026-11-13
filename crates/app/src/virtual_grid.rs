use std::ops::Range;

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
}
