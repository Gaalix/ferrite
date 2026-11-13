use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    sync::{Mutex, RwLock},
    time::{Duration, Instant},
};

use chrono::Utc;
use once_cell::sync::Lazy;
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    titles: HashMap<String, EnrichedTitleMeta>,
    titles_lru: VecDeque<String>,
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

    fn get_title(&self, canonical_or_alt_id: &str) -> Option<EnrichedTitleMeta> {
        self.titles.get(canonical_or_alt_id).cloned()
    }

    fn insert_title(&mut self, key: String, meta: EnrichedTitleMeta) {
        if self.titles.contains_key(&key) {
            self.titles_lru.retain(|k| k != &key);
        } else if self.titles_lru.len() >= MAX_ENRICHED_META_CACHE {
            if let Some(oldest) = self.titles_lru.pop_front() {
                self.titles.remove(&oldest);
            }
        }
        self.titles_lru.push_back(key.clone());
        self.titles.insert(key, meta);
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
    conn: Mutex<Connection>,
    hot: Mutex<HotMetadataCaches>,
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
                 updated_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_meta_titles_imdb ON meta_titles(imdb_id);
             CREATE INDEX IF NOT EXISTS idx_meta_titles_norm ON meta_titles(media_type, norm_title, year);
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

        if !is_memory {
            tighten_sqlite_file_permissions(path);
        }

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
            conn: Mutex::new(conn),
            hot: Mutex::new(HotMetadataCaches::default()),
            http,
            endpoints: RwLock::new(TmdbEndpoints::default()),
        })
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
        for preview in items {
            upsert_preview_tx(&tx, preview, now);
        }
        if tx.commit().is_ok() {
            if self.db_path != Path::new(":memory:") {
                tighten_sqlite_file_permissions(&self.db_path);
            }
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
        upsert_preview_tx(&tx, &item.preview, now);
        if tx.commit().is_ok() {
            if self.db_path != Path::new(":memory:") {
                tighten_sqlite_file_permissions(&self.db_path);
            }
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

        let Ok(conn) = self.conn.lock() else {
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
        if let Ok(hot) = self.hot.lock() {
            if let Some(hit) = hot.get_title(id) {
                return Some(hit);
            }
        }

        let Ok(conn) = self.conn.lock() else {
            return None;
        };
        let canonical = resolve_canonical_id_conn(&conn, id).unwrap_or_else(|| id.to_owned());
        let loaded = load_enriched_title_conn(&conn, &canonical)?;
        if let Ok(mut hot) = self.hot.lock() {
            hot.insert_title(id.to_owned(), loaded.clone());
            if canonical != id {
                hot.insert_title(canonical, loaded.clone());
            }
        }
        Some(loaded)
    }

    pub fn lookup_bridged_imdb_id(&self, media_type: &str, tmdb_id: u64) -> Option<String> {
        let Ok(conn) = self.conn.lock() else {
            return None;
        };
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
                    let imdb_id = self.lookup_bridged_imdb_id(stremio_type, item.id);
                    let canonical_id = imdb_id
                        .clone()
                        .unwrap_or_else(|| format!("tmdb:{}", item.id));
                    let vote = item
                        .vote_average
                        .filter(|v| *v > 0.1)
                        .map(|v| format!("{v:.1}"));
                    let score = score_title_relevance(&title, &norm_q) + 22;
                    media_hits.push(EditorialMediaHit {
                        canonical_id,
                        imdb_id,
                        tmdb_id: Some(item.id),
                        media_type: stremio_type.to_owned(),
                        title,
                        year,
                        imdb_rating: vote,
                        overview: item.overview.filter(|s| !s.trim().is_empty()),
                        poster_url: tmdb_img_url(&endpoints.image_base, "w500", item.poster_path),
                        backdrop_url: tmdb_img_url(
                            &endpoints.image_base,
                            "w1280",
                            item.backdrop_path,
                        ),
                        logo_url: None,
                        genres: Vec::new(),
                        score,
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

        self.persist_search_hits(&media_hits, &person_hits);
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
            return Ok(self.cached_enriched_meta(id));
        }

        // Fast return if already TMDB-enriched with cast + (logo or backdrop).
        if let Some(existing) = self.cached_enriched_meta(id) {
            if existing.tmdb_id.is_some()
                && (!existing.cast.is_empty() || existing.logo_url.is_some())
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
                None => return Ok(self.cached_enriched_meta(id)),
            }
        } else {
            return Ok(self.cached_enriched_meta(id));
        };

        let path = format!("/{resolved_tmdb_type}/{tmdb_id}");
        let detail: TmdbDetailResponse = self
            .tmdb_get_json(
                &endpoints.api_base,
                &path,
                &[
                    ("append_to_response", "images,credits,external_ids"),
                    ("include_image_language", "en,null"),
                ],
                key,
            )
            .await?;

        let enriched = self.store_tmdb_detail_response(
            id,
            resolved_tmdb_type,
            tmdb_id,
            detail,
            &endpoints.image_base,
        );
        Ok(Some(enriched))
    }

    pub async fn resolve_tmdb_to_imdb(
        &self,
        api_key: Option<&str>,
        media_type: &str,
        tmdb_id: u64,
    ) -> Option<String> {
        if let Some(cached) = self.lookup_bridged_imdb_id(media_type, tmdb_id) {
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
        self.upsert_id_bridge(tmdb_type, tmdb_id, &imdb_id);
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
            None
        }
        .or_else(|| self.cached_enriched_meta(id))?;

        Some(enriched_to_stremio_meta_item(&enriched))
    }

    pub async fn fetch_person_filmography(
        &self,
        api_key: Option<&str>,
        person_key: &str,
        fallback_name: &str,
    ) -> PersonFilmography {
        let mut local = self.load_local_person_filmography(person_key, fallback_name);
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

        let headshot_url =
            tmdb_img_url(&endpoints.image_base, "w500", detail.profile_path.clone())
                .or_else(|| local.headshot_url.clone());

        let mut raw_movies = Vec::new();
        let mut raw_series = Vec::new();
        if let Some(credits) = detail.combined_credits {
            let mut seen_credit_keys: HashSet<(String, u64)> = HashSet::new();
            for entry in credits.cast.into_iter().chain(credits.crew) {
                let Some(mt) = entry.media_type.as_deref() else {
                    continue;
                };
                if mt != "movie" && mt != "tv" {
                    continue;
                }
                let stype = if mt == "tv" { "series" } else { "movie" };
                if !seen_credit_keys.insert((stype.to_owned(), entry.id)) {
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
                let imdb_id = self.lookup_bridged_imdb_id(stype, entry.id);
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
                    poster_url: tmdb_img_url(&endpoints.image_base, "w500", entry.poster_path),
                    backdrop_url: tmdb_img_url(
                        &endpoints.image_base,
                        "w1280",
                        entry.backdrop_path,
                    ),
                    logo_url: None,
                    genres: Vec::new(),
                    score: pop_score,
                };
                if stype == "movie" {
                    raw_movies.push(hit);
                } else {
                    raw_series.push(hit);
                }
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

        local = PersonFilmography {
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

        self.persist_person_profile(&local, person_key);
        local
    }

    async fn resolve_imdb_to_tmdb(
        &self,
        api_key: &str,
        api_base: &str,
        imdb_id: &str,
        preferred_tmdb_type: &str,
    ) -> Result<Option<(&'static str, u64)>, String> {
        {
            if let Ok(conn) = self.conn.lock() {
                if let Ok(Some((ttype, tid))) = conn
                    .query_row(
                        "SELECT tmdb_type, tmdb_id FROM id_bridge WHERE imdb_id = ?1 LIMIT 1",
                        params![imdb_id],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
                    )
                    .optional()
                {
                    if let Ok(uid) = u64::try_from(tid) {
                        let static_type = if ttype == "tv" { "tv" } else { "movie" };
                        return Ok(Some((static_type, uid)));
                    }
                }
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
            self.upsert_id_bridge(ttype, tid, imdb_id);
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

    fn upsert_id_bridge(&self, tmdb_type: &str, tmdb_id: u64, imdb_id: &str) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute(
                "INSERT INTO id_bridge (tmdb_type, tmdb_id, imdb_id, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(tmdb_type, tmdb_id) DO UPDATE SET
                     imdb_id = excluded.imdb_id,
                     updated_at = excluded.updated_at",
                params![tmdb_type, tmdb_id as i64, imdb_id, Utc::now().timestamp()],
            );
            if self.db_path != Path::new(":memory:") {
                tighten_sqlite_file_permissions(&self.db_path);
            }
        }
    }

    fn persist_search_hits(
        &self,
        media_hits: &[EditorialMediaHit],
        person_hits: &[EditorialPersonHit],
    ) {
        let Ok(mut conn) = self.conn.lock() else {
            return;
        };
        let Ok(tx) = conn.transaction() else {
            return;
        };
        let now = Utc::now().timestamp();
        for hit in media_hits {
            let norm_title = normalize_title_for_dedup(&hit.title);
            let genres_json = serde_json::to_string(&hit.genres).unwrap_or_else(|_| "[]".to_owned());
            let _ = tx.execute(
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
                params![
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
                ],
            );
            reindex_fts_title_tx(
                &tx,
                &hit.canonical_id,
                &hit.media_type,
                &hit.title,
                hit.overview.as_deref().unwrap_or(""),
                &hit.genres.join(" "),
            );
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
            if self.db_path != Path::new(":memory:") {
                tighten_sqlite_file_permissions(&self.db_path);
            }
            if let Ok(mut hot) = self.hot.lock() {
                hot.invalidate_search();
            }
        }
    }

    fn store_tmdb_detail_response(
        &self,
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
        let poster_url = tmdb_img_url(image_base, "w500", detail.poster_path);
        let backdrop_url = tmdb_img_url(image_base, "w1280", detail.backdrop_path);
        let logo_url = select_best_tmdb_logo(
            image_base,
            detail
                .images
                .as_ref()
                .map(|imgs| imgs.logos.as_slice())
                .unwrap_or(&[]),
        );

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
        };

        if let Ok(mut conn) = self.conn.lock() {
            if let Ok(tx) = conn.transaction() {
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
                         enriched_from_tmdb, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 1, ?15)
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
                         updated_at = excluded.updated_at",
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
                if tx.commit().is_ok() && self.db_path != Path::new(":memory:") {
                    tighten_sqlite_file_permissions(&self.db_path);
                }
            }
        }

        if let Ok(mut hot) = self.hot.lock() {
            hot.invalidate_search();
            hot.insert_title(canonical_id.clone(), enriched.clone());
            if requested_id != canonical_id {
                hot.insert_title(requested_id.to_owned(), enriched.clone());
            }
        }
        enriched
    }

    fn load_local_person_filmography(
        &self,
        person_key: &str,
        fallback_name: &str,
    ) -> PersonFilmography {
        let Ok(conn) = self.conn.lock() else {
            return empty_person_filmography(person_key, fallback_name);
        };

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
            "SELECT DISTINCT m.canonical_id, m.imdb_id, m.tmdb_id, m.media_type, m.title, m.year,
                    m.imdb_rating, m.overview, m.poster_url, m.backdrop_url, m.logo_url, m.genres_json
             FROM title_credits c
             JOIN meta_people p ON p.person_key = c.person_key
             JOIN meta_titles m ON m.canonical_id = c.canonical_id
             WHERE c.person_key = ?1 OR c.person_key = ?2 OR p.norm_name = ?3
             ORDER BY m.year DESC, m.updated_at DESC
             LIMIT 64",
        ) {
            if let Ok(rows) = stmt.query_map(params![person_key, resolved_key, norm_name], |r| {
                let genres_raw: String = r.get(11)?;
                let genres: Vec<String> = serde_json::from_str(&genres_raw).unwrap_or_default();
                let tmdb_id_raw: Option<i64> = r.get(2)?;
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

    fn persist_person_profile(&self, filmography: &PersonFilmography, alt_key: &str) {
        let Ok(mut conn) = self.conn.lock() else {
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
            if self.db_path != Path::new(":memory:") {
                tighten_sqlite_file_permissions(&self.db_path);
            }
            if let Ok(mut hot) = self.hot.lock() {
                hot.invalidate_search();
            }
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

fn load_enriched_title_conn(conn: &Connection, canonical_id: &str) -> Option<EnrichedTitleMeta> {
    let row = conn
        .query_row(
            "SELECT canonical_id, imdb_id, tmdb_id, media_type, title, year, overview, runtime,
                    imdb_rating, genres_json, poster_url, backdrop_url, logo_url
             FROM meta_titles
             WHERE canonical_id = ?1 OR imdb_id = ?1
             LIMIT 1",
            params![canonical_id],
            |r| {
                let genres_raw: String = r.get(9)?;
                let genres: Vec<String> = serde_json::from_str(&genres_raw).unwrap_or_default();
                let tmdb_raw: Option<i64> = r.get(2)?;
                Ok(EnrichedTitleMeta {
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
                })
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

    Some(EnrichedTitleMeta {
        directors,
        cast,
        ..row
    })
}

fn upsert_preview_tx(tx: &rusqlite::Transaction<'_>, preview: &MetaItemPreview, now: i64) {
    if preview.id.trim().is_empty() || preview.name.trim().is_empty() {
        return;
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

    let _ = tx.execute(
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
             updated_at = excluded.updated_at",
        params![
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
        ],
    );

    reindex_fts_title_tx(
        tx,
        &preview.id,
        &stremio_type,
        &preview.name,
        preview.description.as_deref().unwrap_or(""),
        &genres.join(" "),
    );

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
        upsert_person_tx(
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
        );
        let (role_kind, ord) = if is_director {
            let o = director_order;
            director_order += 1;
            ("director", o)
        } else {
            let o = cast_order;
            cast_order += 1;
            ("cast", o)
        };
        let _ = tx.execute(
            "INSERT OR IGNORE INTO title_credits (canonical_id, person_key, role_kind, character_or_job, credit_order)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                preview.id,
                person_key,
                role_kind,
                if is_director { Some("Director") } else { None },
                ord,
            ],
        );
    }
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
) {
    let norm_name = normalize_title_for_dedup(name);
    if norm_name.is_empty() {
        return;
    }
    let known_json = serde_json::to_string(known_for).unwrap_or_else(|_| "[]".to_owned());
    let known_text = known_for.join(" ");
    let _ = tx.execute(
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
             updated_at = excluded.updated_at",
        params![
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
        ],
    );
    let _ = tx.execute(
        "DELETE FROM fts_people WHERE person_key = ?1",
        params![person_key],
    );
    let _ = tx.execute(
        "INSERT INTO fts_people (person_key, name, known_for_text) VALUES (?1, ?2, ?3)",
        params![person_key, name, known_text],
    );
}

fn reindex_fts_title_tx(
    tx: &rusqlite::Transaction<'_>,
    canonical_id: &str,
    media_type: &str,
    title: &str,
    overview: &str,
    genres_text: &str,
) {
    let _ = tx.execute(
        "DELETE FROM fts_titles WHERE canonical_id = ?1",
        params![canonical_id],
    );
    let _ = tx.execute(
        "INSERT INTO fts_titles (canonical_id, media_type, title, overview, genres_text)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![canonical_id, media_type, title, overview, genres_text],
    );
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
    overview: Option<String>,
    release_date: Option<String>,
    first_air_date: Option<String>,
    runtime: Option<u64>,
    episode_run_time: Option<Vec<u64>>,
    vote_average: Option<f64>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    genres: Option<Vec<TmdbGenre>>,
    images: Option<TmdbImagesBlock>,
    credits: Option<TmdbCreditsBlock>,
    external_ids: Option<TmdbExternalIds>,
    created_by: Option<Vec<TmdbCreator>>,
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
}
