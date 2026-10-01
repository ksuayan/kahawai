//! Durable job store for long-running tasks (ISO extraction, bulk
//! transcodes, library scans). (Spec §3.7, S9.)
//!
//! Jobs are server-owned: they keep running when the client disconnects, and
//! a reconnecting client picks up state via `GET /api/jobs`.
//!
//! Phase 5 (S9): every mutation is written through to the `jobs` table
//! (migration 003), so jobs survive server restarts. The in-memory map is
//! the live source of truth for reads; the table is the restart-recovery
//! backup. At startup [`JobStore::persistent`] fails any row still marked
//! `queued`/`running` with the error `"server restarted"` — partially
//! completed work is never silently resumed. The `Job.message` field maps
//! to the `result` column on success and the `error` column on failure.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};

use kahawai_core::{FileProgress, Job, JobKind, JobStatus};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

use crate::db;

/// Background progress ticker: marks the job Running, advances `progress` in
/// `steps` increments of `step`, then marks it Done.
///
/// Factored out of the HTTP handler so the lifecycle is unit-testable.
/// Real extract/transcode workers will replace the sleep loop with actual
/// progress callbacks — same state transitions.
pub async fn run_ticker(store: JobStore, id: String, steps: u32, step: f32, delay: Duration) {
    if !store.set_status(&id, JobStatus::Running).await {
        return; // job vanished; nothing to do
    }
    for _ in 0..steps {
        tokio::time::sleep(delay).await;
        if !store.bump(&id, step).await {
            return; // job removed or already terminal
        }
    }
    store.finish(&id, true, Some("complete".to_string())).await;
}

fn kind_to_str(kind: JobKind) -> &'static str {
    match kind {
        JobKind::ExtractIso => "extract_iso",
        JobKind::Transcode => "transcode",
        JobKind::Scan => "scan",
        JobKind::HashFiles => "hash_files",
        JobKind::EnrichMetadata => "enrich_metadata",
    }
}

fn kind_from_str(s: &str) -> Option<JobKind> {
    match s {
        "extract_iso" => Some(JobKind::ExtractIso),
        "transcode" => Some(JobKind::Transcode),
        "scan" => Some(JobKind::Scan),
        "hash_files" => Some(JobKind::HashFiles),
        "enrich_metadata" => Some(JobKind::EnrichMetadata),
        _ => None,
    }
}

fn status_to_str(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Queued => "queued",
        JobStatus::Running => "running",
        JobStatus::Done => "done",
        JobStatus::Failed => "failed",
        JobStatus::Paused => "paused",
        JobStatus::Cancelled => "cancelled",
    }
}

fn status_from_str(s: &str) -> Option<JobStatus> {
    match s {
        "queued" => Some(JobStatus::Queued),
        "running" => Some(JobStatus::Running),
        "done" => Some(JobStatus::Done),
        "failed" => Some(JobStatus::Failed),
        "paused" => Some(JobStatus::Paused),
        "cancelled" => Some(JobStatus::Cancelled),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct JobStore {
    inner: Arc<RwLock<HashMap<String, Job>>>,
    counter: Arc<AtomicU64>,
    /// When `Some`, every mutation is written through to the `jobs` table.
    pool: Option<SqlitePool>,
}

/// Now, in Unix milliseconds.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Record when a job starts and ends, as its status changes: the first move
/// to Running starts it (a resume keeps that start), an end (done, failed,
/// cancelled) finishes it. A job that ends without ever running (failed
/// while queued) starts and ends at once.
fn stamp(job: &mut Job, to: JobStatus) {
    let now = now_ms();
    match to {
        JobStatus::Running => {
            job.started_at.get_or_insert(now);
            job.finished_at = None;
        }
        JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled => {
            job.started_at.get_or_insert(now);
            job.finished_at = Some(now);
            job.files = None;
        }
        JobStatus::Queued | JobStatus::Paused => {}
    }
    job.status = to;
}

/// Turns a stream of `(done, total)` ticks into files per second and an ETA.
/// The rate is an exponential moving average over windows of at least a
/// second, so a slow patch on a network share doesn't make the ETA jump.
#[derive(Default)]
pub(crate) struct RateTracker {
    last: Option<(i64, u64, u64)>,
    rate: Option<f64>,
    byte_rate: Option<f64>,
}

impl RateTracker {
    /// How much a new window counts against the history.
    const NEW_WEIGHT: f64 = 0.3;
    const WINDOW_MS: i64 = 1000;

    fn smooth(old: Option<f64>, instant: f64) -> f64 {
        match old {
            None => instant,
            Some(r) => r * (1.0 - Self::NEW_WEIGHT) + instant * Self::NEW_WEIGHT,
        }
    }

    /// `bytes` is the content read so far, for jobs that count it.
    pub(crate) fn observe(
        &mut self,
        now: i64,
        done: u64,
        bytes: Option<u64>,
        total: Option<u64>,
    ) -> FileProgress {
        let b = bytes.unwrap_or(0);
        match self.last {
            None => self.last = Some((now, done, b)),
            Some((t, d, bt)) if now - t >= Self::WINDOW_MS => {
                let secs = (now - t) as f64 / 1000.0;
                self.rate = Some(Self::smooth(
                    self.rate,
                    done.saturating_sub(d) as f64 / secs,
                ));
                if bytes.is_some() {
                    let mbps = b.saturating_sub(bt) as f64 / 1e6 / secs;
                    self.byte_rate = Some(Self::smooth(self.byte_rate, mbps));
                }
                self.last = Some((now, done, b));
            }
            Some(_) => {}
        }
        let eta_at = match (total, self.rate) {
            (Some(total), Some(rate)) if rate > 0.05 => {
                let remaining = total.saturating_sub(done) as f64;
                Some(now + (remaining / rate * 1000.0) as i64)
            }
            _ => None,
        };
        FileProgress {
            done,
            total,
            per_sec: self.rate.map(|r| r as f32),
            mb_per_sec: self.byte_rate.map(|r| r as f32),
            eta_at,
        }
    }
}

impl JobStore {
    /// In-memory only: no durability. Used by tests and anywhere the
    /// database is unavailable.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            counter: Arc::new(AtomicU64::new(1)),
            pool: None,
        }
    }

    /// Durable store backed by the `jobs` table. Applies the S9 restart
    /// rule first: any persisted `queued`/`running` job becomes `failed`
    /// with the error `"server restarted"`. Completed and paused jobs load
    /// intact (a paused job's work is resumable, so resume carries on).
    /// Call after migrations have run.
    pub async fn persistent(pool: &SqlitePool) -> Result<Self, kahawai_core::MusicError> {
        sqlx::query(
            "UPDATE jobs SET status = 'failed', error = 'server restarted',
             result = NULL, updated_at = datetime('now'),
             started_at = COALESCE(started_at, ?), finished_at = ?
             WHERE status IN ('queued', 'running')",
        )
        .bind(now_ms())
        .bind(now_ms())
        .execute(pool)
        .await
        .map_err(db::cvt)?;

        let rows = sqlx::query(
            "SELECT id, kind, status, progress, payload, result, error, label,
                    started_at, finished_at
             FROM jobs ORDER BY id",
        )
        .fetch_all(pool)
        .await
        .map_err(db::cvt)?;

        let mut jobs = HashMap::with_capacity(rows.len());
        let mut max_n: u64 = 0;
        for r in &rows {
            let id: String = r.get("id");
            let kind: String = r.get("kind");
            let status: String = r.get("status");
            let (kind, status) = match (kind_from_str(&kind), status_from_str(&status)) {
                (Some(k), Some(s)) => (k, s),
                _ => {
                    tracing::warn!(job_id = %id, "skipping job row with unknown kind/status");
                    continue;
                }
            };
            if let Some(n) = id.strip_prefix("job-").and_then(|s| s.parse::<u64>().ok()) {
                max_n = max_n.max(n);
            }
            // message is result on success, error on failure.
            let message: Option<String> = if status == JobStatus::Failed {
                r.get("error")
            } else {
                r.get("result")
            };
            let job = Job {
                id: id.clone(),
                kind,
                label: r.get("label"),
                payload: r.get("payload"),
                progress: r.get::<f64, _>("progress") as f32,
                status,
                message,
                started_at: r.get("started_at"),
                finished_at: r.get("finished_at"),
                files: None,
            };
            jobs.insert(id, job);
        }
        tracing::info!(
            jobs = jobs.len(),
            recovered = max_n,
            "job store loaded; in-flight jobs failed per restart rule"
        );
        Ok(Self {
            inner: Arc::new(RwLock::new(jobs)),
            counter: Arc::new(AtomicU64::new(max_n + 1)),
            pool: Some(pool.clone()),
        })
    }

    /// Write the job row through to SQLite. A failure is logged but does not
    /// roll back the in-memory state: the map is the live source of truth for
    /// `GET /api/jobs`; the table is the restart-recovery backup.
    /// `Job.message` is stored in `result` on success and `error` on
    /// failure, per the migration 003 column contract.
    async fn write_through(&self, job: &Job) {
        let Some(pool) = &self.pool else { return };
        let (result, error) = match job.status {
            // A paused job's message (why it stopped) survives a restart too.
            JobStatus::Done | JobStatus::Paused | JobStatus::Cancelled => {
                (job.message.as_deref(), None)
            }
            JobStatus::Failed => (None, job.message.as_deref()),
            _ => (None, None),
        };
        let res = sqlx::query(
            "INSERT INTO jobs (id, kind, status, progress, payload, result, error, label,
                               started_at, finished_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
               kind = excluded.kind, status = excluded.status,
               progress = excluded.progress, payload = excluded.payload,
               result = excluded.result, error = excluded.error,
               label = excluded.label, started_at = excluded.started_at,
               finished_at = excluded.finished_at, updated_at = datetime('now')",
        )
        .bind(&job.id)
        .bind(kind_to_str(job.kind))
        .bind(status_to_str(job.status))
        .bind(job.progress as f64)
        .bind(&job.payload)
        .bind(result)
        .bind(error)
        .bind(&job.label)
        .bind(job.started_at)
        .bind(job.finished_at)
        .execute(pool)
        .await;
        if let Err(e) = res {
            tracing::warn!(job_id = %job.id, error = %e, "job write-through failed");
        }
    }

    /// Create a job in `Queued` state. IDs are `job-NNNN`. `payload` is the
    /// machine-readable job input (e.g. the ISO path), persisted verbatim.
    pub async fn create(&self, kind: JobKind, label: String, payload: Option<String>) -> Job {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let job = Job {
            id: format!("job-{n:04}"),
            kind,
            label,
            payload,
            progress: 0.0,
            status: JobStatus::Queued,
            message: None,
            started_at: None,
            finished_at: None,
            files: None,
        };
        self.inner
            .write()
            .unwrap()
            .insert(job.id.clone(), job.clone());
        self.write_through(&job).await;
        job
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        self.inner.read().unwrap().get(id).cloned()
    }

    pub fn list(&self) -> Vec<Job> {
        let mut jobs: Vec<Job> = self.inner.read().unwrap().values().cloned().collect();
        jobs.sort_by(|a, b| a.id.cmp(&b.id));
        jobs
    }

    /// Advance progress by `step`, clamped to 1.0. Returns false if the job
    /// is missing or already terminal.
    pub async fn bump(&self, id: &str, step: f32) -> bool {
        let job = {
            let mut inner = self.inner.write().unwrap();
            match inner.get_mut(id) {
                Some(job) if matches!(job.status, JobStatus::Queued | JobStatus::Running) => {
                    job.progress = (job.progress + step).min(1.0);
                    stamp(job, JobStatus::Running);
                    job.clone()
                }
                _ => return false,
            }
        };
        self.write_through(&job).await;
        true
    }

    /// Set progress absolutely, clamped to 0.0..=1.0. Used by workers that
    /// know files_processed / files_total (scan). Returns false if the job
    /// is missing or already terminal.
    pub async fn set_progress(&self, id: &str, progress: f32) -> bool {
        let job = {
            let mut inner = self.inner.write().unwrap();
            match inner.get_mut(id) {
                Some(job) if matches!(job.status, JobStatus::Queued | JobStatus::Running) => {
                    job.progress = progress.clamp(0.0, 1.0);
                    stamp(job, JobStatus::Running);
                    job.clone()
                }
                _ => return false,
            }
        };
        self.write_through(&job).await;
        true
    }

    pub async fn set_status(&self, id: &str, status: JobStatus) -> bool {
        let job = {
            let mut inner = self.inner.write().unwrap();
            match inner.get_mut(id) {
                Some(job) => {
                    stamp(job, status);
                    job.clone()
                }
                None => return false,
            }
        };
        self.write_through(&job).await;
        true
    }

    /// Record live file counts for a queued or running job. Memory only: it
    /// changes about once a second and means nothing after a restart.
    pub async fn set_files(&self, id: &str, files: FileProgress) -> bool {
        let mut inner = self.inner.write().unwrap();
        match inner.get_mut(id) {
            Some(job) if matches!(job.status, JobStatus::Queued | JobStatus::Running) => {
                job.files = Some(files);
                true
            }
            _ => false,
        }
    }

    /// Mark a job terminal. `ok=true` → Done, else Failed with `message`.
    pub async fn finish(&self, id: &str, ok: bool, message: Option<String>) -> bool {
        let job = {
            let mut inner = self.inner.write().unwrap();
            match inner.get_mut(id) {
                Some(job) => {
                    stamp(
                        job,
                        if ok {
                            JobStatus::Done
                        } else {
                            JobStatus::Failed
                        },
                    );
                    if ok {
                        job.progress = 1.0;
                    }
                    job.message = message;
                    job.clone()
                }
                None => return false,
            }
        };
        self.write_through(&job).await;
        true
    }

    /// Replace a job's message (shown with its status).
    pub async fn set_message(&self, id: &str, message: Option<String>) -> bool {
        let job = {
            let mut inner = self.inner.write().unwrap();
            let Some(job) = inner.get_mut(id) else {
                return false;
            };
            job.message = message;
            job.clone()
        };
        self.write_through(&job).await;
        true
    }

    /// Move a job to `to` if its status is one of `from`. Returns the
    /// updated job, or `None` when it is missing or in another state.
    pub async fn transition(&self, id: &str, from: &[JobStatus], to: JobStatus) -> Option<Job> {
        let job = {
            let mut inner = self.inner.write().unwrap();
            let job = inner.get_mut(id)?;
            if !from.contains(&job.status) {
                return None;
            }
            stamp(job, to);
            job.clone()
        };
        self.write_through(&job).await;
        Some(job)
    }
}

impl Default for JobStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_get_list() {
        let store = JobStore::new();
        let a = store
            .create(
                JobKind::ExtractIso,
                "ISO one".into(),
                Some("/music/x.iso".into()),
            )
            .await;
        let b = store
            .create(JobKind::Transcode, "transcode one".into(), None)
            .await;
        assert_eq!(a.status, JobStatus::Queued);
        assert_eq!(a.progress, 0.0);
        assert_ne!(a.id, b.id);

        assert_eq!(store.get(&a.id).unwrap().label, "ISO one");
        assert_eq!(
            store.get(&a.id).unwrap().payload.as_deref(),
            Some("/music/x.iso")
        );
        assert_eq!(store.get(&b.id).unwrap().payload, None);
        assert!(store.get("job-9999").is_none());

        let list = store.list();
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn bump_clamps_and_refuses_terminal_jobs() {
        let store = JobStore::new();
        let job = store.create(JobKind::Transcode, "t".into(), None).await;
        assert!(store.bump(&job.id, 0.7).await);
        let j = store.get(&job.id).unwrap();
        assert_eq!(j.status, JobStatus::Running);
        assert!((j.progress - 0.7).abs() < f32::EPSILON);

        assert!(store.bump(&job.id, 0.7).await);
        assert!((store.get(&job.id).unwrap().progress - 1.0).abs() < f32::EPSILON);

        store.finish(&job.id, true, None).await;
        assert!(!store.bump(&job.id, 0.1).await); // terminal: no more bumps
        assert!(!store.bump("missing", 0.1).await);
    }

    #[test]
    fn rate_tracker_smooths_and_estimates_the_finish() {
        let mut t = RateTracker::default();
        // The first tick only starts the clock.
        let p = t.observe(0, 0, None, Some(1000));
        assert_eq!((p.per_sec, p.eta_at), (None, None));
        // Under a second of data: still no rate.
        assert_eq!(t.observe(500, 50, None, Some(1000)).per_sec, None);
        // 100 files in the first full second: 100/s, 900 left → 9 s.
        let p = t.observe(1000, 100, None, Some(1000));
        assert_eq!(p.per_sec, Some(100.0));
        assert_eq!(p.eta_at, Some(1000 + 9000));
        // A slow second (10/s) moves the rate only part of the way: 0.7·100 + 0.3·10.
        let p = t.observe(2000, 110, None, Some(1000));
        assert!((p.per_sec.unwrap() - 73.0).abs() < 0.01);
        assert_eq!(p.done, 110);
    }

    #[test]
    fn rate_tracker_reports_megabytes_per_second_when_given_bytes() {
        let mut t = RateTracker::default();
        t.observe(0, 0, Some(0), Some(100));
        // 10 files, 200 MB in one second.
        let p = t.observe(1000, 10, Some(200_000_000), Some(100));
        assert_eq!(p.mb_per_sec, Some(200.0));
        // A slower second (50 MB) is smoothed: 0.7·200 + 0.3·50.
        let p = t.observe(2000, 12, Some(250_000_000), Some(100));
        assert!((p.mb_per_sec.unwrap() - 155.0).abs() < 0.01);
        // A scan passes no bytes and so gets no MB/s.
        let mut scan = RateTracker::default();
        scan.observe(0, 0, None, None);
        assert_eq!(scan.observe(1000, 9, None, None).mb_per_sec, None);
    }

    #[test]
    fn rate_tracker_has_no_eta_on_a_first_scan_or_past_the_estimate() {
        let mut t = RateTracker::default();
        t.observe(0, 0, None, None);
        let p = t.observe(1000, 50, None, None);
        assert_eq!(p.per_sec, Some(50.0), "the rate is still known");
        assert_eq!(
            (p.total, p.eta_at),
            (None, None),
            "but there is nothing to finish against"
        );
        // More files than the last scan had: remaining clamps to 0, ETA is now.
        let p = t.observe(2000, 150, None, Some(100));
        assert_eq!(p.eta_at, Some(2000));
    }

    #[tokio::test]
    async fn live_file_counts_are_for_running_jobs_only() {
        let store = JobStore::new();
        let job = store.create(JobKind::Scan, "scan".into(), None).await;
        let p = FileProgress {
            done: 3,
            total: None,
            mb_per_sec: None,
            per_sec: None,
            eta_at: None,
        };
        assert!(store.set_files(&job.id, p).await);
        assert_eq!(store.get(&job.id).unwrap().files, Some(p));
        store.finish(&job.id, true, None).await;
        assert_eq!(
            store.get(&job.id).unwrap().files,
            None,
            "cleared at the end"
        );
        assert!(
            !store.set_files(&job.id, p).await,
            "a finished job takes no more"
        );
    }

    #[tokio::test]
    async fn set_progress_is_absolute_and_clamped() {
        let store = JobStore::new();
        let job = store.create(JobKind::Scan, "scan".into(), None).await;
        assert!(store.set_progress(&job.id, 0.42).await);
        assert!((store.get(&job.id).unwrap().progress - 0.42).abs() < f32::EPSILON);
        assert!(store.set_progress(&job.id, 99.0).await);
        assert!((store.get(&job.id).unwrap().progress - 1.0).abs() < f32::EPSILON);
        assert!(!store.set_progress("missing", 0.5).await);
    }

    #[tokio::test]
    async fn finish_marks_done_or_failed() {
        let store = JobStore::new();
        let ok_job = store.create(JobKind::ExtractIso, "ok".into(), None).await;
        let bad_job = store.create(JobKind::ExtractIso, "bad".into(), None).await;

        assert!(
            store
                .finish(&ok_job.id, true, Some("3 tracks added".into()))
                .await
        );
        let j = store.get(&ok_job.id).unwrap();
        assert_eq!(j.status, JobStatus::Done);
        assert_eq!(j.progress, 1.0);
        assert_eq!(j.message.as_deref(), Some("3 tracks added"));

        assert!(
            store
                .finish(&bad_job.id, false, Some("sacd_extract failed".into()))
                .await
        );
        let j = store.get(&bad_job.id).unwrap();
        assert_eq!(j.status, JobStatus::Failed);
        assert_eq!(j.message.as_deref(), Some("sacd_extract failed"));

        assert!(!store.finish("missing", true, None).await);
    }

    #[tokio::test]
    async fn ticker_drives_queued_to_done() {
        let store = JobStore::new();
        let job = store
            .create(JobKind::ExtractIso, "ticker test".into(), None)
            .await;
        assert_eq!(job.status, JobStatus::Queued);

        run_ticker(
            store.clone(),
            job.id.clone(),
            4,
            0.25,
            Duration::from_millis(1),
        )
        .await;

        let j = store.get(&job.id).unwrap();
        assert_eq!(j.status, JobStatus::Done);
        assert!((j.progress - 1.0).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn ticker_on_missing_job_is_a_noop() {
        let store = JobStore::new();
        // Must not panic or hang.
        run_ticker(store, "job-4242".into(), 4, 0.25, Duration::from_millis(1)).await;
    }

    /// S9: mutations are written through to the `jobs` table, honoring the
    /// payload/result/error column contract.
    /// Start and end times: set when a job first runs and when it ends,
    /// kept through a pause and resume and across a restart; a job cut
    /// short by a restart ends then.
    #[tokio::test]
    async fn jobs_record_when_they_started_and_ended() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("jobs.db")).await.unwrap();
        let store = JobStore::persistent(&pool).await.unwrap();

        let job = store.create(JobKind::Scan, "scan".into(), None).await;
        assert_eq!((job.started_at, job.finished_at), (None, None), "queued");
        store.set_progress(&job.id, 0.1).await;
        let started = store.get(&job.id).unwrap().started_at.expect("running");
        store.set_progress(&job.id, 0.6).await;
        assert_eq!(
            store.get(&job.id).unwrap().started_at,
            Some(started),
            "only the first run starts it"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        store.finish(&job.id, false, Some("boom".into())).await;
        let failed = store.get(&job.id).unwrap();
        let ended = failed.finished_at.expect("ended");
        assert!(ended >= started + 5, "{started} .. {ended}");

        let paused = store
            .create(JobKind::EnrichMetadata, "lookup".into(), None)
            .await;
        store.set_status(&paused.id, JobStatus::Running).await;
        let p_start = store.get(&paused.id).unwrap().started_at;
        store
            .transition(&paused.id, &[JobStatus::Running], JobStatus::Paused)
            .await;
        store
            .transition(&paused.id, &[JobStatus::Paused], JobStatus::Running)
            .await;
        let resumed = store.get(&paused.id).unwrap();
        assert_eq!(
            (resumed.started_at, resumed.finished_at),
            (p_start, None),
            "a resume keeps the start"
        );

        let cut = store.create(JobKind::Scan, "scan".into(), None).await;
        store.set_progress(&cut.id, 0.3).await;
        let cut_start = store.get(&cut.id).unwrap().started_at;

        // Restart: the times come back from the table; the running scan
        // failed then.
        let again = JobStore::persistent(&pool).await.unwrap();
        let f = again.get(&job.id).unwrap();
        assert_eq!((f.started_at, f.finished_at), (Some(started), Some(ended)));
        let c = again.get(&cut.id).unwrap();
        assert_eq!(c.status, JobStatus::Failed);
        assert_eq!(c.started_at, cut_start);
        assert!(c.finished_at.unwrap() >= cut_start.unwrap());
    }

    #[tokio::test]
    async fn persistent_store_writes_through_to_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("jobs.db")).await.unwrap();
        let store = JobStore::persistent(&pool).await.unwrap();

        let job = store.create(JobKind::Scan, "scan".into(), None).await;
        assert_eq!(job.payload, None);
        store.set_progress(&job.id, 0.5).await;
        store.finish(&job.id, true, Some("done".into())).await;

        // Success message lands in `result`, `error` stays NULL.
        let row: (
            String,
            String,
            f64,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT status, kind, progress, payload, result, error FROM jobs WHERE id = ?",
        )
        .bind(&job.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "done");
        assert_eq!(row.1, "scan");
        assert!((row.2 - 1.0).abs() < 1e-9);
        assert_eq!(row.3, None);
        assert_eq!(row.4.as_deref(), Some("done"));
        assert_eq!(row.5, None);

        // Failure message lands in `error`; payload round-trips.
        let bad = store
            .create(JobKind::ExtractIso, "iso".into(), Some("/m/x.iso".into()))
            .await;
        store.finish(&bad.id, false, Some("boom".into())).await;
        let row: (String, Option<String>, Option<String>, Option<String>) =
            sqlx::query_as("SELECT status, payload, result, error FROM jobs WHERE id = ?")
                .bind(&bad.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "failed");
        assert_eq!(row.1.as_deref(), Some("/m/x.iso"));
        assert_eq!(row.2, None);
        assert_eq!(row.3.as_deref(), Some("boom"));
    }

    /// S9 restart rule: queued/running jobs fail with "server restarted";
    /// done jobs survive intact; IDs keep incrementing.
    #[tokio::test]
    async fn restart_fails_inflight_jobs_and_keeps_completed() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("jobs.db")).await.unwrap();

        let store = JobStore::persistent(&pool).await.unwrap();
        let running = store.create(JobKind::Scan, "scan".into(), None).await;
        store.set_status(&running.id, JobStatus::Running).await;
        let queued = store.create(JobKind::Transcode, "tc".into(), None).await;
        let done = store.create(JobKind::ExtractIso, "iso".into(), None).await;
        store.finish(&done.id, true, Some("ok".into())).await;
        drop(store);

        // Simulate a restart: a fresh store over the same database.
        let store2 = JobStore::persistent(&pool).await.unwrap();
        let r = store2.get(&running.id).unwrap();
        assert_eq!(r.status, JobStatus::Failed);
        assert_eq!(r.message.as_deref(), Some("server restarted"));
        let q = store2.get(&queued.id).unwrap();
        assert_eq!(q.status, JobStatus::Failed);
        assert_eq!(q.message.as_deref(), Some("server restarted"));
        let d = store2.get(&done.id).unwrap();
        assert_eq!(d.status, JobStatus::Done);
        assert_eq!(d.message.as_deref(), Some("ok"));

        // IDs continue past the recovered rows (no reuse).
        let next = store2.create(JobKind::Scan, "after".into(), None).await;
        assert_ne!(next.id, running.id);
        assert_ne!(next.id, queued.id);
        assert_ne!(next.id, done.id);
    }
}
