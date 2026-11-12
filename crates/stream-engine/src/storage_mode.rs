use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

pub const DEFAULT_DISK_LRU_MAX_BYTES: u64 = 20 * 1024 * 1024 * 1024;
pub const DEFAULT_RAM_ONLY_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const LRU_INDEX_FILENAME: &str = "lru_index.json";

static RAM_INSTANCE_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum StorageMode {
    DiskLru {
        #[serde(rename = "maxBytes")]
        max_bytes: u64,
    },
    RamOnly {
        #[serde(rename = "maxBytes")]
        max_bytes: u64,
    },
}

impl Default for StorageMode {
    fn default() -> Self {
        Self::DiskLru {
            max_bytes: DEFAULT_DISK_LRU_MAX_BYTES,
        }
    }
}

impl StorageMode {
    pub const fn disk_lru(max_bytes: u64) -> Self {
        Self::DiskLru { max_bytes }
    }

    pub const fn default_disk_lru() -> Self {
        Self::DiskLru {
            max_bytes: DEFAULT_DISK_LRU_MAX_BYTES,
        }
    }

    pub const fn ram_only(max_bytes: u64) -> Self {
        Self::RamOnly { max_bytes }
    }

    pub const fn default_ram_only() -> Self {
        Self::RamOnly {
            max_bytes: DEFAULT_RAM_ONLY_MAX_BYTES,
        }
    }

    pub const fn max_bytes(&self) -> u64 {
        match *self {
            Self::DiskLru { max_bytes } | Self::RamOnly { max_bytes } => max_bytes,
        }
    }

    pub const fn is_ram_only(&self) -> bool {
        matches!(self, Self::RamOnly { .. })
    }

    pub const fn is_disk_lru(&self) -> bool {
        matches!(self, Self::DiskLru { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TorrentStorageLocation {
    Disk,
    Ram,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LruTorrentEntry {
    pub info_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub size_bytes: u64,
    pub last_accessed_unix_ms: i64,
    pub output_folder: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_file_idx: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LruIndexFile {
    #[serde(default = "default_lru_version")]
    pub version: u32,
    #[serde(default)]
    pub entries: Vec<LruTorrentEntry>,
}

const fn default_lru_version() -> u32 {
    1
}

impl LruIndexFile {
    pub fn load_sync(index_path: &Path) -> Self {
        match std::fs::read(index_path) {
            Ok(bytes) => match serde_json::from_slice::<Self>(&bytes) {
                Ok(parsed) => parsed,
                Err(err) => {
                    warn!(path = %index_path.display(), "corrupt lru_index.json, resetting: {err}");
                    Self {
                        version: 1,
                        entries: Vec::new(),
                    }
                }
            },
            Err(_) => Self {
                version: 1,
                entries: Vec::new(),
            },
        }
    }

    pub async fn save_atomic(&self, index_path: &Path) -> std::io::Result<()> {
        if let Some(parent) = index_path.parent() {
            crate::ensure_dir_0700(parent).await?;
        }
        let payload = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp_path = index_path.with_extension("json.tmp");
        tokio::fs::write(&tmp_path, payload).await?;
        crate::set_file_0600(&tmp_path).await;
        tokio::fs::rename(&tmp_path, index_path).await?;
        crate::set_file_0600(index_path).await;
        Ok(())
    }

    pub fn save_atomic_sync(&self, index_path: &Path) -> std::io::Result<()> {
        if let Some(parent) = index_path.parent() {
            crate::ensure_dir_0700_sync(parent)?;
        }
        let payload = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp_path = index_path.with_extension("json.tmp");
        std::fs::write(&tmp_path, payload)?;
        crate::set_file_0600_sync(&tmp_path);
        std::fs::rename(&tmp_path, index_path)?;
        crate::set_file_0600_sync(index_path);
        Ok(())
    }
}

pub fn allocate_ram_cache_dir_path() -> PathBuf {
    let pid = std::process::id();
    let seq = RAM_INSTANCE_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir_name = if seq == 0 {
        format!("ferrite-ram-{pid}")
    } else {
        format!("ferrite-ram-{pid}-{seq}")
    };

    let shm = Path::new("/dev/shm");
    if shm.is_dir() {
        shm.join(dir_name)
    } else {
        std::env::temp_dir().join(dir_name)
    }
}

pub fn allocated_directory_bytes(dir: &Path) -> u64 {
    if !dir.exists() {
        return 0;
    }
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(curr) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&curr) else {
            continue;
        };
        for entry_res in rd {
            let Ok(entry) = entry_res else {
                continue;
            };
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                if let Ok(meta) = entry.metadata() {
                    total = total.saturating_add(allocated_file_bytes(&meta));
                }
            }
        }
    }
    total
}

#[cfg(unix)]
fn allocated_file_bytes(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    (meta.blocks().saturating_mul(512)).min(meta.len())
}

#[cfg(not(unix))]
fn allocated_file_bytes(meta: &std::fs::Metadata) -> u64 {
    meta.len()
}

fn is_valid_disk_entry_folder(folder: &Path, disk_cache_dir: &Path, ih: &[u8; 20]) -> bool {
    if !crate::is_safe_path_no_traversal(folder) {
        return false;
    }
    folder == disk_cache_dir.join(hex::encode(ih))
}

#[derive(Debug, Clone)]
struct TrackedCacheItem {
    info_hash: [u8; 20],
    location: TorrentStorageLocation,
    name: Option<String>,
    size_bytes: u64,
    last_accessed_unix_ms: i64,
    output_folder: PathBuf,
    selected_file_idx: Option<usize>,
}

pub struct CacheStateTracker {
    mode: RwLock<StorageMode>,
    disk_cache_dir: PathBuf,
    ram_cache_dir: PathBuf,
    items: RwLock<HashMap<[u8; 20], TrackedCacheItem>>,
    monotonic_touch_seq: AtomicU64,
}

impl CacheStateTracker {
    pub fn new(initial_mode: StorageMode, disk_cache_dir: PathBuf, ram_cache_dir: PathBuf) -> Self {
        let mut items_map = HashMap::new();
        if initial_mode.is_disk_lru() {
            let index_path = disk_cache_dir.join(LRU_INDEX_FILENAME);
            let loaded = LruIndexFile::load_sync(&index_path);
            for entry in loaded.entries {
                if let Some(ih) = parse_hex_20(&entry.info_hash) {
                    if is_valid_disk_entry_folder(&entry.output_folder, &disk_cache_dir, &ih)
                        && entry.output_folder.exists()
                    {
                        let measured = allocated_directory_bytes(&entry.output_folder);
                        items_map.insert(
                            ih,
                            TrackedCacheItem {
                                info_hash: ih,
                                location: TorrentStorageLocation::Disk,
                                name: entry.name,
                                size_bytes: entry.size_bytes.max(measured),
                                last_accessed_unix_ms: entry.last_accessed_unix_ms,
                                output_folder: entry.output_folder,
                                selected_file_idx: entry.selected_file_idx,
                            },
                        );
                    }
                }
            }
        }

        Self {
            mode: RwLock::new(initial_mode),
            disk_cache_dir,
            ram_cache_dir,
            items: RwLock::new(items_map),
            monotonic_touch_seq: AtomicU64::new(0),
        }
    }

    pub fn mode(&self) -> StorageMode {
        *self.mode.read()
    }

    pub fn set_mode(&self, next: StorageMode) {
        *self.mode.write() = next;
        if next.is_disk_lru() {
            let index_path = self.disk_cache_dir.join(LRU_INDEX_FILENAME);
            if index_path.exists() {
                let loaded = LruIndexFile::load_sync(&index_path);
                let mut guard = self.items.write();
                for entry in loaded.entries {
                    if let Some(ih) = parse_hex_20(&entry.info_hash) {
                        if is_valid_disk_entry_folder(
                            &entry.output_folder,
                            &self.disk_cache_dir,
                            &ih,
                        ) {
                            guard.entry(ih).or_insert_with(|| TrackedCacheItem {
                                info_hash: ih,
                                location: TorrentStorageLocation::Disk,
                                name: entry.name,
                                size_bytes: entry.size_bytes,
                                last_accessed_unix_ms: entry.last_accessed_unix_ms,
                                output_folder: entry.output_folder,
                                selected_file_idx: entry.selected_file_idx,
                            });
                        }
                    }
                }
            }
        }
    }

    pub fn disk_cache_dir(&self) -> &Path {
        &self.disk_cache_dir
    }

    pub fn ram_cache_dir(&self) -> &Path {
        &self.ram_cache_dir
    }

    pub fn lru_index_path(&self) -> PathBuf {
        self.disk_cache_dir.join(LRU_INDEX_FILENAME)
    }

    fn next_timestamp_ms(&self) -> i64 {
        let now_ms = Utc::now().timestamp_millis();
        let seq = self.monotonic_touch_seq.fetch_add(1, Ordering::Relaxed) as i64;
        now_ms.saturating_mul(1000).saturating_add(seq % 1000) / 1000 + seq
    }

    pub fn output_folder_for_new_torrent(
        &self,
        info_hash: &[u8; 20],
    ) -> (PathBuf, TorrentStorageLocation) {
        let hex_str = hex::encode(info_hash);
        match self.mode() {
            StorageMode::DiskLru { .. } => (
                self.disk_cache_dir.join(hex_str),
                TorrentStorageLocation::Disk,
            ),
            StorageMode::RamOnly { .. } => (
                self.ram_cache_dir.join(hex_str),
                TorrentStorageLocation::Ram,
            ),
        }
    }

    pub fn location_of(&self, info_hash: &[u8; 20]) -> Option<TorrentStorageLocation> {
        self.items.read().get(info_hash).map(|i| i.location)
    }

    pub fn output_folder_of(&self, info_hash: &[u8; 20]) -> Option<PathBuf> {
        self.items
            .read()
            .get(info_hash)
            .map(|i| i.output_folder.clone())
    }

    pub fn record_or_touch(
        &self,
        info_hash: [u8; 20],
        location: TorrentStorageLocation,
        output_folder: PathBuf,
        name: Option<String>,
        size_bytes: u64,
        selected_file_idx: Option<usize>,
    ) {
        let ts = self.next_timestamp_ms();
        let mut guard = self.items.write();
        if let Some(existing) = guard.get_mut(&info_hash) {
            existing.last_accessed_unix_ms = ts;
            if name.is_some() {
                existing.name = name;
            }
            if size_bytes > 0 {
                existing.size_bytes = size_bytes;
            }
            if selected_file_idx.is_some() {
                existing.selected_file_idx = selected_file_idx;
            }
        } else {
            guard.insert(
                info_hash,
                TrackedCacheItem {
                    info_hash,
                    location,
                    name,
                    size_bytes,
                    last_accessed_unix_ms: ts,
                    output_folder,
                    selected_file_idx,
                },
            );
        }
    }

    pub fn update_size_bytes(&self, info_hash: &[u8; 20], size_bytes: u64, name: Option<String>) {
        let mut guard = self.items.write();
        if let Some(item) = guard.get_mut(info_hash) {
            item.size_bytes = size_bytes;
            if item.name.is_none() && name.is_some() {
                item.name = name;
            }
        }
    }

    pub fn remove_item(&self, info_hash: &[u8; 20]) -> Option<(TorrentStorageLocation, PathBuf)> {
        self.items
            .write()
            .remove(info_hash)
            .map(|item| (item.location, item.output_folder))
    }

    pub fn tracked_hashes_by_location(&self, location: TorrentStorageLocation) -> Vec<[u8; 20]> {
        self.items
            .read()
            .values()
            .filter(|i| i.location == location)
            .map(|i| i.info_hash)
            .collect()
    }

    pub fn usage_bytes_for_location(&self, location: TorrentStorageLocation) -> u64 {
        let items: Vec<(PathBuf, u64)> = self
            .items
            .read()
            .values()
            .filter(|i| i.location == location)
            .map(|i| (i.output_folder.clone(), i.size_bytes))
            .collect();

        let mut sum = 0u64;
        for (folder, tracked_bytes) in items {
            let disk_bytes = allocated_directory_bytes(&folder);
            sum = sum.saturating_add(tracked_bytes.max(disk_bytes));
        }
        sum
    }

    pub fn active_mode_usage_bytes(&self) -> u64 {
        let loc = if self.mode().is_ram_only() {
            TorrentStorageLocation::Ram
        } else {
            TorrentStorageLocation::Disk
        };
        self.usage_bytes_for_location(loc)
    }

    pub fn select_eviction_victims(
        &self,
        location: TorrentStorageLocation,
        max_bytes: u64,
        protected_active: &dyn Fn(&[u8; 20]) -> bool,
    ) -> Vec<([u8; 20], PathBuf)> {
        let mut candidates: Vec<(i64, u64, [u8; 20], PathBuf)> = Vec::new();
        let mut current_total = 0u64;

        {
            let guard = self.items.read();
            for item in guard.values().filter(|i| i.location == location) {
                let effective = item
                    .size_bytes
                    .max(allocated_directory_bytes(&item.output_folder));
                current_total = current_total.saturating_add(effective);
                if !protected_active(&item.info_hash) {
                    candidates.push((
                        item.last_accessed_unix_ms,
                        effective,
                        item.info_hash,
                        item.output_folder.clone(),
                    ));
                }
            }
        }

        if current_total <= max_bytes {
            return Vec::new();
        }

        candidates.sort_by_key(|(ts, _, _, _)| *ts);

        let mut victims = Vec::new();
        for (_ts, bytes, ih, folder) in candidates {
            if current_total <= max_bytes {
                break;
            }
            victims.push((ih, folder));
            current_total = current_total.saturating_sub(bytes);
        }

        debug!(
            ?location,
            max_bytes,
            evicted_count = victims.len(),
            "selected cache LRU eviction victims"
        );
        victims
    }

    pub fn snapshot_disk_lru_index(&self) -> LruIndexFile {
        let mut entries: Vec<LruTorrentEntry> = self
            .items
            .read()
            .values()
            .filter(|i| i.location == TorrentStorageLocation::Disk)
            .map(|i| LruTorrentEntry {
                info_hash: hex::encode(i.info_hash),
                name: i.name.clone(),
                size_bytes: i
                    .size_bytes
                    .max(allocated_directory_bytes(&i.output_folder)),
                last_accessed_unix_ms: i.last_accessed_unix_ms,
                output_folder: i.output_folder.clone(),
                selected_file_idx: i.selected_file_idx,
            })
            .collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.last_accessed_unix_ms));
        LruIndexFile {
            version: 1,
            entries,
        }
    }

    pub async fn persist_disk_lru_index(&self) {
        if !self.mode().is_disk_lru() {
            return;
        }
        let snapshot = self.snapshot_disk_lru_index();
        let path = self.lru_index_path();
        if let Err(err) = snapshot.save_atomic(&path).await {
            warn!(path = %path.display(), "failed writing lru_index.json: {err}");
        }
    }

    pub fn persist_disk_lru_index_sync(&self) {
        if !self.mode().is_disk_lru() {
            return;
        }
        let snapshot = self.snapshot_disk_lru_index();
        let path = self.lru_index_path();
        let _ = snapshot.save_atomic_sync(&path);
    }
}

fn parse_hex_20(s: &str) -> Option<[u8; 20]> {
    if s.len() != 40 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 20];
    hex::decode_to_slice(s, &mut out).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_mode_defaults_and_helpers() {
        let def = StorageMode::default();
        assert_eq!(
            def,
            StorageMode::DiskLru {
                max_bytes: DEFAULT_DISK_LRU_MAX_BYTES
            }
        );
        assert!(def.is_disk_lru());
        assert!(!def.is_ram_only());

        let ram = StorageMode::ram_only(DEFAULT_RAM_ONLY_MAX_BYTES);
        assert!(ram.is_ram_only());
        assert_eq!(ram.max_bytes(), DEFAULT_RAM_ONLY_MAX_BYTES);
    }

    #[test]
    fn lru_eviction_selects_oldest_inactive_and_never_evicts_active_streams() {
        let tmp = tempfile::TempDir::new().unwrap();
        let disk_dir = tmp.path().join("disk");
        let ram_dir = tmp.path().join("ram");

        let tracker = CacheStateTracker::new(
            StorageMode::disk_lru(5_000_000),
            disk_dir.clone(),
            ram_dir,
        );

        let ih_oldest_active = [0x11u8; 20];
        let ih_middle_inactive = [0x22u8; 20];
        let ih_newest_inactive = [0x33u8; 20];

        tracker.record_or_touch(
            ih_oldest_active,
            TorrentStorageLocation::Disk,
            disk_dir.join(hex::encode(ih_oldest_active)),
            Some("Movie A (Active)".into()),
            3_000_000,
            Some(0),
        );
        tracker.record_or_touch(
            ih_middle_inactive,
            TorrentStorageLocation::Disk,
            disk_dir.join(hex::encode(ih_middle_inactive)),
            Some("Movie B (Oldest Inactive)".into()),
            3_000_000,
            Some(0),
        );
        tracker.record_or_touch(
            ih_newest_inactive,
            TorrentStorageLocation::Disk,
            disk_dir.join(hex::encode(ih_newest_inactive)),
            Some("Movie C (Newest Inactive)".into()),
            3_000_000,
            Some(0),
        );

        let victims = tracker.select_eviction_victims(
            TorrentStorageLocation::Disk,
            5_000_000,
            &|ih| *ih == ih_oldest_active,
        );
        let victim_hashes: Vec<[u8; 20]> = victims.into_iter().map(|(h, _)| h).collect();
        assert_eq!(victim_hashes, vec![ih_middle_inactive, ih_newest_inactive]);
        assert!(!victim_hashes.contains(&ih_oldest_active));

        let single_victim = tracker.select_eviction_victims(
            TorrentStorageLocation::Disk,
            6_500_000,
            &|ih| *ih == ih_oldest_active,
        );
        assert_eq!(single_victim.len(), 1);
        assert_eq!(single_victim[0].0, ih_middle_inactive);
    }
}
