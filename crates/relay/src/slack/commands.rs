//! `/cc` slash command parsing and responses (spec ss5.4, ss6.4).

use serde_json::{Value, json};

const NO_SESSIONS: &str = "No sessions connected.";
const PICKER_PROMPT: &str = "Select a Claude Code session to connect to this channel.";
const PICKER_PLACEHOLDER: &str = "Choose a session";

#[derive(Debug, PartialEq)]
pub enum Cmd {
    Pick,
    List,
    Unbind,
    Help,
}

pub fn parse(text: &str) -> Cmd {
    match text.trim().to_ascii_lowercase().as_str() {
        "" => Cmd::Pick,
        "list" => Cmd::List,
        "unbind" => Cmd::Unbind,
        _ => Cmd::Help,
    }
}

/// Ephemeral body for `/cc`. `names` MUST already be sorted by the caller (hub order).
pub fn pick_response(names: &[String]) -> Value {
    if names.is_empty() {
        return json!({"response_type": "ephemeral", "text": NO_SESSIONS});
    }

    let options: Vec<Value> = names
        .iter()
        .take(100)
        .map(|name| {
            let label: String = name.chars().take(75).collect();
            json!({"text": {"type": "plain_text", "text": label}, "value": name})
        })
        .collect();

    json!({
        "response_type": "ephemeral",
        "text": PICKER_PROMPT,
        "blocks": [{
            "type": "section",
            "text": {"type": "mrkdwn", "text": PICKER_PROMPT},
            "accessory": {
                "type": "static_select",
                "action_id": "sccr_select",
                "placeholder": {"type": "plain_text", "text": PICKER_PLACEHOLDER},
                "options": options
            }
        }]
    })
}

/// Plain ephemeral text body, used by `List`/`Unbind`/`Help` and rejection messages.
pub fn text_response(text: &str) -> Value {
    json!({"response_type": "ephemeral", "text": text})
}

#[cfg(test)]
mod tests;
