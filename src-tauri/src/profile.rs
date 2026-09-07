use serde::Deserialize;

use crate::error::ApiError;
use crate::model::Profile;
use crate::usage::user_agent;

#[derive(Deserialize)]
struct RawProfile {
    #[serde(default)]
    account: RawAccount,
    #[serde(default)]
    organization: RawOrganization,
}

/// Only these two fields are declared. Everything else in the response —
/// full_name, email, uuids, billing — is never deserialized (spec §12.3).
#[derive(Deserialize, Default)]
struct RawAccount {
    #[serde(default)]
    display_name: String,
}

#[derive(Deserialize, Default)]
struct RawOrganization {
    #[serde(default)]
    rate_limit_tier: String,
}

/// `default_claude_max_5x` → `Claude Max 5×` (spec §4.5).
///
/// A transform rather than a lookup table, so a tier we have never seen still
/// renders sensibly instead of showing a raw identifier.
pub fn plan_label(tier: &str) -> String {
    let tier = tier.strip_prefix("default_").unwrap_or(tier);
    if tier.is_empty() {
        return String::new();
    }
    tier.split('_')
        .map(|word| {
            if let Some(digits) = word.strip_suffix('x') {
                if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                    return format!("{digits}×");
                }
            }
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub async fn fetch_profile(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<Profile, ApiError> {
    let response = client
        .get(format!("{base_url}/api/oauth/profile"))
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("user-agent", user_agent())
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;

    match response.status().as_u16() {
        200 => {}
        401 | 403 => return Err(ApiError::SignedOut),
        429 => return Err(ApiError::RateLimited),
        other => return Err(ApiError::Http(other)),
    }

    let body = response
        .text()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    let raw: RawProfile = serde_json::from_str(&body).map_err(|_| ApiError::Parse)?;

    Ok(Profile {
        display_name: raw.account.display_name,
        plan_label: plan_label(&raw.organization.rate_limit_tier),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn known_tiers_render_with_a_multiplication_sign() {
        assert_eq!(plan_label("default_claude_max_5x"), "Claude Max 5×");
        assert_eq!(plan_label("default_claude_max_20x"), "Claude Max 20×");
    }

    #[test]
    fn tiers_without_a_multiplier_render_plainly() {
        assert_eq!(plan_label("default_claude_pro"), "Claude Pro");
    }

    #[test]
    fn unknown_tiers_are_title_cased_rather_than_dropped() {
        assert_eq!(plan_label("default_claude_ultra_3x"), "Claude Ultra 3×");
        assert_eq!(plan_label("something_new"), "Something New");
    }

    #[test]
    fn an_empty_tier_yields_an_empty_label() {
        assert_eq!(plan_label(""), "");
    }

    #[test]
    fn a_word_ending_in_x_that_is_not_a_multiplier_is_untouched() {
        assert_eq!(plan_label("default_claude_linux"), "Claude Linux");
    }

    #[tokio::test]
    async fn reads_only_display_name_and_tier() {
        let server = MockServer::start().await;
        let body = r#"{
          "account": { "uuid": "u", "full_name": "Full Name", "display_name": "Fichy",
                       "email": "person@example.com", "has_claude_max": true },
          "organization": { "uuid": "o", "name": "Org", "organization_type": "claude_max",
                            "rate_limit_tier": "default_claude_max_5x" },
          "application": { "name": "Claude Code" }
        }"#;
        Mock::given(method("GET"))
            .and(path("/api/oauth/profile"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let profile = fetch_profile(&client, &server.uri(), "t").await.unwrap();
        assert_eq!(profile.display_name, "Fichy");
        assert_eq!(profile.plan_label, "Claude Max 5×");

        // Spec §12.3: nothing else from the response may survive into app state.
        let serialized = serde_json::to_string(&profile).unwrap();
        assert!(!serialized.contains("person@example.com"));
        assert!(!serialized.contains("Full Name"));
    }

    #[tokio::test]
    async fn a_missing_tier_yields_an_empty_plan_label() {
        let server = MockServer::start().await;
        let body = r#"{"account":{"display_name":"Fichy"},"organization":{}}"#;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        let client = reqwest::Client::new();
        let profile = fetch_profile(&client, &server.uri(), "t").await.unwrap();
        assert_eq!(profile.display_name, "Fichy");
        assert_eq!(profile.plan_label, "");
    }
}
