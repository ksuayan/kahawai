//! The `enrich_metadata` job: look up albums that have no MusicBrainz ID
//! (docs/v2/kahawai-metadata-enrichment-spec.md).
//!
//! - Only albums without an ID are looked up: embedded IDs (Picard) never
//!   cost a request. One search per album, never per track.
//! - A match fills blanks only: the ID, the year if unknown, the cover if
//!   there is none (Cover Art Archive). Tags always win.
//! - Below the confidence threshold an album is `no_match`, and is never
//!   looked up again. A failed request is `error`, retried on later runs up
//!   to [`MAX_ATTEMPTS`] times.
//! - Progress lives in the rows (each album is committed as it finishes), so
//!   a killed or paused run carries on where it stopped. Between albums the
//!   worker checks its job: paused waits, cancelled stops.

use std::time::Duration;

use kahawai_core::{JobStatus, MusicError};
use sqlx::{sqlite::SqlitePool, Row};
use tracing::{info, warn};

use crate::db;
use crate::jobs::JobStore;
use crate::musicbrainz::{best_match, unix_now, AlbumQuery, MbClient, MbError};
use crate::normalize::{current_year, sane_year};

/// Failed lookups per album before it is left alone.
pub const MAX_ATTEMPTS: i64 = 3;
const BATCH: i64 = 100;

/// Albums a run would look up (an SQL condition on `albums a`).
pub(crate) fn pending_clause() -> String {
    format!(
        "a.mbid IS NULL AND (a.enrich_status = 'pending'
         OR (a.enrich_status = 'error' AND a.enrich_attempts < {MAX_ATTEMPTS}))"
    )
}

#[derive(Debug, Default, PartialEq)]
pub struct EnrichReport {
    pub matched: u64,
    pub no_match: u64,
    pub errors: u64,
    pub covers: u64,
    /// The run stopped because its job was cancelled.
    pub cancelled: bool,
    /// The run paused itself because MusicBrainz couldn't be reached (no
    /// internet, or the service is down): why. No album was charged for it.
    pub paused_offline: Option<String>,
}

pub async fn pending_count(pool: &SqlitePool) -> Result<u64, MusicError> {
    let n: i64 = sqlx::query(&format!(
        "SELECT COUNT(*) FROM albums a WHERE {}",
        pending_clause()
    ))
    .fetch_one(pool)
    .await
    .map_err(db::cvt)?
    .get(0);
    Ok(n as u64)
}

/// What the job wants the worker to do next.
enum Next {
    Go,
    Stop,
}

/// Wait out a pause; `Stop` when the job was cancelled or has gone.
async fn next_step(jobs: &JobStore, job_id: &str) -> Next {
    loop {
        match jobs.get(job_id).map(|j| j.status) {
            Some(JobStatus::Paused) => tokio::time::sleep(Duration::from_millis(500)).await,
            Some(JobStatus::Queued | JobStatus::Running) => return Next::Go,
            _ => return Next::Stop,
        }
    }
}

/// Look up every pending album. `on_progress(done, total)` fires per album.
pub async fn enrich_pending(
    pool: &SqlitePool,
    client: &MbClient,
    min_confidence: f32,
    jobs: &JobStore,
    job_id: &str,
    on_progress: impl Fn(u64, u64) + Send + Sync,
) -> Result<EnrichReport, MusicError> {
    let total = pending_count(pool).await?;
    let mut report = EnrichReport::default();
    let mut done = 0u64;
    let mut after_id = 0i64;
    loop {
        let rows = sqlx::query(&format!(
            "SELECT a.id, a.title, a.artist, a.year, a.artwork_hash,
               (SELECT COUNT(*) FROM tracks t WHERE t.album_id = a.id AND t.missing = 0) AS n
             FROM albums a WHERE a.id > ? AND {} ORDER BY a.id LIMIT ?",
            pending_clause()
        ))
        .bind(after_id)
        .bind(BATCH)
        .fetch_all(pool)
        .await
        .map_err(db::cvt)?;
        if rows.is_empty() {
            break;
        }
        for r in &rows {
            if let Next::Stop = next_step(jobs, job_id).await {
                report.cancelled = true;
                return Ok(report);
            }
            let id: i64 = r.get("id");
            after_id = id;
            let q = AlbumQuery {
                title: r.get("title"),
                artist: r.get("artist"),
                track_count: r.get::<i64, _>("n").max(0) as u32,
            };
            let has_year = r.get::<Option<i64>, _>("year").is_some();
            let has_art = r.get::<Option<String>, _>("artwork_hash").is_some();
            match client.search_releases(&q).await {
                Ok(candidates) => match best_match(&q, &candidates, min_confidence) {
                    Some((c, conf)) => {
                        let year = (!has_year)
                            .then(|| c.date.get(..4).and_then(|y| y.parse::<u16>().ok()))
                            .flatten();
                        let year = sane_year(year, current_year());
                        sqlx::query(
                            "UPDATE albums SET mbid = ?, enrich_status = 'matched',
                               enrich_source = 'musicbrainz', enriched_at = ?,
                               year = COALESCE(year, ?)
                             WHERE id = ? AND mbid IS NULL",
                        )
                        .bind(&c.mbid)
                        .bind(unix_now())
                        .bind(year.map(i64::from))
                        .bind(id)
                        .execute(pool)
                        .await
                        .map_err(db::cvt)?;
                        report.matched += 1;
                        info!(album = %q.title, mbid = %c.mbid, confidence = conf, "matched on MusicBrainz");
                        if !has_art && attach_cover(pool, client, id, &c.mbid).await {
                            report.covers += 1;
                        }
                    }
                    None => {
                        sqlx::query("UPDATE albums SET enrich_status = 'no_match', enriched_at = ? WHERE id = ?")
                            .bind(unix_now())
                            .bind(id)
                            .execute(pool)
                            .await
                            .map_err(db::cvt)?;
                        report.no_match += 1;
                    }
                },
                Err(MbError::Unreachable(why)) => {
                    // Not this album's fault: leave it pending, and pause the
                    // job so it can carry on from here when the network is back.
                    let left = total.saturating_sub(done);
                    let albums = if left == 1 { "album" } else { "albums" };
                    let msg = format!(
                        "Paused: {why}. Nothing was lost; {left} {albums} still to look up. \
                         Resume to try again (no internet connection, or MusicBrainz is down)."
                    );
                    warn!(%why, left, "MusicBrainz unreachable; pausing enrichment");
                    jobs.transition(
                        job_id,
                        &[JobStatus::Queued, JobStatus::Running],
                        JobStatus::Paused,
                    )
                    .await;
                    jobs.set_message(job_id, Some(msg.clone())).await;
                    report.paused_offline = Some(msg);
                    return Ok(report);
                }
                Err(MbError::Db(e)) => return Err(e),
                Err(MbError::Failed(e)) => {
                    warn!(album = %q.title, error = %e, "MusicBrainz lookup failed; will retry on a later run");
                    sqlx::query(
                        "UPDATE albums SET enrich_status = 'error',
                           enrich_attempts = enrich_attempts + 1, enriched_at = ?
                         WHERE id = ?",
                    )
                    .bind(unix_now())
                    .bind(id)
                    .execute(pool)
                    .await
                    .map_err(db::cvt)?;
                    report.errors += 1;
                }
            }
            done += 1;
            on_progress(done, total);
        }
    }
    info!(?report, "metadata enrichment finished");
    Ok(report)
}

/// Fetch the release's front cover and attach it if the album still has
/// none. Failures are logged, not fatal: the match itself stands.
async fn attach_cover(pool: &SqlitePool, client: &MbClient, album_id: i64, mbid: &str) -> bool {
    let bytes = match client.front_cover(mbid).await {
        Ok(Some(b)) => b,
        Ok(None) => return false,
        Err(e) => {
            warn!(%mbid, error = %e, "Cover Art Archive fetch failed");
            return false;
        }
    };
    let Some(mime) = image_mime(&bytes) else {
        warn!(%mbid, "Cover Art Archive returned something that isn't an image");
        return false;
    };
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let stored = async {
        sqlx::query("INSERT OR IGNORE INTO artwork (hash, mime, bytes) VALUES (?, ?, ?)")
            .bind(&hash)
            .bind(mime)
            .bind(&bytes)
            .execute(pool)
            .await?;
        sqlx::query(
            "UPDATE albums SET artwork_hash = ?, artwork_source = 'caa'
             WHERE id = ? AND artwork_hash IS NULL",
        )
        .bind(&hash)
        .bind(album_id)
        .execute(pool)
        .await
    }
    .await;
    match stored {
        Ok(r) => r.rows_affected() == 1,
        Err(e) => {
            warn!(%mbid, error = %e, "could not store cover");
            false
        }
    }
}

fn image_mime(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if b.starts_with(b"GIF8") {
        Some("image/gif")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::musicbrainz::stub::{self, Mode};
    use kahawai_core::JobKind;

    async fn setup() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open(&dir.path().join("t.db")).await.unwrap();
        (dir, pool)
    }

    /// An album with `tracks` present tracks; returns its id.
    async fn album(
        pool: &SqlitePool,
        title: &str,
        artist: &str,
        year: Option<i64>,
        art: Option<&str>,
        tracks: u32,
    ) -> i64 {
        let id: i64 = sqlx::query(
            "INSERT INTO albums (title, artist, year, artwork_hash) VALUES (?, ?, ?, ?) RETURNING id",
        )
        .bind(title)
        .bind(artist)
        .bind(year)
        .bind(art)
        .fetch_one(pool)
        .await
        .unwrap()
        .get(0);
        for n in 0..tracks {
            sqlx::query("INSERT INTO tracks (path, format, album_id) VALUES (?, 'flac', ?)")
                .bind(format!("/m/{title}/{n}.flac"))
                .bind(id)
                .execute(pool)
                .await
                .unwrap();
        }
        id
    }

    async fn running_job(jobs: &JobStore) -> String {
        let job = jobs
            .create(JobKind::EnrichMetadata, "lookup".into(), None)
            .await;
        jobs.set_status(&job.id, JobStatus::Running).await;
        job.id
    }

    async fn row(
        pool: &SqlitePool,
        id: i64,
    ) -> (
        Option<String>,
        String,
        Option<String>,
        Option<i64>,
        Option<String>,
        i64,
    ) {
        sqlx::query_as(
            "SELECT mbid, enrich_status, enrich_source, year, artwork_source, enrich_attempts FROM albums WHERE id = ?",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    fn time_out() -> serde_json::Value {
        serde_json::json!([stub::release(
            "r1",
            "Time Out",
            "Dave Brubeck",
            "1959-12-14",
            7,
            100
        )])
    }

    /// A confident match fills only the blanks (ID, year, cover); an album
    /// that doesn't match is no_match; an album with an embedded ID costs
    /// no request; nothing already settled is looked up again.
    #[tokio::test]
    async fn matches_fill_blanks_and_settled_albums_are_never_looked_up_again() {
        let (_d, pool) = setup().await;
        let (stub, base) = stub::start(time_out()).await;
        let client = stub::client(pool.clone(), &base, 10);
        let jobs = JobStore::new();
        let brubeck = album(&pool, "Time Out", "Dave Brubeck", None, None, 7).await;
        let davis = album(
            &pool,
            "Kind of Blue",
            "Miles Davis",
            Some(1958),
            Some("abc"),
            5,
        )
        .await;
        let picard = album(&pool, "Giant Steps", "John Coltrane", None, None, 7).await;
        sqlx::query("UPDATE albums SET mbid = 'embedded-id', enrich_status = 'matched', enrich_source = 'embedded' WHERE id = ?")
            .bind(picard)
            .execute(&pool)
            .await
            .unwrap();

        let job = running_job(&jobs).await;
        let r = enrich_pending(&pool, &client, 0.9, &jobs, &job, |_, _| {})
            .await
            .unwrap();
        assert_eq!((r.matched, r.no_match, r.errors, r.covers), (1, 1, 0, 1));
        assert_eq!(stub.search_hits(), 2, "the embedded-ID album cost nothing");

        let (mbid, status, source, year, art, _) = row(&pool, brubeck).await;
        assert_eq!(
            (mbid.as_deref(), status.as_str(), source.as_deref()),
            (Some("r1"), "matched", Some("musicbrainz"))
        );
        assert_eq!(
            (year, art.as_deref()),
            (Some(1959), Some("caa")),
            "blank year and cover filled"
        );
        let (mbid, status, _, year, _, _) = row(&pool, davis).await;
        assert_eq!(
            (mbid, status.as_str(), year),
            (None, "no_match", Some(1958)),
            "tags untouched"
        );

        let job = running_job(&jobs).await;
        let r = enrich_pending(&pool, &client, 0.9, &jobs, &job, |_, _| {})
            .await
            .unwrap();
        assert_eq!(r, EnrichReport::default());
        assert_eq!(
            stub.search_hits(),
            2,
            "matched and no_match albums aren't asked about again"
        );
    }

    /// No internet (or MusicBrainz down): the job pauses itself with a clear
    /// message, and no album is charged an attempt.
    #[tokio::test]
    async fn an_outage_pauses_the_job_without_charging_the_albums() {
        let (_d, pool) = setup().await;
        let id = album(&pool, "Time Out", "Dave Brubeck", None, None, 7).await;
        let jobs = JobStore::new();
        let job = running_job(&jobs).await;
        let offline = stub::client(pool.clone(), "http://127.0.0.1:1", 10);
        let r = enrich_pending(&pool, &offline, 0.9, &jobs, &job, |_, _| {})
            .await
            .unwrap();
        assert!(r.paused_offline.is_some());
        let j = jobs.get(&job).unwrap();
        assert_eq!(j.status, JobStatus::Paused);
        assert!(j
            .message
            .unwrap()
            .contains("Nothing was lost; 1 album still to look up"));
        let (_, status, _, _, _, attempts) = row(&pool, id).await;
        assert_eq!((status.as_str(), attempts), ("pending", 0));

        // Back online: resuming finishes the job.
        let (_stub, base) = stub::start(time_out()).await;
        jobs.set_status(&job, JobStatus::Running).await;
        let r = enrich_pending(
            &pool,
            &stub::client(pool.clone(), &base, 10),
            0.9,
            &jobs,
            &job,
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(r.matched, 1);
    }

    /// A request that fails for the album's own reasons counts an attempt;
    /// after MAX_ATTEMPTS the album is left alone.
    #[tokio::test]
    async fn failed_lookups_are_retried_a_few_times_then_left_alone() {
        let (_d, pool) = setup().await;
        let id = album(&pool, "Time Out", "Dave Brubeck", None, None, 7).await;
        let (stub, base) = stub::start(time_out()).await;
        *stub.mode.lock().unwrap() = Mode::Bad;
        let client = stub::client(pool.clone(), &base, 10);
        let jobs = JobStore::new();
        for _ in 0..MAX_ATTEMPTS + 1 {
            let job = running_job(&jobs).await;
            enrich_pending(&pool, &client, 0.9, &jobs, &job, |_, _| {})
                .await
                .unwrap();
        }
        assert_eq!(stub.search_hits(), MAX_ATTEMPTS as usize);
        let (_, status, _, _, _, attempts) = row(&pool, id).await;
        assert_eq!((status.as_str(), attempts), ("error", MAX_ATTEMPTS));
    }

    /// Paused: the worker waits and sends nothing; resumed, it carries on.
    /// Cancelled: it stops.
    #[tokio::test]
    async fn pause_waits_resume_carries_on_cancel_stops() {
        let (_d, pool) = setup().await;
        album(&pool, "Time Out", "Dave Brubeck", None, None, 7).await;
        let (stub, base) = stub::start(time_out()).await;
        let jobs = JobStore::new();
        let job = running_job(&jobs).await;
        jobs.set_status(&job, JobStatus::Paused).await;
        let (pool2, jobs2, job2, base2) = (pool.clone(), jobs.clone(), job.clone(), base.clone());
        let run = tokio::spawn(async move {
            let client = stub::client(pool2.clone(), &base2, 10);
            enrich_pending(&pool2, &client, 0.9, &jobs2, &job2, |_, _| {}).await
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(stub.search_hits(), 0, "paused: nothing sent");
        jobs.set_status(&job, JobStatus::Running).await;
        assert_eq!(run.await.unwrap().unwrap().matched, 1);

        album(&pool, "Kind of Blue", "Miles Davis", None, None, 5).await;
        let job = running_job(&jobs).await;
        jobs.set_status(&job, JobStatus::Cancelled).await;
        let r = enrich_pending(
            &pool,
            &stub::client(pool.clone(), &base, 10),
            0.9,
            &jobs,
            &job,
            |_, _| {},
        )
        .await
        .unwrap();
        assert!(r.cancelled);
        assert_eq!(stub.search_hits(), 1, "cancelled: nothing more sent");
    }
}
