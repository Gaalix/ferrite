use std::{
    any::TypeId,
    ffi::OsString,
    future::Future,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use chrono::{DateTime, Utc};
use futures::{future, try_join, FutureExt};
use http::{Method, Request};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use stremio_core::{
    constants::{
        DISMISSED_EVENTS_STORAGE_KEY, LIBRARY_RECENT_STORAGE_KEY, LIBRARY_STORAGE_KEY,
        NOTIFICATIONS_STORAGE_KEY, PROFILE_STORAGE_KEY, SEARCH_HISTORY_STORAGE_KEY,
        STREAMING_SERVER_URLS_STORAGE_KEY, STREAMS_STORAGE_KEY,
    },
    models::{ctx::Ctx, streaming_server::StreamingServer},
    runtime::{ConditionalSend, Env, EnvError, EnvFuture, EnvFutureExt, TryEnvFuture},
    types::{
        events::DismissedEventsBucket, library::LibraryBucket, notifications::NotificationsBucket,
        profile::Profile, search_history::SearchHistoryBucket, server_urls::ServerUrlsBucket,
        streams::StreamsBucket,
    },
};
use tokio::sync::mpsc;
use tracing::{debug, error};
use url::Url;

#[cfg(debug_assertions)]
use crate::addon_engine::redact_sensitive_text;
use crate::{
    addon_engine::{
        addon_origin_from_url, AddonCacheStats, AddonEngineConfig, AddonHttpEngine,
        AddonTelemetrySnapshot, MutedAddonEntry, DEFAULT_CIRCUIT_BREAKER_COOLDOWN,
    },
    error::CoreRuntimeError,
};

const DEFAULT_APP_DIR_NAME: &str = "ferrite";
const LEGACY_APP_DIR_NAME: &str = "stremio-gpui";
const DEFAULT_DB_FILENAME: &str = "storage.db";
const DEFAULT_LEGACY_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_millis(2_500);

type BoxedTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

static GLOBAL_ENV: OnceLock<Arc<EnvState>> = OnceLock::new();

tokio::task_local! {
    static SCOPED_ENV: Arc<EnvState>;
}

pub type DesktopEnvConfig = EnvConfig;

pub fn is_loopback_streaming_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && matches!(
            url.host_str(),
            Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
        )
}

#[derive(Clone, Debug)]
pub struct EnvConfig {
    pub storage_path: Option<PathBuf>,
    pub streaming_server_url: Option<Url>,
    pub http_timeout: Duration,
    pub timeout_override: Option<Duration>,
    pub connect_timeout: Duration,
    pub circuit_breaker_cooldown: Duration,
    pub treat_loopback_as_remote_addon: bool,
    pub user_agent: String,
    pub runtime_handle: Option<tokio::runtime::Handle>,
}

impl Default for EnvConfig {
    fn default() -> Self {
        Self {
            storage_path: None,
            streaming_server_url: None,
            http_timeout: DEFAULT_LEGACY_HTTP_TIMEOUT,
            timeout_override: None,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            circuit_breaker_cooldown: DEFAULT_CIRCUIT_BREAKER_COOLDOWN,
            treat_loopback_as_remote_addon: false,
            user_agent: format!("Ferrite/{}", env!("CARGO_PKG_VERSION")),
            runtime_handle: None,
        }
    }
}

impl EnvConfig {
    pub fn with_storage_path(path: impl Into<PathBuf>) -> Self {
        Self {
            storage_path: Some(path.into()),
            ..Self::default()
        }
    }

    pub fn with_streaming_server_url(mut self, url: Url) -> Self {
        self.streaming_server_url = Some(url);
        self
    }

    pub fn with_runtime_handle(mut self, handle: tokio::runtime::Handle) -> Self {
        self.runtime_handle = Some(handle);
        self
    }

    pub fn with_timeout_override(mut self, timeout: Duration) -> Self {
        self.timeout_override = Some(timeout);
        self
    }

    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn with_circuit_breaker_cooldown(mut self, cooldown: Duration) -> Self {
        self.circuit_breaker_cooldown = cooldown;
        self
    }

    pub fn with_loopback_addon_cache_and_circuit(mut self, enabled: bool) -> Self {
        self.treat_loopback_as_remote_addon = enabled;
        self
    }

    pub fn effective_storage_path(&self) -> PathBuf {
        if let Some(path) = &self.storage_path {
            return path.clone();
        }
        default_storage_path()
    }

    fn effective_timeout_override(&self) -> Option<Duration> {
        self.timeout_override.or_else(|| {
            (self.http_timeout != DEFAULT_LEGACY_HTTP_TIMEOUT).then_some(self.http_timeout)
        })
    }
}

pub fn default_storage_path() -> PathBuf {
    if let Some(data_local) = dirs::data_local_dir() {
        return data_local.join(DEFAULT_APP_DIR_NAME).join(DEFAULT_DB_FILENAME);
    }
    if let Some(proj_dirs) = directories::ProjectDirs::from("io", "ferrite", DEFAULT_APP_DIR_NAME) {
        return proj_dirs.data_local_dir().join(DEFAULT_DB_FILENAME);
    }
    std::env::temp_dir()
        .join(DEFAULT_APP_DIR_NAME)
        .join(DEFAULT_DB_FILENAME)
}

pub fn legacy_storage_path() -> Option<PathBuf> {
    if let Some(data_local) = dirs::data_local_dir() {
        return Some(data_local.join(LEGACY_APP_DIR_NAME).join(DEFAULT_DB_FILENAME));
    }
    directories::ProjectDirs::from("com", "stremio", LEGACY_APP_DIR_NAME)
        .map(|proj_dirs| proj_dirs.data_local_dir().join(DEFAULT_DB_FILENAME))
}

fn companion_sqlite_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut os: OsString = db_path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

#[cfg(unix)]
fn is_shared_system_dir(dir: &Path) -> bool {
    let temp_root = std::env::temp_dir();
    dir == Path::new("/")
        || dir == Path::new("/tmp")
        || dir == Path::new("/var/tmp")
        || dir == temp_root.as_path()
}

fn ensure_private_data_dir(dir: &Path) -> std::io::Result<()> {
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        if !is_shared_system_dir(dir) {
            builder.mode(0o700);
        }
        builder.create(dir)?;
        if !is_shared_system_dir(dir) {
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

fn precreate_private_db_file(path: &Path) -> std::io::Result<()> {
    if path == Path::new(":memory:") || path.as_os_str().is_empty() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let _file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn tighten_sqlite_file_permissions(db_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if db_path == Path::new(":memory:") || db_path.as_os_str().is_empty() {
            return;
        }
        if db_path.exists() {
            let _ = std::fs::set_permissions(db_path, std::fs::Permissions::from_mode(0o600));
        }
        for suffix in ["-wal", "-shm", "-journal"] {
            let companion = companion_sqlite_path(db_path, suffix);
            if companion.exists() {
                let _ =
                    std::fs::set_permissions(&companion, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = db_path;
    }
}

fn copy_file_private(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let bytes = std::fs::read(src)?;
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(dst)?;
        out.write_all(&bytes)?;
        out.sync_all()?;
        std::fs::set_permissions(dst, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::copy(src, dst).map(|_| ())
    }
}

pub fn migrate_legacy_storage_file(legacy_db: &Path, target_db: &Path) -> std::io::Result<bool> {
    if target_db.exists() || !legacy_db.is_file() || legacy_db == target_db {
        return Ok(false);
    }
    if let Some(parent) = target_db.parent() {
        ensure_private_data_dir(parent)?;
    }
    copy_file_private(legacy_db, target_db)?;
    for suffix in ["-wal", "-shm"] {
        let src_companion = companion_sqlite_path(legacy_db, suffix);
        if src_companion.is_file() {
            let dst_companion = companion_sqlite_path(target_db, suffix);
            let _ = copy_file_private(&src_companion, &dst_companion);
        }
    }
    tighten_sqlite_file_permissions(target_db);
    Ok(true)
}

#[derive(Clone)]
pub struct SqliteStorage {
    path: PathBuf,
    conn: Arc<Mutex<Connection>>,
}

impl std::fmt::Debug for SqliteStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStorage")
            .field("path", &self.path)
            .finish()
    }
}

impl SqliteStorage {
    pub fn open(path: &Path) -> Result<Self, CoreRuntimeError> {
        let is_memory = path == Path::new(":memory:");
        if !is_memory {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    ensure_private_data_dir(parent).map_err(|source| {
                        CoreRuntimeError::StorageDir {
                            path: parent.to_path_buf(),
                            source,
                        }
                    })?;
                }
            }

            if !path.exists() && path == default_storage_path() {
                if let Some(legacy) = legacy_storage_path() {
                    let _ = migrate_legacy_storage_file(&legacy, path);
                }
            }

            let _ = precreate_private_db_file(path);
        }

        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;
             CREATE TABLE IF NOT EXISTS kv_storage (
                 key TEXT PRIMARY KEY NOT NULL,
                 value TEXT NOT NULL,
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS addon_muted (
                 transport_url TEXT PRIMARY KEY NOT NULL,
                 addon_id TEXT NOT NULL,
                 descriptor_json TEXT NOT NULL,
                 preferred_index INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL
             );",
        )?;

        if !is_memory {
            tighten_sqlite_file_permissions(path);
        }

        Ok(Self {
            path: path.to_path_buf(),
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get_raw(&self, key: &str) -> Result<Option<String>, EnvError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| EnvError::StorageReadError("SQLite mutex poisoned".to_owned()))?;
        conn.query_row(
            "SELECT value FROM kv_storage WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|err| EnvError::StorageReadError(err.to_string()))
    }

    pub fn set_raw(&self, key: &str, value: Option<&str>) -> Result<(), EnvError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| EnvError::StorageWriteError("SQLite mutex poisoned".to_owned()))?;
        match value {
            Some(val) => {
                let now_ms = Utc::now().timestamp_millis();
                conn.execute(
                    "INSERT INTO kv_storage (key, value, updated_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(key) DO UPDATE SET
                         value = excluded.value,
                         updated_at = excluded.updated_at",
                    params![key, val, now_ms],
                )
                .map_err(|err| EnvError::StorageWriteError(err.to_string()))?;
            }
            None => {
                conn.execute("DELETE FROM kv_storage WHERE key = ?1", params![key])
                    .map_err(|err| EnvError::StorageWriteError(err.to_string()))?;
            }
        }
        tighten_sqlite_file_permissions(&self.path);
        Ok(())
    }

    pub fn get_json<T>(&self, key: &str) -> Result<Option<T>, EnvError>
    where
        T: for<'de> Deserialize<'de>,
    {
        match self.get_raw(key)? {
            Some(raw) => serde_json::from_str::<T>(&raw)
                .map(Some)
                .map_err(EnvError::from),
            None => Ok(None),
        }
    }

    pub fn update_json<T, R>(
        &self,
        key: &str,
        f: impl FnOnce(&mut T) -> R,
    ) -> Result<R, EnvError>
    where
        T: Serialize + for<'de> Deserialize<'de> + Default,
    {
        let conn = self
            .conn
            .lock()
            .map_err(|_| EnvError::StorageWriteError("SQLite mutex poisoned".to_owned()))?;

        let existing_raw = conn
            .query_row(
                "SELECT value FROM kv_storage WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|err| EnvError::StorageReadError(err.to_string()))?;

        let mut value: T = match existing_raw {
            Some(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            None => T::default(),
        };

        let output = f(&mut value);
        let serialized = serde_json::to_string(&value).map_err(EnvError::from)?;
        let now_ms = Utc::now().timestamp_millis();

        conn.execute(
            "INSERT INTO kv_storage (key, value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET
                 value = excluded.value,
                 updated_at = excluded.updated_at",
            params![key, serialized, now_ms],
        )
        .map_err(|err| EnvError::StorageWriteError(err.to_string()))?;

        tighten_sqlite_file_permissions(&self.path);
        Ok(output)
    }

    pub fn list_muted_addons(&self) -> Result<Vec<MutedAddonEntry>, EnvError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| EnvError::StorageReadError("SQLite mutex poisoned".to_owned()))?;
        let mut stmt = conn
            .prepare(
                "SELECT descriptor_json, preferred_index
                 FROM addon_muted
                 ORDER BY preferred_index ASC, updated_at ASC",
            )
            .map_err(|err| EnvError::StorageReadError(err.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                let json_str: String = row.get(0)?;
                let idx_i64: i64 = row.get(1)?;
                Ok((json_str, idx_i64.max(0) as usize))
            })
            .map_err(|err| EnvError::StorageReadError(err.to_string()))?;

        let mut entries = Vec::new();
        for row_res in rows {
            let (json_str, preferred_index) =
                row_res.map_err(|err| EnvError::StorageReadError(err.to_string()))?;
            if let Ok(descriptor) = serde_json::from_str(&json_str) {
                entries.push(MutedAddonEntry {
                    descriptor,
                    preferred_index,
                });
            }
        }
        Ok(entries)
    }

    pub fn upsert_muted_addon(&self, entry: &MutedAddonEntry) -> Result<(), EnvError> {
        let descriptor_json = serde_json::to_string(&entry.descriptor).map_err(EnvError::from)?;
        let now_ms = Utc::now().timestamp_millis();
        let transport_str = entry.descriptor.transport_url.to_string();
        let addon_id = entry.descriptor.manifest.id.as_str();

        let conn = self
            .conn
            .lock()
            .map_err(|_| EnvError::StorageWriteError("SQLite mutex poisoned".to_owned()))?;

        conn.execute(
            "DELETE FROM addon_muted WHERE addon_id = ?1 AND transport_url != ?2",
            params![addon_id, transport_str],
        )
        .map_err(|err| EnvError::StorageWriteError(err.to_string()))?;

        conn.execute(
            "INSERT INTO addon_muted (transport_url, addon_id, descriptor_json, preferred_index, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(transport_url) DO UPDATE SET
                 addon_id = excluded.addon_id,
                 descriptor_json = excluded.descriptor_json,
                 preferred_index = excluded.preferred_index,
                 updated_at = excluded.updated_at",
            params![
                transport_str,
                addon_id,
                descriptor_json,
                entry.preferred_index as i64,
                now_ms
            ],
        )
        .map_err(|err| EnvError::StorageWriteError(err.to_string()))?;

        tighten_sqlite_file_permissions(&self.path);
        Ok(())
    }

    pub fn take_muted_addon_by_url(
        &self,
        transport_url: &Url,
    ) -> Result<Option<MutedAddonEntry>, EnvError> {
        let key = transport_url.as_str();
        let conn = self
            .conn
            .lock()
            .map_err(|_| EnvError::StorageWriteError("SQLite mutex poisoned".to_owned()))?;

        let found = conn
            .query_row(
                "SELECT descriptor_json, preferred_index FROM addon_muted WHERE transport_url = ?1",
                params![key],
                |row| {
                    let json_str: String = row.get(0)?;
                    let idx_i64: i64 = row.get(1)?;
                    Ok((json_str, idx_i64.max(0) as usize))
                },
            )
            .optional()
            .map_err(|err| EnvError::StorageReadError(err.to_string()))?;

        let Some((json_str, preferred_index)) = found else {
            return Ok(None);
        };

        conn.execute(
            "DELETE FROM addon_muted WHERE transport_url = ?1",
            params![key],
        )
        .map_err(|err| EnvError::StorageWriteError(err.to_string()))?;

        let descriptor = serde_json::from_str(&json_str).map_err(EnvError::from)?;
        Ok(Some(MutedAddonEntry {
            descriptor,
            preferred_index,
        }))
    }

    pub fn take_muted_addon_by_id(
        &self,
        addon_id: &str,
    ) -> Result<Option<MutedAddonEntry>, EnvError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| EnvError::StorageWriteError("SQLite mutex poisoned".to_owned()))?;

        let found = conn
            .query_row(
                "SELECT transport_url, descriptor_json, preferred_index
                 FROM addon_muted
                 WHERE addon_id = ?1
                 ORDER BY updated_at DESC
                 LIMIT 1",
                params![addon_id],
                |row| {
                    let t_url: String = row.get(0)?;
                    let json_str: String = row.get(1)?;
                    let idx_i64: i64 = row.get(2)?;
                    Ok((t_url, json_str, idx_i64.max(0) as usize))
                },
            )
            .optional()
            .map_err(|err| EnvError::StorageReadError(err.to_string()))?;

        let Some((_, json_str, preferred_index)) = found else {
            return Ok(None);
        };

        conn.execute(
            "DELETE FROM addon_muted WHERE addon_id = ?1",
            params![addon_id],
        )
        .map_err(|err| EnvError::StorageWriteError(err.to_string()))?;

        let descriptor = serde_json::from_str(&json_str).map_err(EnvError::from)?;
        Ok(Some(MutedAddonEntry {
            descriptor,
            preferred_index,
        }))
    }

    pub fn remove_muted_addon_by_url(&self, transport_url: &Url) -> Result<bool, EnvError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| EnvError::StorageWriteError("SQLite mutex poisoned".to_owned()))?;
        let deleted = conn
            .execute(
                "DELETE FROM addon_muted WHERE transport_url = ?1",
                params![transport_url.as_str()],
            )
            .map_err(|err| EnvError::StorageWriteError(err.to_string()))?;
        Ok(deleted > 0)
    }
}

struct EnvState {
    client: reqwest::Client,
    addon_engine: Arc<AddonHttpEngine>,
    handle: tokio::runtime::Handle,
    _owned_runtime: Option<tokio::runtime::Runtime>,
    sequential_tx: mpsc::UnboundedSender<BoxedTask>,
    storage: SqliteStorage,
    storage_path: PathBuf,
    private_config_path: PathBuf,
    streaming_server_url: Option<Url>,
}

impl EnvState {
    fn build(config: EnvConfig) -> Result<Arc<Self>, CoreRuntimeError> {
        let storage_path = config.effective_storage_path();
        let private_config_path = match config.storage_path.as_deref() {
            Some(custom_db) if custom_db != Path::new(":memory:") => custom_db
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| p.join("private_integrations.json"))
                .unwrap_or_else(crate::private_config::default_config_path),
            _ => crate::private_config::default_config_path(),
        };
        let storage = SqliteStorage::open(&storage_path)?;
        let timeout_override = config.effective_timeout_override();

        let client = reqwest::Client::builder()
            .user_agent(&config.user_agent)
            .connect_timeout(config.connect_timeout)
            .tcp_nodelay(true)
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .pool_max_idle_per_host(8)
            .pool_idle_timeout(Duration::from_secs(90))
            .http2_adaptive_window(true)
            .gzip(true)
            .deflate(true)
            .build()?;

        let addon_engine = Arc::new(AddonHttpEngine::new(AddonEngineConfig {
            timeout_override,
            circuit_breaker_cooldown: config.circuit_breaker_cooldown,
            treat_loopback_as_remote: config.treat_loopback_as_remote_addon,
            streaming_server_origin: config
                .streaming_server_url
                .as_ref()
                .map(addon_origin_from_url),
        }));

        let (handle, owned_runtime) = match config.runtime_handle {
            Some(handle) => (handle, None),
            None => {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(4)
                    .thread_name("ferrite-core-env")
                    .enable_all()
                    .build()
                    .map_err(CoreRuntimeError::TokioInit)?;
                let handle = runtime.handle().clone();
                (handle, Some(runtime))
            }
        };

        let (sequential_tx, mut sequential_rx) = mpsc::unbounded_channel::<BoxedTask>();
        handle.spawn(async move {
            while let Some(task) = sequential_rx.recv().await {
                if AssertUnwindSafe(task).catch_unwind().await.is_err() {
                    error!("DesktopEnv sequential task panicked");
                }
            }
        });

        Ok(Arc::new(Self {
            client,
            addon_engine,
            handle,
            _owned_runtime: owned_runtime,
            sequential_tx,
            storage,
            storage_path,
            private_config_path,
            streaming_server_url: config.streaming_server_url,
        }))
    }
}

fn current_env_arc() -> Result<Arc<EnvState>, EnvError> {
    if let Ok(scoped) = SCOPED_ENV.try_with(Arc::clone) {
        return Ok(scoped);
    }
    GLOBAL_ENV
        .get()
        .cloned()
        .ok_or(EnvError::StorageUnavailable)
}

#[derive(Clone, Debug)]
pub struct PersistedBuckets {
    pub profile: Profile,
    pub library: LibraryBucket,
    pub streams: StreamsBucket,
    pub server_urls: ServerUrlsBucket,
    pub notifications: NotificationsBucket,
    pub search_history: SearchHistoryBucket,
    pub dismissed_events: DismissedEventsBucket,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DesktopEnv;

impl DesktopEnv {
    pub fn init(config: EnvConfig) -> Result<(), CoreRuntimeError> {
        let requested_path = config.effective_storage_path();
        if let Some(existing) = GLOBAL_ENV.get() {
            if existing.storage_path == requested_path {
                debug!(
                    path = ?requested_path,
                    "DesktopEnv already initialized with matching path"
                );
                return Ok(());
            }
            return Err(CoreRuntimeError::AlreadyInitialized {
                existing: existing.storage_path.clone(),
                requested: requested_path,
            });
        }

        let state = EnvState::build(config)?;
        match GLOBAL_ENV.set(state) {
            Ok(()) => Ok(()),
            Err(built_state) => {
                std::thread::spawn(move || drop(built_state));
                let existing = GLOBAL_ENV.get().expect("OnceLock must be set");
                if existing.storage_path == requested_path {
                    Ok(())
                } else {
                    Err(CoreRuntimeError::AlreadyInitialized {
                        existing: existing.storage_path.clone(),
                        requested: requested_path,
                    })
                }
            }
        }
    }

    pub fn is_initialized() -> bool {
        SCOPED_ENV.try_with(|_| ()).is_ok() || GLOBAL_ENV.get().is_some()
    }

    pub fn runtime_handle() -> Result<tokio::runtime::Handle, CoreRuntimeError> {
        let env = current_env_arc().map_err(|_| CoreRuntimeError::NotInitialized)?;
        Ok(env.handle.clone())
    }

    pub fn storage_path() -> Result<PathBuf, CoreRuntimeError> {
        let env = current_env_arc().map_err(|_| CoreRuntimeError::NotInitialized)?;
        Ok(env.storage_path.clone())
    }

    pub fn private_config_path() -> Result<PathBuf, CoreRuntimeError> {
        let env = current_env_arc().map_err(|_| CoreRuntimeError::NotInitialized)?;
        Ok(env.private_config_path.clone())
    }

    pub fn sqlite_storage() -> Result<SqliteStorage, CoreRuntimeError> {
        let env = current_env_arc().map_err(|_| CoreRuntimeError::NotInitialized)?;
        Ok(env.storage.clone())
    }

    pub fn http_client() -> Result<reqwest::Client, CoreRuntimeError> {
        let env = current_env_arc().map_err(|_| CoreRuntimeError::NotInitialized)?;
        Ok(env.client.clone())
    }

    pub fn addon_engine() -> Result<Arc<AddonHttpEngine>, CoreRuntimeError> {
        let env = current_env_arc().map_err(|_| CoreRuntimeError::NotInitialized)?;
        Ok(Arc::clone(&env.addon_engine))
    }

    pub fn clear_addon_cache() {
        if let Ok(env) = current_env_arc() {
            env.addon_engine.clear_cache();
        }
    }

    pub fn addon_cache_stats() -> AddonCacheStats {
        current_env_arc()
            .map(|env| env.addon_engine.cache_stats())
            .unwrap_or_default()
    }

    pub fn addon_telemetry() -> std::collections::HashMap<String, AddonTelemetrySnapshot> {
        current_env_arc()
            .map(|env| env.addon_engine.telemetry_by_origin())
            .unwrap_or_default()
    }

    pub fn reset_addon_circuit(transport_url_or_origin: &str) -> bool {
        current_env_arc()
            .map(|env| env.addon_engine.reset_circuit(transport_url_or_origin))
            .unwrap_or(false)
    }

    pub async fn run_isolated<F, T>(mut config: EnvConfig, fut: F) -> Result<T, CoreRuntimeError>
    where
        F: Future<Output = T>,
    {
        if config.runtime_handle.is_none() {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                config.runtime_handle = Some(handle);
            }
        }
        let state = EnvState::build(config)?;
        Ok(SCOPED_ENV.scope(state, fut).await)
    }

    pub async fn bootstrap_buckets() -> Result<PersistedBuckets, EnvError> {
        let env = current_env_arc()?;
        Self::migrate_storage_schema().await?;

        let (
            stored_profile,
            recent_library,
            older_library,
            streams_bucket,
            server_urls_bucket,
            notifications_bucket,
            search_history_bucket,
            dismissed_events_bucket,
        ) = try_join!(
            Self::get_storage::<Profile>(PROFILE_STORAGE_KEY),
            Self::get_storage::<LibraryBucket>(LIBRARY_RECENT_STORAGE_KEY),
            Self::get_storage::<LibraryBucket>(LIBRARY_STORAGE_KEY),
            Self::get_storage::<StreamsBucket>(STREAMS_STORAGE_KEY),
            Self::get_storage::<ServerUrlsBucket>(STREAMING_SERVER_URLS_STORAGE_KEY),
            Self::get_storage::<NotificationsBucket>(NOTIFICATIONS_STORAGE_KEY),
            Self::get_storage::<SearchHistoryBucket>(SEARCH_HISTORY_STORAGE_KEY),
            Self::get_storage::<DismissedEventsBucket>(DISMISSED_EVENTS_STORAGE_KEY),
        )?;

        let profile_was_empty = stored_profile.is_none();
        let mut profile = stored_profile.unwrap_or_default();
        if let Some(override_url) = &env.streaming_server_url {
            if (profile_was_empty
                || is_loopback_streaming_url(&profile.settings.streaming_server_url))
                && profile.settings.streaming_server_url != *override_url
            {
                profile.settings.streaming_server_url = override_url.clone();
                if !profile_was_empty {
                    let _ = Self::set_storage(PROFILE_STORAGE_KEY, Some(&profile)).await;
                }
            }
        }

        let uid = profile.uid();
        let mut library = LibraryBucket::new(uid.clone(), vec![]);
        if let Some(recent) = recent_library {
            library.merge_bucket(recent);
        }
        if let Some(older) = older_library {
            library.merge_bucket(older);
        }

        let streams = streams_bucket.unwrap_or_else(|| StreamsBucket::new(uid.clone()));

        let mut server_urls =
            server_urls_bucket.unwrap_or_else(|| ServerUrlsBucket::new::<Self>(uid.clone()));
        if let Some(override_url) = &env.streaming_server_url {
            server_urls.add_url::<Self>(override_url.clone());
        }

        let notifications = notifications_bucket
            .unwrap_or_else(|| NotificationsBucket::new::<Self>(uid.clone(), vec![]));
        let search_history =
            search_history_bucket.unwrap_or_else(|| SearchHistoryBucket::new(uid.clone()));
        let dismissed_events =
            dismissed_events_bucket.unwrap_or_else(|| DismissedEventsBucket::new(uid));

        Ok(PersistedBuckets {
            profile,
            library,
            streams,
            server_urls,
            notifications,
            search_history,
            dismissed_events,
        })
    }
}

impl Env for DesktopEnv {
    fn fetch<
        IN: Serialize + ConditionalSend + 'static,
        OUT: for<'de> Deserialize<'de> + ConditionalSend + 'static,
    >(
        request: Request<IN>,
    ) -> TryEnvFuture<OUT> {
        let env = match current_env_arc() {
            Ok(env) => env,
            Err(err) => return future::err(err).boxed_env(),
        };

        let (parts, body) = request.into_parts();
        let url = parts.uri.to_string();
        let method = parts.method;
        let headers = parts.headers;

        let serialized_body: Option<Vec<u8>> = if method != Method::GET
            && method != Method::HEAD
            && TypeId::of::<IN>() != TypeId::of::<()>()
        {
            match serde_json::to_vec(&body) {
                Ok(bytes) if bytes != b"null" => Some(bytes),
                Ok(_) => None,
                Err(err) => return future::err(EnvError::from(err)).boxed_env(),
            }
        } else {
            None
        };

        let handle = env.handle.clone();
        let client = env.client.clone();
        let addon_engine = Arc::clone(&env.addon_engine);

        async move {
            let join_res = handle
                .spawn(async move {
                    let bytes = addon_engine
                        .execute_bytes(&client, method, url, headers, serialized_body, false)
                        .await?;

                    if bytes.is_empty() && TypeId::of::<OUT>() == TypeId::of::<()>() {
                        return serde_json::from_slice::<OUT>(b"null")
                            .map_err(|err| EnvError::Fetch(err.to_string()));
                    }

                    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
                    serde_path_to_error::deserialize::<_, OUT>(&mut deserializer)
                        .map_err(|err| EnvError::Fetch(err.to_string()))
                })
                .await;

            match join_res {
                Ok(result) => result,
                Err(join_err) => Err(EnvError::Fetch(format!(
                    "HTTP fetch task aborted: {join_err}"
                ))),
            }
        }
        .boxed_env()
    }

    fn get_storage<T: for<'de> Deserialize<'de> + ConditionalSend + 'static>(
        key: &str,
    ) -> TryEnvFuture<Option<T>> {
        let env = match current_env_arc() {
            Ok(env) => env,
            Err(err) => return future::err(err).boxed_env(),
        };
        let key = key.to_owned();
        let storage = env.storage.clone();
        let handle = env.handle.clone();

        async move {
            let raw_opt = handle
                .spawn_blocking(move || storage.get_raw(&key))
                .await
                .map_err(|err| {
                    EnvError::StorageReadError(format!("Blocking task failed: {err}"))
                })??;

            match raw_opt {
                Some(json_str) => {
                    let parsed = serde_json::from_str::<T>(&json_str).map_err(EnvError::from)?;
                    Ok(Some(parsed))
                }
                None => Ok(None),
            }
        }
        .boxed_env()
    }

    fn set_storage<T: Serialize>(key: &str, value: Option<&T>) -> TryEnvFuture<()> {
        let env = match current_env_arc() {
            Ok(env) => env,
            Err(err) => return future::err(err).boxed_env(),
        };
        let serialized = match value {
            Some(val) => match serde_json::to_string(val) {
                Ok(json_str) => Some(json_str),
                Err(err) => return future::err(EnvError::from(err)).boxed_env(),
            },
            None => None,
        };
        let key = key.to_owned();
        let storage = env.storage.clone();
        let handle = env.handle.clone();

        async move {
            handle
                .spawn_blocking(move || storage.set_raw(&key, serialized.as_deref()))
                .await
                .map_err(|err| {
                    EnvError::StorageWriteError(format!("Blocking task failed: {err}"))
                })?
        }
        .boxed_env()
    }

    fn exec_concurrent<F: Future<Output = ()> + ConditionalSend + 'static>(future: F) {
        let Ok(env) = current_env_arc() else {
            error!("DesktopEnv::exec_concurrent called before DesktopEnv initialization");
            return;
        };
        let handle = env.handle.clone();
        if SCOPED_ENV.try_with(|_| ()).is_ok() {
            handle.spawn(SCOPED_ENV.scope(env, future));
        } else {
            handle.spawn(future);
        }
    }

    fn exec_sequential<F: Future<Output = ()> + ConditionalSend + 'static>(future: F) {
        let Ok(env) = current_env_arc() else {
            error!("DesktopEnv::exec_sequential called before DesktopEnv initialization");
            return;
        };
        let boxed: BoxedTask = if SCOPED_ENV.try_with(|_| ()).is_ok() {
            let scoped_env = env.clone();
            Box::pin(SCOPED_ENV.scope(scoped_env, future))
        } else {
            Box::pin(future)
        };
        if env.sequential_tx.send(boxed).is_err() {
            error!("DesktopEnv sequential task worker channel is closed");
        }
    }

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    fn flush_analytics() -> EnvFuture<'static, ()> {
        future::ready(()).boxed_env()
    }

    fn analytics_context(
        ctx: &Ctx,
        streaming_server: &StreamingServer,
        path: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "appType": "ferrite",
            "appVersion": env!("CARGO_PKG_VERSION"),
            "serverVersion": streaming_server
                .settings
                .as_ref()
                .ready()
                .map(|settings| settings.server_version.clone()),
            "appLanguage": ctx.profile.settings.interface_language,
            "url": path,
        })
    }

    #[cfg(debug_assertions)]
    fn log(message: String) {
        let sanitized = redact_sensitive_text(&message);
        debug!(target: "stremio_core", "{sanitized}");
    }
}
