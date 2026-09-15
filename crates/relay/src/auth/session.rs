//! In-memory OIDC pending-state and cookie-session storage. See spec 9.5.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use std::collections::HashMap;

/// Random 32 bytes, base64url (no padding) encoded: 43 characters.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[derive(Default)]
pub struct Sessions {
    pending: HashMap<String, (String, i64)>,
    cookies: HashMap<String, (String, i64)>,
}

impl Sessions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a login attempt: a fresh (state, nonce) pair, valid 600s.
    pub fn begin(&mut self, now: i64) -> (String, String) {
        let state = random_token();
        let nonce = random_token();
        self.pending
            .insert(state.clone(), (nonce.clone(), now + 600));
        (state, nonce)
    }

    /// Consume a pending state exactly once. Returns None if unknown,
    /// already used, or expired.
    pub fn take_pending(&mut self, state: &str, now: i64) -> Option<String> {
        let (nonce, expires) = self.pending.remove(state)?;
        if now < expires { Some(nonce) } else { None }
    }

    /// Create a cookie session for `user`, valid 86400s.
    pub fn create_cookie(&mut self, user: &str, now: i64) -> String {
        let sid = random_token();
        self.cookies
            .insert(sid.clone(), (user.to_string(), now + 86400));
        sid
    }

    /// Look up the user behind a cookie, if still valid.
    pub fn user_for(&self, sid: &str, now: i64) -> Option<String> {
        let (user, expires) = self.cookies.get(sid)?;
        if now < *expires {
            Some(user.clone())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests;
