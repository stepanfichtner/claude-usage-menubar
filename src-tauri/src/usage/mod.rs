pub mod normalize;
pub mod raw;

pub use normalize::normalize;

use crate::error::ApiError;
use crate::http::send_authed;
use crate::model::Quota;

pub const API_BASE: &str = "https://api.anthropic.com";

pub async fn fetch_usage(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<Vec<Quota>, ApiError> {
    let url = format!("{base_url}/api/oauth/usage?at_wall=1&skip_spend=1");
    let body = send_authed(client, url, token).await?;
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
}
