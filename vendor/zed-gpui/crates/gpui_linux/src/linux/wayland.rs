mod client;
mod clipboard;
mod cursor;
mod display;
mod popup;
mod serial;
mod window;

/// Contains Types for configuring layer_shell surfaces.
pub mod layer_shell;

pub(crate) use client::*;

use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;

use gpui::CursorStyle;

pub(super) fn to_shape(style: CursorStyle) -> Option<Shape> {
    match style {
        CursorStyle::Arrow => Some(Shape::Default),
        CursorStyle::IBeam => Some(Shape::Text),
        CursorStyle::Crosshair => Some(Shape::Crosshair),
        CursorStyle::ClosedHand => Some(Shape::Grabbing),
        CursorStyle::OpenHand => Some(Shape::Grab),
        CursorStyle::PointingHand => Some(Shape::Pointer),
        CursorStyle::ResizeLeft => Some(Shape::WResize),
        CursorStyle::ResizeRight => Some(Shape::EResize),
        CursorStyle::ResizeLeftRight => Some(Shape::EwResize),
        CursorStyle::ResizeUp => Some(Shape::NResize),
        CursorStyle::ResizeDown => Some(Shape::SResize),
        CursorStyle::ResizeUpDown => Some(Shape::NsResize),
        CursorStyle::ResizeUpLeftDownRight => Some(Shape::NwseResize),
        CursorStyle::ResizeUpRightDownLeft => Some(Shape::NeswResize),
        CursorStyle::ResizeColumn => Some(Shape::ColResize),
        CursorStyle::ResizeRow => Some(Shape::RowResize),
        CursorStyle::IBeamCursorForVerticalLayout => Some(Shape::VerticalText),
        CursorStyle::OperationNotAllowed => Some(Shape::NotAllowed),
        CursorStyle::DragLink => Some(Shape::Alias),
        CursorStyle::DragCopy => Some(Shape::Copy),
        CursorStyle::ContextualMenu => Some(Shape::ContextMenu),
        CursorStyle::None => None,
    }
}
