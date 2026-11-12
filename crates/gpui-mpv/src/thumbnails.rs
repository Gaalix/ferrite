use core::ffi::{c_char, c_int, c_void};
use core::ptr;
use std::collections::{HashSet, VecDeque};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use gpui::RenderImage;
use image::{Frame, RgbaImage};

use crate::mpv_ffi::{
    self, error_to_string, mpv_command_async, mpv_create, mpv_event_end_file, mpv_handle,
    mpv_initialize, mpv_render_context, mpv_render_context_create, mpv_render_context_free,
    mpv_render_context_render, mpv_render_context_set_update_callback, mpv_render_context_update,
    mpv_render_param, mpv_set_option_string, mpv_terminate_destroy, mpv_wait_event,
    MPV_END_FILE_REASON_ERROR, MPV_EVENT_COMMAND_REPLY, MPV_EVENT_END_FILE, MPV_EVENT_FILE_LOADED,
    MPV_EVENT_NONE, MPV_EVENT_PLAYBACK_RESTART, MPV_EVENT_SEEK, MPV_EVENT_SHUTDOWN,
    MPV_RENDER_API_TYPE_SW, MPV_RENDER_PARAM_API_TYPE, MPV_RENDER_PARAM_INVALID,
    MPV_RENDER_PARAM_SW_FORMAT, MPV_RENDER_PARAM_SW_POINTER, MPV_RENDER_PARAM_SW_SIZE,
    MPV_RENDER_PARAM_SW_STRIDE, MPV_RENDER_UPDATE_FRAME,
};
use crate::player::validate_stream_url;
use crate::MpvError;

const SW_FORMAT_BGR0: &[u8] = b"bgr0\0";

pub const DEFAULT_MAX_CACHE_ENTRIES: usize = 40;

#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct Aligned64([u8; 64]);

#[derive(Debug, Clone, Copy)]
pub struct ThumbnailerConfig {
    pub width: u32,
    pub height: u32,
    pub bucket_step_secs: f64,
    pub max_cache_entries: usize,
}

impl Default for ThumbnailerConfig {
    fn default() -> Self {
        Self {
            width: 240,
            height: 135,
            bucket_step_secs: 2.0,
            max_cache_entries: DEFAULT_MAX_CACHE_ENTRIES,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ThumbnailFrame {
    pub width: u32,
    pub height: u32,
    pub bgra_bytes: Arc<Vec<u8>>,
    pub bgra_or_rgba_bytes: Arc<Vec<u8>>,
    pub render_image: Arc<RenderImage>,
    pub timestamp_secs: f64,
}

impl ThumbnailFrame {
    pub fn to_rgba_bytes(&self) -> Vec<u8> {
        let mut out = self.bgra_bytes.as_ref().clone();
        for px in out.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        out
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct CacheKey {
    url: String,
    bucket_ms: u64,
}

struct ThumbnailLruCache {
    capacity: usize,
    entries: VecDeque<(CacheKey, ThumbnailFrame)>,
}

impl ThumbnailLruCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::with_capacity(capacity.clamp(1, 256)),
        }
    }

    fn get(&mut self, key: &CacheKey) -> Option<ThumbnailFrame> {
        let idx = self.entries.iter().position(|(k, _)| k == key)?;
        let pair = self.entries.remove(idx)?;
        let frame = pair.1.clone();
        self.entries.push_back(pair);
        Some(frame)
    }

    fn insert(&mut self, key: CacheKey, frame: ThumbnailFrame) {
        if let Some(idx) = self.entries.iter().position(|(k, _)| k == &key) {
            self.entries.remove(idx);
        } else if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back((key, frame));
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.entries.shrink_to_fit();
    }
}

#[inline]
fn quantize_bucket_ms(timestamp_secs: f64, step_secs: f64) -> (f64, u64) {
    let clean_t = if timestamp_secs.is_finite() && timestamp_secs > 0.0 {
        timestamp_secs
    } else {
        0.0
    };
    let step = if step_secs.is_finite() && step_secs >= 0.1 {
        step_secs
    } else {
        2.0
    };
    let bucket_idx = (clean_t / step).round();
    let quantized_secs = bucket_idx * step;
    let bucket_ms = (quantized_secs * 1000.0).round() as u64;
    (quantized_secs, bucket_ms)
}

struct ThumbnailJob {
    key: CacheKey,
    quantized_secs: f64,
    reply_tx: async_channel::Sender<Option<ThumbnailFrame>>,
}

enum WorkerCmd {
    Decode(ThumbnailJob),
    Suspend,
}

struct WorkerState {
    latest_job: Option<ThumbnailJob>,
    suspend_requested: bool,
    shutdown: bool,
}

struct WorkerShared {
    config: ThumbnailerConfig,
    engine_loaded: AtomicBool,
    queue: Mutex<WorkerState>,
    condvar: Condvar,
}

struct LazyWorkerHandle {
    shared: Arc<WorkerShared>,
    join_handle: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for LazyWorkerHandle {
    fn drop(&mut self) {
        if let Ok(mut q) = self.shared.queue.lock() {
            q.latest_job = None;
            q.suspend_requested = false;
            q.shutdown = true;
        }
        self.shared.condvar.notify_all();
        if let Ok(mut jh) = self.join_handle.lock() {
            if let Some(handle) = jh.take() {
                let _ = handle.join();
            }
        }
    }
}

struct MpvThumbnailerInner {
    config: ThumbnailerConfig,
    cache: Arc<Mutex<ThumbnailLruCache>>,
    worker: OnceLock<Result<LazyWorkerHandle, String>>,
}

#[derive(Clone)]
pub struct MpvThumbnailer {
    inner: Arc<MpvThumbnailerInner>,
}

impl Default for MpvThumbnailer {
    fn default() -> Self {
        Self::new()
    }
}

impl MpvThumbnailer {
    pub fn new() -> Self {
        Self::with_config(ThumbnailerConfig::default())
    }

    pub fn with_config(config: ThumbnailerConfig) -> Self {
        let clean_cfg = ThumbnailerConfig {
            width: config.width.clamp(32, 1920),
            height: config.height.clamp(18, 1080),
            bucket_step_secs: if config.bucket_step_secs.is_finite() && config.bucket_step_secs >= 0.1
            {
                config.bucket_step_secs
            } else {
                2.0
            },
            max_cache_entries: config.max_cache_entries.clamp(4, 1024),
        };
        Self {
            inner: Arc::new(MpvThumbnailerInner {
                config: clean_cfg,
                cache: Arc::new(Mutex::new(ThumbnailLruCache::new(
                    clean_cfg.max_cache_entries,
                ))),
                worker: OnceLock::new(),
            }),
        }
    }

    #[inline]
    pub fn quantize_bucket_ms(timestamp_secs: f64) -> i64 {
        let (_, bucket_ms) = quantize_bucket_ms(timestamp_secs, 2.0);
        bucket_ms as i64
    }

    #[inline]
    pub fn bucket_ms_for(&self, timestamp_secs: f64) -> i64 {
        let (_, bucket_ms) = quantize_bucket_ms(timestamp_secs, self.inner.config.bucket_step_secs);
        bucket_ms as i64
    }

    pub fn get_cached_thumbnail(
        &self,
        url: impl AsRef<str>,
        timestamp_secs: f64,
    ) -> Option<ThumbnailFrame> {
        let clean_url = validate_stream_url(url.as_ref()).ok()?;
        let (_, bucket_ms) = quantize_bucket_ms(timestamp_secs, self.inner.config.bucket_step_secs);
        let key = CacheKey {
            url: clean_url.to_owned(),
            bucket_ms,
        };
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
    }

    pub fn clear_cache(&self) {
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    pub fn clear_and_suspend(&self) {
        self.clear_cache();

        let Some(Ok(worker)) = self.inner.worker.get() else {
            return;
        };

        let mut q = worker
            .shared
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if q.shutdown {
            return;
        }
        q.latest_job = None;
        q.suspend_requested = true;
        worker.shared.condvar.notify_all();

        if worker.shared.engine_loaded.load(Ordering::Acquire) {
            let deadline = Instant::now() + Duration::from_millis(25);
            while q.suspend_requested && !q.shutdown {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                let (next_q, _) = worker
                    .shared
                    .condvar
                    .wait_timeout(q, deadline - now)
                    .unwrap_or_else(|e| e.into_inner());
                q = next_q;
            }
        }
    }

    pub fn is_engine_loaded(&self) -> bool {
        self.inner
            .worker
            .get()
            .and_then(|res| res.as_ref().ok())
            .is_some_and(|w| w.shared.engine_loaded.load(Ordering::Acquire))
    }

    pub async fn request_thumbnail(
        &self,
        url: impl AsRef<str>,
        timestamp_secs: f64,
    ) -> Option<ThumbnailFrame> {
        let clean_url = validate_stream_url(url.as_ref()).ok()?;

        let (quantized_secs, bucket_ms) =
            quantize_bucket_ms(timestamp_secs, self.inner.config.bucket_step_secs);
        let key = CacheKey {
            url: clean_url.to_owned(),
            bucket_ms,
        };

        if let Some(hit) = self
            .inner
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return Some(hit);
        }

        let cache_ref = &self.inner.cache;
        let cfg = self.inner.config;
        let worker_res = self.inner.worker.get_or_init(|| {
            let shared = Arc::new(WorkerShared {
                config: cfg,
                engine_loaded: AtomicBool::new(false),
                queue: Mutex::new(WorkerState {
                    latest_job: None,
                    suspend_requested: false,
                    shutdown: false,
                }),
                condvar: Condvar::new(),
            });
            let shared_for_thread = Arc::clone(&shared);
            let outer_cache = Arc::clone(cache_ref);
            let join = thread::Builder::new()
                .name("ferrite-mpv-thumbnailer".into())
                .spawn(move || run_thumbnail_worker(shared_for_thread, outer_cache))
                .map_err(|e| format!("failed to spawn thumbnail worker: {e}"))?;
            Ok(LazyWorkerHandle {
                shared,
                join_handle: Mutex::new(Some(join)),
            })
        });

        let Ok(worker) = worker_res.as_ref() else {
            return None;
        };

        let (reply_tx, reply_rx) = async_channel::bounded(1);
        {
            let mut q = worker
                .shared
                .queue
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if q.shutdown {
                return None;
            }
            q.latest_job = Some(ThumbnailJob {
                key,
                quantized_secs,
                reply_tx,
            });
        }
        worker.shared.condvar.notify_one();

        reply_rx.recv().await.ok().flatten()
    }
}

unsafe extern "C" fn on_sw_render_update(cb_ctx: *mut c_void) {
    if !cb_ctx.is_null() {
        let flag = &*(cb_ctx as *const AtomicBool);
        flag.store(true, Ordering::Release);
    }
}

struct SwMpvEngine {
    mpv: *mut mpv_handle,
    render_ctx: *mut mpv_render_context,
    frame_ready: Arc<AtomicBool>,
    cb_raw: *const AtomicBool,
    next_cmd_id: u64,
    loaded_url: Option<String>,
    scratch: Vec<Aligned64>,
}

impl SwMpvEngine {
    unsafe fn init() -> Result<Self, MpvError> {
        mpv_ffi::ensure_mpv_loaded().map_err(MpvError::Init)?;
        mpv_ffi::set_c_numeric_locale();

        let mpv = mpv_create();
        if mpv.is_null() {
            return Err(MpvError::Init("thumbnailer mpv_create returned NULL".into()));
        }

        let opts: &[(&str, &str)] = &[
            ("vo", "libmpv"),
            ("ao", "null"),
            ("aid", "no"),
            ("sid", "no"),
            ("sub", "no"),
            ("pause", "yes"),
            ("keep-open", "yes"),
            ("ytdl", "no"),
            ("hr-seek", "default"),
            ("vd-lavc-skiploopfilter", "all"),
            ("vd-lavc-fast", "yes"),
            ("hwdec", "no"),
            ("demuxer-max-bytes", "16MiB"),
            ("demuxer-max-back-bytes", "4MiB"),
            ("terminal", "no"),
        ];

        for &(k, v) in opts {
            if let (Ok(ck), Ok(cv)) = (CString::new(k), CString::new(v)) {
                let _ = mpv_set_option_string(mpv, ck.as_ptr(), cv.as_ptr());
            }
        }

        let rc = mpv_initialize(mpv);
        if rc < 0 {
            let msg = error_to_string(rc);
            mpv_terminate_destroy(mpv);
            return Err(MpvError::Init(format!(
                "thumbnailer mpv_initialize failed: {msg}"
            )));
        }

        let mut create_params = [
            mpv_render_param {
                type_: MPV_RENDER_PARAM_API_TYPE,
                data: MPV_RENDER_API_TYPE_SW.as_ptr() as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];

        let mut render_ctx: *mut mpv_render_context = ptr::null_mut();
        let rrc = mpv_render_context_create(&mut render_ctx, mpv, create_params.as_mut_ptr());
        if rrc < 0 || render_ctx.is_null() {
            let msg = error_to_string(rrc);
            mpv_terminate_destroy(mpv);
            return Err(MpvError::Render(format!(
                "thumbnailer mpv_render_context_create(SW) failed: {msg}"
            )));
        }

        let frame_ready = Arc::new(AtomicBool::new(false));
        let cb_raw = Arc::into_raw(Arc::clone(&frame_ready));
        mpv_render_context_set_update_callback(
            render_ctx,
            Some(on_sw_render_update),
            cb_raw as *mut c_void,
        );

        Ok(Self {
            mpv,
            render_ctx,
            frame_ready,
            cb_raw,
            next_cmd_id: 1,
            loaded_url: None,
            scratch: Vec::new(),
        })
    }

    #[inline]
    fn is_aborted(shared: &WorkerShared) -> bool {
        shared
            .queue
            .lock()
            .map(|q| q.shutdown || q.suspend_requested)
            .unwrap_or(true)
    }

    unsafe fn drain_events(&self) {
        loop {
            let ev = mpv_wait_event(self.mpv, 0.0);
            if ev.is_null() || (*ev).event_id == MPV_EVENT_NONE {
                break;
            }
        }
    }

    unsafe fn clear_render_update_flags(&self) {
        self.frame_ready.store(false, Ordering::SeqCst);
        let _ = mpv_render_context_update(self.render_ctx);
        self.frame_ready.store(false, Ordering::SeqCst);
    }

    unsafe fn send_cmd(&mut self, args: &[&str]) -> Option<u64> {
        let mut c_args = Vec::with_capacity(args.len());
        for &a in args {
            let cs = CString::new(a).ok()?;
            c_args.push(cs);
        }
        let mut ptrs: Vec<*const c_char> = c_args.iter().map(|s| s.as_ptr()).collect();
        ptrs.push(ptr::null());
        let cmd_id = self.next_cmd_id;
        self.next_cmd_id = self.next_cmd_id.wrapping_add(1).max(1);
        if mpv_command_async(self.mpv, cmd_id, ptrs.as_ptr()) >= 0 {
            Some(cmd_id)
        } else {
            None
        }
    }

    unsafe fn wait_post_restart_frame(
        &mut self,
        shared: &WorkerShared,
        fallback_timeout: Duration,
    ) -> Option<()> {
        let post_deadline = Instant::now() + fallback_timeout;
        loop {
            if Self::is_aborted(shared) {
                return None;
            }
            self.frame_ready.store(false, Ordering::Release);
            let flags = mpv_render_context_update(self.render_ctx);
            if (flags & MPV_RENDER_UPDATE_FRAME) != 0 || Instant::now() >= post_deadline {
                return Some(());
            }

            let ev_ptr = mpv_wait_event(self.mpv, 0.004);
            if !ev_ptr.is_null() {
                let ev = *ev_ptr;
                if ev.event_id == MPV_EVENT_SHUTDOWN {
                    self.loaded_url = None;
                    return None;
                }
                if ev.event_id == MPV_EVENT_END_FILE && !ev.data.is_null() {
                    let end = *(ev.data as *const mpv_event_end_file);
                    if end.reason == MPV_END_FILE_REASON_ERROR || end.error < 0 {
                        self.loaded_url = None;
                        return None;
                    }
                }
            }
        }
    }

    unsafe fn seek_and_wait_frame(
        &mut self,
        timestamp_secs: f64,
        same_url: bool,
        shared: &WorkerShared,
    ) -> bool {
        self.drain_events();
        self.clear_render_update_flags();

        let t_str = format!("{timestamp_secs:.3}");
        let Some(mut seek_cmd_id) = self.send_cmd(&["seek", &t_str, "absolute", "keyframes"])
        else {
            return false;
        };

        let seek_deadline = Instant::now() + Duration::from_millis(8000);
        let mut tried_exact_fallback = false;
        let mut seek_reply_ok = false;
        let mut seek_restarted = false;

        while Instant::now() < seek_deadline {
            if Self::is_aborted(shared) {
                return false;
            }

            loop {
                let ev_ptr = mpv_wait_event(self.mpv, 0.005);
                if ev_ptr.is_null() {
                    break;
                }
                let ev = *ev_ptr;
                match ev.event_id {
                    MPV_EVENT_NONE => break,
                    MPV_EVENT_SHUTDOWN => {
                        self.loaded_url = None;
                        return false;
                    }
                    MPV_EVENT_END_FILE if !ev.data.is_null() => {
                        let end = *(ev.data as *const mpv_event_end_file);
                        if end.reason == MPV_END_FILE_REASON_ERROR || end.error < 0 {
                            self.loaded_url = None;
                            return false;
                        }
                    }
                    MPV_EVENT_COMMAND_REPLY if ev.reply_userdata == seek_cmd_id => {
                        if ev.error >= 0 {
                            seek_reply_ok = true;
                        } else if !tried_exact_fallback {
                            tried_exact_fallback = true;
                            seek_reply_ok = false;
                            seek_restarted = false;
                            self.clear_render_update_flags();
                            let Some(fallback_id) =
                                self.send_cmd(&["seek", &t_str, "absolute"])
                            else {
                                return false;
                            };
                            seek_cmd_id = fallback_id;
                        } else if !same_url {
                            return self
                                .wait_post_restart_frame(shared, Duration::from_millis(500))
                                .is_some();
                        } else {
                            return false;
                        }
                    }
                    MPV_EVENT_SEEK if !seek_restarted => {
                        self.clear_render_update_flags();
                    }
                    MPV_EVENT_PLAYBACK_RESTART if seek_reply_ok => {
                        seek_restarted = true;
                    }
                    _ => {}
                }
            }

            if seek_reply_ok && seek_restarted {
                break;
            }
        }

        if !(seek_reply_ok && seek_restarted) {
            return false;
        }

        self.wait_post_restart_frame(shared, Duration::from_millis(500))
            .is_some()
    }

    unsafe fn extract_frame(
        &mut self,
        url: &str,
        timestamp_secs: f64,
        width: u32,
        height: u32,
        shared: &WorkerShared,
    ) -> Option<ThumbnailFrame> {
        let clean_url = validate_stream_url(url).ok()?;
        let same_url = self.loaded_url.as_deref() == Some(clean_url);

        if !same_url {
            self.loaded_url = None;
            self.drain_events();
            self.clear_render_update_flags();

            let load_cmd_id = self.send_cmd(&["loadfile", clean_url, "replace"])?;
            let load_deadline = Instant::now() + Duration::from_secs(10);
            let mut load_replied = false;
            let mut file_loaded = false;
            let mut initial_restarted = false;

            while Instant::now() < load_deadline {
                if Self::is_aborted(shared) {
                    return None;
                }

                loop {
                    let ev_ptr = mpv_wait_event(self.mpv, 0.005);
                    if ev_ptr.is_null() {
                        break;
                    }
                    let ev = *ev_ptr;
                    match ev.event_id {
                        MPV_EVENT_NONE => break,
                        MPV_EVENT_SHUTDOWN => return None,
                        MPV_EVENT_COMMAND_REPLY if ev.reply_userdata == load_cmd_id => {
                            if ev.error < 0 {
                                return None;
                            }
                            load_replied = true;
                        }
                        MPV_EVENT_FILE_LOADED => {
                            file_loaded = true;
                        }
                        MPV_EVENT_PLAYBACK_RESTART => {
                            initial_restarted = true;
                        }
                        MPV_EVENT_END_FILE if !ev.data.is_null() => {
                            let end = *(ev.data as *const mpv_event_end_file);
                            if end.reason == MPV_END_FILE_REASON_ERROR || end.error < 0 {
                                return None;
                            }
                        }
                        _ => {}
                    }
                }

                if load_replied && file_loaded && initial_restarted {
                    break;
                }
            }

            if !(load_replied && file_loaded && initial_restarted) {
                return None;
            }

            self.loaded_url = Some(clean_url.to_owned());

            if timestamp_secs <= 0.5 {
                self.wait_post_restart_frame(shared, Duration::from_millis(500))?;
            }
        }

        if (same_url || timestamp_secs > 0.5)
            && !self.seek_and_wait_frame(timestamp_secs, same_url, shared)
        {
            return None;
        }

        let _ = mpv_render_context_update(self.render_ctx);

        let w = width as usize;
        let h = height as usize;
        let stride: usize = ((w * 4) + 63) & !63;
        let total_aligned_bytes = stride * h;
        let blocks = total_aligned_bytes.div_ceil(64);
        if self.scratch.len() < blocks {
            self.scratch.resize(blocks, Aligned64([0u8; 64]));
        }

        let mut sw_size: [c_int; 2] = [width as c_int, height as c_int];
        let mut sw_stride: usize = stride;
        let scratch_ptr = self.scratch.as_mut_ptr() as *mut u8;

        let mut render_params = [
            mpv_render_param {
                type_: MPV_RENDER_PARAM_SW_SIZE,
                data: sw_size.as_mut_ptr() as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_SW_FORMAT,
                data: SW_FORMAT_BGR0.as_ptr() as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_SW_STRIDE,
                data: &mut sw_stride as *mut usize as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_SW_POINTER,
                data: scratch_ptr as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];

        let rc = mpv_render_context_render(self.render_ctx, render_params.as_mut_ptr());
        if rc < 0 {
            tracing::debug!(
                target: "gpui_mpv::thumbnails",
                "SW thumbnail render failed: {}",
                error_to_string(rc)
            );
            return None;
        }

        let row_bytes = w * 4;
        let mut bgra = vec![0u8; row_bytes * h];
        let src_slice = core::slice::from_raw_parts(scratch_ptr, total_aligned_bytes);
        for y in 0..h {
            let src_row = &src_slice[y * stride..y * stride + row_bytes];
            let dst_row = &mut bgra[y * row_bytes..(y + 1) * row_bytes];
            dst_row.copy_from_slice(src_row);
            for px in dst_row.as_chunks_mut::<4>().0 {
                px[3] = 0xFF;
            }
        }

        let img_buf = RgbaImage::from_raw(width, height, bgra.clone())?;
        let render_image = Arc::new(RenderImage::new([Frame::new(img_buf)]));
        let arc_bytes = Arc::new(bgra);

        Some(ThumbnailFrame {
            width,
            height,
            bgra_or_rgba_bytes: Arc::clone(&arc_bytes),
            bgra_bytes: arc_bytes,
            render_image,
            timestamp_secs,
        })
    }
}

impl Drop for SwMpvEngine {
    fn drop(&mut self) {
        unsafe {
            mpv_render_context_set_update_callback(self.render_ctx, None, ptr::null_mut());
            mpv_render_context_free(self.render_ctx);
            let _ = Arc::from_raw(self.cb_raw);
            mpv_terminate_destroy(self.mpv);
        }
    }
}

fn run_thumbnail_worker(
    shared: Arc<WorkerShared>,
    outer_cache: Arc<Mutex<ThumbnailLruCache>>,
) {
    let mut engine: Option<SwMpvEngine> = None;
    let mut failed_buckets: HashSet<CacheKey> = HashSet::new();

    loop {
        let cmd = {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            while !q.shutdown && !q.suspend_requested && q.latest_job.is_none() {
                q = shared.condvar.wait(q).unwrap_or_else(|e| e.into_inner());
            }
            if q.shutdown {
                break;
            }
            if q.suspend_requested {
                Some(WorkerCmd::Suspend)
            } else {
                q.latest_job.take().map(WorkerCmd::Decode)
            }
        };

        match cmd {
            None => continue,
            Some(WorkerCmd::Suspend) => {
                drop(engine.take());
                shared.engine_loaded.store(false, Ordering::Release);
                failed_buckets.clear();
                failed_buckets.shrink_to_fit();
                outer_cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clear();
                crate::trim_process_heap();

                let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
                q.suspend_requested = false;
                shared.condvar.notify_all();
                continue;
            }
            Some(WorkerCmd::Decode(job)) => {
                if let Some(cached) = outer_cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&job.key)
                {
                    let _ = job.reply_tx.send_blocking(Some(cached));
                    continue;
                }

                if failed_buckets.contains(&job.key) {
                    let _ = job.reply_tx.send_blocking(None);
                    continue;
                }

                if engine.is_none() {
                    match unsafe { SwMpvEngine::init() } {
                        Ok(eng) => {
                            engine = Some(eng);
                            shared.engine_loaded.store(true, Ordering::Release);
                        }
                        Err(err) => {
                            tracing::warn!(
                                target: "gpui_mpv::thumbnails",
                                "failed to initialize secondary SW thumbnailer: {err}"
                            );
                            let _ = job.reply_tx.send_blocking(None);
                            continue;
                        }
                    }
                }

                let Some(eng) = engine.as_mut() else {
                    continue;
                };
                let frame_opt = unsafe {
                    eng.extract_frame(
                        &job.key.url,
                        job.quantized_secs,
                        shared.config.width,
                        shared.config.height,
                        &shared,
                    )
                };

                let was_suspended = shared
                    .queue
                    .lock()
                    .map(|q| q.suspend_requested || q.shutdown)
                    .unwrap_or(true);

                if !was_suspended {
                    if let Some(ref frame) = frame_opt {
                        outer_cache
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(job.key, frame.clone());
                    } else if failed_buckets.len() < 128 {
                        failed_buckets.insert(job.key);
                    }
                }

                let _ = job.reply_tx.send_blocking(frame_opt);
            }
        }
    }

    drop(engine);
    shared.engine_loaded.store(false, Ordering::Release);
    crate::trim_process_heap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_quantization_and_lru_eviction_work() {
        let (q1, b1) = quantize_bucket_ms(11.1, 2.0);
        let (q2, b2) = quantize_bucket_ms(12.8, 2.0);
        assert_eq!((q1, b1), (12.0, 12_000));
        assert_eq!((q2, b2), (12.0, 12_000));

        let mut cache = ThumbnailLruCache::new(2);
        let dummy = |ts: f64| {
            let bytes = Arc::new(vec![0u8; 16]);
            let img = RgbaImage::from_raw(2, 2, (*bytes).clone()).unwrap();
            ThumbnailFrame {
                width: 2,
                height: 2,
                bgra_or_rgba_bytes: Arc::clone(&bytes),
                bgra_bytes: bytes,
                render_image: Arc::new(RenderImage::new([Frame::new(img)])),
                timestamp_secs: ts,
            }
        };

        let k1 = CacheKey {
            url: "https://example.com/v.mkv".into(),
            bucket_ms: 0,
        };
        let k2 = CacheKey {
            url: "https://example.com/v.mkv".into(),
            bucket_ms: 2000,
        };
        let k3 = CacheKey {
            url: "https://example.com/v.mkv".into(),
            bucket_ms: 4000,
        };

        cache.insert(k1.clone(), dummy(0.0));
        cache.insert(k2.clone(), dummy(2.0));
        assert!(cache.get(&k1).is_some());
        cache.insert(k3.clone(), dummy(4.0));
        assert!(cache.get(&k1).is_some());
        assert!(cache.get(&k2).is_none());
        assert!(cache.get(&k3).is_some());
    }

    #[test]
    fn sw_thumbnailer_extracts_accurate_bgra_frame_and_caches_bucket() {
        let thumb = MpvThumbnailer::with_config(ThumbnailerConfig {
            width: 160,
            height: 90,
            bucket_step_secs: 2.0,
            max_cache_entries: 16,
        });

        assert!(
            futures_lite_block_on(thumb.request_thumbnail("file:///etc/passwd", 1.0)).is_none()
        );
        assert!(
            futures_lite_block_on(thumb.request_thumbnail("edl://!new_stream;file:///etc/passwd", 1.0)).is_none()
        );
        assert!(!thumb.is_engine_loaded());

        let url = "av://lavfi:color=c=red:s=320x180:d=4:r=15";
        let frame = futures_lite_block_on(thumb.request_thumbnail(url, 1.2))
            .expect("expected SW thumbnail frame from synthetic red video");

        assert_eq!((frame.width, frame.height), (160, 90));
        assert_eq!(frame.bgra_bytes.len(), 160 * 90 * 4);
        assert!(Arc::ptr_eq(&frame.bgra_bytes, &frame.bgra_or_rgba_bytes));

        let mid_idx = ((45 * 160) + 80) * 4;
        let px = &frame.bgra_bytes[mid_idx..mid_idx + 4];
        assert!(
            px[0] < 40 && px[1] < 40 && px[2] > 200 && px[3] == 255,
            "expected BGRA red pixel [~0, ~0, ~255, 255], got {px:?}"
        );

        let cached = thumb
            .get_cached_thumbnail(url, 1.8)
            .expect("expected synchronous LRU cache hit");
        assert!(Arc::ptr_eq(&frame.render_image, &cached.render_image));
    }

    #[test]
    fn clear_and_suspend_drops_cache_and_mpv_engine_then_lazily_recovers() {
        assert_eq!(ThumbnailerConfig::default().max_cache_entries, 40);
        assert_eq!(DEFAULT_MAX_CACHE_ENTRIES, 40);

        let thumb = MpvThumbnailer::new();
        assert!(
            !thumb.is_engine_loaded(),
            "thumbnailer must not allocate libmpv before first request"
        );
        thumb.clear_and_suspend();
        assert!(!thumb.is_engine_loaded());

        let url = "av://lavfi:color=c=blue:s=320x180:d=4:r=15";
        let f1 = futures_lite_block_on(thumb.request_thumbnail(url, 0.4))
            .expect("expected first thumbnail decode");
        assert!(thumb.is_engine_loaded(), "SW mpv engine should be active");
        assert!(thumb.get_cached_thumbnail(url, 0.2).is_some());
        assert_eq!((f1.width, f1.height), (240, 135));

        thumb.clear_and_suspend();
        let wait_deadline = Instant::now() + Duration::from_secs(10);
        while thumb.is_engine_loaded() && Instant::now() < wait_deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            !thumb.is_engine_loaded(),
            "SW mpv engine must be destroyed on clear_and_suspend()"
        );
        assert!(
            thumb.get_cached_thumbnail(url, 0.2).is_none(),
            "LRU cache must be empty after clear_and_suspend()"
        );

        let f2 = futures_lite_block_on(thumb.request_thumbnail(url, 2.1))
            .expect("expected lazy recovery thumbnail decode after suspend");
        assert!(thumb.is_engine_loaded());
        let mid_idx = ((67 * 240) + 120) * 4;
        let px = &f2.bgra_bytes[mid_idx..mid_idx + 4];
        assert!(
            px[0] > 200 && px[1] < 40 && px[2] < 40 && px[3] == 255,
            "expected BGRA blue pixel [~255, ~0, ~0, 255], got {px:?}"
        );

        thumb.clear_and_suspend();
        let wait_deadline2 = Instant::now() + Duration::from_secs(10);
        while thumb.is_engine_loaded() && Instant::now() < wait_deadline2 {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!thumb.is_engine_loaded());
    }

    #[test]
    fn sw_thumbnailer_seeks_distinct_frames_across_timeline() {
        struct TempVideoGuard(Option<std::path::PathBuf>);
        impl Drop for TempVideoGuard {
            fn drop(&mut self) {
                if let Some(ref p) = self.0 {
                    let _ = std::fs::remove_file(p);
                }
            }
        }

        assert_eq!(MpvThumbnailer::quantize_bucket_ms(0.2), 0);
        assert_eq!(MpvThumbnailer::quantize_bucket_ms(2.2), 2_000);
        assert_eq!(MpvThumbnailer::quantize_bucket_ms(4.2), 4_000);

        let tmp_path = std::env::temp_dir().join(format!(
            "ferrite_mpv_thumb_seek_{}_{}.avi",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));

        let ffmpeg_ok = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=160x90:d=2:r=5",
                "-f",
                "lavfi",
                "-i",
                "color=c=lime:s=160x90:d=2:r=5",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=160x90:d=2:r=5",
                "-filter_complex",
                "[0:v][1:v][2:v]concat=n=3:v=1:a=0",
                "-c:v",
                "mpeg4",
                "-q:v",
                "2",
                "-g",
                "5",
            ])
            .arg(&tmp_path)
            .status()
            .is_ok_and(|st| st.success());

        let _guard = TempVideoGuard(ffmpeg_ok.then(|| tmp_path.clone()));
        let url = if ffmpeg_ok {
            tmp_path.to_string_lossy().into_owned()
        } else {
            "av://lavfi:testsrc=duration=6:size=160x90:rate=5".to_owned()
        };

        let thumb = MpvThumbnailer::with_config(ThumbnailerConfig {
            width: 160,
            height: 90,
            bucket_step_secs: 2.0,
            max_cache_entries: 16,
        });

        let frame_0 = futures_lite_block_on(thumb.request_thumbnail(&url, 0.2))
            .expect("expected thumbnail at t=0.2s");
        let frame_2 = futures_lite_block_on(thumb.request_thumbnail(&url, 2.2))
            .expect("expected thumbnail at t=2.2s");
        let frame_4 = futures_lite_block_on(thumb.request_thumbnail(&url, 4.2))
            .expect("expected thumbnail at t=4.2s");

        assert_ne!(
            frame_0.bgra_bytes.as_slice(),
            frame_2.bgra_bytes.as_slice(),
            "t=0.2s and t=2.2s must decode distinct frames"
        );
        assert_ne!(
            frame_2.bgra_bytes.as_slice(),
            frame_4.bgra_bytes.as_slice(),
            "t=2.2s and t=4.2s must decode distinct frames"
        );
        assert_ne!(
            frame_0.bgra_bytes.as_slice(),
            frame_4.bgra_bytes.as_slice(),
            "t=0.2s and t=4.2s must decode distinct frames"
        );

        if ffmpeg_ok {
            let mid = ((45 * 160) + 80) * 4;
            let px0 = &frame_0.bgra_bytes[mid..mid + 4];
            let px2 = &frame_2.bgra_bytes[mid..mid + 4];
            let px4 = &frame_4.bgra_bytes[mid..mid + 4];
            assert!(
                px0[2] > 180 && px0[1] < 60 && px0[0] < 60,
                "expected red segment at t=0.2s, got BGRA {px0:?}"
            );
            assert!(
                px2[1] > 150 && px2[2] < 80 && px2[0] < 80,
                "expected green segment at t=2.2s, got BGRA {px2:?}"
            );
            assert!(
                px4[0] > 180 && px4[1] < 60 && px4[2] < 60,
                "expected blue segment at t=4.2s, got BGRA {px4:?}"
            );
        }

        thumb.clear_and_suspend();

        let direct_4 = futures_lite_block_on(thumb.request_thumbnail(&url, 4.2))
            .expect("expected cold-start thumbnail at t=4.2s");
        let back_0 = futures_lite_block_on(thumb.request_thumbnail(&url, 0.2))
            .expect("expected backward-seek thumbnail at t=0.2s");

        assert_ne!(
            direct_4.bgra_bytes.as_slice(),
            back_0.bgra_bytes.as_slice(),
            "cold-start at t=4.2s and backward seek to t=0.2s must produce distinct frames"
        );
        if ffmpeg_ok {
            let mid = ((45 * 160) + 80) * 4;
            let px_direct_4 = &direct_4.bgra_bytes[mid..mid + 4];
            let px_back_0 = &back_0.bgra_bytes[mid..mid + 4];
            assert!(
                px_direct_4[0] > 180 && px_direct_4[2] < 60,
                "expected blue segment on cold-start at t=4.2s, got BGRA {px_direct_4:?}"
            );
            assert!(
                px_back_0[2] > 180 && px_back_0[0] < 60,
                "expected red segment on backward seek to t=0.2s, got BGRA {px_back_0:?}"
            );
        }

        thumb.clear_and_suspend();
    }

    fn futures_lite_block_on<F: core::future::Future>(mut fut: F) -> F::Output {
        use core::pin::Pin;
        use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

        fn noop_raw_waker() -> RawWaker {
            fn clone(_: *const ()) -> RawWaker {
                noop_raw_waker()
            }
            fn noop(_: *const ()) {}
            static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
            RawWaker::new(ptr::null(), &VTABLE)
        }

        let waker = unsafe { Waker::from_raw(noop_raw_waker()) };
        let mut cx = Context::from_waker(&waker);
        let mut pinned = unsafe { Pin::new_unchecked(&mut fut) };
        loop {
            if let Poll::Ready(out) = pinned.as_mut().poll(&mut cx) {
                return out;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
