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

    /// The stored timestamp is bracketed between two readings of the same
    /// clock rather than measured against a fixed tolerance. The previous
    /// version allowed five seconds of slack, which was both flaky and loose:
    /// a CI box that stalled between `save_profile` and the assertion failed
    /// it, while a `save_profile` that wrote a timestamp four seconds stale
    /// passed it.
    ///
    /// There is no clock seam on this path — `save_profile` reads `Utc::now()`
    /// itself — and cutting one is not worth it for a value only ever compared
    /// against `PROFILE_MAX_AGE_HOURS`. Bracketing does not need a seam:
    /// whatever the wall clock says and however long the machine takes over
    /// the call, the reading taken inside `save_profile` lies between the one
    /// taken before it and the one taken after. That makes this both
    /// unflakeable and tighter than the tolerance it replaces: it fails on a
    /// timestamp that is stale by any amount at all.
    ///
    /// The bracket alone does *not* catch a `load_profile` that reads the
    /// clock instead of returning the stored field — such a value lands
    /// inside `[before, after]` too. The second read below is what catches
    /// that, and it is worth the two milliseconds: this is the only test
    /// asserting that `load_profile`'s timestamp is the stored one, and
    /// `poller.rs`'s startup read is its only production consumer, where a
    /// re-read clock would report the cached profile as freshly fetched and
    /// suppress the refetch forever.
    #[test]
    fn round_trips_a_profile_with_its_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let profile = Profile {
            display_name: "Fichy".into(),
            plan_label: "Claude Max 5×".into(),
        };

        let before = Utc::now();
        save_profile(dir.path(), &profile).unwrap();
        let (loaded, stored_at) = load_profile(dir.path()).unwrap();
        let after = Utc::now();

        assert_eq!(loaded, profile);
        assert!(
            (before..=after).contains(&stored_at),
            "stored_at {stored_at} falls outside [{before}, {after}], so it is \
             not the reading save_profile took"
        );

        // Long enough that any clock this runs on can tell the two reads
        // apart, short enough not to be felt.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let (_, read_again) = load_profile(dir.path()).unwrap();
        assert_eq!(
            read_again, stored_at,
            "two reads of the same file disagreed, so load_profile is \
             reporting a clock rather than the stored timestamp"
        );
    }

    /// Spec §12.3: nothing personal beyond the display name may reach the
    /// disk.
    ///
    /// This replaces `the_cached_profile_contains_no_email_or_full_name`,
    /// which hand-built a two-field `Profile` and asserted the JSON held no
    /// `@`. Nothing could have made that fail. `Profile` has exactly two
    /// `String` fields and the test supplied both, so the assertion was about
    /// its own literals: a `Profile` that grew an `email` would have passed it
    /// unchanged for as long as the test kept using the old constructor, and a
    /// field named `accountName` would have passed it however it was built.
    ///
    /// What is worth guarding is the shape of the file, so that is what this
    /// asserts — `stored_at` plus a profile object holding exactly
    /// `displayName` and `planLabel`. `profile.rs`'s
    /// `reads_only_display_name_and_tier` guards the other end, what may be
    /// deserialized out of the API response, so a new personal field has to
    /// get past two tests to land on disk.
    ///
    /// It fails on *any* added field, harmless ones included. That is the
    /// point: the question it forces is "does this belong on disk", and the
    /// cost of answering it is one line here.
    ///
    /// One hole, for whoever hits that compile error next: this watches the
    /// *serialized* key set, so a field carrying `#[serde(skip)]`, or
    /// `skip_serializing_if = "Option::is_none"` and left `None` in the
    /// literal below, slips through here while `fetch_profile` populates it
    /// in production. If you are filling in a new field to make this compile,
    /// give it a value that would actually be written and check what happens
    /// — the key set is a tripwire, not the whole guard.
    #[test]
    fn the_cached_profile_holds_only_the_two_fields_it_is_allowed_to() {
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
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();

        let mut wrapper: Vec<&str> = value
            .as_object()
            .expect("the cache file is a JSON object")
            .keys()
            .map(String::as_str)
            .collect();
        wrapper.sort_unstable();
        assert_eq!(wrapper, ["profile", "stored_at"]);

        let mut fields: Vec<&str> = value["profile"]
            .as_object()
            .expect("the profile is a JSON object")
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            ["displayName", "planLabel"],
            "a new field reached the cache file: {text}"
        );
    }
}
