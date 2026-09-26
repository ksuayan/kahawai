//! Native macOS menu bar (Tauri 2).
//!
//! The menu owns no behaviour: every custom item forwards its id through the
//! `menu-action` event and the UI decides what it does (today only
//! `app.about`, which opens the About dialog). The rest are the standard
//! predefined items a Mac app is expected to have; without an Edit menu, Cut,
//! Copy, Paste and Select All would stop working in text fields.

use tauri::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    AppHandle, Emitter, Runtime,
};

/// Frontend event carrying the selected menu item's id.
pub const MENU_EVENT: &str = "menu-action";

/// The id of the custom About item.
pub const ABOUT_ID: &str = "app.about";

/// The app's name as shown in the menu bar.
const APP_NAME: &str = "Kahawai Player";

/// Build the application menu (macOS): app menu with a custom About item, Edit
/// and Window.
#[cfg(target_os = "macos")]
pub fn build_app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let about = MenuItem::with_id(app, ABOUT_ID, format!("About {APP_NAME}"), true, None::<&str>)?;
    let app_menu = Submenu::with_items(
        app,
        APP_NAME,
        true,
        &[
            // A custom item, not the stock macOS panel: the UI shows the bundled
            // about.md and the open-source notices.
            &about,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, Some("Services"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some(&format!("Hide {APP_NAME}")))?,
            &PredefinedMenuItem::hide_others(app, Some("Hide Others"))?,
            &PredefinedMenuItem::show_all(app, Some("Show All"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, Some(&format!("Quit {APP_NAME}")))?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, Some("Undo"))?,
            &PredefinedMenuItem::redo(app, Some("Redo"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some("Cut"))?,
            &PredefinedMenuItem::copy(app, Some("Copy"))?,
            &PredefinedMenuItem::paste(app, Some("Paste"))?,
            &PredefinedMenuItem::select_all(app, Some("Select All"))?,
        ],
    )?;
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some("Minimize"))?,
            &PredefinedMenuItem::maximize(app, Some("Zoom"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, Some("Close Window"))?,
        ],
    )?;
    Menu::with_items(app, &[&app_menu, &edit, &window])
}

/// Forward a custom item's id to the UI.
pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    if event.id().as_ref() == ABOUT_ID {
        let _ = app.emit(MENU_EVENT, ABOUT_ID);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_about_id_matches_what_the_ui_listens_for() {
        // ui/src/App.vue opens the About dialog on this id.
        assert_eq!(ABOUT_ID, "app.about");
        assert_eq!(MENU_EVENT, "menu-action");
    }
}
