use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use stremio_core::runtime::EnvError;

use crate::env::SqliteStorage;

pub const SERIES_TRACK_PREFS_STORAGE_KEY: &str = "ferrite_series_track_prefs";
const LEGACY_SERIES_TRACK_PREFS_STORAGE_KEY: &str = "stremio_gpui_series_track_prefs";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeriesTrackPreference {
    pub series_id: String,
    pub audio_lang: Option<String>,
    pub audio_title_keywords: Vec<String>,
    pub subtitles_enabled: bool,
    pub sub_lang: Option<String>,
    pub sub_title_keywords: Vec<String>,
    pub prefer_forced_only: bool,
    pub sub_delay_secs: f64,
}

impl SeriesTrackPreference {
    pub fn new(series_id: impl Into<String>) -> Self {
        Self {
            series_id: series_id.into(),
            audio_lang: None,
            audio_title_keywords: Vec::new(),
            subtitles_enabled: true,
            sub_lang: None,
            sub_title_keywords: Vec::new(),
            prefer_forced_only: false,
            sub_delay_secs: 0.0,
        }
    }

    pub fn from_selected_tracks(
        series_id: impl Into<String>,
        audio: Option<&TrackCandidate>,
        subtitle: Option<&TrackCandidate>,
        subtitles_enabled: bool,
        sub_delay_secs: f64,
    ) -> Self {
        let audio_lang = audio.and_then(|track| {
            track
                .lang
                .as_deref()
                .filter(|lang| !is_undetermined_lang(lang))
                .map(str::to_owned)
                .or_else(|| infer_lang_from_title(track.title.as_deref()).map(str::to_owned))
        });
        let audio_title_keywords = audio
            .and_then(|track| track.title.as_deref())
            .map(extract_track_keywords)
            .unwrap_or_default();

        let (sub_lang, sub_title_keywords, prefer_forced_only) = if subtitles_enabled {
            if let Some(sub) = subtitle {
                let lang = sub
                    .lang
                    .as_deref()
                    .filter(|l| !is_undetermined_lang(l))
                    .map(str::to_owned)
                    .or_else(|| infer_lang_from_title(sub.title.as_deref()).map(str::to_owned));
                let keywords = sub
                    .title
                    .as_deref()
                    .map(extract_track_keywords)
                    .unwrap_or_default();
                let role = classify_subtitle_role(sub.title.as_deref(), sub.forced);
                (lang, keywords, role == SubtitleRole::ForcedOrSigns)
            } else {
                (None, Vec::new(), false)
            }
        } else {
            (None, Vec::new(), false)
        };

        Self {
            series_id: series_id.into(),
            audio_lang,
            audio_title_keywords,
            subtitles_enabled,
            sub_lang,
            sub_title_keywords,
            prefer_forced_only,
            sub_delay_secs,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrackKind {
    Audio,
    Sub,
    Video,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackCandidate {
    pub id: i64,
    pub kind: TrackKind,
    pub lang: Option<String>,
    pub title: Option<String>,
    pub codec: Option<String>,
    pub forced: bool,
    pub default: bool,
    pub external: bool,
}

impl TrackCandidate {
    pub fn audio(id: i64, lang: Option<&str>, title: Option<&str>) -> Self {
        Self {
            id,
            kind: TrackKind::Audio,
            lang: lang.map(str::to_owned),
            title: title.map(str::to_owned),
            codec: None,
            forced: false,
            default: false,
            external: false,
        }
    }

    pub fn sub(id: i64, lang: Option<&str>, title: Option<&str>) -> Self {
        Self {
            id,
            kind: TrackKind::Sub,
            lang: lang.map(str::to_owned),
            title: title.map(str::to_owned),
            codec: None,
            forced: false,
            default: false,
            external: false,
        }
    }

    pub fn with_codec(mut self, codec: impl Into<String>) -> Self {
        self.codec = Some(codec.into());
        self
    }

    pub fn with_forced(mut self, forced: bool) -> Self {
        self.forced = forced;
        self
    }

    pub fn with_default(mut self, default: bool) -> Self {
        self.default = default;
        self
    }

    pub fn with_external(mut self, external: bool) -> Self {
        self.external = external;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchedTracks {
    pub audio_id: Option<i64>,
    pub sub_id: Option<Option<i64>>,
}

pub fn has_full_embedded_subtitle_for_lang(
    tracks: &[TrackCandidate],
    preferred_lang: &str,
) -> bool {
    let Some((target_primary, target_region)) = parse_lang_tag(preferred_lang) else {
        return false;
    };
    tracks.iter().any(|track| {
        if track.kind != TrackKind::Sub || track.external {
            return false;
        }
        if match_track_language(&target_primary, target_region.as_deref(), track).is_none() {
            return false;
        }
        matches!(
            classify_subtitle_role(track.title.as_deref(), track.forced),
            SubtitleRole::FullExplicit | SubtitleRole::Standard | SubtitleRole::Sdh
        )
    })
}

pub fn select_preferred_tracks(
    pref: &SeriesTrackPreference,
    tracks: &[TrackCandidate],
) -> MatchedTracks {
    let audio_id = select_audio_track(pref, tracks);

    let sub_id = if !pref.subtitles_enabled {
        Some(None)
    } else {
        select_subtitle_track(pref, tracks).map(Some)
    };

    MatchedTracks { audio_id, sub_id }
}

fn select_audio_track(pref: &SeriesTrackPreference, tracks: &[TrackCandidate]) -> Option<i64> {
    let has_lang_pref = pref
        .audio_lang
        .as_deref()
        .is_some_and(|l| !l.trim().is_empty() && !is_undetermined_lang(l));
    let normalized_keywords = normalize_keyword_list(&pref.audio_title_keywords);

    if !has_lang_pref && normalized_keywords.is_empty() {
        return None;
    }

    let wants_commentary = keywords_request_commentary(&normalized_keywords);
    let wants_descriptive = keywords_request_descriptive(&normalized_keywords);
    let pref_lang = pref.audio_lang.as_deref().and_then(parse_lang_tag);

    let mut best: Option<(i32, usize, i64)> = None;

    for (idx, track) in tracks.iter().enumerate() {
        if track.kind != TrackKind::Audio {
            continue;
        }

        let mut score: i32 = 0;

        if let Some((ref target_primary, ref target_region)) = pref_lang {
            match match_track_language(target_primary, target_region.as_deref(), track) {
                Some(lang_points) => score += lang_points,
                None => continue,
            }
        }

        let title_lower = track
            .title
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();

        if is_commentary_title(&title_lower) && !wants_commentary {
            score -= 500;
        }
        if is_descriptive_audio_title(&title_lower) && !wants_descriptive {
            score -= 500;
        }

        score += score_keyword_overlap(&normalized_keywords, track.title.as_deref(), track.codec.as_deref());

        if track.default {
            score += 5;
        }
        if !track.external {
            score += 2;
        }

        if score <= 0 {
            continue;
        }

        match best {
            None => best = Some((score, idx, track.id)),
            Some((best_score, best_idx, _))
                if score > best_score || (score == best_score && idx < best_idx) =>
            {
                best = Some((score, idx, track.id));
            }
            _ => {}
        }
    }

    best.map(|(_, _, id)| id)
}

fn select_subtitle_track(pref: &SeriesTrackPreference, tracks: &[TrackCandidate]) -> Option<i64> {
    let has_lang_pref = pref
        .sub_lang
        .as_deref()
        .is_some_and(|l| !l.trim().is_empty() && !is_undetermined_lang(l));
    let normalized_keywords = normalize_keyword_list(&pref.sub_title_keywords);

    if !has_lang_pref && normalized_keywords.is_empty() && !pref.prefer_forced_only {
        return None;
    }

    let wants_forced = pref.prefer_forced_only || keywords_request_forced(&normalized_keywords);
    let wants_commentary = keywords_request_commentary(&normalized_keywords);
    let wants_sdh = keywords_request_sdh(&normalized_keywords);
    let pref_lang = pref.sub_lang.as_deref().and_then(parse_lang_tag);

    let mut best: Option<(i32, usize, i64)> = None;

    for (idx, track) in tracks.iter().enumerate() {
        if track.kind != TrackKind::Sub {
            continue;
        }

        let mut score: i32 = 0;

        if let Some((ref target_primary, ref target_region)) = pref_lang {
            match match_track_language(target_primary, target_region.as_deref(), track) {
                Some(lang_points) => score += lang_points,
                None => continue,
            }
        }

        let role = classify_subtitle_role(track.title.as_deref(), track.forced);

        match role {
            SubtitleRole::Commentary => {
                if wants_commentary {
                    score += 140;
                } else {
                    score -= 500;
                }
            }
            SubtitleRole::ForcedOrSigns => {
                if wants_forced {
                    score += 160;
                    if track.forced {
                        score += 20;
                    }
                } else {
                    score -= 220;
                }
            }
            SubtitleRole::FullExplicit => {
                if wants_forced {
                    score -= 200;
                } else {
                    score += 85;
                }
            }
            SubtitleRole::Sdh => {
                if wants_forced {
                    score -= 220;
                } else if wants_sdh {
                    score += 95;
                } else {
                    score += 25;
                }
            }
            SubtitleRole::Standard => {
                if wants_forced {
                    score -= 200;
                } else {
                    score += 55;
                }
            }
        }

        score += score_keyword_overlap(
            &normalized_keywords,
            track.title.as_deref(),
            track.codec.as_deref(),
        );

        if !track.external {
            score += 5;
        }
        if track.default && !wants_forced {
            score += 3;
        }

        if score <= 0 {
            continue;
        }

        match best {
            None => best = Some((score, idx, track.id)),
            Some((best_score, best_idx, _))
                if score > best_score || (score == best_score && idx < best_idx) =>
            {
                best = Some((score, idx, track.id));
            }
            _ => {}
        }
    }

    best.map(|(_, _, id)| id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubtitleRole {
    FullExplicit,
    Standard,
    ForcedOrSigns,
    Sdh,
    Commentary,
}

fn classify_subtitle_role(title: Option<&str>, forced_flag: bool) -> SubtitleRole {
    let raw = title.unwrap_or_default().to_ascii_lowercase();
    let tokens = tokenize_alphanumeric(&raw);

    if is_commentary_title(&raw) {
        return SubtitleRole::Commentary;
    }

    let has_full_marker = tokens.iter().any(|tok| {
        matches!(
            tok.as_str(),
            "full" | "dialogue" | "dialog" | "complete" | "unforced"
        )
    });

    let has_signs_or_forced_marker = forced_flag
        || raw.contains("s&s")
        || raw.contains("sign & song")
        || raw.contains("signs & songs")
        || raw.contains("signs/songs")
        || raw.contains("sign/song")
        || tokens
            .iter()
            .any(|tok| matches!(tok.as_str(), "signs" | "sign" | "songs" | "forced" | "typeset"));

    if has_full_marker && !forced_flag {
        return SubtitleRole::FullExplicit;
    }
    if has_signs_or_forced_marker {
        return SubtitleRole::ForcedOrSigns;
    }
    if has_full_marker {
        return SubtitleRole::FullExplicit;
    }

    let has_sdh_marker = raw.contains("hearing impaired")
        || raw.contains("closed caption")
        || tokens
            .iter()
            .any(|tok| matches!(tok.as_str(), "sdh" | "cc" | "hi" | "deaf"));
    if has_sdh_marker {
        return SubtitleRole::Sdh;
    }

    SubtitleRole::Standard
}

fn is_commentary_title(lower_title: &str) -> bool {
    lower_title.contains("commentary")
        || lower_title.contains("director's")
        || lower_title.contains("directors ")
        || lower_title.contains("cast & crew")
}

fn is_descriptive_audio_title(lower_title: &str) -> bool {
    lower_title.contains("audio description")
        || lower_title.contains("descriptive")
        || lower_title.contains("visually impaired")
}

fn keywords_request_forced(keywords: &[String]) -> bool {
    keywords.iter().any(|kw| {
        matches!(
            kw.as_str(),
            "signs" | "sign" | "songs" | "s&s" | "forced" | "typeset"
        )
    })
}

fn keywords_request_commentary(keywords: &[String]) -> bool {
    keywords
        .iter()
        .any(|kw| matches!(kw.as_str(), "commentary" | "director" | "directors"))
}

fn keywords_request_descriptive(keywords: &[String]) -> bool {
    keywords
        .iter()
        .any(|kw| matches!(kw.as_str(), "description" | "descriptive" | "impaired"))
}

fn keywords_request_sdh(keywords: &[String]) -> bool {
    keywords
        .iter()
        .any(|kw| matches!(kw.as_str(), "sdh" | "cc" | "impaired" | "deaf"))
}

pub fn extract_track_keywords(title: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "track",
        "audio",
        "sub",
        "subs",
        "subtitle",
        "subtitles",
        "stereo",
        "mono",
        "surround",
        "ch",
        "the",
        "and",
        "with",
        "for",
        "incl",
        "including",
    ];

    let mut out = Vec::new();
    let lower = title.to_ascii_lowercase();
    if lower.contains("s&s") {
        out.push("signs".to_owned());
        out.push("songs".to_owned());
    }

    for tok in tokenize_alphanumeric(&lower) {
        if tok.len() < 2 {
            continue;
        }
        if tok.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if STOPWORDS.contains(&tok.as_str()) {
            continue;
        }
        if canonicalize_primary_lang(&tok).is_some() && tok.len() > 3 {
            continue;
        }
        if !out.contains(&tok) {
            out.push(tok);
        }
    }
    out
}

fn normalize_keyword_list(keywords: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for raw in keywords {
        let lower = raw.trim().to_ascii_lowercase();
        if lower == "s&s" {
            if !normalized.contains(&"signs".to_owned()) {
                normalized.push("signs".to_owned());
            }
            if !normalized.contains(&"songs".to_owned()) {
                normalized.push("songs".to_owned());
            }
            continue;
        }
        for tok in tokenize_alphanumeric(&lower) {
            if !tok.is_empty() && !normalized.contains(&tok) {
                normalized.push(tok);
            }
        }
    }
    normalized
}

fn score_keyword_overlap(
    keywords: &[String],
    title: Option<&str>,
    codec: Option<&str>,
) -> i32 {
    if keywords.is_empty() {
        return 0;
    }

    let mut hay_tokens = Vec::new();
    if let Some(t) = title {
        let lower = t.to_ascii_lowercase();
        if lower.contains("s&s") {
            hay_tokens.push("signs".to_owned());
            hay_tokens.push("songs".to_owned());
        }
        hay_tokens.extend(tokenize_alphanumeric(&lower));
    }
    if let Some(c) = codec {
        hay_tokens.extend(tokenize_alphanumeric(&c.to_ascii_lowercase()));
    }

    if hay_tokens.is_empty() {
        return 0;
    }

    let mut bonus = 0;
    for kw in keywords {
        if matches!(
            kw.as_str(),
            "sub" | "subs" | "subtitle" | "subtitles" | "audio" | "track"
        ) {
            continue;
        }
        if hay_tokens.iter().any(|tok| tok == kw) {
            bonus += 35;
        }
    }
    bonus
}

fn tokenize_alphanumeric(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn is_undetermined_lang(lang: &str) -> bool {
    matches!(
        lang.trim().to_ascii_lowercase().as_str(),
        "" | "und" | "unk" | "unknown" | "mul" | "zxx"
    )
}

fn match_track_language(
    target_primary: &str,
    target_region: Option<&str>,
    track: &TrackCandidate,
) -> Option<i32> {
    if let Some(raw_lang) = track.lang.as_deref() {
        if !is_undetermined_lang(raw_lang) {
            let (cand_primary, cand_region) = parse_lang_tag(raw_lang)?;
            if cand_primary != target_primary {
                return None;
            }
            let mut pts = 100;
            if let (Some(t_reg), Some(c_reg)) = (target_region, cand_region.as_deref()) {
                if t_reg == c_reg {
                    pts += 25;
                } else {
                    pts -= 10;
                }
            }
            return Some(pts);
        }
    }

    if let Some(inferred) = infer_lang_from_title(track.title.as_deref()) {
        if inferred == target_primary {
            return Some(75);
        }
        return None;
    }

    None
}

fn infer_lang_from_title(title: Option<&str>) -> Option<&'static str> {
    let title = title?;
    for tok in tokenize_alphanumeric(title) {
        if tok.len() >= 4 {
            if let Some(code) = canonicalize_known_lang(&tok) {
                return Some(code);
            }
        }
    }
    None
}

pub fn parse_lang_tag(raw: &str) -> Option<(String, Option<String>)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || is_undetermined_lang(trimmed) {
        return None;
    }

    let mut parts = trimmed.split(['-', '_']);
    let primary_raw = parts.next()?.trim().to_ascii_lowercase();
    if primary_raw.is_empty() {
        return None;
    }

    let region = parts
        .next()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());

    let canonical = canonicalize_primary_lang(&primary_raw)
        .map(str::to_owned)
        .unwrap_or(primary_raw);

    Some((canonical, region))
}

fn canonicalize_primary_lang(code: &str) -> Option<&'static str> {
    canonicalize_known_lang(code)
}

fn canonicalize_known_lang(code: &str) -> Option<&'static str> {
    match code {
        "en" | "eng" | "english" => Some("eng"),
        "ja" | "jp" | "jpn" | "japanese" => Some("jpn"),
        "es" | "spa" | "esp" | "spanish" | "espanol" | "castilian" | "latino" => Some("spa"),
        "fr" | "fra" | "fre" | "french" | "francais" => Some("fra"),
        "de" | "deu" | "ger" | "german" | "deutsch" => Some("deu"),
        "pt" | "por" | "portuguese" | "portugues" => Some("por"),
        "it" | "ita" | "italian" | "italiano" => Some("ita"),
        "ru" | "rus" | "russian" => Some("rus"),
        "ko" | "kor" | "korean" => Some("kor"),
        "zh" | "zho" | "chi" | "cmn" | "chinese" | "mandarin" | "cantonese" => Some("zho"),
        "ar" | "ara" | "arabic" => Some("ara"),
        "hi" | "hin" | "hindi" => Some("hin"),
        "pl" | "pol" | "polish" | "polski" => Some("pol"),
        "nl" | "nld" | "dut" | "dutch" | "nederlands" => Some("nld"),
        "tr" | "tur" | "turkish" | "turkce" => Some("tur"),
        "sv" | "swe" | "swedish" | "svenska" => Some("swe"),
        "no" | "nor" | "nob" | "nno" | "norwegian" => Some("nor"),
        "da" | "dan" | "danish" | "dansk" => Some("dan"),
        "fi" | "fin" | "finnish" | "suomi" => Some("fin"),
        "cs" | "ces" | "cze" | "czech" => Some("ces"),
        "hu" | "hun" | "hungarian" | "magyar" => Some("hun"),
        "ro" | "ron" | "rum" | "romanian" => Some("ron"),
        "el" | "ell" | "gre" | "greek" => Some("ell"),
        "he" | "heb" | "hebrew" => Some("heb"),
        "uk" | "ukr" | "ukrainian" => Some("ukr"),
        "th" | "tha" | "thai" => Some("tha"),
        "vi" | "vie" | "vietnamese" => Some("vie"),
        "id" | "ind" | "indonesian" => Some("ind"),
        "ms" | "msa" | "may" | "malay" => Some("msa"),
        "bg" | "bul" | "bulgarian" => Some("bul"),
        "hr" | "hrv" | "croatian" => Some("hrv"),
        "sr" | "srp" | "serbian" => Some("srp"),
        "sk" | "slk" | "slo" | "slovak" => Some("slk"),
        "sl" | "slv" | "slovenian" => Some("slv"),
        _ => None,
    }
}

fn read_merged_track_prefs(storage: &SqliteStorage) -> HashMap<String, SeriesTrackPreference> {
    let mut merged: HashMap<String, SeriesTrackPreference> = storage
        .get_json(LEGACY_SERIES_TRACK_PREFS_STORAGE_KEY)
        .ok()
        .flatten()
        .unwrap_or_default();
    if let Some(primary) = storage
        .get_json::<HashMap<String, SeriesTrackPreference>>(SERIES_TRACK_PREFS_STORAGE_KEY)
        .ok()
        .flatten()
    {
        merged.extend(primary);
    }
    merged
}

pub(crate) fn load_series_track_pref(
    storage: &SqliteStorage,
    series_id: &str,
) -> Option<SeriesTrackPreference> {
    let key = series_id.trim();
    if key.is_empty() {
        return None;
    }
    read_merged_track_prefs(storage).remove(key)
}

pub(crate) fn store_series_track_pref(
    storage: &SqliteStorage,
    mut pref: SeriesTrackPreference,
) -> Result<(), EnvError> {
    let trimmed_id = pref.series_id.trim().to_owned();
    if trimmed_id.is_empty() {
        return Ok(());
    }
    pref.series_id = trimmed_id.clone();
    let legacy_seed: HashMap<String, SeriesTrackPreference> = storage
        .get_json(LEGACY_SERIES_TRACK_PREFS_STORAGE_KEY)
        .ok()
        .flatten()
        .unwrap_or_default();
    storage.update_json::<HashMap<String, SeriesTrackPreference>, ()>(
        SERIES_TRACK_PREFS_STORAGE_KEY,
        move |map| {
            for (k, v) in legacy_seed {
                map.entry(k).or_insert(v);
            }
            map.insert(trimmed_id, pref);
        },
    )
}

pub(crate) fn delete_series_track_pref(
    storage: &SqliteStorage,
    series_id: &str,
) -> Result<bool, EnvError> {
    let key = series_id.trim();
    if key.is_empty() {
        return Ok(false);
    }
    let legacy_removed = if storage
        .get_raw(LEGACY_SERIES_TRACK_PREFS_STORAGE_KEY)
        .ok()
        .flatten()
        .is_some()
    {
        storage
            .update_json::<HashMap<String, SeriesTrackPreference>, bool>(
                LEGACY_SERIES_TRACK_PREFS_STORAGE_KEY,
                |map| map.remove(key).is_some(),
            )
            .unwrap_or(false)
    } else {
        false
    };
    let primary_removed = storage.update_json::<HashMap<String, SeriesTrackPreference>, bool>(
        SERIES_TRACK_PREFS_STORAGE_KEY,
        |map| map.remove(key).is_some(),
    )?;
    Ok(primary_removed || legacy_removed)
}

pub(crate) fn load_all_series_track_prefs(
    storage: &SqliteStorage,
) -> HashMap<String, SeriesTrackPreference> {
    read_merged_track_prefs(storage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dual_audio_anime_tracks() -> Vec<TrackCandidate> {
        vec![
            TrackCandidate::audio(1, Some("eng"), Some("English 5.1 Dub")).with_default(true),
            TrackCandidate::audio(2, Some("jpn"), Some("Japanese 2.0 FLAC")),
            TrackCandidate::audio(3, Some("jpn"), Some("Japanese Cast Commentary")),
            TrackCandidate::sub(1, Some("eng"), Some("Signs & Songs"))
                .with_forced(true)
                .with_default(true),
            TrackCandidate::sub(2, Some("eng"), Some("Full Dialogue [SubsPlease]")),
            TrackCandidate::sub(3, Some("eng"), Some("English [SDH]")),
            TrackCandidate::sub(4, Some("eng"), Some("Director Commentary")),
            TrackCandidate::sub(5, Some("spa"), Some("Español (Latino) [Full]")),
        ]
    }

    #[test]
    fn selects_japanese_audio_and_full_english_dialogue_over_signs_and_commentary() {
        let tracks = dual_audio_anime_tracks();
        let pref = SeriesTrackPreference {
            series_id: "tt0999999".to_owned(),
            audio_lang: Some("ja".to_owned()),
            audio_title_keywords: vec![],
            subtitles_enabled: true,
            sub_lang: Some("en".to_owned()),
            sub_title_keywords: vec!["full".to_owned(), "dialogue".to_owned()],
            prefer_forced_only: false,
            sub_delay_secs: 0.0,
        };

        let matched = select_preferred_tracks(&pref, &tracks);
        assert_eq!(matched.audio_id, Some(2));
        assert_eq!(matched.sub_id, Some(Some(2)));
    }

    #[test]
    fn carries_full_sub_preference_across_episodes_with_different_naming() {
        let ep1_tracks = dual_audio_anime_tracks();
        let saved_pref = SeriesTrackPreference::from_selected_tracks(
            "tt0999999",
            Some(&ep1_tracks[1]),
            Some(&ep1_tracks[4]),
            true,
            -0.15,
        );
        assert!(!saved_pref.prefer_forced_only);

        let ep2_tracks = vec![
            TrackCandidate::audio(10, Some("eng"), Some("English TrueHD 5.1")).with_default(true),
            TrackCandidate::audio(11, Some("jpn"), Some("Japanese LPCM 2.0")),
            TrackCandidate::sub(20, Some("eng"), Some("English [Signs / Songs]")).with_forced(true),
            TrackCandidate::sub(21, Some("eng"), Some("English Subtitles [BD]")),
            TrackCandidate::sub(22, Some("eng"), Some("Commentary")),
        ];

        let matched = select_preferred_tracks(&saved_pref, &ep2_tracks);
        assert_eq!(matched.audio_id, Some(11));
        assert_eq!(matched.sub_id, Some(Some(21)));
    }

    #[test]
    fn selects_signs_and_songs_when_prefer_forced_only_is_enabled() {
        let tracks = dual_audio_anime_tracks();
        let saved_pref = SeriesTrackPreference::from_selected_tracks(
            "tt0999999",
            Some(&tracks[0]),
            Some(&tracks[3]),
            true,
            0.0,
        );
        assert!(saved_pref.prefer_forced_only);

        let ep2_tracks = vec![
            TrackCandidate::audio(1, Some("jpn"), Some("Japanese 2.0")).with_default(true),
            TrackCandidate::audio(2, Some("eng"), Some("English 5.1")),
            TrackCandidate::sub(10, Some("eng"), Some("Full Subtitles")).with_default(true),
            TrackCandidate::sub(11, Some("eng"), Some("Signs & Songs")),
        ];

        let matched = select_preferred_tracks(&saved_pref, &ep2_tracks);
        assert_eq!(matched.audio_id, Some(2));
        assert_eq!(matched.sub_id, Some(Some(11)));
    }

    #[test]
    fn full_plus_signs_combined_track_is_recognized_as_full_dialogue() {
        let tracks = vec![
            TrackCandidate::sub(1, Some("eng"), Some("Signs & Songs")).with_forced(true),
            TrackCandidate::sub(
                2,
                Some("eng"),
                Some("Full Subtitles (incl. Signs & Songs)"),
            ),
        ];

        let pref = SeriesTrackPreference {
            series_id: "tt12345".to_owned(),
            audio_lang: None,
            audio_title_keywords: vec![],
            subtitles_enabled: true,
            sub_lang: Some("eng".to_owned()),
            sub_title_keywords: vec![],
            prefer_forced_only: false,
            sub_delay_secs: 0.0,
        };

        let matched = select_preferred_tracks(&pref, &tracks);
        assert_eq!(matched.sub_id, Some(Some(2)));
    }

    #[test]
    fn disables_subtitles_explicitly_when_subtitles_enabled_is_false() {
        let tracks = dual_audio_anime_tracks();
        let pref = SeriesTrackPreference {
            series_id: "tt0903747".to_owned(),
            audio_lang: Some("eng".to_owned()),
            audio_title_keywords: vec![],
            subtitles_enabled: false,
            sub_lang: None,
            sub_title_keywords: vec![],
            prefer_forced_only: false,
            sub_delay_secs: 0.0,
        };

        let matched = select_preferred_tracks(&pref, &tracks);
        assert_eq!(matched.audio_id, Some(1));
        assert_eq!(matched.sub_id, Some(None));
    }

    #[test]
    fn honors_regional_language_tags_and_undetermined_title_fallback() {
        let tracks = vec![
            TrackCandidate::audio(1, Some("und"), Some("Japanese 2.0 AAC")),
            TrackCandidate::sub(1, Some("es-ES"), Some("Español (España)")),
            TrackCandidate::sub(2, Some("es-419"), Some("Español (Latinoamérica)")),
        ];

        let pref = SeriesTrackPreference {
            series_id: "tt7777777".to_owned(),
            audio_lang: Some("jpn".to_owned()),
            audio_title_keywords: vec![],
            subtitles_enabled: true,
            sub_lang: Some("es-419".to_owned()),
            sub_title_keywords: vec![],
            prefer_forced_only: false,
            sub_delay_secs: 0.0,
        };

        let matched = select_preferred_tracks(&pref, &tracks);
        assert_eq!(matched.audio_id, Some(1));
        assert_eq!(matched.sub_id, Some(Some(2)));
    }

    #[test]
    fn recognizes_only_full_embedded_subtitles_in_preferred_language() {
        let forced_only = vec![
            TrackCandidate::sub(1, Some("eng"), Some("Forced")).with_forced(true),
            TrackCandidate::sub(2, Some("eng"), Some("Commentary")),
            TrackCandidate::sub(3, Some("eng"), Some("External SRT")).with_external(true),
        ];
        assert!(!has_full_embedded_subtitle_for_lang(&forced_only, "eng"));

        let with_full = vec![
            TrackCandidate::sub(1, Some("eng"), Some("Forced")).with_forced(true),
            TrackCandidate::sub(2, Some("eng"), Some("English")),
        ];
        assert!(has_full_embedded_subtitle_for_lang(&with_full, "eng"));
        assert!(!has_full_embedded_subtitle_for_lang(&with_full, "spa"));
    }
}
