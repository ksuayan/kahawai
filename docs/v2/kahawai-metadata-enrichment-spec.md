# Spec: metadata remapping & enrichment (Phase C)

## Status

**Implemented.** The local part is on `phase-c-local` (`normalize.rs`, migration `007_metadata_local.sql`, `scanner.rs`, `GET /api/enrichment/coverage`). The external part is on `phase-c-enrich`: `musicbrainz.rs` (client), `enrich.rs` (worker), migration `008_enrichment.sql`, the `enrich_metadata` job with the `Paused` and `Cancelled` states, and Settings → Album info in the desktop app.

### Local part: what landed, and where it differs from the text below

- **Grouping and sort keys** are computed at scan time, not in a separate Phase C job: albums match on `title_key` / `artist_key` (case and spacing collapsed), and `sort_title`, `sort_artist`, `artists.sort_name` move a leading "The "/"A " to the end. A cheap pass refreshes every key before and after each scan, so existing catalogs and renamed albums stay consistent. Display strings are never changed.
- **Year sanity** applies at scan time and, once, to existing rows in migration 007 (outside 1900 to next year becomes NULL). lofty reads only the first four digits of a date tag, so a typo like `19999` arrives as 1999 and is kept.
- **Embedded MusicBrainz IDs** (Picard's `MUSICBRAINZ_ALBUMID`, `MUSICBRAINZ_TRACKID`) are read during the scan and stored as `albums.mbid` and `tracks.recording_mbid`; the album is marked `matched` / `embedded`. Malformed IDs are ignored. Files already in a catalog get a one-time tag-only re-read on the next scan (`tracks.mbid_checked`), like the MQA backfill. DSF/DFF files and SACD ISOs are not read for IDs.
- **Not done on purpose: the `album_artist` NULL → `artist` fallback.** The scanner already groups files without an album-artist tag by folder, then by artist, promoting mixed folders to "Various Artists". A blanket fallback to the track artist would split compilations back into one album per artist, the regression the album-grouping fix removed.

### External part: what landed

- **Opt-in.** `enrichment_enabled` (config, default off) is set from Settings → Album info in the desktop app, applied live and saved. Turning it off cancels a lookup in progress. While it's on, every completed scan queues a lookup if an album is waiting and no lookup is running or paused.
- **What is looked up:** albums with no `mbid` whose status is `pending`, or `error` with fewer than 3 attempts. Picard-tagged albums cost nothing. `no_match` is terminal.
- **MusicBrainz etiquette** (their published rules: about 1 request per second per IP, 503 when over, a meaningful User-Agent; there is no quota and no API key):
  - one request at a time, spaced at least 1.1 s apart plus up to 0.25 s of random jitter;
  - User-Agent `Kahawai/<version> ( https://github.com/ksuayan/kahawai )`;
  - 503/429 honours `Retry-After`, otherwise backs off exponentially up to 60 s, and stops after 5 retries;
  - every response is kept in `mb_cache` (keyed by the request URL's hash), so re-runs and restarts don't ask again.
- **Matching** is album-level: a search on `release:"title" AND artist:"artist"` (10 candidates), each scored 0–1 as 0.35 × title similarity + 0.25 × artist similarity + 0.25 × track-count agreement + 0.15 × MusicBrainz's own score. A missing artist or track count scores 0.5 for that part. The best candidate is accepted only at or above `enrichment_min_confidence` (default 0.9; Settings offers Relaxed 80%, Balanced 90%, Strict 95%; clamped to 0.5–1.0). MusicBrainz has no minimum-score parameter of its own, so this threshold is ours.
- **Fills blanks only:** a match sets `mbid`, `enrich_status = 'matched'`, `enrich_source = 'musicbrainz'`, `enriched_at`, and `year` only when it's NULL. The Cover Art Archive front cover (`front-500`) is attached only when the album has no artwork (`artwork_source = 'caa'`, stored in the content-addressed `artwork` table). Tags are never overwritten.
- **Outages:** no internet connection, DNS failure, a timeout, 5xx, or 503s past the retry limit count as *unreachable*, not as the album's fault. The job pauses itself with a message ("Nothing was lost; N albums still to look up"), no album is charged an attempt, and Resume carries on. A bad response for one album (for example a parse error) marks just that album `error` and counts an attempt.
- **Job control:** `POST /api/jobs/{id}/pause`, `/resume`, `/cancel` (album info lookups only). The worker checks the job's status between albums: paused, it waits without sending anything; cancelled, it stops. A paused job survives a server restart, and Resume starts a new worker when none is running.
- **Coverage** (`GET /api/enrichment/coverage`) now also reports `matched_online` and `no_match`, and `pending_lookup` includes failed albums that will be retried.

### Deviations from the text below

- **No per-track recording lookup** and no track-duration scoring: matching is by release title, artist, and track count. Recording IDs come only from embedded tags.
- **No `tracks.rev` / `catalog_rev` bump:** those don't exist in this codebase. A run that matched anything sends the existing `CatalogUpdated` event, and players reload.
- **Acceptance** is covered by tests against a local stub server (request spacing and User-Agent, cache hits, retry on busy, offline and down vs a bad response, fills blanks only, `no_match` not re-requested, retry limit, pause, resume, and cancel), not by a 500-album dry run against the live service.

## Goal

Fix inconsistent metadata and fill gaps without touching embedded tags:
deterministic local remapping plus external enrichment (MusicBrainz / Cover
Art Archive) as a resumable background job. This is Phase C of the scan
pipeline: A ingest → B hash → C enrich.

## Current state (verified 2026-09-28)

- The scanner stores lofty tags verbatim (`title/artist/album/album_artist/
  genre/year`). Genre normalization is a separate spec; nothing else is
  normalized.
- No external metadata source. Albums without embedded art have no artwork.

## Design

### Remap — deterministic, local

Runs in Phase C (keeps Phase A fast; CPU-only so it can share Phase B's
worker pool or run as its own job — implementer's choice):

- `album_artist` fallback: NULL → `artist` (fixes Various Artists /
  compilation grouping).
- Grouping keys: whitespace/case-collapsed artist and album names used for
  grouping; display keeps the raw string.
- `sort_artist`, `sort_album` columns: move leading "The "/"A " to the end
  (`"Beatles, The"`); the player's sorted lists use these.
- `year` sanity: outside 1900–(current year + 1) → NULL.

### Enrichment — external, background job `enrich_metadata`

- **Sources:** MusicBrainz (release/recording lookup) + Cover Art Archive
  (front cover for art-less albums). Both free; MB requires a proper
  User-Agent and **1 req/s** rate limit.

### Embedded MusicBrainz IDs — Picard-tagged files skip the API

A large share of the library was already processed with MusicBrainz Picard,
which writes standard identifiers into the files (`MUSICBRAINZ_ALBUMID`,
`MUSICBRAINZ_TRACKID`, `MUSICBRAINZ_RELEASEGROUPID`, …). The pinned
lofty 0.22 exposes these as `ItemKey::MusicBrainzReleaseId`,
`MusicBrainzReleaseGroupId`, `MusicBrainzRecordingId`/`MusicBrainzTrackId`,
`MusicBrainzArtistId` (confirmed against lofty 0.22 docs).

- **At scan time** (`analyze_meta`, Phase A): read the embedded MBIDs and
  store them directly — `albums.mbid`, `tracks.recording_mbid`. No extra
  file I/O; the tag is already open.
- Files with embedded MBIDs are marked `enrich_status = 'matched'` with
  `enrich_source = 'embedded'` immediately: **no API call, no rate-limit
  cost.** Picard's embedded artwork is likewise picked up by the existing
  artwork extraction — CAA is only consulted when no embedded art exists.
- **Phase C matching order:** (1) embedded MBID → done; (2) tag-based MB
  search for the remainder only. The 1 req/s budget therefore applies solely
  to the non-Picard portion of the library, which may be a small fraction.
- **Matching (non-Picard files):** tag-based (artist + album + track count/
  durations), score threshold; below threshold → left unenriched. No guessing.
- **Throughput:** worst case ~12k albums @ 1 req/s ≈ 3.5 h minimum, but
  embedded-MBID albums skip the API entirely, so actual load is proportional
  to the non-Picard share. The job must still be resumable, throttled, with
  jittered backoff on 503.
- **Authority:** embedded tags always win. Enrichment fills NULLs only
  (missing year, missing artwork, MBIDs). Store `enrich_status`,
  `enriched_at`, `enrich_source`; never overwrites embedded values by default.
- **Response cache:** `mb_cache(query_hash PK, response_json, fetched_at)` —
  restarts and re-runs don't re-hit the API.

### Schema

- `albums`: `mbid TEXT`, `enrich_status TEXT` (`'pending' | 'matched' |
  'no_match' | 'error'`, default `'pending'`), `enrich_attempts INTEGER`,
  `enriched_at INTEGER`, `enrich_source TEXT` (`'embedded' | 'musicbrainz' |
  NULL`), `artwork_source TEXT` (`'embedded' | 'caa' | NULL`),
  `sort_artist`, `sort_album`. `no_match` is terminal (not retried on rescan);
  `error` is retry-eligible.
- `tracks`: `recording_mbid TEXT`.
- `mb_cache` table as above.

### API / jobs

- `GET /api/albums/{id}` includes `mbid`, `artwork_source`.
- New persistent job type `enrich_metadata`, controlled via the existing
  `/api/jobs` surface and the desktop scan view.
- Coverage reporting: `GET /api/enrichment/coverage` →
  `{ total_albums, with_embedded_mbid, pending_lookup }`, computed from the
  `albums` table after Phase A. The desktop job dashboard shows this before
  Phase C runs, so the real API workload is known up front rather than
  estimated from "a good portion".
- Extend `JobStatus` (currently Queued/Running/Done/Failed,
  `kahawai-core/src/api.rs:182`) with `Paused` / `Cancelled`, so a multi-hour
  enrichment run can be suspended and resumed. Update `status_to_str` /
  `status_from_str` and the S9 restart rule accordingly. Additive only —
  existing states and their semantics are unchanged.
- CAA front images land in the existing content-addressed `artwork` table
  (dedupe by hash already handles repeats).

### Interplay

- Enrichment that changes visible data bumps `tracks.rev` / `catalog_rev`
  (see player-catalog-cache spec) so player deltas pick it up.

## Tasks

1. Deterministic remaps + sort-key columns + unit tests (messy-artist
   fixtures, `"Last, First"` handling, year clamping).
2. `enrich_metadata` job: MB client (1 req/s throttle, UA, backoff), CAA
   artwork fetch, tag-based match scoring with threshold.
3. Schema migration + `mb_cache`.
4. Wire into `/api/jobs` and desktop scan-view progress.
5. Docs: pipeline overview (A → B → C) in `docs/v1/kahawai-server-spec.md`.

## Acceptance

- 200-album messy fixture: artist/album grouping correct, sort keys sane,
  no display strings altered.
- 500-album enrichment dry-run: ≥ 80% matched above threshold, zero embedded
  values overwritten, kill -9 resumes without re-requesting cached queries.
- Picard fixture: files with embedded MBIDs get `enrich_status = 'matched'` /
  `enrich_source = 'embedded'` at scan time with zero HTTP requests.
- Rate limiter provably ≤ 1 req/s (test with mock clock or local stub server).
- Pause/resume round-trip preserves enrichment position; `no_match` rows are
  not re-requested on subsequent runs; `error` rows are retried with backoff.
- Existing gates green (server tests, clippy `-D warnings`, `cargo fmt`).

## Non-goals

- AcoustID/Chromaprint fingerprinting (needs an external binary; revisit if
  tag matching proves inadequate). Per-user metadata overrides UI. Lyrics.
