use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CredentialError {
    #[error("no Claude Code credentials found")]
    NotFound,
    #[error("Claude Code credentials could not be read")]
    Unreadable,
}

#[derive(Deserialize)]
struct CredentialFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: OauthBlock,
}

#[derive(Deserialize)]
struct OauthBlock {
    #[serde(rename = "accessToken")]
    access_token: String,
}

/// The pure half: JSON in, token out.
///
/// Both error variants render a fixed string. The parse error from serde is
/// deliberately discarded rather than wrapped, because serde includes the input
/// span in its message and that span can contain the token (spec §12.1).
pub fn parse_token(json: &str) -> Result<String, CredentialError> {
    let parsed: CredentialFile =
        serde_json::from_str(json).map_err(|_| CredentialError::Unreadable)?;
    if parsed.claude_ai_oauth.access_token.is_empty() {
        return Err(CredentialError::Unreadable);
    }
    Ok(parsed.claude_ai_oauth.access_token)
}

/// Read the token for the current platform.
///
/// Never caches: Claude Code rotates the token and a cached copy goes stale
/// (spec §4.4). Never writes anything back.
pub fn read_token() -> Result<String, CredentialError> {
    parse_token(&read_raw()?)
}

#[cfg(target_os = "macos")]
fn read_raw() -> Result<String, CredentialError> {
    use std::process::Command;

    // Shelling out to /usr/bin/security rather than reading in-process:
    // the Keychain ACL check then applies to Apple's signed binary instead of
    // our unsigned one, which avoids a prompt on every launch (spec §4.4).
    // The token arrives on stdout, never in argv, so it is not visible in `ps`.
    let output = Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .map_err(|_| CredentialError::NotFound)?;

    if !output.status.success() {
        return Err(CredentialError::NotFound);
    }
    String::from_utf8(output.stdout).map_err(|_| CredentialError::Unreadable)
}

#[cfg(not(target_os = "macos"))]
fn read_raw() -> Result<String, CredentialError> {
    let path = dirs::home_dir()
        .ok_or(CredentialError::NotFound)?
        .join(".claude")
        .join(".credentials.json");
    std::fs::read_to_string(path).map_err(|_| CredentialError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_access_token() {
        let json = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-abc","expiresAt":123}}"#;
        assert_eq!(parse_token(json).unwrap(), "sk-ant-oat01-abc");
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(
            parse_token("not json at all"),
            Err(CredentialError::Unreadable)
        ));
    }

    #[test]
    fn rejects_missing_oauth_object() {
        assert!(matches!(
            parse_token(r#"{"somethingElse":true}"#),
            Err(CredentialError::Unreadable)
        ));
    }

    #[test]
    fn rejects_missing_access_token() {
        assert!(matches!(
            parse_token(r#"{"claudeAiOauth":{"expiresAt":123}}"#),
            Err(CredentialError::Unreadable)
        ));
    }

    #[test]
    fn rejects_empty_access_token() {
        assert!(matches!(
            parse_token(r#"{"claudeAiOauth":{"accessToken":""}}"#),
            Err(CredentialError::Unreadable)
        ));
    }

    /// Spec §12.1: no error rendering may leak token material.
    ///
    /// The second input matters more than the first: serde's *type mismatch* errors
    /// quote the offending value verbatim ("invalid type: string \"sk-ant-...\""),
    /// whereas its syntax and EOF errors carry only a line and column. An
    /// implementation that wrapped serde's error would pass on the truncated input
    /// and fail on this one.
    #[test]
    fn errors_never_contain_token_material() {
        let secret = "sk-ant-oat01-SECRETVALUE";
        let inputs = [
            format!(r#"{{"claudeAiOauth":{{"accessToken":"{secret}","bad"#),
            format!(r#"{{"claudeAiOauth":"{secret}"}}"#),
        ];
        for json in inputs {
            let err = parse_token(&json).unwrap_err();
            let rendered = format!("{err} {err:?}");
            assert!(!rendered.contains("SECRETVALUE"), "leaked: {rendered}");
            assert!(!rendered.contains("sk-ant"), "leaked: {rendered}");
        }
    }
}
