use std::process::ExitCode;
use std::sync::Arc;

use sccr_relay::config;
use sccr_relay::routes::router;
use sccr_relay::slack::api::{HttpSlack, LogSlack, SlackApi};
use sccr_relay::state::{AppState, unix_now};
use sccr_relay::store::Store;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sccr-relay: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let cfg = config::from_env()?;
    let store = Store::load(&cfg.state_file).map_err(|e| format!("cannot load state file: {e}"))?;
    let slack: Arc<dyn SlackApi> = if cfg.dev {
        Arc::new(LogSlack::new(unix_now))
    } else {
        Arc::new(HttpSlack::new(cfg.bot_token.clone()).map_err(|e| e.0)?)
    };
    let listener = tokio::net::TcpListener::bind(&cfg.bind)
        .await
        .map_err(|e| format!("cannot bind {}: {e}", cfg.bind))?;
    let mode = if cfg.dev { " (dev mode)" } else { "" };
    eprintln!("sccr-relay listening on {}{mode}", cfg.bind);
    let state = Arc::new(AppState::new(cfg, store, slack, unix_now));
    axum::serve(listener, router(state))
        .await
        .map_err(|e| e.to_string())
}
