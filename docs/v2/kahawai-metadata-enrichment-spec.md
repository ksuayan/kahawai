# Spec: metadata remapping & enrichment (Phase C)

## Status

**Local part implemented** on the `phase-c-local` branch (`normalize.rs`, migration `007_metadata_local.sql`, `scanner.rs`, `GET /api/enrichment/coverage`). The external part (MusicBrainz client, Cover Art Archive, the `enrich_metadata` job, Paused/Cancelled job states) is not started; see `docs/Backlog.md`.

What landed, and where it differs from the text below:

- **Grouping and sort keys** are computed at scan time, not in a separate Phase C job: albums match on `title_key` / `artist_key` (case and spacing collapsed), and `sort_title`, `sort_artist`, `artists.sort_name` move a leading "The "/"A " to the end. A cheap pass refreshes every key before and after each scan, so existing catalogs and renamed albums stay consistent. Display strings are never changed.
- **Year sanity** applies at scan time and, once, to existing rows in migration 007 (outside 1900 to next year becomes NULL). lofty reads only the first four digits of a date tag, so a typo like `19999` arrives as 1999 and is kept.
- **Embedded MusicBrainz IDs** (Picard's `MUSICBRAINZ_ALBUMID`, `MUSICBRAINZ_TRACKID`) are read during the scan and stored as `albums.mbid` and `tracks.recording_mbid`; the album is marked `matched` / `embedded`. Malformed IDs are ignored. Files already in a catalog get a one-time tag-only re-read on the next scan (`tracks.mbid_checked`), like the MQA backfill. DSF/DFF files and SACD ISOs are not read for IDs.
- **Not done on purpose: the `album_artist` NULL → `artist` fallback.** The scanner already groups files without an album-artist tag by folder, then by artist, promoting mixed folders to "Various Artists". A blanket fallback to the track artist would split compilations back into one album per artist, the regression the album-grouping fix removed.
- **Deferred to the external part:** `enrich_attempts`, `enriched_at`, and the `mb_cache` table (only lookups need them).

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
