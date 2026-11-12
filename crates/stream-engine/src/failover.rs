use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use http::header::{HeaderName, HeaderValue, RANGE};
use serde::{Deserialize, Serialize};
use stremio_core::types::{
    resource::{Stream as StremioStream, StreamProxyHeaders, StreamSource},
    torrent::InfoHash,
};
use tokio::io::AsyncReadExt;
use tracing::{debug, info, warn};
use url::Url;

use crate::{routes, EngineInner, StreamEngineError};

pub const DEFAULT_FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone)]
pub enum StreamCandidateKind {
    Http {
        url: Url,
        request_headers: Vec<(String, String)>,
        response_headers: Vec<(String, String)>,
    },
    Torrent {
        info_hash: [u8; 20],
        file_idx: Option<usize>,
        trackers: Vec<String>,
        file_must_include: Vec<String>,
        torrent_bytes: Option<Vec<u8>>,
        initial_peers: Option<Vec<SocketAddr>>,
    },
}

#[derive(Debug, Clone)]
pub struct StreamCandidate {
    pub label: Option<String>,
    pub kind: StreamCandidateKind,
}

impl StreamCandidate {
    pub fn http(url: Url) -> Self {
        Self {
            label: None,
            kind: StreamCandidateKind::Http {
                url,
                request_headers: Vec::new(),
                response_headers: Vec::new(),
            },
        }
    }

    pub fn http_with_headers(url: Url, request_headers: Vec<(String, String)>) -> Self {
        Self {
            label: None,
            kind: StreamCandidateKind::Http {
                url,
                request_headers,
                response_headers: Vec::new(),
            },
        }
    }

    pub fn torrent(
        info_hash: [u8; 20],
        file_idx: Option<usize>,
        trackers: Vec<String>,
    ) -> Self {
        Self {
            label: None,
            kind: StreamCandidateKind::Torrent {
                info_hash,
                file_idx,
                trackers,
                file_must_include: Vec::new(),
                torrent_bytes: None,
                initial_peers: None,
            },
        }
    }

    pub fn torrent_with_bytes(
        info_hash: [u8; 20],
        torrent_bytes: Vec<u8>,
        file_idx: Option<usize>,
        initial_peers: Option<Vec<SocketAddr>>,
    ) -> Self {
        Self {
            label: None,
            kind: StreamCandidateKind::Torrent {
                info_hash,
                file_idx,
                trackers: Vec::new(),
                file_must_include: Vec::new(),
                torrent_bytes: Some(torrent_bytes),
                initial_peers,
            },
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_file_must_include(mut self, patterns: Vec<String>) -> Self {
        if let StreamCandidateKind::Torrent {
            ref mut file_must_include,
            ..
        } = self.kind
        {
            *file_must_include = patterns;
        }
        self
    }

    pub fn from_stremio_stream(stream: &StremioStream) -> Option<Self> {
        let label = match (&stream.name, &stream.description) {
            (Some(n), Some(d)) => Some(
                format!("{} - {}", n.trim(), d.lines().next().unwrap_or(""))
                    .trim()
                    .to_owned(),
            ),
            (Some(n), None) => Some(n.trim().to_owned()),
            (None, Some(d)) => Some(d.lines().next().unwrap_or("").trim().to_owned()),
            (None, None) => None,
        };

        match &stream.source {
            StreamSource::Url { url } => {
                if crate::validate_outbound_http_url(url).is_err() {
                    return None;
                }
                let (req_hdrs, resp_hdrs) = match &stream.behavior_hints.proxy_headers {
                    Some(StreamProxyHeaders { request, response }) => (
                        request
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                        response
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                    ),
                    None => (Vec::new(), Vec::new()),
                };
                Some(Self {
                    label,
                    kind: StreamCandidateKind::Http {
                        url: url.clone(),
                        request_headers: req_hdrs,
                        response_headers: resp_hdrs,
                    },
                })
            }
            StreamSource::Torrent {
                info_hash,
                file_idx,
                announce,
                file_must_include,
            } => Some(Self {
                label,
                kind: StreamCandidateKind::Torrent {
                    info_hash: *info_hash,
                    file_idx: file_idx.map(usize::from),
                    trackers: routes::extract_trackers_from_sources(announce),
                    file_must_include: file_must_include.clone(),
                    torrent_bytes: None,
                    initial_peers: None,
                },
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedCandidate {
    pub index: usize,
    pub label: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStream {
    pub candidate_index: usize,
    pub label: Option<String>,
    pub playback_url: Url,
    pub info_hash: Option<InfoHash>,
    pub resolved_file_idx: Option<usize>,
    pub failed_attempts: Vec<FailedCandidate>,
}

pub(crate) async fn execute_probe_or_failover(
    engine: &Arc<EngineInner>,
    candidates: &[StreamCandidate],
    per_candidate_timeout: Duration,
) -> Result<ResolvedStream, StreamEngineError> {
    if candidates.is_empty() {
        return Err(StreamEngineError::AllCandidatesFailed {
            attempts: Vec::new(),
        });
    }

    let mut failed_attempts = Vec::new();

    for (idx, candidate) in candidates.iter().enumerate() {
        let label = candidate.label.clone();
        match &candidate.kind {
            StreamCandidateKind::Http {
                url,
                request_headers,
                response_headers,
            } => {
                match tokio::time::timeout(
                    per_candidate_timeout,
                    probe_http_candidate(engine, url, request_headers, response_headers),
                )
                .await
                {
                    Ok(Ok(playback_url)) => {
                        if idx > 0 {
                            info!(
                                candidate_index = idx,
                                ?label,
                                playback_url = %crate::redact_url(&playback_url),
                                skipped = failed_attempts.len(),
                                "auto-failover resolved responsive HTTP stream"
                            );
                        }
                        return Ok(ResolvedStream {
                            candidate_index: idx,
                            label,
                            playback_url,
                            info_hash: None,
                            resolved_file_idx: None,
                            failed_attempts,
                        });
                    }
                    Ok(Err(reason)) => {
                        warn!(
                            candidate_index = idx,
                            ?label,
                            %reason,
                            "HTTP stream candidate failed probe"
                        );
                        failed_attempts.push(FailedCandidate {
                            index: idx,
                            label,
                            reason,
                        });
                    }
                    Err(_) => {
                        let reason = format!(
                            "HTTP probe to {} timed out after {:?}",
                            crate::redact_url(url),
                            per_candidate_timeout
                        );
                        warn!(
                            candidate_index = idx,
                            ?label,
                            %reason,
                            "HTTP stream candidate timed out"
                        );
                        failed_attempts.push(FailedCandidate {
                            index: idx,
                            label,
                            reason,
                        });
                    }
                }
            }
            StreamCandidateKind::Torrent {
                info_hash,
                file_idx,
                trackers,
                file_must_include,
                torrent_bytes,
                initial_peers,
            } => {
                let was_preexisting = engine.get_managed_torrent(info_hash).is_some();
                let probe_res = tokio::time::timeout(
                    per_candidate_timeout,
                    probe_torrent_candidate(
                        engine,
                        *info_hash,
                        *file_idx,
                        trackers.clone(),
                        file_must_include.clone(),
                        torrent_bytes.clone(),
                        initial_peers.clone(),
                    ),
                )
                .await;

                match probe_res {
                    Ok(Ok((playback_url, resolved_idx))) => {
                        if idx > 0 {
                            info!(
                                candidate_index = idx,
                                ?label,
                                info_hash = %hex::encode(info_hash),
                                resolved_idx,
                                skipped = failed_attempts.len(),
                                "auto-failover resolved responsive BitTorrent stream"
                            );
                        }
                        return Ok(ResolvedStream {
                            candidate_index: idx,
                            label,
                            playback_url,
                            info_hash: Some(InfoHash::new(*info_hash)),
                            resolved_file_idx: Some(resolved_idx),
                            failed_attempts,
                        });
                    }
                    Ok(Err(reason)) => {
                        warn!(
                            candidate_index = idx,
                            ?label,
                            info_hash = %hex::encode(info_hash),
                            %reason,
                            "BitTorrent candidate failed probe; cleaning up stalled swarm"
                        );
                        if !was_preexisting
                            && !engine.scheduler.has_active_streams_for_torrent(info_hash)
                        {
                            engine.evict_torrent_completely(*info_hash).await;
                        }
                        failed_attempts.push(FailedCandidate {
                            index: idx,
                            label,
                            reason,
                        });
                    }
                    Err(_) => {
                        let reason = format!(
                            "Dead/stalled swarm timed out after {:?}",
                            per_candidate_timeout
                        );
                        warn!(
                            candidate_index = idx,
                            ?label,
                            info_hash = %hex::encode(info_hash),
                            %reason,
                            "BitTorrent candidate timed out; cleaning up stalled swarm"
                        );
                        if !was_preexisting
                            && !engine.scheduler.has_active_streams_for_torrent(info_hash)
                        {
                            engine.evict_torrent_completely(*info_hash).await;
                        }
                        failed_attempts.push(FailedCandidate {
                            index: idx,
                            label,
                            reason,
                        });
                    }
                }
            }
        }
    }

    Err(StreamEngineError::AllCandidatesFailed {
        attempts: failed_attempts,
    })
}

async fn probe_http_candidate(
    engine: &EngineInner,
    url: &Url,
    request_headers: &[(String, String)],
    response_headers: &[(String, String)],
) -> Result<Url, String> {
    crate::validate_outbound_http_url(url)?;
    let redacted = crate::redact_url(url);

    let mut range_req = engine
        .http_client
        .get(url.as_str())
        .header(RANGE, "bytes=0-0");
    for (k, v) in request_headers {
        if let (Ok(hn), Ok(hv)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v))
        {
            range_req = range_req.header(hn, hv);
        }
    }

    let resp = range_req
        .send()
        .await
        .map_err(|e| format!("HTTP request to {redacted} failed: {}", e.without_url()))?;

    let status = resp.status();
    if status == reqwest::StatusCode::METHOD_NOT_ALLOWED
        || status == reqwest::StatusCode::NOT_IMPLEMENTED
    {
        let mut head_req = engine.http_client.head(url.as_str());
        for (k, v) in request_headers {
            if let (Ok(hn), Ok(hv)) =
                (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v))
            {
                head_req = head_req.header(hn, hv);
            }
        }
        let head_resp = head_req
            .send()
            .await
            .map_err(|e| format!("HTTP HEAD to {redacted} failed: {}", e.without_url()))?;
        if !head_resp.status().is_success() {
            return Err(format!(
                "Upstream {redacted} HEAD returned HTTP {}",
                head_resp.status()
            ));
        }
    } else if status.is_success() {
        let mut stream = resp.bytes_stream();
        match stream.next().await {
            Some(Ok(_)) | None => {}
            Some(Err(e)) => {
                return Err(format!(
                    "Upstream {redacted} body read error: {}",
                    e.without_url()
                ));
            }
        }
    } else {
        return Err(format!("Upstream {redacted} returned HTTP {status}"));
    }

    if request_headers.is_empty() && response_headers.is_empty() {
        return Ok(url.clone());
    }

    build_local_proxy_url(&engine.base_url, url, request_headers, response_headers)
}

fn build_local_proxy_url(
    base_url: &Url,
    target: &Url,
    request_headers: &[(String, String)],
    response_headers: &[(String, String)],
) -> Result<Url, String> {
    let origin = format!(
        "{}://{}{}",
        target.scheme(),
        target.host_str().unwrap_or(""),
        target
            .port()
            .map(|p| format!(":{p}"))
            .unwrap_or_default()
    );
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("d", &origin);
    for (k, v) in request_headers {
        serializer.append_pair("h", &format!("{k}:{v}"));
    }
    for (k, v) in response_headers {
        serializer.append_pair("r", &format!("{k}:{v}"));
    }
    let opts = serializer.finish();
    let path = target.path().trim_start_matches('/');
    let mut built = if path.is_empty() {
        base_url
            .join(&format!("proxy/{opts}"))
            .map_err(|e| e.to_string())?
    } else {
        base_url
            .join(&format!("proxy/{opts}/{path}"))
            .map_err(|e| e.to_string())?
    };
    built.set_query(target.query());
    Ok(built)
}

async fn probe_torrent_candidate(
    engine: &Arc<EngineInner>,
    info_hash: [u8; 20],
    file_idx: Option<usize>,
    trackers: Vec<String>,
    file_must_include: Vec<String>,
    torrent_bytes: Option<Vec<u8>>,
    initial_peers: Option<Vec<SocketAddr>>,
) -> Result<(Url, usize), String> {
    let handle = if let Some(bytes) = torrent_bytes {
        let added = engine
            .add_torrent_bytes(bytes, trackers.clone(), initial_peers)
            .await
            .map_err(|e| e.to_string())?;
        engine
            .get_managed_torrent(&added)
            .ok_or_else(|| "Torrent handle missing after add_torrent_bytes".to_owned())?
    } else {
        engine
            .ensure_torrent_resolved(info_hash, trackers)
            .await
            .map_err(|e| e.message)?
    };

    let req_idx = file_idx.map(|i| i as i32).unwrap_or(-1);
    let (resolved_idx, file_len) = handle
        .with_metadata(|meta| {
            let idx =
                routes::select_best_file_index(&meta.file_infos, req_idx, &file_must_include)
                    .map_err(|e| e.message)?;
            Ok::<_, String>((idx, meta.file_infos[idx].len))
        })
        .map_err(|e| e.to_string())??;

    engine
        .activate_and_touch_episode(&handle, info_hash, resolved_idx)
        .await
        .map_err(|e| e.to_string())?;

    if file_len > 0 {
        let mut file_stream = handle
            .clone()
            .stream(resolved_idx)
            .await
            .map_err(|e| format!("Failed opening torrent stream: {e:#}"))?;
        let mut probe_buf = [0u8; 1024];
        let to_read = (probe_buf.len() as u64).min(file_len) as usize;
        let n = file_stream
            .read(&mut probe_buf[..to_read])
            .await
            .map_err(|e| format!("Failed reading initial head bytes: {e}"))?;
        if n == 0 {
            return Err("Torrent stream returned unexpected EOF on first piece".to_owned());
        }
        debug!(
            info_hash = %hex::encode(info_hash),
            resolved_idx,
            first_bytes = n,
            "torrent probe verified metadata and first head piece"
        );
    }

    engine.refresh_torrent_cache_accounting(&info_hash).await;

    let playback_url = engine
        .base_url
        .join(&format!("{}/{resolved_idx}", hex::encode(info_hash)))
        .map_err(|e| e.to_string())?;

    Ok((playback_url, resolved_idx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use stremio_core::types::resource::StreamBehaviorHints;

    #[test]
    fn converts_stremio_url_and_torrent_streams_into_candidates() {
        let mut req_headers = HashMap::new();
        req_headers.insert("Authorization".into(), "Bearer secret+token".into());

        let debrid_stream = StremioStream {
            source: StreamSource::Url {
                url: Url::parse("https://cdn.debrid.example/video/movie.mkv?sig=1").unwrap(),
            },
            name: Some("Torrentio RD+".into()),
            description: Some("Movie.2026.2160p.REMUX.mkv\n👤 42".into()),
            thumbnail: None,
            subtitles: vec![],
            behavior_hints: StreamBehaviorHints {
                proxy_headers: Some(StreamProxyHeaders {
                    request: req_headers,
                    response: HashMap::new(),
                }),
                ..Default::default()
            },
        };

        let cand_http = StreamCandidate::from_stremio_stream(&debrid_stream).unwrap();
        assert_eq!(
            cand_http.label.as_deref(),
            Some("Torrentio RD+ - Movie.2026.2160p.REMUX.mkv")
        );
        match cand_http.kind {
            StreamCandidateKind::Http {
                url,
                request_headers,
                ..
            } => {
                assert_eq!(url.host_str(), Some("cdn.debrid.example"));
                assert_eq!(request_headers.len(), 1);
            }
            _ => panic!("expected Http candidate"),
        }

        let ih = [0xABu8; 20];
        let torrent_stream = StremioStream {
            source: StreamSource::Torrent {
                info_hash: ih,
                file_idx: Some(2),
                announce: vec![
                    "tracker:udp://tracker.opentrackr.org:1337/announce".into(),
                    "dht:abababababababababababababababababababab".into(),
                ],
                file_must_include: vec!["S01E03".into()],
            },
            name: Some("Torrentio".into()),
            description: Some("Show.S01E03.1080p.WEB-DL.mkv".into()),
            thumbnail: None,
            subtitles: vec![],
            behavior_hints: StreamBehaviorHints::default(),
        };

        let cand_bt = StreamCandidate::from_stremio_stream(&torrent_stream).unwrap();
        match cand_bt.kind {
            StreamCandidateKind::Torrent {
                info_hash,
                file_idx,
                trackers,
                file_must_include,
                ..
            } => {
                assert_eq!(info_hash, ih);
                assert_eq!(file_idx, Some(2));
                assert_eq!(
                    trackers,
                    vec!["udp://tracker.opentrackr.org:1337/announce".to_owned()]
                );
                assert_eq!(file_must_include, vec!["S01E03".to_owned()]);
            }
            _ => panic!("expected Torrent candidate"),
        }
    }
}
