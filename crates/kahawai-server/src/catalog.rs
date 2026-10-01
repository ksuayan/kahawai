//! Catalog snapshot and delta for the player's local cache
//! (docs/v1/kahawai-player-catalog-cache-spec.md).
//!
//! Triggers from migration 010 stamp every shown change to a track, album
//! or artist with the next `meta.catalog_rev`, and leave a tombstone for a
//! deleted row. A player keeps a snapshot and the revision it reflects, and
//! on start asks only for what has a later revision.

use axum::{
    extract::{Query, State},
    Json,
};
use kahawai_core::{Album, Artist, CatalogDelta, CatalogSnapshot, MusicError};
use serde::Deserialize;
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::api::{album_from_row, genres, ApiError, ALBUM_COLS};
use crate::{db, AppState};

/// A delta touching more than this share of the catalog becomes a full
/// pull: one response is cheaper than applying most of the catalog row by
/// row.
const FULL_RESYNC_SHARE: f64 = 0.2;

/// This database's identity and current revision.
async fn current(conn: &mut SqliteConnection) -> Result<(String, i64), MusicError> {
    let rows =
        sqlx::query("SELECT key, value FROM meta WHERE key IN ('catalog_id', 'catalog_rev')")
            .fetch_all(conn)
            .await
            .map_err(db::cvt)?;
    let (mut id, mut rev) = (String::new(), 0);
    for r in &rows {
        if r.get::<String, _>(0) == "catalog_id" {
            id = r.get(1);
        } else {
            rev = r.get(1);
        }
    }
    Ok((id, rev))
}

async fn albums_where(
    conn: &mut SqliteConnection,
    clause: &str,
    arg: i64,
) -> Result<Vec<Album>, MusicError> {
    // track_count stays 0: the player counts the tracks it holds.
    let rows = sqlx::query(&format!(
        "SELECT {ALBUM_COLS} FROM albums WHERE {clause} ORDER BY id"
    ))
    .bind(arg)
    .fetch_all(conn)
    .await
    .map_err(db::cvt)?;
    Ok(rows.iter().map(|r| album_from_row(r, 0)).collect())
}

async fn artists_where(
    conn: &mut SqliteConnection,
    clause: &str,
    arg: i64,
) -> Result<Vec<Artist>, MusicError> {
    let rows = sqlx::query(&format!(
        "SELECT id, name, sort_name FROM artists WHERE {clause} ORDER BY id"
    ))
    .bind(arg)
    .fetch_all(conn)
    .await
    .map_err(db::cvt)?;
    Ok(rows
        .iter()
        .map(|r| Artist {
            id: r.get(0),
            name: r.get(1),
            sort_name: r.get(2),
        })
        .collect())
}

async fn scalar(conn: &mut SqliteConnection, sql: &str, arg: i64) -> Result<i64, MusicError> {
    Ok(sqlx::query(sql)
        .bind(arg)
        .fetch_one(conn)
        .await
        .map_err(db::cvt)?
        .get(0))
}

async fn tombstones(
    conn: &mut SqliteConnection,
    kind: &str,
    since: i64,
) -> Result<Vec<i64>, MusicError> {
    Ok(
        sqlx::query("SELECT id FROM catalog_tombstones WHERE kind = ? AND rev > ? ORDER BY id")
            .bind(kind)
            .bind(since)
            .fetch_all(conn)
            .await
            .map_err(db::cvt)?
            .iter()
            .map(|r| r.get(0))
            .collect(),
    )
}

/// Tracks deleted after `since`, and tracks that became duplicate copies
/// (migration 013): to a player both are gone.
async fn removed_tracks(conn: &mut SqliteConnection, since: i64) -> Result<Vec<i64>, MusicError> {
    let mut ids = tombstones(conn, "track", since).await?;
    ids.extend(
        sqlx::query("SELECT id FROM tracks WHERE rev > ? AND duplicate_of IS NOT NULL")
            .bind(since)
            .fetch_all(&mut *conn)
            .await
            .map_err(db::cvt)?
            .iter()
            .map(|r| r.get::<i64, _>(0)),
    );
    ids.sort_unstable();
    Ok(ids)
}

/// This database's catalog id (random per database, migration 010).
pub async fn catalog_id(pool: &SqlitePool) -> Result<String, MusicError> {
    let mut conn = pool.acquire().await.map_err(db::cvt)?;
    Ok(current(&mut conn).await?.0)
}

/// Everything a player caches. One read transaction, so every row matches
/// the revision reported with it.
pub async fn snapshot(pool: &SqlitePool) -> Result<CatalogSnapshot, MusicError> {
    let mut tx = pool.begin().await.map_err(db::cvt)?;
    let (catalog_id, rev) = current(&mut tx).await?;
    let tracks = db::tracks_where(&mut tx, "missing = ? AND duplicate_of IS NULL", 0).await?;
    let albums = albums_where(&mut tx, "1 = ?", 1).await?;
    let artists = artists_where(&mut tx, "1 = ?", 1).await?;
    tx.commit().await.map_err(db::cvt)?;
    Ok(CatalogSnapshot {
        catalog_id,
        rev,
        tracks,
        albums,
        artists,
        genres: genres(pool).await?,
    })
}

/// What changed after revision `since` of database `catalog_id`.
pub async fn delta(
    pool: &SqlitePool,
    catalog_id: Option<&str>,
    since: i64,
) -> Result<CatalogDelta, MusicError> {
    let mut tx = pool.begin().await.map_err(db::cvt)?;
    let (id, rev) = current(&mut tx).await?;
    let resync = |id: String, rev: i64| CatalogDelta {
        catalog_id: id,
        rev,
        full_resync: true,
        ..Default::default()
    };
    // Another database, or a revision this one never reached: the cache
    // can't be patched.
    if catalog_id.is_some_and(|c| c != id) || since > rev || since < 0 {
        return Ok(resync(id, rev));
    }
    // Nothing changed is the common case (every player start): answer it
    // from the revision indexes alone.
    if since == rev {
        tx.commit().await.map_err(db::cvt)?;
        return Ok(CatalogDelta {
            catalog_id: id,
            rev,
            genres: genres(pool).await?,
            ..Default::default()
        });
    }
    let changed = scalar(&mut tx, "SELECT COUNT(*) FROM tracks WHERE rev > ?", since).await?
        + scalar(&mut tx, "SELECT COUNT(*) FROM albums WHERE rev > ?", since).await?
        + scalar(&mut tx, "SELECT COUNT(*) FROM artists WHERE rev > ?", since).await?;
    if changed > 0 {
        let total = scalar(
            &mut tx,
            "SELECT COUNT(*) FROM tracks WHERE missing = ? AND duplicate_of IS NULL",
            0,
        )
        .await?
            + scalar(&mut tx, "SELECT COUNT(*) FROM albums WHERE 1 = ?", 1).await?
            + scalar(&mut tx, "SELECT COUNT(*) FROM artists WHERE 1 = ?", 1).await?;
        if changed as f64 > FULL_RESYNC_SHARE * total.max(1) as f64 {
            return Ok(resync(id, rev));
        }
    }
    let delta = CatalogDelta {
        catalog_id: id,
        rev,
        full_resync: false,
        tracks: db::tracks_where(&mut tx, "rev > ? AND duplicate_of IS NULL", since).await?,
        albums: albums_where(&mut tx, "rev > ?", since).await?,
        artists: artists_where(&mut tx, "rev > ?", since).await?,
        removed_tracks: removed_tracks(&mut tx, since).await?,
        removed_albums: tombstones(&mut tx, "album", since).await?,
        removed_artists: tombstones(&mut tx, "artist", since).await?,
        genres: Vec::new(),
    };
    tx.commit().await.map_err(db::cvt)?;
    Ok(CatalogDelta {
        genres: genres(pool).await?,
        ..delta
    })
}

/// `GET /api/catalog`
pub async fn get_catalog(State(s): State<AppState>) -> Result<Json<CatalogSnapshot>, ApiError> {
    Ok(Json(snapshot(&s.pool).await?))
}

#[derive(Debug, Deserialize)]
pub struct DeltaQuery {
    pub since: i64,
    #[serde(default)]
    pub catalog_id: Option<String>,
}

/// `GET /api/catalog/delta?since=<rev>&catalog_id=<id>`
pub async fn get_catalog_delta(
    State(s): State<AppState>,
    Query(q): Query<DeltaQuery>,
) -> Result<Json<CatalogDelta>, ApiError> {
    Ok(Json(
        delta(&s.pool, q.catalog_id.as_deref(), q.since).await?,
    ))
}
