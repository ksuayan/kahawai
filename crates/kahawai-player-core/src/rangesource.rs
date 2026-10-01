//! A seekable, Range-backed view of a file on the server.
//!
//! Passthrough streams are plain files, and a demuxer can find any moment in
//! them quickly if it can *seek* (FLAC seek table, MP4 index, WAV arithmetic,
//! Ogg bisection). The ordinary stream is a forward-only response body, so a
//! seek used to mean "download from byte 0 and throw frames away until the
//! target". [`RangeSource`] gives the demuxer a `Read + Seek` source instead:
//!
//! - The first [`HEAD_BYTES`] are fetched up front and cached, which is what
//!   probing the container needs.
//! - Reading elsewhere opens **one** streaming `Range: bytes=N-` request at that
//!   offset, wrapped in the [`ReadAhead`] buffer, so after a seek playback is
//!   again a single sequential download (not a request per block).
//! - A short hop forward is read through; anything else re-opens at the new
//!   offset, dropping (and so cancelling) the old request.
//!
//! If the server does not honour Range the source is simply not offered
//! (`Ok(None)`) and the caller keeps the forward-only path.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use kahawai_core::MusicError;

use crate::readahead::ReadAhead;
use crate::transport::{map_ureq_error, CountingReader, StreamProgress};

/// What the container probe needs from the start of the file.
const HEAD_BYTES: u64 = 256 * 1024;

/// A forward hop up to this far is read through rather than re-requested.
const SKIP_FORWARD_MAX: u64 = 64 * 1024;

/// `Read + Seek + Send`: what the decoder needs from a seekable source.
pub trait SeekableRead: Read + Seek + Send {}
impl<T: Read + Seek + Send> SeekableRead for T {}

/// Side channel to a seekable source that has been handed to the decoder.
pub trait SeekableControl: Send + Sync {
    /// Make sure the sequential download is under way at the current position
    /// (it is opened lazily otherwise), so the first bytes after a seek are
    /// already on their way.
    fn warm(&self);
    /// Progress of the current sequential download, once there is one.
    fn progress(&self) -> Option<StreamProgress>;
}

/// An opened seekable stream and its response metadata.
pub struct SeekableStream {
    pub source: Box<dyn SeekableRead>,
    pub byte_len: u64,
    pub content_type: String,
    pub chain: Option<String>,
    pub control: Option<Arc<dyn SeekableControl>>,
}

struct Live {
    /// Absolute offset of the next byte this download will produce.
    pos: u64,
    reader: Box<dyn Read + Send>,
    progress: StreamProgress,
}

struct Inner {
    agent: ureq::Agent,
    url: String,
    total: u64,
    pos: u64,
    head: Vec<u8>,
    live: Option<Live>,
    read_ahead_bytes: usize,
}

/// The `Read + Seek` handle given to the demuxer.
pub struct RangeSource(Arc<Mutex<Inner>>);

/// The side channel for the same source.
struct RangeControl(Arc<Mutex<Inner>>);

fn io_err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

/// `Content-Range: bytes 100-199/1234` -> (100, 1234).
fn parse_content_range(v: &str) -> Option<(u64, u64)> {
    let rest = v.trim().strip_prefix("bytes")?.trim();
    let (range, total) = rest.split_once('/')?;
    let (start, _) = range.split_once('-')?;
    Some((start.trim().parse().ok()?, total.trim().parse().ok()?))
}

impl RangeSource {
    /// Open `url` for ranged access. `Ok(None)` when the server will not do
    /// Range (answers 200 to a ranged request), so the caller can fall back.
    pub fn open(
        agent: &ureq::Agent,
        url: String,
        read_ahead_bytes: usize,
        track_id: i64,
    ) -> Result<Option<SeekableStream>, MusicError> {
        let resp = agent
            .get(&url)
            .header("Range", &format!("bytes=0-{}", HEAD_BYTES - 1))
            .call()
            .map_err(|e| map_ureq_error(e, track_id))?;
        if resp.status().as_u16() != 206 {
            return Ok(None); // the server ignored Range: not seekable
        }
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        };
        let Some((0, total)) = header("content-range")
            .as_deref()
            .and_then(parse_content_range)
        else {
            return Ok(None);
        };
        let content_type = header("content-type").unwrap_or_default();
        let chain = header("x-transcode-chain");
        let mut head = Vec::new();
        resp.into_body()
            .into_reader()
            .take(HEAD_BYTES)
            .read_to_end(&mut head)
            .map_err(MusicError::Io)?;
        let inner = Arc::new(Mutex::new(Inner {
            agent: agent.clone(),
            url,
            total,
            pos: 0,
            head,
            live: None,
            read_ahead_bytes,
        }));
        Ok(Some(SeekableStream {
            source: Box::new(RangeSource(inner.clone())),
            byte_len: total,
            content_type,
            chain,
            control: Some(Arc::new(RangeControl(inner))),
        }))
    }
}

impl Inner {
    /// Start a sequential download at `pos`.
    fn open_at(&self, pos: u64) -> io::Result<Live> {
        let resp = self
            .agent
            .get(&self.url)
            .header("Range", &format!("bytes={pos}-"))
            .call()
            .map_err(io_err)?;
        let start = resp
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range)
            .map(|(s, _)| s);
        if resp.status().as_u16() != 206 || start != Some(pos) {
            return Err(io_err("server did not honour the byte range"));
        }
        let received = Arc::new(AtomicU64::new(0));
        let counting = CountingReader::new(resp.into_body().into_reader(), received.clone());
        let (reader, stats): (Box<dyn Read + Send>, _) = if self.read_ahead_bytes > 0 {
            let ahead = ReadAhead::new(counting, self.read_ahead_bytes);
            let stats = ahead.stats();
            (Box::new(ahead), Some(stats))
        } else {
            (Box::new(counting), None)
        };
        Ok(Live {
            pos,
            reader,
            progress: StreamProgress {
                received,
                content_length: Some(self.total - pos),
                offset: pos,
                stats,
            },
        })
    }

    /// Get the sequential download to sit exactly at `self.pos`.
    fn align(&mut self) -> io::Result<()> {
        if let Some(live) = self.live.as_mut() {
            if live.pos == self.pos {
                return Ok(());
            }
            if live.pos < self.pos && self.pos - live.pos <= SKIP_FORWARD_MAX {
                let mut sink = [0u8; 8192];
                while live.pos < self.pos {
                    let want = ((self.pos - live.pos) as usize).min(sink.len());
                    let n = live.reader.read(&mut sink[..want])?;
                    if n == 0 {
                        break;
                    }
                    live.pos += n as u64;
                }
                if live.pos == self.pos {
                    return Ok(());
                }
            }
        }
        self.live = None; // dropping the old download cancels its read-ahead
        self.live = Some(self.open_at(self.pos)?);
        Ok(())
    }
}

impl Read for RangeSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut inner = self.0.lock().expect("range source lock");
        if buf.is_empty() || inner.pos >= inner.total {
            return Ok(0);
        }
        let head_len = inner.head.len() as u64;
        if inner.pos < head_len {
            let start = inner.pos as usize;
            let n = buf.len().min(inner.head.len() - start);
            buf[..n].copy_from_slice(&inner.head[start..start + n]);
            inner.pos += n as u64;
            return Ok(n);
        }
        inner.align()?;
        let live = inner.live.as_mut().expect("aligned");
        let n = live.reader.read(buf)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the stream ended before the file did",
            ));
        }
        live.pos += n as u64;
        inner.pos += n as u64;
        Ok(n)
    }
}

impl Seek for RangeSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let mut inner = self.0.lock().expect("range source lock");
        let target = match from {
            SeekFrom::Start(p) => i128::from(p),
            SeekFrom::Current(d) => i128::from(inner.pos) + i128::from(d),
            SeekFrom::End(d) => i128::from(inner.total) + i128::from(d),
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before the start of the file",
            ));
        }
        inner.pos = target as u64; // lazy: no request until something is read
        Ok(inner.pos)
    }
}

impl SeekableControl for RangeControl {
    fn warm(&self) {
        let mut inner = self.0.lock().expect("range source lock");
        if inner.live.is_some() {
            return;
        }
        let at = inner.pos.max(inner.head.len() as u64);
        if at < inner.total {
            match inner.open_at(at) {
                Ok(live) => inner.live = Some(live),
                Err(e) => tracing::warn!(error = %e, "could not prefetch after the seek"),
            }
        }
    }

    fn progress(&self) -> Option<StreamProgress> {
        self.0
            .lock()
            .expect("range source lock")
            .live
            .as_ref()
            .map(|l| l.progress.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    /// Start offset and bytes sent, per request.
    type Requests = Arc<Mutex<Vec<(u64, Arc<AtomicUsize>)>>>;

    /// A tiny HTTP server for one file that honours `Range`, recording each
    /// request's start offset and the bytes it managed to send.
    struct RangeServer {
        addr: std::net::SocketAddr,
        requests: Requests,
    }

    impl RangeServer {
        fn start(body: Vec<u8>, honour_range: bool) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let addr = listener.local_addr().unwrap();
            let requests: Requests = Arc::default();
            let (reqs, body) = (requests.clone(), Arc::new(body));
            thread::spawn(move || {
                for conn in listener.incoming() {
                    let Ok(sock) = conn else { break };
                    let (reqs, body) = (reqs.clone(), body.clone());
                    thread::spawn(move || serve(sock, &body, honour_range, &reqs));
                }
            });
            Self { addr, requests }
        }

        fn url(&self) -> String {
            format!("http://{}/stream/1", self.addr)
        }

        /// Starting offsets of every request so far.
        fn starts(&self) -> Vec<u64> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|(s, _)| *s)
                .collect()
        }

        /// Total body bytes sent across all requests.
        fn bytes_sent(&self) -> usize {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|(_, n)| n.load(Ordering::SeqCst))
                .sum()
        }
    }

    fn serve(
        mut sock: TcpStream,
        body: &[u8],
        honour_range: bool,
        reqs: &Mutex<Vec<(u64, Arc<AtomicUsize>)>>,
    ) {
        let mut buf = [0u8; 4096];
        let n = sock.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]).to_lowercase();
        let start: Option<u64> = req.lines().find_map(|l| {
            let v = l.strip_prefix("range: bytes=")?;
            v.split('-').next()?.trim().parse().ok()
        });
        let end: Option<u64> = req.lines().find_map(|l| {
            let v = l.strip_prefix("range: bytes=")?;
            v.split('-').nth(1)?.trim().parse().ok()
        });
        let sent = Arc::new(AtomicUsize::new(0));
        let total = body.len() as u64;
        let (status, from, to) = match (honour_range, start) {
            (true, Some(s)) if s < total => (
                "206 Partial Content",
                s,
                end.unwrap_or(total - 1).min(total - 1),
            ),
            _ => ("200 OK", 0, total - 1),
        };
        reqs.lock().unwrap().push((from, sent.clone()));
        let slice = &body[from as usize..=to as usize];
        let mut head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: audio/flac\r\nContent-Length: {}\r\nConnection: close\r\n",
            slice.len()
        );
        if status.starts_with("206") {
            head.push_str(&format!("Content-Range: bytes {from}-{to}/{total}\r\n"));
        }
        head.push_str("\r\n");
        if sock.write_all(head.as_bytes()).is_err() {
            return;
        }
        for chunk in slice.chunks(16 * 1024) {
            if sock.write_all(chunk).is_err() {
                return; // the client went away
            }
            sent.fetch_add(chunk.len(), Ordering::SeqCst);
            thread::sleep(Duration::from_millis(1)); // pace it so a cancel lands mid-body
        }
    }

    fn eventually(f: impl Fn() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            if f() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        f()
    }

    fn file(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i.wrapping_mul(131) % 251) as u8).collect()
    }

    fn open(server: &RangeServer, read_ahead: usize) -> Option<SeekableStream> {
        RangeSource::open(
            &ureq::Agent::new_with_defaults(),
            server.url(),
            read_ahead,
            1,
        )
        .expect("open")
    }

    #[test]
    fn parses_content_range() {
        assert_eq!(parse_content_range("bytes 0-99/1000"), Some((0, 1000)));
        assert_eq!(parse_content_range("bytes 500-999/1000"), Some((500, 1000)));
        assert_eq!(parse_content_range("bytes */1000"), None);
        assert_eq!(parse_content_range("garbage"), None);
    }

    #[test]
    fn a_server_that_ignores_range_is_not_offered_as_seekable() {
        let server = RangeServer::start(file(1_000_000), false);
        assert!(open(&server, 0).is_none());
    }

    #[test]
    fn reads_the_whole_file_in_order_across_the_cached_head_and_the_stream() {
        let body = file(900_000); // longer than the cached head
        let server = RangeServer::start(body.clone(), true);
        let mut sk = open(&server, 1 << 20).expect("seekable");
        assert_eq!(sk.byte_len, 900_000);
        let mut got = Vec::new();
        sk.source.read_to_end(&mut got).unwrap();
        assert_eq!(got, body);
        assert_eq!(
            server.starts(),
            vec![0, HEAD_BYTES],
            "one head request, one streaming request"
        );
    }

    #[test]
    fn seeking_far_ahead_downloads_from_there_not_from_the_start() {
        let body = file(4_000_000);
        let server = RangeServer::start(body.clone(), true);
        let mut sk = open(&server, 8 << 20).expect("seekable");
        sk.source.seek(SeekFrom::Start(3_500_000)).unwrap();
        let mut got = vec![0u8; 100_000];
        sk.source.read_exact(&mut got).unwrap();
        assert_eq!(got, body[3_500_000..3_600_000]);
        assert_eq!(
            server.starts(),
            vec![0, 3_500_000],
            "no request for the bytes in between"
        );
        assert!(
            eventually(|| server.bytes_sent() >= 256 * 1024 + 500_000),
            "it streams the rest from the target"
        );
        drop(sk);
        thread::sleep(Duration::from_millis(100));
        assert!(
            server.bytes_sent() < 256 * 1024 + 500_000 + 200_000,
            "only the tail after the target was fetched, not the 3.5 MB before it: {}",
            server.bytes_sent()
        );
    }

    #[test]
    fn a_short_forward_hop_reads_through_instead_of_reconnecting() {
        let body = file(2_000_000);
        let server = RangeServer::start(body.clone(), true);
        let mut sk = open(&server, 4 << 20).expect("seekable");
        sk.source.seek(SeekFrom::Start(1_000_000)).unwrap();
        let mut a = [0u8; 10];
        sk.source.read_exact(&mut a).unwrap();
        sk.source.seek(SeekFrom::Current(20_000)).unwrap(); // a small hop forward
        sk.source.read_exact(&mut a).unwrap();
        assert_eq!(a, body[1_020_010..1_020_020]);
        assert_eq!(
            server.starts(),
            vec![0, 1_000_000],
            "still the one streaming request"
        );
    }

    #[test]
    fn seeking_backwards_or_far_forward_reopens_and_cancels_the_old_download() {
        let body = file(3_000_000);
        let server = RangeServer::start(body.clone(), true);
        let mut sk = open(&server, 4 << 20).expect("seekable");
        sk.source.seek(SeekFrom::Start(2_000_000)).unwrap();
        let mut a = [0u8; 10];
        sk.source.read_exact(&mut a).unwrap();
        sk.source.seek(SeekFrom::Start(1_000_000)).unwrap(); // backwards
        sk.source.read_exact(&mut a).unwrap();
        assert_eq!(a, body[1_000_000..1_000_010]);
        assert_eq!(server.starts(), vec![0, 2_000_000, 1_000_000]);
    }

    #[test]
    fn seek_from_the_end_and_current_work_and_negative_is_an_error() {
        let server = RangeServer::start(file(1_000_000), true);
        let mut sk = open(&server, 0).expect("seekable");
        assert_eq!(sk.source.seek(SeekFrom::End(-100)).unwrap(), 999_900);
        assert_eq!(sk.source.seek(SeekFrom::Current(50)).unwrap(), 999_950);
        assert!(sk.source.seek(SeekFrom::Current(-2_000_000)).is_err());
        let mut rest = Vec::new();
        sk.source.seek(SeekFrom::End(-100)).unwrap();
        sk.source.read_to_end(&mut rest).unwrap();
        assert_eq!(rest.len(), 100);
    }

    #[test]
    fn warm_opens_the_download_and_reports_progress_and_stats() {
        let server = RangeServer::start(file(2_000_000), true);
        let sk = open(&server, 4 << 20).expect("seekable");
        let control = sk.control.clone().expect("control");
        assert!(control.progress().is_none(), "nothing sequential yet");
        control.warm();
        let p = control.progress().expect("a download is under way");
        assert_eq!(p.offset, HEAD_BYTES, "continuing just past the cached head");
        assert_eq!(p.content_length, Some(2_000_000 - HEAD_BYTES));
        assert!(p.stats.is_some(), "with read-ahead stats");
        assert!(eventually(|| p.received.load(Ordering::Relaxed) > 0));
    }

    /// A mono 16-bit 8 kHz WAV of `secs` seconds of 440 Hz.
    fn wav(secs: u32) -> Vec<u8> {
        let rate = 8000u32;
        let data: Vec<u8> = (0..rate * secs)
            .flat_map(|i| {
                let v = (0.5
                    * (2.0 * std::f64::consts::PI * 440.0 * f64::from(i) / f64::from(rate)).sin()
                    * 32767.0) as i16;
                v.to_le_bytes()
            })
            .collect();
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * 2).to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(data.len() as u32).to_le_bytes());
        w.extend_from_slice(&data);
        w
    }

    #[test]
    fn end_to_end_a_seek_over_http_fetches_the_tail_and_decodes_the_right_audio() {
        use crate::decode::StreamDecoder;
        use crate::transport::{HttpTransport, StreamOptions, Transport};
        // 60 s of 8 kHz mono: ~960 KB, well past the cached head.
        let body = wav(60);
        let server = RangeServer::start(body.clone(), true);
        let transport = HttpTransport::new(format!("http://{}", server.addr));
        let sk = transport
            .open_seekable(1, &StreamOptions::default())
            .expect("open")
            .expect("the server does Range, so a seekable source is offered");
        let control = sk.control.clone().expect("control");
        let (mut dec, skip) =
            StreamDecoder::new_seekable(sk.source, sk.byte_len, 50_000).expect("seek to 50 s");
        control.warm();
        let mut pcm = vec![0.0f32; skip as usize + 4000];
        let mut got = 0;
        while got < pcm.len() {
            let n = dec.decode_interleaved(&mut pcm[got..]).unwrap();
            assert!(n > 0, "audio after the seek");
            got += n;
        }
        // Frame k after the target is sample (50 s * rate + k) of the 440 Hz tone.
        for k in 1000..2000usize {
            let t = 50.0 + k as f64 / 8000.0;
            let want = (0.5 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as f32;
            assert!((pcm[skip as usize + k] - want).abs() < 2e-3, "frame {k}");
        }
        // The first request is the head; the second starts near the 50 s byte
        // offset (44-byte header + 50 s * 16 000 B/s), never in between.
        let starts = server.starts();
        assert_eq!(starts[0], 0);
        assert!(
            starts.len() <= 3,
            "head plus the seek's download: {starts:?}"
        );
        assert!(
            starts[1..].iter().all(|&s| s > 700_000),
            "the bytes before the target were never requested: {starts:?}"
        );
        assert!(progress_is_absolute(&control, body.len() as u64));
        drop(dec);
        thread::sleep(Duration::from_millis(100));
        assert!(
            server.bytes_sent() < body.len() / 2,
            "far less than the whole file crossed the wire: {} of {}",
            server.bytes_sent(),
            body.len()
        );
    }

    /// Progress describes the download that is under way as a share of the
    /// whole file: it starts at a non-zero offset and ends at the file's end.
    fn progress_is_absolute(control: &Arc<dyn SeekableControl>, total: u64) -> bool {
        control
            .progress()
            .map(|p| p.offset > 0 && p.offset + p.content_length.unwrap_or(0) == total)
            .unwrap_or(false)
    }
}
