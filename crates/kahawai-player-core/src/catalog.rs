//! Local catalog cache (docs/v2/kahawai-player-catalog-cache-spec.md).
//!
//! The player keeps the server's catalog (present tracks, albums, artists,
//! genres) in a small SQLite file, plus the revision it reflects. The
//! library renders from here at once on start. [`CatalogCache::sync`] then
//! asks the server what changed since that revision: nothing (one small
//! request), a delta to apply, or, the first time and when a delta can't be
//! applied, the whole catalog. While the server is unreachable the cache is
//! all there is, and browsing still works.
//!
//! Rows are stored as the server's JSON, keyed by id (and album id for
//! tracks), so the cache never has to mirror the server's columns. No Tauri
//! or async here: the shell calls [`CatalogCache::sync`] from a worker
//! thread.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use kahawai_core::{Album, Artist, CatalogDelta, CatalogSnapshot, Genre, MusicError, Track};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};

/// Bump to rebuild the cache file's tables (they only ever hold a copy).
const SCHEMA_VERSION: i64 = 1;
/// The whole catalog of a large library is tens of megabytes of JSON.
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024 * 1024;

/// What a sync did, for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    /// The cache was current: one small request.
    Unchanged,
    /// A delta was applied.
    Updated,
    /// The whole catalog was pulled (first run, another server, big change).
    Full,
    /// The server couldn't be reached; the cache is unchanged.
    Offline,
    /// The server has no catalog endpoint (an older version).
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncReport {
    pub status: SyncStatus,
    /// Rows written or removed.
    pub changed: u64,
    /// Why, when offline.
    pub message: Option<String>,
}

/// A failed catalog request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// 404: the server doesn't have the endpoint.
    NotFound,
    /// Anything else: no connection, a timeout, a server error.
    Failed(String),
}

pub struct CatalogCache {
    conn: Mutex<Connection>,
}

fn db_err(e: rusqlite::Error) -> MusicError {
    MusicError::Db(e.to_string())
}

fn to_json<T: Serialize>(v: &T) -> Result<String, MusicError> {
    serde_json::to_string(v).map_err(|e| MusicError::Db(e.to_string()))
}

fn rows<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
) -> Result<Vec<T>, MusicError> {
    let mut stmt = conn.prepare(sql).map_err(db_err)?;
    let jsons = stmt
        .query_map(args, |r| r.get::<_, String>(0))
        .map_err(db_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_err)?;
    // A row that no longer parses (older format) is skipped, not fatal:
    // the next full pull replaces it.
    Ok(jsons
        .iter()
        .filter_map(|j| serde_json::from_str(j).ok())
        .collect())
}

impl CatalogCache {
    /// Open (creating if needed) the cache file.
    pub fn open(path: &Path) -> Result<Self, MusicError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(MusicError::Io)?;
        }
        Self::init(Connection::open(path).map_err(db_err)?)
    }

    /// An in-memory cache (tests, or when the file can't be opened).
    pub fn in_memory() -> Result<Self, MusicError> {
        Self::init(Connection::open_in_memory().map_err(db_err)?)
    }

    fn init(conn: Connection) -> Result<Self, MusicError> {
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(db_err)?;
        if version != SCHEMA_VERSION {
            conn.execute_batch(
                "DROP TABLE IF EXISTS meta; DROP TABLE IF EXISTS tracks;
                 DROP TABLE IF EXISTS albums; DROP TABLE IF EXISTS artists;",
            )
            .map_err(db_err)?;
        }
        conn.execute_batch(&format!(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS tracks (
                 id INTEGER PRIMARY KEY, album_id INTEGER, json TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS tracks_album ON tracks(album_id);
             CREATE TABLE IF NOT EXISTS albums (id INTEGER PRIMARY KEY, json TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS artists (id INTEGER PRIMARY KEY, json TEXT NOT NULL);
             PRAGMA user_version = {SCHEMA_VERSION};"
        ))
        .map_err(db_err)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn meta(conn: &Connection, key: &str) -> Result<Option<String>, MusicError> {
        conn.query_row("SELECT value FROM meta WHERE key = ?", [key], |r| r.get(0))
            .optional()
            .map_err(db_err)
    }

    /// The server database and revision the cache reflects; `None` when it
    /// has never been filled.
    pub fn state(&self) -> Result<Option<(String, i64)>, MusicError> {
        let conn = self.conn.lock().unwrap();
        let id = Self::meta(&conn, "catalog_id")?;
        let rev = Self::meta(&conn, "rev")?.and_then(|r| r.parse().ok());
        Ok(id.zip(rev))
    }

    /// Every album, with `track_count` counted from the cached tracks.
    pub fn albums(&self) -> Result<Vec<Album>, MusicError> {
        let conn = self.conn.lock().unwrap();
        let mut counts = std::collections::HashMap::new();
        let mut stmt = conn
            .prepare("SELECT album_id, COUNT(*) FROM tracks WHERE album_id IS NOT NULL GROUP BY album_id")
            .map_err(db_err)?;
        for row in stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(db_err)?
        {
            let (id, n) = row.map_err(db_err)?;
            counts.insert(id, n.max(0) as u64);
        }
        let mut albums: Vec<Album> = rows(&conn, "SELECT json FROM albums ORDER BY id", [])?;
        for a in &mut albums {
            a.track_count = counts.get(&a.id).copied().unwrap_or(0);
        }
        Ok(albums)
    }

    pub fn artists(&self) -> Result<Vec<Artist>, MusicError> {
        rows(
            &self.conn.lock().unwrap(),
            "SELECT json FROM artists ORDER BY id",
            [],
        )
    }

    pub fn genres(&self) -> Result<Vec<Genre>, MusicError> {
        let conn = self.conn.lock().unwrap();
        Ok(Self::meta(&conn, "genres")?
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default())
    }

    /// One album's tracks in play order.
    pub fn album_tracks(&self, album_id: i64) -> Result<Vec<Track>, MusicError> {
        let mut tracks: Vec<Track> = rows(
            &self.conn.lock().unwrap(),
            "SELECT json FROM tracks WHERE album_id = ?",
            [album_id],
        )?;
        tracks.sort_by_key(|t| (t.disc_no.unwrap_or(0), t.track_no.unwrap_or(0), t.id));
        Ok(tracks)
    }

    /// The cached tracks among `ids`, in the order given.
    pub fn tracks(&self, ids: &[i64]) -> Result<Vec<Track>, MusicError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT json FROM tracks WHERE id = ?")
            .map_err(db_err)?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let json: Option<String> = stmt
                .query_row([id], |r| r.get(0))
                .optional()
                .map_err(db_err)?;
            if let Some(t) = json.and_then(|j| serde_json::from_str(&j).ok()) {
                out.push(t);
            }
        }
        Ok(out)
    }

    fn put_track(tx: &rusqlite::Transaction, t: &Track) -> Result<(), MusicError> {
        if t.missing {
            tx.execute("DELETE FROM tracks WHERE id = ?", [t.id])
                .map_err(db_err)?;
        } else {
            tx.execute(
                "INSERT OR REPLACE INTO tracks (id, album_id, json) VALUES (?, ?, ?)",
                params![t.id, t.album_id, to_json(t)?],
            )
            .map_err(db_err)?;
        }
        Ok(())
    }

    fn put_meta(
        tx: &rusqlite::Transaction,
        catalog_id: &str,
        rev: i64,
        genres: &[Genre],
    ) -> Result<(), MusicError> {
        for (k, v) in [
            ("catalog_id", catalog_id.to_string()),
            ("rev", rev.to_string()),
            ("genres", to_json(&genres)?),
        ] {
            tx.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES (?, ?)",
                [k, v.as_str()],
            )
            .map_err(db_err)?;
        }
        Ok(())
    }

    /// Replace everything with a full snapshot.
    pub fn apply_snapshot(&self, snap: &CatalogSnapshot) -> Result<u64, MusicError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        tx.execute_batch("DELETE FROM tracks; DELETE FROM albums; DELETE FROM artists;")
            .map_err(db_err)?;
        for t in &snap.tracks {
            Self::put_track(&tx, t)?;
        }
        for a in &snap.albums {
            tx.execute(
                "INSERT OR REPLACE INTO albums (id, json) VALUES (?, ?)",
                params![a.id, to_json(a)?],
            )
            .map_err(db_err)?;
        }
        for a in &snap.artists {
            tx.execute(
                "INSERT OR REPLACE INTO artists (id, json) VALUES (?, ?)",
                params![a.id, to_json(a)?],
            )
            .map_err(db_err)?;
        }
        Self::put_meta(&tx, &snap.catalog_id, snap.rev, &snap.genres)?;
        tx.commit().map_err(db_err)?;
        Ok((snap.tracks.len() + snap.albums.len() + snap.artists.len()) as u64)
    }

    /// Apply a delta on top of what's cached. A track that went missing
    /// leaves the cache, like a removed one.
    pub fn apply_delta(&self, d: &CatalogDelta) -> Result<u64, MusicError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        for t in &d.tracks {
            Self::put_track(&tx, t)?;
        }
        for a in &d.albums {
            tx.execute(
                "INSERT OR REPLACE INTO albums (id, json) VALUES (?, ?)",
                params![a.id, to_json(a)?],
            )
            .map_err(db_err)?;
        }
        for a in &d.artists {
            tx.execute(
                "INSERT OR REPLACE INTO artists (id, json) VALUES (?, ?)",
                params![a.id, to_json(a)?],
            )
            .map_err(db_err)?;
        }
        for (table, ids) in [
            ("tracks", &d.removed_tracks),
            ("albums", &d.removed_albums),
            ("artists", &d.removed_artists),
        ] {
            for id in ids {
                tx.execute(&format!("DELETE FROM {table} WHERE id = ?"), [id])
                    .map_err(db_err)?;
            }
        }
        Self::put_meta(&tx, &d.catalog_id, d.rev, &d.genres)?;
        tx.commit().map_err(db_err)?;
        Ok((d.tracks.len()
            + d.albums.len()
            + d.artists.len()
            + d.removed_tracks.len()
            + d.removed_albums.len()
            + d.removed_artists.len()) as u64)
    }

    /// Bring the cache up to date with the server at `base_url`.
    pub fn sync(&self, base_url: &str) -> Result<SyncReport, MusicError> {
        let base = base_url.trim_end_matches('/').to_string();
        self.sync_with(|path| fetch_text(&format!("{base}{path}")))
    }

    /// [`CatalogCache::sync`] with the HTTP GET supplied (`path` includes
    /// the query string); tests pass a fake server.
    pub fn sync_with(
        &self,
        get: impl Fn(&str) -> Result<String, FetchError>,
    ) -> Result<SyncReport, MusicError> {
        let report = |status, changed, message| SyncReport {
            status,
            changed,
            message,
        };
        let fail = |e: FetchError| match e {
            FetchError::NotFound => report(SyncStatus::Unsupported, 0, None),
            FetchError::Failed(m) => report(SyncStatus::Offline, 0, Some(m)),
        };
        let parse_err =
            |e: serde_json::Error| MusicError::Http(format!("bad catalog response: {e}"));

        if let Some((id, rev)) = self.state()? {
            let path = format!("/api/catalog/delta?since={rev}&catalog_id={id}");
            let delta: CatalogDelta = match get(&path) {
                Ok(body) => serde_json::from_str(&body).map_err(parse_err)?,
                Err(e) => return Ok(fail(e)),
            };
            if !delta.full_resync {
                let changed = self.apply_delta(&delta)?;
                let status = if delta.is_empty() {
                    SyncStatus::Unchanged
                } else {
                    SyncStatus::Updated
                };
                return Ok(report(status, changed, None));
            }
        }
        let snap: CatalogSnapshot = match get("/api/catalog") {
            Ok(body) => serde_json::from_str(&body).map_err(parse_err)?,
            Err(e) => return Ok(fail(e)),
        };
        let changed = self.apply_snapshot(&snap)?;
        Ok(report(SyncStatus::Full, changed, None))
    }
}

/// One GET, the body as text. Short connect timeout (an unreachable server
/// should read as offline quickly), a long overall one (a first pull of a
/// big library).
fn fetch_text(url: &str) -> Result<String, FetchError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(4)))
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .into();
    let mut resp = agent.get(url).call().map_err(|e| match e {
        ureq::Error::StatusCode(404) => FetchError::NotFound,
        other => FetchError::Failed(other.to_string()),
    })?;
    resp.body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_string()
        .map_err(|e| FetchError::Failed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kahawai_core::AudioFormat;
    use std::cell::RefCell;

    fn track(id: i64, album: i64, title: &str) -> Track {
        Track {
            id,
            path: format!("/m/{id}.flac"),
            hash: None,
            format: AudioFormat::Flac,
            sample_rate: Some(44100),
            bit_depth: Some(16),
            channels: Some(2),
            duration_ms: Some(1000),
            bitrate: None,
            title: Some(title.into()),
            album: None,
            artist: Some("A".into()),
            album_id: Some(album),
            track_no: Some(id as u32),
            disc_no: Some(1),
            genre: None,
            year: None,
            missing: false,
            decodable: true,
            mqa: false,
            original_sample_rate: None,
        }
    }

    fn album(id: i64, title: &str) -> Album {
        Album {
            id,
            title: title.into(),
            ..Default::default()
        }
    }

    fn snapshot() -> CatalogSnapshot {
        CatalogSnapshot {
            catalog_id: "db1".into(),
            rev: 10,
            tracks: vec![track(1, 7, "One"), track(2, 7, "Two"), track(3, 8, "Three")],
            albums: vec![album(7, "Blue Train"), album(8, "Kind of Blue")],
            artists: vec![Artist {
                id: 1,
                name: "A".into(),
                sort_name: None,
            }],
            genres: vec![Genre {
                name: "Jazz".into(),
                track_count: 3,
            }],
        }
    }

    /// A fake server: answers from a list of (path prefix, response) pairs
    /// and records every request.
    struct Fake {
        routes: Vec<(&'static str, Result<String, FetchError>)>,
        calls: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(routes: Vec<(&'static str, Result<String, FetchError>)>) -> Self {
            Self {
                routes,
                calls: RefCell::new(Vec::new()),
            }
        }
        fn get(&self, path: &str) -> Result<String, FetchError> {
            self.calls.borrow_mut().push(path.to_string());
            self.routes
                .iter()
                .find(|(p, _)| path.starts_with(p))
                .map(|(_, r)| r.clone())
                .unwrap_or(Err(FetchError::NotFound))
        }
    }

    fn json<T: Serialize>(v: &T) -> Result<String, FetchError> {
        Ok(serde_json::to_string(v).unwrap())
    }

    #[test]
    fn first_sync_pulls_everything_and_the_cache_serves_it() {
        let cache = CatalogCache::in_memory().unwrap();
        assert_eq!(cache.state().unwrap(), None);
        let fake = Fake::new(vec![("/api/catalog", json(&snapshot()))]);
        let r = cache.sync_with(|p| fake.get(p)).unwrap();
        assert_eq!((r.status, r.changed), (SyncStatus::Full, 6));
        assert_eq!(*fake.calls.borrow(), ["/api/catalog"]);

        assert_eq!(cache.state().unwrap(), Some(("db1".into(), 10)));
        let albums = cache.albums().unwrap();
        assert_eq!(
            albums
                .iter()
                .map(|a| (a.id, a.track_count))
                .collect::<Vec<_>>(),
            [(7, 2), (8, 1)],
            "track counts come from the cached tracks"
        );
        assert_eq!(cache.artists().unwrap().len(), 1);
        assert_eq!(cache.genres().unwrap()[0].name, "Jazz");
        let titles = |ts: Vec<Track>| ts.into_iter().map(|t| t.title.unwrap()).collect::<Vec<_>>();
        assert_eq!(titles(cache.album_tracks(7).unwrap()), ["One", "Two"]);
        assert_eq!(titles(cache.tracks(&[3, 99, 1]).unwrap()), ["Three", "One"]);
    }

    #[test]
    fn an_unchanged_server_costs_one_small_request() {
        let cache = CatalogCache::in_memory().unwrap();
        cache.apply_snapshot(&snapshot()).unwrap();
        let empty = CatalogDelta {
            catalog_id: "db1".into(),
            rev: 10,
            genres: snapshot().genres,
            ..Default::default()
        };
        let fake = Fake::new(vec![("/api/catalog/delta", json(&empty))]);
        let r = cache.sync_with(|p| fake.get(p)).unwrap();
        assert_eq!(r.status, SyncStatus::Unchanged);
        assert_eq!(
            *fake.calls.borrow(),
            ["/api/catalog/delta?since=10&catalog_id=db1"]
        );
    }

    #[test]
    fn a_delta_adds_updates_and_removes() {
        let cache = CatalogCache::in_memory().unwrap();
        cache.apply_snapshot(&snapshot()).unwrap();
        let mut gone = track(2, 7, "Two");
        gone.missing = true;
        let delta = CatalogDelta {
            catalog_id: "db1".into(),
            rev: 14,
            tracks: vec![track(1, 7, "One (Remastered)"), gone, track(4, 9, "Four")],
            albums: vec![album(9, "Giant Steps")],
            removed_albums: vec![8],
            removed_tracks: vec![3],
            ..Default::default()
        };
        let fake = Fake::new(vec![("/api/catalog/delta", json(&delta))]);
        let r = cache.sync_with(|p| fake.get(p)).unwrap();
        assert_eq!((r.status, r.changed), (SyncStatus::Updated, 6));
        assert_eq!(cache.state().unwrap(), Some(("db1".into(), 14)));
        let albums: Vec<_> = cache
            .albums()
            .unwrap()
            .into_iter()
            .map(|a| (a.id, a.track_count))
            .collect();
        assert_eq!(albums, [(7, 1), (9, 1)]);
        assert_eq!(
            cache.album_tracks(7).unwrap()[0].title.as_deref(),
            Some("One (Remastered)")
        );
        assert!(
            cache.tracks(&[2, 3]).unwrap().is_empty(),
            "missing and removed"
        );
    }

    #[test]
    fn full_resync_pulls_the_catalog_again() {
        let cache = CatalogCache::in_memory().unwrap();
        cache.apply_snapshot(&snapshot()).unwrap();
        let resync = CatalogDelta {
            catalog_id: "db2".into(),
            rev: 3,
            full_resync: true,
            ..Default::default()
        };
        let mut other = snapshot();
        other.catalog_id = "db2".into();
        other.rev = 3;
        other.tracks.truncate(1);
        let fake = Fake::new(vec![
            ("/api/catalog/delta", json(&resync)),
            ("/api/catalog", json(&other)),
        ]);
        let r = cache.sync_with(|p| fake.get(p)).unwrap();
        assert_eq!(r.status, SyncStatus::Full);
        assert_eq!(fake.calls.borrow().len(), 2);
        assert_eq!(cache.state().unwrap(), Some(("db2".into(), 3)));
        assert_eq!(
            cache.tracks(&[1, 2, 3]).unwrap().len(),
            1,
            "old rows are gone"
        );
    }

    #[test]
    fn offline_keeps_the_cache_and_an_old_server_is_unsupported() {
        let cache = CatalogCache::in_memory().unwrap();
        cache.apply_snapshot(&snapshot()).unwrap();
        let fake = Fake::new(vec![(
            "/api/catalog",
            Err(FetchError::Failed("connection refused".into())),
        )]);
        let r = cache.sync_with(|p| fake.get(p)).unwrap();
        assert_eq!(r.status, SyncStatus::Offline);
        assert_eq!(r.message.as_deref(), Some("connection refused"));
        assert_eq!(cache.albums().unwrap().len(), 2, "still browsable");

        let fresh = CatalogCache::in_memory().unwrap();
        let r = fresh.sync_with(|_| Err(FetchError::NotFound)).unwrap();
        assert_eq!(r.status, SyncStatus::Unsupported);
    }

    #[test]
    fn the_cache_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.db");
        CatalogCache::open(&path)
            .unwrap()
            .apply_snapshot(&snapshot())
            .unwrap();
        let reopened = CatalogCache::open(&path).unwrap();
        assert_eq!(reopened.state().unwrap(), Some(("db1".into(), 10)));
        assert_eq!(reopened.albums().unwrap().len(), 2);
    }
}
