//! End-to-end flow over real TCP: Slack webhooks in, fake agent over WebSocket.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::{AgentMsg, Behavior, PROTOCOL_VERSION, RelayMsg};
use sccr_relay::config::Config;
use sccr_relay::routes::router;
use sccr_relay::slack::api::{Call, FakeSlack};
use sccr_relay::slack::verify::sign;
use sccr_relay::state::{AppState, unix_now};
use sccr_relay::store::{Store, TokenRecord};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const SECRET: &str = "e2e-signing-secret";
const TOKEN: &str = "t1";
const USER: &str = "U1";
const CHANNEL: &str = "C1";
const SESSION: &str = "mac";
const THREAD: &str = "1700000000.000001";
const WAIT: Duration = Duration::from_secs(2);
const SILENCE: Duration = Duration::from_millis(300);

struct Relay {
    addr: SocketAddr,
    state: Arc<AppState>,
    slack: Arc<FakeSlack>,
    dir: PathBuf,
    http: reqwest::Client,
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn start(test: &str) -> Relay {
    let dir = std::env::temp_dir().join(format!("sccr-e2e-{}-{test}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let state_file = dir.join("state.json");
    let seed = Store {
        users: vec![USER.into()],
        tokens: vec![TokenRecord {
            sha256: hex::encode(Sha256::digest(TOKEN.as_bytes())),
            label: "e2e".into(),
            created: "2026-09-16T00:00:00Z".into(),
        }],
        bindings: Default::default(),
    };
    seed.save(&state_file).unwrap();

    let cfg = Config {
        bind: "127.0.0.1:0".into(),
        public_url: "https://sccr.example.jp".into(),
        state_file: state_file.clone(),
        admin_user: USER.into(),
        signing_secret: SECRET.into(),
        bot_token: "xoxb-e2e".into(),
        client_id: "cid".into(),
        client_secret: "csecret".into(),
        dev: false,
    };
    let slack = Arc::new(FakeSlack::new());
    let store = Store::load(&state_file).unwrap();
    let state = Arc::new(AppState::new(cfg, store, slack.clone(), unix_now));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    Relay {
        addr,
        state,
        slack,
        dir,
        http: reqwest::Client::new(),
    }
}

impl Relay {
    async fn post(&self, path: &str, body: String) -> reqwest::Response {
        let ts = unix_now().to_string();
        let sig = sign(SECRET, &ts, body.as_bytes());
        let resp = timeout(
            WAIT,
            self.http
                .post(format!("http://{}{path}", self.addr))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-slack-request-timestamp", ts)
                .header("x-slack-signature", sig)
                .body(body)
                .send(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(resp.status(), 200, "{path}");
        resp
    }

    async fn reply_event(&self, event_id: &str, user: &str, text: &str) {
        let body = json!({
            "type": "event_callback",
            "event_id": event_id,
            "event": {
                "type": "message", "channel": CHANNEL, "user": user, "text": text,
                "ts": format!("1700000100.{event_id}"), "thread_ts": THREAD,
                "channel_type": "channel"
            }
        });
        self.post("/slack/events", body.to_string()).await;
    }

    /// Waits until FakeSlack has recorded exactly `n` calls, then checks it stays there.
    async fn calls(&self, n: usize) -> Vec<Call> {
        timeout(WAIT, async {
            while self.slack.calls().len() < n {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out waiting for slack calls");
        self.slack.calls()
    }

    async fn connect(&self) -> Ws {
        let mut req = format!("ws://{}/agent/ws", self.addr)
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
        let mut ws = timeout(WAIT, connect_async(req)).await.unwrap().unwrap().0;
        send(
            &mut ws,
            &AgentMsg::Hello {
                version: PROTOCOL_VERSION,
                name: SESSION.into(),
                host: "mac".into(),
                cwd: "/a/proj".into(),
            },
        )
        .await;
        assert_eq!(
            recv(&mut ws).await,
            RelayMsg::Welcome {
                name: SESSION.into()
            }
        );
        ws
    }
}

async fn send(ws: &mut Ws, msg: &AgentMsg) {
    let text = serde_json::to_string(msg).unwrap();
    ws.send(Message::Text(text.into())).await.unwrap();
}

async fn next_msg(ws: &mut Ws) -> Option<RelayMsg> {
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(t))) => return Some(serde_json::from_str(&t).unwrap()),
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return None,
            Some(Ok(_)) => {}
        }
    }
}

async fn recv(ws: &mut Ws) -> RelayMsg {
    timeout(WAIT, next_msg(ws))
        .await
        .expect("timed out waiting for relay message")
        .expect("socket closed")
}

async fn assert_silent(ws: &mut Ws) {
    if let Ok(msg) = timeout(SILENCE, next_msg(ws)).await {
        panic!("unexpected relay message: {msg:?}");
    }
}

fn thread_post(text: &str) -> Call {
    Call::Post {
        channel: CHANNEL.into(),
        thread_ts: Some(THREAD.into()),
        text: text.into(),
    }
}

/// Steps 4-5: `/cc` lists the agent, selecting it posts the root and binds.
async fn bind_via_slack(relay: &Relay) {
    let command = serde_urlencoded::to_string([
        ("user_id", USER),
        ("channel_id", CHANNEL),
        ("command", "/cc"),
        ("text", ""),
        ("response_url", "https://hooks.slack.com/commands/T1/1/x"),
    ])
    .unwrap();
    let json: Value = relay
        .post("/slack/commands", command)
        .await
        .json()
        .await
        .unwrap();
    let values: Vec<&str> = json["blocks"][0]["accessory"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["value"].as_str().unwrap())
        .collect();
    assert_eq!(values, vec![SESSION]);

    let payload = json!({
        "type": "block_actions",
        "user": {"id": USER},
        "channel": {"id": CHANNEL},
        "response_url": "https://hooks.slack.com/actions/T1/1/x",
        "actions": [{
            "type": "static_select",
            "action_id": "sccr_select",
            "selected_option": {"text": {"type": "plain_text", "text": SESSION}, "value": SESSION}
        }]
    });
    let body = serde_urlencoded::to_string([("payload", payload.to_string())]).unwrap();
    relay.post("/slack/interactions", body).await;

    assert_eq!(
        relay.slack.calls(),
        vec![Call::Post {
            channel: CHANNEL.into(),
            thread_ts: None,
            text: "Connected to session *mac*. Reply in this thread.".into(),
        }]
    );
    let on_disk = Store::load(&relay.state.cfg.state_file).unwrap();
    let binding = &on_disk.bindings[&format!("{CHANNEL}:{THREAD}")];
    assert_eq!(
        (binding.session.as_str(), binding.user.as_str()),
        (SESSION, USER)
    );
}

async fn request_permission(relay: &Relay, ws: &mut Ws, expected_calls: usize) {
    send(
        ws,
        &AgentMsg::PermissionRequest {
            request_id: "abcde".into(),
            tool_name: "Bash".into(),
            description: "List files".into(),
            input_preview: "{\"command\": \"ls\"}".into(),
        },
    )
    .await;
    let calls = relay.calls(expected_calls).await;
    assert_eq!(
        calls.last(),
        Some(&thread_post(
            "Claude wants to run *Bash*: List files\n```{\"command\": \"ls\"}```\nReply \"yes abcde\" or \"no abcde\" in this thread."
        ))
    );
}

#[tokio::test]
async fn full_flow() {
    let relay = start("full_flow").await;
    let mut ws = relay.connect().await;
    bind_via_slack(&relay).await;

    // 6-7: thread reply reaches the agent once, even when redelivered.
    relay.reply_event("Ev1", USER, "hello").await;
    assert_eq!(
        recv(&mut ws).await,
        RelayMsg::Inbound {
            chat_id: format!("{CHANNEL}:{THREAD}"),
            user: USER.into(),
            text: "hello".into(),
        }
    );
    relay.reply_event("Ev1", USER, "hello").await;
    assert_silent(&mut ws).await;

    // 8: agent reply goes to the thread.
    send(
        &mut ws,
        &AgentMsg::Reply {
            chat_id: format!("{CHANNEL}:{THREAD}"),
            text: "hi".into(),
        },
    )
    .await;
    assert_eq!(relay.calls(2).await[1], thread_post("hi"));

    // 9-10: permission prompt and verdict.
    request_permission(&relay, &mut ws, 3).await;
    relay.reply_event("Ev2", USER, "yes abcde").await;
    assert_eq!(
        recv(&mut ws).await,
        RelayMsg::PermissionVerdict {
            request_id: "abcde".into(),
            behavior: Behavior::Allow,
        }
    );

    // 11: long reply is split into three posts.
    let long = "x".repeat(8000);
    send(
        &mut ws,
        &AgentMsg::Reply {
            chat_id: format!("{CHANNEL}:{THREAD}"),
            text: long.clone(),
        },
    )
    .await;
    let calls = relay.calls(6).await;
    assert_eq!(
        calls[3..],
        [
            thread_post(&long[..3900]),
            thread_post(&long[3900..7800]),
            thread_post(&long[7800..]),
        ]
    );

    // 12: a user outside the allow list is ignored.
    relay.reply_event("Ev3", "U2", "hello from U2").await;
    assert_silent(&mut ws).await;
    assert_eq!(relay.slack.calls().len(), 6);
}

#[tokio::test]
async fn verdict_after_agent_disconnect_posts_offline_notice() {
    let relay = start("offline_verdict").await;
    let mut ws = relay.connect().await;
    bind_via_slack(&relay).await;
    request_permission(&relay, &mut ws, 2).await;

    timeout(WAIT, ws.close(None)).await.unwrap().unwrap();
    timeout(WAIT, async {
        while !relay.state.hub.list().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("agent not unregistered after close");

    relay.reply_event("Ev1", USER, "yes abcde").await;
    let calls = relay.calls(3).await;
    tokio::time::sleep(SILENCE).await;
    assert_eq!(relay.slack.calls().len(), 3);
    assert_eq!(
        calls[2],
        thread_post("Session *mac* is offline. Your message was not delivered.")
    );
    assert!(relay.state.pending.lock().await.is_empty());
}
