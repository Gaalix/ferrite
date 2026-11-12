use std::time::Duration;

use stremio_core::{
    constants::{LIBRARY_RECENT_STORAGE_KEY, META_RESOURCE_NAME, STREAM_RESOURCE_NAME},
    models::{common::Loadable, player::Selected as PlayerSelected},
    runtime::{msg::Event, Env, RuntimeEvent},
    types::{
        addon::{
            Descriptor, ExtraProp, Manifest, ManifestCatalog, ManifestExtra, ManifestResource,
            OptionsLimit, ResourcePath, ResourceRequest, Version,
        },
        library::LibraryBucket,
        resource::{MetaItemPreview, PosterShape, Stream, StreamBehaviorHints, StreamSource},
    },
};
use stremio_core_runtime::{
    parse_stream, CoreController, DesktopEnv, DesktopModelField, DesktopRuntimeEvent, EnvConfig,
    HdrFormat, Resolution, StreamQuality, VideoCodec, INITIAL_BOARD_SHELVES,
};
use tempfile::tempdir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

async fn spawn_mock_stremio_backend() -> Url {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url: Url = format!("http://{addr}/").parse().unwrap();
    let server_base = base_url.clone();

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let server_base = server_base.clone();
            tokio::spawn(async move {
                let mut buf = vec![0_u8; 16384];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let req_text = String::from_utf8_lossy(&buf[..n]);
                let path = req_text
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");

                let body = if path == "/settings" {
                    serde_json::json!({
                        "baseUrl": server_base.as_str(),
                        "values": {
                            "appPath": "/tmp/ferrite",
                            "cacheRoot": "/tmp/ferrite/cache",
                            "serverVersion": "4.20.8",
                            "remoteHttps": "",
                            "transcodeProfile": null,
                            "cacheSize": 2147483648.0,
                            "proxyStreamsEnabled": false,
                            "btMaxConnections": 55,
                            "btHandshakeTimeout": 20000,
                            "btRequestTimeout": 4000,
                            "btDownloadSpeedSoftLimit": 2621440.0,
                            "btDownloadSpeedHardLimit": 39321600.0,
                            "btMinPeersForStable": 5
                        },
                        "options": []
                    })
                    .to_string()
                } else if path == "/device-info" {
                    serde_json::json!({
                        "availableHardwareAccelerations": ["vaapi", "vulkan"]
                    })
                    .to_string()
                } else if path == "/network-info" {
                    serde_json::json!({
                        "availableInterfaces": ["127.0.0.1"]
                    })
                    .to_string()
                } else if path == "/casting" {
                    serde_json::json!([]).to_string()
                } else if path.starts_with("/addon/catalog/movie/mock_top") {
                    serde_json::json!({
                        "metas": [
                            {
                                "id": "tt0133093",
                                "type": "movie",
                                "name": "The Matrix",
                                "poster": null,
                                "posterShape": "poster"
                            }
                        ]
                    })
                    .to_string()
                } else if path == "/addon/meta/movie/tt0133093.json" {
                    serde_json::json!({
                        "meta": {
                            "id": "tt0133093",
                            "type": "movie",
                            "name": "The Matrix",
                            "poster": null,
                            "posterShape": "poster",
                            "videos": []
                        }
                    })
                    .to_string()
                } else if path == "/addon/stream/movie/tt0133093.json" {
                    serde_json::json!({
                        "streams": [
                            {
                                "url": format!("{}video.mkv", server_base),
                                "name": "[RD+] Torrentio\n4k DV",
                                "description": "The.Matrix.1999.2160p.UHD.BluRay.REMUX.DV.HEVC.TrueHD.7.1.Atmos-GROUP\n👤 512 💾 62.4 GB",
                                "behaviorHints": {
                                    "filename": "The.Matrix.1999.2160p.UHD.BluRay.REMUX.DV.HEVC.TrueHD.7.1.Atmos-GROUP.mkv",
                                    "videoSize": 67001489817_u64
                                }
                            }
                        ]
                    })
                    .to_string()
                } else {
                    serde_json::json!({}).to_string()
                };

                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            });
        }
    });

    base_url
}

fn build_mock_addon_descriptor(base_url: &Url) -> Descriptor {
    Descriptor {
        manifest: Manifest {
            id: "io.ferrite.mock".to_owned(),
            version: Version::new(1, 0, 0),
            name: "Ferrite Mock Addon".to_owned(),
            contact_email: None,
            description: Some("Local mock addon for integration tests".to_owned()),
            logo: None,
            background: None,
            types: vec!["movie".to_owned()],
            resources: vec![
                ManifestResource::Short("catalog".to_owned()),
                ManifestResource::Short("meta".to_owned()),
                ManifestResource::Short("stream".to_owned()),
            ],
            id_prefixes: Some(vec!["tt".to_owned()]),
            catalogs: vec![ManifestCatalog {
                r#type: "movie".to_owned(),
                id: "mock_top".to_owned(),
                name: Some("Mock Top Movies".to_owned()),
                extra: ManifestExtra::Full {
                    props: vec![ExtraProp {
                        name: "search".to_owned(),
                        is_required: false,
                        options: vec![],
                        options_limit: OptionsLimit(1),
                    }],
                },
            }],
            addon_catalogs: vec![],
            behavior_hints: Default::default(),
        },
        transport_url: base_url.join("addon/manifest.json").unwrap(),
        flags: Default::default(),
    }
}

async fn wait_until<F>(mut predicate: F, timeout: Duration)
where
    F: FnMut() -> bool,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if predicate() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition did not become true within {timeout:?}"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

#[tokio::test]
async fn controller_boot_dispatch_addon_flows_and_persistence() {
    std::env::set_var("EMPTY_OFFICIAL_ADDONS", "1");

    let backend_url = spawn_mock_stremio_backend().await;
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("controller.db");

    let config =
        EnvConfig::with_storage_path(&db_path).with_streaming_server_url(backend_url.clone());

    DesktopEnv::run_isolated(config, async {
        let controller = CoreController::from_initialized_env()
            .await
            .expect("CoreController should boot cleanly");
        let mut events_rx = controller.subscribe();

        controller.reload_streaming_server();
        wait_until(
            || {
                controller.with_model(|m| {
                    m.streaming_server.base_url.as_ref() == Some(&backend_url)
                        && m.streaming_server.settings.is_ready()
                })
            },
            Duration::from_secs(3),
        )
        .await;

        let mock_addon = build_mock_addon_descriptor(&backend_url);
        controller.install_addon(mock_addon.clone());
        assert!(controller.with_model(|m| {
            m.ctx
                .profile
                .addons
                .iter()
                .any(|a| a.manifest.id == "io.ferrite.mock")
        }));

        let mut saw_addon_installed_event = false;
        let mut saw_new_state = false;
        let recv_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while !(saw_addon_installed_event && saw_new_state) {
            let remaining = recv_deadline.saturating_duration_since(tokio::time::Instant::now());
            let ev = tokio::time::timeout(remaining, events_rx.recv())
                .await
                .expect("timed out waiting for broadcast events")
                .expect("broadcast channel should not close");
            match ev.as_ref() {
                RuntimeEvent::NewState(fields) => {
                    if fields.contains(&DesktopModelField::Ctx) {
                        saw_new_state = true;
                    }
                }
                RuntimeEvent::CoreEvent(Event::AddonInstalled { id, .. })
                    if id == "io.ferrite.mock" =>
                {
                    saw_addon_installed_event = true;
                }
                _ => {}
            }
        }
        assert!(saw_new_state, "expected NewState([Ctx]) broadcast");
        assert!(
            saw_addon_installed_event,
            "expected CoreEvent(AddonInstalled) broadcast"
        );

        controller.load_board();
        wait_until(
            || {
                controller.with_model(|m| {
                    m.board.catalogs.iter().any(|cat| {
                        cat.first().is_some_and(|page| {
                            matches!(&page.content, Some(Loadable::Ready(items)) if items.iter().any(|i| i.id == "tt0133093"))
                        })
                    })
                })
            },
            Duration::from_secs(3),
        )
        .await;

        controller.search("Matrix");
        wait_until(
            || {
                controller.with_model(|m| {
                    m.ctx.search_history.items.contains_key("Matrix")
                        && m.search.catalogs.iter().any(|cat| {
                            cat.first().is_some_and(|page| {
                                matches!(&page.content, Some(Loadable::Ready(items)) if !items.is_empty())
                            })
                        })
                })
            },
            Duration::from_secs(3),
        )
        .await;
        assert!(controller.with_model(|m| m.board.selected.is_some()));

        controller.load_meta_details("movie", "tt0133093", None);
        wait_until(
            || {
                controller.with_model(|m| {
                    m.meta_details.streams.iter().any(|res| {
                        matches!(&res.content, Some(Loadable::Ready(streams)) if !streams.is_empty())
                    })
                })
            },
            Duration::from_secs(3),
        )
        .await;

        let fetched_stream: Stream = controller.with_model(|m| {
            m.meta_details
                .streams
                .iter()
                .find_map(|res| match &res.content {
                    Some(Loadable::Ready(streams)) => streams.first().cloned(),
                    _ => None,
                })
                .expect("stream must be loaded")
        });
        let parsed = parse_stream(&fetched_stream);
        assert_eq!(parsed.resolution, Resolution::Uhd4k);
        assert_eq!(parsed.hdr, HdrFormat::DolbyVision);
        assert_eq!(parsed.quality, StreamQuality::Remux);
        assert_eq!(parsed.codec, VideoCodec::HEVC);
        assert!(parsed.is_cached);
        assert_eq!(parsed.debrid_provider.as_deref(), Some("RD"));

        let preview = MetaItemPreview {
            id: "tt0133093".to_owned(),
            r#type: "movie".to_owned(),
            name: "The Matrix".to_owned(),
            poster: None,
            background: None,
            logo: None,
            description: None,
            release_info: Some("1999".to_owned()),
            runtime: None,
            released: None,
            poster_shape: PosterShape::Poster,
            links: vec![],
            trailer_streams: vec![],
            behavior_hints: Default::default(),
        };

        controller.toggle_library_item(preview.clone());
        assert!(controller.with_model(|m| {
            m.ctx
                .library
                .items
                .get("tt0133093")
                .is_some_and(|item| !item.removed)
        }));

        let persist_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let persisted = DesktopEnv::get_storage::<LibraryBucket>(LIBRARY_RECENT_STORAGE_KEY)
                .await
                .ok()
                .flatten()
                .is_some_and(|b| b.items.contains_key("tt0133093"));
            if persisted {
                break;
            }
            assert!(
                tokio::time::Instant::now() < persist_deadline,
                "library_recent did not persist to SQLite in time"
            );
            tokio::time::sleep(Duration::from_millis(15)).await;
        }

        controller.toggle_library_item(preview.clone());
        assert!(controller.with_model(|m| {
            m.ctx
                .library
                .items
                .get("tt0133093")
                .is_some_and(|item| item.removed)
        }));

        controller.toggle_library_item(preview);

        let addon_url = mock_addon.transport_url.clone();
        let player_selected = PlayerSelected {
            stream: Stream {
                source: StreamSource::Url {
                    url: backend_url.join("video.mkv").unwrap(),
                },
                name: Some("[RD+] Torrentio".to_owned()),
                description: None,
                thumbnail: None,
                subtitles: vec![],
                behavior_hints: StreamBehaviorHints::default(),
            },
            stream_request: Some(ResourceRequest {
                base: addon_url.clone(),
                path: ResourcePath {
                    resource: STREAM_RESOURCE_NAME.to_owned(),
                    r#type: "movie".to_owned(),
                    id: "tt0133093".to_owned(),
                    extra: vec![],
                },
            }),
            meta_request: Some(ResourceRequest {
                base: addon_url,
                path: ResourcePath {
                    resource: META_RESOURCE_NAME.to_owned(),
                    r#type: "movie".to_owned(),
                    id: "tt0133093".to_owned(),
                    extra: vec![],
                },
            }),
            subtitles_path: None,
        };

        controller.load_player(player_selected);
        assert!(controller.with_model(|m| m.player.selected.is_some()));

        controller.player_paused_changed(false);
        assert_eq!(controller.with_model(|m| m.player.paused), Some(false));

        controller.player_time_changed(45_000, 120_000, "ferrite");
        assert_eq!(
            controller.with_model(|m| {
                m.player
                    .library_item
                    .as_ref()
                    .map(|li| li.state.time_offset)
            }),
            Some(45_000)
        );

        controller.player_paused_changed(true);
        assert_eq!(controller.with_model(|m| m.player.paused), Some(true));

        controller.player_ended();
        assert!(controller.with_model(|m| m.player.ended));

        controller.unload_player();
        assert!(controller.with_model(|m| m.player.selected.is_none()));
        assert_eq!(
            controller.with_model(|m| {
                m.ctx
                    .library
                    .items
                    .get("tt0133093")
                    .map(|li| li.state.time_offset)
            }),
            Some(45_000)
        );

        controller.load_discover(None);
        assert!(controller.with_model(|m| m.discover.selected.is_some()));
        controller.unload_discover();
        assert!(controller.with_model(|m| {
            m.discover.selected.is_none() && m.discover.catalog.is_empty()
        }));

        assert!(controller.with_model(|m| m.meta_details.selected.is_some()));
        controller.unload_meta_details();
        assert!(controller.with_model(|m| {
            m.meta_details.selected.is_none()
                && m.meta_details.meta_items.is_empty()
                && m.meta_details.streams.is_empty()
        }));

        assert!(controller.with_model(|m| m.search.selected.is_some()));
        controller.unload_search();
        assert!(controller.with_model(|m| {
            m.search.selected.is_none() && m.search.catalogs.is_empty()
        }));

        controller.unload_local_search();
        assert!(controller.with_model(|m| m.local_search.search_results.is_empty()));

        controller.start_link_auth();
        assert!(controller.with_model(|m| m.auth_link.code.is_some()));
        controller.poll_link_auth();
        controller.logout();
        let _: Option<DesktopRuntimeEvent> = None;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn load_board_fetches_only_initial_shelves_until_load_board_range_advances() {
    std::env::set_var("EMPTY_OFFICIAL_ADDONS", "1");

    let backend_url = spawn_mock_stremio_backend().await;
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("board_pagination.db");
    let config =
        EnvConfig::with_storage_path(&db_path).with_streaming_server_url(backend_url.clone());

    DesktopEnv::run_isolated(config, async move {
        let controller = CoreController::from_initialized_env().await.unwrap();

        let catalogs: Vec<ManifestCatalog> = (0..12)
            .map(|i| ManifestCatalog {
                r#type: "movie".to_owned(),
                id: format!("mock_top_{i}"),
                name: Some(format!("Shelf {i}")),
                extra: ManifestExtra::Full { props: vec![] },
            })
            .collect();

        let multi_shelf_addon = Descriptor {
            manifest: Manifest {
                id: "io.ferrite.multishelf".to_owned(),
                version: Version::new(1, 0, 0),
                name: "Multi-Shelf Addon".to_owned(),
                contact_email: None,
                description: None,
                logo: None,
                background: None,
                types: vec!["movie".to_owned()],
                resources: vec![ManifestResource::Short("catalog".to_owned())],
                id_prefixes: None,
                catalogs,
                addon_catalogs: vec![],
                behavior_hints: Default::default(),
            },
            transport_url: backend_url.join("addon/manifest.json").unwrap(),
            flags: Default::default(),
        };

        controller
            .install_or_replace_addon_descriptor(multi_shelf_addon)
            .unwrap();
        controller.load_board();

        let (total_planned, trailing_unfetched) = controller.with_model(|m| {
            let total = m.board.catalogs.len();
            let unfetched = m
                .board
                .catalogs
                .iter()
                .skip(INITIAL_BOARD_SHELVES + 1)
                .filter(|cat| cat.first().is_none_or(|p| p.content.is_none()))
                .count();
            (total, unfetched)
        });
        assert_eq!(total_planned, 12);
        assert_eq!(trailing_unfetched, 12 - (INITIAL_BOARD_SHELVES + 1));

        controller.load_board_range(INITIAL_BOARD_SHELVES..12);
        let remaining_unfetched = controller.with_model(|m| {
            m.board
                .catalogs
                .iter()
                .filter(|cat| cat.first().is_none_or(|p| p.content.is_none()))
                .count()
        });
        assert_eq!(remaining_unfetched, 0);
    })
    .await
    .unwrap();
}
