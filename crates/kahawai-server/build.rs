/// Build identity for `GET /api/identity`: the git commit (and whether the
/// tree had uncommitted changes), when this was built, and for what. Read
/// back with `env!("KAHAWAI_…")` in api.rs. Re-stamped when the commit moves
/// or the server's sources change.
fn stamp_build_info() {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let commit = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    println!("cargo:rustc-env=KAHAWAI_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=KAHAWAI_GIT_DIRTY={dirty}");
    println!("cargo:rustc-env=KAHAWAI_BUILT_AT={}", utc_now_iso());
    println!(
        "cargo:rustc-env=KAHAWAI_TARGET={}",
        std::env::var("TARGET").unwrap_or_default()
    );
    println!(
        "cargo:rustc-env=KAHAWAI_PROFILE={}",
        std::env::var("PROFILE").unwrap_or_default()
    );

    // The commit moves: HEAD itself, the branch it points at, packed refs
    // (worktree-aware paths).
    let git_path = |p: &str| git(&["rev-parse", "--path-format=absolute", "--git-path", p]);
    let mut watch: Vec<String> = ["HEAD", "packed-refs"]
        .iter()
        .filter_map(|p| git_path(p))
        .collect();
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch.extend(git_path(&branch));
    }
    watch.extend(["src".to_string(), "migrations".to_string()]);
    for path in watch {
        println!("cargo:rerun-if-changed={path}");
    }
}

/// Now, UTC, as "2026-09-30T19:02:11Z" (no date crate in a build script).
fn utc_now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since 1970 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn main() {
    stamp_build_info();
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
