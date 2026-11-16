use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use chrono::Utc;
use once_cell::sync::Lazy;
use regex::Regex;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use stremio_core::{
    constants::{CINEMETA_URL, GENRES_LINK_CATEGORY, IMDB_LINK_CATEGORY, IMDB_URL, META_RESOURCE_NAME},
    types::{
        addon::ResourceRequest,
        library::LibraryItem,
        resource::{Link, MetaItem, MetaItemBehaviorHints, MetaItemPreview, PosterShape},
    },
};
use tracing::debug;
use url::Url;

use crate::{
    env::{ensure_private_data_dir, precreate_private_db_file, tighten_sqlite_file_permissions},
    error::CoreRuntimeError,
    private_config::redact_secret,
};

pub const DEFAULT_TMDB_API_BASE_URL: &str = "https://api.themoviedb.org/3";
pub const DEFAULT_TMDB_IMAGE_BASE_URL: &str = "https://image.tmdb.org/t/p";
pub const TMDB_HTTP_TIMEOUT_SECS: u64 = 6;
const MAX_LOCAL_QUERY_CACHE: usize = 128;
const MAX_ENRICHED_META_CACHE: usize = 256;
const MAX_COLLECTION_CACHE: usize = 64;
const MAX_RECOMMENDATION_HITS: usize = 16;

static YEAR_4_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:19|20)\d{2}").expect("valid year regex"));

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmdbAccountStatus {
    pub ok: bool,
    pub auth_mode: &'static str,
    pub image_base_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonCreditChip {
    pub person_key: String,
    pub tmdb_person_id: Option<u64>,
    pub name: String,
    pub role_or_character: Option<String>,
    pub is_director: bool,
    pub headshot_url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrichedCollection {
    pub id: u64,
    pub name: String,
    pub overview: Option<String>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    #[serde(default)]
    pub parts: Vec<EditorialMediaHit>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrichedTitleMeta {
    pub canonical_id: String,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<u64>,
    pub media_type: String,
    pub title: String,
    pub year: Option<String>,
    pub overview: Option<String>,
    pub runtime: Option<String>,
    pub imdb_rating: Option<String>,
    pub genres: Vec<String>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub logo_url: Option<String>,
    pub directors: Vec<PersonCreditChip>,
    pub cast: Vec<PersonCreditChip>,
    #[serde(default)]
    pub tagline: Option<String>,
    #[serde(default)]
    pub certification: Option<String>,
    #[serde(default)]
    pub trailer_youtube_key: Option<String>,
    #[serde(default)]
    pub collection: Option<EnrichedCollection>,
    #[serde(default)]
    pub recommendations: Vec<EditorialMediaHit>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorialMediaHit {
    pub canonical_id: String,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<u64>,
    pub media_type: String,
    pub title: String,
    pub year: Option<String>,
    pub imdb_rating: Option<String>,
    pub overview: Option<String>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub logo_url: Option<String>,
    pub genres: Vec<String>,
    pub score: i64,
    #[serde(default)]
    pub role_hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorialPersonHit {
    pub person_key: String,
    pub tmdb_person_id: Option<u64>,
    pub name: String,
    pub department: String,
    pub headshot_url: Option<String>,
    pub known_for: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonFilmography {
    pub person_key: String,
    pub tmdb_person_id: Option<u64>,
    pub name: String,
    pub department: String,
    pub biography: Option<String>,
    pub birthday: Option<String>,
    pub place_of_birth: Option<String>,
    pub headshot_url: Option<String>,
    pub movies: Vec<EditorialMediaHit>,
    pub series: Vec<EditorialMediaHit>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnifiedEditorialSearch {
    pub query: String,
    pub top_match: Option<EditorialMediaHit>,
    pub movies: Vec<EditorialMediaHit>,
    pub series: Vec<EditorialMediaHit>,
    pub people: Vec<EditorialPersonHit>,
    pub elapsed_us: u64,
}

#[derive(Default)]
struct HotMetadataCaches {
    search_queries: HashMap<String, (Vec<EditorialMediaHit>, Vec<EditorialPersonHit>)>,
    search_lru: VecDeque<String>,
    titles: HashMap<String, (EnrichedTitleMeta, bool)>,
    titles_lru: VecDeque<String>,
    collections: HashMap<u64, EnrichedCollection>,
    collections_lru: VecDeque<u64>,
}

impl HotMetadataCaches {
    fn invalidate_search(&mut self) {
        self.search_queries.clear();
        self.search_lru.clear();
    }

    fn clear_all(&mut self) {
        self.search_queries.clear();
        self.search_lru.clear();
        self.titles.clear();
        self.titles_lru.clear();
        self.collections.clear();
        self.collections_lru.clear();
    }

    fn get_search(
        &self,
        key: &str,
    ) -> Option<(Vec<EditorialMediaHit>, Vec<EditorialPersonHit>)> {
        self.search_queries.get(key).cloned()
    }

    fn insert_search(
        &mut self,
        key: String,
        val: (Vec<EditorialMediaHit>, Vec<EditorialPersonHit>),
    ) {
        if self.search_queries.contains_key(&key) {
            self.search_lru.retain(|k| k != &key);
        } else if self.search_lru.len() >= MAX_LOCAL_QUERY_CACHE {
            if let Some(oldest) = self.search_lru.pop_front() {
                self.search_queries.remove(&oldest);
            }
        }
        self.search_lru.push_back(key.clone());
        self.search_queries.insert(key, val);
    }

    fn get_title(&self, canonical_or_alt_id: &str) -> Option<(EnrichedTitleMeta, bool)> {
        self.titles.get(canonical_or_alt_id).cloned()
    }

    fn insert_title(&mut self, key: String, meta: EnrichedTitleMeta, has_extra: bool) {
        if self.titles.contains_key(&key) {
            self.titles_lru.retain(|k| k != &key);
        } else if self.titles_lru.len() >= MAX_ENRICHED_META_CACHE {
            if let Some(oldest) = self.titles_lru.pop_front() {
                self.titles.remove(&oldest);
            }
        }
        self.titles_lru.push_back(key.clone());
        self.titles.insert(key, (meta, has_extra));
    }

    fn get_collection(&self, collection_id: u64) -> Option<EnrichedCollection> {
        self.collections.get(&collection_id).cloned()
    }

    fn insert_collection(&mut self, collection_id: u64, col: EnrichedCollection) {
        if self.collections.contains_key(&collection_id) {
            self.collections_lru.retain(|k| k != &collection_id);
        } else if self.collections_lru.len() >= MAX_COLLECTION_CACHE {
            if let Some(oldest) = self.collections_lru.pop_front() {
                self.collections.remove(&oldest);
            }
        }
        self.collections_lru.push_back(collection_id);
        self.collections.insert(collection_id, col);
    }
}

#[derive(Clone, Debug)]
struct TmdbEndpoints {
    api_base: String,
    image_base: String,
}

impl Default for TmdbEndpoints {
    fn default() -> Self {
        Self {
            api_base: DEFAULT_TMDB_API_BASE_URL.to_owned(),
            image_base: DEFAULT_TMDB_IMAGE_BASE_URL.to_owned(),
        }
    }
}

pub struct MetadataEngine {
    db_path: std::path::PathBuf,
    conn: Arc<Mutex<Connection>>,
    read_conn: Option<Arc<Mutex<Connection>>>,
    hot: Arc<Mutex<HotMetadataCaches>>,
    http: reqwest::Client,
    endpoints: RwLock<TmdbEndpoints>,
}

impl std::fmt::Debug for MetadataEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetadataEngine")
            .field("db_path", &self.db_path)
            .finish()
    }
}

impl MetadataEngine {
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
            let _ = precreate_private_db_file(path);
        }

        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;
             CREATE TABLE IF NOT EXISTS meta_titles (
                 canonical_id TEXT PRIMARY KEY NOT NULL,
                 imdb_id TEXT,
                 tmdb_id INTEGER,
                 media_type TEXT NOT NULL,
                 title TEXT NOT NULL,
                 norm_title TEXT NOT NULL,
                 year TEXT,
                 overview TEXT,
                 runtime TEXT,
                 imdb_rating TEXT,
                 genres_json TEXT NOT NULL DEFAULT '[]',
                 poster_url TEXT,
                 backdrop_url TEXT,
                 logo_url TEXT,
                 enriched_from_tmdb INTEGER NOT NULL DEFAULT 0,
                 updated_at INTEGER NOT NULL,
                 extra_enrichment_json TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_meta_titles_imdb ON meta_titles(imdb_id);
             CREATE INDEX IF NOT EXISTS idx_meta_titles_norm ON meta_titles(media_type, norm_title, year);
             CREATE TABLE IF NOT EXISTS meta_collections (
                 collection_id INTEGER PRIMARY KEY NOT NULL,
                 name TEXT NOT NULL,
                 overview TEXT,
                 poster_url TEXT,
                 backdrop_url TEXT,
                 parts_json TEXT NOT NULL DEFAULT '[]',
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS id_bridge (
                 tmdb_type TEXT NOT NULL,
                 tmdb_id INTEGER NOT NULL,
                 imdb_id TEXT NOT NULL,
                 updated_at INTEGER NOT NULL,
                 PRIMARY KEY (tmdb_type, tmdb_id)
             );
             CREATE INDEX IF NOT EXISTS idx_id_bridge_imdb ON id_bridge(imdb_id);
             CREATE TABLE IF NOT EXISTS meta_people (
                 person_key TEXT PRIMARY KEY NOT NULL,
                 tmdb_person_id INTEGER,
                 name TEXT NOT NULL,
                 norm_name TEXT NOT NULL,
                 department TEXT NOT NULL DEFAULT 'Acting',
                 biography TEXT,
                 birthday TEXT,
                 place_of_birth TEXT,
                 headshot_url TEXT,
                 known_for_json TEXT NOT NULL DEFAULT '[]',
                 updated_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_meta_people_tmdb ON meta_people(tmdb_person_id);
             CREATE TABLE IF NOT EXISTS title_credits (
                 canonical_id TEXT NOT NULL,
                 person_key TEXT NOT NULL,
                 role_kind TEXT NOT NULL,
                 character_or_job TEXT,
                 credit_order INTEGER NOT NULL,
                 PRIMARY KEY (canonical_id, person_key, role_kind)
             );
             CREATE INDEX IF NOT EXISTS idx_title_credits_person ON title_credits(person_key);
             CREATE VIRTUAL TABLE IF NOT EXISTS fts_titles USING fts5(
                 canonical_id UNINDEXED,
                 media_type UNINDEXED,
                 title,
                 overview,
                 genres_text,
                 tokenize = 'unicode61 remove_diacritics 2'
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS fts_people USING fts5(
                 person_key UNINDEXED,
                 name,
                 known_for_text,
                 tokenize = 'unicode61 remove_diacritics 2'
             );",
        )?;

        if let Err(err) = conn.execute(
            "ALTER TABLE meta_titles ADD COLUMN extra_enrichment_json TEXT",
            [],
        ) {
            let msg = err.to_string().to_ascii_lowercase();
            if !msg.contains("duplicate column") {
                return Err(CoreRuntimeError::Sqlite(err));
            }
        }

        if !is_memory {
            tighten_sqlite_file_permissions(path);
        }

        let read_conn = if !is_memory {
            let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
            let rconn = Connection::open_with_flags(path, flags)?;
            rconn.execute_batch(
                "PRAGMA query_only = ON;
                 PRAGMA busy_timeout = 2000;",
            )?;
            Some(Arc::new(Mutex::new(rconn)))
        } else {
            None
        };

        let http = reqwest::Client::builder()
            .user_agent("Ferrite/0.4.0")
            .connect_timeout(Duration::from_secs(TMDB_HTTP_TIMEOUT_SECS))
            .timeout(Duration::from_secs(TMDB_HTTP_TIMEOUT_SECS))
            .tcp_nodelay(true)
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .pool_max_idle_per_host(6)
            .pool_idle_timeout(Duration::from_secs(90))
            .http2_adaptive_window(true)
            .gzip(true)
            .deflate(true)
            .build()?;

        Ok(Self {
            db_path: path.to_path_buf(),
            conn: Arc::new(Mutex::new(conn)),
            read_conn,
            hot: Arc::new(Mutex::new(HotMetadataCaches::default())),
            http,
            endpoints: RwLock::new(TmdbEndpoints::default()),
        })
    }

    fn reader_conn(&self) -> &Arc<Mutex<Connection>> {
        self.read_conn.as_ref().unwrap_or(&self.conn)
    }

    pub fn set_base_urls(&self, api_base: impl Into<String>, image_base: impl Into<String>) {
        if let Ok(mut guard) = self.endpoints.write() {
            let api = api_base.into().trim_end_matches('/').to_owned();
            let img = image_base.into().trim_end_matches('/').to_owned();
            if !api.is_empty() {
                guard.api_base = api;
            }
            if !img.is_empty() {
                guard.image_base = img;
            }
        }
    }

    pub fn clear_caches(&self) {
        if let Ok(mut guard) = self.hot.lock() {
            guard.clear_all();
        }
    }

    fn current_endpoints(&self) -> TmdbEndpoints {
        self.endpoints
            .read()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    pub fn ingest_stremio_previews(&self, items: &[MetaItemPreview]) {
        if items.is_empty() {
            return;
        }
        let Ok(mut conn) = self.conn.lock() else {
            return;
        };
        let Ok(tx) = conn.transaction() else {
            return;
        };
        let now = Utc::now().timestamp();
        let mut any_changed = false;
        for preview in items {
            if upsert_preview_tx(&tx, preview, now) {
                any_changed = true;
            }
        }
        if tx.commit().is_ok() && any_changed {
            if let Ok(mut hot) = self.hot.lock() {
                hot.invalidate_search();
            }
        }
    }

    pub fn ingest_stremio_meta_item(&self, item: &MetaItem) {
        let Ok(mut conn) = self.conn.lock() else {
            return;
        };
        let Ok(tx) = conn.transaction() else {
            return;
        };
        let now = Utc::now().timestamp();
        let changed = upsert_preview_tx(&tx, &item.preview, now);
        if tx.commit().is_ok() && changed {
            if let Ok(mut hot) = self.hot.lock() {
                hot.invalidate_search();
                hot.titles.remove(&item.preview.id);
            }
        }
    }

    pub fn ingest_library_items(&self, items: &[LibraryItem]) {
        if items.is_empty() {
            return;
        }
        let previews: Vec<MetaItemPreview> = items
            .iter()
            .filter(|li| !li.id.trim().is_empty() && !li.name.trim().is_empty())
            .map(|li| MetaItemPreview {
                id: li.id.clone(),
                r#type: li.r#type.clone(),
                name: li.name.clone(),
                poster: li.poster.clone(),
                background: None,
                logo: None,
                description: None,
                release_info: None,
                runtime: None,
                released: None,
                poster_shape: PosterShape::default(),
                links: Vec::new(),
                trailer_streams: Vec::new(),
                behavior_hints: MetaItemBehaviorHints::default(),
            })
            .collect();
        self.ingest_stremio_previews(&previews);
    }

    pub fn search_local_fts(
        &self,
        query: &str,
        limit: usize,
    ) -> (Vec<EditorialMediaHit>, Vec<EditorialPersonHit>) {
        let trimmed = query.trim();
        if trimmed.is_empty() || limit == 0 {
            return (Vec::new(), Vec::new());
        }
        let cache_key = format!("{}::{}", trimmed.to_ascii_lowercase(), limit);
        if let Ok(hot) = self.hot.lock() {
            if let Some(hit) = hot.get_search(&cache_key) {
                return hit;
            }
        }

        let Some(match_expr) = build_fts5_match_expr(trimmed) else {
            return (Vec::new(), Vec::new());
        };
        let norm_q = normalize_title_for_dedup(trimmed);

        let Ok(conn) = self.reader_conn().lock() else {
            return (Vec::new(), Vec::new());
        };

        let mut titles = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT m.canonical_id, m.imdb_id, m.tmdb_id, m.media_type, m.title, m.year,
                    m.imdb_rating, m.overview, m.poster_url, m.backdrop_url, m.logo_url,
                    m.genres_json, m.enriched_from_tmdb
             FROM fts_titles f
             JOIN meta_titles m ON m.canonical_id = f.canonical_id
             WHERE fts_titles MATCH ?1
             ORDER BY bm25(fts_titles)
             LIMIT ?2",
        ) {
            if let Ok(rows) = stmt.query_map(params![match_expr, (limit * 2).max(12) as i64], |r| {
                let genres_raw: String = r.get(11)?;
                let genres: Vec<String> = serde_json::from_str(&genres_raw).unwrap_or_default();
                let tmdb_id_raw: Option<i64> = r.get(2)?;
                let title: String = r.get(4)?;
                let enriched_flag: i64 = r.get(12)?;
                let base_score = score_title_relevance(&title, &norm_q)
                    + if enriched_flag > 0 { 18 } else { 0 };
                Ok(EditorialMediaHit {
                    canonical_id: r.get(0)?,
                    imdb_id: r.get(1)?,
                    tmdb_id: tmdb_id_raw.and_then(|v| u64::try_from(v).ok()),
                    media_type: r.get(3)?,
                    title,
                    year: r.get(5)?,
                    imdb_rating: r.get(6)?,
                    overview: r.get(7)?,
                    poster_url: r.get(8)?,
                    backdrop_url: r.get(9)?,
                    logo_url: r.get(10)?,
                    genres,
                    score: base_score,
                    role_hint: None,
                })
            }) {
                for row in rows.flatten() {
                    titles.push(row);
                }
            }
        }
        titles.sort_by_key(|a| std::cmp::Reverse(a.score));
        titles.truncate(limit);

        let mut people = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT p.person_key, p.tmdb_person_id, p.name, p.department, p.headshot_url, p.known_for_json
             FROM fts_people f
             JOIN meta_people p ON p.person_key = f.person_key
             WHERE fts_people MATCH ?1
             ORDER BY bm25(fts_people)
             LIMIT ?2",
        ) {
            if let Ok(rows) = stmt.query_map(params![match_expr, limit as i64], |r| {
                let known_raw: String = r.get(5)?;
                let known_for: Vec<String> = serde_json::from_str(&known_raw).unwrap_or_default();
                let tmdb_pid: Option<i64> = r.get(1)?;
                Ok(EditorialPersonHit {
                    person_key: r.get(0)?,
                    tmdb_person_id: tmdb_pid.and_then(|v| u64::try_from(v).ok()),
                    name: r.get(2)?,
                    department: r.get(3)?,
                    headshot_url: r.get(4)?,
                    known_for,
                })
            }) {
                for row in rows.flatten() {
                    people.push(row);
                }
            }
        }

        if let Ok(mut hot) = self.hot.lock() {
            hot.insert_search(cache_key, (titles.clone(), people.clone()));
        }
        (titles, people)
    }

    pub fn cached_enriched_meta(&self, id: &str) -> Option<EnrichedTitleMeta> {
        self.cached_enriched_meta_with_extra(id).map(|(m, _)| m)
    }

    fn cached_enriched_meta_with_extra(&self, id: &str) -> Option<(EnrichedTitleMeta, bool)> {
        if let Ok(hot) = self.hot.lock() {
            if let Some(hit) = hot.get_title(id) {
                return Some(hit);
            }
        }

        let Ok(conn) = self.reader_conn().lock() else {
            return None;
        };
        load_and_cache_enriched_title(&conn, &self.hot, id)
    }

    async fn cached_enriched_meta_with_extra_async(
        &self,
        id: &str,
    ) -> Option<(EnrichedTitleMeta, bool)> {
        if let Ok(hot) = self.hot.lock() {
            if let Some(hit) = hot.get_title(id) {
                return Some(hit);
            }
        }
        let reader = Arc::clone(self.reader_conn());
        let hot = Arc::clone(&self.hot);
        let id_owned = id.to_owned();
        run_blocking_tmdb(move || {
            let Ok(conn) = reader.lock() else {
                return None;
            };
            load_and_cache_enriched_title(&conn, &hot, &id_owned)
        })
        .await
    }

    pub fn lookup_bridged_imdb_id(&self, media_type: &str, tmdb_id: u64) -> Option<String> {
        let Ok(conn) = self.reader_conn().lock() else {
            return None;
        };
        lookup_bridged_imdb_conn(&conn, media_type, tmdb_id)
    }

    pub async fn verify_credentials(&self, api_key: &str) -> Result<TmdbAccountStatus, String> {
        let key = api_key.trim();
        if key.is_empty() {
            return Err("TMDB API key or Read Access Token is empty".to_owned());
        }
        let endpoints = self.current_endpoints();
        let auth_mode = if is_v4_bearer_token(key) {
            "v4_bearer"
        } else {
            "v3_api_key"
        };
        let val: serde_json::Value = self
            .tmdb_get_json(&endpoints.api_base, "/configuration", &[], key)
            .await?;
        let image_base = val
            .get("images")
            .and_then(|imgs| {
                imgs.get("secure_base_url")
                    .or_else(|| imgs.get("base_url"))
                    .and_then(|v| v.as_str())
            })
            .map(|s| s.trim_end_matches('/').to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or(endpoints.image_base);
        Ok(TmdbAccountStatus {
            ok: true,
            auth_mode,
            image_base_url: image_base,
        })
    }

    pub async fn search_multi(
        &self,
        api_key: &str,
        query: &str,
    ) -> Result<(Vec<EditorialMediaHit>, Vec<EditorialPersonHit>), String> {
        let trimmed = query.trim();
        if trimmed.is_empty() || api_key.trim().is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let endpoints = self.current_endpoints();
        let norm_q = normalize_title_for_dedup(trimmed);
        let resp: TmdbSearchMultiResponse = self
            .tmdb_get_json(
                &endpoints.api_base,
                "/search/multi",
                &[("query", trimmed), ("include_adult", "false"), ("page", "1")],
                api_key,
            )
            .await?;

        let mut media_hits = Vec::new();
        let mut person_hits = Vec::new();

        for item in resp.results {
            match item.media_type.as_deref() {
                Some("movie") | Some("tv") => {
                    let stremio_type = if item.media_type.as_deref() == Some("tv") {
                        "series"
                    } else {
                        "movie"
                    };
                    let title = item
                        .title
                        .or(item.name)
                        .unwrap_or_default()
                        .trim()
                        .to_owned();
                    if title.is_empty() {
                        continue;
                    }
                    let year = item
                        .release_date
                        .or(item.first_air_date)
                        .as_deref()
                        .and_then(extract_4_digit_year);
                    let vote = item
                        .vote_average
                        .filter(|v| *v > 0.1)
                        .map(|v| format!("{v:.1}"));
                    let score = score_title_relevance(&title, &norm_q) + 22;
                    media_hits.push(EditorialMediaHit {
                        canonical_id: format!("tmdb:{}", item.id),
                        imdb_id: None,
                        tmdb_id: Some(item.id),
                        media_type: stremio_type.to_owned(),
                        title,
                        year,
                        imdb_rating: vote,
                        overview: item.overview.filter(|s| !s.trim().is_empty()),
                        poster_url: tmdb_img_url(&endpoints.image_base, "w342", item.poster_path),
                        backdrop_url: tmdb_img_url(
                            &endpoints.image_base,
                            "w780",
                            item.backdrop_path,
                        ),
                        logo_url: None,
                        genres: Vec::new(),
                        score,
                        role_hint: None,
                    });
                }
                Some("person") => {
                    let name = item.name.or(item.title).unwrap_or_default().trim().to_owned();
                    if name.is_empty() {
                        continue;
                    }
                    let known_for: Vec<String> = item
                        .known_for
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|k| k.title.or(k.name))
                        .take(3)
                        .collect();
                    person_hits.push(EditorialPersonHit {
                        person_key: format!("tmdb_person:{}", item.id),
                        tmdb_person_id: Some(item.id),
                        name,
                        department: item
                            .known_for_department
                            .unwrap_or_else(|| "Acting".to_owned()),
                        headshot_url: tmdb_img_url(
                            &endpoints.image_base,
                            "w185",
                            item.profile_path,
                        ),
                        known_for,
                    });
                }
                _ => {}
            }
        }

        let conn = Arc::clone(&self.conn);
        let hot = Arc::clone(&self.hot);
        let (media_hits, person_hits) = run_blocking_tmdb(move || {
            if let Ok(conn_guard) = conn.lock() {
                for hit in &mut media_hits {
                    if let Some(tid) = hit.tmdb_id {
                        if let Some(imdb) =
                            lookup_bridged_imdb_conn(&conn_guard, &hit.media_type, tid)
                        {
                            hit.canonical_id = imdb.clone();
                            hit.imdb_id = Some(imdb);
                        }
                    }
                }
            }
            persist_search_hits_sync(&conn, &hot, &media_hits, &person_hits);
            (media_hits, person_hits)
        })
        .await;
        Ok((media_hits, person_hits))
    }

    pub async fn enrich_title(
        &self,
        api_key: &str,
        media_type: &str,
        id: &str,
    ) -> Result<Option<EnrichedTitleMeta>, String> {
        let key = api_key.trim();
        if key.is_empty() {
            return Ok(self
                .cached_enriched_meta_with_extra_async(id)
                .await
                .map(|(m, _)| m));
        }

        // Fast return if already TMDB-enriched (including v0.5.0 extra fields).
        if let Some((existing, has_extra)) = self.cached_enriched_meta_with_extra_async(id).await {
            if existing.tmdb_id.is_some()
                && has_extra
                && (!existing.cast.is_empty()
                    || existing.logo_url.is_some()
                    || existing.trailer_youtube_key.is_some()
                    || !existing.recommendations.is_empty()
                    || existing.collection.is_some())
            {
                return Ok(Some(existing));
            }
        }

        let endpoints = self.current_endpoints();
        let stremio_type = canonical_stremio_type(media_type);
        let preferred_tmdb_type = to_tmdb_media_type(stremio_type);

        let (resolved_tmdb_type, tmdb_id) = if let Some(raw_num) = id.strip_prefix("tmdb:") {
            let parsed = raw_num
                .parse::<u64>()
                .map_err(|_| format!("Invalid TMDB ID '{id}'"))?;
            (preferred_tmdb_type, parsed)
        } else if id.starts_with("tt") {
            match self
                .resolve_imdb_to_tmdb(key, &endpoints.api_base, id, preferred_tmdb_type)
                .await?
            {
                Some(pair) => pair,
                None => {
                    return Ok(self
                        .cached_enriched_meta_with_extra_async(id)
                        .await
                        .map(|(m, _)| m))
                }
            }
        } else {
            return Ok(self
                .cached_enriched_meta_with_extra_async(id)
                .await
                .map(|(m, _)| m));
        };

        let path = format!("/{resolved_tmdb_type}/{tmdb_id}");
        let detail: TmdbDetailResponse = self
            .tmdb_get_json(
                &endpoints.api_base,
                &path,
                &[
                    (
                        "append_to_response",
                        "images,credits,external_ids,recommendations,similar,videos,release_dates,content_ratings",
                    ),
                    ("include_image_language", "en,null"),
                ],
                key,
            )
            .await?;

        let conn = Arc::clone(&self.conn);
        let hot = Arc::clone(&self.hot);
        let req_id = id.to_owned();
        let image_base = endpoints.image_base;
        let enriched = run_blocking_tmdb(move || {
            store_tmdb_detail_response_sync(
                &conn,
                &hot,
                &req_id,
                resolved_tmdb_type,
                tmdb_id,
                detail,
                &image_base,
            )
        })
        .await;
        Ok(Some(enriched))
    }

    pub async fn fetch_collection(
        &self,
        api_key: &str,
        collection_id: u64,
    ) -> Result<EnrichedCollection, CoreRuntimeError> {
        if let Ok(hot) = self.hot.lock() {
            if let Some(cached) = hot.get_collection(collection_id) {
                return Ok(cached);
            }
        }

        let reader = Arc::clone(self.reader_conn());
        let hot = Arc::clone(&self.hot);
        if let Some(cached) = run_blocking_tmdb(move || {
            let Ok(conn) = reader.lock() else {
                return None;
            };
            let loaded = load_collection_conn(&conn, collection_id)?;
            if let Ok(mut hot_guard) = hot.lock() {
                hot_guard.insert_collection(collection_id, loaded.clone());
            }
            Some(loaded)
        })
        .await
        {
            return Ok(cached);
        }

        let key = api_key.trim();
        if key.is_empty() {
            return Err(CoreRuntimeError::Env(
                "TMDB API key or Read Access Token is empty".to_owned(),
            ));
        }

        let endpoints = self.current_endpoints();
        let path = format!("/collection/{collection_id}");
        let resp: TmdbCollectionResponse = self
            .tmdb_get_json(&endpoints.api_base, &path, &[], key)
            .await
            .map_err(CoreRuntimeError::Env)?;

        let conn = Arc::clone(&self.conn);
        let hot = Arc::clone(&self.hot);
        let image_base = endpoints.image_base;
        let enriched = run_blocking_tmdb(move || {
            store_tmdb_collection_response_sync(&conn, &hot, collection_id, resp, &image_base)
        })
        .await;
        Ok(enriched)
    }

    pub async fn resolve_tmdb_to_imdb(
        &self,
        api_key: Option<&str>,
        media_type: &str,
        tmdb_id: u64,
    ) -> Option<String> {
        let reader = Arc::clone(self.reader_conn());
        let mt = media_type.to_owned();
        if let Some(cached) = run_blocking_tmdb(move || {
            let conn = reader.lock().ok()?;
            lookup_bridged_imdb_conn(&conn, &mt, tmdb_id)
        })
        .await
        {
            return Some(cached);
        }
        let key = api_key.map(str::trim).filter(|k| !k.is_empty())?;
        let endpoints = self.current_endpoints();
        let tmdb_type = to_tmdb_media_type(media_type);
        let path = format!("/{tmdb_type}/{tmdb_id}/external_ids");
        let ext: TmdbExternalIds = self
            .tmdb_get_json(&endpoints.api_base, &path, &[], key)
            .await
            .ok()?;
        let imdb_id = ext
            .imdb_id
            .filter(|id| id.starts_with("tt") && id.len() > 2)?;
        let conn = Arc::clone(&self.conn);
        let imdb_clone = imdb_id.clone();
        run_blocking_tmdb(move || {
            upsert_id_bridge_conn(&conn, tmdb_type, tmdb_id, &imdb_clone);
        })
        .await;
        Some(imdb_id)
    }

    pub async fn build_synthetic_meta_item(
        &self,
        api_key: Option<&str>,
        media_type: &str,
        id: &str,
    ) -> Option<MetaItem> {
        let enriched = if let Some(key) = api_key.map(str::trim).filter(|k| !k.is_empty()) {
            self.enrich_title(key, media_type, id).await.ok().flatten()
        } else {
            self.cached_enriched_meta_with_extra_async(id)
                .await
                .map(|(m, _)| m)
        }?;

        Some(enriched_to_stremio_meta_item(&enriched))
    }

    pub async fn fetch_person_filmography(
        &self,
        api_key: Option<&str>,
        person_key: &str,
        fallback_name: &str,
    ) -> PersonFilmography {
        let reader = Arc::clone(self.reader_conn());
        let pkey_owned = person_key.to_owned();
        let fname_owned = fallback_name.to_owned();
        let local = run_blocking_tmdb(move || {
            let Ok(conn) = reader.lock() else {
                return empty_person_filmography(&pkey_owned, &fname_owned);
            };
            load_local_person_filmography_conn(&conn, &pkey_owned, &fname_owned)
        })
        .await;

        let Some(key) = api_key.map(str::trim).filter(|k| !k.is_empty()) else {
            return local;
        };

        let endpoints = self.current_endpoints();
        let mut tmdb_person_id = person_key
            .strip_prefix("tmdb_person:")
            .and_then(|s| s.parse::<u64>().ok())
            .or(local.tmdb_person_id);

        if tmdb_person_id.is_none() && !local.name.trim().is_empty() {
            if let Ok(search_res) = self
                .tmdb_get_json::<TmdbPersonSearchResponse>(
                    &endpoints.api_base,
                    "/search/person",
                    &[("query", local.name.as_str()), ("include_adult", "false")],
                    key,
                )
                .await
            {
                let target_norm = normalize_title_for_dedup(&local.name);
                tmdb_person_id = search_res
                    .results
                    .iter()
                    .find(|p| normalize_title_for_dedup(&p.name) == target_norm)
                    .or_else(|| search_res.results.first())
                    .map(|p| p.id);
            }
        }

        let Some(pid) = tmdb_person_id else {
            return local;
        };

        let path = format!("/person/{pid}");
        let Ok(detail) = self
            .tmdb_get_json::<TmdbPersonDetailResponse>(
                &endpoints.api_base,
                &path,
                &[("append_to_response", "combined_credits")],
                key,
            )
            .await
        else {
            return local;
        };

        let conn = Arc::clone(&self.conn);
        let reader = Arc::clone(self.reader_conn());
        let hot = Arc::clone(&self.hot);
        let alt_key = person_key.to_owned();
        let image_base = endpoints.image_base;
        run_blocking_tmdb(move || {
            let headshot_url =
                tmdb_img_url(&image_base, "w342", detail.profile_path.clone())
                    .or_else(|| local.headshot_url.clone());

            let mut raw_movies: Vec<EditorialMediaHit> = Vec::new();
            let mut raw_series: Vec<EditorialMediaHit> = Vec::new();
            if let Some(credits) = detail.combined_credits {
                let prefer_crew_first = detail
                    .known_for_department
                    .as_deref()
                    .is_some_and(|d| !d.eq_ignore_ascii_case("Acting"));
                let ordered_entries: Vec<(TmdbPersonCreditEntry, bool)> = if prefer_crew_first {
                    credits
                        .crew
                        .into_iter()
                        .map(|e| (e, false))
                        .chain(credits.cast.into_iter().map(|e| (e, true)))
                        .collect()
                } else {
                    credits
                        .cast
                        .into_iter()
                        .map(|e| (e, true))
                        .chain(credits.crew.into_iter().map(|e| (e, false)))
                        .collect()
                };

                let reader_guard = reader.lock().ok();
                let mut seen_idx: HashMap<(String, u64), usize> = HashMap::new();

                for (entry, is_cast) in ordered_entries {
                    let Some(mt) = entry.media_type.as_deref() else {
                        continue;
                    };
                    if mt != "movie" && mt != "tv" {
                        continue;
                    }
                    let stype = if mt == "tv" { "series" } else { "movie" };
                    let role_hint = if is_cast {
                        entry.character
                    } else {
                        entry.job
                    }
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty());

                    let key_pair = (stype.to_owned(), entry.id);
                    if let Some(&idx) = seen_idx.get(&key_pair) {
                        let target_vec = if stype == "movie" {
                            &mut raw_movies
                        } else {
                            &mut raw_series
                        };
                        if let Some(existing) = target_vec.get_mut(idx) {
                            let prefer_director = !is_cast
                                && role_hint
                                    .as_deref()
                                    .is_some_and(|r| r.eq_ignore_ascii_case("Director"));
                            if (existing.role_hint.is_none() && role_hint.is_some())
                                || prefer_director
                            {
                                existing.role_hint = role_hint;
                            }
                        }
                        continue;
                    }

                    let title = entry
                        .title
                        .or(entry.name)
                        .unwrap_or_default()
                        .trim()
                        .to_owned();
                    if title.is_empty() {
                        continue;
                    }
                    let year = entry
                        .release_date
                        .or(entry.first_air_date)
                        .as_deref()
                        .and_then(extract_4_digit_year);
                    let imdb_id = reader_guard
                        .as_ref()
                        .and_then(|g| lookup_bridged_imdb_conn(g, stype, entry.id));
                    let canonical_id = imdb_id
                        .clone()
                        .unwrap_or_else(|| format!("tmdb:{}", entry.id));
                    let pop_score = entry.popularity.unwrap_or(0.0).round() as i64;
                    let vote = entry
                        .vote_average
                        .filter(|v| *v > 0.1)
                        .map(|v| format!("{v:.1}"));
                    let hit = EditorialMediaHit {
                        canonical_id,
                        imdb_id,
                        tmdb_id: Some(entry.id),
                        media_type: stype.to_owned(),
                        title,
                        year,
                        imdb_rating: vote,
                        overview: entry.overview.filter(|s| !s.trim().is_empty()),
                        poster_url: tmdb_img_url(&image_base, "w342", entry.poster_path),
                        backdrop_url: tmdb_img_url(
                            &image_base,
                            "w780",
                            entry.backdrop_path,
                        ),
                        logo_url: None,
                        genres: Vec::new(),
                        score: pop_score,
                        role_hint,
                    };
                    let target_vec = if stype == "movie" {
                        &mut raw_movies
                    } else {
                        &mut raw_series
                    };
                    seen_idx.insert(key_pair, target_vec.len());
                    target_vec.push(hit);
                }
            }

            raw_movies.sort_by_key(|a| std::cmp::Reverse(a.score));
            raw_series.sort_by_key(|a| std::cmp::Reverse(a.score));

            let merged = build_unified_editorial_search(
                &detail.name,
                [local.movies, local.series].concat(),
                [raw_movies, raw_series].concat(),
                Vec::new(),
                Vec::new(),
                0,
            );

            let updated = PersonFilmography {
                person_key: format!("tmdb_person:{pid}"),
                tmdb_person_id: Some(pid),
                name: detail.name,
                department: detail
                    .known_for_department
                    .unwrap_or(local.department),
                biography: detail.biography.filter(|s| !s.trim().is_empty()),
                birthday: detail.birthday.filter(|s| !s.trim().is_empty()),
                place_of_birth: detail.place_of_birth.filter(|s| !s.trim().is_empty()),
                headshot_url,
                movies: merged.movies,
                series: merged.series,
            };

            persist_person_profile_sync(&conn, &hot, &updated, &alt_key);
            updated
        })
        .await
    }

    async fn resolve_imdb_to_tmdb(
        &self,
        api_key: &str,
        api_base: &str,
        imdb_id: &str,
        preferred_tmdb_type: &str,
    ) -> Result<Option<(&'static str, u64)>, String> {
        {
            let reader = Arc::clone(self.reader_conn());
            let imdb_owned = imdb_id.to_owned();
            if let Some(hit) = run_blocking_tmdb(move || {
                let conn = reader.lock().ok()?;
                let (ttype, tid) = conn
                    .query_row(
                        "SELECT tmdb_type, tmdb_id FROM id_bridge WHERE imdb_id = ?1 LIMIT 1",
                        params![imdb_owned],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
                    )
                    .optional()
                    .ok()
                    .flatten()?;
                let uid = u64::try_from(tid).ok()?;
                let static_type = if ttype == "tv" { "tv" } else { "movie" };
                Some((static_type, uid))
            })
            .await
            {
                return Ok(Some(hit));
            }
        }

        let path = format!("/find/{imdb_id}");
        let find: TmdbFindResponse = self
            .tmdb_get_json(api_base, &path, &[("external_source", "imdb_id")], api_key)
            .await?;

        let chosen = if preferred_tmdb_type == "tv" {
            find.tv_results
                .first()
                .map(|r| ("tv", r.id))
                .or_else(|| find.movie_results.first().map(|r| ("movie", r.id)))
        } else {
            find.movie_results
                .first()
                .map(|r| ("movie", r.id))
                .or_else(|| find.tv_results.first().map(|r| ("tv", r.id)))
        };

        if let Some((ttype, tid)) = chosen {
            let conn = Arc::clone(&self.conn);
            let imdb_owned = imdb_id.to_owned();
            run_blocking_tmdb(move || {
                upsert_id_bridge_conn(&conn, ttype, tid, &imdb_owned);
            })
            .await;
        }
        Ok(chosen)
    }

    async fn tmdb_get_json<T: for<'de> Deserialize<'de>>(
        &self,
        api_base: &str,
        path: &str,
        params_slice: &[(&str, &str)],
        api_key: &str,
    ) -> Result<T, String> {
        let key = api_key.trim();
        let base = api_base.trim_end_matches('/');
        let rel = if path.starts_with('/') {
            path.to_owned()
        } else {
            format!("/{path}")
        };
        let mut url =
            Url::parse(&format!("{base}{rel}")).map_err(|e| format!("Invalid TMDB URL: {e}"))?;
        let use_bearer = is_v4_bearer_token(key);
        {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in params_slice {
                pairs.append_pair(k, v);
            }
            if !use_bearer {
                pairs.append_pair("api_key", key);
            }
        }

        let mut req = self.http.get(url);
        if use_bearer {
            req = req.bearer_auth(key);
        }

        let resp = req.send().await.map_err(|err| {
            let clean = err.without_url().to_string();
            redact_secret(&format!("TMDB request failed: {clean}"), &[key])
        })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(redact_secret(
                &format!("TMDB returned HTTP {}", status.as_u16()),
                &[key],
            ));
        }
        let body = resp.bytes().await.map_err(|err| {
            let clean = err.without_url().to_string();
            redact_secret(&format!("TMDB read failed: {clean}"), &[key])
        })?;
        serde_json::from_slice::<T>(&body).map_err(|e| format!("TMDB JSON decode error: {e}"))
    }

    #[cfg(test)]
    fn load_local_person_filmography(
        &self,
        person_key: &str,
        fallback_name: &str,
    ) -> PersonFilmography {
        let Ok(conn) = self.reader_conn().lock() else {
            return empty_person_filmography(person_key, fallback_name);
        };
        load_local_person_filmography_conn(&conn, person_key, fallback_name)
    }
}

async fn run_blocking_tmdb<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle
            .spawn_blocking(f)
            .await
            .expect("MetadataEngine blocking task panicked")
    } else {
        f()
    }
}

#[derive(Default, Serialize, Deserialize)]
struct ExtraEnrichmentPayload {
    #[serde(default)]
    tagline: Option<String>,
    #[serde(default)]
    certification: Option<String>,
    #[serde(default)]
    trailer_youtube_key: Option<String>,
    #[serde(default)]
    collection: Option<EnrichedCollection>,
    #[serde(default)]
    recommendations: Vec<EditorialMediaHit>,
}

fn lookup_bridged_imdb_conn(conn: &Connection, media_type: &str, tmdb_id: u64) -> Option<String> {
    let tmdb_type = to_tmdb_media_type(media_type);
    conn.query_row(
        "SELECT imdb_id FROM id_bridge WHERE tmdb_type = ?1 AND tmdb_id = ?2",
        params![tmdb_type, tmdb_id as i64],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

fn upsert_id_bridge_conn(conn: &Mutex<Connection>, tmdb_type: &str, tmdb_id: u64, imdb_id: &str) {
    if let Ok(conn) = conn.lock() {
        let _ = conn.execute(
            "INSERT INTO id_bridge (tmdb_type, tmdb_id, imdb_id, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(tmdb_type, tmdb_id) DO UPDATE SET
                 imdb_id = excluded.imdb_id,
                 updated_at = excluded.updated_at",
            params![tmdb_type, tmdb_id as i64, imdb_id, Utc::now().timestamp()],
        );
    }
}

fn load_and_cache_enriched_title(
    conn: &Connection,
    hot: &Mutex<HotMetadataCaches>,
    id: &str,
) -> Option<(EnrichedTitleMeta, bool)> {
    let canonical = resolve_canonical_id_conn(conn, id).unwrap_or_else(|| id.to_owned());
    let (loaded, has_extra) = load_enriched_title_conn(conn, &canonical)?;
    if let Ok(mut hot_guard) = hot.lock() {
        hot_guard.insert_title(id.to_owned(), loaded.clone(), has_extra);
        if canonical != id {
            hot_guard.insert_title(canonical, loaded.clone(), has_extra);
        }
    }
    Some((loaded, has_extra))
}

fn persist_search_hits_sync(
    conn: &Mutex<Connection>,
    hot: &Mutex<HotMetadataCaches>,
    media_hits: &[EditorialMediaHit],
    person_hits: &[EditorialPersonHit],
) {
    let Ok(mut conn) = conn.lock() else {
        return;
    };
    let Ok(tx) = conn.transaction() else {
        return;
    };
    let now = Utc::now().timestamp();
    for hit in media_hits {
        let norm_title = normalize_title_for_dedup(&hit.title);
        let genres_json = serde_json::to_string(&hit.genres).unwrap_or_else(|_| "[]".to_owned());
        let changed = tx
            .prepare_cached(
                "INSERT INTO meta_titles (
                     canonical_id, imdb_id, tmdb_id, media_type, title, norm_title, year, overview,
                     runtime, imdb_rating, genres_json, poster_url, backdrop_url, logo_url,
                     enriched_from_tmdb, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?10, ?11, ?12, ?13, 0, ?14)
                 ON CONFLICT(canonical_id) DO UPDATE SET
                     imdb_id = COALESCE(meta_titles.imdb_id, excluded.imdb_id),
                     tmdb_id = COALESCE(excluded.tmdb_id, meta_titles.tmdb_id),
                     overview = COALESCE(meta_titles.overview, excluded.overview),
                     imdb_rating = COALESCE(meta_titles.imdb_rating, excluded.imdb_rating),
                     poster_url = COALESCE(excluded.poster_url, meta_titles.poster_url),
                     backdrop_url = COALESCE(excluded.backdrop_url, meta_titles.backdrop_url),
                     updated_at = excluded.updated_at",
            )
            .and_then(|mut stmt| {
                stmt.execute(params![
                    hit.canonical_id,
                    hit.imdb_id,
                    hit.tmdb_id.map(|v| v as i64),
                    hit.media_type,
                    hit.title,
                    norm_title,
                    hit.year,
                    hit.overview,
                    hit.imdb_rating,
                    genres_json,
                    hit.poster_url,
                    hit.backdrop_url,
                    hit.logo_url,
                    now,
                ])
            })
            .unwrap_or(0);
        if changed > 0 {
            reindex_fts_title_tx(
                &tx,
                &hit.canonical_id,
                &hit.media_type,
                &hit.title,
                hit.overview.as_deref().unwrap_or(""),
                &hit.genres.join(" "),
            );
        }
    }
    for person in person_hits {
        upsert_person_tx(
            &tx,
            &person.person_key,
            person.tmdb_person_id,
            &person.name,
            &person.department,
            None,
            None,
            None,
            person.headshot_url.as_deref(),
            &person.known_for,
            now,
        );
    }
    if tx.commit().is_ok() {
        if let Ok(mut hot_guard) = hot.lock() {
            hot_guard.invalidate_search();
        }
    }
}

fn select_best_youtube_trailer_key(videos: &[TmdbVideoEntry]) -> Option<String> {
    let mut best: Option<((bool, bool, bool, u64), String)> = None;
    for entry in videos {
        let Some(site) = entry.site.as_deref() else {
            continue;
        };
        if !site.eq_ignore_ascii_case("YouTube") {
            continue;
        }
        let Some(vtype) = entry.video_type.as_deref() else {
            continue;
        };
        let is_trailer = vtype.eq_ignore_ascii_case("Trailer");
        let is_teaser = vtype.eq_ignore_ascii_case("Teaser");
        if !is_trailer && !is_teaser {
            continue;
        }
        let Some(raw_key) = entry.key.as_deref() else {
            continue;
        };
        let key = raw_key.trim();
        if key.is_empty() {
            continue;
        }
        let is_official = entry.official.unwrap_or(false);
        let is_en = entry
            .iso_639_1
            .as_deref()
            .is_some_and(|l| l.eq_ignore_ascii_case("en"));
        let rank = (is_official, is_trailer, is_en, entry.size.unwrap_or(0));
        let replace = best
            .as_ref()
            .is_none_or(|(best_rank, _)| rank > *best_rank);
        if replace {
            best = Some((rank, key.to_owned()));
        }
    }
    best.map(|(_, k)| k)
}

fn best_cert_in_country(country: &TmdbReleaseDateCountry) -> Option<String> {
    let theatrical = country.release_dates.iter().find_map(|rd| {
        if rd.release_type == Some(3) {
            rd.certification
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        } else {
            None
        }
    });
    theatrical.or_else(|| {
        country.release_dates.iter().find_map(|rd| {
            rd.certification
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
    })
}

fn select_movie_certification(countries: &[TmdbReleaseDateCountry]) -> Option<String> {
    if let Some(us) = countries.iter().find(|c| {
        c.iso_3166_1
            .as_deref()
            .is_some_and(|cc| cc.eq_ignore_ascii_case("US"))
    }) {
        if let Some(cert) = best_cert_in_country(us) {
            return Some(cert);
        }
    }
    countries.iter().find_map(best_cert_in_country)
}

fn select_tv_certification(entries: &[TmdbContentRatingEntry]) -> Option<String> {
    if let Some(us_rating) = entries
        .iter()
        .filter(|e| {
            e.iso_3166_1
                .as_deref()
                .is_some_and(|cc| cc.eq_ignore_ascii_case("US"))
        })
        .find_map(|e| {
            e.rating
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
    {
        return Some(us_rating);
    }
    entries.iter().find_map(|e| {
        e.rating
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    })
}

fn store_tmdb_detail_response_sync(
    conn: &Mutex<Connection>,
    hot: &Mutex<HotMetadataCaches>,
    requested_id: &str,
    tmdb_type: &str,
    tmdb_id: u64,
    detail: TmdbDetailResponse,
    image_base: &str,
) -> EnrichedTitleMeta {
    let stremio_type = if tmdb_type == "tv" {
        "series".to_owned()
    } else {
        "movie".to_owned()
    };
    let imdb_id = detail
        .external_ids
        .as_ref()
        .and_then(|e| e.imdb_id.clone())
        .filter(|s| s.starts_with("tt") && s.len() > 2)
        .or_else(|| requested_id.starts_with("tt").then(|| requested_id.to_owned()));
    let canonical_id = imdb_id
        .clone()
        .unwrap_or_else(|| format!("tmdb:{tmdb_id}"));
    let title = detail
        .title
        .or(detail.name)
        .unwrap_or_else(|| canonical_id.clone());
    let year = detail
        .release_date
        .or(detail.first_air_date)
        .as_deref()
        .and_then(extract_4_digit_year);
    let runtime = detail
        .runtime
        .or_else(|| {
            detail
                .episode_run_time
                .as_ref()
                .and_then(|list| list.first().copied())
        })
        .filter(|m| *m > 0)
        .map(|m| format!("{m} min"));
    let imdb_rating = detail
        .vote_average
        .filter(|v| *v > 0.1)
        .map(|v| format!("{v:.1}"));
    let genres: Vec<String> = detail
        .genres
        .unwrap_or_default()
        .into_iter()
        .map(|g| g.name)
        .filter(|s| !s.trim().is_empty())
        .collect();
    let poster_url = tmdb_img_url(image_base, "w342", detail.poster_path);
    let backdrop_url = tmdb_img_url(image_base, "w1280", detail.backdrop_path);
    let logo_url = select_best_tmdb_logo(
        image_base,
        detail
            .images
            .as_ref()
            .map(|imgs| imgs.logos.as_slice())
            .unwrap_or(&[]),
    );
    let tagline = detail
        .tagline
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let certification = if tmdb_type == "tv" {
        detail
            .content_ratings
            .as_ref()
            .and_then(|cr| select_tv_certification(&cr.results))
            .or_else(|| {
                detail
                    .release_dates
                    .as_ref()
                    .and_then(|rd| select_movie_certification(&rd.results))
            })
    } else {
        detail
            .release_dates
            .as_ref()
            .and_then(|rd| select_movie_certification(&rd.results))
            .or_else(|| {
                detail
                    .content_ratings
                    .as_ref()
                    .and_then(|cr| select_tv_certification(&cr.results))
            })
    };
    let trailer_youtube_key = detail
        .videos
        .as_ref()
        .and_then(|v| select_best_youtube_trailer_key(&v.results));
    let collection = detail.belongs_to_collection.and_then(|col| {
        let name = col.name.unwrap_or_default().trim().to_owned();
        if name.is_empty() {
            return None;
        }
        Some(EnrichedCollection {
            id: col.id,
            name,
            overview: None,
            poster_url: tmdb_img_url(image_base, "w342", col.poster_path),
            backdrop_url: tmdb_img_url(image_base, "w780", col.backdrop_path),
            parts: Vec::new(),
        })
    });

    let mut directors = Vec::new();
    let mut seen_directors = HashSet::new();
    if let Some(creators) = detail.created_by {
        for c in creators {
            if seen_directors.insert(c.id) {
                directors.push(PersonCreditChip {
                    person_key: format!("tmdb_person:{}", c.id),
                    tmdb_person_id: Some(c.id),
                    name: c.name,
                    role_or_character: Some("Creator".to_owned()),
                    is_director: true,
                    headshot_url: tmdb_img_url(image_base, "w185", c.profile_path),
                });
            }
        }
    }
    let mut cast = Vec::new();
    if let Some(credits) = detail.credits {
        for crew in credits.crew {
            let job = crew.job.as_deref().unwrap_or("");
            if job.eq_ignore_ascii_case("Director") && seen_directors.insert(crew.id) {
                directors.push(PersonCreditChip {
                    person_key: format!("tmdb_person:{}", crew.id),
                    tmdb_person_id: Some(crew.id),
                    name: crew.name,
                    role_or_character: Some("Director".to_owned()),
                    is_director: true,
                    headshot_url: tmdb_img_url(image_base, "w185", crew.profile_path),
                });
            }
        }
        for actor in credits.cast.into_iter().take(18) {
            cast.push(PersonCreditChip {
                person_key: format!("tmdb_person:{}", actor.id),
                tmdb_person_id: Some(actor.id),
                name: actor.name,
                role_or_character: actor.character.filter(|c| !c.trim().is_empty()),
                is_director: false,
                headshot_url: tmdb_img_url(image_base, "w185", actor.profile_path),
            });
        }
    }

    let mut recommendations = Vec::new();
    let mut seen_rec_keys: HashSet<(String, u64)> = HashSet::new();
    seen_rec_keys.insert((stremio_type.clone(), tmdb_id));
    let rec_iter = detail
        .recommendations
        .map(|b| b.results)
        .unwrap_or_default()
        .into_iter()
        .chain(detail.similar.map(|b| b.results).unwrap_or_default());
    for item in rec_iter {
        if recommendations.len() >= MAX_RECOMMENDATION_HITS {
            break;
        }
        let item_stype = match item.media_type.as_deref() {
            Some(mt) => canonical_stremio_type(mt).to_owned(),
            None => stremio_type.clone(),
        };
        if !seen_rec_keys.insert((item_stype.clone(), item.id)) {
            continue;
        }
        let rec_title = item
            .title
            .or(item.name)
            .unwrap_or_default()
            .trim()
            .to_owned();
        if rec_title.is_empty() {
            continue;
        }
        let rec_year = item
            .release_date
            .or(item.first_air_date)
            .as_deref()
            .and_then(extract_4_digit_year);
        let rec_vote = item
            .vote_average
            .filter(|v| *v > 0.1)
            .map(|v| format!("{v:.1}"));
        recommendations.push(EditorialMediaHit {
            canonical_id: format!("tmdb:{}", item.id),
            imdb_id: None,
            tmdb_id: Some(item.id),
            media_type: item_stype,
            title: rec_title,
            year: rec_year,
            imdb_rating: rec_vote,
            overview: item.overview.filter(|s| !s.trim().is_empty()),
            poster_url: tmdb_img_url(image_base, "w342", item.poster_path),
            backdrop_url: tmdb_img_url(image_base, "w780", item.backdrop_path),
            logo_url: None,
            genres: Vec::new(),
            score: item.popularity.unwrap_or(0.0).round() as i64,
            role_hint: None,
        });
    }

    if let Ok(mut conn_guard) = conn.lock() {
        for rec in &mut recommendations {
            if let Some(tid) = rec.tmdb_id {
                if let Some(imdb) = lookup_bridged_imdb_conn(&conn_guard, &rec.media_type, tid) {
                    rec.canonical_id = imdb.clone();
                    rec.imdb_id = Some(imdb);
                }
            }
        }

        let extra_payload = ExtraEnrichmentPayload {
            tagline: tagline.clone(),
            certification: certification.clone(),
            trailer_youtube_key: trailer_youtube_key.clone(),
            collection: collection.clone(),
            recommendations: recommendations.clone(),
        };
        let extra_json =
            serde_json::to_string(&extra_payload).unwrap_or_else(|_| "{}".to_owned());

        let enriched = EnrichedTitleMeta {
            canonical_id: canonical_id.clone(),
            imdb_id: imdb_id.clone(),
            tmdb_id: Some(tmdb_id),
            media_type: stremio_type,
            title,
            year,
            overview: detail.overview.filter(|s| !s.trim().is_empty()),
            runtime,
            imdb_rating,
            genres,
            poster_url,
            backdrop_url,
            logo_url,
            directors,
            cast,
            tagline,
            certification,
            trailer_youtube_key,
            collection,
            recommendations,
        };

        if let Ok(tx) = conn_guard.transaction() {
            let now = Utc::now().timestamp();
            if let Some(ref imdb) = imdb_id {
                let _ = tx.execute(
                    "INSERT INTO id_bridge (tmdb_type, tmdb_id, imdb_id, updated_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(tmdb_type, tmdb_id) DO UPDATE SET
                         imdb_id = excluded.imdb_id,
                         updated_at = excluded.updated_at",
                    params![tmdb_type, tmdb_id as i64, imdb, now],
                );
                let legacy_tmdb_key = format!("tmdb:{tmdb_id}");
                if canonical_id != legacy_tmdb_key {
                    let _ = tx.execute(
                        "DELETE FROM meta_titles WHERE canonical_id = ?1",
                        params![legacy_tmdb_key],
                    );
                    let _ = tx.execute(
                        "DELETE FROM fts_titles WHERE canonical_id = ?1",
                        params![legacy_tmdb_key],
                    );
                    let _ = tx.execute(
                        "UPDATE OR IGNORE title_credits SET canonical_id = ?1 WHERE canonical_id = ?2",
                        params![canonical_id, legacy_tmdb_key],
                    );
                }
            }

            let norm_title = normalize_title_for_dedup(&enriched.title);
            let genres_json =
                serde_json::to_string(&enriched.genres).unwrap_or_else(|_| "[]".to_owned());
            let _ = tx.execute(
                "INSERT INTO meta_titles (
                     canonical_id, imdb_id, tmdb_id, media_type, title, norm_title, year, overview,
                     runtime, imdb_rating, genres_json, poster_url, backdrop_url, logo_url,
                     enriched_from_tmdb, updated_at, extra_enrichment_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 1, ?15, ?16)
                 ON CONFLICT(canonical_id) DO UPDATE SET
                     imdb_id = COALESCE(excluded.imdb_id, meta_titles.imdb_id),
                     tmdb_id = COALESCE(excluded.tmdb_id, meta_titles.tmdb_id),
                     title = excluded.title,
                     norm_title = excluded.norm_title,
                     year = COALESCE(excluded.year, meta_titles.year),
                     overview = COALESCE(excluded.overview, meta_titles.overview),
                     runtime = COALESCE(excluded.runtime, meta_titles.runtime),
                     imdb_rating = COALESCE(excluded.imdb_rating, meta_titles.imdb_rating),
                     genres_json = CASE WHEN excluded.genres_json != '[]' THEN excluded.genres_json ELSE meta_titles.genres_json END,
                     poster_url = COALESCE(excluded.poster_url, meta_titles.poster_url),
                     backdrop_url = COALESCE(excluded.backdrop_url, meta_titles.backdrop_url),
                     logo_url = COALESCE(excluded.logo_url, meta_titles.logo_url),
                     enriched_from_tmdb = 1,
                     updated_at = excluded.updated_at,
                     extra_enrichment_json = COALESCE(excluded.extra_enrichment_json, meta_titles.extra_enrichment_json)",
                params![
                    enriched.canonical_id,
                    enriched.imdb_id,
                    enriched.tmdb_id.map(|v| v as i64),
                    enriched.media_type,
                    enriched.title,
                    norm_title,
                    enriched.year,
                    enriched.overview,
                    enriched.runtime,
                    enriched.imdb_rating,
                    genres_json,
                    enriched.poster_url,
                    enriched.backdrop_url,
                    enriched.logo_url,
                    now,
                    extra_json,
                ],
            );
            reindex_fts_title_tx(
                &tx,
                &enriched.canonical_id,
                &enriched.media_type,
                &enriched.title,
                enriched.overview.as_deref().unwrap_or(""),
                &enriched.genres.join(" "),
            );

            let _ = tx.execute(
                "DELETE FROM title_credits WHERE canonical_id = ?1",
                params![enriched.canonical_id],
            );
            for (order, dir) in enriched.directors.iter().enumerate() {
                upsert_person_tx(
                    &tx,
                    &dir.person_key,
                    dir.tmdb_person_id,
                    &dir.name,
                    "Directing",
                    None,
                    None,
                    None,
                    dir.headshot_url.as_deref(),
                    std::slice::from_ref(&enriched.title),
                    now,
                );
                let _ = tx.execute(
                    "INSERT OR REPLACE INTO title_credits (canonical_id, person_key, role_kind, character_or_job, credit_order)
                     VALUES (?1, ?2, 'director', ?3, ?4)",
                    params![
                        enriched.canonical_id,
                        dir.person_key,
                        dir.role_or_character.as_deref().unwrap_or("Director"),
                        order as i64,
                    ],
                );
            }
            for (order, actor) in enriched.cast.iter().enumerate() {
                upsert_person_tx(
                    &tx,
                    &actor.person_key,
                    actor.tmdb_person_id,
                    &actor.name,
                    "Acting",
                    None,
                    None,
                    None,
                    actor.headshot_url.as_deref(),
                    std::slice::from_ref(&enriched.title),
                    now,
                );
                let _ = tx.execute(
                    "INSERT OR REPLACE INTO title_credits (canonical_id, person_key, role_kind, character_or_job, credit_order)
                     VALUES (?1, ?2, 'cast', ?3, ?4)",
                    params![
                        enriched.canonical_id,
                        actor.person_key,
                        actor.role_or_character,
                        order as i64,
                    ],
                );
            }
            let _ = tx.commit();
        }

        if let Ok(mut hot_guard) = hot.lock() {
            hot_guard.invalidate_search();
            hot_guard.insert_title(canonical_id.clone(), enriched.clone(), true);
            if requested_id != canonical_id {
                hot_guard.insert_title(requested_id.to_owned(), enriched.clone(), true);
            }
        }
        return enriched;
    }

    let enriched = EnrichedTitleMeta {
        canonical_id: canonical_id.clone(),
        imdb_id,
        tmdb_id: Some(tmdb_id),
        media_type: stremio_type,
        title,
        year,
        overview: detail.overview.filter(|s| !s.trim().is_empty()),
        runtime,
        imdb_rating,
        genres,
        poster_url,
        backdrop_url,
        logo_url,
        directors,
        cast,
        tagline,
        certification,
        trailer_youtube_key,
        collection,
        recommendations,
    };
    if let Ok(mut hot_guard) = hot.lock() {
        hot_guard.invalidate_search();
        hot_guard.insert_title(canonical_id.clone(), enriched.clone(), true);
        if requested_id != canonical_id {
            hot_guard.insert_title(requested_id.to_owned(), enriched.clone(), true);
        }
    }
    enriched
}

fn load_collection_conn(conn: &Connection, collection_id: u64) -> Option<EnrichedCollection> {
    conn.query_row(
        "SELECT collection_id, name, overview, poster_url, backdrop_url, parts_json
         FROM meta_collections
         WHERE collection_id = ?1
         LIMIT 1",
        params![collection_id as i64],
        |r| {
            let cid: i64 = r.get(0)?;
            let parts_raw: String = r.get(5)?;
            let parts: Vec<EditorialMediaHit> =
                serde_json::from_str(&parts_raw).unwrap_or_default();
            Ok(EnrichedCollection {
                id: u64::try_from(cid).unwrap_or(collection_id),
                name: r.get(1)?,
                overview: r.get(2)?,
                poster_url: r.get(3)?,
                backdrop_url: r.get(4)?,
                parts,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

fn store_tmdb_collection_response_sync(
    conn: &Mutex<Connection>,
    hot: &Mutex<HotMetadataCaches>,
    collection_id: u64,
    resp: TmdbCollectionResponse,
    image_base: &str,
) -> EnrichedCollection {
    let mut parts_with_dates: Vec<(Option<String>, EditorialMediaHit)> = Vec::new();
    let mut seen_part_ids = HashSet::new();
    let conn_guard = conn.lock().ok();

    for part in resp.parts {
        if !seen_part_ids.insert(part.id) {
            continue;
        }
        let stype = part
            .media_type
            .as_deref()
            .map(canonical_stremio_type)
            .unwrap_or("movie")
            .to_owned();
        let title = part
            .title
            .or(part.name)
            .unwrap_or_default()
            .trim()
            .to_owned();
        if title.is_empty() {
            continue;
        }
        let raw_date = part
            .release_date
            .or(part.first_air_date)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        let year = raw_date.as_deref().and_then(extract_4_digit_year);
        let imdb_id = conn_guard
            .as_ref()
            .and_then(|g| lookup_bridged_imdb_conn(g, &stype, part.id));
        let canonical_id = imdb_id
            .clone()
            .unwrap_or_else(|| format!("tmdb:{}", part.id));
        let vote = part
            .vote_average
            .filter(|v| *v > 0.1)
            .map(|v| format!("{v:.1}"));
        let hit = EditorialMediaHit {
            canonical_id,
            imdb_id,
            tmdb_id: Some(part.id),
            media_type: stype,
            title,
            year,
            imdb_rating: vote,
            overview: part.overview.filter(|s| !s.trim().is_empty()),
            poster_url: tmdb_img_url(image_base, "w342", part.poster_path),
            backdrop_url: tmdb_img_url(image_base, "w780", part.backdrop_path),
            logo_url: None,
            genres: Vec::new(),
            score: part.popularity.unwrap_or(0.0).round() as i64,
            role_hint: None,
        };
        parts_with_dates.push((raw_date, hit));
    }

    parts_with_dates.sort_by(|(da, ha), (db, hb)| match (da.as_deref(), db.as_deref()) {
        (Some(a), Some(b)) => a.cmp(b).then_with(|| ha.title.cmp(&hb.title)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => ha.title.cmp(&hb.title),
    });

    let parts: Vec<EditorialMediaHit> = parts_with_dates.into_iter().map(|(_, h)| h).collect();
    let enriched = EnrichedCollection {
        id: resp.id.unwrap_or(collection_id),
        name: resp.name.unwrap_or_else(|| format!("Collection #{collection_id}")),
        overview: resp.overview.filter(|s| !s.trim().is_empty()),
        poster_url: tmdb_img_url(image_base, "w342", resp.poster_path),
        backdrop_url: tmdb_img_url(image_base, "w780", resp.backdrop_path),
        parts,
    };

    if let Some(guard) = conn_guard {
        let parts_json =
            serde_json::to_string(&enriched.parts).unwrap_or_else(|_| "[]".to_owned());
        let now = Utc::now().timestamp();
        let _ = guard.execute(
            "INSERT INTO meta_collections (
                 collection_id, name, overview, poster_url, backdrop_url, parts_json, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(collection_id) DO UPDATE SET
                 name = excluded.name,
                 overview = excluded.overview,
                 poster_url = excluded.poster_url,
                 backdrop_url = excluded.backdrop_url,
                 parts_json = excluded.parts_json,
                 updated_at = excluded.updated_at",
            params![
                enriched.id as i64,
                enriched.name,
                enriched.overview,
                enriched.poster_url,
                enriched.backdrop_url,
                parts_json,
                now,
            ],
        );
    }

    if let Ok(mut hot_guard) = hot.lock() {
        hot_guard.insert_collection(enriched.id, enriched.clone());
    }
    enriched
}

fn load_local_person_filmography_conn(
    conn: &Connection,
    person_key: &str,
    fallback_name: &str,
) -> PersonFilmography {
    let person_row = conn
        .query_row(
            "SELECT person_key, tmdb_person_id, name, department, biography, birthday, place_of_birth, headshot_url
             FROM meta_people
             WHERE person_key = ?1 OR norm_name = ?2
             ORDER BY CASE WHEN person_key = ?1 THEN 0 ELSE 1 END, updated_at DESC
             LIMIT 1",
            params![person_key, normalize_title_for_dedup(fallback_name)],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()
        .ok()
        .flatten();

    let (resolved_key, tmdb_pid, name, department, bio, bday, birthplace, headshot) =
        match person_row {
            Some((pk, pid, n, dept, bio, bday, pob, head)) => (
                pk,
                pid.and_then(|v| u64::try_from(v).ok()),
                n,
                dept,
                bio,
                bday,
                pob,
                head,
            ),
            None => (
                person_key.to_owned(),
                person_key
                    .strip_prefix("tmdb_person:")
                    .and_then(|s| s.parse::<u64>().ok()),
                fallback_name.to_owned(),
                "Acting".to_owned(),
                None,
                None,
                None,
                None,
            ),
        };

    let norm_name = normalize_title_for_dedup(&name);
    let mut movies = Vec::new();
    let mut series = Vec::new();
    let mut seen_ids = HashSet::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT m.canonical_id, m.imdb_id, m.tmdb_id, m.media_type, m.title, m.year,
                m.imdb_rating, m.overview, m.poster_url, m.backdrop_url, m.logo_url, m.genres_json,
                c.character_or_job
         FROM title_credits c
         JOIN meta_people p ON p.person_key = c.person_key
         JOIN meta_titles m ON m.canonical_id = c.canonical_id
         WHERE c.person_key = ?1 OR c.person_key = ?2 OR p.norm_name = ?3
         ORDER BY m.year DESC, m.updated_at DESC, CASE WHEN c.role_kind = 'director' THEN 0 ELSE 1 END
         LIMIT 64",
    ) {
        if let Ok(rows) = stmt.query_map(params![person_key, resolved_key, norm_name], |r| {
            let genres_raw: String = r.get(11)?;
            let genres: Vec<String> = serde_json::from_str(&genres_raw).unwrap_or_default();
            let tmdb_id_raw: Option<i64> = r.get(2)?;
            let role_hint: Option<String> = r
                .get::<_, Option<String>>(12)?
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty());
            Ok(EditorialMediaHit {
                canonical_id: r.get(0)?,
                imdb_id: r.get(1)?,
                tmdb_id: tmdb_id_raw.and_then(|v| u64::try_from(v).ok()),
                media_type: r.get(3)?,
                title: r.get(4)?,
                year: r.get(5)?,
                imdb_rating: r.get(6)?,
                overview: r.get(7)?,
                poster_url: r.get(8)?,
                backdrop_url: r.get(9)?,
                logo_url: r.get(10)?,
                genres,
                score: 10,
                role_hint,
            })
        }) {
            for hit in rows.flatten() {
                if !seen_ids.insert(hit.canonical_id.clone()) {
                    continue;
                }
                if canonical_stremio_type(&hit.media_type) == "movie" {
                    movies.push(hit);
                } else {
                    series.push(hit);
                }
            }
        }
    }

    PersonFilmography {
        person_key: resolved_key,
        tmdb_person_id: tmdb_pid,
        name,
        department,
        biography: bio,
        birthday: bday,
        place_of_birth: birthplace,
        headshot_url: headshot,
        movies,
        series,
    }
}

fn persist_person_profile_sync(
    conn: &Mutex<Connection>,
    hot: &Mutex<HotMetadataCaches>,
    filmography: &PersonFilmography,
    alt_key: &str,
) {
    let Ok(mut conn) = conn.lock() else {
        return;
    };
    let Ok(tx) = conn.transaction() else {
        return;
    };
    let now = Utc::now().timestamp();
    let known_for: Vec<String> = filmography
        .movies
        .iter()
        .chain(filmography.series.iter())
        .map(|h| h.title.clone())
        .take(4)
        .collect();
    upsert_person_tx(
        &tx,
        &filmography.person_key,
        filmography.tmdb_person_id,
        &filmography.name,
        &filmography.department,
        filmography.biography.as_deref(),
        filmography.birthday.as_deref(),
        filmography.place_of_birth.as_deref(),
        filmography.headshot_url.as_deref(),
        &known_for,
        now,
    );
    if alt_key != filmography.person_key && !alt_key.trim().is_empty() {
        let _ = tx.execute(
            "UPDATE OR IGNORE title_credits SET person_key = ?1 WHERE person_key = ?2",
            params![filmography.person_key, alt_key],
        );
    }
    if tx.commit().is_ok() {
        if let Ok(mut hot_guard) = hot.lock() {
            hot_guard.invalidate_search();
        }
    }
}

fn empty_person_filmography(person_key: &str, name: &str) -> PersonFilmography {
    PersonFilmography {
        person_key: person_key.to_owned(),
        tmdb_person_id: person_key
            .strip_prefix("tmdb_person:")
            .and_then(|s| s.parse::<u64>().ok()),
        name: name.to_owned(),
        department: "Acting".to_owned(),
        biography: None,
        birthday: None,
        place_of_birth: None,
        headshot_url: None,
        movies: Vec::new(),
        series: Vec::new(),
    }
}

fn resolve_canonical_id_conn(conn: &Connection, id: &str) -> Option<String> {
    if let Some(raw_num) = id.strip_prefix("tmdb:") {
        if let Ok(num) = raw_num.parse::<i64>() {
            if let Ok(Some(imdb)) = conn
                .query_row(
                    "SELECT imdb_id FROM id_bridge WHERE tmdb_id = ?1 LIMIT 1",
                    params![num],
                    |r| r.get::<_, String>(0),
                )
                .optional()
            {
                return Some(imdb);
            }
        }
    }
    Some(id.to_owned())
}

fn load_enriched_title_conn(
    conn: &Connection,
    canonical_id: &str,
) -> Option<(EnrichedTitleMeta, bool)> {
    let (row, has_extra) = conn
        .query_row(
            "SELECT canonical_id, imdb_id, tmdb_id, media_type, title, year, overview, runtime,
                    imdb_rating, genres_json, poster_url, backdrop_url, logo_url, extra_enrichment_json
             FROM meta_titles
             WHERE canonical_id = ?1 OR imdb_id = ?1
             LIMIT 1",
            params![canonical_id],
            |r| {
                let genres_raw: String = r.get(9)?;
                let genres: Vec<String> = serde_json::from_str(&genres_raw).unwrap_or_default();
                let tmdb_raw: Option<i64> = r.get(2)?;
                let extra_raw: Option<String> = r.get(13)?;
                let has_extra = extra_raw.is_some();
                let extra: ExtraEnrichmentPayload = extra_raw
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();
                Ok((
                    EnrichedTitleMeta {
                        canonical_id: r.get(0)?,
                        imdb_id: r.get(1)?,
                        tmdb_id: tmdb_raw.and_then(|v| u64::try_from(v).ok()),
                        media_type: r.get(3)?,
                        title: r.get(4)?,
                        year: r.get(5)?,
                        overview: r.get(6)?,
                        runtime: r.get(7)?,
                        imdb_rating: r.get(8)?,
                        genres,
                        poster_url: r.get(10)?,
                        backdrop_url: r.get(11)?,
                        logo_url: r.get(12)?,
                        directors: Vec::new(),
                        cast: Vec::new(),
                        tagline: extra.tagline,
                        certification: extra.certification,
                        trailer_youtube_key: extra.trailer_youtube_key,
                        collection: extra.collection,
                        recommendations: extra.recommendations,
                    },
                    has_extra,
                ))
            },
        )
        .optional()
        .ok()
        .flatten()?;

    let mut directors = Vec::new();
    let mut cast = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT c.person_key, p.tmdb_person_id, p.name, c.role_kind, c.character_or_job, p.headshot_url
         FROM title_credits c
         JOIN meta_people p ON p.person_key = c.person_key
         WHERE c.canonical_id = ?1
         ORDER BY c.role_kind ASC, c.credit_order ASC",
    ) {
        if let Ok(rows) = stmt.query_map(params![row.canonical_id], |r| {
            let tmdb_pid: Option<i64> = r.get(1)?;
            let role_kind: String = r.get(3)?;
            Ok(PersonCreditChip {
                person_key: r.get(0)?,
                tmdb_person_id: tmdb_pid.and_then(|v| u64::try_from(v).ok()),
                name: r.get(2)?,
                role_or_character: r.get(4)?,
                is_director: role_kind == "director",
                headshot_url: r.get(5)?,
            })
        }) {
            for chip in rows.flatten() {
                if chip.is_director {
                    directors.push(chip);
                } else {
                    cast.push(chip);
                }
            }
        }
    }

    Some((
        EnrichedTitleMeta {
            directors,
            cast,
            ..row
        },
        has_extra,
    ))
}

fn upsert_preview_tx(tx: &rusqlite::Transaction<'_>, preview: &MetaItemPreview, now: i64) -> bool {
    if preview.id.trim().is_empty() || preview.name.trim().is_empty() {
        return false;
    }
    let stremio_type = canonical_stremio_type(&preview.r#type).to_owned();
    let imdb_id = preview.id.starts_with("tt").then(|| preview.id.clone());
    let tmdb_id = preview
        .id
        .strip_prefix("tmdb:")
        .and_then(|s| s.parse::<u64>().ok());
    let norm_title = normalize_title_for_dedup(&preview.name);
    let year = preview
        .release_info
        .as_deref()
        .and_then(extract_4_digit_year);
    let imdb_rating = preview
        .links
        .iter()
        .find(|l| l.category == IMDB_LINK_CATEGORY || l.category.eq_ignore_ascii_case("imdb"))
        .map(|l| l.name.trim().to_owned())
        .filter(|s| !s.is_empty());
    let genres: Vec<String> = preview
        .links
        .iter()
        .filter(|l| {
            l.category.eq_ignore_ascii_case(GENRES_LINK_CATEGORY)
                || l.category.eq_ignore_ascii_case("genre")
        })
        .map(|l| l.name.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    let genres_json = serde_json::to_string(&genres).unwrap_or_else(|_| "[]".to_owned());
    let poster_url = preview.poster.as_ref().map(Url::to_string);
    let backdrop_url = preview.background.as_ref().map(Url::to_string);
    let logo_url = preview.logo.as_ref().map(Url::to_string);

    let title_changed = tx
        .prepare_cached(
            "INSERT INTO meta_titles (
                 canonical_id, imdb_id, tmdb_id, media_type, title, norm_title, year, overview,
                 runtime, imdb_rating, genres_json, poster_url, backdrop_url, logo_url,
                 enriched_from_tmdb, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 0, ?15)
             ON CONFLICT(canonical_id) DO UPDATE SET
                 imdb_id = COALESCE(meta_titles.imdb_id, excluded.imdb_id),
                 tmdb_id = COALESCE(meta_titles.tmdb_id, excluded.tmdb_id),
                 year = COALESCE(meta_titles.year, excluded.year),
                 overview = COALESCE(meta_titles.overview, excluded.overview),
                 runtime = COALESCE(meta_titles.runtime, excluded.runtime),
                 imdb_rating = COALESCE(meta_titles.imdb_rating, excluded.imdb_rating),
                 genres_json = CASE WHEN meta_titles.genres_json != '[]' THEN meta_titles.genres_json ELSE excluded.genres_json END,
                 poster_url = COALESCE(meta_titles.poster_url, excluded.poster_url),
                 backdrop_url = COALESCE(meta_titles.backdrop_url, excluded.backdrop_url),
                 logo_url = COALESCE(meta_titles.logo_url, excluded.logo_url),
                 updated_at = excluded.updated_at
             WHERE
                 (meta_titles.imdb_id IS NULL AND excluded.imdb_id IS NOT NULL)
                 OR (meta_titles.tmdb_id IS NULL AND excluded.tmdb_id IS NOT NULL)
                 OR (meta_titles.year IS NULL AND excluded.year IS NOT NULL)
                 OR (meta_titles.overview IS NULL AND excluded.overview IS NOT NULL)
                 OR (meta_titles.runtime IS NULL AND excluded.runtime IS NOT NULL)
                 OR (meta_titles.imdb_rating IS NULL AND excluded.imdb_rating IS NOT NULL)
                 OR (meta_titles.genres_json = '[]' AND excluded.genres_json != '[]')
                 OR (meta_titles.poster_url IS NULL AND excluded.poster_url IS NOT NULL)
                 OR (meta_titles.backdrop_url IS NULL AND excluded.backdrop_url IS NOT NULL)
                 OR (meta_titles.logo_url IS NULL AND excluded.logo_url IS NOT NULL)",
        )
        .and_then(|mut stmt| {
            stmt.execute(params![
                preview.id,
                imdb_id,
                tmdb_id.map(|v| v as i64),
                stremio_type,
                preview.name,
                norm_title,
                year,
                preview.description,
                preview.runtime,
                imdb_rating,
                genres_json,
                poster_url,
                backdrop_url,
                logo_url,
                now,
            ])
        })
        .unwrap_or(0)
        > 0;

    if title_changed {
        reindex_fts_title_tx(
            tx,
            &preview.id,
            &stremio_type,
            &preview.name,
            preview.description.as_deref().unwrap_or(""),
            &genres.join(" "),
        );
    }

    let mut credits_changed = false;
    let mut director_order = 0i64;
    let mut cast_order = 0i64;
    for link in &preview.links {
        let cat = link.category.trim();
        let name = link.name.trim();
        if name.is_empty() {
            continue;
        }
        let is_director = cat.eq_ignore_ascii_case("Directors")
            || cat.eq_ignore_ascii_case("Director")
            || cat.eq_ignore_ascii_case("Writers")
            || cat.eq_ignore_ascii_case("Writer");
        let is_cast = cat.eq_ignore_ascii_case("Cast") || cat.eq_ignore_ascii_case("Actors");
        if !is_director && !is_cast {
            continue;
        }
        let slug = normalize_person_slug(name);
        if slug.is_empty() {
            continue;
        }
        let person_key = format!("name:{slug}");
        let dept = if is_director { "Directing" } else { "Acting" };
        if upsert_person_tx(
            tx,
            &person_key,
            None,
            name,
            dept,
            None,
            None,
            None,
            None,
            std::slice::from_ref(&preview.name),
            now,
        ) {
            credits_changed = true;
        }
        let (role_kind, ord) = if is_director {
            let o = director_order;
            director_order += 1;
            ("director", o)
        } else {
            let o = cast_order;
            cast_order += 1;
            ("cast", o)
        };
        let credit_inserted = tx
            .prepare_cached(
                "INSERT OR IGNORE INTO title_credits (canonical_id, person_key, role_kind, character_or_job, credit_order)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .and_then(|mut stmt| {
                stmt.execute(params![
                    preview.id,
                    person_key,
                    role_kind,
                    if is_director { Some("Director") } else { None },
                    ord,
                ])
            })
            .unwrap_or(0)
            > 0;
        if credit_inserted {
            credits_changed = true;
        }
    }

    title_changed || credits_changed
}

#[allow(clippy::too_many_arguments)]
fn upsert_person_tx(
    tx: &rusqlite::Transaction<'_>,
    person_key: &str,
    tmdb_person_id: Option<u64>,
    name: &str,
    department: &str,
    biography: Option<&str>,
    birthday: Option<&str>,
    place_of_birth: Option<&str>,
    headshot_url: Option<&str>,
    known_for: &[String],
    now: i64,
) -> bool {
    let norm_name = normalize_title_for_dedup(name);
    if norm_name.is_empty() {
        return false;
    }
    let known_json = serde_json::to_string(known_for).unwrap_or_else(|_| "[]".to_owned());
    let known_text = known_for.join(" ");
    let changed = tx
        .prepare_cached(
            "INSERT INTO meta_people (
                 person_key, tmdb_person_id, name, norm_name, department, biography, birthday,
                 place_of_birth, headshot_url, known_for_json, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(person_key) DO UPDATE SET
                 tmdb_person_id = COALESCE(excluded.tmdb_person_id, meta_people.tmdb_person_id),
                 department = COALESCE(NULLIF(excluded.department, ''), meta_people.department),
                 biography = COALESCE(excluded.biography, meta_people.biography),
                 birthday = COALESCE(excluded.birthday, meta_people.birthday),
                 place_of_birth = COALESCE(excluded.place_of_birth, meta_people.place_of_birth),
                 headshot_url = COALESCE(excluded.headshot_url, meta_people.headshot_url),
                 known_for_json = CASE WHEN meta_people.known_for_json != '[]' THEN meta_people.known_for_json ELSE excluded.known_for_json END,
                 updated_at = excluded.updated_at
             WHERE
                 (excluded.tmdb_person_id IS NOT NULL AND meta_people.tmdb_person_id IS NOT excluded.tmdb_person_id)
                 OR (NULLIF(excluded.department, '') IS NOT NULL AND meta_people.department IS NOT excluded.department)
                 OR (excluded.biography IS NOT NULL AND meta_people.biography IS NOT excluded.biography)
                 OR (excluded.birthday IS NOT NULL AND meta_people.birthday IS NOT excluded.birthday)
                 OR (excluded.place_of_birth IS NOT NULL AND meta_people.place_of_birth IS NOT excluded.place_of_birth)
                 OR (excluded.headshot_url IS NOT NULL AND meta_people.headshot_url IS NOT excluded.headshot_url)
                 OR (meta_people.known_for_json = '[]' AND excluded.known_for_json != '[]')",
        )
        .and_then(|mut stmt| {
            stmt.execute(params![
                person_key,
                tmdb_person_id.map(|v| v as i64),
                name,
                norm_name,
                department,
                biography,
                birthday,
                place_of_birth,
                headshot_url,
                known_json,
                now,
            ])
        })
        .unwrap_or(0)
        > 0;

    if changed {
        if let Ok(mut stmt) = tx.prepare_cached("DELETE FROM fts_people WHERE person_key = ?1") {
            let _ = stmt.execute(params![person_key]);
        }
        if let Ok(mut stmt) = tx.prepare_cached(
            "INSERT INTO fts_people (person_key, name, known_for_text) VALUES (?1, ?2, ?3)",
        ) {
            let _ = stmt.execute(params![person_key, name, known_text]);
        }
    }
    changed
}

fn reindex_fts_title_tx(
    tx: &rusqlite::Transaction<'_>,
    canonical_id: &str,
    media_type: &str,
    title: &str,
    overview: &str,
    genres_text: &str,
) {
    if let Ok(mut stmt) = tx.prepare_cached("DELETE FROM fts_titles WHERE canonical_id = ?1") {
        let _ = stmt.execute(params![canonical_id]);
    }
    if let Ok(mut stmt) = tx.prepare_cached(
        "INSERT INTO fts_titles (canonical_id, media_type, title, overview, genres_text)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    ) {
        let _ = stmt.execute(params![canonical_id, media_type, title, overview, genres_text]);
    }
}

pub fn build_fts5_match_expr(query: &str) -> Option<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    for ch in query.chars() {
        if ch.is_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            terms.push(format!("\"{}\"*", current));
            current.clear();
        }
    }
    if !current.is_empty() {
        terms.push(format!("\"{current}\"*"));
    }
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
}

pub fn normalize_title_for_dedup(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_space = true;
    for ch in raw.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim().to_owned()
}

pub fn normalize_person_slug(name: &str) -> String {
    normalize_title_for_dedup(name).replace(' ', "_")
}

pub fn extract_4_digit_year(raw: &str) -> Option<String> {
    YEAR_4_RE.find(raw).map(|m| m.as_str().to_owned())
}

pub fn canonical_stremio_type(media_type: &str) -> &'static str {
    let lower = media_type.trim().to_ascii_lowercase();
    if lower == "series" || lower == "tv" {
        "series"
    } else {
        "movie"
    }
}

pub fn to_tmdb_media_type(media_type: &str) -> &'static str {
    let lower = media_type.trim().to_ascii_lowercase();
    if lower == "series" || lower == "tv" {
        "tv"
    } else {
        "movie"
    }
}

pub fn is_v4_bearer_token(key: &str) -> bool {
    let k = key.trim();
    k.starts_with("eyJ") || k.contains('.') || k.len() > 64
}

pub fn score_title_relevance(title: &str, norm_query: &str) -> i64 {
    if norm_query.is_empty() {
        return 10;
    }
    let norm_t = normalize_title_for_dedup(title);
    if norm_t == norm_query {
        1_000
    } else if norm_t.starts_with(norm_query) {
        700 - (norm_t.len().saturating_sub(norm_query.len()) as i64).min(150)
    } else if norm_t.split_whitespace().any(|w| w.starts_with(norm_query)) {
        480
    } else if norm_t.contains(norm_query) {
        320
    } else {
        120
    }
}

pub fn preview_to_editorial_hit(preview: &MetaItemPreview, norm_query: &str, boost: i64) -> EditorialMediaHit {
    let imdb_id = preview.id.starts_with("tt").then(|| preview.id.clone());
    let tmdb_id = preview
        .id
        .strip_prefix("tmdb:")
        .and_then(|s| s.parse::<u64>().ok());
    let imdb_rating = preview
        .links
        .iter()
        .find(|l| l.category == IMDB_LINK_CATEGORY || l.category.eq_ignore_ascii_case("imdb"))
        .map(|l| l.name.clone());
    let genres = preview
        .links
        .iter()
        .filter(|l| {
            l.category.eq_ignore_ascii_case(GENRES_LINK_CATEGORY)
                || l.category.eq_ignore_ascii_case("genre")
        })
        .map(|l| l.name.clone())
        .take(4)
        .collect();
    EditorialMediaHit {
        canonical_id: preview.id.clone(),
        imdb_id,
        tmdb_id,
        media_type: canonical_stremio_type(&preview.r#type).to_owned(),
        title: preview.name.clone(),
        year: preview
            .release_info
            .as_deref()
            .and_then(extract_4_digit_year),
        imdb_rating,
        overview: preview.description.clone(),
        poster_url: preview.poster.as_ref().map(Url::to_string),
        backdrop_url: preview.background.as_ref().map(Url::to_string),
        logo_url: preview.logo.as_ref().map(Url::to_string),
        genres,
        score: score_title_relevance(&preview.name, norm_query) + boost,
        role_hint: None,
    }
}

pub fn build_unified_editorial_search(
    query: &str,
    local_fts_hits: Vec<EditorialMediaHit>,
    tmdb_hits: Vec<EditorialMediaHit>,
    addon_and_library_hits: Vec<EditorialMediaHit>,
    people_hits: Vec<EditorialPersonHit>,
    elapsed_us: u64,
) -> UnifiedEditorialSearch {
    let mut merged: Vec<EditorialMediaHit> = Vec::new();
    let mut by_imdb: HashMap<String, usize> = HashMap::new();
    let mut by_tmdb: HashMap<(String, u64), usize> = HashMap::new();
    let mut by_norm: HashMap<(String, String, Option<String>), usize> = HashMap::new();

    for cand in tmdb_hits
        .into_iter()
        .chain(local_fts_hits)
        .chain(addon_and_library_hits)
    {
        if cand.title.trim().is_empty() {
            continue;
        }
        let stype = canonical_stremio_type(&cand.media_type).to_owned();
        let norm_t = normalize_title_for_dedup(&cand.title);
        let year = cand.year.clone();
        let imdb_opt = cand
            .imdb_id
            .clone()
            .or_else(|| cand.canonical_id.starts_with("tt").then(|| cand.canonical_id.clone()));

        let existing_idx = imdb_opt
            .as_ref()
            .and_then(|id| by_imdb.get(id).copied())
            .or_else(|| {
                cand.tmdb_id
                    .and_then(|tid| by_tmdb.get(&(stype.clone(), tid)).copied())
            })
            .or_else(|| {
                by_norm
                    .get(&(stype.clone(), norm_t.clone(), year.clone()))
                    .copied()
            })
            .or_else(|| {
                // Also match an untyped-year entry against one with year for the exact same normalized title
                if year.is_some() {
                    by_norm.get(&(stype.clone(), norm_t.clone(), None)).copied()
                } else {
                    None
                }
            });

        if let Some(idx) = existing_idx {
            let slot = &mut merged[idx];
            if slot.imdb_id.is_none() {
                if let Some(ref imdb) = imdb_opt {
                    slot.imdb_id = Some(imdb.clone());
                    if !slot.canonical_id.starts_with("tt") {
                        slot.canonical_id = imdb.clone();
                    }
                    by_imdb.insert(imdb.clone(), idx);
                }
            }
            if slot.tmdb_id.is_none() {
                if let Some(tid) = cand.tmdb_id {
                    slot.tmdb_id = Some(tid);
                    by_tmdb.insert((stype.clone(), tid), idx);
                }
            }
            if slot.year.is_none() && year.is_some() {
                slot.year = year.clone();
                by_norm.insert((stype.clone(), norm_t.clone(), year), idx);
            }
            if slot.imdb_rating.is_none() {
                slot.imdb_rating = cand.imdb_rating;
            }
            if slot.overview.as_deref().map(str::len).unwrap_or(0)
                < cand.overview.as_deref().map(str::len).unwrap_or(0)
            {
                slot.overview = cand.overview;
            }
            if slot.genres.is_empty() && !cand.genres.is_empty() {
                slot.genres = cand.genres;
            }
            if slot.role_hint.is_none() {
                slot.role_hint = cand.role_hint;
            }
            slot.logo_url = pick_preferred_art_url(slot.logo_url.take(), cand.logo_url);
            slot.backdrop_url =
                pick_preferred_art_url(slot.backdrop_url.take(), cand.backdrop_url);
            slot.poster_url = pick_preferred_art_url(slot.poster_url.take(), cand.poster_url);
            slot.score = slot.score.max(cand.score) + 25;
        } else {
            let idx = merged.len();
            let mut item = cand;
            item.media_type = stype.clone();
            if let Some(ref imdb) = imdb_opt {
                item.imdb_id = Some(imdb.clone());
                if !item.canonical_id.starts_with("tt") {
                    item.canonical_id = imdb.clone();
                }
                by_imdb.insert(imdb.clone(), idx);
            }
            if let Some(tid) = item.tmdb_id {
                by_tmdb.insert((stype.clone(), tid), idx);
            }
            by_norm.insert((stype, norm_t, year), idx);
            merged.push(item);
        }
    }

    merged.sort_by_key(|a| std::cmp::Reverse(a.score));

    let top_match = merged.first().cloned();
    let mut movies = Vec::new();
    let mut series = Vec::new();
    for item in merged {
        if item.media_type == "movie" {
            movies.push(item);
        } else {
            series.push(item);
        }
    }

    let mut dedup_people: Vec<EditorialPersonHit> = Vec::new();
    let mut seen_person_tmdb: HashMap<u64, usize> = HashMap::new();
    let mut seen_person_slug: HashMap<String, usize> = HashMap::new();
    for p in people_hits {
        if p.name.trim().is_empty() {
            continue;
        }
        let slug = normalize_person_slug(&p.name);
        let existing_idx = p
            .tmdb_person_id
            .and_then(|id| seen_person_tmdb.get(&id).copied())
            .or_else(|| seen_person_slug.get(&slug).copied());
        if let Some(idx) = existing_idx {
            let slot = &mut dedup_people[idx];
            if slot.tmdb_person_id.is_none() && p.tmdb_person_id.is_some() {
                slot.tmdb_person_id = p.tmdb_person_id;
                slot.person_key = p.person_key;
            }
            if slot.headshot_url.is_none() {
                slot.headshot_url = p.headshot_url;
            }
            if slot.known_for.is_empty() && !p.known_for.is_empty() {
                slot.known_for = p.known_for;
            }
        } else {
            let idx = dedup_people.len();
            if let Some(id) = p.tmdb_person_id {
                seen_person_tmdb.insert(id, idx);
            }
            seen_person_slug.insert(slug, idx);
            dedup_people.push(p);
        }
    }

    UnifiedEditorialSearch {
        query: query.trim().to_owned(),
        top_match,
        movies,
        series,
        people: dedup_people,
        elapsed_us,
    }
}

fn pick_preferred_art_url(existing: Option<String>, candidate: Option<String>) -> Option<String> {
    match (existing, candidate) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => {
            let b_is_tmdb = b.contains("tmdb.org");
            let a_is_tmdb = a.contains("tmdb.org");
            if b_is_tmdb && !a_is_tmdb {
                Some(b)
            } else {
                Some(a)
            }
        }
    }
}

pub fn enriched_to_stremio_meta_item(enriched: &EnrichedTitleMeta) -> MetaItem {
    let mut links = Vec::new();
    if let Some(ref rating) = enriched.imdb_rating {
        let base = IMDB_URL
            .join(&format!("title/{}", enriched.canonical_id))
            .unwrap_or_else(|_| IMDB_URL.clone());
        links.push(Link {
            name: rating.clone(),
            category: IMDB_LINK_CATEGORY.to_owned(),
            url: base,
        });
    }
    for genre in &enriched.genres {
        let url = CINEMETA_URL
            .clone()
            .join(&format!("/catalog/{}/top/genre={genre}.json", enriched.media_type))
            .unwrap_or_else(|_| CINEMETA_URL.clone());
        links.push(Link {
            name: genre.clone(),
            category: GENRES_LINK_CATEGORY.to_owned(),
            url,
        });
    }
    for dir in &enriched.directors {
        links.push(Link {
            name: dir.name.clone(),
            category: "Directors".to_owned(),
            url: CINEMETA_URL.clone(),
        });
    }
    for actor in &enriched.cast {
        links.push(Link {
            name: actor.name.clone(),
            category: "Cast".to_owned(),
            url: CINEMETA_URL.clone(),
        });
    }

    MetaItem {
        preview: MetaItemPreview {
            id: enriched.canonical_id.clone(),
            r#type: enriched.media_type.clone(),
            name: enriched.title.clone(),
            poster: enriched.poster_url.as_deref().and_then(|u| Url::parse(u).ok()),
            background: enriched
                .backdrop_url
                .as_deref()
                .and_then(|u| Url::parse(u).ok()),
            logo: enriched.logo_url.as_deref().and_then(|u| Url::parse(u).ok()),
            description: enriched.overview.clone(),
            release_info: enriched.year.clone(),
            runtime: enriched.runtime.clone(),
            released: None,
            poster_shape: PosterShape::Poster,
            links,
            trailer_streams: Vec::new(),
            behavior_hints: MetaItemBehaviorHints::default(),
        },
        videos: Vec::new(),
    }
}

pub fn format_addon_resource_url(req: &ResourceRequest) -> Option<String> {
    let base_str = req.base.as_str();
    if !base_str.ends_with("/manifest.json") {
        return None;
    }
    let res = urlencoding::encode(&req.path.resource);
    let typ = urlencoding::encode(&req.path.r#type);
    let id = urlencoding::encode(&req.path.id);
    let suffix = if req.path.extra.is_empty() {
        format!("/{res}/{typ}/{id}.json")
    } else {
        let encoded_extra = stremio_core::types::query_params_encode(
            req.path.extra.iter().map(|ev| (&ev.name, &ev.value)),
        );
        format!("/{res}/{typ}/{id}/{encoded_extra}.json")
    };
    Some(base_str.replace("/manifest.json", &suffix))
}

pub fn meta_resource_matches_url(req: &ResourceRequest, sha256_hex: &str) -> bool {
    if req.path.resource != META_RESOURCE_NAME {
        return false;
    }
    format_addon_resource_url(req)
        .map(|u| crate::addon_engine::url_sha256_hex(&u) == sha256_hex)
        .unwrap_or(false)
}

fn tmdb_img_url(image_base: &str, size: &str, file_path: Option<String>) -> Option<String> {
    let path = file_path?;
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }
    let base = image_base.trim_end_matches('/');
    let rel = if trimmed.starts_with('/') {
        trimmed.to_owned()
    } else {
        format!("/{trimmed}")
    };
    Some(format!("{base}/{size}{rel}"))
}

fn select_best_tmdb_logo(image_base: &str, logos: &[TmdbImageAsset]) -> Option<String> {
    let mut ranked: Vec<&TmdbImageAsset> = logos
        .iter()
        .filter(|l| !l.file_path.trim().is_empty())
        .collect();
    if ranked.is_empty() {
        return None;
    }
    ranked.sort_by(|a, b| {
        let a_png = !a.file_path.to_ascii_lowercase().ends_with(".svg");
        let b_png = !b.file_path.to_ascii_lowercase().ends_with(".svg");
        let a_en = a.iso_639_1.as_deref() == Some("en");
        let b_en = b.iso_639_1.as_deref() == Some("en");
        b_png
            .cmp(&a_png)
            .then_with(|| b_en.cmp(&a_en))
            .then_with(|| {
                b.vote_average
                    .unwrap_or(0.0)
                    .partial_cmp(&a.vote_average.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| b.width.unwrap_or(0).cmp(&a.width.unwrap_or(0)))
    });
    let best = ranked.first()?;
    tmdb_img_url(image_base, "w500", Some(best.file_path.clone()))
}

#[derive(Deserialize)]
struct TmdbSearchMultiResponse {
    #[serde(default)]
    results: Vec<TmdbSearchItem>,
}

#[derive(Deserialize)]
struct TmdbSearchItem {
    id: u64,
    media_type: Option<String>,
    title: Option<String>,
    name: Option<String>,
    overview: Option<String>,
    release_date: Option<String>,
    first_air_date: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    profile_path: Option<String>,
    known_for_department: Option<String>,
    vote_average: Option<f64>,
    known_for: Option<Vec<TmdbKnownForItem>>,
}

#[derive(Deserialize)]
struct TmdbKnownForItem {
    title: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct TmdbFindResponse {
    #[serde(default)]
    movie_results: Vec<TmdbIdOnly>,
    #[serde(default)]
    tv_results: Vec<TmdbIdOnly>,
}

#[derive(Deserialize)]
struct TmdbIdOnly {
    id: u64,
}

#[derive(Deserialize)]
struct TmdbDetailResponse {
    title: Option<String>,
    name: Option<String>,
    tagline: Option<String>,
    overview: Option<String>,
    release_date: Option<String>,
    first_air_date: Option<String>,
    runtime: Option<u64>,
    episode_run_time: Option<Vec<u64>>,
    vote_average: Option<f64>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    genres: Option<Vec<TmdbGenre>>,
    belongs_to_collection: Option<TmdbBelongsToCollection>,
    images: Option<TmdbImagesBlock>,
    credits: Option<TmdbCreditsBlock>,
    external_ids: Option<TmdbExternalIds>,
    created_by: Option<Vec<TmdbCreator>>,
    videos: Option<TmdbVideosBlock>,
    release_dates: Option<TmdbReleaseDatesBlock>,
    content_ratings: Option<TmdbContentRatingsBlock>,
    recommendations: Option<TmdbRelatedListBlock>,
    similar: Option<TmdbRelatedListBlock>,
}

#[derive(Deserialize)]
struct TmdbBelongsToCollection {
    id: u64,
    name: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
}

#[derive(Deserialize)]
struct TmdbVideosBlock {
    #[serde(default)]
    results: Vec<TmdbVideoEntry>,
}

#[derive(Deserialize)]
struct TmdbVideoEntry {
    key: Option<String>,
    site: Option<String>,
    #[serde(rename = "type")]
    video_type: Option<String>,
    official: Option<bool>,
    iso_639_1: Option<String>,
    size: Option<u64>,
}

#[derive(Deserialize)]
struct TmdbReleaseDatesBlock {
    #[serde(default)]
    results: Vec<TmdbReleaseDateCountry>,
}

#[derive(Deserialize)]
struct TmdbReleaseDateCountry {
    iso_3166_1: Option<String>,
    #[serde(default)]
    release_dates: Vec<TmdbReleaseDateEntry>,
}

#[derive(Deserialize)]
struct TmdbReleaseDateEntry {
    certification: Option<String>,
    #[serde(rename = "type")]
    release_type: Option<u8>,
}

#[derive(Deserialize)]
struct TmdbContentRatingsBlock {
    #[serde(default)]
    results: Vec<TmdbContentRatingEntry>,
}

#[derive(Deserialize)]
struct TmdbContentRatingEntry {
    iso_3166_1: Option<String>,
    rating: Option<String>,
}

#[derive(Deserialize)]
struct TmdbRelatedListBlock {
    #[serde(default)]
    results: Vec<TmdbRelatedItem>,
}

#[derive(Deserialize)]
struct TmdbRelatedItem {
    id: u64,
    media_type: Option<String>,
    title: Option<String>,
    name: Option<String>,
    overview: Option<String>,
    release_date: Option<String>,
    first_air_date: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    vote_average: Option<f64>,
    popularity: Option<f64>,
}

#[derive(Deserialize)]
struct TmdbCollectionResponse {
    id: Option<u64>,
    name: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    #[serde(default)]
    parts: Vec<TmdbRelatedItem>,
}

#[derive(Deserialize)]
struct TmdbGenre {
    name: String,
}

#[derive(Deserialize)]
struct TmdbImagesBlock {
    #[serde(default)]
    logos: Vec<TmdbImageAsset>,
}

#[derive(Deserialize)]
struct TmdbImageAsset {
    file_path: String,
    iso_639_1: Option<String>,
    vote_average: Option<f64>,
    width: Option<u64>,
}

#[derive(Deserialize)]
struct TmdbCreditsBlock {
    #[serde(default)]
    cast: Vec<TmdbCastEntry>,
    #[serde(default)]
    crew: Vec<TmdbCrewEntry>,
}

#[derive(Deserialize)]
struct TmdbCastEntry {
    id: u64,
    name: String,
    character: Option<String>,
    profile_path: Option<String>,
}

#[derive(Deserialize)]
struct TmdbCrewEntry {
    id: u64,
    name: String,
    job: Option<String>,
    profile_path: Option<String>,
}

#[derive(Deserialize)]
struct TmdbCreator {
    id: u64,
    name: String,
    profile_path: Option<String>,
}

#[derive(Deserialize)]
struct TmdbExternalIds {
    imdb_id: Option<String>,
}

#[derive(Deserialize)]
struct TmdbPersonSearchResponse {
    #[serde(default)]
    results: Vec<TmdbPersonSearchItem>,
}

#[derive(Deserialize)]
struct TmdbPersonSearchItem {
    id: u64,
    name: String,
}

#[derive(Deserialize)]
struct TmdbPersonDetailResponse {
    name: String,
    biography: Option<String>,
    birthday: Option<String>,
    place_of_birth: Option<String>,
    profile_path: Option<String>,
    known_for_department: Option<String>,
    combined_credits: Option<TmdbPersonCombinedCredits>,
}

#[derive(Deserialize)]
struct TmdbPersonCombinedCredits {
    #[serde(default)]
    cast: Vec<TmdbPersonCreditEntry>,
    #[serde(default)]
    crew: Vec<TmdbPersonCreditEntry>,
}

#[derive(Deserialize)]
struct TmdbPersonCreditEntry {
    id: u64,
    media_type: Option<String>,
    title: Option<String>,
    name: Option<String>,
    overview: Option<String>,
    release_date: Option<String>,
    first_air_date: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    vote_average: Option<f64>,
    popularity: Option<f64>,
    character: Option<String>,
    job: Option<String>,
}

#[allow(dead_code)]
pub fn measure_local_search_elapsed_us<F: FnOnce() -> R, R>(f: F) -> (R, u64) {
    let t0 = Instant::now();
    let res = f();
    let us = t0.elapsed().as_micros().min(u64::MAX as u128) as u64;
    debug!(elapsed_us = us, "Measured FTS5 search timing");
    (res, us)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn sample_preview(id: &str, name: &str, director: &str, actor: &str) -> MetaItemPreview {
        MetaItemPreview {
            id: id.to_owned(),
            r#type: "movie".to_owned(),
            name: name.to_owned(),
            poster: Url::parse("https://images.metahub.space/poster/medium/tt15239678/img").ok(),
            background: None,
            logo: None,
            description: Some("Paul Atreides unites with Chani and the Fremen.".to_owned()),
            release_info: Some("2024".to_owned()),
            runtime: Some("166 min".to_owned()),
            released: None,
            poster_shape: PosterShape::Poster,
            links: vec![
                Link {
                    name: "8.5".to_owned(),
                    category: IMDB_LINK_CATEGORY.to_owned(),
                    url: Url::parse("https://imdb.com/title/tt15239678").unwrap(),
                },
                Link {
                    name: director.to_owned(),
                    category: "Directors".to_owned(),
                    url: CINEMETA_URL.clone(),
                },
                Link {
                    name: actor.to_owned(),
                    category: "Cast".to_owned(),
                    url: CINEMETA_URL.clone(),
                },
            ],
            trailer_streams: Vec::new(),
            behavior_hints: MetaItemBehaviorHints::default(),
        }
    }

    #[test]
    fn fts5_diacritics_warm_sub2ms_and_offline_cast_filmography() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("metadata_fts.db");
        let engine = MetadataEngine::open(&db_path).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&db_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "metadata_fts.db must use 0600 permissions");
        }

        let p1 = sample_preview("tt15239678", "Dune: Part Two", "Denis Villeneuve", "Timothée Chalamet");
        let p2 = sample_preview("tt1160419", "Dune", "Denis Villeneuve", "Timothée Chalamet");
        engine.ingest_stremio_previews(&[p1, p2]);

        // Diacritic-insensitive search via unicode61 remove_diacritics 2
        let (cold_titles, cold_people) = engine.search_local_fts("timothee", 10);
        assert!(cold_titles.is_empty());
        assert_eq!(cold_people.len(), 1);
        assert_eq!(cold_people[0].name, "Timothée Chalamet");

        // Warm LRU lookup is sub-2ms
        let (dune_cold, _) = engine.search_local_fts("dune", 10);
        assert_eq!(dune_cold.len(), 2);
        let t0 = Instant::now();
        let (dune_warm, _) = engine.search_local_fts("dune", 10);
        let elapsed = t0.elapsed();
        assert_eq!(dune_warm.len(), 2);
        assert!(
            elapsed < Duration::from_millis(2),
            "warm FTS cache exceeded 2ms: {elapsed:?}"
        );

        let filmography = engine.load_local_person_filmography(
            &cold_people[0].person_key,
            "Timothée Chalamet",
        );
        assert_eq!(filmography.movies.len(), 2);
    }

    #[test]
    fn deduplicates_cross_source_titles_and_promotes_imdb_and_tmdb_artwork() {
        let cinemeta_hit = EditorialMediaHit {
            canonical_id: "tt15239678".to_owned(),
            imdb_id: Some("tt15239678".to_owned()),
            tmdb_id: None,
            media_type: "movie".to_owned(),
            title: "Dune: Part Two".to_owned(),
            year: Some("2024".to_owned()),
            imdb_rating: Some("8.5".to_owned()),
            overview: None,
            poster_url: Some("https://images.metahub.space/poster/tt15239678".to_owned()),
            backdrop_url: None,
            logo_url: None,
            genres: vec!["Sci-Fi".to_owned()],
            score: 800,
            role_hint: None,
        };
        let addon_dup = cinemeta_hit.clone();
        let tmdb_hit = EditorialMediaHit {
            canonical_id: "tmdb:693134".to_owned(),
            imdb_id: None,
            tmdb_id: Some(693134),
            media_type: "movie".to_owned(),
            title: "Dune: Part Two".to_owned(),
            year: Some("2024".to_owned()),
            imdb_rating: Some("8.3".to_owned()),
            overview: Some("Follow the mythic journey of Paul Atreides.".to_owned()),
            poster_url: Some("https://image.tmdb.org/t/p/w500/poster.jpg".to_owned()),
            backdrop_url: Some("https://image.tmdb.org/t/p/w1280/backdrop.jpg".to_owned()),
            logo_url: Some("https://image.tmdb.org/t/p/w500/logo.png".to_owned()),
            genres: Vec::new(),
            score: 900,
            role_hint: None,
        };

        let search = build_unified_editorial_search(
            "dune",
            vec![cinemeta_hit],
            vec![tmdb_hit],
            vec![addon_dup],
            Vec::new(),
            120,
        );
        assert_eq!(search.movies.len(), 1);
        let hit = &search.movies[0];
        assert_eq!(hit.canonical_id, "tt15239678");
        assert_eq!(hit.tmdb_id, Some(693134));
        assert_eq!(
            hit.logo_url.as_deref(),
            Some("https://image.tmdb.org/t/p/w500/logo.png")
        );
        assert_eq!(
            hit.backdrop_url.as_deref(),
            Some("https://image.tmdb.org/t/p/w1280/backdrop.jpg")
        );
        assert!(search.top_match.is_some());
    }

    #[test]
    fn tmdb_client_supports_v3_v4_enrichment_filmography_and_redaction() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();

        let captured_requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let cap_clone = Arc::clone(&captured_requests);

        let addr = rt.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                loop {
                    let Ok((mut sock, _)) = listener.accept().await else {
                        break;
                    };
                    let cap = Arc::clone(&cap_clone);
                    tokio::spawn(async move {
                        let mut buf = [0u8; 4096];
                        let n = sock.read(&mut buf).await.unwrap_or(0);
                        let req = String::from_utf8_lossy(&buf[..n]).to_string();
                        if let Ok(mut g) = cap.lock() {
                            g.push(req.clone());
                        }
                        let first_line = req.lines().next().unwrap_or("");
                        let (status, body) = if first_line.contains("unauthorized") {
                            ("401 Unauthorized", r#"{"status_message":"Invalid API key"}"#.to_owned())
                        } else if first_line.contains("/configuration") {
                            ("200 OK", r#"{"images":{"secure_base_url":"https://image.tmdb.org/t/p/"}}"#.to_owned())
                        } else if first_line.contains("/find/tt15239678") {
                            ("200 OK", r#"{"movie_results":[{"id":693134}],"tv_results":[]}"#.to_owned())
                        } else if first_line.contains("/movie/693134?") {
                            (
                                "200 OK",
                                serde_json::json!({
                                    "title": "Dune: Part Two",
                                    "overview": "Paul Atreides unites with the Fremen.",
                                    "release_date": "2024-02-27",
                                    "runtime": 166,
                                    "vote_average": 8.2,
                                    "poster_path": "/p.jpg",
                                    "backdrop_path": "/b.jpg",
                                    "genres": [{"name": "Science Fiction"}],
                                    "images": {
                                        "logos": [
                                            {"file_path": "/logo.svg", "iso_639_1": "en", "vote_average": 9.0, "width": 500},
                                            {"file_path": "/logo_en.png", "iso_639_1": "en", "vote_average": 7.5, "width": 800}
                                        ]
                                    },
                                    "credits": {
                                        "cast": [{"id": 1190668, "name": "Timothée Chalamet", "character": "Paul Atreides", "profile_path": "/tc.jpg"}],
                                        "crew": [{"id": 137427, "name": "Denis Villeneuve", "job": "Director", "profile_path": "/dv.jpg"}]
                                    },
                                    "external_ids": {"imdb_id": "tt15239678"}
                                }).to_string(),
                            )
                        } else if first_line.contains("/person/137427") {
                            (
                                "200 OK",
                                serde_json::json!({
                                    "name": "Denis Villeneuve",
                                    "biography": "Canadian filmmaker.",
                                    "birthday": "1967-10-03",
                                    "place_of_birth": "Trois-Rivières, Quebec, Canada",
                                    "profile_path": "/dv.jpg",
                                    "known_for_department": "Directing",
                                    "combined_credits": {
                                        "cast": [],
                                        "crew": [
                                            {"id": 693134, "media_type": "movie", "title": "Dune: Part Two", "release_date": "2024-02-27", "poster_path": "/p.jpg", "popularity": 120.0},
                                            {"id": 329865, "media_type": "movie", "title": "Arrival", "release_date": "2016-11-10", "poster_path": "/arr.jpg", "popularity": 80.0}
                                        ]
                                    }
                                }).to_string(),
                            )
                        } else if first_line.contains("/search/multi") {
                            (
                                "200 OK",
                                serde_json::json!({
                                    "results": [
                                        {"id": 693134, "media_type": "movie", "title": "Dune: Part Two", "release_date": "2024-02-27", "poster_path": "/p.jpg", "backdrop_path": "/b.jpg", "vote_average": 8.2}
                                    ]
                                }).to_string(),
                            )
                        } else {
                            ("404 Not Found", "{}".to_owned())
                        };
                        let resp = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = sock.write_all(resp.as_bytes()).await;
                    });
                }
            });
            addr
        });

        let dir = tempfile::tempdir().unwrap();
        let engine = MetadataEngine::open(&dir.path().join("metadata_fts.db")).unwrap();
        engine.set_base_urls(format!("http://{addr}"), "https://image.tmdb.org/t/p");

        // v3 key verify
        let st_v3 = rt
            .block_on(engine.verify_credentials("v3_secret_key_abcdef"))
            .unwrap();
        assert_eq!(st_v3.auth_mode, "v3_api_key");

        // v4 bearer verify
        let st_v4 = rt
            .block_on(engine.verify_credentials("eyJhbGciOiJIUzI1NiJ9.payload.sig"))
            .unwrap();
        assert_eq!(st_v4.auth_mode, "v4_bearer");

        // Secret redaction on error
        let err = rt
            .block_on(engine.verify_credentials("unauthorized_super_secret_token"))
            .unwrap_err();
        assert!(!err.contains("unauthorized_super_secret_token"));

        // Enrich tt15239678 -> chooses PNG logo & credits
        let enriched = rt
            .block_on(engine.enrich_title("v3_secret_key_abcdef", "movie", "tt15239678"))
            .unwrap()
            .unwrap();
        assert_eq!(enriched.tmdb_id, Some(693134));
        assert_eq!(
            enriched.logo_url.as_deref(),
            Some("https://image.tmdb.org/t/p/w500/logo_en.png")
        );
        assert_eq!(enriched.directors[0].name, "Denis Villeneuve");
        assert_eq!(enriched.cast[0].name, "Timothée Chalamet");

        // Person filmography bridges Dune 2 -> tt15239678
        let filmo = rt.block_on(engine.fetch_person_filmography(
            Some("v3_secret_key_abcdef"),
            "tmdb_person:137427",
            "Denis Villeneuve",
        ));
        assert_eq!(filmo.movies.len(), 2);
        assert!(filmo.movies.iter().any(|m| m.canonical_id == "tt15239678"));
        assert!(filmo.movies.iter().any(|m| m.canonical_id == "tmdb:329865"));

        // Search multi bridges known TMDB id to tt15239678 immediately
        let (hits, _) = rt
            .block_on(engine.search_multi("v3_secret_key_abcdef", "dune"))
            .unwrap();
        assert_eq!(hits[0].canonical_id, "tt15239678");

        let reqs = captured_requests.lock().unwrap();
        assert!(reqs.iter().any(|r| r.contains("api_key=v3_secret_key_abcdef")));
        assert!(reqs
            .iter()
            .any(|r| r.to_ascii_lowercase().contains("authorization: bearer eyj")));
    }

    #[test]
    fn wal_reader_split_never_blocks_on_active_write_transaction_and_memory_fallback_works() {
        // :memory: DBs must not open a separate read_conn and must still read/search cleanly.
        let mem_engine = MetadataEngine::open(Path::new(":memory:")).unwrap();
        assert!(mem_engine.read_conn.is_none());
        let p_mem = sample_preview("tt15239678", "Dune: Part Two", "Denis Villeneuve", "Timothée Chalamet");
        mem_engine.ingest_stremio_previews(&[p_mem]);
        let (mem_hits, _) = mem_engine.search_local_fts("Dune", 10);
        assert_eq!(mem_hits.len(), 1);
        assert!(mem_engine.cached_enriched_meta("tt15239678").is_some());

        // File-backed DBs open a separate read-only WAL connection so UI reads never block
        // even while the write connection holds its mutex and an open write transaction.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("wal_split.db");
        let engine = MetadataEngine::open(&db_path).unwrap();
        assert!(engine.read_conn.is_some());

        let committed = sample_preview("tt1160419", "Dune", "Denis Villeneuve", "Timothée Chalamet");
        engine.ingest_stremio_previews(&[committed]);

        let mut write_conn = engine.conn.lock().unwrap();
        let tx = write_conn.transaction().unwrap();
        let uncommitted = sample_preview(
            "tt15239678",
            "Dune: Part Two",
            "Denis Villeneuve",
            "Timothée Chalamet",
        );
        assert!(upsert_preview_tx(&tx, &uncommitted, Utc::now().timestamp()));

        // Same-thread reads via search_local_fts and cached_enriched_meta would deadlock on
        // self.conn if they touched the write mutex; via read_conn they return immediately
        // with the committed WAL snapshot.
        let t0 = Instant::now();
        let (titles, _) = engine.search_local_fts("Dune", 10);
        let cached = engine.cached_enriched_meta("tt1160419");
        let elapsed = t0.elapsed();
        assert!(
            elapsed < Duration::from_millis(50),
            "WAL reader query took too long: {elapsed:?}"
        );
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].canonical_id, "tt1160419");
        assert!(cached.is_some());
        assert!(engine.cached_enriched_meta("tt15239678").is_none());

        tx.commit().unwrap();
        drop(write_conn);

        // Once committed, read_conn immediately sees the newly committed WAL frames.
        engine.clear_caches();
        let (after_commit, _) = engine.search_local_fts("Dune", 10);
        assert_eq!(after_commit.len(), 2);
        assert!(engine.cached_enriched_meta("tt15239678").is_some());
    }

    #[test]
    fn enrichment_parses_collection_recommendations_trailer_certification_role_hints_and_persists() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();

        let collection_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let col_calls_clone = Arc::clone(&collection_calls);

        let addr = rt.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                loop {
                    let Ok((mut sock, _)) = listener.accept().await else {
                        break;
                    };
                    let col_calls = Arc::clone(&col_calls_clone);
                    tokio::spawn(async move {
                        let mut buf = [0u8; 4096];
                        let n = sock.read(&mut buf).await.unwrap_or(0);
                        let req = String::from_utf8_lossy(&buf[..n]).to_string();
                        let first_line = req.lines().next().unwrap_or("");
                        let (status, body) = if first_line.contains("/find/tt15239678") {
                            ("200 OK", r#"{"movie_results":[{"id":693134}],"tv_results":[]}"#.to_owned())
                        } else if first_line.contains("/movie/693134?") {
                            assert!(first_line.contains("recommendations"));
                            assert!(first_line.contains("similar"));
                            assert!(first_line.contains("videos"));
                            assert!(first_line.contains("release_dates"));
                            assert!(first_line.contains("content_ratings"));
                            let extra_recs: Vec<serde_json::Value> = (1..=20)
                                .map(|i| {
                                    serde_json::json!({
                                        "id": 9000 + i,
                                        "media_type": "movie",
                                        "title": format!("Similar Film {i}"),
                                        "release_date": "2020-05-01",
                                        "poster_path": format!("/sim_{i}.jpg"),
                                        "backdrop_path": format!("/sim_bg_{i}.jpg"),
                                        "vote_average": 7.1,
                                        "popularity": (30 - i) as f64
                                    })
                                })
                                .collect();
                            (
                                "200 OK",
                                serde_json::json!({
                                    "title": "Dune: Part Two",
                                    "tagline": "Long live the fighters.",
                                    "overview": "Paul Atreides unites with the Fremen.",
                                    "release_date": "2024-02-27",
                                    "runtime": 166,
                                    "vote_average": 8.2,
                                    "poster_path": "/p.jpg",
                                    "backdrop_path": "/b.jpg",
                                    "genres": [{"name": "Science Fiction"}],
                                    "belongs_to_collection": {
                                        "id": 726871,
                                        "name": "Dune Collection",
                                        "poster_path": "/col_p.jpg",
                                        "backdrop_path": "/col_b.jpg"
                                    },
                                    "videos": {
                                        "results": [
                                            {"key": "clip_key", "site": "YouTube", "type": "Clip", "official": true, "iso_639_1": "en", "size": 1080},
                                            {"key": "unofficial_tr", "site": "YouTube", "type": "Trailer", "official": false, "iso_639_1": "en", "size": 1080},
                                            {"key": "Way9Dexny3w", "site": "YouTube", "type": "Trailer", "official": true, "iso_639_1": "en", "size": 1080}
                                        ]
                                    },
                                    "release_dates": {
                                        "results": [
                                            {"iso_3166_1": "DE", "release_dates": [{"certification": "12", "type": 3}]},
                                            {"iso_3166_1": "US", "release_dates": [{"certification": "", "type": 1}, {"certification": "PG-13", "type": 3}]}
                                        ]
                                    },
                                    "recommendations": {
                                        "results": [
                                            {"id": 693134, "media_type": "movie", "title": "Dune: Part Two"},
                                            {"id": 438631, "media_type": "movie", "title": "Dune", "release_date": "2021-09-15", "poster_path": "/d1_p.jpg", "backdrop_path": "/d1_b.jpg", "vote_average": 7.8, "popularity": 95.0}
                                        ]
                                    },
                                    "similar": {
                                        "results": extra_recs
                                    },
                                    "credits": {
                                        "cast": [{"id": 1190668, "name": "Timothée Chalamet", "character": "Paul Atreides", "profile_path": "/tc.jpg"}],
                                        "crew": [{"id": 137427, "name": "Denis Villeneuve", "job": "Director", "profile_path": "/dv.jpg"}]
                                    },
                                    "external_ids": {"imdb_id": "tt15239678"}
                                }).to_string(),
                            )
                        } else if first_line.contains("/tv/1396?") {
                            (
                                "200 OK",
                                serde_json::json!({
                                    "name": "Breaking Bad",
                                    "tagline": "Change the equation.",
                                    "overview": "A chemistry teacher diagnosed with cancer.",
                                    "first_air_date": "2008-01-20",
                                    "episode_run_time": [47],
                                    "vote_average": 8.9,
                                    "poster_path": "/bb_p.jpg",
                                    "backdrop_path": "/bb_b.jpg",
                                    "content_ratings": {
                                        "results": [
                                            {"iso_3166_1": "GB", "rating": "18"},
                                            {"iso_3166_1": "US", "rating": "TV-MA"}
                                        ]
                                    },
                                    "videos": {
                                        "results": [
                                            {"key": "HhesaQXLuRY", "site": "YouTube", "type": "Teaser", "official": true, "iso_639_1": "en", "size": 720}
                                        ]
                                    },
                                    "similar": {
                                        "results": [
                                            {"id": 60059, "name": "Better Call Saul", "first_air_date": "2015-02-08", "poster_path": "/bcs_p.jpg", "backdrop_path": "/bcs_b.jpg", "vote_average": 8.7, "popularity": 90.0}
                                        ]
                                    },
                                    "external_ids": {"imdb_id": "tt0903747"}
                                }).to_string(),
                            )
                        } else if first_line.contains("/collection/726871") {
                            col_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            (
                                "200 OK",
                                serde_json::json!({
                                    "id": 726871,
                                    "name": "Dune Collection",
                                    "overview": "The desert epic.",
                                    "poster_path": "/col_p.jpg",
                                    "backdrop_path": "/col_b.jpg",
                                    "parts": [
                                        {"id": 693134, "title": "Dune: Part Two", "release_date": "2024-02-27", "poster_path": "/d2_p.jpg", "backdrop_path": "/d2_b.jpg", "vote_average": 8.2},
                                        {"id": 438631, "title": "Dune", "release_date": "2021-09-15", "poster_path": "/d1_p.jpg", "backdrop_path": "/d1_b.jpg", "vote_average": 7.8}
                                    ]
                                }).to_string(),
                            )
                        } else if first_line.contains("/person/1190668") {
                            (
                                "200 OK",
                                serde_json::json!({
                                    "name": "Timothée Chalamet",
                                    "profile_path": "/tc.jpg",
                                    "known_for_department": "Acting",
                                    "combined_credits": {
                                        "cast": [
                                            {"id": 693134, "media_type": "movie", "title": "Dune: Part Two", "character": "Paul Atreides", "release_date": "2024-02-27", "poster_path": "/p.jpg", "backdrop_path": "/b.jpg", "popularity": 120.0}
                                        ],
                                        "crew": []
                                    }
                                }).to_string(),
                            )
                        } else if first_line.contains("/person/137427") {
                            (
                                "200 OK",
                                serde_json::json!({
                                    "name": "Denis Villeneuve",
                                    "profile_path": "/dv.jpg",
                                    "known_for_department": "Directing",
                                    "combined_credits": {
                                        "cast": [],
                                        "crew": [
                                            {"id": 693134, "media_type": "movie", "title": "Dune: Part Two", "job": "Director", "release_date": "2024-02-27", "poster_path": "/p.jpg", "backdrop_path": "/b.jpg", "popularity": 120.0}
                                        ]
                                    }
                                }).to_string(),
                            )
                        } else {
                            ("404 Not Found", "{}".to_owned())
                        };
                        let resp = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = sock.write_all(resp.as_bytes()).await;
                    });
                }
            });
            addr
        });

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("meta_v5.db");

        // Verify legacy DB without extra_enrichment_json migrates cleanly on open.
        {
            let raw_conn = Connection::open(&db_path).unwrap();
            raw_conn
                .execute_batch(
                    "CREATE TABLE meta_titles (
                         canonical_id TEXT PRIMARY KEY NOT NULL,
                         imdb_id TEXT,
                         tmdb_id INTEGER,
                         media_type TEXT NOT NULL,
                         title TEXT NOT NULL,
                         norm_title TEXT NOT NULL,
                         year TEXT,
                         overview TEXT,
                         runtime TEXT,
                         imdb_rating TEXT,
                         genres_json TEXT NOT NULL DEFAULT '[]',
                         poster_url TEXT,
                         backdrop_url TEXT,
                         logo_url TEXT,
                         enriched_from_tmdb INTEGER NOT NULL DEFAULT 0,
                         updated_at INTEGER NOT NULL
                     );",
                )
                .unwrap();
        }

        {
            let engine = MetadataEngine::open(&db_path).unwrap();
            engine.set_base_urls(format!("http://{addr}"), "https://image.tmdb.org/t/p");

            let enriched = rt
                .block_on(engine.enrich_title("v3_key", "movie", "tt15239678"))
                .unwrap()
                .unwrap();
            assert_eq!(enriched.tagline.as_deref(), Some("Long live the fighters."));
            assert_eq!(enriched.certification.as_deref(), Some("PG-13"));
            assert_eq!(enriched.trailer_youtube_key.as_deref(), Some("Way9Dexny3w"));
            assert_eq!(
                enriched.poster_url.as_deref(),
                Some("https://image.tmdb.org/t/p/w342/p.jpg")
            );
            assert_eq!(
                enriched.backdrop_url.as_deref(),
                Some("https://image.tmdb.org/t/p/w1280/b.jpg")
            );
            assert_eq!(
                enriched.cast[0].headshot_url.as_deref(),
                Some("https://image.tmdb.org/t/p/w185/tc.jpg")
            );
            let col = enriched.collection.as_ref().unwrap();
            assert_eq!(col.id, 726871);
            assert_eq!(col.name, "Dune Collection");
            assert_eq!(
                col.poster_url.as_deref(),
                Some("https://image.tmdb.org/t/p/w342/col_p.jpg")
            );
            assert_eq!(
                col.backdrop_url.as_deref(),
                Some("https://image.tmdb.org/t/p/w780/col_b.jpg")
            );
            assert_eq!(enriched.recommendations.len(), 16);
            assert_eq!(enriched.recommendations[0].title, "Dune");
            assert_eq!(
                enriched.recommendations[0].poster_url.as_deref(),
                Some("https://image.tmdb.org/t/p/w342/d1_p.jpg")
            );
            assert_eq!(
                enriched.recommendations[0].backdrop_url.as_deref(),
                Some("https://image.tmdb.org/t/p/w780/d1_b.jpg")
            );

            // TV content_ratings & Teaser fallback + similar inheriting series type
            let tv_enriched = rt
                .block_on(engine.enrich_title("v3_key", "series", "tmdb:1396"))
                .unwrap()
                .unwrap();
            assert_eq!(tv_enriched.certification.as_deref(), Some("TV-MA"));
            assert_eq!(
                tv_enriched.trailer_youtube_key.as_deref(),
                Some("HhesaQXLuRY")
            );
            assert_eq!(tv_enriched.recommendations.len(), 1);
            assert_eq!(tv_enriched.recommendations[0].media_type, "series");
            assert_eq!(tv_enriched.recommendations[0].title, "Better Call Saul");

            // Role hints in fetch_person_filmography & w342 profile headshot
            let actor_filmo = rt.block_on(engine.fetch_person_filmography(
                Some("v3_key"),
                "tmdb_person:1190668",
                "Timothée Chalamet",
            ));
            assert_eq!(
                actor_filmo.headshot_url.as_deref(),
                Some("https://image.tmdb.org/t/p/w342/tc.jpg")
            );
            assert_eq!(
                actor_filmo.movies[0].role_hint.as_deref(),
                Some("Paul Atreides")
            );
            assert_eq!(
                actor_filmo.movies[0].backdrop_url.as_deref(),
                Some("https://image.tmdb.org/t/p/w780/b.jpg")
            );

            let dir_filmo = rt.block_on(engine.fetch_person_filmography(
                Some("v3_key"),
                "tmdb_person:137427",
                "Denis Villeneuve",
            ));
            assert_eq!(dir_filmo.movies[0].role_hint.as_deref(), Some("Director"));

            // Fetch collection sorts chronologically and bridges tt15239678
            let full_col = rt
                .block_on(engine.fetch_collection("v3_key", 726871))
                .unwrap();
            assert_eq!(full_col.overview.as_deref(), Some("The desert epic."));
            assert_eq!(full_col.parts.len(), 2);
            assert_eq!(full_col.parts[0].title, "Dune");
            assert_eq!(full_col.parts[1].title, "Dune: Part Two");
            assert_eq!(full_col.parts[1].canonical_id, "tt15239678");
            assert_eq!(collection_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        }

        // Reopen DB from disk: extra_enrichment_json + meta_collections restore with 0 network calls.
        let reopened = MetadataEngine::open(&db_path).unwrap();
        let restored = reopened.cached_enriched_meta("tt15239678").unwrap();
        assert_eq!(restored.tagline.as_deref(), Some("Long live the fighters."));
        assert_eq!(restored.certification.as_deref(), Some("PG-13"));
        assert_eq!(restored.trailer_youtube_key.as_deref(), Some("Way9Dexny3w"));
        assert_eq!(restored.collection.as_ref().unwrap().id, 726871);
        assert_eq!(restored.recommendations.len(), 16);

        let restored_col = rt.block_on(reopened.fetch_collection("", 726871)).unwrap();
        assert_eq!(restored_col.parts.len(), 2);
        assert_eq!(collection_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn incremental_preview_upsert_skips_noop_updates_and_preserves_search_cache() {
        let dir = tempfile::tempdir().unwrap();
        let engine = MetadataEngine::open(&dir.path().join("noop_ingest.db")).unwrap();

        let preview = sample_preview("tt15239678", "Dune: Part Two", "Denis Villeneuve", "Timothée Chalamet");
        engine.ingest_stremio_previews(std::slice::from_ref(&preview));
        let (hits, _) = engine.search_local_fts("Dune", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(engine.hot.lock().unwrap().search_queries.len(), 1);

        // Re-ingesting identical preview must not invalidate HotMetadataCaches::search_queries.
        engine.ingest_stremio_previews(std::slice::from_ref(&preview));
        assert_eq!(engine.hot.lock().unwrap().search_queries.len(), 1);

        // Enriching with previously-missing background URL MUST update and invalidate search cache.
        let mut updated = preview;
        updated.background = Url::parse("https://image.tmdb.org/t/p/w780/new_bg.jpg").ok();
        engine.ingest_stremio_previews(&[updated]);
        assert!(engine.hot.lock().unwrap().search_queries.is_empty());
    }
}
