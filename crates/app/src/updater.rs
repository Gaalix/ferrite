use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use flate2::read::GzDecoder;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

pub const CURRENT_APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_RELEASE_API_URL: &str =
    "https://api.github.com/repos/Gaalix/ferrite/releases/latest";
pub const GITHUB_RELEASES_PAGE_URL: &str = "https://github.com/Gaalix/ferrite/releases";

pub const ALLOWED_UPDATE_HOSTS: &[&str] = &[
    "api.github.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

const UPDATE_HTTP_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_CHECKSUM_BYTES: usize = 512 * 1024;
const MAX_UPDATE_ASSET_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallTargetKind {
    LinuxTarGz,
    UbuntuDeb,
    WindowsExe,
}

impl InstallTargetKind {
    pub const fn action_button_label(self) -> &'static str {
        match self {
            Self::LinuxTarGz => "Download & install update",
            Self::UbuntuDeb => "Download verified .deb",
            Self::WindowsExe => "Download & run installer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAssetInfo {
    pub name: String,
    pub download_url: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseUpdateInfo {
    pub version: String,
    pub tag_name: String,
    pub release_name: String,
    pub notes_summary: String,
    pub html_url: String,
    pub target_kind: InstallTargetKind,
    pub target_asset: ReleaseAssetInfo,
    pub checksums_asset: ReleaseAssetInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    UpToDate {
        checked_version: String,
    },
    Available(Box<ReleaseUpdateInfo>),
    Downloading {
        version: String,
    },
    ReadyToRestart {
        version: String,
    },
    DebReady {
        version: String,
        deb_path: PathBuf,
        install_cmd: String,
    },
    ApplyingWindowsInstaller {
        version: String,
    },
    Failed(String),
}

impl UpdateStatus {
    pub fn available_version(&self) -> Option<&str> {
        match self {
            Self::Available(info) => Some(info.version.as_str()),
            Self::ReadyToRestart { version } => Some(version.as_str()),
            Self::DebReady { version, .. } => Some(version.as_str()),
            _ => None,
        }
    }

    pub fn is_ready_to_restart(&self) -> bool {
        matches!(self, Self::ReadyToRestart { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppliedUpdateOutcome {
    ReadyToRestart {
        version: String,
    },
    DebDownloaded {
        version: String,
        deb_path: PathBuf,
        install_cmd: String,
    },
    WindowsInstallerPrepared {
        version: String,
        installer_path: PathBuf,
    },
}

#[derive(Debug, Deserialize)]
struct GithubReleaseResponse {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAssetResponse>,
}

#[derive(Debug, Deserialize)]
struct GithubAssetResponse {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

pub fn redact_url_for_log(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Ok(parsed) = Url::parse(trimmed) {
        let scheme = parsed.scheme();
        if let Some(host) = parsed.host_str() {
            return match parsed.port() {
                Some(port) => format!("{scheme}://{host}:{port}/..."),
                None => format!("{scheme}://{host}/..."),
            };
        }
        return format!("{scheme}://...");
    }
    if let Some((scheme, _)) = trimmed.split_once("://") {
        if !scheme.is_empty() && scheme.len() <= 16 {
            return format!("{scheme}://...");
        }
    }
    "<redacted-url>".to_owned()
}

pub fn validate_update_url(raw: &str) -> Result<Url, String> {
    validate_update_url_inner(raw, false)
}

fn validate_update_url_inner(raw: &str, allow_loopback_http: bool) -> Result<Url, String> {
    let parsed = Url::parse(raw.trim()).map_err(|_| "Invalid update URL syntax".to_owned())?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Update URL must not embed credentials".to_owned());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "Update URL missing host".to_owned())?;

    if allow_loopback_http && parsed.scheme() == "http" && (host == "127.0.0.1" || host == "localhost") {
        return Ok(parsed);
    }

    if parsed.scheme() != "https" {
        return Err(format!(
            "Update URL rejected non-HTTPS scheme ({})",
            redact_url_for_log(raw)
        ));
    }

    let allowed = ALLOWED_UPDATE_HOSTS
        .iter()
        .any(|candidate| host.eq_ignore_ascii_case(candidate));
    if !allowed {
        return Err(format!(
            "Update URL host is not on the GitHub release allowlist ({})",
            redact_url_for_log(raw)
        ));
    }

    Ok(parsed)
}

pub fn parse_release_semver(raw_tag: &str) -> Result<Version, String> {
    let trimmed = raw_tag.trim();
    let stripped = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    Version::parse(stripped).map_err(|e| format!("Invalid semantic version '{trimmed}': {e}"))
}

pub fn is_newer_version(current_raw: &str, candidate_raw: &str) -> Result<bool, String> {
    let current = parse_release_semver(current_raw)?;
    let candidate = parse_release_semver(candidate_raw)?;
    if !candidate.pre.is_empty() && current.pre.is_empty() {
        return Ok(false);
    }
    Ok(candidate > current)
}

pub fn parse_sha256sums(contents: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(hash_part) = parts.next() else {
            continue;
        };
        let Some(file_part) = parts.next() else {
            continue;
        };
        if hash_part.len() != 64 || !hash_part.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let clean_name = file_part
            .trim_start_matches('*')
            .trim_start_matches("./")
            .rsplit('/')
            .next()
            .unwrap_or(file_part);
        if !clean_name.is_empty() && is_safe_asset_filename(clean_name) {
            map.insert(clean_name.to_owned(), hash_part.to_ascii_lowercase());
        }
    }
    map
}

pub fn is_safe_asset_filename(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.contains("..")
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

pub fn verify_sha256_bytes(bytes: &[u8], expected_hex: &str) -> Result<String, String> {
    let normalized_expected = expected_hex.trim().to_ascii_lowercase();
    if normalized_expected.len() != 64
        || !normalized_expected.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("Expected SHA-256 digest is malformed".to_owned());
    }
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let actual_hex = hex::encode(hasher.finalize());
    if !constant_time_eq_hex(actual_hex.as_bytes(), normalized_expected.as_bytes()) {
        return Err(format!(
            "SHA-256 verification failed (expected {normalized_expected}, got {actual_hex})"
        ));
    }
    Ok(actual_hex)
}

fn constant_time_eq_hex(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub fn summarize_release_notes(raw_body: Option<&str>) -> String {
    let Some(body) = raw_body else {
        return "Bug fixes, playback refinements, and security updates.".to_owned();
    };
    let mut lines_out = Vec::new();
    for line in body.lines() {
        let cleaned = line
            .trim()
            .trim_start_matches('#')
            .trim_start_matches('-')
            .trim_start_matches('*')
            .trim();
        if cleaned.is_empty() {
            continue;
        }
        lines_out.push(cleaned);
        if lines_out.len() >= 3 {
            break;
        }
    }
    if lines_out.is_empty() {
        return "Bug fixes, playback refinements, and security updates.".to_owned();
    }
    let joined = lines_out.join(" - ");
    if joined.chars().count() > 260 {
        let truncated: String = joined.chars().take(257).collect();
        format!("{truncated}...")
    } else {
        joined
    }
}

pub fn detect_install_target_kind() -> InstallTargetKind {
    if cfg!(target_os = "windows") {
        return InstallTargetKind::WindowsExe;
    }
    match std::env::current_exe() {
        Ok(exe_path) => classify_linux_exe_path(&exe_path),
        Err(_) => InstallTargetKind::UbuntuDeb,
    }
}

fn classify_linux_exe_path(exe_path: &Path) -> InstallTargetKind {
    if exe_path.starts_with("/usr")
        || exe_path.starts_with("/bin")
        || exe_path.starts_with("/sbin")
        || exe_path.starts_with("/snap")
    {
        return InstallTargetKind::UbuntuDeb;
    }
    if let Some(parent) = exe_path.parent() {
        if can_create_temp_in_dir(parent) {
            return InstallTargetKind::LinuxTarGz;
        }
    }
    InstallTargetKind::UbuntuDeb
}

fn can_create_temp_in_dir(dir: &Path) -> bool {
    let probe = dir.join(format!(".ferrite-write-probe-{}", std::process::id()));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn resolve_release_api_endpoint() -> Result<Url, String> {
    #[cfg(debug_assertions)]
    if let Ok(custom) = std::env::var("FERRITE_UPDATE_API_URL") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            return validate_update_url(trimmed);
        }
    }
    validate_update_url(DEFAULT_RELEASE_API_URL)
}

fn build_update_http_client(allow_loopback_http: bool) -> Result<reqwest::Client, String> {
    let redirect_policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 6 {
            return attempt.error("Too many HTTP redirects while fetching update");
        }
        if let Err(reason) = validate_update_url_inner(attempt.url().as_str(), allow_loopback_http)
        {
            return attempt.error(reason);
        }
        attempt.follow()
    });

    reqwest::Client::builder()
        .timeout(UPDATE_HTTP_TIMEOUT)
        .redirect(redirect_policy)
        .user_agent(format!("Ferrite/{CURRENT_APP_VERSION}"))
        .build()
        .map_err(|e| format!("Failed to initialize HTTPS update client: {}", e.without_url()))
}

pub async fn check_for_updates() -> Result<Option<ReleaseUpdateInfo>, String> {
    let endpoint = resolve_release_api_endpoint()?;
    let target_kind = detect_install_target_kind();
    check_for_updates_from_url(&endpoint, CURRENT_APP_VERSION, target_kind, false).await
}

async fn check_for_updates_from_url(
    endpoint: &Url,
    current_version: &str,
    preferred_target: InstallTargetKind,
    allow_loopback_http: bool,
) -> Result<Option<ReleaseUpdateInfo>, String> {
    validate_update_url_inner(endpoint.as_str(), allow_loopback_http)?;
    let client = build_update_http_client(allow_loopback_http)?;

    let resp = client
        .get(endpoint.clone())
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("Update check request failed: {}", e.without_url()))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub Releases returned HTTP {}", resp.status().as_u16()));
    }

    let payload: GithubReleaseResponse = resp
        .json()
        .await
        .map_err(|e| format!("Invalid GitHub release metadata: {}", e.without_url()))?;

    if payload.draft || payload.prerelease {
        return Ok(None);
    }

    let parsed_ver = parse_release_semver(&payload.tag_name)?;
    if !is_newer_version(current_version, &payload.tag_name)? {
        return Ok(None);
    }

    let checksums_asset = payload
        .assets
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case("SHA256SUMS.txt"))
        .ok_or_else(|| "Release is missing mandatory SHA256SUMS.txt".to_owned())?;
    validate_update_url_inner(&checksums_asset.browser_download_url, allow_loopback_http)?;

    let (resolved_kind, matched_asset) =
        select_platform_asset(&payload.assets, preferred_target, allow_loopback_http)?;

    let html_url = payload
        .html_url
        .as_deref()
        .and_then(|u| validate_update_url_inner(u, allow_loopback_http).ok())
        .map(|u| u.to_string())
        .unwrap_or_else(|| GITHUB_RELEASES_PAGE_URL.to_owned());

    let clean_ver = parsed_ver.to_string();
    let release_name = payload
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("Ferrite v{clean_ver}"));

    Ok(Some(ReleaseUpdateInfo {
        version: clean_ver,
        tag_name: payload.tag_name,
        release_name,
        notes_summary: summarize_release_notes(payload.body.as_deref()),
        html_url,
        target_kind: resolved_kind,
        target_asset: matched_asset,
        checksums_asset: ReleaseAssetInfo {
            name: checksums_asset.name.clone(),
            download_url: checksums_asset.browser_download_url.clone(),
            size_bytes: checksums_asset.size,
        },
    }))
}

fn select_platform_asset(
    assets: &[GithubAssetResponse],
    preferred: InstallTargetKind,
    allow_loopback_http: bool,
) -> Result<(InstallTargetKind, ReleaseAssetInfo), String> {
    let find_matching = |pred: &dyn Fn(&str) -> bool| -> Option<&GithubAssetResponse> {
        assets
            .iter()
            .find(|a| is_safe_asset_filename(&a.name) && pred(&a.name))
    };

    let is_win_exe = |name: &str| {
        let lower = name.to_ascii_lowercase();
        lower.starts_with("ferrite-setup-") && lower.ends_with("-x64.exe")
    };
    let is_linux_tar = |name: &str| name.eq_ignore_ascii_case("ferrite-linux-x86_64.tar.gz");
    let is_ubuntu_deb = |name: &str| {
        let lower = name.to_ascii_lowercase();
        lower.starts_with("ferrite_") && lower.ends_with("_amd64.deb")
    };

    let selected: Option<(InstallTargetKind, &GithubAssetResponse)> = match preferred {
        InstallTargetKind::WindowsExe => {
            find_matching(&is_win_exe).map(|a| (InstallTargetKind::WindowsExe, a))
        }
        InstallTargetKind::LinuxTarGz => find_matching(&is_linux_tar)
            .map(|a| (InstallTargetKind::LinuxTarGz, a))
            .or_else(|| find_matching(&is_ubuntu_deb).map(|a| (InstallTargetKind::UbuntuDeb, a))),
        InstallTargetKind::UbuntuDeb => {
            find_matching(&is_ubuntu_deb).map(|a| (InstallTargetKind::UbuntuDeb, a))
        }
    };

    let (kind, asset) = selected
        .ok_or_else(|| "No compatible platform package found in latest release".to_owned())?;
    validate_update_url_inner(&asset.browser_download_url, allow_loopback_http)?;

    Ok((
        kind,
        ReleaseAssetInfo {
            name: asset.name.clone(),
            download_url: asset.browser_download_url.clone(),
            size_bytes: asset.size,
        },
    ))
}

async fn download_verified_bytes(
    client: &reqwest::Client,
    raw_url: &str,
    max_bytes: usize,
    allow_loopback_http: bool,
) -> Result<Vec<u8>, String> {
    let url = validate_update_url_inner(raw_url, allow_loopback_http)?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Asset download failed: {}", e.without_url()))?;

    if !resp.status().is_success() {
        return Err(format!("Asset download returned HTTP {}", resp.status().as_u16()));
    }
    if let Some(len) = resp.content_length() {
        if len > max_bytes as u64 {
            return Err("Update asset exceeds maximum allowed size".to_owned());
        }
    }

    let mut buf = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("Error reading update stream: {}", e.without_url()))?
    {
        if buf.len().saturating_add(chunk.len()) > max_bytes {
            return Err("Update asset exceeded size cap while streaming".to_owned());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

pub async fn download_and_apply_update(
    info: &ReleaseUpdateInfo,
) -> Result<AppliedUpdateOutcome, String> {
    let exe_path = std::env::current_exe()
        .map_err(|e| format!("Cannot resolve current executable path: {e}"))?;
    let updates_dir = default_updates_cache_dir();
    download_and_apply_update_inner(info, &exe_path, &updates_dir, false).await
}

async fn download_and_apply_update_inner(
    info: &ReleaseUpdateInfo,
    current_exe_path: &Path,
    updates_cache_dir: &Path,
    allow_loopback_http: bool,
) -> Result<AppliedUpdateOutcome, String> {
    if !is_safe_asset_filename(&info.target_asset.name) {
        return Err("Unsafe release asset filename rejected".to_owned());
    }
    let client = build_update_http_client(allow_loopback_http)?;

    let sums_bytes = download_verified_bytes(
        &client,
        &info.checksums_asset.download_url,
        MAX_CHECKSUM_BYTES,
        allow_loopback_http,
    )
    .await?;
    let sums_text = std::str::from_utf8(&sums_bytes)
        .map_err(|_| "SHA256SUMS.txt is not valid UTF-8".to_owned())?;
    let sums_map = parse_sha256sums(sums_text);
    let expected_sha = sums_map
        .get(&info.target_asset.name)
        .ok_or_else(|| format!("SHA256SUMS.txt has no entry for {}", info.target_asset.name))?;

    let asset_bytes = download_verified_bytes(
        &client,
        &info.target_asset.download_url,
        MAX_UPDATE_ASSET_BYTES,
        allow_loopback_http,
    )
    .await?;

    verify_sha256_bytes(&asset_bytes, expected_sha)?;

    let info_clone = info.clone();
    let exe_owned = current_exe_path.to_path_buf();
    let cache_owned = updates_cache_dir.to_path_buf();

    tokio::task::spawn_blocking(move || match info_clone.target_kind {
        InstallTargetKind::LinuxTarGz => {
            let binary_bytes = extract_binary_from_tar_gz(&asset_bytes, "ferrite")?;
            if !binary_bytes.starts_with(b"\x7fELF") {
                return Err("Extracted Linux binary failed ELF header check".to_owned());
            }
            atomically_replace_executable(&exe_owned, &binary_bytes)?;
            Ok(AppliedUpdateOutcome::ReadyToRestart {
                version: info_clone.version,
            })
        }
        InstallTargetKind::UbuntuDeb => {
            if !asset_bytes.starts_with(b"!<arch>\n") {
                return Err("Downloaded .deb package failed archive header check".to_owned());
            }
            ensure_private_dir(&cache_owned)?;
            let deb_path = cache_owned.join(&info_clone.target_asset.name);
            write_file_atomically(&deb_path, &asset_bytes, 0o644)?;
            let install_cmd = format!("sudo apt install {}", deb_path.display());
            Ok(AppliedUpdateOutcome::DebDownloaded {
                version: info_clone.version,
                deb_path,
                install_cmd,
            })
        }
        InstallTargetKind::WindowsExe => {
            if !asset_bytes.starts_with(b"MZ") {
                return Err("Downloaded Windows installer failed PE header check".to_owned());
            }
            ensure_private_dir(&cache_owned)?;
            let installer_path = cache_owned.join(&info_clone.target_asset.name);
            write_file_atomically(&installer_path, &asset_bytes, 0o700)?;
            Ok(AppliedUpdateOutcome::WindowsInstallerPrepared {
                version: info_clone.version,
                installer_path,
            })
        }
    })
    .await
    .map_err(|e| format!("Update installer worker failed: {e}"))?
}

pub fn extract_binary_from_tar_gz(gz_bytes: &[u8], binary_name: &str) -> Result<Vec<u8>, String> {
    let mut decoder = GzDecoder::new(gz_bytes);
    let mut block = [0u8; 512];

    loop {
        read_exact_or_eof(&mut decoder, &mut block)?;
        if block.iter().all(|&b| b == 0) {
            break;
        }

        let raw_name = parse_tar_string(&block[0..100]);
        let prefix = if &block[257..262] == b"ustar" {
            parse_tar_string(&block[345..500])
        } else {
            String::new()
        };
        let full_path = if prefix.is_empty() {
            raw_name
        } else {
            format!("{prefix}/{raw_name}")
        };

        if full_path.contains("..") || full_path.starts_with('/') {
            return Err("Archive entry contains unsafe path".to_owned());
        }

        let size = parse_tar_octal(&block[124..136])?;
        if size > MAX_UPDATE_ASSET_BYTES as u64 {
            return Err("Archive entry exceeds maximum allowed size".to_owned());
        }
        let typeflag = block[156];
        let is_regular = typeflag == 0 || typeflag == b'0';

        let entry_basename = full_path.rsplit('/').next().unwrap_or(&full_path);
        if is_regular && entry_basename == binary_name {
            let mut out = vec![0u8; size as usize];
            decoder
                .read_exact(&mut out)
                .map_err(|e| format!("Truncated archive entry: {e}"))?;
            return Ok(out);
        }

        let padded = size.div_ceil(512) * 512;
        skip_bytes(&mut decoder, padded)?;
    }

    Err(format!("Binary '{binary_name}' not found in release archive"))
}

fn read_exact_or_eof(reader: &mut impl Read, buf: &mut [u8; 512]) -> Result<(), String> {
    let mut read_total = 0usize;
    while read_total < 512 {
        match reader.read(&mut buf[read_total..]) {
            Ok(0) => {
                if read_total == 0 {
                    buf.fill(0);
                    return Ok(());
                }
                return Err("Unexpected EOF inside tar header".to_owned());
            }
            Ok(n) => read_total += n,
            Err(e) => return Err(format!("Decompression error: {e}")),
        }
    }
    Ok(())
}

fn skip_bytes(reader: &mut impl Read, mut remaining: u64) -> Result<(), String> {
    let mut scratch = [0u8; 4096];
    while remaining > 0 {
        let chunk = (remaining as usize).min(scratch.len());
        reader
            .read_exact(&mut scratch[..chunk])
            .map_err(|e| format!("Unexpected EOF skipping tar entry: {e}"))?;
        remaining -= chunk as u64;
    }
    Ok(())
}

fn parse_tar_string(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).trim().to_owned()
}

fn parse_tar_octal(field: &[u8]) -> Result<u64, String> {
    let trimmed_bytes: Vec<u8> = field
        .iter()
        .copied()
        .take_while(|&b| b != 0)
        .filter(|&b| b != b' ')
        .collect();
    if trimmed_bytes.is_empty() {
        return Ok(0);
    }
    let s = std::str::from_utf8(&trimmed_bytes)
        .map_err(|_| "Invalid octal size in tar header".to_owned())?;
    u64::from_str_radix(s, 8).map_err(|_| "Malformed octal size in tar header".to_owned())
}

fn atomically_replace_executable(exe_path: &Path, binary_bytes: &[u8]) -> Result<(), String> {
    let parent = exe_path
        .parent()
        .ok_or_else(|| "Executable has no parent directory".to_owned())?;
    write_file_atomically(exe_path, binary_bytes, 0o755).map_err(|e| {
        format!(
            "Failed to replace executable in {}: {e}",
            parent.display()
        )
    })
}

fn ensure_private_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Failed to create update directory: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn write_file_atomically(target: &Path, bytes: &[u8], unix_mode: u32) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| "Target file has no parent directory".to_owned())?;
    let file_name = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("ferrite-update");
    let tmp_path = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));

    let write_res = (|| -> Result<(), String> {
        let mut file = File::create(&tmp_path)
            .map_err(|e| format!("Failed to create temporary update file: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(unix_mode))
                .map_err(|e| format!("Failed to set update file permissions: {e}"))?;
        }
        #[cfg(not(unix))]
        let _ = unix_mode;

        file.write_all(bytes)
            .map_err(|e| format!("Failed to write update payload: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("Failed to flush update payload to disk: {e}"))?;
        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    if let Err(err) = fs::rename(&tmp_path, target) {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("Atomic rename failed: {err}"));
    }
    Ok(())
}

pub fn default_updates_cache_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        let trimmed = xdg.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("ferrite").join("updates");
        }
    }
    if let Some(cache) = dirs::cache_dir() {
        return cache.join("ferrite").join("updates");
    }
    std::env::temp_dir().join("ferrite").join("updates")
}

pub fn spawn_restarted_executable() -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("Cannot resolve executable for restart: {e}"))?;
    Command::new(exe)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Failed to launch updated Ferrite binary: {e}"))
}

pub fn launch_windows_silent_installer(installer_path: &Path) -> Result<(), String> {
    Command::new(installer_path)
        .arg("/SILENT")
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Failed to start Windows update installer: {e}"))
}

pub fn open_deb_package_with_system(deb_path: &Path) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(deb_path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not launch system package installer: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};

    fn build_single_file_tar_gz(entry_name: &str, payload: &[u8]) -> Vec<u8> {
        let mut tar = Vec::new();
        let mut header = [0u8; 512];
        let name_bytes = entry_name.as_bytes();
        header[..name_bytes.len()].copy_from_slice(name_bytes);
        header[100..107].copy_from_slice(b"0000755");
        header[108..115].copy_from_slice(b"0000000");
        header[116..123].copy_from_slice(b"0000000");
        let size_octal = format!("{:011o}", payload.len());
        header[124..135].copy_from_slice(size_octal.as_bytes());
        header[136..147].copy_from_slice(b"00000000000");
        header[148..156].fill(b' ');
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");

        let checksum: u32 = header.iter().map(|&b| b as u32).sum();
        let chk_str = format!("{checksum:06o}\0 ");
        header[148..156].copy_from_slice(chk_str.as_bytes());

        tar.extend_from_slice(&header);
        tar.extend_from_slice(payload);
        let pad = (512 - (payload.len() % 512)) % 512;
        tar.extend(std::iter::repeat_n(0u8, pad));
        tar.extend(std::iter::repeat_n(0u8, 1024));

        let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
        gz.write_all(&tar).expect("write tar into gzip");
        gz.finish().expect("finish gzip")
    }

    #[test]
    fn update_url_enforces_https_and_github_host_allowlist() {
        assert!(validate_update_url(DEFAULT_RELEASE_API_URL).is_ok());
        assert!(validate_update_url(
            "https://github.com/Gaalix/ferrite/releases/download/v0.2.0/SHA256SUMS.txt"
        )
        .is_ok());
        assert!(validate_update_url(
            "https://objects.githubusercontent.com/github-production-release-asset/file.tar.gz"
        )
        .is_ok());

        assert!(
            validate_update_url("http://api.github.com/repos/Gaalix/ferrite/releases/latest")
                .is_err()
        );
        assert!(validate_update_url("https://evil-github.com/ferrite.tar.gz").is_err());
        assert!(validate_update_url("https://token@api.github.com/repos/Gaalix/ferrite").is_err());
        assert!(validate_update_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn redact_url_strips_tokens_paths_and_queries() {
        let secret_addon =
            "https://torrentio.strem.fun/realdebrid=SUPER_SECRET_API_KEY_12345/manifest.json";
        let redacted = redact_url_for_log(secret_addon);
        assert_eq!(redacted, "https://torrentio.strem.fun/...");
        assert!(!redacted.contains("SUPER_SECRET"));
    }

    #[test]
    fn semver_comparison_handles_v_prefix_and_prereleases() {
        assert_eq!(is_newer_version("0.1.0", "v0.2.0"), Ok(true));
        assert_eq!(is_newer_version("v0.1.0", "0.1.1"), Ok(true));
        assert_eq!(is_newer_version("0.1.0", "v0.1.0"), Ok(false));
        assert_eq!(is_newer_version("0.2.0", "v0.1.9"), Ok(false));
        assert_eq!(is_newer_version("0.1.0", "v0.2.0-rc.1"), Ok(false));
        assert!(is_newer_version("0.1.0", "not-a-version").is_err());
    }

    #[test]
    fn sha256sums_parser_and_digest_verifier_reject_tampered_bytes() {
        let payload = b"\x7fELFferrite-test-binary";
        let digest = hex::encode(Sha256::digest(payload));
        let sums_txt = format!(
            "{digest}  ferrite-linux-x86_64.tar.gz\n0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef *ferrite_0.2.0_amd64.deb\n"
        );
        let parsed = parse_sha256sums(&sums_txt);
        assert_eq!(
            parsed
                .get("ferrite-linux-x86_64.tar.gz")
                .map(String::as_str),
            Some(digest.as_str())
        );
        assert!(verify_sha256_bytes(payload, &digest).is_ok());
        assert!(verify_sha256_bytes(b"\x7fELFtampered", &digest).is_err());
    }

    #[test]
    fn tar_gz_extractor_recovers_binary_and_blocks_traversal() {
        let elf_payload = b"\x7fELF\x02\x01\x01\x00ferrite-v0.2.0-payload";
        let archive = build_single_file_tar_gz("ferrite-linux-x86_64/ferrite", elf_payload);
        let extracted = extract_binary_from_tar_gz(&archive, "ferrite").expect("extract ferrite");
        assert_eq!(extracted, elf_payload);

        let traversal_archive = build_single_file_tar_gz("../ferrite", elf_payload);
        assert!(extract_binary_from_tar_gz(&traversal_archive, "ferrite").is_err());
    }

    #[tokio::test]
    async fn end_to_end_local_update_verifies_sha256_and_replaces_binary_atomically() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let new_elf = b"\x7fELF\x02\x01\x01\x00ferrite-0.2.0-verified";
        let tar_gz = build_single_file_tar_gz("ferrite", new_elf);
        let good_sha = hex::encode(Sha256::digest(&tar_gz));
        let sums_body = format!("{good_sha}  ferrite-linux-x86_64.tar.gz\n");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("local addr");

        let sums_clone = sums_body.into_bytes();
        let tar_clone = tar_gz.clone();
        let server_task = tokio::spawn(async move {
            for _ in 0..3 {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut req_buf = [0u8; 2048];
                let n = stream.read(&mut req_buf).await.unwrap_or(0);
                let req_line = String::from_utf8_lossy(&req_buf[..n]);
                let (ct, body): (&str, Vec<u8>) = if req_line.starts_with("GET /latest ") {
                    let json = format!(
                        r#"{{"tag_name":"v0.4.0","name":"Ferrite v0.4.0","body":"- Faster startup","html_url":"http://127.0.0.1:{port}/release","draft":false,"prerelease":false,"assets":[{{"name":"SHA256SUMS.txt","browser_download_url":"http://127.0.0.1:{port}/SHA256SUMS.txt","size":80}},{{"name":"ferrite-linux-x86_64.tar.gz","browser_download_url":"http://127.0.0.1:{port}/ferrite-linux-x86_64.tar.gz","size":{sz}}}]}}"#,
                        port = addr.port(),
                        sz = tar_clone.len()
                    );
                    ("application/json", json.into_bytes())
                } else if req_line.starts_with("GET /SHA256SUMS.txt ") {
                    ("text/plain", sums_clone.clone())
                } else {
                    ("application/octet-stream", tar_clone.clone())
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            }
        });

        let endpoint = Url::parse(&format!("http://127.0.0.1:{}/latest", addr.port()))
            .expect("parse mock url");
        let found = check_for_updates_from_url(
            &endpoint,
            "0.3.0",
            InstallTargetKind::LinuxTarGz,
            true,
        )
        .await
        .expect("check succeeds")
        .expect("update available");
        assert_eq!(found.version, "0.4.0");

        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let fake_exe = tmp_dir.path().join("ferrite");
        fs::write(&fake_exe, b"\x7fELFold-0.3.0").expect("seed old exe");

        let outcome = download_and_apply_update_inner(
            &found,
            &fake_exe,
            &tmp_dir.path().join("cache"),
            true,
        )
        .await
        .expect("apply update succeeds");
        assert_eq!(
            outcome,
            AppliedUpdateOutcome::ReadyToRestart {
                version: "0.4.0".to_owned()
            }
        );
        assert_eq!(fs::read(&fake_exe).expect("read updated exe"), new_elf);
        server_task.abort();
    }

    #[test]
    fn bundled_ferrite_ico_has_valid_multi_res_header() {
        let ico_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/icons/ferrite.ico");
        let gpui_ico_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/zed-gpui/crates/gpui/resources/windows/ferrite.ico");
        let bytes = fs::read(&ico_path).expect("read ferrite.ico");
        let gpui_bytes = fs::read(&gpui_ico_path).expect("read gpui ferrite.ico");
        assert_eq!(bytes, gpui_bytes);
        assert!(bytes.len() > 6 + 4 * 16);
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 0);
        assert_eq!(u16::from_le_bytes([bytes[2], bytes[3]]), 1);
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 4);

        for (idx, expected_dim) in [32u8, 48, 64, 0].into_iter().enumerate() {
            let off = 6 + idx * 16;
            assert_eq!(bytes[off], expected_dim);
            assert_eq!(bytes[off + 1], expected_dim);
            assert_eq!(u16::from_le_bytes([bytes[off + 4], bytes[off + 5]]), 1);
            assert_eq!(u16::from_le_bytes([bytes[off + 6], bytes[off + 7]]), 32);
            let data_len = u32::from_le_bytes([
                bytes[off + 8],
                bytes[off + 9],
                bytes[off + 10],
                bytes[off + 11],
            ]) as usize;
            let data_off = u32::from_le_bytes([
                bytes[off + 12],
                bytes[off + 13],
                bytes[off + 14],
                bytes[off + 15],
            ]) as usize;
            assert!(data_off + data_len <= bytes.len());
            if expected_dim == 0 {
                assert_eq!(&bytes[data_off..data_off + 8], b"\x89PNG\r\n\x1a\n");
            } else {
                assert_eq!(
                    u32::from_le_bytes([
                        bytes[data_off],
                        bytes[data_off + 1],
                        bytes[data_off + 2],
                        bytes[data_off + 3],
                    ]),
                    40
                );
            }
        }
    }
}
