# Kahawai Internet Radio — TODO

Buildable feature list with estimated sizing and status.
Sizes: XS < S < M < L. Spec: `kahawai-radio-spec.md` (2026-10-02).

| # | Feature | Size | Status | Notes |
|---|---------|------|--------|-------|
| 1 | radio-browser.info mirror resolution + proxied search API + 24 h cache | S | Built (server; verify against the live API) | D1; `GET /api/radio/search`; verify endpoints vs their docs at build |
| 2 | Station URL resolve via `/json/url/{uuid}` (counts the click) | XS | Built (`POST /api/radio/favorites/{id}/play`) | D1 |
| 3 | `radio_favorites` table + CRUD/reorder API | S | Built | D2 |
| 4 | Manual station add + stream probe (codec/bitrate sniff) | S | Built | D2; `station_uuid` NULL = manual |
| 5 | Favorites sync to player + offline list cache | S | Built | Favorites come from the server on each visit; the list is not cached for offline use yet |
| 6 | Direct stream connect + ICY metadata parsing (`StreamTitle`) | M | Built | Shoutcast v1 `ICY 200 OK` handled by a hand-rolled http client; https via ureq; titles read as Latin-1 or UTF-8 |
| 7 | StreamTitle → now-playing + `radio_history` logging | S | Built | Title shown in the bar and in a "heard" list; logged to the server, repeats skipped |
| 8 | Codec verification: AAC/AAC+ through existing decode pipeline | S | Built, needs real captures | AAC/AAC+ (ADTS/LOAS) via the pure-Rust `syom` crate; Symphonia keeps MP3/Ogg/FLAC/Opus. Verify on real AAC+ stations (macOS) |
| 9 | Auto-reconnect (exp backoff) + alternate-URL fallback | S | Built (alternate URLs not yet) | Backoff 1/2/5/15/60 s with a visible "trying again" banner; a station that never played gives up after 3 tries. Falling back to the station's other address is not done |
| 10 | Player: directory browse/search UI (genre/country/language) | M | Built | Search, genre/country/language pickers, sort; opt-in message when the server's directory is off |
| 11 | Player: favorites view + reorder + one-tap play | S | Built (up/down buttons, no drag) |  |
| 12 | Player: radio now-playing (art, live title, session history, bitrate badge) | S | Partly built | Station name, live title, heard list, LIVE bar, reconnect banner. Not yet: station art in the bar, bitrate/codec badge there |
| 13 | Keyboard shortcuts (play/pause, mute, favorite) | XS | Not started | D4 |
| 14 | Recording: raw stream bytes to disk, user-chosen folder | S | Not started | D5; no re-encode |
| 15 | Recording: split-on-title into `Artist - Title.ext` | S | Not started | D5; best-effort, cap splits/hour |
| 16 | Timeshift ring buffer (15/30/60 min) + pause/rewind/back-to-live | M | Not started | D6; per station session |
| 17 | Sleep timer reuse for radio | XS | Not started | D7; shares audiobook component |
| 18 | Alarm: start favorite at set time, fade-in | S | Not started | D7; player must be running — state in UI |
| 19 | HLS (`.m3u8`) playback | M | Not started | Backlog |
| 20 | Scheduled/DVR recording (server-side) | M | Not started | Backlog |

Suggested build order: 8 (spike first — AAC+ coverage decides D3) → 1 → 2 → 6 → 7 → 9 → 3 → 4 → 5 → 10 → 11 → 12 → 13 → 14 → 15 → 16 → 17 → 18 → 19 → 20.
