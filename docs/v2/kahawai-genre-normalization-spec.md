# Spec: genre normalization + browse/search

## Goal

Browse and search tracks by genre despite messy embedded tags. Thousands of
raw variants collapse into a curated canonical set. Nothing is discarded: the
raw tag is always preserved.

## Current state (verified at `d6dfde9`)

- `tracks.genre` stores the **raw tag verbatim**; `Track.genre` is exposed in
  the API (`crates/kahawai-core/src/api.rs:27`).
- The FTS index covers **(title, album, artist) only**
  (`scanner.rs:995`) — genre is not text-searchable.
- No genre browse endpoint; the player has no genre view.

## Design

### Normalization (server, at scan time)

`normalize_genre(raw: &str) -> Vec<String>`:

1. Lowercase, trim, strip bracketed suffixes (`"Rock [80s]"` → `"rock"`).
2. Split multi-value tags on `/`, `;`, `|`, `,` — but **not** `&`
   (`"R&B"` must survive).
3. Alias-map lookup per token → canonical name. Check whether the pinned lofty
   version already resolves ID3v1 numeric genres (`"(17)"`); if it does, drop
   the numeric branch.
4. Unmatched tokens: cleaned Title Case becomes its own canonical entry.
   Never dropped, never bucketed into "Other" — the long tail stays visible
   and curatable.

The alias map lives in `crates/kahawai-server/src/genre_aliases.rs` (static
table, easy to extend). Starter canonical taxonomy (~48) in the appendix.

### Schema

- `tracks.genre` (raw) **untouched**.
- New: `genres(name PRIMARY KEY)` + `track_genres(track_id, genre)` join —
  multi-genre tracks (`"Rock; Alternative"`) map to multiple canonical rows.
- Backfill for existing rows: **tag-only revisit** (same pattern as the MQA
  backfill — no rehashing).
- Genre changes bump `tracks.rev` (feeds the catalog-delta sync).

### API

- `GET /api/genres` → `[{ name, track_count }]`, count descending.
- `GET /api/genres/{name}/tracks?page&per_page` → paginated tracks.
- Add `genre` to the FTS index (`title, album, artist, genre`) so text search
  matches genres with no player change.
- `GET /api/genres/report` (curation aid): canonical genres by count plus top
  unmapped raw variants → the feedback loop for growing the alias map.

### Player

- Genre browse view: chips/cards sorted by count; selecting a genre lists its
  tracks (reuse the album-detail track rendering).
- Search box needs no change — genre hits arrive via the FTS update.

## Tasks

1. `normalize_genre` + alias table + unit tests on messy fixtures (case,
   separators, `R&B`, numeric, parentheticals, multi-value).
2. Migration: `genres`, `track_genres`; tag-only backfill job.
3. Wire normalization into the scan upsert path.
4. API endpoints + FTS index update.
5. `kahawai-core`: `Genre { name, track_count }` type.
6. `player-api` client calls + player genre browse view.

## Acceptance

- Fixture library with ~2,000 distinct raw variants → ≤ ~60 canonical genres;
  ≥ 95% correct on a 200-tag spot-check sample.
- `/api/genres` counts reconcile with track totals; FTS query `"jazz"`
  returns jazz tracks.
- Rescan with an edited genre tag updates the canonical mapping without
  rehashing the file.
- Existing gates green (server tests, player suites, clippy, fmt).

## Non-goals

- No ML genre classification. No per-user genre overrides (future).

## Appendix: starter canonical taxonomy

Rock, Alternative Rock, Indie Rock, Hard Rock, Punk, Metal, Pop, Indie Pop,
Synth-Pop, Hip-Hop, R&B, Soul, Funk, Jazz, Smooth Jazz, Blues, Country, Folk,
Americana, Classical, Baroque, Opera, Electronic, House, Techno, Ambient,
Drum & Bass, Trance, Reggae, Dub, Ska, Latin, Bossa Nova, Flamenco, African,
K-Pop, J-Pop, C-Pop, City Pop, Anime, Soundtrack, Musical, Jazz Fusion,
Progressive Rock, Psychedelic, Grunge, New Wave, Post-Punk, Shoegaze,
Experimental, Spoken Word, Children's, Holiday
