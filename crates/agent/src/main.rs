mod mcp;
mod stdio;
mod ws;

use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{BufReader, stdin, stdout};
use tokio::sync::mpsc;

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let (to_relay_tx, to_relay_rx) = mpsc::channel::<protocol::AgentMsg>(32);
    let (to_stdout_tx, to_stdout_rx) = mpsc::channel::<serde_json::Value>(32);

    let relay_url = std::env::var("SCCR_RELAY_URL").unwrap_or_default();
    let token = std::env::var("SCCR_TOKEN").unwrap_or_default();

    // spec §4.1: only start the WS client when both are set; otherwise stdio-only.
    if relay_url.is_empty() || token.is_empty() {
        eprintln!("SCCR_RELAY_URL/SCCR_TOKEN not set; running stdio only");
    } else {
        let session_name = std::env::var("SCCR_SESSION_NAME").ok();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let id = ws::identity_from(session_name, hostname(), &cwd);
        let backoff = ws::Backoff {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(30),
        };
        tokio::spawn(ws::run(
            relay_url,
            token,
            id,
            backoff,
            to_stdout_tx,
            to_relay_rx,
        ));
    }

    stdio::run(BufReader::new(stdin()), stdout(), to_relay_tx, to_stdout_rx).await
}
