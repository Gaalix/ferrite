use std::{
    fmt,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::addon_engine::redact_sensitive_text;

pub const DEFAULT_PROWLARR_TIMEOUT_MS: u64 = 4_500;
pub const MIN_PROWLARR_TIMEOUT_MS: u64 = 1_000;
pub const MAX_PROWLARR_TIMEOUT_MS: u64 = 15_000;

static TEMP_FILE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Local Rust Debrid provider selected by the user (keys stay strictly local in `0600` config).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LocalDebridProvider {
    #[default]
    None,
    RealDebrid,
    TorBox,
    AllDebrid,
}

impl LocalDebridProvider {
    pub const ALL: [LocalDebridProvider; 4] = [
        LocalDebridProvider::None,
        LocalDebridProvider::RealDebrid,
        LocalDebridProvider::TorBox,
        LocalDebridProvider::AllDebrid,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Off",
            Self::RealDebrid => "Real-Debrid",
            Self::TorBox => "TorBox",
            Self::AllDebrid => "AllDebrid",
        }
    }

    pub fn short_badge(self) -> &'static str {
        match self {
            Self::None => "",
            Self::RealDebrid => "RD+",
            Self::TorBox => "TB+",
            Self::AllDebrid => "AD+",
        }
    }

    pub fn is_enabled(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Controls when the local Prowlarr / Torznab indexer fallback is queried.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProwlarrFallbackMode {
    Off,
    #[default]
    FallbackOnOutage,
    AlwaysMerge,
}

impl ProwlarrFallbackMode {
    pub const ALL: [ProwlarrFallbackMode; 3] = [
        ProwlarrFallbackMode::Off,
        ProwlarrFallbackMode::FallbackOnOutage,
        ProwlarrFallbackMode::AlwaysMerge,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::FallbackOnOutage => "Fallback on Outage",
            Self::AlwaysMerge => "Always Merge",
        }
    }
}

/// Zeroized-on-drop wrapper for local API keys stored only in the `0600` private integrations file.
#[derive(Clone, Default, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct SecretApiKey(String);

impl SecretApiKey {
    pub fn new(raw: impl Into<String>) -> Self {
        let mut s = raw.into();
        let trimmed = s.trim().to_owned();
        s.zeroize();
        Self(trimmed)
    }

    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }

    pub fn expose(&self) -> &str {
        self.0.trim()
    }

    pub fn masked_preview(&self) -> String {
        let trimmed = self.0.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        let char_count = trimmed.chars().count();
        if char_count <= 4 {
            return "••••••••".to_owned();
        }
        let tail: String = trimmed.chars().skip(char_count - 4).collect();
        format!("••••••••{tail}")
    }
}

impl fmt::Debug for SecretApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretApiKey([REDACTED])")
    }
}

impl Serialize for SecretApiKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.expose())
    }
}

impl<'de> Deserialize<'de> for SecretApiKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::new(raw))
    }
}

fn default_true() -> bool {
    true
}

fn default_prowlarr_timeout_ms() -> u64 {
    DEFAULT_PROWLARR_TIMEOUT_MS
}

/// Local-only private integrations configuration persisted to `<config_dir>/ferrite/private_integrations.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivateIntegrationsConfig {
    pub active_debrid: LocalDebridProvider,
    #[serde(default = "default_true")]
    pub direct_rust_unrestrict: bool,
    pub debrid_cached_only_filter: bool,
    pub real_debrid_api_key: SecretApiKey,
    pub torbox_api_key: SecretApiKey,
    pub alldebrid_api_key: SecretApiKey,
    pub prowlarr_mode: ProwlarrFallbackMode,
    pub prowlarr_url: String,
    pub prowlarr_api_key: SecretApiKey,
    #[serde(default = "default_prowlarr_timeout_ms")]
    pub prowlarr_timeout_ms: u64,
}

impl Default for PrivateIntegrationsConfig {
    fn default() -> Self {
        Self {
            active_debrid: LocalDebridProvider::None,
            direct_rust_unrestrict: true,
            debrid_cached_only_filter: false,
            real_debrid_api_key: SecretApiKey::default(),
            torbox_api_key: SecretApiKey::default(),
            alldebrid_api_key: SecretApiKey::default(),
            prowlarr_mode: ProwlarrFallbackMode::FallbackOnOutage,
            prowlarr_url: String::new(),
            prowlarr_api_key: SecretApiKey::default(),
            prowlarr_timeout_ms: DEFAULT_PROWLARR_TIMEOUT_MS,
        }
    }
}

impl PrivateIntegrationsConfig {
    pub fn normalized(mut self) -> Self {
        self.prowlarr_timeout_ms = self
            .prowlarr_timeout_ms
            .clamp(MIN_PROWLARR_TIMEOUT_MS, MAX_PROWLARR_TIMEOUT_MS);
        self.prowlarr_url = self.prowlarr_url.trim().trim_end_matches('/').to_owned();
        self.real_debrid_api_key = SecretApiKey::new(self.real_debrid_api_key.expose());
        self.torbox_api_key = SecretApiKey::new(self.torbox_api_key.expose());
        self.alldebrid_api_key = SecretApiKey::new(self.alldebrid_api_key.expose());
        self.prowlarr_api_key = SecretApiKey::new(self.prowlarr_api_key.expose());
        self
    }

    pub fn key_for_provider(&self, provider: LocalDebridProvider) -> Option<&SecretApiKey> {
        match provider {
            LocalDebridProvider::None => None,
            LocalDebridProvider::RealDebrid => Some(&self.real_debrid_api_key),
            LocalDebridProvider::TorBox => Some(&self.torbox_api_key),
            LocalDebridProvider::AllDebrid => Some(&self.alldebrid_api_key),
        }
    }

    pub fn active_debrid_key(&self) -> Option<&SecretApiKey> {
        self.key_for_provider(self.active_debrid)
    }

    pub fn has_local_debrid_configured(&self) -> bool {
        self.active_debrid.is_enabled()
            && self
                .active_debrid_key()
                .is_some_and(|key| !key.is_empty())
    }

    pub fn is_prowlarr_configured(&self) -> bool {
        !matches!(self.prowlarr_mode, ProwlarrFallbackMode::Off)
            && !self.prowlarr_url.trim().is_empty()
    }

    pub fn all_secrets(&self) -> Vec<&str> {
        [
            self.real_debrid_api_key.expose(),
            self.torbox_api_key.expose(),
            self.alldebrid_api_key.expose(),
            self.prowlarr_api_key.expose(),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect()
    }

    pub fn default_config_path() -> PathBuf {
        default_config_path()
    }

    pub fn save_to_path(&self, path: &Path) -> Result<(), String> {
        save_to_path(self, path)
    }

    pub fn load_from_path(path: &Path) -> Result<Self, String> {
        load_from_path(path)
    }
}

/// Redacts URLs, known auth patterns, and any explicit secrets (raw or URL-encoded) from text.
pub fn redact_secret(input: &str, secrets: &[&str]) -> String {
    let mut out = redact_sensitive_text(input);
    for secret in secrets {
        let trimmed = secret.trim();
        if trimmed.is_empty() {
            continue;
        }
        out = out.replace(trimmed, "[REDACTED]");
        let encoded = urlencoding::encode(trimmed);
        if !encoded.is_empty() && encoded.as_ref() != trimmed {
            out = out.replace(encoded.as_ref(), "[REDACTED]");
        }
    }
    out
}

pub fn default_config_path() -> PathBuf {
    if let Some(cfg_dir) = dirs::config_dir() {
        return cfg_dir.join("ferrite").join("private_integrations.json");
    }
    if let Some(proj_dirs) = directories::ProjectDirs::from("io", "ferrite", "ferrite") {
        return proj_dirs.config_dir().join("private_integrations.json");
    }
    std::env::temp_dir()
        .join("ferrite")
        .join("private_integrations.json")
}

#[cfg(unix)]
fn is_shared_system_dir(dir: &Path) -> bool {
    let temp_root = std::env::temp_dir();
    dir == Path::new("/")
        || dir == Path::new("/tmp")
        || dir == Path::new("/var/tmp")
        || dir == temp_root.as_path()
}

fn ensure_private_parent_dir(parent: &Path) -> std::io::Result<()> {
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        if !is_shared_system_dir(parent) {
            builder.mode(0o700);
        }
        builder.create(parent)?;
        if !is_shared_system_dir(parent) {
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(parent)
    }
}

/// Atomically persists private integrations config with `0600` permissions on Unix.
pub fn save_to_path(config: &PrivateIntegrationsConfig, path: &Path) -> Result<(), String> {
    let normalized = config.clone().normalized();
    let secrets = normalized.all_secrets();

    if let Some(parent) = path.parent() {
        ensure_private_parent_dir(parent).map_err(|err| {
            redact_secret(
                &format!("Failed to prepare private config directory: {err}"),
                &secrets,
            )
        })?;
    }

    let mut payload = serde_json::to_vec_pretty(&normalized).map_err(|err| {
        redact_secret(
            &format!("Failed to serialize private config: {err}"),
            &secrets,
        )
    })?;
    payload.push(b'\n');

    let seq = TEMP_FILE_SEQ.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("private_integrations.json");
    let tmp_path = path.with_file_name(format!(".{file_name}.{}.{}.tmp", std::process::id(), seq));

    let write_res: std::io::Result<()> = (|| {
        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp_path)?;
            std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600))?;
            f
        };
        #[cfg(not(unix))]
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;

        file.write_all(&payload)?;
        file.sync_all()?;
        drop(file);

        std::fs::rename(&tmp_path, path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    })();

    payload.zeroize();

    if let Err(err) = write_res {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(redact_secret(
            &format!("Failed to save private config: {err}"),
            &secrets,
        ));
    }

    Ok(())
}

/// Loads private integrations config from disk (returning defaults if the file does not exist).
pub fn load_from_path(path: &Path) -> Result<PrivateIntegrationsConfig, String> {
    let mut raw = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(PrivateIntegrationsConfig::default());
        }
        Err(err) => {
            return Err(redact_sensitive_text(&format!(
                "Failed to read private config: {err}"
            )));
        }
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    let parsed = serde_json::from_str::<PrivateIntegrationsConfig>(&raw).map_err(|err| {
        redact_sensitive_text(&format!("Failed to parse private config JSON: {err}"))
    });
    raw.zeroize();

    parsed.map(PrivateIntegrationsConfig::normalized)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn secret_api_key_debug_redacts_and_masks_preview() {
        let empty = SecretApiKey::default();
        assert!(empty.is_empty());
        assert_eq!(empty.masked_preview(), "");
        assert_eq!(format!("{empty:?}"), "SecretApiKey([REDACTED])");

        let key = SecretApiKey::new("  rd_live_secret_9876  ");
        assert!(!key.is_empty());
        assert_eq!(key.expose(), "rd_live_secret_9876");
        assert_eq!(key.masked_preview(), "••••••••9876");
        assert_eq!(format!("{key:?}"), "SecretApiKey([REDACTED])");
        assert!(!format!("{key:?}").contains("9876"));

        let short = SecretApiKey::new("abc");
        assert_eq!(short.masked_preview(), "••••••••");
    }

    #[test]
    fn config_clamps_timeout_and_persists_with_0600_permissions() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("ferrite_priv").join("private_integrations.json");

        let cfg = PrivateIntegrationsConfig {
            active_debrid: LocalDebridProvider::RealDebrid,
            direct_rust_unrestrict: true,
            debrid_cached_only_filter: true,
            real_debrid_api_key: SecretApiKey::new("rd_secret_abcd"),
            torbox_api_key: SecretApiKey::new("tb_secret_1234"),
            alldebrid_api_key: SecretApiKey::default(),
            prowlarr_mode: ProwlarrFallbackMode::AlwaysMerge,
            prowlarr_url: "http://127.0.0.1:9696/".to_owned(),
            prowlarr_api_key: SecretApiKey::new("prowl_secret_5678"),
            prowlarr_timeout_ms: 99_999,
        };

        cfg.save_to_path(&nested).unwrap();
        let loaded = PrivateIntegrationsConfig::load_from_path(&nested).unwrap();
        assert_eq!(loaded.active_debrid, LocalDebridProvider::RealDebrid);
        assert_eq!(loaded.prowlarr_timeout_ms, MAX_PROWLARR_TIMEOUT_MS);
        assert_eq!(loaded.prowlarr_url, "http://127.0.0.1:9696");
        assert_eq!(loaded.real_debrid_api_key.expose(), "rd_secret_abcd");
        assert!(loaded.has_local_debrid_configured());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file_mode = std::fs::metadata(&nested).unwrap().permissions().mode() & 0o777;
            assert_eq!(file_mode, 0o600);
            let dir_mode = std::fs::metadata(nested.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700);
        }
    }

    #[test]
    fn redact_secret_strips_raw_and_encoded_tokens() {
        let msg = "Request failed for https://api.torbox.app/requestdl?token=my+secret%2Fkey&id=1 with key my+secret/key";
        let redacted = redact_secret(msg, &["my+secret/key"]);
        assert!(!redacted.contains("my+secret/key"));
        assert!(!redacted.contains("my%2Bsecret%2Fkey"));
    }
}
