use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

use crate::tray::TitleEntry;

pub const STORE_FILE: &str = "settings.json";

/// Raised from 30 to 60 (see `the_poll_interval_floor_is_inclusive` below for
/// why): the limit is burst-sensitive rather than rate-sensitive, recovery
/// from a 429 takes about two minutes, and the windows being tracked are
/// measured in hours and days, so a sub-minute poll buys nothing.
pub const MIN_POLL_INTERVAL_SECS: u64 = 60;

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

// Generic over the Tauri runtime, not just `AppHandle` (= `AppHandle<Wry>`),
// so tests can drive this against `tauri::test`'s `MockRuntime` instead of a
// real webview. Every production call site passes a concrete `&AppHandle`
// (Wry), which still satisfies this bound with `R` inferred, so nothing
// downstream changes.
pub fn load<R: Runtime>(app: &AppHandle<R>) -> Settings {
    let Ok(store) = app.store(STORE_FILE) else {
        return Settings::default();
    };
    store
        .get("settings")
        .and_then(|value| serde_json::from_value::<Settings>(value).ok())
        .unwrap_or_default()
        .sanitized()
}

pub fn save<R: Runtime>(app: &AppHandle<R>, settings: &Settings) -> Result<(), String> {
    let sanitized = settings.clone().sanitized();
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    store.set(
        "settings",
        serde_json::to_value(&sanitized).map_err(|e| e.to_string())?,
    );
    store.save().map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) mod tests {
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
        assert_eq!(settings.poll_interval_secs, 60);
    }

    /// The floor itself: 60 must survive untouched, 59 must be lifted. Written
    /// as literals rather than `MIN_POLL_INTERVAL_SECS - 1` / `+ 1` on
    /// purpose — a symbolic-only version of this table would still pass after
    /// someone quietly changed the constant, since both sides would move
    /// together. 60 is not an arbitrary round number either: it is a
    /// measured floor (see the rate-limit provenance note on
    /// `poller::MIN_MANUAL_REFRESH_SECS`'s canary test), so moving it should
    /// mean re-justifying it against the endpoint's behavior, not editing
    /// this assertion.
    #[test]
    fn the_poll_interval_floor_is_inclusive() {
        for (given, expected) in [(59u64, 60u64), (60, 60), (61, 61)] {
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

    /// `sanitized()` is applied on both `load` and `save`, so a value that has
    /// already been through it once must come out unchanged the second time —
    /// otherwise the two call sites could keep nudging a value further on
    /// every round trip.
    #[test]
    fn sanitized_is_idempotent() {
        let once = Settings {
            poll_interval_secs: 5,
            thresholds: vec![90, 50, 50, 0, 101, 80],
            ..Settings::default()
        }
        .sanitized();
        let twice = once.clone().sanitized();
        assert_eq!(once, twice);
    }

    /// Serializes every test that overrides `HOME`.
    ///
    /// `HOME` is process-wide and the test harness runs tests on several
    /// threads, so two guards alive at once would each restore the other's
    /// temp directory. This was a latent hazard while `settings` was the only
    /// module scoping `HOME`; it stopped being latent when
    /// `lib::tests::analytics_summary_*` began scoping it too, because those
    /// tests resolve `~/.claude/projects` through it.
    ///
    /// A poisoned lock is taken anyway: a panicking test has already failed,
    /// and turning that into a cascade of failures in unrelated tests would
    /// only hide which one broke.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Restores the previous `HOME` on drop, even if the test panics, so one
    /// test's temp-directory override can never leak into another, and holds
    /// `HOME_LOCK` for its lifetime so no two overrides overlap.
    ///
    /// `Drop::drop` runs before the struct's fields are dropped, so `HOME` is
    /// always restored before the lock is released.
    pub(crate) struct HomeGuard(
        Option<std::ffi::OsString>,
        #[allow(dead_code)] std::sync::MutexGuard<'static, ()>,
    );

    impl HomeGuard {
        pub(crate) fn scoped_to(dir: &std::path::Path) -> Self {
            let lock = HOME_LOCK.lock().unwrap_or_else(|held| held.into_inner());
            let previous = std::env::var_os("HOME");
            std::env::set_var("HOME", dir);
            Self(previous, lock)
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(home) => std::env::set_var("HOME", home),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    /// A real `tauri::test::MockRuntime` app with the real `tauri-plugin-store`
    /// registered — not a hand-rolled substitute. Call this only after
    /// scoping `HOME` (see `HomeGuard`), so its resolved app-data directory
    /// can never land in a real one.
    pub(crate) fn mock_app_with_store() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .plugin(tauri_plugin_store::Builder::new().build())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app")
    }

    #[test]
    fn load_survives_a_corrupt_store_file_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let _home = HomeGuard::scoped_to(dir.path());
        let app = mock_app_with_store();
        let handle = app.handle();

        // Seed the exact file the store plugin will read, with content that
        // is not valid JSON at all, before the plugin ever touches it — this
        // is what a hand-edited or half-written store file looks like from
        // the plugin's point of view on its very first read.
        let data_dir = tauri::Manager::path(handle).app_data_dir().unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(data_dir.join(STORE_FILE), b"{ this is not valid json !!!").unwrap();

        let loaded = load(handle);
        assert_eq!(loaded, Settings::default());
    }
}
