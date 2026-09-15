use super::*;
use serde_json::json;

fn block_actions_payload() -> serde_json::Value {
    json!({
        "type": "block_actions",
        "user": {"id": "U0123ABCD", "username": "me", "name": "me", "team_id": "T0001ABCD"},
        "api_app_id": "A0001ABCD",
        "token": "9s8d9as89d8as9d8as989",
        "container": {"type": "message", "message_ts": "1789560600.000700", "channel_id": "C0AB12CD3", "is_ephemeral": true},
        "trigger_id": "12321423423.333649436676.d8c1bb837935619ccad0f624c448ffb3",
        "team": {"id": "T0001ABCD", "domain": "example"},
        "enterprise": null,
        "is_enterprise_install": false,
        "channel": {"id": "C0AB12CD3", "name": "dev"},
        "state": {"values": {}},
        "response_url": "https://hooks.slack.com/actions/T0001ABCD/1232321423432/D09sSasdasdAS9091209",
        "actions": [{
            "type": "static_select",
            "action_id": "sccr_select",
            "block_id": "Xy1z",
            "selected_option": {"text": {"type": "plain_text", "text": "macbook:proj", "emoji": true}, "value": "macbook:proj"},
            "placeholder": {"type": "plain_text", "text": "Choose a session", "emoji": true},
            "action_ts": "1789560610.123456"
        }]
    })
}

#[test]
fn block_actions_extracts_selection() {
    let payload = block_actions_payload();
    assert_eq!(
        selected_session(&payload),
        Some(Selection {
            session: "macbook:proj".to_string(),
            channel: "C0AB12CD3".to_string(),
            user: "U0123ABCD".to_string(),
            response_url:
                "https://hooks.slack.com/actions/T0001ABCD/1232321423432/D09sSasdasdAS9091209"
                    .to_string(),
        })
    );
}

#[test]
fn other_action_id_ignored() {
    let mut payload = block_actions_payload();
    payload["actions"][0]["action_id"] = json!("something_else");
    assert_eq!(selected_session(&payload), None);
}

#[test]
fn non_block_actions_ignored() {
    let mut payload = block_actions_payload();
    payload["type"] = json!("view_submission");
    assert_eq!(selected_session(&payload), None);
}
