//! HTTP Range parsing and constant-memory file streaming for
//! `GET /stream/:id`. (Spec §3.4, S3, S12.)
//!
//! Passthrough streams only: the server never buffers a whole track —
//! bytes flow file → socket in 64 KiB chunks.

use axum::{
    body::Body,
    http::{header, StatusCode},
    response::Response,
};
use kahawai_core::MusicError;
use std::io::SeekFrom;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

/// Parse a `Range` request header into an inclusive `(start, end)` byte range.
///
/// Returns `Err(MusicError::BadRange)` for malformed headers and for ranges
/// that cannot be satisfied (caller turns that into `416` with a
/// `Content-Range: bytes */{total}` header).
pub fn parse_range(header_value: &str, total: u64) -> Result<(u64, u64), MusicError> {
    if total == 0 {
        return Err(MusicError::BadRange);
    }
    let spec = header_value
        .strip_prefix("bytes=")
        .ok_or(MusicError::BadRange)?;
    // Single-range only; "bytes=0-99,200-299" is rejected as malformed.
    let (start_s, end_s) = spec.split_once('-').ok_or(MusicError::BadRange)?;
    if start_s.contains(',') || end_s.contains(',') {
        return Err(MusicError::BadRange);
    }

    if start_s.is_empty() {
        // Suffix range: "bytes=-N" = the last N bytes.
        let n: u64 = end_s.parse().map_err(|_| MusicError::BadRange)?;
        if n == 0 {
            return Err(MusicError::BadRange);
        }
        Ok((total.saturating_sub(n), total - 1))
    } else {
        let start: u64 = start_s.parse().map_err(|_| MusicError::BadRange)?;
        if start >= total {
            return Err(MusicError::BadRange);
        }
        let end: u64 = if end_s.is_empty() {
            total - 1
        } else {
            end_s.parse().map_err(|_| MusicError::BadRange)?
        };
        let end = end.min(total - 1);
        if start > end {
            return Err(MusicError::BadRange);
        }
        Ok((start, end))
    }
}

/// Build the response for a (possibly ranged) file stream.
///
/// `range = None` → `200 OK` with the whole file; `Some` → `206 Partial
/// Content` with `Content-Range`. Memory use is O(chunk), independent of file
/// size — a 200 MB DSF streams in the same footprint as a 3 MB MP3.
pub async fn serve_file(
    mut file: tokio::fs::File,
    total: u64,
    range: Option<(u64, u64)>,
    content_type: &'static str,
) -> Result<Response, MusicError> {
    if total == 0 {
        return Response::builder()
            .header(header::CONTENT_TYPE, content_type)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, 0u64)
            .body(Body::empty())
            .map_err(|e| MusicError::Http(e.to_string()));
    }

    let (start, end) = range.unwrap_or((0, total - 1));
    let len = end - start + 1;
    file.seek(SeekFrom::Start(start)).await?;
    let limited = file.take(len);
    let body = Body::from_stream(ReaderStream::new(limited));

    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, len);
    if range.is_some() {
        builder = builder.status(StatusCode::PARTIAL_CONTENT).header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total}"),
        );
    }
    builder
        .body(body)
        .map_err(|e| MusicError::Http(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(header: &str, total: u64) -> (u64, u64) {
        parse_range(header, total).expect("range should parse")
    }

    fn is_bad_range(header: &str, total: u64) -> bool {
        matches!(parse_range(header, total), Err(MusicError::BadRange))
    }

    #[test]
    fn parses_simple_range() {
        assert_eq!(parsed("bytes=0-99", 1000), (0, 99));
    }

    #[test]
    fn parses_open_ended_range() {
        assert_eq!(parsed("bytes=500-", 1000), (500, 999));
    }

    #[test]
    fn parses_suffix_range() {
        assert_eq!(parsed("bytes=-100", 1000), (900, 999));
        // Suffix longer than the file: whole file.
        assert_eq!(parsed("bytes=-5000", 1000), (0, 999));
    }

    #[test]
    fn clamps_end_past_eof() {
        assert_eq!(parsed("bytes=0-9999", 1000), (0, 999));
    }

    #[test]
    fn rejects_unsatisfiable_ranges() {
        for (header, total) in [
            ("bytes=1000-", 1000), // start beyond EOF -> 416 case
            ("bytes=5000-6000", 1000),
            ("bytes=500-100", 1000), // start past end
            ("bytes=-0", 1000),      // zero-length suffix
            ("bytes=0-", 0),         // empty file
        ] {
            assert!(is_bad_range(header, total), "header={header}");
        }
    }

    #[test]
    fn rejects_malformed_headers() {
        for bad in [
            "items=0-99",    // wrong unit
            "bytes=",        // empty
            "bytes=abc-def", // not numbers
            "bytes=--5",
            "bytes=0-99,200-299", // multi-range not supported
            "0-99",               // missing unit
        ] {
            assert!(is_bad_range(bad, 1000), "header={bad}");
        }
    }
}
