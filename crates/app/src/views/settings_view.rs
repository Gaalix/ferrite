use gpui::{
    div, prelude::*, px, rgb, rgba, ClipboardItem, Context, FontWeight, MouseButton, SharedString,
};
use stremio_core_runtime::format_bytes;

use crate::{
    icons::{svg_icon, IconName},
    shell::AppShell,
    state::{
        DebridAccessMode, ShaderPresetChoice, SidebarTab, StorageModeChoice, ToastKind,
        ToneMappingChoice, UpscaleChoice,
    },
    theme::{
        editorial_serif, meta_divider, mono_font, neutral_chip, semantic_status_pill, ui_sans,
        ACCENT, ACCENT_HOVER, ACCENT_ROSE, ACCENT_SOFT, BG_ELEVATED, BG_OBSIDIAN, BG_SURFACE,
        BG_SURFACE_3, BORDER_STRONG, BORDER_SUBTLE, STATUS_WATCHED, TEXT_MUTED, TEXT_PRIMARY,
        TEXT_SECONDARY,
    },
    updater::{self, UpdateStatus, CURRENT_APP_VERSION},
};

impl AppShell {
    pub fn render_settings_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let engine_snap = self.engine.engine_stats();
        let cache_used = self.engine.cache_usage_bytes();
        let engine_url = self.engine.base_url().to_string();
        let auto_debrid_detected = self.core.has_debrid_addon_configured();
        let prefs = &self.preferences;

        div()
            .id("settings-view-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .max_w(px(1080.0))
                    .mx_auto()
                    .px(px(32.0))
                    .pt(px(32.0))
                    .pb(px(72.0))
                    .flex()
                    .flex_col()
                    .gap(px(28.0))
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .justify_between()
                            .gap(px(20.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .font(editorial_serif())
                                            .text_size(px(56.0))
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child("Settings"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(14.5))
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(
                                                "Storage mode, BitTorrent resilience, software updates, and libmpv video quality.",
                                            ),
                                    ),
                            )
                            .child(semantic_status_pill(
                                IconName::Check,
                                "Engine online",
                                STATUS_WATCHED,
                                0x4ade804d,
                                0x4ade801c,
                            )),
                    )
                    .child(
                        div()
                            .grid()
                            .grid_cols(4)
                            .gap(px(14.0))
                            .child(render_metric_card(
                                "Active swarms",
                                format!("{}", engine_snap.active_torrents),
                                format!("{} peers connected", engine_snap.connected_peers),
                            ))
                            .child(render_metric_card(
                                "Throughput",
                                format!(
                                    "{:.2} MB/s",
                                    engine_snap.download_speed_bytes / (1024.0 * 1024.0)
                                ),
                                format!("{} transferred", format_bytes(engine_snap.downloaded_bytes)),
                            ))
                            .child(render_metric_card(
                                "Cache usage",
                                format_bytes(cache_used),
                                prefs.storage_mode.short_badge(),
                            ))
                            .child(render_metric_card(
                                "Local endpoint",
                                engine_url,
                                format!("Tone-map {}", prefs.tone_mapping.label()),
                            )),
                    )
                    .child(self.render_software_updates_card(cx))
                    .child(
                        div()
                            .p(px(24.0))
                            .rounded(px(10.0))
                            .bg(rgb(BG_SURFACE))
                            .border_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .flex()
                            .flex_col()
                            .gap(px(16.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .font(editorial_serif())
                                            .text_size(px(28.0))
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child("Cache & storage mode"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(13.5))
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(
                                                "Choose warm-resume disk LRU caching or zero-SSD-write RAM ring buffering.",
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(8.0))
                                    .children(
                                        [
                                            (
                                                StorageModeChoice::DiskLru { budget_gb: 10 },
                                                "Disk LRU 10 GB",
                                            ),
                                            (
                                                StorageModeChoice::DiskLru { budget_gb: 20 },
                                                "Disk LRU 20 GB",
                                            ),
                                            (
                                                StorageModeChoice::DiskLru { budget_gb: 50 },
                                                "Disk LRU 50 GB",
                                            ),
                                            (
                                                StorageModeChoice::RamOnly { budget_gb: 2 },
                                                "RAM-Only 2 GB",
                                            ),
                                            (
                                                StorageModeChoice::RamOnly { budget_gb: 4 },
                                                "RAM-Only 4 GB",
                                            ),
                                        ]
                                        .into_iter()
                                        .enumerate()
                                        .map(|(idx, (choice, label))| {
                                            let active = prefs.storage_mode == choice;
                                            render_choice_pill(
                                                ("storage-mode-btn", idx),
                                                label,
                                                active,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.set_storage_mode_preference(choice, cx);
                                                }),
                                            )
                                        }),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .p(px(24.0))
                            .rounded(px(10.0))
                            .bg(rgb(BG_SURFACE))
                            .border_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .flex()
                            .flex_col()
                            .gap(px(16.0))
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .font(editorial_serif())
                                            .text_size(px(28.0))
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child("Debrid & cached streams"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(13.5))
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(
                                                "Auto-detects Real-Debrid, AllDebrid, TorBox, or Premiumize from installed addon URLs, or lets you override Cached filter & Debrid stream visibility.",
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(8.0))
                                    .children(
                                        DebridAccessMode::ALL.into_iter().enumerate().map(
                                            |(idx, mode)| {
                                                let active = prefs.debrid_access_mode == mode;
                                                render_choice_pill(
                                                    ("debrid-mode-btn", idx),
                                                    mode.label(),
                                                    active,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.set_debrid_access_mode(mode, cx);
                                                    }),
                                                )
                                            },
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(if auto_debrid_detected {
                                        "Auto-detect status: Debrid credentials detected in installed addon configuration."
                                    } else {
                                        "Auto-detect status: No Debrid addon detected (unusable Debrid-only streams and the Cached filter are hidden unless overridden)."
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .grid()
                            .grid_cols(2)
                            .gap(px(16.0))
                            .child(
                                div()
                                    .p(px(22.0))
                                    .rounded(px(10.0))
                                    .bg(rgb(BG_SURFACE))
                                    .border_1()
                                    .border_color(rgba(BORDER_SUBTLE))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(16.0))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .flex()
                                            .flex_col()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                                    .text_size(px(15.0))
                                                    .text_color(rgb(TEXT_PRIMARY))
                                                    .child("UPnP / NAT-PMP port mapping"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(TEXT_SECONDARY))
                                                    .child(
                                                        "Maps inbound BitTorrent listener ports automatically on your router.",
                                                    ),
                                            ),
                                    )
                                    .child(render_movietracker_switch(
                                        "toggle-upnp",
                                        prefs.enable_upnp,
                                        cx.listener(|this, _, _, cx| {
                                            this.toggle_upnp_preference(cx);
                                        }),
                                    )),
                            )
                            .child(
                                div()
                                    .p(px(22.0))
                                    .rounded(px(10.0))
                                    .bg(rgb(BG_SURFACE))
                                    .border_1()
                                    .border_color(rgba(BORDER_SUBTLE))
                                    .flex()
                                    .flex_col()
                                    .gap(px(14.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap(px(16.0))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.0))
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(4.0))
                                                    .child(
                                                        div()
                                                            .font(ui_sans(FontWeight::SEMIBOLD))
                                                            .text_size(px(15.0))
                                                            .text_color(rgb(TEXT_PRIMARY))
                                                            .child("Dead-swarm auto-failover"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(rgb(TEXT_SECONDARY))
                                                            .child(
                                                                "Steps to the next ranked stream if a swarm stalls on startup.",
                                                            ),
                                                    ),
                                            )
                                            .child(render_movietracker_switch(
                                                "toggle-dead-swarm",
                                                prefs.dead_swarm_failover,
                                                cx.listener(|this, _, _, cx| {
                                                    this.preferences.dead_swarm_failover =
                                                        !this.preferences.dead_swarm_failover;
                                                    let _ = this.preferences.save();
                                                    cx.notify();
                                                }),
                                            )),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(TEXT_MUTED))
                                                    .child("Timeout"),
                                            )
                                            .children([6_u64, 10, 15, 20].into_iter().enumerate().map(
                                                |(idx, secs)| {
                                                    let active =
                                                        prefs.dead_swarm_timeout_secs == secs;
                                                    render_choice_pill(
                                                        ("failover-timeout-pill", idx),
                                                        format!("{secs}s"),
                                                        active,
                                                        cx.listener(move |this, _, _, cx| {
                                                            this.set_dead_swarm_timeout_secs(
                                                                secs, cx,
                                                            );
                                                        }),
                                                    )
                                                },
                                            )),
                                    ),
                            ),
                    )
                    .child(self.render_video_quality_settings_card(cx))
                    .child(self.render_addons_studio_bridge_card(cx)),
            )
    }

    fn render_software_updates_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let status = &self.update_status;
        let busy = matches!(
            status,
            UpdateStatus::Checking | UpdateStatus::Downloading { .. }
        );

        let status_body = match status {
            UpdateStatus::Idle => div()
                .text_xs()
                .text_color(rgb(TEXT_MUTED))
                .child("Releases are verified over HTTPS against SHA256SUMS.txt from Gaalix/ferrite.")
                .into_any_element(),
            UpdateStatus::Checking => div()
                .text_xs()
                .text_color(rgb(TEXT_SECONDARY))
                .child("Checking GitHub Releases for a newer version...")
                .into_any_element(),
            UpdateStatus::UpToDate { checked_version } => div()
                .text_xs()
                .text_color(rgb(STATUS_WATCHED))
                .child(format!(
                    "Ferrite v{checked_version} is the latest release available."
                ))
                .into_any_element(),
            UpdateStatus::Available(info) => {
                let btn_label = info.target_kind.action_button_label();
                let asset_meta = format!(
                    "{} ({})",
                    info.target_asset.name,
                    format_bytes(info.target_asset.size_bytes)
                );
                div()
                    .p(px(16.0))
                    .rounded(px(8.0))
                    .bg(rgb(BG_ELEVATED))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(px(14.5))
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .child(format!("{} (v{})", info.release_name, info.version)),
                            )
                            .child(neutral_chip("SHA-256 verified")),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(info.notes_summary.clone()),
                    )
                    .child(
                        div()
                            .pt(px(2.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .font(mono_font())
                                    .text_xs()
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(asset_meta),
                            )
                            .child(
                                div()
                                    .id("btn-install-update")
                                    .cursor_pointer()
                                    .h(px(36.0))
                                    .px(px(16.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(ACCENT))
                                    .hover(|s| s.bg(rgb(ACCENT_HOVER)))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .font(ui_sans(FontWeight::SEMIBOLD))
                                    .text_size(px(13.0))
                                    .text_color(rgb(BG_OBSIDIAN))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.install_available_update(cx);
                                        }),
                                    )
                                    .child(svg_icon(IconName::RefreshCw, 13.0, rgb(BG_OBSIDIAN)))
                                    .child(btn_label),
                            ),
                    )
                    .into_any_element()
            }
            UpdateStatus::Downloading { version } => div()
                .text_xs()
                .text_color(rgb(ACCENT))
                .child(format!(
                    "Downloading and verifying SHA-256 checksum for Ferrite v{version}..."
                ))
                .into_any_element(),
            UpdateStatus::ReadyToRestart { version } => div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(14.0))
                .child(
                    div()
                        .text_size(px(13.5))
                        .text_color(rgb(STATUS_WATCHED))
                        .child(format!(
                            "Ferrite v{version} has been verified and installed. Restart to finish updating."
                        )),
                )
                .child(
                    div()
                        .id("btn-restart-update")
                        .cursor_pointer()
                        .h(px(36.0))
                        .px(px(16.0))
                        .rounded(px(6.0))
                        .bg(rgb(ACCENT))
                        .hover(|s| s.bg(rgb(ACCENT_HOVER)))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .font(ui_sans(FontWeight::SEMIBOLD))
                        .text_size(px(13.0))
                        .text_color(rgb(BG_OBSIDIAN))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.restart_into_updated_binary(cx);
                            }),
                        )
                        .child("Restart to apply"),
                )
                .into_any_element(),
            UpdateStatus::DebReady {
                version,
                deb_path,
                install_cmd,
            } => {
                let cmd_for_copy = install_cmd.clone();
                let deb_for_open = deb_path.clone();
                div()
                    .p(px(14.0))
                    .rounded(px(8.0))
                    .bg(rgb(BG_ELEVATED))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(rgb(STATUS_WATCHED))
                            .child(format!(
                                "Verified Ferrite v{version} Debian package saved to user cache."
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .font(mono_font())
                                    .text_xs()
                                    .text_color(rgb(TEXT_PRIMARY))
                                    .child(install_cmd.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(render_choice_pill(
                                        "btn-copy-deb-cmd",
                                        "Copy apt command",
                                        false,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                cmd_for_copy.clone(),
                                            ));
                                            this.push_toast(
                                                ToastKind::Success,
                                                "Copied apt install command",
                                                None,
                                                cx,
                                            );
                                        }),
                                    ))
                                    .child(render_choice_pill(
                                        "btn-open-deb",
                                        "Open package",
                                        true,
                                        cx.listener(move |this, _, _, cx| {
                                            if let Err(err) =
                                                updater::open_deb_package_with_system(&deb_for_open)
                                            {
                                                this.push_toast(
                                                    ToastKind::Warning,
                                                    "Package opener unavailable",
                                                    Some(err),
                                                    cx,
                                                );
                                            }
                                        }),
                                    )),
                            ),
                    )
                    .into_any_element()
            }
            UpdateStatus::ApplyingWindowsInstaller { version } => div()
                .text_xs()
                .text_color(rgb(ACCENT))
                .child(format!("Launching Ferrite v{version} setup..."))
                .into_any_element(),
            UpdateStatus::Failed(reason) => div()
                .text_xs()
                .text_color(rgb(ACCENT_ROSE))
                .child(reason.clone())
                .into_any_element(),
        };

        div()
            .p(px(24.0))
            .rounded(px(10.0))
            .bg(rgb(BG_SURFACE))
            .border_1()
            .border_color(rgba(BORDER_SUBTLE))
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .font(editorial_serif())
                                            .text_size(px(28.0))
                                            .text_color(rgb(TEXT_PRIMARY))
                                            .child("Software updates"),
                                    )
                                    .child(neutral_chip(format!("v{CURRENT_APP_VERSION}"))),
                            )
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(
                                        "Signed-checksum updates from github.com/Gaalix/ferrite.",
                                    ),
                            ),
                    )
                    .child(render_choice_pill(
                        "btn-check-updates",
                        if busy {
                            "Working..."
                        } else {
                            "Check for updates"
                        },
                        false,
                        cx.listener(move |this, _, _, cx| {
                            if !busy {
                                this.trigger_update_check(true, cx);
                            }
                        }),
                    )),
            )
            .child(status_body)
    }

    fn render_video_quality_settings_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = &self.preferences;
        let exp = prefs.experimental_video;

        div()
            .p(px(24.0))
            .rounded(px(10.0))
            .bg(rgb(BG_SURFACE))
            .border_1()
            .border_color(rgba(BORDER_SUBTLE))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .font(editorial_serif())
                            .text_size(px(28.0))
                            .text_color(rgb(TEXT_PRIMARY))
                            .child("Video pipeline & experimental shaders"),
                    )
                    .child(
                        div()
                            .text_size(px(13.5))
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(
                                "Experimental GLSL shaders, debanding, and Dolby Vision reshape options remain off by default.",
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .w(px(110.0))
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
                            .child("HDR tone-map"),
                    )
                    .children(ToneMappingChoice::ALL.into_iter().enumerate().map(
                        |(idx, choice)| {
                            let active = prefs.tone_mapping == choice;
                            render_choice_pill(
                                ("settings-tm-pill", idx),
                                choice.label(),
                                active,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_tone_mapping_choice(choice, cx);
                                }),
                            )
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .w(px(110.0))
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
                            .child("GPU upscaler"),
                    )
                    .children(UpscaleChoice::ALL.into_iter().enumerate().map(
                        |(idx, choice)| {
                            let active = prefs.upscale_mode == choice;
                            render_choice_pill(
                                ("settings-upscale-pill", idx),
                                choice.label(),
                                active,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_upscale_choice(choice, cx);
                                }),
                            )
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .w(px(110.0))
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
                            .child("GLSL shader"),
                    )
                    .children(ShaderPresetChoice::ALL.into_iter().enumerate().map(
                        |(idx, preset)| {
                            let active = exp.shader_preset == preset;
                            render_choice_pill(
                                ("settings-shader-pill", idx),
                                preset.label(),
                                active,
                                cx.listener(move |this, _, _, cx| {
                                    this.preferences.experimental_video.shader_preset = preset;
                                    this.apply_experimental_video_to_mpv();
                                    let _ = this.preferences.save();
                                    cx.notify();
                                }),
                            )
                        },
                    )),
            )
            .child(
                div()
                    .pt(px(4.0))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(10.0))
                    .child(render_choice_pill(
                        "exp-toggle-deband",
                        if exp.deband_enabled {
                            "Dark-scene debanding: On"
                        } else {
                            "Dark-scene debanding: Off"
                        },
                        exp.deband_enabled,
                        cx.listener(|this, _, _, cx| {
                            this.preferences.experimental_video.deband_enabled =
                                !this.preferences.experimental_video.deband_enabled;
                            this.apply_experimental_video_to_mpv();
                            let _ = this.preferences.save();
                            cx.notify();
                        }),
                    ))
                    .child(render_choice_pill(
                        "exp-toggle-dv",
                        if exp.dolby_vision_reshape {
                            "Dolby Vision reshape: On"
                        } else {
                            "Dolby Vision reshape: Off"
                        },
                        exp.dolby_vision_reshape,
                        cx.listener(|this, _, _, cx| {
                            this.preferences.experimental_video.dolby_vision_reshape =
                                !this.preferences.experimental_video.dolby_vision_reshape;
                            this.apply_experimental_video_to_mpv();
                            let _ = this.preferences.save();
                            cx.notify();
                        }),
                    ))
                    .child(render_choice_pill(
                        "exp-toggle-peak",
                        if exp.hdr_compute_peak {
                            "Dynamic HDR peak: On"
                        } else {
                            "Dynamic HDR peak: Off"
                        },
                        exp.hdr_compute_peak,
                        cx.listener(|this, _, _, cx| {
                            this.preferences.experimental_video.hdr_compute_peak =
                                !this.preferences.experimental_video.hdr_compute_peak;
                            this.apply_experimental_video_to_mpv();
                            let _ = this.preferences.save();
                            cx.notify();
                        }),
                    )),
            )
    }

    fn render_addons_studio_bridge_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active_count = self.core.with_model(|m| m.ctx.profile.addons.len());
        let muted_count = self.core.list_muted_addons().len();
        let cache_stats = self.core.addon_cache_stats();

        div()
            .p(px(24.0))
            .rounded(px(10.0))
            .bg(rgb(BG_SURFACE))
            .border_1()
            .border_color(rgba(BORDER_SUBTLE))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(20.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .font(editorial_serif())
                            .text_size(px(28.0))
                            .text_color(rgb(TEXT_PRIMARY))
                            .child("Addons Studio"),
                    )
                    .child(
                        div()
                            .text_size(px(13.5))
                            .text_color(rgb(TEXT_SECONDARY))
                            .child(
                                "Reorder execution priority, soft-mute Debrid sources without losing config URLs, or browse the curated directory.",
                            ),
                    )
                    .child(
                        div()
                            .pt(px(2.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .font(mono_font())
                            .text_xs()
                            .text_color(rgb(TEXT_MUTED))
                            .child(format!("{active_count} active"))
                            .child(meta_divider())
                            .child(format!("{muted_count} muted"))
                            .child(meta_divider())
                            .child(format!("{} HTTP cache hits", cache_stats.hits)),
                    ),
            )
            .child(
                div()
                    .id("btn-open-addons-studio")
                    .cursor_pointer()
                    .h(px(40.0))
                    .px(px(18.0))
                    .rounded(px(6.0))
                    .bg(rgb(ACCENT))
                    .hover(|s| s.bg(rgb(ACCENT_HOVER)))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .font(ui_sans(FontWeight::SEMIBOLD))
                    .text_size(px(13.5))
                    .text_color(rgb(BG_OBSIDIAN))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.navigate_tab(SidebarTab::Addons, cx);
                        }),
                    )
                    .child(svg_icon(IconName::Layers, 14.0, rgb(BG_OBSIDIAN)))
                    .child("Open Addons Studio"),
            )
    }
}

fn render_metric_card(label: &'static str, value: String, sub: String) -> impl IntoElement {
    div()
        .p(px(16.0))
        .rounded(px(8.0))
        .bg(rgb(BG_SURFACE))
        .border_1()
        .border_color(rgba(BORDER_SUBTLE))
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(div().text_xs().text_color(rgb(TEXT_MUTED)).child(label))
        .child(
            div()
                .font(mono_font())
                .font_weight(FontWeight::BOLD)
                .text_size(px(16.0))
                .text_color(rgb(TEXT_PRIMARY))
                .truncate()
                .child(value),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(TEXT_SECONDARY))
                .truncate()
                .child(sub),
        )
}

pub(crate) fn render_choice_pill(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .cursor_pointer()
        .h(px(32.0))
        .px(px(13.0))
        .rounded(px(999.0))
        .bg(if active {
            rgba(ACCENT_SOFT)
        } else {
            rgb(BG_ELEVATED)
        })
        .border_1()
        .border_color(if active {
            rgb(ACCENT)
        } else {
            rgba(BORDER_SUBTLE)
        })
        .hover(|s| {
            s.border_color(if active {
                rgb(ACCENT)
            } else {
                rgba(BORDER_STRONG)
            })
        })
        .flex()
        .items_center()
        .font(ui_sans(FontWeight::MEDIUM))
        .text_size(px(12.5))
        .text_color(if active {
            rgb(ACCENT)
        } else {
            rgb(TEXT_SECONDARY)
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.into())
}

pub(crate) fn render_movietracker_switch(
    id: impl Into<gpui::ElementId>,
    enabled: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .cursor_pointer()
        .w(px(40.0))
        .h(px(22.0))
        .p(px(3.0))
        .rounded(px(999.0))
        .bg(if enabled {
            rgb(ACCENT)
        } else {
            rgb(BG_SURFACE_3)
        })
        .flex()
        .items_center()
        .when(enabled, |el| el.justify_end())
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            div()
                .w(px(16.0))
                .h(px(16.0))
                .rounded(px(999.0))
                .bg(if enabled {
                    rgb(BG_OBSIDIAN)
                } else {
                    rgb(TEXT_PRIMARY)
                }),
        )
}
