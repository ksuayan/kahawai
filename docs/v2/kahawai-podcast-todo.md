# Kahawai Podcasts — TODO

Buildable feature list with estimated sizing and status.
Sizes: XS < S < M < L. Spec: `kahawai-podcast-spec.md` (2026-10-02).

| # | Feature | Size | Status | Notes |
|---|---------|------|--------|-------|
| 1 | `podcast_feeds` table + subscribe/unsubscribe API + URL probe | S | Not started | D1 |
| 2 | RSS 2.0 + Atom parser; iTunes namespace (`duration`, `image`, `explicit`); Podlove chapters stored | M | Not started | D1; chapters stored, UI later |
| 3 | OPML import + export | S | Not started | D1; never lock in subscriptions |
| 4 | Background refresh scheduler (persistent jobs; default 6 h, staggered) | M | Not started | D2 |
| 5 | `podcast_episodes` table; GUID-keyed upsert; feed-truncation handling | S | Not started | D2 |
| 6 | Episode list API (newest-first, unplayed filter, played state, 97% rule) | S | Not started | D2 |
| 7 | Failing-feed errors surfaced (`last_error`, UI badge) | XS | Not started | D2; never silently drop |
| 8 | Download manager: queued jobs, resume via Range, file layout `Show/date - title.ext` | M | Not started | D3 |
| 9 | Auto-download rules: keep-N latest unplayed; delete oldest played first | S | Not started | D3 |
| 10 | Played-file janitor (`delete_played_after_days`, 0 = never) | XS | Not started | D3 |
| 11 | Stream-undownloaded-episodes direct from enclosure URL | S | Not started | D3; no forced download |
| 12 | `podcast_positions` + throttled PUT + per-day session history (reuse audiobook pattern) | S | Not started | D4 |
| 13 | Per-feed settings: speed, skips, auto-download, keep-N, auto-advance | S | Not started | D4 + D7 |
| 14 | Player: subscriptions view (artwork, unread counts, failing badges) | S | Not started | D5 |
| 15 | Player: feed/episode views (download states, filters) | M | Not started | D5 |
| 16 | Player: episode view — sanitized show notes, chapters, mark played | S | Not started | D5 |
| 17 | "Up Next" episode queue; third now-playing context (music/audiobook/podcast) | S | Not started | D5 |
| 18 | Downloads management UI (sizes, per-feed rules editor) | S | Not started | D5 |
| 19 | Discovery: iTunes Search API proxy + charts + one-tap subscribe | S | Not started | D6 |
| 20 | Silence trimming / Smart-Speed DSP spike | M | Not started | Backlog; spike before committing |

Suggested build order: 2 (spike the parser on real feeds first) → 1 → 5 → 4 → 6 → 7 → 3 → 8 → 9 → 10 → 11 → 12 → 13 → 17 → 14 → 15 → 16 → 18 → 19 → 20.
