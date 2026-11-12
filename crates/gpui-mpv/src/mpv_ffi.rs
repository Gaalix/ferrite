#![allow(non_camel_case_types, dead_code)]

use core::ffi::{c_char, c_double, c_int, c_ulong, c_void, CStr};

#[repr(C)]
pub struct mpv_handle {
    _private: [u8; 0],
}

#[repr(C)]
pub struct mpv_render_context {
    _private: [u8; 0],
}

pub type mpv_format = c_int;

pub const MPV_FORMAT_NONE: mpv_format = 0;
pub const MPV_FORMAT_STRING: mpv_format = 1;
pub const MPV_FORMAT_OSD_STRING: mpv_format = 2;
pub const MPV_FORMAT_FLAG: mpv_format = 3;
pub const MPV_FORMAT_INT64: mpv_format = 4;
pub const MPV_FORMAT_DOUBLE: mpv_format = 5;
pub const MPV_FORMAT_NODE: mpv_format = 6;
pub const MPV_FORMAT_NODE_ARRAY: mpv_format = 7;
pub const MPV_FORMAT_NODE_MAP: mpv_format = 8;
pub const MPV_FORMAT_BYTE_ARRAY: mpv_format = 9;

pub type mpv_event_id = c_int;

pub const MPV_EVENT_NONE: mpv_event_id = 0;
pub const MPV_EVENT_SHUTDOWN: mpv_event_id = 1;
pub const MPV_EVENT_LOG_MESSAGE: mpv_event_id = 2;
pub const MPV_EVENT_GET_PROPERTY_REPLY: mpv_event_id = 3;
pub const MPV_EVENT_SET_PROPERTY_REPLY: mpv_event_id = 4;
pub const MPV_EVENT_COMMAND_REPLY: mpv_event_id = 5;
pub const MPV_EVENT_START_FILE: mpv_event_id = 6;
pub const MPV_EVENT_END_FILE: mpv_event_id = 7;
pub const MPV_EVENT_FILE_LOADED: mpv_event_id = 8;
pub const MPV_EVENT_CLIENT_MESSAGE: mpv_event_id = 16;
pub const MPV_EVENT_VIDEO_RECONFIG: mpv_event_id = 17;
pub const MPV_EVENT_AUDIO_RECONFIG: mpv_event_id = 18;
pub const MPV_EVENT_SEEK: mpv_event_id = 20;
pub const MPV_EVENT_PLAYBACK_RESTART: mpv_event_id = 21;
pub const MPV_EVENT_PROPERTY_CHANGE: mpv_event_id = 22;
pub const MPV_EVENT_QUEUE_OVERFLOW: mpv_event_id = 24;
pub const MPV_EVENT_HOOK: mpv_event_id = 25;

pub const MPV_END_FILE_REASON_EOF: c_int = 0;
pub const MPV_END_FILE_REASON_STOP: c_int = 2;
pub const MPV_END_FILE_REASON_QUIT: c_int = 3;
pub const MPV_END_FILE_REASON_ERROR: c_int = 4;
pub const MPV_END_FILE_REASON_REDIRECT: c_int = 5;

#[repr(C)]
#[derive(Copy, Clone)]
pub union mpv_node_union {
    pub string: *mut c_char,
    pub flag: c_int,
    pub int64: i64,
    pub double_: c_double,
    pub list: *mut mpv_node_list,
    pub ba: *mut c_void,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct mpv_node {
    pub u: mpv_node_union,
    pub format: mpv_format,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct mpv_node_list {
    pub num: c_int,
    pub values: *mut mpv_node,
    pub keys: *mut *mut c_char,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct mpv_event {
    pub event_id: mpv_event_id,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct mpv_event_property {
    pub name: *const c_char,
    pub format: mpv_format,
    pub data: *mut c_void,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct mpv_event_end_file {
    pub reason: c_int,
    pub error: c_int,
    pub playlist_entry_id: i64,
    pub playlist_insert_id: i64,
    pub playlist_insert_num_entries: c_int,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct mpv_event_log_message {
    pub prefix: *const c_char,
    pub level: *const c_char,
    pub text: *const c_char,
    pub log_level: c_int,
}

pub type mpv_render_param_type = c_int;

pub const MPV_RENDER_PARAM_INVALID: mpv_render_param_type = 0;
pub const MPV_RENDER_PARAM_API_TYPE: mpv_render_param_type = 1;
pub const MPV_RENDER_PARAM_OPENGL_INIT_PARAMS: mpv_render_param_type = 2;
pub const MPV_RENDER_PARAM_OPENGL_FBO: mpv_render_param_type = 3;
pub const MPV_RENDER_PARAM_FLIP_Y: mpv_render_param_type = 4;
pub const MPV_RENDER_PARAM_DEPTH: mpv_render_param_type = 5;
pub const MPV_RENDER_PARAM_WL_DISPLAY: mpv_render_param_type = 9;
pub const MPV_RENDER_PARAM_ADVANCED_CONTROL: mpv_render_param_type = 10;
pub const MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME: mpv_render_param_type = 12;
pub const MPV_RENDER_PARAM_SKIP_RENDERING: mpv_render_param_type = 13;
pub const MPV_RENDER_PARAM_SW_SIZE: mpv_render_param_type = 17;
pub const MPV_RENDER_PARAM_SW_FORMAT: mpv_render_param_type = 18;
pub const MPV_RENDER_PARAM_SW_STRIDE: mpv_render_param_type = 19;
pub const MPV_RENDER_PARAM_SW_POINTER: mpv_render_param_type = 20;

pub const MPV_RENDER_API_TYPE_OPENGL: &[u8] = b"opengl\0";
pub const MPV_RENDER_API_TYPE_SW: &[u8] = b"sw\0";
pub const MPV_RENDER_UPDATE_FRAME: u64 = 1 << 0;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct mpv_render_param {
    pub type_: mpv_render_param_type,
    pub data: *mut c_void,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct mpv_opengl_init_params {
    pub get_proc_address:
        Option<unsafe extern "C" fn(ctx: *mut c_void, name: *const c_char) -> *mut c_void>,
    pub get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct mpv_opengl_fbo {
    pub fbo: c_int,
    pub w: c_int,
    pub h: c_int,
    pub internal_format: c_int,
}

pub type mpv_render_update_fn = unsafe extern "C" fn(cb_ctx: *mut c_void);

#[inline]
pub(crate) fn set_c_numeric_locale() {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr());
    }
}

#[cfg(not(target_os = "windows"))]
mod sys {
    use super::*;

    #[inline]
    pub(crate) fn ensure_mpv_loaded() -> Result<(), String> {
        Ok(())
    }

    unsafe extern "C" {
        pub fn mpv_client_api_version() -> c_ulong;
        pub fn mpv_error_string(error: c_int) -> *const c_char;
        pub fn mpv_free(data: *mut c_void);

        pub fn mpv_create() -> *mut mpv_handle;
        pub fn mpv_initialize(ctx: *mut mpv_handle) -> c_int;
        pub fn mpv_terminate_destroy(ctx: *mut mpv_handle);

        pub fn mpv_request_log_messages(ctx: *mut mpv_handle, min_level: *const c_char) -> c_int;
        pub fn mpv_set_option_string(
            ctx: *mut mpv_handle,
            name: *const c_char,
            data: *const c_char,
        ) -> c_int;

        pub fn mpv_command_async(
            ctx: *mut mpv_handle,
            reply_userdata: u64,
            args: *const *const c_char,
        ) -> c_int;

        pub fn mpv_set_property_async(
            ctx: *mut mpv_handle,
            reply_userdata: u64,
            name: *const c_char,
            format: mpv_format,
            data: *mut c_void,
        ) -> c_int;

        pub fn mpv_get_property(
            ctx: *mut mpv_handle,
            name: *const c_char,
            format: mpv_format,
            data: *mut c_void,
        ) -> c_int;

        pub fn mpv_observe_property(
            mpv: *mut mpv_handle,
            reply_userdata: u64,
            name: *const c_char,
            format: mpv_format,
        ) -> c_int;

        pub fn mpv_unobserve_property(
            mpv: *mut mpv_handle,
            registered_reply_userdata: u64,
        ) -> c_int;

        pub fn mpv_wait_event(ctx: *mut mpv_handle, timeout: c_double) -> *mut mpv_event;
        pub fn mpv_wakeup(ctx: *mut mpv_handle);
        pub fn mpv_free_node_contents(node: *mut mpv_node);

        pub fn mpv_render_context_create(
            res: *mut *mut mpv_render_context,
            mpv: *mut mpv_handle,
            params: *mut mpv_render_param,
        ) -> c_int;

        pub fn mpv_render_context_set_parameter(
            ctx: *mut mpv_render_context,
            param: mpv_render_param,
        ) -> c_int;

        pub fn mpv_render_context_get_info(
            ctx: *mut mpv_render_context,
            param: mpv_render_param,
        ) -> c_int;

        pub fn mpv_render_context_set_update_callback(
            ctx: *mut mpv_render_context,
            callback: Option<mpv_render_update_fn>,
            callback_ctx: *mut c_void,
        );

        pub fn mpv_render_context_update(ctx: *mut mpv_render_context) -> u64;

        pub fn mpv_render_context_render(
            ctx: *mut mpv_render_context,
            params: *mut mpv_render_param,
        ) -> c_int;

        pub fn mpv_render_context_report_swap(ctx: *mut mpv_render_context);
        pub fn mpv_render_context_free(ctx: *mut mpv_render_context);
    }
}

#[cfg(target_os = "windows")]
mod sys {
    use super::*;
    use core::ptr;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_APPLICATION_DIR,
        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };

    type FnClientApiVersion = unsafe extern "C" fn() -> c_ulong;
    type FnErrorString = unsafe extern "C" fn(c_int) -> *const c_char;
    type FnFree = unsafe extern "C" fn(*mut c_void);
    type FnCreate = unsafe extern "C" fn() -> *mut mpv_handle;
    type FnInitialize = unsafe extern "C" fn(*mut mpv_handle) -> c_int;
    type FnTerminateDestroy = unsafe extern "C" fn(*mut mpv_handle);
    type FnRequestLogMessages = unsafe extern "C" fn(*mut mpv_handle, *const c_char) -> c_int;
    type FnSetOptionString =
        unsafe extern "C" fn(*mut mpv_handle, *const c_char, *const c_char) -> c_int;
    type FnCommandAsync =
        unsafe extern "C" fn(*mut mpv_handle, u64, *const *const c_char) -> c_int;
    type FnSetPropertyAsync =
        unsafe extern "C" fn(*mut mpv_handle, u64, *const c_char, mpv_format, *mut c_void) -> c_int;
    type FnGetProperty =
        unsafe extern "C" fn(*mut mpv_handle, *const c_char, mpv_format, *mut c_void) -> c_int;
    type FnObserveProperty =
        unsafe extern "C" fn(*mut mpv_handle, u64, *const c_char, mpv_format) -> c_int;
    type FnUnobserveProperty = unsafe extern "C" fn(*mut mpv_handle, u64) -> c_int;
    type FnWaitEvent = unsafe extern "C" fn(*mut mpv_handle, c_double) -> *mut mpv_event;
    type FnWakeup = unsafe extern "C" fn(*mut mpv_handle);
    type FnFreeNodeContents = unsafe extern "C" fn(*mut mpv_node);
    type FnRenderContextCreate = unsafe extern "C" fn(
        *mut *mut mpv_render_context,
        *mut mpv_handle,
        *mut mpv_render_param,
    ) -> c_int;
    type FnRenderContextSetParameter =
        unsafe extern "C" fn(*mut mpv_render_context, mpv_render_param) -> c_int;
    type FnRenderContextGetInfo =
        unsafe extern "C" fn(*mut mpv_render_context, mpv_render_param) -> c_int;
    type FnRenderContextSetUpdateCallback =
        unsafe extern "C" fn(*mut mpv_render_context, Option<mpv_render_update_fn>, *mut c_void);
    type FnRenderContextUpdate = unsafe extern "C" fn(*mut mpv_render_context) -> u64;
    type FnRenderContextRender =
        unsafe extern "C" fn(*mut mpv_render_context, *mut mpv_render_param) -> c_int;
    type FnRenderContextReportSwap = unsafe extern "C" fn(*mut mpv_render_context);
    type FnRenderContextFree = unsafe extern "C" fn(*mut mpv_render_context);

    struct MpvDllApi {
        client_api_version: FnClientApiVersion,
        error_string: FnErrorString,
        free: FnFree,
        create: FnCreate,
        initialize: FnInitialize,
        terminate_destroy: FnTerminateDestroy,
        request_log_messages: FnRequestLogMessages,
        set_option_string: FnSetOptionString,
        command_async: FnCommandAsync,
        set_property_async: FnSetPropertyAsync,
        get_property: FnGetProperty,
        observe_property: FnObserveProperty,
        unobserve_property: FnUnobserveProperty,
        wait_event: FnWaitEvent,
        wakeup: FnWakeup,
        free_node_contents: FnFreeNodeContents,
        render_context_create: FnRenderContextCreate,
        render_context_set_parameter: FnRenderContextSetParameter,
        render_context_get_info: FnRenderContextGetInfo,
        render_context_set_update_callback: FnRenderContextSetUpdateCallback,
        render_context_update: FnRenderContextUpdate,
        render_context_render: FnRenderContextRender,
        render_context_report_swap: FnRenderContextReportSwap,
        render_context_free: FnRenderContextFree,
    }

    static MPV_API: OnceLock<Result<MpvDllApi, String>> = OnceLock::new();

    fn to_wide_nul(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(core::iter::once(0)).collect()
    }

    unsafe fn load_sym<T: Copy>(module: HMODULE, symbol: &[u8]) -> Result<T, String> {
        let proc = GetProcAddress(module, symbol.as_ptr()).ok_or_else(|| {
            let trimmed = symbol.split_last().map_or(symbol, |(_, head)| head);
            let name = core::str::from_utf8(trimmed).unwrap_or("unknown");
            format!("missing symbol {name} in libmpv-2.dll")
        })?;
        Ok(core::mem::transmute_copy::<unsafe extern "system" fn() -> isize, T>(&proc))
    }

    unsafe fn open_mpv_module() -> Result<HMODULE, String> {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let app_dll: PathBuf = exe_dir.join("libmpv-2.dll");
                if app_dll.is_absolute() {
                    let wide = to_wide_nul(app_dll.as_os_str());
                    let module = LoadLibraryExW(
                        wide.as_ptr(),
                        0,
                        LOAD_LIBRARY_SEARCH_APPLICATION_DIR
                            | LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR
                            | LOAD_LIBRARY_SEARCH_SYSTEM32,
                    );
                    if module != 0 {
                        return Ok(module);
                    }
                }
            }
        }

        let rel_wide = to_wide_nul(OsStr::new("libmpv-2.dll"));
        let app_module = LoadLibraryExW(
            rel_wide.as_ptr(),
            0,
            LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        );
        if app_module != 0 {
            return Ok(app_module);
        }

        if let Some(custom) = std::env::var_os("FERRITE_MPV_DLL") {
            let custom_path = Path::new(&custom);
            if custom_path.is_absolute() {
                let wide = to_wide_nul(custom_path.as_os_str());
                let module = LoadLibraryExW(
                    wide.as_ptr(),
                    0,
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
                );
                if module != 0 {
                    return Ok(module);
                }
            }
        }

        Err("libmpv-2.dll not found in application directory (or absolute FERRITE_MPV_DLL)".into())
    }

    fn init_mpv_api() -> Result<MpvDllApi, String> {
        unsafe {
            let module = open_mpv_module()?;
            Ok(MpvDllApi {
                client_api_version: load_sym(module, b"mpv_client_api_version\0")?,
                error_string: load_sym(module, b"mpv_error_string\0")?,
                free: load_sym(module, b"mpv_free\0")?,
                create: load_sym(module, b"mpv_create\0")?,
                initialize: load_sym(module, b"mpv_initialize\0")?,
                terminate_destroy: load_sym(module, b"mpv_terminate_destroy\0")?,
                request_log_messages: load_sym(module, b"mpv_request_log_messages\0")?,
                set_option_string: load_sym(module, b"mpv_set_option_string\0")?,
                command_async: load_sym(module, b"mpv_command_async\0")?,
                set_property_async: load_sym(module, b"mpv_set_property_async\0")?,
                get_property: load_sym(module, b"mpv_get_property\0")?,
                observe_property: load_sym(module, b"mpv_observe_property\0")?,
                unobserve_property: load_sym(module, b"mpv_unobserve_property\0")?,
                wait_event: load_sym(module, b"mpv_wait_event\0")?,
                wakeup: load_sym(module, b"mpv_wakeup\0")?,
                free_node_contents: load_sym(module, b"mpv_free_node_contents\0")?,
                render_context_create: load_sym(module, b"mpv_render_context_create\0")?,
                render_context_set_parameter: load_sym(
                    module,
                    b"mpv_render_context_set_parameter\0",
                )?,
                render_context_get_info: load_sym(module, b"mpv_render_context_get_info\0")?,
                render_context_set_update_callback: load_sym(
                    module,
                    b"mpv_render_context_set_update_callback\0",
                )?,
                render_context_update: load_sym(module, b"mpv_render_context_update\0")?,
                render_context_render: load_sym(module, b"mpv_render_context_render\0")?,
                render_context_report_swap: load_sym(module, b"mpv_render_context_report_swap\0")?,
                render_context_free: load_sym(module, b"mpv_render_context_free\0")?,
            })
        }
    }

    #[inline]
    fn api() -> Option<&'static MpvDllApi> {
        MPV_API.get_or_init(init_mpv_api).as_ref().ok()
    }

    pub(crate) fn ensure_mpv_loaded() -> Result<(), String> {
        MPV_API
            .get_or_init(init_mpv_api)
            .as_ref()
            .map(|_| ())
            .map_err(Clone::clone)
    }

    pub unsafe fn mpv_client_api_version() -> c_ulong {
        api().map_or(0, |f| (f.client_api_version)())
    }

    pub unsafe fn mpv_error_string(error: c_int) -> *const c_char {
        api().map_or(c"libmpv-2.dll unavailable".as_ptr(), |f| {
            (f.error_string)(error)
        })
    }

    pub unsafe fn mpv_free(data: *mut c_void) {
        if let Some(f) = api() {
            (f.free)(data);
        }
    }

    pub unsafe fn mpv_create() -> *mut mpv_handle {
        api().map_or(ptr::null_mut(), |f| (f.create)())
    }

    pub unsafe fn mpv_initialize(ctx: *mut mpv_handle) -> c_int {
        api().map_or(-1, |f| (f.initialize)(ctx))
    }

    pub unsafe fn mpv_terminate_destroy(ctx: *mut mpv_handle) {
        if let Some(f) = api() {
            (f.terminate_destroy)(ctx);
        }
    }

    pub unsafe fn mpv_request_log_messages(ctx: *mut mpv_handle, min_level: *const c_char) -> c_int {
        api().map_or(-1, |f| (f.request_log_messages)(ctx, min_level))
    }

    pub unsafe fn mpv_set_option_string(
        ctx: *mut mpv_handle,
        name: *const c_char,
        data: *const c_char,
    ) -> c_int {
        api().map_or(-1, |f| (f.set_option_string)(ctx, name, data))
    }

    pub unsafe fn mpv_command_async(
        ctx: *mut mpv_handle,
        reply_userdata: u64,
        args: *const *const c_char,
    ) -> c_int {
        api().map_or(-1, |f| (f.command_async)(ctx, reply_userdata, args))
    }

    pub unsafe fn mpv_set_property_async(
        ctx: *mut mpv_handle,
        reply_userdata: u64,
        name: *const c_char,
        format: mpv_format,
        data: *mut c_void,
    ) -> c_int {
        api().map_or(-1, |f| {
            (f.set_property_async)(ctx, reply_userdata, name, format, data)
        })
    }

    pub unsafe fn mpv_get_property(
        ctx: *mut mpv_handle,
        name: *const c_char,
        format: mpv_format,
        data: *mut c_void,
    ) -> c_int {
        api().map_or(-1, |f| (f.get_property)(ctx, name, format, data))
    }

    pub unsafe fn mpv_observe_property(
        mpv: *mut mpv_handle,
        reply_userdata: u64,
        name: *const c_char,
        format: mpv_format,
    ) -> c_int {
        api().map_or(-1, |f| {
            (f.observe_property)(mpv, reply_userdata, name, format)
        })
    }

    pub unsafe fn mpv_unobserve_property(
        mpv: *mut mpv_handle,
        registered_reply_userdata: u64,
    ) -> c_int {
        api().map_or(-1, |f| {
            (f.unobserve_property)(mpv, registered_reply_userdata)
        })
    }

    pub unsafe fn mpv_wait_event(ctx: *mut mpv_handle, timeout: c_double) -> *mut mpv_event {
        api().map_or(ptr::null_mut(), |f| (f.wait_event)(ctx, timeout))
    }

    pub unsafe fn mpv_wakeup(ctx: *mut mpv_handle) {
        if let Some(f) = api() {
            (f.wakeup)(ctx);
        }
    }

    pub unsafe fn mpv_free_node_contents(node: *mut mpv_node) {
        if let Some(f) = api() {
            (f.free_node_contents)(node);
        }
    }

    pub unsafe fn mpv_render_context_create(
        res: *mut *mut mpv_render_context,
        mpv: *mut mpv_handle,
        params: *mut mpv_render_param,
    ) -> c_int {
        api().map_or(-1, |f| (f.render_context_create)(res, mpv, params))
    }

    pub unsafe fn mpv_render_context_set_parameter(
        ctx: *mut mpv_render_context,
        param: mpv_render_param,
    ) -> c_int {
        api().map_or(-1, |f| (f.render_context_set_parameter)(ctx, param))
    }

    pub unsafe fn mpv_render_context_get_info(
        ctx: *mut mpv_render_context,
        param: mpv_render_param,
    ) -> c_int {
        api().map_or(-1, |f| (f.render_context_get_info)(ctx, param))
    }

    pub unsafe fn mpv_render_context_set_update_callback(
        ctx: *mut mpv_render_context,
        callback: Option<mpv_render_update_fn>,
        callback_ctx: *mut c_void,
    ) {
        if let Some(f) = api() {
            (f.render_context_set_update_callback)(ctx, callback, callback_ctx);
        }
    }

    pub unsafe fn mpv_render_context_update(ctx: *mut mpv_render_context) -> u64 {
        api().map_or(0, |f| (f.render_context_update)(ctx))
    }

    pub unsafe fn mpv_render_context_render(
        ctx: *mut mpv_render_context,
        params: *mut mpv_render_param,
    ) -> c_int {
        api().map_or(-1, |f| (f.render_context_render)(ctx, params))
    }

    pub unsafe fn mpv_render_context_report_swap(ctx: *mut mpv_render_context) {
        if let Some(f) = api() {
            (f.render_context_report_swap)(ctx);
        }
    }

    pub unsafe fn mpv_render_context_free(ctx: *mut mpv_render_context) {
        if let Some(f) = api() {
            (f.render_context_free)(ctx);
        }
    }
}

pub(crate) use sys::*;

pub fn error_to_string(code: c_int) -> String {
    unsafe {
        let ptr = mpv_error_string(code);
        if ptr.is_null() {
            format!("mpv error {code}")
        } else {
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    }
}

impl mpv_node {
    pub(crate) unsafe fn as_str(&self) -> Option<&str> {
        if self.format == MPV_FORMAT_STRING && !self.u.string.is_null() {
            CStr::from_ptr(self.u.string).to_str().ok()
        } else {
            None
        }
    }

    pub(crate) unsafe fn as_i64(&self) -> Option<i64> {
        if self.format == MPV_FORMAT_INT64 {
            Some(self.u.int64)
        } else {
            None
        }
    }

    pub(crate) unsafe fn as_bool(&self) -> Option<bool> {
        if self.format == MPV_FORMAT_FLAG {
            Some(self.u.flag != 0)
        } else {
            None
        }
    }

    pub(crate) unsafe fn as_array(&self) -> Option<&[mpv_node]> {
        if self.format != MPV_FORMAT_NODE_ARRAY || self.u.list.is_null() {
            return None;
        }
        let list = &*self.u.list;
        if list.num <= 0 || list.values.is_null() {
            return Some(&[]);
        }
        Some(core::slice::from_raw_parts(list.values, list.num as usize))
    }

    pub(crate) unsafe fn map_get(&self, target_key: &str) -> Option<&mpv_node> {
        if self.format != MPV_FORMAT_NODE_MAP || self.u.list.is_null() {
            return None;
        }
        let list = &*self.u.list;
        if list.num <= 0 || list.values.is_null() || list.keys.is_null() {
            return None;
        }
        let len = list.num as usize;
        let keys = core::slice::from_raw_parts(list.keys, len);
        let values = core::slice::from_raw_parts(list.values, len);
        for (k_ptr, val) in keys.iter().zip(values.iter()) {
            if !k_ptr.is_null() && CStr::from_ptr(*k_ptr).to_bytes() == target_key.as_bytes() {
                return Some(val);
            }
        }
        None
    }
}
