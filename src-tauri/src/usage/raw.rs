use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Deliberately tolerant of unknown keys: nothing in this module carries
/// `deny_unknown_fields`, at any level. The endpoint is undocumented and its
/// response already ships keys this app does not model — codenamed feature
/// flags (`nimbus_quill`, `juniper_tide`), `member_dashboard_available`,
/// `group` inside a limit, `surface` inside a scope — and `fetch_usage` maps
/// any deserialization failure to `ApiError::Parse`, so rejecting an
/// unrecognised key would blank the menu bar the day a new one appears.
///
/// Dropping them is also what keeps them out of the UI: only `limits[]` and
/// the four legacy windows below are ever read, so no top-level key can
/// become a quota. That half needs no test — there is no branch behind it —
/// but the tolerance does, and
/// `normalize::tests::unknown_response_keys_are_tolerated_rather_than_rejected`
/// is it.
#[derive(Debug, Clone, Deserialize)]
pub struct RawUsage {
    #[serde(default)]
    pub limits: Vec<RawLimit>,
    #[serde(default)]
    pub five_hour: Option<RawWindow>,
    #[serde(default)]
    pub seven_day: Option<RawWindow>,
    #[serde(default)]
    pub seven_day_opus: Option<RawWindow>,
    #[serde(default)]
    pub seven_day_sonnet: Option<RawWindow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawLimit {
    pub kind: String,
    #[serde(default)]
    pub percent: f64,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scope: Option<RawScope>,
    #[serde(default)]
    pub is_active: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawScope {
    #[serde(default)]
    pub model: Option<RawModel>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawModel {
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawWindow {
    #[serde(default)]
    pub utilization: Option<f64>,
    #[serde(default)]
    pub resets_at: Option<DateTime<Utc>>,
}
