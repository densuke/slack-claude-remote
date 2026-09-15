use super::*;

#[test]
fn hello_roundtrip() {
    let msg = AgentMsg::Hello {
        version: 1,
        name: "macbook:proj".to_string(),
        host: "macbook".to_string(),
        cwd: "/Users/me/src/proj".to_string(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"type\":\"hello\""));
    let back: AgentMsg = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, back);
}

#[test]
fn reply_roundtrip() {
    let msg = AgentMsg::Reply {
        chat_id: "C0AB12CD3:1789560000.000100".to_string(),
        text: "Done.".to_string(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: AgentMsg = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, back);
}

#[test]
fn permission_request_roundtrip() {
    let msg = AgentMsg::PermissionRequest {
        request_id: "abcde".to_string(),
        tool_name: "Bash".to_string(),
        description: "List files".to_string(),
        input_preview: "{\"command\": \"ls -la\"}".to_string(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: AgentMsg = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, back);
}

#[test]
fn inbound_roundtrip() {
    let msg = RelayMsg::Inbound {
        chat_id: "C0AB12CD3:1789560000.000100".to_string(),
        user: "U0123ABCD".to_string(),
        text: "What does main.rs do?".to_string(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: RelayMsg = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, back);
}

#[test]
fn verdict_serializes_lowercase() {
    let msg = RelayMsg::PermissionVerdict {
        request_id: "abcde".to_string(),
        behavior: Behavior::Allow,
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"behavior\":\"allow\""));
}

#[test]
fn unknown_type_rejected() {
    let result: Result<AgentMsg, _> = serde_json::from_str("{\"type\":\"nope\"}");
    assert!(result.is_err());
}

#[test]
fn unknown_field_rejected() {
    let json = "{\"type\":\"reply\",\"chat_id\":\"c\",\"text\":\"t\",\"extra\":1}";
    let result: Result<AgentMsg, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

#[test]
fn version_constant_is_1() {
    assert_eq!(PROTOCOL_VERSION, 1);
}
