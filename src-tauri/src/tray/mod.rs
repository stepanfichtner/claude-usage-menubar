//! The half of the tray that talks to Tauri: building the icon, pushing
//! titles and menus at it, the click and menu-event handlers, and the two
//! pieces of managed state the rebuilds render from. The strings themselves
//! come from `render`, which knows nothing about any of this.

mod render;
pub use render::*;

use chrono::{DateTime, Utc};

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use crate::model::UsageSnapshot;

pub const TRAY_ID: &str = "main";

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app, &[], &check_updates_label(app))?;
    // On non-macOS platforms nothing below reads `tray` again — the icon is
    // set once at build time and never switched to a template afterward.
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    let tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(tauri::image::Image::from_bytes(IconKind::Neutral.bytes())?)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(|tray, event| {
            tauri_plugin_positioner::on_tray_event(tray.app_handle(), &event);
            if let tauri::tray::TrayIconEvent::Click {
                button,
                button_state,
                ..
            } = event
            {
                if is_toggle_click(button, button_state) {
                    toggle_popover(tray.app_handle());
                }
            }
        })
        .build(app)?;

    // macOS renders coloured tray icons only when template mode is off (spec §8.2).
    #[cfg(target_os = "macos")]
    tray.set_icon_as_template(false)?;

    Ok(())
}

/// The `Check for Updates…` row's label right now. Falls back to the idle
/// text when the status was never managed, which no production path does.
///
/// Read by the callers rather than inside `build_menu`, because the two
/// varying inputs to a menu — this and the quota lines — are also what
/// `LastMenu` compares to decide whether a rebuild would change anything.
/// Reading it here keeps that comparison looking at the same string the
/// rebuild would render.
fn check_updates_label<R: Runtime>(app: &AppHandle<R>) -> String {
    app.try_state::<std::sync::Arc<crate::updater::UpdateCheckStatus>>()
        .map(|status| status.label())
        .unwrap_or_else(|| "Check for Updates…".to_string())
}

fn build_menu<R: Runtime>(
    app: &AppHandle<R>,
    labels: &[String],
    check_updates_label: &str,
) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    for (index, label) in labels.iter().enumerate() {
        let item = MenuItem::with_id(app, format!("quota-{index}"), label, false, None::<&str>)?;
        menu.append(&item)?;
    }
    if !labels.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    for row in trailing_rows(check_updates_label.to_string()) {
        match row {
            MenuRow::Item { id, label, enabled } => {
                menu.append(&MenuItem::with_id(app, id, label, enabled, None::<&str>)?)?;
            }
            MenuRow::Separator => {
                menu.append(&PredefinedMenuItem::separator(app)?)?;
            }
        }
    }
    Ok(menu)
}

/// The most recently rendered quota lines — `menu_labels(&snapshot.quotas,
/// now)` — held so a menu rebuild triggered without a fresh `UsageSnapshot`
/// (an update check starting or finishing; see `refresh_menu`) can still
/// show them, rather than blanking the top of the menu. Empty before the
/// first poll, which is exactly what the menu already looks like at launch
/// — not a hazard to guard against, just the normal startup state.
#[derive(Default)]
pub struct LastQuotaLines(std::sync::Mutex<Vec<String>>);

impl LastQuotaLines {
    fn set(&self, lines: Vec<String>) {
        *self.0.lock().unwrap() = lines;
    }

    fn get(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// The quota lines to render right now: the last ones `apply` stored, or an
/// empty list if none have been stored yet (no snapshot has arrived, or the
/// state was never managed — same result either way, and neither is an
/// error). Split out from `refresh_menu` so it can be tested without
/// touching `muda`: reading managed state needs no main-thread dispatch,
/// unlike constructing a `Menu`.
fn current_quota_lines<R: Runtime>(app: &AppHandle<R>) -> Vec<String> {
    app.try_state::<std::sync::Arc<LastQuotaLines>>()
        .map(|lines| lines.get())
        .unwrap_or_default()
}

/// What the tray menu currently shows, as the pair `build_menu` was last
/// called with. Held so `apply` can skip a rebuild that would produce a
/// byte-identical menu — while signed out there are no quota lines at all
/// and nothing else changes either, so every poll was reconstructing the
/// same seven native menu items and handing them to the tray.
///
/// The pair is the whole of it: `build_menu`'s only varying inputs are the
/// quota lines and the `Check for Updates…` label, and everything else it
/// renders is a literal. The label has to be in here rather than left out as
/// "not really menu content" — it expires from an outcome back to idle after
/// `updater::OUTCOME_VISIBLE_FOR`, and the poll-driven rebuild is what
/// `updater` relies on to notice (see the comment on that constant). Skipping
/// on unchanged quota lines alone would leave a finished check's result in
/// the menu until the numbers happened to move.
#[derive(Default)]
pub struct LastMenu(std::sync::Mutex<Option<(Vec<String>, String)>>);

impl LastMenu {
    /// Records what is about to be rendered and reports whether it differs
    /// from what was rendered before. `true` the first time, so the menu
    /// after launch is always built.
    fn changed(&self, lines: &[String], check_updates_label: &str) -> bool {
        let rendering = (lines.to_vec(), check_updates_label.to_string());
        let mut last = self.0.lock().unwrap();
        if last.as_ref() == Some(&rendering) {
            return false;
        }
        *last = Some(rendering);
        true
    }
}

/// The last snapshot `apply` rendered, held so the menu-bar title can be
/// rebuilt from it without a fresh one. Which figures the title shows is a
/// display setting: changing it needs no new numbers, only the numbers
/// already in hand. Empty until the first `apply` — the launch state, where
/// there is genuinely nothing to re-render.
#[derive(Default)]
pub struct LastSnapshot(std::sync::Mutex<Option<UsageSnapshot>>);

impl LastSnapshot {
    fn set(&self, snapshot: UsageSnapshot) {
        *self.0.lock().unwrap() = Some(snapshot);
    }

    fn get(&self) -> Option<UsageSnapshot> {
        self.0.lock().unwrap().clone()
    }
}

/// The snapshot to re-render from, or `None` if none has been stored yet (no
/// snapshot has arrived, or the state was never managed — same answer either
/// way, and neither is an error). Split out for the same reason
/// `current_quota_lines` is: reading managed state needs no main-thread
/// dispatch, unlike anything that touches a real tray.
fn current_snapshot<R: Runtime>(app: &AppHandle<R>) -> Option<UsageSnapshot> {
    app.try_state::<std::sync::Arc<LastSnapshot>>()
        .and_then(|last| last.get())
}

/// The menu-bar title for `snapshot` under the settings in force right now.
/// The single definition of that, used both by `apply` as a snapshot arrives
/// and by `refresh_title` when the settings change under a stored one, so the
/// two can never come to disagree about what the title should say.
fn title_for<R: Runtime>(
    app: &AppHandle<R>,
    snapshot: &UsageSnapshot,
    now: DateTime<Utc>,
) -> String {
    let entries = crate::settings::load(app).title_entries;
    render_title(&snapshot.quotas, &entries, now)
}

/// Re-renders the menu-bar title from the last snapshot `apply` stored, with
/// no fetch of any kind. The sibling of `refresh_menu`, and here for the same
/// reason it is: a user-initiated change has to produce a visible result when
/// it is made, not when an unrelated timer happens to fire. There the change
/// was an update check and the timer was `apply`'s next run; here the change
/// is a settings save and the timer is the poller's next cycle — up to ten
/// minutes away on the longest poll interval the settings window offers. The
/// pairing is the one R51 and R52 arrived at together: state to render from,
/// and a rebuild forced at the moment that state changes, because either one
/// alone leaves the same silence.
///
/// Returns the title it rendered, or `None` when nothing has been stored yet
/// and there is nothing to render. It means "this is the title that was
/// computed", not "the menu bar now shows it": a `Some` comes back even when
/// the lookup below finds no tray to push it at. Returned because a test has
/// no other way to see it: `MockRuntime` has no tray to read a title back
/// from — the same `muda` constraint `current_quota_lines` exists for — so
/// without this, a save that re-rendered the right title and a save that
/// re-rendered nothing at all would look identical from the outside.
///
/// Not race-free, and knowingly so. The poller is the other writer of this
/// title, and it computes its own with `title_for` a moment before pushing
/// it, so a save committing inside that window pushes the new title first
/// and the poller's pre-save one lands second — the reported symptom again,
/// until the next poll redraws it. The window is microseconds wide and opens
/// once per poll interval; a generation counter, or rendering under the
/// snapshot lock, would close it, and neither earns its machinery here.
pub fn refresh_title<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    let snapshot = current_snapshot(app)?;
    let title = title_for(app, &snapshot, Utc::now());
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_title(Some(title.clone()));
    }
    Some(title)
}

/// Push a fresh snapshot into the tray: icon, title, and menu rows.
///
/// Generic over the Tauri runtime for the same reason `settings::load` is.
/// Every production call site — the poller's three — passes a concrete
/// `&AppHandle` (Wry) and is unchanged by it; `MockRuntime` has no tray, so
/// everything below the lookup is skipped there, which is what lets the
/// storing above it be driven from a test.
pub fn apply<R: Runtime>(app: &AppHandle<R>, snapshot: &UsageSnapshot) {
    // Recorded before the tray lookup, and before anything else can fail:
    // this is the one point all three of the poller's render paths pass
    // through — the cached replay at startup, a published poll, and the empty
    // snapshot the signed-out path renders — so storing here is what keeps
    // `refresh_title` rendering the numbers actually on screen rather than an
    // older set. Having no tray to render into is not a reason to forget what
    // the latest snapshot was, and putting it above the lookup is also what
    // lets it be driven under `MockRuntime`, which has none.
    if let Some(last) = app.try_state::<std::sync::Arc<LastSnapshot>>() {
        last.set(snapshot.clone());
    }
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let now = Utc::now();

    if let Ok(image) =
        tauri::image::Image::from_bytes(IconKind::for_quotas(&snapshot.quotas).bytes())
    {
        let _ = tray.set_icon(Some(image));
    }
    let _ = tray.set_title(Some(title_for(app, snapshot, now)));
    let labels = menu_labels(&snapshot.quotas, now);
    if let Some(lines) = app.try_state::<std::sync::Arc<LastQuotaLines>>() {
        lines.set(labels.clone());
    }
    // Only rebuild when the result would differ. An unmanaged `LastMenu`
    // means "rebuild every time", which is what this did before.
    let check_updates = check_updates_label(app);
    let changed = app
        .try_state::<std::sync::Arc<LastMenu>>()
        .is_none_or(|last| last.changed(&labels, &check_updates));
    if changed {
        if let Ok(menu) = build_menu(app, &labels, &check_updates) {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

/// Rebuilds the menu from the last rendered quota lines, without needing a
/// fresh `UsageSnapshot`. Used when the update-check status changes, so the
/// outcome appears the moment it changes rather than waiting for the
/// poller's next cycle to rebuild the menu incidentally. The update-check
/// label itself is read fresh inside `build_menu`, same as any other
/// rebuild.
pub fn refresh_menu(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let labels = current_quota_lines(app);
    let check_updates = check_updates_label(app);
    // Unconditional: this is only called because something already changed.
    // The record is still updated, so `apply`'s skip stays a true statement
    // about what the tray is showing rather than about what `apply` last
    // pushed.
    if let Some(last) = app.try_state::<std::sync::Arc<LastMenu>>() {
        last.changed(&labels, &check_updates);
    }
    if let Ok(menu) = build_menu(app, &labels, &check_updates) {
        let _ = tray.set_menu(Some(menu));
    }
}

/// Whether a tray click event should toggle the popover.
///
/// `TrayIconEvent::Click` fires once for the press (`button_state: Down`) and
/// once for the release (`Up`) — matching on the button alone toggles twice
/// per click (open on press, closed on release), so the panel only stayed
/// visible while the mouse button was held down. Acting on `Up` only also
/// matches platform convention: pressing the icon and dragging away cancels
/// instead of opening the panel.
fn is_toggle_click(
    button: tauri::tray::MouseButton,
    button_state: tauri::tray::MouseButtonState,
) -> bool {
    button == tauri::tray::MouseButton::Left && button_state == tauri::tray::MouseButtonState::Up
}

fn toggle_popover(app: &AppHandle) {
    let Some(window) = app.get_webview_window("popover") else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
    } else {
        #[cfg(target_os = "macos")]
        {
            use tauri_plugin_positioner::{Position, WindowExt};
            let _ = window.move_window(Position::TrayCenter);
        }
        #[cfg(not(target_os = "macos"))]
        {
            use tauri_plugin_positioner::{Position, WindowExt};
            let _ = window.move_window(Position::Center);
        }
        let _ = window.show();
        let _ = window.set_focus();
        // Spec §7: opening the popover refreshes, under the same manual-refresh throttle.
        if let Some(signal) = app.try_state::<std::sync::Arc<crate::poller::RefreshSignal>>() {
            signal.request();
        }
    }
}

fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        "open" => toggle_popover(app),
        "refresh" => {
            if let Some(signal) = app.try_state::<std::sync::Arc<crate::poller::RefreshSignal>>() {
                signal.request();
            }
        }
        "settings" => {
            if let Some(window) = app.get_webview_window("settings") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "check_updates" => crate::updater::spawn_check(app.clone()),
        "quit" => app.exit(0),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Quota, Severity};
    use chrono::{TimeZone, Utc};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 7, 9, 0, 0).unwrap()
    }

    fn quota(id: &str, percent: f64, minutes_ahead: i64) -> Quota {
        Quota {
            id: id.into(),
            label: format!("Label {id}"),
            percent,
            severity: Severity::from_percent(percent),
            resets_at: Some(now() + chrono::Duration::minutes(minutes_ahead)),
            is_active: false,
        }
    }

    #[test]
    fn only_a_left_click_release_toggles_the_popover() {
        use tauri::tray::{MouseButton, MouseButtonState};

        assert!(is_toggle_click(MouseButton::Left, MouseButtonState::Up));
        // The press, not just the release, used to also match — toggling the
        // panel open and shut again before the button was let go.
        assert!(!is_toggle_click(MouseButton::Left, MouseButtonState::Down));
        assert!(!is_toggle_click(MouseButton::Right, MouseButtonState::Up));
        assert!(!is_toggle_click(MouseButton::Middle, MouseButtonState::Up));
    }

    /// Before the first poll (or if the state was somehow never managed),
    /// there is nothing to show at the top of the menu — that must resolve
    /// to an empty list, not an error, exactly as it already does today at
    /// launch, before `apply` has run even once.
    #[test]
    fn no_stored_lines_yields_an_empty_list_rather_than_an_error() {
        let app = tauri::test::mock_builder()
            .manage(std::sync::Arc::new(LastQuotaLines::default()))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app");
        assert_eq!(current_quota_lines(app.handle()), Vec::<String>::new());
    }

    /// R52's actual point: a rebuild triggered by an update check — which
    /// has no `UsageSnapshot` of its own — must still show whatever quota
    /// lines the last poll rendered, read twice here to simulate the
    /// `set_checking` and `set_done` rebuilds both happening between polls.
    #[test]
    fn stored_lines_are_returned_without_needing_a_fresh_poll() {
        let app = tauri::test::mock_builder()
            .manage(std::sync::Arc::new(LastQuotaLines::default()))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app");
        let lines = app.handle().state::<std::sync::Arc<LastQuotaLines>>();
        lines.set(vec!["Label session — 20% · in 3h 58m".to_string()]);

        assert_eq!(
            current_quota_lines(app.handle()),
            vec!["Label session — 20% · in 3h 58m".to_string()]
        );
        // Read again without anything having polled in between.
        assert_eq!(
            current_quota_lines(app.handle()),
            vec!["Label session — 20% · in 3h 58m".to_string()]
        );
    }

    /// The first menu after launch has nothing to compare against and must
    /// be built, and an unchanged one after it must not be. That second
    /// assertion is the whole point: while signed out there are no quota
    /// lines to change, so every poll was rebuilding seven identical native
    /// menu items and handing them to the tray.
    #[test]
    fn an_unchanged_menu_is_not_rebuilt_after_the_first_one() {
        let last = LastMenu::default();
        assert!(last.changed(&[], "Check for Updates…"));
        assert!(!last.changed(&[], "Check for Updates…"));
        assert!(!last.changed(&[], "Check for Updates…"));
    }

    #[test]
    fn a_changed_quota_line_rebuilds() {
        let last = LastMenu::default();
        last.changed(&["Label session — 20% · in 3h 58m".to_string()], "Check…");
        assert!(last.changed(&["Label session — 21% · in 3h 57m".to_string()], "Check…"));
    }

    /// The case that decides whether the label belongs in the comparison at
    /// all. `updater::UpdateCheckStatus` expires a finished check's label
    /// back to idle lazily, and the poll-driven rebuild is what it relies on
    /// to notice — so a skip that watched only the quota lines would leave
    /// "Up to date" sitting in the menu until the numbers happened to move.
    #[test]
    fn a_changed_check_updates_label_rebuilds_even_with_identical_quota_lines() {
        let lines = ["Label session — 20% · in 3h 58m".to_string()];
        let last = LastMenu::default();
        last.changed(&lines, "Up to date");
        assert!(last.changed(&lines, "Check for Updates…"));
    }

    /// Every snapshot `apply` renders lands in `LastSnapshot`, and each one
    /// replaces the last — including the empty snapshot the signed-out path
    /// renders, which is the second half here. `apply` is the one point all
    /// three of the poller's render paths pass through (the cached replay at
    /// startup, a published poll, that signed-out blank), so storing in it is
    /// what covers all three. No save happens in this test: what a save then
    /// makes of the stored snapshot is
    /// `lib::tests::a_save_after_signing_out_does_not_bring_back_the_old_numbers`'s
    /// to pin.
    ///
    /// Runs against a `MockRuntime` app, which has no tray, so `apply`
    /// returns at the lookup — the store therefore has to happen above it,
    /// and this fails if it moves below.
    #[test]
    fn apply_stores_every_snapshot_it_renders_including_the_signed_out_one() {
        let app = tauri::test::mock_builder()
            .manage(std::sync::Arc::new(LastSnapshot::default()))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app");

        let polled = UsageSnapshot {
            quotas: vec![quota("session", 42.0, 200)],
            fetched_at: now(),
            stale: false,
        };
        apply(app.handle(), &polled);
        assert_eq!(current_snapshot(app.handle()), Some(polled));

        let signed_out = UsageSnapshot {
            quotas: Vec::new(),
            fetched_at: now(),
            stale: false,
        };
        apply(app.handle(), &signed_out);
        assert_eq!(
            current_snapshot(app.handle()),
            Some(signed_out),
            "the signed-out render must replace the numbers, not leave them behind"
        );
    }
}
