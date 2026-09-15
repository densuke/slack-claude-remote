//! Permission relay: verdict parsing, prompt text, pending expiry (spec 7).

use std::collections::{BTreeMap, HashMap};

use protocol::Behavior;

use crate::chunk::chunk;
use crate::slack::api::escape_mrkdwn;
use crate::slack::events::Candidate;
use crate::state::{AppState, Pending};
use crate::store::Binding;

const CHUNK_CHARS: usize = 3900;
const PENDING_TTL_SECS: i64 = 1800;

/// Hand-written parser for `"yes <id>" / "no <id>"` (spec 7.5). No regex crate.
pub fn parse_verdict(text: &str) -> Option<(String, Behavior)> {
    let trimmed = text.trim();
    let split_at = trimmed.find(char::is_whitespace)?;
    let (word1, rest) = trimmed.split_at(split_at);
    let word2 = rest.trim_start();
    if word2.chars().any(char::is_whitespace) {
        return None;
    }

    let behavior = match word1.to_ascii_lowercase().as_str() {
        "y" | "yes" => Behavior::Allow,
        "n" | "no" => Behavior::Deny,
        _ => return None,
    };

    let id = word2.to_ascii_lowercase();
    let valid = id.chars().count() == 5 && id.chars().all(|c| c.is_ascii_lowercase() && c != 'l');
    if !valid {
        return None;
    }
    Some((id, behavior))
}

/// Spec 7.3. `{...}` values are mrkdwn-escaped; `request_id` is inserted verbatim.
pub fn prompt_text(
    tool_name: &str,
    description: &str,
    input_preview: &str,
    request_id: &str,
) -> String {
    format!(
        "Claude wants to run *{}*: {}\n```{}```\nReply \"yes {request_id}\" or \"no {request_id}\" in this thread.",
        escape_mrkdwn(tool_name),
        escape_mrkdwn(description),
        escape_mrkdwn(input_preview),
    )
}

/// Drops pending requests older than 1800s (spec 7.4). Exactly 1800s is kept.
pub fn sweep(pending: &mut HashMap<String, Pending>, now: i64) {
    pending.retain(|_, p| now - p.created <= PENDING_TTL_SECS);
}

/// Spec 7.2 step 2: the binding for `session` with the highest `created`,
/// breaking ties by the largest chat_id key.
fn latest_binding<'a>(
    bindings: &'a BTreeMap<String, Binding>,
    session: &str,
) -> Option<(&'a str, &'a Binding)> {
    bindings
        .iter()
        .filter(|(_, b)| b.session == session)
        .max_by(|(k1, b1), (k2, b2)| (b1.created, k1.as_str()).cmp(&(b2.created, k2.as_str())))
        .map(|(k, b)| (k.as_str(), b))
}

/// Spec 6.2 step 9: whether a thread reply is a valid verdict for its pending request.
pub fn match_verdict(
    pending: &HashMap<String, Pending>,
    candidate: &Candidate,
    binding: &Binding,
) -> Option<(String, Behavior)> {
    let (request_id, behavior) = parse_verdict(&candidate.text)?;
    let p = pending.get(&request_id)?;
    if p.chat_id == candidate.chat_id
        && p.session == binding.session
        && candidate.user == binding.user
    {
        Some((request_id, behavior))
    } else {
        None
    }
}

/// Spec 7.2: agent asked for permission. Finds the binding, records `pending`,
/// and posts the prompt to the thread.
pub async fn handle_request(
    state: &AppState,
    session: &str,
    request_id: String,
    tool_name: &str,
    description: &str,
    input_preview: &str,
) {
    let now = (state.now)();
    let chat_id = {
        let mut pending = state.pending.lock().await;
        sweep(&mut pending, now);
        let store = state.store.lock().await;
        let Some((chat_id, _)) = latest_binding(&store.bindings, session) else {
            eprintln!("permission request {request_id} from {session} dropped: no binding");
            return;
        };
        let chat_id = chat_id.to_string();
        pending.insert(
            request_id.clone(),
            Pending {
                chat_id: chat_id.clone(),
                session: session.to_string(),
                created: now,
            },
        );
        chat_id
    };

    let Some((channel, thread_ts)) = chat_id.split_once(':') else {
        eprintln!("permission request {request_id}: invalid chat_id");
        state.pending.lock().await.remove(&request_id);
        return;
    };

    let prompt = prompt_text(tool_name, description, input_preview, &request_id);
    for piece in chunk(&prompt, CHUNK_CHARS) {
        if let Err(e) = state
            .slack
            .post_message(channel, Some(thread_ts), &piece)
            .await
        {
            eprintln!(
                "permission request {request_id}: prompt post failed: {}",
                e.0
            );
            state.pending.lock().await.remove(&request_id);
            return;
        }
    }
}

#[cfg(test)]
mod tests;
