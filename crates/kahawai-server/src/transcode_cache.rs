//! Seekable transcodes: a disk cache of fully rendered single-track
//! transcodes. (docs/v2/kahawai-server-vlc-client-spec.md, D3.)
//!
//! A live transcode is chunked — no Content-Length, no byte ranges — so a
//! player that seeks with HTTP Range (VLC) can neither seek it nor show its
//! duration. A request for one track with no `?next=` and no `?seek_ms=` is
//! instead rendered once to `<data>/transcode-cache/` and, once complete,
//! served like a file. `?next=` chains and `?seek_ms=` keep the live path:
//! the first-party gapless contract is untouched, and DoP never comes here.
//!
//! - **Never waits for a render.** DSD→FLAC runs at only a little over
//!   realtime (a 5-minute DSD64 track takes minutes), so a miss starts the
//!   render in the background and streams the file *as it grows* —
//!   chunked, like a live transcode, from the one decode. Every later
//!   request is served from the finished file with byte ranges.
//! - **Key:** track id, source path, length + mtime, target, and
//!   [`transcode::RENDER_PARAMS`]. A retagged or replaced file changes the
//!   key, so stale renders are never served — they just age out.
//! - **Atomic:** rendered to a temporary name, renamed into place when
//!   complete; a crash leaves only temporaries, removed at startup.
//! - **Single flight:** one render per key; concurrent requests join it.
//!   A render runs on its own, so a client that hangs up (or seeks) mid-
//!   render doesn't waste it.
//! - **Bounded:** after each render the least recently served files are
//!   evicted until the cache fits its cap. Hits refresh a file's mtime.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use kahawai_core::{MusicError, StreamFormat};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::{mpsc, watch};

use crate::transcode::{self, TranscodePlan};

/// Temporary files of in-progress renders start with this.
const TMP_PREFIX: &str = ".render-";

#[derive(Debug, Clone, Default)]
pub struct TranscodeCache {
    /// `None`: the cache is off and every transcode streams live.
    inner: Option<Arc<Inner>>,
}

#[derive(Debug)]
struct Inner {
    dir: PathBuf,
    max_bytes: u64,
    /// Renders in progress, by cache file name (single flight).
    inflight: Mutex<HashMap<String, Render>>,
    /// Renders started, for tests of single flight.
    #[cfg(test)]
    renders: std::sync::atomic::AtomicUsize,
}

/// An in-progress render that requests can join.
#[derive(Debug, Clone)]
struct Render {
    tmp: PathBuf,
    progress: watch::Receiver<Progress>,
}

#[derive(Debug, Clone, Default)]
struct Progress {
    /// `X-Transcode-Chain`, once the pipeline is set up.
    chain: Option<String>,
    /// Bytes in the temporary file so far.
    written: u64,
    /// `Ok(final path)` once renamed into place; `Err` if the render failed.
    end: Option<Result<PathBuf, String>>,
}

/// How to serve a cacheable request.
pub enum Served {
    /// A finished render: serve with byte ranges. `chain` is `None` on a
    /// hit (the caller derives it from the source without rendering).
    File {
        path: PathBuf,
        chain: Option<String>,
    },
    /// A render in progress: its bytes so far, then the rest as they come.
    Growing {
        chain: String,
        body: axum::body::Body,
    },
}

impl TranscodeCache {
    /// The cache off: every transcode streams live.
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    /// A cache in `dir` holding at most `max_bytes` (0 = off). Creates the
    /// directory and removes temporaries left by a crash.
    pub fn open(dir: PathBuf, max_bytes: u64) -> Self {
        if max_bytes == 0 {
            return Self::disabled();
        }
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(dir = %dir.display(), error = %e, "transcode cache unavailable");
            return Self::disabled();
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                if e.file_name().to_string_lossy().starts_with(TMP_PREFIX) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        tracing::info!(dir = %dir.display(), max_mib = max_bytes >> 20, "transcode cache");
        Self {
            inner: Some(Arc::new(Inner {
                dir,
                max_bytes,
                inflight: Mutex::new(HashMap::new()),
                #[cfg(test)]
                renders: Default::default(),
            })),
        }
    }

    pub fn enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// The finished cache file for a plan, if there is one (no render).
    /// HEAD uses this to report what a GET would serve.
    pub async fn lookup(&self, track_id: i64, plan: &TranscodePlan) -> Option<PathBuf> {
        let inner = self.inner.as_ref()?;
        let path = inner.dir.join(file_name(track_id, plan).await.ok()?);
        tokio::fs::metadata(&path).await.ok().map(|_| path)
    }

    /// Serve `plan` from the cache: the finished file on a hit, otherwise
    /// the render (started now, or already running) as it grows.
    pub async fn serve(&self, track_id: i64, plan: TranscodePlan) -> Result<Served, MusicError> {
        let inner = self.inner()?;
        let name = file_name(track_id, &plan).await?;
        let path = inner.dir.join(&name);
        if touch(&path).await {
            tracing::info!(track_id, file = %name, "transcode cache hit");
            return Ok(Served::File { path, chain: None });
        }
        let render = match inner.join_or_start(track_id, &name, plan).await? {
            Ok(render) => render,
            // Finished between the hit check and the join.
            Err(path) => return Ok(Served::File { path, chain: None }),
        };
        let mut progress = render.progress.clone();
        let p = progress
            .wait_for(|p| p.chain.is_some() || p.end.is_some())
            .await
            .map_err(|_| MusicError::Http("transcode render vanished".into()))?
            .clone();
        match (p.chain, p.end) {
            (chain, Some(Ok(path))) => Ok(Served::File { path, chain }),
            (_, Some(Err(e))) => Err(MusicError::Http(e)),
            (Some(chain), None) => Ok(Served::Growing {
                chain,
                body: follow(render).await?,
            }),
            (None, None) => unreachable!("wait_for guarantees one"),
        }
    }

    /// Render `plan` unless it is cached, and wait for the finished file.
    #[cfg(test)]
    pub async fn ensure(&self, track_id: i64, plan: TranscodePlan) -> Result<PathBuf, MusicError> {
        let inner = self.inner()?;
        let name = file_name(track_id, &plan).await?;
        let path = inner.dir.join(&name);
        if touch(&path).await {
            return Ok(path);
        }
        let render = match inner.join_or_start(track_id, &name, plan).await? {
            Ok(render) => render,
            Err(path) => return Ok(path),
        };
        let mut progress = render.progress;
        let end = progress
            .wait_for(|p| p.end.is_some())
            .await
            .map_err(|_| MusicError::Http("transcode render vanished".into()))?
            .end
            .clone();
        end.expect("waited for it").map_err(MusicError::Http)
    }

    fn inner(&self) -> Result<Arc<Inner>, MusicError> {
        self.inner
            .clone()
            .ok_or_else(|| MusicError::Http("transcode cache is off".into()))
    }
}

impl Inner {
    /// Join the render of `name`, or start it. `Ok(Err(path))`: it is
    /// already finished. Setup errors (unreadable source, …) come back as
    /// the request's own error, with its real status.
    async fn join_or_start(
        self: &Arc<Self>,
        track_id: i64,
        name: &str,
        plan: TranscodePlan,
    ) -> Result<Result<Render, PathBuf>, MusicError> {
        let path = self.dir.join(name);
        let tmp = self
            .dir
            .join(format!("{TMP_PREFIX}{}-{name}", std::process::id()));
        let (tx, render) = {
            let mut inflight = self.inflight.lock().unwrap();
            if let Some(render) = inflight.get(name) {
                return Ok(Ok(render.clone()));
            }
            // The previous render may have finished just before the lock.
            if path.exists() {
                return Ok(Err(path));
            }
            let (tx, rx) = watch::channel(Progress::default());
            let render = Render {
                tmp: tmp.clone(),
                progress: rx,
            };
            inflight.insert(name.to_string(), render.clone());
            (tx, render)
        };

        tracing::info!(track_id, plan = %plan.describe(), "transcode cache miss: rendering");
        #[cfg(test)]
        self.renders
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let tmp_c = tmp.clone();
        let setup = tokio::task::spawn_blocking(move || {
            let out = std::fs::File::create(&tmp_c)?;
            let prepared = transcode::PreparedTranscode::setup(&plan)?;
            Ok::<_, MusicError>((out, prepared))
        })
        .await
        .map_err(|e| MusicError::Http(format!("transcode setup panicked: {e}")))
        .and_then(|r| r);
        let (out, prepared) = match setup {
            Ok(v) => v,
            Err(e) => {
                tx.send_modify(|p| p.end = Some(Err(e.to_string())));
                self.inflight.lock().unwrap().remove(name);
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(e);
            }
        };
        let chain = prepared.chain.clone();
        tx.send_modify(|p| p.chain = Some(chain));

        // The render owns itself from here: it outlives this request.
        let inner = Arc::clone(self);
        let name = name.to_string();
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            let tx_w = tx.clone();
            let (tmp_c, path_c) = (tmp.clone(), path.clone());
            let rendered = tokio::task::spawn_blocking(move || {
                transcode::render_to_file(prepared, out, |n| {
                    tx_w.send_modify(|p| p.written = n);
                })?;
                std::fs::rename(&tmp_c, &path_c)?;
                Ok::<_, MusicError>(())
            })
            .await
            .map_err(|e| MusicError::Http(format!("transcode render panicked: {e}")))
            .and_then(|r| r);
            match rendered {
                Ok(()) => {
                    tracing::info!(
                        track_id,
                        file = %name,
                        secs = started.elapsed().as_secs(),
                        "transcode cached"
                    );
                    // Back under the cap before anyone sees the render end.
                    let (dir, max, keep) = (inner.dir.clone(), inner.max_bytes, path.clone());
                    let _ = tokio::task::spawn_blocking(move || evict(&dir, max, &keep)).await;
                    tx.send_modify(|p| p.end = Some(Ok(path)));
                }
                Err(e) => {
                    tracing::warn!(track_id, file = %name, error = %e, "transcode render failed");
                    let _ = tokio::fs::remove_file(&tmp).await;
                    tx.send_modify(|p| p.end = Some(Err(e.to_string())));
                }
            }
            inner.inflight.lock().unwrap().remove(&name);
        });
        Ok(Ok(render))
    }
}

/// A body that streams a render's file as it grows: everything written so
/// far, then each new piece, until the render ends. The file handle stays
/// valid across the rename into place. A failed render truncates the body
/// (the client sees an error, as with a failed live transcode).
async fn follow(render: Render) -> Result<axum::body::Body, MusicError> {
    let mut progress = render.progress;
    let mut file = match tokio::fs::File::open(&render.tmp).await {
        Ok(f) => f,
        // Finished and renamed before we got here: read the final file.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let end = progress
                .wait_for(|p| p.end.is_some())
                .await
                .map_err(|_| MusicError::Http("transcode render vanished".into()))?
                .end
                .clone();
            let path = end.expect("waited for it").map_err(MusicError::Http)?;
            tokio::fs::File::open(path).await?
        }
        Err(e) => return Err(e.into()),
    };
    let (tx, rx) = mpsc::channel::<Result<Bytes, MusicError>>(8);
    tokio::spawn(async move {
        let mut sent = 0u64;
        loop {
            let (written, end) = {
                let p = progress.borrow_and_update();
                (p.written, p.end.clone())
            };
            if let Some(Err(e)) = end {
                let _ = tx.send(Err(MusicError::Http(e))).await;
                return;
            }
            while sent < written {
                let mut buf = vec![0u8; (written - sent).min(64 * 1024) as usize];
                let read = async {
                    file.seek(std::io::SeekFrom::Start(sent)).await?;
                    file.read_exact(&mut buf).await
                };
                if let Err(e) = read.await {
                    let _ = tx.send(Err(MusicError::Io(e))).await;
                    return;
                }
                sent += buf.len() as u64;
                if tx.send(Ok(Bytes::from(buf))).await.is_err() {
                    return; // client gone; the render carries on
                }
            }
            if end.is_some() {
                return; // finished, and every byte sent
            }
            if progress.changed().await.is_err() {
                return;
            }
        }
    });
    Ok(axum::body::Body::from_stream(
        tokio_stream::wrappers::ReceiverStream::new(rx),
    ))
}

/// `{track}-{key}.{ext}`: the key hashes everything that shapes the bytes.
async fn file_name(track_id: i64, plan: &TranscodePlan) -> Result<String, MusicError> {
    let meta = tokio::fs::metadata(&plan.path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            MusicError::NotFound(format!("file missing: {}", plan.path.display()))
        } else {
            MusicError::Io(e)
        }
    })?;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let ext = match plan.target {
        StreamFormat::Flac => "flac",
        StreamFormat::Opus => "opus",
        StreamFormat::Mp3 => "mp3",
        other => {
            return Err(MusicError::BadRequest(format!(
                "{other:?} is not a cacheable rendition"
            )))
        }
    };
    let key = format!(
        "{track_id}|{}|{}|{mtime_ns}|{ext}|{}",
        plan.path.display(),
        meta.len(),
        transcode::RENDER_PARAMS
    );
    let hash = blake3::hash(key.as_bytes()).to_hex();
    Ok(format!("{track_id}-{}.{ext}", &hash[..24]))
}

/// Mark a cached file as just used; false when it isn't there.
async fn touch(path: &Path) -> bool {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|f| f.set_modified(std::time::SystemTime::now()))
            .is_ok()
    })
    .await
    .unwrap_or(false)
}

/// Delete least recently used renders until the cache fits `max_bytes`.
/// Never deletes `keep` (just rendered), even when it alone is over the
/// cap. Files being served stay readable on Unix (open handles survive
/// deletion); elsewhere a failed delete just leaves the file for next time.
fn evict(dir: &Path, max_bytes: u64, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with(TMP_PREFIX))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            if !m.is_file() {
                return None;
            }
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort();
    for (_, len, path) in files {
        if total <= max_bytes {
            break;
        }
        if path == keep {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {
                total -= len;
                tracing::info!(file = %path.display(), "transcode cache evicted");
            }
            Err(e) => tracing::warn!(file = %path.display(), error = %e, "evict failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration_tests::{dsf_fixture, wav_fixture};
    use crate::transcode::PcmSource;
    use crate::{db, jobs, AppState};
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
        response::Response,
        Router,
    };
    use kahawai_core::{config::ServerConfig, AudioFormat};
    use tower::ServiceExt;

    /// STREAMINFO's 36-bit total sample count.
    fn flac_total_samples(file: &[u8]) -> u64 {
        assert_eq!(&file[..4], b"fLaC");
        let si = &file[8..42];
        (((si[13] & 0x0F) as u64) << 32) | u32::from_be_bytes(si[14..18].try_into().unwrap()) as u64
    }

    /// Decode a file end to end; PCM frames out.
    fn decoded_frames(path: &Path) -> u64 {
        let mut src = crate::transcode::SymphoniaSource::open(path).unwrap();
        let ch = src.spec().channels;
        let mut buf = vec![0f32; 4096 * ch];
        let mut frames = 0u64;
        loop {
            let n = src.fill(&mut buf).unwrap();
            if n == 0 {
                return frames;
            }
            frames += (n / ch) as u64;
        }
    }

    /// Render `plan` straight to `out`; the chain.
    fn render(plan: &TranscodePlan, out: &Path) -> String {
        let prepared = transcode::PreparedTranscode::setup(plan).unwrap();
        let chain = prepared.chain.clone();
        let mut last = 0;
        transcode::render_to_file(prepared, std::fs::File::create(out).unwrap(), |n| {
            assert!(n > last, "progress only grows");
            last = n;
        })
        .unwrap();
        assert_eq!(
            last,
            std::fs::metadata(out).unwrap().len(),
            "every byte reported"
        );
        chain
    }

    fn renders(cache: &TranscodeCache) -> usize {
        cache
            .inner
            .as_ref()
            .unwrap()
            .renders
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Canonical, as the stream handler resolves it (the path is part of
    /// the cache key).
    fn plan(path: PathBuf, source_format: AudioFormat) -> TranscodePlan {
        TranscodePlan {
            path: path.canonicalize().unwrap(),
            source_format,
            target: StreamFormat::Flac,
            seek_ms: None,
        }
    }

    #[test]
    fn rendered_flac_knows_its_length() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("sine.wav");
        // 1.5 s: 16 full 4096-frame blocks plus a partial one.
        std::fs::write(&wav, wav_fixture(44_100, 2, 66_150)).unwrap();
        let out = dir.path().join("out.flac");
        assert_eq!(
            render(&plan(wav, AudioFormat::Wav), &out),
            "wav->flac 24/44.1"
        );
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(flac_total_samples(&bytes), 66_150);
        let si = &bytes[8..42];
        let min_frame = u32::from_be_bytes([0, si[4], si[5], si[6]]);
        let max_frame = u32::from_be_bytes([0, si[7], si[8], si[9]]);
        assert!(
            min_frame > 0 && min_frame <= max_frame,
            "{min_frame} {max_frame}"
        );
        // And a decoder agrees with the header.
        assert_eq!(decoded_frames(&out), 66_150);
    }

    #[test]
    fn rendered_dsd_flac_knows_its_length() {
        let dir = tempfile::tempdir().unwrap();
        let dsf = dir.path().join("t.dsf");
        std::fs::write(&dsf, dsf_fixture(16, 4096, 0x69)).unwrap();
        let out = dir.path().join("out.flac");
        let chain = render(&plan(dsf, AudioFormat::Dsf), &out);
        assert!(chain.ends_with("->flac 24/88.2"), "{chain}");
        let total = flac_total_samples(&std::fs::read(&out).unwrap());
        assert!(total > 0);
        assert_eq!(decoded_frames(&out), total);
    }

    struct Fixture {
        app: Router,
        cache: TranscodeCache,
        cache_dir: PathBuf,
        music: PathBuf,
        _dir: tempfile::TempDir,
    }

    /// Track 1: a 1 s 44.1 kHz WAV; track 2: a DSF; track 3: a 0.5 s WAV.
    async fn fixture(max_bytes: u64) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("music");
        std::fs::create_dir(&music).unwrap();
        let pool = db::open(&dir.path().join("test.db")).await.unwrap();
        for (file, fmt, bytes) in [
            ("a.wav", "wav", wav_fixture(44_100, 2, 44_100)),
            ("b.dsf", "dsf", dsf_fixture(16, 4096, 0x69)),
            ("c.wav", "wav", wav_fixture(44_100, 2, 22_050)),
        ] {
            let p = music.join(file);
            std::fs::write(&p, bytes).unwrap();
            db::insert_track_minimal(&pool, p.to_str().unwrap(), "h", fmt)
                .await
                .unwrap();
        }
        let cache_dir = dir.path().join("transcode-cache");
        let cache = TranscodeCache::open(cache_dir.clone(), max_bytes);
        let state = AppState {
            pool,
            jobs: jobs::JobStore::new(),
            config: Arc::new(std::sync::RwLock::new(ServerConfig {
                music_dirs: vec![music.clone()],
                ..Default::default()
            })),
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            hash_lock: Arc::new(tokio::sync::Mutex::new(())),
            enrich_lock: Arc::new(tokio::sync::Mutex::new(())),
            catalog_events: tokio::sync::broadcast::channel(16).0,
            transcode_cache: cache.clone(),
        };
        Fixture {
            app: crate::app(state),
            cache,
            cache_dir,
            music,
            _dir: dir,
        }
    }

    fn cached_files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    async fn request(app: &Router, method: &str, uri: &str, range: Option<&str>) -> Response {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(r) = range {
            req = req.header(header::RANGE, r);
        }
        app.clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        range: Option<&str>,
    ) -> (axum::http::HeaderMap, StatusCode, Vec<u8>) {
        let res = request(app, method, uri, range).await;
        let (status, headers) = (res.status(), res.headers().clone());
        let body = to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (headers, status, body)
    }

    #[tokio::test]
    async fn first_play_streams_the_render_then_replays_are_seekable() {
        let f = fixture(1 << 30).await;
        // HEAD before any render: the live transcode's shape.
        let (h, status, _) = send(&f.app, "HEAD", "/stream/1?format=flac", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(h.get(header::ACCEPT_RANGES).is_none());

        // Miss: no waiting for the whole render — the file as it grows.
        let (h, status, first) = send(&f.app, "GET", "/stream/1?format=flac", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(h[header::CONTENT_TYPE], "audio/flac");
        assert!(h.get(header::CONTENT_LENGTH).is_none(), "growing: chunked");
        assert!(h.get(header::ACCEPT_RANGES).is_none());
        assert_eq!(h["x-transcode-chain"], "wav->flac 24/44.1");
        assert_eq!(&first[..4], b"fLaC");
        assert_eq!(cached_files(&f.cache_dir).len(), 1);

        // Replay: the finished file, with its length and byte ranges.
        let (h, status, full) = send(&f.app, "GET", "/stream/1?format=flac", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(h[header::ACCEPT_RANGES], "bytes");
        assert_eq!(h[header::CONTENT_LENGTH], full.len().to_string());
        assert_eq!(
            h["x-transcode-chain"], "wav->flac 24/44.1",
            "chain on a hit"
        );
        assert!(h.get(header::CONTENT_DISPOSITION).is_some());
        assert_eq!(flac_total_samples(&full), 44_100);
        // The same audio both times; only the header's length field may
        // differ (the first play may have gone out before it was known).
        assert_eq!(first.len(), full.len());
        assert_eq!(first[42..], full[42..]);

        // A seek: a byte range of the cached file.
        let (h, status, part) = send(
            &f.app,
            "GET",
            "/stream/1?format=flac",
            Some("bytes=1000-1999"),
        )
        .await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            h[header::CONTENT_RANGE],
            format!("bytes 1000-1999/{}", full.len())
        );
        assert_eq!(part, &full[1000..2000]);
        let (_, status, _) = send(
            &f.app,
            "GET",
            "/stream/1?format=flac",
            Some("bytes=999999999-"),
        )
        .await;
        assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);

        // HEAD now agrees with GET.
        let (h, _, _) = send(&f.app, "HEAD", "/stream/1?format=flac", None).await;
        assert_eq!(h[header::ACCEPT_RANGES], "bytes");
        assert_eq!(h[header::CONTENT_LENGTH], full.len().to_string());
        assert_eq!(renders(&f.cache), 1, "rendered once");
    }

    #[tokio::test]
    async fn dsd_flac_is_cached_with_its_duration() {
        let f = fixture(1 << 30).await;
        let _ = send(&f.app, "GET", "/stream/2?format=flac", None).await;
        let (h, status, body) = send(&f.app, "GET", "/stream/2?format=flac", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(h[header::ACCEPT_RANGES], "bytes");
        assert!(flac_total_samples(&body) > 0);
    }

    #[tokio::test]
    async fn chains_and_seek_ms_stay_live() {
        let f = fixture(1 << 30).await;
        for uri in [
            "/stream/1?format=flac&next=3",
            "/stream/1?format=flac&seek_ms=500",
        ] {
            let (h, status, body) = send(&f.app, "GET", uri, None).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
            assert!(h.get(header::CONTENT_LENGTH).is_none(), "{uri}: live");
            assert_eq!(&body[..4], b"fLaC");
        }
        // Passthrough never touches the cache either.
        let (_, status, _) = send(&f.app, "GET", "/stream/1", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(cached_files(&f.cache_dir).is_empty());
        assert_eq!(renders(&f.cache), 0);
    }

    #[tokio::test]
    async fn concurrent_requests_share_one_render() {
        let f = fixture(1 << 30).await;
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let app = f.app.clone();
                tokio::spawn(async move { send(&app, "GET", "/stream/2?format=flac", None).await })
            })
            .collect();
        let mut bodies = Vec::new();
        for h in handles {
            let (_, status, body) = h.await.unwrap();
            assert_eq!(status, StatusCode::OK);
            bodies.push(body);
        }
        assert_eq!(renders(&f.cache), 1, "exactly one render");
        assert!(bodies.iter().all(|b| b[42..] == bodies[0][42..]));
        assert_eq!(cached_files(&f.cache_dir).len(), 1);
    }

    #[tokio::test]
    async fn a_client_hanging_up_does_not_waste_the_render() {
        let f = fixture(1 << 30).await;
        let res = request(&f.app, "GET", "/stream/2?format=flac", None).await;
        assert_eq!(res.status(), StatusCode::OK);
        drop(res); // VLC seeking: it closes the connection mid-stream
        let path = f
            .cache
            .ensure(2, plan(f.music.join("b.dsf"), AudioFormat::Dsf))
            .await
            .unwrap();
        assert!(flac_total_samples(&std::fs::read(path).unwrap()) > 0);
        assert_eq!(renders(&f.cache), 1, "joined, not restarted");
    }

    #[tokio::test]
    async fn a_failed_setup_keeps_its_status_and_leaves_nothing() {
        let f = fixture(1 << 30).await;
        std::fs::write(f.music.join("a.wav"), b"not a wav at all").unwrap();
        let (_, status, _) = send(&f.app, "GET", "/stream/1?format=flac", None).await;
        assert!(
            status.is_client_error() || status.is_server_error(),
            "{status}"
        );
        assert!(cached_files(&f.cache_dir).is_empty(), "no temporaries left");
        // And the next request tries again rather than joining a dead render.
        let _ = send(&f.app, "GET", "/stream/1?format=flac", None).await;
        assert_eq!(renders(&f.cache), 2);
    }

    #[tokio::test]
    async fn a_changed_source_is_rendered_again() {
        let f = fixture(1 << 30).await;
        let p = plan(f.music.join("a.wav"), AudioFormat::Wav);
        let first = f.cache.ensure(1, p.clone()).await.unwrap();
        assert_eq!(f.cache.ensure(1, p.clone()).await.unwrap(), first);
        assert_eq!(renders(&f.cache), 1);
        // Retagged/replaced on disk: a different length changes the key.
        std::fs::write(f.music.join("a.wav"), wav_fixture(44_100, 2, 4_410)).unwrap();
        let second = f.cache.ensure(1, p).await.unwrap();
        assert_eq!(renders(&f.cache), 2, "re-rendered");
        assert_ne!(first, second);
        assert_eq!(flac_total_samples(&std::fs::read(&second).unwrap()), 4_410);
    }

    #[tokio::test]
    async fn least_recently_used_renders_are_evicted_past_the_cap() {
        let f = fixture(1 << 30).await;
        let (a, c) = (f.music.join("a.wav"), f.music.join("c.wav"));
        let ra = f
            .cache
            .ensure(1, plan(a.clone(), AudioFormat::Wav))
            .await
            .unwrap();
        // Room for about one 1 s render.
        let cap = std::fs::metadata(&ra).unwrap().len() + 100;
        let small = TranscodeCache::open(f.cache_dir.clone(), cap);
        let rc = small.ensure(3, plan(c, AudioFormat::Wav)).await.unwrap();
        // a (older) went to make room for c.
        assert!(!ra.exists(), "oldest evicted");
        assert!(rc.exists(), "newest kept");
        // Over the cap on its own: still served, and kept until next time.
        let tiny = TranscodeCache::open(f.cache_dir.clone(), 1);
        let ra2 = tiny.ensure(1, plan(a, AudioFormat::Wav)).await.unwrap();
        assert!(ra2.exists());
        assert!(!rc.exists());
    }

    #[test]
    fn open_clears_crashed_renders_and_zero_disables() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("tc");
        std::fs::create_dir(&cache_dir).unwrap();
        std::fs::write(cache_dir.join(".render-1-1-x.flac"), b"partial").unwrap();
        std::fs::write(cache_dir.join("1-abc.flac"), b"done").unwrap();
        assert!(TranscodeCache::open(cache_dir.clone(), 1 << 20).enabled());
        assert_eq!(cached_files(&cache_dir), ["1-abc.flac"]);
        assert!(!TranscodeCache::open(cache_dir, 0).enabled());
        assert!(!TranscodeCache::disabled().enabled());
    }

    /// How long a real track takes to render (the first play of a DSD track
    /// is limited by this; it must stay above realtime).
    /// `KAHAWAI_BENCH_DSF=/path/to.dsf cargo test --release -p
    /// kahawai-server render_benchmark -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn render_benchmark() {
        let src = PathBuf::from(std::env::var("KAHAWAI_BENCH_DSF").expect("KAHAWAI_BENCH_DSF"));
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.flac");
        let started = std::time::Instant::now();
        let chain = render(&plan(src, AudioFormat::Dsf), &out);
        let took = started.elapsed();
        let bytes = std::fs::read(&out).unwrap();
        let audio_s = flac_total_samples(&bytes) as f64 / 88_200.0;
        println!(
            "{chain}: {audio_s:.1} s of audio rendered in {took:.2?} ({:.1}x realtime, {} MiB)",
            audio_s / took.as_secs_f64(),
            bytes.len() >> 20
        );
    }
}
