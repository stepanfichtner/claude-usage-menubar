use std::collections::hash_map::Entry;
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
    /// The highest level already accounted for in this window: a threshold
    /// that fired, or a percentage this app simply observed without
    /// announcing. Every evaluation raises it to at least the observed
    /// percentage, so a climb watched while notifications were off is
    /// accounted for rather than left to announce itself later. See
    /// `primed_level`. It never falls within a window; only a new window
    /// zeroes it.
    highest_fired: u8,
    window: Option<DateTime<Utc>>,
}

/// The server recomputes `resets_at` per request rather than returning a fixed
/// window end, so it wobbles between polls inside one window — observed
/// crossing a minute boundary, which is why truncating to the minute does not
/// work either. A genuine reset jumps it forward by the whole window, five
/// hours at the shortest, so anything closer than this is the same window.
const SAME_WINDOW_TOLERANCE_SECS: i64 = 5 * 60;

/// What priming records: the observed percentage itself, floored to a whole
/// percent, rather than the highest configured threshold beneath it. Applied
/// on the first sight of a quota and again after every later evaluation.
///
/// The distinction is invisible while notifications are on *and the
/// threshold list is not changing* (every threshold is an integer, so the
/// floored percentage is never below the highest one crossed, and the two
/// records answer every later comparison identically) and load-bearing while
/// they are off. With notifications off,
/// `settings::sanitized()` hands `evaluate` an empty threshold list, so
/// nothing counts as crossed and a threshold-derived priming would record 0 —
/// and the first poll after the user switches notifications back on would
/// then announce levels crossed while this app was watching and deliberately
/// silent. The record has to reflect what was seen, not what was enabled.
///
/// This is also why the ordering in `evaluate` matters: the observed
/// percentage is folded in *after* the crossing check, never before. Folded
/// in first it would swallow every crossing, since the highest threshold at
/// or below a percentage can never exceed that percentage floored.
///
/// One consequence, since folding the observation in on every poll is
/// stricter than folding it in only on first sight: adding a threshold
/// *below* the quota's current usage no longer announces it. Someone sitting
/// at 85% who has climbed there while configured for `[90]` and then adds 80
/// stays silent, because 85 is already recorded. That is the decided rule
/// working — 80 was crossed while this app watched and did not announce it,
/// and turning a threshold on is no more a crossing than turning
/// notifications on is — but it is a behaviour change from a
/// threshold-derived record, which would have held 0 and fired.
fn primed_level(percent: f64) -> u8 {
    // `as u8` on a float saturates rather than wrapping, and every threshold
    // is <= 100, so clamping here only makes the stored value readable as the
    // percentage it is.
    percent.floor().clamp(0.0, 100.0) as u8
}

/// What a poll does to the anchor stored for a quota.
///
/// Deliberately not an `Option<DateTime<Utc>>`: `None` there would have to
/// mean both "this quota has no anchor" and "leave the stored anchor alone",
/// and those are the two things a `(Some a, None)` poll has to tell apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnchorUpdate {
    /// Leave the stored anchor exactly as it is.
    Keep,
    /// Store this timestamp as the anchor for future comparisons.
    Adopt(DateTime<Utc>),
}

/// The two independent answers one poll's `resets_at` gives about a quota's
/// window. They used to be a single `same_window` bool, which forced them to
/// agree: "not a new window" also meant "do not touch the anchor", and a
/// quota whose reset time arrived late could then never anchor at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowVerdict {
    /// Whether this poll starts a new window, which zeroes `highest_fired`
    /// and lets every threshold under the current percentage announce again.
    re_arms: bool,
    /// What to do with the stored anchor, regardless of the answer above.
    anchor: AnchorUpdate,
}

/// Reads one poll's `resets_at` against the anchor stored for that quota.
///
/// | stored → incoming            | re-arms | anchor      |
/// |------------------------------|---------|-------------|
/// | `(None, None)`               | no      | `Keep`      |
/// | `(Some a, Some b)` within tol | no     | `Keep`      |
/// | `(Some a, Some b)` beyond tol | **yes** | `Adopt(b)`  |
/// | `(Some a, None)`             | no      | `Keep` (`a`) |
/// | `(None, Some b)`             | no      | `Adopt(b)`  |
///
/// Only `Some → Some` past the tolerance re-arms. A `resets_at` that appears
/// or disappears is far more likely a transient gap in an undocumented API's
/// response than a window boundary — a real reset moves the timestamp forward
/// by the whole window, five hours at the shortest, and shows up as
/// `Some → Some` well past the tolerance.
fn classify_window(
    stored: Option<DateTime<Utc>>,
    incoming: Option<DateTime<Utc>>,
) -> WindowVerdict {
    match (stored, incoming) {
        // Nothing on either side has ever said where this window ends, so
        // nothing can say it moved.
        (None, None) => WindowVerdict {
            re_arms: false,
            anchor: AnchorUpdate::Keep,
        },
        (Some(a), Some(b)) => {
            if (a - b).num_seconds().abs() < SAME_WINDOW_TOLERANCE_SECS {
                // The server's per-request wobble. Keeping the first-seen
                // anchor rather than adopting `b` is what makes cumulative
                // drift detectable at all; see
                // `a_slow_drift_past_the_tolerance_is_eventually_caught`.
                WindowVerdict {
                    re_arms: false,
                    anchor: AnchorUpdate::Keep,
                }
            } else {
                WindowVerdict {
                    re_arms: true,
                    anchor: AnchorUpdate::Adopt(b),
                }
            }
        }
        // The field went missing. Keep `a`: the window it described is still
        // the best information there is, and dropping it would leave the
        // quota unanchored, so the poll where the timestamp comes back would
        // read as `(None, Some)` instead of the same-window comparison it is.
        (Some(_), None) => WindowVerdict {
            re_arms: false,
            anchor: AnchorUpdate::Keep,
        },
        // The field arrived. Not a boundary — the server merely started
        // reporting one — but it must be recorded, or the quota stays
        // unanchored and the genuine reset that follows goes undetected
        // forever.
        (None, Some(b)) => WindowVerdict {
            re_arms: false,
            anchor: AnchorUpdate::Adopt(b),
        },
    }
}

/// Remembers, per quota, the highest threshold already announced for the
/// current window. Dropping back below a threshold does not re-arm it — only a
/// new window does (spec §10). The window's anchor is the `resets_at` first
/// seen for it and is kept fixed while polls keep landing inside the
/// tolerance, since the server's value wobbles a little on every poll rather
/// than staying fixed (spec §10.1). Whether a poll re-arms and what it does
/// to the anchor are two separate answers; `classify_window` gives both.
///
/// Notifies on transitions this app has observed, not on state it inherited
/// at startup (spec §10.2): the first `evaluate` call for a given quota id
/// primes whatever it was already at as silently announced, rather than
/// firing for it — and every evaluation, first or not, primes from the
/// observed percentage, so the record holds even if notifications were off at
/// the time (`primed_level`). This matters for two reasons — a fresh install should not
/// carpet-bomb someone who was already at 85% with banners for 50 and 80, and
/// on macOS a first-ever launch spends its opening seconds inside the OS's own
/// notification-authorization window, where anything posted is delivered but
/// never actually presented to the user, and silently lost until the window
/// resets (hours to a week later). Priming means nothing is posted on that
/// first poll at all, so there is nothing for the OS to lose.
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
            let crossed = thresholds
                .iter()
                .copied()
                .filter(|t| quota.percent >= f64::from(*t))
                .max();

            match self.state.entry(quota.id.clone()) {
                Entry::Vacant(slot) => {
                    // First sight of this quota id: record where it already
                    // stood as already-announced, but never fire — that state
                    // was inherited, not observed. Recorded from the
                    // percentage rather than from the thresholds, so an
                    // empty threshold list (notifications off) still leaves a
                    // faithful record; see `primed_level`.
                    slot.insert(Armed {
                        highest_fired: primed_level(quota.percent),
                        window: quota.resets_at,
                    });
                }
                Entry::Occupied(mut slot) => {
                    let entry = slot.get_mut();
                    // Two separate answers, applied separately. A poll can
                    // update the anchor without re-arming — that is the whole
                    // point of the split.
                    let verdict = classify_window(entry.window, quota.resets_at);
                    if let AnchorUpdate::Adopt(anchor) = verdict.anchor {
                        entry.window = Some(anchor);
                    }
                    if verdict.re_arms {
                        entry.highest_fired = 0;
                    }

                    if let Some(highest) = crossed {
                        if highest > entry.highest_fired {
                            entry.highest_fired = highest;
                            fired.push(Notification {
                                quota_label: quota.label.clone(),
                                threshold: highest,
                            });
                        }
                    }

                    // Account for what was observed, whether or not a
                    // threshold was crossed. With notifications off the
                    // threshold list is empty and `crossed` is always `None`,
                    // so without this the record would stand still through a
                    // whole muted climb and the first armed poll afterwards
                    // would announce it. `max` keeps it from ever falling
                    // within a window; only a new window zeroes it.
                    entry.highest_fired = entry.highest_fired.max(primed_level(quota.percent));
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
        notifier.evaluate(&[quota(0.0, reset_at(10))], &THRESHOLDS); // prime past the first-sight rule
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
        notifier.evaluate(&[quota(0.0, reset_at(10))], &THRESHOLDS); // prime past the first-sight rule
        let fired = notifier.evaluate(&[quota(50.0, reset_at(10))], &THRESHOLDS);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].threshold, 50);
    }

    #[test]
    fn stays_silent_below_every_threshold() {
        let mut notifier = Notifier::new();
        notifier.evaluate(&[quota(0.0, reset_at(10))], &THRESHOLDS); // prime, so the checked call below is not itself the priming call
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
        notifier.evaluate(&[quota(0.0, reset_at(10))], &THRESHOLDS); // prime past the first-sight rule
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

        // A fourth poll, still in the new window, nothing newly crossed. The
        // backstop for `classify_window`'s reset row adopting `b`: an
        // implementation that re-armed but kept the *stale* anchor would
        // compare this against reset_at(10) again, call it another new
        // window, and re-announce 50 — on every poll forever after the first
        // reset, which is the loudest failure this module can produce.
        assert!(
            notifier
                .evaluate(&[quota(55.0, reset_at(15))], &THRESHOLDS)
                .is_empty(),
            "a new window re-arms once, not on every poll thereafter"
        );
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
        // Prime with the real thresholds first, so the checked call below is
        // testing "an empty list disables notifications", not merely "the
        // first sight of a quota is silent" for an unrelated reason.
        notifier.evaluate(&[quota(0.0, reset_at(10))], &THRESHOLDS);
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

        let mut weekly_priming = quota(0.0, reset_at(10));
        weekly_priming.id = "weekly_all".into();
        notifier.evaluate(&[quota(0.0, reset_at(10)), weekly_priming], &THRESHOLDS); // prime both quotas past the first-sight rule

        let fired = notifier.evaluate(&[quota(55.0, reset_at(10)), weekly], &THRESHOLDS);
        assert_eq!(fired.len(), 2);
    }

    /// A fresh start inherits whatever the user was already at. Firing for
    /// thresholds crossed before the app existed is noise, and on a first-ever
    /// launch it lands inside macOS's authorization window, where notifications
    /// are delivered but never presented — silently lost until the window resets.
    #[test]
    fn the_first_evaluation_primes_without_firing() {
        let mut notifier = Notifier::new();
        assert!(
            notifier
                .evaluate(&[quota(85.0, reset_at(10))], &THRESHOLDS)
                .is_empty(),
            "the first sight of a quota must prime, not fire"
        );
    }

    #[test]
    fn priming_records_what_was_already_crossed() {
        let mut notifier = Notifier::new();
        notifier.evaluate(&[quota(85.0, reset_at(10))], &THRESHOLDS);
        // 80 was already crossed at priming, so re-seeing it is silent...
        assert!(notifier
            .evaluate(&[quota(85.0, reset_at(10))], &THRESHOLDS)
            .is_empty());
        // ...but crossing 90 afterwards is a transition this app observed.
        let fired = notifier.evaluate(&[quota(92.0, reset_at(10))], &THRESHOLDS);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].threshold, 90);
    }

    /// The second door into the priming rule. With notifications off at
    /// launch, `settings::sanitized()` clears the threshold list, so nothing
    /// counted as crossed and priming recorded 0 — and the first poll after
    /// the user turned notifications back on then fired for thresholds that
    /// had been crossed the whole time the app was watching, silently. That
    /// is exactly the state the priming rule exists to refuse to announce.
    /// Priming has to record what was observed, not what was enabled.
    #[test]
    fn thresholds_crossed_while_notifications_were_off_stay_silent_when_they_come_back_on() {
        let mut notifier = Notifier::new();
        // Notifications off at launch: the threshold list arrives empty.
        assert!(notifier
            .evaluate(&[quota(85.0, reset_at(10))], &[])
            .is_empty());

        // Switched on again, same window, same 85%.
        assert!(
            notifier
                .evaluate(&[quota(85.0, reset_at(10))], &THRESHOLDS)
                .is_empty(),
            "50 and 80 were crossed before the switch was flipped — this app \
             watched it happen without announcing it, so turning notifications \
             on must not announce it retroactively"
        );

        // Crossing 90 afterwards is still a transition this app observed, so
        // priming must not have swallowed the whole window either.
        let fired = notifier.evaluate(&[quota(92.0, reset_at(10))], &THRESHOLDS);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].threshold, 90);
    }

    /// The third door into the priming rule, and the one first sight cannot
    /// cover. Priming on the *first* sight of a quota already records the
    /// observed percentage, but every later poll only ever advanced
    /// `highest_fired` from a crossed threshold — and with notifications off
    /// the threshold list is empty, so nothing is ever crossed and the record
    /// froze at whatever the quota happened to stand at when the app started.
    /// A quota that then climbed while muted would announce that climb on the
    /// first poll after the switch came back on.
    ///
    /// The climb has to span two muted polls: a single muted poll is the
    /// first-sight case, which `primed_level` has handled since batch A.
    #[test]
    fn a_climb_across_several_muted_polls_stays_silent_when_notifications_return() {
        let mut notifier = Notifier::new();
        // Notifications off: `settings::sanitized()` hands over an empty list.
        assert!(notifier
            .evaluate(&[quota(40.0, reset_at(10))], &[])
            .is_empty());
        // Still off, and now past 50 and 80. The tray icon and the panel
        // showed this the whole time; only the banners were suppressed.
        assert!(notifier
            .evaluate(&[quota(85.0, reset_at(10))], &[])
            .is_empty());

        assert!(
            notifier
                .evaluate(&[quota(85.0, reset_at(10))], &THRESHOLDS)
                .is_empty(),
            "50 and 80 were crossed while this app watched and stayed \
             deliberately silent, so switching notifications on must prime \
             from what was observed rather than announce it retroactively"
        );

        // ...and the fix cannot be a blanket mute of the poll after the
        // switch: a threshold crossed while armed still has to fire.
        let fired = notifier.evaluate(&[quota(92.0, reset_at(10))], &THRESHOLDS);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].threshold, 90);
    }

    #[test]
    fn a_quota_first_seen_mid_run_also_primes() {
        let mut notifier = Notifier::new();
        notifier.evaluate(&[quota(55.0, reset_at(10))], &THRESHOLDS);
        let mut weekly = quota(95.0, reset_at(10));
        weekly.id = "weekly_all".into();
        weekly.label = "Week · all models".into();
        assert!(
            notifier
                .evaluate(&[quota(58.0, reset_at(10)), weekly], &THRESHOLDS)
                .is_empty(),
            "a newly appearing quota primes on its own first sight"
        );
    }

    /// Priming must not swallow a genuine window reset later on.
    #[test]
    fn a_window_reset_after_priming_still_re_arms() {
        let mut notifier = Notifier::new();
        notifier.evaluate(&[quota(85.0, reset_at(10))], &THRESHOLDS);
        notifier.evaluate(&[quota(5.0, reset_at(15))], &THRESHOLDS);
        let fired = notifier.evaluate(&[quota(55.0, reset_at(15))], &THRESHOLDS);
        assert_eq!(
            fired[0].threshold, 50,
            "a new window re-arms even after priming"
        );
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
    /// eventually trip a fresh re-arm here — twice, across the five
    /// four-minute steps the loop below takes, on the steps that land 8
    /// minutes past the anchor of the moment. Counting the initial crossing
    /// that precedes the loop, three notifications in all. That is the
    /// intended behaviour, not a bug. What a refresh-every-poll bug actually
    /// produces is silence forever instead.)
    #[test]
    fn a_slow_drift_past_the_tolerance_is_eventually_caught() {
        let mut notifier = Notifier::new();
        let t0 = Utc.with_ymd_and_hms(2026, 9, 7, 16, 0, 0).unwrap();
        notifier.evaluate(&[quota(0.0, Some(t0))], &THRESHOLDS); // prime past the first-sight rule

        // Then five more polls, four minutes apart. Every consecutive pair is
        // well inside the 5-minute tolerance, but a step landing 8 minutes
        // past the anchor of the moment exceeds it.
        let first_fired = notifier.evaluate(&[quota(55.0, Some(t0))], &THRESHOLDS);
        assert_eq!(first_fired.len(), 1, "the initial crossing must still fire");

        let mut re_arms = 0;
        for step in 1..6 {
            let t = t0 + chrono::Duration::minutes(4 * step);
            let fired = notifier.evaluate(&[quota(55.0, Some(t))], &THRESHOLDS);
            if !fired.is_empty() {
                re_arms += 1;
            }
        }

        // Asserted rather than described, so the count in the doc comment
        // above is checked rather than merely claimed. Zero is what a
        // refresh-the-anchor-every-poll bug produces.
        assert_eq!(
            re_arms, 2,
            "cumulative drift past the tolerance must re-arm the threshold — \
             twice across these five steps, on the two that land 8 minutes \
             past the anchor of the moment. An implementation that refreshes \
             the anchor to the latest value every poll would stay silent \
             through all five, never noticing the drift"
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

        notifier.evaluate(&[quota(0.0, Some(first))], &THRESHOLDS); // prime past the first-sight rule

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

    /// The tolerance comparison is a strict `<`, so a `resets_at` exactly
    /// `SAME_WINDOW_TOLERANCE_SECS` from the anchor is a *different* window
    /// and re-arms every threshold under the quota's current percentage.
    ///
    /// Both halves fail against opposite mutants. The 299s call fails if the
    /// tolerance shrinks — jitter would start re-firing banners on every
    /// poll, which is the bug the tolerance exists for. The 300s call fails
    /// if the `<` becomes `<=`, or if the tolerance grows. The seconds are
    /// written out rather than derived from `SAME_WINDOW_TOLERANCE_SECS` on
    /// purpose: deriving them would make the test follow a changed constant
    /// instead of pinning it, and the five minutes is itself a judgement
    /// (the shortest real window is five hours, so a genuine reset can never
    /// land this close to the previous one).
    ///
    /// Note that the anchor stays at `t0` for all three calls: a `Some, Some`
    /// pair inside the tolerance answers `AnchorUpdate::Keep`, so the 300s
    /// call is compared against the window's first-seen value and not against
    /// the 299s one.
    #[test]
    fn a_reset_time_exactly_the_tolerance_away_is_a_new_window() {
        let mut notifier = Notifier::new();
        let t0 = Utc.with_ymd_and_hms(2026, 9, 7, 16, 0, 0).unwrap();
        notifier.evaluate(&[quota(0.0, Some(t0))], &THRESHOLDS); // prime past the first-sight rule
        assert_eq!(
            notifier
                .evaluate(&[quota(55.0, Some(t0))], &THRESHOLDS)
                .len(),
            1,
            "the initial crossing must fire"
        );

        assert!(
            notifier
                .evaluate(
                    &[quota(55.0, Some(t0 + chrono::Duration::seconds(299)))],
                    &THRESHOLDS
                )
                .is_empty(),
            "one second inside the tolerance is still the same window"
        );

        let fired = notifier.evaluate(
            &[quota(55.0, Some(t0 + chrono::Duration::seconds(300)))],
            &THRESHOLDS,
        );
        assert_eq!(
            fired.len(),
            1,
            "exactly the tolerance away is a new window — a `<=` here would \
             swallow it and stay silent"
        );
    }

    /// The window table itself, read off `classify_window` so that each of
    /// its two answers is checked apart from the other. Every one of the five
    /// `(stored, incoming)` shapes is asserted on both axes here: `(None,
    /// None)`, `(Some, Some)` inside the tolerance, `(Some, Some)` beyond it,
    /// `(Some, None)` and `(None, Some)`. The behavioural consequences are
    /// pinned in the `evaluate` tests below; this pins the classification.
    ///
    /// The two `Adopt` rows are what the old single-bool shape could not
    /// express. `(None, Some b)` has to adopt without re-arming, and
    /// `(Some a, None)` has to keep without re-arming — one bool driving both
    /// effects forces those two rows to the same answer, and either choice is
    /// wrong for one of them.
    #[test]
    fn classify_window_answers_re_arming_and_anchoring_separately() {
        let a = Utc.with_ymd_and_hms(2026, 9, 7, 16, 0, 0).unwrap();
        let jittered = a + chrono::Duration::seconds(299);
        let next_window = a + chrono::Duration::hours(5);

        assert_eq!(
            classify_window(None, None),
            WindowVerdict {
                re_arms: false,
                anchor: AnchorUpdate::Keep
            }
        );
        assert_eq!(
            classify_window(Some(a), Some(jittered)),
            WindowVerdict {
                re_arms: false,
                anchor: AnchorUpdate::Keep
            },
            "jitter inside the tolerance keeps the first-seen anchor"
        );
        assert_eq!(
            classify_window(Some(a), Some(next_window)),
            WindowVerdict {
                re_arms: true,
                anchor: AnchorUpdate::Adopt(next_window)
            },
            "a genuine reset is the only shape that re-arms"
        );
        assert_eq!(
            classify_window(Some(a), None),
            WindowVerdict {
                re_arms: false,
                anchor: AnchorUpdate::Keep
            },
            "a `resets_at` going missing must not re-arm, and must not \
             discard the anchor it leaves behind"
        );
        assert_eq!(
            classify_window(None, Some(a)),
            WindowVerdict {
                re_arms: false,
                anchor: AnchorUpdate::Adopt(a)
            },
            "a `resets_at` arriving must not re-arm, but must be recorded — \
             keeping `None` here leaves the quota unable to ever detect one"
        );
    }

    /// A `resets_at` that goes missing and comes back is a gap in the
    /// response, not a window boundary. A genuine reset arrives as
    /// `Some → Some` with a delta of hours, which the tolerance already
    /// catches; nothing else can move a real window.
    ///
    /// The shape is not hypothetical: `weekly_scoped` ships
    /// `"resets_at": null` in the live payload today
    /// (tests/fixtures/usage_full.json), so a quota that carries a timestamp
    /// on one poll and null on the next is something this server can
    /// produce. This used to fall to `same_window`'s `_` arm, answer "new
    /// window", zero `highest_fired` and re-announce on that same poll — so a
    /// field that flapped fired on every other poll. Both the disappearance
    /// and the return are checked here, because the old bug needed both to
    /// sustain itself.
    #[test]
    fn a_resets_at_that_flaps_announces_once_not_on_every_other_poll() {
        let mut notifier = Notifier::new();
        let window = reset_at(10);
        notifier.evaluate(&[quota(0.0, window)], &THRESHOLDS); // prime past the first-sight rule

        let fired = notifier.evaluate(&[quota(85.0, window)], &THRESHOLDS);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].threshold, 80, "80 is announced once, normally");

        // Same window, same 85% — but this poll carried no `resets_at`.
        assert!(
            notifier
                .evaluate(&[quota(85.0, None)], &THRESHOLDS)
                .is_empty(),
            "a `resets_at` going missing is a gap in the response, not a \
             window boundary — re-announcing 80 here is a banner the user \
             has already seen for a window that never reset"
        );

        // And back again, at the very timestamp it left.
        assert!(
            notifier
                .evaluate(&[quota(85.0, window)], &THRESHOLDS)
                .is_empty(),
            "the timestamp returning is not a window boundary either"
        );
    }

    /// That the `None` poll above stayed quiet does not by itself prove the
    /// anchor survived it: had the gap overwritten the anchor with nothing,
    /// the return trip would be `(None, Some)`, which does not re-arm either,
    /// and the flap test would pass regardless. `AnchorUpdate::Keep` on the
    /// `(Some a, None)` row only becomes observable through `evaluate` when a
    /// *genuine* reset follows the gap.
    ///
    /// So: a window, a poll with no `resets_at` at all, then a real boundary
    /// five hours on — the shortest real window there is. That must re-arm,
    /// which it can only do by comparing against the anchor the gap left
    /// alone. An implementation that dropped the anchor during the gap sees
    /// `(None, Some)` here, adopts without re-arming, and stays silent.
    #[test]
    fn an_anchor_survives_a_gap_in_resets_at() {
        let mut notifier = Notifier::new();
        let first = Utc.with_ymd_and_hms(2026, 9, 7, 10, 0, 0).unwrap();

        notifier.evaluate(&[quota(0.0, Some(first))], &THRESHOLDS); // prime past the first-sight rule
        assert_eq!(
            notifier
                .evaluate(&[quota(85.0, Some(first))], &THRESHOLDS)
                .len(),
            1,
            "80 is announced once, normally"
        );

        // The gap.
        assert!(notifier
            .evaluate(&[quota(85.0, None)], &THRESHOLDS)
            .is_empty());

        // A real reset, measured from the anchor the gap must not have eaten.
        let fired = notifier.evaluate(
            &[quota(85.0, Some(first + chrono::Duration::hours(5)))],
            &THRESHOLDS,
        );
        assert_eq!(
            fired.len(),
            1,
            "the gap must have kept the anchor — dropping it makes this \
             `(None, Some)`, which adopts without re-arming and never fires"
        );
        assert_eq!(fired[0].threshold, 80);
    }

    /// The `(None, Some)` row, and the reason it adopts rather than keeps.
    /// A quota that has never carried a `resets_at` has no anchor; the poll
    /// where one finally arrives must stay silent — nothing about the window
    /// changed, the server just started reporting it — but it must *record*
    /// that timestamp. Keeping `None` instead would be silent too, and would
    /// look correct right up until the genuine reset in the last step, which
    /// would then compare `(None, Some)` all over again and never re-arm:
    /// the quota would go permanently deaf.
    #[test]
    fn a_resets_at_that_arrives_late_anchors_without_re_arming() {
        let mut notifier = Notifier::new();
        let arrived = Utc.with_ymd_and_hms(2026, 9, 7, 16, 0, 0).unwrap();

        // First sight, and a second poll, with no reset time at all.
        notifier.evaluate(&[quota(85.0, None)], &THRESHOLDS); // primes at 85
        assert!(notifier
            .evaluate(&[quota(85.0, None)], &THRESHOLDS)
            .is_empty());

        assert!(
            notifier
                .evaluate(&[quota(85.0, Some(arrived))], &THRESHOLDS)
                .is_empty(),
            "the server starting to report a reset time is not a reset"
        );

        // Five hours on, a real window boundary — the shortest real window
        // there is, so this is the closest a genuine reset can ever land.
        let fired = notifier.evaluate(
            &[quota(85.0, Some(arrived + chrono::Duration::hours(5)))],
            &THRESHOLDS,
        );
        assert_eq!(
            fired.len(),
            1,
            "the late arrival had to be adopted as the anchor, or this \
             comparison is `(None, Some)` again and the quota never re-arms"
        );
        assert_eq!(fired[0].threshold, 80);
    }

    #[test]
    fn a_quota_with_no_reset_time_does_not_re_arm() {
        let mut notifier = Notifier::new();
        notifier.evaluate(&[quota(0.0, None)], &THRESHOLDS); // prime past the first-sight rule
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

        let mut weekly_priming = quota(0.0, reset_at(10));
        weekly_priming.id = "weekly_all".into();
        notifier.evaluate(&[quota(0.0, reset_at(10)), weekly_priming], &THRESHOLDS); // prime both quotas past the first-sight rule

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

        let mut weekly_priming = quota(0.0, reset_at(10));
        weekly_priming.id = "weekly_all".into();
        notifier.evaluate(&[quota(0.0, reset_at(10)), weekly_priming], &THRESHOLDS); // prime both quotas past the first-sight rule

        // First real call: session arms at 50, weekly arms at 90.
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
