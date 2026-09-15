use super::*;

fn make_jwt(payload: &serde_json::Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(b"{}");
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string());
    format!("{header}.{payload_b64}.sig")
}

fn base_claims(now: i64, client_id: &str, nonce: &str) -> serde_json::Value {
    serde_json::json!({
        "iss": "https://slack.com",
        "sub": "U0123ABCD",
        "aud": client_id,
        "exp": now + 300,
        "nonce": nonce,
        "name": "Me",
    })
}

#[test]
fn authorize_url_contains_required_params() {
    let url = authorize_url(
        "CID123",
        "https://sccr.example.jp/auth/callback",
        "st4te",
        "n0nce",
    );
    assert!(url.starts_with("https://slack.com/openid/connect/authorize?"));
    assert!(url.contains("response_type=code"));
    assert!(url.contains("scope=openid%20profile"));
    assert!(url.contains("client_id=CID123"));
    assert!(url.contains("state=st4te"));
    assert!(url.contains("nonce=n0nce"));
    assert!(url.contains("redirect_uri=https%3A%2F%2Fsccr.example.jp%2Fauth%2Fcallback"));
}

#[test]
fn id_token_ok_extracts_sub() {
    let claims = base_claims(1_000, "CID123", "n0nce");
    let jwt = make_jwt(&claims);
    let out = parse_id_token(&jwt, "CID123", "n0nce", 1_000).unwrap();
    assert_eq!(
        out,
        IdClaims {
            sub: "U0123ABCD".to_string(),
            name: Some("Me".to_string()),
        }
    );
}

#[test]
fn id_token_nonce_mismatch() {
    let claims = base_claims(1_000, "CID123", "n0nce");
    let jwt = make_jwt(&claims);
    assert_eq!(
        parse_id_token(&jwt, "CID123", "other", 1_000),
        Err(OidcError::Nonce)
    );
}

#[test]
fn id_token_aud_mismatch() {
    let claims = base_claims(1_000, "CID123", "n0nce");
    let jwt = make_jwt(&claims);
    assert_eq!(
        parse_id_token(&jwt, "OTHER", "n0nce", 1_000),
        Err(OidcError::Audience)
    );
}

#[test]
fn id_token_iss_mismatch() {
    let mut claims = base_claims(1_000, "CID123", "n0nce");
    claims["iss"] = serde_json::json!("https://evil.example");
    let jwt = make_jwt(&claims);
    assert_eq!(
        parse_id_token(&jwt, "CID123", "n0nce", 1_000),
        Err(OidcError::Issuer)
    );
}

#[test]
fn id_token_expired() {
    let mut claims = base_claims(1_000, "CID123", "n0nce");
    claims["exp"] = serde_json::json!(999);
    let jwt = make_jwt(&claims);
    assert_eq!(
        parse_id_token(&jwt, "CID123", "n0nce", 1_000),
        Err(OidcError::Expired)
    );
}

#[test]
fn id_token_malformed() {
    assert_eq!(
        parse_id_token("not-a-jwt", "CID123", "n0nce", 1_000),
        Err(OidcError::Malformed)
    );
    assert_eq!(
        parse_id_token("a.b", "CID123", "n0nce", 1_000),
        Err(OidcError::Malformed)
    );
    let bad_payload = format!(
        "{}.{}.sig",
        URL_SAFE_NO_PAD.encode(b"{}"),
        URL_SAFE_NO_PAD.encode(b"not-json")
    );
    assert_eq!(
        parse_id_token(&bad_payload, "CID123", "n0nce", 1_000),
        Err(OidcError::Malformed)
    );
}
