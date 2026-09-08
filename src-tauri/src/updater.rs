//! The manual "Check for Updates…" path (spec §13.1).
//!
//! This is a Rust-only feature: the check runs entirely off a menu event
//! handler, never through the frontend, so it needs no entry in
//! `capabilities/default.json` — the capability system only gates commands
//! reached through `invoke()` from a webview, and nothing here goes through
//! that path.
//!
//! That holds for the dialogs too, which are the one thing in this module a
//! webview *could* have reached. `tauri_plugin_dialog` does register
//! `commands::message`/`open`/`save` in its `invoke_handler` and ships
//! `allow-message`/`allow-open`/`allow-save` permissions for them — but that
//! is the IPC door only. `DialogExt::dialog()` is a blanket impl over
//! `Manager<R>` returning the plugin's `.manage()`d `Dialog`, and
//! `MessageDialogBuilder::show` runs
//! `desktop::show_message_dialog` → `AppHandle::run_on_main_thread` → `rfd`
//! (read in `tauri-plugin-dialog-2.7.3/src/desktop.rs:215-257`), which
//! consults no ACL on the way. Registering the plugin in `lib.rs` is the
//! whole of what a Rust-side dialog needs; `capabilities/default.json` stays
//! at the three permissions the webview actually uses.
//!
//! Tauri's ACL denies silently, so reading that was not enough on its own.
//! All three of these dialogs were driven onto the screen of a `tauri dev`
//! build whose capability file granted no dialog permission of any kind, and
//! each one's text was read back out of the live accessibility tree — the
//! confirm prompt, the "up to date" message and the failure message.
//!
//! Not automatic on launch (spec's original wording notwithstanding): an
//! explicit menu item is honest about when a network request happens and
//! adds no startup latency. Once the mechanism has proven itself in the
//! wild, automatic checking can follow.
//!
//! **The dialog is now the primary channel, and it is also where consent
//! lives.** Finding a newer version no longer downloads it: [`run_check`]
//! asks first, and the single `download_and_install` call site sits behind
//! that answer. A check that finds nothing, or fails, says so in a dialog of
//! its own rather than leaving the user to notice a menu label they are no
//! longer looking at.
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
//! The channel that actually works when nobody is looking is
//! [`UpdateCheckStatus`]: the outcome of the last check, held in
//! `.manage()`d state and rendered fresh into the `Check for Updates…` menu
//! label every time the tray menu is rebuilt. That rebuild is not left to
//! chance or to the poller's own cadence — `spawn_check` forces one
//! (`tray::refresh_menu`) the instant the status changes, when a check
//! starts, when it finishes, and when the user cancels it, so the label
//! reflects reality within the same click rather than up to a poll interval
//! later. The user clicked a menu item; the answer belongs in that menu, on
//! that click, where this app controls delivery completely — nothing about
//! it depends on an OS notification daemon existing.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

/// The three, and only three, outcomes of a check. Reporting only the happy
/// path is the failure mode this project keeps rediscovering — a menu item
/// that does nothing visible when the network is down is worse than no menu
/// item at all.
///
/// A cancelled check is deliberately *not* one of these: it produces no
/// outcome to report through any channel, which is what [`CheckRun`] exists
/// to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// A newer release was found, the user agreed to it, and it was
    /// downloaded, verified and installed. The app restarts into it right
    /// after this is reported.
    Installing { version: String },
    /// The running version is already current.
    UpToDate,
    /// The check (or the download, or the install) could not complete.
    Failed { reason: String },
}

/// What one run of [`run_check`] ended in.
///
/// The point of the second variant is that it is *not* a `CheckOutcome`.
/// Cancel means nothing was downloaded, nothing was installed and nothing
/// happened that the user needs telling about — they are the one who made it
/// happen, one dialog ago. So there is no notification, no dialog and no
/// menu label for it: `record_run` puts the status straight back to idle,
/// which is also what keeps the re-entrancy guard from wedging the menu item
/// after a Cancel.
#[derive(Debug)]
enum CheckRun {
    Reported(CheckOutcome),
    Cancelled,
}

/// Everything a native message dialog needs, decided in one pure place so
/// the plugin call is a thin wrapper that adds no wording of its own.
///
/// `title` is the alert's bold header and `body` the smaller paragraph under
/// it. Both of this module's dialogs are opened with no parent window — a
/// menu-bar app's popover is hidden most of the time and is nothing to hang a
/// sheet off — and rfd's macOS backend routes the parentless case to
/// `CFUserNotificationDisplayAlert` rather than to `NSAlert`
/// (`rfd-0.16.0/src/backend/macos/utils/user_alert.rs`), where the two
/// strings become the alert header and the alert message. That alert is drawn
/// by the system's `UserNotificationCenter` process rather than by this one,
/// which is why it comes up over whatever the user is doing without an
/// accessory app having to steal focus first. Confirmed by reading the
/// accessibility tree of the live dialog, an `AXSystemDialog` owned by
/// `UserNotificationCenter`: both strings came back exactly as written here.
#[derive(Debug, Clone)]
struct UpdateDialog {
    kind: MessageDialogKind,
    title: String,
    body: String,
    buttons: MessageDialogButtons,
}

/// The confirm dialog, shown before a single byte of the new bundle is
/// fetched. Pure, so the exact wording is pinned by a test rather than only
/// by whatever a reviewer happened to read.
///
/// Both versions are named because "an update is available" is not enough to
/// decide on — a user who has been ignoring the menu for a month wants to
/// know how far behind they are. The restart is stated up front for the same
/// reason: it is the part of this that a user cannot undo, and finding out
/// afterwards is the whole complaint this dialog exists to answer.
///
/// `OkCancelCustom(ok, cancel)` puts the first label on the alert's default
/// button and the second on its alternate. macOS draws the default rightmost
/// and activates it on Return, so Return means Update and the button next to
/// it means Cancel. Escape produces `kCFUserNotificationCancelResponse`,
/// which rfd reports as `MessageDialogResult::Cancel`, which the plugin maps
/// to the *cancel* label before comparing it against the confirm label — so
/// dismissing the alert declines, like every other way out of it that is not
/// a press on Update
/// (`rfd-0.16.0/src/backend/macos/utils/user_alert.rs` `UserAlert::run`,
/// `tauri-plugin-dialog-2.7.3/src/desktop.rs:228-253` and its `lib.rs:328-350`).
fn update_prompt(current_version: &str, new_version: &str) -> UpdateDialog {
    UpdateDialog {
        kind: MessageDialogKind::Info,
        title: "Update available".to_string(),
        body: format!(
            "You have {current_version}. Version {new_version} is available.\n\
             Claude Usage will restart to finish installing."
        ),
        buttons: MessageDialogButtons::OkCancelCustom("Update".to_string(), "Cancel".to_string()),
    }
}

impl CheckOutcome {
    /// The message dialog this outcome puts in front of the user, or `None`
    /// when there is nothing worth interrupting them for. Pure, and the only
    /// place this wording lives.
    ///
    /// `Installing` is the `None`: the confirm dialog the user just clicked
    /// through already said the app would restart, and `spawn_check` calls
    /// `app.restart()` on the next line. A dialog here would either be
    /// destroyed unread by the restart or hold the restart open until
    /// someone clicked it — and the user may well have walked away during
    /// the download, which is exactly why that outcome keeps its system
    /// notification instead (see [`Self::deserves_a_system_notification`]).
    ///
    /// `Failed` is titled "Update failed" rather than "Update check failed"
    /// because this one variant also carries a download or install failure,
    /// raised *after* a check that plainly succeeded — it found the release
    /// and offered it. "Update failed" is true of all three; "Update check
    /// failed" would be a small lie in the case the user cared most about.
    fn dialog(&self) -> Option<UpdateDialog> {
        match self {
            CheckOutcome::Installing { .. } => None,
            CheckOutcome::UpToDate => Some(UpdateDialog {
                kind: MessageDialogKind::Info,
                title: "You're up to date".to_string(),
                body: format!(
                    "Claude Usage {} is the latest version.",
                    env!("CARGO_PKG_VERSION")
                ),
                buttons: MessageDialogButtons::Ok,
            }),
            CheckOutcome::Failed { reason } => Some(UpdateDialog {
                kind: MessageDialogKind::Error,
                title: "Update failed".to_string(),
                body: reason.clone(),
                buttons: MessageDialogButtons::Ok,
            }),
        }
    }

    /// Title and body for the two passive channels: the tray tooltip, which
    /// takes them on every reported outcome, and the system notification,
    /// which takes them only when [`Self::deserves_a_system_notification`]
    /// agrees. See the module doc comment: showing a notification can
    /// silently do nothing, so this is never the only place an outcome is
    /// recorded.
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

    /// Whether this outcome is still worth an OS banner now that a dialog
    /// reports the outcomes the user is present for. Only `Installing` is.
    ///
    /// `UpToDate` and `Failed` each raise a modal dialog the moment the
    /// user's own click produces them; a banner repeating the same sentence
    /// a second later is noise, and it is the *less* reliable of the two.
    /// `Installing` raises no dialog at all, and it is the one outcome that
    /// can arrive minutes after the click, with the user gone and the app
    /// about to relaunch itself — so it keeps the banner.
    ///
    /// Honest caveat, since this module has a history of claiming more than
    /// it delivers: that banner is not merely best-effort, it is raced. The
    /// plugin hands delivery to a detached task and returns immediately
    /// (module doc comment), and `spawn_check` calls `app.restart()` on the
    /// next line, so the process may well be gone before the task is ever
    /// polled. This is exactly the behaviour 0.2.0 shipped; keeping it costs
    /// nothing and is the only signal that outcome has, but nobody should
    /// build on it.
    fn deserves_a_system_notification(&self) -> bool {
        matches!(self, CheckOutcome::Installing { .. })
    }

    /// The `Check for Updates…` menu label while this outcome is showing —
    /// the channel this feature falls back on when nobody saw the dialog.
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
/// a menu should stretch to. The dialog shows the reason in full; this is
/// only the menu's copy of it.
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
/// the channel this feature falls back on.
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
    ///
    /// A check counts as "in flight" while the confirm dialog is open, too,
    /// which is what stops a second click stacking a second dialog on top of
    /// the first.
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

    /// Leaves `Checking` without recording an outcome. The Cancel path, and
    /// the only other way out of `Checking` there is: without it the guard
    /// in `set_checking` would hold forever after a Cancel and the menu item
    /// would never work again. Not `set_done`, because there is no outcome —
    /// a `Done` state would also put a fourth string in the menu label for
    /// thirty seconds, and the label's states are deliberately unchanged.
    fn set_idle(&self) {
        *self.0.lock().unwrap() = CheckState::Idle;
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

/// Where the shared status lands for each way a run can end. Split out of
/// `spawn_check` so both branches — including the Cancel one, which is the
/// branch that has to be right — are reachable from a test without an
/// `AppHandle`, a tray or a network.
fn record_run(status: &UpdateCheckStatus, run: &CheckRun) {
    match run {
        CheckRun::Reported(outcome) => status.set_done(outcome.clone()),
        CheckRun::Cancelled => status.set_idle(),
    }
}

/// Runs one check, asks before installing anything, updates the shared
/// status the menu label reads from, and restarts the app if an update was
/// installed.
///
/// Spawned rather than run inline because `check()`, the confirm dialog and
/// `download_and_install()` are all async while the menu event handler that
/// calls this is not. The status is set to `Checking` synchronously, before
/// the async work is scheduled, and `tray::refresh_menu` is called right
/// after — both here and again once the run has ended, whether it ended in
/// an outcome or in a Cancel — because nothing else rebuilds the menu on
/// demand: `tray::apply` only runs on the poller's own cycle, up to a minute
/// away, and without this the label would sit unchanged until then. That gap
/// — a user-initiated action producing nothing visible until an unrelated
/// timer happens to fire — is exactly the failure mode this module exists to
/// rule out; deriving the label from state (R51) is necessary but was not
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
        let asked = app.clone();
        let run = run_check(&app, move |current_version, new_version| async move {
            ask_to_update(&asked, &current_version, &new_version).await
        })
        .await;
        if let Some(status) = app.try_state::<std::sync::Arc<UpdateCheckStatus>>() {
            record_run(&status, &run);
        }
        crate::tray::refresh_menu(&app);
        // Cancel stops here: the menu label is already back to idle, and
        // there is nothing to notify or to show a dialog about.
        let CheckRun::Reported(outcome) = run else {
            return;
        };
        notify(
            &app,
            &outcome,
            crate::settings::load(&app).notifications_enabled,
        );
        show_outcome_dialog(&app, &outcome);
        if let CheckOutcome::Installing { .. } = outcome {
            app.restart();
        }
    });
}

/// Asks whether to install `new_version`, and answers `false` unless the
/// user actually pressed Update.
///
/// `show` rather than `blocking_show`: the callback form needs no thread of
/// its own and cannot deadlock if this ever moves onto the main thread. The
/// oneshot is what turns it back into something `run_check` can await.
///
/// Every way this can go wrong answers "no". `show` maps the pressed button
/// to `true` only when it equals the confirm label, so Cancel and the
/// Escape key both give `false`; and if the callback is dropped without
/// firing — `run_on_main_thread` failing, the app shutting down — the sender
/// dies with it, `rx` resolves to `Err`, and `unwrap_or(false)` treats that
/// as Cancel as well. There is no path through here that installs without a
/// press on Update.
///
/// Nothing bounds how long this waits: `REQUEST_TIMEOUT` bounds requests, not
/// people, so the re-entrancy guard stays held for as long as the alert is
/// up and further clicks on the menu item do nothing. That is the intended
/// trade rather than the wedge `REQUEST_TIMEOUT` exists to prevent — what is
/// holding the guard here is a system alert sitting in front of the user with
/// two buttons on it, not a silent stall with no way out.
async fn ask_to_update<R: tauri::Runtime>(
    app: &AppHandle<R>,
    current_version: &str,
    new_version: &str,
) -> bool {
    let prompt = update_prompt(current_version, new_version);
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .message(prompt.body)
        .title(prompt.title)
        .kind(prompt.kind)
        .buttons(prompt.buttons)
        .show(move |confirmed| {
            let _ = tx.send(confirmed);
        });
    rx.await.unwrap_or(false)
}

/// Puts `outcome`'s message dialog on screen, if it has one. Fire and
/// forget: nothing downstream waits on the user acknowledging it, and the
/// dialog carries a single OK button precisely because there is nothing left
/// to decide by the time it appears.
fn show_outcome_dialog<R: tauri::Runtime>(app: &AppHandle<R>, outcome: &CheckOutcome) {
    let Some(dialog) = outcome.dialog() else {
        return;
    };
    app.dialog()
        .message(dialog.body)
        .title(dialog.title)
        .kind(dialog.kind)
        .buttons(dialog.buttons)
        .show(|_| {});
}

/// Shows a system notification for `outcome`, if the outcome is one a banner
/// still adds anything to (see `deserves_a_system_notification`) and the
/// user's `notifications_enabled` setting allows it, and echoes the same
/// text onto the tray icon's tooltip.
///
/// Both are courtesies on top of the real channels, which are now the dialog
/// (`show_outcome_dialog`) and, behind it, the `Check for Updates…` menu
/// label (`UpdateCheckStatus`, already rebuilt into the menu via
/// `tray::refresh_menu` by the time `spawn_check` calls this). Neither call
/// here can report whether anything actually became visible (see the module
/// doc comment), so neither is something the user should have to rely on to
/// learn the outcome. On Linux, the tooltip echo is itself a documented
/// no-op in the `tray-icon` crate's GTK/AppIndicator backend (confirmed by
/// reading `tray-icon-0.24.2/src/platform_impl/gtk/mod.rs`, whose
/// `set_tooltip` always returns `Ok(())` without doing anything); on macOS
/// it genuinely sets the native tooltip.
///
/// The tooltip is set for every reported outcome, unlike the banner: it
/// interrupts nobody, it survives the dialog being dismissed, and gating it
/// was never what "the banner is noise now" was about.
fn notify<R: tauri::Runtime>(
    app: &AppHandle<R>,
    outcome: &CheckOutcome,
    notifications_enabled: bool,
) {
    let (title, body) = outcome.notification();
    // One switch, honoured the same way the poller honours it: with
    // notifications off, nothing is posted to the OS from here either. Only
    // this call is gated — the dialog and the menu label are the channels
    // this feature depends on and stay unconditional, and so does the
    // tooltip echo below, which is part of the tray this app draws rather
    // than something handed to a notification daemon.
    if notifications_enabled && outcome.deserves_a_system_notification() {
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
/// Five minutes rather than something snappier because the download has to
/// fit inside it as well as the check, and the 0.1.0 bundle is ~6 MB — a
/// short timeout would abort legitimate slow downloads. The common failures
/// (no route, DNS, refused connection) return in seconds regardless; this
/// only bounds the rare stalled-mid-transfer case.
///
/// Covering the download takes an explicit assignment in `run_check`, which
/// is easy to mistake for redundancy and is not: tauri-plugin-updater 2.11.0
/// threads `UpdaterBuilder::timeout` into `Updater` and uses it for the
/// metadata fetch (`updater.rs:388` and `:504`), but then constructs the
/// returned `Update` with a hard-coded `timeout: None` (`updater.rs:595`).
/// The download's own `if let Some(timeout) = self.timeout`
/// (`updater.rs:698`) therefore reads `None` unless the caller sets the
/// public field back, and a stalled download would run unbounded — the exact
/// wedge this constant exists to prevent, in the half of the flow where a
/// stall is most likely.
///
/// **That assignment is not covered by any test, and deleting it leaves the
/// suite green** (checked, not assumed: 238 passed with the line removed).
/// This module has a history of claiming coverage it did not have, so the gap
/// is written down rather than left to be rediscovered. It is not laziness —
/// the plugin's `Update` carries two private fields, `extract_path` and
/// `context` (`updater.rs:669-671`), and exposes no constructor, so a test
/// cannot build one to assert the field on. The only route that reaches a real
/// `Update` is a wiremock end-to-end run, and observing the timeout there means
/// stalling a download for the full 300 seconds. Both costs are worse than the
/// note. If you delete the line as redundant, nothing will fail; re-read the
/// paragraph above before you do.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// One check, from the network request to the install, with the "do you want
/// this?" step passed in so a test can answer it.
///
/// `confirm` receives the running version and the offered one, and its
/// answer gates the *only* `download_and_install` call site in this crate.
/// A `false` returns before `update` is touched again, so no bundle request
/// is ever made — that is the property the module's two wiremock tests pin
/// from opposite directions.
async fn run_check<R, C, Fut>(app: &AppHandle<R>, confirm: C) -> CheckRun
where
    R: tauri::Runtime,
    C: FnOnce(String, String) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let updater = match app.updater_builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(updater) => updater,
        Err(e) => {
            return CheckRun::Reported(CheckOutcome::Failed {
                reason: e.to_string(),
            })
        }
    };
    match updater.check().await {
        Ok(Some(mut update)) => {
            if !confirm(update.current_version.clone(), update.version.clone()).await {
                return CheckRun::Cancelled;
            }
            // See `REQUEST_TIMEOUT`: the plugin drops the builder's timeout
            // when it constructs this `Update`, so the download is unbounded
            // until it is put back.
            update.timeout = Some(REQUEST_TIMEOUT);
            let version = update.version.clone();
            match update
                .download_and_install(|_chunk_len, _total| {}, || {})
                .await
            {
                Ok(()) => CheckRun::Reported(CheckOutcome::Installing { version }),
                Err(e) => CheckRun::Reported(CheckOutcome::Failed {
                    reason: e.to_string(),
                }),
            }
        }
        Ok(None) => CheckRun::Reported(CheckOutcome::UpToDate),
        Err(e) => CheckRun::Reported(CheckOutcome::Failed {
            reason: e.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A mock app with the updater plugin registered and pointed at
    /// `endpoint`. `mock_context` leaves every plugin's config section
    /// absent, which the updater plugin's mandatory (non-`Option`) `Config`
    /// cannot deserialize from — so this fills in the two fields that matter
    /// for building an `Updater`. The pubkey is never validated at this
    /// stage, only when a fetched signature is actually verified, so any
    /// string does here.
    fn mock_app_with_updater(endpoint: &str) -> tauri::App<tauri::test::MockRuntime> {
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().plugins.0.insert(
            "updater".into(),
            serde_json::json!({
                "pubkey": "test",
                "endpoints": [endpoint]
            }),
        );
        tauri::test::mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .expect("failed to build mock app with the updater plugin registered")
    }

    /// The manifest an update endpoint serves. Deliberately the "dynamic"
    /// shape — a top-level `url`/`signature` rather than a `platforms` map —
    /// because `RemoteRelease::download_url` hands that back for any target
    /// (`tauri-plugin-updater-2.11.0/src/updater.rs:105-114`), so these tests
    /// mean the same thing on the macOS and Linux halves of CI. The version
    /// is far beyond anything this crate will ship, so `check()` always
    /// reports an update rather than `UpToDate`.
    fn available_update(bundle_url: &str) -> serde_json::Value {
        serde_json::json!({
            "version": "99.0.0",
            "notes": "a release that does not exist",
            "pub_date": "2026-01-01T00:00:00Z",
            "url": bundle_url,
            "signature": "not-a-real-signature",
        })
    }

    /// **The one that matters.** Cancel must download nothing.
    ///
    /// Not a test of the pure decision function: this drives the real
    /// `run_check` against a real `tauri-plugin-updater`, over a real HTTP
    /// server that has a real bundle route mounted, and asserts the server
    /// never saw a request for it. Anyone who moves the
    /// `download_and_install` call out from behind the `confirm` answer —
    /// above it, into a `let` that runs first, into a spawned task that
    /// races it — makes this fail, because the bytes would go over the wire.
    ///
    /// The `Cancelled` assertion is not decoration either: without it the
    /// test would also pass if the manifest stopped parsing and the run
    /// failed before ever reaching the prompt. `Cancelled` is only reachable
    /// through `Ok(Some(update))` followed by a `false` answer, so it proves
    /// the run really did get as far as the decision this is about. The twin
    /// below proves the same wiring downloads when the answer is `true`.
    #[tokio::test]
    async fn cancelling_the_update_prompt_downloads_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/latest.json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(available_update(&format!("{}/bundle.tar.gz", server.uri()))),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/bundle.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"not a real bundle".to_vec()))
            // Belt and braces: `MockServer` verifies this on drop, so even a
            // download that somehow escaped the assertion below still fails
            // the test.
            .expect(0)
            .mount(&server)
            .await;

        let app = mock_app_with_updater(&format!("{}/latest.json", server.uri()));
        let run = run_check(app.handle(), |_current, _new| async { false }).await;

        assert!(
            matches!(run, CheckRun::Cancelled),
            "a declined prompt must end the run as Cancelled, got {run:?}"
        );
        let bundle_requests = server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|request| request.url.path() == "/bundle.tar.gz")
            .count();
        assert_eq!(
            bundle_requests, 0,
            "Cancel downloaded the bundle anyway: {bundle_requests} request(s) for it"
        );
    }

    /// The twin, and the reason the Cancel test is not vacuous: the same
    /// wiring, the same manifest, the same bundle route — and a `true`
    /// answer does fetch it. Without this, a `run_check` that had stopped
    /// downloading under every condition would pass the test above.
    ///
    /// It ends in `Failed` because the fake signature does not verify.
    /// `Update::download` reads the whole body before calling
    /// `verify_signature` and `install` runs only after that returns
    /// (`tauri-plugin-updater-2.11.0/src/updater.rs:740` and `:766-767`), so
    /// this test downloads seventeen bytes and writes nothing to disk.
    #[tokio::test]
    async fn confirming_the_update_prompt_does_download() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/latest.json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(available_update(&format!("{}/bundle.tar.gz", server.uri()))),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/bundle.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"not a real bundle".to_vec()))
            .expect(1)
            .mount(&server)
            .await;

        let app = mock_app_with_updater(&format!("{}/latest.json", server.uri()));
        let run = run_check(app.handle(), |_current, _new| async { true }).await;

        assert!(
            matches!(run, CheckRun::Reported(CheckOutcome::Failed { .. })),
            "an unsigned fake bundle must fail verification, got {run:?}"
        );
        let bundle_requests = server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|request| request.url.path() == "/bundle.tar.gz")
            .count();
        assert_eq!(
            bundle_requests, 1,
            "an accepted prompt must fetch the bundle exactly once"
        );
    }

    /// The prompt is handed both versions it was given, and says the restart
    /// out loud. Anyone who drops the "will restart" sentence — the part the
    /// user cannot undo and the reason this dialog exists — fails here.
    #[test]
    fn the_update_prompt_names_both_versions_and_warns_about_the_restart() {
        let prompt = update_prompt("0.1.0", "0.2.0");
        assert_eq!(prompt.title, "Update available");
        assert_eq!(
            prompt.body,
            "You have 0.1.0. Version 0.2.0 is available.\n\
             Claude Usage will restart to finish installing."
        );
        assert_eq!(prompt.kind, MessageDialogKind::Info);
    }

    /// The confirm button says what it does, and the cancel button is the
    /// second of the pair — which is what makes it the one Escape maps to
    /// and the one that is *not* the default. Swapping the two arguments in
    /// `update_prompt` would make Return install; that is what this catches.
    #[test]
    fn the_update_prompt_confirms_with_update_and_declines_with_cancel() {
        let prompt = update_prompt("0.1.0", "0.2.0");
        match prompt.buttons {
            MessageDialogButtons::OkCancelCustom(confirm, cancel) => {
                assert_eq!(confirm, "Update");
                assert_eq!(cancel, "Cancel");
            }
            other => panic!("the prompt must offer a confirm/cancel pair, got {other:?}"),
        }
    }

    #[test]
    fn an_up_to_date_check_says_so_in_a_dialog_and_names_the_running_version() {
        let dialog = CheckOutcome::UpToDate
            .dialog()
            .expect("an up-to-date check must report itself");
        assert_eq!(dialog.title, "You're up to date");
        assert_eq!(
            dialog.body,
            format!(
                "Claude Usage {} is the latest version.",
                env!("CARGO_PKG_VERSION")
            )
        );
        assert_eq!(dialog.kind, MessageDialogKind::Info);
        assert!(matches!(dialog.buttons, MessageDialogButtons::Ok));
    }

    /// The exact case this module exists to prevent: a check that fails
    /// silently. Whatever the reason, it must reach the user — in full, not
    /// truncated the way the menu label has to truncate it.
    #[test]
    fn a_failed_check_puts_the_whole_reason_in_a_dialog() {
        let reason = "error sending request for url (https://example.com/latest.json)";
        let dialog = CheckOutcome::Failed {
            reason: reason.into(),
        }
        .dialog()
        .expect("a failed check must report itself");
        assert_eq!(dialog.title, "Update failed");
        assert_eq!(dialog.body, reason);
        assert_eq!(dialog.kind, MessageDialogKind::Error);
    }

    /// `Installing` is the outcome with no dialog, and that is a decision
    /// rather than an omission: `spawn_check` restarts the app on the next
    /// line, so a dialog here would be destroyed unread or would hold the
    /// restart open indefinitely.
    #[test]
    fn an_installing_outcome_shows_no_dialog_because_the_app_is_about_to_restart() {
        assert!(CheckOutcome::Installing {
            version: "0.2.0".into()
        }
        .dialog()
        .is_none());
    }

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
    /// shaped like the real `tauri.conf.json`.
    ///
    /// This does *not* by itself demonstrate ACL-freedom: `app.updater()`
    /// was never routed through `invoke_handler` to begin with, so there
    /// was no capability gate for this call to pass through in the first
    /// place, mocked runtime or real one. The ACL argument is architectural
    /// (see the module doc comment above) — `UpdaterExt` is a blanket impl
    /// for `Manager<R>`, called directly from a menu event handler, never
    /// through `invoke()` — not something this test exercises. The same is
    /// true of `DialogExt`.
    #[test]
    fn the_updater_plugin_registers_with_a_valid_config() {
        let app = mock_app_with_updater("https://example.com/latest.json");
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
    ///
    /// `Installing` rather than `UpToDate`, because `UpToDate` no longer
    /// posts a banner under any setting — this test would pass on a broken
    /// switch if it used one of the outcomes the dialog reports.
    #[test]
    fn the_system_notification_is_suppressed_when_notifications_are_off() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app");
        notify(
            app.handle(),
            &CheckOutcome::Installing {
                version: "0.2.0".into(),
            },
            false,
        );
    }

    /// The other half: with notifications on, `notify` still goes to the
    /// notification plugin for the one outcome that keeps its banner.
    /// Without this, a `notify` that had quietly stopped notifying anyone at
    /// all would pass the test above.
    #[test]
    #[should_panic(expected = "state() called before manage()")]
    fn the_system_notification_is_attempted_when_notifications_are_on() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app");
        notify(
            app.handle(),
            &CheckOutcome::Installing {
                version: "0.2.0".into(),
            },
            true,
        );
    }

    /// The outcomes a dialog reports get no banner even with notifications
    /// on — the whole point of `deserves_a_system_notification`. Same
    /// observability trick, read the other way: reaching the notification
    /// plugin here *would* panic, so not panicking is the assertion.
    #[test]
    fn the_outcomes_a_dialog_reports_get_no_system_notification() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app");
        notify(app.handle(), &CheckOutcome::UpToDate, true);
        notify(
            app.handle(),
            &CheckOutcome::Failed {
                reason: "boom".into(),
            },
            true,
        );
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
    // for a test to observe there even in principle. The same goes for the
    // dialogs: `show_message_dialog` dispatches to the main thread and hands
    // the window to `rfd`, so what a test can pin is the wording and the
    // buttons, which is why those live in `update_prompt` and
    // `CheckOutcome::dialog` rather than inline at the call sites.

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

    /// The timeout has to outlast a whole bundle download, not just a metadata
    /// fetch, because `run_check` applies it to both. Written as
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

    /// Two clicks on `Check for Updates…` in quick succession used to give
    /// two concurrent `download_and_install` calls writing the same
    /// `/Applications` bundle, and then two `app.restart()`. `spawn_check`
    /// needs an `AppHandle`, so the guard lives here in the state machine
    /// where it can be tested: entering `Checking` twice must report one
    /// transition, not two.
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

    /// A Cancel returns the state machine to idle, which is the second half
    /// of getting Cancel right: the guard that stops a second click during a
    /// check would otherwise still be holding, and the menu item would be
    /// dead for the rest of the session.
    ///
    /// Driven through `record_run` — the same function `spawn_check` calls
    /// with the same value `run_check` returns — rather than through
    /// `set_idle` directly, so a `record_run` that sent Cancel down the
    /// `set_done` branch fails here too.
    #[test]
    fn a_cancelled_run_returns_the_state_machine_to_idle_and_lets_the_next_click_through() {
        let status = UpdateCheckStatus::default();
        assert!(status.set_checking(), "the click must start a check");
        record_run(&status, &CheckRun::Cancelled);
        assert_eq!(
            status.label(),
            "Check for Updates…",
            "a cancelled check must leave the menu label as it found it"
        );
        assert!(
            status.set_checking(),
            "after a Cancel the next click must start a new check"
        );
    }

    /// The other branch of the same function, so the pair pins which run
    /// ends where: a reported outcome is the one that sticks in the label.
    #[test]
    fn a_reported_run_leaves_its_outcome_in_the_menu_label() {
        let status = UpdateCheckStatus::default();
        status.set_checking();
        record_run(&status, &CheckRun::Reported(CheckOutcome::UpToDate));
        assert_eq!(
            status.label(),
            format!("Up to date (v{})", env!("CARGO_PKG_VERSION"))
        );
    }

    /// The requirement R51 exists for: `tray::apply` rebuilds the whole
    /// menu whenever its content moves, so if the label were derived once
    /// and baked into the menu rather than read fresh from this state, the
    /// next rebuild — which a poll a few seconds later can trigger — would
    /// silently wipe it. Reading the label twice in a row, simulating two
    /// such rebuilds while the outcome is still fresh, must return the same
    /// text both times.
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
