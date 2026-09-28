fn main() {
    #[cfg(target_os = "macos")]
    {
        // `tauri::generate_context!()` (in main.rs) embeds `ui/dist` into the
        // binary at compile time. Skipping that silently — the earlier
        // version of this file just warned and moved on — produces a binary
        // that compiles and runs fine but shows a genuinely blank window,
        // with nothing printed anywhere to explain why. Build it here
        // instead, so the only way to end up without it is npm itself
        // failing, which does show up loudly.
        let ui_dir = std::path::Path::new("ui");
        let dist_index = ui_dir.join("dist").join("index.html");
        if !dist_index.exists() {
            println!(
                "cargo:warning=kahawai-server: ui/dist is missing — building the desktop UI \
                 (npm install && npm run build)…"
            );
            let install_ok = std::process::Command::new("npm")
                .arg("install")
                .current_dir(ui_dir)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            let build_ok = install_ok
                && std::process::Command::new("npm")
                    .args(["run", "build"])
                    .current_dir(ui_dir)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
            if !build_ok || !dist_index.exists() {
                panic!(
                    "kahawai-server: could not build the desktop UI automatically. Run \
                     `npm --prefix ui install && npm --prefix ui run build` by hand (check for \
                     the actual npm error above), then rebuild kahawai-server."
                );
            }
        }
        tauri_build::build();
    }
}
