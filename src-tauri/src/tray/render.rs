//! The half of the tray that is only text: countdown formatting, menu-bar
//! title assembly, the quota lines the menu shows, the trailing menu rows,
//! and severity-to-icon selection. No Tauri handle reaches anything here,
//! which is what lets all of it be exercised by plain `#[test]`s — the
//! wiring that pushes these strings at a real tray lives in the parent
//! module.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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

/// What goes between one quota's segment and the next in the menu-bar title.
///
/// The parts *within* a segment are already joined by `" · "` — percentage,
/// then countdown — so the whole job of a group separator is to be something
/// that middle dot cannot be mistaken for at menu-bar size. That is what
/// rules out the obvious candidate: a bullet (`•`) is a middle dot at twice
/// the diameter, and telling the two apart by size is exactly the judgement
/// this setting exists to spare the reader. The three visible marks differ
/// from `·` in shape rather than in size — a full-height stroke, a diamond,
/// a diagonal.
///
/// `Space` is the two literal spaces 0.1.0 rendered, and stays the default:
/// nobody's menu bar changes until they ask it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitleSeparator {
    #[default]
    Space,
    Pipe,
    Diamond,
    Slash,
    /// A name this build does not know: written by a newer build, or by
    /// hand. Deserializing lands here instead of failing, because failing
    /// would take the whole `Settings` down with it — one unreadable field
    /// would silently reset the poll interval, the thresholds and the title
    /// entries too. `Settings::sanitized` replaces it on every load and
    /// every save, so it is not a value the rest of the app has to think
    /// about; `glyph` still answers for it, so even an unsanitized one
    /// renders today's title rather than panicking.
    Unknown,
}

impl TitleSeparator {
    /// What is actually inserted between two segments, spacing included.
    pub fn glyph(self) -> &'static str {
        match self {
            TitleSeparator::Space => "  ",
            TitleSeparator::Pipe => " | ",
            TitleSeparator::Diamond => " ◆ ",
            TitleSeparator::Slash => " / ",
            // Deliberately the default's glyph rather than a second literal:
            // an unrecognised choice should leave the menu bar exactly as it
            // was, and that stays true if the default's spacing is ever
            // revised.
            TitleSeparator::Unknown => Self::default().glyph(),
        }
    }

    /// The name this choice is stored under in `settings.json` and sent to
    /// the settings window under. Paired with `from_stored_name` below —
    /// both directions written out here, rather than derived on one side and
    /// hand-matched on the other, so a name can only be changed in one
    /// place. `separator_names_round_trip_through_their_stored_form` holds
    /// the pair together.
    fn stored_name(self) -> &'static str {
        match self {
            TitleSeparator::Space => "space",
            TitleSeparator::Pipe => "pipe",
            TitleSeparator::Diamond => "diamond",
            TitleSeparator::Slash => "slash",
            TitleSeparator::Unknown => "unknown",
        }
    }

    fn from_stored_name(name: &str) -> Self {
        match name {
            "space" => TitleSeparator::Space,
            "pipe" => TitleSeparator::Pipe,
            "diamond" => TitleSeparator::Diamond,
            "slash" => TitleSeparator::Slash,
            _ => TitleSeparator::Unknown,
        }
    }
}

impl Serialize for TitleSeparator {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.stored_name())
    }
}

/// Written by hand for one reason: a derived `Deserialize` rejects a name it
/// does not know, and `settings::load` reads the whole `Settings` in one
/// `from_value` — so a value only a newer build understands would discard
/// every other setting alongside it. An unknown *name* becomes
/// `TitleSeparator::Unknown` here and is normalised by `sanitized`. An
/// unknown *type* (a number, an object) is still an error, which is how
/// every other field in the store behaves.
impl<'de> Deserialize<'de> for TitleSeparator {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_stored_name(&String::deserialize(deserializer)?))
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

/// The menu-bar title: one segment per entry, each `percent · countdown`,
/// joined by `separator`. An entry with nothing to show — no figures ticked,
/// or no quota in this snapshot to take them from — drops out before the
/// join, so it never leaves a separator behind with nothing on one side of
/// it.
///
/// Mirrored by `renderTitle` in `src/lib/titlePreview.ts`, which the settings
/// window's live preview draws with; the two case tables are written to match
/// each other string for string.
pub fn render_title(
    quotas: &[Quota],
    entries: &[TitleEntry],
    separator: TitleSeparator,
    now: DateTime<Utc>,
) -> String {
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
        .join(separator.glyph())
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
            IconKind::Neutral => include_bytes!("../../icons/tray-neutral.png"),
            IconKind::Normal => include_bytes!("../../icons/tray-normal.png"),
            IconKind::Warning => include_bytes!("../../icons/tray-warning.png"),
            IconKind::High => include_bytes!("../../icons/tray-high.png"),
            IconKind::Critical => include_bytes!("../../icons/tray-critical.png"),
        }
    }
}

/// One row below the quota list. Kept as data — rather than inlined
/// `menu.append` calls — so the ids, labels and enabled state can be
/// asserted in a plain unit test: `muda` (the native menu backend) refuses
/// to construct a `Menu` off the main thread, and every `#[test]` runs on a
/// worker thread, so a test can never build the real thing on macOS.
pub(super) enum MenuRow {
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
/// the label has to be derived from `updater::UpdateCheckStatus` at each
/// rebuild (see that module), not baked in once: `tray::apply` rebuilds
/// whenever the menu's content moves, so a stale literal would be pushed
/// over whatever a check had just reported.
pub(super) fn trailing_rows(check_updates_label: String) -> Vec<MenuRow> {
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

    /// The other half of the mirror. `src/lib/countdown.test.ts` pins
    /// `formatLong` at 59 and 60 minutes; until now the Rust side pinned that
    /// boundary for `format_compact` only, so the two implementations were
    /// held to the same rule — `<` vs `<=` on the hour, and whether the hours
    /// branch keeps its minutes — on one side and not the other.
    /// `the_long_countdown_keeps_the_smaller_unit` above uses 45 and 238
    /// minutes, both well inside their branches, and passes unchanged against
    /// a `minutes <= 60`.
    #[test]
    fn the_long_countdown_switches_from_minutes_to_hours_at_exactly_sixty() {
        assert_eq!(
            format_long(now() + chrono::Duration::minutes(59), now()),
            "59m"
        );
        assert_eq!(
            format_long(now() + chrono::Duration::minutes(60), now()),
            "1h 0m"
        );
    }

    #[test]
    fn the_default_title_shows_session_and_weekly_all() {
        let quotas = vec![
            quota("session", 20.0, 238),
            quota("weekly_all", 2.0, 6 * 24 * 60),
        ];
        assert_eq!(
            render_title(
                &quotas,
                &TitleEntry::defaults(),
                TitleSeparator::Space,
                now()
            ),
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
        assert_eq!(
            render_title(&quotas, &entries, TitleSeparator::Space, now()),
            "20%"
        );
    }

    #[test]
    fn entries_naming_an_absent_quota_are_skipped_silently() {
        let quotas = vec![quota("session", 20.0, 238)];
        assert_eq!(
            render_title(
                &quotas,
                &TitleEntry::defaults(),
                TitleSeparator::Space,
                now()
            ),
            "20% · 3h58m"
        );
    }

    #[test]
    fn an_empty_entry_list_renders_nothing() {
        let quotas = vec![quota("session", 20.0, 238)];
        assert_eq!(render_title(&quotas, &[], TitleSeparator::Space, now()), "");
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
                TitleSeparator::Space,
                now()
            ),
            "0%"
        );
    }

    /// The three entries of the complaint this setting answers, under every
    /// separator: `24% · 4h1m  33%  3%` cannot be read as three groups,
    /// because the gap between groups is the same gap as the one inside the
    /// first. Three entries rather than two on purpose — with two, a
    /// separator inserted once (after the first segment only) would look
    /// exactly like one joining every pair.
    ///
    /// Mirrored case for case by `renderTitle`'s
    /// "puts the chosen separator between every pair of entries" in
    /// `src/lib/titlePreview.test.ts`, with these same expected strings.
    #[test]
    fn every_separator_joins_each_pair_of_entries_with_its_own_glyph() {
        let quotas = vec![
            quota("session", 24.0, 241),
            quota("weekly_all", 33.0, 6 * 24 * 60),
            quota("weekly:Fable", 3.0, 6 * 24 * 60),
        ];
        let entries = vec![
            TitleEntry {
                quota_id: "session".into(),
                show_percent: true,
                show_countdown: true,
            },
            TitleEntry {
                quota_id: "weekly_all".into(),
                show_percent: true,
                show_countdown: false,
            },
            TitleEntry {
                quota_id: "weekly:Fable".into(),
                show_percent: true,
                show_countdown: false,
            },
        ];

        for (separator, expected) in [
            (TitleSeparator::Space, "24% · 4h1m  33%  3%"),
            (TitleSeparator::Pipe, "24% · 4h1m | 33% | 3%"),
            (TitleSeparator::Diamond, "24% · 4h1m ◆ 33% ◆ 3%"),
            (TitleSeparator::Slash, "24% · 4h1m / 33% / 3%"),
        ] {
            assert_eq!(
                render_title(&quotas, &entries, separator, now()),
                expected,
                "{separator:?}"
            );
        }
    }

    /// A separator separates, and nothing else: with one segment there is
    /// nothing for it to stand between, and with none there is nothing at
    /// all. Both rows run under every choice, which keeps this table square
    /// with the TypeScript mirror's.
    ///
    /// Measured rather than assumed: replacing the `join` with a per-entry
    /// append fails the one-segment row for all four choices — `Space`
    /// included, whose trailing pair of spaces the menu bar would hide but
    /// this comparison does not. The empty row survives that mutation, since
    /// there is nothing to append to; it is here for the boundary, not for
    /// its strength.
    #[test]
    fn nothing_is_separated_from_nothing() {
        let quotas = vec![quota("session", 20.0, 238)];
        let one = vec![TitleEntry {
            quota_id: "session".into(),
            show_percent: true,
            show_countdown: true,
        }];

        for separator in [
            TitleSeparator::Space,
            TitleSeparator::Pipe,
            TitleSeparator::Diamond,
            TitleSeparator::Slash,
        ] {
            assert_eq!(
                render_title(&quotas, &one, separator, now()),
                "20% · 3h58m",
                "one entry, {separator:?}"
            );
            assert_eq!(
                render_title(&quotas, &[], separator, now()),
                "",
                "no entries, {separator:?}"
            );
        }
    }

    /// The dropped entries again, this time with a separator visible enough
    /// to show what dropping them costs if the filter ran after the join:
    /// `24% | ` for a quota this snapshot does not carry, and `24% |  | 3%`
    /// for an entry with both boxes unticked.
    #[test]
    fn an_entry_that_shows_nothing_leaves_no_separator_behind() {
        let quotas = vec![quota("session", 24.0, 241), quota("weekly:Fable", 3.0, 60)];

        // An entry whose quota is not in this snapshot at all.
        let absent = vec![
            TitleEntry {
                quota_id: "session".into(),
                show_percent: true,
                show_countdown: false,
            },
            TitleEntry {
                quota_id: "weekly_all".into(),
                show_percent: true,
                show_countdown: true,
            },
        ];
        assert_eq!(
            render_title(&quotas, &absent, TitleSeparator::Pipe, now()),
            "24%"
        );

        // And an entry between two others with neither figure ticked.
        let unticked = vec![
            TitleEntry {
                quota_id: "session".into(),
                show_percent: true,
                show_countdown: false,
            },
            TitleEntry {
                quota_id: "weekly_all".into(),
                show_percent: false,
                show_countdown: false,
            },
            TitleEntry {
                quota_id: "weekly:Fable".into(),
                show_percent: true,
                show_countdown: false,
            },
        ];
        assert_eq!(
            render_title(&quotas, &unticked, TitleSeparator::Pipe, now()),
            "24% | 3%"
        );
    }

    /// The rule every offered glyph is chosen under, kept as an assertion so
    /// that adding a fifth choice has to clear it too: a group separator that
    /// is a middle dot — or any dot — puts the same mark between groups as
    /// `render_title` already puts inside one, which is worse than the two
    /// spaces it replaces. It also has to be visible at all; the invisible
    /// one is `Space`, and that is the default rather than a choice made to
    /// mark a boundary.
    #[test]
    fn no_offered_separator_can_be_mistaken_for_the_dot_inside_a_segment() {
        for separator in [
            TitleSeparator::Pipe,
            TitleSeparator::Diamond,
            TitleSeparator::Slash,
        ] {
            let glyph = separator.glyph();
            assert!(
                !glyph.contains('·'),
                "{separator:?} uses the intra-segment dot"
            );
            assert!(
                !glyph.contains('•'),
                "{separator:?} uses a bullet, which is that dot at twice the size"
            );
            assert!(!glyph.trim().is_empty(), "{separator:?} has nothing to see");
        }
        assert_eq!(
            TitleSeparator::Space.glyph(),
            "  ",
            "the default is the two spaces 0.1.0 rendered, exactly"
        );
    }

    /// `stored_name` and `from_stored_name` are the two halves of what a
    /// `settings.json` holds, and they are written out separately — so this
    /// walks every variant through both. A name changed on one side only
    /// would mean a saved choice silently reading back as `Unknown`, and
    /// from there as the default: the setting would appear to forget itself
    /// on every restart.
    #[test]
    fn separator_names_round_trip_through_their_stored_form() {
        for separator in [
            TitleSeparator::Space,
            TitleSeparator::Pipe,
            TitleSeparator::Diamond,
            TitleSeparator::Slash,
            TitleSeparator::Unknown,
        ] {
            let json = serde_json::to_string(&separator).unwrap();
            let parsed: TitleSeparator = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, separator, "{separator:?} came back as {parsed:?}");
        }
        assert_eq!(
            serde_json::to_string(&TitleSeparator::Pipe).unwrap(),
            "\"pipe\"",
            "the stored form is the name, not a number or a glyph"
        );
    }

    /// A name only a newer build knows must not be an error: `settings::load`
    /// parses the whole `Settings` in one go, so a rejected value here would
    /// take the poll interval, the thresholds and the title entries down with
    /// it. It parses as `Unknown` — which `Settings::sanitized` replaces (see
    /// `an_unrecognised_separator_is_replaced_without_losing_the_rest`) and
    /// which renders as the default even if it somehow arrives unsanitized.
    #[test]
    fn a_separator_name_this_build_does_not_know_is_not_an_error() {
        let parsed: TitleSeparator = serde_json::from_str("\"arrow\"").unwrap();
        assert_eq!(parsed, TitleSeparator::Unknown);
        assert_eq!(
            parsed.glyph(),
            TitleSeparator::Space.glyph(),
            "an unknown choice renders as today's title, not as nothing"
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
    /// idle default — the case that matters since what arrives here is
    /// `updater::UpdateCheckStatus::label()`'s live output, read by
    /// `tray::apply` and `tray::refresh_menu` and threaded down through
    /// `build_menu` at each rebuild.
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

    /// The other half of R52: a status change must reach `trailing_rows`'
    /// output on its own, with no poll — and so no `UsageSnapshot`, and no
    /// call to `apply` — involved anywhere in the chain.
    #[test]
    fn a_status_change_reaches_trailing_rows_without_any_poll_having_run() {
        let status = crate::updater::UpdateCheckStatus::default();
        status.set_checking();

        let items: Vec<(&str, String, bool)> = trailing_rows(status.label())
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
