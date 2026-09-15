use super::*;

const SECRET: &str = "test-signing-secret";
const TS: &str = "1789560000";
const BODY: &[u8] = b"token=x&team_id=T0001&user_id=U0123&command=%2Fcc&text=list";
const NOW: i64 = 1789560000;

#[test]
fn valid_signature_ok() {
    let sig = sign(SECRET, TS, BODY);
    assert_eq!(verify(SECRET, TS, BODY, &sig, NOW), Ok(()));
}

#[test]
fn known_vector() {
    let expected = "v0=a19c040b7bcee240257b5dc0fc2c1601cce758204a931ec68a6e95b13446607a";
    assert_eq!(sign(SECRET, TS, BODY), expected);
    assert_eq!(verify(SECRET, TS, BODY, expected, NOW), Ok(()));
}

#[test]
fn tampered_body_fails() {
    let sig = sign(SECRET, TS, BODY);
    assert_eq!(
        verify(
            SECRET,
            TS,
            b"token=y&team_id=T0001&user_id=U0123&command=%2Fcc&text=list",
            &sig,
            NOW
        ),
        Err(VerifyError::BadSignature)
    );
}

#[test]
fn wrong_secret_fails() {
    let sig = sign(SECRET, TS, BODY);
    assert_eq!(
        verify("other-secret", TS, BODY, &sig, NOW),
        Err(VerifyError::BadSignature)
    );
}

#[test]
fn old_timestamp_fails() {
    let ts = "1789560000";
    let now = 1789560000 + 301;
    let sig = sign(SECRET, ts, BODY);
    assert_eq!(verify(SECRET, ts, BODY, &sig, now), Err(VerifyError::Stale));
}

#[test]
fn future_timestamp_fails() {
    let ts = "1789560000";
    let now = 1789560000 - 301;
    let sig = sign(SECRET, ts, BODY);
    assert_eq!(verify(SECRET, ts, BODY, &sig, now), Err(VerifyError::Stale));
}

#[test]
fn boundary_300_ok() {
    let ts = "1789560000";
    let now = 1789560000 + 300;
    let sig = sign(SECRET, ts, BODY);
    assert_eq!(verify(SECRET, ts, BODY, &sig, now), Ok(()));
}

#[test]
fn non_numeric_timestamp_fails() {
    let sig = sign(SECRET, "not-a-number", BODY);
    assert_eq!(
        verify(SECRET, "not-a-number", BODY, &sig, NOW),
        Err(VerifyError::BadTimestamp)
    );
}

#[test]
fn missing_v0_prefix_fails() {
    let sig = sign(SECRET, TS, BODY);
    let bad = sig.strip_prefix("v0=").unwrap();
    assert_eq!(
        verify(SECRET, TS, BODY, bad, NOW),
        Err(VerifyError::BadSignature)
    );
}

#[test]
fn non_hex_signature_fails() {
    assert_eq!(
        verify(SECRET, TS, BODY, "v0=not-hex-zzzz", NOW),
        Err(VerifyError::BadSignature)
    );
}
