# Roadmap — future enhancements

*Possibilities, not commitments. Grouped by area, roughly ordered by how much they unlock versus how much they cost. Anything here can be adopted, reshaped, or dropped.*

## How to read this

- **Unlocks** — what the work makes possible beyond itself.
- **Depends on** — what must land first.
- **Risk** — the honest reason it might not happen.

---

## Server

### Auth + TLS (v2 flagship)

Token-based auth (per-client long-lived tokens, generated server-side) and TLS termination (self-signed by default, user-supplied certs supported). Unlocks everything below marked "internet". **Depends on:** nothing architectural — the API is already stateless. **Risk:** v1's trusted-LAN posture cannot be incrementally patched; this needs a full security review (rate limiting, token scopes, header hygiene), not a flag flip.

### Internet-safe remote access

Reverse-proxy documentation (Caddy/nginx recipes), optional Tailscale/WireGuard notes. **Depends on:** auth + TLS. **Risk:** support burden for network topologies; keep it docs-first.

### Server-computed loudness metadata

R128 integrated loudness measured at scan time (or as a background job), stored on the track record, served to clients. Deletes the client's double-bandwidth pre-scan and enables instant normalization. **Depends on:** a schema migration + a measurement worker (can reuse the transcode pipeline's DSD decimator for DSD sources). **Risk:** scan-time cost on huge libraries — make it a backfill job, not a scan blocker.

### Stronger catalog identity

MusicBrainz/Discogs external IDs alongside the `(title, album-artist)` heuristic; prefer the ID when present. **Depends on:** an ID acquisition path (scan-time lookup is network-dependent — likely a manual/assisted flow). **Risk:** wrong IDs are worse than no IDs; needs a user-verification UX.

### Smart playlists / saved searches

Rule-based playlists (genre, year, rating, "added in the last month") evaluated server-side, exposed through the existing playlist API shape. **Depends on:** nothing. **Risk:** rule-language scope creep — keep it to the 13-ish predicates users actually need.

### Multi-user (later v2)

Per-user queues, play counts, ratings, and permissions on top of auth. **Depends on:** auth + TLS. **Risk:** fundamentally changes the single-household simplicity; only if real demand appears.

### Container image + service files

Docker image, systemd unit, launchd plist. **Depends on:** nothing. **Risk:** none, just maintenance.

---

## Audio engine & formats

### PCM exclusive-mode toggle

WASAPI exclusive / CoreAudio hog-mode for PCM (not just DoP), user-toggled per device. **Depends on:** Mac validation of the existing hog-mode code (it's the same mechanism). **Risk:** exclusive mode steals the device — needs careful acquire/release UX.

### Output-device selection

Actually route PCM to a chosen device instead of the system default; rebuild the sink on selection. The v1 device list is already informational. **Depends on:** nothing. **Risk:** device hot-plug handling.

### Automatic DSD story per device

Probe the output device once, remember "this DAC does DoP at 176.4 kHz", and pick `native` vs `convert` automatically instead of a global setting. **Depends on:** Mac validation of `dop_status`. **Risk:** flaky USB DACs lying about capabilities — needs a manual override regardless.

### Higher-quality DSD→PCM options

Selectable decimation quality (e.g. wider FIR, higher intermediate rate) for users who convert but want better than the fast default. **Depends on:** nothing — the decimator is parameterized. **Risk:** benchmark to prove audibility; don't ship knobs without evidence.

### VST3/AU plugin hosting (research spike)

Host user plugins in the DSP chain. The chain API has a node-insertion shape reserved for this. **Depends on:** a hosting crate maturing (none is production-ready in Rust today) or writing one. **Risk:** high — this is a spike, not a plan; sandboxing and crash isolation are real problems.

### Crossfade (optional)

User-toggled, off by default — the v1 stance is gapless-only. **Depends on:** nothing architectural. **Risk:** interacts with the `?next=` gapless chain; keep it client-side and simple.

---

## Desktop client

### iOS / Android shells (v2)

Tauri 2 mobile targets reusing `kahawai-player-core` untouched, with platform audio sinks (background playback, lock-screen/now-playing controls). **Depends on:** `kahawai-player-core` purity rule holding (it does). **Risk:** mobile background-audio and store policies, not engine work.

### Offline downloads

Download `?format=opus` renditions (or originals) for offline queues; server already serves the right rendition. **Depends on:** mobile shells (desktop offline is less valuable). **Risk:** storage management UX.

### Artwork disk cache

Content-addressed LRU cache keyed by the artwork hash the server already provides — the invalidation story is free because the key *is* the content. **Depends on:** nothing. **Risk:** none; straightforward.

### Honest buffer metric

Expose the engine's ring fill level + in-flight HTTP state as a real buffered-range readout (v1 honestly shows none). **Depends on:** plumbing two numbers through the state event. **Risk:** none.

### Remote control

One client drives playback on another (phone as remote for the desktop). **Depends on:** auth + TLS (never on the trusted-LAN v1). **Risk:** discovery/pairing UX.

### Library management views

Recently added, most played (needs play-count tracking — server), ratings, tag editing (server writes tags? conflicts with the read-only scanner philosophy — would need a deliberate design exception). **Depends on:** decisions, mostly. **Risk:** scope creep into "music manager" territory; the philosophy doc is the guardrail.

---

## Explicitly not on the roadmap

- Streaming-service integration (Spotify/Apple Music/Tidal) — contradicts "your library, your machine".
- Social features, scrobbling-as-a-service, analytics, telemetry.
- Cloud sync of any kind.
- DRM or protected-content playback — v1 decision, stands.
- SACD ISO extraction/decoding, in any form — see `SACD-Extraction.md`.

---

## Suggested sequencing if v2 happens

1. Mac validation + signing (close out v1 properly).
2. Server-computed loudness (deletes a real daily cost) + artwork disk cache (small, satisfying).
3. Auth + TLS with a real security review.
4. PCM exclusive toggle, device selection.
5. Mobile shells + offline.
6. Everything else as demand dictates.
