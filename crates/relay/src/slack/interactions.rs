//! `block_actions` interaction payload parsing (spec ss5.5).

use serde_json::Value;

#[derive(Debug, PartialEq)]
pub struct Selection {
    pub session: String,
    pub channel: String,
    pub user: String,
    pub response_url: String,
}

/// Extracts the selected session from a `block_actions` payload, if any.
pub fn selected_session(payload: &Value) -> Option<Selection> {
    if payload.get("type").and_then(Value::as_str) != Some("block_actions") {
        return None;
    }
    let action = payload.get("actions")?.get(0)?;
    if action.get("action_id").and_then(Value::as_str) != Some("sccr_select") {
        return None;
    }
    let session = action.get("selected_option")?.get("value")?.as_str()?;
    let channel = payload.get("channel")?.get("id")?.as_str()?;
    let user = payload.get("user")?.get("id")?.as_str()?;
    let response_url = payload.get("response_url")?.as_str()?;

    Some(Selection {
        session: session.to_string(),
        channel: channel.to_string(),
        user: user.to_string(),
        response_url: response_url.to_string(),
    })
}

#[cfg(test)]
mod tests;
