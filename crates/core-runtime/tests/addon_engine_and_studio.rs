use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use http::Request;
use serde::Deserialize;
use stremio_core::{
    constants::{CATALOG_RESOURCE_NAME, STREAM_RESOURCE_NAME, SUBTITLES_RESOURCE_NAME},
    runtime::{Env, RuntimeEvent},
    types::addon::{Descriptor, DescriptorFlags, Manifest, ManifestResource, Version},
};
use stremio_core_runtime::{
    addon_configure_url, classify_descriptor_categories, curated_addon_directory,
    detect_clipboard_addon_url, normalize_addon_url, AddonCategory, AddonHealthStatus,
    CoreController, DesktopEnv, DesktopEnvConfig, DesktopModelField,
};
use tempfile::tempdir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct MockStreamPayload {
    streams: Vec<serde_json::Value>,
}

struct MockAddonServer {
    base_url: Url,
    request_count: Arc<AtomicUsize>,
    fail_mode: Arc<AtomicBool>,
}

async fn spawn_mock_addon_server(artificial_delay: Duration) -> MockAddonServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url: Url = format!("http://{addr}/").parse().unwrap();
    let request_count = Arc::new(AtomicUsize::new(0));
    let fail_mode = Arc::new(AtomicBool::new(false));

    let count_clone = Arc::clone(&request_count);
    let fail_clone = Arc::clone(&fail_mode);

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let count_clone = Arc::clone(&count_clone);
            let fail_clone = Arc::clone(&fail_clone);

            tokio::spawn(async move {
                let mut buf = vec![0_u8; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                count_clone.fetch_add(1, Ordering::SeqCst);

                let req_text = String::from_utf8_lossy(&buf[..n]);
                let raw_path = req_text
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");

                if !artificial_delay.is_zero() {
                    tokio::time::sleep(artificial_delay).await;
                }

                if fail_clone.load(Ordering::SeqCst) {
                    let body = r#"{"error":"upstream overloaded"}"#;
                    let resp = format!(
                        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    let _ = socket.shutdown().await;
                    return;
                }

                let body = if raw_path.contains("/token_v1/manifest.json") {
                    serde_json::json!({
                        "id": "stremio.comet.fast",
                        "version": "1.0.0",
                        "name": "Comet [RD v1]",
                        "description": "Configured Comet v1",
                        "resources": ["stream"],
                        "types": ["movie", "series"],
                        "catalogs": []
                    })
                    .to_string()
                } else if raw_path.contains("/token_v2/manifest.json") {
                    serde_json::json!({
                        "id": "stremio.comet.fast",
                        "version": "2.1.0",
                        "name": "Comet [RD+TorBox v2]",
                        "description": "Upgraded Comet v2",
                        "resources": ["stream"],
                        "types": ["movie", "series"],
                        "catalogs": []
                    })
                    .to_string()
                } else if raw_path.ends_with("/manifest.json") {
                    serde_json::json!({
                        "id": "org.mock.streams",
                        "version": "1.2.3",
                        "name": "Mock Streams Addon",
                        "description": "Local test addon",
                        "resources": ["stream", "subtitles"],
                        "types": ["movie", "series"],
                        "catalogs": []
                    })
                    .to_string()
                } else {
                    serde_json::json!({
                        "streams": [
                            {
                                "name": "Comet [RD+] 4K",
                                "title": "Dune.Part.Two.2024.2160p.WEB-DL.DDP5.1.Atmos.DV.HDR.H.265",
                                "url": "https://cdn.example.com/dune2.mkv"
                            }
                        ]
                    })
                    .to_string()
                };

                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    MockAddonServer {
        base_url,
        request_count,
        fail_mode,
    }
}

fn make_custom_descriptor(id: &str, name: &str, url: &str, resources: &[&str]) -> Descriptor {
    Descriptor {
        manifest: Manifest {
            id: id.to_owned(),
            version: Version::new(1, 0, 0),
            name: name.to_owned(),
            contact_email: None,
            description: Some(format!("{name} description")),
            logo: None,
            background: None,
            types: vec!["movie".to_owned(), "series".to_owned()],
            resources: resources
                .iter()
                .map(|r| ManifestResource::Short((*r).to_owned()))
                .collect(),
            id_prefixes: None,
            catalogs: vec![],
            addon_catalogs: vec![],
            behavior_hints: Default::default(),
        },
        transport_url: Url::parse(url).unwrap(),
        flags: DescriptorFlags::default(),
    }
}

#[test]
fn url_normalization_configure_mapping_and_curated_directory_work() {
    let u1 =
        normalize_addon_url("  \"stremio://comet.elfhosted.com/eyJapiKeyIjoiabc123/configure\" ")
            .expect("should normalize stremio:// configure URL");
    assert_eq!(
        u1.as_str(),
        "https://comet.elfhosted.com/eyJapiKeyIjoiabc123/manifest.json"
    );

    let cfg_url = addon_configure_url(&u1).expect("should map manifest.json to configure");
    assert_eq!(
        cfg_url.as_str(),
        "https://comet.elfhosted.com/eyJapiKeyIjoiabc123/configure"
    );

    let u2 = normalize_addon_url("stremio://127.0.0.1:7000").expect("loopback stremio://");
    assert_eq!(u2.as_str(), "http://127.0.0.1:7000/manifest.json");

    assert_eq!(
        detect_clipboard_addon_url("stremio://mediafusion.elfhosted.com/secret_cfg/manifest.json")
            .as_ref()
            .map(Url::as_str),
        Some("https://mediafusion.elfhosted.com/secret_cfg/manifest.json")
    );
    assert!(detect_clipboard_addon_url("just normal text in clipboard").is_none());

    let directory = curated_addon_directory();
    assert!(directory.len() >= 10);
    assert!(directory
        .iter()
        .any(|e| e.category == AddonCategory::Streams));
    assert!(directory
        .iter()
        .any(|e| e.category == AddonCategory::Subtitles));
    assert!(directory
        .iter()
        .any(|e| e.category == AddonCategory::Catalogs));
    assert!(directory
        .iter()
        .any(|e| e.category == AddonCategory::Metadata));

    let desc = make_custom_descriptor(
        "org.example.multi",
        "Multi Addon",
        "https://example.com/manifest.json",
        &[
            STREAM_RESOURCE_NAME,
            SUBTITLES_RESOURCE_NAME,
            CATALOG_RESOURCE_NAME,
        ],
    );
    let cats = classify_descriptor_categories(&desc);
    assert!(cats.contains(&AddonCategory::Streams));
    assert!(cats.contains(&AddonCategory::Subtitles));
    assert!(cats.contains(&AddonCategory::Catalogs));
}

#[tokio::test]
async fn ttl_cache_and_singleflight_coalescing_deduplicate_identical_gets() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("cache_coalesce.db");
    let mock = spawn_mock_addon_server(Duration::from_millis(45)).await;
    let dummy_ss: Url = "http://127.0.0.1:1/".parse().unwrap();

    let config = DesktopEnvConfig::with_storage_path(&db_path)
        .with_streaming_server_url(dummy_ss)
        .with_loopback_addon_cache_and_circuit(true);

    let stream_url = format!("{}stream/movie/tt15239678.json", mock.base_url);

    DesktopEnv::run_isolated(config, async move {
        let controller = CoreController::from_initialized_env().await.unwrap();

        let mut handles = Vec::new();
        for _ in 0..10 {
            let url = stream_url.clone();
            handles.push(async move {
                let req = Request::get(&url).body(()).unwrap();
                DesktopEnv::fetch::<(), MockStreamPayload>(req).await
            });
        }

        let results = futures::future::join_all(handles).await;
        for res in &results {
            let payload = res.as_ref().expect("coalesced request must succeed");
            assert_eq!(payload.streams.len(), 1);
        }

        assert_eq!(
            mock.request_count.load(Ordering::SeqCst),
            1,
            "10 concurrent identical GETs must coalesce into exactly 1 network fetch"
        );

        let stats_after_coalesce = controller.addon_cache_stats();
        assert_eq!(stats_after_coalesce.entries, 1);
        assert_eq!(stats_after_coalesce.misses, 1);
        assert_eq!(stats_after_coalesce.coalesced, 9);

        let snap1 = controller.addon_telemetry_for(&mock.base_url);
        let initial_ema = snap1
            .ema_latency_ms
            .expect("network fetch must record EMA RTT");
        assert!(initial_ema >= 30);
        assert_eq!(snap1.status, AddonHealthStatus::Healthy);

        let cached_req = Request::get(&stream_url).body(()).unwrap();
        let cached_payload: MockStreamPayload = DesktopEnv::fetch(cached_req).await.unwrap();
        assert_eq!(cached_payload.streams.len(), 1);
        assert_eq!(mock.request_count.load(Ordering::SeqCst), 1);

        let stats_after_hit = controller.addon_cache_stats();
        assert_eq!(stats_after_hit.hits, 1);

        let snap2 = controller.addon_telemetry_for(&mock.base_url);
        assert_eq!(snap2.cache_hits, 1);
        assert_eq!(
            snap2.ema_latency_ms,
            Some(initial_ema),
            "cache hit must not skew network EMA RTT"
        );

        controller.clear_addon_cache();
        assert_eq!(controller.addon_cache_stats().entries, 0);

        let fresh_req = Request::get(&stream_url).body(()).unwrap();
        let _: MockStreamPayload = DesktopEnv::fetch(fresh_req).await.unwrap();
        assert_eq!(mock.request_count.load(Ordering::SeqCst), 2);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn soft_circuit_breaker_trips_after_three_failures_fast_fails_and_resets() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("circuit_breaker.db");
    let mock = spawn_mock_addon_server(Duration::from_millis(25)).await;
    mock.fail_mode.store(true, Ordering::SeqCst);

    let dummy_ss: Url = "http://127.0.0.1:1/".parse().unwrap();
    let config = DesktopEnvConfig::with_storage_path(&db_path)
        .with_streaming_server_url(dummy_ss)
        .with_loopback_addon_cache_and_circuit(true)
        .with_circuit_breaker_cooldown(Duration::from_secs(30));

    let endpoint = format!("{}rd_secret_key_xyz/stream/movie/tt0133093.json", mock.base_url);

    DesktopEnv::run_isolated(config, async move {
        let controller = CoreController::from_initialized_env().await.unwrap();

        for attempt in 1..=3 {
            let req = Request::get(&endpoint).body(()).unwrap();
            let err = DesktopEnv::fetch::<(), MockStreamPayload>(req)
                .await
                .expect_err("502 should fail");
            assert!(!err.to_string().contains("rd_secret_key_xyz"));
            let snap = controller.addon_telemetry_for(&mock.base_url);
            assert_eq!(snap.consecutive_failures, attempt, "err={err:?}");
            if let Some(ref last_err) = snap.last_error {
                assert!(!last_err.contains("rd_secret_key_xyz"));
            }
            if attempt < 3 {
                assert_eq!(snap.status, AddonHealthStatus::Degraded);
            } else {
                assert!(matches!(
                    snap.status,
                    AddonHealthStatus::CircuitOpen { remaining_secs } if remaining_secs >= 1
                ));
            }
        }
        assert_eq!(mock.request_count.load(Ordering::SeqCst), 3);

        let start = Instant::now();
        let req4 = Request::get(&endpoint).body(()).unwrap();
        let fast_err = DesktopEnv::fetch::<(), MockStreamPayload>(req4)
            .await
            .expect_err("open circuit must fast-fail");
        assert!(
            start.elapsed() < Duration::from_millis(10),
            "circuit open fast-fail took {:?}",
            start.elapsed()
        );
        assert!(fast_err.to_string().contains("circuit-breaker open"));
        assert!(!fast_err.to_string().contains("rd_secret_key_xyz"));
        assert_eq!(
            mock.request_count.load(Ordering::SeqCst),
            3,
            "open circuit must not hit upstream server"
        );

        let failed_install_err = controller
            .install_or_replace_addon_by_url(&format!(
                "{}rd_secret_key_xyz/manifest.json",
                mock.base_url
            ))
            .await
            .expect_err("502 manifest install must fail");
        assert!(
            !failed_install_err.contains("rd_secret_key_xyz"),
            "install error must redact secret path segments: {failed_install_err}"
        );

        mock.fail_mode.store(false, Ordering::SeqCst);
        assert!(controller.reset_addon_circuit(mock.base_url.as_str()));

        let req5 = Request::get(&endpoint).body(()).unwrap();
        let ok_payload: MockStreamPayload = DesktopEnv::fetch(req5)
            .await
            .expect("request after circuit reset should succeed");
        assert_eq!(ok_payload.streams.len(), 1);

        let snap_recovered = controller.addon_telemetry_for(&mock.base_url);
        assert_eq!(snap_recovered.consecutive_failures, 0);
        assert_eq!(snap_recovered.status, AddonHealthStatus::Healthy);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_place_upgrade_reordering_and_soft_mute_persist_across_reopen() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("studio_addons.db");
    let mock = spawn_mock_addon_server(Duration::from_millis(5)).await;
    let host_port = format!(
        "{}:{}",
        mock.base_url.host_str().unwrap(),
        mock.base_url.port().unwrap()
    );
    let dummy_ss: Url = "http://127.0.0.1:1/".parse().unwrap();

    let config1 = DesktopEnvConfig::with_storage_path(&db_path)
        .with_streaming_server_url(dummy_ss.clone())
        .with_loopback_addon_cache_and_circuit(true);

    let alpha_url = "https://alpha.addons.example/manifest.json";
    let omega_url = "https://omega.addons.example/manifest.json";
    let expected_comet_v2_url = format!("{}token_v2/manifest.json", mock.base_url);

    let expected_comet_v2_url_clone = expected_comet_v2_url.clone();
    DesktopEnv::run_isolated(config1, async move {
        let controller = CoreController::from_initialized_env().await.unwrap();
        controller.load_installed_addons(None);
        let mut events_rx = controller.subscribe();

        let alpha = make_custom_descriptor("org.alpha", "Alpha Catalog", alpha_url, &["catalog"]);
        let omega =
            make_custom_descriptor("org.omega", "Omega Subtitles", omega_url, &["subtitles"]);

        controller
            .install_or_replace_addon_descriptor(alpha)
            .expect("install alpha");

        let comet_v1_stremio_url = format!("stremio://{host_port}/token_v1/configure");
        let v1_outcome = controller
            .install_or_replace_addon_by_url(&comet_v1_stremio_url)
            .await
            .expect("install comet v1 from stremio:// configure URL");
        assert!(!v1_outcome.replaced_existing);
        assert_eq!(v1_outcome.descriptor.manifest.name, "Comet [RD v1]");
        let comet_slot = v1_outcome.slot_index;

        controller
            .install_or_replace_addon_descriptor(omega)
            .expect("install omega");

        let total_addons = controller.with_model(|m| m.ctx.profile.addons.len());
        assert_eq!(comet_slot + 2, total_addons, "Comet should be before Omega");

        let ctrl_for_gpui_thread = controller.clone();
        let comet_v2_stremio_url = format!("stremio://{host_port}/token_v2/manifest.json");
        let v2_outcome = std::thread::spawn(move || {
            futures::executor::block_on(
                ctrl_for_gpui_thread.install_or_replace_addon_by_url(&comet_v2_stremio_url),
            )
        })
        .join()
        .expect("GPUI non-Tokio thread must not panic")
        .expect("in-place upgrade must succeed");

        assert!(v2_outcome.replaced_existing);
        assert!(!v2_outcome.was_muted);
        assert_eq!(
            v2_outcome.slot_index, comet_slot,
            "same manifest.id must upgrade IN-PLACE at exact priority slot"
        );
        assert_eq!(
            v2_outcome.descriptor.transport_url.as_str(),
            expected_comet_v2_url_clone
        );
        assert_eq!(
            controller.with_model(|m| m.ctx.profile.addons.len()),
            total_addons,
            "in-place upgrade must not append duplicates"
        );
        assert_eq!(
            controller.with_model(|m| m.ctx.profile.addons[comet_slot].manifest.name.clone()),
            "Comet [RD+TorBox v2]"
        );

        let telemetry_map = controller.addon_telemetry();
        assert!(telemetry_map.contains_key(&expected_comet_v2_url_clone));

        let comet_url: Url = expected_comet_v2_url_clone.parse().unwrap();
        assert!(controller.move_addon_to_top(&comet_url));
        assert_eq!(
            controller.with_model(|m| m.ctx.profile.addons[0].transport_url.clone()),
            comet_url
        );
        assert_eq!(
            controller.with_model(|m| m.installed_addons.catalog[0].transport_url.clone()),
            comet_url,
            "installed_addons model must stay synchronously aligned with profile.addons"
        );
        assert!(!controller.move_addon_up(&comet_url), "already at index 0");

        assert!(controller.move_addon_down(&comet_url));
        assert_eq!(
            controller.with_model(|m| m.ctx.profile.addons[1].transport_url.clone()),
            comet_url
        );

        assert!(controller.move_addon_up(&comet_url));
        assert_eq!(
            controller.with_model(|m| m.ctx.profile.addons[0].transport_url.clone()),
            comet_url
        );

        let mut saw_new_state = false;
        while let Ok(ev) = events_rx.try_recv() {
            if matches!(ev.as_ref(), RuntimeEvent::NewState(fields, ..) if fields.contains(&DesktopModelField::Ctx))
            {
                saw_new_state = true;
            }
        }
        assert!(
            saw_new_state,
            "addon mutations must emit RuntimeEvent::NewState"
        );

        let protected_url = controller.with_model(|m| {
            m.ctx
                .profile
                .addons
                .iter()
                .find(|a| a.flags.protected)
                .map(|a| a.transport_url.clone())
                .expect("profile should include protected Cinemeta")
        });
        assert!(
            controller.set_addon_enabled(&protected_url, false).is_err(),
            "protected core addon must reject soft-disable"
        );

        let unknown_secret_url =
            Url::parse("https://comet.elfhosted.com/super_secret_rd_key/manifest.json").unwrap();
        let not_installed_err = controller
            .set_addon_enabled(&unknown_secret_url, false)
            .expect_err("unknown addon must fail");
        assert!(!not_installed_err.contains("super_secret_rd_key"));

        assert!(controller.move_addon_down(&comet_url));
        assert!(controller.move_addon_down(&comet_url));
        assert_eq!(
            controller.with_model(|m| m.ctx.profile.addons[2].transport_url.clone()),
            comet_url
        );

        assert!(
            controller.has_debrid_addon_configured(),
            "active Comet [RD+TorBox v2] must auto-detect Debrid access"
        );

        controller
            .set_addon_enabled(&comet_url, false)
            .expect("soft-disable configured Comet");
        assert!(
            !controller.has_debrid_addon_configured(),
            "soft-muted Debrid addon must not report active Debrid access"
        );
        assert!(
            controller.with_model(|m| !m.ctx.profile.addons.iter().any(|a| a.transport_url == comet_url)),
            "muted addon must be absent from active profile.addons"
        );
        let muted = controller.list_muted_addons();
        assert_eq!(muted.len(), 1);
        assert_eq!(muted[0].descriptor.transport_url, comet_url);
        assert_eq!(muted[0].preferred_index, 2);
    })
    .await
    .unwrap();

    let config2 = DesktopEnvConfig::with_storage_path(&db_path)
        .with_streaming_server_url(dummy_ss)
        .with_loopback_addon_cache_and_circuit(true);

    DesktopEnv::run_isolated(config2, async move {
        let controller = CoreController::from_initialized_env().await.unwrap();
        let comet_url: Url = expected_comet_v2_url.parse().unwrap();

        let muted = controller.list_muted_addons();
        assert_eq!(muted.len(), 1);
        assert_eq!(muted[0].descriptor.transport_url, comet_url);
        assert_eq!(muted[0].descriptor.manifest.name, "Comet [RD+TorBox v2]");
        assert_eq!(muted[0].preferred_index, 2);

        controller
            .set_addon_enabled(&comet_url, true)
            .expect("re-enable muted Comet");
        assert!(controller.list_muted_addons().is_empty());
        assert_eq!(
            controller.with_model(|m| m.ctx.profile.addons[2].transport_url.clone()),
            comet_url,
            "unmuting must restore secret config URL at its preferred priority slot"
        );
    })
    .await
    .unwrap();
}
