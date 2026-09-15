use super::*;

#[test]
fn pending_state_consumed_once() {
    let mut s = Sessions::new();
    let (state, nonce) = s.begin(0);
    assert_eq!(s.take_pending(&state, 10), Some(nonce));
    assert_eq!(s.take_pending(&state, 10), None);
}

#[test]
fn pending_state_expires() {
    let mut s = Sessions::new();
    let (state, _nonce) = s.begin(0);
    assert_eq!(s.take_pending(&state, 601), None);
}

#[test]
fn unknown_state_none() {
    let mut s = Sessions::new();
    assert_eq!(s.take_pending("nope", 0), None);
}

#[test]
fn cookie_roundtrip() {
    let mut s = Sessions::new();
    let sid = s.create_cookie("U1", 0);
    assert_eq!(s.user_for(&sid, 100), Some("U1".to_string()));
}

#[test]
fn cookie_expires() {
    let mut s = Sessions::new();
    let sid = s.create_cookie("U1", 0);
    assert_eq!(s.user_for(&sid, 86400), None);
}

#[test]
fn random_token_is_43_chars_and_unique() {
    let a = random_token();
    let b = random_token();
    assert_eq!(a.chars().count(), 43);
    assert_eq!(b.chars().count(), 43);
    assert_ne!(a, b);
}
