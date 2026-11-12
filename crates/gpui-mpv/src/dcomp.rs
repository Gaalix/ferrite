#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DCompBackendMode {
    DirectSwapChainVisual,
    FlipChildHwndUnderlay,
    #[default]
    SoftwareFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DCompUnderlayRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnderlayPlacement {
    pub viewport_rect: DCompUnderlayRect,
    pub content_rect: DCompUnderlayRect,
    pub offset_x: f32,
    pub offset_y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub clip_left: f32,
    pub clip_top: f32,
    pub clip_right: f32,
    pub clip_bottom: f32,
}

impl Default for UnderlayPlacement {
    fn default() -> Self {
        Self {
            viewport_rect: DCompUnderlayRect::default(),
            content_rect: DCompUnderlayRect::default(),
            offset_x: 0.0,
            offset_y: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            clip_left: 0.0,
            clip_top: 0.0,
            clip_right: 0.0,
            clip_bottom: 0.0,
        }
    }
}

#[inline]
fn sanitize_scale(scale_factor: f32) -> f32 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

#[allow(clippy::too_many_arguments)]
pub fn compute_underlay_placement(
    logical_x: i32,
    logical_y: i32,
    logical_width: i32,
    logical_height: i32,
    scale_factor: f32,
    source_width: u32,
    source_height: u32,
) -> UnderlayPlacement {
    let scale = sanitize_scale(scale_factor);
    let vx = ((logical_x as f32) * scale).round() as i32;
    let vy = ((logical_y as f32) * scale).round() as i32;
    let vw = (((logical_width.max(1)) as f32) * scale).round().max(1.0) as i32;
    let vh = (((logical_height.max(1)) as f32) * scale).round().max(1.0) as i32;
    let viewport_rect = DCompUnderlayRect {
        x: vx,
        y: vy,
        width: vw,
        height: vh,
    };

    if source_width == 0 || source_height == 0 {
        return UnderlayPlacement {
            viewport_rect,
            content_rect: viewport_rect,
            offset_x: vx as f32,
            offset_y: vy as f32,
            scale_x: 1.0,
            scale_y: 1.0,
            clip_left: 0.0,
            clip_top: 0.0,
            clip_right: vw as f32,
            clip_bottom: vh as f32,
        };
    }

    let sw = source_width as f32;
    let sh = source_height as f32;
    let fit = (vw as f32 / sw).min(vh as f32 / sh);
    let cw = (sw * fit).round().clamp(1.0, vw as f32) as i32;
    let ch = (sh * fit).round().clamp(1.0, vh as f32) as i32;
    let cx = vx + (vw - cw) / 2;
    let cy = vy + (vh - ch) / 2;

    let scale_x = (cw as f32 / sw).max(f32::EPSILON);
    let scale_y = (ch as f32 / sh).max(f32::EPSILON);

    let clip_right = (vw as f32 / scale_x).min(sw);
    let clip_bottom = (vh as f32 / scale_y).min(sh);

    UnderlayPlacement {
        viewport_rect,
        content_rect: DCompUnderlayRect {
            x: cx,
            y: cy,
            width: cw,
            height: ch,
        },
        offset_x: cx as f32,
        offset_y: cy as f32,
        scale_x,
        scale_y,
        clip_left: 0.0,
        clip_top: 0.0,
        clip_right,
        clip_bottom,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DCompHandleSnapshot {
    pub hwnd: isize,
    pub dcomp_device: usize,
    pub dcomp_video_visual: usize,
    pub d3d11_device: usize,
    pub direct_composition_active: bool,
}

impl From<gpui::WindowsVideoHandles> for DCompHandleSnapshot {
    fn from(handles: gpui::WindowsVideoHandles) -> Self {
        Self {
            hwnd: handles.hwnd,
            dcomp_device: handles.dcomp_device,
            dcomp_video_visual: handles.dcomp_video_visual,
            d3d11_device: handles.d3d11_device,
            direct_composition_active: handles.direct_composition_active,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DCompDesiredState {
    pub handles: DCompHandleSnapshot,
    pub mode: DCompBackendMode,
    pub placement: UnderlayPlacement,
    pub visible: bool,
    pub swapchain_ptr: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DCompSyncDiff {
    pub handles_changed: bool,
    pub parent_hwnd_changed: bool,
    pub dcomp_pointers_changed: bool,
    pub mode_changed: bool,
    pub geometry_changed: bool,
    pub visibility_changed: bool,
    pub swapchain_changed: bool,
}

impl DCompSyncDiff {
    #[inline]
    pub fn any_changed(self) -> bool {
        self.handles_changed
            || self.mode_changed
            || self.geometry_changed
            || self.visibility_changed
            || self.swapchain_changed
    }

    #[inline]
    pub fn needs_dcomp_commit(self) -> bool {
        self.dcomp_pointers_changed
            || self.mode_changed
            || self.geometry_changed
            || self.visibility_changed
            || self.swapchain_changed
    }

    #[inline]
    pub fn needs_child_hwnd_sync(self) -> bool {
        self.parent_hwnd_changed
            || self.mode_changed
            || self.geometry_changed
            || self.visibility_changed
    }
}

#[inline]
fn approx_eq_f32(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-3
}

#[inline]
fn placement_equal(a: &UnderlayPlacement, b: &UnderlayPlacement) -> bool {
    a.viewport_rect == b.viewport_rect
        && a.content_rect == b.content_rect
        && approx_eq_f32(a.offset_x, b.offset_x)
        && approx_eq_f32(a.offset_y, b.offset_y)
        && approx_eq_f32(a.scale_x, b.scale_x)
        && approx_eq_f32(a.scale_y, b.scale_y)
        && approx_eq_f32(a.clip_left, b.clip_left)
        && approx_eq_f32(a.clip_top, b.clip_top)
        && approx_eq_f32(a.clip_right, b.clip_right)
        && approx_eq_f32(a.clip_bottom, b.clip_bottom)
}

#[derive(Debug, Default)]
pub struct DCompStateTracker {
    last_committed: Option<DCompDesiredState>,
}

impl DCompStateTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn diff(&self, next: &DCompDesiredState) -> DCompSyncDiff {
        let Some(prev) = &self.last_committed else {
            return DCompSyncDiff {
                handles_changed: true,
                parent_hwnd_changed: true,
                dcomp_pointers_changed: true,
                mode_changed: true,
                geometry_changed: true,
                visibility_changed: true,
                swapchain_changed: true,
            };
        };

        let parent_hwnd_changed = prev.handles.hwnd != next.handles.hwnd;
        let dcomp_pointers_changed = prev.handles.dcomp_device != next.handles.dcomp_device
            || prev.handles.dcomp_video_visual != next.handles.dcomp_video_visual
            || prev.handles.d3d11_device != next.handles.d3d11_device
            || prev.handles.direct_composition_active != next.handles.direct_composition_active;
        let handles_changed = parent_hwnd_changed || dcomp_pointers_changed;

        DCompSyncDiff {
            handles_changed,
            parent_hwnd_changed,
            dcomp_pointers_changed,
            mode_changed: prev.mode != next.mode,
            geometry_changed: !placement_equal(&prev.placement, &next.placement),
            visibility_changed: prev.visible != next.visible,
            swapchain_changed: prev.swapchain_ptr != next.swapchain_ptr,
        }
    }

    pub fn mark_committed(&mut self, state: DCompDesiredState) {
        self.last_committed = Some(state);
    }

    pub fn reset(&mut self) {
        self.last_committed = None;
    }

    pub fn last_committed(&self) -> Option<&DCompDesiredState> {
        self.last_committed.as_ref()
    }
}

#[allow(dead_code)]
pub(crate) fn parse_swapchain_ptr_str(raw: &str) -> Option<usize> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed = if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        usize::from_str_radix(hex, 16).ok()
    } else {
        trimmed.parse::<usize>().ok()
    };
    parsed.filter(|&ptr| ptr != 0)
}

#[cfg(target_os = "windows")]
mod win_impl {
    use core::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Once;
    use std::thread::ThreadId;

    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::DirectComposition::{
        IDCompositionClip, IDCompositionDevice, IDCompositionRectangleClip,
        IDCompositionScaleTransform, IDCompositionTransform, IDCompositionVisual,
    };
    use windows::Win32::Graphics::Dxgi::IDXGISwapChain1;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, IsWindow, PostMessageW, RegisterClassW,
        SetWindowPos, CS_HREDRAW, CS_VREDRAW, HTTRANSPARENT, SWP_HIDEWINDOW, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, WM_APP,
        WM_ERASEBKGND, WM_NCHITTEST, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
        WS_EX_NOACTIVATE, WS_EX_NOPARENTNOTIFY, WS_EX_TRANSPARENT,
    };
    use windows_core::{IUnknown, Interface};

    use super::{
        compute_underlay_placement, DCompBackendMode, DCompDesiredState, DCompHandleSnapshot,
        DCompStateTracker, DCompSyncDiff,
    };
    use crate::MpvError;

    const VIDEO_UNDERLAY_CLASS_NAME: PCWSTR = w!("Ferrite::MpvVideoUnderlay");
    const WM_APP_DESTROY_UNDERLAY: u32 = WM_APP + 1;

    unsafe extern "system" fn video_underlay_wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_ERASEBKGND => LRESULT(1),
            WM_APP_DESTROY_UNDERLAY => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    fn ensure_video_underlay_class_registered() -> Result<(), MpvError> {
        static REGISTER_ONCE: Once = Once::new();
        static REGISTER_OK: AtomicBool = AtomicBool::new(false);
        REGISTER_ONCE.call_once(|| {
            let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
                return;
            };
            let wc = WNDCLASSW {
                lpfnWndProc: Some(video_underlay_wndproc),
                lpszClassName: VIDEO_UNDERLAY_CLASS_NAME,
                style: CS_HREDRAW | CS_VREDRAW,
                hInstance: module.into(),
                ..Default::default()
            };
            let atom = unsafe { RegisterClassW(&wc) };
            REGISTER_OK.store(atom != 0, Ordering::Release);
        });
        if REGISTER_OK.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(MpvError::Render(
                "failed to register DirectComposition child video HWND class".into(),
            ))
        }
    }

    fn destroy_hwnd_safely(hwnd: HWND, owner_thread: Option<ThreadId>) {
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() {
                return;
            }
            if owner_thread == Some(std::thread::current().id()) {
                let _ = DestroyWindow(hwnd);
            } else {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_HIDEWINDOW
                        | SWP_NOMOVE
                        | SWP_NOSIZE
                        | SWP_NOZORDER
                        | SWP_NOACTIVATE
                        | SWP_NOOWNERZORDER,
                );
                let _ = PostMessageW(Some(hwnd), WM_APP_DESTROY_UNDERLAY, WPARAM(0), LPARAM(0));
            }
        }
    }

    pub struct WindowsDCompUnderlay {
        tracker: DCompStateTracker,
        mode: DCompBackendMode,
        comp_device: Option<IDCompositionDevice>,
        video_visual: Option<IDCompositionVisual>,
        scale_transform: Option<IDCompositionScaleTransform>,
        clip_rect: Option<IDCompositionRectangleClip>,
        attached_swapchain_ptr: usize,
        child_hwnd: Option<HWND>,
        child_owner_thread: Option<ThreadId>,
    }

    unsafe impl Send for WindowsDCompUnderlay {}

    impl WindowsDCompUnderlay {
        pub fn new(mode: DCompBackendMode) -> Self {
            Self {
                tracker: DCompStateTracker::new(),
                mode,
                comp_device: None,
                video_visual: None,
                scale_transform: None,
                clip_rect: None,
                attached_swapchain_ptr: 0,
                child_hwnd: None,
                child_owner_thread: None,
            }
        }

        pub fn mode(&self) -> DCompBackendMode {
            self.mode
        }

        pub fn child_hwnd_isize(&self) -> Option<isize> {
            self.child_hwnd.map(|h| h.0 as isize)
        }

        pub fn last_handles(&self) -> Option<DCompHandleSnapshot> {
            self.tracker.last_committed().map(|s| s.handles)
        }

        #[allow(clippy::too_many_arguments)]
        pub fn sync(
            &mut self,
            handles: gpui::WindowsVideoHandles,
            logical_x: i32,
            logical_y: i32,
            logical_width: i32,
            logical_height: i32,
            scale_factor: f32,
            visible: bool,
            swapchain_ptr: usize,
            video_width: i64,
            video_height: i64,
        ) -> Result<DCompSyncDiff, MpvError> {
            if !handles.direct_composition_active || handles.hwnd == 0 {
                return Err(MpvError::Render(
                    "DirectComposition is not active for this window".into(),
                ));
            }
            if self.mode == DCompBackendMode::SoftwareFallback {
                return Err(MpvError::Render(
                    "WindowsDCompUnderlay is configured in software fallback mode".into(),
                ));
            }
            if self.mode == DCompBackendMode::DirectSwapChainVisual
                && (handles.dcomp_device == 0 || handles.dcomp_video_visual == 0)
            {
                return Err(MpvError::Render(
                    "missing DirectComposition device or video visual handle".into(),
                ));
            }

            let (source_w, source_h) = match self.mode {
                DCompBackendMode::DirectSwapChainVisual if swapchain_ptr != 0 => unsafe {
                    let raw = swapchain_ptr as *mut c_void;
                    if let Some(sc) = IDXGISwapChain1::from_raw_borrowed(&raw) {
                        match sc.GetDesc1() {
                            Ok(desc) if desc.Width > 0 && desc.Height > 0 => {
                                (desc.Width, desc.Height)
                            }
                            _ => (
                                u32::try_from(video_width).unwrap_or(0),
                                u32::try_from(video_height).unwrap_or(0),
                            ),
                        }
                    } else {
                        (
                            u32::try_from(video_width).unwrap_or(0),
                            u32::try_from(video_height).unwrap_or(0),
                        )
                    }
                },
                DCompBackendMode::DirectSwapChainVisual => (
                    u32::try_from(video_width).unwrap_or(0),
                    u32::try_from(video_height).unwrap_or(0),
                ),
                DCompBackendMode::FlipChildHwndUnderlay | DCompBackendMode::SoftwareFallback => {
                    (0, 0)
                }
            };

            let placement = compute_underlay_placement(
                logical_x,
                logical_y,
                logical_width,
                logical_height,
                scale_factor,
                source_w,
                source_h,
            );
            let desired = DCompDesiredState {
                handles: DCompHandleSnapshot::from(handles),
                mode: self.mode,
                placement,
                visible,
                swapchain_ptr: if self.mode == DCompBackendMode::DirectSwapChainVisual {
                    swapchain_ptr
                } else {
                    0
                },
            };

            let diff = self.tracker.diff(&desired);
            if !diff.any_changed() {
                return Ok(diff);
            }

            match self.mode {
                DCompBackendMode::DirectSwapChainVisual => {
                    self.sync_direct_swapchain_visual(&desired, diff)?;
                }
                DCompBackendMode::FlipChildHwndUnderlay => {
                    self.sync_child_hwnd_underlay(&desired, diff)?;
                }
                DCompBackendMode::SoftwareFallback => unreachable!(),
            }

            self.tracker.mark_committed(desired);
            Ok(diff)
        }

        fn sync_direct_swapchain_visual(
            &mut self,
            desired: &DCompDesiredState,
            diff: DCompSyncDiff,
        ) -> Result<(), MpvError> {
            if let Some(old_hwnd) = self.child_hwnd.take() {
                let owner = self.child_owner_thread.take();
                destroy_hwnd_safely(old_hwnd, owner);
            }

            if diff.dcomp_pointers_changed
                || self.comp_device.is_none()
                || self.video_visual.is_none()
            {
                let dev_raw = desired.handles.dcomp_device as *mut c_void;
                let vis_raw = desired.handles.dcomp_video_visual as *mut c_void;
                let comp_device = unsafe { IDCompositionDevice::from_raw_borrowed(&dev_raw) }
                    .cloned()
                    .ok_or_else(|| {
                        MpvError::Render("null IDCompositionDevice pointer in handles".into())
                    })?;
                let video_visual = unsafe { IDCompositionVisual::from_raw_borrowed(&vis_raw) }
                    .cloned()
                    .ok_or_else(|| {
                        MpvError::Render("null IDCompositionVisual pointer in handles".into())
                    })?;
                let scale_transform = unsafe { comp_device.CreateScaleTransform() }.map_err(|e| {
                    MpvError::Render(format!("CreateScaleTransform failed: {e}"))
                })?;
                let clip_rect = unsafe { comp_device.CreateRectangleClip() }.map_err(|e| {
                    MpvError::Render(format!("CreateRectangleClip failed: {e}"))
                })?;

                self.comp_device = Some(comp_device);
                self.video_visual = Some(video_visual);
                self.scale_transform = Some(scale_transform);
                self.clip_rect = Some(clip_rect);
                self.attached_swapchain_ptr = 0;
            }

            let comp_device = self.comp_device.as_ref().unwrap();
            let video_visual = self.video_visual.as_ref().unwrap();
            let scale_transform = self.scale_transform.as_ref().unwrap();
            let clip_rect = self.clip_rect.as_ref().unwrap();

            if !desired.visible || desired.swapchain_ptr == 0 {
                if self.attached_swapchain_ptr != 0
                    || diff.visibility_changed
                    || diff.dcomp_pointers_changed
                {
                    unsafe {
                        video_visual
                            .SetContent(None::<&IUnknown>)
                            .map_err(|e| MpvError::Render(format!("SetContent(None) failed: {e}")))?;
                        comp_device
                            .Commit()
                            .map_err(|e| MpvError::Render(format!("DComp Commit failed: {e}")))?;
                    }
                    self.attached_swapchain_ptr = 0;
                }
                return Ok(());
            }

            if diff.swapchain_changed
                || diff.dcomp_pointers_changed
                || self.attached_swapchain_ptr != desired.swapchain_ptr
            {
                let sc_raw = desired.swapchain_ptr as *mut c_void;
                let swap_chain = unsafe { IDXGISwapChain1::from_raw_borrowed(&sc_raw) }.ok_or_else(
                    || MpvError::Render("null IDXGISwapChain1 pointer from libmpv".into()),
                )?;
                unsafe {
                    video_visual
                        .SetContent(swap_chain)
                        .map_err(|e| MpvError::Render(format!("SetContent(swapchain) failed: {e}")))?;
                }
                self.attached_swapchain_ptr = desired.swapchain_ptr;
            }

            if diff.needs_dcomp_commit() {
                let p = &desired.placement;
                unsafe {
                    video_visual
                        .SetOffsetX2(p.offset_x)
                        .map_err(|e| MpvError::Render(format!("SetOffsetX2 failed: {e}")))?;
                    video_visual
                        .SetOffsetY2(p.offset_y)
                        .map_err(|e| MpvError::Render(format!("SetOffsetY2 failed: {e}")))?;
                    scale_transform
                        .SetCenterX2(0.0)
                        .map_err(|e| MpvError::Render(format!("SetCenterX2 failed: {e}")))?;
                    scale_transform
                        .SetCenterY2(0.0)
                        .map_err(|e| MpvError::Render(format!("SetCenterY2 failed: {e}")))?;
                    scale_transform
                        .SetScaleX2(p.scale_x)
                        .map_err(|e| MpvError::Render(format!("SetScaleX2 failed: {e}")))?;
                    scale_transform
                        .SetScaleY2(p.scale_y)
                        .map_err(|e| MpvError::Render(format!("SetScaleY2 failed: {e}")))?;
                    video_visual
                        .SetTransform(scale_transform)
                        .map_err(|e| MpvError::Render(format!("SetTransform failed: {e}")))?;
                    clip_rect
                        .SetLeft2(p.clip_left)
                        .map_err(|e| MpvError::Render(format!("SetLeft2 failed: {e}")))?;
                    clip_rect
                        .SetTop2(p.clip_top)
                        .map_err(|e| MpvError::Render(format!("SetTop2 failed: {e}")))?;
                    clip_rect
                        .SetRight2(p.clip_right)
                        .map_err(|e| MpvError::Render(format!("SetRight2 failed: {e}")))?;
                    clip_rect
                        .SetBottom2(p.clip_bottom)
                        .map_err(|e| MpvError::Render(format!("SetBottom2 failed: {e}")))?;
                    video_visual
                        .SetClip(clip_rect)
                        .map_err(|e| MpvError::Render(format!("SetClip failed: {e}")))?;
                    comp_device
                        .Commit()
                        .map_err(|e| MpvError::Render(format!("DComp Commit failed: {e}")))?;
                }
            }

            Ok(())
        }

        fn sync_child_hwnd_underlay(
            &mut self,
            desired: &DCompDesiredState,
            diff: DCompSyncDiff,
        ) -> Result<(), MpvError> {
            self.release_direct_visual_content();

            if diff.parent_hwnd_changed || self.child_hwnd.is_none() {
                if let Some(old_hwnd) = self.child_hwnd.take() {
                    let owner = self.child_owner_thread.take();
                    destroy_hwnd_safely(old_hwnd, owner);
                }

                ensure_video_underlay_class_registered()?;
                let module = unsafe { GetModuleHandleW(None) }.map_err(|e| {
                    MpvError::Render(format!("GetModuleHandleW failed for child HWND: {e}"))
                })?;
                let parent = HWND(desired.handles.hwnd as *mut c_void);
                let rect = desired.placement.viewport_rect;
                let ex_style = WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_NOPARENTNOTIFY;
                let style = WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;
                let child = unsafe {
                    CreateWindowExW(
                        ex_style,
                        VIDEO_UNDERLAY_CLASS_NAME,
                        w!(""),
                        style,
                        rect.x,
                        rect.y,
                        rect.width,
                        rect.height,
                        Some(parent),
                        None,
                        Some(module.into()),
                        None,
                    )
                }
                .map_err(|e| {
                    MpvError::Render(format!("CreateWindowExW for video underlay failed: {e}"))
                })?;

                self.child_hwnd = Some(child);
                self.child_owner_thread = Some(std::thread::current().id());
            }

            if diff.needs_child_hwnd_sync() {
                if let Some(child) = self.child_hwnd {
                    let rect = desired.placement.viewport_rect;
                    let mut flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
                    flags |= if desired.visible {
                        SWP_SHOWWINDOW
                    } else {
                        SWP_HIDEWINDOW
                    };
                    unsafe {
                        SetWindowPos(child, None, rect.x, rect.y, rect.width, rect.height, flags)
                    }
                    .map_err(|e| {
                        MpvError::Render(format!("SetWindowPos on video underlay failed: {e}"))
                    })?;
                }
            }

            Ok(())
        }

        fn release_direct_visual_content(&mut self) {
            if let (Some(comp_device), Some(video_visual)) =
                (self.comp_device.as_ref(), self.video_visual.as_ref())
            {
                if self.attached_swapchain_ptr != 0 {
                    unsafe {
                        let _ = video_visual.SetContent(None::<&IUnknown>);
                        let _ = video_visual.SetTransform(None::<&IDCompositionTransform>);
                        let _ = video_visual.SetClip(None::<&IDCompositionClip>);
                        let _ = comp_device.Commit();
                    }
                }
            }
            self.attached_swapchain_ptr = 0;
            self.scale_transform = None;
            self.clip_rect = None;
            self.video_visual = None;
            self.comp_device = None;
        }

        pub fn detach(&mut self) {
            self.release_direct_visual_content();
            if let Some(child) = self.child_hwnd.take() {
                let owner = self.child_owner_thread.take();
                destroy_hwnd_safely(child, owner);
            }
            self.child_owner_thread = None;
            self.tracker.reset();
        }
    }

    impl Drop for WindowsDCompUnderlay {
        fn drop(&mut self) {
            self.detach();
        }
    }
}

#[cfg(target_os = "windows")]
pub use win_impl::WindowsDCompUnderlay;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_handles() -> gpui::WindowsVideoHandles {
        gpui::WindowsVideoHandles {
            hwnd: 0x1000,
            dcomp_device: 0x2000,
            dcomp_video_visual: 0x3000,
            d3d11_device: 0x4000,
            direct_composition_active: true,
        }
    }

    #[test]
    fn underlay_placement_scales_hi_dpi_and_preserves_aspect_ratio() {
        // 16:9 1920x1080 video inside a 1280x800 logical viewport at 1.5x DPI (1920x1200 physical)
        let p = compute_underlay_placement(0, 0, 1280, 800, 1.5, 1920, 1080);
        assert_eq!(
            p.viewport_rect,
            DCompUnderlayRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1200,
            }
        );
        assert_eq!(
            p.content_rect,
            DCompUnderlayRect {
                x: 0,
                y: 60,
                width: 1920,
                height: 1080,
            }
        );
        assert!((p.offset_x - 0.0).abs() < 1e-3);
        assert!((p.offset_y - 60.0).abs() < 1e-3);
        assert!((p.scale_x - 1.0).abs() < 1e-3);
        assert!((p.scale_y - 1.0).abs() < 1e-3);
        assert!((p.clip_left - 0.0).abs() < 1e-3);
        assert!((p.clip_top - 0.0).abs() < 1e-3);
        assert!((p.clip_right - 1920.0).abs() < 1e-3);
        assert!((p.clip_bottom - 1080.0).abs() < 1e-3);
    }

    #[test]
    fn underlay_placement_pillarboxes_narrow_video_and_clamps_invalid_inputs() {
        // 4:3 (1440x1080) inside 1920x1080 viewport at 1.0x -> width 1440 centered at x=240
        let p = compute_underlay_placement(10, 20, 1920, 1080, 1.0, 1440, 1080);
        assert_eq!(
            p.viewport_rect,
            DCompUnderlayRect {
                x: 10,
                y: 20,
                width: 1920,
                height: 1080,
            }
        );
        assert_eq!(
            p.content_rect,
            DCompUnderlayRect {
                x: 250,
                y: 20,
                width: 1440,
                height: 1080,
            }
        );
        assert!((p.offset_x - 250.0).abs() < 1e-3);
        assert!((p.offset_y - 20.0).abs() < 1e-3);

        // Zero source dims (child-HWND mode) and invalid scale fallback
        let child = compute_underlay_placement(5, 10, 0, -4, f32::NAN, 0, 0);
        assert_eq!(
            child.viewport_rect,
            DCompUnderlayRect {
                x: 5,
                y: 10,
                width: 1,
                height: 1,
            }
        );
        assert_eq!(child.content_rect, child.viewport_rect);
        assert!((child.scale_x - 1.0).abs() < 1e-3);
        assert!((child.scale_y - 1.0).abs() < 1e-3);
    }

    #[test]
    fn dirty_check_diffing_performs_zero_work_on_steady_frames_and_detects_device_lost() {
        let mut tracker = DCompStateTracker::new();
        let handles = sample_handles();
        let placement = compute_underlay_placement(0, 0, 1280, 720, 1.0, 1920, 1080);

        let initial = DCompDesiredState {
            handles: DCompHandleSnapshot::from(handles),
            mode: DCompBackendMode::DirectSwapChainVisual,
            placement,
            visible: true,
            swapchain_ptr: 0xDEAD_BEEF,
        };

        let d0 = tracker.diff(&initial);
        assert!(d0.any_changed());
        assert!(d0.needs_dcomp_commit());
        tracker.mark_committed(initial);

        // Steady video frames with identical state must report no changes (0 Commit/SetWindowPos work).
        let d_steady = tracker.diff(&initial);
        assert!(!d_steady.any_changed());
        assert!(!d_steady.needs_dcomp_commit());
        assert!(!d_steady.needs_child_hwnd_sync());

        // Visibility toggle triggers only visibility_changed.
        let hidden = DCompDesiredState {
            visible: false,
            ..initial
        };
        let d_vis = tracker.diff(&hidden);
        assert!(d_vis.visibility_changed);
        assert!(!d_vis.geometry_changed);
        assert!(!d_vis.handles_changed);
        assert!(!d_vis.swapchain_changed);
        tracker.mark_committed(hidden);

        // Geometry change triggers only geometry_changed.
        let resized_placement = compute_underlay_placement(0, 0, 1600, 900, 1.25, 1920, 1080);
        let resized = DCompDesiredState {
            placement: resized_placement,
            ..hidden
        };
        let d_geom = tracker.diff(&resized);
        assert!(d_geom.geometry_changed);
        assert!(!d_geom.handles_changed);
        assert!(!d_geom.visibility_changed);
        tracker.mark_committed(resized);

        // Device lost in gpui_windows recreates DComp device / video_visual / D3D11 device pointers.
        let mut post_device_lost_handles = handles;
        post_device_lost_handles.dcomp_device = 0x9000;
        post_device_lost_handles.dcomp_video_visual = 0xA000;
        post_device_lost_handles.d3d11_device = 0xB000;
        let post_lost = DCompDesiredState {
            handles: DCompHandleSnapshot::from(post_device_lost_handles),
            ..resized
        };
        let d_lost = tracker.diff(&post_lost);
        assert!(d_lost.handles_changed);
        assert!(d_lost.dcomp_pointers_changed);
        assert!(!d_lost.parent_hwnd_changed);
        assert!(d_lost.needs_dcomp_commit());
        assert!(!d_lost.needs_child_hwnd_sync());

        // Child HWND mode: same parent hwnd across device lost does not recreate child HWND.
        let mut child_tracker = DCompStateTracker::new();
        let child_p = compute_underlay_placement(0, 0, 1280, 720, 1.0, 0, 0);
        let child_init = DCompDesiredState {
            handles: DCompHandleSnapshot::from(handles),
            mode: DCompBackendMode::FlipChildHwndUnderlay,
            placement: child_p,
            visible: true,
            swapchain_ptr: 0,
        };
        child_tracker.mark_committed(child_init);
        let child_steady = child_tracker.diff(&child_init);
        assert!(!child_steady.any_changed());

        let child_after_lost = DCompDesiredState {
            handles: DCompHandleSnapshot::from(post_device_lost_handles),
            ..child_init
        };
        let d_child_lost = child_tracker.diff(&child_after_lost);
        assert!(d_child_lost.handles_changed);
        assert!(!d_child_lost.parent_hwnd_changed);
        assert!(!d_child_lost.needs_child_hwnd_sync());
    }

    #[test]
    fn parse_swapchain_ptr_handles_hex_decimal_and_zero() {
        assert_eq!(parse_swapchain_ptr_str("0x1a2b3c"), Some(0x1a2b3c));
        assert_eq!(parse_swapchain_ptr_str("0XFF00"), Some(0xFF00));
        assert_eq!(parse_swapchain_ptr_str("4096"), Some(4096));
        assert_eq!(parse_swapchain_ptr_str("0x0"), None);
        assert_eq!(parse_swapchain_ptr_str("0"), None);
        assert_eq!(parse_swapchain_ptr_str(""), None);
        assert_eq!(parse_swapchain_ptr_str("not-a-ptr"), None);
    }
}
