/// Every variant renders without token material (spec §12.1).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    #[error("not signed in to Claude Code")]
    SignedOut,
    #[error("rate limited by the API")]
    RateLimited,
    #[error("unexpected response: HTTP {0}")]
    Http(u16),
    #[error("network error: {0}")]
    Network(String),
    #[error("could not parse the API response")]
    Parse,
}
