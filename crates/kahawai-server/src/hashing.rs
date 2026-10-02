//! Content hashing: Phase B of the scan pipeline
//! (docs/v1/kahawai-fast-first-scan-spec.md).
//!
//! The metadata scan catalogs files without reading their contents and
//! leaves `tracks.hash` NULL ("pending"). This pass hashes those files in
//! the background, without blocking use of the library.
//!
//! - The NULL marker is the checkpoint. Each hash is written on its own as
//!   soon as it is computed, so a killed job loses only the files it was
//!   reading, and the next run picks up exactly the rows still pending.
//! - A file whose size or mtime no longer match its row changed since the
//!   scan: it is skipped, and the next scan re-reads it (resetting its hash
//!   to pending). The write is guarded on the same values, so a hash is
//!   never stamped onto a row a concurrent scan has just rewritten.
//! - Hashing is throughput-bound (whole files over the network), so a few
//!   workers with large reads beat many small ones.

use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use kahawai_core::MusicError;
use sqlx::{sqlite::SqlitePool, Row};
use tokio::task::JoinSet;
use tracing::{info, warn};

use crate::db;

/// Recorded in `tracks.hash_algo` next to every hash this module writes.
pub const HASH_ALGO: &str = "blake3-v1";
/// Files hashed at once.
const HASH_WORKERS: usize = 4;
/// Pending rows fetched per query.
const HASH_BATCH: i64 = 200;
/// Read size: large reads keep a network share streaming.
const READ_BUF: usize = 1 << 20;
/// How often a running hash job logs its throughput.
const PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(30);

/// Outcome of one hashing run.
#[derive(Debug, Default)]
pub struct HashReport {
    /// Rows that got their hash.
    pub hashed: u64,
    /// Rows left pending: the file changed since the scan, or couldn't be
    /// read. They are tried again on the next run.
    pub skipped: u64,
    /// Bytes read for the rows that got their hash.
    pub bytes: u64,
    pub elapsed_secs: f64,
}

/// BLAKE3 of a file's contents, as lowercase hex.
pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut f = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; READ_BUF];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Tracks on disk still waiting for their hash.
pub async fn pending_count(pool: &SqlitePool) -> Result<u64, MusicError> {
    let n: i64 = sqlx::query(
        "SELECT COUNT(*) FROM tracks WHERE hash IS NULL AND missing = 0 AND kind = 'music'",
    )
    .fetch_one(pool)
    .await
    .map_err(db::cvt)?
    .get(0);
    Ok(n as u64)
}

/// One row waiting for its hash, with the stat the scan recorded.
struct Pending {
    id: i64,
    path: PathBuf,
    size: Option<i64>,
    mtime: Option<i64>,
}

/// Hash `p` if the file on disk is still the one the scan cataloged.
/// `Ok(None)` means it changed since; `Err` means it couldn't be read.
fn hash_if_unchanged(p: &Pending) -> io::Result<Option<String>> {
    let meta = std::fs::metadata(&p.path)?;
    if Some(meta.len() as i64) != p.size || crate::scanner::mtime_secs(&meta) != p.mtime {
        return Ok(None);
    }
    hash_file(&p.path).map(Some)
}

/// Hash every pending track, in id order. `on_progress(done, total, bytes)`
/// fires once per file; `total` is the pending count when the run started
/// and `bytes` the content hashed so far.
pub async fn hash_pending(
    pool: &SqlitePool,
    on_progress: impl Fn(u64, u64, u64) + Send + Sync,
) -> Result<HashReport, MusicError> {
    let start = Instant::now();
    let total = pending_count(pool).await?;
    let mut report = HashReport::default();
    let mut in_flight: JoinSet<(Pending, io::Result<Option<String>>)> = JoinSet::new();
    // Keyset paging: a row that stays pending (unreadable) is not revisited
    // in the same run.
    let mut after_id = 0i64;
    let mut exhausted = false;
    let mut queue: VecDeque<Pending> = VecDeque::new();
    let mut last_log = Instant::now();

    loop {
        if queue.is_empty() && !exhausted {
            let rows = sqlx::query(
                "SELECT id, path, file_size, file_mtime FROM tracks
                 WHERE hash IS NULL AND missing = 0 AND kind = 'music' AND id > ?
                 ORDER BY id LIMIT ?",
            )
            .bind(after_id)
            .bind(HASH_BATCH)
            .fetch_all(pool)
            .await
            .map_err(db::cvt)?;
            exhausted = (rows.len() as i64) < HASH_BATCH;
            for r in &rows {
                after_id = r.get("id");
                queue.push_back(Pending {
                    id: after_id,
                    path: PathBuf::from(r.get::<String, _>("path")),
                    size: r.get("file_size"),
                    mtime: r.get("file_mtime"),
                });
            }
        }
        // Keep HASH_WORKERS files in flight.
        while in_flight.len() < HASH_WORKERS {
            let Some(p) = queue.pop_front() else { break };
            in_flight.spawn_blocking(move || {
                let result = hash_if_unchanged(&p);
                (p, result)
            });
        }
        let Some(joined) = in_flight.join_next().await else {
            break; // nothing queued, nothing in flight: done
        };
        let (p, result) =
            joined.map_err(|e| MusicError::JobFailed(format!("hash worker panicked: {e}")))?;
        let hashed = match result {
            Ok(Some(hash)) => {
                let res = sqlx::query(
                    "UPDATE tracks SET hash = ?, hash_algo = ?
                     WHERE id = ? AND hash IS NULL AND file_size IS ? AND file_mtime IS ?",
                )
                .bind(&hash)
                .bind(HASH_ALGO)
                .bind(p.id)
                .bind(p.size)
                .bind(p.mtime)
                .execute(pool)
                .await
                .map_err(db::cvt)?;
                res.rows_affected() == 1
            }
            Ok(None) => false,
            Err(e) => {
                warn!(path = %p.path.display(), error = %e, "hash failed; left pending");
                false
            }
        };
        if hashed {
            report.hashed += 1;
            report.bytes += p.size.unwrap_or(0).max(0) as u64;
        } else {
            report.skipped += 1;
        }
        on_progress(report.hashed + report.skipped, total, report.bytes);
        if last_log.elapsed() >= PROGRESS_LOG_INTERVAL {
            last_log = Instant::now();
            log_throughput("hash progress", &report, total, start.elapsed());
        }
    }

    report.elapsed_secs = start.elapsed().as_secs_f64();
    log_throughput("content hashing complete", &report, total, start.elapsed());
    Ok(report)
}

fn log_throughput(what: &str, r: &HashReport, total: u64, elapsed: Duration) {
    let secs = elapsed.as_secs_f64().max(1e-9);
    info!(
        hashed = r.hashed,
        skipped = r.skipped,
        remaining = total.saturating_sub(r.hashed + r.skipped),
        files_per_sec = (r.hashed as f64 / secs * 10.0).round() / 10.0,
        mb_per_sec = (r.bytes as f64 / 1e6 / secs * 10.0).round() / 10.0,
        gb_hashed = (r.bytes as f64 / 1e8).round() / 10.0,
        elapsed_secs = secs.round() as u64,
        "{what}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A catalog row for `path` as a Phase A scan would leave it: hash
    /// pending, with the file's current stat.
    async fn pending_row(pool: &SqlitePool, path: &Path) -> i64 {
        let meta = std::fs::metadata(path).unwrap();
        sqlx::query(
            "INSERT INTO tracks (path, format, file_size, file_mtime) VALUES (?, 'flac', ?, ?)
             RETURNING id",
        )
        .bind(path.to_string_lossy().to_string())
        .bind(meta.len() as i64)
        .bind(crate::scanner::mtime_secs(&meta))
        .fetch_one(pool)
        .await
        .unwrap()
        .get(0)
    }

    async fn hash_of(pool: &SqlitePool, id: i64) -> (Option<String>, Option<String>) {
        let r = sqlx::query("SELECT hash, hash_algo FROM tracks WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
        (r.get("hash"), r.get("hash_algo"))
    }

    /// Phase B benchmark against a real catalog (e.g. one written by
    /// `scanner::first_scan_benchmark`). Not run by default: it reads every
    /// pending file. Release mode:
    ///
    /// ```text
    /// KAHAWAI_BENCH_DB=/local/disk/bench.db \
    /// cargo test --release -p kahawai-server hash_benchmark -- --ignored --nocapture
    /// ```
    ///
    /// Safe to kill at any point and start again: it resumes.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "reads a whole real library; run by hand"]
    async fn hash_benchmark() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
            .try_init();
        let db_path = PathBuf::from(std::env::var("KAHAWAI_BENCH_DB").expect("KAHAWAI_BENCH_DB"));
        assert!(db_path.exists(), "{} does not exist", db_path.display());
        let pool = db::open(&db_path).await.unwrap();
        let r = hash_pending(&pool, |_, _, _| {}).await.unwrap();
        println!(
            "HASH RUN: {} hashed, {} skipped, {:.1} GB in {:.1} s ({:.1} MB/s)",
            r.hashed,
            r.skipped,
            r.bytes as f64 / 1e9,
            r.elapsed_secs,
            r.bytes as f64 / 1e6 / r.elapsed_secs.max(1e-9)
        );
    }

    #[test]
    fn hash_file_is_blake3_of_the_contents_across_read_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.flac");
        let bytes: Vec<u8> = (0..READ_BUF * 2 + 123).map(|i| (i % 251) as u8).collect();
        std::fs::write(&p, &bytes).unwrap();
        assert_eq!(
            hash_file(&p).unwrap(),
            blake3::hash(&bytes).to_hex().to_string()
        );
    }

    #[tokio::test]
    async fn hashes_every_pending_track_and_labels_the_algorithm() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("t.db")).await.unwrap();
        let mut ids = Vec::new();
        for i in 0..(HASH_BATCH as usize + 7) {
            let p = dir.path().join(format!("{i}.flac"));
            std::fs::write(&p, format!("audio {i}")).unwrap();
            ids.push((pending_row(&pool, &p).await, format!("audio {i}")));
        }
        assert_eq!(pending_count(&pool).await.unwrap(), ids.len() as u64);

        let ticks = std::sync::Mutex::new(Vec::new());
        let r = hash_pending(&pool, |done, total, _| {
            ticks.lock().unwrap().push((done, total))
        })
        .await
        .unwrap();
        assert_eq!((r.hashed, r.skipped), (ids.len() as u64, 0));
        for (id, contents) in &ids {
            let (hash, algo) = hash_of(&pool, *id).await;
            assert_eq!(
                hash,
                Some(blake3::hash(contents.as_bytes()).to_hex().to_string())
            );
            assert_eq!(algo.as_deref(), Some(HASH_ALGO));
        }
        assert_eq!(pending_count(&pool).await.unwrap(), 0);
        let ticks = ticks.into_inner().unwrap();
        assert_eq!(ticks.last(), Some(&(ids.len() as u64, ids.len() as u64)));
    }

    /// Killed mid-run, then restarted: rows already hashed are not read
    /// again (the NULL marker is the checkpoint), the rest are finished.
    #[tokio::test]
    async fn a_restarted_run_never_rehashes_finished_rows() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("t.db")).await.unwrap();
        let mut ids = Vec::new();
        for i in 0..10 {
            let p = dir.path().join(format!("{i}.flac"));
            std::fs::write(&p, format!("audio {i}")).unwrap();
            ids.push(pending_row(&pool, &p).await);
        }
        // The "previous run" got through the first four. A marker value
        // (not the real hash) shows whether they are ever rewritten.
        for id in &ids[..4] {
            sqlx::query(
                "UPDATE tracks SET hash = 'from-the-first-run', hash_algo = ? WHERE id = ?",
            )
            .bind(HASH_ALGO)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        }
        let r = hash_pending(&pool, |_, _, _| {}).await.unwrap();
        assert_eq!(r.hashed, 6, "only the unfinished rows");
        for id in &ids[..4] {
            assert_eq!(
                hash_of(&pool, *id).await.0.as_deref(),
                Some("from-the-first-run")
            );
        }
        for id in &ids[4..] {
            assert_eq!(hash_of(&pool, *id).await.0.map(|h| h.len()), Some(64));
        }
    }

    /// Files that changed since the scan, can't be read, or are marked
    /// missing stay pending; the run still finishes everything else.
    #[tokio::test]
    async fn changed_unreadable_and_missing_files_stay_pending() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("t.db")).await.unwrap();
        let ok = dir.path().join("ok.flac");
        let changed = dir.path().join("changed.flac");
        let gone = dir.path().join("gone.flac");
        let missing = dir.path().join("missing.flac");
        for p in [&ok, &changed, &gone, &missing] {
            std::fs::write(p, b"audio").unwrap();
        }
        let ok_id = pending_row(&pool, &ok).await;
        let changed_id = pending_row(&pool, &changed).await;
        let gone_id = pending_row(&pool, &gone).await;
        let missing_id = pending_row(&pool, &missing).await;
        std::fs::write(&changed, b"audio, re-encoded since the scan").unwrap();
        std::fs::remove_file(&gone).unwrap();
        sqlx::query("UPDATE tracks SET missing = 1 WHERE id = ?")
            .bind(missing_id)
            .execute(&pool)
            .await
            .unwrap();

        let r = hash_pending(&pool, |_, _, _| {}).await.unwrap();
        assert_eq!(
            (r.hashed, r.skipped),
            (1, 2),
            "the missing row is not even tried"
        );
        assert!(hash_of(&pool, ok_id).await.0.is_some());
        for id in [changed_id, gone_id, missing_id] {
            assert_eq!(hash_of(&pool, id).await, (None, None));
        }
    }
}
