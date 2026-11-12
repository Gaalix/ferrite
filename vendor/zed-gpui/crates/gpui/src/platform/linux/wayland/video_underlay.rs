use core::ffi::c_void;

/// Raw Wayland display and surface handles for attaching a video underlay `wl_subsurface`.
#[derive(Clone, Copy, Debug)]
pub struct WaylandVideoHandles {
    /// Raw `*mut wl_display` pointer for the window's Wayland connection.
    pub wl_display: *mut c_void,
    /// Raw `*mut wl_surface` pointer for the window's primary Wayland surface.
    pub wl_surface: *mut c_void,
}

unsafe impl Send for WaylandVideoHandles {}
unsafe impl Sync for WaylandVideoHandles {}
