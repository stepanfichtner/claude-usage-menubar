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

/// Whether the profile is due for a re-fetch this cycle: stale (or never
/// fetched) by `PROFILE_MAX_AGE_HOURS`, and — R55 — not while `backing_off`
/// is true. The endpoint is asking for quiet during a backoff, and a second
/// request per cycle is the opposite of that; the plan changes a few times a
/// year at most (spec §4.5), so waiting out the backoff costs nothing here.
fn profile_is_due(
    backing_off: bool,
    fetched_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    !backing_off
        && fetched_at
            .map(|at| now - at > ChronoDuration::hours(PROFILE_MAX_AGE_HOURS))
            .unwrap_or(true)
}

/// R56: whether this iteration should attempt a profile fetch, composing
/// this iteration's own usage-poll `result` with the entering `backoff`
/// state and with whether this iteration is the immediate re-auth retry.
/// At the call site, `decide()` has not run yet for this iteration's
/// `result` — only `backoff`'s bookkeeping lags, not the outcome itself,
/// which is already known. Consulting `backoff.is_backing_off()` alone was
/// off by one iteration: it missed a rate limit that started on *this* very
/// poll, since `on_rate_limited()` would not fold it in until `decide()` ran
/// afterwards. Reading `result` directly closes that gap.
///
/// `retry_not_before` is R57's term: the hold `profile_retry_after` puts on
/// the endpoint after the *profile* fetch itself fails.
fn should_fetch_profile(
    result: &Result<Vec<Quota>, ApiError>,
    backoff: &Backoff,
    retrying_auth: bool,
    fetched_at: Option<DateTime<Utc>>,
    retry_not_before: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    // The retry pass: the one `Decision::RetryAuthOnce` sent straight back
    // round with no wait at all. The re-read of the credential is deliberate and stays;
    // the profile fetch riding along with it is not — `profile_is_due`
    // answers "due" on both passes whenever no profile has ever been cached,
    // so a token that reads but 401s emitted four requests in about a second.
    // Suppressing it here makes the retry cost one request rather than two.
    if retrying_auth {
        return false;
    }
    // R57. Nothing else here can see a failed profile fetch: a failure
    // leaves `fetched_at` exactly as stale as it was and touches neither
    // `result` nor `backoff`, both of which are the *usage* poll's. So
    // staleness alone re-asked on every single poll for as long as the
    // profile endpoint kept failing — sixty requests an hour at the interval
    // floor, at an endpoint already known to rate-limit.
    if retry_not_before.is_some_and(|at| now < at) {
        return false;
    }
    let rate_limited_now = matches!(result, Err(ApiError::RateLimited));
    profile_is_due(
        rate_limited_now || backoff.is_backing_off(),
        fetched_at,
        now,
    )
}

/// When the profile endpoint may be asked again after this attempt: `None`
/// once it answers, otherwise a point far enough out to stop the refetch
/// loop above.
///
/// The escalation is `Backoff`'s, not a second mechanism — the same
/// `BACKOFF_STEPS` the usage poll walks (2, 5, then 15 minutes), advanced by
/// a failure and cleared by a success. `next_delay`'s `base` argument is
/// what it answers with after *zero* failures, and this is only ever
/// consulted after one, so the zero passed for it is never the value that
/// comes back.
fn profile_retry_after(
    backoff: &mut Backoff,
    succeeded: bool,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if succeeded {
        backoff.on_success();
        return None;
    }
    backoff.on_rate_limited();
    let delay = backoff.next_delay(Duration::ZERO).as_secs() as i64;
    Some(now + ChronoDuration::seconds(delay))
}

/// Stamps every snapshot this poller emits with a `fetched_at` distinct
/// from the one immediately before it.
///
/// R58, and a cross-language invariant: the settings window counts each
/// snapshot exactly once when deciding whether a title entry's quota has
/// been absent long enough to retire, and it identifies snapshots by
/// `fetchedAt` alone (`titleEntries.ts`'s `reconcileTitleEntries`). Two
/// emits sharing a stamp are therefore counted as one — the absence tally
/// stops advancing and a genuinely retired quota's row never goes away.
/// `Utc::now()` on its own left that resting on the platform clock's
/// resolution rather than on anything in this codebase; this makes it true
/// by construction instead.
///
/// *Immediately* before is the whole contract, deliberately, and this does
/// **not** promise a globally increasing sequence. `reconcileTitleEntries`
/// compares one field — `standing.fetchedAt === state.countedAt` — and
/// `countedAt` is overwritten on every emit that differs, so the only stamp
/// a new one can be confused with is its predecessor. Buying monotonicity on
/// top would cost something real: after a clock stepped backwards by an
/// hour, holding the sequence increasing means emitting a `fetched_at` up to
/// an hour ahead of the wall clock for an hour, and `freshness.ts`'s
/// `secondsSince` clamps a future stamp to zero — so the footer would read
/// "updated just now" and `ageIsStale` would never fire while the numbers
/// really were going stale. Every stamp here is a genuine clock reading
/// except where one would exactly repeat its predecessor.
#[derive(Debug, Default)]
struct SnapshotClock {
    last: Option<DateTime<Utc>>,
}

impl SnapshotClock {
    /// Seeded from the cached snapshot replayed at launch, when there is
    /// one. That replay is an emit like any other as far as the frontend is
    /// concerned, and its stamp comes from a previous process rather than
    /// from this clock, so the first live snapshot has to be distinct from
    /// it too.
    fn after(previous: Option<DateTime<Utc>>) -> Self {
        Self { last: previous }
    }

    /// One live snapshot, stamped `now` — or one nanosecond past the
    /// previous stamp in the one case where `now` would repeat it exactly.
    fn snapshot(&mut self, quotas: Vec<Quota>, now: DateTime<Utc>) -> UsageSnapshot {
        let fetched_at = match self.last {
            Some(last) if now == last => last + ChronoDuration::nanoseconds(1),
            _ => now,
        };
        self.last = Some(fetched_at);
        UsageSnapshot {
            quotas,
            fetched_at,
            stale: false,
        }
    }
}

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
        let mut profile_backoff = Backoff::new();
        let mut profile_retry_at: Option<DateTime<Utc>> = None;
        let mut auth = AuthState::Ok;
        let mut retrying_auth = false;
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
        //
        // Both files are read in one `spawn_blocking`, and the profile once
        // rather than twice for the two halves of its tuple. These are
        // `std::fs` reads on a task that shares its runtime with the
        // analytics scan and every other async worker.
        let dir = cache_dir.clone();
        let (cached_profile, cached_snapshot) = tauri::async_runtime::spawn_blocking(move || {
            (cache::load_profile(&dir), cache::load_snapshot(&dir))
        })
        .await
        .unwrap_or((None, None));
        let (mut profile, mut profile_fetched_at) = cached_profile.unzip();
        let mut clock = SnapshotClock::after(cached_snapshot.as_ref().map(|s| s.fetched_at));
        if let Some(cached) = cached_snapshot {
            crate::tray::apply(&app, &cached);
            emit(&app, &cached, &profile, false);
        }

        let signal = app.state::<Arc<RefreshSignal>>().inner().clone();

        loop {
            // On macOS this shells out to `/usr/bin/security`, which can put
            // a modal Keychain ACL prompt on screen and then wait for it —
            // holding an async worker for as long as the dialog sits there
            // unanswered. The blocking pool is where that belongs.
            let token = tauri::async_runtime::spawn_blocking(credentials::read_token)
                .await
                .ok()
                .and_then(|token| token.ok());
            let result = match &token {
                Some(token) => usage::fetch_usage(&client, &config.base_url, token).await,
                None => Err(crate::error::ApiError::SignedOut),
            };

            // This runs whenever a token was readable, independent of
            // whether the usage poll itself succeeded: a signed-out or
            // offline usage poll must not also starve the profile of the one
            // thing it needs, or the panel header sits on the "Claude usage"
            // fallback until a usage poll finally succeeds. A failed profile
            // fetch stays non-fatal: nothing here changes what happens next.
            //
            // R55/R56: not while rate-limited, though — the endpoint is
            // asking for quiet, and a second request per cycle is the
            // opposite of that. `result` (this iteration's usage-poll
            // outcome) is already known at this point, eighteen lines up;
            // what is *not* yet updated is `backoff`'s bookkeeping — decide()
            // folds `result` into it only below. Gating on `backoff` alone
            // would miss a rate limit that started on this very poll, so
            // `should_fetch_profile` consults both: this iteration's own
            // result, and whatever `backoff` was still carrying in from
            // before it — and whether this pass is the immediate re-auth
            // retry, which `Decision::RetryAuthOnce` reaches with no wait in
            // between. That retry is one deliberate extra usage request; it
            // must not also drag a profile fetch along, or a token that reads
            // but 401s emits four requests inside a second.
            let stale_profile = should_fetch_profile(
                &result,
                &backoff,
                retrying_auth,
                profile_fetched_at,
                profile_retry_at,
                Utc::now(),
            );
            if stale_profile {
                if let Some(token) = &token {
                    let fetched =
                        profile_api::fetch_profile(&client, &config.base_url, token).await;
                    // R57: a failure holds the endpoint off rather than
                    // letting the next poll ask again immediately.
                    profile_retry_at =
                        profile_retry_after(&mut profile_backoff, fetched.is_ok(), Utc::now());
                    if let Ok(fresh) = fetched {
                        save_profile(&cache_dir, &fresh).await;
                        profile_fetched_at = Some(Utc::now());
                        profile = Some(fresh);
                    }
                }
            }

            let decision = decide(&result, &mut backoff, &mut auth);
            retrying_auth = decision == Decision::RetryAuthOnce;
            match decision {
                Decision::RetryAuthOnce => continue,
                Decision::Publish => {
                    let snapshot = clock.snapshot(result.unwrap_or_default(), Utc::now());
                    save_snapshot(&cache_dir, &snapshot).await;

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
                        let empty = clock.snapshot(Vec::new(), Utc::now());
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

/// The two cache writes, on the blocking pool rather than on this task's
/// async worker: both are `std::fs`, and the runtime they would otherwise
/// occupy is shared with the analytics scan and every command the panel
/// invokes.
///
/// Both discard their failure, and that swallow is deliberate rather than
/// overlooked. This app has no log sink, and one cache write failing costs
/// exactly one thing: at the *next* launch the panel and the menu bar are
/// empty for one poll interval instead of showing the previous session's
/// numbers — which is also what a first-ever launch looks like, and is
/// undone by the first successful poll. Nothing the user entered is at
/// stake, nothing else reads these files, and no action is available to
/// them if they were told. Surfacing it would mean inventing a user-facing
/// channel (a banner, a system notification) for a condition that is neither
/// actionable nor harmful; the honest alternative to this comment is a log
/// line, and there is nowhere to put one. If a sink ever appears, this is
/// one of the places that wants it.
async fn save_snapshot(dir: &std::path::Path, snapshot: &UsageSnapshot) {
    let dir = dir.to_path_buf();
    let snapshot = snapshot.clone();
    let _ =
        tauri::async_runtime::spawn_blocking(move || cache::save_snapshot(&dir, &snapshot)).await;
}

/// The profile half of `save_snapshot` above, with the same reasoning about
/// both the blocking pool and the discarded failure.
async fn save_profile(dir: &std::path::Path, profile: &Profile) {
    let dir = dir.to_path_buf();
    let profile = profile.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || cache::save_profile(&dir, &profile)).await;
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

    /// The two lines at the top of the poll loop's tail, run against the worst
    /// interval a hand-edited `settings.json` can still produce. `Instant::add`
    /// panics on overflow, and a panic in this spawned task is swallowed by
    /// tokio: the loop would stop, the tray would keep showing its last
    /// snapshot, and nothing would say so until the next launch. Remove
    /// `settings::MAX_POLL_INTERVAL_SECS` (or weaken `sanitized`'s `clamp` back
    /// to a `max`) and the first `+` below panics with `overflow when adding
    /// duration to instant`, failing this test — checked by doing exactly that.
    #[tokio::test]
    async fn an_absurd_settings_interval_still_yields_a_deadline() {
        let sanitized = crate::settings::Settings {
            poll_interval_secs: u64::MAX,
            ..Default::default()
        }
        .sanitized();
        let interval = Duration::from_secs(sanitized.poll_interval_secs);
        let backoff = Backoff::new();

        let before = tokio::time::Instant::now();
        let deadline = before + backoff.next_delay(interval);

        assert!(deadline > before);
        assert!(
            deadline <= before + Duration::from_secs(crate::settings::MAX_POLL_INTERVAL_SECS),
            "a sanitized interval must not outrun the ceiling"
        );
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

    /// R55: pins the composed `profile_is_due` predicate, not just its two
    /// inputs in isolation — a rate-limited account is exactly where the
    /// profile fetch is also likely to fail, so the backoff term has to
    /// override staleness rather than merely combine with it.
    mod profile_is_due_tests {
        use super::*;

        fn hours_ago(h: i64) -> DateTime<Utc> {
            Utc::now() - ChronoDuration::hours(h)
        }

        #[test]
        fn due_when_never_fetched_and_not_backing_off() {
            assert!(profile_is_due(false, None, Utc::now()));
        }

        #[test]
        fn due_when_stale_and_not_backing_off() {
            let fetched_at = hours_ago(PROFILE_MAX_AGE_HOURS + 1);
            assert!(profile_is_due(false, Some(fetched_at), Utc::now()));
        }

        #[test]
        fn not_due_when_fresh_and_not_backing_off() {
            let fetched_at = hours_ago(PROFILE_MAX_AGE_HOURS - 1);
            assert!(!profile_is_due(false, Some(fetched_at), Utc::now()));
        }

        /// The case R55 exists for: staleness alone would say "due", but an
        /// active backoff must override that rather than merely factor into
        /// it — this is what would still pass if `profile_is_due` used `||`
        /// instead of `&&`, or dropped the backoff term outright.
        #[test]
        fn not_due_while_backing_off_even_though_stale() {
            let fetched_at = hours_ago(PROFILE_MAX_AGE_HOURS + 1);
            assert!(!profile_is_due(true, Some(fetched_at), Utc::now()));
        }

        /// The narrower, worse case: never having a profile at all is the
        /// strongest possible claim to staleness, and an active backoff
        /// still has to win.
        #[test]
        fn not_due_while_backing_off_even_when_never_fetched() {
            assert!(!profile_is_due(true, None, Utc::now()));
        }
    }

    /// R56: pins the actual call-site wiring, not just `profile_is_due` in
    /// isolation. The five `profile_is_due` tests above pin the pure
    /// predicate faithfully and could never have caught R56's bug, because
    /// none of them touch a `Result` — the loop's `backoff.is_backing_off()`
    /// alone was off by one iteration, missing a rate limit that started on
    /// the very poll being evaluated (its outcome, `result`, is known at the
    /// call site; only `backoff`'s bookkeeping for it is not, since
    /// `decide()` runs later). `should_fetch_profile` is threaded verbatim
    /// into `spawn()`, so these tests exercise the same composition the loop
    /// runs, built from a plain `Result` and a plain `Backoff` — no
    /// `AppHandle`, no credentials, no async runtime required.
    mod should_fetch_profile_tests {
        use super::*;

        /// The bug R56 exists for: a fresh `Backoff` (nothing decided yet)
        /// says "not backing off", but *this* poll's own result already is
        /// a 429. Gating on `backoff` alone (the pre-R56 behaviour) would
        /// say "due" here — this is the exact case that slipped through
        /// with 5 passing `profile_is_due` tests and 0 failing ones.
        #[test]
        fn not_due_when_this_very_poll_is_rate_limited_even_with_a_fresh_backoff() {
            let result: Result<Vec<Quota>, ApiError> = Err(ApiError::RateLimited);
            let backoff = Backoff::new();
            assert!(!should_fetch_profile(
                &result,
                &backoff,
                false,
                None,
                None,
                Utc::now()
            ));
        }

        /// The ordinary path is unaffected: a successful poll, nothing ever
        /// fetched, no backoff in play — still due.
        #[test]
        fn due_when_this_poll_succeeds_and_nothing_indicates_trouble() {
            let result: Result<Vec<Quota>, ApiError> = Ok(vec![]);
            let backoff = Backoff::new();
            assert!(should_fetch_profile(
                &result,
                &backoff,
                false,
                None,
                None,
                Utc::now()
            ));
        }

        /// A backoff already active from an earlier iteration still holds
        /// the fetch back even when *this particular* poll happened to
        /// succeed — the OR must not let a good `result` alone override
        /// bookkeeping that says the endpoint is still owed quiet.
        #[test]
        fn not_due_when_backoff_is_already_active_even_if_this_poll_succeeded() {
            let result: Result<Vec<Quota>, ApiError> = Ok(vec![]);
            let mut backoff = Backoff::new();
            backoff.on_rate_limited();
            assert!(!should_fetch_profile(
                &result,
                &backoff,
                false,
                None,
                None,
                Utc::now()
            ));
        }

        /// Staleness still gates through the composition end to end: a
        /// clean poll with no backoff in play, but a profile fetched
        /// recently, is not due.
        #[test]
        fn not_due_when_this_poll_succeeds_but_the_profile_is_still_fresh() {
            let result: Result<Vec<Quota>, ApiError> = Ok(vec![]);
            let backoff = Backoff::new();
            let fetched_at = Utc::now() - ChronoDuration::hours(PROFILE_MAX_AGE_HOURS - 1);
            assert!(!should_fetch_profile(
                &result,
                &backoff,
                false,
                Some(fetched_at),
                None,
                Utc::now()
            ));
        }

        /// The 401 retry pass. `decide` answers a first `SignedOut` with
        /// `RetryAuthOnce`, and the loop `continue`s straight into another
        /// poll with no wait at all — deliberately, since the token has
        /// usually just rotated. What must not come with it is a second
        /// profile fetch: `profile_is_due` still says due on both passes
        /// whenever no profile has ever been cached, so the retry turned two
        /// requests into four inside about a second, which is the shape the
        /// module header documents as tripping this endpoint's burst limit.
        #[test]
        fn not_due_on_the_immediate_re_auth_retry_pass() {
            let result: Result<Vec<Quota>, ApiError> = Err(ApiError::SignedOut);
            let backoff = Backoff::new();
            assert!(!should_fetch_profile(
                &result,
                &backoff,
                true,
                None,
                None,
                Utc::now()
            ));
        }

        /// A different kind of failure this same poll (offline, signed out)
        /// must not trip the rate-limit term — only `RateLimited` should.
        #[test]
        fn a_non_rate_limit_failure_this_poll_does_not_hold_the_fetch_back_by_itself() {
            let result: Result<Vec<Quota>, ApiError> = Err(ApiError::SignedOut);
            let backoff = Backoff::new();
            assert!(should_fetch_profile(
                &result,
                &backoff,
                false,
                None,
                None,
                Utc::now()
            ));
        }
    }

    /// R57: the profile endpoint's *own* failures back off.
    ///
    /// Nothing in `should_fetch_profile`'s other four inputs can see one. A
    /// failed profile fetch leaves `fetched_at` exactly as stale as it was
    /// and touches neither the usage `result` nor the usage `backoff`, so
    /// before this the loop asked again on the very next poll, and on every
    /// poll after that, for as long as the endpoint kept failing.
    mod profile_retry_tests {
        use super::*;

        #[test]
        fn repeated_failures_escalate_through_the_usage_poll_s_own_steps() {
            let now = Utc::now();
            let mut backoff = Backoff::new();
            let after = |backoff: &mut Backoff| profile_retry_after(backoff, false, now);

            assert_eq!(
                after(&mut backoff),
                Some(now + ChronoDuration::seconds(120))
            );
            assert_eq!(
                after(&mut backoff),
                Some(now + ChronoDuration::seconds(300))
            );
            assert_eq!(
                after(&mut backoff),
                Some(now + ChronoDuration::seconds(900))
            );
            assert_eq!(
                after(&mut backoff),
                Some(now + ChronoDuration::seconds(900)),
                "and caps there rather than growing without bound"
            );
        }

        /// The reset half, which is what keeps a single blip from holding
        /// the endpoint off for a quarter of an hour: the success clears the
        /// hold, and the *next* failure starts from the first step again
        /// rather than resuming where the last run left off.
        #[test]
        fn a_success_clears_the_hold_and_the_escalation_with_it() {
            let now = Utc::now();
            let mut backoff = Backoff::new();
            profile_retry_after(&mut backoff, false, now);
            profile_retry_after(&mut backoff, false, now);

            assert_eq!(profile_retry_after(&mut backoff, true, now), None);
            assert_eq!(
                profile_retry_after(&mut backoff, false, now),
                Some(now + ChronoDuration::seconds(120))
            );
        }

        /// The composition, which is the half R56 taught us not to leave
        /// untested: a hold produced by `profile_retry_after` really does
        /// suppress `should_fetch_profile` while it stands, and really does
        /// stop suppressing it once it expires. Every other input here says
        /// "due" — a clean poll, no usage backoff, no profile ever cached —
        /// so the hold is the only thing under test.
        #[test]
        fn a_standing_hold_suppresses_the_fetch_until_the_moment_it_expires() {
            let now = Utc::now();
            let mut backoff = Backoff::new();
            let retry_at = profile_retry_after(&mut backoff, false, now);
            let result: Result<Vec<Quota>, ApiError> = Ok(vec![]);

            assert!(
                should_fetch_profile(&result, &Backoff::new(), false, None, None, now),
                "with no hold at all, everything here says due"
            );
            assert!(!should_fetch_profile(
                &result,
                &Backoff::new(),
                false,
                None,
                retry_at,
                now + ChronoDuration::seconds(119)
            ));
            assert!(
                should_fetch_profile(
                    &result,
                    &Backoff::new(),
                    false,
                    None,
                    retry_at,
                    now + ChronoDuration::seconds(120)
                ),
                "the hold is exclusive: the instant it names is already free"
            );
        }
    }

    /// R58: `fetched_at` is how the settings window tells one snapshot from
    /// the next. `titleEntries.ts`'s `reconcileTitleEntries` counts each
    /// snapshot exactly once towards retiring an absent quota's title entry,
    /// and `fetchedAt` is the whole of that identity — so two emits sharing
    /// a stamp count as one, the absence tally stalls, and a row for a quota
    /// the user no longer has never goes away.
    ///
    /// A frozen clock is the case `Utc::now()` cannot answer for by itself:
    /// two readings landing inside one tick of whatever the platform's clock
    /// resolution happens to be.
    mod snapshot_clock_tests {
        use super::*;

        #[test]
        fn two_snapshots_stamped_from_one_reading_still_differ() {
            let frozen = Utc::now();
            let mut clock = SnapshotClock::default();

            let first = clock.snapshot(Vec::new(), frozen).fetched_at;
            let second = clock.snapshot(Vec::new(), frozen).fetched_at;
            assert_ne!(first, second);
        }

        /// The run, not just the pair: five emits inside one tick must never
        /// repeat the stamp before them, or a quota needs more than
        /// `RETIREMENT_MISSES` absences to retire.
        ///
        /// Adjacent distinctness, not a strictly increasing sequence — that
        /// is the contract, and it is the whole of what
        /// `reconcileTitleEntries` compares. See `SnapshotClock`'s own
        /// comment for why the stronger promise is deliberately not made.
        #[test]
        fn a_run_of_snapshots_from_one_reading_never_repeats_its_predecessor() {
            let frozen = Utc::now();
            let mut clock = SnapshotClock::default();

            let stamps: Vec<DateTime<Utc>> = (0..5)
                .map(|_| clock.snapshot(Vec::new(), frozen).fetched_at)
                .collect();
            assert!(
                stamps.windows(2).all(|pair| pair[0] != pair[1]),
                "{stamps:?}"
            );
        }

        /// An NTP correction stepping the clock backwards must still hand
        /// out a stamp distinct from the one before it — and must hand out
        /// the *real* reading while doing so.
        ///
        /// Forcing the stamp forward instead would leave `fetched_at` an
        /// hour ahead of the wall clock for an hour, and `secondsSince`
        /// clamps a future stamp to zero: the footer would say "updated just
        /// now" and never turn amber, for the whole hour, however stale the
        /// numbers actually got. Distinctness is what the frontend needs;
        /// this is the price it does not have to pay for it.
        #[test]
        fn a_clock_that_steps_backwards_is_stamped_with_the_real_reading() {
            let now = Utc::now();
            let stepped_back = now - ChronoDuration::hours(1);
            let mut clock = SnapshotClock::default();

            let first = clock.snapshot(Vec::new(), now).fetched_at;
            let second = clock.snapshot(Vec::new(), stepped_back).fetched_at;

            assert_ne!(first, second);
            assert_eq!(
                second, stepped_back,
                "a stepped-back clock is reported, not overridden, or the age goes wrong"
            );
        }

        /// Every stamp is a genuine clock reading except the one case where
        /// it would exactly repeat its predecessor. So an advancing clock is
        /// reported verbatim, and the nanosecond nudge cannot accumulate
        /// into drift that pulls the panel's age away from the truth.
        #[test]
        fn an_advancing_clock_is_stamped_verbatim() {
            let now = Utc::now();
            let later = now + ChronoDuration::seconds(60);
            let mut clock = SnapshotClock::default();

            assert_eq!(clock.snapshot(Vec::new(), now).fetched_at, now);
            assert_eq!(clock.snapshot(Vec::new(), later).fetched_at, later);

            // And the nudge itself does not push the *next* reading off: one
            // repeat, then a clock that has moved on, lands on the truth.
            assert_ne!(clock.snapshot(Vec::new(), later).fetched_at, later);
            let later_still = later + ChronoDuration::seconds(60);
            assert_eq!(
                clock.snapshot(Vec::new(), later_still).fetched_at,
                later_still
            );
        }

        /// The cached snapshot replayed at launch is an emit like any other
        /// as far as the settings window is concerned, and its stamp comes
        /// from a previous process rather than from this clock. So the first
        /// live snapshot has to be distinct from that one too — which is
        /// what `SnapshotClock::after` is for.
        #[test]
        fn the_first_live_stamp_differs_from_the_replayed_cache_snapshot() {
            let cached = Utc::now();
            let mut clock = SnapshotClock::after(Some(cached));

            assert_ne!(clock.snapshot(Vec::new(), cached).fetched_at, cached);
        }

        /// The invariant as the other language actually sees it. `fetchedAt`
        /// crosses the boundary as a JSON string, so stamps that differ by
        /// less than the serialized precision would arrive identical however
        /// distinct they are in Rust — and the nudge above is one
        /// nanosecond, which is exactly the size that would be lost to a
        /// millisecond-truncating encoder.
        #[test]
        fn distinct_stamps_survive_serialization_to_the_frontend() {
            let frozen = Utc::now();
            let mut clock = SnapshotClock::default();

            let first = serde_json::to_value(clock.snapshot(Vec::new(), frozen)).unwrap();
            let second = serde_json::to_value(clock.snapshot(Vec::new(), frozen)).unwrap();
            assert_ne!(
                first["fetchedAt"], second["fetchedAt"],
                "the two stamps arrive at the frontend as the same string: {first}, {second}"
            );
        }

        /// Everything else the two literals this replaced were carrying:
        /// the quotas pass through untouched, and the snapshot is live.
        /// `stale` is what `cache::load_snapshot` sets on the replayed one,
        /// and nothing in the loop sets it back.
        #[test]
        fn the_quotas_pass_through_and_a_stamped_snapshot_is_never_stale() {
            let quotas = vec![Quota {
                id: "session".into(),
                label: "Session".into(),
                percent: 20.0,
                severity: crate::model::Severity::Normal,
                resets_at: None,
                is_active: true,
            }];
            let snapshot = SnapshotClock::default().snapshot(quotas.clone(), Utc::now());

            assert_eq!(snapshot.quotas, quotas);
            assert!(!snapshot.stale);
        }
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

    /// These constants are measured, not chosen, and the measurement is the
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
    /// place the absolute values are pinned — the manual-refresh throttle, the
    /// backoff steps, and `settings::MIN_POLL_INTERVAL_SECS`, which the same
    /// burst-sensitivity and ~2-minute-recovery measurement justifies. Changing
    /// any of them should mean re-measuring, not editing this test.
    #[test]
    fn the_measured_rate_limit_values_are_what_ship() {
        assert_eq!(MIN_MANUAL_REFRESH_SECS, 60);
        assert_eq!(BACKOFF_STEPS, [2 * 60, 5 * 60, 15 * 60]);
        assert_eq!(crate::settings::MIN_POLL_INTERVAL_SECS, 60);
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

    /// The harder case named in the brief: a refresh that lands *before*
    /// `wait_for_next_poll` is even called (e.g. the previous poll was still
    /// in flight when the panel was opened). `tokio::sync::Notify` stores at
    /// most one wake for the next call to `notified()` to consume, so this
    /// still must not shorten an active backoff — the stored wake gets
    /// consumed and discarded by the first loop iteration, which then
    /// re-arms `notified()` and waits for a genuinely new signal.
    #[tokio::test]
    async fn a_refresh_that_arrived_before_the_wait_began_does_not_shorten_a_backoff() {
        let mut backoff = Backoff::new();
        backoff.on_rate_limited();
        let signal = RefreshSignal::default();
        assert!(
            signal.request(),
            "the first-ever refresh is never throttled"
        );

        let wait = std::time::Duration::from_millis(200);
        let deadline = tokio::time::Instant::now() + wait;
        let started = std::time::Instant::now();
        wait_for_next_poll(deadline, &signal, &backoff).await;
        let elapsed = started.elapsed();

        assert!(
            elapsed >= std::time::Duration::from_millis(180),
            "a pre-arrived refresh must not shorten an active backoff either, elapsed = {elapsed:?}"
        );
    }

    /// Same pre-arrival, outside a backoff: the stored wake should resolve
    /// the wait almost immediately, same as a live signal would.
    #[tokio::test]
    async fn a_refresh_that_arrived_before_the_wait_began_still_cuts_an_ordinary_wait_short() {
        let backoff = Backoff::new();
        let signal = RefreshSignal::default();
        assert!(
            signal.request(),
            "the first-ever refresh is never throttled"
        );

        let wait = std::time::Duration::from_millis(200);
        let deadline = tokio::time::Instant::now() + wait;
        let started = std::time::Instant::now();
        wait_for_next_poll(deadline, &signal, &backoff).await;
        let elapsed = started.elapsed();

        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "a pre-arrived refresh outside a backoff should resolve almost immediately, elapsed = {elapsed:?}"
        );
    }
}
