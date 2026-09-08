//! The manual "Check for Updates…" path (spec §13.1).
//!
//! This is a Rust-only feature: the check runs entirely off a menu event
//! handler, never through the frontend, so it needs no entry in
//! `capabilities/default.json` — the capability system only gates commands
//! reached through `invoke()` from a webview, and nothing here goes through
//! that path.
//!
//! Not automatic on launch (spec's original wording notwithstanding): an
//! explicit menu item is honest about when a network request happens and
//! adds no startup latency. Once the mechanism has proven itself in the
//! wild, automatic checking can follow.
//!
//! **The system notification is best-effort, not the channel this feature
//! depends on.** `tauri_plugin_notification` 2.4.0's `show()` (read in
//! `tauri-plugin-notification-2.4.0/src/desktop.rs:179-217`) hands the real
//! OS dispatch to a detached `tauri::async_runtime::spawn` and discards
//! *that* task's result (`let _ = notification.show();`), then returns
//! `Ok(())` unconditionally — the only fallible step in the function is
//! gated to `#[cfg(windows)]`, and this project does not ship Windows. So on
//! macOS and Linux, `.show()` cannot report a revoked permission or a
//! missing notification daemon; it always claims success. Wrapping that
//! call's `Result` cannot be a fallback for those failure modes, because
//! that branch is unreachable for them — an earlier version of this module
//! did exactly that and was wrong to.
//!
//! The channel that actually works is [`UpdateCheckStatus`]: the outcome of
//! the last check, held in `.manage()`d state and rendered fresh into the
//! `Check for Updates…` menu label every time the tray menu is rebuilt. That
//! rebuild is not left to chance or to the poller's own cadence — `spawn_check`
//! forces one (`tray::refresh_menu`) the instant the status changes, both when
//! a check starts and when it finishes, so the label reflects reality within
//! the same click rather than up to a poll interval later. The user clicked a
//! menu item; the answer belongs in that menu, on that click, where this app
//! controls delivery completely — nothing about it depends on an OS
//! notification daemon existing.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

/// The three, and only three, outcomes of a check. Reporting only the happy
/// path is the failure mode this project keeps rediscovering — a menu item
/// that does nothing visible when the network is down is worse than no menu
/// item at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// A newer release was found, downloaded, verified and installed. The
    /// app restarts into it right after this is reported.
    Installing { version: String },
    /// The running version is already current.
    UpToDate,
    /// The check (or the download, or the install) could not complete.
    Failed { reason: String },
}

impl CheckOutcome {
    /// Title and body for the best-effort system notification. See the
    /// module doc comment: showing it can silently do nothing, so this is
    /// never the only place an outcome is recorded.
    fn notification(&self) -> (String, String) {
        match self {
            CheckOutcome::Installing { version } => (
                "Claude Usage update found".to_string(),
                format!("Installing version {version} — restarting…"),
            ),
            CheckOutcome::UpToDate => (
                "Claude Usage is up to date".to_string(),
                format!("Running version {}", env!("CARGO_PKG_VERSION")),
            ),
            CheckOutcome::Failed { reason } => (
                "Claude Usage update check failed".to_string(),
                reason.clone(),
            ),
        }
    }

    /// The `Check for Updates…` menu label while this outcome is showing —
    /// the channel this feature actually depends on.
    fn menu_label(&self) -> String {
        match self {
            CheckOutcome::Installing { .. } => "Update available — installing…".to_string(),
            CheckOutcome::UpToDate => format!("Up to date (v{})", env!("CARGO_PKG_VERSION")),
            CheckOutcome::Failed { reason } => format!("Check failed — {}", short_reason(reason)),
        }
    }
}

/// Keeps a failure reason short enough to sit in one menu row. A `reqwest`
/// or signature-verification error's `Display` text can run well past what
/// a menu should stretch to.
fn short_reason(reason: &str) -> String {
    const MAX_CHARS: usize = 40;
    if reason.chars().count() <= MAX_CHARS {
        reason.to_string()
    } else {
        let truncated: String = reason.chars().take(MAX_CHARS).collect();
        format!("{truncated}…")
    }
}

/// How long a completed outcome stays in the menu label before falling back
/// to idle. Checked lazily whenever the label is read — `tray::apply` reads
/// it on every poll, so that read already happens regularly enough that no
/// separate timer is needed to clear it. The read is what expires the
/// outcome, and the expiry then shows up as a changed label, which is what
/// makes `apply` rebuild the menu that poll: it skips a rebuild only when
/// this label *and* the quota lines are both unchanged (`tray::LastMenu`).
const OUTCOME_VISIBLE_FOR: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default)]
enum CheckState {
    #[default]
    Idle,
    Checking,
    Done {
        outcome: CheckOutcome,
        at: Instant,
    },
}

/// `.manage()`d state behind the `Check for Updates…` menu label — see the
/// module doc comment for why this, and not the system notification, is
/// the channel this feature depends on.
#[derive(Default)]
pub struct UpdateCheckStatus(Mutex<CheckState>);

impl UpdateCheckStatus {
    // `pub(crate)` rather than private: `tray.rs`'s tests drive these
    // directly to prove a status change reaches `trailing_rows()` output
    // with no poll involved (R52) — production code only ever calls them
    // from `spawn_check` below.

    /// Enters `Checking`, reporting whether it actually transitioned. `false`
    /// means a check was already in flight and this caller must not start a
    /// second one: two `download_and_install` calls would write the same
    /// `/Applications` bundle concurrently and then restart the app twice.
    /// The check-and-set happens under one lock, so two menu clicks racing on
    /// two threads still yield exactly one `true`.
    pub(crate) fn set_checking(&self) -> bool {
        let mut state = self.0.lock().unwrap();
        if matches!(*state, CheckState::Checking) {
            return false;
        }
        *state = CheckState::Checking;
        true
    }

    pub(crate) fn set_done(&self, outcome: CheckOutcome) {
        *self.0.lock().unwrap() = CheckState::Done {
            outcome,
            at: Instant::now(),
        };
    }

    /// The label to render right now. Expires a completed outcome back to
    /// idle once it has been showing for `OUTCOME_VISIBLE_FOR`; reading the
    /// label before that never resets it — two menu rebuilds in a row while
    /// an outcome is still fresh both see the same text.
    pub fn label(&self) -> String {
        let mut state = self.0.lock().unwrap();
        if let CheckState::Done { at, .. } = &*state {
            if at.elapsed() >= OUTCOME_VISIBLE_FOR {
                *state = CheckState::Idle;
            }
        }
        match &*state {
            CheckState::Idle => "Check for Updates…".to_string(),
            CheckState::Checking => "Checking for updates…".to_string(),
            CheckState::Done { outcome, .. } => outcome.menu_label(),
        }
    }
}

/// Runs one check, updates the shared status the menu label reads from, and
/// restarts the app if an update was installed. Also shows a best-effort
/// system notification — genuinely nicer when it works, but see the module
/// doc comment for why it is never the thing being depended on.
///
/// Spawned rather than run inline because `check()` and `download_and_install()`
/// are async while the menu event handler that calls this is not. The status
/// is set to `Checking` synchronously, before the async work is scheduled, and
/// `tray::refresh_menu` is called right after — both here and again once the
/// outcome is known — because nothing else rebuilds the menu on demand:
/// `tray::apply` only runs on the poller's own cycle, up to a minute away, and
/// without this the label would sit unchanged until then. That gap — a
/// user-initiated action producing nothing visible until an unrelated timer
/// happens to fire — is exactly the failure mode this module exists to rule
/// out; deriving the label from state (R51) is necessary but was not
/// sufficient on its own without also forcing the rebuild that reads it.
pub fn spawn_check(app: AppHandle) {
    if let Some(status) = app.try_state::<std::sync::Arc<UpdateCheckStatus>>() {
        // A check is already running: leave it alone. Returning before
        // `refresh_menu` too, because the label already says "Checking for
        // updates…" — rebuilding the menu again would only redraw the same
        // text this click did not change.
        if !status.set_checking() {
            return;
        }
    }
    crate::tray::refresh_menu(&app);
    tauri::async_runtime::spawn(async move {
        let outcome = run_check(&app).await;
        if let Some(status) = app.try_state::<std::sync::Arc<UpdateCheckStatus>>() {
            status.set_done(outcome.clone());
        }
        crate::tray::refresh_menu(&app);
        notify(
            &app,
            &outcome,
            crate::settings::load(&app).notifications_enabled,
        );
        if let CheckOutcome::Installing { .. } = outcome {
            app.restart();
        }
    });
}

/// Shows a best-effort system notification for `outcome`, if the user's
/// `notifications_enabled` setting allows it, and echoes the same text onto
/// the tray icon's tooltip. Both are courtesies on top of the
/// real channel, which is the `Check for Updates…` menu label
/// (`UpdateCheckStatus`, already rebuilt into the menu via
/// `tray::refresh_menu` by the time `spawn_check` calls this). Neither call
/// here can report whether anything actually became visible (see the module
/// doc comment), so neither is something the user should have to rely on to
/// learn the outcome. On Linux, the tooltip echo is itself a documented
/// no-op in the `tray-icon` crate's
/// GTK/AppIndicator backend (confirmed by reading
/// `tray-icon-0.24.2/src/platform_impl/gtk/mod.rs`, whose `set_tooltip`
/// always returns `Ok(())` without doing anything); on macOS it genuinely
/// sets the native tooltip.
fn notify<R: tauri::Runtime>(
    app: &AppHandle<R>,
    outcome: &CheckOutcome,
    notifications_enabled: bool,
) {
    let (title, body) = outcome.notification();
    // One switch, honoured the same way the poller honours it: with
    // notifications off, nothing is posted to the OS from here either. Only
    // this call is gated — the menu label is the channel this feature depends
    // on and stays unconditional, and so does the tooltip echo below, which
    // is part of the tray this app draws rather than something handed to a
    // notification daemon.
    if notifications_enabled {
        let _ = app
            .notification()
            .builder()
            .title(title.clone())
            .body(body.clone())
            .show();
    }
    if let Some(tray) = app.tray_by_id(crate::tray::TRAY_ID) {
        let _ = tray.set_tooltip(Some(tooltip_line(&title, &body)));
    }
}

/// The single line the tray tooltip echo uses.
fn tooltip_line(title: &str, body: &str) -> String {
    format!("{title}: {body}")
}

/// Bounds every request the updater makes. Without it there is none: neither
/// `check()` nor `download_and_install()` sets one, so a connection that opens
/// and then stalls leaves the check running forever. That used to be survivable
/// — a second click started a fresh one — but the re-entrancy guard now turns
/// the same click into a no-op, so a wedged check would sit in the menu reading
/// "Checking for updates…" until the app was restarted, with no way out and no
/// explanation. A bounded request fails instead, and a failure is a state this
/// module already knows how to show.
///
/// Five minutes rather than something snappier because tauri-plugin-updater
/// 2.11.0 applies this one value to the download as well as the check
/// (`updater.rs:504` and `:698`), and the 0.1.0 bundle is ~6 MB — a short
/// timeout would abort legitimate slow downloads. The common failures (no
/// route, DNS, refused connection) return in seconds regardless; this only
/// bounds the rare stalled-mid-transfer case.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

async fn run_check(app: &AppHandle) -> CheckOutcome {
    let updater = match app.updater_builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(updater) => updater,
        Err(e) => {
            return CheckOutcome::Failed {
                reason: e.to_string(),
            }
        }
    };
    match updater.check().await {
        Ok(Some(update)) => {
            let version = update.version.clone();
            match update
                .download_and_install(|_chunk_len, _total| {}, || {})
                .await
            {
                Ok(()) => CheckOutcome::Installing { version },
                Err(e) => CheckOutcome::Failed {
                    reason: e.to_string(),
                },
            }
        }
        Ok(None) => CheckOutcome::UpToDate,
        Err(e) => CheckOutcome::Failed {
            reason: e.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_available_update_reports_the_version_and_says_it_is_installing() {
        let (title, body) = CheckOutcome::Installing {
            version: "0.2.0".into(),
        }
        .notification();
        assert_eq!(title, "Claude Usage update found");
        assert_eq!(body, "Installing version 0.2.0 — restarting…");
    }

    #[test]
    fn an_up_to_date_app_reports_the_running_version() {
        let (title, body) = CheckOutcome::UpToDate.notification();
        assert_eq!(title, "Claude Usage is up to date");
        assert_eq!(
            body,
            format!("Running version {}", env!("CARGO_PKG_VERSION"))
        );
    }

    /// The exact case this module exists to prevent: a check that fails
    /// silently. Whatever the reason, it must reach the user.
    #[test]
    fn a_failed_check_reports_a_reason_rather_than_nothing() {
        let (title, body) = CheckOutcome::Failed {
            reason: "error sending request".into(),
        }
        .notification();
        assert_eq!(title, "Claude Usage update check failed");
        assert_eq!(body, "error sending request");
    }

    /// What this actually proves: the plugin registers, and `app.updater()`
    /// succeeds once the app's config carries a `plugins.updater` section
    /// shaped like the real `tauri.conf.json` (a non-empty endpoint list —
    /// the pubkey is never validated at this stage, only when a fetched
    /// signature is actually verified, so any string does here).
    ///
    /// This does *not* by itself demonstrate ACL-freedom: `app.updater()`
    /// was never routed through `invoke_handler` to begin with, so there
    /// was no capability gate for this call to pass through in the first
    /// place, mocked runtime or real one. The ACL argument is architectural
    /// (see the module doc comment above) — `UpdaterExt` is a blanket impl
    /// for `Manager<R>`, called directly from a menu event handler, never
    /// through `invoke()` — not something this test exercises.
    #[test]
    fn the_updater_plugin_registers_with_a_valid_config() {
        // `mock_context` leaves every plugin's config section absent, which
        // the updater plugin's mandatory (non-`Option`) `Config` cannot
        // deserialize from — so this fills in the one field that matters
        // for building an `Updater` (a non-empty endpoint list).
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().plugins.0.insert(
            "updater".into(),
            serde_json::json!({
                "pubkey": "test",
                "endpoints": ["https://example.com/latest.json"]
            }),
        );
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .expect("failed to build mock app with the updater plugin registered");
        assert!(app.handle().updater().is_ok());
    }

    /// The `notifications_enabled` switch is one switch: the poller honours
    /// it (`settings::sanitized()` clears the thresholds it evaluates
    /// against), so a user who turned notifications off must not still get
    /// update banners from here.
    ///
    /// Testable because this mock app has no notification plugin registered,
    /// and `app.notification()` panics in that case ("state() called before
    /// manage()"). Reaching the notification at all is therefore observable,
    /// which is what lets this test fail — see the twin below, which asserts
    /// that the very same call does reach it when the setting is on. The
    /// delivery of a notification that *is* posted stays untestable for the
    /// reasons the module doc comment gives.
    #[test]
    fn the_system_notification_is_suppressed_when_notifications_are_off() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app");
        notify(app.handle(), &CheckOutcome::UpToDate, false);
    }

    /// The other half: with notifications on, `notify` still goes to the
    /// notification plugin. Without this, a `notify` that had quietly stopped
    /// notifying anyone at all would pass the test above.
    #[test]
    #[should_panic(expected = "state() called before manage()")]
    fn the_system_notification_is_attempted_when_notifications_are_on() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app");
        notify(app.handle(), &CheckOutcome::UpToDate, true);
    }

    #[test]
    fn the_tooltip_line_combines_title_and_body() {
        assert_eq!(
            tooltip_line("Claude Usage update check failed", "boom"),
            "Claude Usage update check failed: boom"
        );
    }

    // The system notification's own delivery is not exercised by a test.
    // `tauri_plugin_notification`'s `.show()` calls straight into the real
    // OS notification stack (`notify-rust` on desktop) regardless of
    // `tauri::test::MockRuntime`, and — per the module doc comment — always
    // returns `Ok(())` on the platforms this app ships, so there is nothing
    // for a test to observe there even in principle. That is exactly why
    // the menu label below, not the notification, is what is tested as the
    // feature's real channel.

    #[test]
    fn each_outcome_maps_to_its_menu_label() {
        assert_eq!(
            CheckOutcome::Installing {
                version: "0.2.0".into()
            }
            .menu_label(),
            "Update available — installing…"
        );
        assert_eq!(
            CheckOutcome::UpToDate.menu_label(),
            format!("Up to date (v{})", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            CheckOutcome::Failed {
                reason: "boom".into()
            }
            .menu_label(),
            "Check failed — boom"
        );
    }

    #[test]
    fn a_long_failure_reason_is_truncated_in_the_menu_label() {
        let reason = "x".repeat(100);
        let label = CheckOutcome::Failed {
            reason: reason.clone(),
        }
        .menu_label();
        assert!(label.starts_with("Check failed — "));
        assert!(label.chars().count() < reason.chars().count());
        assert!(label.ends_with('…'));
    }

    #[test]
    fn the_status_starts_idle_and_reflects_a_check_in_flight() {
        let status = UpdateCheckStatus::default();
        assert_eq!(status.label(), "Check for Updates…");
        status.set_checking();
        assert_eq!(status.label(), "Checking for updates…");
    }

    /// The requirement R51 exists for: `tray::apply` rebuilds the whole
    /// menu whenever its content moves, so if the label were derived once
    /// and baked into the menu rather than read fresh from this state, the
    /// next rebuild — which a poll a few seconds later can trigger — would
    /// silently wipe it. Reading the label twice in a row, simulating two
    /// such rebuilds while the outcome is still fresh, must return the same
    /// text both times.
    /// Two clicks on `Check for Updates…` in quick succession used to give
    /// two concurrent `download_and_install` calls writing the same
    /// `/Applications` bundle, and then two `app.restart()`. `spawn_check`
    /// needs an `AppHandle`, so the guard lives here in the state machine
    /// where it can be tested: entering `Checking` twice must report one
    /// transition, not two.
    /// The timeout has to outlast a whole bundle download, not just a metadata
    /// fetch, because the plugin applies one value to both requests. Written as
    /// the inequality that actually constrains it rather than as `assert_eq!`
    /// on the constant: lowering it to something that feels responsive is the
    /// plausible wrong move, and this is the reason that would be wrong.
    #[test]
    fn the_request_timeout_outlasts_a_slow_bundle_download() {
        const BUNDLE_BYTES: u64 = 7 * 1024 * 1024;
        const SLOW_LINK_BYTES_PER_SEC: u64 = 50 * 1024;
        let needed = BUNDLE_BYTES / SLOW_LINK_BYTES_PER_SEC;
        assert!(
            REQUEST_TIMEOUT.as_secs() > needed,
            "{}s does not cover a {} MB download at {} KB/s, which needs {}s",
            REQUEST_TIMEOUT.as_secs(),
            BUNDLE_BYTES / (1024 * 1024),
            SLOW_LINK_BYTES_PER_SEC / 1024,
            needed
        );
    }

    #[test]
    fn a_second_check_while_one_is_in_flight_does_not_start_another() {
        let status = UpdateCheckStatus::default();
        assert!(status.set_checking(), "the first click must start a check");
        assert!(
            !status.set_checking(),
            "a second click while a check is in flight must not start another"
        );
        status.set_done(CheckOutcome::UpToDate);
        assert!(
            status.set_checking(),
            "once the check has finished, the next click starts a new one"
        );
    }

    #[test]
    fn a_done_outcome_survives_being_read_across_multiple_menu_rebuilds() {
        let status = UpdateCheckStatus::default();
        status.set_done(CheckOutcome::UpToDate);
        let first_rebuild = status.label();
        let second_rebuild = status.label();
        assert_eq!(first_rebuild, second_rebuild);
        assert_eq!(
            first_rebuild,
            format!("Up to date (v{})", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn a_done_outcome_expires_back_to_idle_after_the_visible_window() {
        let status = UpdateCheckStatus::default();
        status.set_done(CheckOutcome::Failed {
            reason: "boom".into(),
        });
        // Back-date the outcome rather than sleeping the test suite for
        // `OUTCOME_VISIBLE_FOR` — `Instant` supports subtracting a
        // `Duration` directly, so this is a real elapsed-time check, not a
        // mocked clock.
        {
            let mut state = status.0.lock().unwrap();
            if let CheckState::Done { at, .. } = &mut *state {
                *at = Instant::now() - OUTCOME_VISIBLE_FOR - Duration::from_secs(1);
            }
        }
        assert_eq!(status.label(), "Check for Updates…");
    }
}
