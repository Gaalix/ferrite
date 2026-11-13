use std::time::Duration;

use stremio_core::{
    constants::{META_RESOURCE_NAME, STREAM_RESOURCE_NAME, SUBTITLES_RESOURCE_NAME},
    models::player::Selected as PlayerSelected,
    types::{
        addon::{
            Descriptor, Manifest, ManifestResource, ResourcePath, ResourceRequest, Version,
        },
        resource::{Stream, StreamBehaviorHints, StreamSource},
    },
};
use stremio_core_runtime::{
    select_preferred_tracks, CoreController, DesktopEnv, EnvConfig, PrewarmTriggerReason,
    SeriesTrackPreference, TrackCandidate,
};
use tempfile::tempdir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

async fn spawn_series_addon_backend() -> Url {
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
                let raw_path = req_text
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let decoded_path = raw_path.replace("%3A", ":");

                let body = if decoded_path == "/addon/meta/series/tt0999888.json" {
                    serde_json::json!({
                        "meta": {
                            "id": "tt0999888",
                            "type": "series",
                            "name": "Frieren",
                            "posterShape": "poster",
                            "videos": [
                                {
                                    "id": "tt0999888:1:1",
                                    "title": "The Journey's End",
                                    "season": 1,
                                    "episode": 1,
                                    "released": "2023-09-29T00:00:00.000Z"
                                },
                                {
                                    "id": "tt0999888:1:2",
                                    "title": "It Didn't Have to Be Magic...",
                                    "season": 1,
                                    "episode": 2,
                                    "released": "2023-09-29T01:00:00.000Z"
                                }
                            ]
                        }
                    })
                    .to_string()
                } else if decoded_path == "/addon/stream/series/tt0999888:1:2.json" {
                    serde_json::json!({
                        "streams": [
                            {
                                "infoHash": "1234567890abcdef1234567890abcdef12345678",
                                "fileIdx": 1,
                                "name": "Torrentio\n1080p",
                                "description": "[SubsPlease] Frieren - 02 (1080p).mkv",
                                "behaviorHints": {
                                    "bingeGroup": "torrentio|1080p|subsplease"
                                }
                            },
                            {
                                "url": format!("{}debrid/frieren-s01e02.mkv", server_base),
                                "name": "[RD+] Torrentio\n4K",
                                "description": "[BD] Frieren - S01E02 2160p HEVC Dual-Audio.mkv",
                                "behaviorHints": {
                                    "bingeGroup": "torrentio|4k|bd"
                                }
                            }
                        ]
                    })
                    .to_string()
                } else {
                    serde_json::json!({ "ok": true }).to_string()
                };

                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            });
        }
    });

    base_url
}

#[tokio::test]
async fn series_track_preferences_persist_across_restarts_and_plain_os_threads() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("track_prefs.db");

    let config1 = EnvConfig::with_storage_path(&db_path);
    DesktopEnv::run_isolated(config1, async {
        let controller = CoreController::from_initialized_env().await.unwrap();
        assert!(controller.get_series_track_pref("tt0999888").is_none());

        let pref = SeriesTrackPreference {
            series_id: "tt0999888".to_owned(),
            audio_lang: Some("jpn".to_owned()),
            audio_title_keywords: vec!["flac".to_owned()],
            subtitles_enabled: true,
            sub_lang: Some("eng".to_owned()),
            sub_title_keywords: vec!["full".to_owned(), "dialogue".to_owned()],
            prefer_forced_only: false,
            sub_delay_secs: 0.25,
        };

        let ui_controller = controller.clone();
        let pref_clone = pref.clone();
        let read_back = std::thread::spawn(move || {
            ui_controller.save_series_track_pref(pref_clone);
            ui_controller.get_series_track_pref("tt0999888")
        })
        .join()
        .unwrap();

        assert_eq!(read_back, Some(pref));
    })
    .await
    .unwrap();

    let config2 = EnvConfig::with_storage_path(&db_path);
    DesktopEnv::run_isolated(config2, async {
        let controller = CoreController::from_initialized_env().await.unwrap();
        let loaded = controller
            .get_series_track_pref("tt0999888")
            .expect("preference must persist across SQLite reopen");
        assert_eq!(loaded.audio_lang.as_deref(), Some("jpn"));
        assert_eq!(loaded.sub_lang.as_deref(), Some("eng"));
        assert!(!loaded.prefer_forced_only);
        assert!((loaded.sub_delay_secs - 0.25).abs() < f64::EPSILON);

        let ep2_tracks = vec![
            TrackCandidate::audio(1, Some("eng"), Some("English 5.1 Dub")).with_default(true),
            TrackCandidate::audio(2, Some("jpn"), Some("Japanese 2.0 AAC")),
            TrackCandidate::sub(10, Some("eng"), Some("Signs & Songs")).with_forced(true),
            TrackCandidate::sub(11, Some("eng"), Some("Full Subtitles")),
        ];
        let matched = select_preferred_tracks(&loaded, &ep2_tracks);
        assert_eq!(matched.audio_id, Some(2));
        assert_eq!(matched.sub_id, Some(Some(11)));

        assert_eq!(controller.list_series_track_prefs().len(), 1);
        assert!(controller.remove_series_track_pref("tt0999888"));
        assert!(controller.get_series_track_pref("tt0999888").is_none());
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn end_to_end_next_episode_prewarm_and_url_resolution_in_controller() {
    let server_base = spawn_series_addon_backend().await;
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("prewarm.db");

    let streaming_server_url: Url = "http://127.0.0.1:11470/".parse().unwrap();
    let config = EnvConfig::with_storage_path(&db_path)
        .with_streaming_server_url(streaming_server_url.clone());

    DesktopEnv::run_isolated(config, async move {
        let controller = CoreController::from_initialized_env().await.unwrap();
        let addon_manifest_url = server_base.join("addon/manifest.json").unwrap();

        let addon = Descriptor {
            manifest: Manifest {
                id: "org.stremio.mock.series".to_owned(),
                version: Version::new(1, 0, 0),
                name: "Mock Series Addon".to_owned(),
                contact_email: None,
                description: None,
                logo: None,
                background: None,
                types: vec!["series".to_owned()],
                resources: vec![
                    ManifestResource::Short(META_RESOURCE_NAME.to_owned()),
                    ManifestResource::Short(STREAM_RESOURCE_NAME.to_owned()),
                ],
                id_prefixes: Some(vec!["tt".to_owned()]),
                catalogs: vec![],
                addon_catalogs: vec![],
                behavior_hints: Default::default(),
            },
            transport_url: addon_manifest_url.clone(),
            flags: Default::default(),
        };
        controller.install_addon(addon);

        let ep1_stream = Stream {
            source: StreamSource::Url {
                url: server_base.join("debrid/frieren-s01e01.mkv").unwrap(),
            },
            name: Some("Torrentio\n1080p".to_owned()),
            description: Some("[SubsPlease] Frieren - 01 (1080p).mkv".to_owned()),
            thumbnail: None,
            subtitles: vec![],
            behavior_hints: StreamBehaviorHints {
                binge_group: Some("torrentio|1080p|subsplease".to_owned()),
                ..Default::default()
            },
        };

        controller.load_player(PlayerSelected {
            stream: ep1_stream,
            stream_request: Some(ResourceRequest {
                base: addon_manifest_url.clone(),
                path: ResourcePath {
                    resource: STREAM_RESOURCE_NAME.to_owned(),
                    r#type: "series".to_owned(),
                    id: "tt0999888:1:1".to_owned(),
                    extra: vec![],
                },
            }),
            meta_request: Some(ResourceRequest {
                base: addon_manifest_url.clone(),
                path: ResourcePath {
                    resource: META_RESOURCE_NAME.to_owned(),
                    r#type: "series".to_owned(),
                    id: "tt0999888".to_owned(),
                    extra: vec![],
                },
            }),
            subtitles_path: None,
        });

        assert_eq!(
            controller.active_player_series_id().as_deref(),
            Some("tt0999888")
        );

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let resolved = loop {
            if let Some(res) = controller.resolve_next_episode_playback_url() {
                break res;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for resolve_next_episode_playback_url"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        };

        assert_eq!(resolved.next_video.id, "tt0999888:1:2");
        assert!(resolved
            .url
            .as_str()
            .starts_with("http://127.0.0.1:11470/1234567890abcdef1234567890abcdef12345678/1"));
        assert_eq!(
            resolved
                .player_selected
                .stream_request
                .as_ref()
                .unwrap()
                .path
                .id,
            "tt0999888:1:2"
        );

        assert!(controller
            .should_trigger_next_episode_prewarm(300.0, 1420.0)
            .is_none());
        assert!(controller
            .should_trigger_next_episode_prewarm(1350.0, 1420.0)
            .is_none());
        let prewarm = controller
            .should_trigger_next_episode_prewarm(1365.0, 1420.0)
            .expect("should trigger in final 60s");
        assert_eq!(prewarm.series_id.as_deref(), Some("tt0999888"));
        assert_eq!(prewarm.video_id, "tt0999888:1:2");
        assert_eq!(prewarm.trigger_reason, PrewarmTriggerReason::EndingWindow);
        assert!(prewarm.stream_ready);

        controller.load_player(PlayerSelected {
            stream: Stream {
                source: StreamSource::Url {
                    url: server_base.join("debrid/frieren-s01e01-4k.mkv").unwrap(),
                },
                name: Some("[RD+] Torrentio\n4K".to_owned()),
                description: Some("[BD] Frieren - S01E01 2160p HEVC Dual-Audio.mkv".to_owned()),
                thumbnail: None,
                subtitles: vec![],
                behavior_hints: StreamBehaviorHints {
                    binge_group: Some("torrentio|4k|bd".to_owned()),
                    ..Default::default()
                },
            },
            stream_request: None,
            meta_request: Some(ResourceRequest {
                base: addon_manifest_url,
                path: ResourcePath {
                    resource: META_RESOURCE_NAME.to_owned(),
                    r#type: "series".to_owned(),
                    id: "tt0999888".to_owned(),
                    extra: vec![],
                },
            }),
            subtitles_path: Some(ResourcePath {
                resource: SUBTITLES_RESOURCE_NAME.to_owned(),
                r#type: "series".to_owned(),
                id: "tt0999888:1:1".to_owned(),
                extra: vec![],
            }),
        });

        let deadline2 = tokio::time::Instant::now() + Duration::from_secs(5);
        let fallback_resolved = loop {
            if let Some(res) = controller.resolve_next_episode_playback_url() {
                break res;
            }
            assert!(
                tokio::time::Instant::now() < deadline2,
                "timed out waiting for fallback meta_details stream resolution"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        };

        assert_eq!(
            fallback_resolved.url.as_str(),
            format!("{}debrid/frieren-s01e02.mkv", server_base)
        );
    })
    .await
    .unwrap();
}
