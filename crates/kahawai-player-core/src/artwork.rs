//! Album-art disk cache.
//!
//! The server names artwork by the SHA-256 of its bytes (`/api/artwork/{hash}`),
//! so an entry can never go stale: the same hash is always the same image.
//! That makes the cache trivially safe — no expiry, no revalidation, and it
//! stays valid across server-URL changes and server restarts, and lets covers
//! show while the server is unreachable.
//!
//! Layout: `<dir>/<hash>.img` (raw image bytes; the mime type is sniffed from
//! the magic bytes, so there is no sidecar to get out of sync). Writes are
//! atomic (temp file + rename) so a crash never leaves a truncated image.
//! When the directory exceeds `max_bytes`, the least recently used files
//! (by mtime, refreshed on every hit) are deleted first.
//!
//! No Tauri or async here: the shell calls [`ArtworkCache::get_or_fetch`]
//! from a worker thread, with [`fetch_from_server`] as the fetcher.

use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use kahawai_core::MusicError;

/// Largest image we will accept from the server.
const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
/// Leftover temp files older than this are swept during eviction.
const STALE_TMP: Duration = Duration::from_secs(3600);

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// One artwork image plus where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedArt {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    /// `true` when served from disk without touching the network.
    pub from_cache: bool,
}

#[derive(Debug)]
pub struct ArtworkCache {
    dir: PathBuf,
    /// Atomic so the cap can be changed from the Settings UI without a
    /// `Mutex` around the whole cache.
    max_bytes: AtomicU64,
}

/// Content hashes are hex digests; anything else is rejected before it
/// gets near the filesystem (no path traversal) or the network.
pub fn is_valid_hash(hash: &str) -> bool {
    !hash.is_empty() && hash.len() <= 128 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Image type from magic bytes. `None` for anything that is not a
/// browser-renderable image (so an HTML error page is never cached as art).
pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"BM") && bytes.len() > 14 {
        Some("image/bmp")
    } else {
        None
    }
}

impl ArtworkCache {
    /// Open (creating if needed) a cache directory capped at `max_bytes`.
    pub fn new(dir: impl Into<PathBuf>, max_bytes: u64) -> Result<Self, MusicError> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(MusicError::Io)?;
        Ok(Self {
            dir,
            max_bytes: AtomicU64::new(max_bytes),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.load(Ordering::Relaxed)
    }

    /// Change the cap (Settings UI). Evicts immediately if the new cap is
    /// smaller than what's already on disk, rather than waiting for the
    /// next write.
    pub fn set_max_bytes(&self, max_bytes: u64) {
        self.max_bytes.store(max_bytes, Ordering::Relaxed);
        self.evict();
    }

    fn path_for(&self, hash: &str) -> PathBuf {
        self.dir.join(format!("{}.img", hash.to_ascii_lowercase()))
    }

    /// Read a cached image, refreshing its recency. `None` on a miss (or an
    /// unreadable/garbled entry, which is removed so it gets re-fetched).
    pub fn get(&self, hash: &str) -> Option<CachedArt> {
        if !is_valid_hash(hash) {
            return None;
        }
        let path = self.path_for(hash);
        let bytes = fs::read(&path).ok()?;
        let Some(mime) = sniff_mime(&bytes) else {
            let _ = fs::remove_file(&path);
            return None;
        };
        // Touch for LRU; best effort.
        if let Ok(f) = OpenOptions::new().write(true).open(&path) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(CachedArt {
            bytes,
            mime,
            from_cache: true,
        })
    }

    /// Store an image (atomically) and trim the cache to its size cap.
    pub fn put(&self, hash: &str, bytes: &[u8]) -> Result<(), MusicError> {
        if !is_valid_hash(hash) {
            return Err(MusicError::BadRequest(
                "artwork hash must be hexadecimal".into(),
            ));
        }
        if sniff_mime(bytes).is_none() {
            return Err(MusicError::BadRequest("not a recognised image".into()));
        }
        let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = self.dir.join(format!(
            ".{}.{}.{n}.tmp",
            hash.to_ascii_lowercase(),
            std::process::id()
        ));
        fs::write(&tmp, bytes).map_err(MusicError::Io)?;
        if let Err(e) = fs::rename(&tmp, self.path_for(hash)) {
            let _ = fs::remove_file(&tmp);
            return Err(MusicError::Io(e));
        }
        self.evict();
        Ok(())
    }

    /// Cache hit, or `fetch` the bytes, store them and return them.
    /// A failed fetch is *not* cached, so a later call retries.
    pub fn get_or_fetch<F>(&self, hash: &str, fetch: F) -> Result<CachedArt, MusicError>
    where
        F: FnOnce() -> Result<Vec<u8>, MusicError>,
    {
        if !is_valid_hash(hash) {
            return Err(MusicError::BadRequest(
                "artwork hash must be hexadecimal".into(),
            ));
        }
        if let Some(hit) = self.get(hash) {
            return Ok(hit);
        }
        let bytes = fetch()?;
        let mime = sniff_mime(&bytes)
            .ok_or_else(|| MusicError::Http("artwork response is not an image".into()))?;
        // A full disk / read-only cache must not stop the cover showing.
        if let Err(e) = self.put(hash, &bytes) {
            tracing::warn!(error = %e, "artwork cache write failed");
        }
        Ok(CachedArt {
            bytes,
            mime,
            from_cache: false,
        })
    }

    /// Total size and count of cached images.
    pub fn stats(&self) -> (u64, usize) {
        self.entries()
            .iter()
            .fold((0, 0), |(b, n), (_, len, _)| (b + len, n + 1))
    }

    /// Delete every cached image. Returns how many were removed.
    pub fn clear(&self) -> usize {
        self.entries()
            .into_iter()
            .filter(|(p, _, _)| fs::remove_file(p).is_ok())
            .count()
    }

    /// `(path, len, mtime)` of every `.img` entry.
    fn entries(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        rd.filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "img"))
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                Some((
                    e.path(),
                    m.len(),
                    m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                ))
            })
            .collect()
    }

    /// Drop least-recently-used images until the cache fits `max_bytes`,
    /// and sweep abandoned temp files.
    fn evict(&self) {
        if let Ok(rd) = fs::read_dir(&self.dir) {
            for e in rd.filter_map(|e| e.ok()) {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "tmp") {
                    let old = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > STALE_TMP);
                    if old {
                        let _ = fs::remove_file(p);
                    }
                }
            }
        }
        let max_bytes = self.max_bytes();
        let mut entries = self.entries();
        let mut total: u64 = entries.iter().map(|(_, len, _)| len).sum();
        if total <= max_bytes {
            return;
        }
        entries.sort_by_key(|(_, _, mtime)| *mtime);
        for (path, len, _) in entries {
            if total <= max_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
            }
        }
    }
}

/// Fetch one image from the server (`GET {base}/api/artwork/{hash}`).
pub fn fetch_from_server(base_url: &str, hash: &str) -> Result<Vec<u8>, MusicError> {
    if !is_valid_hash(hash) {
        return Err(MusicError::BadRequest(
            "artwork hash must be hexadecimal".into(),
        ));
    }
    let url = format!("{}/api/artwork/{hash}", base_url.trim_end_matches('/'));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let resp = agent.get(&url).call().map_err(|e| match e {
        ureq::Error::StatusCode(404) => MusicError::NotFound(format!("artwork {hash}")),
        other => MusicError::Http(other.to_string()),
    })?;
    let mut bytes = Vec::new();
    resp.into_body()
        .into_reader()
        .take(MAX_IMAGE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(MusicError::Io)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F'];
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

    fn cache(max: u64) -> (tempfile::TempDir, ArtworkCache) {
        let dir = tempfile::tempdir().unwrap();
        let c = ArtworkCache::new(dir.path().join("art"), max).unwrap();
        (dir, c)
    }

    #[test]
    fn hash_validation_blocks_traversal() {
        assert!(is_valid_hash("deadBEEF01"));
        for bad in [
            "",
            "../etc/passwd",
            "a/b",
            "xyz",
            "abc.img",
            &"a".repeat(129),
        ] {
            assert!(!is_valid_hash(bad), "{bad}");
        }
    }

    #[test]
    fn sniffs_common_image_types_and_rejects_others() {
        assert_eq!(sniff_mime(JPEG), Some("image/jpeg"));
        assert_eq!(sniff_mime(PNG), Some("image/png"));
        assert_eq!(sniff_mime(b"GIF89a.."), Some("image/gif"));
        assert_eq!(sniff_mime(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_mime(b"<html>not found</html>"), None);
        assert_eq!(sniff_mime(b""), None);
    }

    #[test]
    fn miss_fetches_once_then_serves_from_disk() {
        let (_d, c) = cache(1 << 20);
        let calls = Cell::new(0);
        let fetch = || {
            calls.set(calls.get() + 1);
            Ok(JPEG.to_vec())
        };
        let first = c.get_or_fetch("abc123", fetch).unwrap();
        assert!(!first.from_cache);
        assert_eq!(first.mime, "image/jpeg");

        let second = c
            .get_or_fetch("abc123", || panic!("must not hit the network"))
            .unwrap();
        assert!(second.from_cache);
        assert_eq!(second.bytes, JPEG);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn survives_reopening_the_cache() {
        let (d, c) = cache(1 << 20);
        c.put("abc123", PNG).unwrap();
        let reopened = ArtworkCache::new(d.path().join("art"), 1 << 20).unwrap();
        assert_eq!(reopened.get("abc123").unwrap().mime, "image/png");
    }

    #[test]
    fn hash_case_does_not_create_duplicates() {
        let (_d, c) = cache(1 << 20);
        c.put("ABCDEF", JPEG).unwrap();
        assert!(c.get("abcdef").is_some());
        assert_eq!(c.stats().1, 1);
    }

    #[test]
    fn failed_or_non_image_fetch_is_not_cached() {
        let (_d, c) = cache(1 << 20);
        assert!(c
            .get_or_fetch("aa", || Err(MusicError::NotFound("x".into())))
            .is_err());
        assert!(c
            .get_or_fetch("aa", || Ok(b"<html>oops</html>".to_vec()))
            .is_err());
        assert_eq!(c.stats(), (0, 0));
        // A later good response works.
        assert!(c.get_or_fetch("aa", || Ok(JPEG.to_vec())).is_ok());
    }

    #[test]
    fn corrupt_entry_is_dropped_and_refetched() {
        let (_d, c) = cache(1 << 20);
        fs::write(c.path_for("bb"), b"garbage").unwrap();
        assert!(c.get("bb").is_none());
        assert!(!c.path_for("bb").exists());
        assert!(c.get_or_fetch("bb", || Ok(PNG.to_vec())).is_ok());
    }

    #[test]
    fn evicts_least_recently_used_over_the_cap() {
        let img = |n: u8| {
            let mut v = JPEG.to_vec();
            v.resize(100, n);
            v
        };
        // Room for two 100-byte images.
        let (_d, c) = cache(250);
        c.put("01", &img(1)).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        c.put("02", &img(2)).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        // Touch 01 so 02 becomes the oldest.
        assert!(c.get("01").is_some());
        std::thread::sleep(Duration::from_millis(30));
        c.put("03", &img(3)).unwrap();

        assert!(c.get("01").is_some(), "recently used entry kept");
        assert!(c.get("02").is_none(), "LRU entry evicted");
        assert!(c.get("03").is_some());
        assert!(c.stats().0 <= 250);
    }

    #[test]
    fn lowering_the_cap_evicts_immediately_not_on_the_next_write() {
        let img = |n: u8| {
            let mut v = JPEG.to_vec();
            v.resize(100, n);
            v
        };
        let (_d, c) = cache(1 << 20); // room for plenty
        c.put("01", &img(1)).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        c.put("02", &img(2)).unwrap();
        assert_eq!(c.stats(), (200, 2));

        // Settings UI drops the cap below what's already on disk.
        c.set_max_bytes(150);
        assert_eq!(c.max_bytes(), 150);
        assert!(c.get("01").is_none(), "oldest entry evicted right away");
        assert!(c.get("02").is_some());
        assert!(c.stats().0 <= 150);
    }

    #[test]
    fn clear_removes_everything() {
        let (_d, c) = cache(1 << 20);
        c.put("01", JPEG).unwrap();
        c.put("02", PNG).unwrap();
        assert_eq!(c.clear(), 2);
        assert_eq!(c.stats(), (0, 0));
    }

    #[test]
    fn rejects_bad_hash_everywhere() {
        let (_d, c) = cache(1 << 20);
        assert!(c.get("../x").is_none());
        assert!(c.put("../x", JPEG).is_err());
        assert!(c.get_or_fetch("../x", || Ok(JPEG.to_vec())).is_err());
        assert!(fetch_from_server("http://localhost:1", "../x").is_err());
    }
}
