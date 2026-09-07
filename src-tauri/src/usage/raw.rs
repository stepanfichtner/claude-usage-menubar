use chrono::{DateTime, Utc};
use serde::Deserialize;

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
