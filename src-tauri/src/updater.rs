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

use tauri::AppHandle;
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
    /// Title and body for the system notification reporting this outcome.
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
}

/// Runs one check and reports the outcome to the user unconditionally, then
/// restarts the app if an update was installed.
///
/// Spawned rather than run inline because `check()` and `download_and_install()`
/// are async while the menu event handler that calls this is not.
pub fn spawn_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let outcome = run_check(&app).await;
        report(&app, &outcome);
        if let CheckOutcome::Installing { .. } = outcome {
            app.restart();
        }
    });
}

/// Delivers an outcome to the user. The system notification is the primary
/// channel, but showing it is itself fallible — notification permission
/// revoked, no notification daemon on a minimal Linux desktop, any other
/// OS-level refusal — and a `Check for Updates…` that produces nothing at
/// all in that case is exactly the failure mode this module exists to rule
/// out, for every outcome, `Failed` included.
///
/// On that failure this falls back to the tray icon's tooltip (persists
/// until replaced, discoverable by hovering) and a stderr line. Neither is
/// as good as the notification: the tooltip is a documented no-op on Linux
/// in the `tray-icon` crate's GTK/AppIndicator backend (confirmed by
/// reading `tray-icon-0.24.2/src/platform_impl/gtk/mod.rs`, whose
/// `set_tooltip` always returns `Ok(())` without doing anything — so on
/// Linux this call "succeeds" and nothing becomes visible), and stderr is
/// only seen by someone who launched the app from a terminal rather than
/// via autostart. On Linux specifically, if the notification daemon is the
/// thing that failed, there is genuinely no channel left that is both
/// reliable and visible without a dialog dependency this project has not
/// taken on — that residual gap is real and is documented in the README
/// rather than left to be discovered.
fn report(app: &AppHandle, outcome: &CheckOutcome) {
    let (title, body) = outcome.notification();
    if let Err(e) = app
        .notification()
        .builder()
        .title(title.clone())
        .body(body.clone())
        .show()
    {
        let line = fallback_text(&title, &body);
        eprintln!("failed to show update-check notification ({line}): {e}");
        if let Some(tray) = app.tray_by_id(crate::tray::TRAY_ID) {
            let _ = tray.set_tooltip(Some(line));
        }
    }
}

/// The single line shared by the tray-tooltip and stderr fallbacks.
fn fallback_text(title: &str, body: &str) -> String {
    format!("{title}: {body}")
}

async fn run_check(app: &AppHandle) -> CheckOutcome {
    let updater = match app.updater() {
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

    #[test]
    fn the_fallback_line_combines_title_and_body() {
        assert_eq!(
            fallback_text("Claude Usage update check failed", "boom"),
            "Claude Usage update check failed: boom"
        );
    }

    // `report`'s fallback path (the tray tooltip and the stderr line, taken
    // when `.show()` itself fails) is not exercised by a test. Forcing that
    // failure deterministically would require `tauri_plugin_notification`'s
    // `.show()` to fail on command — but it calls straight into the real OS
    // notification stack (`notify-rust` on desktop) regardless of
    // `tauri::test::MockRuntime`, which stubs the windowing runtime, not the
    // notification plugin's own OS calls. Making that failure injectable
    // would mean adding fault-injection scaffolding to `report` for the
    // sake of one fallback path, which is not warranted here. This is
    // stated plainly rather than papered over with a test that stops at
    // `notification()`'s string output, one layer above delivery, the way
    // the three tests above do.
}
