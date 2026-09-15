use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use protocol::RelayMsg;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tower::ServiceExt;

use super::router;
use crate::auth::session::random_token;
use crate::config::Config;
use crate::slack::api::{Call, FakeSlack, SlackError};
use crate::slack::verify::sign;
use crate::state::AppState;
use crate::store::{Binding, Store, TokenRecord};

mod ws;

const SECRET: &str = "test-signing-secret";
const NOW: i64 = 1789560000;
const USER: &str = "U0123ABCD";
const CHANNEL: &str = "C0AB12CD3";
const THREAD: &str = "1789560000.000100";
const CHAT: &str = "C0AB12CD3:1789560000.000100";
const RESPONSE_URL: &str = "https://hooks.slack.com/actions/T0001ABCD/1/abc";
const DEV_SHA256: &str = "ef260e9aa3c673af240d17a2660480361a8e081d1ffeca2a5ed0e3219fc18567";

fn fixed_now() -> i64 {
    NOW
}

struct Setup {
    state: Arc<AppState>,
    slack: Arc<FakeSlack>,
}

fn config(dev: bool, secret: &str) -> Config {
    let dir = std::env::temp_dir().join(format!("sccr-routes-{}", random_token()));
    std::fs::create_dir_all(&dir).unwrap();
    Config {
        bind: "127.0.0.1:0".into(),
        public_url: "https://sccr.example.jp".into(),
        state_file: dir.join("state.json"),
        admin_user: USER.into(),
        signing_secret: secret.into(),
        bot_token: "xoxb-test".into(),
        client_id: "cid".into(),
        client_secret: "csecret".into(),
        dev,
    }
}

fn setup_with(slack: FakeSlack, cfg: Config, tokens: bool) -> Setup {
    let slack = Arc::new(slack);
    let store = Store {
        users: vec![USER.into()],
        tokens: if tokens {
            vec![TokenRecord {
                sha256: DEV_SHA256.into(),
                label: "macbook".into(),
                created: "2026-09-16T00:00:00Z".into(),
            }]
        } else {
            vec![]
        },
        bindings: Default::default(),
    };
    let state = Arc::new(AppState::new(cfg, store, slack.clone(), fixed_now));
    Setup { state, slack }
}

fn setup() -> Setup {
    setup_with(FakeSlack::new(), config(false, SECRET), true)
}

async fn bind(state: &AppState, chat_id: &str, session: &str, user: &str) {
    state.store.lock().await.bindings.insert(
        chat_id.into(),
        Binding {
            session: session.into(),
            user: user.into(),
            created: 1,
        },
    );
}

async fn register(state: &AppState, name: &str) -> mpsc::Receiver<RelayMsg> {
    let (tx, rx) = mpsc::channel(8);
    state.hub.register(name, tx).await.unwrap();
    rx
}

fn signed_with(uri: &str, body: &str, secret: &str, ts: i64) -> Request<Body> {
    let ts = ts.to_string();
    Request::post(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("X-Slack-Request-Timestamp", &ts)
        .header("X-Slack-Signature", sign(secret, &ts, body.as_bytes()))
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn signed(uri: &str, body: &str) -> Request<Body> {
    signed_with(uri, body, SECRET, NOW)
}

async fn call(state: &Arc<AppState>, req: Request<Body>) -> (StatusCode, String) {
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

fn thread_event(event_id: &str, user: &str) -> String {
    json!({
        "type": "event_callback",
        "event_id": event_id,
        "event": {
            "type": "message", "channel": CHANNEL, "user": user,
            "text": "What does main.rs do?",
            "ts": "1789560100.000200", "thread_ts": THREAD, "channel_type": "channel"
        }
    })
    .to_string()
}

fn command_body(user: &str, text: &str) -> String {
    serde_urlencoded::to_string([
        ("token", "x"),
        ("team_id", "T0001ABCD"),
        ("channel_id", CHANNEL),
        ("user_id", user),
        ("command", "/cc"),
        ("text", text),
        (
            "response_url",
            "https://hooks.slack.com/commands/T0001ABCD/1/x",
        ),
    ])
    .unwrap()
}

fn interaction_body(user: &str, session: &str) -> String {
    let payload = json!({
        "type": "block_actions",
        "user": {"id": user},
        "channel": {"id": CHANNEL, "name": "dev"},
        "response_url": RESPONSE_URL,
        "actions": [{
            "type": "static_select",
            "action_id": "sccr_select",
            "selected_option": {"text": {"type": "plain_text", "text": session}, "value": session}
        }]
    });
    serde_urlencoded::to_string([("payload", payload.to_string())]).unwrap()
}

async fn wait_calls(slack: &FakeSlack, n: usize) -> Vec<Call> {
    timeout(Duration::from_secs(1), async {
        loop {
            let calls = slack.calls();
            if calls.len() >= n {
                return calls;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed out waiting for slack calls")
}

async fn assert_silent(rx: &mut mpsc::Receiver<RelayMsg>) {
    assert!(
        timeout(Duration::from_millis(200), rx.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn healthz_ok() {
    let s = setup();
    let req = Request::get("/healthz").body(Body::empty()).unwrap();
    assert_eq!(call(&s.state, req).await, (StatusCode::OK, "ok".into()));
}

#[tokio::test]
async fn events_bad_signature_401() {
    let s = setup();
    let body = thread_event("Ev1", USER);
    let challenge = r#"{"type":"url_verification","challenge":"abc"}"#;
    let unauthorized = (StatusCode::UNAUTHORIZED, String::new());

    let wrong_secret = signed_with("/slack/events", &body, "other", NOW);
    assert_eq!(call(&s.state, wrong_secret).await, unauthorized);
    let verification = signed_with("/slack/events", challenge, "other", NOW);
    assert_eq!(call(&s.state, verification).await, unauthorized);
    let stale = signed_with("/slack/events", &body, SECRET, NOW - 301);
    assert_eq!(call(&s.state, stale).await, unauthorized);
    let unsigned = Request::post("/slack/events")
        .body(Body::from(body.clone()))
        .unwrap();
    assert_eq!(call(&s.state, unsigned).await, unauthorized);

    let empty = setup_with(FakeSlack::new(), config(true, ""), true);
    let req = signed_with("/slack/events", challenge, "", NOW);
    assert_eq!(call(&empty.state, req).await, unauthorized);
}

#[tokio::test]
async fn url_verification_echoes_challenge() {
    let s = setup();
    let body = r#"{"token":"Jhj5dZrVaK7ZwHHjRyZWjbDl","challenge":"3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P","type":"url_verification"}"#;
    let resp = router(s.state.clone())
        .oneshot(signed("/slack/events", body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers()[header::CONTENT_TYPE].to_str().unwrap();
    assert!(ct.starts_with("text/plain"));
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        &bytes[..],
        b"3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P"
    );

    let (status, _) = call(&s.state, signed("/slack/events", "{not json")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unauthorized_user_dropped() {
    let s = setup();
    bind(&s.state, CHAT, "mac:proj", USER).await;
    let mut rx = register(&s.state, "mac:proj").await;
    let req = signed("/slack/events", &thread_event("Ev1", "U9999OUT"));
    assert_eq!(call(&s.state, req).await.0, StatusCode::OK);
    assert_silent(&mut rx).await;
    assert!(s.slack.calls().is_empty());
}

#[tokio::test]
async fn bound_thread_forwards_inbound() {
    let s = setup();
    bind(&s.state, CHAT, "mac:proj", USER).await;
    let mut rx = register(&s.state, "mac:proj").await;
    let req = signed("/slack/events", &thread_event("Ev1", USER));
    assert_eq!(call(&s.state, req).await, (StatusCode::OK, String::new()));
    let msg = timeout(Duration::from_secs(1), rx.recv()).await.unwrap();
    assert_eq!(
        msg,
        Some(RelayMsg::Inbound {
            chat_id: CHAT.into(),
            user: USER.into(),
            text: "What does main.rs do?".into(),
        })
    );
}

#[tokio::test]
async fn unbound_thread_ignored() {
    let s = setup();
    let mut rx = register(&s.state, "mac:proj").await;
    let req = signed("/slack/events", &thread_event("Ev1", USER));
    assert_eq!(call(&s.state, req).await.0, StatusCode::OK);
    assert_silent(&mut rx).await;
    assert!(s.slack.calls().is_empty());
}

#[tokio::test]
async fn duplicate_event_forwarded_once() {
    let s = setup();
    bind(&s.state, CHAT, "mac:proj", USER).await;
    let mut rx = register(&s.state, "mac:proj").await;
    let body = thread_event("EvDup", USER);
    assert_eq!(
        call(&s.state, signed("/slack/events", &body)).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&s.state, signed("/slack/events", &body)).await.0,
        StatusCode::OK
    );
    let first = timeout(Duration::from_secs(1), rx.recv()).await.unwrap();
    assert!(matches!(first, Some(RelayMsg::Inbound { .. })));
    assert_silent(&mut rx).await;
}

#[tokio::test]
async fn offline_session_posts_notice() {
    let s = setup();
    bind(&s.state, CHAT, "mac:<proj>&", USER).await;
    let req = signed("/slack/events", &thread_event("Ev1", USER));
    assert_eq!(call(&s.state, req).await.0, StatusCode::OK);
    let calls = wait_calls(&s.slack, 1).await;
    assert_eq!(
        calls,
        vec![Call::Post {
            channel: CHANNEL.into(),
            thread_ts: Some(THREAD.into()),
            text: "Session *mac:&lt;proj&gt;&amp;* is offline. Your message was not delivered."
                .into(),
        }]
    );
}

#[tokio::test]
async fn command_bad_signature_401() {
    let s = setup();
    let req = signed_with("/slack/commands", &command_body(USER, "list"), "other", NOW);
    assert_eq!(
        call(&s.state, req).await,
        (StatusCode::UNAUTHORIZED, String::new())
    );

    let missing = serde_urlencoded::to_string([("user_id", USER)]).unwrap();
    let (status, _) = call(&s.state, signed("/slack/commands", &missing)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn command_from_unauthorized_user_gets_nothing_useful() {
    let s = setup();
    let _rx = register(&s.state, "mac:proj").await;
    for text in ["", "list", "unbind"] {
        let req = signed("/slack/commands", &command_body("U9999OUT", text));
        let (status, body) = call(&s.state, req).await;
        assert_eq!(status, StatusCode::OK);
        let json: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            json,
            json!({"response_type": "ephemeral", "text": "You are not authorized to use /cc."})
        );
    }
}

#[tokio::test]
async fn command_pick_returns_select() {
    let s = setup();
    let _rx1 = register(&s.state, "mac:proj").await;
    let _rx2 = register(&s.state, "mac:other").await;
    let resp = router(s.state.clone())
        .oneshot(signed("/slack/commands", &command_body(USER, "")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers()[header::CONTENT_TYPE].to_str().unwrap();
    assert!(ct.starts_with("application/json"));
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    let options = json["blocks"][0]["accessory"]["options"]
        .as_array()
        .unwrap();
    let values: Vec<&str> = options
        .iter()
        .map(|o| o["value"].as_str().unwrap())
        .collect();
    assert_eq!(values, vec!["mac:other", "mac:proj"]);
}

#[tokio::test]
async fn command_list_and_help() {
    let s = setup();
    let text_of = |body: String| {
        let json: Value = serde_json::from_str(&body).unwrap();
        json["text"].as_str().unwrap().to_string()
    };
    let list = call(
        &s.state,
        signed("/slack/commands", &command_body(USER, "list")),
    )
    .await;
    assert_eq!(text_of(list.1), "No sessions connected.");

    let _rx1 = register(&s.state, "mac:proj").await;
    let _rx2 = register(&s.state, "mac:other").await;
    let list = call(
        &s.state,
        signed("/slack/commands", &command_body(USER, " LIST ")),
    )
    .await;
    assert_eq!(
        text_of(list.1),
        "Connected sessions:\n- mac:other\n- mac:proj"
    );

    let help = call(
        &s.state,
        signed("/slack/commands", &command_body(USER, "help")),
    )
    .await;
    assert_eq!(
        text_of(help.1),
        "Usage:\n/cc - pick a session and start a thread in this channel\n/cc list - show connected sessions\n/cc unbind - remove all of your session threads in this channel\n/cc help - show this help\nIn a session thread, answer a permission prompt with \"yes <id>\" or \"no <id>\"."
    );
}

#[tokio::test]
async fn command_unbind_removes_bindings() {
    let s = setup();
    bind(&s.state, "C0AB12CD3:1.1", "mac:proj", USER).await;
    bind(&s.state, "C0AB12CD3:2.2", "mac:proj", "U0OTHER").await;
    bind(&s.state, "C0AB12CD30:3.3", "mac:proj", USER).await;
    bind(&s.state, "C0OTHER:4.4", "mac:proj", USER).await;

    let req = signed("/slack/commands", &command_body(USER, "unbind"));
    let (status, body) = call(&s.state, req).await;
    assert_eq!(status, StatusCode::OK);
    let json: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["text"], "Unbound 1 thread(s) in this channel.");

    let expected = vec!["C0AB12CD30:3.3", "C0AB12CD3:2.2", "C0OTHER:4.4"];
    let in_memory: Vec<String> = s
        .state
        .store
        .lock()
        .await
        .bindings
        .keys()
        .cloned()
        .collect();
    assert_eq!(in_memory, expected);
    let on_disk = Store::load(&s.state.cfg.state_file).unwrap();
    assert_eq!(on_disk.bindings.keys().collect::<Vec<_>>(), expected);
}

#[tokio::test]
async fn interaction_binds_and_posts_root() {
    let s = setup();
    let req = signed(
        "/slack/interactions",
        &interaction_body(USER, "macbook:proj"),
    );
    assert_eq!(call(&s.state, req).await, (StatusCode::OK, String::new()));

    assert_eq!(
        s.slack.calls(),
        vec![Call::Post {
            channel: CHANNEL.into(),
            thread_ts: None,
            text: "Connected to session *macbook:proj*. Reply in this thread.".into(),
        }]
    );
    let expected = Binding {
        session: "macbook:proj".into(),
        user: USER.into(),
        created: NOW,
    };
    let key = "C0AB12CD3:1700000000.000001";
    assert_eq!(
        s.state.store.lock().await.bindings.get(key),
        Some(&expected)
    );
    let on_disk = Store::load(&s.state.cfg.state_file).unwrap();
    assert_eq!(on_disk.bindings.get(key), Some(&expected));
}

#[tokio::test]
async fn interaction_post_failure_responds_invite_hint() {
    let s = setup_with(
        FakeSlack::failing(SlackError("not_in_channel".into())),
        config(false, SECRET),
        true,
    );
    let req = signed(
        "/slack/interactions",
        &interaction_body(USER, "macbook:proj"),
    );
    assert_eq!(call(&s.state, req).await, (StatusCode::OK, String::new()));
    assert_eq!(
        s.slack.calls(),
        vec![
            Call::Post {
                channel: CHANNEL.into(),
                thread_ts: None,
                text: "Connected to session *macbook:proj*. Reply in this thread.".into(),
            },
            Call::Respond {
                response_url: RESPONSE_URL.into(),
                text: "Invite the bot to this channel first.".into(),
            },
        ]
    );
    assert!(s.state.store.lock().await.bindings.is_empty());
}

#[tokio::test]
async fn interaction_rejects_bad_requests() {
    let s = setup();
    let body = interaction_body(USER, "macbook:proj");
    let bad_sig = signed_with("/slack/interactions", &body, "other", NOW);
    assert_eq!(
        call(&s.state, bad_sig).await,
        (StatusCode::UNAUTHORIZED, String::new())
    );

    let no_payload = signed("/slack/interactions", "foo=bar");
    assert_eq!(call(&s.state, no_payload).await.0, StatusCode::BAD_REQUEST);
    let bad_json = signed("/slack/interactions", "payload=%7Bnope");
    assert_eq!(call(&s.state, bad_json).await.0, StatusCode::BAD_REQUEST);

    let outsider = signed("/slack/interactions", &interaction_body("U9999OUT", "x"));
    assert_eq!(
        call(&s.state, outsider).await,
        (StatusCode::OK, String::new())
    );
    assert!(s.slack.calls().is_empty());
    assert!(s.state.store.lock().await.bindings.is_empty());
}

#[tokio::test]
async fn dev_inject_disabled_without_dev() {
    let s = setup();
    let body = r#"{"session":"smoke","chat_id":"C1:1.1","user":"U1","text":"say hi"}"#;
    let req = Request::post("/dev/inject")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap();
    assert_eq!(call(&s.state, req).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dev_inject_forwards_in_dev_mode() {
    let s = setup_with(FakeSlack::new(), config(true, ""), false);
    let inject = |body: &'static str| {
        Request::post("/dev/inject")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap()
    };
    let body = r#"{"session":"smoke","chat_id":"C1:1.1","user":"U1","text":"say hi"}"#;
    assert_eq!(
        call(&s.state, inject(body)).await,
        (StatusCode::CONFLICT, "offline".into())
    );

    let mut rx = register(&s.state, "smoke").await;
    assert_eq!(
        call(&s.state, inject(body)).await,
        (StatusCode::OK, "ok".into())
    );
    assert_eq!(
        rx.recv().await,
        Some(RelayMsg::Inbound {
            chat_id: "C1:1.1".into(),
            user: "U1".into(),
            text: "say hi".into(),
        })
    );
    assert_eq!(
        call(&s.state, inject(r#"{"session":"smoke"}"#)).await,
        (StatusCode::BAD_REQUEST, "bad request".into())
    );
}
