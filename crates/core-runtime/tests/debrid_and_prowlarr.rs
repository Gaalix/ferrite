use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use stremio_core::types::resource::StreamSource;
use stremio_core_runtime::{
    parse_info_hash, CoreController, DebridBaseUrls, DebridCacheStatus, DesktopEnv, EnvConfig,
    LocalDebridProvider, LocalDebridResolver, PrivateIntegrationsConfig, ProwlarrClient,
    ProwlarrFallbackMode, SecretApiKey,
};
use tempfile::tempdir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

#[derive(Clone, Debug)]
struct RecordedRequest {
    method: String,
    path_and_query: String,
    headers: HashMap<String, String>,
    body: String,
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Option<RecordedRequest> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 2048];
    let mut header_end = None;

    while header_end.is_none() {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            header_end = Some(pos + 4);
        }
    }

    let head_end = header_end?;
    let head_str = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head_str.lines();
    let first_line = lines.next()?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path_and_query = parts.next()?.to_owned();

    let mut headers = HashMap::new();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key_lower = k.trim().to_ascii_lowercase();
            let val = v.trim().to_owned();
            if key_lower == "content-length" {
                content_length = val.parse().unwrap_or(0);
            }
            headers.insert(key_lower, val);
        }
    }

    while buf.len() < head_end + content_length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }

    let body = String::from_utf8_lossy(
        &buf[head_end..buf.len().min(head_end + content_length)],
    )
    .to_string();

    Some(RecordedRequest {
        method,
        path_and_query,
        headers,
        body,
    })
}

async fn write_http_response(
    socket: &mut tokio::net::TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Error",
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = socket.write_all(resp.as_bytes()).await;
    let _ = socket.shutdown().await;
}

async fn spawn_mock_server<F>(handler: F) -> (Url, Arc<Mutex<Vec<RecordedRequest>>>)
where
    F: Fn(RecordedRequest) -> (u16, &'static str, String) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = Url::parse(&format!("http://{addr}")).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let req_log = Arc::clone(&requests);
    let handler = Arc::new(handler);

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let req_log = Arc::clone(&req_log);
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                if let Some(req) = read_http_request(&mut socket).await {
                    if let Ok(mut guard) = req_log.lock() {
                        guard.push(req.clone());
                    }
                    let (status, content_type, body) = handler(req);
                    write_http_response(&mut socket, status, content_type, &body).await;
                }
            });
        }
    });

    (base_url, requests)
}

#[tokio::test]
async fn real_debrid_verify_cache_resolve_and_uncached_cleanup_wire_mock() {
    let info_hash_hex = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let info_hash = parse_info_hash(info_hash_hex).unwrap();
    let info_calls = Arc::new(AtomicUsize::new(0));
    let info_calls_clone = Arc::clone(&info_calls);
    let uncached_mode = Arc::new(AtomicUsize::new(0));
    let uncached_clone = Arc::clone(&uncached_mode);
    let instant_forbidden = Arc::new(AtomicUsize::new(0));
    let instant_clone = Arc::clone(&instant_forbidden);

    let (base, requests) = spawn_mock_server(move |req| {
        assert_eq!(
            req.headers.get("authorization").map(String::as_str),
            Some("Bearer rd_live_secret_9999")
        );

        if req.method == "GET" && req.path_and_query == "/user" {
            return (
                200,
                "application/json",
                serde_json::json!({
                    "username": "ferrite_rd",
                    "type": "premium",
                    "expiration": "2027-01-01T00:00:00Z"
                })
                .to_string(),
            );
        }

        if req.method == "GET" && req.path_and_query.starts_with("/torrents/instantAvailability/") {
            if instant_clone.load(Ordering::SeqCst) == 1 {
                return (403, "application/json", r#"{"error":"permission_denied"}"#.to_owned());
            }
            return (
                200,
                "application/json",
                serde_json::json!({
                    info_hash_hex: {
                        "rd": [{ "1": { "filename": "Show.S01E02.mkv", "filesize": 123456 } }]
                    }
                })
                .to_string(),
            );
        }

        if req.method == "POST" && req.path_and_query == "/torrents/addMagnet" {
            return (
                200,
                "application/json",
                serde_json::json!({ "id": "rd_torrent_42" }).to_string(),
            );
        }

        if req.method == "GET" && req.path_and_query == "/torrents/info/rd_torrent_42" {
            if uncached_clone.load(Ordering::SeqCst) == 1 {
                return (
                    200,
                    "application/json",
                    serde_json::json!({
                        "id": "rd_torrent_42",
                        "status": "downloading",
                        "files": [{ "id": 1, "path": "/Movie.mkv", "bytes": 1000, "selected": 1 }],
                        "links": []
                    })
                    .to_string(),
                );
            }
            let n = info_calls_clone.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                return (
                    200,
                    "application/json",
                    serde_json::json!({
                        "id": "rd_torrent_42",
                        "status": "waiting_files_selection",
                        "files": [
                            { "id": 1, "path": "/Show/Sample/Show.S01E02.sample.mkv", "bytes": 5000000, "selected": 0 },
                            { "id": 2, "path": "/Show/Show.S01E01.1080p.mkv", "bytes": 2000000000_u64, "selected": 0 },
                            { "id": 3, "path": "/Show/Show.S01E02.1080p.mkv", "bytes": 1800000000_u64, "selected": 0 }
                        ],
                        "links": []
                    })
                    .to_string(),
                );
            }
            return (
                200,
                "application/json",
                serde_json::json!({
                    "id": "rd_torrent_42",
                    "status": "downloaded",
                    "files": [
                        { "id": 3, "path": "/Show/Show.S01E02.1080p.mkv", "bytes": 1800000000_u64, "selected": 1 }
                    ],
                    "links": ["https://real-debrid.com/d/FILE3"]
                })
                .to_string(),
            );
        }

        if req.method == "POST" && req.path_and_query == "/torrents/selectFiles/rd_torrent_42" {
            assert_eq!(req.body, "files=3");
            return (204, "application/json", String::new());
        }

        if req.method == "POST" && req.path_and_query == "/unrestrict/link" {
            let dl = format!("http://{}/cdn/Show.S01E02.1080p.mkv", req.headers["host"]);
            return (
                200,
                "application/json",
                serde_json::json!({
                    "id": "unres_1",
                    "filename": "Show.S01E02.1080p.mkv",
                    "download": dl
                })
                .to_string(),
            );
        }

        if req.method == "DELETE" && req.path_and_query == "/torrents/delete/rd_torrent_42" {
            return (204, "application/json", String::new());
        }

        (404, "application/json", r#"{"error":"not_found"}"#.to_owned())
    })
    .await;

    let resolver = LocalDebridResolver::with_base_urls(DebridBaseUrls {
        real_debrid: base.clone(),
        ..Default::default()
    });

    let account = resolver
        .verify_account(LocalDebridProvider::RealDebrid, "rd_live_secret_9999")
        .await
        .unwrap();
    assert_eq!(account.username, "ferrite_rd");
    assert!(account.premium);

    // Cache check succeeds when 200.
    let cache = resolver
        .check_cache(
            LocalDebridProvider::RealDebrid,
            "rd_live_secret_9999",
            &[info_hash],
        )
        .await;
    assert_eq!(cache.get(&info_hash), Some(&DebridCacheStatus::CachedInstant));

    // Cache check gracefully falls back on 403 when uncached in memory.
    resolver.clear_caches();
    instant_forbidden.store(1, Ordering::SeqCst);
    let cache_403 = resolver
        .check_cache(
            LocalDebridProvider::RealDebrid,
            "rd_live_secret_9999",
            &[info_hash],
        )
        .await;
    assert_eq!(
        cache_403.get(&info_hash),
        Some(&DebridCacheStatus::UncachedOrUnknown)
    );

    // Resolve S01E02 via full RD pipeline.
    let resolved = resolver
        .resolve_torrent(
            LocalDebridProvider::RealDebrid,
            "rd_live_secret_9999",
            info_hash,
            None,
            &[],
            Some((1, 2)),
        )
        .await
        .unwrap();
    assert_eq!(resolved.provider, LocalDebridProvider::RealDebrid);
    assert_eq!(resolved.filename.as_deref(), Some("Show.S01E02.1080p.mkv"));
    assert!(!resolved.from_memory_cache);
    assert!(resolver.last_resolve_latency_ms().is_some());

    // Second resolve hits 15-min memory cache without extra HTTP calls.
    let req_count_before = requests.lock().unwrap().len();
    let cached_resolved = resolver
        .resolve_torrent(
            LocalDebridProvider::RealDebrid,
            "rd_live_secret_9999",
            info_hash,
            None,
            &[],
            Some((1, 2)),
        )
        .await
        .unwrap();
    assert!(cached_resolved.from_memory_cache);
    assert_eq!(requests.lock().unwrap().len(), req_count_before);

    // Uncached torrent triggers DELETE /torrents/delete/rd_torrent_42 and returns clean error.
    resolver.clear_caches();
    uncached_mode.store(1, Ordering::SeqCst);
    let err = resolver
        .resolve_torrent(
            LocalDebridProvider::RealDebrid,
            "rd_live_secret_9999",
            info_hash,
            None,
            &[],
            None,
        )
        .await
        .unwrap_err();
    assert!(err.contains("not instantly cached"));
    assert!(!err.contains("rd_live_secret_9999"));
    assert!(requests
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.method == "DELETE" && r.path_and_query == "/torrents/delete/rd_torrent_42"));
}

#[tokio::test]
async fn torbox_verify_cache_resolve_and_secret_redaction_wire_mock() {
    let info_hash_hex = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let info_hash = parse_info_hash(info_hash_hex).unwrap();
    let fail_requestdl = Arc::new(AtomicUsize::new(0));
    let fail_clone = Arc::clone(&fail_requestdl);

    let (base, requests) = spawn_mock_server(move |req| {
        if req.method == "GET" && req.path_and_query == "/user/me" {
            return (
                200,
                "application/json",
                serde_json::json!({
                    "success": true,
                    "data": {
                        "email": "user@torbox.local",
                        "is_subscribed": true,
                        "plan": 2,
                        "premium_expires_at": "2028-02-01T00:00:00Z"
                    }
                })
                .to_string(),
            );
        }

        if req.method == "GET" && req.path_and_query.starts_with("/torrents/checkcached") {
            return (
                200,
                "application/json",
                serde_json::json!({
                    "success": true,
                    "data": {
                        info_hash_hex: { "name": "Movie.2160p", "size": 5000000 }
                    }
                })
                .to_string(),
            );
        }

        if req.method == "POST" && req.path_and_query == "/torrents/createtorrent" {
            assert!(req.body.contains("add_only_if_cached=true"));
            return (
                200,
                "application/json",
                serde_json::json!({
                    "success": true,
                    "data": { "torrent_id": 777, "hash": info_hash_hex }
                })
                .to_string(),
            );
        }

        if req.method == "GET" && req.path_and_query.starts_with("/torrents/mylist") {
            return (
                200,
                "application/json",
                serde_json::json!({
                    "success": true,
                    "data": [{
                        "id": 777,
                        "hash": info_hash_hex,
                        "files": [
                            { "id": 0, "name": "Movie.2160p.sample.mkv", "size": 1000 },
                            { "id": 1, "name": "Movie.2160p.HEVC.mkv", "size": 8000000000_u64 }
                        ]
                    }]
                })
                .to_string(),
            );
        }

        if req.method == "GET" && req.path_and_query.starts_with("/torrents/requestdl") {
            if fail_clone.load(Ordering::SeqCst) == 1 {
                return (
                    500,
                    "application/json",
                    r#"{"success":false,"detail":"failed for token=tb_secret_tok_777"}"#.to_owned(),
                );
            }
            let dl = format!("http://{}/tb/Movie.2160p.HEVC.mkv", req.headers["host"]);
            return (
                200,
                "application/json",
                serde_json::json!({ "success": true, "data": dl }).to_string(),
            );
        }

        (404, "application/json", "{}".to_owned())
    })
    .await;

    let resolver = LocalDebridResolver::with_base_urls(DebridBaseUrls {
        torbox: base,
        ..Default::default()
    });

    let account = resolver
        .verify_account(LocalDebridProvider::TorBox, "tb_secret_tok_777")
        .await
        .unwrap();
    assert_eq!(account.username, "user@torbox.local");
    assert!(account.premium);

    let cache = resolver
        .check_cache(LocalDebridProvider::TorBox, "tb_secret_tok_777", &[info_hash])
        .await;
    assert_eq!(cache.get(&info_hash), Some(&DebridCacheStatus::CachedInstant));

    let resolved = resolver
        .resolve_torrent(
            LocalDebridProvider::TorBox,
            "tb_secret_tok_777",
            info_hash,
            None,
            &[],
            None,
        )
        .await
        .unwrap();
    assert_eq!(resolved.filename.as_deref(), Some("Movie.2160p.HEVC.mkv"));
    assert!(requests
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.path_and_query.contains("file_id=1")));

    // Secret redaction check on failure.
    resolver.clear_caches();
    fail_requestdl.store(1, Ordering::SeqCst);
    let err = resolver
        .resolve_torrent(
            LocalDebridProvider::TorBox,
            "tb_secret_tok_777",
            info_hash,
            None,
            &[],
            None,
        )
        .await
        .unwrap_err();
    assert!(!err.contains("tb_secret_tok_777"));
}

#[tokio::test]
async fn alldebrid_verify_resolve_and_uncached_delete_wire_mock() {
    let info_hash_hex = "cccccccccccccccccccccccccccccccccccccccc";
    let info_hash = parse_info_hash(info_hash_hex).unwrap();
    let status_code = Arc::new(AtomicUsize::new(4));
    let status_clone = Arc::clone(&status_code);

    let (base, requests) = spawn_mock_server(move |req| {
        if req.method == "GET" && req.path_and_query.starts_with("/v4/user") {
            return (
                200,
                "application/json",
                serde_json::json!({
                    "status": "success",
                    "data": {
                        "user": {
                            "username": "ferrite_ad",
                            "isPremium": true,
                            "premiumUntil": 1893456000_i64
                        }
                    }
                })
                .to_string(),
            );
        }

        if req.method == "GET" && req.path_and_query.starts_with("/v4/magnet/instant") {
            return (
                200,
                "application/json",
                serde_json::json!({
                    "status": "success",
                    "data": {
                        "magnets": [{ "hash": info_hash_hex, "instant": true }]
                    }
                })
                .to_string(),
            );
        }

        if req.method == "POST" && req.path_and_query.starts_with("/v4/magnet/upload") {
            return (
                200,
                "application/json",
                serde_json::json!({
                    "status": "success",
                    "data": {
                        "magnets": [{ "id": 909, "hash": info_hash_hex, "ready": true }]
                    }
                })
                .to_string(),
            );
        }

        if req.method == "GET" && req.path_and_query.starts_with("/v4.1/magnet/status") {
            let code = status_clone.load(Ordering::SeqCst);
            return (
                200,
                "application/json",
                serde_json::json!({
                    "status": "success",
                    "data": {
                        "magnets": {
                            "id": 909,
                            "statusCode": code,
                            "status": if code == 4 { "Ready" } else { "Downloading" },
                            "files": [{
                                "n": "Season 01",
                                "e": [
                                    { "n": "Show.S01E03.1080p.mkv", "s": 2200000000_u64, "l": "https://alldebrid.com/f/ep3" }
                                ]
                            }]
                        }
                    }
                })
                .to_string(),
            );
        }

        if req.method == "GET" && req.path_and_query.starts_with("/v4/link/unlock") {
            let dl = format!("http://{}/ad/Show.S01E03.1080p.mkv", req.headers["host"]);
            return (
                200,
                "application/json",
                serde_json::json!({
                    "status": "success",
                    "data": {
                        "link": dl,
                        "filename": "Show.S01E03.1080p.mkv"
                    }
                })
                .to_string(),
            );
        }

        if req.method == "GET" && req.path_and_query.starts_with("/v4/magnet/delete") {
            return (
                200,
                "application/json",
                r#"{"status":"success","data":{"message":"deleted"}}"#.to_owned(),
            );
        }

        (404, "application/json", "{}".to_owned())
    })
    .await;

    let resolver = LocalDebridResolver::with_base_urls(DebridBaseUrls {
        alldebrid: base,
        ..Default::default()
    });

    let account = resolver
        .verify_account(LocalDebridProvider::AllDebrid, "ad_secret_key_8888")
        .await
        .unwrap();
    assert_eq!(account.username, "ferrite_ad");
    assert!(account.premium);
    assert!(account.expiration.is_some());

    let resolved = resolver
        .resolve_torrent(
            LocalDebridProvider::AllDebrid,
            "ad_secret_key_8888",
            info_hash,
            None,
            &[],
            Some((1, 3)),
        )
        .await
        .unwrap();
    assert_eq!(resolved.filename.as_deref(), Some("Show.S01E03.1080p.mkv"));

    // Uncached magnet statusCode != 4 must invoke /v4/magnet/delete.
    resolver.clear_caches();
    status_code.store(1, Ordering::SeqCst);
    let err = resolver
        .resolve_torrent(
            LocalDebridProvider::AllDebrid,
            "ad_secret_key_8888",
            info_hash,
            None,
            &[],
            None,
        )
        .await
        .unwrap_err();
    assert!(err.contains("not cached on AllDebrid"));
    assert!(requests
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.path_and_query.starts_with("/v4/magnet/delete")));
}

#[tokio::test]
async fn prowlarr_rest_v1_and_torznab_xml_wire_mock_with_fallback() {
    let (rest_base, rest_reqs) = spawn_mock_server(|req| {
        assert_eq!(
            req.headers.get("x-api-key").map(String::as_str),
            Some("prowlarr_api_secret_55")
        );
        if req.path_and_query == "/api/v1/indexer" {
            return (
                200,
                "application/json",
                serde_json::json!([
                    { "id": 1, "name": "1337x" },
                    { "id": 2, "name": "TorrentGalaxy" }
                ])
                .to_string(),
            );
        }
        if req.path_and_query.starts_with("/api/v1/search") {
            return (
                200,
                "application/json",
                serde_json::json!([
                    {
                        "title": "Interstellar.2014.2160p.UHD.BluRay.HDR.HEVC.Atmos-GROUP",
                        "infoHash": "dddddddddddddddddddddddddddddddddddddddd",
                        "magnetUrl": "magnet:?xt=urn:btih:dddddddddddddddddddddddddddddddddddddddd&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce",
                        "size": 32212254720_u64,
                        "seeders": 320,
                        "indexer": "1337x"
                    }
                ])
                .to_string(),
            );
        }
        (404, "application/json", "{}".to_owned())
    })
    .await;

    let client = ProwlarrClient::new();
    let rest_test = client
        .test_connection(rest_base.as_str(), "prowlarr_api_secret_55", 4500)
        .await
        .unwrap();
    assert!(rest_test.reachable);
    assert_eq!(rest_test.mode_detected, "Prowlarr REST v1");
    assert_eq!(rest_test.item_count, 2);

    let rest_items = client
        .search(
            rest_base.as_str(),
            "prowlarr_api_secret_55",
            4500,
            "movie",
            "tt0816692",
            Some("Interstellar"),
        )
        .await
        .unwrap();
    assert_eq!(rest_items.len(), 1);
    assert_eq!(rest_items[0].parsed.seeders, Some(320));
    assert_eq!(rest_items[0].parsed.indexer.as_deref(), Some("1337x"));
    assert!(rest_reqs
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.path_and_query.starts_with("/api/v1/search")));

    // Torznab XML mock: ?t=caps, ?t=tvsearch (returns empty -> triggers ?t=search&q= fallback).
    let (torznab_base, torznab_reqs) = spawn_mock_server(|req| {
        if req.path_and_query.contains("t=caps") {
            return (
                200,
                "application/xml",
                r#"<?xml version="1.0" encoding="UTF-8"?>
<caps>
  <searching><movie-search available="yes"/><tv-search available="yes"/></searching>
  <categories><category id="2000" name="Movies"/><category id="5000" name="TV"/></categories>
</caps>"#
                    .to_owned(),
            );
        }
        if req.path_and_query.contains("t=tvsearch") {
            return (
                200,
                "application/xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><rss version="2.0"><channel></channel></rss>"#.to_owned(),
            );
        }
        if req.path_and_query.contains("t=search") {
            return (
                200,
                "application/xml",
                r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">
  <channel>
    <item>
      <title>Severance.S02E01.1080p.WEB-DL.DDP5.1.H.265-NTb</title>
      <jackettindexer>Nyaa</jackettindexer>
      <torznab:attr name="magneturl" value="magnet:?xt=urn:btih:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee&amp;tr=udp%3A%2F%2Fopen.stealth.si%3A80%2Fannounce" />
      <torznab:attr name="seeders" value="95" />
      <torznab:attr name="size" value="2147483648" />
    </item>
  </channel>
</rss>"#
                    .to_owned(),
            );
        }
        (404, "text/plain", "not found".to_owned())
    })
    .await;

    let torznab_url = format!("{torznab_base}api/v2.0/indexers/all/results/torznab/api");
    let torznab_test = client
        .test_connection(&torznab_url, "torznab_secret_33", 4500)
        .await
        .unwrap();
    assert!(torznab_test.reachable);
    assert_eq!(torznab_test.mode_detected, "Torznab XML");
    assert_eq!(torznab_test.item_count, 2);

    let torznab_items = client
        .search(
            &torznab_url,
            "torznab_secret_33",
            4500,
            "series",
            "tt11280740:2:1",
            Some("Severance"),
        )
        .await
        .unwrap();
    assert_eq!(torznab_items.len(), 1);
    assert_eq!(torznab_items[0].parsed.seeders, Some(95));
    assert_eq!(torznab_items[0].parsed.indexer.as_deref(), Some("Nyaa"));
    assert!(matches!(
        torznab_items[0].stream.source,
        StreamSource::Torrent { .. }
    ));

    let reqs = torznab_reqs.lock().unwrap();
    assert!(reqs.iter().any(|r| r.path_and_query.contains("t=tvsearch") && r.path_and_query.contains("season=2") && r.path_and_query.contains("ep=1")));
    assert!(reqs.iter().any(|r| r.path_and_query.contains("t=search") && r.path_and_query.contains("Severance")));
}

#[tokio::test]
async fn core_controller_end_to_end_private_config_debrid_and_prowlarr() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("storage.db");
    let expected_priv_path = dir.path().join("private_integrations.json");
    let info_hash_hex = "ffffffffffffffffffffffffffffffffffffffff";
    let info_hash = parse_info_hash(info_hash_hex).unwrap();

    let (rd_base, _) = spawn_mock_server(move |req| {
        if req.method == "GET" && req.path_and_query == "/user" {
            return (
                200,
                "application/json",
                r#"{"username":"ctrl_user","type":"premium","expiration":"2029-01-01"}"#.to_owned(),
            );
        }
        if req.method == "GET" && req.path_and_query.starts_with("/torrents/instantAvailability/") {
            return (
                200,
                "application/json",
                serde_json::json!({
                    info_hash_hex: { "rd": [{ "1": { "filename": "Film.mkv", "filesize": 100 } }] }
                })
                .to_string(),
            );
        }
        if req.method == "POST" && req.path_and_query == "/torrents/addMagnet" {
            return (200, "application/json", r#"{"id":"ctrl_tid"}"#.to_owned());
        }
        if req.method == "GET" && req.path_and_query == "/torrents/info/ctrl_tid" {
            return (
                200,
                "application/json",
                serde_json::json!({
                    "id": "ctrl_tid",
                    "status": "downloaded",
                    "files": [{ "id": 1, "path": "/Film.1080p.mkv", "bytes": 999999, "selected": 1 }],
                    "links": ["https://real-debrid.com/d/CTRL"]
                })
                .to_string(),
            );
        }
        if req.method == "POST" && req.path_and_query == "/unrestrict/link" {
            let dl = format!("http://{}/cdn/Film.1080p.mkv", req.headers["host"]);
            return (
                200,
                "application/json",
                serde_json::json!({ "filename": "Film.1080p.mkv", "download": dl }).to_string(),
            );
        }
        (404, "application/json", "{}".to_owned())
    })
    .await;

    let (prowl_base, _) = spawn_mock_server(|req| {
        if req.path_and_query == "/api/v1/indexer" {
            return (200, "application/json", r#"[{"id":1,"name":"EZTV"}]"#.to_owned());
        }
        if req.path_and_query.starts_with("/api/v1/search") {
            return (
                200,
                "application/json",
                serde_json::json!([{
                    "title": "Film.2025.1080p.WEB-DL.x265-GRP",
                    "infoHash": "ffffffffffffffffffffffffffffffffffffffff",
                    "size": 1073741824_u64,
                    "seeders": 42,
                    "indexer": "EZTV"
                }])
                .to_string(),
            );
        }
        (404, "application/json", "{}".to_owned())
    })
    .await;

    DesktopEnv::run_isolated(EnvConfig::with_storage_path(&db_path), async move {
        let controller = CoreController::from_initialized_env().await.unwrap();
        let initial_addons_count = controller.with_model(|m| m.ctx.profile.addons.len());
        assert!(!controller.has_local_debrid_configured());
        assert!(!controller.has_debrid_addon_configured());
        assert_eq!(controller.last_debrid_resolve_latency_ms(), None);

        controller.set_debrid_base_urls(DebridBaseUrls {
            real_debrid: rd_base,
            ..Default::default()
        });

        let next = PrivateIntegrationsConfig {
            active_debrid: LocalDebridProvider::RealDebrid,
            direct_rust_unrestrict: true,
            debrid_cached_only_filter: false,
            real_debrid_api_key: SecretApiKey::new("ctrl_rd_secret_1111"),
            torbox_api_key: SecretApiKey::default(),
            alldebrid_api_key: SecretApiKey::default(),
            prowlarr_mode: ProwlarrFallbackMode::FallbackOnOutage,
            prowlarr_url: prowl_base.to_string(),
            prowlarr_api_key: SecretApiKey::new("ctrl_prowl_secret_2222"),
            prowlarr_timeout_ms: 500, // clamped to 1000
        };
        controller.update_private_integrations(next).unwrap();

        assert!(expected_priv_path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&expected_priv_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        // Keys must NEVER be pushed into Stremio addons or profile sync!
        assert_eq!(
            controller.with_model(|m| m.ctx.profile.addons.len()),
            initial_addons_count
        );
        assert!(!controller.has_debrid_addon_configured());
        let profile_json =
            controller.with_model(|m| serde_json::to_string(&m.ctx.profile).unwrap());
        assert!(!profile_json.contains("ctrl_rd_secret_1111"));
        assert!(!profile_json.contains("ctrl_prowl_secret_2222"));
        assert!(controller.has_local_debrid_configured());
        assert!(controller.has_any_debrid_configured());
        assert_eq!(controller.private_integrations().prowlarr_timeout_ms, 1000);

        let status = controller
            .verify_local_debrid(LocalDebridProvider::RealDebrid, None)
            .await
            .unwrap();
        assert_eq!(status.username, "ctrl_user");

        let cache = controller.check_local_debrid_cache(vec![info_hash]).await;
        assert_eq!(cache.get(&info_hash), Some(&DebridCacheStatus::CachedInstant));

        let resolved = controller
            .resolve_torrent_via_local_debrid(info_hash, None, vec![], None)
            .await
            .unwrap();
        assert_eq!(resolved.filename.as_deref(), Some("Film.1080p.mkv"));
        assert!(controller.last_debrid_resolve_latency_ms().is_some());

        let prowl_test = controller.test_prowlarr(None, None).await.unwrap();
        assert!(prowl_test.reachable);
        assert_eq!(prowl_test.item_count, 1);

        let streams = controller
            .fetch_prowlarr_streams("movie", "tt9999999", Some("Film"))
            .await
            .unwrap();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].parsed.seeders, Some(42));

        // When Prowlarr mode is Off, fetch_prowlarr_streams returns empty cleanly.
        let mut disabled_cfg = controller.private_integrations();
        disabled_cfg.prowlarr_mode = ProwlarrFallbackMode::Off;
        controller.update_private_integrations(disabled_cfg).unwrap();
        let off_streams = controller
            .fetch_prowlarr_streams("movie", "tt9999999", Some("Film"))
            .await
            .unwrap();
        assert!(off_streams.is_empty());
    })
    .await
    .unwrap();
}
