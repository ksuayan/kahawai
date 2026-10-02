//! Downloading podcast episodes, and keeping the folder tidy
//! (docs/v2/kahawai-podcast-spec.md, D3).
//!
//! The server downloads, so one copy serves every Player and the server stays
//! the sync point. A download resumes where it stopped (HTTP Range, into a
//! `.part` file), runs as a job so it shows up like a scan does, and at most
//! two run at once. The newest few unplayed episodes of a feed are fetched
//! automatically, and played ones are cleared away after a while.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use kahawai_core::{JobKind, JobStatus, MusicError, ServerConfig};
use sqlx::{sqlite::SqlitePool, Row};
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;

use crate::db::cvt;
use crate::musicbrainz::USER_AGENT;
use crate::podcasts::now_ms;
use crate::AppState;

/// Downloads running at the same time.
const PARALLEL: usize = 2;

fn gate() -> &'static Semaphore {
    static G: OnceLock<Semaphore> = OnceLock::new();
    G.get_or_init(|| Semaphore::new(PARALLEL))
}

// ---------------------------------------------------------------------------
// Where files go (pure)
// ---------------------------------------------------------------------------

/// The podcast folder: the configured one, else `podcasts/` next to the database.
pub fn podcast_root(cfg: &ServerConfig) -> PathBuf {
    cfg.podcast_dir.clone().unwrap_or_else(|| {
        cfg.db_path
            .parent()
            .map(|p| p.join("podcasts"))
            .unwrap_or_else(|| PathBuf::from("podcasts"))
    })
}

/// A name that is safe as a file or folder name on macOS, Linux and Windows,
/// and short enough not to hit a limit.
pub fn safe_name(s: &str, max_chars: usize) -> String {
    let mut out: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let out: String = out.chars().take(max_chars).collect();
    let out = out.trim().trim_matches('.').trim().to_string();
    if out.is_empty() {
        "untitled".into()
    } else {
        out
    }
}

/// `YYYY-MM-DD` (UTC) for a Unix time in milliseconds.
pub fn utc_date(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// File extension for an enclosure: from its MIME type, else its address.
pub fn extension_for(mime: Option<&str>, url: &str) -> &'static str {
    let m = mime.unwrap_or("").to_ascii_lowercase();
    let by_mime = if m.contains("mpeg") || m.contains("mp3") {
        Some("mp3")
    } else if m.contains("mp4") || m.contains("m4a") || m.contains("x-m4a") {
        Some("m4a")
    } else if m.contains("aac") {
        Some("aac")
    } else if m.contains("opus") {
        Some("opus")
    } else if m.contains("ogg") || m.contains("vorbis") {
        Some("ogg")
    } else if m.contains("flac") {
        Some("flac")
    } else if m.contains("wav") {
        Some("wav")
    } else {
        None
    };
    if let Some(e) = by_mime {
        return e;
    }
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    for (suffix, e) in [
        (".mp3", "mp3"),
        (".m4a", "m4a"),
        (".aac", "aac"),
        (".opus", "opus"),
        (".ogg", "ogg"),
        (".flac", "flac"),
        (".wav", "wav"),
    ] {
        if path.ends_with(suffix) {
            return e;
        }
    }
    "mp3"
}

/// `<root>/Show/2026-10-02 - Episode title.mp3`
pub fn episode_path(
    root: &Path,
    show: &str,
    published_ms: Option<i64>,
    title: &str,
    ext: &str,
) -> PathBuf {
    let date = published_ms
        .map(utc_date)
        .unwrap_or_else(|| "undated".into());
    root.join(safe_name(show, 80))
        .join(format!("{date} - {}.{ext}", safe_name(title, 100)))
}

// ---------------------------------------------------------------------------
// Downloading
// ---------------------------------------------------------------------------

struct EpisodeInfo {
    feed_id: i64,
    show: String,
    title: String,
    published_at: Option<i64>,
    url: String,
    mime: Option<String>,
    file_path: Option<String>,
}

async fn episode_info(pool: &SqlitePool, id: i64) -> Result<EpisodeInfo, MusicError> {
    let r = sqlx::query(
        "SELECT e.feed_id, f.title AS show, e.title, e.published_at, e.enclosure_url, e.enclosure_type, e.file_path
         FROM podcast_episodes e JOIN podcast_feeds f ON f.id = e.feed_id WHERE e.id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(cvt)?
    .ok_or_else(|| MusicError::NotFound(format!("episode {id}")))?;
    Ok(EpisodeInfo {
        feed_id: r.get("feed_id"),
        show: r.get("show"),
        title: r.get("title"),
        published_at: r.get("published_at"),
        url: r.get("enclosure_url"),
        mime: r.get("enclosure_type"),
        file_path: r.get("file_path"),
    })
}

fn cancelled(state: &AppState, job_id: &str) -> bool {
    state
        .jobs
        .get(job_id)
        .is_some_and(|j| j.status == JobStatus::Cancelled)
}

/// Download one episode into the podcast folder. Returns the file's path.
/// Resumes a `.part` file left by an earlier try. Reports progress to the job.
pub async fn download_episode(
    state: &AppState,
    id: i64,
    job_id: &str,
) -> Result<PathBuf, MusicError> {
    let info = episode_info(&state.pool, id).await?;
    if let Some(p) = &info.file_path {
        if Path::new(p).is_file() {
            return Ok(PathBuf::from(p));
        }
    }
    let root = podcast_root(&state.config.read().unwrap());
    let ext = extension_for(info.mime.as_deref(), &info.url);
    let mut target = episode_path(&root, &info.show, info.published_at, &info.title, ext);
    // Two episodes with the same date and title must not share a file.
    let taken: Option<i64> =
        sqlx::query("SELECT id FROM podcast_episodes WHERE file_path = ? AND id != ?")
            .bind(target.to_string_lossy().to_string())
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(cvt)?
            .map(|r| r.get(0));
    if taken.is_some() {
        target.set_extension(format!("{id}.{ext}"));
    }
    if let Some(dir) = target.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| {
            MusicError::BadRequest(format!(
                "cannot use the podcast folder {}: {e}",
                root.display()
            ))
        })?;
    }
    let part = PathBuf::from(format!("{}.part", target.display()));

    let _slot = gate().acquire().await.expect("download gate open");
    state.jobs.set_status(job_id, JobStatus::Running).await;

    let have = tokio::fs::metadata(&part)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| MusicError::Http(e.to_string()))?;
    let mut req = client.get(&info.url);
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let mut res = req
        .send()
        .await
        .map_err(|e| MusicError::Http(format!("could not reach the episode: {e}")))?;
    let status = res.status();
    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        // The partial file is no use (the file changed or was complete): start over.
        let _ = tokio::fs::remove_file(&part).await;
        return Err(MusicError::Http(
            "the partial download did not fit the file; it will start again next try".into(),
        ));
    }
    if !status.is_success() {
        return Err(MusicError::Http(format!(
            "the episode's address answered HTTP {status}"
        )));
    }
    let ctype = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ctype.starts_with("text/html") {
        return Err(MusicError::Http(
            "the episode's address opened a web page, not audio".into(),
        ));
    }
    let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT && have > 0;
    let remaining = res.content_length();
    let total = remaining.map(|r| r + if resumed { have } else { 0 });
    let mut file = if resumed {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .await
            .map_err(MusicError::Io)?
    } else {
        tokio::fs::File::create(&part)
            .await
            .map_err(MusicError::Io)?
    };
    let mut written = if resumed { have } else { 0 };
    let mut last_report = std::time::Instant::now();
    while let Some(chunk) = res
        .chunk()
        .await
        .map_err(|e| MusicError::Http(format!("the download broke off: {e}")))?
    {
        file.write_all(&chunk).await.map_err(MusicError::Io)?;
        written += chunk.len() as u64;
        if last_report.elapsed() >= Duration::from_millis(300) {
            last_report = std::time::Instant::now();
            if cancelled(state, job_id) {
                drop(file);
                let _ = tokio::fs::remove_file(&part).await;
                return Err(MusicError::Conflict("cancelled".into()));
            }
            if let Some(t) = total.filter(|t| *t > 0) {
                state
                    .jobs
                    .set_progress(job_id, (written as f64 / t as f64).clamp(0.0, 0.99) as f32)
                    .await;
            }
        }
    }
    file.flush().await.map_err(MusicError::Io)?;
    drop(file);
    if let Some(t) = total {
        if written < t {
            return Err(MusicError::Http(format!(
                "the download ended early ({written} of {t} bytes); it will resume next try"
            )));
        }
    }
    tokio::fs::rename(&part, &target)
        .await
        .map_err(MusicError::Io)?;
    sqlx::query("UPDATE podcast_episodes SET file_path = ?, downloaded_at = ? WHERE id = ?")
        .bind(target.to_string_lossy().to_string())
        .bind(now_ms())
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(cvt)?;
    Ok(target)
}

/// Queue a download as a job and run it in the background. The job's payload
/// is the episode id, which is how a download is found again to cancel it.
pub async fn spawn_download(state: AppState, id: i64) -> Result<kahawai_core::Job, MusicError> {
    let info = episode_info(&state.pool, id).await?;
    if info
        .file_path
        .as_deref()
        .is_some_and(|p| Path::new(p).is_file())
    {
        return Err(MusicError::Conflict(
            "that episode is already downloaded".into(),
        ));
    }
    if active_job(&state, id).is_some() {
        return Err(MusicError::Conflict(
            "that episode is already downloading".into(),
        ));
    }
    let job = state
        .jobs
        .create(
            JobKind::PodcastDownload,
            format!("Download: {}", info.title),
            Some(id.to_string()),
        )
        .await;
    let job_id = job.id.clone();
    let feed_id = info.feed_id;
    tokio::spawn(async move {
        match download_episode(&state, id, &job_id).await {
            Ok(path) => {
                state
                    .jobs
                    .finish(&job_id, true, Some(format!("saved {}", path.display())))
                    .await;
                let _ = enforce_keep(&state.pool, feed_id).await;
            }
            Err(MusicError::Conflict(m)) if m == "cancelled" => {}
            Err(e) => {
                tracing::warn!(episode = id, error = %e, "podcast download failed");
                state.jobs.finish(&job_id, false, Some(e.to_string())).await;
            }
        }
    });
    Ok(job)
}

/// The queued or running download of an episode, if any.
pub fn active_job(state: &AppState, episode_id: i64) -> Option<kahawai_core::Job> {
    let want = episode_id.to_string();
    state.jobs.list().into_iter().find(|j| {
        j.kind == JobKind::PodcastDownload
            && j.payload.as_deref() == Some(want.as_str())
            && matches!(j.status, JobStatus::Queued | JobStatus::Running)
    })
}

/// Cancel a download in progress and delete the file (or the partial one).
pub async fn remove_download(state: &AppState, id: i64) -> Result<(), MusicError> {
    let info = episode_info(&state.pool, id).await?;
    if let Some(j) = active_job(state, id) {
        state
            .jobs
            .transition(
                &j.id,
                &[JobStatus::Queued, JobStatus::Running],
                JobStatus::Cancelled,
            )
            .await;
    }
    if let Some(p) = &info.file_path {
        delete_file_and_empty_dir(Path::new(p)).await;
    }
    sqlx::query("UPDATE podcast_episodes SET file_path = NULL, downloaded_at = NULL WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(cvt)?;
    Ok(())
}

async fn delete_file_and_empty_dir(path: &Path) {
    let _ = tokio::fs::remove_file(path).await;
    let _ = tokio::fs::remove_file(format!("{}.part", path.display())).await;
    if let Some(dir) = path.parent() {
        // Only removes an empty folder; anything left in it keeps it.
        let _ = tokio::fs::remove_dir(dir).await;
    }
}

// ---------------------------------------------------------------------------
// Keeping the folder tidy
// ---------------------------------------------------------------------------

/// Keep at most `keep_n` downloaded episodes of a feed (when it is set to
/// download automatically): played ones go first, oldest first, then the
/// oldest unplayed. Episodes are never deleted from the catalog, only their
/// files.
pub async fn enforce_keep(pool: &SqlitePool, feed_id: i64) -> Result<usize, MusicError> {
    let Some(feed) = sqlx::query("SELECT keep_n, auto_download FROM podcast_feeds WHERE id = ?")
        .bind(feed_id)
        .fetch_optional(pool)
        .await
        .map_err(cvt)?
    else {
        return Ok(0);
    };
    if feed.get::<i64, _>("auto_download") == 0 {
        return Ok(0);
    }
    let keep_n: i64 = feed.get("keep_n");
    let rows = sqlx::query(
        "SELECT id, file_path FROM podcast_episodes
         WHERE feed_id = ? AND file_path IS NOT NULL
         ORDER BY (played_at IS NULL), COALESCE(published_at, 0), id",
    )
    .bind(feed_id)
    .fetch_all(pool)
    .await
    .map_err(cvt)?;
    let excess = rows.len().saturating_sub(keep_n.max(1) as usize);
    for r in rows.iter().take(excess) {
        let (id, path): (i64, String) = (r.get(0), r.get(1));
        delete_file_and_empty_dir(Path::new(&path)).await;
        sqlx::query(
            "UPDATE podcast_episodes SET file_path = NULL, downloaded_at = NULL WHERE id = ?",
        )
        .bind(id)
        .execute(pool)
        .await
        .map_err(cvt)?;
    }
    Ok(excess)
}

/// Delete the files of episodes played more than `delete_played_after_days`
/// days ago (0 = never). The episode and its place stay.
pub async fn janitor(pool: &SqlitePool, now: i64) -> Result<usize, MusicError> {
    let rows = sqlx::query(
        "SELECT e.id, e.file_path FROM podcast_episodes e JOIN podcast_feeds f ON f.id = e.feed_id
         WHERE e.file_path IS NOT NULL AND e.played_at IS NOT NULL
           AND f.delete_played_after_days > 0
           AND e.played_at < ? - f.delete_played_after_days * 86400000",
    )
    .bind(now)
    .fetch_all(pool)
    .await
    .map_err(cvt)?;
    for r in &rows {
        let (id, path): (i64, String) = (r.get(0), r.get(1));
        delete_file_and_empty_dir(Path::new(&path)).await;
        sqlx::query(
            "UPDATE podcast_episodes SET file_path = NULL, downloaded_at = NULL WHERE id = ?",
        )
        .bind(id)
        .execute(pool)
        .await
        .map_err(cvt)?;
    }
    Ok(rows.len())
}

/// Download the newest `keep_n` unplayed episodes of a feed that are not on
/// disk yet (when the feed is set to download automatically). Returns how many
/// downloads were queued.
pub async fn auto_download(state: &AppState, feed_id: i64) -> Result<usize, MusicError> {
    let Some(feed) = sqlx::query("SELECT keep_n, auto_download FROM podcast_feeds WHERE id = ?")
        .bind(feed_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(cvt)?
    else {
        return Ok(0);
    };
    if feed.get::<i64, _>("auto_download") == 0 {
        return Ok(0);
    }
    let keep_n: i64 = feed.get("keep_n");
    let wanted: Vec<i64> = sqlx::query(
        "SELECT id FROM podcast_episodes
         WHERE feed_id = ? AND played_at IS NULL AND dropped_from_feed = 0
         ORDER BY COALESCE(published_at, 0) DESC, id DESC LIMIT ?",
    )
    .bind(feed_id)
    .bind(keep_n.max(1))
    .fetch_all(&state.pool)
    .await
    .map_err(cvt)?
    .iter()
    .map(|r| r.get(0))
    .collect();
    let mut queued = 0;
    for id in wanted {
        let have: Option<String> =
            sqlx::query("SELECT file_path FROM podcast_episodes WHERE id = ?")
                .bind(id)
                .fetch_one(&state.pool)
                .await
                .map_err(cvt)?
                .get(0);
        if have.is_some_and(|p| Path::new(&p).is_file()) {
            continue;
        }
        match spawn_download(state.clone(), id).await {
            Ok(_) => queued += 1,
            Err(MusicError::Conflict(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(queued)
}

/// One pass of the background work: read every feed (a few seconds apart, so
/// the hosts are not all hit at once), fetch what is new, and tidy up.
pub async fn run_pass(state: &AppState, gap: Duration) {
    let feeds = match crate::podcasts::list_feeds(&state.pool).await {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = %e, "podcast pass: could not list feeds");
            return;
        }
    };
    for (i, f) in feeds.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(gap).await;
        }
        match crate::podcasts::refresh_feed(&state.pool, f.id).await {
            Ok(r) if r.error.is_some() => {
                tracing::info!(feed = %f.title, error = ?r.error, "podcast refresh failed")
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(feed = %f.title, error = %e, "podcast refresh error"),
        }
        if let Err(e) = auto_download(state, f.id).await {
            tracing::warn!(feed = %f.title, error = %e, "podcast auto-download error");
        }
    }
    let _ = janitor(&state.pool, now_ms()).await;
}

/// The background loop: a pass shortly after start, then every
/// `podcast_refresh_hours` (read each time, so the setting applies live; 0
/// pauses it).
pub fn spawn_scheduler(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            let hours = state.config.read().unwrap().podcast_refresh_hours;
            if hours > 0 {
                run_pass(&state, Duration::from_secs(5)).await;
                tokio::time::sleep(Duration::from_secs(u64::from(hours) * 3600)).await;
            } else {
                tokio::time::sleep(Duration::from_secs(300)).await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_and_short() {
        assert_eq!(safe_name("AC/DC: Live? <1979>", 80), "AC-DC- Live- -1979-");
        assert_eq!(safe_name("  ..hidden..  ", 80), "hidden");
        assert_eq!(safe_name("", 80), "untitled");
        assert_eq!(safe_name("a\u{0}b\tc", 80), "a b c");
        assert_eq!(safe_name(&"x".repeat(500), 80).chars().count(), 80);
        assert_eq!(
            safe_name("日本語のタイトル", 4),
            "日本語の",
            "counts characters, not bytes"
        );
    }

    #[test]
    fn dates_are_utc_civil_dates() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(1_772_445_600_000), "2026-03-02");
        assert_eq!(utc_date(951_782_400_000), "2000-02-29", "a leap day");
        assert_eq!(utc_date(-86_400_000), "1969-12-31");
        assert_eq!(utc_date(1_798_675_199_000), "2026-12-30");
    }

    #[test]
    fn extensions_come_from_the_type_then_the_address() {
        assert_eq!(extension_for(Some("audio/mpeg"), "https://c/x"), "mp3");
        assert_eq!(extension_for(Some("audio/x-m4a"), "https://c/x"), "m4a");
        assert_eq!(extension_for(Some("audio/mp4"), "https://c/x"), "m4a");
        assert_eq!(extension_for(Some("audio/ogg"), "https://c/x"), "ogg");
        assert_eq!(extension_for(None, "https://c/ep.OPUS?token=1"), "opus");
        assert_eq!(
            extension_for(Some("application/octet-stream"), "https://c/ep.m4a#t=1"),
            "m4a"
        );
        assert_eq!(
            extension_for(None, "https://c/stream"),
            "mp3",
            "the usual default"
        );
    }

    #[test]
    fn episode_paths_group_by_show() {
        let p = episode_path(
            Path::new("/pod"),
            "Show: One",
            Some(1_772_445_600_000),
            "Ep 1 / the beginning",
            "mp3",
        );
        assert_eq!(
            p,
            PathBuf::from("/pod/Show- One/2026-03-02 - Ep 1 - the beginning.mp3")
        );
        assert!(episode_path(Path::new("/p"), "S", None, "T", "mp3")
            .to_string_lossy()
            .contains("undated - T.mp3"));
    }

    #[test]
    fn the_folder_defaults_to_next_to_the_database() {
        let cfg = ServerConfig {
            db_path: PathBuf::from("/data/music.db"),
            ..Default::default()
        };
        assert_eq!(podcast_root(&cfg), PathBuf::from("/data/podcasts"));
        let cfg = ServerConfig {
            podcast_dir: Some(PathBuf::from("/Volumes/pods")),
            ..cfg
        };
        assert_eq!(podcast_root(&cfg), PathBuf::from("/Volumes/pods"));
    }
}
