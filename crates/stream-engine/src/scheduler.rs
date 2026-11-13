use std::collections::{HashMap, HashSet};
use std::io::SeekFrom;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use librqbit::{ManagedTorrent, Session};
use parking_lot::RwLock;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace};

pub const HEAD_CRITICAL_BYTES: u64 = 2 * 1024 * 1024;
pub const TAIL_CRITICAL_BYTES: u64 = 2 * 1024 * 1024;
pub const MIN_LOOKAHEAD_BYTES: u64 = 8 * 1024 * 1024;
pub const DEFAULT_LOOKAHEAD_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_LOOKAHEAD_BYTES: u64 = 128 * 1024 * 1024;
pub const LOOKAHEAD_TARGET_SECONDS: u64 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PiecePriority {
    Ignore,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorityPlan {
    pub file_piece_range: Range<u32>,
    pub head_pieces: Range<u32>,
    pub tail_pieces: Range<u32>,
    pub lookahead_pieces: Range<u32>,
    pub lookahead_bytes: u64,
    pub playhead_piece: Option<u32>,
}

impl PriorityPlan {
    pub fn empty() -> Self {
        Self {
            file_piece_range: 0..0,
            head_pieces: 0..0,
            tail_pieces: 0..0,
            lookahead_pieces: 0..0,
            lookahead_bytes: 0,
            playhead_piece: None,
        }
    }

    pub fn priority_of(&self, piece_idx: u32) -> PiecePriority {
        if !self.file_piece_range.contains(&piece_idx) {
            return PiecePriority::Ignore;
        }
        if self.head_pieces.contains(&piece_idx) || self.tail_pieces.contains(&piece_idx) {
            return PiecePriority::Critical;
        }
        if self.lookahead_pieces.contains(&piece_idx) {
            return PiecePriority::High;
        }
        PiecePriority::Normal
    }

    pub fn critical_pieces(&self) -> Vec<u32> {
        let mut out: Vec<u32> = self
            .head_pieces
            .clone()
            .chain(self.tail_pieces.clone())
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    pub fn ordered_priority_pieces(&self) -> Vec<u32> {
        let mut seen = HashSet::new();
        let mut ordered = Vec::new();
        for piece in self
            .head_pieces
            .clone()
            .chain(self.tail_pieces.clone())
            .chain(self.lookahead_pieces.clone())
        {
            if seen.insert(piece) {
                ordered.push(piece);
            }
        }
        ordered
    }
}

fn byte_span_to_piece_range(
    file_offset: u64,
    rel_start: u64,
    rel_end_exclusive: u64,
    piece_len: u64,
) -> Range<u32> {
    if rel_end_exclusive <= rel_start || piece_len == 0 {
        return 0..0;
    }
    let abs_start = file_offset.saturating_add(rel_start);
    let abs_end = file_offset.saturating_add(rel_end_exclusive);
    let start_piece = (abs_start / piece_len) as u32;
    let end_piece = abs_end.div_ceil(piece_len) as u32;
    start_piece..end_piece
}

#[derive(Debug, Clone)]
struct ActiveRangeStream {
    current_offset: u64,
    bytes_delivered: u64,
    started_at: Instant,
    is_trailer_probe: bool,
}

struct EpisodeScheduleState {
    file_idx: usize,
    file_offset: u64,
    file_len: u64,
    piece_len: u64,
    playhead_byte_offset: u64,
    bitrate_estimate_bps: Option<u64>,
    active_streams: HashMap<u64, ActiveRangeStream>,
    warmer_cancel: CancellationToken,
    head_warmed: Arc<AtomicBool>,
    tail_warmed: Arc<AtomicBool>,
    prebuffered_file_idx: Option<usize>,
    prebuffer_warmer_cancel: Option<CancellationToken>,
    retiring_streams: HashMap<usize, HashSet<u64>>,
    session_handle: Option<(Arc<Session>, Arc<ManagedTorrent>)>,
}

impl EpisodeScheduleState {
    fn desired_only_files(&self) -> HashSet<usize> {
        let mut files = HashSet::new();
        files.insert(self.file_idx);
        if let Some(pre_idx) = self.prebuffered_file_idx {
            files.insert(pre_idx);
        }
        for &retiring_idx in self.retiring_streams.keys() {
            files.insert(retiring_idx);
        }
        files
    }
}

fn only_files_matches(handle: &ManagedTorrent, desired: &HashSet<usize>) -> bool {
    handle.only_files().is_some_and(|existing| {
        let existing_set: HashSet<usize> = existing.into_iter().collect();
        existing_set == *desired
    })
}

pub struct PlayheadScheduler {
    next_cursor_id: AtomicU64,
    torrents: DashMap<[u8; 20], Arc<RwLock<EpisodeScheduleState>>>,
    cursor_closed_notify: tokio::sync::Notify,
    shutdown_token: CancellationToken,
}

impl Default for PlayheadScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayheadScheduler {
    pub fn new() -> Self {
        Self {
            next_cursor_id: AtomicU64::new(1),
            torrents: DashMap::new(),
            cursor_closed_notify: tokio::sync::Notify::new(),
            shutdown_token: CancellationToken::new(),
        }
    }

    pub fn compute_priority_window(
        file_offset: u64,
        file_len: u64,
        piece_len: u64,
        playhead_byte_offset: u64,
        bitrate_estimate: Option<u64>,
    ) -> PriorityPlan {
        if file_len == 0 || piece_len == 0 {
            return PriorityPlan::empty();
        }

        let clamped_playhead = playhead_byte_offset.min(file_len - 1);
        let file_piece_range = byte_span_to_piece_range(file_offset, 0, file_len, piece_len);

        let head_end = HEAD_CRITICAL_BYTES.min(file_len);
        let head_pieces = byte_span_to_piece_range(file_offset, 0, head_end, piece_len);

        let tail_start = file_len.saturating_sub(TAIL_CRITICAL_BYTES);
        let tail_pieces = byte_span_to_piece_range(file_offset, tail_start, file_len, piece_len);

        let lookahead_bytes = match bitrate_estimate {
            Some(bps) if bps > 0 => bps
                .saturating_mul(LOOKAHEAD_TARGET_SECONDS)
                .clamp(MIN_LOOKAHEAD_BYTES, MAX_LOOKAHEAD_BYTES),
            _ => DEFAULT_LOOKAHEAD_BYTES,
        };

        let lookahead_end = clamped_playhead
            .saturating_add(lookahead_bytes)
            .max(clamped_playhead + 1)
            .min(file_len);
        let lookahead_pieces =
            byte_span_to_piece_range(file_offset, clamped_playhead, lookahead_end, piece_len);

        let playhead_piece =
            Some(((file_offset.saturating_add(clamped_playhead)) / piece_len) as u32);

        PriorityPlan {
            file_piece_range,
            head_pieces,
            tail_pieces,
            lookahead_pieces,
            lookahead_bytes,
            playhead_piece,
        }
    }

    pub async fn activate_episode(
        &self,
        session: &Arc<Session>,
        handle: &Arc<ManagedTorrent>,
        info_hash: [u8; 20],
        file_idx: usize,
        shutdown_token: &CancellationToken,
    ) -> Result<PriorityPlan, crate::StreamEngineError> {
        handle
            .wait_until_initialized()
            .await
            .map_err(|e| crate::StreamEngineError::Torrent(format!("torrent init failed: {e:#}")))?;

        let (fi_opt, piece_len, total_files) = handle
            .with_metadata(|meta| {
                let total_files = meta.file_infos.len();
                let fi = meta.file_infos.get(file_idx).cloned();
                let piece_len = meta.lengths().default_piece_length() as u64;
                (fi, piece_len, total_files)
            })
            .map_err(|e| crate::StreamEngineError::Torrent(format!("{e:#}")))?;

        let fi = fi_opt.ok_or_else(|| {
            crate::StreamEngineError::Torrent(format!(
                "file_idx {file_idx} out of range (torrent has {total_files} files)"
            ))
        })?;

        let mut should_spawn_warmers = false;
        let warmer_cancel;
        let head_warmed;
        let tail_warmed;
        let desired_only_files;

        if let Some(existing) = self.torrents.get(&info_hash) {
            let mut guard = existing.write();
            guard.session_handle = Some((Arc::clone(session), Arc::clone(handle)));
            if guard.file_idx != file_idx {
                let prev_file_idx = guard.file_idx;
                if !guard.active_streams.is_empty() {
                    let prev_cursor_ids: Vec<u64> = guard.active_streams.keys().copied().collect();
                    let retiring = guard.retiring_streams.entry(prev_file_idx).or_default();
                    retiring.extend(prev_cursor_ids);
                }
                guard.warmer_cancel.cancel();
                if let Some(prebuf_cancel) = guard.prebuffer_warmer_cancel.take() {
                    prebuf_cancel.cancel();
                }
                let next_cancel = shutdown_token.child_token();
                let next_head = Arc::new(AtomicBool::new(false));
                let next_tail = Arc::new(AtomicBool::new(false));
                warmer_cancel = next_cancel.clone();
                head_warmed = next_head.clone();
                tail_warmed = next_tail.clone();
                let retiring_streams = core::mem::take(&mut guard.retiring_streams);
                let session_handle = guard.session_handle.clone();
                *guard = EpisodeScheduleState {
                    file_idx,
                    file_offset: fi.offset_in_torrent,
                    file_len: fi.len,
                    piece_len,
                    playhead_byte_offset: 0,
                    bitrate_estimate_bps: None,
                    active_streams: HashMap::new(),
                    warmer_cancel: next_cancel,
                    head_warmed: next_head,
                    tail_warmed: next_tail,
                    prebuffered_file_idx: None,
                    prebuffer_warmer_cancel: None,
                    retiring_streams,
                    session_handle,
                };
                should_spawn_warmers = fi.len > 0;
            } else {
                warmer_cancel = guard.warmer_cancel.clone();
                head_warmed = guard.head_warmed.clone();
                tail_warmed = guard.tail_warmed.clone();
            }
            desired_only_files = guard.desired_only_files();
        } else {
            let next_cancel = shutdown_token.child_token();
            let next_head = Arc::new(AtomicBool::new(false));
            let next_tail = Arc::new(AtomicBool::new(false));
            warmer_cancel = next_cancel.clone();
            head_warmed = next_head.clone();
            tail_warmed = next_tail.clone();
            let state = EpisodeScheduleState {
                file_idx,
                file_offset: fi.offset_in_torrent,
                file_len: fi.len,
                piece_len,
                playhead_byte_offset: 0,
                bitrate_estimate_bps: None,
                active_streams: HashMap::new(),
                warmer_cancel: next_cancel,
                head_warmed: next_head,
                tail_warmed: next_tail,
                prebuffered_file_idx: None,
                prebuffer_warmer_cancel: None,
                retiring_streams: HashMap::new(),
                session_handle: Some((Arc::clone(session), Arc::clone(handle))),
            };
            desired_only_files = state.desired_only_files();
            self.torrents.insert(info_hash, Arc::new(RwLock::new(state)));
            should_spawn_warmers = fi.len > 0;
        }

        if !only_files_matches(handle, &desired_only_files) {
            session
                .update_only_files(handle, &desired_only_files)
                .await
                .map_err(|e| {
                    crate::StreamEngineError::Torrent(format!(
                        "failed updating librqbit only_files for episode isolation: {e:#}"
                    ))
                })?;
            debug!(
                info_hash = %hex::encode(info_hash),
                file_idx,
                only_files = ?desired_only_files,
                "updated torrent only_files for active episode"
            );
        }

        if should_spawn_warmers {
            spawn_critical_head_tail_warmers(
                handle.clone(),
                file_idx,
                fi.len,
                warmer_cancel,
                head_warmed,
                tail_warmed,
            );
        }

        Ok(self
            .active_plan(&info_hash, file_idx)
            .unwrap_or_else(|| {
                Self::compute_priority_window(fi.offset_in_torrent, fi.len, piece_len, 0, None)
            }))
    }

    /// Pre-buffers an episode on a torrent without cancelling or deselecting an active episode on the same pack.
    pub async fn prebuffer_episode(
        &self,
        session: &Arc<Session>,
        info_hash: [u8; 20],
        handle: &Arc<ManagedTorrent>,
        file_idx: usize,
    ) -> Result<PriorityPlan, crate::StreamEngineError> {
        handle
            .wait_until_initialized()
            .await
            .map_err(|e| crate::StreamEngineError::Torrent(format!("torrent init failed: {e:#}")))?;

        let (fi_opt, piece_len, total_files) = handle
            .with_metadata(|meta| {
                let total_files = meta.file_infos.len();
                let fi = meta.file_infos.get(file_idx).cloned();
                let piece_len = meta.lengths().default_piece_length() as u64;
                (fi, piece_len, total_files)
            })
            .map_err(|e| crate::StreamEngineError::Torrent(format!("{e:#}")))?;

        let fi = fi_opt.ok_or_else(|| {
            crate::StreamEngineError::Torrent(format!(
                "file_idx {file_idx} out of range (torrent has {total_files} files)"
            ))
        })?;

        let mut should_spawn_warmers = false;
        let mut warmer_cancel = None;
        let mut head_warmed = None;
        let mut tail_warmed = None;
        let desired_only_files;

        if let Some(existing) = self.torrents.get(&info_hash) {
            let mut guard = existing.write();
            guard.session_handle = Some((Arc::clone(session), Arc::clone(handle)));
            if guard.file_idx != file_idx && guard.prebuffered_file_idx != Some(file_idx) {
                if let Some(prev_cancel) = guard.prebuffer_warmer_cancel.take() {
                    prev_cancel.cancel();
                }
                guard.prebuffered_file_idx = Some(file_idx);
                let next_cancel = self.shutdown_token.child_token();
                guard.prebuffer_warmer_cancel = Some(next_cancel.clone());
                if fi.len > 0 {
                    should_spawn_warmers = true;
                    warmer_cancel = Some(next_cancel);
                    head_warmed = Some(Arc::new(AtomicBool::new(false)));
                    tail_warmed = Some(Arc::new(AtomicBool::new(false)));
                }
            }
            desired_only_files = guard.desired_only_files();
        } else {
            let next_cancel = self.shutdown_token.child_token();
            let next_head = Arc::new(AtomicBool::new(false));
            let next_tail = Arc::new(AtomicBool::new(false));
            if fi.len > 0 {
                should_spawn_warmers = true;
                warmer_cancel = Some(next_cancel.clone());
                head_warmed = Some(next_head.clone());
                tail_warmed = Some(next_tail.clone());
            }
            let state = EpisodeScheduleState {
                file_idx,
                file_offset: fi.offset_in_torrent,
                file_len: fi.len,
                piece_len,
                playhead_byte_offset: 0,
                bitrate_estimate_bps: None,
                active_streams: HashMap::new(),
                warmer_cancel: next_cancel,
                head_warmed: next_head,
                tail_warmed: next_tail,
                prebuffered_file_idx: None,
                prebuffer_warmer_cancel: None,
                retiring_streams: HashMap::new(),
                session_handle: Some((Arc::clone(session), Arc::clone(handle))),
            };
            desired_only_files = state.desired_only_files();
            self.torrents.insert(info_hash, Arc::new(RwLock::new(state)));
        }

        if !only_files_matches(handle, &desired_only_files) {
            session
                .update_only_files(handle, &desired_only_files)
                .await
                .map_err(|e| {
                    crate::StreamEngineError::Torrent(format!(
                        "failed updating librqbit only_files for episode prebuffering: {e:#}"
                    ))
                })?;
            debug!(
                info_hash = %hex::encode(info_hash),
                file_idx,
                only_files = ?desired_only_files,
                "updated torrent only_files for prebuffered episode"
            );
        }

        if should_spawn_warmers {
            if let (Some(cancel), Some(head), Some(tail)) =
                (warmer_cancel, head_warmed, tail_warmed)
            {
                spawn_critical_head_tail_warmers(
                    handle.clone(),
                    file_idx,
                    fi.len,
                    cancel,
                    head,
                    tail,
                );
            }
        }

        Ok(Self::compute_priority_window(
            fi.offset_in_torrent,
            fi.len,
            piece_len,
            0,
            None,
        ))
    }

    pub fn open_range_cursor(
        self: &Arc<Self>,
        info_hash: [u8; 20],
        file_idx: usize,
        start_offset: u64,
        range_len: u64,
    ) -> RangeCursorGuard {
        let cursor_id = self.next_cursor_id.fetch_add(1, Ordering::Relaxed);
        if let Some(entry) = self.torrents.get(&info_hash) {
            let mut state = entry.write();
            if state.file_idx == file_idx {
                let tail_zone = state.file_len.saturating_sub(TAIL_CRITICAL_BYTES);
                let has_other_primary = state
                    .active_streams
                    .values()
                    .any(|s| !s.is_trailer_probe);
                let is_trailer_probe =
                    has_other_primary && start_offset >= tail_zone && range_len <= TAIL_CRITICAL_BYTES;

                state.active_streams.insert(
                    cursor_id,
                    ActiveRangeStream {
                        current_offset: start_offset,
                        bytes_delivered: 0,
                        started_at: Instant::now(),
                        is_trailer_probe,
                    },
                );

                if !is_trailer_probe {
                    state.playhead_byte_offset = start_offset;
                }
            }
        }

        RangeCursorGuard {
            scheduler: Arc::clone(self),
            info_hash,
            file_idx,
            cursor_id,
        }
    }

    fn record_cursor_progress(
        &self,
        info_hash: &[u8; 20],
        file_idx: usize,
        cursor_id: u64,
        bytes_read: u64,
    ) {
        let Some(entry) = self.torrents.get(info_hash) else {
            return;
        };
        let mut state = entry.write();
        if state.file_idx != file_idx {
            return;
        }
        let mut updated_playhead = None;
        let mut measured_bps = None;
        if let Some(cursor) = state.active_streams.get_mut(&cursor_id) {
            cursor.current_offset = cursor.current_offset.saturating_add(bytes_read);
            cursor.bytes_delivered = cursor.bytes_delivered.saturating_add(bytes_read);

            let elapsed_secs = cursor.started_at.elapsed().as_secs_f64();
            if elapsed_secs >= 0.25 && cursor.bytes_delivered >= 256 * 1024 {
                measured_bps = Some((cursor.bytes_delivered as f64 / elapsed_secs) as u64);
            }
            if !cursor.is_trailer_probe {
                updated_playhead = Some(cursor.current_offset);
            }
        }

        if let Some(pos) = updated_playhead {
            state.playhead_byte_offset = pos.min(state.file_len.saturating_sub(1));
        }
        if let Some(sample_bps) = measured_bps {
            state.bitrate_estimate_bps = Some(match state.bitrate_estimate_bps {
                Some(prev) => (prev * 3 + sample_bps) / 4,
                None => sample_bps,
            });
        }
    }

    fn close_cursor(&self, info_hash: &[u8; 20], file_idx: usize, cursor_id: u64) {
        let mut narrow_target: Option<(Arc<Session>, Arc<ManagedTorrent>)> = None;
        if let Some(entry) = self.torrents.get(info_hash) {
            let mut state = entry.write();
            if state.file_idx == file_idx {
                state.active_streams.remove(&cursor_id);
            } else if let Some(retiring) = state.retiring_streams.get_mut(&file_idx) {
                retiring.remove(&cursor_id);
                if retiring.is_empty() {
                    state.retiring_streams.remove(&file_idx);
                    narrow_target = state.session_handle.clone();
                }
            }
        }
        self.cursor_closed_notify.notify_waiters();
        if let Some((session, handle)) = narrow_target {
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                if let Some(entry) = self.torrents.get(info_hash).map(|e| Arc::clone(e.value())) {
                    rt.spawn(async move {
                        let desired = entry.read().desired_only_files();
                        if !only_files_matches(&handle, &desired) {
                            let _ = session.update_only_files(&handle, &desired).await;
                        }
                    });
                }
            }
        }
    }

    pub fn cursor_closed(&self) -> &tokio::sync::Notify {
        &self.cursor_closed_notify
    }

    pub fn has_active_streams_for_torrent(&self, info_hash: &[u8; 20]) -> bool {
        self.torrents.get(info_hash).is_some_and(|e| {
            let s = e.read();
            !s.active_streams.is_empty() || s.retiring_streams.values().any(|r| !r.is_empty())
        })
    }

    pub fn total_active_streams_for_torrent(&self, info_hash: &[u8; 20]) -> usize {
        self.torrents
            .get(info_hash)
            .map(|e| {
                let s = e.read();
                let retiring: usize = s.retiring_streams.values().map(HashSet::len).sum();
                s.active_streams.len() + retiring
            })
            .unwrap_or(0)
    }

    pub fn remove_torrent(&self, info_hash: &[u8; 20]) {
        if let Some((_, entry)) = self.torrents.remove(info_hash) {
            let guard = entry.read();
            guard.warmer_cancel.cancel();
            if let Some(prebuf) = &guard.prebuffer_warmer_cancel {
                prebuf.cancel();
            }
        }
    }

    pub fn set_bitrate_estimate(&self, info_hash: &[u8; 20], file_idx: usize, bps: Option<u64>) {
        if let Some(entry) = self.torrents.get(info_hash) {
            let mut state = entry.write();
            if state.file_idx == file_idx {
                state.bitrate_estimate_bps = bps;
            }
        }
    }

    pub fn active_plan(&self, info_hash: &[u8; 20], file_idx: usize) -> Option<PriorityPlan> {
        let entry = self.torrents.get(info_hash)?;
        let state = entry.read();
        if state.file_idx != file_idx {
            return None;
        }
        Some(Self::compute_priority_window(
            state.file_offset,
            state.file_len,
            state.piece_len,
            state.playhead_byte_offset,
            state.bitrate_estimate_bps,
        ))
    }

    pub fn selected_file(&self, info_hash: &[u8; 20]) -> Option<usize> {
        self.torrents.get(info_hash).map(|e| e.read().file_idx)
    }

    pub fn prebuffered_file(&self, info_hash: &[u8; 20]) -> Option<usize> {
        self.torrents
            .get(info_hash)
            .and_then(|e| e.read().prebuffered_file_idx)
    }

    pub fn active_stream_count(&self, info_hash: &[u8; 20], file_idx: usize) -> usize {
        self.torrents
            .get(info_hash)
            .map(|e| {
                let s = e.read();
                if s.file_idx == file_idx {
                    s.active_streams.len()
                } else {
                    s.retiring_streams.get(&file_idx).map_or(0, HashSet::len)
                }
            })
            .unwrap_or(0)
    }

    pub fn clear_all(&self) {
        for entry in self.torrents.iter() {
            let guard = entry.read();
            guard.warmer_cancel.cancel();
            if let Some(prebuf) = &guard.prebuffer_warmer_cancel {
                prebuf.cancel();
            }
        }
        self.torrents.clear();
        self.cursor_closed_notify.notify_waiters();
    }

    pub fn shutdown(&self) {
        self.shutdown_token.cancel();
        for entry in self.torrents.iter() {
            let guard = entry.read();
            guard.warmer_cancel.cancel();
            if let Some(prebuf) = &guard.prebuffer_warmer_cancel {
                prebuf.cancel();
            }
        }
    }
}

pub struct RangeCursorGuard {
    scheduler: Arc<PlayheadScheduler>,
    info_hash: [u8; 20],
    file_idx: usize,
    cursor_id: u64,
}

impl RangeCursorGuard {
    pub fn record_read(&mut self, bytes_read: usize) {
        if bytes_read > 0 {
            self.scheduler.record_cursor_progress(
                &self.info_hash,
                self.file_idx,
                self.cursor_id,
                bytes_read as u64,
            );
        }
    }
}

impl Drop for RangeCursorGuard {
    fn drop(&mut self) {
        self.scheduler
            .close_cursor(&self.info_hash, self.file_idx, self.cursor_id);
    }
}

fn spawn_critical_head_tail_warmers(
    handle: Arc<ManagedTorrent>,
    file_idx: usize,
    file_len: u64,
    cancel: CancellationToken,
    head_warmed: Arc<AtomicBool>,
    tail_warmed: Arc<AtomicBool>,
) {
    {
        let handle = handle.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let head_target = HEAD_CRITICAL_BYTES.min(file_len);
            let fut = async {
                let mut stream = handle.stream(file_idx).await.map_err(|e| e.to_string())?;
                let mut remaining = head_target;
                let mut buf = vec![0u8; 64 * 1024];
                while remaining > 0 {
                    let to_read = (buf.len() as u64).min(remaining) as usize;
                    let n = stream.read(&mut buf[..to_read]).await.map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    remaining -= n as u64;
                }
                head_warmed.store(true, Ordering::Release);
                trace!(file_idx, head_target, "head critical 2 MiB warmer finished");
                Ok::<(), String>(())
            };
            tokio::select! {
                _ = cancel.cancelled() => {}
                res = fut => {
                    if let Err(err) = res {
                        trace!(file_idx, "head warmer exited early: {err}");
                    }
                }
            }
        });
    }

    if file_len > HEAD_CRITICAL_BYTES {
        tokio::spawn(async move {
            let tail_start = file_len.saturating_sub(TAIL_CRITICAL_BYTES);
            let fut = async {
                let mut stream = handle.stream(file_idx).await.map_err(|e| e.to_string())?;
                stream.seek(SeekFrom::Start(tail_start)).await.map_err(|e| e.to_string())?;
                let mut remaining = file_len - tail_start;
                let mut buf = vec![0u8; 64 * 1024];
                while remaining > 0 {
                    let to_read = (buf.len() as u64).min(remaining) as usize;
                    let n = stream.read(&mut buf[..to_read]).await.map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    remaining -= n as u64;
                }
                tail_warmed.store(true, Ordering::Release);
                trace!(file_idx, tail_start, "tail critical 2 MiB warmer finished");
                Ok::<(), String>(())
            };
            tokio::select! {
                _ = cancel.cancelled() => {}
                res = fut => {
                    if let Err(err) = res {
                        trace!(file_idx, "tail warmer exited early: {err}");
                    }
                }
            }
        });
    } else {
        tail_warmed.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_and_empty_files_handle_boundaries_cleanly() {
        let empty = PlayheadScheduler::compute_priority_window(0, 0, 262_144, 0, None);
        assert!(empty.head_pieces.is_empty());
        assert!(empty.tail_pieces.is_empty());
        assert!(empty.lookahead_pieces.is_empty());

        let small =
            PlayheadScheduler::compute_priority_window(131_072, 1024 * 1024, 262_144, 0, None);
        assert_eq!(small.file_piece_range, 0..5);
        for p in small.file_piece_range.clone() {
            assert_eq!(small.priority_of(p), PiecePriority::Critical);
        }
        assert_eq!(small.priority_of(5), PiecePriority::Ignore);
    }

    #[tokio::test]
    async fn season_pack_prebuffer_preserves_only_files_union_and_narrows_after_old_streams_close() {
        use librqbit::{AddTorrent, AddTorrentOptions, CreateTorrentOptions, SessionOptions};
        use tempfile::TempDir;

        let seed_dir = TempDir::new().unwrap();
        let pack_dir = seed_dir.path().join("Show.S01");
        tokio::fs::create_dir_all(&pack_dir).await.unwrap();
        tokio::fs::write(pack_dir.join("01_ep1.mkv"), vec![1u8; 64 * 1024])
            .await
            .unwrap();
        tokio::fs::write(pack_dir.join("02_ep2.mkv"), vec![2u8; 64 * 1024])
            .await
            .unwrap();
        tokio::fs::write(pack_dir.join("03_ep3.mkv"), vec![3u8; 64 * 1024])
            .await
            .unwrap();

        let seeder = Session::new_with_opts(
            seed_dir.path().to_path_buf(),
            SessionOptions {
                dht: None,
                disable_trackers: true,
                disable_local_service_discovery: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let (created_meta, _seed_handle) = seeder
            .create_and_serve_torrent(
                &pack_dir,
                CreateTorrentOptions {
                    name: Some("Show.S01"),
                    piece_length: Some(32 * 1024),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let torrent_bytes = created_meta.as_bytes().unwrap().to_vec();

        let leech_dir = TempDir::new().unwrap();
        let session = Arc::new(
            Session::new_with_opts(
                leech_dir.path().to_path_buf(),
                SessionOptions {
                    dht: None,
                    disable_trackers: true,
                    disable_local_service_discovery: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
        );

        let resp = session
            .add_torrent(
                AddTorrent::from_bytes(torrent_bytes),
                Some(AddTorrentOptions {
                    paused: true,
                    overwrite: true,
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        let handle = resp.into_handle().unwrap();
        handle.wait_until_initialized().await.unwrap();
        let info_hash = handle.info_hash().0;

        let scheduler = Arc::new(PlayheadScheduler::new());
        let shutdown = CancellationToken::new();

        // 1. Activate Episode 0 and open an active stream cursor on it.
        scheduler
            .activate_episode(&session, &handle, info_hash, 0, &shutdown)
            .await
            .unwrap();
        assert_eq!(scheduler.selected_file(&info_hash), Some(0));
        assert_eq!(
            handle
                .only_files()
                .unwrap()
                .into_iter()
                .collect::<HashSet<_>>(),
            HashSet::from([0])
        );

        let ep0_cursor = scheduler.open_range_cursor(info_hash, 0, 0, 64 * 1024);
        assert_eq!(scheduler.active_stream_count(&info_hash, 0), 1);

        let ep0_warmer_cancelled = {
            let entry = scheduler.torrents.get(&info_hash).unwrap();
            let tok = entry.read().warmer_cancel.clone();
            tok
        };

        // 2. Prebuffer Episode 1 while Episode 0 is actively streaming.
        scheduler
            .prebuffer_episode(&session, info_hash, &handle, 1)
            .await
            .unwrap();

        assert_eq!(scheduler.selected_file(&info_hash), Some(0));
        assert_eq!(scheduler.prebuffered_file(&info_hash), Some(1));
        assert!(
            !ep0_warmer_cancelled.is_cancelled(),
            "prebuffer_episode must not cancel active episode warmer"
        );
        assert_eq!(
            handle
                .only_files()
                .unwrap()
                .into_iter()
                .collect::<HashSet<_>>(),
            HashSet::from([0, 1]),
            "only_files must keep both active episode 0 and prebuffered episode 1"
        );

        // 3. Activate Episode 1 while Episode 0 still has active_streams > 0 -> keep {0, 1}.
        scheduler
            .activate_episode(&session, &handle, info_hash, 1, &shutdown)
            .await
            .unwrap();
        assert_eq!(scheduler.selected_file(&info_hash), Some(1));
        assert_eq!(scheduler.prebuffered_file(&info_hash), None);
        assert_eq!(scheduler.active_stream_count(&info_hash, 0), 1);
        assert_eq!(
            handle
                .only_files()
                .unwrap()
                .into_iter()
                .collect::<HashSet<_>>(),
            HashSet::from([0, 1]),
            "only_files must retain retiring episode 0 while its cursor is still open"
        );

        // 4. Drop Episode 0 cursor -> retiring stream closes and only_files narrows cleanly to {1}.
        drop(ep0_cursor);
        assert_eq!(scheduler.active_stream_count(&info_hash, 0), 0);

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let cur: HashSet<usize> = handle.only_files().unwrap().into_iter().collect();
            if cur == HashSet::from([1]) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for only_files to narrow to {{1}}, got {cur:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // 5. Prebuffer Episode 2, then activate Episode 2 with 0 active streams -> immediately narrows to {2}.
        scheduler
            .prebuffer_episode(&session, info_hash, &handle, 2)
            .await
            .unwrap();
        assert_eq!(
            handle
                .only_files()
                .unwrap()
                .into_iter()
                .collect::<HashSet<_>>(),
            HashSet::from([1, 2])
        );

        scheduler
            .activate_episode(&session, &handle, info_hash, 2, &shutdown)
            .await
            .unwrap();
        assert_eq!(scheduler.prebuffered_file(&info_hash), None);
        assert_eq!(
            handle
                .only_files()
                .unwrap()
                .into_iter()
                .collect::<HashSet<_>>(),
            HashSet::from([2])
        );

        scheduler.shutdown();
        session.cancellation_token().cancel();
        seeder.cancellation_token().cancel();
    }
}
