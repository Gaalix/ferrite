use core::ffi::{c_char, c_uint, c_void, CStr};
use core::ptr::{self, NonNull};

use wayland_sys::client::{wayland_client_handle, wl_display, wl_event_queue, wl_proxy};
use wayland_sys::common::{wl_interface, wl_message};
use wayland_sys::egl::{wayland_egl_handle, wl_egl_window};
use wayland_sys::ffi_dispatch;

use crate::MpvError;

const WL_MARSHAL_FLAG_DESTROY: u32 = 1 << 0;

const WL_DISPLAY_GET_REGISTRY: u32 = 1;
const WL_REGISTRY_BIND: u32 = 0;

const WL_COMPOSITOR_CREATE_SURFACE: u32 = 0;
const WL_COMPOSITOR_CREATE_REGION: u32 = 1;

const WL_SURFACE_DESTROY: u32 = 0;
const WL_SURFACE_SET_INPUT_REGION: u32 = 5;
const WL_SURFACE_COMMIT: u32 = 6;
const WL_SURFACE_SET_BUFFER_SCALE: u32 = 8;

const WL_REGION_DESTROY: u32 = 0;

const WL_SUBCOMPOSITOR_DESTROY: u32 = 0;
const WL_SUBCOMPOSITOR_GET_SUBSURFACE: u32 = 1;

const WL_SUBSURFACE_DESTROY: u32 = 0;
const WL_SUBSURFACE_SET_POSITION: u32 = 1;
const WL_SUBSURFACE_PLACE_BELOW: u32 = 3;
const WL_SUBSURFACE_SET_DESYNC: u32 = 5;

const WP_VIEWPORTER_DESTROY: u32 = 0;
const WP_VIEWPORTER_GET_VIEWPORT: u32 = 1;

const WP_VIEWPORT_DESTROY: u32 = 0;
const WP_VIEWPORT_SET_DESTINATION: u32 = 2;

unsafe extern "C" {
    static wl_registry_interface: wl_interface;
    static wl_compositor_interface: wl_interface;
    static wl_surface_interface: wl_interface;
    static wl_region_interface: wl_interface;
    static wl_subcompositor_interface: wl_interface;
    static wl_subsurface_interface: wl_interface;

    fn wl_proxy_marshal_flags(
        proxy: *mut wl_proxy,
        opcode: u32,
        interface: *const wl_interface,
        version: u32,
        flags: u32,
        ...
    ) -> *mut wl_proxy;
}

#[repr(transparent)]
struct SyncPtrArray<const N: usize>([*const wl_interface; N]);
unsafe impl<const N: usize> Sync for SyncPtrArray<N> {}

#[repr(transparent)]
struct SyncMessages<const N: usize>([wl_message; N]);
unsafe impl<const N: usize> Sync for SyncMessages<N> {}

static VIEWPORT_NULL_TYPES: SyncPtrArray<4> = SyncPtrArray([ptr::null(); 4]);

static WP_VIEWPORT_REQUESTS: SyncMessages<3> = SyncMessages([
    wl_message {
        name: c"destroy".as_ptr(),
        signature: c"".as_ptr(),
        types: VIEWPORT_NULL_TYPES.0.as_ptr(),
    },
    wl_message {
        name: c"set_source".as_ptr(),
        signature: c"ffff".as_ptr(),
        types: VIEWPORT_NULL_TYPES.0.as_ptr(),
    },
    wl_message {
        name: c"set_destination".as_ptr(),
        signature: c"ii".as_ptr(),
        types: VIEWPORT_NULL_TYPES.0.as_ptr(),
    },
]);

pub(crate) static WP_VIEWPORT_INTERFACE: wl_interface = wl_interface {
    name: c"wp_viewport".as_ptr(),
    version: 1,
    request_count: 3,
    requests: WP_VIEWPORT_REQUESTS.0.as_ptr(),
    event_count: 0,
    events: ptr::null(),
};

static GET_VIEWPORT_TYPES: SyncPtrArray<2> = SyncPtrArray([
    &raw const WP_VIEWPORT_INTERFACE,
    &raw const wl_surface_interface,
]);

static WP_VIEWPORTER_REQUESTS: SyncMessages<2> = SyncMessages([
    wl_message {
        name: c"destroy".as_ptr(),
        signature: c"".as_ptr(),
        types: VIEWPORT_NULL_TYPES.0.as_ptr(),
    },
    wl_message {
        name: c"get_viewport".as_ptr(),
        signature: c"no".as_ptr(),
        types: GET_VIEWPORT_TYPES.0.as_ptr(),
    },
]);

pub(crate) static WP_VIEWPORTER_INTERFACE: wl_interface = wl_interface {
    name: c"wp_viewporter".as_ptr(),
    version: 1,
    request_count: 2,
    requests: WP_VIEWPORTER_REQUESTS.0.as_ptr(),
    event_count: 0,
    events: ptr::null(),
};

type EGLBoolean = c_uint;
type EGLint = i32;
type EGLenum = c_uint;
type EGLAttrib = isize;
type EGLDisplay = *mut c_void;
type EGLConfig = *mut c_void;
type EGLContext = *mut c_void;
type EGLSurface = *mut c_void;

const EGL_FALSE: EGLBoolean = 0;
const EGL_NONE: EGLint = 0x3038;
const EGL_NO_CONTEXT: EGLContext = ptr::null_mut();
const EGL_NO_DISPLAY: EGLDisplay = ptr::null_mut();
const EGL_NO_SURFACE: EGLSurface = ptr::null_mut();

const EGL_PLATFORM_WAYLAND_KHR: EGLenum = 0x31D8;
const EGL_OPENGL_API: EGLenum = 0x30A2;
const EGL_OPENGL_ES_API: EGLenum = 0x30A0;

const EGL_ALPHA_SIZE: EGLint = 0x3021;
const EGL_BLUE_SIZE: EGLint = 0x3022;
const EGL_GREEN_SIZE: EGLint = 0x3023;
const EGL_RED_SIZE: EGLint = 0x3024;
const EGL_SURFACE_TYPE: EGLint = 0x3033;
const EGL_RENDERABLE_TYPE: EGLint = 0x3040;
const EGL_WINDOW_BIT: EGLint = 0x0004;
const EGL_OPENGL_ES2_BIT: EGLint = 0x0004;
const EGL_OPENGL_BIT: EGLint = 0x0008;
const EGL_OPENGL_ES3_BIT: EGLint = 0x0040;

const EGL_CONTEXT_MAJOR_VERSION: EGLint = 0x3098;
const EGL_CONTEXT_MINOR_VERSION: EGLint = 0x30FB;
const EGL_CONTEXT_OPENGL_PROFILE_MASK: EGLint = 0x30FD;
const EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT: EGLint = 0x0000_0001;

const GL_COLOR_BUFFER_BIT: u32 = 0x0000_4000;

unsafe extern "C" {
    fn eglGetError() -> EGLint;
    fn eglGetPlatformDisplay(
        platform: EGLenum,
        native_display: *mut c_void,
        attrib_list: *const EGLAttrib,
    ) -> EGLDisplay;
    fn eglGetDisplay(display_id: *mut c_void) -> EGLDisplay;
    fn eglInitialize(dpy: EGLDisplay, major: *mut EGLint, minor: *mut EGLint) -> EGLBoolean;
    fn eglBindAPI(api: EGLenum) -> EGLBoolean;
    fn eglChooseConfig(
        dpy: EGLDisplay,
        attrib_list: *const EGLint,
        configs: *mut EGLConfig,
        config_size: EGLint,
        num_config: *mut EGLint,
    ) -> EGLBoolean;
    fn eglCreateContext(
        dpy: EGLDisplay,
        config: EGLConfig,
        share_context: EGLContext,
        attrib_list: *const EGLint,
    ) -> EGLContext;
    fn eglCreatePlatformWindowSurface(
        dpy: EGLDisplay,
        config: EGLConfig,
        native_window: *mut c_void,
        attrib_list: *const EGLAttrib,
    ) -> EGLSurface;
    fn eglCreateWindowSurface(
        dpy: EGLDisplay,
        config: EGLConfig,
        win: *mut c_void,
        attrib_list: *const EGLint,
    ) -> EGLSurface;
    fn eglMakeCurrent(
        dpy: EGLDisplay,
        draw: EGLSurface,
        read: EGLSurface,
        ctx: EGLContext,
    ) -> EGLBoolean;
    fn eglSwapInterval(dpy: EGLDisplay, interval: EGLint) -> EGLBoolean;
    fn eglSwapBuffers(dpy: EGLDisplay, surface: EGLSurface) -> EGLBoolean;
    fn eglDestroySurface(dpy: EGLDisplay, surface: EGLSurface) -> EGLBoolean;
    fn eglDestroyContext(dpy: EGLDisplay, ctx: EGLContext) -> EGLBoolean;
    fn eglReleaseThread() -> EGLBoolean;
    fn eglGetProcAddress(procname: *const c_char) -> *mut c_void;
}

pub(crate) unsafe extern "C" fn egl_get_proc_address(
    _ctx: *mut c_void,
    name: *const c_char,
) -> *mut c_void {
    if name.is_null() {
        return ptr::null_mut();
    }
    let sym = eglGetProcAddress(name);
    if !sym.is_null() {
        sym
    } else {
        libc::dlsym(libc::RTLD_DEFAULT, name)
    }
}

#[repr(C)]
struct WlRegistryListener {
    global: unsafe extern "C" fn(
        data: *mut c_void,
        registry: *mut wl_proxy,
        name: u32,
        interface: *const c_char,
        version: u32,
    ),
    global_remove: unsafe extern "C" fn(data: *mut c_void, registry: *mut wl_proxy, name: u32),
}

#[derive(Default)]
struct BoundGlobals {
    compositor: Option<NonNull<wl_proxy>>,
    subcompositor: Option<NonNull<wl_proxy>>,
    viewporter: Option<NonNull<wl_proxy>>,
}

unsafe extern "C" fn registry_handle_global(
    data: *mut c_void,
    registry: *mut wl_proxy,
    name: u32,
    interface: *const c_char,
    version: u32,
) {
    if data.is_null() || registry.is_null() || interface.is_null() {
        return;
    }
    let globals = &mut *(data as *mut BoundGlobals);
    let iface_bytes = CStr::from_ptr(interface).to_bytes();

    if iface_bytes == b"wl_compositor" && globals.compositor.is_none() {
        let target_ver = version.clamp(1, 4);
        let ptr = wl_registry_bind(registry, name, &raw const wl_compositor_interface, target_ver);
        globals.compositor = NonNull::new(ptr);
    } else if iface_bytes == b"wl_subcompositor" && globals.subcompositor.is_none() {
        let ptr = wl_registry_bind(registry, name, &raw const wl_subcompositor_interface, 1);
        globals.subcompositor = NonNull::new(ptr);
    } else if iface_bytes == b"wp_viewporter" && globals.viewporter.is_none() {
        let ptr = wl_registry_bind(registry, name, &raw const WP_VIEWPORTER_INTERFACE, 1);
        globals.viewporter = NonNull::new(ptr);
    }
}

unsafe extern "C" fn registry_handle_global_remove(
    _data: *mut c_void,
    _registry: *mut wl_proxy,
    _name: u32,
) {
}

static REGISTRY_LISTENER: WlRegistryListener = WlRegistryListener {
    global: registry_handle_global,
    global_remove: registry_handle_global_remove,
};

unsafe fn wl_registry_bind(
    registry: *mut wl_proxy,
    name: u32,
    interface: *const wl_interface,
    version: u32,
) -> *mut wl_proxy {
    wl_proxy_marshal_flags(
        registry,
        WL_REGISTRY_BIND,
        interface,
        version,
        0,
        name,
        (*interface).name,
        version,
        ptr::null_mut::<c_void>(),
    )
}

unsafe fn wl_compositor_create_surface(compositor: *mut wl_proxy) -> *mut wl_proxy {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, compositor);
    wl_proxy_marshal_flags(
        compositor,
        WL_COMPOSITOR_CREATE_SURFACE,
        &raw const wl_surface_interface,
        ver,
        0,
        ptr::null_mut::<c_void>(),
    )
}

unsafe fn wl_compositor_create_region(compositor: *mut wl_proxy) -> *mut wl_proxy {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, compositor);
    wl_proxy_marshal_flags(
        compositor,
        WL_COMPOSITOR_CREATE_REGION,
        &raw const wl_region_interface,
        ver,
        0,
        ptr::null_mut::<c_void>(),
    )
}

unsafe fn wl_region_destroy(region: *mut wl_proxy) {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, region);
    let _ = wl_proxy_marshal_flags(
        region,
        WL_REGION_DESTROY,
        ptr::null(),
        ver,
        WL_MARSHAL_FLAG_DESTROY,
    );
}

unsafe fn wl_surface_set_input_region(surface: *mut wl_proxy, region: *mut wl_proxy) {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, surface);
    let _ = wl_proxy_marshal_flags(
        surface,
        WL_SURFACE_SET_INPUT_REGION,
        ptr::null(),
        ver,
        0,
        region,
    );
}

unsafe fn wl_surface_set_buffer_scale(surface: *mut wl_proxy, scale: i32) {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, surface);
    if ver >= 3 {
        let _ = wl_proxy_marshal_flags(
            surface,
            WL_SURFACE_SET_BUFFER_SCALE,
            ptr::null(),
            ver,
            0,
            scale,
        );
    }
}

unsafe fn wl_surface_commit(surface: *mut wl_proxy) {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, surface);
    let _ = wl_proxy_marshal_flags(surface, WL_SURFACE_COMMIT, ptr::null(), ver, 0);
}

unsafe fn wl_surface_destroy(surface: *mut wl_proxy) {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, surface);
    let _ = wl_proxy_marshal_flags(
        surface,
        WL_SURFACE_DESTROY,
        ptr::null(),
        ver,
        WL_MARSHAL_FLAG_DESTROY,
    );
}

unsafe fn wl_subcompositor_get_subsurface(
    subcompositor: *mut wl_proxy,
    surface: *mut wl_proxy,
    parent: *mut wl_proxy,
) -> *mut wl_proxy {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, subcompositor);
    wl_proxy_marshal_flags(
        subcompositor,
        WL_SUBCOMPOSITOR_GET_SUBSURFACE,
        &raw const wl_subsurface_interface,
        ver,
        0,
        ptr::null_mut::<c_void>(),
        surface,
        parent,
    )
}

unsafe fn wl_subcompositor_destroy(subcompositor: *mut wl_proxy) {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, subcompositor);
    let _ = wl_proxy_marshal_flags(
        subcompositor,
        WL_SUBCOMPOSITOR_DESTROY,
        ptr::null(),
        ver,
        WL_MARSHAL_FLAG_DESTROY,
    );
}

unsafe fn wl_subsurface_set_position(subsurface: *mut wl_proxy, x: i32, y: i32) {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, subsurface);
    let _ = wl_proxy_marshal_flags(
        subsurface,
        WL_SUBSURFACE_SET_POSITION,
        ptr::null(),
        ver,
        0,
        x,
        y,
    );
}

unsafe fn wl_subsurface_place_below(subsurface: *mut wl_proxy, sibling: *mut wl_proxy) {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, subsurface);
    let _ = wl_proxy_marshal_flags(
        subsurface,
        WL_SUBSURFACE_PLACE_BELOW,
        ptr::null(),
        ver,
        0,
        sibling,
    );
}

unsafe fn wl_subsurface_set_desync(subsurface: *mut wl_proxy) {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, subsurface);
    let _ = wl_proxy_marshal_flags(subsurface, WL_SUBSURFACE_SET_DESYNC, ptr::null(), ver, 0);
}

unsafe fn wl_subsurface_destroy(subsurface: *mut wl_proxy) {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, subsurface);
    let _ = wl_proxy_marshal_flags(
        subsurface,
        WL_SUBSURFACE_DESTROY,
        ptr::null(),
        ver,
        WL_MARSHAL_FLAG_DESTROY,
    );
}

unsafe fn wp_viewporter_get_viewport(
    viewporter: *mut wl_proxy,
    surface: *mut wl_proxy,
) -> *mut wl_proxy {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, viewporter);
    wl_proxy_marshal_flags(
        viewporter,
        WP_VIEWPORTER_GET_VIEWPORT,
        &raw const WP_VIEWPORT_INTERFACE,
        ver,
        0,
        ptr::null_mut::<c_void>(),
        surface,
    )
}

unsafe fn wp_viewporter_destroy(viewporter: *mut wl_proxy) {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, viewporter);
    let _ = wl_proxy_marshal_flags(
        viewporter,
        WP_VIEWPORTER_DESTROY,
        ptr::null(),
        ver,
        WL_MARSHAL_FLAG_DESTROY,
    );
}

unsafe fn wp_viewport_set_destination(viewport: *mut wl_proxy, width: i32, height: i32) {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, viewport);
    let _ = wl_proxy_marshal_flags(
        viewport,
        WP_VIEWPORT_SET_DESTINATION,
        ptr::null(),
        ver,
        0,
        width,
        height,
    );
}

unsafe fn wp_viewport_destroy(viewport: *mut wl_proxy) {
    let ver = ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, viewport);
    let _ = wl_proxy_marshal_flags(
        viewport,
        WP_VIEWPORT_DESTROY,
        ptr::null(),
        ver,
        WL_MARSHAL_FLAG_DESTROY,
    );
}

#[inline]
pub(crate) fn compute_surface_dimensions(
    logical_width: i32,
    logical_height: i32,
    scale_factor: f32,
    has_viewporter: bool,
) -> (i32, i32, i32, i32, i32) {
    let lw = logical_width.max(1);
    let lh = logical_height.max(1);
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };

    if has_viewporter {
        let bw = ((lw as f32 * scale).round() as i32).max(1);
        let bh = ((lh as f32 * scale).round() as i32).max(1);
        (lw, lh, bw, bh, 1)
    } else {
        let int_scale = (scale.round() as i32).max(1);
        let bw = (lw * int_scale).max(1);
        let bh = (lh * int_scale).max(1);
        (lw, lh, bw, bh, int_scale)
    }
}

pub(crate) struct WaylandVideoSubsurface {
    wl_display: NonNull<wl_display>,
    queue: Option<NonNull<wl_event_queue>>,
    registry: Option<NonNull<wl_proxy>>,
    compositor: Option<NonNull<wl_proxy>>,
    subcompositor: Option<NonNull<wl_proxy>>,
    viewporter: Option<NonNull<wl_proxy>>,
    video_surface: Option<NonNull<wl_proxy>>,
    subsurface: Option<NonNull<wl_proxy>>,
    viewport: Option<NonNull<wl_proxy>>,
    egl_window: Option<NonNull<wl_egl_window>>,
    egl_display: EGLDisplay,
    egl_context: EGLContext,
    egl_surface: EGLSurface,
    gl_clear_color: Option<unsafe extern "C" fn(f32, f32, f32, f32)>,
    gl_clear: Option<unsafe extern "C" fn(u32)>,
    gl_viewport: Option<unsafe extern "C" fn(i32, i32, i32, i32)>,
    x: i32,
    y: i32,
    logical_width: i32,
    logical_height: i32,
    buffer_width: i32,
    buffer_height: i32,
    buffer_scale: i32,
}

impl WaylandVideoSubsurface {
    pub(crate) unsafe fn new(
        wl_display_ptr: *mut c_void,
        parent_wl_surface_ptr: *mut c_void,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        scale_factor: f32,
    ) -> Result<Self, MpvError> {
        let wl_display = NonNull::new(wl_display_ptr as *mut wl_display)
            .ok_or_else(|| MpvError::Wayland("wl_display pointer is null".into()))?;
        let parent_surface = NonNull::new(parent_wl_surface_ptr as *mut wl_proxy)
            .ok_or_else(|| MpvError::Wayland("parent wl_surface pointer is null".into()))?;

        let mut this = Self {
            wl_display,
            queue: None,
            registry: None,
            compositor: None,
            subcompositor: None,
            viewporter: None,
            video_surface: None,
            subsurface: None,
            viewport: None,
            egl_window: None,
            egl_display: EGL_NO_DISPLAY,
            egl_context: EGL_NO_CONTEXT,
            egl_surface: EGL_NO_SURFACE,
            gl_clear_color: None,
            gl_clear: None,
            gl_viewport: None,
            x,
            y,
            logical_width: width.max(1),
            logical_height: height.max(1),
            buffer_width: width.max(1),
            buffer_height: height.max(1),
            buffer_scale: 1,
        };

        let raw_queue = ffi_dispatch!(
            wayland_client_handle(),
            wl_display_create_queue,
            wl_display.as_ptr()
        );
        let queue = NonNull::new(raw_queue)
            .ok_or_else(|| MpvError::Wayland("wl_display_create_queue failed".into()))?;
        this.queue = Some(queue);

        let display_wrapper = ffi_dispatch!(
            wayland_client_handle(),
            wl_proxy_create_wrapper,
            wl_display.as_ptr() as *mut wl_proxy
        );
        if display_wrapper.is_null() {
            return Err(MpvError::Wayland("wl_proxy_create_wrapper failed".into()));
        }
        ffi_dispatch!(
            wayland_client_handle(),
            wl_proxy_set_queue,
            display_wrapper,
            queue.as_ptr()
        );

        let display_ver =
            ffi_dispatch!(wayland_client_handle(), wl_proxy_get_version, display_wrapper);
        let raw_registry = wl_proxy_marshal_flags(
            display_wrapper,
            WL_DISPLAY_GET_REGISTRY,
            &raw const wl_registry_interface,
            display_ver,
            0,
            ptr::null_mut::<c_void>(),
        );
        ffi_dispatch!(
            wayland_client_handle(),
            wl_proxy_wrapper_destroy,
            display_wrapper
        );

        let registry = NonNull::new(raw_registry)
            .ok_or_else(|| MpvError::Wayland("wl_display_get_registry failed".into()))?;
        this.registry = Some(registry);

        let mut bound = BoundGlobals::default();
        let listener_ptr =
            (&raw const REGISTRY_LISTENER).cast::<extern "C" fn()>().cast_mut();
        let add_rc = ffi_dispatch!(
            wayland_client_handle(),
            wl_proxy_add_listener,
            registry.as_ptr(),
            listener_ptr,
            &mut bound as *mut BoundGlobals as *mut c_void
        );
        if add_rc < 0 {
            return Err(MpvError::Wayland("wl_proxy_add_listener failed".into()));
        }

        let rt_rc = ffi_dispatch!(
            wayland_client_handle(),
            wl_display_roundtrip_queue,
            wl_display.as_ptr(),
            queue.as_ptr()
        );
        ffi_dispatch!(
            wayland_client_handle(),
            wl_proxy_set_user_data,
            registry.as_ptr(),
            ptr::null_mut()
        );

        if rt_rc < 0 {
            return Err(MpvError::Wayland(
                "wl_display_roundtrip_queue failed while binding globals".into(),
            ));
        }

        this.compositor = bound.compositor;
        this.subcompositor = bound.subcompositor;
        this.viewporter = bound.viewporter;

        let compositor = this.compositor.ok_or_else(|| {
            MpvError::Wayland("Wayland compositor did not advertise wl_compositor".into())
        })?;
        let subcompositor = this.subcompositor.ok_or_else(|| {
            MpvError::Wayland("Wayland compositor did not advertise wl_subcompositor".into())
        })?;

        let video_surface = NonNull::new(wl_compositor_create_surface(compositor.as_ptr()))
            .ok_or_else(|| MpvError::Wayland("wl_compositor_create_surface failed".into()))?;
        this.video_surface = Some(video_surface);

        let empty_region = wl_compositor_create_region(compositor.as_ptr());
        if empty_region.is_null() {
            return Err(MpvError::Wayland("wl_compositor_create_region failed".into()));
        }
        wl_surface_set_input_region(video_surface.as_ptr(), empty_region);
        wl_region_destroy(empty_region);

        let subsurface = NonNull::new(wl_subcompositor_get_subsurface(
            subcompositor.as_ptr(),
            video_surface.as_ptr(),
            parent_surface.as_ptr(),
        ))
        .ok_or_else(|| MpvError::Wayland("wl_subcompositor_get_subsurface failed".into()))?;
        this.subsurface = Some(subsurface);

        wl_subsurface_place_below(subsurface.as_ptr(), parent_surface.as_ptr());
        wl_subsurface_set_desync(subsurface.as_ptr());
        wl_subsurface_set_position(subsurface.as_ptr(), x, y);

        let has_viewporter = if let Some(vp_global) = this.viewporter {
            let vp = NonNull::new(wp_viewporter_get_viewport(
                vp_global.as_ptr(),
                video_surface.as_ptr(),
            ))
            .ok_or_else(|| MpvError::Wayland("wp_viewporter_get_viewport failed".into()))?;
            this.viewport = Some(vp);
            true
        } else {
            false
        };

        let (lw, lh, bw, bh, buf_scale) =
            compute_surface_dimensions(width, height, scale_factor, has_viewporter);
        this.logical_width = lw;
        this.logical_height = lh;
        this.buffer_width = bw;
        this.buffer_height = bh;
        this.buffer_scale = buf_scale;

        if let Some(vp) = this.viewport {
            wp_viewport_set_destination(vp.as_ptr(), lw, lh);
        } else {
            wl_surface_set_buffer_scale(video_surface.as_ptr(), buf_scale);
        }

        wl_surface_commit(video_surface.as_ptr());
        ffi_dispatch!(wayland_client_handle(), wl_display_flush, wl_display.as_ptr());

        let raw_egl_win = ffi_dispatch!(
            wayland_egl_handle(),
            wl_egl_window_create,
            video_surface.as_ptr(),
            bw,
            bh
        );
        let egl_window = NonNull::new(raw_egl_win)
            .ok_or_else(|| MpvError::Egl("wl_egl_window_create failed".into()))?;
        this.egl_window = Some(egl_window);

        this.init_egl(egl_window.as_ptr())?;
        this.clear_transparent_and_swap();

        Ok(this)
    }

    unsafe fn init_egl(&mut self, egl_window: *mut wl_egl_window) -> Result<(), MpvError> {
        let mut dpy = eglGetPlatformDisplay(
            EGL_PLATFORM_WAYLAND_KHR,
            self.wl_display.as_ptr() as *mut c_void,
            ptr::null(),
        );
        if dpy == EGL_NO_DISPLAY {
            dpy = eglGetDisplay(self.wl_display.as_ptr() as *mut c_void);
        }
        if dpy == EGL_NO_DISPLAY {
            return Err(MpvError::Egl(format!(
                "eglGetPlatformDisplay/eglGetDisplay failed (0x{:x})",
                eglGetError()
            )));
        }

        let mut major: EGLint = 0;
        let mut minor: EGLint = 0;
        if eglInitialize(dpy, &mut major, &mut minor) == EGL_FALSE {
            return Err(MpvError::Egl(format!(
                "eglInitialize failed (0x{:x})",
                eglGetError()
            )));
        }
        self.egl_display = dpy;

        let (config, ctx) = Self::choose_config_and_context(dpy)?;
        self.egl_context = ctx;

        let mut surface =
            eglCreatePlatformWindowSurface(dpy, config, egl_window as *mut c_void, ptr::null());
        if surface == EGL_NO_SURFACE {
            surface = eglCreateWindowSurface(dpy, config, egl_window as *mut c_void, ptr::null());
        }
        if surface == EGL_NO_SURFACE {
            return Err(MpvError::Egl(format!(
                "eglCreatePlatformWindowSurface failed (0x{:x})",
                eglGetError()
            )));
        }
        self.egl_surface = surface;

        if eglMakeCurrent(dpy, surface, surface, ctx) == EGL_FALSE {
            return Err(MpvError::Egl(format!(
                "eglMakeCurrent failed (0x{:x})",
                eglGetError()
            )));
        }

        let _ = eglSwapInterval(dpy, 0);

        let clear_color_ptr = egl_get_proc_address(ptr::null_mut(), c"glClearColor".as_ptr());
        if !clear_color_ptr.is_null() {
            self.gl_clear_color = Some(core::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(f32, f32, f32, f32),
            >(clear_color_ptr));
        }
        let clear_ptr = egl_get_proc_address(ptr::null_mut(), c"glClear".as_ptr());
        if !clear_ptr.is_null() {
            self.gl_clear =
                Some(core::mem::transmute::<*mut c_void, unsafe extern "C" fn(u32)>(clear_ptr));
        }
        let viewport_ptr = egl_get_proc_address(ptr::null_mut(), c"glViewport".as_ptr());
        if !viewport_ptr.is_null() {
            self.gl_viewport = Some(core::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(i32, i32, i32, i32),
            >(viewport_ptr));
        }

        Ok(())
    }

    unsafe fn choose_config_and_context(
        dpy: EGLDisplay,
    ) -> Result<(EGLConfig, EGLContext), MpvError> {
        if eglBindAPI(EGL_OPENGL_API) != EGL_FALSE {
            if let Some(cfg) = Self::pick_config(dpy, EGL_OPENGL_BIT) {
                let core_attribs = [
                    EGL_CONTEXT_MAJOR_VERSION,
                    3,
                    EGL_CONTEXT_MINOR_VERSION,
                    2,
                    EGL_CONTEXT_OPENGL_PROFILE_MASK,
                    EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT,
                    EGL_NONE,
                ];
                let ctx = eglCreateContext(dpy, cfg, EGL_NO_CONTEXT, core_attribs.as_ptr());
                if ctx != EGL_NO_CONTEXT {
                    return Ok((cfg, ctx));
                }
                let fallback_attribs = [EGL_NONE];
                let ctx = eglCreateContext(dpy, cfg, EGL_NO_CONTEXT, fallback_attribs.as_ptr());
                if ctx != EGL_NO_CONTEXT {
                    return Ok((cfg, ctx));
                }
            }
        }

        if eglBindAPI(EGL_OPENGL_ES_API) != EGL_FALSE {
            for (renderable, ver) in [(EGL_OPENGL_ES3_BIT, 3), (EGL_OPENGL_ES2_BIT, 2)] {
                if let Some(cfg) = Self::pick_config(dpy, renderable) {
                    let es_attribs = [EGL_CONTEXT_MAJOR_VERSION, ver, EGL_NONE];
                    let ctx = eglCreateContext(dpy, cfg, EGL_NO_CONTEXT, es_attribs.as_ptr());
                    if ctx != EGL_NO_CONTEXT {
                        return Ok((cfg, ctx));
                    }
                }
            }
        }

        Err(MpvError::Egl(format!(
            "Failed to create OpenGL or OpenGL ES EGLContext (0x{:x})",
            eglGetError()
        )))
    }

    unsafe fn pick_config(dpy: EGLDisplay, renderable_bit: EGLint) -> Option<EGLConfig> {
        for alpha_bits in [8, 0] {
            let attribs = [
                EGL_SURFACE_TYPE,
                EGL_WINDOW_BIT,
                EGL_RENDERABLE_TYPE,
                renderable_bit,
                EGL_RED_SIZE,
                8,
                EGL_GREEN_SIZE,
                8,
                EGL_BLUE_SIZE,
                8,
                EGL_ALPHA_SIZE,
                alpha_bits,
                EGL_NONE,
            ];
            let mut config: EGLConfig = ptr::null_mut();
            let mut num_configs: EGLint = 0;
            if eglChooseConfig(dpy, attribs.as_ptr(), &mut config, 1, &mut num_configs)
                != EGL_FALSE
                && num_configs > 0
                && !config.is_null()
            {
                return Some(config);
            }
        }
        None
    }

    #[inline]
    pub(crate) fn wl_display_ptr(&self) -> *mut c_void {
        self.wl_display.as_ptr() as *mut c_void
    }

    #[inline]
    pub(crate) fn buffer_size(&self) -> (i32, i32) {
        (self.buffer_width, self.buffer_height)
    }

    pub(crate) unsafe fn make_current(&self) -> bool {
        if self.egl_display == EGL_NO_DISPLAY
            || self.egl_surface == EGL_NO_SURFACE
            || self.egl_context == EGL_NO_CONTEXT
        {
            return false;
        }
        eglMakeCurrent(
            self.egl_display,
            self.egl_surface,
            self.egl_surface,
            self.egl_context,
        ) != EGL_FALSE
    }

    pub(crate) unsafe fn dispatch_pending_queue(&self) {
        if let Some(queue) = self.queue {
            let _ = ffi_dispatch!(
                wayland_client_handle(),
                wl_display_dispatch_queue_pending,
                self.wl_display.as_ptr(),
                queue.as_ptr()
            );
        }
    }

    pub(crate) unsafe fn set_geometry(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        scale_factor: f32,
    ) -> bool {
        let mut pos_changed = false;
        if x != self.x || y != self.y {
            self.x = x;
            self.y = y;
            if let Some(subsurface) = self.subsurface {
                wl_subsurface_set_position(subsurface.as_ptr(), x, y);
                pos_changed = true;
            }
        }

        let has_viewporter = self.viewport.is_some();
        let (lw, lh, bw, bh, buf_scale) =
            compute_surface_dimensions(width, height, scale_factor, has_viewporter);

        let mut size_changed = false;
        if lw != self.logical_width || lh != self.logical_height {
            self.logical_width = lw;
            self.logical_height = lh;
            if let Some(vp) = self.viewport {
                wp_viewport_set_destination(vp.as_ptr(), lw, lh);
                size_changed = true;
            }
        }

        if !has_viewporter && buf_scale != self.buffer_scale {
            self.buffer_scale = buf_scale;
            if let Some(surface) = self.video_surface {
                wl_surface_set_buffer_scale(surface.as_ptr(), buf_scale);
                size_changed = true;
            }
        }

        if bw != self.buffer_width || bh != self.buffer_height {
            self.buffer_width = bw;
            self.buffer_height = bh;
            if let Some(egl_win) = self.egl_window {
                ffi_dispatch!(
                    wayland_egl_handle(),
                    wl_egl_window_resize,
                    egl_win.as_ptr(),
                    bw,
                    bh,
                    0,
                    0
                );
            }
            size_changed = true;
        }

        if pos_changed || size_changed {
            let _ = ffi_dispatch!(
                wayland_client_handle(),
                wl_display_flush,
                self.wl_display.as_ptr()
            );
        }

        size_changed
    }

    pub(crate) unsafe fn clear_transparent_and_swap(&self) {
        if let Some(gl_viewport) = self.gl_viewport {
            gl_viewport(0, 0, self.buffer_width, self.buffer_height);
        }
        if let Some(gl_clear_color) = self.gl_clear_color {
            gl_clear_color(0.0, 0.0, 0.0, 0.0);
        }
        if let Some(gl_clear) = self.gl_clear {
            gl_clear(GL_COLOR_BUFFER_BIT);
        }
        self.swap_buffers();
    }

    pub(crate) unsafe fn swap_buffers(&self) {
        if self.egl_display != EGL_NO_DISPLAY && self.egl_surface != EGL_NO_SURFACE {
            let _ = eglSwapBuffers(self.egl_display, self.egl_surface);
            let _ = ffi_dispatch!(
                wayland_client_handle(),
                wl_display_flush,
                self.wl_display.as_ptr()
            );
        }
    }
}

impl Drop for WaylandVideoSubsurface {
    fn drop(&mut self) {
        unsafe {
            if self.egl_display != EGL_NO_DISPLAY {
                let _ = eglMakeCurrent(
                    self.egl_display,
                    EGL_NO_SURFACE,
                    EGL_NO_SURFACE,
                    EGL_NO_CONTEXT,
                );
                if self.egl_surface != EGL_NO_SURFACE {
                    let _ = eglDestroySurface(self.egl_display, self.egl_surface);
                    self.egl_surface = EGL_NO_SURFACE;
                }
                if self.egl_context != EGL_NO_CONTEXT {
                    let _ = eglDestroyContext(self.egl_display, self.egl_context);
                    self.egl_context = EGL_NO_CONTEXT;
                }
                self.egl_display = EGL_NO_DISPLAY;
            }
            let _ = eglReleaseThread();

            if let Some(egl_win) = self.egl_window.take() {
                ffi_dispatch!(wayland_egl_handle(), wl_egl_window_destroy, egl_win.as_ptr());
            }

            if let Some(vp) = self.viewport.take() {
                wp_viewport_destroy(vp.as_ptr());
            }

            if let Some(sub) = self.subsurface.take() {
                wl_subsurface_destroy(sub.as_ptr());
            }

            if let Some(surf) = self.video_surface.take() {
                wl_surface_destroy(surf.as_ptr());
            }

            if let Some(viewporter) = self.viewporter.take() {
                wp_viewporter_destroy(viewporter.as_ptr());
            }
            if let Some(subcompositor) = self.subcompositor.take() {
                wl_subcompositor_destroy(subcompositor.as_ptr());
            }
            if let Some(compositor) = self.compositor.take() {
                ffi_dispatch!(
                    wayland_client_handle(),
                    wl_proxy_destroy,
                    compositor.as_ptr()
                );
            }
            if let Some(registry) = self.registry.take() {
                ffi_dispatch!(
                    wayland_client_handle(),
                    wl_proxy_destroy,
                    registry.as_ptr()
                );
            }

            let _ = ffi_dispatch!(
                wayland_client_handle(),
                wl_display_flush,
                self.wl_display.as_ptr()
            );

            if let Some(queue) = self.queue.take() {
                ffi_dispatch!(
                    wayland_client_handle(),
                    wl_event_queue_destroy,
                    queue.as_ptr()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewporter_interface_metadata_matches_protocol() {
        unsafe {
            assert_eq!(
                CStr::from_ptr(WP_VIEWPORTER_INTERFACE.name).to_bytes(),
                b"wp_viewporter"
            );
            assert_eq!(WP_VIEWPORTER_INTERFACE.version, 1);
            assert_eq!(WP_VIEWPORTER_INTERFACE.request_count, 2);

            assert_eq!(
                CStr::from_ptr(WP_VIEWPORT_INTERFACE.name).to_bytes(),
                b"wp_viewport"
            );
            assert_eq!(WP_VIEWPORT_INTERFACE.version, 1);
            assert_eq!(WP_VIEWPORT_INTERFACE.request_count, 3);
        }
    }

    #[test]
    fn compute_dimensions_handles_fractional_and_fallback_scales() {
        let (lw, lh, bw, bh, scale) = compute_surface_dimensions(1920, 1080, 1.5, true);
        assert_eq!((lw, lh, bw, bh, scale), (1920, 1080, 2880, 1620, 1));

        let (lw2, lh2, bw2, bh2, scale2) = compute_surface_dimensions(1920, 1080, 1.6, false);
        assert_eq!((lw2, lh2, bw2, bh2, scale2), (1920, 1080, 3840, 2160, 2));

        let (lw3, lh3, bw3, bh3, _) = compute_surface_dimensions(0, -10, 0.0, true);
        assert_eq!((lw3, lh3, bw3, bh3), (1, 1, 1, 1));
    }

    #[test]
    fn null_wayland_handles_return_clean_error() {
        let res = unsafe {
            WaylandVideoSubsurface::new(ptr::null_mut(), ptr::null_mut(), 0, 0, 800, 600, 1.0)
        };
        assert!(matches!(res, Err(MpvError::Wayland(_))));
    }
}
