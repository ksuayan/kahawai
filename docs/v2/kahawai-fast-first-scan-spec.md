# Spec: fast first-time SMB ingestion (server)

## Goal

First scan of a 2.5 TB / ~125k-file SMB share reaches a browsable library
(Phase A) in under an hour on 1 GbE. Full content hashing completes afterwards
as a background job without blocking use.

## Current state (verified at `d6dfde9`)

- `run_scan_with_progress` (`crates/kahawai-server/src/scanner.rs:91`): walks the
  tree **twice** (pre-count for progress total, then the real walk), processes
  files **sequentially** (`spawn_blocking(analyze_file)` awaited inline), calls
  `std::fs::metadata` per file, and commits **one SQLite transaction per track**.
- `analyze_file` (`scanner.rs:332`): streams the **entire file** through BLAKE3
  in 64 KiB chunks, then `lofty::read_from_path` opens the file **a second time**
  for tags.
- The (path, size, mtime) skip fast path already exists — it only helps rescan
  #2 and later.

## Bottlenecks, ranked

1. **2.5 TB of full-content hashing over SMB.** ~6 h minimum at 110 MB/s line
   rate, 8–12 h realistic with SMB per-read overhead. On a first scan the hash
   buys nothing: there is no baseline to compare against. It only pays off when
   a file's size/mtime later changes.
2. **Double tree walk** over high-latency SMB (enumeration is latency-bound).
3. **Sequential pipeline + per-track transactions.**

## Design

### Phase A — metadata scan (foreground `scan` job)

- **Single walk.** Drop the pre-count: progress events carry `files_seen`,
  `audio_files`, `files_per_min`; no total (or estimate from the previous
  scan's count).
- **Bounded worker pool:** semaphore (default 16) around
  `spawn_blocking(analyze_meta)`; results flow over `mpsc` to a single DB
  writer task committing **500 tracks per transaction**. Stays on the existing
  Tokio runtime — no new executor.
- **`analyze_meta` (no hashing):** reuse `walkdir::DirEntry::metadata()` instead
  of a second `fs::metadata`; read tags from the already-open `File` via
  `lofty::read_from` (check the pinned lofty version in `Cargo.toml`; fall back
  to `read_from_path` if the API differs). Store `hash = NULL`.
- **Schema:** `tracks.hash` becomes nullable (`NULL` = "hash pending"), plus
  `tracks.hash_algo TEXT` (e.g. `'blake3-v1'`) so a future change of identity
  algorithm doesn't require retrofitting already-hashed rows.

### Phase B — content hashing (background persistent job `hash_files`)

- Processes `WHERE hash IS NULL` in batches (~200), **1 MiB read buffer**,
  low concurrency (4 workers — hashing is throughput-bound, concurrency does
  not help).
- Resumable by construction: the `NULL` marker is the checkpoint. Kill -9 safe.
  Each completed row stamps `hash` + `hash_algo` together.
- Until a file is hashed, change detection is size+mtime only (documented
  limitation, matches pre-existing semantics for never-hashed rows).

### SMB / environment notes

- Enumeration is latency-bound → concurrency wins. Hashing is throughput-bound
  → only deferral or a faster link wins.
- `db_path` must stay on local disk (WAL mode). Never on the share.

### Cross-spec touchpoint

- The batched writer bumps `catalog_rev` once per committed batch (see
  player-catalog-cache spec).

## Tasks

1. Migration: `tracks.hash` nullable + `tracks.hash_algo`.
2. Split `analyze_file` into `analyze_meta` (stat + tags, no hash) and
   `hash_file(path) -> String`.
3. Rewrite `run_scan_with_progress`: single walk, semaphore-bounded workers,
   `mpsc` → batched writer; progress without total; keep SSE `/api/events`
   working. Count walk errors instead of swallowing them (`filter_map(|e|`
   `e.ok())` at scanner.rs:103,164) — warn per error, include the count in
   the scan report.
4. New persistent job `hash_files` in `jobs.rs` with resume; auto-queued on
   scan completion; manually triggerable via `/api/jobs`.
5. Instrument the pipeline (tracing counters): files/sec, avg metadata
   latency, DB commits/sec — so the next bottleneck after hashing is measured,
   not guessed.
6. Update the scan section of `docs/v1/kahawai-server-spec.md`.

## Acceptance

- First-scan Phase A of the 125k-file SMB share: wall time < 1 h on 1 GbE;
  library browsable immediately after.
- Kill -9 mid-Phase-B → restart resumes with zero rehash of completed files.
- Existing gates green: server tests, clippy `-D warnings`, `cargo fmt`.

## Non-goals

- No change to rescan fast-path semantics. No NAS-side agent. No ML.
