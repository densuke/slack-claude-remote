//! stdio transport: reads MCP requests/notifications from `input`, dispatches
//! them (see `mcp.rs`), and writes responses plus relay-originated
//! notifications to `output`. No business logic lives here.

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::mcp::{self, Outcome};

pub async fn run<R, W>(
    input: R,
    mut output: W,
    to_relay: mpsc::Sender<protocol::AgentMsg>,
    mut to_stdout: mpsc::Receiver<serde_json::Value>,
) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = input.lines();
    // Stays true until the sending half of `to_stdout` is dropped; once
    // false the branch is disabled so a closed channel doesn't spin.
    let mut stdout_open = true;

    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else {
                    return Ok(());
                };
                match mcp::dispatch(&line) {
                    Outcome::Respond(v) => write_line(&mut output, &v).await?,
                    Outcome::Emit(msg) => {
                        let _ = to_relay.send(msg).await;
                    }
                    Outcome::Both(v, msg) => {
                        write_line(&mut output, &v).await?;
                        let _ = to_relay.send(msg).await;
                    }
                    Outcome::Ignore => {}
                }
            }
            value = to_stdout.recv(), if stdout_open => {
                match value {
                    Some(v) => write_line(&mut output, &v).await?,
                    None => stdout_open = false,
                }
            }
        }
    }
}

async fn write_line<W: AsyncWrite + Unpin>(
    output: &mut W,
    value: &serde_json::Value,
) -> std::io::Result<()> {
    let mut line = serde_json::to_string(value).map_err(std::io::Error::other)?;
    line.push('\n');
    output.write_all(line.as_bytes()).await?;
    output.flush().await
}
