mod mcp;
mod stdio;

use tokio::io::{BufReader, stdin, stdout};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let (to_relay_tx, mut to_relay_rx) = mpsc::channel::<protocol::AgentMsg>(32);
    let (to_stdout_tx, to_stdout_rx) = mpsc::channel::<serde_json::Value>(32);

    if std::env::var("SCCR_RELAY_URL")
        .unwrap_or_default()
        .is_empty()
        || std::env::var("SCCR_TOKEN").unwrap_or_default().is_empty()
    {
        eprintln!("SCCR_RELAY_URL/SCCR_TOKEN not set; running stdio only");
    }
    // TODO(T3-1): replace this sink with the real relay WebSocket client.
    tokio::spawn(async move { while to_relay_rx.recv().await.is_some() {} });
    // Keep the sender alive so `to_stdout_rx` doesn't close prematurely
    // (T3-1 wires the relay's incoming frames into `to_stdout_tx`).
    let _to_stdout_tx = to_stdout_tx;

    stdio::run(BufReader::new(stdin()), stdout(), to_relay_tx, to_stdout_rx).await
}
