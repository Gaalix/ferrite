use gpui::{
    div, prelude::*, px, rgb, rgba, size, App, AsyncApp, Bounds, Context, MouseButton,
    MouseDownEvent, Render, SharedString, WeakEntity, Window, WindowBackgroundAppearance,
    WindowBounds, WindowOptions,
};
use gpui_mpv::{MpvController, MpvViewport, PlayerSnapshot};

struct DemoPlayerView {
    controller: MpvController,
    snapshot: PlayerSnapshot,
    source_label: SharedString,
}

impl DemoPlayerView {
    fn new(media_url: String, cx: &mut Context<Self>) -> Self {
        let controller = MpvController::new().expect("failed to initialize libmpv controller");
        controller
            .load_file(&media_url, None)
            .expect("failed to queue media URL in libmpv");

        let snapshot = controller.snapshot();
        let mut watch_rx = controller.subscribe();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while watch_rx.changed().await.is_ok() {
                let latest = watch_rx.borrow_and_update().clone();
                if this
                    .update(cx, |view, cx| {
                        view.snapshot = latest;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            controller,
            snapshot,
            source_label: SharedString::from(media_url),
        }
    }

    fn on_toggle_pause(
        &mut self,
        _event: &MouseDownEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let _ = self.controller.toggle_pause();
    }

    fn on_seek_back(
        &mut self,
        _event: &MouseDownEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let _ = self.controller.seek_relative(-10.0);
    }

    fn on_seek_forward(
        &mut self,
        _event: &MouseDownEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let _ = self.controller.seek_relative(10.0);
    }

    fn on_toggle_mute(
        &mut self,
        _event: &MouseDownEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let _ = self.controller.set_muted(!self.snapshot.muted);
    }
}

fn format_seconds(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "00:00".to_string();
    }
    let total = secs.floor() as u64;
    let hours = total / 3600;
    let mins = (total % 3600) / 60;
    let rem = total % 60;
    if hours > 0 {
        format!("{hours:02}:{mins:02}:{rem:02}")
    } else {
        format!("{mins:02}:{rem:02}")
    }
}

impl Render for DemoPlayerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = &self.snapshot;
        let play_label = if snap.paused { "Play" } else { "Pause" };
        let mute_label = if snap.muted { "Unmute" } else { "Mute" };
        let time_text = format!(
            "{} / {}",
            format_seconds(snap.time_pos),
            format_seconds(snap.duration)
        );
        let hwdec_badge = if snap.hwdec_current.is_empty() || snap.hwdec_current == "no" {
            "HWDEC: SW".to_string()
        } else {
            format!("HWDEC: {}", snap.hwdec_current.to_uppercase())
        };
        let res_badge = if snap.video_width > 0 && snap.video_height > 0 {
            if snap.video_codec.is_empty() {
                format!("{}x{}", snap.video_width, snap.video_height)
            } else {
                format!(
                    "{}x{} · {}",
                    snap.video_width,
                    snap.video_height,
                    snap.video_codec.to_uppercase()
                )
            }
        } else if snap.buffering {
            "Buffering...".to_string()
        } else {
            "Ready".to_string()
        };

        MpvViewport::new(self.controller.clone()).child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .justify_between()
                .p(px(24.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .px(px(14.0))
                                .py(px(8.0))
                                .rounded(px(999.0))
                                .bg(rgba(0x0d1017cc))
                                .border_1()
                                .border_color(rgba(0xffffff22))
                                .text_sm()
                                .text_color(rgb(0xe6edf3))
                                .child(self.source_label.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .px(px(12.0))
                                        .py(px(6.0))
                                        .rounded(px(999.0))
                                        .bg(rgba(0x0d1017cc))
                                        .border_1()
                                        .border_color(rgba(0x7c5cff66))
                                        .text_xs()
                                        .text_color(rgb(0xc4b5fd))
                                        .child(hwdec_badge),
                                )
                                .child(
                                    div()
                                        .px(px(12.0))
                                        .py(px(6.0))
                                        .rounded(px(999.0))
                                        .bg(rgba(0x0d1017cc))
                                        .border_1()
                                        .border_color(rgba(0xffffff22))
                                        .text_xs()
                                        .text_color(rgb(0x9ca3af))
                                        .child(res_badge),
                                ),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(12.0))
                                .px(px(20.0))
                                .py(px(12.0))
                                .rounded(px(999.0))
                                .bg(rgba(0x0d1017d9))
                                .border_1()
                                .border_color(rgba(0xffffff26))
                                .child(
                                    div()
                                        .id("btn-seek-back")
                                        .cursor_pointer()
                                        .px(px(12.0))
                                        .py(px(6.0))
                                        .rounded(px(999.0))
                                        .bg(rgba(0xffffff14))
                                        .hover(|s| s.bg(rgba(0xffffff28)))
                                        .text_sm()
                                        .text_color(rgb(0xf3f4f6))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(Self::on_seek_back),
                                        )
                                        .child("-10s"),
                                )
                                .child(
                                    div()
                                        .id("btn-play-pause")
                                        .cursor_pointer()
                                        .px(px(18.0))
                                        .py(px(6.0))
                                        .rounded(px(999.0))
                                        .bg(rgb(0x7c5cff))
                                        .hover(|s| s.bg(rgb(0x8b6eff)))
                                        .text_sm()
                                        .text_color(rgb(0xffffff))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(Self::on_toggle_pause),
                                        )
                                        .child(play_label),
                                )
                                .child(
                                    div()
                                        .id("btn-seek-fwd")
                                        .cursor_pointer()
                                        .px(px(12.0))
                                        .py(px(6.0))
                                        .rounded(px(999.0))
                                        .bg(rgba(0xffffff14))
                                        .hover(|s| s.bg(rgba(0xffffff28)))
                                        .text_sm()
                                        .text_color(rgb(0xf3f4f6))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(Self::on_seek_forward),
                                        )
                                        .child("+10s"),
                                )
                                .child(
                                    div()
                                        .px(px(10.0))
                                        .text_sm()
                                        .text_color(rgb(0xd1d5db))
                                        .child(time_text),
                                )
                                .child(
                                    div()
                                        .id("btn-mute")
                                        .cursor_pointer()
                                        .px(px(12.0))
                                        .py(px(6.0))
                                        .rounded(px(999.0))
                                        .bg(rgba(0xffffff14))
                                        .hover(|s| s.bg(rgba(0xffffff28)))
                                        .text_sm()
                                        .text_color(rgb(0xf3f4f6))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(Self::on_toggle_mute),
                                        )
                                        .child(mute_label),
                                ),
                        ),
                ),
        )
    }
}

fn main() {
    let Some(media_url) = std::env::args().nth(1) else {
        tracing::error!("usage: wayland_demo <http(s)://stream-url>");
        std::process::exit(2);
    };

    gpui_platform::application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(720.0)), cx);
        let url_for_win = media_url.clone();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_background: WindowBackgroundAppearance::Transparent,
                ..Default::default()
            },
            move |_window, cx| cx.new(|cx| DemoPlayerView::new(url_for_win, cx)),
        )
        .expect("failed to open transparent Wayland GPUI window");
        cx.activate(true);
    });
}
