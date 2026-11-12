//! OS media transport controls (Linux MPRIS2 D-Bus / Windows SMTC).

use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc,
};
use std::time::Instant;

/// Minimum discontinuity (in seconds) between expected and reported playback position
/// that triggers a seek notification and metadata/timeline sync.
pub const SEEK_JUMP_THRESHOLD_SECS: f64 = 2.5;

/// Media transport commands received from the OS (MPRIS2 / SMTC / media keys).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OsMediaCommand {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    SeekRelativeSecs(f64),
    SetPositionSecs(f64),
}

/// Playback state reported to OS media controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OsPlaybackStatus {
    #[default]
    Stopped,
    Playing,
    Paused,
}

/// Snapshot of currently active media metadata and transport capabilities.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OsMediaMetadata {
    pub status: OsPlaybackStatus,
    pub title: String,
    pub subtitle: String,
    pub poster_url: Option<String>,
    pub duration_secs: f64,
    pub position_secs: f64,
    pub can_seek: bool,
    pub can_go_next: bool,
    pub can_go_previous: bool,
}

/// Converts seconds to non-negative microseconds clamped to `i64::MAX`.
pub fn secs_to_micros(secs: f64) -> i64 {
    if !secs.is_finite() || secs <= 0.0 {
        return 0;
    }
    let micros = (secs * 1_000_000.0).round();
    if micros >= i64::MAX as f64 {
        i64::MAX
    } else {
        micros as i64
    }
}

/// Returns `true` when `next.position_secs` jumped beyond [`SEEK_JUMP_THRESHOLD_SECS`]
/// relative to the expected playback position after `elapsed_secs`.
pub fn is_seek_jump(prev: &OsMediaMetadata, next: &OsMediaMetadata, elapsed_secs: f64) -> bool {
    if !prev.position_secs.is_finite() || !next.position_secs.is_finite() {
        return false;
    }
    let expected =
        if prev.status == OsPlaybackStatus::Playing && next.status == OsPlaybackStatus::Playing {
            prev.position_secs + elapsed_secs.max(0.0)
        } else {
            prev.position_secs
        };
    (next.position_secs - expected).abs() > SEEK_JUMP_THRESHOLD_SECS
}

fn duration_changed(prev_secs: f64, next_secs: f64) -> bool {
    if prev_secs.is_finite() != next_secs.is_finite() {
        return true;
    }
    if !prev_secs.is_finite() && !next_secs.is_finite() {
        return false;
    }
    (prev_secs - next_secs).abs() > 0.25
}

/// Returns `true` if a state update should be pushed to the OS media backend.
///
/// Smooth 60Hz position progression during playback returns `false` unless
/// status, metadata, duration, capabilities, or a seek jump > 2.5s occurred.
pub fn should_sync_media_update(
    prev: &OsMediaMetadata,
    next: &OsMediaMetadata,
    elapsed_secs: f64,
) -> bool {
    prev.status != next.status
        || prev.title != next.title
        || prev.subtitle != next.subtitle
        || prev.poster_url != next.poster_url
        || duration_changed(prev.duration_secs, next.duration_secs)
        || prev.can_seek != next.can_seek
        || prev.can_go_next != next.can_go_next
        || prev.can_go_previous != next.can_go_previous
        || is_seek_jump(prev, next, elapsed_secs)
}

/// Controls OS-level media session integration and emits inbound [`OsMediaCommand`]s.
pub struct OsMediaController {
    enabled: bool,
    last_synced: Option<OsMediaMetadata>,
    last_synced_at: Option<Instant>,
    latest_metadata: Option<OsMediaMetadata>,
    latest_position_us: Arc<AtomicI64>,
    backend: MediaBackend,
}

impl OsMediaController {
    /// Starts the OS media integration backend and returns `(controller, command_receiver)`.
    pub fn start(
        tokio_handle: tokio::runtime::Handle,
        windows_hwnd: Option<isize>,
    ) -> (Self, async_channel::Receiver<OsMediaCommand>) {
        let (cmd_tx, cmd_rx) = async_channel::unbounded();
        let latest_position_us = Arc::new(AtomicI64::new(0));
        let backend = MediaBackend::start(
            tokio_handle,
            windows_hwnd,
            cmd_tx,
            Arc::clone(&latest_position_us),
        );
        (
            Self {
                enabled: true,
                last_synced: None,
                last_synced_at: None,
                latest_metadata: None,
                latest_position_us,
                backend,
            },
            cmd_rx,
        )
    }

    /// Enables or disables OS media session publishing.
    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        self.backend.set_enabled(enabled);
        if enabled {
            if let Some(meta) = self.latest_metadata.clone() {
                self.last_synced = Some(meta.clone());
                self.last_synced_at = Some(Instant::now());
                self.backend.sync_state(meta, false);
            }
        } else {
            self.last_synced = None;
            self.last_synced_at = None;
        }
    }

    /// Synchronizes playback metadata and transport status with diff-based throttling.
    pub fn sync_state(&mut self, next: OsMediaMetadata) {
        self.latest_position_us
            .store(secs_to_micros(next.position_secs), Ordering::Relaxed);
        self.latest_metadata = Some(next.clone());

        if !self.enabled {
            return;
        }

        let now = Instant::now();
        let (should_push, seek_jumped) = match (&self.last_synced, self.last_synced_at) {
            (Some(prev), Some(prev_at)) => {
                let elapsed = now.saturating_duration_since(prev_at).as_secs_f64();
                let seek_jumped = is_seek_jump(prev, &next, elapsed);
                let changed = should_sync_media_update(prev, &next, elapsed);
                (changed, seek_jumped)
            }
            _ => (true, false),
        };

        if should_push {
            self.last_synced = Some(next.clone());
            self.last_synced_at = Some(now);
            self.backend.sync_state(next, seek_jumped);
        }
    }

    /// Explicitly notifies the OS media backend that a seek completed at `position_secs`.
    pub fn notify_seeked(&self, position_secs: f64) {
        let pos_us = secs_to_micros(position_secs);
        self.latest_position_us.store(pos_us, Ordering::Relaxed);
        if self.enabled {
            self.backend.notify_seeked(position_secs, pos_us);
        }
    }
}

impl Drop for OsMediaController {
    fn drop(&mut self) {
        self.backend.shutdown();
    }
}

#[cfg(target_os = "linux")]
pub mod linux_mpris {
    use std::collections::HashMap;
    use std::sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    };

    use tokio::sync::mpsc;
    use zbus::fdo::{RequestNameFlags, RequestNameReply};
    use zbus::interface;
    use zbus::object_server::SignalEmitter;
    use zbus::zvariant::{ObjectPath, OwnedValue, Str, Value};

    use super::{secs_to_micros, OsMediaCommand, OsMediaMetadata, OsPlaybackStatus};

    /// D-Bus object path required by the MPRIS2 specification.
    pub const MPRIS_OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
    /// Primary well-known D-Bus name for Ferrite's MPRIS2 service.
    pub const MPRIS_BUS_NAME_PRIMARY: &str = "org.mpris.MediaPlayer2.ferrite";

    const MPRIS_TRACK_CURRENT_PATH: &str = "/org/mpris/MediaPlayer2/ferrite/track/current";
    const MPRIS_TRACK_NONE_PATH: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

    /// Returns the PID-scoped fallback MPRIS2 well-known bus name.
    pub fn mpris_fallback_bus_name(pid: u32) -> String {
        format!("org.mpris.MediaPlayer2.ferrite.instance{pid}")
    }

    /// Maps [`OsPlaybackStatus`] to the MPRIS2 `PlaybackStatus` string value.
    pub fn mpris_playback_status(status: OsPlaybackStatus) -> &'static str {
        match status {
            OsPlaybackStatus::Stopped => "Stopped",
            OsPlaybackStatus::Playing => "Playing",
            OsPlaybackStatus::Paused => "Paused",
        }
    }

    /// Formats [`OsMediaMetadata`] into an MPRIS2 `Metadata` `a{sv}` dictionary.
    pub fn build_mpris_metadata_map(meta: &OsMediaMetadata) -> HashMap<String, OwnedValue> {
        let mut map = HashMap::new();

        let track_path = if meta.status == OsPlaybackStatus::Stopped && meta.title.is_empty() {
            MPRIS_TRACK_NONE_PATH
        } else {
            MPRIS_TRACK_CURRENT_PATH
        };
        if let Ok(path) = ObjectPath::try_from(track_path) {
            map.insert("mpris:trackid".to_owned(), OwnedValue::from(path));
        }

        if meta.duration_secs.is_finite() && meta.duration_secs > 0.0 {
            let length_us = secs_to_micros(meta.duration_secs);
            map.insert("mpris:length".to_owned(), OwnedValue::from(length_us));
        }

        if !meta.title.is_empty() {
            map.insert(
                "xesam:title".to_owned(),
                OwnedValue::from(Str::from(meta.title.clone())),
            );
        }

        if !meta.subtitle.is_empty() {
            if let Ok(artists) = OwnedValue::try_from(Value::from(vec![meta.subtitle.clone()])) {
                map.insert("xesam:artist".to_owned(), artists);
            }
        }

        if let Some(url) = meta.poster_url.as_deref().filter(|u| !u.is_empty()) {
            map.insert(
                "mpris:artUrl".to_owned(),
                OwnedValue::from(Str::from(url.to_owned())),
            );
        }

        map
    }

    pub(super) enum MprisMsg {
        SetEnabled(bool),
        Sync {
            meta: OsMediaMetadata,
            emit_seeked: bool,
        },
        NotifySeeked(i64),
        Shutdown,
    }

    pub(super) struct MediaBackend {
        tx: mpsc::UnboundedSender<MprisMsg>,
    }

    impl MediaBackend {
        pub(super) fn start(
            tokio_handle: tokio::runtime::Handle,
            _windows_hwnd: Option<isize>,
            cmd_tx: async_channel::Sender<OsMediaCommand>,
            position_us: Arc<AtomicI64>,
        ) -> Self {
            let (tx, rx) = mpsc::unbounded_channel();
            tokio_handle.spawn(run_mpris_actor(rx, cmd_tx, position_us));
            Self { tx }
        }

        pub(super) fn set_enabled(&self, enabled: bool) {
            let _ = self.tx.send(MprisMsg::SetEnabled(enabled));
        }

        pub(super) fn sync_state(&self, meta: OsMediaMetadata, emit_seeked: bool) {
            let _ = self.tx.send(MprisMsg::Sync { meta, emit_seeked });
        }

        pub(super) fn notify_seeked(&self, _position_secs: f64, pos_us: i64) {
            let _ = self.tx.send(MprisMsg::NotifySeeked(pos_us));
        }

        pub(super) fn shutdown(&mut self) {
            let _ = self.tx.send(MprisMsg::Shutdown);
        }
    }

    struct MprisRoot;

    #[interface(name = "org.mpris.MediaPlayer2")]
    impl MprisRoot {
        async fn raise(&self) {}

        async fn quit(&self) {}

        #[zbus(property)]
        async fn can_quit(&self) -> bool {
            false
        }

        #[zbus(property)]
        async fn can_raise(&self) -> bool {
            false
        }

        #[zbus(property)]
        async fn has_track_list(&self) -> bool {
            false
        }

        #[zbus(property)]
        async fn identity(&self) -> &str {
            "Ferrite"
        }

        #[zbus(property)]
        async fn desktop_entry(&self) -> &str {
            "ferrite"
        }

        #[zbus(property)]
        async fn supported_uri_schemes(&self) -> Vec<String> {
            vec![
                "http".to_owned(),
                "https".to_owned(),
                "file".to_owned(),
                "magnet".to_owned(),
            ]
        }

        #[zbus(property)]
        async fn supported_mime_types(&self) -> Vec<String> {
            vec![
                "video/mp4".to_owned(),
                "video/x-matroska".to_owned(),
                "video/webm".to_owned(),
            ]
        }
    }

    struct MprisPlayer {
        meta: OsMediaMetadata,
        enabled: bool,
        position_us: Arc<AtomicI64>,
        cmd_tx: async_channel::Sender<OsMediaCommand>,
    }

    impl MprisPlayer {
        fn send_cmd(&self, cmd: OsMediaCommand) {
            let _ = self.cmd_tx.try_send(cmd);
        }
    }

    #[interface(name = "org.mpris.MediaPlayer2.Player")]
    impl MprisPlayer {
        async fn next(&self) {
            self.send_cmd(OsMediaCommand::Next);
        }

        async fn previous(&self) {
            self.send_cmd(OsMediaCommand::Previous);
        }

        async fn pause(&self) {
            self.send_cmd(OsMediaCommand::Pause);
        }

        async fn play_pause(&self) {
            self.send_cmd(OsMediaCommand::PlayPause);
        }

        async fn stop(&self) {
            self.send_cmd(OsMediaCommand::Stop);
        }

        async fn play(&self) {
            self.send_cmd(OsMediaCommand::Play);
        }

        async fn seek(&self, offset: i64) {
            self.send_cmd(OsMediaCommand::SeekRelativeSecs(
                offset as f64 / 1_000_000.0,
            ));
        }

        async fn set_position(&self, _track_id: ObjectPath<'_>, position: i64) {
            let clamped = position.max(0) as f64 / 1_000_000.0;
            self.send_cmd(OsMediaCommand::SetPositionSecs(clamped));
        }

        async fn open_uri(&self, _uri: String) {}

        #[zbus(signal)]
        async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

        #[zbus(property)]
        async fn playback_status(&self) -> String {
            mpris_playback_status(self.meta.status).to_owned()
        }

        #[zbus(property)]
        async fn rate(&self) -> f64 {
            1.0
        }

        #[zbus(property)]
        async fn metadata(&self) -> HashMap<String, OwnedValue> {
            build_mpris_metadata_map(&self.meta)
        }

        #[zbus(property)]
        async fn volume(&self) -> f64 {
            1.0
        }

        #[zbus(property(emits_changed_signal = "false"))]
        async fn position(&self) -> i64 {
            self.position_us.load(Ordering::Relaxed)
        }

        #[zbus(property)]
        async fn minimum_rate(&self) -> f64 {
            1.0
        }

        #[zbus(property)]
        async fn maximum_rate(&self) -> f64 {
            1.0
        }

        #[zbus(property)]
        async fn can_go_next(&self) -> bool {
            self.enabled && self.meta.can_go_next
        }

        #[zbus(property)]
        async fn can_go_previous(&self) -> bool {
            self.enabled && self.meta.can_go_previous
        }

        #[zbus(property)]
        async fn can_play(&self) -> bool {
            self.enabled
        }

        #[zbus(property)]
        async fn can_pause(&self) -> bool {
            self.enabled
        }

        #[zbus(property)]
        async fn can_seek(&self) -> bool {
            self.enabled && self.meta.can_seek
        }

        #[zbus(property)]
        async fn can_control(&self) -> bool {
            self.enabled
        }
    }

    struct MprisConnectionState {
        conn: zbus::Connection,
        owned_name: Option<String>,
    }

    async fn acquire_mpris_name(conn: &zbus::Connection) -> Option<String> {
        if let Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) = conn
            .request_name_with_flags(MPRIS_BUS_NAME_PRIMARY, RequestNameFlags::DoNotQueue.into())
            .await
        {
            return Some(MPRIS_BUS_NAME_PRIMARY.to_owned());
        }

        let fallback = mpris_fallback_bus_name(std::process::id());
        if conn.request_name(fallback.as_str()).await.is_ok() {
            return Some(fallback);
        }
        None
    }

    async fn init_mpris_connection(
        cmd_tx: async_channel::Sender<OsMediaCommand>,
        position_us: Arc<AtomicI64>,
    ) -> Option<MprisConnectionState> {
        let builder = match zbus::connection::Builder::session() {
            Ok(b) => b,
            Err(err) => {
                tracing::debug!(
                    target: "ferrite::os_integration",
                    "MPRIS2 disabled: session D-Bus unavailable: {err}"
                );
                return None;
            }
        };

        let player = MprisPlayer {
            meta: OsMediaMetadata::default(),
            enabled: true,
            position_us,
            cmd_tx,
        };

        let conn = match builder
            .serve_at(MPRIS_OBJECT_PATH, MprisRoot)
            .and_then(|b| b.serve_at(MPRIS_OBJECT_PATH, player))
        {
            Ok(b) => match b.build().await {
                Ok(c) => c,
                Err(err) => {
                    tracing::debug!(
                        target: "ferrite::os_integration",
                        "MPRIS2 disabled: failed to connect session bus: {err}"
                    );
                    return None;
                }
            },
            Err(err) => {
                tracing::debug!(
                    target: "ferrite::os_integration",
                    "MPRIS2 disabled: failed to register object server interfaces: {err}"
                );
                return None;
            }
        };

        let owned_name = acquire_mpris_name(&conn).await;
        tracing::debug!(
            target: "ferrite::os_integration",
            bus_name = ?owned_name,
            "MPRIS2 server initialized"
        );
        Some(MprisConnectionState { conn, owned_name })
    }

    async fn run_mpris_actor(
        mut rx: mpsc::UnboundedReceiver<MprisMsg>,
        cmd_tx: async_channel::Sender<OsMediaCommand>,
        position_us: Arc<AtomicI64>,
    ) {
        let mut state: Option<MprisConnectionState> = None;
        let mut init_attempted = false;

        while let Some(msg) = rx.recv().await {
            match msg {
                MprisMsg::Shutdown => {
                    if let Some(mut s) = state.take() {
                        if let Some(name) = s.owned_name.take() {
                            let _ = s.conn.release_name(name).await;
                        }
                    }
                    break;
                }
                MprisMsg::SetEnabled(enabled) => {
                    if !enabled && !init_attempted {
                        continue;
                    }
                    if !init_attempted {
                        init_attempted = true;
                        state =
                            init_mpris_connection(cmd_tx.clone(), Arc::clone(&position_us)).await;
                    }
                    let Some(s) = state.as_mut() else {
                        continue;
                    };
                    if enabled {
                        if s.owned_name.is_none() {
                            s.owned_name = acquire_mpris_name(&s.conn).await;
                        }
                    } else if let Some(name) = s.owned_name.take() {
                        let _ = s.conn.release_name(name).await;
                    }

                    if let Ok(iface_ref) = s
                        .conn
                        .object_server()
                        .interface::<_, MprisPlayer>(MPRIS_OBJECT_PATH)
                        .await
                    {
                        {
                            let mut player = iface_ref.get_mut().await;
                            player.enabled = enabled;
                            if !enabled {
                                player.meta = OsMediaMetadata::default();
                            }
                        }
                        let emitter = iface_ref.signal_emitter();
                        let player = iface_ref.get().await;
                        let _ = player.can_play_changed(emitter).await;
                        let _ = player.can_pause_changed(emitter).await;
                        let _ = player.can_control_changed(emitter).await;
                        let _ = player.playback_status_changed(emitter).await;
                        let _ = player.metadata_changed(emitter).await;
                    }
                }
                MprisMsg::Sync { meta, emit_seeked } => {
                    if !init_attempted {
                        init_attempted = true;
                        state =
                            init_mpris_connection(cmd_tx.clone(), Arc::clone(&position_us)).await;
                    }
                    let Some(s) = state.as_ref() else {
                        continue;
                    };
                    let Ok(iface_ref) = s
                        .conn
                        .object_server()
                        .interface::<_, MprisPlayer>(MPRIS_OBJECT_PATH)
                        .await
                    else {
                        continue;
                    };

                    let (
                        status_changed,
                        metadata_changed,
                        can_seek_changed,
                        can_next_changed,
                        can_prev_changed,
                        seek_pos_us,
                    ) = {
                        let mut player = iface_ref.get_mut().await;
                        let prev = &player.meta;
                        let status_changed = prev.status != meta.status;
                        let metadata_changed = prev.title != meta.title
                            || prev.subtitle != meta.subtitle
                            || prev.poster_url != meta.poster_url
                            || super::duration_changed(prev.duration_secs, meta.duration_secs)
                            || status_changed;
                        let can_seek_changed = prev.can_seek != meta.can_seek;
                        let can_next_changed = prev.can_go_next != meta.can_go_next;
                        let can_prev_changed = prev.can_go_previous != meta.can_go_previous;
                        let seek_pos_us = secs_to_micros(meta.position_secs);
                        player.meta = meta;
                        (
                            status_changed,
                            metadata_changed,
                            can_seek_changed,
                            can_next_changed,
                            can_prev_changed,
                            seek_pos_us,
                        )
                    };

                    let emitter = iface_ref.signal_emitter();
                    let player = iface_ref.get().await;
                    if status_changed {
                        let _ = player.playback_status_changed(emitter).await;
                    }
                    if metadata_changed {
                        let _ = player.metadata_changed(emitter).await;
                    }
                    if can_seek_changed {
                        let _ = player.can_seek_changed(emitter).await;
                    }
                    if can_next_changed {
                        let _ = player.can_go_next_changed(emitter).await;
                    }
                    if can_prev_changed {
                        let _ = player.can_go_previous_changed(emitter).await;
                    }
                    drop(player);

                    if emit_seeked {
                        let _ = MprisPlayer::seeked(emitter, seek_pos_us).await;
                    }
                }
                MprisMsg::NotifySeeked(pos_us) => {
                    let Some(s) = state.as_ref() else {
                        continue;
                    };
                    if let Ok(iface_ref) = s
                        .conn
                        .object_server()
                        .interface::<_, MprisPlayer>(MPRIS_OBJECT_PATH)
                        .await
                    {
                        let emitter = iface_ref.signal_emitter();
                        let _ = MprisPlayer::seeked(emitter, pos_us).await;
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
use linux_mpris::MediaBackend;

#[cfg(target_os = "windows")]
mod windows_smtc {
    use std::ffi::c_void;
    use std::sync::{atomic::AtomicI64, mpsc, Arc};
    use std::thread::JoinHandle;

    use windows::Foundation::{TimeSpan, TypedEventHandler, Uri};
    use windows::Media::{
        MediaPlaybackStatus, MediaPlaybackType, PlaybackPositionChangeRequestedEventArgs,
        SystemMediaTransportControls, SystemMediaTransportControlsButton,
        SystemMediaTransportControlsButtonPressedEventArgs,
        SystemMediaTransportControlsTimelineProperties,
    };
    use windows::Storage::Streams::RandomAccessStreamReference;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::WinRT::Media::ISystemMediaTransportControlsInterop;
    use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
    use windows_core::{factory, Ref, HSTRING};

    use super::{OsMediaCommand, OsMediaMetadata, OsPlaybackStatus};

    const TICKS_PER_SEC: f64 = 10_000_000.0;

    fn secs_to_winrt_ticks(secs: f64) -> i64 {
        if !secs.is_finite() || secs <= 0.0 {
            return 0;
        }
        let ticks = (secs * TICKS_PER_SEC).round();
        if ticks >= i64::MAX as f64 {
            i64::MAX
        } else {
            ticks as i64
        }
    }

    enum WindowsMediaMsg {
        SetEnabled(bool),
        Sync(OsMediaMetadata),
        NotifySeeked(f64),
        Shutdown,
    }

    pub(super) struct MediaBackend {
        tx: Option<mpsc::Sender<WindowsMediaMsg>>,
        worker: Option<JoinHandle<()>>,
    }

    impl MediaBackend {
        pub(super) fn start(
            _tokio_handle: tokio::runtime::Handle,
            windows_hwnd: Option<isize>,
            cmd_tx: async_channel::Sender<OsMediaCommand>,
            _position_us: Arc<AtomicI64>,
        ) -> Self {
            let Some(hwnd_raw) = windows_hwnd.filter(|&h| h != 0) else {
                return Self {
                    tx: None,
                    worker: None,
                };
            };

            let (tx, rx) = mpsc::channel();
            let worker = std::thread::Builder::new()
                .name("ferrite-smtc".to_owned())
                .spawn(move || run_smtc_worker(hwnd_raw, rx, cmd_tx))
                .ok();

            Self {
                tx: Some(tx),
                worker,
            }
        }

        pub(super) fn set_enabled(&self, enabled: bool) {
            if let Some(tx) = &self.tx {
                let _ = tx.send(WindowsMediaMsg::SetEnabled(enabled));
            }
        }

        pub(super) fn sync_state(&self, meta: OsMediaMetadata, _emit_seeked: bool) {
            if let Some(tx) = &self.tx {
                let _ = tx.send(WindowsMediaMsg::Sync(meta));
            }
        }

        pub(super) fn notify_seeked(&self, position_secs: f64, _pos_us: i64) {
            if let Some(tx) = &self.tx {
                let _ = tx.send(WindowsMediaMsg::NotifySeeked(position_secs));
            }
        }

        pub(super) fn shutdown(&mut self) {
            if let Some(tx) = self.tx.take() {
                let _ = tx.send(WindowsMediaMsg::Shutdown);
            }
            if let Some(handle) = self.worker.take() {
                let _ = handle.join();
            }
        }
    }

    fn run_smtc_worker(
        hwnd_raw: isize,
        rx: mpsc::Receiver<WindowsMediaMsg>,
        cmd_tx: async_channel::Sender<OsMediaCommand>,
    ) {
        let ro_initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_ok();

        let smtc = (|| -> windows_core::Result<SystemMediaTransportControls> {
            let interop: ISystemMediaTransportControlsInterop =
                factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>()?;
            unsafe { interop.GetForWindow(HWND(hwnd_raw as *mut c_void)) }
        })();

        let Ok(smtc) = smtc else {
            tracing::debug!(
                target: "ferrite::os_integration",
                "Failed to acquire Windows SystemMediaTransportControls for HWND"
            );
            if ro_initialized {
                unsafe { RoUninitialize() };
            }
            return;
        };

        let btn_tx = cmd_tx.clone();
        let button_token = smtc
            .ButtonPressed(&TypedEventHandler::<
                SystemMediaTransportControls,
                SystemMediaTransportControlsButtonPressedEventArgs,
            >::new(
                move |_sender, args: Ref<'_, SystemMediaTransportControlsButtonPressedEventArgs>| {
                    if let Some(args) = args.as_ref() {
                        if let Ok(button) = args.Button() {
                            let cmd = match button {
                                SystemMediaTransportControlsButton::Play => Some(OsMediaCommand::Play),
                                SystemMediaTransportControlsButton::Pause => Some(OsMediaCommand::Pause),
                                SystemMediaTransportControlsButton::Stop => Some(OsMediaCommand::Stop),
                                SystemMediaTransportControlsButton::Next => Some(OsMediaCommand::Next),
                                SystemMediaTransportControlsButton::Previous => {
                                    Some(OsMediaCommand::Previous)
                                }
                                SystemMediaTransportControlsButton::FastForward => {
                                    Some(OsMediaCommand::SeekRelativeSecs(10.0))
                                }
                                SystemMediaTransportControlsButton::Rewind => {
                                    Some(OsMediaCommand::SeekRelativeSecs(-10.0))
                                }
                                _ => None,
                            };
                            if let Some(cmd) = cmd {
                                let _ = btn_tx.try_send(cmd);
                            }
                        }
                    }
                    Ok(())
                },
            ))
            .ok();

        let seek_tx = cmd_tx;
        let seek_token = smtc
            .PlaybackPositionChangeRequested(&TypedEventHandler::<
                SystemMediaTransportControls,
                PlaybackPositionChangeRequestedEventArgs,
            >::new(
                move |_sender, args: Ref<'_, PlaybackPositionChangeRequestedEventArgs>| {
                    if let Some(args) = args.as_ref() {
                        if let Ok(pos) = args.RequestedPlaybackPosition() {
                            let secs = (pos.Duration.max(0) as f64) / TICKS_PER_SEC;
                            let _ = seek_tx.try_send(OsMediaCommand::SetPositionSecs(secs));
                        }
                    }
                    Ok(())
                },
            ))
            .ok();

        let mut last_duration_secs = 0.0_f64;
        let mut last_can_seek = false;

        while let Ok(msg) = rx.recv() {
            match msg {
                WindowsMediaMsg::SetEnabled(enabled) => {
                    let _ = smtc.SetIsEnabled(enabled);
                }
                WindowsMediaMsg::Sync(meta) => {
                    last_duration_secs = meta.duration_secs;
                    last_can_seek = meta.can_seek;

                    let _ = smtc.SetIsEnabled(true);
                    let _ = smtc.SetIsPlayEnabled(true);
                    let _ = smtc.SetIsPauseEnabled(true);
                    let _ = smtc.SetIsStopEnabled(true);
                    let _ = smtc.SetIsNextEnabled(meta.can_go_next);
                    let _ = smtc.SetIsPreviousEnabled(meta.can_go_previous);

                    let status = match meta.status {
                        OsPlaybackStatus::Stopped => MediaPlaybackStatus::Stopped,
                        OsPlaybackStatus::Playing => MediaPlaybackStatus::Playing,
                        OsPlaybackStatus::Paused => MediaPlaybackStatus::Paused,
                    };
                    let _ = smtc.SetPlaybackStatus(status);

                    if let Ok(updater) = smtc.DisplayUpdater() {
                        let _ = updater.SetType(MediaPlaybackType::Video);
                        if let Ok(video) = updater.VideoProperties() {
                            let _ = video.SetTitle(&HSTRING::from(&meta.title));
                            let _ = video.SetSubtitle(&HSTRING::from(&meta.subtitle));
                        }
                        if let Some(url) = meta.poster_url.as_deref().filter(|u| !u.is_empty()) {
                            if let Ok(uri) = Uri::CreateUri(&HSTRING::from(url)) {
                                if let Ok(thumb) = RandomAccessStreamReference::CreateFromUri(&uri)
                                {
                                    let _ = updater.SetThumbnail(&thumb);
                                }
                            }
                        }
                        let _ = updater.Update();
                    }

                    update_smtc_timeline(
                        &smtc,
                        meta.duration_secs,
                        meta.position_secs,
                        meta.can_seek,
                    );
                }
                WindowsMediaMsg::NotifySeeked(position_secs) => {
                    update_smtc_timeline(&smtc, last_duration_secs, position_secs, last_can_seek);
                }
                WindowsMediaMsg::Shutdown => break,
            }
        }

        if let Some(token) = button_token {
            let _ = smtc.RemoveButtonPressed(token);
        }
        if let Some(token) = seek_token {
            let _ = smtc.RemovePlaybackPositionChangeRequested(token);
        }
        let _ = smtc.SetIsEnabled(false);
        drop(smtc);

        if ro_initialized {
            unsafe { RoUninitialize() };
        }
    }

    fn update_smtc_timeline(
        smtc: &SystemMediaTransportControls,
        duration_secs: f64,
        position_secs: f64,
        can_seek: bool,
    ) {
        let Ok(timeline) = SystemMediaTransportControlsTimelineProperties::new() else {
            return;
        };
        let end_ticks = secs_to_winrt_ticks(duration_secs);
        let pos_ticks = secs_to_winrt_ticks(position_secs).clamp(0, end_ticks.max(0));
        let _ = timeline.SetStartTime(TimeSpan { Duration: 0 });
        let _ = timeline.SetMinSeekTime(TimeSpan { Duration: 0 });
        let _ = timeline.SetEndTime(TimeSpan {
            Duration: end_ticks,
        });
        let _ = timeline.SetMaxSeekTime(TimeSpan {
            Duration: if can_seek { end_ticks } else { 0 },
        });
        let _ = timeline.SetPosition(TimeSpan {
            Duration: pos_ticks,
        });
        let _ = smtc.UpdateTimelineProperties(&timeline);
    }
}

#[cfg(target_os = "windows")]
use windows_smtc::MediaBackend;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod noop_media {
    use std::sync::{atomic::AtomicI64, Arc};

    use super::{OsMediaCommand, OsMediaMetadata};

    pub(super) struct MediaBackend;

    impl MediaBackend {
        pub(super) fn start(
            _tokio_handle: tokio::runtime::Handle,
            _windows_hwnd: Option<isize>,
            _cmd_tx: async_channel::Sender<OsMediaCommand>,
            _position_us: Arc<AtomicI64>,
        ) -> Self {
            Self
        }

        pub(super) fn set_enabled(&self, _enabled: bool) {}

        pub(super) fn sync_state(&self, _meta: OsMediaMetadata, _emit_seeked: bool) {}

        pub(super) fn notify_seeked(&self, _position_secs: f64, _pos_us: i64) {}

        pub(super) fn shutdown(&mut self) {}
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
use noop_media::MediaBackend;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta() -> OsMediaMetadata {
        OsMediaMetadata {
            status: OsPlaybackStatus::Playing,
            title: "Dune: Part Two".to_owned(),
            subtitle: "2024 • 4K HDR".to_owned(),
            poster_url: Some("https://example.com/poster.jpg".to_owned()),
            duration_secs: 9960.0,
            position_secs: 120.0,
            can_seek: true,
            can_go_next: false,
            can_go_previous: false,
        }
    }

    #[test]
    fn sync_diff_ignores_60hz_smooth_playback_and_triggers_on_meaningful_changes() {
        let prev = sample_meta();

        // 60Hz tick (+16.6ms) must NOT trigger a sync.
        let mut tick = prev.clone();
        tick.position_secs = 120.0166;
        assert!(!should_sync_media_update(&prev, &tick, 0.0166));

        // 5 seconds of continuous 1x playback must NOT trigger a sync.
        let mut continuous = prev.clone();
        continuous.position_secs = 125.0;
        assert!(!should_sync_media_update(&prev, &continuous, 5.0));

        // Seek jump > 2.5s while playing MUST trigger a sync.
        let mut seeked = prev.clone();
        seeked.position_secs = 125.0;
        assert!(should_sync_media_update(&prev, &seeked, 0.1));
        assert!(is_seek_jump(&prev, &seeked, 0.1));

        // Backward seek jump > 2.5s while paused MUST trigger a sync.
        let mut paused_prev = prev.clone();
        paused_prev.status = OsPlaybackStatus::Paused;
        let mut paused_seek = paused_prev.clone();
        paused_seek.position_secs = 115.0;
        assert!(should_sync_media_update(&paused_prev, &paused_seek, 4.0));

        // Status change MUST trigger.
        let mut paused = prev.clone();
        paused.status = OsPlaybackStatus::Paused;
        assert!(should_sync_media_update(&prev, &paused, 0.0));

        // Title, subtitle, poster, duration, capability changes MUST trigger.
        let mut title_changed = prev.clone();
        title_changed.title = "Arrival".to_owned();
        assert!(should_sync_media_update(&prev, &title_changed, 0.0));

        let mut subtitle_changed = prev.clone();
        subtitle_changed.subtitle = "S01E02".to_owned();
        assert!(should_sync_media_update(&prev, &subtitle_changed, 0.0));

        let mut poster_changed = prev.clone();
        poster_changed.poster_url = None;
        assert!(should_sync_media_update(&prev, &poster_changed, 0.0));

        let mut duration_changed = prev.clone();
        duration_changed.duration_secs = 5400.0;
        assert!(should_sync_media_update(&prev, &duration_changed, 0.0));

        let mut cap_changed = prev.clone();
        cap_changed.can_go_next = true;
        assert!(should_sync_media_update(&prev, &cap_changed, 0.0));
    }

    #[test]
    fn controller_lifecycle_and_throttling_update_atomic_position() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio test runtime");

        let (mut controller, _rx) = OsMediaController::start(rt.handle().clone(), None);
        let mut meta = sample_meta();
        controller.sync_state(meta.clone());
        assert_eq!(
            controller.latest_position_us.load(Ordering::Relaxed),
            120_000_000
        );

        let first_synced_at = controller.last_synced_at.expect("initial sync timestamp");

        // Smooth 60Hz frame advance updates position atomic without re-syncing backend.
        meta.position_secs = 120.05;
        controller.sync_state(meta.clone());
        assert_eq!(
            controller.latest_position_us.load(Ordering::Relaxed),
            120_050_000
        );
        assert_eq!(controller.last_synced_at, Some(first_synced_at));

        // Disable then re-enable clears and restores synced state cleanly.
        controller.set_enabled(false);
        assert!(controller.last_synced.is_none());
        controller.notify_seeked(200.0);
        assert_eq!(
            controller.latest_position_us.load(Ordering::Relaxed),
            200_000_000
        );
        controller.set_enabled(true);
        assert!(controller.last_synced.is_some());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mpris2_metadata_map_formats_spec_fields() {
        use zbus::zvariant::{ObjectPath, Value};

        let meta = sample_meta();
        let map = linux_mpris::build_mpris_metadata_map(&meta);

        let trackid = map.get("mpris:trackid").expect("mpris:trackid");
        assert!(matches!(
            &**trackid,
            Value::ObjectPath(p) if *p == ObjectPath::from_static_str_unchecked("/org/mpris/MediaPlayer2/ferrite/track/current")
        ));

        let length = map.get("mpris:length").expect("mpris:length");
        assert_eq!(i64::try_from(length).expect("i64 length"), 9_960_000_000);

        let title = map.get("xesam:title").expect("xesam:title");
        assert_eq!(
            String::try_from(title.clone()).expect("string title"),
            "Dune: Part Two"
        );

        let artist = map.get("xesam:artist").expect("xesam:artist");
        assert_eq!(
            Vec::<String>::try_from(artist.clone()).expect("vec string artist"),
            vec!["2024 • 4K HDR".to_owned()]
        );

        let art_url = map.get("mpris:artUrl").expect("mpris:artUrl");
        assert_eq!(
            String::try_from(art_url.clone()).expect("string artUrl"),
            "https://example.com/poster.jpg"
        );

        let stopped = OsMediaMetadata::default();
        let stopped_map = linux_mpris::build_mpris_metadata_map(&stopped);
        let stopped_trackid = stopped_map.get("mpris:trackid").expect("NoTrack id");
        assert!(matches!(
            &**stopped_trackid,
            Value::ObjectPath(p) if *p == ObjectPath::from_static_str_unchecked("/org/mpris/MediaPlayer2/TrackList/NoTrack")
        ));
        assert!(!stopped_map.contains_key("mpris:length"));
        assert!(!stopped_map.contains_key("xesam:title"));
        assert!(!stopped_map.contains_key("xesam:artist"));
        assert!(!stopped_map.contains_key("mpris:artUrl"));

        assert_eq!(
            linux_mpris::mpris_playback_status(OsPlaybackStatus::Stopped),
            "Stopped"
        );
        assert_eq!(
            linux_mpris::mpris_playback_status(OsPlaybackStatus::Playing),
            "Playing"
        );
        assert_eq!(
            linux_mpris::mpris_playback_status(OsPlaybackStatus::Paused),
            "Paused"
        );
        assert_eq!(
            linux_mpris::mpris_fallback_bus_name(4242),
            "org.mpris.MediaPlayer2.ferrite.instance4242"
        );
    }
}
