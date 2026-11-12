use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use gpui::RenderImage;
use image::{imageops::FilterType, Frame, RgbaImage};
use parking_lot::{Mutex, RwLock};
use sha2::{Digest, Sha256};
use smallvec::smallvec;
use tokio::sync::Semaphore;
use url::Url;

const MAX_CONCURRENT_POSTER_DOWNLOADS: usize = 12;
pub const MAX_CONCURRENT_IMAGE_DECODES: usize = 4;
const HTTP_POSTER_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_POSTER_BYTES: usize = 16 * 1024 * 1024;

pub const MAX_DECODED_TEXTURES: usize = 64;
pub const MAX_AMBIENT_TEXTURES: usize = 16;
pub const MAX_HERO_TEXTURES: usize = 2;

pub const PLAYBACK_RETAINED_DECODED_TEXTURES: usize = 16;
pub const PLAYBACK_RETAINED_HERO_TEXTURES: usize = 2;
pub const PLAYBACK_RETAINED_AMBIENT_TEXTURES: usize = 2;

const POSTER_MAX_W: u32 = 300;
const POSTER_MAX_H: u32 = 450;
const WIDESCREEN_MAX_W: u32 = 640;
const WIDESCREEN_MAX_H: u32 = 360;
const HERO_MAX_W: u32 = 960;
const HERO_MAX_H: u32 = 540;

pub const AMBIENT_BLEED_WIDTH: u32 = 48;
pub const AMBIENT_BLEED_HEIGHT: u32 = 27;

#[derive(Clone, Debug, PartialEq, Eq)]
enum CacheEntry {
    Loading,
    Ready(PathBuf),
    Failed,
}

#[derive(Clone)]
enum TextureSlot {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

pub struct BoundedTextureLru {
    capacity: usize,
    map: HashMap<String, TextureSlot>,
    order: VecDeque<String>,
}

impl BoundedTextureLru {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            map: HashMap::with_capacity(capacity.saturating_add(16)),
            order: VecDeque::with_capacity(capacity.saturating_add(16)),
        }
    }

    pub fn len_ready(&self) -> usize {
        self.order.len()
    }

    fn touch_order(&mut self, key: &str) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.to_owned());
    }

    fn lookup_or_begin(&mut self, key: &str) -> (Option<Arc<RenderImage>>, bool) {
        if let Some(slot) = self.map.get(key).cloned() {
            return match slot {
                TextureSlot::Ready(img) => {
                    self.touch_order(key);
                    (Some(img), false)
                }
                TextureSlot::Loading | TextureSlot::Failed => (None, false),
            };
        }
        self.map.insert(key.to_owned(), TextureSlot::Loading);
        (None, true)
    }

    pub fn insert_ready(&mut self, key: String, image: Arc<RenderImage>) -> Vec<Arc<RenderImage>> {
        let mut evicted = Vec::new();
        self.touch_order(&key);
        if let Some(TextureSlot::Ready(prev)) = self.map.insert(key, TextureSlot::Ready(Arc::clone(&image))) {
            if !Arc::ptr_eq(&prev, &image) {
                evicted.push(prev);
            }
        }
        while self.order.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                if let Some(TextureSlot::Ready(old_img)) = self.map.remove(&oldest) {
                    if !Arc::ptr_eq(&old_img, &image) {
                        evicted.push(old_img);
                    }
                }
            }
        }
        evicted
    }

    pub fn trim_to_limit(&mut self, keep_mru: usize) -> Vec<Arc<RenderImage>> {
        let mut evicted = Vec::new();
        while self.order.len() > keep_mru {
            if let Some(oldest) = self.order.pop_front() {
                if let Some(TextureSlot::Ready(old_img)) = self.map.remove(&oldest) {
                    evicted.push(old_img);
                }
            }
        }
        evicted
    }

    pub fn drain_all(&mut self) -> Vec<Arc<RenderImage>> {
        self.order.clear();
        let mut out = Vec::new();
        for (_, slot) in self.map.drain() {
            if let TextureSlot::Ready(img) = slot {
                out.push(img);
            }
        }
        out
    }

    fn mark_failed(&mut self, key: String) {
        self.map.insert(key, TextureSlot::Failed);
    }

    fn clear_failures(&mut self) {
        self.map.retain(|_, v| !matches!(v, TextureSlot::Failed));
    }
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

pub fn cache_path_for_url(cache_root: &Path, url: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let digest = hex::encode(hasher.finalize());
    let shard = &digest[..2];
    let ext = infer_image_extension(url);
    cache_root.join(shard).join(format!("{digest}.{ext}"))
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
        let r = px_slice[0] as u16;
        let g = px_slice[1] as u16;
        let b = px_slice[2] as u16;
        let a = px_slice[3] as u16;
        px_slice[0] = ((b * a + 127) / 255) as u8;
        px_slice[1] = ((g * a + 127) / 255) as u8;
        px_slice[2] = ((r * a + 127) / 255) as u8;
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

struct PosterCacheInner {
    cache_root: PathBuf,
    entries: RwLock<HashMap<String, CacheEntry>>,
    decoded_lru: Mutex<BoundedTextureLru>,
    hero_lru: Mutex<BoundedTextureLru>,
    ambient_lru: Mutex<BoundedTextureLru>,
    evicted_images: Mutex<Vec<Arc<RenderImage>>>,
    client: reqwest::Client,
    semaphore: Semaphore,
    decode_semaphore: Semaphore,
    runtime: tokio::runtime::Handle,
    wakeup_tx: async_channel::Sender<()>,
    temp_seq: AtomicU64,
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

        Self {
            inner: Arc::new(PosterCacheInner {
                cache_root,
                entries: RwLock::new(HashMap::with_capacity(256)),
                decoded_lru: Mutex::new(BoundedTextureLru::with_capacity(MAX_DECODED_TEXTURES)),
                hero_lru: Mutex::new(BoundedTextureLru::with_capacity(MAX_HERO_TEXTURES)),
                ambient_lru: Mutex::new(BoundedTextureLru::with_capacity(MAX_AMBIENT_TEXTURES)),
                evicted_images: Mutex::new(Vec::with_capacity(16)),
                client,
                semaphore: Semaphore::new(MAX_CONCURRENT_POSTER_DOWNLOADS),
                decode_semaphore: Semaphore::new(MAX_CONCURRENT_IMAGE_DECODES),
                runtime,
                wakeup_tx,
                temp_seq: AtomicU64::new(1),
            }),
            wakeup_rx,
        }
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

    pub fn resolve_poster_image(&self, url: &str) -> Option<Arc<RenderImage>> {
        self.resolve_decoded_image_bounded(url, "poster", POSTER_MAX_W, POSTER_MAX_H)
    }

    pub fn resolve_poster_url(&self, url: Option<&Url>) -> Option<Arc<RenderImage>> {
        self.resolve_poster_image(url?.as_str())
    }

    pub fn resolve_widescreen_image(&self, url: &str) -> Option<Arc<RenderImage>> {
        self.resolve_decoded_image_bounded(url, "wide", WIDESCREEN_MAX_W, WIDESCREEN_MAX_H)
    }

    pub fn resolve_widescreen_url(&self, url: Option<&Url>) -> Option<Arc<RenderImage>> {
        self.resolve_widescreen_image(url?.as_str())
    }

    pub fn resolve_hero_backdrop_image(&self, url: &str) -> Option<Arc<RenderImage>> {
        let trimmed = url.trim();
        if trimmed.is_empty() || !(trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        {
            return None;
        }
        let lru_key = format!("hero:{trimmed}");
        let (hit, should_spawn) = self.inner.hero_lru.lock().lookup_or_begin(&lru_key);
        if hit.is_some() || !should_spawn {
            return hit;
        }

        let inner = Arc::clone(&self.inner);
        let url_owned = trimmed.to_owned();
        self.inner.runtime.spawn(async move {
            fetch_and_decode_hero(inner, url_owned, lru_key).await;
        });
        None
    }

    pub fn resolve_hero_backdrop_url(&self, url: Option<&Url>) -> Option<Arc<RenderImage>> {
        self.resolve_hero_backdrop_image(url?.as_str())
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

    pub fn drain_evicted_images(&self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut *self.inner.evicted_images.lock())
    }

    fn resolve_decoded_image_bounded(
        &self,
        url: &str,
        variant: &'static str,
        max_w: u32,
        max_h: u32,
    ) -> Option<Arc<RenderImage>> {
        let trimmed = url.trim();
        if trimmed.is_empty() || !(trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        {
            return None;
        }
        let lru_key = format!("{variant}:{trimmed}");
        let (hit, should_spawn) = self.inner.decoded_lru.lock().lookup_or_begin(&lru_key);
        if hit.is_some() || !should_spawn {
            return hit;
        }

        let inner = Arc::clone(&self.inner);
        let url_owned = trimmed.to_owned();
        self.inner.runtime.spawn(async move {
            fetch_and_decode_into_lru(inner, url_owned, lru_key, max_w, max_h).await;
        });
        None
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
        let trimmed = url.trim();
        if trimmed.is_empty() || !(trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        {
            return None;
        }

        let (hit, should_spawn) = self.inner.ambient_lru.lock().lookup_or_begin(trimmed);
        if hit.is_some() || !should_spawn {
            return hit;
        }

        let inner = Arc::clone(&self.inner);
        let url_owned = trimmed.to_owned();
        self.inner.runtime.spawn(async move {
            compute_or_fetch_ambient(inner, url_owned).await;
        });

        None
    }

    pub fn resolve_ambient_url(&self, url: Option<&Url>) -> Option<Arc<RenderImage>> {
        self.resolve_ambient_bleed(url?.as_str())
    }

    pub fn clear_failures(&self) {
        self.inner
            .entries
            .write()
            .retain(|_, v| !matches!(v, CacheEntry::Failed));
        self.inner.decoded_lru.lock().clear_failures();
        self.inner.hero_lru.lock().clear_failures();
        self.inner.ambient_lru.lock().clear_failures();
    }
}

async fn load_or_download_bytes(inner: &Arc<PosterCacheInner>, url: &str) -> Option<Vec<u8>> {
    let target_path = cache_path_for_url(&inner.cache_root, url);

    if let Ok(bytes) = tokio::fs::read(&target_path).await {
        if bytes.len() > 64 {
            inner
                .entries
                .write()
                .insert(url.to_owned(), CacheEntry::Ready(target_path));
            return Some(bytes);
        }
    }

    let Ok(_permit) = inner.semaphore.acquire().await else {
        return None;
    };

    if let Ok(bytes) = tokio::fs::read(&target_path).await {
        if bytes.len() > 64 {
            inner
                .entries
                .write()
                .insert(url.to_owned(), CacheEntry::Ready(target_path));
            return Some(bytes);
        }
    }

    match download_and_persist(inner, url, &target_path).await {
        Ok(bytes) => {
            inner
                .entries
                .write()
                .insert(url.to_owned(), CacheEntry::Ready(target_path));
            Some(bytes)
        }
        Err(err) => {
            let safe_url = crate::updater::redact_url_for_log(url);
            tracing::debug!(target: "ferrite::poster_cache", url = %safe_url, "artwork fetch failed: {err}");
            inner
                .entries
                .write()
                .insert(url.to_owned(), CacheEntry::Failed);
            None
        }
    }
}

async fn fetch_and_decode_into_lru(
    inner: Arc<PosterCacheInner>,
    url: String,
    lru_key: String,
    max_w: u32,
    max_h: u32,
) {
    let Some(bytes) = load_or_download_bytes(&inner, &url).await else {
        inner.decoded_lru.lock().mark_failed(lru_key);
        return;
    };

    let Ok(_decode_permit) = inner.decode_semaphore.acquire().await else {
        inner.decoded_lru.lock().mark_failed(lru_key);
        return;
    };

    let decoded =
        tokio::task::spawn_blocking(move || decode_scaled_render_image(&bytes, max_w, max_h))
            .await
            .ok()
            .flatten();
    drop(_decode_permit);

    match decoded {
        Some(render_img) => {
            let evicted = inner.decoded_lru.lock().insert_ready(lru_key, render_img);
            if !evicted.is_empty() {
                inner.evicted_images.lock().extend(evicted);
            }
            let _ = inner.wakeup_tx.try_send(());
        }
        None => {
            inner.decoded_lru.lock().mark_failed(lru_key);
        }
    }
}

async fn fetch_and_decode_hero(inner: Arc<PosterCacheInner>, url: String, lru_key: String) {
    let Some(bytes) = load_or_download_bytes(&inner, &url).await else {
        inner.hero_lru.lock().mark_failed(lru_key);
        return;
    };

    let Ok(_decode_permit) = inner.decode_semaphore.acquire().await else {
        inner.hero_lru.lock().mark_failed(lru_key);
        return;
    };

    let decoded = tokio::task::spawn_blocking(move || {
        decode_scaled_render_image(&bytes, HERO_MAX_W, HERO_MAX_H)
    })
    .await
    .ok()
    .flatten();
    drop(_decode_permit);

    match decoded {
        Some(render_img) => {
            let evicted = inner.hero_lru.lock().insert_ready(lru_key, render_img);
            if !evicted.is_empty() {
                inner.evicted_images.lock().extend(evicted);
            }
            let _ = inner.wakeup_tx.try_send(());
        }
        None => {
            inner.hero_lru.lock().mark_failed(lru_key);
        }
    }
}

async fn fetch_or_load_disk(inner: Arc<PosterCacheInner>, url: String) -> Option<PathBuf> {
    let target_path = cache_path_for_url(&inner.cache_root, &url);

    if let Ok(meta) = tokio::fs::metadata(&target_path).await {
        if meta.is_file() && meta.len() > 64 {
            inner
                .entries
                .write()
                .insert(url, CacheEntry::Ready(target_path.clone()));
            let _ = inner.wakeup_tx.try_send(());
            return Some(target_path);
        }
    }

    let Ok(_permit) = inner.semaphore.acquire().await else {
        inner.entries.write().insert(url, CacheEntry::Failed);
        return None;
    };

    match download_and_persist(&inner, &url, &target_path).await {
        Ok(_) => {
            inner
                .entries
                .write()
                .insert(url, CacheEntry::Ready(target_path.clone()));
            let _ = inner.wakeup_tx.try_send(());
            Some(target_path)
        }
        Err(err) => {
            let safe_url = crate::updater::redact_url_for_log(&url);
            tracing::debug!(target: "ferrite::poster_cache", url = %safe_url, "artwork fetch failed: {err}");
            inner.entries.write().insert(url, CacheEntry::Failed);
            None
        }
    }
}

async fn compute_or_fetch_ambient(inner: Arc<PosterCacheInner>, url: String) {
    let Some(bytes) = load_or_download_bytes(&inner, &url).await else {
        inner.ambient_lru.lock().mark_failed(url);
        return;
    };

    let Ok(_decode_permit) = inner.decode_semaphore.acquire().await else {
        inner.ambient_lru.lock().mark_failed(url);
        return;
    };

    let built = tokio::task::spawn_blocking(move || build_ambient_bleed_image(&bytes))
        .await
        .ok()
        .flatten();
    drop(_decode_permit);

    match built {
        Some(render_img) => {
            let evicted = inner.ambient_lru.lock().insert_ready(url, render_img);
            if !evicted.is_empty() {
                inner.evicted_images.lock().extend(evicted);
            }
            let _ = inner.wakeup_tx.try_send(());
        }
        None => {
            inner.ambient_lru.lock().mark_failed(url);
        }
    }
}

async fn download_and_persist(
    inner: &PosterCacheInner,
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

    if let Some(parent) = target_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = tokio::fs::set_permissions(&inner.cache_root, std::fs::Permissions::from_mode(0o700)).await;
            let _ = tokio::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).await;
        }
    }

    let seq = inner.temp_seq.fetch_add(1, Ordering::Relaxed);
    let tmp_path = target_path.with_extension(format!("part.{seq}"));
    tokio::fs::write(&tmp_path, &bytes).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600)).await;
    }
    tokio::fs::rename(&tmp_path, target_path).await?;
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn deterministic_sharded_cache_paths() {
        let root = Path::new("/tmp/stremio-posters");
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
        let mut img = RgbaImage::new(96, 54);
        for (x, y, px_mut) in img.enumerate_pixels_mut() {
            *px_mut = image::Rgba([(x * 2) as u8, (y * 3) as u8, 220, 255]);
        }
        let mut png_bytes = Vec::new();
        let dyn_img = image::DynamicImage::ImageRgba8(img);
        dyn_img
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();

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
        let mut img = RgbaImage::new(8, 8);
        for (_, _, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([255, 94, 58, 255]);
        }
        let mut png_bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let img1 = decode_scaled_render_image(&png_bytes, 64, 64).expect("decode png 1");
        let img2 = decode_scaled_render_image(&png_bytes, 64, 64).expect("decode png 2");
        let img3 = decode_scaled_render_image(&png_bytes, 64, 64).expect("decode png 3");

        let mut lru = BoundedTextureLru::with_capacity(2);
        assert!(lru.insert_ready("k1".to_owned(), Arc::clone(&img1)).is_empty());
        assert!(lru.insert_ready("k2".to_owned(), Arc::clone(&img2)).is_empty());
        assert!(lru.lookup_or_begin("k1").0.is_some());
        let evicted = lru.insert_ready("k3".to_owned(), Arc::clone(&img3));
        assert_eq!(evicted.len(), 1);
        assert!(Arc::ptr_eq(&evicted[0], &img2));

        assert_eq!(lru.len_ready(), 2);
        assert!(lru.lookup_or_begin("k1").0.is_some());
        assert!(lru.lookup_or_begin("k3").0.is_some());
        assert!(lru.lookup_or_begin("k2").1);

        let drained = lru.drain_all();
        assert_eq!(drained.len(), 2);
        assert_eq!(lru.len_ready(), 0);
    }

    #[tokio::test]
    async fn poster_cache_eviction_queue_drains_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PosterCache::new(dir.path().to_path_buf(), tokio::runtime::Handle::current());
        assert!(cache.drain_evicted_images().is_empty());

        let mut img = RgbaImage::new(4, 4);
        for (_, _, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([20, 30, 40, 255]);
        }
        let mut png_bytes = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let tex = decode_scaled_render_image(&png_bytes, 32, 32).unwrap();
        cache.push_evicted_image(Arc::clone(&tex));
        let drained = cache.drain_evicted_images();
        assert_eq!(drained.len(), 1);
        assert!(Arc::ptr_eq(&drained[0], &tex));
        assert!(cache.drain_evicted_images().is_empty());

        for i in 0..24 {
            let t = decode_scaled_render_image(&png_bytes, 32, 32).unwrap();
            assert!(cache
                .inner
                .decoded_lru
                .lock()
                .insert_ready(format!("poster:http://example/{i}.jpg"), t)
                .is_empty());
        }
        for i in 0..4 {
            let a = decode_scaled_render_image(&png_bytes, 32, 32).unwrap();
            assert!(cache
                .inner
                .ambient_lru
                .lock()
                .insert_ready(format!("http://example/amb-{i}.jpg"), a)
                .is_empty());
        }
        assert_eq!(cache.decoded_ready_count(), 24);
        cache.trim_for_playback();
        assert_eq!(cache.decoded_ready_count(), PLAYBACK_RETAINED_DECODED_TEXTURES);
        let playback_evicted = cache.drain_evicted_images();
        assert_eq!(playback_evicted.len(), 10);
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
}
