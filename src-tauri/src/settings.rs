use tauri::AppHandle;

use crate::tray::TitleEntry;

pub struct Settings {
    pub title_entries: Vec<TitleEntry>,
}

/// Replaced in Task 9 by the persisted implementation.
pub fn load(_app: &AppHandle) -> Settings {
    Settings {
        title_entries: TitleEntry::defaults(),
    }
}
