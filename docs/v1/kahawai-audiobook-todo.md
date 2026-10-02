# Kahawai Audiobook — TODO

Buildable feature list with estimated sizing and status.
Sizes: XS < S < M < L. Spec: [kahawai-audiobook-spec.md](kahawai-audiobook-spec.md) (2026-10-01). Built on `claude/audiobook`; nothing here has been listened to or tried on the Mac yet, so "Built" means the code and its tests exist.

| # | Feature | Size | Status | Notes |
|---|---------|------|--------|-------|
| 1 | Audiobook scan roots API + SQLite storage | S | Built | `GET/POST/DELETE /api/audiobook-roots`; a new folder starts a scan; overlapping folders are refused |
| 2 | Scanner: walk audiobook roots, `tracks.kind='audiobook'` | M | Built | `audiobooks::scan`; the music scan skips audiobook folders, never marks their files missing, and the catalog, search, genres, hashing and live counts leave them out |
| 3 | Book grouping: directory → book, part ordering (disc/track tags, filename fallback) | S | Built | folder = book, `Disc N`/`CD N` merge, `(disc, track)` order (disc taken from the folder when untagged), natural file-name fallback |
| 4 | Folder-name metadata parsing (`Author/Series/Vol N - Year - Title {Narrator}`) | S | Built | `parse_folder`, written from the convention, not from Audiobookshelf's code |
| 5 | m4b embedded chapter parsing | S | Built | Nero `chpl` chapters, as written by most m4b tools; QuickTime text-track chapters are not read (Backlog) |
| 6 | Tables: audiobooks, parts, chapters, positions, bookmarks, sessions, settings | S | Built | migration 014 (+015 `enriched_at`); `audiobooks` also has `path`, `year`, `meta_edited` |
| 7 | List/detail API (`GET /api/audiobooks`, `GET /api/audiobooks/{id}`, continue shelf) | S | Built | `GET /api/audiobooks` (`shelf`, `q`, `author`, `series`, `finished`) and `/{id}` |
| 8 | Position PUT + throttled auto-resume | S | Built | `PUT .../position`; the player saves every 10 s and on pause, stop, part change, seek and close |
| 9 | Bookmarks CRUD API | XS | Built | `GET/POST/PATCH/DELETE .../bookmarks` |
| 10 | Listening-session derivation + history API | S | Built | sessions derive from position updates (15-minute gap, or a new UTC day); `GET .../history` |
| 11 | Finished detection (97%) + mark-finished endpoint | XS | Built | 97% rule both ways, `POST .../finished` |
| 12 | Offset resolver (`GET /api/audiobooks/{id}/resolve?offset_ms=`) | XS | Built | `GET .../resolve?offset_ms=`; the player resolves locally with the same rule |
| 13 | Player: audiobook library view (grid, continue shelf, filters, search) | M | Built | library view: grid, Continue shelf, author/series/finished filters, search |
| 14 | Player: book detail (chapters, bookmarks, per-day history, progress) | S | Built | detail: chapters, bookmarks, history by day, progress, edit, look up |
| 15 | Playback speed UI (0.75–2.5x) + engine hookup | S | Built | speed 0.75 to 2.5 in the UI, per book, applied when the book starts |
| 16 | Time-stretch DSP stage (WSOLA, 0.5–3.0, bypass at 1.0) | M | Built | `timestretch.rs` (WSOLA, 0.5 to 3.0, bit-identical bypass at 1.0, seam-free hand-over, end-of-stream flush). Not an in-place `DspStage` (it changes the frame count). Measured, **not yet listened to** |
| 17 | Configurable skip back/forward + key bindings | XS | Built | per-book seconds (15/30 by default); keys `j` and `l` |
| 18 | Sleep timer (durations + end-of-chapter, 10 s fade) | S | Built | 5 to 60 minutes and end of chapter; 10 s fade; saves the position when it fires |
| 19 | Per-book settings persistence (speed, skips) | XS | Built | `audiobook_settings`; speed and skips come back with the book |
| 20 | Separate now-playing context (music vs audiobook) | S | Built | starting a book puts the music queue aside; Back to music restores it exactly; whole-book seek bar and chapter prev/next while a book plays; a restored book queue is taken up after a restart |
| 21 | Spoken-word DSP preset (voice-tuned EQ/loudness) | XS | Built | built-in EQ preset "Spoken word" plus a Voice button (EQ, loudness, limiter). Values are starting points, to tune by ear |
| 22 | Enrichment: Open Library (CC0, keyless, ≤3 req/s) | S | Built | Open Library, ~3 req/s, polite User-Agent; opt-in with the album lookup switch; fills blanks only |
| 23 | Enrichment: Google Books fallback | XS | Built | Google Books for what Open Library left blank |
| 24 | Cover resolution chain (embedded → OL → Google → placeholder) | XS | Built | embedded or folder cover, then Open Library, then Google Books; the "generated placeholder" is the client's book-less cover icon |
| 25 | Manual metadata edit UI | S | Built | Edit details dialog (`PATCH /api/audiobooks/{id}`); edited books are left alone by rescans and lookups |

Suggested build order: 6 → 1 → 2 → 3 → 4 → 5 → 7 → 8 → 12 → 16 (spike) → 15 → 13 → 14 → 9 → 10 → 11 → 17 → 18 → 19 → 20 → 21 → 22 → 23 → 24 → 25.

Added after the first build: duplicate copies of a book are found by content (see the spec's deviations) and hidden; the Server app has audiobook folders in its wizard and Settings, and Status shows audiobook scanning.
