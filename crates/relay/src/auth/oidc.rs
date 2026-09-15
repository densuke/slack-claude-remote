//! Slack OpenID Connect: authorize URL, id_token verification, token exchange.
//! See spec section 9.

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;

const SLACK_ISSUER: &str = "https://slack.com";

/// Percent-encode per RFC 3986 unreserved characters (A-Z a-z 0-9 - . _ ~).
/// Everything else becomes %XX with uppercase hex.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build the `GET /login` redirect target (spec 9.1).
pub fn authorize_url(client_id: &str, redirect_uri: &str, state: &str, nonce: &str) -> String {
    format!(
        "https://slack.com/openid/connect/authorize?response_type=code&scope=openid%20profile&client_id={}&state={}&nonce={}&redirect_uri={}",
        percent_encode(client_id),
        percent_encode(state),
        percent_encode(nonce),
        percent_encode(redirect_uri),
    )
}

#[derive(Debug, PartialEq)]
pub struct IdClaims {
    pub sub: String,
    pub name: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum OidcError {
    Malformed,
    Issuer,
    Audience,
    Nonce,
    Expired,
    Http(String),
}

fn field_str<'a>(
    obj: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    obj.get(key).and_then(|v| v.as_str())
}

/// Validate and extract claims from a Slack id_token (spec 9.4). The JWT
/// signature is intentionally not verified: the token arrives over a direct
/// TLS token exchange with Slack (see README section 4.6).
pub fn parse_id_token(
    jwt: &str,
    client_id: &str,
    nonce: &str,
    now_unix: i64,
) -> Result<IdClaims, OidcError> {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return Err(OidcError::Malformed);
    }

    let payload_b64 = parts[1].trim_end_matches('=');
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| OidcError::Malformed)?;
    let value: serde_json::Value =
        serde_json::from_slice(&payload_bytes).map_err(|_| OidcError::Malformed)?;
    let obj = value.as_object().ok_or(OidcError::Malformed)?;

    if field_str(obj, "iss") != Some(SLACK_ISSUER) {
        return Err(OidcError::Issuer);
    }
    if field_str(obj, "aud") != Some(client_id) {
        return Err(OidcError::Audience);
    }
    if field_str(obj, "nonce") != Some(nonce) {
        return Err(OidcError::Nonce);
    }
    match obj.get("exp").and_then(|v| v.as_i64()) {
        Some(exp) if exp > now_unix => {}
        _ => return Err(OidcError::Expired),
    }
    let sub = field_str(obj, "sub")
        .ok_or(OidcError::Malformed)?
        .to_string();
    let name = field_str(obj, "name").map(String::from);

    Ok(IdClaims { sub, name })
}

#[derive(Deserialize)]
struct TokenResponse {
    ok: bool,
    id_token: Option<String>,
    error: Option<String>,
}

/// The `openid.connect.token` exchange (spec 9.3), abstracted for testing.
#[async_trait]
pub trait OidcHttp: Send + Sync {
    async fn token(
        &self,
        code: &str,
        client_id: &str,
        client_secret: &str,
        redirect_uri: &str,
    ) -> Result<String, OidcError>;
}

/// Real implementation using reqwest. Not unit-tested (see spec appendix on
/// intentionally untested HTTP boundaries); must compile.
pub struct HttpOidc;

#[async_trait]
impl OidcHttp for HttpOidc {
    async fn token(
        &self,
        code: &str,
        client_id: &str,
        client_secret: &str,
        redirect_uri: &str,
    ) -> Result<String, OidcError> {
        let client = reqwest::Client::new();
        let resp = client
            .post("https://slack.com/api/openid.connect.token")
            .form(&[
                ("code", code),
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("redirect_uri", redirect_uri),
            ])
            .send()
            .await
            .map_err(|e| OidcError::Http(e.to_string()))?;
        let body: TokenResponse = resp
            .json()
            .await
            .map_err(|e| OidcError::Http(e.to_string()))?;
        if body.ok {
            body.id_token
                .ok_or_else(|| OidcError::Http("missing id_token".to_string()))
        } else {
            Err(OidcError::Http(body.error.unwrap_or_default()))
        }
    }
}

#[cfg(test)]
mod tests;
