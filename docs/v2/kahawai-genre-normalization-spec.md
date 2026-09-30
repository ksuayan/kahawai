# Spec: genre normalization + browse/search

## Status

**Implemented** on the `genre-cleanup` branch: `genre.rs` (normalizer and refresh), `genre_aliases.rs` (alias table and keyword list), migration `009_genres.sql`, `GET /api/genres`, `GET /api/genres/{name}/tracks`, `GET /api/genres/report`, genre in the search index, `Genre` in `kahawai-core`, `genres` / `genre_tracks` in `kahawai-player-api`, and a Genres view in the player (sidebar, shortcut `g`).

Measured on the real library (888 distinct raw values on 70,119 tagged tracks of 81,316): **65 genres**. That's the 62-genre taxonomy plus 3 leftovers (a label, an album title, a band: 42 tracks), left under their own names for curation. 193 tracks were mapped by the keyword fallback, and 1,613 tracks carry values that aren't genres ("Other", "Unknown genre", "Divers", "0", a URL).

Where it differs from the text below:

- **No tag re-read to backfill.** The raw tag is already in `tracks.genre`, so `genre_map` and `track_genres` are rebuilt from it at startup (when no startup scan runs), at the start of every scan, and at its end. An edit to the alias table applies on the next scan, and a rescan with an edited tag updates the mapping without rehashing anything.
- **No `genres` table.** Counts come from `track_genres` (present tracks only). `genre_map(raw, genre, how)` records how each raw value was mapped, for the report.
- **Keyword fallback.** A value the alias table doesn't list is mapped by the genre words inside it ("Pinoy Rock" -> Rock, "Uplifting Trance" -> Trance) before falling back to itself. The report lists these separately (`by_keyword`), next to `unmapped` and `ignored`.
- **Not-genre values are dropped:** placeholders ("Other", "Unknown", "Various", "Divers", "Onbekend"), bare numbers, "Genre_013"-style values and web addresses. This departs from "never dropped" only for values that aren't genres. The raw tag is still kept.
- **Separators** also include a spaced dash ("Punk - New Wave - Pop"). Brackets anywhere are removed ("New Wave (A-Z)"). Spelling variants match on letters only ("Synth-pop", "Synthpop", "Synthie Pop").
- **ID3v1 numbers:** none occur in the real library. `(17)Rock` keeps its name, and a bare number is dropped.
- **No `tracks.rev` bump** (it doesn't exist in this codebase). The scan's `CatalogUpdated` event makes players reload, including genres.
- **The search index stores the raw tag,** so "jazz" finds jazz tracks and "rap" finds "Hip-Hop/Rap" ones.
- **Player:** genres are chips sorted by track count. A genre's tracks load 200 at a time with "Show more", and Play plays the loaded tracks.
- **Taxonomy:** grown from the starter list to fit the library, for example Downtempo, Lounge, Chillout, Breakbeat, New Age, Easy Listening, Instrumental, Vocal, Acoustic and World. Sub-genres fold into their parent (Deep House -> House, Hard Bop -> Jazz).

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
