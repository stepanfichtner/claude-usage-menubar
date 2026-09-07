use crate::error::ApiError;

pub fn user_agent() -> String {
    format!("claude-usage-menubar/{}", env!("CARGO_PKG_VERSION"))
}

/// GET `url` with the OAuth token, returning the response body.
///
/// The token travels in a header via `bearer_auth` and never in the URL. That is
/// what makes `ApiError::Network` safe to carry reqwest's message: reqwest's
/// error rendering includes the URL but never headers (spec §12.1).
pub async fn send_authed(
    client: &reqwest::Client,
    url: String,
    token: &str,
) -> Result<String, ApiError> {
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

    response
        .text()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn maps_401_to_signed_out() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let client = reqwest::Client::new();
        assert_eq!(
            send_authed(&client, server.uri(), "t").await.unwrap_err(),
            ApiError::SignedOut
        );
    }

    #[tokio::test]
    async fn maps_403_to_signed_out() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let client = reqwest::Client::new();
        assert_eq!(
            send_authed(&client, server.uri(), "t").await.unwrap_err(),
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
            send_authed(&client, server.uri(), "t").await.unwrap_err(),
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
            send_authed(&client, server.uri(), "t").await.unwrap_err(),
            ApiError::Http(503)
        );
    }

    /// Spec §12.1. Port 1 is reserved and never listening, so this fails fast
    /// and deterministically without a real peer on the other end.
    #[tokio::test]
    async fn network_errors_never_contain_the_token() {
        let secret = "sk-ant-oat01-SECRETVALUE";
        let client = reqwest::Client::new();
        let err = send_authed(&client, "http://127.0.0.1:1".to_string(), secret)
            .await
            .unwrap_err();
        let rendered = format!("{err} {err:?}");
        assert!(!rendered.contains("SECRETVALUE"), "leaked: {rendered}");
    }
}
