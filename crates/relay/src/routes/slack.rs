//! Slack webhooks: events, slash commands, interactions (spec 5, 6).

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use protocol::RelayMsg;
use serde::Deserialize;
use serde_json::Value;

use crate::permission;
use crate::slack::api::escape_mrkdwn;
use crate::slack::commands::{Cmd, parse, pick_response, text_response};
use crate::slack::events::{Candidate, Envelope, classify};
use crate::slack::interactions::selected_session;
use crate::slack::verify::verify;
use crate::state::{AppState, save_store};
use crate::store::Binding;

const UNAUTHORIZED_COMMAND: &str = "You are not authorized to use /cc.";
const INVITE_BOT: &str = "Invite the bot to this channel first.";
const NO_SESSIONS: &str = "No sessions connected.";
const HELP: &str = "Usage:\n/cc - pick a session and start a thread in this channel\n/cc list - show connected sessions\n/cc unbind - remove all of your session threads in this channel\n/cc help - show this help\nIn a session thread, answer a permission prompt with \"yes <id>\" or \"no <id>\".";

/// Spec 5.1. An empty signing secret (dev mode) rejects everything.
fn signature_ok(state: &AppState, headers: &HeaderMap, body: &[u8]) -> bool {
    if state.cfg.signing_secret.is_empty() {
        return false;
    }
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let (Some(ts), Some(sig)) = (
        header("x-slack-request-timestamp"),
        header("x-slack-signature"),
    ) else {
        return false;
    };
    verify(&state.cfg.signing_secret, ts, body, sig, (state.now)()).is_ok()
}

pub async fn events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !signature_ok(&state, &headers, &body) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(json) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match classify(&json) {
        Envelope::UrlVerification { challenge } => {
            ([(header::CONTENT_TYPE, "text/plain")], challenge).into_response()
        }
        Envelope::Other => StatusCode::OK.into_response(),
        Envelope::Message {
            event_id,
            candidate,
        } => {
            tokio::spawn(handle_message(state, event_id, candidate));
            StatusCode::OK.into_response()
        }
    }
}

/// Spec 6.2 steps 4-10, including permission verdicts (steps 8-9).
async fn handle_message(state: Arc<AppState>, event_id: String, candidate: Option<Candidate>) {
    if !state.dedupe.lock().await.first_seen(&event_id) {
        eprintln!("event {event_id}: duplicate");
        return;
    }
    let Some(c) = candidate else {
        return;
    };
    let binding = {
        let store = state.store.lock().await;
        if !store.users.contains(&c.user) {
            eprintln!("event {event_id} chat_id={}: unauthorized user", c.chat_id);
            return;
        }
        match store.bindings.get(&c.chat_id) {
            Some(binding) => binding.clone(),
            None => {
                eprintln!("event {event_id} chat_id={}: unbound", c.chat_id);
                return;
            }
        }
    };

    let verdict = {
        let mut pending = state.pending.lock().await;
        permission::sweep(&mut pending, (state.now)());
        permission::match_verdict(&pending, &c, &binding)
    };
    if let Some((request_id, behavior)) = verdict {
        state.pending.lock().await.remove(&request_id);
        let verdict_msg = RelayMsg::PermissionVerdict {
            request_id,
            behavior,
        };
        if state.hub.send(&binding.session, verdict_msg).await.is_ok() {
            eprintln!("event {event_id} chat_id={}: verdict forwarded", c.chat_id);
        } else {
            eprintln!(
                "event {event_id} chat_id={}: session offline for verdict",
                c.chat_id
            );
            notify_offline(&state, &c, &binding.session, &event_id).await;
        }
        return;
    }

    let inbound = RelayMsg::Inbound {
        chat_id: c.chat_id.clone(),
        user: c.user.clone(),
        text: c.text.clone(),
    };
    if state.hub.send(&binding.session, inbound).await.is_ok() {
        eprintln!("event {event_id} chat_id={}: forwarded", c.chat_id);
        return;
    }
    eprintln!("event {event_id} chat_id={}: session offline", c.chat_id);
    notify_offline(&state, &c, &binding.session, &event_id).await;
}

/// Spec 6.2 step 10 / 7.2 step 9's offline case; both post the same notice.
async fn notify_offline(state: &AppState, c: &Candidate, session: &str, event_id: &str) {
    let notice = format!(
        "Session *{}* is offline. Your message was not delivered.",
        escape_mrkdwn(session)
    );
    if let Err(e) = state
        .slack
        .post_message(&c.channel, Some(&c.thread_ts), &notice)
        .await
    {
        eprintln!("event {event_id}: offline notice failed: {}", e.0);
    }
}

#[derive(Deserialize)]
struct CommandForm {
    user_id: String,
    channel_id: String,
    #[serde(default)]
    text: String,
}

pub async fn commands(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !signature_ok(&state, &headers, &body) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(form) = serde_urlencoded::from_bytes::<CommandForm>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if !state.store.lock().await.users.contains(&form.user_id) {
        return Json(text_response(UNAUTHORIZED_COMMAND)).into_response();
    }

    let reply = match parse(&form.text) {
        Cmd::Pick => pick_response(&state.hub.list().await),
        Cmd::List => text_response(&list_text(&state.hub.list().await)),
        Cmd::Unbind => {
            let count = unbind(&state, &form.channel_id, &form.user_id).await;
            text_response(&format!("Unbound {count} thread(s) in this channel."))
        }
        Cmd::Help => text_response(HELP),
    };
    Json(reply).into_response()
}

fn list_text(names: &[String]) -> String {
    if names.is_empty() {
        return NO_SESSIONS.to_string();
    }
    names
        .iter()
        .fold("Connected sessions:".to_string(), |acc, name| {
            format!("{acc}\n- {name}")
        })
}

/// Removes the user's bindings in `channel` (spec 6.4) and returns the count.
async fn unbind(state: &AppState, channel: &str, user: &str) -> usize {
    let prefix = format!("{channel}:");
    let mut store = state.store.lock().await;
    let before = store.bindings.len();
    store
        .bindings
        .retain(|chat_id, b| !(chat_id.starts_with(&prefix) && b.user == user));
    let count = before - store.bindings.len();
    save_store(state, &store);
    count
}

#[derive(Deserialize)]
struct InteractionForm {
    payload: String,
}

/// Spec 6.1. Processed before responding.
pub async fn interactions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !signature_ok(&state, &headers, &body) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let payload = serde_urlencoded::from_bytes::<InteractionForm>(&body)
        .ok()
        .and_then(|form| serde_json::from_str::<Value>(&form.payload).ok());
    let Some(payload) = payload else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(sel) = selected_session(&payload) else {
        return StatusCode::OK.into_response();
    };
    if !state.store.lock().await.users.contains(&sel.user) {
        eprintln!("interaction: unauthorized user");
        return StatusCode::OK.into_response();
    }

    let root = format!(
        "Connected to session *{}*. Reply in this thread.",
        escape_mrkdwn(&sel.session)
    );
    match state.slack.post_message(&sel.channel, None, &root).await {
        Ok(ts) => {
            let chat_id = format!("{}:{ts}", sel.channel);
            let binding = Binding {
                session: sel.session,
                user: sel.user,
                created: (state.now)(),
            };
            let mut store = state.store.lock().await;
            store.bindings.insert(chat_id.clone(), binding);
            save_store(&state, &store);
            eprintln!("interaction: bound chat_id={chat_id}");
        }
        Err(e) => {
            eprintln!("interaction: root post failed: {}", e.0);
            if let Err(e) = state.slack.respond(&sel.response_url, INVITE_BOT).await {
                eprintln!("interaction: respond failed: {}", e.0);
            }
        }
    }
    StatusCode::OK.into_response()
}
