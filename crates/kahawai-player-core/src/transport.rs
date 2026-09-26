//! HTTP transport for `/stream/:id`. (Spec §3.4, S12.)
//!
//! [`Transport`] is the seam between the playback engine and the network:
//! tests inject an in-memory stub, the Tauri shell uses [`HttpTransport`]
//! (blocking `ureq` — the engine owns its own thread, so no async runtime
//! is needed inside kahawai-player-core).

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use kahawai_core::{api::StreamFormat, MusicError};

/// How to open one stream. Mirrors the server's `StreamQuery` (§3.4).
#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    /// Explicit `?format=` rendition. `None` = let the server ladder decide.
    pub format: Option<StreamFormat>,
    /// Transcode-side seek in milliseconds (S12). Ignored by the server
    /// for passthrough — see `range_start`.
    pub seek_ms: Option<u64>,
    /// Gapless chaining (S8): the track id to serve after this one.
    pub next: Option<i64>,
    /// Passthrough seek: resume the byte stream here (HTTP Range).
    pub range_start: Option<u64>,
}

/// How much of a response the client has pulled off the network. The
/// transport owns the counter (it is the only party that sees every byte).
#[derive(Debug, Clone)]
pub struct StreamProgress {
    /// Bytes read from this response so far.
    pub received: Arc<AtomicU64>,
    /// `Content-Length` of this response; `None` for chunked (transcoded)
    /// bodies, whose total size is unknown.
    pub content_length: Option<u64>,
    /// Byte offset this response starts at (HTTP Range resume), else 0.
    pub offset: u64,
}

impl StreamProgress {
    /// Fraction of the whole resource received, 0.0–1.0 (`None` if unknown).
    /// Relative to the response, or to the full file for a Range resume.
    pub fn fraction(&self) -> Option<f64> {
        let len = self.content_length?;
        if len == 0 {
            return None;
        }
        let got = self.received.load(Ordering::Relaxed);
        Some(((self.offset + got) as f64 / (self.offset + len) as f64).clamp(0.0, 1.0))
    }
}

/// Counts bytes as they pass through.
struct CountingReader<R> {
    inner: R,
    count: Arc<AtomicU64>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// An opened stream: the byte reader plus the response metadata the
/// engine needs for gapless handoff and UI display.
pub struct StreamInfo {
    pub reader: Box<dyn Read + Send>,
    /// Response `Content-Type`.
    pub content_type: String,
    /// `X-Transcode-Chain`, e.g. `wav->passthrough`.
    pub chain: Option<String>,
    /// `X-Gapless-Next`: the track id the server chained (or named).
    pub gapless_next: Option<i64>,
    /// `X-Gapless-Mode`: `single-session` | `chained`, when the server
    /// actually chained audio into this response.
    pub gapless_mode: Option<String>,
    /// Network progress, when the transport can measure it.
    pub progress: Option<StreamProgress>,
}

/// Opens `/stream/:id` responses. Object-safe so the engine can hold
/// `Box<dyn Transport>`.
pub trait Transport: Send {
    fn open_stream(&self, track_id: i64, opts: &StreamOptions) -> Result<StreamInfo, MusicError>;
}

/// Live transport over HTTP. The base URL is behind a lock so the UI can
/// change servers without rebuilding the engine.
pub struct HttpTransport {
    base_url: Arc<RwLock<String>>,
    agent: ureq::Agent,
}

impl HttpTransport {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: Arc::new(RwLock::new(base_url.into())),
            agent: ureq::Agent::new_with_defaults(),
        }
    }

    /// Share ownership of the base URL with the engine, so a settings
    /// change applies to in-flight playback without rebuilding anything.
    pub fn with_shared_url(base_url: Arc<RwLock<String>>) -> Self {
        Self {
            base_url,
            agent: ureq::Agent::new_with_defaults(),
        }
    }

    pub fn set_base_url(&self, url: &str) {
        *self.base_url.write().expect("base_url lock") = url.to_string();
    }

    fn stream_url(&self, track_id: i64, opts: &StreamOptions) -> String {
        let base = self.base_url.read().expect("base_url lock");
        let base = base.trim_end_matches('/');
        let mut url = format!("{base}/stream/{track_id}");
        let mut first = true;
        let mut push = |k: &str, v: String| {
            url.push(if first { '?' } else { '&' });
            first = false;
            url.push_str(k);
            url.push('=');
            url.push_str(&v);
        };
        if let Some(f) = opts.format {
            // StreamFormat serializes snake_case: passthrough|flac|opus|mp3|dop.
            push(
                "format",
                serde_json::to_string(&f)
                    .expect("format serializes")
                    .trim_matches('"')
                    .to_string(),
            );
        }
        if let Some(ms) = opts.seek_ms {
            push("seek_ms", ms.to_string());
        }
        if let Some(n) = opts.next {
            push("next", n.to_string());
        }
        url
    }
}

impl Transport for HttpTransport {
    fn open_stream(&self, track_id: i64, opts: &StreamOptions) -> Result<StreamInfo, MusicError> {
        let url = self.stream_url(track_id, opts);
        let mut req = self.agent.get(&url);
        if let Some(start) = opts.range_start {
            req = req.header("Range", &format!("bytes={start}-"));
        }
        let resp = req.call().map_err(|e| map_ureq_error(e, track_id))?;

        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        };
        // Capture every header BEFORE moving the response into the reader.
        let content_type = header("content-type").unwrap_or_default();
        let chain = header("x-transcode-chain");
        let gapless_next = header("x-gapless-next").and_then(|v| v.parse().ok());
        let gapless_mode = header("x-gapless-mode");
        let content_length = header("content-length").and_then(|v| v.parse::<u64>().ok());
        let received = Arc::new(AtomicU64::new(0));
        let reader = CountingReader {
            inner: resp.into_body().into_reader(),
            count: received.clone(),
        };
        Ok(StreamInfo {
            reader: Box::new(reader),
            progress: Some(StreamProgress {
                received,
                content_length,
                offset: opts.range_start.unwrap_or(0),
            }),
            content_type,
            chain,
            gapless_next,
            gapless_mode,
        })
    }
}

fn map_ureq_error(e: ureq::Error, track_id: i64) -> MusicError {
    match e {
        ureq::Error::StatusCode(code) => match code {
            404 => MusicError::NotFound(format!("track {track_id}")),
            416 => MusicError::BadRange,
            400 | 415 => MusicError::BadRequest(format!("server rejected stream request ({code})")),
            501 => MusicError::FeatureDisabled {
                feature: "encode-opus/encode-mp3".to_string(),
                detail: format!("server cannot produce the requested rendition ({code})"),
            },
            _ => MusicError::Http(format!("server returned HTTP {code} for track {track_id}")),
        },
        other => MusicError::Http(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;

    /// Minimal raw-HTTP stub: serves one canned response, records the
    /// request line + Range header. No external crates, fully hermetic.
    struct Stub {
        addr: std::net::SocketAddr,
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl Stub {
        fn serve_once(response: &'static str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
            let addr = listener.local_addr().expect("addr");
            let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let seen2 = seen.clone();
            thread::spawn(move || {
                let (mut sock, _) = listener.accept().expect("accept");
                let mut buf = [0u8; 4096];
                let n = std::io::Read::read(&mut sock, &mut buf).expect("read");
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                seen2.lock().unwrap().push(req);
                sock.write_all(response.as_bytes()).expect("write");
                sock.flush().expect("flush");
            });
            Self { addr, seen }
        }
    }

    const RESPONSE: &str = "HTTP/1.1 200 OK\r\nContent-Type: audio/flac\r\nX-Transcode-Chain: wav->flac\r\nX-Gapless-Next: 43\r\nX-Gapless-Mode: single-session\r\nContent-Length: 4\r\nConnection: close\r\n\r\nfLaC";

    #[test]
    fn url_building_and_header_parsing() {
        let stub = Stub::serve_once(RESPONSE);
        let t = HttpTransport::new(format!("http://{}", stub.addr));
        let info = t
            .open_stream(
                42,
                &StreamOptions {
                    format: Some(StreamFormat::Flac),
                    seek_ms: Some(1500),
                    next: Some(43),
                    range_start: None,
                },
            )
            .expect("open");
        assert_eq!(info.content_type, "audio/flac");
        assert_eq!(info.chain.as_deref(), Some("wav->flac"));
        assert_eq!(info.gapless_next, Some(43));
        assert_eq!(info.gapless_mode.as_deref(), Some("single-session"));

        let reqs = stub.seen.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        let line = reqs[0].lines().next().unwrap();
        assert!(line.starts_with("GET /stream/42?"), "request line: {line}");
        assert!(line.contains("format=flac"), "request line: {line}");
        assert!(line.contains("seek_ms=1500"), "request line: {line}");
        assert!(line.contains("next=43"), "request line: {line}");
    }

    #[test]
    fn range_header_sent_for_passthrough_seek() {
        let stub = Stub::serve_once(RESPONSE);
        let t = HttpTransport::new(format!("http://{}", stub.addr));
        let _ = t
            .open_stream(
                7,
                &StreamOptions {
                    range_start: Some(12345),
                    ..Default::default()
                },
            )
            .expect("open");
        let reqs = stub.seen.lock().unwrap();
        assert!(
            reqs[0].to_lowercase().contains("range: bytes=12345-"),
            "request:\n{}",
            reqs[0]
        );
    }

    #[test]
    fn progress_counts_bytes_read_against_content_length() {
        let stub = Stub::serve_once(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/flac\r\nContent-Length: 10\r\nConnection: close\r\n\r\n0123456789",
        );
        let t = HttpTransport::new(format!("http://{}", stub.addr));
        let mut info = t.open_stream(1, &StreamOptions::default()).expect("open");
        let p = info.progress.clone().expect("http transport reports progress");
        assert_eq!(p.content_length, Some(10));
        assert_eq!(p.fraction(), Some(0.0));
        let mut buf = [0u8; 4];
        info.reader.read_exact(&mut buf).unwrap();
        assert_eq!(p.fraction(), Some(0.4));
        let mut rest = Vec::new();
        info.reader.read_to_end(&mut rest).unwrap();
        assert_eq!(p.fraction(), Some(1.0));
    }

    #[test]
    fn progress_is_relative_to_the_whole_file_for_range_resumes() {
        let p = StreamProgress {
            received: Arc::new(AtomicU64::new(25)),
            content_length: Some(50),
            offset: 50,
        };
        assert_eq!(p.fraction(), Some(0.75)); // (50 + 25) / (50 + 50)
        let unknown = StreamProgress { content_length: None, ..p };
        assert_eq!(unknown.fraction(), None);
    }

    #[test]
    fn http_404_maps_to_not_found() {
        let stub = Stub::serve_once(
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let t = HttpTransport::new(format!("http://{}", stub.addr));
        let err = match t.open_stream(999, &StreamOptions::default()) {
            Ok(_) => panic!("expected an error"),
            Err(e) => e,
        };
        assert!(matches!(err, MusicError::NotFound(_)), "got {err:?}");
    }
}
