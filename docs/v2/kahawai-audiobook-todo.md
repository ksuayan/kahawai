# Kahawai Audiobook — TODO

Buildable feature list with estimated sizing and status.
Sizes: XS < S < M < L. Spec: `kahawai-audiobook-spec.md` (2026-10-01).

| # | Feature | Size | Status | Notes |
|---|---------|------|--------|-------|
| 1 | Audiobook scan roots API + SQLite storage | S | Not started | D1; `GET/POST/DELETE /api/audiobook-roots` |
| 2 | Scanner: walk audiobook roots, `tracks.kind='audiobook'` | M | Not started | D1; music scanner ignores these roots |
| 3 | Book grouping: directory → book, part ordering (disc/track tags, filename fallback) | S | Not started | D1; merge `Disc N`/`CD N` subfolders |
| 4 | Folder-name metadata parsing (`Author/Series/Vol N - Year - Title {Narrator}`) | S | Not started | D1; reimplement convention, no copied code |
| 5 | m4b embedded chapter parsing | S | Not started | D1; fallback = one chapter per part |
| 6 | Tables: audiobooks, parts, chapters, positions, bookmarks, sessions, settings | S | Not started | D2; includes reserved `user_id` |
| 7 | List/detail API (`GET /api/audiobooks`, `GET /api/audiobooks/{id}`, continue shelf) | S | Not started | D2 |
| 8 | Position PUT + throttled auto-resume | S | Not started | D2; 10 s throttle + pause/stop/seek/close |
| 9 | Bookmarks CRUD API | XS | Not started | D2 |
| 10 | Listening-session derivation + history API | S | Not started | D2; 15-min gap / day-change rule |
| 11 | Finished detection (97%) + mark-finished endpoint | XS | Not started | D2 |
| 12 | Offset resolver (`GET /api/audiobooks/{id}/resolve?offset_ms=`) | XS | Not started | Maps book offset → (track_id, track offset) |
| 13 | Player: audiobook library view (grid, continue shelf, filters, search) | M | Not started | D3 |
| 14 | Player: book detail (chapters, bookmarks, per-day history, progress) | S | Not started | D3 |
| 15 | Playback speed UI (0.75–2.5x) + engine hookup | S | Not started | D4; depends on #16 |
| 16 | Time-stretch DSP stage (WSOLA, 0.5–3.0, bypass at 1.0) | M | Not started | D5; hardest item — spike first |
| 17 | Configurable skip back/forward + key bindings | XS | Not started | D4; defaults 15 s / 30 s |
| 18 | Sleep timer (durations + end-of-chapter, 10 s fade) | S | Not started | D4 |
| 19 | Per-book settings persistence (speed, skips) | XS | Not started | D4; auto-applied on book start |
| 20 | Separate now-playing context (music vs audiobook) | S | Not started | D4; switching pauses, never destroys |
| 21 | Spoken-word DSP preset (voice-tuned EQ/loudness) | XS | Not started | D7; tune by ear |
| 22 | Enrichment: Open Library (CC0, keyless, ≤3 req/s) | S | Not started | D6; fast follow after v1 |
| 23 | Enrichment: Google Books fallback | XS | Not started | D6 |
| 24 | Cover resolution chain (embedded → OL → Google → placeholder) | XS | Not started | D6 |
| 25 | Manual metadata edit UI | S | Not started | D6; always available |

Suggested build order: 6 → 1 → 2 → 3 → 4 → 5 → 7 → 8 → 12 → 16 (spike) → 15 → 13 → 14 → 9 → 10 → 11 → 17 → 18 → 19 → 20 → 21 → 22 → 23 → 24 → 25.
