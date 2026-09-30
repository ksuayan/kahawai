//! Log file for the desktop app (macOS): `~/Library/Logs/Kahawai Server/kahawai-server.log`.
//!
//! Opened from Finder, an app's standard output and error go nowhere, so a
//! problem on someone else's Mac would leave nothing to send. When they
//! aren't a terminal, both are pointed at this file: the tracing output,
//! panics and any other printing land there. A run from a terminal (the
//! development scripts) is left as it is. The file is rotated at startup
//! once it passes [`ROTATE_AT`], keeping one older copy (`.log.1`).

use std::fs::{self, OpenOptions};
use std::io::IsTerminal;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

/// Rotate when the log passes this size (10 MB).
const ROTATE_AT: u64 = 10 * 1024 * 1024;

/// Where the logs are kept: `~/Library/Logs/Kahawai Server`.
pub fn log_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join("Library/Logs/Kahawai Server"))
}

/// Open the log file and, when nothing is reading the app's output, send
/// that output to it. Returns the file's path (`None`: no home folder, or
/// it couldn't be written, and output stays where it was).
pub fn init() -> Option<PathBuf> {
    let dir = log_dir()?;
    fs::create_dir_all(&dir).ok()?;
    let path = dir.join("kahawai-server.log");
    if fs::metadata(&path).is_ok_and(|m| m.len() > ROTATE_AT) {
        let _ = fs::rename(&path, dir.join("kahawai-server.log.1"));
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    if !std::io::stderr().is_terminal() {
        // SAFETY: dup2 on two valid descriptors; `file` stays open for the
        // call, and the duplicates keep the file open after it's dropped.
        unsafe {
            libc::dup2(file.as_raw_fd(), libc::STDOUT_FILENO);
            libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
        }
    }
    Some(path)
}

/// Show the log folder in Finder.
pub fn reveal() {
    if let Some(dir) = log_dir() {
        let _ = fs::create_dir_all(&dir);
        let _ = std::process::Command::new("open").arg(dir).spawn();
    }
}
