# Kahawai Podcasts — TODO

Buildable feature list with estimated sizing and status.
Sizes: XS < S < M < L. Spec: `kahawai-podcast-spec.md` (2026-10-02). "Built" means built and tested without real feeds or the Mac.

| # | Feature | Size | Status | Notes |
|---|---------|------|--------|-------|
| 1 | `podcast_feeds` table + subscribe/unsubscribe API + URL probe | S | Built | `POST/GET/DELETE /api/podcasts/feeds`; fetched and read before saving; duplicate guard on the normalized address; a web page is refused and the feed it points to is named |
| 2 | RSS 2.0 + Atom parser; iTunes namespace (`duration`, `image`, `explicit`); Podlove chapters stored | M | Built | `feed-rs` plus a cleaning pass (BOM, junk, UTF-16/Windows-1252, bare & and HTML entities, control characters, truncated feeds); `itunes:duration` read by us because `feed-rs` does not; Podlove chapters not stored yet |
| 3 | OPML import + export | S | Built | `POST /api/podcasts/feeds/import-opml` (file as the body; feeds read in the background, 4 at a time) and `GET .../export-opml` |
| 4 | Background refresh scheduler (persistent jobs; default 6 h, staggered) | M | Built | Background loop in `podcast_dl::spawn_scheduler`: a pass a minute after start, then every `podcast_refresh_hours` (default 6, 0 = off), feeds 5 s apart. It is a plain loop, not a persisted job, so a restart just starts the clock again |
| 5 | `podcast_episodes` table; GUID-keyed upsert; feed-truncation handling | S | Built | `podcast_episodes` keyed by (feed, guid); an entry with no guid is keyed by its audio address; dropped episodes are kept and flagged |
| 6 | Episode list API (newest-first, unplayed filter, played state, 97% rule) | S | Built | `GET /api/podcasts/feeds/{id}/episodes?unplayed=1`, mark played/unplayed. The 97% auto-played rule comes with playback positions |
| 7 | Failing-feed errors surfaced (`last_error`, UI badge) | XS | Built | `last_error` is stored on the feed and cleared when it works again; the UI badge comes with the Player views |
| 8 | Download manager: queued jobs, resume via Range, file layout `Show/date - title.ext` | M | Built | Downloads are jobs (`podcast_download`), resume with Range into a `.part` file, two at a time, saved as `Show/date - title.ext`; `POST/DELETE /api/podcasts/episodes/{id}/download`, `GET .../file` serves it with byte ranges |
| 9 | Auto-download rules: keep-N latest unplayed; delete oldest played first | S | Built | Newest `keep_n` unplayed episodes are fetched automatically; over the limit, played files go first, then the oldest |
| 10 | Played-file janitor (`delete_played_after_days`, 0 = never) | XS | Built | `delete_played_after_days` (default 7, 0 = never); the episode and its place stay |
| 11 | Stream-undownloaded-episodes direct from enclosure URL | S | Built | `/stream/{2^40 + id}` serves the download, or passes the enclosure through the server with the Player's Range (seeking works where the host supports it) |
| 12 | `podcast_positions` + throttled PUT + per-day session history (reuse audiobook pattern) | S | Built | migration 021; `PUT /api/podcasts/episodes/{id}/position`, `GET .../history`, `GET /api/podcasts/in-progress`; played at 97% |
| 13 | Per-feed settings: speed, skips, auto-download, keep-N, auto-advance | S | Built | `PUT /api/podcasts/feeds/{id}/settings` covers all of them; the Player applies speed and skips when an episode starts |
| 14 | Player: subscriptions view (artwork, unread counts, failing badges) | S | Built | Podcasts section: grid or list, Up Next, Continue listening, add by address, OPML import/export |
| 15 | Player: feed/episode views (download states, filters) | M | Built | show page: newest first, unplayed filter, downloaded (size) / downloading (%) / streams, download, delete, Up Next, mark played |
| 16 | Player: episode view — sanitized show notes, chapters, mark played | S | Built | sanitized notes (formatting, lists, web links; links open in the browser), history by day; chapters wait for Podlove storage (item 2) |
| 17 | "Up Next" episode queue; third now-playing context (music/audiobook/podcast) | S | Built | Up Next rides behind the episode in the engine queue and survives restarts; auto-advance queues the show's next unplayed one; the music stash is shared with audiobooks |
| 18 | Downloads management UI (sizes, per-feed rules editor) | S | Built | Downloads page: folder and total, running downloads, files with sizes, per-show rules |
| 19 | Discovery: iTunes Search API proxy + charts + one-tap subscribe | S | Not started | D6 |
| 20 | Silence trimming / Smart-Speed DSP spike | M | Not started | Backlog; spike before committing |

Suggested build order: 2 (spike the parser on real feeds first) → 1 → 5 → 4 → 6 → 7 → 3 → 8 → 9 → 10 → 11 → 12 → 13 → 17 → 14 → 15 → 16 → 18 → 19 → 20.
