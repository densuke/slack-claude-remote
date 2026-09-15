use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn spawn() -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_sccr-agent"))
        .env_remove("SCCR_RELAY_URL")
        .env_remove("SCCR_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn sccr-agent")
}

#[test]
fn initialize_over_stdio() {
    let mut child = spawn();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":0,"method":"initialize","params":{{"protocolVersion":"2025-06-18"}}}}"#
    )
    .unwrap();
    drop(stdin);

    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();

    let value: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(value["id"], 0);
    assert_eq!(value["result"]["serverInfo"]["name"], "sccr");

    child.wait().unwrap();
}

#[test]
fn exits_on_eof() {
    let mut child = spawn();
    drop(child.stdin.take().unwrap());
    let status = child.wait().unwrap();
    assert!(status.success());
}

#[test]
fn stdout_has_only_json() {
    let mut child = spawn();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "not json at all").unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"ping","params":{{}}}}"#
    )
    .unwrap();
    drop(stdin);

    let stdout = child.stdout.take().unwrap();
    let reader = BufReader::new(stdout);
    let mut count = 0;
    for line in reader.lines() {
        let line = line.unwrap();
        serde_json::from_str::<serde_json::Value>(&line)
            .unwrap_or_else(|e| panic!("line was not valid JSON: {line:?}: {e}"));
        count += 1;
    }
    assert_eq!(count, 1);

    child.wait().unwrap();
}
