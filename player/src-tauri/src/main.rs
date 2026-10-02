//! Desktop entry point. The app itself lives in the library (`lib.rs`) so
//! mobile builds can load it as a cdylib through `tauri::mobile_entry_point`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    kahawai_player::run();
}
