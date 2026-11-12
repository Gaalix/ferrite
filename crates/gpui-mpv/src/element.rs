use core::panic::Location;

use gpui::{
    fill, relative, AnyElement, App, Bounds, Corners, Display, Element, ElementId, FlexDirection,
    GlobalElementId, InspectorElementId, LayoutId, Length, ParentElement, Pixels, Position, Rgba,
    Size, Style, Window,
};

use crate::player::drain_evicted_video_images;
use crate::MpvController;

#[track_caller]
pub fn mpv_viewport(controller: MpvController) -> MpvViewport {
    MpvViewport::new(controller)
}

pub struct MpvViewport {
    id: ElementId,
    controller: MpvController,
    style: Style,
    children: Vec<AnyElement>,
    source_location: &'static Location<'static>,
}

impl MpvViewport {
    #[track_caller]
    pub fn new(controller: MpvController) -> Self {
        let style = Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            position: Position::Relative,
            size: Size {
                width: relative(1.0).into(),
                height: relative(1.0).into(),
            },
            ..Default::default()
        };

        Self {
            id: ElementId::Name("mpv-viewport".into()),
            controller,
            style,
            children: Vec::new(),
            source_location: Location::caller(),
        }
    }

    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    pub fn w(mut self, width: impl Into<Length>) -> Self {
        self.style.size.width = width.into();
        self
    }

    pub fn h(mut self, height: impl Into<Length>) -> Self {
        self.style.size.height = height.into();
        self
    }

    pub fn with_style(mut self, f: impl FnOnce(&mut Style)) -> Self {
        f(&mut self.style);
        self
    }
}

impl ParentElement for MpvViewport {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Element for MpvViewport {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        Some(self.source_location)
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let child_layout_ids: Vec<LayoutId> = self
            .children
            .iter_mut()
            .map(|child| child.request_layout(window, cx))
            .collect();
        let layout_id = window.request_layout(self.style.clone(), child_layout_ids, cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        for child in &mut self.children {
            child.prepaint(window, cx);
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        for evicted in drain_evicted_video_images() {
            let _ = window.drop_image(evicted);
        }

        let x = bounds.origin.x.round().to_i32();
        let y = bounds.origin.y.round().to_i32();
        let width = bounds.size.width.round().to_i32().max(1);
        let height = bounds.size.height.round().to_i32().max(1);
        let scale_factor = window.scale_factor();

        self.controller
            .set_viewport_rect(x, y, width, height, scale_factor);

        #[cfg(target_os = "windows")]
        if self.controller.is_windows_dcomp_active() {
            if let Some(handles) = window
                .windows_video_handles()
                .filter(|h| h.direct_composition_active && h.hwnd != 0)
            {
                match self.controller.attach_or_sync_windows_dcomp(handles) {
                    Ok(()) => {
                        window.paint_quad(fill(
                            bounds,
                            Rgba {
                                r: 0.0,
                                g: 0.0,
                                b: 0.0,
                                a: 0.0,
                            },
                        ));

                        for child in &mut self.children {
                            child.paint(window, cx);
                        }
                        return;
                    }
                    Err(err) => {
                        tracing::warn!(
                            target: "gpui_mpv::element",
                            "Windows DirectComposition underlay failed, falling back to software renderer: {err}"
                        );
                    }
                }
            }
        }

        #[cfg(target_os = "linux")]
        if let Some(handles) = window.wayland_video_handles() {
            if !self.controller.is_subsurface_attached() {
                if let Err(err) = self
                    .controller
                    .attach_wayland_subsurface(handles.wl_display, handles.wl_surface)
                {
                    tracing::warn!(
                        target: "gpui_mpv::element",
                        "Wayland video subsurface attach failed: {err}"
                    );
                }
            } else {
                let _ = self
                    .controller
                    .attach_wayland_subsurface(handles.wl_display, handles.wl_surface);
            }

            window.paint_quad(fill(
                bounds,
                Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                },
            ));

            for child in &mut self.children {
                child.paint(window, cx);
            }
            return;
        }

        if let Err(err) = self.controller.attach_software_renderer() {
            tracing::warn!(
                target: "gpui_mpv::element",
                "software video renderer attach failed: {err}"
            );
        } else if !self.controller.mark_software_notifier_attached() {
            let entity = window.current_view();
            let mut rx = self.controller.subscribe_software_frames();
            window
                .spawn(cx, async move |wcx| {
                    while rx.changed().await.is_ok() {
                        let _ = rx.borrow_and_update();
                        if wcx.update(|_window, cx| cx.notify(entity)).is_err() {
                            break;
                        }
                    }
                })
                .detach();
        }

        let frame_opt = self.controller.latch_video_frame_for_paint();
        for evicted in self.controller.take_evicted_video_frames() {
            let _ = window.drop_image(evicted);
        }

        window.paint_quad(fill(
            bounds,
            Rgba {
                r: 11.0 / 255.0,
                g: 11.0 / 255.0,
                b: 13.0 / 255.0,
                a: 1.0,
            },
        ));

        if let Some(frame) = frame_opt {
            let _ = window.paint_image(bounds, bounds, Corners::default(), frame, 0, false);
        }

        for child in &mut self.children {
            child.paint(window, cx);
        }
    }
}
