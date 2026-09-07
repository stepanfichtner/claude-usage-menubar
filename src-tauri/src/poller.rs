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
