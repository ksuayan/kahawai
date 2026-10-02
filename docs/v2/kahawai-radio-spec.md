# Kahawai Internet Radio — Spec

Date: 2026-10-02. Status: spec only, not implemented.
Scope: **listening** to publicly accessible streams. Not serving/broadcasting.
Style: compact, agent-ready. Sizes: XS < S < M < L.

## Goal

First-class internet radio in Kahawai Player: browse a real station
directory, one-tap play with live track titles, favorites that follow you
across clients, and the two differentiators that make a radio player
formidable — **recording** and **timeshift** (pause/rewind live radio).

## Non-goals (v1)

- Serving/broadcasting. No Icecast/Shoutcast server; the VLC spec's
  ICY/radio item stays parked separately.
- HLS (`.m3u8`) streams. Note which stations need it; park for v2.
- DRM or authenticated/subscription streams.
- Podcasts (separate future work).

## Core decisions

- The **player connects directly** to stream URLs. The server stores
  favorites/history and proxies the directory. No server-side relay in v1
  (no relay bandwidth, no extra failure mode, simpler auth story: none).
- Directory = **radio-browser.info**: the community station database, the
  MusicBrainz-of-radio analog. Free API, no key, app name in `User-Agent`.
  Server resolves mirrors per their docs and caches 24 h. Verify endpoint
  details against their docs at build time.
- **ICY metadata** (Shoutcast/Icecast `StreamTitle`) is parsed for
  now-playing display and logged to history. This is the feature that
  separates a radio player from a dumb stream URL box.

## D1 — Station directory via radio-browser.info (P0, S)

- Server-side mirror resolution with fallback to the next mirror on failure.
- `GET /api/radio/search?q=&tag=&country=&language=&order=&limit=` —
  proxies radio-browser.info, 24 h cache keyed by normalized query.
- Station URL resolution goes through their `/json/url/{uuid}` endpoint,
  which counts the click (good citizenship with the community DB).
- Never expose raw directory credentials/keys: there are none; keep the
  `User-Agent` app identifier in server config.

## D2 — Favorites + custom stations, server-side (P0, S)

```sql
radio_favorites(id INTEGER PK, station_uuid TEXT NULL, name TEXT,
  url TEXT, url_resolved TEXT, favicon TEXT, tags TEXT, country TEXT,
  language TEXT, bitrate INTEGER, codec TEXT, manual INTEGER DEFAULT 0,
  sort_order INTEGER, added_at);
radio_history(id INTEGER PK, station_name TEXT, stream_title TEXT,
  played_at);
```

- `GET/POST/DELETE /api/radio/favorites`; `PATCH /api/radio/favorites/{id}`
  for rename/reorder.
- Manual add: paste a stream URL; probe it (ICY handshake + content-type
  sniff) to fill codec/bitrate where volunteered; user names it.
  `station_uuid` NULL marks manual entries.
- Favorites sync to the player on connect; list cached for offline viewing
  (playback still needs network, obviously).

## D3 — Stream playback engine (P0, M)

- Direct HTTP(S) GET with `Icy-MetaData: 1`. If the response carries
  `icy-metaint`, parse metadata blocks; extract
  `StreamTitle='Artist - Title'`; update now-playing; log changes to
  `radio_history`. Tolerate stations that never send metadata.
- Codecs through the existing decode pipeline: MP3, AAC/AAC+, OGG Vorbis,
  Opus, FLAC. **Verify AAC/AAC+ coverage at build time** — most stations
  are AAC+ or MP3; if the pipeline lacks HE-AAC, that is a build blocker
  for D3, not a spec change.
- Auto-reconnect: exponential backoff 1 s → 2 s → 5 s → 15 s → 60 s cap;
  try the station's alternate URLs/mounts before surfacing failure;
  visible "reconnecting…" state, never a silent stall.
- A stream is a live edge: no duration, no seeking (without D6). Model on
  the existing live-transcode no-seek path.

## D4 — Player radio UI (P0, M)

- Browse: genre / country / language pickers + full-text search over the
  directory. Station rows: name, country, tags, bitrate + codec badge,
  popularity (click count).
- Favorites view with drag reorder; one-tap play; favorite toggle on any
  station row.
- Now-playing: station art (favicon w/ fallback icon), live StreamTitle,
  "heard this session" title list, bitrate/codec badge, reconnect state.
- Keyboard: play/pause, mute, favorite toggle. (Full shortcut map with the
  existing keyboard settings.)

## D5 — Recording (P1, S)

- Client-side: write **raw stream bytes** to disk, no re-encode, into a
  user-chosen folder: `Station/2026-10-02 14-30.mp3` (extension from codec).
- Optional split-on-title: new file when `StreamTitle` changes, named
  `Artist - Title.ext`. Best-effort — some stations spam metadata; cap
  splits per hour and note it in UI.
- Recording indicator, stop button; recordings listed in a simple local
  list with reveal-in-folder. No server involvement in v1.

## D6 — Timeshift: pause/rewind live radio (P1, M)

- Ring buffer of stream bytes: 15/30/60 min configurable, default 30.
- Pause = keep buffering, stop consuming. Resume plays from the pause
  point (now behind live). Rewind within the buffer. "Back to live" jumps
  to the edge.
- Buffer is per station session; changing stations clears it. Buffer
  survives reconnects; the gap is marked, not hidden.

## D7 — Sleep timer + alarm (P1, S)

- Sleep: reuse the audiobook sleep-timer component (durations + 10 s fade).
- Alarm: start a chosen favorite at a set time with fade-in. State the
  caveat in UI: the player must be running — no background daemon in v1.

## Backlog (parked, not spec'd)

- HLS (`.m3u8`) segment playback.
- Scheduled / DVR recording (needs server-side capture + jobs).
- Cross-station loudness normalization (the stages exist; easy win later).
- Stream health dashboard (probe favorites periodically, badge the dead).
- Station sharing (export/import favorite lists).

## Phase gates

Zero warnings, green tests, clippy clean per phase, as usual.
v1 = D1–D4. D5–D7 follow in order.
