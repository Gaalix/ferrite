use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use axum::{
    extract::OriginalUri,
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::StreamExt;
use librqbit::{CreateTorrentOptions, ListenerMode, ListenerOptions, Session, SessionOptions};
use stremio_core::{
    models::streaming_server::PlaybackDevice,
    types::{
        api::SuccessResponse,
        resource::{Stream as CoreStream, StreamBehaviorHints, StreamProxyHeaders, StreamSource},
        streaming_server::{
            CreateMagnetBody, CreateMagnetRequest, CreateTorrentBlobBody, DeviceInfo, NetworkInfo,
            SettingsResponse, Statistics,
        },
        streams::ConvertedStreamSource,
        torrent::InfoHash,
    },
};
use stremio_stream_engine::{
    compute_opensubhash_bytes, default_cache_dir, redact_url, redact_url_str,
    validate_info_hash_str, LruIndexFile, LruTorrentEntry, OpensubHashResponse, PiecePriority,
    PlayheadScheduler, StorageMode, StreamCandidate, StreamEngine, StreamEngineConfig,
    StreamEngineError, StreamEngineHandle, DEFAULT_DISK_LRU_MAX_BYTES, DEFAULT_FIRST_BYTE_TIMEOUT,
    DEFAULT_LOOKAHEAD_BYTES, DEFAULT_OPENTRACKERS, DEFAULT_RAM_ONLY_MAX_BYTES, HEAD_CRITICAL_BYTES,
    LRU_INDEX_FILENAME, MAX_LOOKAHEAD_BYTES, MIN_LOOKAHEAD_BYTES, OPENSUB_HASH_CHUNK_SIZE,
    TAIL_CRITICAL_BYTES,
};
use tempfile::TempDir;

async fn start_test_engine(cache_dir: &TempDir, metadata_timeout: Duration) -> StreamEngineHandle {
    let cfg = StreamEngineConfig {
        bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        cache_dir: cache_dir.path().to_path_buf(),
        enable_dht: false,
        enable_trackers: false,
        enable_lsd: false,
        enable_peer_listener: true,
        enable_upnp: false,
        metadata_timeout,
        ..Default::default()
    };
    StreamEngine::start(cfg)
        .await
        .expect("StreamEngine should start cleanly")
}

fn deterministic_payload(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    let mut out = vec![0u8; len];
    for chunk in out.chunks_mut(8) {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        let bytes = z.to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    out
}

fn reference_opensubtitles_hash(file_bytes: &[u8]) -> String {
    assert!(file_bytes.len() >= 65_536);
    let file_size = file_bytes.len() as u64;
    let mut hash: u64 = file_size;
    for i in 0..(65_536 / 8) {
        let off = i * 8;
        let w = u64::from_le_bytes(file_bytes[off..off + 8].try_into().unwrap());
        hash = hash.wrapping_add(w);
    }
    let tail_base = file_bytes.len() - 65_536;
    for i in 0..(65_536 / 8) {
        let off = tail_base + i * 8;
        let w = u64::from_le_bytes(file_bytes[off..off + 8].try_into().unwrap());
        hash = hash.wrapping_add(w);
    }
    format!("{hash:016x}")
}

#[tokio::test]
async fn settings_device_network_and_casting_endpoints_match_stremio_core() {
    let tmp = TempDir::new().unwrap();
    let engine = start_test_engine(&tmp, Duration::from_secs(5)).await;
    let base = engine.base_url();
    let client = reqwest::Client::new();

    let settings_resp: SettingsResponse = client
        .get(base.join("settings").unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(settings_resp.base_url, base);
    assert_eq!(
        settings_resp.values.cache_root,
        tmp.path().to_string_lossy()
    );

    let new_cache = tmp.path().join("updated-cache");
    let update_resp: SuccessResponse = client
        .post(base.join("settings").unwrap())
        .json(&serde_json::json!({
            "cacheRoot": new_cache.to_string_lossy(),
            "cacheSize": 5_368_709_120.0_f64,
            "proxyStreamsEnabled": true
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let _ = update_resp.success;

    let updated_settings: SettingsResponse = client
        .get(base.join("settings").unwrap())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        updated_settings.values.cache_root,
        new_cache.to_string_lossy()
    );
    assert_eq!(updated_settings.values.cache_size, Some(5_368_709_120.0));
    assert!(updated_settings.values.proxy_streams_enabled);

    let bad_resp = client
        .post(base.join("settings").unwrap())
        .json(&serde_json::json!({ "cacheRoot": "" }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_resp.status(), StatusCode::BAD_REQUEST);

    let dev_info: DeviceInfo = client
        .get(base.join("device-info").unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let _ = dev_info.available_hardware_accelerations;

    let net_info: NetworkInfo = client
        .get(base.join("network-info").unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!net_info.available_interfaces.is_empty());

    let casting: Vec<PlaybackDevice> = client
        .get(base.join("casting").unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(casting.is_empty());

    engine.shutdown().await;
}

#[tokio::test]
async fn opensubtitles_moviehash_matches_official_reference_algorithm() {
    let tmp = TempDir::new().unwrap();
    let engine = start_test_engine(&tmp, Duration::from_secs(5)).await;
    let base = engine.base_url();
    let client = reqwest::Client::new();

    let zeros = vec![0u8; 131_072];
    let zero_res = compute_opensubhash_bytes(&zeros).unwrap();
    assert_eq!(zero_res.size, 131_072);
    assert_eq!(zero_res.hash, "0000000000020000");
    assert_eq!(zero_res.hash, reference_opensubtitles_hash(&zeros));

    let all_ff = vec![0xFFu8; 131_072];
    let ff_res = compute_opensubhash_bytes(&all_ff).unwrap();
    assert_eq!(ff_res.hash, reference_opensubtitles_hash(&all_ff));

    let overlap_data = deterministic_payload(42, 90_123);
    let overlap_res = compute_opensubhash_bytes(&overlap_data).unwrap();
    assert_eq!(overlap_res.size, 90_123);
    assert_eq!(
        overlap_res.hash,
        reference_opensubtitles_hash(&overlap_data)
    );

    let movie_bytes = deterministic_payload(777, 3 * 1024 * 1024 + 371);
    let expected_hash = reference_opensubtitles_hash(&movie_bytes);
    let movie_path = tmp.path().join("sample_video.mkv");
    tokio::fs::write(&movie_path, &movie_bytes).await.unwrap();

    let mut endpoint = base.join("opensubhash").unwrap();
    endpoint
        .query_pairs_mut()
        .append_pair("url", &movie_path.to_string_lossy());

    let http_res: OpensubHashResponse = client
        .get(endpoint)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(http_res.result.size, movie_bytes.len() as u64);
    assert_eq!(http_res.result.hash, expected_hash);

    let tiny_path = tmp.path().join("tiny.mp4");
    tokio::fs::write(&tiny_path, vec![1u8; OPENSUB_HASH_CHUNK_SIZE - 1])
        .await
        .unwrap();
    let mut tiny_ep = base.join("opensubhash").unwrap();
    tiny_ep
        .query_pairs_mut()
        .append_pair("url", &tiny_path.to_string_lossy());
    let tiny_resp = client.get(tiny_ep).send().await.unwrap();
    assert_eq!(tiny_resp.status(), StatusCode::BAD_REQUEST);

    engine.shutdown().await;
}

#[tokio::test]
async fn torrent_streaming_200_206_ranges_file_selection_only_files_and_stats() {
    let seed_dir = TempDir::new().unwrap();
    let season_folder = seed_dir.path().join("Show.S01.Pack");
    tokio::fs::create_dir_all(&season_folder).await.unwrap();

    let readme_bytes = b"Season pack NFO".to_vec();
    let ep1_bytes = deterministic_payload(101, 1_100_000);
    let ep2_bytes = deterministic_payload(202, 2_400_000);
    let ep3_bytes = deterministic_payload(303, 850_000);

    tokio::fs::write(season_folder.join("00_readme.txt"), &readme_bytes)
        .await
        .unwrap();
    tokio::fs::write(season_folder.join("Show.S01E01.1080p.mkv"), &ep1_bytes)
        .await
        .unwrap();
    tokio::fs::write(season_folder.join("Show.S01E02.1080p.mp4"), &ep2_bytes)
        .await
        .unwrap();
    tokio::fs::write(season_folder.join("Show.S01E03.720p.mkv"), &ep3_bytes)
        .await
        .unwrap();

    let seeder_session = Session::new_with_opts(
        seed_dir.path().to_path_buf(),
        SessionOptions {
            dht: None,
            disable_trackers: true,
            disable_local_service_discovery: true,
            listen: Some(ListenerOptions {
                mode: ListenerMode::TcpOnly,
                listen_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
                enable_upnp_port_forwarding: false,
                ipv4_only: true,
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let seeder_addr = seeder_session
        .listen_addr()
        .expect("seeder should bind TCP listener");

    let (created_meta, _seed_handle) = seeder_session
        .create_and_serve_torrent(
            &season_folder,
            CreateTorrentOptions {
                name: Some("Show.S01.Pack"),
                piece_length: Some(32 * 1024),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let torrent_bytes = created_meta.as_bytes().unwrap().to_vec();

    let leech_cache = TempDir::new().unwrap();
    let engine = start_test_engine(&leech_cache, Duration::from_secs(15)).await;
    let base = engine.base_url();
    let client = reqwest::Client::new();

    let info_hash = engine
        .add_torrent_bytes(torrent_bytes.clone(), vec![], Some(vec![seeder_addr]))
        .await
        .unwrap();
    let ih_hex = info_hash.to_string();

    let create_stats: Statistics = client
        .post(base.join("create").unwrap())
        .json(&serde_json::json!({
            "blob": CreateTorrentBlobBody { blob: hex::encode(&torrent_bytes) }.blob
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(create_stats.info_hash, ih_hex);
    assert_eq!(create_stats.files.len(), 4);

    let ep1_idx = create_stats
        .files
        .iter()
        .position(|f| f.name.contains("S01E01"))
        .unwrap();
    let ep2_idx = create_stats
        .files
        .iter()
        .position(|f| f.name.contains("S01E02"))
        .unwrap();
    let ep3_idx = create_stats
        .files
        .iter()
        .position(|f| f.name.contains("S01E03"))
        .unwrap();

    let head_resp = client
        .head(base.join(&format!("{ih_hex}/-1")).unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(head_resp.status(), StatusCode::OK);
    assert_eq!(
        head_resp.headers().get(header::ACCEPT_RANGES).unwrap(),
        "bytes"
    );
    assert_eq!(
        head_resp
            .headers()
            .get(header::CONTENT_LENGTH)
            .unwrap()
            .to_str()
            .unwrap(),
        ep2_bytes.len().to_string()
    );
    assert_eq!(
        head_resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "video/mp4"
    );
    assert_eq!(
        engine.scheduler().selected_file(&info_hash.as_array()),
        Some(ep2_idx)
    );
    let rqbit_only_files = engine
        .session()
        .with_torrents(|it| it.map(|(_, h)| h.only_files()).next().flatten())
        .unwrap();
    assert_eq!(rqbit_only_files, vec![ep2_idx]);

    let range_mid = client
        .get(base.join(&format!("{ih_hex}/-1")).unwrap())
        .header(header::RANGE, "bytes=100000-250000")
        .send()
        .await
        .unwrap();
    assert_eq!(range_mid.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        range_mid.headers().get(header::ACCEPT_RANGES).unwrap(),
        "bytes"
    );
    assert_eq!(
        range_mid
            .headers()
            .get(header::CONTENT_RANGE)
            .unwrap()
            .to_str()
            .unwrap(),
        format!("bytes 100000-250000/{}", ep2_bytes.len())
    );
    let mid_body = range_mid.bytes().await.unwrap();
    assert_eq!(mid_body.len(), 150_001);
    assert_eq!(&mid_body[..], &ep2_bytes[100_000..=250_000]);

    let open_start = 2_300_000usize;
    let range_open = client
        .get(base.join(&format!("{ih_hex}/{ep2_idx}")).unwrap())
        .header(header::RANGE, format!("bytes={open_start}-"))
        .send()
        .await
        .unwrap();
    assert_eq!(range_open.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        range_open
            .headers()
            .get(header::CONTENT_RANGE)
            .unwrap()
            .to_str()
            .unwrap(),
        format!(
            "bytes {open_start}-{}/{}",
            ep2_bytes.len() - 1,
            ep2_bytes.len()
        )
    );
    let open_body = range_open.bytes().await.unwrap();
    assert_eq!(&open_body[..], &ep2_bytes[open_start..]);

    let range_tail = client
        .get(base.join(&format!("{ih_hex}/{ep2_idx}")).unwrap())
        .header(header::RANGE, "bytes=-65536")
        .send()
        .await
        .unwrap();
    assert_eq!(range_tail.status(), StatusCode::PARTIAL_CONTENT);
    let tail_body = range_tail.bytes().await.unwrap();
    assert_eq!(&tail_body[..], &ep2_bytes[ep2_bytes.len() - 65_536..]);

    let stream_url = base.join(&format!("{ih_hex}/{ep2_idx}")).unwrap();
    let mut osh_url = base.join("opensubhash").unwrap();
    osh_url
        .query_pairs_mut()
        .append_pair("url", stream_url.as_str());
    let osh_resp: OpensubHashResponse = client
        .get(osh_url)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(osh_resp.result.size, ep2_bytes.len() as u64);
    assert_eq!(
        osh_resp.result.hash,
        reference_opensubtitles_hash(&ep2_bytes)
    );

    let ep1_resp = client
        .get(base.join(&format!("{ih_hex}/-1?f=S01E01")).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(ep1_resp.status(), StatusCode::OK);
    assert_eq!(
        ep1_resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "video/x-matroska"
    );
    let ep1_downloaded = ep1_resp.bytes().await.unwrap();
    assert_eq!(&ep1_downloaded[..], &ep1_bytes[..]);
    assert_eq!(
        engine.scheduler().selected_file(&info_hash.as_array()),
        Some(ep1_idx)
    );

    let stats_no_idx: Statistics = client
        .get(base.join(&format!("{ih_hex}/stats.json")).unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(stats_no_idx.stream_len.is_none());

    let stats_ep1: Statistics = client
        .get(
            base.join(&format!("{ih_hex}/{ep1_idx}/stats.json"))
                .unwrap(),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stats_ep1.stream_len, Some(ep1_bytes.len() as u64));
    assert_eq!(
        stats_ep1.stream_name.as_deref(),
        Some("Show.S01E01.1080p.mkv")
    );
    assert_eq!(stats_ep1.stream_progress, Some(1.0));

    let managed = engine
        .session()
        .get(librqbit::api::TorrentIdOrHash::Hash(
            librqbit::dht::Id20::new(info_hash.as_array()),
        ))
        .expect("managed torrent present");
    assert_eq!(managed.only_files(), Some(vec![ep1_idx]));

    let stats_ep3: Statistics = client
        .get(
            base.join(&format!("{ih_hex}/{ep3_idx}/stats.json"))
                .unwrap(),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ep3_progress = stats_ep3.stream_progress.expect("ep3 stream_progress");
    let ep3_have_bytes = ep3_progress * (ep3_bytes.len() as f64);
    assert!(
        ep3_have_bytes < (32 * 1024) as f64,
        "expected 0 exclusive pieces of unwatched E03 downloaded, got {ep3_have_bytes} bytes (progress {ep3_progress})"
    );

    {
        let aborted_resp = client
            .get(base.join(&format!("{ih_hex}/{ep2_idx}")).unwrap())
            .header(header::RANGE, "bytes=0-2000000")
            .send()
            .await
            .unwrap();
        assert_eq!(aborted_resp.status(), StatusCode::PARTIAL_CONTENT);
        let mut byte_stream = aborted_resp.bytes_stream();
        let first_chunk = byte_stream.next().await.unwrap().unwrap();
        assert!(!first_chunk.is_empty());
    }
    for _ in 0..200 {
        if engine
            .scheduler()
            .active_stream_count(&info_hash.as_array(), ep2_idx)
            == 0
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        engine
            .scheduler()
            .active_stream_count(&info_hash.as_array(), ep2_idx),
        0,
        "aborted HTTP Range stream must drop cleanly without leaking cursor"
    );

    engine.shutdown().await;
    seeder_session.cancellation_token().cancel();
}

#[tokio::test]
async fn create_magnet_merges_opentrackers_and_unresolvable_stream_times_out_cleanly() {
    let tmp = TempDir::new().unwrap();
    let engine = start_test_engine(&tmp, Duration::from_millis(350)).await;
    let base = engine.base_url();
    let client = reqwest::Client::new();

    let info_hash: InfoHash = "df389295484b3059a4726dc6d8a57f71bb5f4c81".parse().unwrap();
    let create_req: http::Request<CreateMagnetBody> = CreateMagnetRequest {
        server_url: base.clone(),
        info_hash,
        announce: vec!["udp://custom.tracker.example:1337/announce".to_owned()],
    }
    .into();

    let (parts, body) = create_req.into_parts();
    let stats: Statistics = client
        .post(parts.uri.to_string())
        .json(&serde_json::to_value(body).unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(stats.info_hash, info_hash.to_string());
    assert!(stats.peer_search_running);
    assert!(stats
        .opts
        .peer_search
        .sources
        .iter()
        .any(|s| s == "tracker:udp://custom.tracker.example:1337/announce"));
    for opentracker in DEFAULT_OPENTRACKERS {
        let expected = format!("tracker:{opentracker}");
        assert!(
            stats
                .opts
                .peer_search
                .sources
                .iter()
                .any(|s| s == &expected),
            "missing merged opentracker {expected}"
        );
    }

    let dead_resp = client
        .get(base.join(&format!("{info_hash}/-1")).unwrap())
        .send()
        .await
        .unwrap();
    assert!(
        dead_resp.status() == StatusCode::GATEWAY_TIMEOUT
            || dead_resp.status() == StatusCode::BAD_GATEWAY,
        "expected 504 or 502 instead of hanging forever, got {}",
        dead_resp.status()
    );

    engine.shutdown().await;
}

#[tokio::test]
async fn proxy_endpoint_parses_stremio_core_proxy_urls_headers_and_ranges() {
    let upstream_payload = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ".to_vec();
    let payload_clone = upstream_payload.clone();
    let upstream_app = Router::new().route(
        "/videos/episode.mp4",
        get(move |headers: HeaderMap, OriginalUri(uri): OriginalUri| {
            let body_data = payload_clone.clone();
            async move {
                assert_eq!(
                    headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok()),
                    Some("Bearer my+secret+token"),
                    "custom `h=` header must preserve `+` without turning into spaces"
                );
                assert_eq!(uri.query(), Some("token=abc&quality=1080p"));

                if let Some(range_val) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
                    assert_eq!(range_val, "bytes=10-19");
                    return (
                        StatusCode::PARTIAL_CONTENT,
                        [
                            (header::ACCEPT_RANGES, "bytes".to_owned()),
                            (
                                header::CONTENT_RANGE,
                                format!("bytes 10-19/{}", body_data.len()),
                            ),
                            (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
                        ],
                        body_data[10..=19].to_vec(),
                    )
                        .into_response();
                }

                (StatusCode::OK, body_data).into_response()
            }
        }),
    );

    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream_listener.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move {
        axum::serve(upstream_listener, upstream_app).await.unwrap();
    });

    let tmp = TempDir::new().unwrap();
    let engine = start_test_engine(&tmp, Duration::from_secs(5)).await;
    let base = engine.base_url();

    let stream = CoreStream {
        source: StreamSource::Url {
            url: format!("http://{upstream_addr}/videos/episode.mp4?token=abc&quality=1080p")
                .parse()
                .unwrap(),
        },
        name: Some("Proxied Stream".to_owned()),
        description: None,
        thumbnail: None,
        subtitles: vec![],
        behavior_hints: StreamBehaviorHints {
            proxy_headers: Some(StreamProxyHeaders {
                request: HashMap::from([(
                    "Authorization".to_owned(),
                    "Bearer my+secret+token".to_owned(),
                )]),
                response: HashMap::from([("Content-Type".to_owned(), "video/mp4".to_owned())]),
            }),
            ..Default::default()
        },
    };

    let converted = stream.convert(Some(&base)).unwrap();
    let proxied_url = match converted.source {
        ConvertedStreamSource::Url { url } => url,
        other => panic!("expected ConvertedStreamSource::Url, got {other:?}"),
    };
    assert!(proxied_url.path().starts_with("/proxy/"));

    let client = reqwest::Client::new();
    let resp = client
        .get(proxied_url)
        .header(header::RANGE, "bytes=10-19")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), "video/mp4");
    assert_eq!(
        resp.headers().get(header::CONTENT_RANGE).unwrap(),
        "bytes 10-19/36"
    );
    let body = resp.bytes().await.unwrap();
    assert_eq!(&body[..], b"ABCDEFGHIJ");

    engine.shutdown().await;
    upstream_task.abort();
}

#[test]
fn playhead_scheduler_computes_head_tail_and_adaptive_lookahead_windows() {
    let piece_len = 512 * 1024;
    let file_offset = 10 * piece_len;
    let file_len = 100 * 1024 * 1024;

    let plan0 = PlayheadScheduler::compute_priority_window(file_offset, file_len, piece_len, 0, None);
    assert_eq!(plan0.file_piece_range, 10..210);
    assert_eq!(
        plan0.head_pieces,
        10..(10 + (HEAD_CRITICAL_BYTES / piece_len) as u32)
    );
    assert_eq!(
        plan0.tail_pieces,
        (210 - (TAIL_CRITICAL_BYTES / piece_len) as u32)..210
    );
    assert_eq!(plan0.lookahead_bytes, DEFAULT_LOOKAHEAD_BYTES);
    assert_eq!(plan0.lookahead_pieces, 10..74);

    assert_eq!(plan0.priority_of(9), PiecePriority::Ignore);
    assert_eq!(plan0.priority_of(210), PiecePriority::Ignore);
    assert_eq!(plan0.priority_of(10), PiecePriority::Critical);
    assert_eq!(plan0.priority_of(13), PiecePriority::Critical);
    assert_eq!(plan0.priority_of(206), PiecePriority::Critical);
    assert_eq!(plan0.priority_of(209), PiecePriority::Critical);
    assert_eq!(plan0.priority_of(20), PiecePriority::High);
    assert_eq!(plan0.priority_of(120), PiecePriority::Normal);

    let seek_offset = 50 * 1024 * 1024;
    let plan_seek = PlayheadScheduler::compute_priority_window(
        file_offset,
        file_len,
        piece_len,
        seek_offset,
        Some(5 * 1024 * 1024),
    );
    assert_eq!(plan_seek.lookahead_bytes, 100 * 1024 * 1024);
    assert_eq!(plan_seek.playhead_piece, Some(110));
    assert_eq!(plan_seek.lookahead_pieces, 110..210);
    assert_eq!(plan_seek.head_pieces, 10..14);
    assert_eq!(plan_seek.tail_pieces, 206..210);

    let low_plan =
        PlayheadScheduler::compute_priority_window(0, 500 * 1024 * 1024, piece_len, 0, Some(100_000));
    assert_eq!(low_plan.lookahead_bytes, MIN_LOOKAHEAD_BYTES);

    let max_plan = PlayheadScheduler::compute_priority_window(
        0,
        500 * 1024 * 1024,
        piece_len,
        0,
        Some(20 * 1024 * 1024),
    );
    assert_eq!(max_plan.lookahead_bytes, MAX_LOOKAHEAD_BYTES);

    let small_plan =
        PlayheadScheduler::compute_priority_window(0, 3 * 1024 * 1024, piece_len, 0, None);
    assert_eq!(small_plan.critical_pieces(), vec![0, 1, 2, 3, 4, 5]);
}

async fn spawn_local_seeder_for_single_file(
    seed_dir: &TempDir,
    file_name: &str,
    payload: &[u8],
) -> (std::sync::Arc<Session>, SocketAddr, Vec<u8>, [u8; 20]) {
    let folder = seed_dir.path().join(format!("{file_name}_folder"));
    tokio::fs::create_dir_all(&folder).await.unwrap();
    tokio::fs::write(folder.join(file_name), payload)
        .await
        .unwrap();

    let seeder = Session::new_with_opts(
        seed_dir.path().to_path_buf(),
        SessionOptions {
            dht: None,
            disable_trackers: true,
            disable_local_service_discovery: true,
            listen: Some(ListenerOptions {
                mode: ListenerMode::TcpOnly,
                listen_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
                enable_upnp_port_forwarding: false,
                ipv4_only: true,
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let seeder_addr = seeder.listen_addr().expect("seeder TCP listener");
    let (meta, _handle) = seeder
        .create_and_serve_torrent(
            &folder,
            CreateTorrentOptions {
                name: Some(file_name),
                piece_length: Some(32 * 1024),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let torrent_bytes = meta.as_bytes().unwrap().to_vec();
    let info_hash = meta.info_hash().0;
    (seeder, seeder_addr, torrent_bytes, info_hash)
}

#[tokio::test]
async fn upnp_and_peer_listener_configuration_binds_tcp_utp_and_custom_port() {
    let prod_default = StreamEngineConfig::default();
    assert!(prod_default.enable_upnp);
    assert!(prod_default.enable_peer_listener);
    assert_eq!(prod_default.listen_port, None);
    assert_eq!(
        prod_default.storage_mode,
        StorageMode::disk_lru(DEFAULT_DISK_LRU_MAX_BYTES)
    );
    assert_eq!(
        prod_default.first_byte_timeout,
        DEFAULT_FIRST_BYTE_TIMEOUT
    );
    assert_eq!(
        StorageMode::default_ram_only(),
        StorageMode::ram_only(DEFAULT_RAM_ONLY_MAX_BYTES)
    );

    let tmp = TempDir::new().unwrap();
    let test_cfg = StreamEngineConfig::for_testing(tmp.path().to_path_buf());
    assert!(!test_cfg.enable_upnp);

    let mut started = None;
    for _ in 0..6 {
        let probe_tcp = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let candidate_port = probe_tcp.local_addr().unwrap().port();
        drop(probe_tcp);
        if std::net::UdpSocket::bind(("0.0.0.0", candidate_port)).is_err() {
            continue;
        }

        let cfg = StreamEngineConfig {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            cache_dir: tmp.path().to_path_buf(),
            enable_dht: false,
            enable_trackers: false,
            enable_lsd: false,
            enable_peer_listener: true,
            enable_upnp: false,
            listen_port: Some(candidate_port),
            ..Default::default()
        };
        if let Ok(handle) = StreamEngine::start(cfg).await {
            started = Some((handle, candidate_port));
            break;
        }
    }

    let (engine, chosen_port) = started.expect("StreamEngine should bind explicit listen_port");
    let peer_addr = engine
        .peer_listen_addr()
        .expect("TCP+uTP peer listener should report bound SocketAddr");
    assert_eq!(peer_addr.port(), chosen_port);
    assert!(!engine.enable_upnp());
    engine.set_enable_upnp(true);
    assert!(engine.enable_upnp());
    engine.set_enable_upnp(false);

    engine.shutdown().await;
}

#[tokio::test]
async fn disk_lru_warm_resume_across_restart_and_active_cursor_safe_eviction() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let seed_a_dir = TempDir::new().unwrap();
    let seed_b_dir = TempDir::new().unwrap();
    let bytes_a = deterministic_payload(9001, 6_000_000);
    let bytes_b = deterministic_payload(9002, 6_000_000);

    let (seeder_a, addr_a, torrent_a, ih_a) =
        spawn_local_seeder_for_single_file(&seed_a_dir, "WarmResumeA.mkv", &bytes_a).await;
    let (seeder_b, addr_b, torrent_b, ih_b) =
        spawn_local_seeder_for_single_file(&seed_b_dir, "WarmResumeB.mkv", &bytes_b).await;
    let ih_a_hex = hex::encode(ih_a);
    let ih_b_hex = hex::encode(ih_b);

    let shared_cache = TempDir::new().unwrap();
    let quota_bytes = 9_000_000u64;

    {
        let mut cfg1 = StreamEngineConfig::for_testing(shared_cache.path().to_path_buf());
        cfg1.storage_mode = StorageMode::disk_lru(quota_bytes);
        let engine1 = StreamEngine::start(cfg1).await.unwrap();
        let client = reqwest::Client::new();

        engine1
            .add_torrent_bytes(torrent_a.clone(), vec![], Some(vec![addr_a]))
            .await
            .unwrap();
        let resp_a = client
            .get(engine1.base_url().join(&format!("{ih_a_hex}/0")).unwrap())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let body_a = resp_a.bytes().await.unwrap();
        assert_eq!(&body_a[..], &bytes_a[..]);
        assert!(engine1.cache_usage_bytes() >= 5_800_000);

        engine1.shutdown().await;
    }

    seeder_a.cancellation_token().cancel();
    drop(seeder_a);
    drop(seed_a_dir);

    assert!(
        shared_cache.path().join(LRU_INDEX_FILENAME).exists(),
        "lru_index.json must be persisted on disk"
    );

    let mut cfg2 = StreamEngineConfig::for_testing(shared_cache.path().to_path_buf());
    cfg2.storage_mode = StorageMode::disk_lru(quota_bytes);
    cfg2.metadata_timeout = Duration::from_secs(5);
    cfg2.first_byte_timeout = Duration::from_secs(5);
    let engine2 = StreamEngine::start(cfg2).await.unwrap();
    let client = reqwest::Client::new();

    let warm_t0 = std::time::Instant::now();
    let warm_resp = client
        .get(engine2.base_url().join(&format!("{ih_a_hex}/0")).unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let warm_bytes = warm_resp.bytes().await.unwrap();
    assert_eq!(&warm_bytes[..], &bytes_a[..]);
    assert!(
        warm_t0.elapsed() < Duration::from_secs(10),
        "warm resume from persisted pieces should complete near-instantly"
    );

    let sock = tokio::net::TcpSocket::new_v4().unwrap();
    sock.set_recv_buffer_size(8192).unwrap();
    let mut active_range_conn = sock.connect(engine2.local_addr()).await.unwrap();
    let raw_req = format!(
        "GET /{ih_a_hex}/0 HTTP/1.1\r\nHost: {}\r\nRange: bytes=0-{}\r\n\r\n",
        engine2.local_addr(),
        bytes_a.len() - 1
    );
    active_range_conn.write_all(raw_req.as_bytes()).await.unwrap();
    let mut head_buf = vec![0u8; 1024];
    let n_read = active_range_conn.read(&mut head_buf).await.unwrap();
    assert!(
        std::str::from_utf8(&head_buf[..n_read.min(64)])
            .unwrap_or("")
            .starts_with("HTTP/1.1 206"),
        "expected HTTP/1.1 206 Partial Content on live Range cursor"
    );

    assert!(
        engine2.scheduler().has_active_streams_for_torrent(&ih_a),
        "Torrent A must register an active HTTP Range cursor while open"
    );

    engine2
        .set_storage_mode(StorageMode::disk_lru(3_000_000))
        .await
        .unwrap();
    let evicted_while_active = engine2.enforce_cache_quota().await;
    assert_eq!(
        evicted_while_active, 0,
        "Active torrent with open HTTP Range cursor must NEVER be evicted even above quota"
    );
    assert!(shared_cache.path().join(&ih_a_hex).exists());
    assert!(engine2.torrent_stats(&ih_a_hex, Some(0)).is_some());

    engine2
        .set_storage_mode(StorageMode::disk_lru(20_000_000))
        .await
        .unwrap();
    engine2
        .add_torrent_bytes(torrent_b.clone(), vec![], Some(vec![addr_b]))
        .await
        .unwrap();
    let resp_b = client
        .get(engine2.base_url().join(&format!("{ih_b_hex}/0")).unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let body_b = resp_b.bytes().await.unwrap();
    assert_eq!(&body_b[..], &bytes_b[..]);

    drop(active_range_conn);
    for _ in 0..200 {
        if !engine2.scheduler().has_active_streams_for_torrent(&ih_a) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!engine2.scheduler().has_active_streams_for_torrent(&ih_a));

    engine2
        .set_storage_mode(StorageMode::disk_lru(quota_bytes))
        .await
        .unwrap();
    let _ = engine2.enforce_cache_quota().await;

    assert!(
        !shared_cache.path().join(&ih_a_hex).exists(),
        "Oldest inactive Torrent A should be evicted once its HTTP Range cursor closes"
    );
    assert!(engine2.torrent_stats(&ih_a_hex, Some(0)).is_none());
    assert!(
        engine2.cache_usage_bytes() <= quota_bytes,
        "cache_usage_bytes ({}) must be <= quota ({quota_bytes})",
        engine2.cache_usage_bytes()
    );
    assert!(shared_cache.path().join(&ih_b_hex).exists());
    assert!(engine2.torrent_stats(&ih_b_hex, Some(0)).is_some());

    engine2.shutdown().await;
    seeder_b.cancellation_token().cancel();
}

#[tokio::test]
async fn ram_only_mode_zero_ssd_writes_stream_switch_eviction_and_runtime_mode_switch() {
    let seed_a_dir = TempDir::new().unwrap();
    let seed_b_dir = TempDir::new().unwrap();
    let bytes_a = deterministic_payload(4101, 600_000);
    let bytes_b = deterministic_payload(4102, 600_000);

    let (seeder_a, addr_a, torrent_a, ih_a) =
        spawn_local_seeder_for_single_file(&seed_a_dir, "RamMovieA.mp4", &bytes_a).await;
    let (seeder_b, addr_b, torrent_b, ih_b) =
        spawn_local_seeder_for_single_file(&seed_b_dir, "RamMovieB.mp4", &bytes_b).await;
    let ih_a_hex = hex::encode(ih_a);
    let ih_b_hex = hex::encode(ih_b);

    let ssd_parent = TempDir::new().unwrap();
    let untouched_ssd_dir = ssd_parent.path().join("ssd_must_stay_untouched");

    let mut cfg = StreamEngineConfig::for_testing(untouched_ssd_dir.clone());
    cfg.storage_mode = StorageMode::ram_only(8 * 1024 * 1024);
    let engine = StreamEngine::start(cfg).await.unwrap();
    let ram_dir = engine.ram_cache_dir();
    assert!(ram_dir.to_string_lossy().contains("ferrite-ram-"));
    #[cfg(target_os = "linux")]
    if std::path::Path::new("/dev/shm").is_dir() {
        assert!(
            ram_dir.starts_with("/dev/shm"),
            "RamOnly buffer must live in /dev/shm tmpfs on Linux, got {ram_dir:?}"
        );
    }
    assert_eq!(
        engine.storage_mode(),
        StorageMode::ram_only(8 * 1024 * 1024)
    );
    assert!(!untouched_ssd_dir.exists());

    let client = reqwest::Client::new();

    engine
        .add_torrent_bytes(torrent_a, vec![], Some(vec![addr_a]))
        .await
        .unwrap();
    let got_a = client
        .get(engine.base_url().join(&format!("{ih_a_hex}/0")).unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&got_a[..], &bytes_a[..]);
    assert!(
        !untouched_ssd_dir.exists(),
        "RamOnly mode must write 0 bytes to SSD cache_dir"
    );
    assert!(ram_dir.join(&ih_a_hex).exists());
    assert!(engine.cache_usage_bytes() >= 550_000);

    engine
        .add_torrent_bytes(torrent_b, vec![], Some(vec![addr_b]))
        .await
        .unwrap();
    let got_b = client
        .get(engine.base_url().join(&format!("{ih_b_hex}/0")).unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&got_b[..], &bytes_b[..]);
    assert!(
        !ram_dir.join(&ih_a_hex).exists(),
        "Switching streams in RamOnly mode must immediately evict previous inactive torrent"
    );
    assert!(ram_dir.join(&ih_b_hex).exists());
    assert!(!untouched_ssd_dir.exists());

    engine
        .set_storage_mode(StorageMode::disk_lru(16 * 1024 * 1024))
        .await
        .unwrap();
    assert_eq!(
        engine.storage_mode(),
        StorageMode::disk_lru(16 * 1024 * 1024)
    );
    assert!(
        !ram_dir.join(&ih_b_hex).exists(),
        "Switching from RamOnly to DiskLru must purge inactive RAM torrents"
    );
    assert!(untouched_ssd_dir.exists());

    engine
        .set_storage_mode(StorageMode::ram_only(4 * 1024 * 1024))
        .await
        .unwrap();
    assert_eq!(
        engine.storage_mode(),
        StorageMode::ram_only(4 * 1024 * 1024)
    );

    engine.shutdown().await;
    assert!(
        !ram_dir.exists(),
        "RamOnly directory must be removed on engine shutdown"
    );

    seeder_a.cancellation_token().cancel();
    seeder_b.cancellation_token().cancel();
}

#[tokio::test]
async fn dead_swarm_first_byte_timeout_failover_and_prewarm_stream() {
    let dead_seed_dir = TempDir::new().unwrap();
    let dead_payload = deterministic_payload(6001, 512 * 1024);
    let (dead_seeder, _dead_addr, dead_torrent_bytes, dead_ih) =
        spawn_local_seeder_for_single_file(&dead_seed_dir, "DeadSwarm.mkv", &dead_payload).await;
    dead_seeder.cancellation_token().cancel();
    drop(dead_seeder);
    drop(dead_seed_dir);
    let dead_hex = hex::encode(dead_ih);

    let dead2_seed_dir = TempDir::new().unwrap();
    let (dead2_seeder, _dead2_addr, dead2_torrent_bytes, dead2_ih) =
        spawn_local_seeder_for_single_file(&dead2_seed_dir, "DeadSwarm2.mkv", &dead_payload).await;
    dead2_seeder.cancellation_token().cancel();
    drop(dead2_seeder);
    drop(dead2_seed_dir);
    let dead2_hex = hex::encode(dead2_ih);

    let live_seed_dir = TempDir::new().unwrap();
    let live_payload = deterministic_payload(6002, 750_000);
    let (live_seeder, live_addr, live_torrent_bytes, live_ih) =
        spawn_local_seeder_for_single_file(&live_seed_dir, "HealthySwarm.mkv", &live_payload).await;
    let live_hex = hex::encode(live_ih);

    let http_payload = live_payload.clone();
    let http_app = Router::new()
        .route(
            "/broken.mp4",
            get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        )
        .route(
            "/debrid.mp4",
            get(move |headers: HeaderMap| {
                let data = http_payload.clone();
                async move {
                    if let Some(r) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
                        let parsed = r.strip_prefix("bytes=").unwrap();
                        let (s, e) = parsed.split_once('-').unwrap();
                        let start: usize = s.parse().unwrap();
                        let end: usize = if e.is_empty() {
                            data.len() - 1
                        } else {
                            e.parse::<usize>().unwrap().min(data.len() - 1)
                        };
                        return (
                            StatusCode::PARTIAL_CONTENT,
                            [
                                (header::ACCEPT_RANGES, "bytes".to_owned()),
                                (
                                    header::CONTENT_RANGE,
                                    format!("bytes {start}-{end}/{}", data.len()),
                                ),
                                (header::CONTENT_TYPE, "video/mp4".to_owned()),
                            ],
                            data[start..=end].to_vec(),
                        )
                            .into_response();
                    }
                    (StatusCode::OK, data).into_response()
                }
            }),
        );
    let http_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_addr = http_listener.local_addr().unwrap();
    let http_task = tokio::spawn(async move {
        axum::serve(http_listener, http_app).await.unwrap();
    });

    let cache_dir = TempDir::new().unwrap();
    let mut cfg = StreamEngineConfig::for_testing(cache_dir.path().to_path_buf());
    cfg.metadata_timeout = Duration::from_secs(8);
    cfg.first_byte_timeout = Duration::from_millis(250);
    let engine = StreamEngine::start(cfg).await.unwrap();
    assert_eq!(engine.first_byte_timeout(), Duration::from_millis(250));
    let client = reqwest::Client::new();

    engine
        .add_torrent_bytes(dead_torrent_bytes, vec![], None)
        .await
        .unwrap();
    let stall_t0 = std::time::Instant::now();
    let stalled_resp = client
        .get(engine.base_url().join(&format!("{dead_hex}/0")).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(
        stalled_resp.status(),
        StatusCode::GATEWAY_TIMEOUT,
        "0-seed stalled torrent must fail fast with 504 Gateway Timeout instead of hanging mpv"
    );
    assert!(stall_t0.elapsed() < Duration::from_secs(6));
    engine.set_first_byte_timeout(Duration::from_secs(4));
    assert_eq!(engine.first_byte_timeout(), Duration::from_secs(4));

    let broken_url: url::Url = format!("http://{http_addr}/broken.mp4").parse().unwrap();
    let candidates = vec![
        StreamCandidate::torrent_with_bytes(dead2_ih, dead2_torrent_bytes, Some(0), None)
            .with_label("Dead 4K Swarm (0 seeds)"),
        StreamCandidate::http(broken_url.clone()).with_label("Expired Debrid Link (503)"),
        StreamCandidate::torrent_with_bytes(
            live_ih,
            live_torrent_bytes,
            Some(0),
            Some(vec![live_addr]),
        )
        .with_label("Healthy 1080p Swarm"),
    ];

    let resolved = engine
        .probe_or_failover_streams(&candidates, Duration::from_millis(2000))
        .await
        .expect("failover should resolve healthy 3rd candidate");
    assert_eq!(resolved.candidate_index, 2);
    assert_eq!(resolved.label.as_deref(), Some("Healthy 1080p Swarm"));
    assert_eq!(resolved.failed_attempts.len(), 2);
    assert_eq!(
        resolved.playback_url,
        engine.base_url().join(&format!("{live_hex}/0")).unwrap()
    );
    assert_eq!(resolved.info_hash, Some(InfoHash::new(live_ih)));
    assert_eq!(resolved.resolved_file_idx, Some(0));

    assert!(
        engine.torrent_stats(&dead2_hex, None).is_none(),
        "probe_or_failover_streams must clean up stalled dead torrent candidates"
    );

    let broken_core_stream = CoreStream {
        source: StreamSource::Url { url: broken_url },
        name: Some("Broken CDN".to_owned()),
        description: None,
        thumbnail: None,
        subtitles: vec![],
        behavior_hints: Default::default(),
    };
    let healthy_debrid_url: url::Url = format!("http://{http_addr}/debrid.mp4").parse().unwrap();
    let healthy_core_stream = CoreStream {
        source: StreamSource::Url {
            url: healthy_debrid_url,
        },
        name: Some("RD+ Instant".to_owned()),
        description: Some("1080p BluRay".to_owned()),
        thumbnail: None,
        subtitles: vec![],
        behavior_hints: StreamBehaviorHints {
            proxy_headers: Some(StreamProxyHeaders {
                request: HashMap::from([("X-Test-Auth".to_owned(), "ok".to_owned())]),
                response: HashMap::new(),
            }),
            ..Default::default()
        },
    };
    let http_candidates: Vec<StreamCandidate> = [&broken_core_stream, &healthy_core_stream]
        .into_iter()
        .filter_map(StreamCandidate::from_stremio_stream)
        .collect();
    let resolved_http = engine
        .probe_or_failover_streams(&http_candidates, Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(resolved_http.candidate_index, 1);
    assert!(resolved_http.playback_url.path().starts_with("/proxy/"));

    let warmed_torrent = engine
        .prewarm_stream(&resolved.playback_url, 128 * 1024)
        .await
        .unwrap();
    assert_eq!(warmed_torrent, 128 * 1024);

    let warmed_http = engine
        .prewarm_stream(&resolved_http.playback_url, 64 * 1024)
        .await
        .unwrap();
    assert_eq!(warmed_http, 64 * 1024);

    engine.shutdown().await;
    live_seeder.cancellation_token().cancel();
    http_task.abort();
}

#[tokio::test]
async fn release_playback_sessions_hibernates_active_torrents_and_warm_restores_offline() {
    let seed_a_dir = TempDir::new().unwrap();
    let seed_b_dir = TempDir::new().unwrap();
    let bytes_a = deterministic_payload(7101, 700_000);
    let bytes_b = deterministic_payload(7102, 700_000);

    let (seeder_a, addr_a, torrent_a, ih_a) =
        spawn_local_seeder_for_single_file(&seed_a_dir, "HibernationShow1.mkv", &bytes_a).await;
    let (seeder_b, addr_b, torrent_b, ih_b) =
        spawn_local_seeder_for_single_file(&seed_b_dir, "HibernationShow2.mkv", &bytes_b).await;
    let ih_a_hex = hex::encode(ih_a);
    let ih_b_hex = hex::encode(ih_b);

    let cache_dir = TempDir::new().unwrap();
    let mut cfg = StreamEngineConfig::for_testing(cache_dir.path().to_path_buf());
    cfg.storage_mode = StorageMode::disk_lru(50 * 1024 * 1024);
    cfg.metadata_timeout = Duration::from_secs(5);
    cfg.first_byte_timeout = Duration::from_secs(5);
    let engine = StreamEngine::start(cfg).await.unwrap();
    let client = reqwest::Client::new();

    assert_eq!(engine.stats().active_torrents, 0);

    engine
        .add_torrent_bytes(torrent_a, vec![], Some(vec![addr_a]))
        .await
        .unwrap();
    let got_a = client
        .get(engine.base_url().join(&format!("{ih_a_hex}/0")).unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&got_a[..], &bytes_a[..]);
    assert_eq!(engine.stats().active_torrents, 1);

    engine
        .add_torrent_bytes(torrent_b, vec![], Some(vec![addr_b]))
        .await
        .unwrap();
    let got_b = client
        .get(engine.base_url().join(&format!("{ih_b_hex}/0")).unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&got_b[..], &bytes_b[..]);
    assert_eq!(
        engine.stats().active_torrents,
        1,
        "switching streams must hibernate previous inactive torrent so active_torrents stays 1"
    );
    assert!(
        cache_dir.path().join(&ih_a_hex).exists(),
        "DiskLru hibernation must keep Show 1 warm-cached files on disk"
    );

    engine.release_playback_sessions().await;
    assert_eq!(
        engine.stats().active_torrents,
        0,
        "release_playback_sessions() must drop active_torrents to 0"
    );
    assert_eq!(
        engine.session().with_torrents(|it| it.count()),
        0,
        "librqbit::Session must hold 0 ManagedTorrent handles after release_playback_sessions()"
    );
    assert!(cache_dir.path().join(&ih_a_hex).exists());
    assert!(cache_dir.path().join(&ih_b_hex).exists());
    assert!(engine.cache_usage_bytes() >= 1_300_000);

    seeder_a.cancellation_token().cancel();
    seeder_b.cancellation_token().cancel();
    drop(seeder_a);
    drop(seeder_b);
    drop(seed_a_dir);
    drop(seed_b_dir);

    let t0 = std::time::Instant::now();
    let warm_a = client
        .get(engine.base_url().join(&format!("{ih_a_hex}/0")).unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let warm_elapsed = t0.elapsed();
    assert_eq!(&warm_a[..], &bytes_a[..]);
    assert!(
        warm_elapsed < Duration::from_secs(5),
        "offline warm restore from .torrents backup should be near-instant, took {warm_elapsed:?}"
    );
    assert_eq!(engine.stats().active_torrents, 1);

    engine.release_playback_resources_bg();
    for _ in 0..200 {
        if engine.stats().active_torrents == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(engine.stats().active_torrents, 0);

    let warm_b = client
        .get(engine.base_url().join(&format!("{ih_b_hex}/0")).unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&warm_b[..], &bytes_b[..]);

    engine.release_playback_sessions().await;
    assert_eq!(engine.stats().active_torrents, 0);

    engine.shutdown().await;
}

#[tokio::test]
async fn security_hardening_cors_origin_traversal_schemes_permissions_and_redaction() {
    let def_dir = default_cache_dir();
    let def_str = def_dir.to_string_lossy();
    assert!(
        def_str.contains("ferrite"),
        "default_cache_dir must use ferrite brand directory, got {def_str}"
    );

    let secret_url: url::Url =
        "https://user:supersecret@cdn.debrid.example:8443/dl/API_KEY_98765/Movie.mkv?token=SECRET123#frag"
            .parse()
            .unwrap();
    let redacted = redact_url(&secret_url);
    assert_eq!(redacted, "https://cdn.debrid.example:8443/...");
    assert!(!redacted.contains("supersecret"));
    assert!(!redacted.contains("API_KEY_98765"));
    assert!(!redacted.contains("SECRET123"));
    assert_eq!(
        redact_url_str("http://addon.example/APIKEY/manifest.json?sig=abc"),
        "http://addon.example/..."
    );

    assert!(validate_info_hash_str("df389295484b3059a4726dc6d8a57f71bb5f4c81").is_ok());
    assert!(validate_info_hash_str("344JFFKITTIFTJDSNXDNRJLPWG5V6TEA").is_ok());
    assert!(validate_info_hash_str("..").is_err());
    assert!(validate_info_hash_str("df389295484b3059a4726dc6d8a57f71/b5f4c81").is_err());
    assert!(validate_info_hash_str("df389295484b3059a4726dc6d8a57f71\\b5f4c81").is_err());
    assert!(validate_info_hash_str("df389295484b3059a4726dc6d8a57f71\0b5f4c81").is_err());
    assert!(validate_info_hash_str("ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ").is_err());
    assert!(validate_info_hash_str("11111111111111111111111111111111").is_err());

    let outside_victim_dir = TempDir::new().unwrap();
    let victim_subfolder = outside_victim_dir.path().join("must_never_be_deleted");
    tokio::fs::create_dir_all(&victim_subfolder).await.unwrap();
    let victim_marker = victim_subfolder.join("keep.txt");
    tokio::fs::write(&victim_marker, vec![0xAAu8; 80_000])
        .await
        .unwrap();

    let cache_tmp = TempDir::new().unwrap();
    let cache_sub = cache_tmp.path().join("ferrite_cache");
    tokio::fs::create_dir_all(&cache_sub).await.unwrap();

    let tampered_index = LruIndexFile {
        version: 1,
        entries: vec![LruTorrentEntry {
            info_hash: "1111111111111111111111111111111111111111".to_owned(),
            name: Some("Tampered".to_owned()),
            size_bytes: 999_999_999,
            last_accessed_unix_ms: 1,
            output_folder: victim_subfolder.clone(),
            selected_file_idx: Some(0),
        }],
    };
    tampered_index
        .save_atomic(&cache_sub.join(LRU_INDEX_FILENAME))
        .await
        .unwrap();

    let mut cfg = StreamEngineConfig::for_testing(&cache_sub);
    cfg.storage_mode = StorageMode::disk_lru(1024);
    let engine = StreamEngine::start(cfg).await.unwrap();
    let _ = engine.enforce_cache_quota().await;
    assert!(
        victim_marker.exists(),
        "tampered lru_index.json entry pointing outside cache_dir must be rejected and never deleted"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir_mode = std::fs::metadata(&cache_sub).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        let idx_mode = std::fs::metadata(cache_sub.join(LRU_INDEX_FILENAME))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(idx_mode, 0o600);
    }

    let base = engine.base_url();
    let client = reqwest::Client::new();

    let evil_get = client
        .get(base.join("settings").unwrap())
        .header(header::ORIGIN, "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(evil_get.status(), StatusCode::FORBIDDEN);
    assert!(evil_get
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .is_none());

    let evil_post = client
        .post(base.join("settings").unwrap())
        .header(header::ORIGIN, "http://attacker.lan:8080")
        .json(&serde_json::json!({ "proxyStreamsEnabled": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(evil_post.status(), StatusCode::FORBIDDEN);

    let loopback_origin = format!("http://127.0.0.1:{}", engine.local_addr().port());
    let ok_loopback = client
        .get(base.join("settings").unwrap())
        .header(header::ORIGIN, &loopback_origin)
        .send()
        .await
        .unwrap();
    assert_eq!(ok_loopback.status(), StatusCode::OK);
    assert_eq!(
        ok_loopback
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok()),
        Some(loopback_origin.as_str())
    );

    let bad_ih_resp = client
        .get(format!("{base}..%2fetc/0"))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_ih_resp.status(), StatusCode::BAD_REQUEST);

    let bad_ih_charset = client
        .get(format!("{base}ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ/0"))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_ih_charset.status(), StatusCode::BAD_REQUEST);

    let traversal_settings = client
        .post(base.join("settings").unwrap())
        .json(&serde_json::json!({
            "cacheRoot": format!("{}/../escaped", cache_sub.display())
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(traversal_settings.status(), StatusCode::BAD_REQUEST);

    let relative_settings = client
        .post(base.join("settings").unwrap())
        .json(&serde_json::json!({
            "cacheRoot": "relative/cache/dir"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(relative_settings.status(), StatusCode::BAD_REQUEST);

    let mut outside_osh = base.join("opensubhash").unwrap();
    outside_osh
        .query_pairs_mut()
        .append_pair("url", &victim_marker.to_string_lossy());
    let outside_osh_resp = client.get(outside_osh).send().await.unwrap();
    assert_eq!(outside_osh_resp.status(), StatusCode::FORBIDDEN);

    if let Ok(file_url) = url::Url::from_file_path(&victim_marker) {
        let mut outside_file_osh = base.join("opensubhash").unwrap();
        outside_file_osh
            .query_pairs_mut()
            .append_pair("url", file_url.as_str());
        let resp = client.get(outside_file_osh).send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    let mut bad_scheme_osh = base.join("opensubhash").unwrap();
    bad_scheme_osh
        .query_pairs_mut()
        .append_pair("url", "ftp://example.com/movie.mkv");
    let bad_scheme_resp = client.get(bad_scheme_osh).send().await.unwrap();
    assert_eq!(bad_scheme_resp.status(), StatusCode::BAD_REQUEST);

    let file_proxy = client
        .get(base.join("proxy/d=file%3A%2F%2F%2Fetc/passwd").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(file_proxy.status(), StatusCode::BAD_REQUEST);

    let cred_proxy = client
        .get(
            base.join("proxy/d=https%3A%2F%2Fuser%3Apass%40example.com/video.mp4")
                .unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(cred_proxy.status(), StatusCode::BAD_REQUEST);

    let bad_candidates = vec![
        StreamCandidate::http("file:///etc/passwd".parse().unwrap()),
        StreamCandidate::http(
            "http://127.0.0.1:9/debrid/SUPER_SECRET_TOKEN/video.mkv?apikey=PRIVATE999"
                .parse()
                .unwrap(),
        ),
    ];
    let fail_err = engine
        .probe_or_failover_streams(&bad_candidates, Duration::from_millis(500))
        .await
        .expect_err("invalid scheme and dead port must fail");
    match fail_err {
        StreamEngineError::AllCandidatesFailed { attempts } => {
            assert_eq!(attempts.len(), 2);
            assert!(attempts[0].reason.contains("Unsupported URL scheme"));
            assert!(!attempts[1].reason.contains("SUPER_SECRET_TOKEN"));
            assert!(!attempts[1].reason.contains("PRIVATE999"));
        }
        other => panic!("unexpected error variant: {other:?}"),
    }

    engine.shutdown().await;
}

#[tokio::test]
async fn magnet_stream_endpoint_waits_for_metadata_or_times_out_never_returning_uninitialized_500() {
    // Part 1: Real magnet ut_metadata exchange over 127.0.0.1 while handle is Initializing.
    // Without wait_until_initialized() in ensure_torrent_resolved, GET /{ih_hex}/0 immediately
    // hit handle.with_metadata(...) and failed with 500 "Torrent is not initialized".
    let seed_dir = TempDir::new().unwrap();
    let movie_folder = seed_dir.path().join("Magnet.Movie");
    tokio::fs::create_dir_all(&movie_folder).await.unwrap();
    let movie_bytes = deterministic_payload(777, 256 * 1024);
    tokio::fs::write(movie_folder.join("Hoodwinked.1080p.mp4"), &movie_bytes)
        .await
        .unwrap();

    let seeder_session = Session::new_with_opts(
        seed_dir.path().to_path_buf(),
        SessionOptions {
            dht: None,
            disable_trackers: true,
            disable_local_service_discovery: true,
            listen: Some(ListenerOptions {
                mode: ListenerMode::TcpOnly,
                listen_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
                enable_upnp_port_forwarding: false,
                ipv4_only: true,
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let seeder_addr = seeder_session
        .listen_addr()
        .expect("seeder should bind TCP listener");
    let (_created_meta, seed_handle) = seeder_session
        .create_and_serve_torrent(
            &movie_folder,
            CreateTorrentOptions {
                name: Some("Magnet.Movie"),
                piece_length: Some(32 * 1024),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let ih_hex = hex::encode(seed_handle.info_hash().0);

    let tmp = TempDir::new().unwrap();
    let engine = start_test_engine(&tmp, Duration::from_secs(10)).await;
    let base = engine.base_url();
    let client = reqwest::Client::new();

    let out_dir = tmp.path().join(&ih_hex);
    tokio::fs::create_dir_all(&out_dir).await.unwrap();
    let magnet_uri = format!("magnet:?xt=urn:btih:{ih_hex}");
    let _magnet_resp = engine
        .session()
        .add_torrent(
            librqbit::AddTorrent::from_url(magnet_uri),
            Some(librqbit::AddTorrentOptions {
                overwrite: true,
                output_folder: Some(out_dir.to_string_lossy().into_owned()),
                initial_peers: Some(vec![seeder_addr]),
                ..Default::default()
            }),
        )
        .await
        .unwrap();

    // Immediately request GET /{ih_hex}/0 before ut_metadata finishes: must wait for metadata
    // initialization and stream 200 OK with exact bytes, NEVER 500 "Torrent is not initialized".
    let seeded_resp = client
        .get(base.join(&format!("{ih_hex}/0")).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(seeded_resp.status(), StatusCode::OK);
    let downloaded = seeded_resp.bytes().await.unwrap();
    assert_eq!(&downloaded[..], &movie_bytes[..]);

    // Part 2: Unresolvable magnets (both warm via POST /{info_hash}/create and cold via direct GET)
    // must return 504 Gateway Timeout or 502 Bad Gateway, NEVER 500 "Torrent is not initialized".
    let dead_tmp = TempDir::new().unwrap();
    let dead_engine = start_test_engine(&dead_tmp, Duration::from_millis(400)).await;
    let dead_base = dead_engine.base_url();

    let info_hash: InfoHash = "0123456789abcdef0123456789abcdef01234567".parse().unwrap();
    let create_req: http::Request<CreateMagnetBody> = CreateMagnetRequest {
        server_url: dead_base.clone(),
        info_hash,
        announce: vec![],
    }
    .into();
    let (parts, body) = create_req.into_parts();
    let create_resp = client
        .post(parts.uri.to_string())
        .json(&serde_json::to_value(body).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(create_resp.status(), StatusCode::OK);

    let stream_resp = client
        .get(dead_base.join(&format!("{info_hash}/0")).unwrap())
        .send()
        .await
        .unwrap();
    assert!(
        stream_resp.status() == StatusCode::GATEWAY_TIMEOUT
            || stream_resp.status() == StatusCode::BAD_GATEWAY,
        "expected 504 or 502 on unresolvable warm magnet, got {}",
        stream_resp.status()
    );
    let body_text = stream_resp.text().await.unwrap();
    assert!(
        !body_text.contains("Torrent is not initialized"),
        "unexpected uninitialized 500 error: {body_text}"
    );

    let cold_hash = "fedcba9876543210fedcba9876543210fedcba98";
    let cold_resp = client
        .get(dead_base.join(&format!("{cold_hash}/0")).unwrap())
        .send()
        .await
        .unwrap();
    assert!(
        cold_resp.status() == StatusCode::GATEWAY_TIMEOUT
            || cold_resp.status() == StatusCode::BAD_GATEWAY,
        "expected 504 or 502 on unresolvable cold magnet, got {}",
        cold_resp.status()
    );
    let cold_body = cold_resp.text().await.unwrap();
    assert!(
        !cold_body.contains("Torrent is not initialized"),
        "unexpected uninitialized 500 error on cold magnet: {cold_body}"
    );

    dead_engine.shutdown().await;
    engine.shutdown().await;
    seeder_session.cancellation_token().cancel();
}
