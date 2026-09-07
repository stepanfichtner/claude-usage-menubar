use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{Quota, Severity};

/// One segment of the menu bar title (spec §8.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleEntry {
    pub quota_id: String,
    pub show_percent: bool,
    pub show_countdown: bool,
}

impl TitleEntry {
    pub fn defaults() -> Vec<TitleEntry> {
        vec![
            TitleEntry {
                quota_id: "session".into(),
                show_percent: true,
                show_countdown: true,
            },
            TitleEntry {
                quota_id: "weekly_all".into(),
                show_percent: true,
                show_countdown: true,
            },
        ]
    }
}

/// Menu bar: `49m`, `1h49m`, `6d13h`.
pub fn format_compact(until: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let minutes = (until - now).num_minutes();
    if minutes <= 0 {
        "now".to_string()
    } else if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 24 * 60 {
        format!("{}h{}m", minutes / 60, minutes % 60)
    } else {
        format!("{}d{}h", minutes / (24 * 60), (minutes % (24 * 60)) / 60)
    }
}

/// Panel and menu form: two units. `45m`, `3h 58m`, `2d 13h`.
///
/// Spec §8.1 offered `4h 12m` in the title as a configurable option. Splitting
/// it by surface instead — compact in the menu bar, long in the panel and the
/// menu — gives the same information without another setting to explain.
pub fn format_long(until: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let minutes = (until - now).num_minutes();
    if minutes <= 0 {
        "now".to_string()
    } else if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 24 * 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{}d {}h", minutes / (24 * 60), (minutes % (24 * 60)) / 60)
    }
}

pub fn render_title(quotas: &[Quota], entries: &[TitleEntry], now: DateTime<Utc>) -> String {
    entries
        .iter()
        .filter_map(|entry| {
            let quota = quotas.iter().find(|q| q.id == entry.quota_id)?;
            let mut parts = Vec::new();
            if entry.show_percent {
                parts.push(format!("{}%", quota.percent.round() as i64));
            }
            if entry.show_countdown {
                if let Some(resets_at) = quota.resets_at {
                    parts.push(format_compact(resets_at, now));
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(parts.join(" · "))
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// The text rows in the tray menu. On Linux this is the only glanceable
/// surface, because AppIndicator emits no click events (spec §14).
pub fn menu_labels(quotas: &[Quota], now: DateTime<Utc>) -> Vec<String> {
    quotas
        .iter()
        .map(|quota| {
            let head = format!("{} — {}%", quota.label, quota.percent.round() as i64);
            match quota.resets_at {
                Some(resets_at) => format!("{head} · in {}", format_long(resets_at, now)),
                None => head,
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Neutral,
    Normal,
    Warning,
    High,
    Critical,
}

impl IconKind {
    pub fn for_quotas(quotas: &[Quota]) -> Self {
        match quotas.iter().map(|q| q.severity).max() {
            None => IconKind::Neutral,
            Some(Severity::Normal) => IconKind::Normal,
            Some(Severity::Warning) => IconKind::Warning,
            Some(Severity::High) => IconKind::High,
            Some(Severity::Critical) => IconKind::Critical,
        }
    }

    pub fn bytes(self) -> &'static [u8] {
        match self {
            IconKind::Neutral => include_bytes!("../icons/tray-neutral.png"),
            IconKind::Normal => include_bytes!("../icons/tray-normal.png"),
            IconKind::Warning => include_bytes!("../icons/tray-warning.png"),
            IconKind::High => include_bytes!("../icons/tray-high.png"),
            IconKind::Critical => include_bytes!("../icons/tray-critical.png"),
        }
    }
}

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use crate::model::UsageSnapshot;

pub const TRAY_ID: &str = "main";

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app, &[])?;
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

/// One row below the quota list. Kept as data — rather than inlined
/// `menu.append` calls — so the ids, labels and enabled state can be
/// asserted in a plain unit test: `muda` (the native menu backend) refuses
/// to construct a `Menu` off the main thread, and every `#[test]` runs on a
/// worker thread, so a test can never build the real thing on macOS.
enum MenuRow {
    Item {
        id: &'static str,
        label: String,
        enabled: bool,
    },
    Separator,
}

/// Every row below the quota list, in order. `build_menu` renders exactly
/// this list, so a unit test asserting against it is asserting against what
/// actually ships, not a description that could drift from it.
///
/// `check_updates_label` is a parameter rather than a literal here because
/// `tray::apply` calls `build_menu` fresh on every poll: the label has to be
/// derived from `updater::UpdateCheckStatus` at each rebuild (see that
/// module), or the very next poll — seconds later — would silently wipe
/// whatever a check just reported.
fn trailing_rows(check_updates_label: String) -> Vec<MenuRow> {
    use MenuRow::{Item, Separator};
    vec![
        Item {
            id: "open",
            label: "Open panel".to_string(),
            enabled: true,
        },
        Item {
            id: "refresh",
            label: "Refresh now".to_string(),
            enabled: true,
        },
        Item {
            id: "settings",
            label: "Settings…".to_string(),
            enabled: true,
        },
        Separator,
        // Disabled: this row exists only to display the running version,
        // the same `CARGO_PKG_VERSION` the User-Agent is built from, so
        // there is never a question about what build is running.
        Item {
            id: "about",
            label: format!("About Claude Usage (v{})", env!("CARGO_PKG_VERSION")),
            enabled: false,
        },
        Item {
            id: "check_updates",
            label: check_updates_label,
            enabled: true,
        },
        Separator,
        Item {
            id: "quit",
            label: "Quit".to_string(),
            enabled: true,
        },
    ]
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, labels: &[String]) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    for (index, label) in labels.iter().enumerate() {
        let item = MenuItem::with_id(app, format!("quota-{index}"), label, false, None::<&str>)?;
        menu.append(&item)?;
    }
    if !labels.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    let check_updates_label = app
        .try_state::<std::sync::Arc<crate::updater::UpdateCheckStatus>>()
        .map(|status| status.label())
        .unwrap_or_else(|| "Check for Updates…".to_string());
    for row in trailing_rows(check_updates_label) {
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

/// Push a fresh snapshot into the tray: icon, title, and menu rows.
pub fn apply(app: &AppHandle, snapshot: &UsageSnapshot) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let now = chrono::Utc::now();

    if let Ok(image) =
        tauri::image::Image::from_bytes(IconKind::for_quotas(&snapshot.quotas).bytes())
    {
        let _ = tray.set_icon(Some(image));
    }
    let entries = crate::settings::load(app).title_entries;
    let _ = tray.set_title(Some(render_title(&snapshot.quotas, &entries, now)));
    if let Ok(menu) = build_menu(app, &menu_labels(&snapshot.quotas, now)) {
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
        // Spec §7: opening the popover refreshes, under the same 20 s throttle.
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
    fn compact_countdown_uses_minutes_under_an_hour() {
        assert_eq!(
            format_compact(now() + chrono::Duration::minutes(47), now()),
            "47m"
        );
        assert_eq!(
            format_compact(now() + chrono::Duration::minutes(1), now()),
            "1m"
        );
    }

    #[test]
    fn compact_countdown_switches_from_minutes_to_hours_at_exactly_sixty() {
        assert_eq!(
            format_compact(now() + chrono::Duration::minutes(59), now()),
            "59m"
        );
        assert_eq!(
            format_compact(now() + chrono::Duration::minutes(60), now()),
            "1h0m"
        );
    }

    /// The exact complaint that prompted this change: 1h49m was rounding down
    /// to 2h in the menu bar, throwing away the minutes at the point they
    /// matter most.
    #[test]
    fn compact_countdown_keeps_the_smaller_unit_instead_of_rounding() {
        assert_eq!(
            format_compact(now() + chrono::Duration::minutes(109), now()),
            "1h49m"
        );
    }

    #[test]
    fn compact_countdown_uses_hours_and_minutes_under_a_day() {
        assert_eq!(
            format_compact(now() + chrono::Duration::minutes(238), now()),
            "3h58m"
        );
        assert_eq!(
            format_compact(now() + chrono::Duration::hours(23), now()),
            "23h0m"
        );
    }

    #[test]
    fn compact_countdown_uses_days_and_hours_beyond_that() {
        assert_eq!(
            format_compact(now() + chrono::Duration::hours(24), now()),
            "1d0h"
        );
        assert_eq!(
            format_compact(
                now() + chrono::Duration::days(6) + chrono::Duration::hours(13),
                now()
            ),
            "6d13h"
        );
    }

    #[test]
    fn a_past_reset_reads_as_now() {
        assert_eq!(
            format_compact(now() - chrono::Duration::minutes(5), now()),
            "now"
        );
    }

    #[test]
    fn the_long_countdown_keeps_the_smaller_unit() {
        assert_eq!(
            format_long(now() + chrono::Duration::minutes(238), now()),
            "3h 58m"
        );
        assert_eq!(
            format_long(now() + chrono::Duration::minutes(45), now()),
            "45m"
        );
        assert_eq!(
            format_long(
                now() + chrono::Duration::minutes(2 * 24 * 60 + 13 * 60),
                now()
            ),
            "2d 13h"
        );
    }

    #[test]
    fn the_default_title_shows_session_and_weekly_all() {
        let quotas = vec![
            quota("session", 20.0, 238),
            quota("weekly_all", 2.0, 6 * 24 * 60),
        ];
        assert_eq!(
            render_title(&quotas, &TitleEntry::defaults(), now()),
            "20% · 3h58m  2% · 6d0h"
        );
    }

    #[test]
    fn entries_can_drop_the_countdown() {
        let quotas = vec![quota("session", 20.0, 238)];
        let entries = vec![TitleEntry {
            quota_id: "session".into(),
            show_percent: true,
            show_countdown: false,
        }];
        assert_eq!(render_title(&quotas, &entries, now()), "20%");
    }

    #[test]
    fn entries_naming_an_absent_quota_are_skipped_silently() {
        let quotas = vec![quota("session", 20.0, 238)];
        assert_eq!(
            render_title(&quotas, &TitleEntry::defaults(), now()),
            "20% · 3h58m"
        );
    }

    #[test]
    fn an_empty_entry_list_renders_nothing() {
        let quotas = vec![quota("session", 20.0, 238)];
        assert_eq!(render_title(&quotas, &[], now()), "");
    }

    #[test]
    fn a_quota_without_a_reset_time_omits_the_countdown() {
        let mut q = quota("weekly:Fable", 0.0, 0);
        q.resets_at = None;
        assert_eq!(
            render_title(
                &[q],
                &[TitleEntry {
                    quota_id: "weekly:Fable".into(),
                    show_percent: true,
                    show_countdown: true
                }],
                now()
            ),
            "0%"
        );
    }

    #[test]
    fn menu_labels_spell_the_quota_out_in_full() {
        let quotas = vec![quota("session", 20.0, 238)];
        assert_eq!(
            menu_labels(&quotas, now()),
            vec!["Label session — 20% · in 3h 58m"]
        );
    }

    #[test]
    fn menu_labels_omit_the_countdown_when_there_is_no_reset() {
        let mut q = quota("weekly:Fable", 0.0, 0);
        q.resets_at = None;
        assert_eq!(menu_labels(&[q], now()), vec!["Label weekly:Fable — 0%"]);
    }

    #[test]
    fn the_icon_follows_the_worst_quota() {
        assert_eq!(
            IconKind::for_quotas(&[quota("a", 10.0, 60), quota("b", 95.0, 60)]),
            IconKind::Critical
        );
        assert_eq!(
            IconKind::for_quotas(&[quota("a", 10.0, 60), quota("b", 60.0, 60)]),
            IconKind::Warning
        );
        assert_eq!(
            IconKind::for_quotas(&[quota("a", 10.0, 60)]),
            IconKind::Normal
        );
    }

    #[test]
    fn the_icon_maps_high_severity_to_the_high_icon() {
        assert_eq!(
            IconKind::for_quotas(&[quota("a", 85.0, 60)]),
            IconKind::High
        );
    }

    #[test]
    fn the_icon_orders_high_between_warning_and_critical() {
        // High must outrank Warning...
        assert_eq!(
            IconKind::for_quotas(&[quota("a", 60.0, 60), quota("b", 85.0, 60)]),
            IconKind::High
        );
        // ...and Critical must still outrank High.
        assert_eq!(
            IconKind::for_quotas(&[quota("a", 85.0, 60), quota("b", 95.0, 60)]),
            IconKind::Critical
        );
    }

    #[test]
    fn the_icon_follows_the_worst_quota_regardless_of_order() {
        // Worse quota first: distinguishes .max() from .last().
        assert_eq!(
            IconKind::for_quotas(&[quota("a", 95.0, 60), quota("b", 10.0, 60)]),
            IconKind::Critical
        );
        // Worse quota last: distinguishes .max() from .first().
        assert_eq!(
            IconKind::for_quotas(&[quota("a", 10.0, 60), quota("b", 95.0, 60)]),
            IconKind::Critical
        );
    }

    #[test]
    fn no_quotas_means_the_neutral_icon() {
        assert_eq!(IconKind::for_quotas(&[]), IconKind::Neutral);
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

    /// Proves the menu is wired: `About` is present, disabled, and names the
    /// running version; `Check for Updates…` carries whatever label the
    /// caller supplied (the live label comes from
    /// `updater::UpdateCheckStatus`, exercised in `updater.rs`'s own tests)
    /// and is enabled; both sit above `Quit`. Asserted against
    /// `trailing_rows()` — the exact data `build_menu` renders — rather
    /// than a live `muda::Menu`, which refuses to build off the main thread
    /// and so cannot be constructed inside a `#[test]` on macOS.
    #[test]
    fn about_and_check_for_updates_sit_above_quit() {
        let items: Vec<(&str, String, bool)> = trailing_rows("Check for Updates…".to_string())
            .into_iter()
            .filter_map(|row| match row {
                MenuRow::Item { id, label, enabled } => Some((id, label, enabled)),
                MenuRow::Separator => None,
            })
            .collect();

        let (_, about_label, about_enabled) = items
            .iter()
            .find(|(id, _, _)| *id == "about")
            .expect("About item missing");
        assert_eq!(
            *about_label,
            format!("About Claude Usage (v{})", env!("CARGO_PKG_VERSION"))
        );
        assert!(
            !about_enabled,
            "About should be informational, not clickable"
        );

        let (_, check_label, check_enabled) = items
            .iter()
            .find(|(id, _, _)| *id == "check_updates")
            .expect("Check for Updates… item missing");
        assert_eq!(*check_label, "Check for Updates…");
        assert!(*check_enabled);

        let ids: Vec<&str> = items.iter().map(|(id, _, _)| *id).collect();
        let position = |id: &str| ids.iter().position(|&candidate| candidate == id).unwrap();
        assert!(position("about") < position("quit"));
        assert!(position("check_updates") < position("quit"));
    }

    /// The label parameter really does reach the rendered row, not just the
    /// idle default — the case that matters since `build_menu` passes
    /// `updater::UpdateCheckStatus::label()`'s live output here on every
    /// rebuild.
    #[test]
    fn the_check_updates_row_carries_whatever_label_it_is_given() {
        let items: Vec<(&str, String, bool)> = trailing_rows("Checking for updates…".to_string())
            .into_iter()
            .filter_map(|row| match row {
                MenuRow::Item { id, label, enabled } => Some((id, label, enabled)),
                MenuRow::Separator => None,
            })
            .collect();
        let (_, check_label, _) = items
            .iter()
            .find(|(id, _, _)| *id == "check_updates")
            .expect("Check for Updates… item missing");
        assert_eq!(*check_label, "Checking for updates…");
    }
}
