use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::error::ApiError;
use crate::model::Quota;

const BACKOFF_STEPS: [u64; 3] = [5 * 60, 15 * 60, 30 * 60];

/// Spec §7: a manual refresh — the menu item, the popover's button, or the
/// popover simply being opened — is allowed at most once every 20 seconds.
pub const MIN_MANUAL_REFRESH_SECS: i64 = 20;

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
    /// refresh through every 20 seconds rather than none.
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
/// gated by the 20-second throttle above.
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
        let mut profile = cache::load_profile(&cache_dir).map(|(p, _)| p);
        let mut profile_fetched_at = cache::load_profile(&cache_dir).map(|(_, at)| at);
        if let Some(cached) = cache::load_snapshot(&cache_dir) {
            crate::tray::apply(&app, &cached);
            emit(&app, &cached, &profile, false);
        }

        let signal = app.state::<Arc<RefreshSignal>>().inner().clone();
        let mut last_snapshot: Option<UsageSnapshot> = None;

        loop {
            let token = credentials::read_token().ok();
            let result = match &token {
                Some(token) => usage::fetch_usage(&client, &config.base_url, token).await,
                None => Err(crate::error::ApiError::SignedOut),
            };

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

                    // The plan changes a few times a year at most (spec §4.5).
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
                    last_snapshot = Some(snapshot);
                }
                Decision::Wait => {
                    if auth == AuthState::SignedOut {
                        let empty = UsageSnapshot {
                            quotas: Vec::new(),
                            fetched_at: Utc::now(),
                            stale: false,
                        };
                        crate::tray::apply(&app, &empty);
                        emit(&app, &empty, &profile, true);
                    } else if let Some(previous) = &last_snapshot {
                        // Spec §7: keep showing the last figures, but say they
                        // are no longer fresh.
                        let stale = UsageSnapshot {
                            stale: true,
                            ..previous.clone()
                        };
                        emit(&app, &stale, &profile, false);
                    }
                }
            }

            let interval = Duration::from_secs(crate::settings::load(&app).poll_interval_secs);
            let delay = backoff.next_delay(interval);
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = signal.notified() => {}
            }
        }
    });
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
    fn rate_limiting_escalates_5_15_30_and_caps() {
        let mut backoff = Backoff::new();
        backoff.on_rate_limited();
        assert_eq!(backoff.next_delay(BASE), Duration::from_secs(5 * 60));
        backoff.on_rate_limited();
        assert_eq!(backoff.next_delay(BASE), Duration::from_secs(15 * 60));
        backoff.on_rate_limited();
        assert_eq!(backoff.next_delay(BASE), Duration::from_secs(30 * 60));
        backoff.on_rate_limited();
        assert_eq!(backoff.next_delay(BASE), Duration::from_secs(30 * 60));
    }

    #[test]
    fn a_success_clears_the_backoff() {
        let mut backoff = Backoff::new();
        backoff.on_rate_limited();
        backoff.on_rate_limited();
        backoff.on_success();
        assert_eq!(backoff.next_delay(BASE), BASE);
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
        assert_eq!(backoff.next_delay(BASE), Duration::from_secs(5 * 60));
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
    fn a_second_manual_refresh_within_twenty_seconds_is_refused() {
        let mut throttle = RefreshThrottle::new();
        throttle.allow(at(0));
        assert!(!throttle.allow(at(19)));
    }

    #[test]
    fn a_manual_refresh_after_twenty_seconds_is_allowed() {
        let mut throttle = RefreshThrottle::new();
        throttle.allow(at(0));
        assert!(throttle.allow(at(20)));
    }

    #[test]
    fn a_refused_refresh_does_not_extend_the_window() {
        let mut throttle = RefreshThrottle::new();
        throttle.allow(at(0));
        assert!(!throttle.allow(at(10)));
        assert!(
            throttle.allow(at(20)),
            "the refusal must not reset the clock"
        );
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
}
