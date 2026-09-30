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

use kahawai_core::{Job, JobKind, JobStatus};
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
             result = NULL, updated_at = datetime('now')
             WHERE status IN ('queued', 'running')",
        )
        .execute(pool)
        .await
        .map_err(db::cvt)?;

        let rows = sqlx::query(
            "SELECT id, kind, status, progress, payload, result, error, label
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
            "INSERT INTO jobs (id, kind, status, progress, payload, result, error, label, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
               kind = excluded.kind, status = excluded.status,
               progress = excluded.progress, payload = excluded.payload,
               result = excluded.result, error = excluded.error,
               label = excluded.label, updated_at = datetime('now')",
        )
        .bind(&job.id)
        .bind(kind_to_str(job.kind))
        .bind(status_to_str(job.status))
        .bind(job.progress as f64)
        .bind(&job.payload)
        .bind(result)
        .bind(error)
        .bind(&job.label)
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
                    job.status = JobStatus::Running;
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
                    job.status = JobStatus::Running;
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
                    job.status = status;
                    job.clone()
                }
                None => return false,
            }
        };
        self.write_through(&job).await;
        true
    }

    /// Mark a job terminal. `ok=true` → Done, else Failed with `message`.
    pub async fn finish(&self, id: &str, ok: bool, message: Option<String>) -> bool {
        let job = {
            let mut inner = self.inner.write().unwrap();
            match inner.get_mut(id) {
                Some(job) => {
                    job.status = if ok {
                        JobStatus::Done
                    } else {
                        JobStatus::Failed
                    };
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
            job.status = to;
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
