# Kahawai Server — macOS Desktop UI: First-Run Setup Wizard

Spec — 2026-09-27. Decisions ratified by Kyo; do not relitigate without asking.

## Decisions

1. The UI is **built into the `kahawai-server` binary on macOS only** (target-gated). Linux and Windows stay headless, byte-for-byte today's behavior.
2. Config home: `~/Library/Application Support/Kahawai Server/config.toml`. Resolution order: `argv[1]` (explicit) → `./config.toml` (if it exists, legacy) → app-support default. The wizard always writes the app-support default.
3. Wizard scope: music folder locations (multiple) + database path. Bind address under an Advanced disclosure. Everything else keeps server defaults.
4. Trigger: on macOS launch with no usable config, the app opens the wizard. Afterwards the same window shows a minimal status view.

## Architecture

```
crates/kahawai-server/
  src/main.rs            # extracts run_server(); branches on target_os
  src/desktop.rs         # NEW, #![cfg(target_os = "macos")]: Tauri setup + commands
  build.rs               # NEW: tauri_build::build() when TARGET is apple-darwin
                         # and ui/dist exists (skip with warning otherwise, so
                         # plain `cargo build` keeps working without npm)
  tauri.conf.json        # NEW: productName "Kahawai Server",
                         # identifier com.suayan.kahawai-server,
                         # frontendDist "ui/dist",
                         # beforeBuildCommand "npm --prefix ui run build",
                         # devUrl http://localhost:1421 (player uses 1420)
  capabilities/
    default.json         # NEW: dialog:allow-open (mirrors player's)
  icons/                 # NEW: generated on the Mac via `tauri icon`
                         # from the 1024px server icon PNG
  ui/                    # NEW: Vue 3 + Reka UI + Pinia + Tailwind wizard app
```

Cargo:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
tauri = { version = "2" }
tauri-plugin-dialog = { version = "2" }

[target.'cfg(target_os = "macos")'.build-dependencies]
tauri-build = { version = "2" }
```

No workspace impact on Linux: the deps vanish from the graph there, so `cargo check/test/clippy --workspace` stays green. `src/desktop.rs` is entirely `#[cfg(target_os = "macos")]`; keep Tauri-free logic out of it.

### kahawai-core additions (cross-platform, Linux-tested)

- `ServerConfig::default_config_path() -> PathBuf`
  - macOS: `$HOME/Library/Application Support/Kahawai Server/config.toml`
  - Linux: `$XDG_CONFIG_HOME/kahawai-server/config.toml`, else `$HOME/.config/kahawai-server/config.toml`
  - Windows: `%APPDATA%\Kahawai Server\config.toml`
  - No new crates: read env vars directly.
- `ServerConfig::save(&self, path: &Path) -> Result<(), MusicError>` (`create_dir_all` parents, `toml::to_string_pretty`).
- `ServerConfig::resolve_path(argv1: Option<&Path>) -> PathBuf` (explicit → `./config.toml` if exists → `default_config_path()`).
- Unit tests for all three (missing `$HOME` → error path covered).

### scanner.rs

- `fn is_audio` → `pub(crate) fn is_audio` for reuse by the dir validator. (No behavior change.)

### main.rs refactor

Extract the current main body (after config load) into:

```rust
async fn run_server(config: ServerConfig) -> anyhow::Result<()>
```

covers: db open, jobs, `scan_on_startup`, bind, `axum::serve` with graceful shutdown. Both entry points call it.

- `#[cfg(not(target_os = "macos"))]`: today's `#[tokio::main] async fn main()`, unchanged apart from calling `run_server` and the new resolve order.
- `#[cfg(target_os = "macos")]`: plain `fn main()` → `tauri::Builder`:
  - `.setup()`: resolve path → `ServerConfig::load`.
    - Ok → spawn server task, show status view.
    - Err → show wizard view.
  - Tauri state: `DesktopState { server_task: Mutex<Option<JoinHandle<()>>> }`.
  - Server runs via `tauri::async_runtime::spawn(run_server(cfg))`. Bind failure → surface to UI, stay on wizard/status with the error.
  - Window: single window, 720×560, view switches wizard ↔ status.

## Tauri commands (`src/desktop.rs`, thin like the player's shell)

| Command | Purpose |
|---|---|
| `setup_get_state` | `{ config_path, config_exists, config?: ServerConfig }` for prefill |
| `setup_pick_directory` | `dialog` plugin, directory mode → `Option<String>` |
| `setup_validate_dir(path)` | `{ exists, is_dir, readable, writable, audio_files }`; walkdir, stops after 2000 files, `is_audio` per file; writability via create/remove of a probe file |
| `setup_save_config(input)` | Validates (≥1 music dir, `db_dir` writable, bind parses as `SocketAddr`), writes `ServerConfig::save` to the default path. `Result<(), String>` |
| `setup_start_server` | Spawns `run_server`; stores handle. `Result<{running, bind}, String>` |
| `setup_server_status` | `{ running: bool, bind: String }` |
| `setup_reveal_config` | `open` the config dir in Finder |
| `setup_quit` | `app.exit(0)` |

`SetupInput { music_dirs: Vec<String>, db_dir: String, bind: String }`. `db_path` is derived: `<db_dir>/music.db`. `bind` default `0.0.0.0:8080`.

## UI (`crates/kahawai-server/ui`)

Mirrors `player/ui` conventions. `package.json` copies player deps (vue, reka-ui, pinia, `@tauri-apps/api`, lucide-vue-next, tailwindcss v4, `@tailwindcss/vite`, fontsource Plex, vitest/happy-dom/vue-tsc) **plus** `@tauri-apps/plugin-dialog`. Vite alias `@pw` → `../../../player/ui/src` for primitives (`UiButton`, `UiDialog`, `UiInput`, `UiHint`, `UiSelect`, `ViewShell`, `cn`) and the tauri mock in tests. `style.css` imports the player's token theme (verify no player-specific leakage; else copy the `@theme` block).

No vue-router (player doesn't use one): `useSetupStore` holds `view: "wizard" | "status"` and `step: 0..4`, mirroring player store idioms. `ui/tauri.ts`: guarded `invoke` wrapper (soft-fail under plain `vite dev`), same as the player's.

### Wizard steps

0. **Welcome** — what this sets up; shows the config path (UiHint).
1. **Music folders** — rows: path, validation chip (`ok` "N audio files" / `warn` "no audio files found" / `err` "not accessible"), Add (folder picker), Remove. Continue requires ≥1 `ok`.
2. **Database** — folder picker for the DB directory; shows resolved `<dir>/music.db`; requires writable.
3. **Review** — summary of folders + DB + config path; Advanced disclosure: bind address (UiInput, validated as `SocketAddr`).
4. **Done** — "Configuration saved to `<path>`"; [Start Server & Continue] → attempts start → status view (bind errors return here with the message); [Reveal in Finder]; [Quit].

### Status view

"Server running" + bind address; [Edit Configuration] → wizard prefilled from `setup_get_state` (step 1); after saving, "Restart now to apply" → `app.restart()` (verify availability on the Mac); [Reveal Config in Finder]; [Quit Server]. Config edits never hot-reload in v1 — stated in the UI.

## Out of scope (v1)

System-tray icon, single-instance guard, in-place config reload, scan progress in the wizard, launch-at-login. Closing the window never quits the server; Cmd+Q / Dock quit does. If the window is closed mid-wizard, relaunch returns to the wizard (config still missing).

## Packaging

- `scripts/build-server-app.sh` (macOS only): `npm --prefix crates/kahawai-server/ui ci && npm run build`, then `tauri build --target universal-apple-darwin` → `dist/Kahawai Server.app`. Replaces `build-server-universal.sh` for macOS distribution (that script's lipo flow stays valid for the headless binary; decide keep/retire in P4).
- Icons via `tauri icon` on the Mac from the 1024px server PNG.
- README + `kahawai-server-spec.md`: short section on the desktop UI, config resolution order, and the macOS/elsewhere split.

## Gates

- `cargo check --workspace`, `cargo test --workspace`, clippy, fmt: green on Linux (Tauri code absent from the graph there).
- New core tests: `default_config_path` per-platform logic (env-overrideable in tests), `save` round-trip, `resolve_path` ordering.
- UI: `vue-tsc --noEmit`, `vitest run`, `vite build` green; component tests for step gating; reuse player's tauri-mock via the alias.
- Mac-only validation (his checklist): first `tauri build`; wizard on a fresh profile (no config); picker → save → TOML at the app-support path; server serves; restart applies edits; universal `.app`; signing/notarization.

## Phases

- **P1** — kahawai-core: path/save/resolve + tests; main.rs `run_server` extraction; scanner `is_audio` visibility. Fully Linux-gated.
- **P2** — `crates/kahawai-server/ui`: wizard + store + tests (works in `vite dev` against the mock).
- **P3** — macOS shell: target deps, build.rs, tauri.conf, capabilities, `src/desktop.rs`, main branching.
- **P4** — packaging: icons, `build-server-app.sh`, docs, full gates.
