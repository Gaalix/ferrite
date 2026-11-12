use std::io::SeekFrom;
use std::path::Path;

use futures::StreamExt;
use http::StatusCode;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use url::Url;

pub const OPENSUB_HASH_CHUNK_SIZE: usize = 65_536;
const FALLBACK_MAX_FULL_STREAM_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpensubHashResult {
    pub hash: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpensubHashResponse {
    pub result: OpensubHashResult,
}

#[derive(Debug, Error)]
pub enum OpensubHashError {
    #[error("File too small to hash ({0} bytes; minimum 65536 bytes required)")]
    FileTooSmall(u64),
    #[error("I/O error while hashing file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid URL or path: {0}")]
    InvalidTarget(String),
    #[error("HTTP error while hashing stream: {0}")]
    Http(String),
}

pub fn compute_opensubhash_from_chunks(
    file_size: u64,
    head_64k: &[u8; OPENSUB_HASH_CHUNK_SIZE],
    tail_64k: &[u8; OPENSUB_HASH_CHUNK_SIZE],
) -> String {
    let mut hash = file_size;
    for word in head_64k.as_chunks::<8>().0 {
        let val = u64::from_le_bytes(*word);
        hash = hash.wrapping_add(val);
    }
    for word in tail_64k.as_chunks::<8>().0 {
        let val = u64::from_le_bytes(*word);
        hash = hash.wrapping_add(val);
    }
    format!("{hash:016x}")
}

pub fn compute_opensubhash_bytes(data: &[u8]) -> Result<OpensubHashResult, OpensubHashError> {
    let size = data.len() as u64;
    let Some(head) = data.first_chunk::<OPENSUB_HASH_CHUNK_SIZE>() else {
        return Err(OpensubHashError::FileTooSmall(size));
    };
    let Some(tail) = data.last_chunk::<OPENSUB_HASH_CHUNK_SIZE>() else {
        return Err(OpensubHashError::FileTooSmall(size));
    };
    Ok(OpensubHashResult {
        hash: compute_opensubhash_from_chunks(size, head, tail),
        size,
    })
}

pub async fn compute_opensubhash_path(path: &Path) -> Result<OpensubHashResult, OpensubHashError> {
    if !crate::is_safe_path_no_traversal(path) {
        return Err(OpensubHashError::InvalidTarget(
            "unsafe filesystem path".to_owned(),
        ));
    }
    let mut file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    if size < OPENSUB_HASH_CHUNK_SIZE as u64 {
        return Err(OpensubHashError::FileTooSmall(size));
    }

    let mut head = [0u8; OPENSUB_HASH_CHUNK_SIZE];
    file.read_exact(&mut head).await?;

    file.seek(SeekFrom::Start(size - OPENSUB_HASH_CHUNK_SIZE as u64))
        .await?;
    let mut tail = [0u8; OPENSUB_HASH_CHUNK_SIZE];
    file.read_exact(&mut tail).await?;

    Ok(OpensubHashResult {
        hash: compute_opensubhash_from_chunks(size, &head, &tail),
        size,
    })
}

fn parse_total_from_content_range(header_val: &str) -> Option<u64> {
    let (_, total_str) = header_val.trim().rsplit_once('/')?;
    if total_str == "*" {
        return None;
    }
    total_str.parse::<u64>().ok()
}

pub async fn compute_opensubhash_http(
    client: &reqwest::Client,
    url: &Url,
) -> Result<OpensubHashResult, OpensubHashError> {
    crate::validate_outbound_http_url(url).map_err(OpensubHashError::InvalidTarget)?;
    let redacted = crate::redact_url(url);
    let head_end = OPENSUB_HASH_CHUNK_SIZE - 1;
    let head_resp = client
        .get(url.as_str())
        .header(http::header::RANGE, format!("bytes=0-{head_end}"))
        .send()
        .await
        .map_err(|e| {
            OpensubHashError::Http(format!("request to {redacted} failed: {}", e.without_url()))
        })?;

    match head_resp.status() {
        StatusCode::PARTIAL_CONTENT => {
            let content_range = head_resp
                .headers()
                .get(http::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    OpensubHashError::Http("206 response missing Content-Range header".to_owned())
                })?;
            let total_size = parse_total_from_content_range(content_range).ok_or_else(|| {
                OpensubHashError::Http(format!("Unparseable Content-Range: {content_range}"))
            })?;

            if total_size < OPENSUB_HASH_CHUNK_SIZE as u64 {
                return Err(OpensubHashError::FileTooSmall(total_size));
            }

            let head_bytes = head_resp.bytes().await.map_err(|e| {
                OpensubHashError::Http(format!(
                    "reading head from {redacted} failed: {}",
                    e.without_url()
                ))
            })?;
            let Some(&head) = head_bytes.first_chunk::<OPENSUB_HASH_CHUNK_SIZE>() else {
                return Err(OpensubHashError::Http(format!(
                    "Expected {OPENSUB_HASH_CHUNK_SIZE} head bytes, got {}",
                    head_bytes.len()
                )));
            };

            let tail_start = total_size - OPENSUB_HASH_CHUNK_SIZE as u64;
            let tail_end = total_size - 1;
            let tail: [u8; OPENSUB_HASH_CHUNK_SIZE] = if tail_start == 0 {
                head
            } else {
                let tail_resp = client
                    .get(url.as_str())
                    .header(
                        http::header::RANGE,
                        format!("bytes={tail_start}-{tail_end}"),
                    )
                    .send()
                    .await
                    .map_err(|e| {
                        OpensubHashError::Http(format!(
                            "tail request to {redacted} failed: {}",
                            e.without_url()
                        ))
                    })?;
                if tail_resp.status() != StatusCode::PARTIAL_CONTENT {
                    return Err(OpensubHashError::Http(format!(
                        "Tail Range request to {redacted} returned unexpected status {}",
                        tail_resp.status()
                    )));
                }
                let tail_bytes = tail_resp.bytes().await.map_err(|e| {
                    OpensubHashError::Http(format!(
                        "reading tail from {redacted} failed: {}",
                        e.without_url()
                    ))
                })?;
                let Some(&tail_arr) = tail_bytes.first_chunk::<OPENSUB_HASH_CHUNK_SIZE>() else {
                    return Err(OpensubHashError::Http(format!(
                        "Expected {OPENSUB_HASH_CHUNK_SIZE} tail bytes, got {}",
                        tail_bytes.len()
                    )));
                };
                tail_arr
            };

            Ok(OpensubHashResult {
                hash: compute_opensubhash_from_chunks(total_size, &head, &tail),
                size: total_size,
            })
        }
        StatusCode::RANGE_NOT_SATISFIABLE => {
            let reported = head_resp
                .headers()
                .get(http::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_total_from_content_range)
                .unwrap_or(0);
            Err(OpensubHashError::FileTooSmall(reported))
        }
        StatusCode::OK => {
            if let Some(cl) = head_resp.content_length() {
                if cl < OPENSUB_HASH_CHUNK_SIZE as u64 {
                    return Err(OpensubHashError::FileTooSmall(cl));
                }
                if cl > FALLBACK_MAX_FULL_STREAM_BYTES {
                    return Err(OpensubHashError::Http(
                        "Upstream server does not support HTTP Range for large file".to_owned(),
                    ));
                }
            }

            let mut head = [0u8; OPENSUB_HASH_CHUNK_SIZE];
            let mut head_filled = 0usize;
            let mut tail_ring = std::collections::VecDeque::with_capacity(OPENSUB_HASH_CHUNK_SIZE);
            let mut total_read = 0u64;
            let mut stream = head_resp.bytes_stream();

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| {
                    OpensubHashError::Http(format!(
                        "stream read from {redacted} failed: {}",
                        e.without_url()
                    ))
                })?;
                total_read = total_read.saturating_add(chunk.len() as u64);
                if total_read > FALLBACK_MAX_FULL_STREAM_BYTES {
                    return Err(OpensubHashError::Http(
                        "Upstream stream exceeded fallback size cap without Range support"
                            .to_owned(),
                    ));
                }
                if head_filled < OPENSUB_HASH_CHUNK_SIZE {
                    let take = (OPENSUB_HASH_CHUNK_SIZE - head_filled).min(chunk.len());
                    head[head_filled..head_filled + take].copy_from_slice(&chunk[..take]);
                    head_filled += take;
                }
                if chunk.len() >= OPENSUB_HASH_CHUNK_SIZE {
                    tail_ring.clear();
                    tail_ring.extend(&chunk[chunk.len() - OPENSUB_HASH_CHUNK_SIZE..]);
                } else {
                    let overflow =
                        (tail_ring.len() + chunk.len()).saturating_sub(OPENSUB_HASH_CHUNK_SIZE);
                    for _ in 0..overflow {
                        tail_ring.pop_front();
                    }
                    tail_ring.extend(chunk.iter().copied());
                }
            }

            if total_read < OPENSUB_HASH_CHUNK_SIZE as u64 {
                return Err(OpensubHashError::FileTooSmall(total_read));
            }
            let tail_vec: Vec<u8> = tail_ring.into_iter().collect();
            let tail: [u8; OPENSUB_HASH_CHUNK_SIZE] = tail_vec
                .try_into()
                .map_err(|_| OpensubHashError::FileTooSmall(total_read))?;
            Ok(OpensubHashResult {
                hash: compute_opensubhash_from_chunks(total_read, &head, &tail),
                size: total_read,
            })
        }
        status => Err(OpensubHashError::Http(format!(
            "Upstream {redacted} returned HTTP {status}"
        ))),
    }
}

pub async fn compute_opensubhash_url_or_path(
    client: &reqwest::Client,
    target: &str,
) -> Result<OpensubHashResult, OpensubHashError> {
    let trimmed = target.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(OpensubHashError::InvalidTarget(
            "empty or invalid url parameter".to_owned(),
        ));
    }

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let url =
            Url::parse(trimmed).map_err(|e| OpensubHashError::InvalidTarget(e.to_string()))?;
        return compute_opensubhash_http(client, &url).await;
    }

    if trimmed.starts_with("file://") {
        let url =
            Url::parse(trimmed).map_err(|e| OpensubHashError::InvalidTarget(e.to_string()))?;
        let path = url.to_file_path().map_err(|_| {
            OpensubHashError::InvalidTarget("cannot convert file URL to local path".to_owned())
        })?;
        return compute_opensubhash_path(&path).await;
    }

    if let Some((scheme, _)) = trimmed.split_once("://") {
        if !scheme.is_empty() && scheme.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.') {
            return Err(OpensubHashError::InvalidTarget(format!(
                "unsupported URL scheme `{scheme}`"
            )));
        }
    }

    compute_opensubhash_path(Path::new(trimmed)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_overflow_and_zero_padded_hex_work() {
        let all_ff = vec![0xFFu8; OPENSUB_HASH_CHUNK_SIZE * 2];
        let res = compute_opensubhash_bytes(&all_ff).unwrap();
        assert_eq!(res.size, (OPENSUB_HASH_CHUNK_SIZE * 2) as u64);
        assert_eq!(res.hash.len(), 16);

        let zeros = vec![0u8; OPENSUB_HASH_CHUNK_SIZE];
        let zero_res = compute_opensubhash_bytes(&zeros).unwrap();
        assert_eq!(zero_res.hash, "0000000000010000");
        assert_eq!(zero_res.size, 65_536);

        let tiny = vec![0u8; OPENSUB_HASH_CHUNK_SIZE - 1];
        assert!(matches!(
            compute_opensubhash_bytes(&tiny),
            Err(OpensubHashError::FileTooSmall(65_535))
        ));
    }
}
