# Kahawai Audiobook Support — Spec

Date: 2026-10-01. Status: **built** on `claude/audiobook` (see [the build list](kahawai-audiobook-todo.md) for what each item became); not yet listened to or tried on the Mac.
Style: compact, agent-ready. Sizes: XS < S < M < L.

## Goal

Make Kahawai Player a formidable audiobook player: a server-side audiobook
library separate from music, seamless multi-part books, pitch-corrected
playback speed, and best-in-class bookmark / position / history tracking.

## Non-goals (v1)

- Podcasts. (Schema reserves room; not built.)
- Audible `.aax` / any DRM content. Never.
- Multi-user accounts. `audiobook_positions` carries a reserved `user_id`
  column defaulting to 0; single user assumed.
- Cloud sync. The LAN server is the sync point; positions live server-side
  so they follow the listener across clients.

## Core unit

Position is always **book_offset_ms**: milliseconds from the start of the
book, never a file offset. Every feature (resume, bookmarks, history,
chapters) is expressed in book offsets, which makes multi-part books
seamless and positions portable.

## D1 — Audiobook library roots, scan, book grouping (P0, M)

- New API: `GET /api/audiobook-roots`, `POST /api/audiobook-roots {path, name}`,
  `DELETE /api/audiobook-roots/{id}`. Stored in SQLite (`audiobook_roots`).
- `tracks` gains `kind TEXT DEFAULT 'music'` (`'music' | 'audiobook'`).
  All music browse/search/queue paths exclude `kind='audiobook'`.
- Scanner walks audiobook roots separately from music roots and creates
  track rows with `kind='audiobook'`. Music scanner ignores audiobook roots.
- Grouping rules:
  1. Each directory containing audio files = one book. Nested `Disc N` /
     `CD N` folders merge into the parent book.
  2. Parts ordered by `(disc, track)` tags; fallback: natural filename sort.
  3. Single `.m4b`/`.m4a`: parse embedded chapter atoms → chapters; one part.
     Multi-file: one chapter per part by default (title = part title).
  4. Folder-name parsing (Audiobookshelf-compatible convention,
     reimplemented — do NOT copy their code, see D6):
     `Author/Series/Vol 1 - 1999 - Title {Narrator}/`.
  5. Field precedence: embedded tags win; folder parse fills blanks; series
     fields prefer folder parse.
- `audiobook_parts.start_offset_ms` = sum of prior parts' `duration_ms`.

## D2 — Data model + position/bookmark/history API (P0, M)

```sql
audiobook_roots(id INTEGER PK, path TEXT UNIQUE, name TEXT);
audiobooks(id INTEGER PK, root_id, title, author, narrator,
           series, series_index, cover_hash, duration_ms,
           added_at, finished_at NULL);
audiobook_parts(id INTEGER PK, book_id, track_id UNIQUE, part_index,
                title, start_offset_ms, duration_ms);
audiobook_chapters(id INTEGER PK, book_id, part_id, title,
                   start_offset_ms, duration_ms);
audiobook_positions(book_id INTEGER PK, book_offset_ms, updated_at,
                    user_id INTEGER DEFAULT 0);
audiobook_bookmarks(id INTEGER PK, book_id, book_offset_ms,
                    name, note, created_at);
audiobook_sessions(id INTEGER PK, book_id, started_at, ended_at,
                   start_offset_ms, end_offset_ms);
audiobook_settings(book_id INTEGER PK, speed REAL DEFAULT 1.0,
                   skip_back_s INTEGER DEFAULT 15,
                   skip_forward_s INTEGER DEFAULT 30);
```

- `GET /api/audiobooks` — list; `?shelf=continue` sorts by
  `positions.updated_at DESC`, unfinished first. Includes progress
  (`book_offset_ms / duration_ms`) per book.
- `GET /api/audiobooks/{id}` — detail: parts, chapters, bookmarks,
  position, settings, progress.
- `PUT /api/audiobooks/{id}/position {book_offset_ms}` — auto-resume.
  Player sends throttled: every 10 s during playback, plus on
  pause/stop/seek/part-change/app-close.
- `POST /api/audiobooks/{id}/bookmarks {book_offset_ms, name?, note?}`;
  `GET .../bookmarks`; `DELETE .../bookmarks/{bid}`.
- `GET /api/audiobooks/{id}/history` — listening sessions, newest first.
- `POST /api/audiobooks/{id}/finished` and auto-finish when
  `book_offset_ms >= 0.97 * duration_ms` (sets `finished_at`; clears on
  rewind below threshold by user action).
- Session derivation (server, on position PUT): if `now - updated_at >
  15 min` or calendar day changed since `updated_at`, close the current
  session (`ended_at = updated_at`, `end_offset_ms` = last offset) and
  open a new one. This yields the per-day "where I stopped" markers.

## D3 — Player audiobook UI (P0, M)

- Library view: book grid; "Continue listening" shelf (recency, unfinished
  first); filter by author / series / finished; search title/author/narrator.
- Book detail: cover, metadata, progress bar, chapter list (tap to jump),
  bookmarks (add at current position, rename, delete, tap to jump),
  listening history grouped by day ("Tue Sep 30 — stopped at 4:12:33,
  47 min listened").
- Playback bar: speed selector, skip back/forward, sleep timer, chapter
  next/previous.

## D4 — Playback controls + separate context (P0, S)

- Speed 0.75x–2.5x, pitch-corrected always (D5). No chipmunk mode.
- Skip back/forward: configurable per book (defaults 15 s / 30 s),
  single-key bindings.
- Sleep timer: 5/10/15/30/45/60 min + "end of chapter"; 10 s linear
  fade-out; on fire → pause + final position PUT.
- Per-book settings (`audiobook_settings`) applied automatically when a
  book starts.
- Separate now-playing context: `music` vs `audiobook`. Switching to a
  book pauses the music queue in place without destroying it; switching
  back resumes exactly. Never interleave.

## D5 — Time-stretch DSP stage (P0, M)

- New `DspStage`: `TimeStretchStage { rate: f32 }`, WSOLA implementation,
  f32 PCM, any channel count, rate range 0.5–3.0 (UI exposes 0.75–2.5).
- Speech-grade quality is sufficient; music-grade fidelity is not required.
- Placed first in the chain after decode for audiobook playback; full
  flush on seek.
- Position accounting: player advances `book_offset_ms` by
  `rate × wall-clock elapsed`; on seek, recompute from target offset.
- Bit-transparency rule: rate exactly 1.0 → bypass (bit-identical).

## D6 — Metadata enrichment (P1, S)

There is no MusicBrainz equivalent for audiobooks. Narrator data is the
gap: no open, audiobook-specific, community-curated database exists.

- Enrichment chain: embedded tags → folder parse (D1) → Open Library →
  Google Books → manual edit UI (always available, fields editable).
- Open Library: `openlibrary.org/search.json`, CC0 public-domain data, no
  key, courtesy limit ~1–3 req/s, polite `User-Agent`. Good for
  title/author; rarely has narrator.
- Google Books: fallback for contemporary titles/covers (keyless quota is
  small; free key raises it).
- Covers: embedded art → `covers.openlibrary.org` → Google Books →
  generated placeholder.
- Narrator: accept `{Narrator}` folder convention, tags, or manual entry.
  No open source; do not scrape Audible.
- Reference: Audiobookshelf (AGPL-3.0, like Kahawai) is the design
  reference — provider chain, folder conventions, match-ranking approach.
  **Do not copy its code**: sole-copyright must be preserved for possible
  future dual-licensing.
- Parked: Librivox API (public-domain catalogs only).

## D7 — Spoken-word DSP preset (P1, XS)

- Voice-tuned preset over the existing stages: high-pass ~80 Hz, gentle
  presence lift 2–5 kHz, loudness on, limiter ceiling conservative.
  Audiobooks are mastered quiet; this is nearly free given the current
  chain. Tune by ear; values above are starting points, not mandates.

## Streaming

- Parts are tracks: reuse `/stream/{track_id}` unchanged (ranges,
  Content-Length, HEAD). m4b/mp3 direct-stream; no transcode path needed.
- Chapter jump = seek to `part.start_offset_ms + chapter offset within
  part`, resolved server-side to `(track_id, track_offset_ms)`;
  `GET /api/audiobooks/{id}/resolve?offset_ms=N` returns that mapping.

## Phase gates

Zero warnings, green tests, clippy clean per phase, as usual.
v1 = D1–D5 + D7 + D6-local (tags/folder/manual). Online enrichment (D6)
follows as a fast follow.

## Where the build differs from this spec

- **Time stretch** is `timestretch.rs` with its own `process` that appends to an output buffer, not an in-place `DspStage`: a stretch changes the frame count. It is first in the chain after decode and resampling. Leaving it for exactly 1.0 hands the audio over raw, with no seam.
- **Chapters** come from Nero `chpl` atoms only. QuickTime chapter text tracks are not read.
- **Sessions** are derived on each position update (the latest session ends at the last update), and a day is a UTC day on the server; the player groups by local day.
- **Extra columns:** `audiobooks.path` (the book's folder), `year`, `meta_edited` (a hand edit is kept across rescans and lookups), `enriched_at` (asked once).
- **Hashing:** audiobook files are not content-hashed (they are many GB, and duplicate detection is by album).
- **Online lookup** needs the existing `enrichment_enabled` switch (off by default), because it sends titles off the LAN.
- **Playback speed** is not a saved engine setting: the book being played decides it, and the engine returns to 1.0 for music. Anything but 1.0 holds exclusive (bit-perfect) output back, like the EQ does.
