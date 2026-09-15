use super::*;

#[test]
fn empty_is_pick() {
    assert_eq!(parse(""), Cmd::Pick);
}

#[test]
fn whitespace_is_pick() {
    assert_eq!(parse("   "), Cmd::Pick);
}

#[test]
fn list_and_unbind_parsed() {
    assert_eq!(parse("list"), Cmd::List);
    assert_eq!(parse("  list  "), Cmd::List);
    assert_eq!(parse("LIST"), Cmd::List);
    assert_eq!(parse("unbind"), Cmd::Unbind);
    assert_eq!(parse("  UNBIND  "), Cmd::Unbind);
}

#[test]
fn unknown_is_help() {
    assert_eq!(parse("help"), Cmd::Help);
    assert_eq!(parse("bogus"), Cmd::Help);
}

#[test]
fn pick_response_has_static_select() {
    let names = vec!["macbook:other".to_string(), "macbook:proj".to_string()];
    let body = pick_response(&names);
    assert_eq!(body["response_type"], "ephemeral");
    let accessory = &body["blocks"][0]["accessory"];
    assert_eq!(accessory["action_id"], "sccr_select");
    let options = accessory["options"].as_array().unwrap();
    assert_eq!(options.len(), 2);
    assert_eq!(options[0]["value"], "macbook:other");
    assert_eq!(options[1]["value"], "macbook:proj");
}

#[test]
fn pick_response_empty_is_text_only() {
    let body = pick_response(&[]);
    assert_eq!(
        body,
        serde_json::json!({"response_type": "ephemeral", "text": "No sessions connected."})
    );
}

#[test]
fn text_response_shape() {
    let body = text_response("Unbound 2 thread(s) in this channel.");
    assert_eq!(
        body,
        serde_json::json!({"response_type": "ephemeral", "text": "Unbound 2 thread(s) in this channel."})
    );
}
