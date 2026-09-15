use protocol::{Behavior, RelayMsg};
use serde_json::json;

use super::*;
use crate::permission;
use crate::state::Pending;

fn thread_event_text(event_id: &str, user: &str, text: &str) -> String {
    json!({
        "type": "event_callback",
        "event_id": event_id,
        "event": {
            "type": "message", "channel": CHANNEL, "user": user,
            "text": text,
            "ts": "1789560100.000200", "thread_ts": THREAD, "channel_type": "channel"
        }
    })
    .to_string()
}

#[tokio::test]
async fn permission_request_posts_prompt() {
    let s = setup();
    bind(&s.state, CHAT, "mac:proj", USER).await;

    permission::handle_request(
        &s.state,
        "mac:proj",
        "abcde".into(),
        "Bash",
        "List files",
        "{\"command\":\"ls\"}",
    )
    .await;

    let calls = wait_calls(&s.slack, 1).await;
    assert_eq!(
        calls,
        vec![Call::Post {
            channel: CHANNEL.into(),
            thread_ts: Some(THREAD.into()),
            text: "Claude wants to run *Bash*: List files\n\
                   ```{\"command\":\"ls\"}```\n\
                   Reply \"yes abcde\" or \"no abcde\" in this thread."
                .into(),
        }]
    );
    let pending = s.state.pending.lock().await;
    assert_eq!(
        pending.get("abcde"),
        Some(&Pending {
            chat_id: CHAT.into(),
            session: "mac:proj".into(),
            created: NOW,
        })
    );
}

#[tokio::test]
async fn permission_request_without_binding_dropped() {
    let s = setup();
    permission::handle_request(&s.state, "mac:proj", "abcde".into(), "Bash", "x", "y").await;
    assert!(s.slack.calls().is_empty());
    assert!(s.state.pending.lock().await.is_empty());
}

#[tokio::test]
async fn yes_reply_sends_allow_and_no_inbound() {
    let s = setup();
    bind(&s.state, CHAT, "mac:proj", USER).await;
    s.state.pending.lock().await.insert(
        "abcde".into(),
        Pending {
            chat_id: CHAT.into(),
            session: "mac:proj".into(),
            created: NOW,
        },
    );
    let mut rx = register(&s.state, "mac:proj").await;

    let req = signed(
        "/slack/events",
        &thread_event_text("Ev1", USER, "yes abcde"),
    );
    assert_eq!(call(&s.state, req).await.0, StatusCode::OK);

    let msg = timeout(Duration::from_secs(1), rx.recv()).await.unwrap();
    assert_eq!(
        msg,
        Some(RelayMsg::PermissionVerdict {
            request_id: "abcde".into(),
            behavior: Behavior::Allow,
        })
    );
    assert_silent(&mut rx).await;
    assert!(s.state.pending.lock().await.is_empty());
}

#[tokio::test]
async fn verdict_from_other_user_is_plain_inbound_or_dropped() {
    let slack = Arc::new(FakeSlack::new());
    let store = Store {
        users: vec![USER.into(), "U0OTHER".into()],
        tokens: vec![],
        bindings: Default::default(),
    };
    let state = Arc::new(AppState::new(
        config(false, SECRET),
        store,
        slack.clone(),
        fixed_now,
    ));
    bind(&state, CHAT, "mac:proj", USER).await;
    state.pending.lock().await.insert(
        "abcde".into(),
        Pending {
            chat_id: CHAT.into(),
            session: "mac:proj".into(),
            created: NOW,
        },
    );
    let mut rx = register(&state, "mac:proj").await;

    // Allowed user, but not the binding's creator: plain Inbound, verdict untouched.
    let req = signed(
        "/slack/events",
        &thread_event_text("Ev1", "U0OTHER", "yes abcde"),
    );
    assert_eq!(call(&state, req).await.0, StatusCode::OK);
    let msg = timeout(Duration::from_secs(1), rx.recv()).await.unwrap();
    assert_eq!(
        msg,
        Some(RelayMsg::Inbound {
            chat_id: CHAT.into(),
            user: "U0OTHER".into(),
            text: "yes abcde".into(),
        })
    );
    assert!(state.pending.lock().await.contains_key("abcde"));

    // Not an allowed user at all: dropped entirely, nothing posted.
    let req = signed(
        "/slack/events",
        &thread_event_text("Ev2", "U9999OUT", "yes abcde"),
    );
    assert_eq!(call(&state, req).await.0, StatusCode::OK);
    assert_silent(&mut rx).await;
    assert!(slack.calls().is_empty());
    assert!(state.pending.lock().await.contains_key("abcde"));
}

#[tokio::test]
async fn unknown_request_id_is_normal_inbound() {
    let s = setup();
    bind(&s.state, CHAT, "mac:proj", USER).await;
    let mut rx = register(&s.state, "mac:proj").await;

    let req = signed(
        "/slack/events",
        &thread_event_text("Ev1", USER, "yes zzzzz"),
    );
    assert_eq!(call(&s.state, req).await.0, StatusCode::OK);
    let msg = timeout(Duration::from_secs(1), rx.recv()).await.unwrap();
    assert_eq!(
        msg,
        Some(RelayMsg::Inbound {
            chat_id: CHAT.into(),
            user: USER.into(),
            text: "yes zzzzz".into(),
        })
    );
}
