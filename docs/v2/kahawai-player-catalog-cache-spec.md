# Spec: player catalog cache (no full re-pull on restart)

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
