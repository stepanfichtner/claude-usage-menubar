use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Normal,
    Warning,
    Critical,
}

impl Severity {
    /// Locally derived severity. Boundaries match the default notification
    /// thresholds so the colour change and the notification agree (spec §6.2).
    pub fn from_percent(percent: f64) -> Self {
        if percent >= 90.0 {
            Severity::Critical
        } else if percent >= 50.0 {
            Severity::Warning
        } else {
            Severity::Normal
        }
    }

    /// The server's own severity. Anything unrecognised is treated as normal
    /// rather than rejected, so a new value cannot break the app.
    pub fn from_api(value: Option<&str>) -> Self {
        match value {
            Some("warning") => Severity::Warning,
            Some("critical") => Severity::Critical,
            _ => Severity::Normal,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    pub id: String,
    pub label: String,
    pub percent: f64,
    pub severity: Severity,
    pub resets_at: Option<DateTime<Utc>>,
    pub is_active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub quotas: Vec<Quota>,
    pub fetched_at: DateTime<Utc>,
    /// True when this came from the disk cache rather than a live fetch.
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub display_name: String,
    pub plan_label: String,
}
