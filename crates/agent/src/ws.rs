//! WebSocket client: connects to the relay, sends `Hello`, and shuttles
//! `AgentMsg`/`RelayMsg` frames. See spec §4, §8.

use std::path::Path;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::{AgentMsg, PROTOCOL_VERSION, RelayMsg};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;

use crate::mcp;

pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
}

pub struct Identity {
    pub name: String,
    pub host: String,
    pub cwd: String,
}

/// spec §4.1: explicit `SCCR_SESSION_NAME` wins; otherwise `{host}:{last path
/// component}` (or `{host}:/` when `cwd` has no last component).
pub fn identity_from(session_name: Option<String>, host: String, cwd: &Path) -> Identity {
    let name = session_name.filter(|s| !s.is_empty()).unwrap_or_else(|| {
        let dir = cwd.file_name().and_then(|s| s.to_str()).unwrap_or("/");
        format!("{host}:{dir}")
    });
    Identity {
        name,
        host,
        cwd: cwd.to_string_lossy().into_owned(),
    }
}

fn dropped_kind(msg: &AgentMsg) -> &'static str {
    match msg {
        AgentMsg::Hello { .. } => "hello",
        AgentMsg::Reply { .. } => "reply",
        AgentMsg::PermissionRequest { .. } => "permission_request",
    }
}

/// Drains and drops anything arriving on `from_mcp` while `fut` is pending,
/// so a caller blocked on connecting or backing off never stalls the mpsc
/// channel (spec §4.4 MUST). Logs only the message kind, never its body.
async fn drain_while<F: std::future::Future>(
    from_mcp: &mut mpsc::Receiver<AgentMsg>,
    fut: F,
) -> F::Output {
    tokio::pin!(fut);
    loop {
        tokio::select! {
            out = &mut fut => return out,
            msg = from_mcp.recv() => {
                match msg {
                    Some(m) => eprintln!("dropped {} while offline", dropped_kind(&m)),
                    None => return (&mut fut).await,
                }
            }
        }
    }
}

fn build_request(
    url: &str,
    token: &str,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, String> {
    let mut request = url
        .into_client_request()
        .map_err(|e| format!("invalid relay url: {e}"))?;
    let value = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|e| format!("invalid token: {e}"))?;
    request.headers_mut().insert(AUTHORIZATION, value);
    Ok(request)
}

pub async fn run(
    url: String,
    token: String,
    id: Identity,
    backoff: Backoff,
    to_stdout: mpsc::Sender<serde_json::Value>,
    mut from_mcp: mpsc::Receiver<AgentMsg>,
) {
    let mut wait = backoff.initial;

    loop {
        let request = match build_request(&url, &token) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{e}");
                drain_while(&mut from_mcp, tokio::time::sleep(wait)).await;
                wait = (wait * 2).min(backoff.max);
                continue;
            }
        };

        let connected = drain_while(&mut from_mcp, tokio_tungstenite::connect_async(request)).await;
        let mut ws = match connected {
            Ok((stream, _response)) => stream,
            Err(e) => {
                eprintln!("failed to connect to relay: {e}");
                drain_while(&mut from_mcp, tokio::time::sleep(wait)).await;
                wait = (wait * 2).min(backoff.max);
                continue;
            }
        };

        let hello = AgentMsg::Hello {
            version: PROTOCOL_VERSION,
            name: id.name.clone(),
            host: id.host.clone(),
            cwd: id.cwd.clone(),
        };
        let hello_text = match serde_json::to_string(&hello) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("failed to serialize hello: {e}");
                drain_while(&mut from_mcp, tokio::time::sleep(wait)).await;
                wait = (wait * 2).min(backoff.max);
                continue;
            }
        };
        if ws.send(Message::text(hello_text)).await.is_err() {
            drain_while(&mut from_mcp, tokio::time::sleep(wait)).await;
            wait = (wait * 2).min(backoff.max);
            continue;
        }

        'session: loop {
            tokio::select! {
                incoming = ws.next() => {
                    match incoming {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<RelayMsg>(text.as_str()) {
                                Ok(RelayMsg::Welcome { .. }) => {
                                    eprintln!("connected as {}", id.name);
                                    wait = backoff.initial;
                                }
                                Ok(RelayMsg::Inbound { chat_id, user, text }) => {
                                    let _ = to_stdout
                                        .send(mcp::channel_notification(&chat_id, &user, &text))
                                        .await;
                                }
                                Ok(RelayMsg::PermissionVerdict { request_id, behavior }) => {
                                    let _ = to_stdout
                                        .send(mcp::permission_notification(&request_id, behavior))
                                        .await;
                                }
                                Ok(RelayMsg::Error { message }) => {
                                    eprintln!("relay error: {message}");
                                    break 'session;
                                }
                                Err(_) => eprintln!("ignoring relay frame that failed to parse"),
                            }
                        }
                        Some(Ok(_)) => {} // binary/ping/pong/close: tungstenite handles pong itself
                        Some(Err(e)) => {
                            eprintln!("relay connection error: {e}");
                            break 'session;
                        }
                        None => break 'session,
                    }
                }
                msg = from_mcp.recv() => {
                    match msg {
                        Some(agent_msg) => {
                            if let Ok(text) = serde_json::to_string(&agent_msg)
                                && ws.send(Message::text(text)).await.is_err()
                            {
                                break 'session;
                            }
                        }
                        None => break 'session,
                    }
                }
            }
        }

        drain_while(&mut from_mcp, tokio::time::sleep(wait)).await;
        wait = (wait * 2).min(backoff.max);
    }
}

#[cfg(test)]
mod tests;
