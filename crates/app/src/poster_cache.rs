use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

use gpui::RenderImage;
use image::{imageops::FilterType, Frame, RgbaImage};
use parking_lot::{Mutex, RwLock};
use sha2::{Digest, Sha256};
use smallvec::{smallvec, SmallVec};
use tokio::sync::{Notify, Semaphore};
use url::Url;

const MAX_CONCURRENT_POSTER_DOWNLOADS: usize = 12;
pub const MAX_CONCURRENT_IMAGE_DECODES: usize = 4;
const MAX_CONCURRENT_DISK_IO: usize = 16;
const HTTP_POSTER_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_POSTER_BYTES: usize = 16 * 1024 * 1024;
const MAX_FAILED_ENTRIES: usize = 256;
const DISK_PRUNE_INTERVAL_WRITES: u64 = 200;

pub const MAX_DECODED_TEXTURES: usize = 128;
pub const MAX_THUMB_TEXTURES: usize = 32;
pub const MAX_AMBIENT_TEXTURES: usize = 16;
pub const MAX_HERO_TEXTURES: usize = 4;
pub const MAX_STILL_TEXTURES: usize = 48;

pub const PLAYBACK_RETAINED_DECODED_TEXTURES: usize = 16;
pub const PLAYBACK_RETAINED_THUMB_TEXTURES: usize = 8;
pub const PLAYBACK_RETAINED_HERO_TEXTURES: usize = 2;
pub const PLAYBACK_RETAINED_AMBIENT_TEXTURES: usize = 2;
pub const PLAYBACK_RETAINED_STILL_TEXTURES: usize = 8;

pub const MAX_DISK_CACHE_BYTES: u64 = 256 * 1024 * 1024;

const POSTER_MAX_W: u32 = 300;
const POSTER_MAX_H: u32 = 450;
const POSTER_CDN_MAX_W: u32 = 342;
const THUMB_MAX_W: u32 = 80;
const THUMB_MAX_H: u32 = 120;
const WIDESCREEN_MAX_W: u32 = 640;
const WIDESCREEN_MAX_H: u32 = 360;
pub const STILL_MAX_W: u32 = 320;
pub const STILL_MAX_H: u32 = 180;
pub const HERO_MAX_W: u32 = 1280;
pub const HERO_MAX_H: u32 = 720;
const LOGO_DECODE_MAX_WIDTH: u32 = 520;
const LOGO_DECODE_MAX_HEIGHT: u32 = 200;

pub const AMBIENT_BLEED_WIDTH: u32 = 48;
pub const AMBIENT_BLEED_HEIGHT: u32 = 27;

const PROTECTED_EPOCH_WINDOW: u64 = 2;

const SIDECAR_MAGIC: &[u8; 4] = b"FBG1";
const SIDECAR_HEADER_LEN: usize = 12;
const SIDECAR_MAX_DIMENSION: u32 = 2048;

#[derive(Clone, Debug, PartialEq, Eq)]
enum CacheEntry {
    Loading,
    Ready(PathBuf),
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
enum ImageVariant {
    Raw = 0,
    Poster = 1,
    Thumb = 2,
    Wide = 3,
    Hero = 4,
    Logo = 5,
    Still = 6,
    Ambient = 7,
}

impl ImageVariant {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Poster => "poster",
            Self::Thumb => "thumb",
            Self::Wide => "wide",
            Self::Hero => "hero",
            Self::Logo => "logo",
            Self::Still => "still",
            Self::Ambient => "ambient",
        }
    }

    const fn uses_sidecar(self) -> bool {
        matches!(
            self,
            Self::Poster | Self::Thumb | Self::Still | Self::Ambient
        )
    }
}

#[derive(Clone)]
enum TextureSlot {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

#[derive(Clone)]
struct TextureRecord {
    variant: ImageVariant,
    url: Arc<str>,
    slot: TextureSlot,
    last_used_epoch: u64,
    access_seq: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LookupOutcome {
    Ready,
    Loading,
    Spawned,
    Failed,
}

#[inline]
fn is_epoch_protected(last_used_epoch: u64, current_epoch: u64) -> bool {
    last_used_epoch.saturating_add(PROTECTED_EPOCH_WINDOW) >= current_epoch
}

#[inline]
fn hash_variant_url(variant: ImageVariant, url: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (variant as u8).hash(&mut hasher);
    url.hash(&mut hasher);
    hasher.finish()
}

pub struct BoundedTextureLru {
    capacity: usize,
    map: HashMap<u64, SmallVec<[TextureRecord; 1]>>,
    ready_count: usize,
    failed_count: usize,
    next_seq: u64,
}

impl BoundedTextureLru {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            map: HashMap::with_capacity(capacity.saturating_add(16)),
            ready_count: 0,
            failed_count: 0,
            next_seq: 0,
        }
    }

    pub fn len_ready(&self) -> usize {
        self.ready_count
    }

    #[inline]
    fn bump_seq(&mut self) -> u64 {
        self.next_seq = self.next_seq.wrapping_add(1);
        self.next_seq
    }

    fn lookup_or_begin_variant(
        &mut self,
        variant: ImageVariant,
        key: &str,
        current_epoch: u64,
    ) -> (Option<Arc<RenderImage>>, LookupOutcome) {
        let key_hash = hash_variant_url(variant, key);
        if let Some(bucket) = self.map.get_mut(&key_hash) {
            if let Some(rec) = bucket
                .iter_mut()
                .find(|r| r.variant == variant && r.url.as_ref() == key)
            {
                rec.last_used_epoch = rec.last_used_epoch.max(current_epoch);
                match &rec.slot {
                    TextureSlot::Ready(img) => {
                        let img = Arc::clone(img);
                        self.next_seq = self.next_seq.wrapping_add(1);
                        rec.access_seq = self.next_seq;
                        return (Some(img), LookupOutcome::Ready);
                    }
                    TextureSlot::Loading => return (None, LookupOutcome::Loading),
                    TextureSlot::Failed => return (None, LookupOutcome::Failed),
                }
            }
        }

        let seq = self.bump_seq();
        let rec = TextureRecord {
            variant,
            url: Arc::from(key),
            slot: TextureSlot::Loading,
            last_used_epoch: current_epoch,
            access_seq: seq,
        };
        self.map.entry(key_hash).or_default().push(rec);
        (None, LookupOutcome::Spawned)
    }

    pub fn lookup_or_begin(
        &mut self,
        key: &str,
        current_epoch: u64,
    ) -> (Option<Arc<RenderImage>>, bool) {
        let (img, outcome) = self.lookup_or_begin_variant(ImageVariant::Raw, key, current_epoch);
        (img, outcome == LookupOutcome::Spawned)
    }

    fn insert_ready_variant(
        &mut self,
        variant: ImageVariant,
        key: &str,
        image: Arc<RenderImage>,
        current_epoch: u64,
    ) -> Vec<Arc<RenderImage>> {
        let mut evicted = Vec::new();
        let key_hash = hash_variant_url(variant, key);
        let seq = self.bump_seq();
        let image_ptr = Arc::as_ptr(&image);

        let bucket = self.map.entry(key_hash).or_default();
        if let Some(rec) = bucket
            .iter_mut()
            .find(|r| r.variant == variant && r.url.as_ref() == key)
        {
            rec.last_used_epoch = rec.last_used_epoch.max(current_epoch);
            rec.access_seq = seq;
            match std::mem::replace(&mut rec.slot, TextureSlot::Ready(image)) {
                TextureSlot::Ready(prev) => {
                    if !std::ptr::eq(Arc::as_ptr(&prev), image_ptr) {
                        evicted.push(prev);
                    }
                }
                TextureSlot::Loading => {
                    self.ready_count += 1;
                }
                TextureSlot::Failed => {
                    self.failed_count = self.failed_count.saturating_sub(1);
                    self.ready_count += 1;
                }
            }
        } else {
            bucket.push(TextureRecord {
                variant,
                url: Arc::from(key),
                slot: TextureSlot::Ready(image),
                last_used_epoch: current_epoch,
                access_seq: seq,
            });
            self.ready_count += 1;
        }

        evicted.extend(self.evict_stale_excess(current_epoch, Some(image_ptr)));
        evicted
    }

    pub fn insert_ready(
        &mut self,
        key: String,
        image: Arc<RenderImage>,
        current_epoch: u64,
    ) -> Vec<Arc<RenderImage>> {
        self.insert_ready_variant(ImageVariant::Raw, &key, image, current_epoch)
    }

    pub fn sweep_to_capacity(&mut self, current_epoch: u64) -> Vec<Arc<RenderImage>> {
        self.evict_stale_excess(current_epoch, None)
    }

    fn evict_stale_excess(
        &mut self,
        current_epoch: u64,
        protect_ptr: Option<*const RenderImage>,
    ) -> Vec<Arc<RenderImage>> {
        if self.ready_count <= self.capacity {
            return Vec::new();
        }
        let mut candidates: Vec<(u64, u64)> = Vec::new();
        for (&key_hash, bucket) in &self.map {
            for rec in bucket {
                if matches!(rec.slot, TextureSlot::Ready(_))
                    && !is_epoch_protected(rec.last_used_epoch, current_epoch)
                {
                    candidates.push((rec.access_seq, key_hash));
                }
            }
        }
        self.evict_ready_candidates(candidates, self.capacity, protect_ptr)
    }

    pub fn trim_to_limit(&mut self, keep_mru: usize) -> Vec<Arc<RenderImage>> {
        if self.ready_count <= keep_mru {
            return Vec::new();
        }
        let mut candidates: Vec<(u64, u64)> = Vec::with_capacity(self.ready_count);
        for (&key_hash, bucket) in &self.map {
            for rec in bucket {
                if matches!(rec.slot, TextureSlot::Ready(_)) {
                    candidates.push((rec.access_seq, key_hash));
                }
            }
        }
        self.evict_ready_candidates(candidates, keep_mru, None)
    }

    fn evict_ready_candidates(
        &mut self,
        mut candidates: Vec<(u64, u64)>,
        target_limit: usize,
        protect_ptr: Option<*const RenderImage>,
    ) -> Vec<Arc<RenderImage>> {
        if self.ready_count <= target_limit || candidates.is_empty() {
            return Vec::new();
        }
        let excess = self.ready_count - target_limit;
        if candidates.len() > excess {
            candidates.select_nth_unstable_by_key(excess - 1, |&(seq, _)| seq);
            candidates.truncate(excess);
        }
        candidates.sort_unstable_by_key(|&(seq, _)| seq);

        let mut evicted = Vec::with_capacity(candidates.len());
        for (target_seq, key_hash) in candidates {
            if self.ready_count <= target_limit {
                break;
            }
            let mut remove_bucket = false;
            if let Some(bucket) = self.map.get_mut(&key_hash) {
                if let Some(pos) = bucket.iter().position(|r| r.access_seq == target_seq) {
                    let old_rec = bucket.swap_remove(pos);
                    remove_bucket = bucket.is_empty();
                    if let TextureSlot::Ready(old_img) = old_rec.slot {
                        self.ready_count = self.ready_count.saturating_sub(1);
                        if protect_ptr.is_none_or(|p| !std::ptr::eq(Arc::as_ptr(&old_img), p)) {
                            evicted.push(old_img);
                        }
                    }
                }
            }
            if remove_bucket {
                self.map.remove(&key_hash);
            }
        }
        evicted
    }

    pub fn drain_all(&mut self) -> Vec<Arc<RenderImage>> {
        self.ready_count = 0;
        self.failed_count = 0;
        let mut out = Vec::new();
        for (_, bucket) in self.map.drain() {
            for rec in bucket {
                if let TextureSlot::Ready(img) = rec.slot {
                    out.push(img);
                }
            }
        }
        out
    }

    fn mark_failed_variant(&mut self, variant: ImageVariant, key: &str, current_epoch: u64) {
        let key_hash = hash_variant_url(variant, key);
        let seq = self.bump_seq();
        let bucket = self.map.entry(key_hash).or_default();
        if let Some(rec) = bucket
            .iter_mut()
            .find(|r| r.variant == variant && r.url.as_ref() == key)
        {
            rec.last_used_epoch = rec.last_used_epoch.max(current_epoch);
            rec.access_seq = seq;
            match std::mem::replace(&mut rec.slot, TextureSlot::Failed) {
                TextureSlot::Ready(_) => {
                    self.ready_count = self.ready_count.saturating_sub(1);
                    self.failed_count += 1;
                }
                TextureSlot::Loading => {
                    self.failed_count += 1;
                }
                TextureSlot::Failed => {}
            }
        } else {
            bucket.push(TextureRecord {
                variant,
                url: Arc::from(key),
                slot: TextureSlot::Failed,
                last_used_epoch: current_epoch,
                access_seq: seq,
            });
            self.failed_count += 1;
        }

        self.evict_excess_failures();
    }

    pub fn mark_failed(&mut self, key: &str, current_epoch: u64) {
        self.mark_failed_variant(ImageVariant::Raw, key, current_epoch);
    }

    fn evict_excess_failures(&mut self) {
        while self.failed_count > MAX_FAILED_ENTRIES {
            let mut oldest: Option<(u64, u64)> = None;
            for (&key_hash, bucket) in &self.map {
                for rec in bucket {
                    if matches!(rec.slot, TextureSlot::Failed)
                        && oldest.is_none_or(|(best_seq, _)| rec.access_seq < best_seq)
                    {
                        oldest = Some((rec.access_seq, key_hash));
                    }
                }
            }
            let Some((target_seq, key_hash)) = oldest else {
                break;
            };
            let mut remove_bucket = false;
            if let Some(bucket) = self.map.get_mut(&key_hash) {
                if let Some(pos) = bucket.iter().position(|r| r.access_seq == target_seq) {
                    bucket.swap_remove(pos);
                    remove_bucket = bucket.is_empty();
                    self.failed_count = self.failed_count.saturating_sub(1);
                }
            }
            if remove_bucket {
                self.map.remove(&key_hash);
            }
        }
    }

    pub fn clear_failures(&mut self) {
        self.failed_count = 0;
        self.map.retain(|_, bucket| {
            bucket.retain(|rec| !matches!(rec.slot, TextureSlot::Failed));
            !bucket.is_empty()
        });
    }
}

pub fn rewrite_cdn_image_url(url: &str, max_w: u32) -> Cow<'_, str> {
    const TMDB_HTTPS: &str = "https://image.tmdb.org/t/p/";
    const TMDB_HTTP: &str = "http://image.tmdb.org/t/p/";
    const METAHUB_HTTPS: &str = "https://images.metahub.space/";
    const METAHUB_HTTP: &str = "http://images.metahub.space/";

    let tmdb_prefix = if url.starts_with(TMDB_HTTPS) {
        Some(TMDB_HTTPS)
    } else if url.starts_with(TMDB_HTTP) {
        Some(TMDB_HTTP)
    } else {
        None
    };

    if let Some(prefix) = tmdb_prefix {
        let rest = &url[prefix.len()..];
        if let Some(slash_idx) = rest.find('/') {
            let size_seg = &rest[..slash_idx];
            let tail = &rest[slash_idx..];
            let (target_w, target_seg) = if max_w <= 185 {
                (185_u32, "w185")
            } else if max_w <= 320 {
                (300_u32, "w300")
            } else if max_w <= 500 {
                (342_u32, "w342")
            } else if max_w <= 780 {
                (780_u32, "w780")
            } else {
                (1280_u32, "w1280")
            };

            let should_rewrite = if size_seg == "original" {
                true
            } else if let Some(digits) = size_seg.strip_prefix('w') {
                !digits.is_empty()
                    && digits.bytes().all(|b| b.is_ascii_digit())
                    && digits.parse::<u32>().is_ok_and(|w| w > target_w)
            } else {
                false
            };

            if should_rewrite {
                return Cow::Owned(format!("{prefix}{target_seg}{tail}"));
            }
        }
        return Cow::Borrowed(url);
    }

    let metahub_prefix = if url.starts_with(METAHUB_HTTPS) {
        Some(METAHUB_HTTPS)
    } else if url.starts_with(METAHUB_HTTP) {
        Some(METAHUB_HTTP)
    } else {
        None
    };

    if let Some(prefix) = metahub_prefix {
        let rest = &url[prefix.len()..];
        if let Some(kind_slash) = rest.find('/') {
            let kind = &rest[..kind_slash];
            let after_kind = &rest[kind_slash..];
            if !kind.is_empty() {
                if let Some(tail) = after_kind.strip_prefix("/large/") {
                    let target_size = if max_w <= 120 { "small" } else { "medium" };
                    return Cow::Owned(format!("{prefix}{kind}/{target_size}/{tail}"));
                }
            }
        }
        return Cow::Borrowed(url);
    }

    Cow::Borrowed(url)
}

pub fn default_poster_cache_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        let trimmed = xdg.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("ferrite").join("posters");
        }
    }
    if let Some(cache) = dirs::cache_dir() {
        return cache.join("ferrite").join("posters");
    }
    std::env::temp_dir().join("ferrite").join("posters")
}

#[inline]
fn url_sha256_hex(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn cache_path_for_url(cache_root: &Path, url: &str) -> PathBuf {
    let digest = url_sha256_hex(url);
    let shard = &digest[..2];
    let ext = infer_image_extension(url);
    cache_root.join(shard).join(format!("{digest}.{ext}"))
}

pub fn sidecar_path_for_url(cache_root: &Path, url: &str, variant: &str) -> PathBuf {
    let digest = url_sha256_hex(url);
    let shard = &digest[..2];
    cache_root
        .join(shard)
        .join(format!("{digest}.{variant}.bin"))
}

fn infer_image_extension(url_str: &str) -> &'static str {
    let path_lower = Url::parse(url_str)
        .ok()
        .map(|u| u.path().to_ascii_lowercase())
        .unwrap_or_else(|| url_str.to_ascii_lowercase());

    if path_lower.ends_with(".png") {
        "png"
    } else if path_lower.ends_with(".webp") {
        "webp"
    } else if path_lower.ends_with(".gif") {
        "gif"
    } else {
        "jpg"
    }
}

pub fn decode_scaled_render_image(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Arc<RenderImage>> {
    let dyn_img = image::load_from_memory(bytes).ok()?;
    let resized = if dyn_img.width() > max_w || dyn_img.height() > max_h {
        dyn_img.resize(max_w, max_h, FilterType::Triangle)
    } else {
        dyn_img
    };
    let mut rgba = resized.to_rgba8();
    for px_slice in rgba.as_chunks_mut::<4>().0 {
        let a = px_slice[3];
        if a == 255 {
            px_slice.swap(0, 2);
        } else if a == 0 {
            px_slice[0] = 0;
            px_slice[1] = 0;
            px_slice[2] = 0;
        } else {
            let r = px_slice[0] as u16;
            let g = px_slice[1] as u16;
            let b = px_slice[2] as u16;
            let a16 = a as u16;
            px_slice[0] = ((b * a16 + 127) / 255) as u8;
            px_slice[1] = ((g * a16 + 127) / 255) as u8;
            px_slice[2] = ((r * a16 + 127) / 255) as u8;
        }
    }
    let frame = Frame::new(rgba);
    Some(Arc::new(RenderImage::new(smallvec![frame])))
}

pub fn build_ambient_bleed_image(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let dyn_img = image::load_from_memory(bytes).ok()?;
    let small = dyn_img
        .resize_exact(AMBIENT_BLEED_WIDTH, AMBIENT_BLEED_HEIGHT, FilterType::Triangle)
        .to_rgba8();

    let w = AMBIENT_BLEED_WIDTH as usize;
    let h = AMBIENT_BLEED_HEIGHT as usize;
    let mut src = small.into_raw();
    let mut tmp = vec![0_u8; src.len()];

    for _ in 0..2 {
        box_blur_horizontal(&src, &mut tmp, w, h, 4);
        box_blur_vertical(&tmp, &mut src, w, h, 3);
    }

    for px_slice in src.as_chunks_mut::<4>().0 {
        let r = px_slice[0] as f32;
        let g = px_slice[1] as f32;
        let b = px_slice[2] as f32;
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let sat = 1.18_f32;
        let nr = (luma + (r - luma) * sat).clamp(0.0, 255.0) as u8;
        let ng = (luma + (g - luma) * sat).clamp(0.0, 255.0) as u8;
        let nb = (luma + (b - luma) * sat).clamp(0.0, 255.0) as u8;
        px_slice[0] = nb;
        px_slice[1] = ng;
        px_slice[2] = nr;
        px_slice[3] = 255;
    }

    let bgra_buf = RgbaImage::from_raw(AMBIENT_BLEED_WIDTH, AMBIENT_BLEED_HEIGHT, src)?;
    let frame = Frame::new(bgra_buf);
    Some(Arc::new(RenderImage::new(smallvec![frame])))
}

fn encode_sidecar_bytes(image: &RenderImage) -> Option<Vec<u8>> {
    let size = image.size(0);
    if size.width.0 <= 0 || size.height.0 <= 0 {
        return None;
    }
    let width = size.width.0 as u32;
    let height = size.height.0 as u32;
    let raw = image.as_bytes(0)?;
    let expected_len = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if raw.len() != expected_len {
        return None;
    }
    let mut out = Vec::with_capacity(SIDECAR_HEADER_LEN + raw.len());
    out.extend_from_slice(SIDECAR_MAGIC);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(raw);
    Some(out)
}

fn decode_sidecar_render_image(
    bytes: &[u8],
    max_w: u32,
    max_h: u32,
) -> Option<Arc<RenderImage>> {
    if bytes.len() < SIDECAR_HEADER_LEN || &bytes[0..4] != SIDECAR_MAGIC {
        return None;
    }
    let width = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    if width == 0
        || height == 0
        || width > max_w
        || height > max_h
        || width > SIDECAR_MAX_DIMENSION
        || height > SIDECAR_MAX_DIMENSION
    {
        return None;
    }
    let expected_len = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if bytes.len() != SIDECAR_HEADER_LEN + expected_len {
        return None;
    }
    let bgra = RgbaImage::from_raw(width, height, bytes[SIDECAR_HEADER_LEN..].to_vec())?;
    Some(Arc::new(RenderImage::new(smallvec![Frame::new(bgra)])))
}

struct DiskCacheFile {
    mtime: SystemTime,
    len: u64,
    path: PathBuf,
}

pub fn prune_disk_cache_to_budget(cache_root: &Path, max_bytes: u64) -> u64 {
    if !cache_root.is_dir() {
        return 0;
    }
    let mut stack = vec![cache_root.to_path_buf()];
    let mut files: Vec<DiskCacheFile> = Vec::new();
    let mut total_bytes: u64 = 0;
    let now = SystemTime::now();

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                let is_recent_part = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.contains(".part."))
                    && meta
                        .modified()
                        .ok()
                        .and_then(|m| now.duration_since(m).ok())
                        .is_none_or(|age| age < Duration::from_secs(60));
                if is_recent_part {
                    continue;
                }
                let len = meta.len();
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                total_bytes = total_bytes.saturating_add(len);
                files.push(DiskCacheFile { mtime, len, path });
            }
        }
    }

    if total_bytes <= max_bytes {
        return 0;
    }

    files.sort_unstable_by(|a, b| a.mtime.cmp(&b.mtime).then_with(|| a.path.cmp(&b.path)));
    let mut freed: u64 = 0;
    for file in files {
        if total_bytes <= max_bytes {
            break;
        }
        if std::fs::remove_file(&file.path).is_ok() {
            total_bytes = total_bytes.saturating_sub(file.len);
            freed = freed.saturating_add(file.len);
        }
    }
    freed
}

fn box_blur_horizontal(src: &[u8], dst: &mut [u8], w: usize, h: usize, radius: usize) {
    for y in 0..h {
        for x in 0..w {
            let mut r_acc = 0_u32;
            let mut g_acc = 0_u32;
            let mut b_acc = 0_u32;
            let mut count = 0_u32;
            let x0 = x.saturating_sub(radius);
            let x1 = (x + radius).min(w - 1);
            for sx in x0..=x1 {
                let idx = (y * w + sx) * 4;
                r_acc += src[idx] as u32;
                g_acc += src[idx + 1] as u32;
                b_acc += src[idx + 2] as u32;
                count += 1;
            }
            let out = (y * w + x) * 4;
            dst[out] = (r_acc / count) as u8;
            dst[out + 1] = (g_acc / count) as u8;
            dst[out + 2] = (b_acc / count) as u8;
            dst[out + 3] = 255;
        }
    }
}

fn box_blur_vertical(src: &[u8], dst: &mut [u8], w: usize, h: usize, radius: usize) {
    for y in 0..h {
        let y0 = y.saturating_sub(radius);
        let y1 = (y + radius).min(h - 1);
        for x in 0..w {
            let mut r_acc = 0_u32;
            let mut g_acc = 0_u32;
            let mut b_acc = 0_u32;
            let mut count = 0_u32;
            for sy in y0..=y1 {
                let idx = (sy * w + x) * 4;
                r_acc += src[idx] as u32;
                g_acc += src[idx + 1] as u32;
                b_acc += src[idx + 2] as u32;
                count += 1;
            }
            let out = (y * w + x) * 4;
            dst[out] = (r_acc / count) as u8;
            dst[out + 1] = (g_acc / count) as u8;
            dst[out + 2] = (b_acc / count) as u8;
            dst[out + 3] = 255;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LruTarget {
    Decoded,
    Thumb,
    Still,
    Hero,
    Ambient,
}

#[derive(Clone, Copy)]
struct DecodeTargetSpec {
    variant: ImageVariant,
    cdn_max_w: u32,
    max_w: u32,
    max_h: u32,
    target: LruTarget,
}

impl DecodeTargetSpec {
    const POSTER: Self = Self {
        variant: ImageVariant::Poster,
        cdn_max_w: POSTER_CDN_MAX_W,
        max_w: POSTER_MAX_W,
        max_h: POSTER_MAX_H,
        target: LruTarget::Decoded,
    };
    const THUMB: Self = Self {
        variant: ImageVariant::Thumb,
        cdn_max_w: THUMB_MAX_W,
        max_w: THUMB_MAX_W,
        max_h: THUMB_MAX_H,
        target: LruTarget::Thumb,
    };
    const WIDE: Self = Self {
        variant: ImageVariant::Wide,
        cdn_max_w: WIDESCREEN_MAX_W,
        max_w: WIDESCREEN_MAX_W,
        max_h: WIDESCREEN_MAX_H,
        target: LruTarget::Decoded,
    };
    const STILL: Self = Self {
        variant: ImageVariant::Still,
        cdn_max_w: STILL_MAX_W,
        max_w: STILL_MAX_W,
        max_h: STILL_MAX_H,
        target: LruTarget::Still,
    };
    const HERO: Self = Self {
        variant: ImageVariant::Hero,
        cdn_max_w: HERO_MAX_W,
        max_w: HERO_MAX_W,
        max_h: HERO_MAX_H,
        target: LruTarget::Hero,
    };
    const LOGO: Self = Self {
        variant: ImageVariant::Logo,
        cdn_max_w: LOGO_DECODE_MAX_WIDTH,
        max_w: LOGO_DECODE_MAX_WIDTH,
        max_h: LOGO_DECODE_MAX_HEIGHT,
        target: LruTarget::Decoded,
    };
    const AMBIENT: Self = Self {
        variant: ImageVariant::Ambient,
        cdn_max_w: HERO_MAX_W,
        max_w: AMBIENT_BLEED_WIDTH,
        max_h: AMBIENT_BLEED_HEIGHT,
        target: LruTarget::Ambient,
    };
}

struct PosterCacheInner {
    cache_root: PathBuf,
    entries: RwLock<HashMap<String, CacheEntry>>,
    failed_entry_urls: Mutex<VecDeque<String>>,
    in_flight_urls: Mutex<HashMap<String, Arc<Notify>>>,
    initialized_dirs: Mutex<HashSet<PathBuf>>,
    decoded_lru: Mutex<BoundedTextureLru>,
    thumb_lru: Mutex<BoundedTextureLru>,
    still_lru: Mutex<BoundedTextureLru>,
    hero_lru: Mutex<BoundedTextureLru>,
    ambient_lru: Mutex<BoundedTextureLru>,
    evicted_images: Mutex<Vec<Arc<RenderImage>>>,
    client: reqwest::Client,
    semaphore: Semaphore,
    decode_semaphore: Semaphore,
    disk_io_semaphore: Semaphore,
    runtime: tokio::runtime::Handle,
    wakeup_tx: async_channel::Sender<()>,
    temp_seq: AtomicU64,
    render_epoch: AtomicU64,
    disk_writes: AtomicU64,
    disk_prune_in_flight: AtomicBool,
}

impl PosterCacheInner {
    #[inline]
    fn current_epoch(&self) -> u64 {
        self.render_epoch.load(Ordering::Acquire)
    }

    fn lru_mutex(&self, target: LruTarget) -> &Mutex<BoundedTextureLru> {
        match target {
            LruTarget::Decoded => &self.decoded_lru,
            LruTarget::Thumb => &self.thumb_lru,
            LruTarget::Still => &self.still_lru,
            LruTarget::Hero => &self.hero_lru,
            LruTarget::Ambient => &self.ambient_lru,
        }
    }
}

#[derive(Clone)]
pub struct PosterCache {
    inner: Arc<PosterCacheInner>,
    wakeup_rx: async_channel::Receiver<()>,
}

impl PosterCache {
    pub fn new(cache_root: PathBuf, runtime: tokio::runtime::Handle) -> Self {
        let client = reqwest::Client::builder()
            .timeout(HTTP_POSTER_TIMEOUT)
            .user_agent("Ferrite/0.1.0")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let (wakeup_tx, wakeup_rx) = async_channel::bounded(1);

        let inner = Arc::new(PosterCacheInner {
            cache_root,
            entries: RwLock::new(HashMap::with_capacity(256)),
            failed_entry_urls: Mutex::new(VecDeque::with_capacity(64)),
            in_flight_urls: Mutex::new(HashMap::with_capacity(32)),
            initialized_dirs: Mutex::new(HashSet::with_capacity(64)),
            decoded_lru: Mutex::new(BoundedTextureLru::with_capacity(MAX_DECODED_TEXTURES)),
            thumb_lru: Mutex::new(BoundedTextureLru::with_capacity(MAX_THUMB_TEXTURES)),
            still_lru: Mutex::new(BoundedTextureLru::with_capacity(MAX_STILL_TEXTURES)),
            hero_lru: Mutex::new(BoundedTextureLru::with_capacity(MAX_HERO_TEXTURES)),
            ambient_lru: Mutex::new(BoundedTextureLru::with_capacity(MAX_AMBIENT_TEXTURES)),
            evicted_images: Mutex::new(Vec::with_capacity(16)),
            client,
            semaphore: Semaphore::new(MAX_CONCURRENT_POSTER_DOWNLOADS),
            decode_semaphore: Semaphore::new(MAX_CONCURRENT_IMAGE_DECODES),
            disk_io_semaphore: Semaphore::new(MAX_CONCURRENT_DISK_IO),
            runtime,
            wakeup_tx,
            temp_seq: AtomicU64::new(1),
            render_epoch: AtomicU64::new(1),
            disk_writes: AtomicU64::new(0),
            disk_prune_in_flight: AtomicBool::new(false),
        });

        if inner.cache_root.is_dir() {
            trigger_background_disk_prune(&inner);
        }

        Self { inner, wakeup_rx }
    }

    pub fn with_default_dir(runtime: tokio::runtime::Handle) -> Self {
        Self::new(default_poster_cache_dir(), runtime)
    }

    pub fn wakeup_receiver(&self) -> async_channel::Receiver<()> {
        self.wakeup_rx.clone()
    }

    pub fn cache_root(&self) -> &Path {
        &self.inner.cache_root
    }

    pub fn current_render_epoch(&self) -> u64 {
        self.inner.current_epoch()
    }

    pub fn begin_render_frame(&self) -> u64 {
        let epoch = self.inner.render_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let mut swept = self.inner.decoded_lru.lock().sweep_to_capacity(epoch);
        swept.extend(self.inner.thumb_lru.lock().sweep_to_capacity(epoch));
        swept.extend(self.inner.still_lru.lock().sweep_to_capacity(epoch));
        swept.extend(self.inner.hero_lru.lock().sweep_to_capacity(epoch));
        swept.extend(self.inner.ambient_lru.lock().sweep_to_capacity(epoch));
        if !swept.is_empty() {
            self.inner.evicted_images.lock().extend(swept);
        }
        epoch
    }

    pub fn resolve_poster_image(&self, url: &str) -> Option<Arc<RenderImage>> {
        self.resolve_spec_if_ready(url, DecodeTargetSpec::POSTER)
    }

    pub fn resolve_poster_url(&self, url: Option<&Url>) -> Option<Arc<RenderImage>> {
        self.resolve_poster_image(url?.as_str())
    }

    pub fn resolve_thumbnail_image(&self, url: &str) -> Option<Arc<RenderImage>> {
        self.resolve_spec_if_ready(url, DecodeTargetSpec::THUMB)
    }

    pub fn resolve_thumbnail_url(&self, url: Option<&Url>) -> Option<Arc<RenderImage>> {
        self.resolve_thumbnail_image(url?.as_str())
    }

    pub fn resolve_widescreen_image(&self, url: &str) -> Option<Arc<RenderImage>> {
        self.resolve_spec_if_ready(url, DecodeTargetSpec::WIDE)
    }

    pub fn resolve_widescreen_url(&self, url: Option<&Url>) -> Option<Arc<RenderImage>> {
        self.resolve_widescreen_image(url?.as_str())
    }

    pub fn resolve_episode_still_image(&self, url: &str) -> Option<Arc<RenderImage>> {
        self.resolve_spec_if_ready(url, DecodeTargetSpec::STILL)
    }

    pub fn resolve_episode_still_url(&self, url: Option<&Url>) -> Option<Arc<RenderImage>> {
        self.resolve_episode_still_image(url?.as_str())
    }

    pub fn resolve_hero_backdrop_image(&self, url: &str) -> Option<Arc<RenderImage>> {
        self.resolve_spec_if_ready(url, DecodeTargetSpec::HERO)
    }

    pub fn resolve_hero_backdrop_url(&self, url: Option<&Url>) -> Option<Arc<RenderImage>> {
        self.resolve_hero_backdrop_image(url?.as_str())
    }

    pub fn resolve_logo_image(&self, url: &str) -> Option<Arc<RenderImage>> {
        self.resolve_spec_if_ready(url, DecodeTargetSpec::LOGO)
    }

    pub fn resolve_logo_url(&self, url: Option<&Url>) -> Option<Arc<RenderImage>> {
        self.resolve_logo_image(url?.as_str())
    }

    pub fn resolve_first_available_hero<'a>(
        &self,
        urls: impl IntoIterator<Item = &'a str>,
    ) -> Option<Arc<RenderImage>> {
        self.resolve_first_available_for_spec(urls, DecodeTargetSpec::HERO)
    }

    pub fn resolve_first_available_ambient<'a>(
        &self,
        urls: impl IntoIterator<Item = &'a str>,
    ) -> Option<Arc<RenderImage>> {
        self.resolve_first_available_for_spec(urls, DecodeTargetSpec::AMBIENT)
    }

    pub fn resolve_first_available_logo<'a>(
        &self,
        urls: impl IntoIterator<Item = &'a str>,
    ) -> Option<Arc<RenderImage>> {
        self.resolve_first_available_for_spec(urls, DecodeTargetSpec::LOGO)
    }

    pub fn resolve_widescreen_or_poster(
        &self,
        wide_url: Option<&str>,
        poster_url: Option<&str>,
    ) -> Option<Arc<RenderImage>> {
        if let Some(wide) = wide_url {
            let (img, outcome) = self.resolve_candidate_in_target(wide, DecodeTargetSpec::WIDE);
            match outcome {
                LookupOutcome::Ready => return img,
                LookupOutcome::Loading | LookupOutcome::Spawned => return None,
                LookupOutcome::Failed => {}
            }
        }
        if let Some(poster) = poster_url {
            return self.resolve_poster_image(poster);
        }
        None
    }

    pub fn prefetch_poster(&self, url: &str) {
        let _ = self.resolve_poster_image(url);
    }

    pub fn prefetch_hero_backdrop(&self, url: &str) {
        let _ = self.resolve_hero_backdrop_image(url);
    }

    pub fn push_evicted_image(&self, image: Arc<RenderImage>) {
        self.inner.evicted_images.lock().push(image);
    }

    pub fn trim_for_playback(&self) {
        let mut newly_evicted = self
            .inner
            .decoded_lru
            .lock()
            .trim_to_limit(PLAYBACK_RETAINED_DECODED_TEXTURES);
        newly_evicted.extend(
            self.inner
                .thumb_lru
                .lock()
                .trim_to_limit(PLAYBACK_RETAINED_THUMB_TEXTURES),
        );
        newly_evicted.extend(
            self.inner
                .still_lru
                .lock()
                .trim_to_limit(PLAYBACK_RETAINED_STILL_TEXTURES),
        );
        newly_evicted.extend(
            self.inner
                .hero_lru
                .lock()
                .trim_to_limit(PLAYBACK_RETAINED_HERO_TEXTURES),
        );
        newly_evicted.extend(
            self.inner
                .ambient_lru
                .lock()
                .trim_to_limit(PLAYBACK_RETAINED_AMBIENT_TEXTURES),
        );
        if !newly_evicted.is_empty() {
            self.inner.evicted_images.lock().extend(newly_evicted);
        }
    }

    pub fn decoded_ready_count(&self) -> usize {
        self.inner.decoded_lru.lock().len_ready()
    }

    pub fn thumb_ready_count(&self) -> usize {
        self.inner.thumb_lru.lock().len_ready()
    }

    pub fn still_ready_count(&self) -> usize {
        self.inner.still_lru.lock().len_ready()
    }

    pub fn drain_evicted_images(&self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut *self.inner.evicted_images.lock())
    }

    fn resolve_spec_if_ready(
        &self,
        url: &str,
        spec: DecodeTargetSpec,
    ) -> Option<Arc<RenderImage>> {
        self.resolve_candidate_in_target(url, spec).0
    }

    fn resolve_first_available_for_spec<'a>(
        &self,
        urls: impl IntoIterator<Item = &'a str>,
        spec: DecodeTargetSpec,
    ) -> Option<Arc<RenderImage>> {
        for url in urls {
            let (img, outcome) = self.resolve_candidate_in_target(url, spec);
            match outcome {
                LookupOutcome::Ready => return img,
                LookupOutcome::Loading | LookupOutcome::Spawned => return None,
                LookupOutcome::Failed => continue,
            }
        }
        None
    }

    fn resolve_candidate_in_target(
        &self,
        url: &str,
        spec: DecodeTargetSpec,
    ) -> (Option<Arc<RenderImage>>, LookupOutcome) {
        let trimmed = url.trim();
        if trimmed.is_empty() || !(trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        {
            return (None, LookupOutcome::Failed);
        }

        let epoch = self.inner.current_epoch();
        let (hit, outcome) = self
            .inner
            .lru_mutex(spec.target)
            .lock()
            .lookup_or_begin_variant(spec.variant, trimmed, epoch);

        if outcome == LookupOutcome::Spawned {
            let inner = Arc::clone(&self.inner);
            let url_owned = trimmed.to_owned();
            self.inner.runtime.spawn(async move {
                if spec.target == LruTarget::Ambient {
                    compute_or_fetch_ambient(inner, url_owned, spec.cdn_max_w).await;
                } else {
                    fetch_and_decode_into_target(inner, url_owned, spec).await;
                }
            });
        }

        (hit, outcome)
    }

    pub fn resolve_or_fetch(&self, url: &str) -> Option<PathBuf> {
        let trimmed = url.trim();
        if trimmed.is_empty() || !(trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        {
            return None;
        }

        {
            let guard = self.inner.entries.read();
            if let Some(entry) = guard.get(trimmed) {
                return match entry {
                    CacheEntry::Ready(path) => Some(path.clone()),
                    CacheEntry::Loading | CacheEntry::Failed => None,
                };
            }
        }

        {
            let mut guard = self.inner.entries.write();
            if let Some(entry) = guard.get(trimmed) {
                return match entry {
                    CacheEntry::Ready(path) => Some(path.clone()),
                    CacheEntry::Loading | CacheEntry::Failed => None,
                };
            }
            guard.insert(trimmed.to_owned(), CacheEntry::Loading);
        }

        let inner = Arc::clone(&self.inner);
        let url_owned = trimmed.to_owned();
        self.inner.runtime.spawn(async move {
            let _ = fetch_or_load_disk(inner, url_owned).await;
        });

        None
    }

    pub fn resolve_url(&self, url: Option<&Url>) -> Option<PathBuf> {
        self.resolve_or_fetch(url?.as_str())
    }

    pub fn resolve_ambient_bleed(&self, url: &str) -> Option<Arc<RenderImage>> {
        self.resolve_spec_if_ready(url, DecodeTargetSpec::AMBIENT)
    }

    pub fn resolve_ambient_url(&self, url: Option<&Url>) -> Option<Arc<RenderImage>> {
        self.resolve_ambient_bleed(url?.as_str())
    }

    pub fn clear_failures(&self) {
        self.inner
            .entries
            .write()
            .retain(|_, v| !matches!(v, CacheEntry::Failed));
        self.inner.failed_entry_urls.lock().clear();
        self.inner.initialized_dirs.lock().clear();
        self.inner.decoded_lru.lock().clear_failures();
        self.inner.thumb_lru.lock().clear_failures();
        self.inner.still_lru.lock().clear_failures();
        self.inner.hero_lru.lock().clear_failures();
        self.inner.ambient_lru.lock().clear_failures();
    }
}

struct InFlightGuard {
    inner: Arc<PosterCacheInner>,
    url: String,
    notify: Arc<Notify>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        {
            let mut entries = self.inner.entries.write();
            if matches!(entries.get(&self.url), Some(CacheEntry::Loading)) {
                entries.remove(&self.url);
            }
        }
        {
            let mut inflight = self.inner.in_flight_urls.lock();
            if inflight
                .get(&self.url)
                .is_some_and(|cur| Arc::ptr_eq(cur, &self.notify))
            {
                inflight.remove(&self.url);
            }
        }
        self.notify.notify_waiters();
    }
}

struct PruneFlagGuard {
    inner: Arc<PosterCacheInner>,
}

impl Drop for PruneFlagGuard {
    fn drop(&mut self) {
        self.inner
            .disk_prune_in_flight
            .store(false, Ordering::Release);
    }
}

fn trigger_background_disk_prune(inner: &Arc<PosterCacheInner>) {
    if inner.disk_prune_in_flight.swap(true, Ordering::AcqRel) {
        return;
    }
    let guard = PruneFlagGuard {
        inner: Arc::clone(inner),
    };
    let root = inner.cache_root.clone();
    inner.runtime.spawn(async move {
        let _guard = guard;
        let _ = tokio::task::spawn_blocking(move || {
            prune_disk_cache_to_budget(&root, MAX_DISK_CACHE_BYTES)
        })
        .await;
    });
}

fn mark_entry_ready(inner: &PosterCacheInner, url: &str, path: PathBuf) {
    inner
        .entries
        .write()
        .insert(url.to_owned(), CacheEntry::Ready(path));
}

fn mark_entry_failed(inner: &PosterCacheInner, url: &str) {
    let mut entries = inner.entries.write();
    entries.insert(url.to_owned(), CacheEntry::Failed);
    let mut failed_queue = inner.failed_entry_urls.lock();
    failed_queue.push_back(url.to_owned());
    while failed_queue.len() > MAX_FAILED_ENTRIES {
        if let Some(oldest) = failed_queue.pop_front() {
            if oldest != url && matches!(entries.get(&oldest), Some(CacheEntry::Failed)) {
                entries.remove(&oldest);
            }
        }
    }
}

fn remove_stale_ready_entry(inner: &PosterCacheInner, url: &str, stale_path: &Path) {
    let mut entries = inner.entries.write();
    if matches!(entries.get(url), Some(CacheEntry::Ready(p)) if p == stale_path) {
        entries.remove(url);
    }
}

enum DiskOrBytes {
    ReadyPath(PathBuf),
    Downloaded(Vec<u8>),
}

async fn acquire_raw_artwork(
    inner: &Arc<PosterCacheInner>,
    fetch_url: &str,
    force_disk_recheck: bool,
) -> Option<DiskOrBytes> {
    if !force_disk_recheck {
        if let Some(entry) = inner.entries.read().get(fetch_url).cloned() {
            match entry {
                CacheEntry::Ready(path) => return Some(DiskOrBytes::ReadyPath(path)),
                CacheEntry::Failed => return None,
                CacheEntry::Loading => {}
            }
        }
    }

    loop {
        let target_path = cache_path_for_url(&inner.cache_root, fetch_url);

        let (notify, is_leader) = {
            let mut inflight = inner.in_flight_urls.lock();
            if let Some(existing) = inflight.get(fetch_url) {
                (Arc::clone(existing), false)
            } else {
                let created = Arc::new(Notify::new());
                inflight.insert(fetch_url.to_owned(), Arc::clone(&created));
                (created, true)
            }
        };

        if !is_leader {
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let still_in_flight = {
                let inflight = inner.in_flight_urls.lock();
                inflight
                    .get(fetch_url)
                    .is_some_and(|cur| Arc::ptr_eq(cur, &notify))
            };
            if still_in_flight {
                notified.await;
            }
            match inner.entries.read().get(fetch_url).cloned() {
                Some(CacheEntry::Ready(path)) => return Some(DiskOrBytes::ReadyPath(path)),
                Some(CacheEntry::Failed) => return None,
                Some(CacheEntry::Loading) | None => continue,
            }
        }

        let _in_flight_guard = InFlightGuard {
            inner: Arc::clone(inner),
            url: fetch_url.to_owned(),
            notify,
        };

        if !force_disk_recheck {
            if let Some(entry) = inner.entries.read().get(fetch_url).cloned() {
                match entry {
                    CacheEntry::Ready(path) => return Some(DiskOrBytes::ReadyPath(path)),
                    CacheEntry::Failed => return None,
                    CacheEntry::Loading => {}
                }
            }
        }

        if let Ok(meta) = tokio::fs::metadata(&target_path).await {
            if meta.is_file() && meta.len() >= 64 {
                mark_entry_ready(inner, fetch_url, target_path.clone());
                return Some(DiskOrBytes::ReadyPath(target_path));
            }
        }

        let Ok(_download_permit) = inner.semaphore.acquire().await else {
            mark_entry_failed(inner, fetch_url);
            return None;
        };

        if let Ok(meta) = tokio::fs::metadata(&target_path).await {
            if meta.is_file() && meta.len() >= 64 {
                mark_entry_ready(inner, fetch_url, target_path.clone());
                return Some(DiskOrBytes::ReadyPath(target_path));
            }
        }

        match download_and_persist(inner, fetch_url, &target_path).await {
            Ok(bytes) => {
                mark_entry_ready(inner, fetch_url, target_path);
                return Some(DiskOrBytes::Downloaded(bytes));
            }
            Err(err) => {
                let safe_url = crate::updater::redact_url_for_log(fetch_url);
                tracing::debug!(
                    target: "ferrite::poster_cache",
                    url = %safe_url,
                    "artwork fetch failed: {err}"
                );
                mark_entry_failed(inner, fetch_url);
                return None;
            }
        }
    }
}

async fn read_and_decode_with_retry<F>(
    inner: &Arc<PosterCacheInner>,
    fetch_url: &str,
    first_source: DiskOrBytes,
    decode_fn: F,
) -> Option<Arc<RenderImage>>
where
    F: Fn(&[u8]) -> Option<Arc<RenderImage>> + Send + Sync + Copy + 'static,
{
    let mut source = first_source;
    for attempt in 0..2 {
        match source {
            DiskOrBytes::Downloaded(bytes) => {
                let decode_permit = inner.decode_semaphore.acquire().await.ok()?;
                let decoded = tokio::task::spawn_blocking(move || decode_fn(&bytes))
                    .await
                    .ok()
                    .flatten();
                drop(decode_permit);
                return decoded;
            }
            DiskOrBytes::ReadyPath(path) => {
                let decode_permit = inner.decode_semaphore.acquire().await.ok()?;
                let disk_permit = inner.disk_io_semaphore.acquire().await.ok()?;
                let read_res = tokio::fs::read(&path).await;
                drop(disk_permit);

                match read_res {
                    Ok(bytes) if bytes.len() >= 64 => {
                        let decoded = tokio::task::spawn_blocking(move || decode_fn(&bytes))
                            .await
                            .ok()
                            .flatten();
                        drop(decode_permit);
                        return decoded;
                    }
                    _ => {
                        drop(decode_permit);
                        if attempt == 0 {
                            remove_stale_ready_entry(inner, fetch_url, &path);
                            source = acquire_raw_artwork(inner, fetch_url, true).await?;
                        } else {
                            return None;
                        }
                    }
                }
            }
        }
    }
    None
}

async fn try_read_sidecar_image(
    inner: &PosterCacheInner,
    sidecar_path: &Path,
    max_w: u32,
    max_h: u32,
) -> Option<Arc<RenderImage>> {
    let disk_permit = inner.disk_io_semaphore.acquire().await.ok()?;
    let bytes = tokio::fs::read(sidecar_path).await.ok()?;
    drop(disk_permit);
    if let Some(img) = decode_sidecar_render_image(&bytes, max_w, max_h) {
        return Some(img);
    }
    let _ = tokio::fs::remove_file(sidecar_path).await;
    None
}

async fn persist_sidecar_if_enabled(
    inner: &Arc<PosterCacheInner>,
    fetch_url: &str,
    variant: ImageVariant,
    image: &RenderImage,
) {
    if !variant.uses_sidecar() {
        return;
    }
    let Some(bytes) = encode_sidecar_bytes(image) else {
        return;
    };
    let sidecar_path = sidecar_path_for_url(&inner.cache_root, fetch_url, variant.as_str());
    let _ = write_atomic_bytes(inner, &sidecar_path, &bytes).await;
}

fn insert_ready_and_wake(
    inner: &PosterCacheInner,
    target: LruTarget,
    variant: ImageVariant,
    original_url: &str,
    render_img: Arc<RenderImage>,
) {
    let ep = inner.current_epoch();
    let evicted = inner
        .lru_mutex(target)
        .lock()
        .insert_ready_variant(variant, original_url, render_img, ep);
    if !evicted.is_empty() {
        inner.evicted_images.lock().extend(evicted);
    }
    let _ = inner.wakeup_tx.try_send(());
}

fn mark_failed_and_wake(
    inner: &PosterCacheInner,
    target: LruTarget,
    variant: ImageVariant,
    original_url: &str,
) {
    let ep = inner.current_epoch();
    inner
        .lru_mutex(target)
        .lock()
        .mark_failed_variant(variant, original_url, ep);
    let _ = inner.wakeup_tx.try_send(());
}

async fn fetch_and_decode_into_target(
    inner: Arc<PosterCacheInner>,
    original_url: String,
    spec: DecodeTargetSpec,
) {
    let fetch_url = rewrite_cdn_image_url(&original_url, spec.cdn_max_w);
    let fetch_url_str: &str = fetch_url.as_ref();

    if spec.variant.uses_sidecar() {
        let sidecar_path =
            sidecar_path_for_url(&inner.cache_root, fetch_url_str, spec.variant.as_str());
        if let Some(render_img) =
            try_read_sidecar_image(&inner, &sidecar_path, spec.max_w, spec.max_h).await
        {
            insert_ready_and_wake(&inner, spec.target, spec.variant, &original_url, render_img);
            return;
        }
    }

    let Some(source) = acquire_raw_artwork(&inner, fetch_url_str, false).await else {
        mark_failed_and_wake(&inner, spec.target, spec.variant, &original_url);
        return;
    };

    let max_w = spec.max_w;
    let max_h = spec.max_h;
    let decoded = read_and_decode_with_retry(&inner, fetch_url_str, source, move |bytes| {
        decode_scaled_render_image(bytes, max_w, max_h)
    })
    .await;

    match decoded {
        Some(render_img) => {
            persist_sidecar_if_enabled(&inner, fetch_url_str, spec.variant, &render_img).await;
            insert_ready_and_wake(&inner, spec.target, spec.variant, &original_url, render_img);
        }
        None => {
            mark_failed_and_wake(&inner, spec.target, spec.variant, &original_url);
        }
    }
}

async fn fetch_or_load_disk(inner: Arc<PosterCacheInner>, url: String) -> Option<PathBuf> {
    match acquire_raw_artwork(&inner, &url, false).await {
        Some(DiskOrBytes::ReadyPath(path)) => {
            let _ = inner.wakeup_tx.try_send(());
            Some(path)
        }
        Some(DiskOrBytes::Downloaded(_)) => {
            let path = cache_path_for_url(&inner.cache_root, &url);
            let _ = inner.wakeup_tx.try_send(());
            Some(path)
        }
        None => None,
    }
}

async fn compute_or_fetch_ambient(
    inner: Arc<PosterCacheInner>,
    original_url: String,
    cdn_max_w: u32,
) {
    let fetch_url = rewrite_cdn_image_url(&original_url, cdn_max_w);
    let fetch_url_str: &str = fetch_url.as_ref();

    let sidecar_path = sidecar_path_for_url(
        &inner.cache_root,
        fetch_url_str,
        ImageVariant::Ambient.as_str(),
    );
    if let Some(render_img) = try_read_sidecar_image(
        &inner,
        &sidecar_path,
        AMBIENT_BLEED_WIDTH,
        AMBIENT_BLEED_HEIGHT,
    )
    .await
    {
        insert_ready_and_wake(
            &inner,
            LruTarget::Ambient,
            ImageVariant::Ambient,
            &original_url,
            render_img,
        );
        return;
    }

    let Some(source) = acquire_raw_artwork(&inner, fetch_url_str, false).await else {
        mark_failed_and_wake(
            &inner,
            LruTarget::Ambient,
            ImageVariant::Ambient,
            &original_url,
        );
        return;
    };

    let built =
        read_and_decode_with_retry(&inner, fetch_url_str, source, build_ambient_bleed_image).await;

    match built {
        Some(render_img) => {
            persist_sidecar_if_enabled(&inner, fetch_url_str, ImageVariant::Ambient, &render_img)
                .await;
            insert_ready_and_wake(
                &inner,
                LruTarget::Ambient,
                ImageVariant::Ambient,
                &original_url,
                render_img,
            );
        }
        None => {
            mark_failed_and_wake(
                &inner,
                LruTarget::Ambient,
                ImageVariant::Ambient,
                &original_url,
            );
        }
    }
}

async fn ensure_dir_initialized(inner: &PosterCacheInner, dir: &Path) -> std::io::Result<()> {
    if inner.initialized_dirs.lock().contains(dir) {
        return Ok(());
    }
    tokio::fs::create_dir_all(dir).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let should_chmod_root = inner
            .initialized_dirs
            .lock()
            .insert(inner.cache_root.clone());
        if should_chmod_root {
            let _ = tokio::fs::set_permissions(
                &inner.cache_root,
                std::fs::Permissions::from_mode(0o700),
            )
            .await;
        }
        let _ = tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).await;
    }
    inner.initialized_dirs.lock().insert(dir.to_path_buf());
    Ok(())
}

async fn write_atomic_bytes(
    inner: &Arc<PosterCacheInner>,
    target_path: &Path,
    bytes: &[u8],
) -> anyhow::Result<()> {
    let Some(parent) = target_path.parent() else {
        anyhow::bail!("missing parent directory for {}", target_path.display());
    };
    ensure_dir_initialized(inner, parent).await?;

    let seq = inner.temp_seq.fetch_add(1, Ordering::Relaxed);
    let tmp_path = target_path.with_extension(format!("part.{seq}"));
    {
        let disk_permit = inner
            .disk_io_semaphore
            .acquire()
            .await
            .map_err(|_| anyhow::anyhow!("disk semaphore closed"))?;

        if let Err(err) = tokio::fs::write(&tmp_path, bytes).await {
            if err.kind() == std::io::ErrorKind::NotFound {
                {
                    inner.initialized_dirs.lock().remove(parent);
                }
                ensure_dir_initialized(inner, parent).await?;
                tokio::fs::write(&tmp_path, bytes).await?;
            } else {
                return Err(err.into());
            }
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = tokio::fs::set_permissions(
                &tmp_path,
                std::fs::Permissions::from_mode(0o600),
            )
            .await;
        }

        if let Err(err) = tokio::fs::rename(&tmp_path, target_path).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(err.into());
        }
        drop(disk_permit);
    }

    let writes = inner.disk_writes.fetch_add(1, Ordering::Relaxed) + 1;
    if writes.is_multiple_of(DISK_PRUNE_INTERVAL_WRITES) {
        trigger_background_disk_prune(inner);
    }
    Ok(())
}

async fn download_and_persist(
    inner: &Arc<PosterCacheInner>,
    url: &str,
    target_path: &Path,
) -> anyhow::Result<Vec<u8>> {
    let resp = inner
        .client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.without_url()))?
        .error_for_status()
        .map_err(|e| anyhow::anyhow!("{}", e.without_url()))?;
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.without_url()))?;
    if bytes.len() < 64 || bytes.len() > MAX_POSTER_BYTES {
        anyhow::bail!("invalid image byte length {}", bytes.len());
    }

    write_atomic_bytes(inner, target_path, &bytes).await?;
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn sample_png_bytes(w: u32, h: u32) -> Vec<u8> {
        let mut img = RgbaImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([(x * 3) as u8, (y * 5) as u8, 200, 255]);
        }
        let mut png_bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        png_bytes
    }

    #[test]
    fn deterministic_sharded_cache_paths() {
        let root = Path::new("/tmp/ferrite-posters");
        let p1 = cache_path_for_url(
            root,
            "https://images.metahub.space/poster/medium/tt0133093/img",
        );
        let p2 = cache_path_for_url(
            root,
            "https://images.metahub.space/poster/medium/tt0133093/img",
        );
        let png = cache_path_for_url(root, "https://example.com/logo.PNG?size=large");
        assert_eq!(p1, p2);
        assert_eq!(p1.extension().and_then(|e| e.to_str()), Some("jpg"));
        assert_eq!(png.extension().and_then(|e| e.to_str()), Some("png"));
        assert!(p1.starts_with(root));
    }

    #[test]
    fn builds_48x27_box_blurred_bgra_ambient_bleed_image() {
        let png_bytes = sample_png_bytes(96, 54);
        let ambient = build_ambient_bleed_image(&png_bytes).expect("ambient bleed image");
        let raw = ambient.as_bytes(0).expect("frame 0 bytes");
        assert_eq!(
            raw.len(),
            (AMBIENT_BLEED_WIDTH * AMBIENT_BLEED_HEIGHT * 4) as usize
        );
        assert!(raw.as_chunks::<4>().0.iter().all(|c| c[3] == 255));
    }

    #[test]
    fn bounded_texture_lru_evicts_least_recently_used_entry_and_returns_handles() {
        let png_bytes = sample_png_bytes(8, 8);
        let img1 = decode_scaled_render_image(&png_bytes, 64, 64).expect("decode png 1");
        let img2 = decode_scaled_render_image(&png_bytes, 64, 64).expect("decode png 2");
        let img3 = decode_scaled_render_image(&png_bytes, 64, 64).expect("decode png 3");

        let mut lru = BoundedTextureLru::with_capacity(2);
        assert!(lru.insert_ready("k1".to_owned(), Arc::clone(&img1), 1).is_empty());
        assert!(lru.insert_ready("k2".to_owned(), Arc::clone(&img2), 1).is_empty());
        assert!(lru.lookup_or_begin("k1", 4).0.is_some());
        let evicted = lru.insert_ready("k3".to_owned(), Arc::clone(&img3), 4);
        assert_eq!(evicted.len(), 1);
        assert!(Arc::ptr_eq(&evicted[0], &img2));

        assert_eq!(lru.len_ready(), 2);
        assert!(lru.lookup_or_begin("k1", 4).0.is_some());
        assert!(lru.lookup_or_begin("k3", 4).0.is_some());
        assert!(lru.lookup_or_begin("k2", 4).1);

        let drained = lru.drain_all();
        assert_eq!(drained.len(), 2);
        assert_eq!(lru.len_ready(), 0);
    }

    #[test]
    fn active_epoch_working_set_above_capacity_never_thrashes_and_sweeps_when_stale() {
        let png_bytes = sample_png_bytes(8, 12);
        let capacity = 64usize;
        let working_set = 90usize;
        let mut lru = BoundedTextureLru::with_capacity(capacity);

        let epoch_a = 10u64;
        for i in 0..working_set {
            let key = format!("poster:http://example/{i}.jpg");
            let (hit, should_spawn) = lru.lookup_or_begin(&key, epoch_a);
            assert!(hit.is_none());
            assert!(should_spawn);
        }

        for i in 0..working_set {
            let key = format!("poster:http://example/{i}.jpg");
            let tex = decode_scaled_render_image(&png_bytes, 32, 48).expect("decode tex");
            let evicted = lru.insert_ready(key, tex, epoch_a);
            assert!(evicted.is_empty());
        }
        assert_eq!(lru.len_ready(), working_set);

        let epoch_b = epoch_a + 1;
        assert!(lru.sweep_to_capacity(epoch_b).is_empty());
        for i in 0..working_set {
            let key = format!("poster:http://example/{i}.jpg");
            let (hit, should_spawn) = lru.lookup_or_begin(&key, epoch_b);
            assert!(hit.is_some());
            assert!(!should_spawn);
        }

        let epoch_stale = epoch_b + 3;
        for i in 0..10 {
            let key = format!("poster:http://example/{i}.jpg");
            assert!(lru.lookup_or_begin(&key, epoch_stale).0.is_some());
        }

        let swept = lru.sweep_to_capacity(epoch_stale);
        assert_eq!(swept.len(), working_set - capacity);
        assert_eq!(lru.len_ready(), capacity);

        for i in 0..10 {
            let key = format!("poster:http://example/{i}.jpg");
            let (hit, should_spawn) = lru.lookup_or_begin(&key, epoch_stale);
            assert!(hit.is_some());
            assert!(!should_spawn);
        }
    }

    #[tokio::test]
    async fn poster_cache_eviction_queue_drains_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PosterCache::new(dir.path().to_path_buf(), tokio::runtime::Handle::current());
        assert!(cache.drain_evicted_images().is_empty());

        let png_bytes = sample_png_bytes(4, 4);
        let tex = decode_scaled_render_image(&png_bytes, 32, 32).unwrap();
        cache.push_evicted_image(Arc::clone(&tex));
        let drained = cache.drain_evicted_images();
        assert_eq!(drained.len(), 1);
        assert!(Arc::ptr_eq(&drained[0], &tex));
        assert!(cache.drain_evicted_images().is_empty());

        let ep = cache.begin_render_frame();
        for i in 0..24 {
            let t = decode_scaled_render_image(&png_bytes, 32, 32).unwrap();
            assert!(cache
                .inner
                .decoded_lru
                .lock()
                .insert_ready(format!("poster:http://example/{i}.jpg"), t, ep)
                .is_empty());
        }
        for i in 0..12 {
            let th = decode_scaled_render_image(&png_bytes, 16, 24).unwrap();
            assert!(cache
                .inner
                .thumb_lru
                .lock()
                .insert_ready(format!("thumb:http://example/{i}.jpg"), th, ep)
                .is_empty());
        }
        for i in 0..4 {
            let a = decode_scaled_render_image(&png_bytes, 32, 32).unwrap();
            assert!(cache
                .inner
                .ambient_lru
                .lock()
                .insert_ready(format!("http://example/amb-{i}.jpg"), a, ep)
                .is_empty());
        }
        assert_eq!(cache.decoded_ready_count(), 24);
        assert_eq!(cache.thumb_ready_count(), 12);
        cache.trim_for_playback();
        assert_eq!(cache.decoded_ready_count(), PLAYBACK_RETAINED_DECODED_TEXTURES);
        assert_eq!(cache.thumb_ready_count(), PLAYBACK_RETAINED_THUMB_TEXTURES);
        let playback_evicted = cache.drain_evicted_images();
        assert_eq!(playback_evicted.len(), 14);
        assert!(cache.drain_evicted_images().is_empty());
    }

    #[tokio::test]
    async fn fetches_persists_and_deduplicates_artwork() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicU64::new(0));
        let hits_srv = Arc::clone(&hits);

        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                hits_srv.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0_u8; 1024];
                let _ = sock.read(&mut buf).await;
                let body = vec![0xFF_u8; 256];
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(header.as_bytes()).await;
                let _ = sock.write_all(&body).await;
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let cache = PosterCache::new(dir.path().to_path_buf(), tokio::runtime::Handle::current());
        let rx = cache.wakeup_receiver();
        let url = format!("http://{addr}/poster.jpg");

        assert!(cache.resolve_or_fetch(&url).is_none());
        assert!(cache.resolve_or_fetch(&url).is_none());

        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("wakeup timed out")
            .expect("wakeup channel closed");

        let resolved = cache
            .resolve_or_fetch(&url)
            .expect("poster should now resolve synchronously");
        assert!(resolved.is_file());
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        let cache2 = PosterCache::new(dir.path().to_path_buf(), tokio::runtime::Handle::current());
        let rx2 = cache2.wakeup_receiver();
        assert!(cache2.resolve_or_fetch(&url).is_none());
        tokio::time::timeout(Duration::from_secs(2), rx2.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cache2.resolve_or_fetch(&url), Some(resolved));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn logo_decoder_preserves_transparent_png_alpha_and_fast_paths_opaque_pixels() {
        let mut rgba = RgbaImage::new(4, 1);
        rgba.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        rgba.put_pixel(1, 0, image::Rgba([255, 255, 255, 255]));
        rgba.put_pixel(2, 0, image::Rgba([40, 120, 220, 255]));
        rgba.put_pixel(3, 0, image::Rgba([200, 100, 50, 128]));
        let mut png_bytes = Vec::new();
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png)
            .unwrap();

        let rendered = decode_scaled_render_image(
            &png_bytes,
            LOGO_DECODE_MAX_WIDTH,
            LOGO_DECODE_MAX_HEIGHT,
        )
        .expect("PNG logo decoded");
        let bytes = rendered.as_bytes(0).expect("frame bytes");
        assert_eq!(&bytes[0..4], &[0, 0, 0, 0]);
        assert_eq!(&bytes[4..8], &[255, 255, 255, 255]);
        assert_eq!(&bytes[8..12], &[220, 120, 40, 255]);
        // Premultiplied BGRA for [200, 100, 50, 128]
        assert_eq!(bytes[12], ((50_u16 * 128 + 127) / 255) as u8);
        assert_eq!(bytes[13], ((100_u16 * 128 + 127) / 255) as u8);
        assert_eq!(bytes[14], ((200_u16 * 128 + 127) / 255) as u8);
        assert_eq!(bytes[15], 128);
    }

    #[test]
    fn cdn_url_rewriting_steps_down_tmdb_and_metahub_preserving_query_and_fragment() {
        // TMDB /original/ steps down according to max_w
        assert_eq!(
            rewrite_cdn_image_url("https://image.tmdb.org/t/p/original/abc.jpg?v=2#frag", 80),
            "https://image.tmdb.org/t/p/w185/abc.jpg?v=2#frag"
        );
        assert_eq!(
            rewrite_cdn_image_url("https://image.tmdb.org/t/p/original/still.jpg", STILL_MAX_W),
            "https://image.tmdb.org/t/p/w300/still.jpg"
        );
        assert_eq!(
            rewrite_cdn_image_url("https://image.tmdb.org/t/p/original/poster.jpg", 342),
            "https://image.tmdb.org/t/p/w342/poster.jpg"
        );
        assert_eq!(
            rewrite_cdn_image_url("https://image.tmdb.org/t/p/w500/poster.jpg", 500),
            "https://image.tmdb.org/t/p/w342/poster.jpg"
        );
        assert_eq!(
            rewrite_cdn_image_url("https://image.tmdb.org/t/p/original/wide.jpg", WIDESCREEN_MAX_W),
            "https://image.tmdb.org/t/p/w780/wide.jpg"
        );
        assert_eq!(
            rewrite_cdn_image_url("https://image.tmdb.org/t/p/original/hero.jpg", HERO_MAX_W),
            "https://image.tmdb.org/t/p/w1280/hero.jpg"
        );

        // Already <= target size: borrowed without allocation
        let borrowed_small = rewrite_cdn_image_url("https://image.tmdb.org/t/p/w92/p.jpg", 185);
        assert!(matches!(borrowed_small, Cow::Borrowed(_)));
        assert_eq!(borrowed_small, "https://image.tmdb.org/t/p/w92/p.jpg");

        let borrowed_w342 = rewrite_cdn_image_url("https://image.tmdb.org/t/p/w342/p.jpg", 500);
        assert!(matches!(borrowed_w342, Cow::Borrowed(_)));

        let borrowed_w780 = rewrite_cdn_image_url("https://image.tmdb.org/t/p/w780/b.jpg", 1280);
        assert!(matches!(borrowed_w780, Cow::Borrowed(_)));

        // Metahub /large/ steps down to /medium/ or /small/ (<= 120)
        assert_eq!(
            rewrite_cdn_image_url(
                "https://images.metahub.space/poster/large/tt0133093/img?t=1#top",
                80
            ),
            "https://images.metahub.space/poster/small/tt0133093/img?t=1#top"
        );
        assert_eq!(
            rewrite_cdn_image_url(
                "https://images.metahub.space/background/large/tt0133093/img",
                342
            ),
            "https://images.metahub.space/background/medium/tt0133093/img"
        );
        let metahub_med =
            rewrite_cdn_image_url("https://images.metahub.space/poster/medium/tt0133093/img", 80);
        assert!(matches!(metahub_med, Cow::Borrowed(_)));

        // Non-CDN URLs untouched
        let custom = rewrite_cdn_image_url("http://127.0.0.1:8080/poster/large/a.jpg", 80);
        assert!(matches!(custom, Cow::Borrowed(_)));
    }

    #[test]
    fn bounded_texture_lru_variant_separation_and_failed_cap() {
        let png_bytes = sample_png_bytes(8, 8);
        let poster_img = decode_scaled_render_image(&png_bytes, 32, 48).unwrap();
        let wide_img = decode_scaled_render_image(&png_bytes, 64, 36).unwrap();

        let mut lru = BoundedTextureLru::with_capacity(8);
        let shared_url = "https://example.com/art.jpg";
        assert!(lru
            .insert_ready_variant(ImageVariant::Poster, shared_url, Arc::clone(&poster_img), 1)
            .is_empty());
        assert!(lru
            .insert_ready_variant(ImageVariant::Wide, shared_url, Arc::clone(&wide_img), 1)
            .is_empty());
        assert_eq!(lru.len_ready(), 2);

        let (got_poster, out_p) = lru.lookup_or_begin_variant(ImageVariant::Poster, shared_url, 2);
        let (got_wide, out_w) = lru.lookup_or_begin_variant(ImageVariant::Wide, shared_url, 2);
        assert_eq!(out_p, LookupOutcome::Ready);
        assert_eq!(out_w, LookupOutcome::Ready);
        assert!(Arc::ptr_eq(&got_poster.unwrap(), &poster_img));
        assert!(Arc::ptr_eq(&got_wide.unwrap(), &wide_img));

        // Failure map cap evicts oldest failures above MAX_FAILED_ENTRIES (256)
        for i in 0..300 {
            let bad_url = format!("https://example.com/missing-{i}.jpg");
            lru.mark_failed_variant(ImageVariant::Poster, &bad_url, 3);
        }
        assert_eq!(lru.failed_count, MAX_FAILED_ENTRIES);
        // Oldest entries (0..44) were evicted and now return Spawned again
        let (_, oldest_outcome) =
            lru.lookup_or_begin_variant(ImageVariant::Poster, "https://example.com/missing-0.jpg", 3);
        assert_eq!(oldest_outcome, LookupOutcome::Spawned);
        // Newest entry (299) is still marked Failed
        let (_, newest_outcome) = lru.lookup_or_begin_variant(
            ImageVariant::Poster,
            "https://example.com/missing-299.jpg",
            3,
        );
        assert_eq!(newest_outcome, LookupOutcome::Failed);

        lru.clear_failures();
        assert_eq!(lru.failed_count, 0);
    }

    #[tokio::test]
    async fn episode_still_target_resolves_and_trims_for_playback() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let still_png = sample_png_bytes(640, 360);

        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0_u8; 1024];
                let _ = sock.read(&mut buf).await;
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    still_png.len()
                );
                let _ = sock.write_all(header.as_bytes()).await;
                let _ = sock.write_all(&still_png).await;
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let cache = PosterCache::new(dir.path().to_path_buf(), tokio::runtime::Handle::current());
        let rx = cache.wakeup_receiver();
        let url = Url::parse(&format!("http://{addr}/ep1.png")).unwrap();

        assert!(cache.resolve_episode_still_url(Some(&url)).is_none());
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();

        let still_tex = cache
            .resolve_episode_still_url(Some(&url))
            .expect("episode still should resolve");
        let sz = still_tex.size(0);
        assert_eq!(sz.width.0, STILL_MAX_W as i32);
        assert_eq!(sz.height.0, STILL_MAX_H as i32);
        assert_eq!(cache.still_ready_count(), 1);
        assert_eq!(cache.decoded_ready_count(), 0);

        // Verify playback trim retains PLAYBACK_RETAINED_STILL_TEXTURES
        let ep = cache.begin_render_frame();
        let tiny = sample_png_bytes(16, 9);
        for i in 0..16 {
            let img = decode_scaled_render_image(&tiny, STILL_MAX_W, STILL_MAX_H).unwrap();
            cache.inner.still_lru.lock().insert_ready_variant(
                ImageVariant::Still,
                &format!("http://example/still-{i}.png"),
                img,
                ep,
            );
        }
        assert_eq!(cache.still_ready_count(), 17);
        cache.trim_for_playback();
        assert_eq!(cache.still_ready_count(), PLAYBACK_RETAINED_STILL_TEXTURES);
    }

    #[tokio::test]
    async fn fallback_resolvers_advance_only_on_failure_without_eager_cascade() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requested_paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let req_log = Arc::clone(&requested_paths);
        let good_png = sample_png_bytes(64, 36);

        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0_u8; 2048];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req_str = String::from_utf8_lossy(&buf[..n]);
                let path = req_str
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_owned();
                req_log.lock().push(path.clone());

                if path.contains("fail") {
                    let _ = sock
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                } else {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        good_png.len()
                    );
                    let _ = sock.write_all(header.as_bytes()).await;
                    let _ = sock.write_all(&good_png).await;
                }
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let cache = PosterCache::new(dir.path().to_path_buf(), tokio::runtime::Handle::current());
        let rx = cache.wakeup_receiver();

        let u1 = format!("http://{addr}/hero-fail.png");
        let u2 = format!("http://{addr}/hero-ok.png");
        let u3 = format!("http://{addr}/hero-never.png");

        // First call spawns ONLY u1 (not u2 or u3)
        assert!(cache
            .resolve_first_available_hero(["", u1.as_str(), u2.as_str(), u3.as_str()])
            .is_none());
        // Calling again while Loading must still NOT spawn u2 or u3
        assert!(cache
            .resolve_first_available_hero([u1.as_str(), u2.as_str(), u3.as_str()])
            .is_none());

        // Wakes up when u1 fails
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("failure wakeup timed out")
            .unwrap();
        assert_eq!(requested_paths.lock().as_slice(), &["/hero-fail.png"]);

        // Second call advances past Failed u1 and spawns ONLY u2 (still NOT u3)
        assert!(cache
            .resolve_first_available_hero([u1.as_str(), u2.as_str(), u3.as_str()])
            .is_none());
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("success wakeup timed out")
            .unwrap();

        let resolved = cache
            .resolve_first_available_hero([u1.as_str(), u2.as_str(), u3.as_str()])
            .expect("hero fallback u2 should now be ready");
        assert!(resolved.size(0).width.0 > 0);
        assert_eq!(
            requested_paths.lock().as_slice(),
            &["/hero-fail.png", "/hero-ok.png"]
        );

        // Also test resolve_widescreen_or_poster advancing only on failure
        let w_fail = format!("http://{addr}/wide-fail.png");
        let p_ok = format!("http://{addr}/poster-ok.png");
        assert!(cache
            .resolve_widescreen_or_poster(Some(&w_fail), Some(&p_ok))
            .is_none());
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(cache
            .resolve_widescreen_or_poster(Some(&w_fail), Some(&p_ok))
            .is_none());
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(cache
            .resolve_widescreen_or_poster(Some(&w_fail), Some(&p_ok))
            .is_some());

        // Also test resolve_first_available_ambient & resolve_first_available_logo
        let amb_fail = format!("http://{addr}/amb-fail.png");
        let amb_ok = format!("http://{addr}/amb-ok.png");
        assert!(cache
            .resolve_first_available_ambient([amb_fail.as_str(), amb_ok.as_str()])
            .is_none());
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(cache
            .resolve_first_available_ambient([amb_fail.as_str(), amb_ok.as_str()])
            .is_none());
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(cache
            .resolve_first_available_ambient([amb_fail.as_str(), amb_ok.as_str()])
            .is_some());

        let logo_fail = format!("http://{addr}/logo-fail.png");
        let logo_ok = format!("http://{addr}/logo-ok.png");
        assert!(cache
            .resolve_first_available_logo([logo_fail.as_str(), logo_ok.as_str()])
            .is_none());
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(cache
            .resolve_first_available_logo([logo_fail.as_str(), logo_ok.as_str()])
            .is_none());
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(cache
            .resolve_first_available_logo([logo_fail.as_str(), logo_ok.as_str()])
            .is_some());
    }

    #[tokio::test]
    async fn hero_and_ambient_coalesce_concurrent_same_url_into_single_http_get() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicU64::new(0));
        let hits_srv = Arc::clone(&hits);
        let png = sample_png_bytes(160, 90);

        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                hits_srv.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0_u8; 1024];
                let _ = sock.read(&mut buf).await;
                tokio::time::sleep(Duration::from_millis(40)).await;
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    png.len()
                );
                let _ = sock.write_all(header.as_bytes()).await;
                let _ = sock.write_all(&png).await;
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let cache = PosterCache::new(dir.path().to_path_buf(), tokio::runtime::Handle::current());
        let rx = cache.wakeup_receiver();
        let url = format!("http://{addr}/shared-backdrop.png");

        assert!(cache.resolve_hero_backdrop_image(&url).is_none());
        assert!(cache.resolve_ambient_bleed(&url).is_none());

        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            if cache.resolve_hero_backdrop_image(&url).is_some()
                && cache.resolve_ambient_bleed(&url).is_some()
            {
                break;
            }
            tokio::time::timeout_at(deadline, rx.recv())
                .await
                .expect("timed out waiting for hero + ambient")
                .unwrap();
        }

        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn prescaled_sidecar_roundtrip_serves_warm_launch_without_raw_file() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicU64::new(0));
        let hits_srv = Arc::clone(&hits);
        let poster_png = sample_png_bytes(400, 600);

        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                hits_srv.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0_u8; 1024];
                let _ = sock.read(&mut buf).await;
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    poster_png.len()
                );
                let _ = sock.write_all(header.as_bytes()).await;
                let _ = sock.write_all(&poster_png).await;
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let cache1 = PosterCache::new(dir.path().to_path_buf(), tokio::runtime::Handle::current());
        let rx1 = cache1.wakeup_receiver();
        let url = format!("http://{addr}/poster-sidecar.png");

        assert!(cache1.resolve_poster_image(&url).is_none());
        tokio::time::timeout(Duration::from_secs(2), rx1.recv())
            .await
            .unwrap()
            .unwrap();
        let img1 = cache1
            .resolve_poster_image(&url)
            .expect("poster decoded on cold run");
        assert_eq!(img1.size(0).width.0, POSTER_MAX_W as i32);
        assert_eq!(img1.size(0).height.0, POSTER_MAX_H as i32);

        let raw_path = cache_path_for_url(dir.path(), &url);
        let sidecar_path = sidecar_path_for_url(dir.path(), &url, "poster");
        assert!(raw_path.is_file());
        assert!(sidecar_path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&sidecar_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        // Remove raw compressed file to prove warm launch reads purely from pre-scaled sidecar!
        std::fs::remove_file(&raw_path).unwrap();

        let cache2 = PosterCache::new(dir.path().to_path_buf(), tokio::runtime::Handle::current());
        let rx2 = cache2.wakeup_receiver();
        assert!(cache2.resolve_poster_image(&url).is_none());
        tokio::time::timeout(Duration::from_secs(2), rx2.recv())
            .await
            .unwrap()
            .unwrap();
        let img2 = cache2
            .resolve_poster_image(&url)
            .expect("poster loaded from pre-scaled sidecar");
        assert_eq!(img2.size(0), img1.size(0));
        assert_eq!(img2.as_bytes(0), img1.as_bytes(0));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn disk_budget_pruning_deletes_oldest_mtime_files_first() {
        let dir = tempfile::tempdir().unwrap();
        let shard_a = dir.path().join("aa");
        let shard_b = dir.path().join("bb");
        std::fs::create_dir_all(&shard_a).unwrap();
        std::fs::create_dir_all(&shard_b).unwrap();

        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let paths = [
            shard_a.join("oldest.jpg"),
            shard_b.join("second.jpg"),
            shard_a.join("third.poster.bin"),
            shard_b.join("newest.jpg"),
        ];

        for (i, path) in paths.iter().enumerate() {
            use std::io::Write;
            let mut file = std::fs::File::create(path).unwrap();
            file.write_all(&vec![0xAB_u8; 1024]).unwrap();
            file.set_modified(base + Duration::from_secs((i as u64 + 1) * 100))
                .unwrap();
        }

        let freed = prune_disk_cache_to_budget(dir.path(), 2048);
        assert_eq!(freed, 2048);
        assert!(!paths[0].exists());
        assert!(!paths[1].exists());
        assert!(paths[2].exists());
        assert!(paths[3].exists());
    }

    #[tokio::test]
    async fn corrupt_sidecar_regenerates_from_raw_and_prefetch_populates_targets() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicU64::new(0));
        let hits_srv = Arc::clone(&hits);
        let png = sample_png_bytes(160, 240);

        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                hits_srv.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0_u8; 1024];
                let _ = sock.read(&mut buf).await;
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    png.len()
                );
                let _ = sock.write_all(header.as_bytes()).await;
                let _ = sock.write_all(&png).await;
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let poster_url = format!("http://{addr}/prefetch-poster.png");
        let hero_url = format!("http://{addr}/prefetch-hero.png");

        let cache1 = PosterCache::new(dir.path().to_path_buf(), tokio::runtime::Handle::current());
        let rx1 = cache1.wakeup_receiver();
        cache1.prefetch_poster(&poster_url);
        cache1.prefetch_hero_backdrop(&hero_url);

        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            if cache1.resolve_poster_image(&poster_url).is_some()
                && cache1.resolve_hero_backdrop_image(&hero_url).is_some()
            {
                break;
            }
            tokio::time::timeout_at(deadline, rx1.recv())
                .await
                .expect("prefetch timed out")
                .unwrap();
        }
        assert_eq!(hits.load(Ordering::SeqCst), 2);

        // Corrupt the poster sidecar; warm cache2 should discard corrupt sidecar and regenerate from raw
        let sidecar_path = sidecar_path_for_url(dir.path(), &poster_url, "poster");
        std::fs::write(&sidecar_path, b"BAD_MAGIC_DATA").unwrap();

        let cache2 = PosterCache::new(dir.path().to_path_buf(), tokio::runtime::Handle::current());
        let rx2 = cache2.wakeup_receiver();
        assert!(cache2.resolve_poster_image(&poster_url).is_none());
        tokio::time::timeout(Duration::from_secs(2), rx2.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(cache2.resolve_poster_image(&poster_url).is_some());
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert!(decode_sidecar_render_image(
            &std::fs::read(&sidecar_path).unwrap(),
            POSTER_MAX_W,
            POSTER_MAX_H
        )
        .is_some());
    }
}
