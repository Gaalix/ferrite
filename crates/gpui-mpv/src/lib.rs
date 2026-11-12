pub mod dcomp;
pub mod element;
pub(crate) mod mpv_ffi;
pub mod player;
pub mod shaders;
#[cfg(target_os = "linux")]
pub(crate) mod subsurface;
pub mod thumbnails;

#[cfg(target_os = "windows")]
pub use dcomp::WindowsDCompUnderlay;
pub use dcomp::{
    compute_underlay_placement, DCompBackendMode, DCompDesiredState, DCompHandleSnapshot,
    DCompStateTracker, DCompSyncDiff, DCompUnderlayRect, UnderlayPlacement,
};
pub use element::{mpv_viewport, MpvViewport};
pub use player::{
    drain_evicted_video_images, shorten_codec_label, validate_stream_url, MediaTrack,
    MpvController, PlayerEvent, PlayerSnapshot, ToneMappingPreset, TrackType, UpscalePreset,
};
pub use shaders::{
    default_shader_cache_dir, materialize_shader_preset_in_dir, DebandConfig, HdrBackendOptions,
    ShaderPreset,
};
pub use thumbnails::{
    MpvThumbnailer, ThumbnailFrame, ThumbnailerConfig, DEFAULT_MAX_CACHE_ENTRIES,
};

#[derive(Debug, thiserror::Error)]
pub enum MpvError {
    #[error("libmpv initialization error: {0}")]
    Init(String),

    #[error("Wayland subsurface error: {0}")]
    Wayland(String),

    #[error("EGL context error: {0}")]
    Egl(String),

    #[error("libmpv render context error: {0}")]
    Render(String),

    #[error("libmpv command error: {0}")]
    Command(String),

    #[error("libmpv property error: {0}")]
    Property(String),
}

pub type Result<T> = core::result::Result<T, MpvError>;

#[inline]
pub fn trim_process_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> core::ffi::c_int;
        }
        let _ = malloc_trim(0);
    }
}
