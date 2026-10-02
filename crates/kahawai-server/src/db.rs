//! SQLite catalog access. WAL mode, runtime-checked queries only
//! (no sqlx macros — the `macros` feature is deliberately disabled).
//! (Spec §3.2, §4.)

use std::path::Path;

use kahawai_core::{AudioFormat, MusicError, Track};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePool, SqliteRow},
    Row,
};

/// Map sqlx errors into the shared error type.
pub fn cvt(e: sqlx::Error) -> MusicError {
    MusicError::Db(e.to_string())
}

pub async fn open(db_path: &Path) -> anyhow::Result<SqlitePool> {
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let opts = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await?;
    // WAL: readers never block the single writer (spec §4).
    sqlx::query("PRAGMA journal_mode=WAL;")
        .execute(&pool)
        .await?;
    sqlx::query("PRAGMA synchronous=NORMAL;")
        .execute(&pool)
        .await?;
    run_migrations(&pool).await?;
    optimize(&pool).await?;
    Ok(pool)
}

/// Keep the query planner's statistics current. Without them SQLite guesses
/// index selectivity, and on this schema it guessed badly (see migration
/// 011). The first time, a full `ANALYZE` (well under a second for a big
/// library). After that, `PRAGMA optimize` re-analyzes only tables whose
/// size changed a lot, which costs next to nothing when nothing did. Run
/// at open and after every scan.
pub async fn optimize(pool: &SqlitePool) -> Result<(), MusicError> {
    let has_stats: bool =
        sqlx::query("SELECT COUNT(*) > 0 FROM sqlite_master WHERE name = 'sqlite_stat1'")
            .fetch_one(pool)
            .await
            .map_err(cvt)?
            .get(0);
    let sql = if has_stats {
        "PRAGMA optimize"
    } else {
        "ANALYZE"
    };
    sqlx::query(sql).execute(pool).await.map_err(cvt)?;
    Ok(())
}

async fn run_migrations(pool: &SqlitePool) -> anyhow::Result<()> {
    // Numbered, idempotent migrations. The version table makes re-runs safe;
    // 001 predates it, so version 1 is recorded the first time this runs.
    const MIGRATIONS: &[(i64, &str)] = &[
        (1, include_str!("../migrations/001_init.sql")),
        (2, include_str!("../migrations/002_scan_columns.sql")),
        (3, include_str!("../migrations/003_jobs.sql")),
        (4, include_str!("../migrations/004_mqa.sql")),
        (5, include_str!("../migrations/005_hash_pending.sql")),
        (6, include_str!("../migrations/006_albums_title_index.sql")),
        (7, include_str!("../migrations/007_metadata_local.sql")),
        (8, include_str!("../migrations/008_enrichment.sql")),
        (9, include_str!("../migrations/009_genres.sql")),
        (10, include_str!("../migrations/010_catalog_rev.sql")),
        (11, include_str!("../migrations/011_indexes.sql")),
        (12, include_str!("../migrations/012_job_times.sql")),
        (13, include_str!("../migrations/013_duplicates.sql")),
        (14, include_str!("../migrations/014_audiobooks.sql")),
        (15, include_str!("../migrations/015_audiobook_enrich.sql")),
        (
            16,
            include_str!("../migrations/016_audiobook_duplicates.sql"),
        ),
        (
            17,
            include_str!("../migrations/017_audiobook_listeners.sql"),
        ),
        (
            18,
            include_str!("../migrations/018_audiobook_shelf_dismiss.sql"),
        ),
        (19, include_str!("../migrations/019_podcasts.sql")),
        (20, include_str!("../migrations/020_radio.sql")),
    ];
    // One connection throughout: `PRAGMA foreign_keys` is per connection, and
    // 005 rebuilds `tracks`, which SQLite only allows with foreign keys off
    // (dropping a referenced table would otherwise fail or cascade).
    let mut conn = pool.acquire().await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY)")
        .execute(&mut *conn)
        .await?;
    // Backfill: if the catalog tables already exist but no version was ever
    // recorded, 001 was applied by the old runner.
    let has_tracks: bool =
        sqlx::query("SELECT COUNT(*) > 0 FROM sqlite_master WHERE name = 'tracks'")
            .fetch_one(&mut *conn)
            .await?
            .get(0);
    // Builds of claude/online-sources numbered the radio migration 18 before
    // it became 20, so a database from one of them has 18 recorded without the
    // audiobook shelf migration that 18 now is. Forget that 18 (it is checked
    // by its column, and 020 re-runs the radio part safely).
    let has_dismissed: bool = sqlx::query(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('audiobook_positions') WHERE name = 'dismissed'",
    )
    .fetch_one(&mut *conn)
    .await?
    .get(0);
    if !has_dismissed {
        sqlx::query("DELETE FROM schema_migrations WHERE version = 18")
            .execute(&mut *conn)
            .await?;
    }
    let applied: Vec<i64> = sqlx::query("SELECT version FROM schema_migrations")
        .fetch_all(&mut *conn)
        .await?
        .iter()
        .map(|r| r.get(0))
        .collect();
    if has_tracks && !applied.contains(&1) {
        sqlx::query("INSERT OR IGNORE INTO schema_migrations (version) VALUES (1)")
            .execute(&mut *conn)
            .await?;
    }
    let pending: Vec<&(i64, &str)> = MIGRATIONS
        .iter()
        .filter(|(v, _)| !(applied.contains(v) || (*v == 1 && has_tracks)))
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await?;
    let result = async {
        for (version, sql) in pending {
            let mut tx = sqlx::Connection::begin(&mut *conn).await?;
            apply_sql(&mut tx, sql).await?;
            sqlx::query("INSERT INTO schema_migrations (version) VALUES (?)")
                .bind(version)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        let broken = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut *conn)
            .await?;
        anyhow::ensure!(
            broken.is_empty(),
            "migrations left {} foreign-key violations",
            broken.len()
        );
        anyhow::Ok(())
    }
    .await;
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *conn)
        .await?;
    result
}

/// Execute a migration file: strip full-line comments (the naive statement
/// splitter would trip on semicolons inside comments), then split on ';'.
async fn apply_sql(conn: &mut sqlx::SqliteConnection, sql: &str) -> anyhow::Result<()> {
    for stmt in split_statements(sql) {
        sqlx::query(&stmt).execute(&mut *conn).await?;
    }
    Ok(())
}

/// A migration file's statements, split on ';'. A `CREATE TRIGGER` body
/// holds statements of its own, so it runs through to its closing `END`.
fn split_statements(sql: &str) -> Vec<String> {
    let cleaned: String = sql
        .lines()
        .filter(|l| !l.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = Vec::new();
    let mut pending = String::new();
    for piece in cleaned.split(';') {
        if !pending.is_empty() {
            pending.push(';');
        }
        pending.push_str(piece);
        let stmt = pending.trim();
        let upper = stmt.to_ascii_uppercase();
        if upper.starts_with("CREATE TRIGGER") && !upper.ends_with("END") {
            continue;
        }
        if !stmt.is_empty() {
            out.push(stmt.to_string());
        }
        pending.clear();
    }
    let rest = pending.trim();
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

fn track_from_row(r: &SqliteRow) -> Track {
    Track {
        id: r.get("id"),
        path: r.get("path"),
        hash: r.get("hash"),
        format: AudioFormat::from_wire(&r.get::<String, _>("format")),
        sample_rate: r.get::<Option<i64>, _>("sample_rate").map(|v| v as u32),
        bit_depth: r.get::<Option<i64>, _>("bit_depth").map(|v| v as u8),
        channels: r.get::<Option<i64>, _>("channels").map(|v| v as u8),
        duration_ms: r.get::<Option<i64>, _>("duration_ms").map(|v| v as u64),
        bitrate: r.get::<Option<i64>, _>("bitrate").map(|v| v as u32),
        title: r.get("title"),
        album: r.get("album"),
        artist: r.get("artist"),
        album_id: r.get("album_id"),
        track_no: r.get::<Option<i64>, _>("track_no").map(|v| v as u32),
        disc_no: r.get::<Option<i64>, _>("disc_no").map(|v| v as u32),
        genre: r.get("genre"),
        year: r
            .get::<Option<i64>, _>("year")
            .and_then(|y| u16::try_from(y).ok()),
        missing: r.get::<i64, _>("missing") != 0,
        decodable: r.get::<i64, _>("decodable") != 0,
        mqa: r.get::<i64, _>("mqa") != 0,
        original_sample_rate: r
            .get::<Option<i64>, _>("original_sample_rate")
            .and_then(|v| u32::try_from(v).ok()),
    }
}

const TRACK_COLS: &str = "id, path, hash, format, sample_rate, bit_depth, channels, \
     duration_ms, bitrate, title, album, artist, album_id, track_no, disc_no, \
     genre, year, missing, decodable, mqa, original_sample_rate";

pub async fn get_track(pool: &SqlitePool, id: i64) -> Result<Option<Track>, MusicError> {
    let row = sqlx::query(&format!("SELECT {TRACK_COLS} FROM tracks WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(cvt)?;
    Ok(row.map(|r| track_from_row(&r)))
}

/// Tracks matching a `WHERE` clause with one integer parameter, by id.
/// Backs the player catalog snapshot and delta.
pub async fn tracks_where(
    conn: &mut sqlx::SqliteConnection,
    clause: &str,
    arg: i64,
) -> Result<Vec<Track>, MusicError> {
    let rows = sqlx::query(&format!(
        "SELECT {TRACK_COLS} FROM tracks WHERE {clause} ORDER BY id"
    ))
    .bind(arg)
    .fetch_all(conn)
    .await
    .map_err(cvt)?;
    Ok(rows.iter().map(track_from_row).collect())
}

/// Non-missing tracks of one album in play order. (S2 browse.)
pub async fn tracks_for_album(pool: &SqlitePool, album_id: i64) -> Result<Vec<Track>, MusicError> {
    let rows = sqlx::query(&format!(
        "SELECT {TRACK_COLS} FROM tracks \
         WHERE album_id = ? AND missing = 0 AND duplicate_of IS NULL \
         ORDER BY disc_no, track_no, id"
    ))
    .bind(album_id)
    .fetch_all(pool)
    .await
    .map_err(cvt)?;
    Ok(rows.iter().map(track_from_row).collect())
}

/// A duplicate copy of another track (migration 013)?
pub async fn is_duplicate(pool: &SqlitePool, id: i64) -> Result<bool, MusicError> {
    Ok(
        sqlx::query("SELECT duplicate_of IS NOT NULL FROM tracks WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(cvt)?
            .is_some_and(|r| r.get::<bool, _>(0)),
    )
}

/// Mark duplicate copies (migration 013): among the present, hashed tracks
/// of an album, those with the same content hash keep the lowest id and the
/// rest point at it. Only rows whose value changes are written, so an
/// unchanged library costs one read and bumps no revisions. Run after every
/// scan, after hashing, and at startup. Returns how many tracks changed
/// (became, or stopped being, a duplicate).
pub async fn refresh_duplicates(pool: &SqlitePool) -> Result<u64, MusicError> {
    let rows = sqlx::query(
        "SELECT t.id, t.duplicate_of,
           CASE WHEN t.missing = 0 THEN
             (SELECT MIN(k.id) FROM tracks k
              WHERE k.album_id = t.album_id AND k.hash = t.hash AND k.missing = 0)
           END AS keep
         FROM tracks t
         WHERE t.missing = 0 OR t.duplicate_of IS NOT NULL",
    )
    .fetch_all(pool)
    .await
    .map_err(cvt)?;
    let mut changes = Vec::new();
    let mut duplicates = 0u64;
    for r in &rows {
        let id: i64 = r.get(0);
        let current: Option<i64> = r.get(1);
        let keep: Option<i64> = r.get(2);
        // keep is NULL for a missing track, or one without an album or a
        // hash yet: none of them is a duplicate.
        let want = keep.filter(|&k| k != id);
        if want.is_some() {
            duplicates += 1;
        }
        if want != current {
            changes.push((id, want));
        }
    }
    if !changes.is_empty() {
        let mut tx = pool.begin().await.map_err(cvt)?;
        for (id, want) in &changes {
            sqlx::query("UPDATE tracks SET duplicate_of = ? WHERE id = ?")
                .bind(want)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(cvt)?;
        }
        tx.commit().await.map_err(cvt)?;
        tracing::info!(
            changed = changes.len(),
            duplicates,
            "duplicate tracks refreshed"
        );
    }
    Ok(changes.len() as u64)
}

/// Sort orders for a genre's tracks: by artist, album title or year, either
/// way. Albums stay together in play order within each. Unknown years sort
/// last both ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GenreSortField {
    #[default]
    Artist,
    Title,
    Year,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GenreTrackOrder {
    pub field: GenreSortField,
    pub descending: bool,
}

impl GenreTrackOrder {
    fn sql(self) -> String {
        let dir = if self.descending { "DESC" } else { "ASC" };
        let album = "t.album COLLATE NOCASE, t.album_id, t.disc_no, t.track_no, t.id";
        match self.field {
            GenreSortField::Artist => format!("t.artist COLLATE NOCASE {dir}, {album}"),
            GenreSortField::Title => {
                format!("t.album COLLATE NOCASE {dir}, t.album_id, t.disc_no, t.track_no, t.id")
            }
            GenreSortField::Year => {
                format!("t.year IS NULL, t.year {dir}, t.artist COLLATE NOCASE, {album}")
            }
        }
    }
}

/// One page of a canonical genre's present tracks, by artist, album and
/// play order, and how many there are in all.
pub async fn tracks_for_genre(
    pool: &SqlitePool,
    genre: &str,
    order: GenreTrackOrder,
    limit: u64,
    offset: u64,
) -> Result<(Vec<Track>, u64), MusicError> {
    let total: i64 = sqlx::query(
        "SELECT COUNT(*) FROM track_genres g JOIN tracks t ON t.id = g.track_id
         WHERE g.genre = ? AND t.missing = 0 AND t.duplicate_of IS NULL",
    )
    .bind(genre)
    .fetch_one(pool)
    .await
    .map_err(cvt)?
    .get(0);
    let cols = TRACK_COLS
        .split(", ")
        .map(|c| format!("t.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    let rows = sqlx::query(&format!(
        "SELECT {cols} FROM track_genres g JOIN tracks t ON t.id = g.track_id
         WHERE g.genre = ? AND t.missing = 0 AND t.duplicate_of IS NULL
         ORDER BY {}
         LIMIT ? OFFSET ?",
        order.sql()
    ))
    .bind(genre)
    .bind(limit as i64)
    .bind(offset as i64)
    .fetch_all(pool)
    .await
    .map_err(cvt)?;
    Ok((rows.iter().map(track_from_row).collect(), total as u64))
}

/// Minimal insert used by the integration tests to seed a catalog row.
#[cfg(test)]
pub async fn insert_track_minimal(
    pool: &SqlitePool,
    path: &str,
    hash: &str,
    format: &str,
) -> Result<i64, MusicError> {
    let id: i64 =
        sqlx::query("INSERT INTO tracks (path, hash, format) VALUES (?, ?, ?) RETURNING id")
            .bind(path)
            .bind(hash)
            .bind(format)
            .fetch_one(pool)
            .await
            .map_err(cvt)?
            .get("id");
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Row;

    async fn versions(pool: &SqlitePool) -> Vec<i64> {
        sqlx::query("SELECT version FROM schema_migrations ORDER BY version")
            .fetch_all(pool)
            .await
            .unwrap()
            .iter()
            .map(|r| r.get(0))
            .collect()
    }

    /// A scaffold-era database (001 applied by the old runner, no version
    /// table) must be backfilled to 1 and upgraded to 002 without losing rows.
    #[tokio::test]
    async fn migrates_legacy_v1_database() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("legacy.db");
        let opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        apply_sql(
            &mut pool.acquire().await.unwrap(),
            include_str!("../migrations/001_init.sql"),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO tracks (path, hash, format) VALUES ('/m/a.flac', 'abc', 'flac')")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;

        let pool = open(&db_path).await.unwrap();
        assert_eq!(
            versions(&pool).await,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
        );

        // Old row survived; new columns carry their defaults.
        let r = sqlx::query(
            "SELECT missing, decodable, file_size, genre, year, artwork_hash
             FROM tracks WHERE path = '/m/a.flac'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(r.get::<i64, _>("missing"), 0);
        assert_eq!(r.get::<i64, _>("decodable"), 1);
        assert!(r.get::<Option<i64>, _>("file_size").is_none());
        assert!(r.get::<Option<String>, _>("genre").is_none());
        assert!(r.get::<Option<String>, _>("artwork_hash").is_none());

        // scan_log gained the new counters.
        sqlx::query("INSERT INTO scan_log (started_at, files_added) VALUES ('t', 1)")
            .execute(&pool)
            .await
            .unwrap();
        let n: i64 = sqlx::query("SELECT files_updated FROM scan_log")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(n, 0);

        // The current row mapping reads the upgraded row.
        let track = get_track(&pool, 1).await.unwrap().unwrap();
        assert_eq!(track.path, "/m/a.flac");
        assert!(!track.missing && track.decodable);
        // MQA columns exist, off by default.
        assert!(!track.mqa);
        assert_eq!(track.original_sample_rate, None);
    }

    /// Migration 004 on a catalog that already has rows: only FLAC rows are
    /// queued for the MQA tag backfill (`mqa_checked = 0`); every other format
    /// is already "checked" so a scan never revisits them.
    #[tokio::test]
    async fn migration_004_queues_only_flac_rows_for_the_mqa_backfill() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("v3.db");
        let opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        for (v, sql) in [
            (1, include_str!("../migrations/001_init.sql")),
            (2, include_str!("../migrations/002_scan_columns.sql")),
            (3, include_str!("../migrations/003_jobs.sql")),
        ] {
            apply_sql(&mut pool.acquire().await.unwrap(), sql)
                .await
                .unwrap();
            sqlx::query("INSERT INTO schema_migrations (version) VALUES (?)")
                .bind(v)
                .execute(&pool)
                .await
                .unwrap();
        }
        for (path, fmt) in [
            ("/m/a.flac", "flac"),
            ("/m/b.mp3", "mp3"),
            ("/m/c.m4a", "m4a"),
            ("/m/d.dsf", "dsf"),
        ] {
            sqlx::query("INSERT INTO tracks (path, hash, format) VALUES (?, 'h', ?)")
                .bind(path)
                .bind(fmt)
                .execute(&pool)
                .await
                .unwrap();
        }
        pool.close().await;

        let pool = open(&db_path).await.unwrap();
        assert_eq!(
            versions(&pool).await,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
        );
        let rows = sqlx::query("SELECT format, mqa, mqa_checked FROM tracks ORDER BY path")
            .fetch_all(&pool)
            .await
            .unwrap();
        let got: Vec<(String, i64, i64)> = rows
            .iter()
            .map(|r| (r.get("format"), r.get("mqa"), r.get("mqa_checked")))
            .collect();
        assert_eq!(
            got,
            vec![
                ("flac".into(), 0, 0),
                ("mp3".into(), 0, 1),
                ("m4a".into(), 0, 1),
                ("dsf".into(), 0, 1)
            ]
        );
    }

    /// Migration 005 rebuilds `tracks` to make `hash` nullable. Existing rows,
    /// their hashes (now labelled BLAKE3), and every row that points at a
    /// track by id survive; new rows can be written without a hash.
    #[tokio::test]
    async fn migration_005_makes_hash_nullable_and_keeps_every_link() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("v4.db");
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&db_path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        for (v, sql) in [
            (1, include_str!("../migrations/001_init.sql")),
            (2, include_str!("../migrations/002_scan_columns.sql")),
            (3, include_str!("../migrations/003_jobs.sql")),
            (4, include_str!("../migrations/004_mqa.sql")),
        ] {
            apply_sql(&mut pool.acquire().await.unwrap(), sql)
                .await
                .unwrap();
            sqlx::query("INSERT INTO schema_migrations (version) VALUES (?)")
                .bind(v)
                .execute(&pool)
                .await
                .unwrap();
        }
        for stmt in [
            "INSERT INTO albums (id, title) VALUES (1, 'Blue Train')",
            "INSERT INTO artists (id, name) VALUES (1, 'John Coltrane')",
            "INSERT INTO tracks (id, path, hash, format, title, artist, album_id, file_size, mqa, mqa_checked)
             VALUES (7, '/m/a.flac', 'abc123', 'flac', 'Moment''s Notice', 'John Coltrane', 1, 100, 1, 1)",
            "INSERT INTO track_artists (track_id, artist_id) VALUES (7, 1)",
            "INSERT INTO playlists (id, name) VALUES (1, 'mix')",
            "INSERT INTO playlist_tracks (playlist_id, position, track_id) VALUES (1, 0, 7)",
            "INSERT INTO search_fts (rowid, title, album, artist) VALUES (7, 'Moment''s Notice', 'Blue Train', 'John Coltrane')",
        ] {
            sqlx::query(stmt).execute(&pool).await.unwrap();
        }
        pool.close().await;

        let pool = open(&db_path).await.unwrap();
        assert_eq!(
            versions(&pool).await,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
        );
        let r =
            sqlx::query("SELECT id, hash, hash_algo, title, album_id, file_size, mqa FROM tracks")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(r.get::<i64, _>("id"), 7);
        assert_eq!(
            r.get::<Option<String>, _>("hash").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            r.get::<Option<String>, _>("hash_algo").as_deref(),
            Some("blake3-v1")
        );
        assert_eq!(r.get::<String, _>("title"), "Moment's Notice");
        assert_eq!(r.get::<i64, _>("album_id"), 1);
        assert_eq!(r.get::<i64, _>("file_size"), 100);
        assert_eq!(r.get::<i64, _>("mqa"), 1);
        for q in [
            "SELECT COUNT(*) FROM track_artists WHERE track_id = 7",
            "SELECT COUNT(*) FROM playlist_tracks WHERE track_id = 7",
            "SELECT COUNT(*) FROM search_fts WHERE search_fts MATCH 'coltrane'",
        ] {
            let n: i64 = sqlx::query(q).fetch_one(&pool).await.unwrap().get(0);
            assert_eq!(n, 1, "{q}");
        }
        let t = get_track(&pool, 7).await.unwrap().unwrap();
        assert_eq!(t.hash.as_deref(), Some("abc123"));

        // A row with its hash still pending.
        sqlx::query("INSERT INTO tracks (path, format) VALUES ('/m/b.flac', 'flac')")
            .execute(&pool)
            .await
            .unwrap();
        let id: i64 = sqlx::query("SELECT id FROM tracks WHERE path = '/m/b.flac'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(get_track(&pool, id).await.unwrap().unwrap().hash, None);

        // Foreign keys are back on after the rebuild.
        let fk: i64 = sqlx::query("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(fk, 1);
        // scan_log has the walk-error counter.
        sqlx::query("INSERT INTO scan_log (started_at, walk_errors) VALUES ('t', 3)")
            .execute(&pool)
            .await
            .unwrap();
    }

    /// Migration 007 on an existing catalog: impossible years are cleared,
    /// embedded covers are labelled, DSD rows skip the ID re-read (their tag
    /// reader has no MusicBrainz IDs), and every album starts pending.
    #[tokio::test]
    async fn migration_007_cleans_years_and_queues_the_id_backfill() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("v6.db");
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&db_path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        for (v, sql) in [
            (1, include_str!("../migrations/001_init.sql")),
            (2, include_str!("../migrations/002_scan_columns.sql")),
            (3, include_str!("../migrations/003_jobs.sql")),
            (4, include_str!("../migrations/004_mqa.sql")),
            (5, include_str!("../migrations/005_hash_pending.sql")),
            (6, include_str!("../migrations/006_albums_title_index.sql")),
        ] {
            apply_sql(&mut pool.acquire().await.unwrap(), sql)
                .await
                .unwrap();
            sqlx::query("INSERT INTO schema_migrations (version) VALUES (?)")
                .bind(v)
                .execute(&pool)
                .await
                .unwrap();
        }
        for stmt in [
            "INSERT INTO albums (id, title, year, artwork_hash) VALUES (1, 'Old', 1800, 'abc')",
            "INSERT INTO albums (id, title, year) VALUES (2, 'Fine', 1959)",
            "INSERT INTO tracks (path, format, year) VALUES ('/m/a.flac', 'flac', 0)",
            "INSERT INTO tracks (path, format, year) VALUES ('/m/b.dsf', 'dsf', 1959)",
        ] {
            sqlx::query(stmt).execute(&pool).await.unwrap();
        }
        pool.close().await;

        let pool = open(&db_path).await.unwrap();
        let albums: Vec<(i64, Option<i64>, Option<String>, String)> = sqlx::query_as(
            "SELECT id, year, artwork_source, enrich_status FROM albums ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            albums,
            vec![
                (1, None, Some("embedded".into()), "pending".into()),
                (2, Some(1959), None, "pending".into()),
            ]
        );
        let tracks: Vec<(String, Option<i64>, i64)> =
            sqlx::query_as("SELECT format, year, mbid_checked FROM tracks ORDER BY path")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            tracks,
            vec![("flac".into(), None, 0), ("dsf".into(), Some(1959), 1)]
        );
    }

    /// Opening keeps planner statistics, and the hot queries use the
    /// indexes meant for them (migration 011).
    #[tokio::test]
    async fn statistics_and_index_choices() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = open(&dir.path().join("t.db")).await.unwrap();
        let has_stats: bool =
            sqlx::query("SELECT COUNT(*) > 0 FROM sqlite_master WHERE name = 'sqlite_stat1'")
                .fetch_one(&pool)
                .await
                .unwrap()
                .get(0);
        assert!(has_stats);
        let plan = |sql: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query(&format!("EXPLAIN QUERY PLAN {sql}"))
                    .fetch_all(&pool)
                    .await
                    .unwrap()
                    .iter()
                    .map(|r| r.get::<String, _>("detail"))
                    .collect::<Vec<_>>()
                    .join(" | ")
            }
        };
        let album = plan("SELECT id FROM tracks WHERE album_id = 1 AND missing = 0").await;
        assert!(album.contains("idx_tracks_album"), "{album}");
        let pending =
            plan("SELECT id FROM tracks WHERE hash IS NULL AND missing = 0 AND id > 0 ORDER BY id")
                .await;
        assert!(pending.contains("idx_tracks_hash_pending"), "{pending}");
        let artist = plan("SELECT album_id FROM album_artists WHERE artist_id = 1").await;
        assert!(artist.contains("idx_album_artists_artist"), "{artist}");
        for gone in ["idx_tracks_missing", "idx_albums_title", "idx_tracks_hash"] {
            let n: i64 = sqlx::query("SELECT COUNT(*) FROM sqlite_master WHERE name = ?")
                .bind(gone)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get(0);
            assert_eq!(n, 0, "{gone}");
        }
    }

    /// Trigger bodies hold statements of their own and stay whole.
    #[test]
    fn split_statements_keeps_trigger_bodies_whole() {
        let sql = "-- a; comment\nCREATE TABLE t (x);\nCREATE TRIGGER tr AFTER INSERT ON t\nBEGIN\n  UPDATE t SET x = 1;\n  UPDATE t SET x = 2;\nEND;\nINSERT INTO t VALUES (3);\n";
        let stmts = split_statements(sql);
        assert_eq!(stmts.len(), 3, "{stmts:?}");
        assert!(stmts[1].starts_with("CREATE TRIGGER") && stmts[1].ends_with("END"));
        assert!(stmts[1].contains("x = 1;") && stmts[1].contains("x = 2"));
    }

    /// Fresh databases get every migration, and reopening is idempotent.
    #[tokio::test]
    async fn fresh_database_gets_all_migrations_idempotently() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("fresh.db");
        let pool = open(&db_path).await.unwrap();
        assert_eq!(
            versions(&pool).await,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
        );
        pool.close().await;
        let pool = open(&db_path).await.unwrap();
        assert_eq!(
            versions(&pool).await,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
        );
    }

    async fn has_table(pool: &SqlitePool, name: &str) -> bool {
        sqlx::query("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap()
            .get(0)
    }

    /// A database where 18 was main's audiobook-shelf migration (so the radio
    /// migration, once numbered 18, was skipped) gets its radio tables; one
    /// that already ran the radio migration as 18 reopens without error.
    #[tokio::test]
    async fn radio_tables_appear_whatever_ran_as_18() {
        for radio_ran_as_18 in [false, true] {
            let dir = tempfile::TempDir::new().unwrap();
            let db_path = dir.path().join("t.db");
            let pool = open(&db_path).await.unwrap();
            // Take the database back to "18 recorded, 20 not run".
            for t in ["radio_favorites", "radio_history", "radio_cache"] {
                sqlx::query(&format!("DROP TABLE {t}"))
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            sqlx::query("DELETE FROM schema_migrations WHERE version = 20")
                .execute(&pool)
                .await
                .unwrap();
            if radio_ran_as_18 {
                // A database from such a build never had the shelf column.
                sqlx::query("ALTER TABLE audiobook_positions DROP COLUMN dismissed")
                    .execute(&pool)
                    .await
                    .unwrap();
                let mut conn = pool.acquire().await.unwrap();
                let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
                apply_sql(&mut tx, include_str!("../migrations/020_radio.sql"))
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
            }
            sqlx::query("INSERT OR IGNORE INTO schema_migrations (version) VALUES (18)")
                .execute(&pool)
                .await
                .unwrap();
            pool.close().await;

            let pool = open(&db_path).await.unwrap();
            for t in ["radio_favorites", "radio_history", "radio_cache"] {
                assert!(
                    has_table(&pool, t).await,
                    "{t} (radio ran as 18: {radio_ran_as_18})"
                );
            }
            assert!(versions(&pool).await.contains(&20));
            let dismissed: bool = sqlx::query(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('audiobook_positions') WHERE name = 'dismissed'",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
            assert!(
                dismissed,
                "the shelf migration ran (radio ran as 18: {radio_ran_as_18})"
            );
        }
    }
}
