//! Internet radio streams (docs/v2/kahawai-radio-spec.md, D3).
//!
//! The Player connects to a station itself. This module is the part that has
//! nothing to do with the engine: opening the connection (including the old
//! Shoutcast `ICY 200 OK` reply that ordinary HTTP libraries refuse), taking
//! the track titles out of the byte stream ([`IcyReader`]), and decoding AAC
//! streams with a decoder that understands HE-AAC (AAC+), which Symphonia's
//! does not ([`LiveAac`]).

use std::collections::VecDeque;
use std::io::{Cursor, Read, Write};
use std::net::ToSocketAddrs;
use std::time::Duration;

use kahawai_core::MusicError;

use crate::decode::DecodedSpec;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// A live stream that sends nothing for this long is dead.
const READ_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_REDIRECTS: usize = 4;

// ---------------------------------------------------------------------------
// ICY metadata
// ---------------------------------------------------------------------------

/// The `StreamTitle` in one ICY metadata block (`StreamTitle='Artist - Song';
/// StreamUrl='';`), or `None` when there is none or it is empty. Stations
/// send UTF-8 or Latin-1; both are read.
pub fn parse_stream_title(block: &[u8]) -> Option<String> {
    let text = match std::str::from_utf8(block) {
        Ok(s) => s.to_string(),
        Err(_) => block.iter().map(|&b| b as char).collect(),
    };
    let text = text.trim_end_matches('\0');
    let start = text.find("StreamTitle='")? + "StreamTitle='".len();
    let rest = &text[start..];
    // The title ends at the quote that is followed by `;` (a title may hold
    // quotes of its own: `Don't Stop`), or at the end of the block.
    let end = rest
        .find("';")
        .or_else(|| rest.strip_suffix('\'').map(str::len))?;
    let title = rest[..end].trim();
    (!title.is_empty()).then(|| title.to_string())
}

/// Takes the metadata blocks out of a stream that has them, so what is read
/// is pure audio. `metaint` is the `icy-metaint` header: after that many
/// audio bytes comes a length byte (x16) and then that many bytes of
/// metadata. `on_title` is called each time the title changes.
pub struct IcyReader<R> {
    inner: R,
    metaint: usize,
    until_meta: usize,
    last: Option<String>,
    on_title: Box<dyn FnMut(String) + Send>,
}

impl<R: Read> IcyReader<R> {
    /// `metaint` 0 means the stream carries no metadata: bytes pass through.
    pub fn new(inner: R, metaint: usize, on_title: Box<dyn FnMut(String) + Send>) -> Self {
        Self {
            inner,
            metaint,
            until_meta: metaint,
            last: None,
            on_title,
        }
    }

    fn read_metadata(&mut self) -> std::io::Result<bool> {
        let mut len = [0u8; 1];
        if self.inner.read(&mut len)? == 0 {
            return Ok(false);
        }
        let n = len[0] as usize * 16;
        if n > 0 {
            let mut block = vec![0u8; n];
            self.inner.read_exact(&mut block)?;
            if let Some(title) = parse_stream_title(&block) {
                if self.last.as_deref() != Some(title.as_str()) {
                    self.last = Some(title.clone());
                    (self.on_title)(title);
                }
            }
        }
        self.until_meta = self.metaint;
        Ok(true)
    }
}

impl<R: Read> Read for IcyReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.metaint == 0 || buf.is_empty() {
            return self.inner.read(buf);
        }
        if self.until_meta == 0 && !self.read_metadata()? {
            return Ok(0);
        }
        let want = buf.len().min(self.until_meta);
        let n = self.inner.read(&mut buf[..want])?;
        self.until_meta -= n;
        Ok(n)
    }
}

// ---------------------------------------------------------------------------
// Opening a station
// ---------------------------------------------------------------------------

/// What a station said about itself, and its bytes.
pub struct Opened {
    pub content_type: String,
    pub name: Option<String>,
    pub bitrate: Option<u32>,
    pub genre: Option<String>,
    /// Bytes between metadata blocks (0 = the stream has none).
    pub metaint: usize,
    /// The audio bytes, metadata still in them when `metaint` > 0.
    pub reader: Box<dyn Read + Send>,
}

fn head_value(head: &str, key: &str) -> Option<String> {
    head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case(key)
            .then(|| v.trim().to_string())
    })
}

/// Where the response head of `buf` ends (after the blank line), if it does.
fn head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

fn status_of(head: &str) -> Option<u16> {
    let mut parts = head.lines().next()?.split_whitespace();
    let proto = parts.next()?;
    if !(proto.starts_with("HTTP/") || proto == "ICY") {
        return None;
    }
    parts.next()?.parse().ok()
}

fn io_err(what: &str, e: impl std::fmt::Display) -> MusicError {
    MusicError::Http(format!("{what}: {e}"))
}

/// Connect to `url`, asking for metadata, and follow redirects. Plain
/// `http://` goes over a socket by hand because Shoutcast v1 answers
/// `ICY 200 OK`; `https://` goes through ureq.
pub fn open_station(url: &str, user_agent: &str) -> Result<Opened, MusicError> {
    let mut target = url.trim().to_string();
    for _ in 0..=MAX_REDIRECTS {
        let (status, head, reader) = if target.starts_with("https://") {
            open_https(&target, user_agent)?
        } else if target.starts_with("http://") {
            open_http(&target, user_agent)?
        } else {
            return Err(MusicError::BadRequest(
                "a station address starts with http:// or https://".into(),
            ));
        };
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let loc = head_value(&head, "location")
                .ok_or_else(|| MusicError::Http("the station redirects nowhere".into()))?;
            target = resolve_location(&target, &loc);
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(MusicError::Http(format!(
                "the station answered HTTP {status}"
            )));
        }
        let content_type = head_value(&head, "content-type").unwrap_or_default();
        let lower = content_type.to_ascii_lowercase();
        if lower.contains("mpegurl") || lower.contains("x-scpls") || lower.contains("text/html") {
            return Err(MusicError::BadRequest(
                "that address is a playlist or a web page, not a stream".into(),
            ));
        }
        return Ok(Opened {
            content_type,
            name: head_value(&head, "icy-name").filter(|s| !s.is_empty()),
            bitrate: head_value(&head, "icy-br")
                .and_then(|b| b.split(',').next().and_then(|n| n.trim().parse().ok()))
                .filter(|b| *b > 0),
            genre: head_value(&head, "icy-genre").filter(|s| !s.is_empty()),
            metaint: head_value(&head, "icy-metaint")
                .and_then(|m| m.parse().ok())
                .unwrap_or(0),
            reader,
        });
    }
    Err(MusicError::Http("too many redirects".into()))
}

fn resolve_location(base: &str, loc: &str) -> String {
    if loc.starts_with("http://") || loc.starts_with("https://") {
        return loc.to_string();
    }
    // Relative: keep the scheme and host of `base`.
    let (scheme_host, _) = match base.find("://") {
        Some(i) => {
            let after = i + 3;
            let end = base[after..]
                .find('/')
                .map(|p| after + p)
                .unwrap_or(base.len());
            (&base[..end], &base[end..])
        }
        None => (base, ""),
    };
    if loc.starts_with('/') {
        format!("{scheme_host}{loc}")
    } else {
        format!("{scheme_host}/{loc}")
    }
}

type Response = (u16, String, Box<dyn Read + Send>);

fn open_http(url: &str, user_agent: &str) -> Result<Response, MusicError> {
    let rest = &url["http://".len()..];
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (
            h,
            p.parse::<u16>()
                .map_err(|_| io_err("address", "bad port"))?,
        ),
        _ => (authority, 80),
    };
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| io_err("could not find the station", e))?
        .collect();
    let mut last = None;
    let mut stream = None;
    for a in addrs {
        match std::net::TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let mut stream = stream.ok_or_else(|| {
        io_err(
            "could not connect to the station",
            last.map(|e| e.to_string())
                .unwrap_or_else(|| "no address".into()),
        )
    })?;
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|e| io_err("socket", e))?;
    let _ = stream.set_nodelay(true);
    let req = format!(
        "GET {path} HTTP/1.0\r\nHost: {authority}\r\nUser-Agent: {user_agent}\r\n\
         Icy-MetaData: 1\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| io_err("could not ask the station", e))?;
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    let end = loop {
        if let Some(e) = head_end(&buf) {
            break e;
        }
        if buf.len() > 16 * 1024 {
            return Err(MusicError::Http(
                "the station's reply is not a stream".into(),
            ));
        }
        let n = stream
            .read(&mut chunk)
            .map_err(|e| io_err("the station did not answer", e))?;
        if n == 0 {
            return Err(MusicError::Http("the station hung up".into()));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let status = status_of(&head).ok_or_else(|| MusicError::Http("that is not a stream".into()))?;
    let leftover = buf[end..].to_vec();
    Ok((status, head, Box::new(Cursor::new(leftover).chain(stream))))
}

fn open_https(url: &str, user_agent: &str) -> Result<Response, MusicError> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_recv_response(Some(Duration::from_secs(10)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build(),
    );
    let resp = agent
        .get(url)
        .header("Icy-MetaData", "1")
        .header("User-Agent", user_agent)
        .call()
        .map_err(|e| io_err("could not connect to the station", e))?;
    let status = resp.status().as_u16();
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for (k, v) in resp.headers() {
        if let Ok(v) = v.to_str() {
            head.push_str(&format!("{}: {v}\r\n", k.as_str()));
        }
    }
    head.push_str("\r\n");
    Ok((status, head, Box::new(resp.into_body().into_reader())))
}

// ---------------------------------------------------------------------------
// AAC (including HE-AAC) from a live stream
// ---------------------------------------------------------------------------

/// Does this look like the start of an ADTS or LOAS/LATM AAC stream?
pub fn looks_like_aac(head: &[u8]) -> bool {
    head.len() >= 2
        && ((head[0] == 0xFF && head[1] & 0xF6 == 0xF0) // ADTS, layer 00
            || (head[0] == 0x56 && head[1] & 0xE0 == 0xE0)) // LOAS
}

/// Index of the next ADTS or LOAS sync word in `buf`, at or after `from`.
fn next_sync(buf: &[u8], from: usize) -> Option<usize> {
    (from..buf.len().saturating_sub(1)).find(|&i| looks_like_aac(&buf[i..i + 2]))
}

/// Decodes an AAC stream of any profile (LC, HE-AAC v1/v2) into interleaved
/// `f32`. A live stream is joined mid-frame, so it starts at the first sync
/// word and, after damaged data, finds the next one.
pub struct LiveAac {
    src: Box<dyn Read + Send>,
    dec: syom::Decoder,
    spec: DecodedSpec,
    out: VecDeque<f32>,
    pending: Vec<u8>,
    errors_in_a_row: u32,
    /// The decoder has been given the start of a frame and is following the
    /// stream; false until the first sync word, and again after damage.
    synced: bool,
    eof: bool,
}

/// After this many failures with no frame between them the stream is not AAC.
const MAX_BAD_READS: u32 = 400;

impl LiveAac {
    /// Opens the stream and decodes until the first frame, so the format is
    /// known. Fails if no frame can be found in the first part of the stream.
    pub fn new(src: Box<dyn Read + Send>) -> Result<Self, MusicError> {
        let mut this = Self {
            src,
            dec: syom::Decoder::new(syom::DecodeOptions::unbounded()),
            spec: DecodedSpec {
                sample_rate: 0,
                channels: 0,
            },
            out: VecDeque::new(),
            pending: Vec::new(),
            errors_in_a_row: 0,
            synced: false,
            eof: false,
        };
        while this.spec.sample_rate == 0 {
            if !this.step()? {
                return Err(MusicError::Metadata(
                    "no AAC audio found in the stream".into(),
                ));
            }
        }
        Ok(this)
    }

    pub fn spec(&self) -> DecodedSpec {
        self.spec
    }

    /// Reads and decodes one chunk of the stream. `false` at the end of it.
    fn step(&mut self) -> Result<bool, MusicError> {
        if self.eof {
            return Ok(false);
        }
        let mut chunk = [0u8; 4096];
        let n = self.src.read(&mut chunk).map_err(MusicError::Io)?;
        if n == 0 {
            self.eof = true;
            return Ok(false);
        }
        if self.synced {
            // Following the stream: the decoder keeps partial frames itself.
            let data = chunk[..n].to_vec();
            self.feed(&data)?;
            return Ok(true);
        }
        self.pending.extend_from_slice(&chunk[..n]);
        // Not following yet: start at the first sync word.
        let Some(start) = next_sync(&self.pending, 0) else {
            if self.pending.len() > 64 * 1024 {
                return Err(MusicError::Metadata(
                    "no AAC audio found in the stream".into(),
                ));
            }
            // Keep a byte: the sync word may straddle two reads.
            let keep = self.pending.len().saturating_sub(1);
            self.pending.drain(..keep);
            return Ok(true);
        };
        let data: Vec<u8> = self.pending.drain(start..).collect();
        self.pending.clear();
        self.synced = true;
        self.feed(&data)?;
        Ok(true)
    }

    fn feed(&mut self, data: &[u8]) -> Result<(), MusicError> {
        let spec = &mut self.spec;
        let out = &mut self.out;
        let mut frames = 0u32;
        let res = self.dec.feed(data, |f| {
            frames += 1;
            let ch = f.planar.len();
            if spec.sample_rate == 0 {
                spec.sample_rate = f.sample_rate;
                spec.channels = ch as u16;
            }
            // Rate or channel changes mid-stream are not followed; frames
            // of the wrong shape are dropped rather than played wrong.
            if f.sample_rate == spec.sample_rate && ch == spec.channels as usize {
                for i in 0..f.samples {
                    for plane in f.planar {
                        out.push_back(plane[i]);
                    }
                }
            }
            Ok(())
        });
        if frames > 0 {
            self.errors_in_a_row = 0;
        }
        if res.is_err() {
            // The decoder gives up on a damaged stream: start over from the
            // next sync word.
            self.dec.reset();
            self.synced = false;
            self.errors_in_a_row += 1;
            if self.errors_in_a_row > MAX_BAD_READS {
                return Err(MusicError::Metadata("the AAC stream is unreadable".into()));
            }
        }
        Ok(())
    }

    /// Up to `out.len()` interleaved samples; returns the number of frames
    /// (0 = the stream ended).
    pub fn decode_interleaved(&mut self, out: &mut [f32]) -> Result<usize, MusicError> {
        let ch = self.spec.channels.max(1) as usize;
        let want = out.len() - out.len() % ch;
        while self.out.len() < ch && self.step()? {}
        let n = self.out.len().min(want);
        let n = n - n % ch;
        for (slot, v) in out.iter_mut().zip(self.out.drain(..n)) {
            *slot = v;
        }
        Ok(n / ch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn titles_are_read_from_a_block() {
        assert_eq!(
            parse_stream_title(b"StreamTitle='Daft Punk - One More Time';StreamUrl='';\0\0\0"),
            Some("Daft Punk - One More Time".into())
        );
        assert_eq!(
            parse_stream_title(b"StreamTitle='Don't Stop Me Now';"),
            Some("Don't Stop Me Now".into()),
            "a quote inside a title"
        );
        assert_eq!(parse_stream_title(b"StreamTitle='';"), None, "empty title");
        assert_eq!(parse_stream_title(b"StreamUrl='x';"), None);
        assert_eq!(parse_stream_title(b""), None);
        // Latin-1 bytes (not valid UTF-8).
        assert_eq!(
            parse_stream_title(b"StreamTitle='Caf\xe9 del Mar';"),
            Some("Café del Mar".into())
        );
        assert_eq!(
            parse_stream_title(b"StreamTitle='Unterminated"),
            None,
            "a block cut short is ignored"
        );
    }

    /// A stream with a metadata block every `metaint` bytes.
    fn icy_stream(audio: &[u8], metaint: usize, titles: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, chunk) in audio.chunks(metaint).enumerate() {
            out.extend_from_slice(chunk);
            if chunk.len() < metaint {
                break;
            }
            let title = titles.get(i).copied().unwrap_or("");
            if title.is_empty() {
                out.push(0);
            } else {
                let mut b = format!("StreamTitle='{title}';").into_bytes();
                let blocks = b.len().div_ceil(16);
                b.resize(blocks * 16, 0);
                out.push(blocks as u8);
                out.extend_from_slice(&b);
            }
        }
        out
    }

    #[test]
    fn the_audio_comes_out_clean_and_each_new_title_is_reported_once() {
        let audio: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let stream = icy_stream(&audio, 1000, &["A - One", "A - One", "B - Two", "", ""]);
        let heard = Arc::new(Mutex::new(Vec::new()));
        let h = heard.clone();
        let mut r = IcyReader::new(
            Cursor::new(stream),
            1000,
            Box::new(move |t| h.lock().unwrap().push(t)),
        );
        let mut got = Vec::new();
        // Awkward read sizes, across the metadata boundaries.
        let mut buf = [0u8; 333];
        loop {
            let n = r.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        assert_eq!(got, audio, "no metadata bytes leak into the audio");
        assert_eq!(
            *heard.lock().unwrap(),
            ["A - One", "B - Two"],
            "repeats are not reported"
        );
    }

    #[test]
    fn a_stream_without_metadata_passes_straight_through() {
        let audio: Vec<u8> = (0..3000u32).map(|i| i as u8).collect();
        let mut r = IcyReader::new(Cursor::new(audio.clone()), 0, Box::new(|_| {}));
        let mut got = Vec::new();
        r.read_to_end(&mut got).unwrap();
        assert_eq!(got, audio);
    }

    /// A stream that ends in the middle of a metadata block just ends.
    #[test]
    fn a_stream_cut_inside_metadata_ends_cleanly_or_errors() {
        let mut stream = vec![7u8; 100];
        stream.push(2); // 32 bytes of metadata promised
        stream.extend_from_slice(b"StreamTitle='cut");
        let mut r = IcyReader::new(Cursor::new(stream), 100, Box::new(|_| {}));
        let mut got = Vec::new();
        let res = r.read_to_end(&mut got);
        assert!(
            res.is_err(),
            "unexpected end of stream is an error, not garbage audio"
        );
        assert_eq!(got.len(), 100);
    }

    #[test]
    fn aac_sync_words_are_found() {
        assert!(looks_like_aac(&[0xFF, 0xF1]));
        assert!(looks_like_aac(&[0xFF, 0xF9]));
        assert!(looks_like_aac(&[0x56, 0xE1]));
        assert!(!looks_like_aac(&[0xFF, 0xFB]), "that is MP3");
        assert!(!looks_like_aac(b"ID"));
        assert_eq!(next_sync(&[1, 2, 0xFF, 0xF1, 5], 0), Some(2));
        assert_eq!(next_sync(&[1, 2, 3], 0), None);
    }

    #[test]
    fn redirects_may_be_relative() {
        assert_eq!(
            resolve_location("http://h:8000/a/b", "/live"),
            "http://h:8000/live"
        );
        assert_eq!(resolve_location("http://h/a", "https://x/y"), "https://x/y");
        assert_eq!(resolve_location("http://h", "live"), "http://h/live");
    }

    #[test]
    fn response_heads_are_understood() {
        let h = "ICY 200 OK\r\nicy-name: X FM\r\nICY-BR: 128\r\n\r\n";
        assert_eq!(status_of(h), Some(200));
        assert_eq!(head_value(h, "icy-br").as_deref(), Some("128"));
        assert_eq!(head_value(h, "missing"), None);
        assert_eq!(status_of("<html>"), None);
        assert_eq!(head_end(b"a\r\n\r\nrest"), Some(5));
    }

    fn tone(n: usize, sr: u32) -> (Vec<f32>, Vec<f32>) {
        let f = |hz: f32, i: usize| {
            0.3 * (2.0 * std::f32::consts::PI * hz * i as f32 / sr as f32).sin()
        };
        (
            (0..n).map(|i| f(440.0, i)).collect(),
            (0..n).map(|i| f(660.0, i)).collect(),
        )
    }

    fn dominant(samples: &[f32], sr: u32) -> f32 {
        // Zero crossings are enough to tell 440 from 660 Hz.
        let crossings = samples
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        crossings as f32 / (samples.len() as f32 / sr as f32)
    }

    fn encode(he: bool) -> Vec<u8> {
        let (l, r) = tone(48_000 * 3, 48_000);
        let opts = syom::EncodeOptions::adts()
            .with_he(he)
            .with_bitrate_bps(if he { 64_000 } else { 128_000 });
        syom::encode_with(&[&l[..], &r[..]], 48_000, &opts).expect("encode")
    }

    fn decode_all(bytes: Vec<u8>) -> (DecodedSpec, Vec<f32>) {
        let mut d = LiveAac::new(Box::new(Cursor::new(bytes))).expect("open");
        let spec = d.spec();
        let mut all = Vec::new();
        let mut buf = vec![0f32; 4096];
        loop {
            let n = d.decode_interleaved(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            all.extend_from_slice(&buf[..n * spec.channels as usize]);
        }
        (spec, all)
    }

    #[test]
    fn he_aac_decodes_at_the_full_rate() {
        let (spec, pcm) = decode_all(encode(true));
        assert_eq!(
            (spec.sample_rate, spec.channels),
            (48_000, 2),
            "SBR doubles the core rate"
        );
        assert!(pcm.len() > 48_000 * 2 * 2, "{} samples", pcm.len());
        let left: Vec<f32> = pcm
            .iter()
            .step_by(2)
            .copied()
            .skip(20_000)
            .take(48_000)
            .collect();
        let right: Vec<f32> = pcm
            .iter()
            .skip(1)
            .step_by(2)
            .copied()
            .skip(20_000)
            .take(48_000)
            .collect();
        assert!((dominant(&left, 48_000) - 440.0).abs() < 5.0);
        assert!((dominant(&right, 48_000) - 660.0).abs() < 5.0);
    }

    #[test]
    fn plain_aac_lc_decodes_too() {
        let (spec, pcm) = decode_all(encode(false));
        assert_eq!((spec.sample_rate, spec.channels), (48_000, 2));
        let left: Vec<f32> = pcm
            .iter()
            .step_by(2)
            .copied()
            .skip(20_000)
            .take(48_000)
            .collect();
        assert!((dominant(&left, 48_000) - 440.0).abs() < 5.0);
    }

    #[test]
    fn joining_a_stream_mid_frame_finds_the_next_frame() {
        let bytes = encode(true);
        // Slip in at an arbitrary byte, as a radio client does.
        let (spec, pcm) = decode_all(bytes[777..].to_vec());
        assert_eq!(spec.sample_rate, 48_000);
        assert!(pcm.len() > 48_000 * 2 * 2);
    }

    #[test]
    fn damage_in_the_middle_is_skipped() {
        let mut bytes = encode(true);
        for b in &mut bytes[12_000..12_400] {
            *b = 0x55;
        }
        let (_, pcm) = decode_all(bytes);
        assert!(pcm.len() > 48_000 * 2 * 2, "{} samples", pcm.len());
    }

    #[test]
    fn something_that_is_not_aac_is_refused() {
        let junk = vec![0x41u8; 200_000];
        assert!(LiveAac::new(Box::new(Cursor::new(junk))).is_err());
        assert!(LiveAac::new(Box::new(Cursor::new(Vec::new()))).is_err());
    }
}
