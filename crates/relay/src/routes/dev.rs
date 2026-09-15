//! `POST /dev/inject`, registered only in dev mode (spec 12.1).

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use protocol::RelayMsg;
use serde::Deserialize;

use crate::state::AppState;

#[derive(Deserialize)]
struct Inject {
    session: String,
    chat_id: String,
    user: String,
    text: String,
}

pub async fn inject(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Ok(req) = serde_json::from_slice::<Inject>(&body) else {
        return (StatusCode::BAD_REQUEST, "bad request").into_response();
    };
    let msg = RelayMsg::Inbound {
        chat_id: req.chat_id,
        user: req.user,
        text: req.text,
    };
    match state.hub.send(&req.session, msg).await {
        Ok(()) => (StatusCode::OK, "ok").into_response(),
        Err(_) => (StatusCode::CONFLICT, "offline").into_response(),
    }
}
