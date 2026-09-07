use crate::model::{Quota, Severity};
use crate::usage::raw::{RawLimit, RawUsage, RawWindow};

/// Turn the server's response into the flat list the UI renders.
///
/// `limits[]` is authoritative (spec §4.3). The legacy top-level keys are used
/// only when it is absent or empty.
pub fn normalize(raw: &RawUsage) -> Vec<Quota> {
    if raw.limits.is_empty() {
        legacy(raw)
    } else {
        raw.limits.iter().map(from_limit).collect()
    }
}

fn from_limit(limit: &RawLimit) -> Quota {
    let (id, label) = match limit.kind.as_str() {
        "session" => ("session".to_string(), "Current session".to_string()),
        "weekly_all" => ("weekly_all".to_string(), "This week".to_string()),
        "weekly_scoped" => {
            let model = limit
                .scope
                .as_ref()
                .and_then(|s| s.model.as_ref())
                .and_then(|m| m.display_name.as_deref())
                .unwrap_or("scoped");
            (format!("weekly:{model}"), format!("{model} this week"))
        }
        other => (other.to_string(), title_case(other)),
    };

    Quota {
        id,
        label,
        percent: limit.percent,
        severity: Severity::from_api(limit.severity.as_deref())
            .max(Severity::from_percent(limit.percent)),
        resets_at: limit.resets_at,
        is_active: limit.is_active,
    }
}

fn legacy(raw: &RawUsage) -> Vec<Quota> {
    let mut out = Vec::new();
    let mut push = |window: &Option<RawWindow>, id: &str, label: &str, active: bool| {
        if let Some(w) = window {
            if let Some(percent) = w.utilization {
                out.push(Quota {
                    id: id.to_string(),
                    label: label.to_string(),
                    percent,
                    severity: Severity::from_percent(percent),
                    resets_at: w.resets_at,
                    is_active: active,
                });
            }
        }
    };
    push(&raw.five_hour, "session", "Current session", true);
    push(&raw.seven_day, "weekly_all", "This week", false);
    push(&raw.seven_day_opus, "weekly:Opus", "Opus this week", false);
    push(
        &raw.seven_day_sonnet,
        "weekly:Sonnet",
        "Sonnet this week",
        false,
    );
    out
}

/// `monthly_experiment` → `Monthly Experiment`
fn title_case(kind: &str) -> String {
    kind.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Severity;

    fn load(name: &str) -> crate::usage::raw::RawUsage {
        let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
        let text = std::fs::read_to_string(path).expect("fixture missing");
        serde_json::from_str(&text).expect("fixture does not parse")
    }

    #[test]
    fn limits_array_wins_over_legacy_keys() {
        let quotas = normalize(&load("usage_full.json"));
        assert_eq!(quotas.len(), 3);
        assert_eq!(quotas[0].id, "session");
        assert_eq!(quotas[0].percent, 20.0);
        assert_eq!(quotas[1].id, "weekly_all");
        assert_eq!(quotas[1].percent, 2.0);
    }

    #[test]
    fn scoped_weekly_uses_the_server_display_name() {
        let quotas = normalize(&load("usage_full.json"));
        assert_eq!(quotas[2].id, "weekly:Fable");
        assert_eq!(quotas[2].label, "Fable this week");
        assert_eq!(quotas[2].resets_at, None);
    }

    #[test]
    fn session_and_weekly_all_get_fixed_labels() {
        let quotas = normalize(&load("usage_full.json"));
        assert_eq!(quotas[0].label, "Current session");
        assert_eq!(quotas[1].label, "This week");
        assert!(quotas[0].is_active);
        assert!(!quotas[1].is_active);
    }

    #[test]
    fn codename_keys_are_ignored() {
        let quotas = normalize(&load("usage_full.json"));
        assert!(!quotas.iter().any(|q| q.id.contains("nimbus")));
        assert!(!quotas.iter().any(|q| q.id.contains("juniper")));
    }

    #[test]
    fn falls_back_to_legacy_keys_when_limits_absent() {
        let quotas = normalize(&load("usage_legacy_only.json"));
        let ids: Vec<&str> = quotas.iter().map(|q| q.id.as_str()).collect();
        assert_eq!(ids, vec!["session", "weekly_all", "weekly:Opus"]);
        assert_eq!(quotas[0].percent, 41.5);
        assert_eq!(quotas[2].percent, 12.0);
        // The legacy path renames its labels to the same family as
        // `from_limit` — unguarded until now, so a revert of either would
        // have passed silently.
        assert_eq!(quotas[0].label, "Current session");
        assert_eq!(quotas[1].label, "This week");
        assert_eq!(quotas[2].label, "Opus this week");
    }

    #[test]
    fn falls_back_when_limits_is_present_but_empty() {
        let raw: crate::usage::raw::RawUsage =
            serde_json::from_str(r#"{"limits":[],"five_hour":{"utilization":3.0}}"#).unwrap();
        let quotas = normalize(&raw);
        assert_eq!(quotas.len(), 1);
        assert_eq!(quotas[0].id, "session");
    }

    #[test]
    fn unknown_kind_is_rendered_not_dropped() {
        let quotas = normalize(&load("usage_unknown_kind.json"));
        assert_eq!(quotas.len(), 1);
        assert_eq!(quotas[0].id, "monthly_experiment");
        assert_eq!(quotas[0].label, "Monthly Experiment");
    }

    #[test]
    fn effective_severity_is_the_higher_of_server_and_derived() {
        // server says normal, 95% derives critical → critical
        assert_eq!(
            Severity::from_api(Some("normal")).max(Severity::from_percent(95.0)),
            Severity::Critical
        );
        // server says critical, 10% derives normal → critical
        assert_eq!(
            Severity::from_api(Some("critical")).max(Severity::from_percent(10.0)),
            Severity::Critical
        );
    }

    #[test]
    fn effective_severity_is_composed_inside_normalize() {
        let quotas = normalize(&load("usage_severity_mismatch.json"));
        // Server says normal, 95% derives Critical — the higher wins.
        assert_eq!(quotas[0].severity, Severity::Critical);
        // Server says critical, 10% derives Normal — the higher still wins.
        assert_eq!(quotas[1].severity, Severity::Critical);
    }

    #[test]
    fn derived_severity_boundaries() {
        assert_eq!(Severity::from_percent(49.9), Severity::Normal);
        assert_eq!(Severity::from_percent(50.0), Severity::Warning);
        assert_eq!(Severity::from_percent(79.9), Severity::Warning);
        assert_eq!(Severity::from_percent(80.0), Severity::High);
        assert_eq!(Severity::from_percent(89.9), Severity::High);
        assert_eq!(Severity::from_percent(90.0), Severity::Critical);
    }

    #[test]
    fn unrecognised_server_severity_is_normal() {
        assert_eq!(Severity::from_api(Some("chartreuse")), Severity::Normal);
        assert_eq!(Severity::from_api(None), Severity::Normal);
    }
}
