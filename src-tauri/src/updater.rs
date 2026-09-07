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
        let (title, body) = outcome.notification();
        let _ = app.notification().builder().title(title).body(body).show();
        if let CheckOutcome::Installing { .. } = outcome {
            app.restart();
        }
    });
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

    /// This project has twice shipped a feature an ACL silently blocked
    /// (the settings window's `invoke`, the popover's `setSize`). `updater()`
    /// is a plain Rust method call — via `UpdaterExt`'s blanket impl for
    /// `Manager<R>` — never routed through `invoke_handler`, so there is no
    /// capability for it to be blocked by. That is not asserted by reading
    /// the plugin's source: it is exercised here, against a real
    /// (mocked-runtime) app with the plugin actually registered, and it
    /// returns `Ok`, proving the plugin initialises and the call is
    /// permitted rather than merely assumed to be.
    #[test]
    fn the_updater_plugin_initialises_and_the_call_is_not_acl_blocked() {
        // `mock_context` leaves every plugin's config section absent, which
        // the updater plugin's mandatory (non-`Option`) `Config` cannot
        // deserialize from — so this fills in the one field that matters
        // for building an `Updater` (a non-empty endpoint list). The pubkey
        // is never validated at this stage, only when a fetched signature
        // is actually verified, so any string does here.
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
}
