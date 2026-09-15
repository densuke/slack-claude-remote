use super::*;
use protocol::AgentMsg;
use serde_json::json;

fn initialize_line(id: i64, protocol_version: Option<&str>) -> String {
    let params = match protocol_version {
        Some(v) => {
            json!({"protocolVersion": v, "capabilities": {}, "clientInfo": {"name": "claude-code", "version": "2.1.240"}})
        }
        None => json!({"capabilities": {}}),
    };
    json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": params}).to_string()
}

#[test]
fn initialize_declares_channel_capabilities() {
    let outcome = dispatch(&initialize_line(0, Some("2025-06-18")));
    let Outcome::Respond(v) = outcome else {
        panic!("expected Respond");
    };
    let experimental = &v["result"]["capabilities"]["experimental"];
    assert!(experimental.get("claude/channel").is_some());
    assert!(experimental.get("claude/channel/permission").is_some());
    assert!(v["result"]["capabilities"]["tools"].is_object());
    assert!(!v["result"]["instructions"].as_str().unwrap().is_empty());
}

#[test]
fn initialize_echoes_protocol_version() {
    let outcome = dispatch(&initialize_line(0, Some("2099-01-01")));
    let Outcome::Respond(v) = outcome else {
        panic!("expected Respond");
    };
    assert_eq!(v["result"]["protocolVersion"], "2099-01-01");

    let outcome = dispatch(&initialize_line(0, None));
    let Outcome::Respond(v) = outcome else {
        panic!("expected Respond");
    };
    assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn response_echoes_id() {
    let outcome = dispatch(&initialize_line(42, Some("2025-06-18")));
    let Outcome::Respond(v) = outcome else {
        panic!("expected Respond");
    };
    assert_eq!(v["id"], 42);

    let line = json!({"jsonrpc": "2.0", "id": "abc", "method": "ping"}).to_string();
    let outcome = dispatch(&line);
    let Outcome::Respond(v) = outcome else {
        panic!("expected Respond");
    };
    assert_eq!(v["id"], "abc");
}

#[test]
fn tools_list_has_only_reply() {
    let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}).to_string();
    let outcome = dispatch(&line);
    let Outcome::Respond(v) = outcome else {
        panic!("expected Respond");
    };
    let tools = v["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "reply");
    let required = tools[0]["inputSchema"]["required"].as_array().unwrap();
    assert!(required.contains(&json!("chat_id")));
    assert!(required.contains(&json!("text")));
}

#[test]
fn reply_call_emits_agent_reply_and_responds() {
    let line = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": "reply", "arguments": {"chat_id": "C1:1.1", "text": "hi"}}
    })
    .to_string();
    let outcome = dispatch(&line);
    let Outcome::Both(v, msg) = outcome else {
        panic!("expected Both");
    };
    assert_eq!(v["result"]["content"][0]["text"], "sent");
    assert_eq!(
        msg,
        AgentMsg::Reply {
            chat_id: "C1:1.1".to_string(),
            text: "hi".to_string(),
        }
    );
}

#[test]
fn reply_call_missing_text_is_invalid_params() {
    let line = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {"name": "reply", "arguments": {"chat_id": "C1:1.1"}}
    })
    .to_string();
    let outcome = dispatch(&line);
    let Outcome::Respond(v) = outcome else {
        panic!("expected Respond");
    };
    assert_eq!(v["error"]["code"], -32602);
}

#[test]
fn unknown_tool_is_invalid_params() {
    let line = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {"name": "nope", "arguments": {}}
    })
    .to_string();
    let outcome = dispatch(&line);
    let Outcome::Respond(v) = outcome else {
        panic!("expected Respond");
    };
    assert_eq!(v["error"]["code"], -32602);
}

#[test]
fn ping_returns_empty_object() {
    let line = json!({"jsonrpc": "2.0", "id": 4, "method": "ping"}).to_string();
    let outcome = dispatch(&line);
    let Outcome::Respond(v) = outcome else {
        panic!("expected Respond");
    };
    assert_eq!(v["result"], json!({}));
}

#[test]
fn unknown_method_is_32601() {
    let line =
        json!({"jsonrpc": "2.0", "id": 5, "method": "resources/list", "params": {}}).to_string();
    let outcome = dispatch(&line);
    let Outcome::Respond(v) = outcome else {
        panic!("expected Respond");
    };
    assert_eq!(v["error"]["code"], -32601);
}

#[test]
fn initialized_notification_ignored() {
    let line = json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string();
    assert!(matches!(dispatch(&line), Outcome::Ignore));
}

#[test]
fn unknown_notification_ignored() {
    let line = json!({"jsonrpc": "2.0", "method": "notifications/cancelled"}).to_string();
    assert!(matches!(dispatch(&line), Outcome::Ignore));
}

#[test]
fn permission_request_notification_emits() {
    let line = json!({
        "jsonrpc": "2.0",
        "method": "notifications/claude/channel/permission_request",
        "params": {
            "request_id": "abcde",
            "tool_name": "Bash",
            "description": "List files",
            "input_preview": "{\"command\": \"ls -la\"}"
        }
    })
    .to_string();
    let outcome = dispatch(&line);
    let Outcome::Emit(AgentMsg::PermissionRequest {
        request_id,
        tool_name,
        description,
        input_preview,
    }) = outcome
    else {
        panic!("expected Emit(PermissionRequest)");
    };
    assert_eq!(request_id, "abcde");
    assert_eq!(tool_name, "Bash");
    assert_eq!(description, "List files");
    assert_eq!(input_preview, "{\"command\": \"ls -la\"}");
}

#[test]
fn channel_notification_shape() {
    let v = channel_notification("C1:1.1", "U1", "hi");
    assert_eq!(v["method"], "notifications/claude/channel");
    assert_eq!(v["params"]["content"], "hi");
    let meta = v["params"]["meta"].as_object().unwrap();
    assert_eq!(meta.len(), 2);
    assert_eq!(meta["chat_id"], "C1:1.1");
    assert_eq!(meta["user"], "U1");
    assert!(v.get("id").is_none());
}

#[test]
fn permission_notification_shape() {
    let v = permission_notification("abcde", protocol::Behavior::Allow);
    assert_eq!(v["method"], "notifications/claude/channel/permission");
    assert_eq!(v["params"]["request_id"], "abcde");
    assert_eq!(v["params"]["behavior"], "allow");
}

#[test]
fn garbage_line_ignored() {
    assert!(matches!(dispatch("not json"), Outcome::Ignore));
    assert!(matches!(dispatch("[1,2,3]"), Outcome::Ignore));
}
