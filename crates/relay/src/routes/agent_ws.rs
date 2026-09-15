//! `GET /agent/ws`: agent authentication, handshake and message loop (spec 8).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use protocol::{AgentMsg, PROTOCOL_VERSION, RelayMsg};
use subtle::ConstantTimeEq;
use tokio::sync::mpsc;

use crate::chunk::chunk;
use crate::permission;
use crate::state::AppState;

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const QUEUE_SIZE: usize = 64;
const CHUNK_CHARS: usize = 3900;

/// Authenticates before touching the upgrade, so a bad token is always 401.
pub async fn handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some(token) = token else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !token_accepted(&state, token).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match ws {
        Ok(ws) => ws.on_upgrade(move |socket| run(state, socket)),
        Err(rejection) => rejection.into_response(),
    }
}

async fn token_accepted(state: &AppState, token: &str) -> bool {
    if state.store.lock().await.token_ok(token) {
        return true;
    }
    state.cfg.dev && bool::from(token.as_bytes().ct_eq(b"dev"))
}

async fn run(state: Arc<AppState>, mut socket: WebSocket) {
    let (tx, rx) = mpsc::channel(QUEUE_SIZE);
    let name = match handshake(&state, &mut socket, tx).await {
        Ok(name) => name,
        Err(message) => {
            // Best effort: the peer may already be gone.
            let _ = send(&mut socket, &RelayMsg::Error { message }).await;
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };
    serve(&state, &mut socket, &name, rx).await;
    state.hub.unregister(&name).await;
    eprintln!("agent disconnected: {name}");
}

/// Spec 8 steps 4-8. On success the name is registered in the hub.
async fn handshake(
    state: &AppState,
    socket: &mut WebSocket,
    tx: mpsc::Sender<RelayMsg>,
) -> Result<String, String> {
    let first = tokio::time::timeout(HELLO_TIMEOUT, socket.recv())
        .await
        .map_err(|_| "hello timeout".to_string())?;
    let parsed = match first {
        Some(Ok(Message::Text(text))) => serde_json::from_str::<AgentMsg>(&text).ok(),
        _ => None,
    };
    let Some(AgentMsg::Hello {
        version,
        name,
        host,
        cwd,
    }) = parsed
    else {
        return Err("first frame must be hello".to_string());
    };
    if version != PROTOCOL_VERSION {
        return Err(format!("unsupported protocol version: {version}"));
    }
    if !valid_session_name(&name) {
        return Err("invalid session name".to_string());
    }
    if state.hub.register(&name, tx).await.is_err() {
        return Err(format!("session name already connected: {name}"));
    }
    eprintln!("agent connected: name={name} host={host} cwd={cwd}");
    Ok(name)
}

/// Spec 1.1.
fn valid_session_name(name: &str) -> bool {
    let len = name.chars().count();
    (1..=150).contains(&len) && !name.chars().any(char::is_control)
}

async fn serve(
    state: &AppState,
    socket: &mut WebSocket,
    name: &str,
    mut rx: mpsc::Receiver<RelayMsg>,
) {
    let welcome = RelayMsg::Welcome {
        name: name.to_string(),
    };
    if send(socket, &welcome).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => handle_frame(state, name, &text).await,
                Some(Ok(Message::Binary(_))) => eprintln!("agent {name}: binary frame ignored"),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            outgoing = rx.recv() => match outgoing {
                Some(msg) => {
                    if send(socket, &msg).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
        }
    }
}

async fn handle_frame(state: &AppState, name: &str, text: &str) {
    match serde_json::from_str::<AgentMsg>(text) {
        Ok(AgentMsg::Reply { chat_id, text }) => post_reply(state, &chat_id, &text).await,
        Ok(AgentMsg::PermissionRequest {
            request_id,
            tool_name,
            description,
            input_preview,
        }) => {
            permission::handle_request(
                state,
                name,
                request_id,
                &tool_name,
                &description,
                &input_preview,
            )
            .await
        }
        Ok(AgentMsg::Hello { .. }) => {}
        Err(_) => eprintln!("agent {name}: unparseable frame ignored"),
    }
}

/// Spec 6.3.
async fn post_reply(state: &AppState, chat_id: &str, text: &str) {
    let Some((channel, thread_ts)) = chat_id
        .split_once(':')
        .filter(|(c, t)| !c.is_empty() && !t.is_empty())
    else {
        eprintln!("reply dropped: invalid chat_id");
        return;
    };
    if text.is_empty() {
        return;
    }
    for piece in chunk(text, CHUNK_CHARS) {
        if let Err(e) = state
            .slack
            .post_message(channel, Some(thread_ts), &piece)
            .await
        {
            eprintln!("reply to chat_id={chat_id} failed: {}", e.0);
            return;
        }
    }
}

async fn send(socket: &mut WebSocket, msg: &RelayMsg) -> Result<(), axum::Error> {
    let text = serde_json::to_string(msg).map_err(axum::Error::new)?;
    socket.send(Message::Text(text.into())).await
}
