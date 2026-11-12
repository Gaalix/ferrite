use std::path::Path;

pub const VIDEO_EXTENSIONS: &[&str] = &["mkv", "mp4", "avi", "webm", "mov", "m2ts", "ts"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedByteRange {
    pub start: u64,
    pub end: u64,
    pub total_len: u64,
}

impl ResolvedByteRange {
    pub const fn content_length(&self) -> u64 {
        self.end - self.start + 1
    }

    pub fn content_range_header(&self) -> String {
        format!("bytes {}-{}/{}", self.start, self.end, self.total_len)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeParseOutcome {
    Partial(ResolvedByteRange),
    Ignore,
    Unsatisfiable { total_len: u64 },
}

pub fn parse_http_byte_range(header_val: &str, total_len: u64) -> RangeParseOutcome {
    let trimmed = header_val.trim();
    let Some(spec_list) = trimmed.strip_prefix("bytes=") else {
        return RangeParseOutcome::Ignore;
    };

    if total_len == 0 {
        return RangeParseOutcome::Unsatisfiable { total_len: 0 };
    }

    let first_spec = spec_list.split(',').next().unwrap_or("").trim();
    let Some((start_str, end_str)) = first_spec.split_once('-') else {
        return RangeParseOutcome::Unsatisfiable { total_len };
    };

    let start_str = start_str.trim();
    let end_str = end_str.trim();

    if start_str.is_empty() {
        let Ok(suffix_len) = end_str.parse::<u64>() else {
            return RangeParseOutcome::Unsatisfiable { total_len };
        };
        if suffix_len == 0 {
            return RangeParseOutcome::Unsatisfiable { total_len };
        }
        let start = total_len.saturating_sub(suffix_len);
        let end = total_len - 1;
        return RangeParseOutcome::Partial(ResolvedByteRange {
            start,
            end,
            total_len,
        });
    }

    let Ok(start) = start_str.parse::<u64>() else {
        return RangeParseOutcome::Unsatisfiable { total_len };
    };

    if start >= total_len {
        return RangeParseOutcome::Unsatisfiable { total_len };
    }

    let end = if end_str.is_empty() {
        total_len - 1
    } else {
        let Ok(parsed_end) = end_str.parse::<u64>() else {
            return RangeParseOutcome::Unsatisfiable { total_len };
        };
        if parsed_end < start {
            return RangeParseOutcome::Unsatisfiable { total_len };
        }
        parsed_end.min(total_len - 1)
    };

    RangeParseOutcome::Partial(ResolvedByteRange {
        start,
        end,
        total_len,
    })
}

pub fn is_video_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            VIDEO_EXTENSIONS
                .iter()
                .any(|video_ext| ext.eq_ignore_ascii_case(video_ext))
        })
}

pub fn content_type_for_path(path: &Path) -> String {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        match ext.to_ascii_lowercase().as_str() {
            "mkv" => return "video/x-matroska".to_owned(),
            "mp4" | "m4v" => return "video/mp4".to_owned(),
            "avi" => return "video/x-msvideo".to_owned(),
            "webm" => return "video/webm".to_owned(),
            "mov" => return "video/quicktime".to_owned(),
            "m2ts" | "ts" => return "video/mp2t".to_owned(),
            _ => {}
        }
    }
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_open_suffix_and_invalid_byte_ranges() {
        assert_eq!(
            parse_http_byte_range("bytes=0-1023", 10_000),
            RangeParseOutcome::Partial(ResolvedByteRange {
                start: 0,
                end: 1023,
                total_len: 10_000,
            })
        );
        assert_eq!(
            parse_http_byte_range("bytes=1024-", 10_000),
            RangeParseOutcome::Partial(ResolvedByteRange {
                start: 1024,
                end: 9_999,
                total_len: 10_000,
            })
        );
        assert_eq!(
            parse_http_byte_range("bytes=-500", 10_000),
            RangeParseOutcome::Partial(ResolvedByteRange {
                start: 9_500,
                end: 9_999,
                total_len: 10_000,
            })
        );
        assert_eq!(
            parse_http_byte_range("bytes=9000-999999", 10_000),
            RangeParseOutcome::Partial(ResolvedByteRange {
                start: 9_000,
                end: 9_999,
                total_len: 10_000,
            })
        );
        assert_eq!(
            parse_http_byte_range("bytes=10000-10001", 10_000),
            RangeParseOutcome::Unsatisfiable { total_len: 10_000 }
        );
        assert_eq!(
            parse_http_byte_range("bytes=500-400", 10_000),
            RangeParseOutcome::Unsatisfiable { total_len: 10_000 }
        );
        assert_eq!(
            parse_http_byte_range("bytes=-0", 10_000),
            RangeParseOutcome::Unsatisfiable { total_len: 10_000 }
        );
        assert_eq!(
            parse_http_byte_range("items=0-5", 10_000),
            RangeParseOutcome::Ignore
        );
    }

    #[test]
    fn recognizes_video_extensions_and_mime_types() {
        assert!(is_video_path(Path::new("Episode.S01E01.MKV")));
        assert!(is_video_path(Path::new("movie.m2ts")));
        assert!(!is_video_path(Path::new("subs.srt")));
        assert_eq!(
            content_type_for_path(Path::new("movie.mkv")),
            "video/x-matroska"
        );
        assert_eq!(content_type_for_path(Path::new("movie.mp4")), "video/mp4");
    }
}
