//! Ferrite frontend design system primitives.
//!
//! Centralizes canonical layout geometry (`PageWidth`, `page_gutter_px`, vertical rhythm),
//! headers (`PageHeader`, `SectionHeader`), toolbar & input controls (`toolbar_bar`,
//! `search_field`, `dropdown_trigger`, `choice_pill`), unified empty states, poster and
//! landscape media cards (`PosterMetrics`, `render_poster_card`, `render_landscape_card`),
//! and virtual grid sizing (`compute_app_grid_metrics`).

pub mod cards;
pub mod controls;
pub mod empty_state;
pub mod grid;
pub mod header;
pub mod layout;

pub use cards::{
    clamp_progress_fraction, format_notification_badge_label, landscape_card_dimensions,
    landscape_card_width_px, render_landscape_card, render_poster_card, LandscapeCardBadge,
    LandscapeCardProps, PosterCardProps, PosterMetrics,
};
pub use controls::{
    choice_pill, click_handler, dropdown_trigger, glass_back_button, glass_dropdown_option,
    glass_dropdown_panel, glass_segmented_bar, glass_segmented_item, glass_stepper, glass_switch,
    hover_handler, keycap, meta_divider, neutral_chip, primary_accent_button,
    render_stream_source_row, search_field, secondary_glass_button, semantic_status_pill,
    toolbar_bar, ClickHandler, DropdownTriggerProps, HoverHandler, SearchFieldProps,
    StreamSourceRowProps,
};
pub use empty_state::{render_empty_state, EmptyStateProps};
pub use grid::{
    compute_app_grid_metrics, compute_app_landscape_grid_metrics, infer_grid_poster_aspect,
};
pub use header::{
    render_page_header, render_section_header, PageHeader, PageHeaderBackButton, SectionHeader,
};
pub use layout::{
    apply_page_width, full_height_page_container, is_compact_viewport, page_container,
    page_gutter_px, topnav_gutter_px, PageWidth, COMPACT_VIEWPORT_BREAKPOINT_PX,
    FOCUSED_PAGE_MAX_WIDTH_PX, PAGE_GUTTER_COMPACT_PX, PAGE_GUTTER_DEFAULT_PX, PAGE_GUTTER_MAX_PX,
    PAGE_GUTTER_MIN_PX, PAGE_LIST_PADDING_BOTTOM_PX, PAGE_PADDING_BOTTOM_PX, PAGE_PADDING_TOP_PX,
    PAGE_SECTION_GAP_PX,
};
