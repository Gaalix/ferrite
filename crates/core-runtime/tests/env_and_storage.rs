use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use http::{header::CONTENT_TYPE, Request};
use serde::{Deserialize, Serialize};
use stremio_core::{
    constants::{
        DISMISSED_EVENTS_STORAGE_KEY, LIBRARY_RECENT_STORAGE_KEY, LIBRARY_STORAGE_KEY,
        PROFILE_STORAGE_KEY, SCHEMA_VERSION, SCHEMA_VERSION_STORAGE_KEY,
        SEARCH_HISTORY_STORAGE_KEY,
    },
    runtime::{Env, EnvError},
    types::{
        events::DismissedEventsBucket,
        library::{LibraryBucket, LibraryItem, LibraryItemState},
        profile::Profile,
        search_history::SearchHistoryBucket,
    },
};
use stremio_core_runtime::{
    default_storage_path, migrate_legacy_storage_file, DesktopEnv, EnvConfig, SqliteStorage,
};
use tempfile::tempdir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TestPayload {
    greeting: String,
    count: u32,
}

#[test]
fn ferrite_defaults_and_legacy_storage_migration_preserve_user_data_and_unix_permissions() {
    assert_eq!(EnvConfig::default().user_agent, "Ferrite/0.1.0");
    let def_path = default_storage_path();
    assert_eq!(def_path.file_name().and_then(|n| n.to_str()), Some("storage.db"));
    assert_eq!(
        def_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str()),
        Some("ferrite")
    );

    let root = tempdir().unwrap();
    let legacy_dir = root.path().join("legacy_data");
    let legacy_db = legacy_dir.join("storage.db");
    {
        let legacy_storage = SqliteStorage::open(&legacy_db).unwrap();
        legacy_storage
            .set_raw("migrated_session", Some(r#"{"greeting":"preserved","count":9}"#))
            .unwrap();
    }

    let ferrite_dir = root.path().join("ferrite_data");
    let ferrite_db = ferrite_dir.join("storage.db");

    assert!(migrate_legacy_storage_file(&legacy_db, &ferrite_db).unwrap());
    assert!(!migrate_legacy_storage_file(&legacy_db, &ferrite_db).unwrap());

    let migrated_storage = SqliteStorage::open(&ferrite_db).unwrap();
    let migrated_val: Option<TestPayload> = migrated_storage.get_json("migrated_session").unwrap();
    assert_eq!(
        migrated_val,
        Some(TestPayload {
            greeting: "preserved".to_owned(),
            count: 9,
        })
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir_mode = std::fs::metadata(&ferrite_dir).unwrap().permissions().mode() & 0o777;
        let db_mode = std::fs::metadata(&ferrite_db).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(db_mode, 0o600);

        std::fs::set_permissions(&ferrite_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&ferrite_db, std::fs::Permissions::from_mode(0o644)).unwrap();
        let reopened = SqliteStorage::open(&ferrite_db).unwrap();
        reopened.set_raw("tighten_check", Some("1")).unwrap();

        let tightened_dir_mode =
            std::fs::metadata(&ferrite_dir).unwrap().permissions().mode() & 0o777;
        let tightened_db_mode =
            std::fs::metadata(&ferrite_db).unwrap().permissions().mode() & 0o777;
        assert_eq!(tightened_dir_mode, 0o700);
        assert_eq!(tightened_db_mode, 0o600);
    }
}

#[tokio::test]
async fn global_init_sqlite_kv_and_schema_migration() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("global_init_storage.db");
    let config = EnvConfig::with_storage_path(&db_path);

    DesktopEnv::init(config.clone()).expect("initial DesktopEnv::init should succeed");
    DesktopEnv::init(config).expect("idempotent DesktopEnv::init should succeed");
    assert!(DesktopEnv::is_initialized());
    assert_eq!(DesktopEnv::storage_path().unwrap(), db_path);

    assert_eq!(
        DesktopEnv::get_storage::<TestPayload>("custom_key")
            .await
            .unwrap(),
        None
    );

    let payload = TestPayload {
        greeting: "hello ferrite".to_owned(),
        count: 42,
    };
    DesktopEnv::set_storage("custom_key", Some(&payload))
        .await
        .unwrap();

    let loaded = DesktopEnv::get_storage::<TestPayload>("custom_key")
        .await
        .unwrap();
    assert_eq!(loaded, Some(payload));

    DesktopEnv::set_storage::<TestPayload>("custom_key", None)
        .await
        .unwrap();
    assert_eq!(
        DesktopEnv::get_storage::<TestPayload>("custom_key")
            .await
            .unwrap(),
        None
    );

    DesktopEnv::migrate_storage_schema().await.unwrap();
    let version = DesktopEnv::get_storage::<u32>(SCHEMA_VERSION_STORAGE_KEY)
        .await
        .unwrap();
    assert_eq!(version, Some(SCHEMA_VERSION));
}

#[tokio::test]
async fn schema_downgrade_is_rejected_and_buckets_persist_across_reopen() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("reopen_buckets.db");

    DesktopEnv::run_isolated(EnvConfig::with_storage_path(&db_path), async {
        DesktopEnv::migrate_storage_schema().await.unwrap();

        let mut profile = Profile::default();
        profile.settings.interface_language = "fra".to_owned();
        profile.settings.subtitles_size = 125;
        DesktopEnv::set_storage(PROFILE_STORAGE_KEY, Some(&profile))
            .await
            .unwrap();

        let uid = profile.uid();
        let now = DesktopEnv::now();
        let recent_item = LibraryItem {
            id: "tt15239678".to_owned(),
            name: "Dune: Part Two".to_owned(),
            r#type: "movie".to_owned(),
            poster: None,
            poster_shape: Default::default(),
            removed: false,
            temp: false,
            ctime: Some(now),
            mtime: now,
            state: LibraryItemState::default(),
            behavior_hints: Default::default(),
        };
        let older_item = LibraryItem {
            id: "tt0133093".to_owned(),
            name: "The Matrix".to_owned(),
            r#type: "movie".to_owned(),
            poster: None,
            poster_shape: Default::default(),
            removed: false,
            temp: false,
            ctime: Some(now),
            mtime: now,
            state: LibraryItemState::default(),
            behavior_hints: Default::default(),
        };

        let recent_bucket = LibraryBucket::new(uid.clone(), vec![recent_item]);
        let older_bucket = LibraryBucket::new(uid.clone(), vec![older_item]);
        DesktopEnv::set_storage(LIBRARY_RECENT_STORAGE_KEY, Some(&recent_bucket))
            .await
            .unwrap();
        DesktopEnv::set_storage(LIBRARY_STORAGE_KEY, Some(&older_bucket))
            .await
            .unwrap();

        let mut search_history = SearchHistoryBucket::new(uid.clone());
        search_history.items.insert("dune".to_owned(), now);
        DesktopEnv::set_storage(SEARCH_HISTORY_STORAGE_KEY, Some(&search_history))
            .await
            .unwrap();

        let mut dismissed = DismissedEventsBucket::new(uid);
        dismissed
            .items
            .insert("evt_welcome".to_owned(), DesktopEnv::local_now().into());
        DesktopEnv::set_storage(DISMISSED_EVENTS_STORAGE_KEY, Some(&dismissed))
            .await
            .unwrap();
    })
    .await
    .unwrap();

    DesktopEnv::run_isolated(EnvConfig::with_storage_path(&db_path), async {
        let buckets = DesktopEnv::bootstrap_buckets().await.unwrap();
        assert_eq!(buckets.profile.settings.interface_language, "fra");
        assert_eq!(buckets.profile.settings.subtitles_size, 125);
        assert!(buckets.library.items.contains_key("tt15239678"));
        assert!(buckets.library.items.contains_key("tt0133093"));
        assert!(buckets.search_history.items.contains_key("dune"));
        assert!(buckets.dismissed_events.items.contains_key("evt_welcome"));

        let future_version = SCHEMA_VERSION + 10;
        DesktopEnv::set_storage(SCHEMA_VERSION_STORAGE_KEY, Some(&future_version))
            .await
            .unwrap();
        let err = DesktopEnv::migrate_storage_schema()
            .await
            .expect_err("downgrading schema must fail");
        assert_eq!(
            err,
            EnvError::StorageSchemaVersionDowngrade(future_version, SCHEMA_VERSION)
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn exec_sequential_runs_tasks_in_strict_fifo_order() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("fifo.db");

    DesktopEnv::run_isolated(EnvConfig::with_storage_path(&db_path), async {
        let recorded = Arc::new(Mutex::new(Vec::<u32>::new()));
        let (done_tx, done_rx) = oneshot::channel::<()>();

        {
            let recorded = recorded.clone();
            DesktopEnv::exec_sequential(async move {
                tokio::time::sleep(Duration::from_millis(60)).await;
                recorded.lock().unwrap().push(1);
            });
        }
        {
            let recorded = recorded.clone();
            DesktopEnv::exec_sequential(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                recorded.lock().unwrap().push(2);
            });
        }
        {
            let recorded = recorded.clone();
            DesktopEnv::exec_sequential(async move {
                recorded.lock().unwrap().push(3);
                let _ = done_tx.send(());
            });
        }

        tokio::time::timeout(Duration::from_secs(3), done_rx)
            .await
            .expect("sequential queue timed out")
            .unwrap();

        assert_eq!(*recorded.lock().unwrap(), vec![1, 2, 3]);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn fetch_handles_get_post_headers_status_and_json() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = vec![0_u8; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let req_text = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_text.lines().next().unwrap_or("");

                let (status, body) = if first_line.starts_with("GET /ok") {
                    assert!(req_text
                        .to_ascii_lowercase()
                        .contains("x-test-header: ferrite"));
                    assert!(req_text
                        .to_ascii_lowercase()
                        .contains("user-agent: ferrite/0.1.0"));
                    (
                        "200 OK",
                        serde_json::json!({ "greeting": "pong", "count": 7 }).to_string(),
                    )
                } else if first_line.starts_with("POST /echo") {
                    assert!(req_text
                        .to_ascii_lowercase()
                        .contains("content-type: application/json"));
                    let req_body = req_text.split("\r\n\r\n").nth(1).unwrap_or("{}");
                    ("201 Created", req_body.to_owned())
                } else {
                    ("503 Service Unavailable", r#"{"error":"down"}"#.to_owned())
                };

                let http_resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(http_resp.as_bytes()).await;
            });
        }
    });

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("fetch.db");

    DesktopEnv::run_isolated(EnvConfig::with_storage_path(&db_path), async move {
        let get_req = Request::get(format!("http://{addr}/ok"))
            .header("X-Test-Header", "ferrite")
            .body(())
            .unwrap();
        let resp: TestPayload = DesktopEnv::fetch(get_req).await.unwrap();
        assert_eq!(
            resp,
            TestPayload {
                greeting: "pong".to_owned(),
                count: 7,
            }
        );

        let sent = TestPayload {
            greeting: "echoed".to_owned(),
            count: 99,
        };
        let post_req = Request::post(format!("http://{addr}/echo"))
            .body(sent.clone())
            .unwrap();
        let echoed: TestPayload = DesktopEnv::fetch(post_req).await.unwrap();
        assert_eq!(echoed, sent);

        let fail_req = Request::get(format!("http://{addr}/secret_token_999/fail"))
            .header(CONTENT_TYPE, "application/json")
            .body(())
            .unwrap();
        let err = DesktopEnv::fetch::<_, TestPayload>(fail_req)
            .await
            .expect_err("503 should fail");
        match err {
            EnvError::Fetch(msg) => {
                assert!(msg.contains("503"));
                assert!(!msg.contains("secret_token_999"));
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    })
    .await
    .unwrap();
}
