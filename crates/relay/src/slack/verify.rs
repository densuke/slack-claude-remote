//! Slack request signature verification (spec ss5.1).

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

#[derive(Debug, PartialEq)]
pub enum VerifyError {
    BadTimestamp,
    Stale,
    BadSignature,
}

/// Computes the "v0=<hex>" signature Slack expects.
pub fn sign(secret: &str, timestamp: &str, body: &[u8]) -> String {
    let base = base_string(timestamp, body);
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(&base);
    format!("v0={}", hex::encode(mac.finalize().into_bytes()))
}

/// Verifies a Slack request signature per spec ss5.1.
pub fn verify(
    secret: &str,
    timestamp: &str,
    body: &[u8],
    signature: &str,
    now_unix: i64,
) -> Result<(), VerifyError> {
    let ts: i64 = timestamp.parse().map_err(|_| VerifyError::BadTimestamp)?;
    if (now_unix - ts).abs() > 300 {
        return Err(VerifyError::Stale);
    }

    let hex_part = signature
        .strip_prefix("v0=")
        .ok_or(VerifyError::BadSignature)?;
    let given = hex::decode(hex_part).map_err(|_| VerifyError::BadSignature)?;

    let base = base_string(timestamp, body);
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(&base);
    let expected = mac.finalize().into_bytes();

    if expected.ct_eq(&given).into() {
        Ok(())
    } else {
        Err(VerifyError::BadSignature)
    }
}

fn base_string(timestamp: &str, body: &[u8]) -> Vec<u8> {
    let mut base = format!("v0:{timestamp}:").into_bytes();
    base.extend_from_slice(body);
    base
}

#[cfg(test)]
mod tests;
