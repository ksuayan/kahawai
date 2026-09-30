# Spec: player catalog cache (no full re-pull on restart)

## Status

**Implemented** on the `catalog-cache` branch: migration `010_catalog_rev.sql` and `catalog.rs` (server), `CatalogSnapshot` / `CatalogDelta` (`kahawai-core`), `catalog()` / `catalog_delta()` (`kahawai-player-api`), `catalog.rs` (`kahawai-player-core`, the cache and sync), the `catalog_*` commands in the player shell, and a cache-first `library.ts`.

Where it differs from the text below:

- **Revisions come from SQLite triggers,** not from the scan writer. Every change to a shown track, album or artist field takes the next `meta.catalog_rev`, whoever writes it (scan, tag backfill, album merges, key refreshes, online lookups). An update that doesn't change a shown value (a rescan rewriting identical tags, a content hash) doesn't count, so a no-op rescan costs nothing. Deleted rows (albums, when duplicates merge) leave a tombstone.
- **Albums and artists are tracked too,** not just tracks: covers, years and sort keys change through enrichment and key refreshes. The delta carries changed tracks, albums and artists whole, plus `removed_tracks` / `removed_albums` / `removed_artists`. A track that went missing comes back with `missing: true`, which the player treats like a removal, so there's no separate `missing_ids`.
- **`catalog_id`** (random per server database) travels with every snapshot and delta. A cache from another database (a new install, another server URL) gets `full_resync`, as does a revision from the future or a delta touching more than 20% of the catalog.
- **Start-up check is the delta call, not an ETag.** With a cache, the player asks `GET /api/catalog/delta?since=<rev>&catalog_id=<id>`: when nothing changed, that one small request is all the catalog traffic. `GET /api/catalog` (the full snapshot, about 45 MB of JSON for the real library's 81k tracks) runs only on the first start, and on `full_resync`.
- **Genres** come whole with every snapshot and delta (they're a short list), and are cached. A genre's track list still needs the server.
- **Player cache:** `kahawai-player-core::catalog` (rusqlite 0.32, which shares `libsqlite3-sys` with the server's sqlx), stored at `<app data dir>/catalog.db`. Rows are kept as the server's JSON keyed by id (tracks also by album id), and album track counts are counted from the cached tracks. It lives in player-core rather than the Tauri shell so it's tested with the workspace. The shell exposes `catalog_cached`, `catalog_sync`, `catalog_album_tracks` and `catalog_tracks`.
- **Offline:** the library, album detail and artist detail (albums matched by artist name) come from the cache, under a banner saying so. Search, genre track lists, playlists not yet cached, and playback need the server.
- **Older server** (no `/api/catalog`, a 404): the player reads the lists directly, as before.
- **Measured on the real library** (81,316 tracks): the start-up check with nothing changed takes 11 ms on the server; the full snapshot is 48.7 MB of JSON, built in about 5 s.
- **Artwork disk cache** was already built (the content-addressed, LRU-capped cache with the Settings size choice).

## Goal

A player restart never re-pulls the full catalog. The library renders instantly
from a local cache; only deltas cross the LAN. The player also works
(read-only) when the server is unreachable.

## Current state (verified at `d6dfde9`)

- Player `library.ts` (`player/ui/src/stores/library.ts`): `loadAll()` fetches
  **all albums (paginated) + all artists into in-memory Pinia refs on every
  startup**; tracks are fetched per album on demand. Zero persistence — a
  restart re-downloads everything.
- Server API (`main.rs:79`): albums, artists, track-by-id, search, playlists,
  artwork, scan, jobs, SSE events. **No catalog revision, no delta endpoint.**
- `Track` (`crates/kahawai-core/src/api.rs:10`) carries `id`, `hash`, `missing`,
  `genre`, etc.
- Artwork is already content-addressed (`/api/artwork/{hash}`) but the player
  keeps no disk cache (known v1 limit).

## Design

### Server

- `catalog_rev`: monotonic INTEGER in a `meta` table, bumped **once per
  committed scan batch** (the batched writer from the fast-scan spec).
- `tracks.rev`: set to `catalog_rev` on every insert/update — including genre
  remaps (see genre spec), so all mutations flow through one channel.
- `GET /api/catalog` → `{ rev, tracks, albums, artists, scanned_at }`.
- `GET /api/catalog/delta?since=<rev>` →
  `{ rev, upserted: Track[], missing_ids: number[], full_resync: bool }`.
  `upserted` = rows with `rev > since`. `missing_ids` = newly-missing tracks
  (current semantics mark missing rather than delete). If `since` is older
  than retention or the delta exceeds a threshold (e.g. 20% of the catalog),
  return `full_resync: true` and let the player re-pull.
- ETag/`If-None-Match` on `/api/catalog` is a cheap alternative for the
  no-change case; delta endpoint still required for the change case.

### Player

- **SQLite cache in the Tauri shell** (rusqlite; mirrors the server's
  `tracks`/`albums`/`artists` essentials plus `meta{rev}`), stored in the app
  data dir. Chosen over a JSON snapshot: ~125k tracks ≈ 40 MB JSON with a
  parse cost on every launch; SQLite gives indexed startup queries.
- **Startup flow:** render from cache immediately → `GET /api/catalog` →
  rev matches: done; else apply `delta` → update cached rev.
- **Offline:** server unreachable → stay on cache, banner
  "offline — showing cached library". Playback still requires the server.
- **Artwork disk cache:** keyed by content `{hash}`, LRU-capped (e.g. 1 GB);
  check disk before `GET /api/artwork/{hash}`.
- Queue persistence already exists — unchanged.

## Tasks

1. Server: `meta` table, `tracks.rev` column + backfill, rev bump in the scan
   writer; `GET /api/catalog`, `GET /api/catalog/delta`.
2. `kahawai-core`: `CatalogInfo` and `CatalogDelta` types.
3. `kahawai-player-api`: `catalog()` and `catalog_delta(since)` client calls.
4. Player shell (`player/src-tauri`): rusqlite cache DB + artwork disk cache;
   rewrite `library.ts` startup as cache-first → delta sync.
5. Tests: delta correctness (insert / update / newly-missing), rev
   monotonicity, offline startup from cache, `full_resync` path.

## Acceptance

- Second startup against an unchanged server: zero catalog HTTP traffic beyond
  one `GET /api/catalog` (assert via test double or request logs).
- Scan that adds/updates/marks-missing tracks → next player start applies the
  delta only; local counts match the server.
- Server down at launch: library browsable from cache with the offline banner.
- Existing gates green (server 147 tests, player suites, clippy, fmt).

## Non-goals

- No real-time push (SSE `/api/events` already exists for live updates —
  optional follow-up). No multi-server support.
