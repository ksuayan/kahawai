# Kahawai Server as a VLC client endpoint — spec (refined)

Goal: any VLC (desktop 3.x) on the LAN can open a Kahawai playlist/album/queue
export and play it with seeking and per-track metadata. No new auth, no TLS,
no changes to the first-party player contract.

Verified against: fresh clone `616b3ab` (2026-10-01). Code refs below are to
that tree: routes `kahawai-server/src/main.rs:104-158`, streaming
`kahawai-server/src/api.rs:1618-1948`, formats
`kahawai-core/src/format.rs:79-107`, transcode planning
`kahawai-server/src/transcode.rs:1025-1043`, scan durations
`kahawai-server/src/scanner.rs:606,740`.

## What already works (no code needed)

- Passthrough serves a complete MIME table (`format.rs:94` — mp3, flac,
  m4a→audio/mp4, aac, wav, aiff, ogg, opus→audio/ogg), `Accept-Ranges: bytes`,
  `Content-Length`, single-range `206` + `Content-Range`, `416` on
  unsatisfiable. Non-streamable formats are refused passthrough
  (`is_directly_streamable`, `format.rs:79`).
- `HEAD /stream/{id}` exists for probing (`api.rs:1912`).
- No auth anywhere (LAN-only by design) — VLC needs no credentials.
- `/api/artwork/{hash}` exists (`main.rs:131`) — XSPF `<image>` is viable.
- `duration_ms` is populated at scan time (`scanner.rs:606,740`) —
  `#EXTINF` durations are viable.

## The gaps

1. **No playlist export.** `/api/playlists/import` parses M3U *in*; no export
   route exists (`main.rs`). A VLC user must hand-build URLs from JSON.
2. **No `Content-Disposition`** on `/stream` — VLC's playlist shows bare
   `/stream/123` instead of track titles.
3. **Live transcodes are chunked** — no `Content-Length`, no `Accept-Ranges`
   (`api.rs:1817`, "Deliberately... Axum will use chunked transfer
   encoding"). DSD→FLAC plays in VLC but cannot seek and shows no duration,
   contradicting this spec's own "with seeking" goal.
4. **SACD ISO has no decode path** — `resolve_plan` errors on it
   (`transcode.rs:1033`; surfaces as 415; extraction is future work). An
   export must never emit a URL the server will 415.

## Deliverables

### D1 — Playlist export endpoints (P0, S)

New routes (existing JSON API conventions, same LAN trust, no auth):

- `GET /api/playlists/{id}/export?format=m3u|xspf|pls` → `200` with
  `Content-Type: audio/x-mpegurl` / `application/xspf+xml` / `audio/x-scpls`,
  `Content-Disposition: attachment; filename="<name>.<ext>"` (RFC 5987 for
  non-ASCII names).
- `GET /api/albums/{id}/export?format=m3u|xspf|pls` — same, disc/track order.
- (Optional, same unit) `GET /api/queue/export?format=...` if a server-side
  queue view exists; skip if it doesn't — do not invent one.

Entry URL construction:

- **Absolute URLs** from the request's `Host` header:
  `http://{host}:8080/stream/{track_id}?format={rendition}`. A playlist saved
  to disk and reopened loses its HTTP base URL; relative entries break there.
- **One URL per track; never `?next=`** in exports. `?next=` chaining is the
  first-party player's contract; VLC advances entries itself, and on
  transcodes `?next=` merges audio into one response, destroying per-track
  metadata.

Rendition resolution per track (VLC-safe):

| Source | Emit | Why |
|---|---|---|
| mp3, flac, m4a, aac, wav, aiff, ogg, opus | `?format=passthrough` | VLC decodes natively; explicit value keeps entries self-describing (`StreamFormat::Passthrough` is a valid target) |
| dsf, dff | `?format=flac` | Server resolves dsf64→FLAC 24/88.2; never serve DSD bytes to VLC |
| sacd-iso | **omit the entry** | Server 415s it (transcode.rs:1033); extraction is future work |
| unknown | `?format=flac` | Never passthrough `application/octet-stream` |
| file missing on disk | **omit the entry** | Would 404 on play |

- Omitted entries are reported, not silent: `X-Export-Skipped: n` response
  header (count covers sacd-iso + missing files).
- Never emit `?format=dop` — DoP-in-WAV without a DoP-aware DAC is wrong
  for VLC.

M3U content: `#EXTM3U` header, `#EXTINF:{duration_s},{artist} - {title}`
per entry (duration from catalog `duration_ms`; `-1` if unknown, never 0;
fall back to filename stem when artist/title are missing). Prefer `.m3u`:
VLC sniffs `.m3u8` content for HLS tags and can misclassify; plain M3U has
no such ambiguity.

XSPF content: `<track>` per entry with `<location>`, `<title>`,
`<creator>`, `<album>`, `<duration>` (ms), and `<image>` pointing at
`http://{host}:8080/api/artwork/{hash}` when the track has art.

PLS content: `[playlist]`, `File{n}=`, `Title{n}=`, `Length{n}=` (seconds,
`-1` unknown), `NumberOfEntries`, `Version=2`.

Acceptance:

- `curl` the m3u export for a playlist with mixed formats incl. one DSF, one
  SACD ISO, one missing file: every entry is an absolute `http://` URL; the
  DSF entry carries `?format=flac`; no entry carries `?next=` or
  `?format=dop`; SACD ISO and missing file are absent;
  `X-Export-Skipped: 2`; `#EXTINF` durations match catalog `duration_ms`
  within 1 s.
- Paste the export URL into VLC → Media → Open Network Stream: full playlist
  loads, titles/artists display, advancing works, seeking works on every
  passthrough track.
- Export URL saved to disk as `.m3u` and reopened: still plays.

Size: **S** (< 1 day). Pure rendering over existing catalog queries; no new
tables, no new auth.

### D2 — `Content-Disposition` on `/stream` responses (P0, XS)

Set on `GET /stream/{id}` (passthrough and file-backed renditions):

- `Content-Disposition: inline; filename="<artist> - <title>.<ext>"`.
- Sanitize: strip `"`/`\`/`/` and control chars; fall back to
  `track-{id}.{ext}` when metadata is missing. RFC 5987-encode non-ASCII;
  never emit a raw unescaped header value.
- On transcode responses use the *rendition* extension (`.flac`/`.opus`/
  `.mp3`), never the source extension.

Acceptance: VLC playlist shows `Artist - Title` instead of `/stream/123`;
`curl -sI` shows a syntactically valid header for tracks with CJK titles.

Size: **XS** (hours). Touches `stream.rs` response headers only.

### D3 — Seekable transcodes via pre-rendered cache (P1, M) — NEW

The missed must-have behind gap 3. On
`GET /stream/{id}?format=flac|opus|mp3` **with no `?next=`**:

- Transcode once to a cache file under `<data>/transcode-cache/`, keyed by
  (track id, source mtime + length, target format + params), then serve via
  the existing `serve_file` path → real `Content-Length`, `Accept-Ranges`,
  `206` seeking.
- Atomic write (temp file + rename); single-flight per key so concurrent
  requests don't transcode twice. mtime+length in the key gives free
  invalidation on rescan/retag. Cap ~8 GiB with oldest-first eviction
  (tunable); cache survives restarts.
- Scope: single-track requests only. `?next=` chained gapless keeps the live
  chunked path — the first-party contract is untouched. DoP untouched
  (already has `Content-Length`; no ranges by design).
- Bonus: opus/mp3 transcodes become seekable too, not just DSD→FLAC.

Acceptance: a DSD album export plays in VLC with working seek and correct
durations on every track; replaying a track hits the cache (log line, no
re-transcode); eviction works under the cap.

Size: **M** (2–4 days). This completes the goal's "with seeking" promise;
sequenced after the P0s because playback + metadata land first.

### D4 — VLC client flow documentation (P1, XS)

Short doc (repo `docs/` or the user guide): "Playing your library in VLC":
paste `http://<server>:8080/api/playlists/{id}/export?format=m3u` into
Media → Open Network Stream; seeking works on passthrough and cached
transcodes (D3); DSD plays as 24/88.2 FLAC; no credentials needed on a
trusted LAN. Note the SMB alternative: VLC can also open `smb://` shares
directly against the NAS — zero server involvement, seeking works, auth via
URL or prompt (SMB2+ required).

Write this **after D3** so it documents final behavior.

Size: **XS**. No code.

## Parked (explicitly out of scope)

- **UPnP/DLNA MediaServer.** SSDP responder + `/description.xml` + SOAP
  `Browse` returning DIDL-Lite backed by the range-capable `/stream`. L-size
  for a browse UX widely reported as slow in VLC on large trees. Revisit
  when a TV or receiver — not VLC — is the target client.
- **ICY/radio mode.** Only relevant if Kahawai ever adds an auto-DJ mode:
  `Icy-MetaData: 1` handling on a single endless stream. Parked
  indefinitely.

## Non-goals (explicit)

- **Auth/TLS for the VLC path.** v1 is LAN-only with no auth by design; the
  VLC endpoint inherits that posture. If auth ever lands, exports must embed
  per-entry tokens (VLC re-resolves each playlist entry independently;
  custom `Authorization:` headers are not settable from the desktop GUI).
- **`?next=` chaining for VLC.** VLC's own playlist advance is the advance
  mechanism; do not entangle the first-party gapless contract.
- **HLS.** Wrong tool for an on-demand library.

## Test plan (real VLC 3.0.x, second LAN machine — never localhost)

1. m3u export incl. DSF + SACD ISO + one missing file: playlist loads with
   titles; SACD ISO absent; `X-Export-Skipped: 2`.
2. Seek to mid-track on MP3, FLAC, Opus, M4A: audio resumes promptly via
   the range path (no rebuffer-from-zero).
3. DSD album export (post-D3): plays as FLAC, seeks, durations correct.
4. Save-as `.m3u` → reopen from disk: still resolves (absolute-URL check).
5. 500-track playlist export: generation < 1 s, VLC loads without visible
   stall.
6. XSPF export: artwork displays.

## Sequencing

D1 → D2 → D3 → D4. (D1+D2: VLC plays the library with real titles. D3:
seeking holds for transcoded tracks too. D4: documents the finished
behavior.)
