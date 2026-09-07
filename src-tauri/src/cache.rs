use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{Profile, UsageSnapshot};

fn snapshot_path(dir: &Path) -> PathBuf {
    dir.join("snapshot.json")
}

fn profile_path(dir: &Path) -> PathBuf {
    dir.join("profile.json")
}

pub fn save_snapshot(dir: &Path, snapshot: &UsageSnapshot) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string(snapshot)?;
    std::fs::write(snapshot_path(dir), json)
}

/// A cached snapshot is always stale by definition, whatever was serialized.
pub fn load_snapshot(dir: &Path) -> Option<UsageSnapshot> {
    let text = std::fs::read_to_string(snapshot_path(dir)).ok()?;
    let mut snapshot: UsageSnapshot = serde_json::from_str(&text).ok()?;
    snapshot.stale = true;
    Some(snapshot)
}

#[derive(Serialize, Deserialize)]
struct StoredProfile {
    profile: Profile,
    stored_at: DateTime<Utc>,
}

pub fn save_profile(dir: &Path, profile: &Profile) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let stored = StoredProfile {
        profile: profile.clone(),
        stored_at: Utc::now(),
    };
    std::fs::write(profile_path(dir), serde_json::to_string(&stored)?)
}

pub fn load_profile(dir: &Path) -> Option<(Profile, DateTime<Utc>)> {
    let text = std::fs::read_to_string(profile_path(dir)).ok()?;
    let stored: StoredProfile = serde_json::from_str(&text).ok()?;
    Some((stored.profile, stored.stored_at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Profile, Quota, Severity, UsageSnapshot};
    use chrono::{TimeZone, Utc};

    fn snapshot() -> UsageSnapshot {
        UsageSnapshot {
            quotas: vec![Quota {
                id: "session".into(),
                label: "Session (5h)".into(),
                percent: 20.0,
                severity: Severity::Normal,
                resets_at: Some(Utc.with_ymd_and_hms(2026, 9, 7, 10, 59, 59).unwrap()),
                is_active: true,
            }],
            fetched_at: Utc.with_ymd_and_hms(2026, 9, 7, 9, 0, 0).unwrap(),
            stale: false,
        }
    }

    #[test]
    fn round_trips_a_snapshot_and_marks_it_stale() {
        let dir = tempfile::tempdir().unwrap();
        save_snapshot(dir.path(), &snapshot()).unwrap();
        let loaded = load_snapshot(dir.path()).unwrap();
        assert_eq!(loaded.quotas, snapshot().quotas);
        assert!(loaded.stale, "a cached snapshot is always stale");
    }

    #[test]
    fn returns_none_when_nothing_is_cached() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_snapshot(dir.path()).is_none());
    }

    #[test]
    fn returns_none_for_a_corrupt_cache_rather_than_failing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("snapshot.json"), "{{{ not json").unwrap();
        assert!(load_snapshot(dir.path()).is_none());
    }

    #[test]
    fn round_trips_a_profile_with_its_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let profile = Profile {
            display_name: "Fichy".into(),
            plan_label: "Claude Max 5×".into(),
        };
        save_profile(dir.path(), &profile).unwrap();
        let (loaded, at) = load_profile(dir.path()).unwrap();
        assert_eq!(loaded, profile);
        assert!(Utc::now().signed_duration_since(at).num_seconds() < 5);
    }

    /// Spec §12.3: the cached profile must not contain personal data beyond the
    /// display name we deliberately show.
    #[test]
    fn the_cached_profile_contains_no_email_or_full_name() {
        let dir = tempfile::tempdir().unwrap();
        save_profile(
            dir.path(),
            &Profile {
                display_name: "Fichy".into(),
                plan_label: "Claude Max 5×".into(),
            },
        )
        .unwrap();
        let text = std::fs::read_to_string(dir.path().join("profile.json")).unwrap();
        assert!(
            !text.contains('@'),
            "cache looks like it holds an email: {text}"
        );
        assert!(!text.contains("fullName"));
    }
}
