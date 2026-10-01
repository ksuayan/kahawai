# Kahawai Player — Client Design (v1)

*2026-09-25. Companion to `kahawai-server-spec.md` (C1–C3). v1: macOS desktop. iOS and Android planned for v2 — the architecture below keeps that path open rather than requiring a rewrite.*

## 0. Platform strategy

**Tauri 2** for v1 macOS. The decisive reason beyond "same stack as Koa": Tauri 2 ships **iOS and Android targets** from the same codebase. The v2 mobile apps are then a platform shell + audio-output swap, not a second app.

To make that real, the Rust side is split from day one:

```
  kahawai-player-core/          # pure Rust, no Tauri, no platform APIs
    api/                # server REST client (albums, search, stream URLs, jobs)
    queue/              # play queue, shuffle/repeat, persistence
    audio/              # decode orchestration, DSP chain, output abstraction
    dsp/                # parametric EQ, loudness normalization
  src-tauri/            # thin Tauri shell: commands delegate to kahawai-player-core
  src/ (Vue 3 + Pinia)  # UI; identical for desktop and (later) mobile
```

Rule: nothing in `kahawai-player-core` imports Tauri or platform audio APIs directly — audio output goes through a small `AudioSink` trait (`cpal` on desktop now, Oboe/CoreAudio-mobile later). Anything that violates this gets flagged in review; it's the whole mobile bet.

## 1. Audio engine

- **Output:** `cpal`, shared mode, device sample rate followed (no forced resample when the DAC matches the track — bit-perfect path stays bit-perfect). macOS CoreAudio via cpal is fine for v1.
- **Pipeline per track:** HTTP stream (Range-capable, resume on dropout) → decode (symphonia for PCM formats/Vorbis; libopus adapter for Opus; server already transcoded DSD→FLAC for Story A) → DSP chain (EQ → loudness) → resample only if sink demands it (rubato) → ring buffer → `AudioSink`.
- **Gapless:** while track N plays, the engine pre-fetches and pre-decodes the head of N+1 (using the server's `?next=` hint). Handoff is sample-accurate at the ring-buffer level — no silence insertion, no crossfade unless the user enables it (optional crossfade is a setting, default off).
- **Decode threads:** decode runs off the audio callback thread; the callback only drains the ring buffer (underrun → log + soft mute, never panic).
- **v2 hooks (designed for, not built):** exclusive/hog-mode output on macOS (needs CoreAudio-specific code behind the `AudioSink` trait), and the **native DSD / DoP path** (S5b): DoP-encapsulated stream routed bit-perfect to a DSD-capable DAC, bypassing the DSP chain entirely (DSP on DSD is a separate research topic — bypass, don't fake it).

## 2. State management (Pinia)

| Store | Owns |
|---|---|
| `library` | Albums/artists/tracks pages, search results + query, artwork cache index |
| `player` | Transport (play/pause/seek), current track, position, duration, audio path badge (e.g. "FLAC 24/96 → EQ → DAC 96k") |
| `queue` | Ordered queue, shuffle/repeat modes, history; persisted to disk, restored on launch; save-queue-as-playlist |
| `playlists` | CRUD against the server API, optimistic UI |
| `jobs` | Long-running server jobs (ISO extraction…): toast state, progress, completion |
| `settings` | Audio device, EQ preset, loudness target, crossfade, artwork cache size, preferred format ladder, DSD story selector |

The webview never holds the server token and never streams audio itself — playback state lives in Rust (`kahawai-player-core`); Pinia mirrors it via Tauri events. One source of truth, no drift.

## 3. UI/UX (macOS v1)

- **Views:** Albums (grid, artwork-first), Artists, Playlists, Search, Queue, Settings. Album detail: track list with per-track format badge.
- **Now Playing:** full view with large artwork, format/sample-rate badge, EQ quick access; mini-player mode for the menu bar footprint. Seek bar with buffered-range display; scrubbing maps to Range on passthrough, `?seek_ms=` on transcodes.
- **Queue view:** drag-to-reorder (same interaction vocabulary as Koa collections), remove, "play next" / "add to queue" context actions, "save queue as playlist".
- **Keyboard:** space play/pause, ←/→ seek, ↑/↓ volume, `N`/`P` next/previous, `F` search, `1–6` switch views — efficient keyboard commands throughout, per the standing Koa philosophy. Full map defined with the command-palette pass.
- **Toasts:** job start/progress/complete (ISO extraction), stream errors with retry, device-change notices ("Output switched to …").

## 4. Library & artwork caching

- Server browse results paged (100/page); Pinia caches pages in-memory per session.
- Artwork: fetched via `/api/artwork/{hash}`, cached on disk (content-addressed by hash — same dedup logic as Koa), LRU-capped (default 2 GB, user-configurable to 512 MB/1 GB/2 GB in Settings → Album art cache, alongside a free-disk-space readout), ETag-aware.
- Offline is **not** v1 — but the disk cache layout is designed so v2 mobile can promote cached tracks to offline downloads without re-fetching.

## 5. Built-in DSP (v1)

- **Parametric EQ:** up to 12 bands (peaking, low/high shelf, low/high pass), presets (Flat, plus user-saved), per-output-device preset memory.
- **Loudness normalization:** EBU R128-style gain, target configurable (default −14 LUFS), applied as metadata-driven gain — never rewrites files.
- Both sit in the `dsp` module behind a chain API so a future plugin-host can insert VST3/AU nodes at the same point (v2 spike, per server spec §5.1).

## 6. Designing for mobile (v2, not building yet)

Decisions taken now so v2 is a shell, not a rewrite:

- `kahawai-player-core` has zero desktop assumptions (no menu bar, no window APIs).
- Audio goes through `AudioSink`; iOS/Android shells provide their platform sinks (background audio: iOS `audio` background mode, Android foreground service + MediaSession — both are shell-level config).
- The Vue UI is responsive from day one (the phone layout is a CSS problem, not an architecture problem, if views are componentized).
- Offline downloads (v2 feature): server already supports `?format=opus` — the mobile shell downloads Opus for space efficiency; `kahawai-player-core` gains a local-file source alongside the HTTP source.

## 7. Task checklist

| Done | ID | Task |
|------|----|------|
| ☐ | PL1 | `kahawai-player-core` crate: api client (browse, search, stream URLs, jobs) |
| ☐ | PL2 | Queue model: order/shuffle/repeat/history + disk persistence |
| ☐ | PL3 | Audio engine: cpal sink, ring buffer, decode-off-callback, gapless handoff |
| ☐ | PL4 | Pinia stores (library / player / queue / playlists / jobs / settings) |
| ☐ | PL5 | Views: Albums, Artists, Playlists, Search, Queue, Now Playing, Settings |
| ☐ | PL6 | Artwork disk cache (content-addressed, LRU, ETag-aware) |
| ☐ | PL7 | DSP: parametric EQ + loudness normalization in the chain |
| ☐ | PL8 | Keyboard map + command palette entries |
| ☐ | PL9 | Toasts: jobs, stream errors, device changes |
| ☐ | PL10 | `AudioSink` trait boundary audit (no Tauri/platform imports in core) |
| ☐ | PL11 | macOS packaging: bundle, signing, notarization |

**v2 (mobile):** iOS/Android Tauri shells, platform `AudioSink`s, background audio + lock-screen controls, offline downloads, native DSD/DoP path, exclusive-mode output, VST3/AU hosting spike.

## 8. Open decisions (Kyo's call)

1. Exclusive (hog) mode on macOS in v1, or shared mode first and hog as a v1.x toggle? (Shared is simpler; hog is the audiophile expectation.)
2. Crossfade: offered as a setting in v1, or gapless-only?
3. Should the desktop client also serve as a **remote control** for another instance (phone as remote for the Mac)? Cheap if the queue state is already server-visible — but it's scope creep unless wanted.
