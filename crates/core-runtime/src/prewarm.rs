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
        filter_and_rank_streams, is_allowed_stream_source, is_allowed_stream_url, parse_stream,
        Resolution, StreamFilter,
    },
};

pub const MIN_EPISODE_DURATION_FOR_ENDING_PREWARM_SECS: f64 = 180.0;

pub const ENDING_PREWARM_WINDOW_SECS: f64 = 90.0;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedNextEpisode {
    pub url: Url,
    pub stream_urls: StreamUrls,
    pub converted_stream: Stream<ConvertedStreamSource>,
    pub stream: Stream,
    pub next_video: Video,
    pub player_selected: PlayerSelected,
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
    let stream_ready = find_ready_next_stream(model, &next_video)
        .and_then(|(stream, _)| convert_stream_to_playback_url(&stream, server_url).ok())
        .is_some();

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
    if current_pos_secs >= outro_secs {
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

pub(crate) fn inspect_next_episode_resolution(model: &DesktopModel) -> NextEpisodeResolution {
    let Some(next_video) = effective_next_video(model) else {
        return NextEpisodeResolution::Unavailable;
    };

    let streaming_server_url = effective_streaming_server_url(model);

    if let Some((chosen_stream, stream_req)) = find_ready_next_stream(model, &next_video) {
        if let Ok((url, stream_urls, converted_stream)) =
            convert_stream_to_playback_url(&chosen_stream, streaming_server_url)
        {
            let player_selected = build_next_player_selected(
                &model.player,
                &next_video,
                chosen_stream.clone(),
                stream_req,
            );
            return NextEpisodeResolution::Ready(Box::new(ResolvedNextEpisode {
                url,
                stream_urls,
                converted_stream,
                stream: chosen_stream,
                next_video,
                player_selected,
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
        let any_loading = model.meta_details.streams.is_empty()
            || model
                .meta_details
                .streams
                .iter()
                .any(|res| matches!(res.content, None | Some(Loadable::Loading)));
        if any_loading {
            return NextEpisodeResolution::Unavailable;
        }
        return NextEpisodeResolution::Unavailable;
    }

    if let Some((type_name, meta_id)) = extract_player_type_and_meta_id(&model.player) {
        return NextEpisodeResolution::NeedsStreamFetch {
            type_name,
            meta_id,
            video_id: next_video.id.clone(),
        };
    }

    NextEpisodeResolution::Unavailable
}

fn find_ready_next_stream(
    model: &DesktopModel,
    next_video: &Video,
) -> Option<(Stream, Option<ResourceRequest>)> {
    let current_stream = model.player.selected.as_ref().map(|sel| &sel.stream);

    if let Some(next_stream) = model.player.next_stream.as_ref() {
        if is_allowed_stream_source(next_stream) {
            let req = model
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
            return Some((next_stream.clone(), req));
        }
    }

    if let Some(ResourceLoadable {
        request,
        content: Some(Loadable::Ready(streams)),
    }) = model.player.next_streams.as_ref()
    {
        if request.path.id == next_video.id && !streams.is_empty() {
            if let Some(best) = select_best_continuation_stream(streams, current_stream) {
                return Some((best, Some(request.clone())));
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
        let mut candidates: Vec<(Stream, ResourceRequest)> = Vec::new();
        for res in model
            .meta_details
            .streams
            .iter()
            .chain(model.meta_details.meta_streams.iter())
        {
            if res.request.path.id == next_video.id {
                if let Some(Loadable::Ready(list)) = res.content.as_ref() {
                    for s in list {
                        candidates.push((s.clone(), res.request.clone()));
                    }
                }
            }
        }
        if !candidates.is_empty() {
            let plain_streams: Vec<Stream> = candidates.iter().map(|(s, _)| s.clone()).collect();
            if let Some(best) = select_best_continuation_stream(&plain_streams, current_stream) {
                let matched_req = candidates
                    .into_iter()
                    .find(|(s, _)| s == &best)
                    .map(|(_, req)| req);
                return Some((best, matched_req));
            }
        }
    }

    if !next_video.streams.is_empty() {
        if let Some(best) = select_best_continuation_stream(&next_video.streams, current_stream) {
            let req =
                next_stream_request_from_selected(model.player.selected.as_ref(), &next_video.id);
            return Some((best, req));
        }
    } else if let Some(yt_stream) = next_video.stream() {
        let owned = yt_stream.into_owned();
        if is_allowed_stream_source(&owned) {
            return Some((owned, None));
        }
    }

    None
}

fn select_best_continuation_stream(
    candidates: &[Stream],
    current: Option<&Stream>,
) -> Option<Stream> {
    if candidates.is_empty() {
        return None;
    }

    if let Some(cur) = current {
        if let Some(binge) = candidates
            .iter()
            .find(|cand| is_allowed_stream_source(cand) && cand.is_binge_match(cur))
        {
            return Some(binge.clone());
        }

        let cur_parsed = parse_stream(cur);
        let ranked = filter_and_rank_streams(candidates, &StreamFilter::default());

        let mut best_idx: Option<(i32, usize)> = None;
        for (rank_pos, (_, parsed_cand)) in ranked.iter().enumerate() {
            let mut score: i32 = 1000 - (rank_pos as i32 * 5);

            if cur_parsed.is_cached && parsed_cand.is_cached {
                score += 200;
            }
            if cur_parsed.debrid_provider.is_some()
                && cur_parsed.debrid_provider == parsed_cand.debrid_provider
            {
                score += 80;
            }
            if cur_parsed.resolution != Resolution::Unknown
                && cur_parsed.resolution == parsed_cand.resolution
            {
                score += 120;
            }
            if cur_parsed.hdr == parsed_cand.hdr {
                score += 40;
            }
            if cur_parsed.quality == parsed_cand.quality {
                score += 35;
            }

            match best_idx {
                None => best_idx = Some((score, rank_pos)),
                Some((best_score, _)) if score > best_score => {
                    best_idx = Some((score, rank_pos));
                }
                _ => {}
            }
        }

        if let Some((_, pos)) = best_idx {
            return Some(ranked[pos].0.clone());
        }
    }

    filter_and_rank_streams(candidates, &StreamFilter::default())
        .into_iter()
        .next()
        .map(|(stream, _)| stream.clone())
        .or_else(|| {
            candidates
                .iter()
                .find(|cand| is_allowed_stream_source(cand))
                .cloned()
        })
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
    fn triggers_prewarm_inside_outro_credits_window() {
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

        assert!(evaluate_next_episode_prewarm(&model, 1200.0, 1500.0).is_none());

        let target = evaluate_next_episode_prewarm(&model, 1265.0, 1500.0)
            .expect("should trigger inside outro window");
        assert_eq!(target.video_id, "tt0903747:1:2");
        assert_eq!(target.trigger_reason, PrewarmTriggerReason::OutroCredits);
        assert!(target.stream_ready);
    }

    #[test]
    fn triggers_prewarm_in_final_90s_only_for_episodes_longer_than_180s() {
        let mut model = dummy_model();
        model.player.next_video = Some(make_video(
            "tt0903747:1:3",
            1,
            3,
            "...And the Bag's in the River",
        ));

        assert!(evaluate_next_episode_prewarm(&model, 95.0, 120.0).is_none());
        assert!(evaluate_next_episode_prewarm(&model, 1300.0, 1440.0).is_none());

        let target = evaluate_next_episode_prewarm(&model, 1355.0, 1440.0)
            .expect("should trigger in final 90s");
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
}
