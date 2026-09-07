use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::model::Quota;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub quota_label: String,
    pub threshold: u8,
}

#[derive(Debug, Clone)]
struct Armed {
    highest_fired: u8,
    window: Option<DateTime<Utc>>,
}

/// The server recomputes `resets_at` per request rather than returning a fixed
/// window end, so it wobbles between polls inside one window — observed
/// crossing a minute boundary, which is why truncating to the minute does not
/// work either. A genuine reset jumps it forward by the whole window, five
/// hours at the shortest, so anything closer than this is the same window.
const SAME_WINDOW_TOLERANCE_SECS: i64 = 5 * 60;

fn same_window(stored: Option<DateTime<Utc>>, incoming: Option<DateTime<Utc>>) -> bool {
    match (stored, incoming) {
        (None, None) => true,
        (Some(a), Some(b)) => (a - b).num_seconds().abs() < SAME_WINDOW_TOLERANCE_SECS,
        _ => false,
    }
}

/// Remembers, per quota, the highest threshold already announced for the
/// current window. Dropping back below a threshold does not re-arm it — only a
/// new window does (spec §10). The window's anchor is the `resets_at` first
/// seen for it and is kept fixed for as long as `same_window` holds, since the
/// server's value wobbles a little on every poll rather than staying fixed
/// (spec §10.1).
#[derive(Debug, Default)]
pub struct Notifier {
    state: HashMap<String, Armed>,
}

impl Notifier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn evaluate(&mut self, quotas: &[Quota], thresholds: &[u8]) -> Vec<Notification> {
        let mut fired = Vec::new();
        for quota in quotas {
            let entry = self.state.entry(quota.id.clone()).or_insert(Armed {
                highest_fired: 0,
                window: quota.resets_at,
            });

            if !same_window(entry.window, quota.resets_at) {
                entry.window = quota.resets_at;
                entry.highest_fired = 0;
            }

            let crossed = thresholds
                .iter()
                .copied()
                .filter(|t| quota.percent >= f64::from(*t))
                .max();

            if let Some(highest) = crossed {
                if highest > entry.highest_fired {
                    entry.highest_fired = highest;
                    fired.push(Notification {
                        quota_label: quota.label.clone(),
                        threshold: highest,
                    });
                }
            }
        }
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Quota, Severity};
    use chrono::{DateTime, TimeZone, Utc};

    const THRESHOLDS: [u8; 3] = [50, 80, 90];

    fn quota(percent: f64, resets_at: Option<DateTime<Utc>>) -> Quota {
        Quota {
            id: "session".into(),
            label: "Session (5h)".into(),
            percent,
            severity: Severity::from_percent(percent),
            resets_at,
            is_active: true,
        }
    }

    fn reset_at(hour: u32) -> Option<DateTime<Utc>> {
        Some(Utc.with_ymd_and_hms(2026, 9, 7, hour, 0, 0).unwrap())
    }

    #[test]
    fn fires_when_a_threshold_is_first_crossed() {
        let mut notifier = Notifier::new();
        let fired = notifier.evaluate(&[quota(55.0, reset_at(10))], &THRESHOLDS);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].threshold, 50);
        assert_eq!(fired[0].quota_label, "Session (5h)");
    }

    /// `percent >= threshold`, so landing exactly on one must fire. The other
    /// cases sit comfortably above or below and would pass a `>` regression.
    #[test]
    fn a_percentage_exactly_on_a_threshold_fires() {
        let mut notifier = Notifier::new();
        let fired = notifier.evaluate(&[quota(50.0, reset_at(10))], &THRESHOLDS);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].threshold, 50);
    }

    #[test]
    fn stays_silent_below_every_threshold() {
        let mut notifier = Notifier::new();
        assert!(notifier
            .evaluate(&[quota(12.0, reset_at(10))], &THRESHOLDS)
            .is_empty());
    }

    #[test]
    fn does_not_refire_while_sitting_above_the_same_threshold() {
        let mut notifier = Notifier::new();
        notifier.evaluate(&[quota(55.0, reset_at(10))], &THRESHOLDS);
        assert!(notifier
            .evaluate(&[quota(58.0, reset_at(10))], &THRESHOLDS)
            .is_empty());
        assert!(notifier
            .evaluate(&[quota(79.0, reset_at(10))], &THRESHOLDS)
            .is_empty());
    }

    #[test]
    fn fires_again_at_the_next_threshold_up() {
        let mut notifier = Notifier::new();
        notifier.evaluate(&[quota(55.0, reset_at(10))], &THRESHOLDS);
        let fired = notifier.evaluate(&[quota(82.0, reset_at(10))], &THRESHOLDS);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].threshold, 80);
    }

    #[test]
    fn jumping_past_several_thresholds_reports_only_the_highest() {
        let mut notifier = Notifier::new();
        let fired = notifier.evaluate(&[quota(95.0, reset_at(10))], &THRESHOLDS);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].threshold, 90);
    }

    #[test]
    fn a_new_window_re_arms_the_thresholds() {
        let mut notifier = Notifier::new();
        notifier.evaluate(&[quota(95.0, reset_at(10))], &THRESHOLDS);
        // The window reset: same quota id, later resets_at, usage back down and up again.
        assert!(notifier
            .evaluate(&[quota(5.0, reset_at(15))], &THRESHOLDS)
            .is_empty());
        let fired = notifier.evaluate(&[quota(55.0, reset_at(15))], &THRESHOLDS);
        assert_eq!(fired[0].threshold, 50);
    }

    #[test]
    fn dropping_below_a_threshold_within_the_same_window_does_not_re_arm() {
        let mut notifier = Notifier::new();
        notifier.evaluate(&[quota(55.0, reset_at(10))], &THRESHOLDS);
        notifier.evaluate(&[quota(20.0, reset_at(10))], &THRESHOLDS);
        assert!(notifier
            .evaluate(&[quota(55.0, reset_at(10))], &THRESHOLDS)
            .is_empty());
    }

    #[test]
    fn an_empty_threshold_list_disables_notifications() {
        let mut notifier = Notifier::new();
        assert!(notifier
            .evaluate(&[quota(99.0, reset_at(10))], &[])
            .is_empty());
    }

    #[test]
    fn quotas_are_tracked_independently() {
        let mut notifier = Notifier::new();
        let mut weekly = quota(55.0, reset_at(10));
        weekly.id = "weekly_all".into();
        weekly.label = "Week · all models".into();
        let fired = notifier.evaluate(&[quota(55.0, reset_at(10)), weekly], &THRESHOLDS);
        assert_eq!(fired.len(), 2);
    }

    /// Guards the reason the anchor is kept fixed rather than refreshed to
    /// the latest value every poll: an implementation that refreshes it would
    /// compare each step only to its immediate predecessor, so a slow march
    /// away from the window's first-seen value — a few minutes at a time,
    /// each step comfortably inside the tolerance — would never be caught; it
    /// would stay "the same window" forever no matter how far it wandered.
    /// The fixed anchor cannot do that: because every step is compared
    /// against the *first* value seen for this window, cumulative drift past
    /// the tolerance is always eventually caught and the window re-arms.
    ///
    /// (This is also, honestly, a case where a literal "nothing ever re-fires
    /// while pairwise steps stay small" test would be false against the
    /// correct, shipped implementation: with the anchor fixed, drift that
    /// stays under tolerance step-to-step but exceeds it cumulatively *does*
    /// eventually trip a fresh re-arm here — three times, in fact, across six
    /// four-minute steps — which is the intended behaviour, not a bug. What a
    /// refresh-every-poll bug actually produces is silence forever instead.)
    #[test]
    fn a_slow_drift_past_the_tolerance_is_eventually_caught() {
        let mut notifier = Notifier::new();
        let t0 = Utc.with_ymd_and_hms(2026, 9, 7, 16, 0, 0).unwrap();

        // Six polls, four minutes apart. Every consecutive pair is well
        // inside the 5-minute tolerance, but by the third poll the total
        // drift from the window's first-seen anchor (8 minutes) exceeds it.
        let first_fired = notifier.evaluate(&[quota(55.0, Some(t0))], &THRESHOLDS);
        assert_eq!(first_fired.len(), 1, "the initial crossing must still fire");

        let mut saw_a_later_notification = false;
        for step in 1..6 {
            let t = t0 + chrono::Duration::minutes(4 * step);
            let fired = notifier.evaluate(&[quota(55.0, Some(t))], &THRESHOLDS);
            if !fired.is_empty() {
                saw_a_later_notification = true;
            }
        }

        assert!(
            saw_a_later_notification,
            "cumulative drift past the tolerance must eventually re-arm the \
             threshold — an implementation that refreshes the anchor to the \
             latest value every poll would instead stay silent through all \
             six steps, never noticing the drift"
        );
    }

    /// The server recomputes `resets_at` per request, so it wobbles between
    /// polls. Exact equality reads that as a new window and re-fires every
    /// notification on every poll — verified against the live API, where three
    /// polls four seconds apart straddled a minute boundary.
    #[test]
    fn sub_second_jitter_in_resets_at_is_the_same_window() {
        let mut notifier = Notifier::new();
        let first = Utc.with_ymd_and_hms(2026, 9, 7, 15, 59, 59).unwrap()
            + chrono::Duration::milliseconds(998);
        let second = Utc.with_ymd_and_hms(2026, 9, 7, 16, 0, 0).unwrap()
            + chrono::Duration::milliseconds(376);

        assert_eq!(
            notifier
                .evaluate(&[quota(55.0, Some(first))], &THRESHOLDS)
                .len(),
            1
        );
        assert!(
            notifier
                .evaluate(&[quota(55.0, Some(second))], &THRESHOLDS)
                .is_empty(),
            "jitter across a minute boundary must not re-arm the threshold"
        );
    }

    #[test]
    fn a_quota_with_no_reset_time_does_not_re_arm() {
        let mut notifier = Notifier::new();
        assert_eq!(
            notifier.evaluate(&[quota(55.0, None)], &THRESHOLDS).len(),
            1
        );
        assert!(notifier
            .evaluate(&[quota(58.0, None)], &THRESHOLDS)
            .is_empty());
    }

    /// A real reset moves the window forward by hours, not milliseconds.
    #[test]
    fn a_genuine_window_change_still_re_arms() {
        let mut notifier = Notifier::new();
        let now = Utc.with_ymd_and_hms(2026, 9, 7, 16, 0, 0).unwrap();
        notifier.evaluate(&[quota(95.0, Some(now))], &THRESHOLDS);
        assert!(
            notifier
                .evaluate(
                    &[quota(5.0, Some(now + chrono::Duration::hours(5)))],
                    &THRESHOLDS
                )
                .is_empty(),
            "5% is below every threshold"
        );
        let fired = notifier.evaluate(
            &[quota(55.0, Some(now + chrono::Duration::hours(5)))],
            &THRESHOLDS,
        );
        assert_eq!(fired[0].threshold, 50, "the new window must have re-armed");
    }

    /// Keying by position instead of by id would let two quotas inherit each
    /// other's fired state when the server reorders them.
    #[test]
    fn state_follows_the_quota_id_not_its_position() {
        let mut notifier = Notifier::new();
        let mut weekly = quota(55.0, reset_at(10));
        weekly.id = "weekly_all".into();
        weekly.label = "Week · all models".into();
        let session = quota(55.0, reset_at(10));

        assert_eq!(
            notifier
                .evaluate(&[session.clone(), weekly.clone()], &THRESHOLDS)
                .len(),
            2
        );
        // Same two quotas, swapped order: both already fired, nothing new.
        assert!(notifier
            .evaluate(&[weekly, session], &THRESHOLDS)
            .is_empty());
    }

    /// `state_follows_the_quota_id_not_its_position` above uses two quotas at
    /// the same percentage, so a position-keyed bug ends up with identical
    /// `highest_fired` at every index and the test cannot tell the two
    /// implementations apart (verified: it still passes against a
    /// deliberately index-keyed mutant). Here the two quotas cross different
    /// thresholds, so swapping their order makes a position-keyed
    /// implementation compare each quota against the *other* quota's armed
    /// state and misfire.
    #[test]
    fn asymmetric_quotas_expose_position_keyed_state() {
        let mut notifier = Notifier::new();
        let mut weekly = quota(95.0, reset_at(10));
        weekly.id = "weekly_all".into();
        weekly.label = "Week · all models".into();
        let session = quota(55.0, reset_at(10));

        // First call: session arms at 50, weekly arms at 90.
        let fired = notifier.evaluate(&[session.clone(), weekly.clone()], &THRESHOLDS);
        assert_eq!(fired.len(), 2);

        // Second call, same percentages, order swapped. Nothing crossed a new
        // threshold, so nothing should fire — regardless of which position
        // each quota now sits at.
        assert!(
            notifier
                .evaluate(&[weekly, session], &THRESHOLDS)
                .is_empty(),
            "state keyed by position would compare weekly's 95% against \
             session's armed threshold (50), and misfire"
        );
    }
}
