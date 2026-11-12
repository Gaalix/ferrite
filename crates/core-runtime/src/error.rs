use std::path::PathBuf;
use stremio_core::runtime::EnvError;

#[derive(Debug, thiserror::Error)]
pub enum CoreRuntimeError {
    #[error("DesktopEnv has already been initialized with a different storage path (existing: {existing:?}, requested: {requested:?})")]
    AlreadyInitialized {
        existing: PathBuf,
        requested: PathBuf,
    },

    #[error("DesktopEnv has not been initialized; call DesktopEnv::init or CoreController::boot first")]
    NotInitialized,

    #[error("Failed to initialize Tokio runtime: {0}")]
    TokioInit(#[source] std::io::Error),

    #[error("Failed to create storage directory {path:?}: {source}")]
    StorageDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("HTTP client initialization error: {0}")]
    HttpClient(#[source] reqwest::Error),

    #[error("Core environment error: {0}")]
    Env(String),
}

impl From<reqwest::Error> for CoreRuntimeError {
    fn from(err: reqwest::Error) -> Self {
        Self::HttpClient(err.without_url())
    }
}

impl From<EnvError> for CoreRuntimeError {
    fn from(err: EnvError) -> Self {
        Self::Env(err.message())
    }
}
