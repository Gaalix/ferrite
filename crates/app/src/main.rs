#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use ferrite::{
    icons::EmbeddedIconAssets,
    poster_cache::PosterCache,
    shell::AppShell,
    state::{
        default_library_selected, extract_cli_addon_arg, start_stream_engine_with_fallback,
        AppPreferences, BootServices,
    },
};
use gpui::{
    px, size, App, AppContext as _, Bounds, TitlebarOptions, WindowBackgroundAppearance,
    WindowBounds, WindowDecorations, WindowOptions,
};
use gpui_mpv::PlayerSnapshot;
use stremio_core_runtime::{CoreController, EnvConfig};

fn main() -> anyhow::Result<()> {
    let initial_addon_url = extract_cli_addon_arg(std::env::args().skip(1));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("ferrite-tokio")
        .build()?;
    let rt_handle = runtime.handle().clone();

    let preferences = AppPreferences::load_or_default();

    let engine = runtime.block_on(start_stream_engine_with_fallback(&preferences))?;
    let streaming_url = engine.base_url();

    let env_config = EnvConfig::default()
        .with_streaming_server_url(streaming_url.clone())
        .with_runtime_handle(rt_handle.clone());
    let core = runtime.block_on(CoreController::boot(env_config))?;

    if !core.sync_loopback_streaming_server_url(streaming_url) {
        core.reload_streaming_server();
    }
    core.load_board();
    core.load_library(default_library_selected());
    core.load_installed_addons(None);

    let poster_cache = PosterCache::with_default_dir(rt_handle.clone());
    let mpv = None;
    let initial_snapshot = PlayerSnapshot::default();

    let engine_for_shutdown = engine.clone();
    let boot_services = BootServices {
        tokio_handle: rt_handle,
        core,
        engine,
        poster_cache,
        preferences,
        mpv,
        initial_snapshot,
        clipboard_seed: None,
        initial_addon_url,
    };

    let _rt_guard = runtime.enter();
    gpui_platform::application()
        .with_assets(EmbeddedIconAssets)
        .run(move |cx: &mut App| {
            cx.on_window_closed(|cx, _window_id| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);

            if let Err(err) = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(900.0), px(560.0))),
                    window_decorations: Some(WindowDecorations::Client),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Ferrite".into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    app_id: Some("ferrite".to_owned()),
                    window_background: WindowBackgroundAppearance::Blurred,
                    ..Default::default()
                },
                move |window, cx| {
                    cx.new(|cx| {
                        let mut shell = AppShell::new(boot_services, window, cx);
                        shell.schedule_startup_update_check(cx);
                        shell
                    })
                },
            ) {
                tracing::error!(target: "ferrite::boot", "failed to open Ferrite window: {err}");
                cx.quit();
                return;
            }

            cx.activate(true);
        });

    runtime.block_on(engine_for_shutdown.shutdown());
    Ok(())
}
