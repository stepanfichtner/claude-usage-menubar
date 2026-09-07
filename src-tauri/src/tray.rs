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

/// Menu bar form: one unit only. `47m`, `3h`, `6d`.
pub fn format_compact(until: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let minutes = (until - now).num_minutes();
    if minutes <= 0 {
        "now".to_string()
    } else if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 24 * 60 {
        format!("{}h", minutes / 60)
    } else {
        format!("{}d", minutes / (24 * 60))
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
    Critical,
}

impl IconKind {
    pub fn for_quotas(quotas: &[Quota]) -> Self {
        match quotas.iter().map(|q| q.severity).max() {
            None => IconKind::Neutral,
            Some(Severity::Normal) => IconKind::Normal,
            Some(Severity::Warning) => IconKind::Warning,
            Some(Severity::Critical) => IconKind::Critical,
        }
    }

    pub fn bytes(self) -> &'static [u8] {
        match self {
            IconKind::Neutral => include_bytes!("../icons/tray-neutral.png"),
            IconKind::Normal => include_bytes!("../icons/tray-normal.png"),
            IconKind::Warning => include_bytes!("../icons/tray-warning.png"),
            IconKind::Critical => include_bytes!("../icons/tray-critical.png"),
        }
    }
}

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

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
            if let tauri::tray::TrayIconEvent::Click { button, .. } = event {
                if button == tauri::tray::MouseButton::Left {
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

fn build_menu(app: &AppHandle, labels: &[String]) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::new(app)?;
    for (index, label) in labels.iter().enumerate() {
        let item = MenuItem::with_id(app, format!("quota-{index}"), label, false, None::<&str>)?;
        menu.append(&item)?;
    }
    if !labels.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    menu.append(&MenuItem::with_id(
        app,
        "open",
        "Open panel",
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        "refresh",
        "Refresh now",
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        "settings",
        "Settings…",
        true,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?)?;
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
    fn compact_countdown_uses_whole_hours_under_a_day() {
        assert_eq!(
            format_compact(now() + chrono::Duration::minutes(238), now()),
            "3h"
        );
        assert_eq!(
            format_compact(now() + chrono::Duration::hours(23), now()),
            "23h"
        );
    }

    #[test]
    fn compact_countdown_uses_whole_days_beyond_that() {
        assert_eq!(
            format_compact(now() + chrono::Duration::hours(24), now()),
            "1d"
        );
        assert_eq!(
            format_compact(now() + chrono::Duration::days(6), now()),
            "6d"
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
            "20% · 3h  2% · 6d"
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
            "20% · 3h"
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
    fn no_quotas_means_the_neutral_icon() {
        assert_eq!(IconKind::for_quotas(&[]), IconKind::Neutral);
    }
}
