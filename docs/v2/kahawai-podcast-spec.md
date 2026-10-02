# Kahawai Podcasts — Spec

Date: 2026-10-02. Status: spec only, not implemented.
Scope: subscribing to and listening to podcasts. Not publishing/hosting.
Style: compact, agent-ready. Sizes: XS < S < M < L.

## Goal

First-class podcasts in Kahawai Player: subscribe by URL or OPML, automatic
episode fetching and downloads, per-episode resume, and playback that reuses
the audiobook machinery (pitch-corrected speed, skips, sleep timer) instead
of rebuilding it.

## Non-goals (v1)

- Publishing/hosting a podcast.
- Video podcasts (audio enclosures only; note video feeds, skip them).
- Podcasting 2.0 value blocks / Lightning payments. Never.
- Transcripts (namespace parsed and stored if present; no UI in v1).

## Core decisions

- **~60% of podcast playback already exists.** Positions, pitch-corrected
  speed, skip intervals, sleep timer, per-show settings, and the separate
  queue context are built for audiobooks (docs/v1/kahawai-audiobook-spec.md).
  Podcasts reuse them; the new work is feeds, episodes, and downloads.
- **Downloads live server-side.** The server fetches enclosures into its
  media folder via the existing persistent-jobs infrastructure; the player
  streams them like any other track. One download serves all clients, and
  the server stays the sync point. No client-side download store in v1.
- **Three now-playing contexts**: `music`, `audiobook`, `podcast`. Starting
  one pauses and preserves the others exactly (extends the audiobook D4 rule).
- Discovery = **iTunes Search API** (`itunes.apple.com/search?media=podcast`):
  free, no key, the standard choice for open clients. Verify params at build.
- Member feeds: credentials may be embedded in the feed URL
  (`https://user:pass@…`), the standard pattern. No separate credential
  store in v1 — state this plainly in UI.

## D1 — Feed subscriptions (P0, M)

```sql
podcast_feeds(id INTEGER PK, feed_url TEXT UNIQUE, title, author,
  description, artwork_hash, last_fetched, last_error,
  auto_download INTEGER DEFAULT 1, keep_n INTEGER DEFAULT 5,
  delete_played_after_days INTEGER DEFAULT 7, sort_order, added_at);
```

- `GET/POST/DELETE /api/podcasts/feeds`; subscribe by pasting a feed URL.
  Probe on subscribe: fetch, parse, report title/episode count or the error.
- RSS 2.0 + Atom; read the iTunes namespace (`itunes:duration`,
  `itunes:image`, `itunes:explicit`) and Podlove Simple Chapters where
  present (store chapters; UI later).
- OPML import (`POST /api/podcasts/feeds/import-opml`, file upload) and
  export (`GET /api/podcasts/feeds/export-opml`). Subscriptions must never
  be locked in.
- Duplicate guard: same normalized feed URL subscribes once.

## D2 — Episode catalog + refresh scheduler (P0, M)

```sql
podcast_episodes(id INTEGER PK, feed_id, guid TEXT UNIQUE, title,
  description_html TEXT, pub_date, duration_ms, enclosure_url,
  enclosure_type, file_path NULL, downloaded_at NULL, played_at NULL,
  created_at);
podcast_positions(episode_id INTEGER PK, offset_ms, updated_at,
  user_id INTEGER DEFAULT 0);
```

- Background refresh job (persistent jobs infra): default every 6 h,
  staggered per feed; `POST /api/podcasts/refresh` for manual
  (per-feed or all). Record `last_error` per feed; surface failing feeds
  in UI, never silently drop them.
- Episodes keyed by GUID (fallback: enclosure URL). New episodes appear
  newest-first. Keep episodes the feed drops (some feeds truncate to N);
  mark feed-truncated ones so re-fetch can't resurrect-then-duplicate.
- `GET /api/podcasts/feeds/{id}/episodes?unplayed=1&limit=`; played state
  derived: `played_at` set explicitly or auto at ≥97% like audiobooks.

## D3 — Download manager, server-side (P0, M)

- `POST /api/podcasts/episodes/{id}/download` (queue job),
  `DELETE …/download` (cancel + delete file). Files under a configured
  podcast folder: `Show/2026-10-02 - Episode title.mp3`.
- Auto-download rules per feed: keep N latest **unplayed** (`keep_n`,
  default 5); when a new episode downloads beyond N, delete the oldest
  downloaded-and-played file first, then oldest downloaded.
- `delete_played_after_days` (default 7, 0 = never): janitor job removes
  played files, keeps the episode row + position.
- Resume partial downloads (Range) — feeds are big files on flaky hosts.
- Player plays downloads via the normal track/stream path; undownloaded
  episodes stream from the enclosure URL directly (no forced download).

## D4 — Positions, history, played state (P0, S)

- Same pattern as audiobooks: `PUT /api/podcasts/episodes/{id}/position`,
  throttled every 10 s + pause/stop/seek/close; per-day listening sessions
  derived the same way (reuse the derivation, new table
  `podcast_sessions` or a `kind` column — implementer's choice, spec the
  behavior not the table).
- `GET /api/podcasts/episodes/{id}/history` — per-day stop markers.
- Speed/skip settings per **feed** (`podcast_settings(feed_id PK, speed,
  skip_back_s, skip_forward_s)`), auto-applied like per-book settings.

## D5 — Player podcast UI (P0, M)

- Subscriptions view: artwork grid/list, unread counts, failing-feed badges.
- Feed view: episodes newest-first, unplayed filter, download state per
  episode (downloaded / queued / stream-only), per-episode download/delete.
- Episode view: rendered show notes (sanitized HTML, links open externally,
  timestamps plain text in v1), chapters list if parsed, mark played/unplayed.
- "Up Next" queue: explicit episode queue, separate from music/audiobook
  contexts; auto-advance option per feed ("play next unplayed").
- Downloads management: what's stored, sizes, per-feed rules editor.

## D6 — Discovery via iTunes Search API (P1, S)

- `GET /api/podcasts/directory/search?term=` → iTunes Search
  (`media=podcast`), server-side, small cache. Results show artwork, title,
  author, episode count; one-tap subscribe.
- Charts: `GET /api/podcasts/directory/charts?country=` (top podcasts;
  same API family). Nice-to-have within the S.

## D7 — Per-podcast settings (P1, S)

- Per feed: playback speed, skip back/forward seconds, auto-download
  on/off + keep-N, delete-played-after days, auto-advance next episode,
  refresh override. One settings dialog; server persists; player applies.

## Backlog (parked, not spec'd)

- Silence trimming / Smart-Speed-style (DSP; the hard one — spike first
  if ever attempted).
- Transcripts UI (Podcasting 2.0 `podcast:transcript`).
- Podlove chapter UI + embedded MP3 chapters.
- Notifications for new episodes (no notification infra in v1).
- Cross-device push of position beyond the LAN server.

## Phase gates

Zero warnings, green tests, clippy clean per phase, as usual.
v1 = D1–D5. D6–D7 follow in order.
