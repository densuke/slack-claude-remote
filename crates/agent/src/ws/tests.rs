use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

use super::*;
use protocol::{AgentMsg, Behavior, RelayMsg};

const TIMEOUT: Duration = Duration::from_secs(2);
const FAST: Backoff = Backoff {
    initial: Duration::from_millis(50),
    max: Duration::from_millis(50),
};

#[test]
fn identity_default_name() {
    let id = identity_from(None, "mac".to_string(), Path::new("/a/proj"));
    assert_eq!(id.name, "mac:proj");
    assert_eq!(id.host, "mac");
    assert_eq!(id.cwd, "/a/proj");
}

#[test]
fn identity_explicit_name() {
    let id = identity_from(
        Some("custom".to_string()),
        "mac".to_string(),
        Path::new("/a/proj"),
    );
    assert_eq!(id.name, "custom");
}

fn test_identity() -> Identity {
    Identity {
        name: "mac:proj".to_string(),
        host: "mac".to_string(),
        cwd: "/a/proj".to_string(),
    }
}

async fn fake_relay() -> (String, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (format!("ws://{addr}/agent/ws"), listener)
}

/// Accepts one connection, capturing the `Authorization` header sent during the handshake.
#[allow(clippy::result_large_err)]
async fn accept_capturing_auth(
    listener: &TcpListener,
) -> (WebSocketStream<TcpStream>, Option<String>) {
    let (stream, _) = listener.accept().await.unwrap();
    let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let captured2 = captured.clone();
    let callback = move |req: &Request, resp: Response| {
        let auth = req
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        *captured2.lock().unwrap() = auth;
        Ok(resp)
    };
    let ws = tokio_tungstenite::accept_hdr_async(stream, callback)
        .await
        .unwrap();
    let auth = captured.lock().unwrap().clone();
    (ws, auth)
}

async fn recv_agent_msg(ws: &mut WebSocketStream<TcpStream>) -> AgentMsg {
    let msg = tokio::time::timeout(TIMEOUT, ws.next())
        .await
        .expect("timed out waiting for frame")
        .expect("stream closed")
        .expect("frame error");
    let text = msg.into_text().unwrap();
    serde_json::from_str(text.as_str()).unwrap()
}

#[tokio::test]
async fn sends_bearer_and_hello_first() {
    let (url, listener) = fake_relay().await;
    let (to_stdout_tx, _to_stdout_rx) = mpsc::channel(8);
    let (_from_mcp_tx, from_mcp_rx) = mpsc::channel(8);
    tokio::spawn(run(
        url,
        "t".to_string(),
        test_identity(),
        FAST,
        to_stdout_tx,
        from_mcp_rx,
    ));

    let (mut ws, auth) = accept_capturing_auth(&listener).await;
    assert_eq!(auth.as_deref(), Some("Bearer t"));

    match recv_agent_msg(&mut ws).await {
        AgentMsg::Hello { version, .. } => assert_eq!(version, 1),
        other => panic!("expected hello, got {other:?}"),
    }
}

#[tokio::test]
async fn inbound_becomes_channel_notification() {
    let (url, listener) = fake_relay().await;
    let (to_stdout_tx, mut to_stdout_rx) = mpsc::channel(8);
    let (_from_mcp_tx, from_mcp_rx) = mpsc::channel(8);
    tokio::spawn(run(
        url,
        "t".to_string(),
        test_identity(),
        FAST,
        to_stdout_tx,
        from_mcp_rx,
    ));

    let (mut ws, _auth) = accept_capturing_auth(&listener).await;
    let _hello = recv_agent_msg(&mut ws).await;

    let inbound = RelayMsg::Inbound {
        chat_id: "C1:1.1".to_string(),
        user: "U1".to_string(),
        text: "hi".to_string(),
    };
    ws.send(Message::text(serde_json::to_string(&inbound).unwrap()))
        .await
        .unwrap();

    let v = tokio::time::timeout(TIMEOUT, to_stdout_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["method"], "notifications/claude/channel");
    assert_eq!(v["params"]["content"], "hi");
    assert_eq!(v["params"]["meta"]["chat_id"], "C1:1.1");
    assert_eq!(v["params"]["meta"]["user"], "U1");
}

#[tokio::test]
async fn reply_forwarded_to_relay() {
    let (url, listener) = fake_relay().await;
    let (to_stdout_tx, _to_stdout_rx) = mpsc::channel(8);
    let (from_mcp_tx, from_mcp_rx) = mpsc::channel(8);
    tokio::spawn(run(
        url,
        "t".to_string(),
        test_identity(),
        FAST,
        to_stdout_tx,
        from_mcp_rx,
    ));

    let (mut ws, _auth) = accept_capturing_auth(&listener).await;
    let _hello = recv_agent_msg(&mut ws).await;

    let reply = AgentMsg::Reply {
        chat_id: "C1:1.1".to_string(),
        text: "hi".to_string(),
    };
    from_mcp_tx.send(reply.clone()).await.unwrap();

    assert_eq!(recv_agent_msg(&mut ws).await, reply);
}

#[tokio::test]
async fn verdict_becomes_permission_notification() {
    let (url, listener) = fake_relay().await;
    let (to_stdout_tx, mut to_stdout_rx) = mpsc::channel(8);
    let (_from_mcp_tx, from_mcp_rx) = mpsc::channel(8);
    tokio::spawn(run(
        url,
        "t".to_string(),
        test_identity(),
        FAST,
        to_stdout_tx,
        from_mcp_rx,
    ));

    let (mut ws, _auth) = accept_capturing_auth(&listener).await;
    let _hello = recv_agent_msg(&mut ws).await;

    let verdict = RelayMsg::PermissionVerdict {
        request_id: "abcde".to_string(),
        behavior: Behavior::Allow,
    };
    ws.send(Message::text(serde_json::to_string(&verdict).unwrap()))
        .await
        .unwrap();

    let v = tokio::time::timeout(TIMEOUT, to_stdout_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["method"], "notifications/claude/channel/permission");
    assert_eq!(v["params"]["request_id"], "abcde");
    assert_eq!(v["params"]["behavior"], "allow");
}

#[tokio::test]
async fn reconnects_after_close() {
    let (url, listener) = fake_relay().await;
    let (to_stdout_tx, _to_stdout_rx) = mpsc::channel(8);
    let (_from_mcp_tx, from_mcp_rx) = mpsc::channel(8);
    tokio::spawn(run(
        url,
        "t".to_string(),
        test_identity(),
        FAST,
        to_stdout_tx,
        from_mcp_rx,
    ));

    let (mut ws1, _) = accept_capturing_auth(&listener).await;
    let _hello = recv_agent_msg(&mut ws1).await;
    ws1.close(None).await.unwrap();
    drop(ws1);

    let (mut ws2, _) = tokio::time::timeout(TIMEOUT, accept_capturing_auth(&listener))
        .await
        .expect("agent did not reconnect in time");
    match recv_agent_msg(&mut ws2).await {
        AgentMsg::Hello { .. } => {}
        other => panic!("expected hello on reconnect, got {other:?}"),
    }
}

#[tokio::test]
async fn messages_sent_while_offline_are_dropped_not_blocking() {
    // Nothing is listening on this port, so the agent stays "offline" (connect fails/backs off).
    let (to_stdout_tx, _to_stdout_rx) = mpsc::channel(8);
    let (from_mcp_tx, from_mcp_rx) = mpsc::channel(2);
    tokio::spawn(run(
        "ws://127.0.0.1:1".to_string(),
        "t".to_string(),
        test_identity(),
        FAST,
        to_stdout_tx,
        from_mcp_rx,
    ));

    for i in 0..5u32 {
        let reply = AgentMsg::Reply {
            chat_id: format!("c{i}"),
            text: "x".to_string(),
        };
        tokio::time::timeout(TIMEOUT, from_mcp_tx.send(reply))
            .await
            .expect("send blocked while offline")
            .unwrap();
    }
}
