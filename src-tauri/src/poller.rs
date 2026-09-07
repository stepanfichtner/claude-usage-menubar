use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::error::ApiError;
use crate::model::Quota;

const BACKOFF_STEPS: [u64; 3] = [2 * 60, 5 * 60, 15 * 60];

/// Spec §7. The limit is burst-sensitive rather than rate-sensitive — three
/// requests four seconds apart pass, four inside ten seconds do not — so this
/// exists to stop a burst forming. Twenty seconds was short enough to let one
/// form from panel opens alone.
pub const MIN_MANUAL_REFRESH_SECS: i64 = 60;

#[derive(Debug, Default)]
pub struct RefreshThrottle {
    last: Option<DateTime<Utc>>,
}

impl RefreshThrottle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether the request is let through, recording it if so. A refused
    /// request does not push the window out, so a burst of clicks still lets one
    /// refresh through every `MIN_MANUAL_REFRESH_SECS` seconds rather than none.
    pub fn allow(&mut self, now: DateTime<Utc>) -> bool {
        let allowed = match self.last {
            None => true,
            Some(last) => (now - last).num_seconds() >= MIN_MANUAL_REFRESH_SECS,
        };
        if allowed {
            self.last = Some(now);
        }
        allowed
    }
}

/// Rate-limit backoff. Pure state — no clock, no timers.
#[derive(Debug, Clone, Copy, Default)]
pub struct Backoff {
    consecutive_rate_limits: usize,
}

impl Backoff {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_rate_limited(&mut self) {
        self.consecutive_rate_limits += 1;
    }

    pub fn on_success(&mut self) {
        self.consecutive_rate_limits = 0;
    }

    pub fn next_delay(&self, base: Duration) -> Duration {
        match self.consecutive_rate_limits {
            0 => base,
            n => Duration::from_secs(BACKOFF_STEPS[(n - 1).min(BACKOFF_STEPS.len() - 1)]),
        }
    }

    /// True while the last poll was rate-limited and the escalated wait has not
    /// yet been served.
    pub fn is_backing_off(&self) -> bool {
        self.consecutive_rate_limits > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthState {
    Ok,
    SignedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Publish the new quotas to the tray, the UI and the notifier.
    Publish,
    /// Re-read the credential and try once more immediately.
    RetryAuthOnce,
    /// Keep the previous snapshot and sleep for `next_delay`.
    Wait,
}

/// The whole scheduling policy, as one pure function.
pub fn decide(
    result: &Result<Vec<Quota>, ApiError>,
    backoff: &mut Backoff,
    auth: &mut AuthState,
) -> Decision {
    match result {
        Ok(_) => {
            backoff.on_success();
            *auth = AuthState::Ok;
            Decision::Publish
        }
        Err(ApiError::RateLimited) => {
            backoff.on_rate_limited();
            Decision::Wait
        }
        Err(ApiError::SignedOut) => match auth {
            // First failure: the token probably just rotated under us.
            AuthState::Ok => {
                *auth = AuthState::SignedOut;
                Decision::RetryAuthOnce
            }
            // Still failing after a fresh read: really signed out.
            AuthState::SignedOut => Decision::Wait,
        },
        Err(_) => Decision::Wait,
    }
}

use std::sync::Arc;

use chrono::Duration as ChronoDuration;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Notify;

use crate::model::{Profile, UsageSnapshot};
use crate::notifier::Notifier;
use crate::{cache, credentials, profile as profile_api, usage};

pub const PROFILE_MAX_AGE_HOURS: i64 = 24;

#[derive(Debug, Clone)]
pub struct PollConfig {
    pub base_url: String,
}

impl Default for PollConfig {
    fn default() -> Self {
        Self {
            base_url: usage::API_BASE.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotEvent {
    pub snapshot: UsageSnapshot,
    pub profile: Option<Profile>,
    pub signed_out: bool,
}

/// Signals an out-of-band refresh (menu item, refresh button, popover opening),
/// gated by the `MIN_MANUAL_REFRESH_SECS` throttle above.
#[derive(Default)]
pub struct RefreshSignal {
    notify: Notify,
    throttle: std::sync::Mutex<RefreshThrottle>,
}

impl RefreshSignal {
    /// Request a refresh. Returns whether it was let through.
    pub fn request(&self) -> bool {
        let allowed = self
            .throttle
            .lock()
            .map(|mut throttle| throttle.allow(Utc::now()))
            .unwrap_or(false);
        if allowed {
            self.notify.notify_one();
        }
        allowed
    }

    pub async fn notified(&self) {
        self.notify.notified().await;
    }
}

pub fn spawn(app: AppHandle, config: PollConfig) {
    tauri::async_runtime::spawn(async move {
        let client = reqwest::Client::new();
        let mut backoff = Backoff::new();
        let mut auth = AuthState::Ok;
        let mut notifier = Notifier::new();
        let cache_dir = app
            .path()
            .app_cache_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."));

        // Serve the cached snapshot immediately so the UI is never empty.
        // `cache::load_snapshot` always marks it `stale: true` — it has not
        // been confirmed by a live fetch yet this session, which is the one
        // genuine meaning of the flag. A live fetch below always publishes
        // with `stale: false`, and nothing in this loop sets it back to true
        // afterwards.
        let mut profile = cache::load_profile(&cache_dir).map(|(p, _)| p);
        let mut profile_fetched_at = cache::load_profile(&cache_dir).map(|(_, at)| at);
        if let Some(cached) = cache::load_snapshot(&cache_dir) {
            crate::tray::apply(&app, &cached);
            emit(&app, &cached, &profile, false);
        }

        let signal = app.state::<Arc<RefreshSignal>>().inner().clone();

        loop {
            let token = credentials::read_token().ok();
            let result = match &token {
                Some(token) => usage::fetch_usage(&client, &config.base_url, token).await,
                None => Err(crate::error::ApiError::SignedOut),
            };

            // The plan changes a few times a year at most (spec §4.5). This
            // runs whenever a token was readable, independent of whether the
            // usage poll itself succeeded: a signed-out, offline or
            // rate-limited usage poll must not also starve the profile of the
            // one thing it needs, or the panel header sits on the "Claude
            // usage" fallback until a usage poll finally succeeds — which
            // under a sustained backoff can be a long wait. A failed profile
            // fetch stays non-fatal: nothing here changes what happens next.
            let stale_profile = profile_fetched_at
                .map(|at| Utc::now() - at > ChronoDuration::hours(PROFILE_MAX_AGE_HOURS))
                .unwrap_or(true);
            if stale_profile {
                if let Some(token) = &token {
                    if let Ok(fresh) =
                        profile_api::fetch_profile(&client, &config.base_url, token).await
                    {
                        let _ = cache::save_profile(&cache_dir, &fresh);
                        profile_fetched_at = Some(Utc::now());
                        profile = Some(fresh);
                    }
                }
            }

            match decide(&result, &mut backoff, &mut auth) {
                Decision::RetryAuthOnce => continue,
                Decision::Publish => {
                    let quotas = result.unwrap_or_default();
                    let snapshot = UsageSnapshot {
                        quotas,
                        fetched_at: Utc::now(),
                        stale: false,
                    };
                    let _ = cache::save_snapshot(&cache_dir, &snapshot);

                    crate::tray::apply(&app, &snapshot);

                    let settings = crate::settings::load(&app);
                    for notification in notifier.evaluate(&snapshot.quotas, &settings.thresholds) {
                        let _ = app
                            .notification()
                            .builder()
                            .title(format!(
                                "{} at {}%",
                                notification.quota_label, notification.threshold
                            ))
                            .body("Claude usage limit approaching")
                            .show();
                    }

                    emit(&app, &snapshot, &profile, false);
                }
                Decision::Wait => {
                    // A failed poll (rate limited, network error) re-emits
                    // nothing: the frontend ticks its own age clock from the
                    // last snapshot it has, so there is nothing new to say.
                    // Re-emitting here previously flipped a perfectly fresh
                    // snapshot's `stale` flag to true on every rate-limited
                    // retry — including the one the panel's own open-triggered
                    // refresh causes — which is not what `stale` is for.
                    if auth == AuthState::SignedOut {
                        let empty = UsageSnapshot {
                            quotas: Vec::new(),
                            fetched_at: Utc::now(),
                            stale: false,
                        };
                        crate::tray::apply(&app, &empty);
                        emit(&app, &empty, &profile, true);
                    }
                }
            }

            let interval = Duration::from_secs(crate::settings::load(&app).poll_interval_secs);
            let deadline = tokio::time::Instant::now() + backoff.next_delay(interval);
            wait_for_next_poll(deadline, &signal, &backoff).await;
        }
    });
}

/// Waits until `deadline`, unless a manual refresh (menu item, refresh
/// button, popover opening) arrives first — except while `backoff` is active.
/// A backoff exists to buy the endpoint a quiet period; `toggle_popover`
/// fires a refresh on every panel open, so a worried user checking the panel
/// repeatedly while rate-limited must not be able to redeliver a request every
/// time, or the quiet period never happens and the backoff can never
/// escalate to where it would help. Outside a backoff, a refresh still cuts
/// the wait short as before.
///
/// `deadline` is a fixed `Instant`, computed once by the caller, so re-running
/// `sleep_until` on every loop iteration below still targets the same point
/// in time rather than restarting the wait. A refresh signal that arrived
/// before this function was even called — while the previous poll was still
/// in flight — is handled the same way: `Notify` delivers at most one stored
/// wake per call to `notified()`, so the first iteration below consumes it,
/// checks the (already-decided) backoff state, and either honours it or
/// discards it and re-arms `notified()` for a genuinely new signal.
async fn wait_for_next_poll(
    deadline: tokio::time::Instant,
    signal: &RefreshSignal,
    backoff: &Backoff,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            _ = signal.notified() => {
                if backoff.is_backing_off() {
                    continue;
                }
                break;
            }
        }
    }
}

fn emit(app: &AppHandle, snapshot: &UsageSnapshot, profile: &Option<Profile>, signed_out: bool) {
    let _ = app.emit(
        "usage://snapshot",
        SnapshotEvent {
            snapshot: snapshot.clone(),
            profile: profile.clone(),
            signed_out,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ApiError;
    use std::time::Duration;

    const BASE: Duration = Duration::from_secs(60);

    #[test]
    fn normal_operation_uses_the_base_interval() {
        let backoff = Backoff::new();
        assert_eq!(backoff.next_delay(BASE), BASE);
    }

    #[test]
    fn rate_limiting_escalates_2_5_15_and_caps() {
        let mut backoff = Backoff::new();
        backoff.on_rate_limited();
        assert_eq!(backoff.next_delay(BASE), Duration::from_secs(2 * 60));
        backoff.on_rate_limited();
        assert_eq!(backoff.next_delay(BASE), Duration::from_secs(5 * 60));
        backoff.on_rate_limited();
        assert_eq!(backoff.next_delay(BASE), Duration::from_secs(15 * 60));
        backoff.on_rate_limited();
        assert_eq!(backoff.next_delay(BASE), Duration::from_secs(15 * 60));
    }

    #[test]
    fn a_success_clears_the_backoff() {
        let mut backoff = Backoff::new();
        backoff.on_rate_limited();
        backoff.on_rate_limited();
        backoff.on_success();
        assert_eq!(backoff.next_delay(BASE), BASE);
    }

    /// Pinned across the full escalate/reset cycle, not just the endpoints:
    /// a fresh `Backoff` is not active, one rate limit starts it, a second
    /// consecutive one leaves it active rather than toggling it, and a
    /// success clears it. `wait_for_next_poll` below trusts this predicate
    /// with the decision of whether a refresh may cut its wait short.
    #[test]
    fn is_backing_off_reflects_the_escalate_reset_cycle() {
        let mut backoff = Backoff::new();
        assert!(!backoff.is_backing_off(), "a fresh backoff is not active");
        backoff.on_rate_limited();
        assert!(backoff.is_backing_off(), "one rate limit starts it");
        backoff.on_rate_limited();
        assert!(backoff.is_backing_off(), "still active while escalating");
        backoff.on_success();
        assert!(!backoff.is_backing_off(), "a success clears it");
    }

    #[test]
    fn a_success_publishes_and_clears_backoff() {
        let mut backoff = Backoff::new();
        let mut auth = AuthState::Ok;
        backoff.on_rate_limited();
        let decision = decide(&Ok(vec![]), &mut backoff, &mut auth);
        assert_eq!(decision, Decision::Publish);
        assert_eq!(backoff.next_delay(BASE), BASE);
        assert_eq!(auth, AuthState::Ok);
    }

    #[test]
    fn the_first_401_asks_for_one_credential_reread() {
        let mut backoff = Backoff::new();
        let mut auth = AuthState::Ok;
        let decision = decide(&Err(ApiError::SignedOut), &mut backoff, &mut auth);
        assert_eq!(decision, Decision::RetryAuthOnce);
    }

    #[test]
    fn a_second_consecutive_401_enters_signed_out() {
        let mut backoff = Backoff::new();
        let mut auth = AuthState::Ok;
        decide(&Err(ApiError::SignedOut), &mut backoff, &mut auth);
        let decision = decide(&Err(ApiError::SignedOut), &mut backoff, &mut auth);
        assert_eq!(decision, Decision::Wait);
        assert_eq!(auth, AuthState::SignedOut);
    }

    #[test]
    fn a_success_after_signing_back_in_clears_the_signed_out_state() {
        let mut backoff = Backoff::new();
        let mut auth = AuthState::SignedOut;
        decide(&Ok(vec![]), &mut backoff, &mut auth);
        assert_eq!(auth, AuthState::Ok);
    }

    #[test]
    fn a_429_backs_off_without_touching_auth() {
        let mut backoff = Backoff::new();
        let mut auth = AuthState::Ok;
        let decision = decide(&Err(ApiError::RateLimited), &mut backoff, &mut auth);
        assert_eq!(decision, Decision::Wait);
        assert_eq!(backoff.next_delay(BASE), Duration::from_secs(2 * 60));
        assert_eq!(auth, AuthState::Ok);
    }

    fn at(seconds: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now() + chrono::Duration::seconds(seconds)
    }

    #[test]
    fn the_first_manual_refresh_is_allowed() {
        let mut throttle = RefreshThrottle::new();
        assert!(throttle.allow(at(0)));
    }

    #[test]
    fn a_second_manual_refresh_before_the_throttle_window_elapses_is_refused() {
        let mut throttle = RefreshThrottle::new();
        throttle.allow(at(0));
        assert!(!throttle.allow(at(MIN_MANUAL_REFRESH_SECS - 1)));
    }

    #[test]
    fn a_manual_refresh_after_the_throttle_window_elapses_is_allowed() {
        let mut throttle = RefreshThrottle::new();
        throttle.allow(at(0));
        assert!(throttle.allow(at(MIN_MANUAL_REFRESH_SECS)));
    }

    #[test]
    fn a_refused_refresh_does_not_extend_the_window() {
        let mut throttle = RefreshThrottle::new();
        throttle.allow(at(0));
        assert!(!throttle.allow(at(MIN_MANUAL_REFRESH_SECS / 2)));
        assert!(
            throttle.allow(at(MIN_MANUAL_REFRESH_SECS)),
            "the refusal must not reset the clock"
        );
    }

    /// These two constants are measured, not chosen, and the measurement is the
    /// only thing that justifies them: against the live endpoint, four requests
    /// inside ten seconds returned 429, three requests four seconds apart did
    /// not, and recovery from a 429 took 111 seconds.
    ///
    /// The limit is therefore burst-sensitive rather than rate-sensitive, which
    /// is why the throttle exists at all — 20 seconds was short enough to let a
    /// burst form from panel opens alone. And why the backoff starts at two
    /// minutes rather than five: the condition it answers lasts about two.
    ///
    /// The other throttle tests are deliberately symbolic, so this is the only
    /// place the absolute values are pinned. Changing either constant should
    /// mean re-measuring, not editing this test.
    #[test]
    fn the_measured_rate_limit_values_are_what_ship() {
        assert_eq!(MIN_MANUAL_REFRESH_SECS, 60);
        assert_eq!(BACKOFF_STEPS, [2 * 60, 5 * 60, 15 * 60]);
    }

    #[test]
    fn a_network_error_waits_at_the_base_interval() {
        let mut backoff = Backoff::new();
        let mut auth = AuthState::Ok;
        let decision = decide(
            &Err(ApiError::Network("offline".into())),
            &mut backoff,
            &mut auth,
        );
        assert_eq!(decision, Decision::Wait);
        assert_eq!(backoff.next_delay(BASE), BASE);
    }

    // `wait_for_next_poll` has no test-util feature available to it (the
    // `tokio` dependency is frozen without `test-util`, so `time::pause`/
    // `time::advance` are not options here): these two run on the real clock
    // with short, wide-margin durations rather than a deterministic virtual
    // one. They pin direction and rough magnitude — "did not shorten" versus
    // "shortened by roughly this much" — not exact timing, which a loaded CI
    // runner could never guarantee. What they cannot rule out is a shortening
    // small enough to still pass the margin, or that `RefreshSignal`'s
    // observable behavior under real concurrent access (task scheduling
    // fairness, executor jitter) matches what a single-threaded reading of
    // the code suggests it should.

    /// A refresh landing partway through an active backoff must not shorten
    /// the wait at all: the full deadline still has to elapse.
    #[tokio::test]
    async fn a_refresh_during_backoff_does_not_shorten_the_wait() {
        let mut backoff = Backoff::new();
        backoff.on_rate_limited();
        let signal = RefreshSignal::default();
        let wait = std::time::Duration::from_millis(250);
        let deadline = tokio::time::Instant::now() + wait;

        let started = std::time::Instant::now();
        tokio::join!(
            async {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                assert!(
                    signal.request(),
                    "the first-ever refresh is never throttled"
                );
            },
            wait_for_next_poll(deadline, &signal, &backoff),
        );
        let elapsed = started.elapsed();

        assert!(
            elapsed >= std::time::Duration::from_millis(230),
            "a refresh during an active backoff must not cut the wait short, elapsed = {elapsed:?}"
        );
    }

    /// The same refresh, outside a backoff, must cut an ordinary wait short
    /// rather than making it run to the full deadline.
    #[tokio::test]
    async fn a_refresh_outside_backoff_cuts_the_wait_short() {
        let backoff = Backoff::new();
        let signal = RefreshSignal::default();
        let wait = std::time::Duration::from_millis(250);
        let deadline = tokio::time::Instant::now() + wait;

        let started = std::time::Instant::now();
        tokio::join!(
            async {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                assert!(
                    signal.request(),
                    "the first-ever refresh is never throttled"
                );
            },
            wait_for_next_poll(deadline, &signal, &backoff),
        );
        let elapsed = started.elapsed();

        assert!(
            elapsed < std::time::Duration::from_millis(125),
            "a refresh outside a backoff should cut the wait well short of the deadline, elapsed = {elapsed:?}"
        );
    }
}
