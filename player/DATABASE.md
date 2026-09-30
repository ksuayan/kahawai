# Player data: schema, key queries and indexes

How the player stores what it knows, which queries it depends on (its own
and the server's), which indexes serve them, and the rules that keep them
fast. Measured numbers come from a real library: 81,316 tracks, 5,754
albums, 16,317 artists, on a server whose database sits on a spinning disk.

Related specs: [catalog cache](../docs/v2/kahawai-player-catalog-cache-spec.md),
[genres](../docs/v2/kahawai-genre-normalization-spec.md),
[server](../docs/v1/kahawai-server-spec.md).

## Overview: where the player keeps things

The player owns one small database, its **catalog cache**. Everything else it
remembers is a JSON file or a directory of images.

| Store | Location | Owner | Holds |
|---|---|---|---|
| Catalog cache | `<app data dir>/catalog.db` (SQLite) | `kahawai-player-core::catalog` | The server's present tracks, albums, artists, genres, and the revision they reflect |
| Artwork cache | `<app cache dir>/artwork/<hash>.img` | `kahawai-player-core::artwork` | Cover images by content hash, LRU-capped (Settings: 512 MB / 1 GB / 2 GB) |
| Engine settings | `<app config dir>/engine-settings.json` | `EngineController` | Server URL, DSP settings, output choices, volume |
| Saved queue | `<app config dir>/queue.json` | `EngineController` | Queue tracks, position, repeat and shuffle |
| Artwork cap | `<app config dir>/artwork-cache.json` | Tauri shell | The chosen artwork cache size |
| Developer tools | `<app config dir>/developer.json` | Tauri shell | Settings → Developer tools (WebKit menu, Web Inspector) |
| UI state | `<app config dir>/ui-state.json` | `lib/uiState.ts` (through the shell) | `kahawai.viewPrefs` (list/grid, sort per view), `kahawai-player.theme`, `kahawai-player.eq-rows`, `kahawai-player.eq-user-presets`, `kahawai-player.analog-ab`, `kahawai.nav` (the last view), `kahawai.scroll` (scroll positions), `kahawai.search` (the last search) |

The app data dir holds what's costly to lose (losing `catalog.db` means a
full re-pull of about 49 MB). The cache dir holds what the OS may purge
(artwork refetches on demand).

**UI state** lives in a file, not the webview's `localStorage`: a development
build (served from `localhost:1420`) and a release build (`tauri://localhost`)
are different origins with separate storage, and clearing webview data wipes
it. `main.ts` loads `ui-state.json` before any store is created; `uiGet` /
`uiSet` read it and write every change through. `localStorage` stays as a
mirror (the store in a plain browser and in tests), and values saved there
by earlier versions move into the file the first time they're read. Every
read tolerates a missing or unreadable value.

The server owns the library itself. The player never writes to it except
through the HTTP API (playlists, jobs, settings).

## The player's schema: `catalog.db`

```sql
PRAGMA journal_mode = WAL;
PRAGMA user_version = 1;          -- SCHEMA_VERSION in catalog.rs

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,       -- 'catalog_id', 'rev', 'genres'
    value TEXT NOT NULL
);
CREATE TABLE tracks (
    id       INTEGER PRIMARY KEY, -- server track id
    album_id INTEGER,             -- copied out of the JSON for the index
    json     TEXT NOT NULL        -- the server's Track, as sent
);
CREATE INDEX tracks_album ON tracks(album_id);
CREATE TABLE albums  (id INTEGER PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE artists (id INTEGER PRIMARY KEY, json TEXT NOT NULL);
```

- **`meta`:**
  - `catalog_id`: which server database the cache came from;
  - `rev`: the catalog revision it reflects;
  - `genres`: the genre list, as JSON.

  With no `catalog_id`/`rev`, the cache has never been filled.
- **Rows are the server's JSON,** keyed by id. The cache never mirrors the
  server's columns, so a new field on `Track` or `Album` needs no player
  migration. Only what the cache queries by gets its own column (`album_id`).
- **Only present tracks are kept.** A track that goes missing on the server
  arrives in a delta with `missing: true` and is deleted here.
- **Album track counts are counted, not stored:** the server sends albums
  with `track_count: 0`, and `albums()` fills it from `tracks`.
- **Schema changes:** bump `SCHEMA_VERSION`. On open, a different
  `user_version` drops the tables, and the next sync pulls everything again.
  The file only ever holds a copy, so this is the whole migration story.

## Key queries

### In the player (`catalog.rs`)

| Purpose | Query | Served by |
|---|---|---|
| Album list with counts | `SELECT album_id, COUNT(*) FROM tracks WHERE album_id IS NOT NULL GROUP BY album_id`, then `SELECT json FROM albums ORDER BY id` | `tracks_album` (covering), `albums` PK |
| Artist list | `SELECT json FROM artists ORDER BY id` | PK |
| An album's tracks (offline) | `SELECT json FROM tracks WHERE album_id = ?` | `tracks_album` |
| Tracks by id (offline queue, playlists) | `SELECT json FROM tracks WHERE id = ?` (prepared once, run per id) | PK |
| Cache state | `SELECT value FROM meta WHERE key = ?` | PK |
| Apply snapshot | one transaction: delete all, then `INSERT OR REPLACE` each row | PKs |
| Apply delta | one transaction: `INSERT OR REPLACE` changed rows, `DELETE` removed and newly missing ones, update `meta` | PKs |

The UI never queries SQLite directly. It calls the shell commands
`catalog_cached`, `catalog_sync`, `catalog_album_tracks` and
`catalog_tracks`, which run the queries above on a blocking worker.

### On the server, for the player

These are the queries behind the endpoints the player calls. Times are
server-side, measured on the real library after the fixes in migration 011
(the `real_db_timings` test in `crates/kahawai-server/src/main.rs` repeats
them against a database copy).

| Endpoint | Query shape | Index used | Time |
|---|---|---|---|
| `GET /api/catalog/delta` (every start) | Nothing changed: `meta` only. Otherwise `… WHERE rev > ?` on tracks, albums, artists, plus tombstones | `meta` PK, `idx_*_rev`, `idx_catalog_tombstones_rev` | **11 ms** when nothing changed |
| `GET /api/catalog` (first start, full resync) | Present tracks, all albums and artists, in one read transaction | table scans (reads everything by design) | 5 s, 48.7 MB |
| `GET /api/albums?page=` | Albums `ORDER BY COALESCE(sort_title, title)`, with `(SELECT COUNT(*) FROM tracks t WHERE t.album_id = albums.id AND t.missing = 0)` per row | `idx_tracks_album` for each count | 1.0 s for all 12 pages |
| `GET /api/albums/{id}` | `WHERE album_id = ? AND missing = 0 ORDER BY disc_no, track_no, id` | `idx_tracks_album` | 1.7 ms |
| `GET /api/artists/{id}` | `album_artists` joined to `albums` `WHERE artist_id = ?`, counts as above | `idx_album_artists_artist` | 39 ms (595 albums) |
| `GET /api/genres` | `SELECT genre, COUNT(*) FROM track_genres GROUP BY genre` | `idx_track_genres_genre` (covering) | 13 ms |
| `GET /api/genres/{name}/tracks` | `track_genres` joined to `tracks` `WHERE genre = ?`, sorted by artist, album title or year, `LIMIT/OFFSET` | `idx_track_genres_genre`, then tracks PK | 47 ms (first 200 of 15,286) |
| `GET /api/search` | FTS5 `MATCH` on `search_fts(title, album, artist, genre)`, top 50 by rank | FTS5 index | – |
| `GET /api/artwork/{hash}` | `SELECT mime, bytes FROM artwork WHERE hash = ?` | `artwork` PK | – |

## Indexes

### Player (`catalog.db`)

| Index | Serves |
|---|---|
| `tracks` PK (`id`) | tracks by id, upserts and deletes |
| `tracks_album` (`album_id`) | an album's tracks, album track counts |
| `albums` PK, `artists` PK, `meta` PK | lookups, upserts and deletes |

### Server (`music.db`), as of migration 011

| Table | Index | Serves |
|---|---|---|
| `tracks` | PK `id` | row lookups, keyset paging |
| | `UNIQUE(path)` | the scanner's "is this file known?" |
| | `idx_tracks_album(album_id)` | album pages and album track counts |
| | `idx_tracks_rev(rev)` | catalog deltas |
| | `idx_tracks_hash_pending(id) WHERE hash IS NULL` | the hashing job's pending rows (partial: empty once all are hashed) |
| `albums` | PK `id` | row lookups |
| | `idx_albums_title_key(title_key)` | the scanner matching a file to its album |
| | `idx_albums_enrich_status` | albums waiting for an online lookup |
| | `idx_albums_rev(rev)` | catalog deltas |
| `artists` | PK `id`, `UNIQUE(name)` | lookups, matching by name during scans |
| | `idx_artists_rev(rev)` | catalog deltas |
| `album_artists` | PK `(album_id, artist_id)` | an album's artists |
| | `idx_album_artists_artist(artist_id)` | an artist's albums |
| `track_artists` | PK `(track_id, artist_id)` | a track's artists |
| `track_genres` | PK `(track_id, genre)` | the genre refresh |
| | `idx_track_genres_genre(genre)` | genre counts and a genre's tracks |
| `genre_map` | `idx_genre_map_raw(raw)` | joining raw tags to canonical genres |
| `catalog_tombstones` | PK `(kind, id)`, `idx_catalog_tombstones_rev` | deleted rows in deltas |
| `meta` | PK `key` | `catalog_rev`, `catalog_id` |
| `playlist_tracks` | PK `(playlist_id, position)` | a playlist's tracks in order |
| `jobs` | PK `id`, `idx_jobs_status` | job lookups, restart rules |
| `artwork` | PK `hash` | cover bytes |
| `mb_cache` | PK `query_hash` | cached MusicBrainz responses |
| `search_fts` | FTS5 | text search |

**Removed in migration 011, and why:**
- `idx_tracks_missing`: nearly every row has `missing = 0`, so it narrowed
  nothing. Worse, without statistics the planner chose it over
  `idx_tracks_album`, which scanned every track per album: about 2 minutes to
  list all albums.
- `idx_albums_title`: albums match on `title_key` since migration 007.
- `idx_tracks_hash`: nothing looks a track up by its hash. The pending-hash
  partial index replaces it.

### Planner statistics

`db::open` runs `ANALYZE` the first time (0.8 s on the real library), then
`PRAGMA optimize` on every open and after every scan. `optimize` re-analyzes
only tables whose size changed a lot, so it costs almost nothing when
nothing did (the server opens in 14 ms). Without statistics SQLite guesses
how selective each index is, and on this schema it guessed wrong (see
`idx_tracks_missing` above).

## Best practices

**Every query**
- **Check the plan, on real data.** Run `EXPLAIN QUERY PLAN` against a copy
  of a real database (`sqlite3 music.db "VACUUM INTO 'copy.db'"` is safe while
  the server runs). A 10-row test database plans differently from an
  80,000-row one. A hot query that must use a specific index gets a test
  (see `statistics_and_index_choices` in `db.rs`).
- **Time it at library scale** before calling it done: `real_db_timings`
  exists for this. Anything the player runs on every start or every page
  view should be in milliseconds.
- **No N+1 queries.** Fetch a list's counts or related rows in the same query
  (a correlated subquery on an indexed column, or a `GROUP BY`), not one
  query per row. The album list used to run 5,754 extra queries.
- **Page anything that can be large,** with `LIMIT`/`OFFSET` and an explicit
  `ORDER BY`. Sort on the server when the list is paged, or later pages come
  back in the wrong order (a genre's tracks do this).
- **Read related data in one transaction** when it has to agree (the catalog
  snapshot and delta pin one WAL snapshot, so their rows match the revision
  sent with them).

**Indexes**
- **Index for a query that exists,** not for a column that might be searched
  someday. Every index costs space and a write on every insert and update:
  the scan writes about 100k rows.
- **Skip low-selectivity indexes.** A flag that's the same on nearly every row
  (`missing`, `decodable`, `mqa`) doesn't narrow anything, and can mislead
  the planner. If the rare value matters, use a partial index on it (as
  `idx_tracks_hash_pending` does for `hash IS NULL`).
- **A composite key only serves queries on its leading column.**
  `album_artists(album_id, artist_id)` can't find an artist's albums; that
  needed its own index.
- **Keep statistics current.** Never ship a schema change without checking
  that `db::optimize` still runs (at open and after scans).

**Changing data the player caches**
- **Every change a player shows must bump a revision.** Triggers from
  migration 010 do this for `tracks`, `albums` and `artists` automatically,
  whoever writes. A new column the player displays must be added to the
  trigger's `WHEN` list, or the change won't reach cached players until a
  full resync.
- **Deleting a row leaves a tombstone** (the delete triggers do it). Prefer
  marking rows missing to deleting them, as the scanner does.
- **Keep the startup path cheap.** The delta call with nothing changed must
  stay an index lookup: it runs on every player start.

**Preferences**
- **Save UI preferences with `uiGet`/`uiSet`, never `localStorage` directly,**
  so they survive restarts in every build. Validate what you read (an older
  or hand-edited file may hold anything) and fall back to a default.
- **Batch high-frequency changes:** a slider or a scroll sends many values a
  second. Volume is written at most every 0.4 s (and on quit), scroll
  positions likewise.

**The player's cache**
- **Store the server's JSON, index only what you query by.** New fields then
  need no cache migration.
- **Change the cache schema by bumping `SCHEMA_VERSION`.** It's a copy: drop
  and re-pull rather than migrate.
- **Never let a cache failure break the app.** An unusable file falls back to
  an in-memory cache. `offline` keeps showing the cache. An older server (a
  404 from the catalog endpoint) falls back to reading the lists directly.
- **Run cache work off the UI thread.** The shell commands use
  `spawn_blocking`; don't add a synchronous one.

**Migrations (server)**
- **One numbered file per change,** applied once each in its own transaction
  (`db::run_migrations`).
- **No semicolons in `--` comments** (the splitter strips comment lines, then
  splits on `;`). `CREATE TRIGGER … END` blocks are kept whole.
- **Explain why in the file header** (what it changes, and what it measured
  or fixed), as 010 and 011 do.
