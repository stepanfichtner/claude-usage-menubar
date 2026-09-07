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

/// Remembers, per quota, the highest threshold already announced for the
/// current window. Dropping back below a threshold does not re-arm it — only a
/// new window does (spec §10).
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

            if entry.window != quota.resets_at {
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
}
