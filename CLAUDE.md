# CLAUDE.md

Guidance for Claude Code (and other coding agents) working in this repository.

## What this is

Kahawai is two Rust programs that work together: **Kahawai Server** (self-hosted
music streaming: catalog, HTTP Range streaming, transcoding, background jobs)
and **Kahawai Player** (Tauri 2 + Vue 3 desktop client). No cloud, no accounts,
trusted-LAN only. Read [README.md](README.md) and [docs/v1/DESIGN.md](docs/v1/DESIGN.md)
before making architectural changes — the latter states the seven governing
principles; when two things conflict, the earlier principle wins.

## Repo layout

```
crates/
  kahawai-core/          shared types (API models, config, errors) — no I/O, compiles anywhere
  kahawai-server/        axum binary; on macOS this crate IS a Tauri app (see below)
  kahawai-player-core/   platform-independent playback engine (state machine, DSP, queue) — zero Tauri
  kahawai-player-api/    async reqwest client for the server API
  kahawai-player-audio/  cpal audio output
player/
  ui/                    Vue 3 + Vite + Pinia + strict TypeScript frontend
  src-tauri/              Tauri 2 shell (excluded from the cargo workspace — needs GTK/WebKit on Linux)
docs/                    see "Documentation" below
scripts/                 setup, build and dev-loop scripts — read one before reinventing it
guidelines/              house style guides (visual, editorial, Rust concurrency, Tauri/Reka/Pinia)
```

**`kahawai-server` is macOS-special.** `crates/kahawai-server/src/main.rs` has two
`main()` functions behind `cfg(target_os = "macos")`: on macOS the binary *is* a
Tauri wizard/tray app (`crates/kahawai-server/tauri.conf.json`, UI in
`crates/kahawai-server/ui/`) that autostarts the real axum server in-process;
everywhere else it's a plain headless binary. Don't assume `cargo run -p
kahawai-server` gives you a headless process on macOS — it doesn't.

## Documentation

Docs live under `docs/`, split by how settled the material is:

- **`docs/v1/`** — design & feature docs for what's actually built and shipped.
- **`docs/v2/`** — specs for work that has **not been started** (the folder
  doesn't exist while there are none). Verify against the actual code before
  trusting a v2 doc's "current state" section — that's a snapshot from
  whenever the spec was written.
- **`docs/users/`** — non-technical, general-audience material (press release,
  background articles). Nothing here assumes the reader is a contributor.
- **`docs/Backlog.md`** — deferred work with enough context to pick up cold,
  including a section mapping each spec that began in `docs/v2/` to its status. Update this
  when you defer something or finish a backlog item.
- **`docs/Roadmap.md`** — possibilities, not commitments.

**When a `docs/v2/` spec gets implemented:** move its file to `docs/v1/` (`git
mv`), update its Backlog.md entry, and fix any links pointing at the old path
(grep for the filename across `.md`/`.ts`/`.rs` — at least one test reads a doc
file by relative path: `player/ui/src/stores/analog.test.ts` reads
`docs/v1/Analog-Emulation.md` to check the listening recipes stay in sync).

## Build, test, lint

```bash
cargo check --workspace                                  # fast validation, works on Linux too
cargo test --workspace                                   # hermetic: temp dirs, temp SQLite, no network
cargo clippy --workspace --all-targets -- -D warnings     # must be clean
cargo fmt                                                # CI runs `cargo fmt --all --check`; the toolchain is pinned in rust-toolchain.toml

cd player/ui && npm test                                  # Vitest + Vue Test Utils + happy-dom
cd crates/kahawai-server/ui && npm test                    # server wizard UI tests
```

Gate for every change: `cargo check` zero warnings, full suite green, clippy
clean, `cargo fmt` applied (run it before committing — CI rejects unformatted code). The player's Tauri shell (`player/src-tauri`) is
excluded from the workspace and needs macOS (or GTK/WebKit dev libs) to link —
`cargo check --workspace` staying green on Linux is intentional; don't add it
back to the workspace.

## Day-to-day scripts (`scripts/`)

| Script | Use |
|---|---|
| `setup.sh` | Guided wizard for `config.toml` and player settings. |
| `start-server.sh` / `start-client.sh` | Run built binaries; `-d` backgrounds the server, `--dev` runs the player's Tauri dev mode. |
| `start-dev-combined.sh` | Iterative dev: both the server's wizard UI (`:1421`) and the player (`:1420`) via `cargo tauri dev`, hot-reloading, in parallel. macOS only. |
| `build-*-universal.sh`, `build-combined-dmg.sh` | Release bundles (Mac-only, universal binaries). |

## Conventions worth knowing before editing

- **AGPL-3.0-or-later.** Server-as-a-service must keep source available; keep
  that in mind for anything that touches licensing or third-party deps
  (`player/ui/src/content/notices.md` tracks those).
- **No SACD extraction, ever** — a deliberate scope decision, not a gap. See
  `docs/v1/SACD-Extraction.md` before touching `.iso` handling.
- **The server is trusted-LAN only** — no auth, no TLS in v1 by design. Don't
  add network-facing conveniences that assume a hostile network.
- Style guides for UI, editorial tone, Rust concurrency patterns, and
  Tauri/Reka/Pinia usage live in `guidelines/` — check them before writing UI
  code or prose docs.
- Rust tests are hermetic (temp dirs/DB, no network) — keep new tests that way.
