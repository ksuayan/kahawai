# Spec: fast first-time SMB ingestion (server)

**Status: implemented** (Phases A and B). "Current state" below describes the code before this work; see "Status and measurements" at the end for what landed and how it measured.

## Goal

First scan of a 2.5 TB / ~125k-file SMB share reaches a browsable library (Phase A) in under an hour on 1 GbE. Full content hashing completes afterwards as a background job without blocking use.

## Current state (verified at `d6dfde9`)

- `run_scan_with_progress` (`crates/kahawai-server/src/scanner.rs:91`): walks the tree **twice** (pre-count for progress total, then the real walk), processes files **sequentially** (`spawn_blocking(analyze_file)` awaited inline), calls `std::fs::metadata` per file, and commits **one SQLite transaction per track**.
- `analyze_file` (`scanner.rs:332`): streams the **entire file** through BLAKE3 in 64 KiB chunks, then `lofty::read_from_path` opens the file **a second time** for tags.
- The (path, size, mtime) skip fast path already exists — it only helps rescan
  #2 and later.

## Bottlenecks, ranked

1. **2.5 TB of full-content hashing over SMB.** ~6 h minimum at 110 MB/s line rate, 8–12 h realistic with SMB per-read overhead. On a first scan the hash buys nothing: there is no baseline to compare against. It only pays off when a file's size/mtime later changes.
2. **Double tree walk** over high-latency SMB (enumeration is latency-bound).
3. **Sequential pipeline + per-track transactions.**

## Design

### Phase A — metadata scan (foreground `scan` job)

- **Single walk.** Drop the pre-count: progress events carry `files_seen`, `audio_files`, `files_per_min`; no total (or estimate from the previous scan's count).
- **Bounded worker pool:** semaphore (default 16) around `spawn_blocking(analyze_meta)`; results flow over `mpsc` to a single DB writer task committing **500 tracks per transaction**. Stays on the existing Tokio runtime — no new executor.
- **`analyze_meta` (no hashing):** reuse `walkdir::DirEntry::metadata()` instead of a second `fs::metadata`; read tags from the already-open `File` via `lofty::read_from` (check the pinned lofty version in `Cargo.toml`; fall back to `read_from_path` if the API differs). Store `hash = NULL`.
- **Schema:** `tracks.hash` becomes nullable (`NULL` = "hash pending"), plus `tracks.hash_algo TEXT` (e.g. `'blake3-v1'`) so a future change of identity algorithm doesn't require retrofitting already-hashed rows.

### Phase B — content hashing (background persistent job `hash_files`)

- Processes `WHERE hash IS NULL` in batches (~200), **1 MiB read buffer**, low concurrency (4 workers — hashing is throughput-bound, concurrency does not help).
- Resumable by construction: the `NULL` marker is the checkpoint. Kill -9 safe. Each completed row stamps `hash` + `hash_algo` together.
- Until a file is hashed, change detection is size+mtime only (documented limitation, matches pre-existing semantics for never-hashed rows).

### SMB / environment notes

- Enumeration is latency-bound → concurrency wins. Hashing is throughput-bound → only deferral or a faster link wins.
- `db_path` must stay on local disk (WAL mode). Never on the share.

### Cross-spec touchpoint

- The batched writer bumps `catalog_rev` once per committed batch (see player-catalog-cache spec).

## Tasks

1. Migration: `tracks.hash` nullable + `tracks.hash_algo`.
2. Split `analyze_file` into `analyze_meta` (stat + tags, no hash) and `hash_file(path) -> String`.
3. Rewrite `run_scan_with_progress`: single walk, semaphore-bounded workers, `mpsc` → batched writer; progress without total; keep SSE `/api/events` working. Count walk errors instead of swallowing them (`filter_map(|e|` `e.ok())` at scanner.rs:103,164) — warn per error, include the count in the scan report.
4. New persistent job `hash_files` in `jobs.rs` with resume; auto-queued on scan completion; manually triggerable via `/api/jobs`.
5. Instrument the pipeline (tracing counters): files/sec, avg metadata latency, DB commits/sec — so the next bottleneck after hashing is measured, not guessed.
6. Update the scan section of `docs/v1/kahawai-server-spec.md`.

## Acceptance

- First-scan Phase A of the 125k-file SMB share: wall time < 1 h on 1 GbE; library browsable immediately after.
- Kill -9 mid-Phase-B → restart resumes with zero rehash of completed files.
- Existing gates green: server tests, clippy `-D warnings`, `cargo fmt`.

## Status and measurements

Phases A and B are implemented on the `tune-up` branch: migrations 005 and 006 and `scanner.rs` (A), `hashing.rs` and the `hash_files` job kind (B). Not done: the `catalog_rev` bump (the player-catalog-cache spec that defines it doesn't exist in code yet).

First scan into an empty catalog: 105,248 files (81,316 audio), local `_Dev-Media` plus the `NetMusic` SMB share on 1 GbE, release build. Measured with the ignored `first_scan_benchmark` test in `scanner.rs`.

| Run | Workers | Wall time | Audio files/s | Avg analyze | Commits/s |
| --- | --- | --- | --- | --- | --- |
| Phase A | 16 | 31.7 min | 43 | 316 ms | 27.0 (falling) |
| + `albums(title)` index (006) | 16 | 30.9 min | 44 | 303 ms | 40.7 (flat) |
| same, NAS under other load | 16 | 45.3 min | 30 | 475 ms | 27.4 (keeping up) |
| + index, 64 workers (stopped at 12k files) | 64 | — | 40 | 1,388 ms | 36.8 |

- Per-file tag reads over SMB (~300 ms each) set the pace, and vary a lot with whatever else the NAS is doing: compare runs made close together.
- More workers don't help. At 64, per-file time rose about fourfold and files/s stayed at ~40: the share (or the macOS SMB client) is saturated at ~40–45 files/s, so it is throughput-bound, not latency-bound. 16 stays the default. Further gains would have to come from reading less per file.
- The catalog writer was the second bottleneck: the album match for files without an album-artist tag scanned every track. Migration 006 fixed it.
- Around 120 files with accented names, and one folder, were intermittently unreadable through the macOS SMB mount: `readdir` lists them, `stat` fails under every Unicode normalization. It depends on the SMB session (a later run read them all), not the scanner. The scan now logs and counts these (`walk_errors`, "stat failed" warnings) instead of skipping them silently.

Phase B, hashing the whole library from that first-scan catalog: 81,214 pending tracks, 1,976 GB, same share and link, release build, measured with the ignored `hash_benchmark` test in `hashing.rs`.

| Run | Files hashed | Data | Wall time | Throughput |
| --- | --- | --- | --- | --- |
| 1, killed with `kill -9` at 591 s | 3,406 | 65 GB | 9.9 min | 107 MB/s |
| 2, restarted to completion | 77,773 (35 left pending) | 1,910 GB | 5.2 h | 102 MB/s |

- A full content hash takes about **5.3 h** on 1 GbE. 4 workers with 1 MiB reads keep the link saturated (~105 MB/s), so hashing is bound by the network: a faster link, not more workers, would shorten it.
- **Kill -9 resume, verified.** Run 2 started with exactly the 77,808 rows still pending, and the 3,406 hashes written before the kill were byte-identical afterwards: zero rehash. The database passed `PRAGMA integrity_check` after the kill.
- All 81,179 hashes are 64-character BLAKE3 hex labelled `blake3-v1`. The 35 files left pending are the same accented-name SMB problem as above ("No such file or directory"); a later run retries them.

## Non-goals

- No change to rescan fast-path semantics. No NAS-side agent. No ML.
