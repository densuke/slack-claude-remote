//! MCP stdio dispatch: pure functions, no I/O. See spec.md §3.

use protocol::{AgentMsg, Behavior};
use serde_json::{Value, json};

pub const INSTRUCTIONS: &str = "Messages from Slack arrive as <channel source=\"sccr\" chat_id=\"...\" user=\"...\">. Always answer by calling the reply tool with the same chat_id. Keep replies concise; Slack renders plain text and mrkdwn.";

pub enum Outcome {
    Respond(Value),
    Emit(AgentMsg),
    Both(Value, AgentMsg),
    Ignore,
}

/// Dispatch one JSON-RPC line. Returns `Ignore` for anything unparsable
/// (spec §3.1); never logs the line content (message bodies stay out of logs).
pub fn dispatch(line: &str) -> Outcome {
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("ignoring line that is not valid JSON");
            return Outcome::Ignore;
        }
    };
    let Some(obj) = value.as_object() else {
        eprintln!("ignoring line whose top level is not an object");
        return Outcome::Ignore;
    };
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        eprintln!("ignoring line with no method");
        return Outcome::Ignore;
    };
    let params = obj.get("params");

    match obj.get("id") {
        Some(id) => dispatch_request(id.clone(), method, params),
        None => dispatch_notification(method, params),
    }
}

fn dispatch_request(id: Value, method: &str, params: Option<&Value>) -> Outcome {
    match method {
        "initialize" => Outcome::Respond(handle_initialize(id, params)),
        "tools/list" => Outcome::Respond(handle_tools_list(id)),
        "tools/call" => handle_tools_call(id, params),
        "ping" => Outcome::Respond(respond(id, json!({}))),
        _ => Outcome::Respond(error_response(id, -32601, "Method not found")),
    }
}

fn dispatch_notification(method: &str, params: Option<&Value>) -> Outcome {
    match method {
        "notifications/claude/channel/permission_request" => handle_permission_request(params),
        _ => Outcome::Ignore,
    }
}

fn respond(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn handle_initialize(id: Value, params: Option<&Value>) -> Value {
    let protocol_version = params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str)
        .unwrap_or("2025-06-18");
    respond(
        id,
        json!({
            "protocolVersion": protocol_version,
            "serverInfo": {"name": "sccr", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {
                "experimental": {"claude/channel": {}, "claude/channel/permission": {}},
                "tools": {}
            },
            "instructions": INSTRUCTIONS
        }),
    )
}

fn handle_tools_list(id: Value) -> Value {
    respond(
        id,
        json!({"tools": [{
            "name": "reply",
            "description": "Send a message to the Slack thread identified by chat_id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "chat_id": {"type": "string", "description": "The chat_id attribute from the channel tag"},
                    "text": {"type": "string", "description": "The message to post"}
                },
                "required": ["chat_id", "text"]
            }
        }]}),
    )
}

fn handle_tools_call(id: Value, params: Option<&Value>) -> Outcome {
    let name = params.and_then(|p| p.get("name")).and_then(Value::as_str);
    if name != Some("reply") {
        return Outcome::Respond(error_response(id, -32602, "Invalid params"));
    }
    let args = params
        .and_then(|p| p.get("arguments"))
        .and_then(Value::as_object);
    let Some(args) = args else {
        return Outcome::Respond(error_response(id, -32602, "Invalid params"));
    };
    let chat_id = args.get("chat_id").and_then(Value::as_str);
    let text = args.get("text").and_then(Value::as_str);
    let (Some(chat_id), Some(text)) = (chat_id, text) else {
        return Outcome::Respond(error_response(id, -32602, "Invalid params"));
    };
    let response = respond(id, json!({"content": [{"type": "text", "text": "sent"}]}));
    let agent_msg = AgentMsg::Reply {
        chat_id: chat_id.to_string(),
        text: text.to_string(),
    };
    Outcome::Both(response, agent_msg)
}

fn handle_permission_request(params: Option<&Value>) -> Outcome {
    let Some(obj) = params.and_then(Value::as_object) else {
        eprintln!("ignoring malformed permission_request notification");
        return Outcome::Ignore;
    };
    let field = |key: &str| obj.get(key).and_then(Value::as_str).map(str::to_string);
    let (Some(request_id), Some(tool_name), Some(description), Some(input_preview)) = (
        field("request_id"),
        field("tool_name"),
        field("description"),
        field("input_preview"),
    ) else {
        eprintln!("ignoring malformed permission_request notification");
        return Outcome::Ignore;
    };
    Outcome::Emit(AgentMsg::PermissionRequest {
        request_id,
        tool_name,
        description,
        input_preview,
    })
}

/// Build the `notifications/claude/channel` line sent to Claude on an inbound Slack message.
pub fn channel_notification(chat_id: &str, user: &str, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/claude/channel",
        "params": {
            "content": text,
            "meta": {"chat_id": chat_id, "user": user}
        }
    })
}

/// Build the `notifications/claude/channel/permission` line sent to Claude after a verdict.
pub fn permission_notification(request_id: &str, behavior: Behavior) -> Value {
    let behavior = match behavior {
        Behavior::Allow => "allow",
        Behavior::Deny => "deny",
    };
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/claude/channel/permission",
        "params": {"request_id": request_id, "behavior": behavior}
    })
}

#[cfg(test)]
mod tests;
