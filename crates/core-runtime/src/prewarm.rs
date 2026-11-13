use serde::{Deserialize, Serialize};
use stremio_core::{
    constants::{META_RESOURCE_NAME, STREAM_RESOURCE_NAME, SUBTITLES_RESOURCE_NAME},
    models::{
        common::{Loadable, ResourceLoadable},
        player::{Player, Selected as PlayerSelected},
    },
    runtime::EnvError,
    types::{
        addon::{ResourcePath, ResourceRequest},
        resource::{SeriesInfo, Stream, StreamUrls, Video},
        streams::ConvertedStreamSource,
    },
};
use url::Url;

use crate::{
    model::DesktopModel,
    stream_parser::{
        filter_and_rank_streams, is_allowed_stream_source, is_allowed_stream_url,
        is_native_playable_stream, parse_stream, AudioCodec, HdrFormat, Resolution, StreamFilter,
        StreamQuality, VideoCodec,
    },
};

pub const MIN_EPISODE_DURATION_FOR_ENDING_PREWARM_SECS: f64 = 180.0;

pub const OUTRO_PREWARM_LEAD_SECS: f64 = 45.0;

pub const ENDING_PREWARM_WINDOW_SECS: f64 = 60.0;

pub const MAX_RANKED_NEXT_EPISODE_CANDIDATES: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrewarmTriggerReason {
    OutroCredits,
    EndingWindow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextEpisodeTarget {
    pub series_id: Option<String>,
    pub video_id: String,
    pub title: String,
    pub series_info: Option<SeriesInfo>,
    pub trigger_reason: PrewarmTriggerReason,
    pub stream_ready: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveEpisodeMatchSpec {
    pub binge_group: Option<String>,
    pub release_group: Option<String>,
    pub addon_base_url: Option<String>,
    pub resolution: Resolution,
    pub quality: StreamQuality,
    pub hdr: HdrFormat,
    pub codec: VideoCodec,
    pub audio: AudioCodec,
    pub has_debrid: bool,
}

impl Default for ActiveEpisodeMatchSpec {
    fn default() -> Self {
        Self {
            binge_group: None,
            release_group: None,
            addon_base_url: None,
            resolution: Resolution::Unknown,
            quality: StreamQuality::Unknown,
            hdr: HdrFormat::Sdr,
            codec: VideoCodec::Unknown,
            audio: AudioCodec::Unknown,
            has_debrid: false,
        }
    }
}

impl ActiveEpisodeMatchSpec {
    pub fn from_stream(stream: &Stream, stream_request: Option<&ResourceRequest>) -> Self {
        let parsed = parse_stream(stream);
        let has_debrid = parsed.is_debrid_stream();
        let binge_group = stream
            .behavior_hints
            .binge_group
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let addon_base_url = stream_request.map(|req| req.base.as_str().to_owned());

        Self {
            binge_group,
            release_group: parsed.release_group,
            addon_base_url,
            resolution: parsed.resolution,
            quality: parsed.quality,
            hdr: parsed.hdr,
            codec: parsed.codec,
            audio: parsed.audio,
            has_debrid,
        }
    }

    pub fn from_player_selected(selected: &PlayerSelected) -> Self {
        Self::from_stream(&selected.stream, selected.stream_request.as_ref())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedNextEpisode {
    pub url: Url,
    pub stream_urls: StreamUrls,
    pub converted_stream: Stream<ConvertedStreamSource>,
    pub stream: Stream,
    pub next_video: Video,
    pub player_selected: PlayerSelected,
    pub addon_request: Option<ResourceRequest>,
    pub matched_summary_badge: String,
    pub ranked_candidates: Vec<(Stream, Option<ResourceRequest>)>,
}

pub(crate) enum NextEpisodeResolution {
    Ready(Box<ResolvedNextEpisode>),
    NeedsStreamFetch {
        type_name: String,
        meta_id: String,
        video_id: String,
    },
    Unavailable,
}

pub fn convert_stream_to_playback_url(
    stream: &Stream,
    streaming_server_url: Option<&Url>,
) -> Result<(Url, StreamUrls, Stream<ConvertedStreamSource>), EnvError> {
    if !is_allowed_stream_source(stream) {
        return Err(EnvError::Other(
            "Stream rejected: disallowed or untrusted URL scheme".into(),
        ));
    }
    if !is_native_playable_stream(stream) {
        return Err(EnvError::Other(
            "Stream rejected: source is external or unsupported by native player".into(),
        ));
    }
    let converted = stream.convert(streaming_server_url)?;
    let urls = StreamUrls::new(converted.clone(), streaming_server_url);
    let playback_url = urls.streaming_url.clone().ok_or_else(|| {
        EnvError::Other("Converted stream does not expose an HTTP/streaming playback URL".into())
    })?;
    if !matches!(playback_url.scheme(), "http" | "https") || !is_allowed_stream_url(&playback_url)
    {
        return Err(EnvError::Other(
            "Converted playback URL rejected: scheme must be http or https".into(),
        ));
    }
    Ok((playback_url, urls, converted))
}

pub fn evaluate_next_episode_prewarm(
    model: &DesktopModel,
    current_pos_secs: f64,
    duration_secs: f64,
) -> Option<NextEpisodeTarget> {
    if !current_pos_secs.is_finite() || current_pos_secs < 0.0 {
        return None;
    }
    if model.player.live.is_some() {
        return None;
    }

    let next_video = effective_next_video(model)?;

    let trigger_reason = check_outro_trigger(&model.player, current_pos_secs, duration_secs)
        .or_else(|| check_ending_window_trigger(current_pos_secs, duration_secs))?;

    let series_id = extract_player_series_id(&model.player);
    let server_url = effective_streaming_server_url(model);
    let stream_ready = find_ready_next_stream(model, &next_video).is_some_and(|matched| {
        matched
            .ranked_candidates
            .iter()
            .any(|(stream, _)| convert_stream_to_playback_url(stream, server_url).is_ok())
    });

    Some(NextEpisodeTarget {
        series_id,
        video_id: next_video.id,
        title: next_video.title,
        series_info: next_video.series_info,
        trigger_reason,
        stream_ready,
    })
}

fn check_outro_trigger(
    player: &Player,
    current_pos_secs: f64,
    duration_secs: f64,
) -> Option<PrewarmTriggerReason> {
    let outro_ms = player.intro_outro.as_ref()?.outro?;
    if outro_ms == 0 {
        return None;
    }
    let outro_secs = outro_ms as f64 / 1000.0;
    if duration_secs.is_finite() && duration_secs > 0.0 && outro_secs > duration_secs {
        return None;
    }
    let trigger_at_secs = if outro_secs > OUTRO_PREWARM_LEAD_SECS {
        (outro_secs - OUTRO_PREWARM_LEAD_SECS).max(5.0)
    } else {
        (outro_secs * 0.85).max(1.0)
    };
    if current_pos_secs >= trigger_at_secs {
        Some(PrewarmTriggerReason::OutroCredits)
    } else {
        None
    }
}

fn check_ending_window_trigger(
    current_pos_secs: f64,
    duration_secs: f64,
) -> Option<PrewarmTriggerReason> {
    if !duration_secs.is_finite() || duration_secs <= MIN_EPISODE_DURATION_FOR_ENDING_PREWARM_SECS
    {
        return None;
    }
    if current_pos_secs > duration_secs + 5.0 {
        return None;
    }
    let remaining_secs = duration_secs - current_pos_secs;
    if remaining_secs <= ENDING_PREWARM_WINDOW_SECS {
        Some(PrewarmTriggerReason::EndingWindow)
    } else {
        None
    }
}

pub(crate) fn effective_streaming_server_url(model: &DesktopModel) -> Option<&Url> {
    model
        .streaming_server
        .base_url
        .as_ref()
        .or(Some(&model.ctx.profile.settings.streaming_server_url))
}

pub(crate) fn extract_player_series_id(player: &Player) -> Option<String> {
    if let Some(id) = player
        .selected
        .as_ref()
        .and_then(|sel| sel.meta_request.as_ref())
        .map(|req| req.path.id.clone())
    {
        return Some(id);
    }
    if let Some(id) = player
        .meta_item
        .as_ref()
        .map(|meta| meta.request.path.id.clone())
    {
        return Some(id);
    }
    if let Some(id) = player.library_item.as_ref().map(|item| item.id.clone()) {
        return Some(id);
    }
    player
        .selected
        .as_ref()
        .and_then(|sel| sel.stream_request.as_ref())
        .and_then(|req| req.path.id.split(':').next())
        .filter(|prefix| !prefix.is_empty())
        .map(str::to_owned)
}

fn extract_player_type_and_meta_id(player: &Player) -> Option<(String, String)> {
    if let Some(meta_req) = player
        .selected
        .as_ref()
        .and_then(|sel| sel.meta_request.as_ref())
    {
        return Some((meta_req.path.r#type.clone(), meta_req.path.id.clone()));
    }
    if let Some(meta_item) = player.meta_item.as_ref() {
        return Some((
            meta_item.request.path.r#type.clone(),
            meta_item.request.path.id.clone(),
        ));
    }
    if let Some(lib_item) = player.library_item.as_ref() {
        return Some((lib_item.r#type.clone(), lib_item.id.clone()));
    }
    if let Some(stream_req) = player
        .selected
        .as_ref()
        .and_then(|sel| sel.stream_request.as_ref())
    {
        if let Some(meta_id) = stream_req.path.id.split(':').next() {
            if !meta_id.is_empty() {
                return Some((stream_req.path.r#type.clone(), meta_id.to_owned()));
            }
        }
    }
    None
}

fn effective_next_video(model: &DesktopModel) -> Option<Video> {
    if let Some(next_video) = model.player.next_video.as_ref() {
        return Some(next_video.clone());
    }

    let meta_item = model
        .player
        .meta_item
        .as_ref()
        .and_then(|res| res.content.as_ref())
        .and_then(|loadable| loadable.ready())?;

    let current_video_id = model
        .player
        .selected
        .as_ref()
        .and_then(|sel| sel.stream_request.as_ref().map(|r| r.path.id.as_str()))
        .or_else(|| {
            model
                .player
                .selected
                .as_ref()
                .and_then(|sel| sel.subtitles_path.as_ref().map(|p| p.id.as_str()))
        })
        .or_else(|| {
            model
                .player
                .library_item
                .as_ref()
                .and_then(|item| item.state.video_id.as_deref())
        })?;

    meta_item
        .next_video(current_video_id, &chrono::Utc::now())
        .cloned()
}

struct ReadyNextStreamMatch {
    stream: Stream,
    addon_request: Option<ResourceRequest>,
    matched_summary_badge: String,
    ranked_candidates: Vec<(Stream, Option<ResourceRequest>)>,
}

pub(crate) fn inspect_next_episode_resolution(model: &DesktopModel) -> NextEpisodeResolution {
    let Some(next_video) = effective_next_video(model) else {
        return NextEpisodeResolution::Unavailable;
    };

    let streaming_server_url = effective_streaming_server_url(model);
    let active_spec = model
        .player
        .selected
        .as_ref()
        .map(ActiveEpisodeMatchSpec::from_player_selected);

    if let Some(matched) = find_ready_next_stream(model, &next_video) {
        struct ConvertibleCandidate {
            stream: Stream,
            req: Option<ResourceRequest>,
            url: Url,
            stream_urls: StreamUrls,
            converted_stream: Stream<ConvertedStreamSource>,
        }
        let mut convertible: Option<ConvertibleCandidate> = None;

        for (cand_stream, cand_req) in std::iter::once((
            matched.stream.clone(),
            matched.addon_request.clone(),
        ))
        .chain(matched.ranked_candidates.iter().cloned())
        {
            if let Ok((url, stream_urls, converted_stream)) =
                convert_stream_to_playback_url(&cand_stream, streaming_server_url)
            {
                convertible = Some(ConvertibleCandidate {
                    stream: cand_stream,
                    req: cand_req,
                    url,
                    stream_urls,
                    converted_stream,
                });
                break;
            }
        }

        if let Some(chosen) = convertible {
            let matched_summary_badge = if chosen.stream == matched.stream {
                matched.matched_summary_badge
            } else {
                build_matched_summary_badge(
                    &chosen.stream,
                    chosen.req.as_ref(),
                    active_spec.as_ref(),
                )
            };
            let player_selected = build_next_player_selected(
                &model.player,
                &next_video,
                chosen.stream.clone(),
                chosen.req.clone(),
            );
            return NextEpisodeResolution::Ready(Box::new(ResolvedNextEpisode {
                url: chosen.url,
                stream_urls: chosen.stream_urls,
                converted_stream: chosen.converted_stream,
                stream: chosen.stream,
                next_video,
                player_selected,
                addon_request: chosen.req,
                matched_summary_badge,
                ranked_candidates: matched.ranked_candidates,
            }));
        }
    }

    let next_streams_in_flight = matches!(
        model.player.next_streams.as_ref(),
        Some(ResourceLoadable {
            request,
            content: None | Some(Loadable::Loading),
        }) if request.path.id == next_video.id
    );
    if next_streams_in_flight {
        return NextEpisodeResolution::Unavailable;
    }

    let meta_details_targeting_next = model
        .meta_details
        .selected
        .as_ref()
        .and_then(|sel| sel.stream_path.as_ref())
        .is_some_and(|path| path.id == next_video.id);

    if meta_details_targeting_next {
        return NextEpisodeResolution::Unavailable;
    }

    if let Some((type_name, meta_id)) = extract_player_type_and_meta_id(&model.player) {
        return NextEpisodeResolution::NeedsStreamFetch {
            type_name,
            meta_id,
            video_id: next_video.id,
        };
    }

    NextEpisodeResolution::Unavailable
}

fn find_ready_next_stream(
    model: &DesktopModel,
    next_video: &Video,
) -> Option<ReadyNextStreamMatch> {
    let active_spec = model
        .player
        .selected
        .as_ref()
        .map(ActiveEpisodeMatchSpec::from_player_selected);

    let mut all_candidates: Vec<(Stream, Option<ResourceRequest>)> = Vec::new();

    if let Some(ResourceLoadable {
        request,
        content: Some(Loadable::Ready(streams)),
    }) = model.player.next_streams.as_ref()
    {
        if request.path.id == next_video.id {
            for s in streams {
                if is_allowed_stream_source(s) {
                    all_candidates.push((s.clone(), Some(request.clone())));
                }
            }
        }
    }

    let meta_details_matches_next = model
        .meta_details
        .selected
        .as_ref()
        .and_then(|sel| sel.stream_path.as_ref())
        .is_some_and(|path| path.id == next_video.id);

    if meta_details_matches_next {
        for res in model
            .meta_details
            .streams
            .iter()
            .chain(model.meta_details.meta_streams.iter())
        {
            if res.request.path.id == next_video.id {
                if let Some(Loadable::Ready(list)) = res.content.as_ref() {
                    for s in list {
                        if is_allowed_stream_source(s) {
                            all_candidates.push((s.clone(), Some(res.request.clone())));
                        }
                    }
                }
            }
        }
    }

    if !next_video.streams.is_empty() {
        let fallback_req =
            next_stream_request_from_selected(model.player.selected.as_ref(), &next_video.id);
        for s in &next_video.streams {
            if is_allowed_stream_source(s) {
                all_candidates.push((s.clone(), fallback_req.clone()));
            }
        }
    }

    let mut ranked = rank_continuation_candidates(&all_candidates, active_spec.as_ref());

    if let Some(next_stream) = model.player.next_stream.as_ref() {
        if is_allowed_stream_source(next_stream) {
            let explicit_req = model
                .player
                .next_streams
                .as_ref()
                .map(|ns| ns.request.clone())
                .or_else(|| {
                    next_stream_request_from_selected(
                        model.player.selected.as_ref(),
                        &next_video.id,
                    )
                });
            let mut combined = Vec::with_capacity(
                (1 + ranked.len()).min(MAX_RANKED_NEXT_EPISODE_CANDIDATES),
            );
            combined.push((next_stream.clone(), explicit_req.clone()));
            for pair in ranked.drain(..) {
                if pair.0 != *next_stream && combined.len() < MAX_RANKED_NEXT_EPISODE_CANDIDATES {
                    combined.push(pair);
                }
            }
            let badge = build_matched_summary_badge(
                next_stream,
                explicit_req.as_ref(),
                active_spec.as_ref(),
            );
            return Some(ReadyNextStreamMatch {
                stream: next_stream.clone(),
                addon_request: explicit_req,
                matched_summary_badge: badge,
                ranked_candidates: combined,
            });
        }
    }

    if let Some((best_stream, best_req)) = ranked.first().cloned() {
        let badge =
            build_matched_summary_badge(&best_stream, best_req.as_ref(), active_spec.as_ref());
        return Some(ReadyNextStreamMatch {
            stream: best_stream,
            addon_request: best_req,
            matched_summary_badge: badge,
            ranked_candidates: ranked,
        });
    }

    if let Some(yt_stream) = next_video.stream() {
        let owned = yt_stream.into_owned();
        if is_allowed_stream_source(&owned) {
            let badge = build_matched_summary_badge(&owned, None, active_spec.as_ref());
            return Some(ReadyNextStreamMatch {
                stream: owned.clone(),
                addon_request: None,
                matched_summary_badge: badge,
                ranked_candidates: vec![(owned, None)],
            });
        }
    }

    None
}

fn score_continuation_candidate(
    cand: &Stream,
    cand_req: Option<&ResourceRequest>,
    spec: &ActiveEpisodeMatchSpec,
) -> i64 {
    let parsed = parse_stream(cand);
    let mut score: i64 = 0;

    let binge_matched = spec.binge_group.as_deref().is_some_and(|bg| {
        cand.behavior_hints
            .binge_group
            .as_deref()
            .map(str::trim)
            .is_some_and(|cand_bg| cand_bg == bg)
    });
    if binge_matched {
        score += 2_000_000;
    }

    if let (Some(spec_rg), Some(cand_rg)) = (
        spec.release_group.as_deref(),
        parsed.release_group.as_deref(),
    ) {
        if spec_rg.eq_ignore_ascii_case(cand_rg) {
            score += 1_200_000;
        }
    }

    if let (Some(spec_base), Some(req)) = (spec.addon_base_url.as_deref(), cand_req) {
        if spec_base.eq_ignore_ascii_case(req.base.as_str()) {
            score += 600_000;
        }
    }

    if spec.resolution != Resolution::Unknown && spec.resolution == parsed.resolution {
        score += 500_000;
    }

    if parsed.is_cached {
        score += 400_000;
    }

    if spec.quality != StreamQuality::Unknown && spec.quality == parsed.quality {
        score += 350_000;
    }

    if spec.hdr == parsed.hdr {
        score += 250_000;
    }

    if spec.codec != VideoCodec::Unknown && spec.codec == parsed.codec {
        score += 100_000;
    }

    if spec.audio != AudioCodec::Unknown && spec.audio == parsed.audio {
        score += 100_000;
    }

    score += i64::from(parsed.seeders.unwrap_or(0).min(50_000));
    score
}

pub fn rank_continuation_candidates(
    candidates: &[(Stream, Option<ResourceRequest>)],
    active_spec: Option<&ActiveEpisodeMatchSpec>,
) -> Vec<(Stream, Option<ResourceRequest>)> {
    let allowed: Vec<(Stream, Option<ResourceRequest>)> = candidates
        .iter()
        .filter(|(stream, _)| is_allowed_stream_source(stream))
        .cloned()
        .collect();

    if allowed.is_empty() {
        return Vec::new();
    }

    if let Some(spec) = active_spec {
        let mut scored: Vec<(i64, usize, (Stream, Option<ResourceRequest>))> = allowed
            .into_iter()
            .enumerate()
            .map(|(idx, (stream, req))| {
                let score = score_continuation_candidate(&stream, req.as_ref(), spec);
                (score, idx, (stream, req))
            })
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        scored
            .into_iter()
            .map(|(_, _, pair)| pair)
            .take(MAX_RANKED_NEXT_EPISODE_CANDIDATES)
            .collect()
    } else {
        let plain: Vec<Stream> = allowed.iter().map(|(s, _)| s.clone()).collect();
        let ranked = filter_and_rank_streams(&plain, &StreamFilter::default());
        let mut remaining = allowed;
        let mut ordered = Vec::with_capacity(remaining.len().min(MAX_RANKED_NEXT_EPISODE_CANDIDATES));
        for (ranked_stream, _) in ranked {
            if let Some(pos) = remaining.iter().position(|(s, _)| s == ranked_stream) {
                ordered.push(remaining.remove(pos));
                if ordered.len() >= MAX_RANKED_NEXT_EPISODE_CANDIDATES {
                    break;
                }
            }
        }
        ordered
    }
}

fn build_matched_summary_badge(
    stream: &Stream,
    req: Option<&ResourceRequest>,
    spec: Option<&ActiveEpisodeMatchSpec>,
) -> String {
    let parsed = parse_stream(stream);
    let mut parts: Vec<String> = Vec::new();

    if let Some(s) = spec {
        let binge_matched = s.binge_group.as_deref().is_some_and(|bg| {
            stream
                .behavior_hints
                .binge_group
                .as_deref()
                .map(str::trim)
                .is_some_and(|cbg| cbg == bg)
        });
        if binge_matched {
            parts.push("Binge Group".to_owned());
        }
        if let (Some(rg1), Some(rg2)) = (
            s.release_group.as_deref(),
            parsed.release_group.as_deref(),
        ) {
            if rg1.eq_ignore_ascii_case(rg2) {
                parts.push(rg2.to_owned());
            }
        }
        if let (Some(base1), Some(r)) = (s.addon_base_url.as_deref(), req) {
            if base1.eq_ignore_ascii_case(r.base.as_str()) {
                if let Some(addon_name) = parsed.addon_name.as_deref() {
                    parts.push(addon_name.to_owned());
                }
            }
        }
    }

    if parsed.resolution != Resolution::Unknown {
        parts.push(parsed.resolution.label().to_owned());
    }
    if parsed.quality != StreamQuality::Unknown {
        parts.push(parsed.quality.label().to_owned());
    }
    if parsed.hdr.is_hdr() {
        parts.push(parsed.hdr.label().to_owned());
    }
    if parsed.is_cached {
        parts.push("Cached".to_owned());
    }

    if parts.is_empty() {
        "Auto-Matched".to_owned()
    } else {
        parts.join(" · ")
    }
}

fn next_stream_request_from_selected(
    selected: Option<&PlayerSelected>,
    next_video_id: &str,
) -> Option<ResourceRequest> {
    let mut req = selected?.stream_request.clone()?;
    req.path.id = next_video_id.to_owned();
    Some(req)
}

fn build_next_player_selected(
    player: &Player,
    next_video: &Video,
    stream: Stream,
    stream_request_override: Option<ResourceRequest>,
) -> PlayerSelected {
    let meta_request = player
        .selected
        .as_ref()
        .and_then(|sel| sel.meta_request.clone())
        .or_else(|| player.meta_item.as_ref().map(|m| m.request.clone()));

    let type_name = meta_request
        .as_ref()
        .map(|req| req.path.r#type.clone())
        .or_else(|| {
            player
                .selected
                .as_ref()
                .and_then(|sel| sel.stream_request.as_ref())
                .map(|req| req.path.r#type.clone())
        })
        .or_else(|| player.library_item.as_ref().map(|item| item.r#type.clone()))
        .unwrap_or_else(|| "series".to_owned());

    let stream_request = stream_request_override.or_else(|| {
        next_stream_request_from_selected(player.selected.as_ref(), &next_video.id).or_else(|| {
            meta_request.as_ref().map(|m_req| ResourceRequest {
                base: m_req.base.clone(),
                path: ResourcePath {
                    resource: STREAM_RESOURCE_NAME.to_owned(),
                    r#type: type_name.clone(),
                    id: next_video.id.clone(),
                    extra: vec![],
                },
            })
        })
    });

    let subtitles_path = match player
        .selected
        .as_ref()
        .and_then(|sel| sel.subtitles_path.as_ref())
    {
        Some(existing) => Some(ResourcePath {
            resource: existing.resource.clone(),
            r#type: existing.r#type.clone(),
            id: next_video.id.clone(),
            extra: vec![],
        }),
        None => Some(ResourcePath {
            resource: SUBTITLES_RESOURCE_NAME.to_owned(),
            r#type: type_name,
            id: next_video.id.clone(),
            extra: vec![],
        }),
    };

    let _ = META_RESOURCE_NAME;

    PlayerSelected {
        stream,
        stream_request,
        meta_request,
        subtitles_path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::PersistedBuckets;
    use stremio_core::types::{
        events::DismissedEventsBucket,
        library::LibraryBucket,
        notifications::NotificationsBucket,
        player::{IntroData, IntroOutro},
        profile::Profile,
        resource::{StreamBehaviorHints, StreamSource},
        search_history::SearchHistoryBucket,
        server_urls::ServerUrlsBucket,
        streams::StreamsBucket,
    };

    fn dummy_model() -> DesktopModel {
        let profile = Profile::default();
        let uid = profile.uid();
        let buckets = PersistedBuckets {
            profile,
            library: LibraryBucket::new(uid.clone(), vec![]),
            streams: StreamsBucket::new(uid.clone()),
            server_urls: ServerUrlsBucket {
                uid: uid.clone(),
                items: Default::default(),
            },
            notifications: NotificationsBucket::new::<crate::DesktopEnv>(uid.clone(), vec![]),
            search_history: SearchHistoryBucket::new(uid.clone()),
            dismissed_events: DismissedEventsBucket::new(uid),
        };
        let (model, _) = DesktopModel::from_buckets(buckets);
        model
    }

    fn make_video(id: &str, season: u32, episode: u32, title: &str) -> Video {
        Video {
            id: id.to_owned(),
            title: title.to_owned(),
            released: None,
            overview: None,
            thumbnail: None,
            streams: vec![],
            series_info: Some(SeriesInfo { season, episode }),
            epg_info: None,
            trailer_streams: vec![],
        }
    }

    fn make_http_stream(url: &str, binge_group: Option<&str>) -> Stream {
        Stream {
            source: StreamSource::Url {
                url: Url::parse(url).unwrap(),
            },
            name: Some("[RD+] Torrentio 4K".to_owned()),
            description: Some("Show.S01E02.2160p.WEB-DL.DDP5.1.H.265.mkv".to_owned()),
            thumbnail: None,
            subtitles: vec![],
            behavior_hints: StreamBehaviorHints {
                binge_group: binge_group.map(str::to_owned),
                ..Default::default()
            },
        }
    }

    #[test]
    fn triggers_prewarm_45s_before_outro_credits_and_handles_short_outro() {
        let mut model = dummy_model();
        model.player.next_video = Some(make_video("tt0903747:1:2", 1, 2, "Cat's in the Bag..."));
        model.player.next_stream = Some(make_http_stream(
            "https://debrid.example.com/ep2.mkv",
            Some("torrentio|4k"),
        ));
        model.player.intro_outro = Some(IntroOutro {
            intro: Some(IntroData {
                from: 60_000,
                to: 90_000,
                duration: None,
            }),
            outro: Some(1_260_000),
        });

        // 1260 - 45 = 1215s threshold
        assert!(evaluate_next_episode_prewarm(&model, 1200.0, 1500.0).is_none());
        assert!(evaluate_next_episode_prewarm(&model, 1214.0, 1500.0).is_none());

        let target = evaluate_next_episode_prewarm(&model, 1216.0, 1500.0)
            .expect("should trigger 45s before outro credits");
        assert_eq!(target.video_id, "tt0903747:1:2");
        assert_eq!(target.trigger_reason, PrewarmTriggerReason::OutroCredits);
        assert!(target.stream_ready);

        // Short outro <= 45s uses (outro_secs * 0.85).max(1.0) = 25.5s
        model.player.intro_outro = Some(IntroOutro {
            intro: None,
            outro: Some(30_000),
        });
        assert!(evaluate_next_episode_prewarm(&model, 25.0, 120.0).is_none());
        let short_target = evaluate_next_episode_prewarm(&model, 26.0, 120.0)
            .expect("should trigger at 85% of short outro");
        assert_eq!(
            short_target.trigger_reason,
            PrewarmTriggerReason::OutroCredits
        );
    }

    #[test]
    fn triggers_prewarm_in_final_60s_only_for_episodes_longer_than_180s() {
        let mut model = dummy_model();
        model.player.next_video = Some(make_video(
            "tt0903747:1:3",
            1,
            3,
            "...And the Bag's in the River",
        ));

        assert!(evaluate_next_episode_prewarm(&model, 95.0, 120.0).is_none());
        assert!(evaluate_next_episode_prewarm(&model, 1300.0, 1440.0).is_none());
        assert!(evaluate_next_episode_prewarm(&model, 1375.0, 1440.0).is_none());

        let target = evaluate_next_episode_prewarm(&model, 1385.0, 1440.0)
            .expect("should trigger in final 60s");
        assert_eq!(target.trigger_reason, PrewarmTriggerReason::EndingWindow);
        assert!(!target.stream_ready);
    }

    #[test]
    fn resolves_torrent_and_http_next_episode_playback_urls_and_player_selected() {
        let mut model = dummy_model();
        let addon_base = Url::parse("https://torrentio.strem.fun/manifest.json").unwrap();
        model.streaming_server.base_url = Some(Url::parse("http://127.0.0.1:11470/").unwrap());

        let ep1_stream = make_http_stream("https://debrid.example.com/ep1.mkv", Some("group-a"));
        model.player.selected = Some(PlayerSelected {
            stream: ep1_stream,
            stream_request: Some(ResourceRequest {
                base: addon_base,
                path: ResourcePath {
                    resource: STREAM_RESOURCE_NAME.to_owned(),
                    r#type: "series".to_owned(),
                    id: "tt0903747:1:1".to_owned(),
                    extra: vec![],
                },
            }),
            meta_request: Some(ResourceRequest {
                base: Url::parse("https://v3-cinemeta.strem.io/manifest.json").unwrap(),
                path: ResourcePath {
                    resource: META_RESOURCE_NAME.to_owned(),
                    r#type: "series".to_owned(),
                    id: "tt0903747".to_owned(),
                    extra: vec![],
                },
            }),
            subtitles_path: Some(ResourcePath {
                resource: SUBTITLES_RESOURCE_NAME.to_owned(),
                r#type: "series".to_owned(),
                id: "tt0903747:1:1".to_owned(),
                extra: vec![],
            }),
        });

        model.player.next_video = Some(make_video("tt0903747:1:2", 1, 2, "Episode 2"));

        let info_hash = [0xabu8; 20];
        model.player.next_stream = Some(Stream {
            source: StreamSource::Torrent {
                info_hash,
                file_idx: Some(1),
                announce: vec!["tracker:udp://tracker.opentrackr.org:1337/announce".to_owned()],
                file_must_include: vec![],
            },
            name: Some("Torrentio 1080p".to_owned()),
            description: Some("Show.S01E02.1080p.mkv".to_owned()),
            thumbnail: None,
            subtitles: vec![],
            behavior_hints: StreamBehaviorHints::default(),
        });

        match inspect_next_episode_resolution(&model) {
            NextEpisodeResolution::Ready(resolved) => {
                assert!(resolved
                    .url
                    .as_str()
                    .starts_with("http://127.0.0.1:11470/abababababababababababababababababababab/1"));
                assert_eq!(
                    resolved
                        .player_selected
                        .stream_request
                        .as_ref()
                        .unwrap()
                        .path
                        .id,
                    "tt0903747:1:2"
                );
                assert_eq!(
                    resolved
                        .player_selected
                        .subtitles_path
                        .as_ref()
                        .unwrap()
                        .id,
                    "tt0903747:1:2"
                );
                assert_eq!(
                    resolved
                        .player_selected
                        .meta_request
                        .as_ref()
                        .unwrap()
                        .path
                        .id,
                    "tt0903747"
                );
            }
            _ => panic!("expected Ready next episode resolution"),
        }
    }

    #[test]
    fn rejects_malicious_stream_schemes_during_playback_url_conversion() {
        let server_url = Url::parse("http://127.0.0.1:11470/").unwrap();
        for bad in [
            "file:///etc/passwd",
            "edl://!new_stream;/etc/shadow",
            "lavfi://testsrc",
            "av://v4l2:/dev/video0",
            "fd://0",
            "archive:///tmp/x.zip|/etc/passwd",
            "memory://payload",
        ] {
            let stream = make_http_stream(bad, None);
            assert!(
                convert_stream_to_playback_url(&stream, Some(&server_url)).is_err(),
                "expected conversion rejection for {bad}"
            );
        }
    }

    #[test]
    fn matches_release_group_addon_and_preserves_request_and_top_40_candidates() {
        let mut model = dummy_model();
        let addon_a = Url::parse("https://torrentio.strem.fun/manifest.json").unwrap();
        let addon_b = Url::parse("https://comet.elfhosted.com/manifest.json").unwrap();

        let current_stream = Stream {
            source: StreamSource::Url {
                url: Url::parse("https://debrid.example.com/ep1.mkv").unwrap(),
            },
            name: Some("[RD+] Torrentio\n1080p".to_owned()),
            description: Some(
                "Severance.S02E01.1080p.ATVP.WEB-DL.DDP5.1.H.265-NTb.mkv\n👤 120 💾 3.5 GB"
                    .to_owned(),
            ),
            thumbnail: None,
            subtitles: vec![],
            behavior_hints: StreamBehaviorHints {
                binge_group: Some("torrentio|1080p|NTb".to_owned()),
                ..Default::default()
            },
        };

        model.player.selected = Some(PlayerSelected {
            stream: current_stream,
            stream_request: Some(ResourceRequest {
                base: addon_a.clone(),
                path: ResourcePath {
                    resource: STREAM_RESOURCE_NAME.to_owned(),
                    r#type: "series".to_owned(),
                    id: "tt11280740:2:1".to_owned(),
                    extra: vec![],
                },
            }),
            meta_request: None,
            subtitles_path: None,
        });
        model.player.next_video = Some(make_video("tt11280740:2:2", 2, 2, "Goodbye, Mrs. Selvig"));

        // Candidate 1 (addon_b, 4K Remux, different group)
        let mut candidates_b = vec![Stream {
            source: StreamSource::Url {
                url: Url::parse("https://debrid.example.com/ep2_other.mkv").unwrap(),
            },
            name: Some("[RD+] Comet\n4K".to_owned()),
            description: Some(
                "Severance.S02E02.2160p.ATVP.WEB-DL.DDP5.1.H.265-FLUX.mkv\n👤 900 💾 8.0 GB"
                    .to_owned(),
            ),
            thumbnail: None,
            subtitles: vec![],
            behavior_hints: StreamBehaviorHints {
                binge_group: Some("comet|4k|FLUX".to_owned()),
                ..Default::default()
            },
        }];
        // Pad with 45 extra candidates to verify 40-item cap
        for i in 0..45 {
            candidates_b.push(Stream {
                source: StreamSource::Url {
                    url: Url::parse(&format!("https://debrid.example.com/extra_{i}.mkv")).unwrap(),
                },
                name: Some("Comet 720p".to_owned()),
                description: Some(format!("Severance.S02E02.720p.WEB-DL.x264-GRP{i}.mkv")),
                thumbnail: None,
                subtitles: vec![],
                behavior_hints: StreamBehaviorHints::default(),
            });
        }

        // Candidate 2 (addon_a, exact binge_group + release_group NTb + 1080p WEB-DL HEVC DDP)
        let exact_match = Stream {
            source: StreamSource::Url {
                url: Url::parse("https://debrid.example.com/ep2_ntb.mkv").unwrap(),
            },
            name: Some("[RD+] Torrentio\n1080p".to_owned()),
            description: Some(
                "Severance.S02E02.1080p.ATVP.WEB-DL.DDP5.1.H.265-NTb.mkv\n👤 80 💾 3.6 GB"
                    .to_owned(),
            ),
            thumbnail: None,
            subtitles: vec![],
            behavior_hints: StreamBehaviorHints {
                binge_group: Some("torrentio|1080p|NTb".to_owned()),
                ..Default::default()
            },
        };

        let req_a = ResourceRequest {
            base: addon_a,
            path: ResourcePath {
                resource: STREAM_RESOURCE_NAME.to_owned(),
                r#type: "series".to_owned(),
                id: "tt11280740:2:2".to_owned(),
                extra: vec![],
            },
        };
        let req_b = ResourceRequest {
            base: addon_b,
            path: ResourcePath {
                resource: STREAM_RESOURCE_NAME.to_owned(),
                r#type: "series".to_owned(),
                id: "tt11280740:2:2".to_owned(),
                extra: vec![],
            },
        };

        model.meta_details.selected = Some(stremio_core::models::meta_details::Selected {
            meta_path: ResourcePath {
                resource: META_RESOURCE_NAME.to_owned(),
                r#type: "series".to_owned(),
                id: "tt11280740".to_owned(),
                extra: vec![],
            },
            stream_path: Some(ResourcePath {
                resource: STREAM_RESOURCE_NAME.to_owned(),
                r#type: "series".to_owned(),
                id: "tt11280740:2:2".to_owned(),
                extra: vec![],
            }),
            guess_stream: false,
        });
        model.meta_details.streams = vec![
            ResourceLoadable {
                request: req_b,
                content: Some(Loadable::Ready(candidates_b)),
            },
            ResourceLoadable {
                request: req_a.clone(),
                content: Some(Loadable::Ready(vec![exact_match.clone()])),
            },
        ];

        match inspect_next_episode_resolution(&model) {
            NextEpisodeResolution::Ready(resolved) => {
                assert_eq!(resolved.stream, exact_match);
                assert_eq!(resolved.addon_request.as_ref(), Some(&req_a));
                assert_eq!(
                    resolved.player_selected.stream_request.as_ref(),
                    Some(&req_a)
                );
                assert!(resolved.matched_summary_badge.contains("Binge Group"));
                assert!(resolved.matched_summary_badge.contains("NTb"));
                assert_eq!(
                    resolved.ranked_candidates.len(),
                    MAX_RANKED_NEXT_EPISODE_CANDIDATES
                );
                assert_eq!(resolved.ranked_candidates[0].0, exact_match);
                assert_eq!(resolved.ranked_candidates[0].1.as_ref(), Some(&req_a));
            }
            _ => panic!("expected Ready resolution"),
        }
    }
}
