//! HTTP surface of the relay (README 4.3).

mod agent_ws;
mod dev;
mod slack;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};

use crate::state::AppState;

pub fn router(state: Arc<AppState>) -> Router {
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/slack/events", post(slack::events))
        .route("/slack/commands", post(slack::commands))
        .route("/slack/interactions", post(slack::interactions))
        .route("/agent/ws", get(agent_ws::handler));
    let app = if state.cfg.dev {
        app.route("/dev/inject", post(dev::inject))
    } else {
        app
    };
    app.with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests;
