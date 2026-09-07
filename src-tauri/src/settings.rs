use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

use crate::tray::TitleEntry;

pub const STORE_FILE: &str = "settings.json";
pub const MIN_POLL_INTERVAL_SECS: u64 = 30;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub poll_interval_secs: u64,
    pub title_entries: Vec<TitleEntry>,
    pub thresholds: Vec<u8>,
    pub notifications_enabled: bool,
    pub launch_at_login: bool,
    pub analytics_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_interval_secs: 60,
            title_entries: TitleEntry::defaults(),
            thresholds: vec![50, 80, 90],
            notifications_enabled: true,
            launch_at_login: false,
            analytics_enabled: false,
        }
    }
}

impl Settings {
    /// Coerce anything a hand-edited store file or a stale build could contain
    /// into something the rest of the app can rely on.
    pub fn sanitized(mut self) -> Self {
        self.poll_interval_secs = self.poll_interval_secs.max(MIN_POLL_INTERVAL_SECS);
        if self.notifications_enabled {
            self.thresholds.retain(|t| *t > 0 && *t <= 100);
            self.thresholds.sort_unstable();
            self.thresholds.dedup();
        } else {
            self.thresholds.clear();
        }
        self
    }
}

pub fn load(app: &AppHandle) -> Settings {
    let Ok(store) = app.store(STORE_FILE) else {
        return Settings::default();
    };
    store
        .get("settings")
        .and_then(|value| serde_json::from_value::<Settings>(value).ok())
        .unwrap_or_default()
        .sanitized()
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let sanitized = settings.clone().sanitized();
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    store.set(
        "settings",
        serde_json::to_value(&sanitized).map_err(|e| e.to_string())?,
    );
    store.save().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let settings = Settings::default();
        assert_eq!(settings.poll_interval_secs, 60);
        assert_eq!(settings.thresholds, vec![50, 80, 90]);
        assert!(settings.notifications_enabled);
        assert!(!settings.analytics_enabled, "analytics is off by default");
        assert!(!settings.launch_at_login);
        assert_eq!(settings.title_entries, TitleEntry::defaults());
    }

    #[test]
    fn the_poll_interval_is_clamped_to_the_floor() {
        let settings = Settings {
            poll_interval_secs: 5,
            ..Settings::default()
        }
        .sanitized();
        assert_eq!(settings.poll_interval_secs, 30);
    }

    /// The floor itself: 30 must survive untouched, 29 must be lifted.
    #[test]
    fn the_poll_interval_floor_is_inclusive() {
        for (given, expected) in [(29u64, 30u64), (30, 30), (31, 31)] {
            let settings = Settings {
                poll_interval_secs: given,
                ..Settings::default()
            }
            .sanitized();
            assert_eq!(settings.poll_interval_secs, expected, "input {given}");
        }
    }

    #[test]
    fn a_generous_poll_interval_is_left_alone() {
        let settings = Settings {
            poll_interval_secs: 600,
            ..Settings::default()
        }
        .sanitized();
        assert_eq!(settings.poll_interval_secs, 600);
    }

    #[test]
    fn thresholds_are_sorted_deduplicated_and_bounded() {
        let settings = Settings {
            thresholds: vec![90, 50, 50, 0, 101, 80],
            ..Settings::default()
        }
        .sanitized();
        assert_eq!(settings.thresholds, vec![50, 80, 90]);
    }

    /// The retain predicate is `> 0 && <= 100`. The case above uses 0 and 101,
    /// which sit outside both edges; 100 is the value that must survive.
    #[test]
    fn a_hundred_percent_is_a_valid_threshold() {
        let settings = Settings {
            thresholds: vec![100, 1],
            ..Settings::default()
        }
        .sanitized();
        assert_eq!(settings.thresholds, vec![1, 100]);
    }

    #[test]
    fn disabling_notifications_empties_the_threshold_list() {
        let settings = Settings {
            notifications_enabled: false,
            ..Settings::default()
        }
        .sanitized();
        assert!(settings.thresholds.is_empty());
    }

    #[test]
    fn settings_round_trip_through_json() {
        let original = Settings::default();
        let json = serde_json::to_string(&original).unwrap();
        let parsed: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let parsed: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, Settings::default());
    }
}
