use super::*;
use serde_json::json;

#[test]
fn url_verification_detected() {
    let body = json!({
        "token": "Jhj5dZrVaK7ZwHHjRyZWjbDl",
        "challenge": "3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P",
        "type": "url_verification"
    });
    assert_eq!(
        classify(&body),
        Envelope::UrlVerification {
            challenge: "3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P".to_string()
        }
    );
}

fn message_event(event: serde_json::Value, event_id: &str) -> serde_json::Value {
    json!({
        "token": "XXYYZZ",
        "team_id": "T0001ABCD",
        "api_app_id": "A0001ABCD",
        "event": event,
        "type": "event_callback",
        "event_id": event_id,
        "event_time": 1789560100,
    })
}

#[test]
fn thread_reply_is_candidate() {
    let body = message_event(
        json!({
            "type": "message",
            "channel": "C0AB12CD3",
            "user": "U0123ABCD",
            "text": "What does main.rs do?",
            "ts": "1789560100.000200",
            "thread_ts": "1789560000.000100",
            "channel_type": "channel"
        }),
        "Ev0001ABCD",
    );
    assert_eq!(
        classify(&body),
        Envelope::Message {
            event_id: "Ev0001ABCD".to_string(),
            candidate: Some(Candidate {
                chat_id: "C0AB12CD3:1789560000.000100".to_string(),
                channel: "C0AB12CD3".to_string(),
                thread_ts: "1789560000.000100".to_string(),
                user: "U0123ABCD".to_string(),
                text: "What does main.rs do?".to_string(),
            })
        }
    );
}

#[test]
fn top_level_message_not_candidate() {
    let body = message_event(
        json!({
            "type": "message",
            "channel": "C0AB12CD3",
            "user": "U0123ABCD",
            "text": "hello channel",
            "ts": "1789560200.000300",
            "channel_type": "channel"
        }),
        "Ev0002ABCD",
    );
    assert_eq!(
        classify(&body),
        Envelope::Message {
            event_id: "Ev0002ABCD".to_string(),
            candidate: None
        }
    );
}

#[test]
fn thread_parent_not_candidate() {
    let body = message_event(
        json!({
            "type": "message",
            "channel": "C0AB12CD3",
            "user": "U0123ABCD",
            "text": "root message",
            "ts": "1789560000.000100",
            "thread_ts": "1789560000.000100",
            "channel_type": "channel"
        }),
        "Ev0002B",
    );
    assert_eq!(
        classify(&body),
        Envelope::Message {
            event_id: "Ev0002B".to_string(),
            candidate: None
        }
    );
}

#[test]
fn bot_message_not_candidate() {
    let body = message_event(
        json!({
            "type": "message",
            "channel": "C0AB12CD3",
            "user": "U0BOT0001",
            "bot_id": "B0001ABCD",
            "app_id": "A0001ABCD",
            "text": "Done. I updated README.md.",
            "ts": "1789560300.000400",
            "thread_ts": "1789560000.000100",
            "channel_type": "channel"
        }),
        "Ev0003ABCD",
    );
    assert_eq!(
        classify(&body),
        Envelope::Message {
            event_id: "Ev0003ABCD".to_string(),
            candidate: None
        }
    );
}

#[test]
fn subtype_not_candidate() {
    let changed = message_event(
        json!({
            "type": "message",
            "subtype": "message_changed",
            "channel": "C0AB12CD3",
            "ts": "1789560500.000600",
            "channel_type": "channel"
        }),
        "Ev0005ABCD",
    );
    assert_eq!(
        classify(&changed),
        Envelope::Message {
            event_id: "Ev0005ABCD".to_string(),
            candidate: None
        }
    );

    let joined = message_event(
        json!({
            "type": "message",
            "subtype": "channel_join",
            "channel": "C0AB12CD3",
            "user": "U0123ABCD",
            "ts": "1789560600.000700",
            "thread_ts": "1789560000.000100",
            "channel_type": "channel"
        }),
        "Ev0006ABCD",
    );
    assert_eq!(
        classify(&joined),
        Envelope::Message {
            event_id: "Ev0006ABCD".to_string(),
            candidate: None
        }
    );
}

#[test]
fn non_message_event_is_other() {
    let body = json!({
        "token": "XXYYZZ",
        "team_id": "T0001ABCD",
        "api_app_id": "A0001ABCD",
        "event": {"type": "reaction_added"},
        "type": "event_callback",
        "event_id": "Ev0007ABCD",
    });
    assert_eq!(classify(&body), Envelope::Other);
}

#[test]
fn private_channel_message_same_as_public() {
    let body = message_event(
        json!({
            "type": "message",
            "channel": "G0AB12CD3",
            "user": "U0123ABCD",
            "text": "private thread reply",
            "ts": "1789560100.000200",
            "thread_ts": "1789560000.000100",
            "channel_type": "group"
        }),
        "Ev0008ABCD",
    );
    assert_eq!(
        classify(&body),
        Envelope::Message {
            event_id: "Ev0008ABCD".to_string(),
            candidate: Some(Candidate {
                chat_id: "G0AB12CD3:1789560000.000100".to_string(),
                channel: "G0AB12CD3".to_string(),
                thread_ts: "1789560000.000100".to_string(),
                user: "U0123ABCD".to_string(),
                text: "private thread reply".to_string(),
            })
        }
    );
}

#[test]
fn dedupe_drops_second() {
    let mut d = Dedupe::new(10);
    assert!(d.first_seen("a"));
    assert!(!d.first_seen("a"));
}

#[test]
fn dedupe_evicts_oldest() {
    let mut d = Dedupe::new(2);
    assert!(d.first_seen("a"));
    assert!(d.first_seen("b"));
    assert!(d.first_seen("c"));
    assert!(d.first_seen("a"));
}
