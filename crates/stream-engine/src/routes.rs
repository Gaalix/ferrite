use std::path::{Path as FsPath, PathBuf};
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};

use axum::{
    body::Body,
    extract::{OriginalUri, Path, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use futures::StreamExt;
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use stremio_core::{
    models::streaming_server::PlaybackDevice,
    types::{
        api::SuccessResponse,
        streaming_server::{DeviceInfo, NetworkInfo, PeerSearch, SettingsResponse, Statistics},
        True,
    },
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, ReadBuf, Take};
use tokio_util::io::ReaderStream;
use tower_http::cors::{AllowMethods, AllowOrigin, CorsLayer};
use tracing::{debug, warn};
use url::Url;

use crate::{
    opensubhash::{
        compute_opensubhash_http, compute_opensubhash_path, OpensubHashError, OpensubHashResponse,
    },
    range::{content_type_for_path, is_video_path, parse_http_byte_range, RangeParseOutcome},
    scheduler::RangeCursorGuard,
    EngineInner,
};

#[derive(Debug, Serialize)]
struct ErrorPayload {
    error: ErrorDetail,
}

#[derive(Debug, Serialize)]
struct ErrorDetail {
    message: String,
    code: u16,
}

pub(crate) struct ApiHttpError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiHttpError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: msg.into(),
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
        }
    }

    pub fn timeout(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            message: msg.into(),
        }
    }

    pub fn bad_gateway(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: msg.into(),
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
        }
    }
}

impl IntoResponse for ApiHttpError {
    fn into_response(self) -> Response {
        let code = self.status.as_u16();
        (
            self.status,
            Json(ErrorPayload {
                error: ErrorDetail {
                    message: self.message,
                    code,
                },
            }),
        )
            .into_response()
    }
}

fn is_loopback_host(host: Option<&str>) -> bool {
    matches!(host, Some("127.0.0.1" | "localhost" | "[::1]" | "::1"))
}

fn is_loopback_origin_str(raw: &str) -> bool {
    let Ok(parsed) = Url::parse(raw.trim()) else {
        return false;
    };
    (parsed.scheme() == "http" || parsed.scheme() == "https")
        && is_loopback_host(parsed.host_str())
        && parsed.username().is_empty()
        && parsed.password().is_none()
}

async fn enforce_loopback_origin(req: Request<Body>, next: Next) -> Response {
    if let Some(origin_val) = req.headers().get(header::ORIGIN) {
        let allowed = origin_val.to_str().is_ok_and(is_loopback_origin_str);
        if !allowed {
            return ApiHttpError::forbidden("Non-loopback Origin header is forbidden")
                .into_response();
        }
    }
    next.run(req).await
}

fn build_loopback_cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _parts| {
            origin.to_str().is_ok_and(is_loopback_origin_str)
        }))
        .allow_methods(AllowMethods::list([
            Method::GET,
            Method::HEAD,
            Method::POST,
            Method::OPTIONS,
        ]))
        .allow_headers([
            header::CONTENT_TYPE,
            header::RANGE,
            header::IF_RANGE,
            header::ACCEPT,
        ])
        .expose_headers([
            header::ACCEPT_RANGES,
            header::CONTENT_LENGTH,
            header::CONTENT_RANGE,
            header::CONTENT_TYPE,
        ])
}

pub(crate) fn build_router(state: std::sync::Arc<EngineInner>) -> Router {
    Router::new()
        .route("/settings", get(get_settings).post(post_settings))
        .route("/device-info", get(get_device_info))
        .route("/network-info", get(get_network_info))
        .route("/casting", get(get_casting))
        .route("/create", post(post_create_blob))
        .route("/opensubhash", get(get_opensubhash))
        .route("/opensubHash", get(get_opensubhash))
        .route("/proxy/{opts}", get(proxy_stream).head(proxy_stream))
        .route("/proxy/{opts}/{*path}", get(proxy_stream).head(proxy_stream))
        .route("/{info_hash}/create", post(post_create_magnet))
        .route("/{info_hash}/stats.json", get(get_stats_without_file))
        .route(
            "/{info_hash}/{file_idx}",
            get(stream_torrent_file).head(stream_torrent_file),
        )
        .route("/{info_hash}/{file_idx}/stats.json", get(get_stats_with_file))
        .layer(build_loopback_cors_layer())
        .layer(axum::middleware::from_fn(enforce_loopback_origin))
        .with_state(state)
}

async fn get_settings(State(state): State<std::sync::Arc<EngineInner>>) -> Json<SettingsResponse> {
    Json(state.settings_response())
}

async fn post_settings(
    State(state): State<std::sync::Arc<EngineInner>>,
    Json(patch): Json<serde_json::Map<String, serde_json::Value>>,
) -> Result<Json<SuccessResponse>, ApiHttpError> {
    state
        .apply_settings_patch(patch)
        .await
        .map_err(ApiHttpError::bad_request)?;
    Ok(Json(SuccessResponse { success: True }))
}

async fn get_device_info(State(state): State<std::sync::Arc<EngineInner>>) -> Json<DeviceInfo> {
    Json(state.device_info())
}

async fn get_network_info(State(state): State<std::sync::Arc<EngineInner>>) -> Json<NetworkInfo> {
    Json(state.network_info())
}

async fn get_casting() -> Json<Vec<PlaybackDevice>> {
    Json(Vec::new())
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateMagnetIncoming {
    #[serde(default)]
    peer_search: Option<PeerSearch>,
    #[serde(default)]
    blob: Option<String>,
    #[serde(default)]
    torrent: Option<CreateIncomingTorrentField>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateIncomingTorrentField {
    #[serde(default)]
    info_hash: Option<String>,
    #[serde(default)]
    blob: Option<String>,
}

pub(crate) fn extract_trackers_from_sources(sources: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for raw in sources {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with("dht:") {
            continue;
        }
        let without_prefix = trimmed.strip_prefix("tracker:").unwrap_or(trimmed);
        let decoded = percent_decode_str(without_prefix)
            .decode_utf8_lossy()
            .into_owned();
        let clean = decoded.strip_prefix("tracker:").unwrap_or(&decoded).trim();
        if crate::is_allowed_tracker_url(clean) {
            out.push(clean.to_owned());
        }
    }
    out
}

pub(crate) fn parse_info_hash_param(raw: &str) -> Result<[u8; 20], ApiHttpError> {
    crate::validate_info_hash_str(raw).map_err(ApiHttpError::bad_request)
}

async fn post_create_magnet(
    State(state): State<std::sync::Arc<EngineInner>>,
    Path(info_hash_str): Path<String>,
    body_bytes: Bytes,
) -> Result<Json<Statistics>, ApiHttpError> {
    let info_hash = parse_info_hash_param(&info_hash_str)?;
    let parsed: CreateMagnetIncoming = if body_bytes.is_empty() {
        CreateMagnetIncoming::default()
    } else {
        serde_json::from_slice(&body_bytes)
            .map_err(|e| ApiHttpError::bad_request(format!("Invalid JSON body: {e}")))?
    };

    let trackers = parsed
        .peer_search
        .as_ref()
        .map(|ps| extract_trackers_from_sources(&ps.sources))
        .unwrap_or_default();

    if let Some(blob_hex) = parsed
        .blob
        .or_else(|| parsed.torrent.and_then(|t| t.blob))
    {
        let torrent_bytes = hex::decode(blob_hex.trim())
            .map_err(|e| ApiHttpError::bad_request(format!("Invalid torrent hex blob: {e}")))?;
        let added_hash = state
            .add_torrent_bytes(torrent_bytes, trackers, None)
            .await
            .map_err(|e| ApiHttpError::bad_request(e.to_string()))?;
        let stats = state
            .build_statistics(&added_hash, None)
            .ok_or_else(|| ApiHttpError::internal("Torrent missing after creation"))?;
        return Ok(Json(stats));
    }

    state.register_magnet_and_prefetch(info_hash, trackers);
    let stats = state
        .build_statistics(&info_hash, None)
        .ok_or_else(|| ApiHttpError::internal("Failed building magnet statistics"))?;
    Ok(Json(stats))
}

async fn post_create_blob(
    State(state): State<std::sync::Arc<EngineInner>>,
    Json(payload): Json<CreateMagnetIncoming>,
) -> Result<Json<Statistics>, ApiHttpError> {
    let trackers = payload
        .peer_search
        .as_ref()
        .map(|ps| extract_trackers_from_sources(&ps.sources))
        .unwrap_or_default();

    if let Some(blob_hex) = payload
        .blob
        .or_else(|| payload.torrent.as_ref().and_then(|t| t.blob.clone()))
    {
        let torrent_bytes = hex::decode(blob_hex.trim())
            .map_err(|e| ApiHttpError::bad_request(format!("Invalid torrent hex blob: {e}")))?;
        let info_hash = state
            .add_torrent_bytes(torrent_bytes, trackers, None)
            .await
            .map_err(|e| ApiHttpError::bad_request(e.to_string()))?;
        let stats = state
            .build_statistics(&info_hash, None)
            .ok_or_else(|| ApiHttpError::internal("Torrent missing after blob creation"))?;
        return Ok(Json(stats));
    }

    if let Some(ih_str) = payload.torrent.and_then(|t| t.info_hash) {
        let info_hash = parse_info_hash_param(&ih_str)?;
        state.register_magnet_and_prefetch(info_hash, trackers);
        let stats = state
            .build_statistics(&info_hash, None)
            .ok_or_else(|| ApiHttpError::internal("Failed building magnet statistics"))?;
        return Ok(Json(stats));
    }

    Err(ApiHttpError::bad_request(
        "POST /create requires hex `blob` or `torrent.infoHash`",
    ))
}

async fn get_stats_without_file(
    State(state): State<std::sync::Arc<EngineInner>>,
    Path(info_hash_str): Path<String>,
) -> Result<Json<Statistics>, ApiHttpError> {
    let info_hash = parse_info_hash_param(&info_hash_str)?;
    let stats = state
        .build_statistics(&info_hash, None)
        .ok_or_else(|| ApiHttpError::not_found("Torrent not found"))?;
    Ok(Json(stats))
}

async fn get_stats_with_file(
    State(state): State<std::sync::Arc<EngineInner>>,
    Path((info_hash_str, file_idx)): Path<(String, i32)>,
) -> Result<Json<Statistics>, ApiHttpError> {
    let info_hash = parse_info_hash_param(&info_hash_str)?;
    let resolved_idx = if file_idx < 0 {
        state.resolve_file_idx_from_handle(&info_hash, file_idx, &[])
    } else {
        Some(file_idx as usize)
    };
    let stats = state
        .build_statistics(&info_hash, resolved_idx)
        .ok_or_else(|| ApiHttpError::not_found("Torrent not found"))?;
    Ok(Json(stats))
}

pub(crate) fn select_best_file_index(
    file_infos: &[librqbit::file_info::FileInfo],
    requested_idx: i32,
    file_must_include: &[String],
) -> Result<usize, ApiHttpError> {
    if file_infos.is_empty() {
        return Err(ApiHttpError::not_found("Torrent contains no files"));
    }

    if requested_idx >= 0 {
        let idx = requested_idx as usize;
        if idx >= file_infos.len() {
            return Err(ApiHttpError::not_found(format!(
                "file_idx {idx} out of bounds (torrent has {} files)",
                file_infos.len()
            )));
        }
        return Ok(idx);
    }

    if requested_idx < -1 {
        return Err(ApiHttpError::bad_request(format!(
            "Invalid negative file_idx {requested_idx}; only -1 is supported for auto-selection"
        )));
    }

    if !file_must_include.is_empty() {
        let needles: Vec<String> = file_must_include
            .iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();

        if !needles.is_empty() {
            let match_all = file_infos
                .iter()
                .enumerate()
                .filter(|(_, fi)| !fi.attrs.padding)
                .filter(|(_, fi)| {
                    let full = fi.relative_filename.to_string_lossy().to_lowercase();
                    needles.iter().all(|n| full.contains(n))
                })
                .max_by_key(|(_, fi)| (is_video_path(&fi.relative_filename), fi.len));

            if let Some((idx, _)) = match_all {
                return Ok(idx);
            }

            let match_any = file_infos
                .iter()
                .enumerate()
                .filter(|(_, fi)| !fi.attrs.padding)
                .filter(|(_, fi)| {
                    let full = fi.relative_filename.to_string_lossy().to_lowercase();
                    needles.iter().any(|n| full.contains(n))
                })
                .max_by_key(|(_, fi)| (is_video_path(&fi.relative_filename), fi.len));

            if let Some((idx, _)) = match_any {
                return Ok(idx);
            }
        }
    }

    if let Some((idx, _)) = file_infos
        .iter()
        .enumerate()
        .filter(|(_, fi)| !fi.attrs.padding && is_video_path(&fi.relative_filename))
        .max_by_key(|(_, fi)| fi.len)
    {
        return Ok(idx);
    }

    file_infos
        .iter()
        .enumerate()
        .filter(|(_, fi)| !fi.attrs.padding)
        .max_by_key(|(_, fi)| fi.len)
        .map(|(idx, _)| idx)
        .ok_or_else(|| ApiHttpError::not_found("No playable non-padding files in torrent"))
}

struct TrackedFileStream<R> {
    inner: Take<R>,
    cursor: RangeCursorGuard,
    engine: std::sync::Arc<EngineInner>,
    info_hash: [u8; 20],
}

impl<R: AsyncRead + Unpin> AsyncRead for TrackedFileStream<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &poll {
            let delta = buf.filled().len().saturating_sub(before);
            self.cursor.record_read(delta);
        }
        poll
    }
}

impl<R> Drop for TrackedFileStream<R> {
    fn drop(&mut self) {
        self.engine.update_torrent_cache_size_sync(&self.info_hash);
    }
}

async fn stream_torrent_file(
    State(state): State<std::sync::Arc<EngineInner>>,
    method: Method,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Path((info_hash_str, file_idx)): Path<(String, i32)>,
) -> Result<Response, ApiHttpError> {
    let info_hash = parse_info_hash_param(&info_hash_str)?;

    let mut query_trackers = Vec::new();
    let mut file_must_include = Vec::new();
    if let Some(q) = uri.query() {
        for (k, v) in url::form_urlencoded::parse(q.as_bytes()) {
            match k.as_ref() {
                "tr" => query_trackers.push(v.into_owned()),
                "f" => file_must_include.push(v.into_owned()),
                _ => {}
            }
        }
    }
    let normalized_trackers = extract_trackers_from_sources(&query_trackers);

    let handle = state
        .ensure_torrent_resolved(info_hash, normalized_trackers)
        .await?;

    let (resolved_idx, file_len, rel_path) = handle
        .with_metadata(|meta| {
            let idx = select_best_file_index(&meta.file_infos, file_idx, &file_must_include)?;
            let fi = &meta.file_infos[idx];
            Ok::<_, ApiHttpError>((idx, fi.len, fi.relative_filename.clone()))
        })
        .map_err(|e| ApiHttpError::internal(e.to_string()))??;

    state
        .activate_and_touch_episode(&handle, info_hash, resolved_idx)
        .await
        .map_err(|e| ApiHttpError::internal(e.to_string()))?;

    let content_type = content_type_for_path(&rel_path);
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());

    let (status, start_offset, content_len, content_range) = match range_header {
        Some(raw_range) => match parse_http_byte_range(raw_range, file_len) {
            RangeParseOutcome::Partial(r) => (
                StatusCode::PARTIAL_CONTENT,
                r.start,
                r.content_length(),
                Some(r.content_range_header()),
            ),
            RangeParseOutcome::Ignore => (StatusCode::OK, 0, file_len, None),
            RangeParseOutcome::Unsatisfiable { total_len } => {
                let mut resp = Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::ACCEPT_RANGES, "bytes")
                    .header(header::CONTENT_RANGE, format!("bytes */{total_len}"))
                    .header(header::CONTENT_LENGTH, "0")
                    .body(Body::empty())
                    .map_err(|e| {
                        ApiHttpError::internal(format!("Failed building 416 response: {e}"))
                    })?;
                resp.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_str(&content_type)
                        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
                );
                return Ok(resp);
            }
        },
        None => (StatusCode::OK, 0, file_len, None),
    };

    let mut builder = Response::builder()
        .status(status)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, content_len.to_string())
        .header(header::CONTENT_TYPE, &content_type);

    if let Some(cr) = content_range {
        builder = builder.header(header::CONTENT_RANGE, cr);
    }

    if method == Method::HEAD || content_len == 0 {
        return builder
            .body(Body::empty())
            .map_err(|e| ApiHttpError::internal(format!("Failed building HEAD response: {e}")));
    }

    let mut file_stream = handle
        .clone()
        .stream(resolved_idx)
        .await
        .map_err(|e| {
            ApiHttpError::internal(format!("Failed opening torrent file stream: {e:#}"))
        })?;

    if start_offset > 0 {
        file_stream
            .seek(std::io::SeekFrom::Start(start_offset))
            .await
            .map_err(|e| ApiHttpError::internal(format!("Failed seeking torrent stream: {e}")))?;
    }

    let cursor_guard =
        state
            .scheduler
            .open_range_cursor(info_hash, resolved_idx, start_offset, content_len);

    let mut tracked = TrackedFileStream {
        inner: file_stream.take(content_len),
        cursor: cursor_guard,
        engine: std::sync::Arc::clone(&state),
        info_hash,
    };

    let first_byte_timeout = state.first_byte_timeout();
    let initial_cap = (64 * 1024).min(content_len) as usize;
    let mut initial_buf = vec![0u8; initial_cap];
    let first_read = tokio::time::timeout(first_byte_timeout, tracked.read(&mut initial_buf))
        .await
        .map_err(|_| {
            ApiHttpError::timeout(format!(
                "Timed out after {:?} waiting for initial playhead bytes ({})",
                first_byte_timeout,
                hex::encode(info_hash)
            ))
        })?
        .map_err(|e| ApiHttpError::bad_gateway(format!("Torrent stream read error: {e}")))?;

    if first_read == 0 {
        return Err(ApiHttpError::bad_gateway(
            "Torrent stream returned unexpected EOF before delivering initial byte",
        ));
    }

    initial_buf.truncate(first_read);
    let first_chunk = Bytes::from(initial_buf);
    state.refresh_torrent_cache_accounting(&info_hash).await;

    let body_stream = futures::stream::once(std::future::ready(Ok::<Bytes, std::io::Error>(
        first_chunk,
    )))
    .chain(ReaderStream::with_capacity(tracked, 64 * 1024));

    debug!(
        info_hash = %hex::encode(info_hash),
        file_idx = resolved_idx,
        start_offset,
        content_len,
        first_read,
        %status,
        "serving torrent HTTP stream"
    );

    builder
        .body(Body::from_stream(body_stream))
        .map_err(|e| ApiHttpError::internal(format!("Failed building stream response: {e}")))
}

fn map_opensubhash_err(err: &OpensubHashError) -> ApiHttpError {
    match err {
        OpensubHashError::FileTooSmall(_) | OpensubHashError::InvalidTarget(_) => {
            ApiHttpError::bad_request(err.to_string())
        }
        OpensubHashError::Io(io_err) if io_err.kind() == std::io::ErrorKind::NotFound => {
            ApiHttpError::not_found("File not found")
        }
        OpensubHashError::Io(_) => ApiHttpError::internal("Failed reading local stream file"),
        OpensubHashError::Http(_) => ApiHttpError::bad_gateway(err.to_string()),
    }
}

async fn resolve_confined_local_opensubhash_path(
    state: &EngineInner,
    raw_path: &FsPath,
) -> Result<PathBuf, ApiHttpError> {
    if !crate::is_safe_path_no_traversal(raw_path) {
        return Err(ApiHttpError::bad_request("Unsafe local file path"));
    }

    let allowed_raw = [
        state.cache_tracker.disk_cache_dir().to_path_buf(),
        state.cache_tracker.ram_cache_dir().to_path_buf(),
        PathBuf::from(&state.settings.read().cache_root),
    ];

    let mut allowed_canon = Vec::with_capacity(allowed_raw.len());
    for root in &allowed_raw {
        if let Ok(canon) = tokio::fs::canonicalize(root).await {
            allowed_canon.push(canon);
        }
    }

    let canon_target = match tokio::fs::canonicalize(raw_path).await {
        Ok(p) => p,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let lexically_inside = allowed_raw
                .iter()
                .any(|root| raw_path.starts_with(root) && raw_path != root);
            if lexically_inside {
                return Err(ApiHttpError::not_found("File not found"));
            }
            return Err(ApiHttpError::forbidden(
                "Local opensubhash path must reside inside active cache directory",
            ));
        }
        Err(_) => {
            return Err(ApiHttpError::forbidden(
                "Local opensubhash path must reside inside active cache directory",
            ));
        }
    };

    let inside_allowed = allowed_canon
        .iter()
        .any(|root| canon_target.starts_with(root) && &canon_target != root);
    if !inside_allowed {
        return Err(ApiHttpError::forbidden(
            "Local opensubhash path must reside inside active cache directory",
        ));
    }

    Ok(canon_target)
}

async fn get_opensubhash(
    State(state): State<std::sync::Arc<EngineInner>>,
    OriginalUri(uri): OriginalUri,
) -> Result<Json<OpensubHashResponse>, ApiHttpError> {
    let mut target_url = None;
    if let Some(q) = uri.query() {
        for (k, v) in url::form_urlencoded::parse(q.as_bytes()) {
            if k == "url" {
                target_url = Some(v.into_owned());
                break;
            }
        }
    }
    let raw_target = target_url
        .ok_or_else(|| ApiHttpError::bad_request("Missing required `url` query parameter"))?;
    let trimmed = raw_target.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(ApiHttpError::bad_request("Invalid `url` query parameter"));
    }

    let result = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let parsed = Url::parse(trimmed)
            .map_err(|_| ApiHttpError::bad_request("Invalid HTTP URL parameter"))?;
        compute_opensubhash_http(&state.http_client, &parsed)
            .await
            .map_err(|e| map_opensubhash_err(&e))?
    } else if trimmed.starts_with("file://") {
        let parsed = Url::parse(trimmed)
            .map_err(|_| ApiHttpError::bad_request("Invalid file URL parameter"))?;
        let local_path = parsed
            .to_file_path()
            .map_err(|_| ApiHttpError::bad_request("Cannot convert file URL to local path"))?;
        let confined = resolve_confined_local_opensubhash_path(&state, &local_path).await?;
        compute_opensubhash_path(&confined)
            .await
            .map_err(|e| map_opensubhash_err(&e))?
    } else {
        if let Some((scheme, _)) = trimmed.split_once("://") {
            if !scheme.is_empty()
                && scheme
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.')
            {
                return Err(ApiHttpError::bad_request(format!(
                    "Unsupported URL scheme `{scheme}`"
                )));
            }
        }
        let confined = resolve_confined_local_opensubhash_path(&state, FsPath::new(trimmed)).await?;
        compute_opensubhash_path(&confined)
            .await
            .map_err(|e| map_opensubhash_err(&e))?
    };

    Ok(Json(OpensubHashResponse { result }))
}

#[derive(Debug, Default)]
struct ParsedProxyOpts {
    origin: Option<String>,
    request_headers: Vec<(String, String)>,
    response_headers: Vec<(String, String)>,
}

fn parse_proxy_opts_kv(raw: &str) -> ParsedProxyOpts {
    let mut out = ParsedProxyOpts::default();
    for (k, v) in url::form_urlencoded::parse(raw.as_bytes()) {
        match k.as_ref() {
            "d" => out.origin = Some(v.into_owned()),
            "h" => {
                if let Some((hk, hv)) = v.split_once(':') {
                    out.request_headers
                        .push((hk.trim().to_owned(), hv.trim_start().to_owned()));
                }
            }
            "r" => {
                if let Some((rk, rv)) = v.split_once(':') {
                    out.response_headers
                        .push((rk.trim().to_owned(), rv.trim_start().to_owned()));
                }
            }
            _ => {}
        }
    }
    out
}

async fn proxy_stream(
    State(state): State<std::sync::Arc<EngineInner>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    req: Request<Body>,
) -> Result<Response, ApiHttpError> {
    let raw_path = uri.path();
    let after_prefix = raw_path
        .strip_prefix("/proxy/")
        .ok_or_else(|| ApiHttpError::bad_request("Invalid proxy path"))?;

    let (raw_opts, raw_rest_path) = match after_prefix.split_once('/') {
        Some((opts, rest)) => (opts, rest),
        None => (after_prefix, ""),
    };

    let mut parsed_opts = parse_proxy_opts_kv(raw_opts);
    if parsed_opts.origin.is_none() {
        let decoded_once = percent_decode_str(raw_opts).decode_utf8_lossy();
        parsed_opts = parse_proxy_opts_kv(&decoded_once);
    }

    let origin_str = parsed_opts
        .origin
        .ok_or_else(|| ApiHttpError::bad_request("Proxy options missing `d=` destination origin"))?;

    let base_origin = Url::parse(&origin_str)
        .map_err(|_| ApiHttpError::bad_request("Invalid proxy `d=` origin URL"))?;
    crate::validate_outbound_http_url(&base_origin).map_err(ApiHttpError::bad_request)?;

    let mut target_url_str = format!(
        "{}/{}",
        origin_str.trim_end_matches('/'),
        raw_rest_path.trim_start_matches('/')
    );
    if let Some(query) = uri.query() {
        target_url_str.push('?');
        target_url_str.push_str(query);
    }

    let target_url = Url::parse(&target_url_str)
        .map_err(|_| ApiHttpError::bad_request("Invalid target proxy URL"))?;
    crate::validate_outbound_http_url(&target_url).map_err(ApiHttpError::bad_request)?;

    if is_loopback_host(target_url.host_str())
        && target_url.port_or_known_default() == Some(state.local_addr.port())
        && target_url.path().starts_with("/proxy")
    {
        return Err(ApiHttpError::forbidden(
            "Recursive self-proxying is forbidden",
        ));
    }

    let redacted_target = crate::redact_url(&target_url);

    let reqwest_method = match method {
        Method::HEAD => reqwest::Method::HEAD,
        _ => reqwest::Method::GET,
    };

    let mut upstream_req = state.http_client.request(reqwest_method, target_url);

    for forward_hdr in [
        header::RANGE,
        header::IF_RANGE,
        header::ACCEPT,
        header::USER_AGENT,
    ] {
        if let Some(val) = req.headers().get(&forward_hdr) {
            upstream_req = upstream_req.header(forward_hdr, val.clone());
        }
    }

    for (hk, hv) in &parsed_opts.request_headers {
        match (
            HeaderName::from_bytes(hk.as_bytes()),
            HeaderValue::from_str(hv),
        ) {
            (Ok(name), Ok(val)) => {
                upstream_req = upstream_req.header(name, val);
            }
            _ => warn!(header_name = %hk, "ignoring invalid `h=` proxy request header"),
        }
    }

    let upstream_resp = upstream_req.send().await.map_err(|e| {
        ApiHttpError::bad_gateway(format!(
            "Proxy upstream request to {redacted_target} failed: {}",
            e.without_url()
        ))
    })?;

    let status = upstream_resp.status();
    let mut resp_builder = Response::builder().status(status);
    if let Some(resp_headers) = resp_builder.headers_mut() {
        for pass_hdr in [
            header::CONTENT_TYPE,
            header::CONTENT_LENGTH,
            header::CONTENT_RANGE,
            header::ACCEPT_RANGES,
            header::ETAG,
            header::LAST_MODIFIED,
            header::CACHE_CONTROL,
        ] {
            if let Some(val) = upstream_resp.headers().get(&pass_hdr) {
                resp_headers.insert(pass_hdr, val.clone());
            }
        }

        for (rk, rv) in &parsed_opts.response_headers {
            if let (Ok(name), Ok(val)) = (
                HeaderName::from_bytes(rk.as_bytes()),
                HeaderValue::from_str(rv),
            ) {
                resp_headers.insert(name, val);
            }
        }
    }

    if method == Method::HEAD {
        return resp_builder.body(Body::empty()).map_err(|e| {
            ApiHttpError::internal(format!("Failed building proxied HEAD response: {e}"))
        });
    }

    let stream = upstream_resp.bytes_stream();
    resp_builder.body(Body::from_stream(stream)).map_err(|e| {
        ApiHttpError::internal(format!("Failed building proxied stream response: {e}"))
    })
}
