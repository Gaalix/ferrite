//! OS display-sleep inhibition while video playback is active.

/// Returns `true` when display sleep should be inhibited for the current player state.
pub fn should_inhibit_display_sleep(
    pref_enabled: bool,
    in_player_route: bool,
    video_ready: bool,
    paused: bool,
    eof_reached: bool,
) -> bool {
    pref_enabled && in_player_route && video_ready && !paused && !eof_reached
}

/// Manages an OS-level display/idle sleep inhibition lock across platforms.
///
/// - Linux: D-Bus `org.freedesktop.ScreenSaver.Inhibit` with fallback to
///   `org.gnome.SessionManager.Inhibit`.
/// - Windows: dedicated worker thread calling `SetThreadExecutionState`.
/// - Other platforms: clean no-op tracking boolean state.
pub struct SleepInhibitor {
    inhibited: bool,
    backend: SleepBackend,
}

impl SleepInhibitor {
    /// Creates a new `SleepInhibitor` bound to the given Tokio runtime handle.
    pub fn new(tokio_handle: tokio::runtime::Handle) -> Self {
        Self {
            inhibited: false,
            backend: SleepBackend::new(tokio_handle),
        }
    }

    /// Transitions the sleep inhibition state, acquiring or releasing the OS lock as needed.
    pub fn set_inhibited(&mut self, active: bool, reason: &str) {
        if self.inhibited == active {
            return;
        }
        self.inhibited = active;
        self.backend.set_inhibited(active, reason);
    }

    /// Returns whether display sleep inhibition is currently active.
    pub fn is_inhibited(&self) -> bool {
        self.inhibited
    }
}

impl Drop for SleepInhibitor {
    fn drop(&mut self) {
        if self.inhibited {
            self.inhibited = false;
            self.backend.set_inhibited(false, "drop");
        }
        self.backend.shutdown();
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use tokio::sync::mpsc;

    enum SleepCmd {
        Acquire(String),
        Release,
        Shutdown,
    }

    #[derive(Debug, Clone, Copy)]
    enum ActiveCookie {
        Freedesktop(u32),
        Gnome(u32),
    }

    pub(super) struct SleepBackend {
        tx: mpsc::UnboundedSender<SleepCmd>,
    }

    impl SleepBackend {
        pub(super) fn new(tokio_handle: tokio::runtime::Handle) -> Self {
            let (tx, rx) = mpsc::unbounded_channel();
            tokio_handle.spawn(run_linux_sleep_actor(rx));
            Self { tx }
        }

        pub(super) fn set_inhibited(&self, active: bool, reason: &str) {
            let cmd = if active {
                SleepCmd::Acquire(reason.to_owned())
            } else {
                SleepCmd::Release
            };
            let _ = self.tx.send(cmd);
        }

        pub(super) fn shutdown(&mut self) {
            let _ = self.tx.send(SleepCmd::Shutdown);
        }
    }

    async fn run_linux_sleep_actor(mut rx: mpsc::UnboundedReceiver<SleepCmd>) {
        let mut conn: Option<zbus::Connection> = None;
        let mut active_cookie: Option<ActiveCookie> = None;

        while let Some(cmd) = rx.recv().await {
            match cmd {
                SleepCmd::Acquire(reason) => {
                    if active_cookie.is_some() {
                        continue;
                    }
                    let Some(bus) = ensure_session_bus(&mut conn).await else {
                        continue;
                    };
                    active_cookie = acquire_dbus_inhibit(bus, &reason).await;
                }
                SleepCmd::Release => {
                    if let (Some(cookie), Some(bus)) = (active_cookie.take(), conn.as_ref()) {
                        release_dbus_inhibit(bus, cookie).await;
                    }
                }
                SleepCmd::Shutdown => {
                    if let (Some(cookie), Some(bus)) = (active_cookie.take(), conn.as_ref()) {
                        release_dbus_inhibit(bus, cookie).await;
                    }
                    break;
                }
            }
        }

        if let (Some(cookie), Some(bus)) = (active_cookie.take(), conn.as_ref()) {
            release_dbus_inhibit(bus, cookie).await;
        }
    }

    async fn ensure_session_bus(slot: &mut Option<zbus::Connection>) -> Option<&zbus::Connection> {
        if slot.is_none() {
            match zbus::Connection::session().await {
                Ok(c) => *slot = Some(c),
                Err(err) => {
                    tracing::debug!(
                        target: "ferrite::os_integration",
                        "Session D-Bus unavailable for sleep inhibition: {err}"
                    );
                    return None;
                }
            }
        }
        slot.as_ref()
    }

    async fn acquire_dbus_inhibit(conn: &zbus::Connection, reason: &str) -> Option<ActiveCookie> {
        let effective_reason = if reason.is_empty() {
            "Video playback active"
        } else {
            reason
        };

        match conn
            .call_method(
                Some("org.freedesktop.ScreenSaver"),
                "/org/freedesktop/ScreenSaver",
                Some("org.freedesktop.ScreenSaver"),
                "Inhibit",
                &("Ferrite", effective_reason),
            )
            .await
        {
            Ok(reply) => match reply.body().deserialize::<u32>() {
                Ok(cookie) => {
                    tracing::debug!(
                        target: "ferrite::os_integration",
                        cookie,
                        "Acquired org.freedesktop.ScreenSaver inhibit lock"
                    );
                    return Some(ActiveCookie::Freedesktop(cookie));
                }
                Err(err) => {
                    tracing::debug!(
                        target: "ferrite::os_integration",
                        "Unexpected reply from org.freedesktop.ScreenSaver.Inhibit: {err}"
                    );
                }
            },
            Err(err) => {
                tracing::debug!(
                    target: "ferrite::os_integration",
                    "org.freedesktop.ScreenSaver.Inhibit unavailable ({err}), trying GNOME SessionManager"
                );
            }
        }

        match conn
            .call_method(
                Some("org.gnome.SessionManager"),
                "/org/gnome/SessionManager",
                Some("org.gnome.SessionManager"),
                "Inhibit",
                &("Ferrite", 0u32, effective_reason, 8u32),
            )
            .await
        {
            Ok(reply) => match reply.body().deserialize::<u32>() {
                Ok(cookie) => {
                    tracing::debug!(
                        target: "ferrite::os_integration",
                        cookie,
                        "Acquired org.gnome.SessionManager inhibit lock"
                    );
                    return Some(ActiveCookie::Gnome(cookie));
                }
                Err(err) => {
                    tracing::debug!(
                        target: "ferrite::os_integration",
                        "Unexpected reply from org.gnome.SessionManager.Inhibit: {err}"
                    );
                }
            },
            Err(err) => {
                tracing::debug!(
                    target: "ferrite::os_integration",
                    "org.gnome.SessionManager.Inhibit unavailable: {err}"
                );
            }
        }

        None
    }

    async fn release_dbus_inhibit(conn: &zbus::Connection, cookie: ActiveCookie) {
        match cookie {
            ActiveCookie::Freedesktop(cookie) => {
                if let Err(err) = conn
                    .call_method(
                        Some("org.freedesktop.ScreenSaver"),
                        "/org/freedesktop/ScreenSaver",
                        Some("org.freedesktop.ScreenSaver"),
                        "UnInhibit",
                        &(cookie,),
                    )
                    .await
                {
                    tracing::debug!(
                        target: "ferrite::os_integration",
                        cookie,
                        "org.freedesktop.ScreenSaver.UnInhibit failed: {err}"
                    );
                }
            }
            ActiveCookie::Gnome(cookie) => {
                let res = conn
                    .call_method(
                        Some("org.gnome.SessionManager"),
                        "/org/gnome/SessionManager",
                        Some("org.gnome.SessionManager"),
                        "Uninhibit",
                        &(cookie,),
                    )
                    .await;
                if res.is_err() {
                    let _ = conn
                        .call_method(
                            Some("org.gnome.SessionManager"),
                            "/org/gnome/SessionManager",
                            Some("org.gnome.SessionManager"),
                            "UnInhibit",
                            &(cookie,),
                        )
                        .await;
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::sync::mpsc;
    use std::thread::JoinHandle;
    use windows::Win32::System::Power::{
        SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
    };

    enum WindowsSleepCmd {
        Set(bool),
        Shutdown,
    }

    pub(super) struct SleepBackend {
        tx: mpsc::Sender<WindowsSleepCmd>,
        worker: Option<JoinHandle<()>>,
    }

    impl SleepBackend {
        pub(super) fn new(_tokio_handle: tokio::runtime::Handle) -> Self {
            let (tx, rx) = mpsc::channel();
            let worker = std::thread::Builder::new()
                .name("ferrite-sleep-inhibitor".to_owned())
                .spawn(move || run_windows_sleep_worker(rx))
                .ok();
            Self { tx, worker }
        }

        pub(super) fn set_inhibited(&self, active: bool, _reason: &str) {
            let _ = self.tx.send(WindowsSleepCmd::Set(active));
        }

        pub(super) fn shutdown(&mut self) {
            let _ = self.tx.send(WindowsSleepCmd::Shutdown);
            if let Some(handle) = self.worker.take() {
                let _ = handle.join();
            }
        }
    }

    fn run_windows_sleep_worker(rx: mpsc::Receiver<WindowsSleepCmd>) {
        let mut active = false;
        while let Ok(cmd) = rx.recv() {
            match cmd {
                WindowsSleepCmd::Set(true) => {
                    unsafe {
                        let _ = SetThreadExecutionState(
                            ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED,
                        );
                    }
                    active = true;
                }
                WindowsSleepCmd::Set(false) => {
                    if active {
                        unsafe {
                            let _ = SetThreadExecutionState(ES_CONTINUOUS);
                        }
                        active = false;
                    }
                }
                WindowsSleepCmd::Shutdown => {
                    break;
                }
            }
        }
        if active {
            unsafe {
                let _ = SetThreadExecutionState(ES_CONTINUOUS);
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod platform {
    pub(super) struct SleepBackend;

    impl SleepBackend {
        pub(super) fn new(_tokio_handle: tokio::runtime::Handle) -> Self {
            Self
        }

        pub(super) fn set_inhibited(&self, _active: bool, _reason: &str) {}

        pub(super) fn shutdown(&mut self) {}
    }
}

use platform::SleepBackend;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_inhibit_display_sleep_truth_table() {
        assert!(should_inhibit_display_sleep(true, true, true, false, false));
        assert!(!should_inhibit_display_sleep(
            false, true, true, false, false
        ));
        assert!(!should_inhibit_display_sleep(
            true, false, true, false, false
        ));
        assert!(!should_inhibit_display_sleep(
            true, true, false, false, false
        ));
        assert!(!should_inhibit_display_sleep(true, true, true, true, false));
        assert!(!should_inhibit_display_sleep(true, true, true, false, true));
    }

    #[test]
    fn sleep_inhibitor_lifecycle_tracks_state_and_drops_cleanly() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio test runtime");

        let mut inhibitor = SleepInhibitor::new(rt.handle().clone());
        assert!(!inhibitor.is_inhibited());

        inhibitor.set_inhibited(true, "Playing test video");
        assert!(inhibitor.is_inhibited());

        // Idempotent re-enable should keep state true without panic.
        inhibitor.set_inhibited(true, "Still playing");
        assert!(inhibitor.is_inhibited());

        inhibitor.set_inhibited(false, "Paused");
        assert!(!inhibitor.is_inhibited());

        // Re-inhibit and drop directly while active to verify Drop cleanup.
        inhibitor.set_inhibited(true, "Resumed");
        assert!(inhibitor.is_inhibited());
        drop(inhibitor);

        // Drain any spawned actor work cleanly.
        rt.block_on(async {
            tokio::task::yield_now().await;
        });
    }
}
