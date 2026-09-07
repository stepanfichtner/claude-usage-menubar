use tauri::AppHandle;

use crate::tray::TitleEntry;

pub struct Settings {
    pub title_entries: Vec<TitleEntry>,
    pub thresholds: Vec<u8>,
}

/// Replaced in Task 9 by the persisted implementation.
pub fn load(_app: &AppHandle) -> Settings {
    Settings {
        title_entries: TitleEntry::defaults(),
        thresholds: vec![50, 80, 90],
    }
}
