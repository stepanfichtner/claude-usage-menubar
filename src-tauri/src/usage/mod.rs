pub mod normalize;
pub mod raw;

pub use normalize::normalize;

use crate::error::ApiError;
use crate::model::Quota;

pub const API_BASE: &str = "https://api.anthropic.com";

pub fn user_agent() -> String {
    format!("claude-usage-menubar/{}", env!("CARGO_PKG_VERSION"))
}

pub async fn fetch_usage(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<Vec<Quota>, ApiError> {
    let url = format!("{base_url}/api/oauth/usage?at_wall=1&skip_spend=1");
    let response = client
        .get(url)
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
    let raw: raw::RawUsage = serde_json::from_str(&body).map_err(|_| ApiError::Parse)?;
    Ok(normalize(&raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{}",
            env!("CARGO_MANIFEST_DIR"),
            name
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn fetches_and_normalizes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/oauth/usage"))
            .and(query_param("at_wall", "1"))
            .and(query_param("skip_spend", "1"))
            .and(header("authorization", "Bearer test-token"))
            .and(header("anthropic-beta", "oauth-2025-04-20"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(fixture("usage_full.json"), "application/json"),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let quotas = fetch_usage(&client, &server.uri(), "test-token")
            .await
            .unwrap();
        assert_eq!(quotas.len(), 3);
        assert_eq!(quotas[0].id, "session");
    }

    #[tokio::test]
    async fn maps_401_to_signed_out() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let client = reqwest::Client::new();
        assert_eq!(
            fetch_usage(&client, &server.uri(), "t").await.unwrap_err(),
            ApiError::SignedOut
        );
    }

    #[tokio::test]
    async fn maps_429_to_rate_limited() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;
        let client = reqwest::Client::new();
        assert_eq!(
            fetch_usage(&client, &server.uri(), "t").await.unwrap_err(),
            ApiError::RateLimited
        );
    }

    #[tokio::test]
    async fn maps_other_statuses_to_http() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let client = reqwest::Client::new();
        assert_eq!(
            fetch_usage(&client, &server.uri(), "t").await.unwrap_err(),
            ApiError::Http(503)
        );
    }

    #[tokio::test]
    async fn maps_garbage_body_to_parse() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("<html>", "text/html"))
            .mount(&server)
            .await;
        let client = reqwest::Client::new();
        assert_eq!(
            fetch_usage(&client, &server.uri(), "t").await.unwrap_err(),
            ApiError::Parse
        );
    }

    /// Spec §12.1. Port 1 is reserved and never listening, so this fails fast
    /// and deterministically without touching the network.
    #[tokio::test]
    async fn network_errors_never_contain_the_token() {
        let secret = "sk-ant-oat01-SECRETVALUE";
        let client = reqwest::Client::new();
        let err = fetch_usage(&client, "http://127.0.0.1:1", secret)
            .await
            .unwrap_err();
        let rendered = format!("{err} {err:?}");
        assert!(!rendered.contains("SECRETVALUE"), "leaked: {rendered}");
    }
}
