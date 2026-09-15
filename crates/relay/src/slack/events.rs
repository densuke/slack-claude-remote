//! Slack Events API envelope classification and dedupe (spec ss5.2, ss5.3).

use std::collections::{HashSet, VecDeque};

use serde_json::Value;

#[derive(Debug, PartialEq)]
pub enum Envelope {
    UrlVerification {
        challenge: String,
    },
    Message {
        event_id: String,
        candidate: Option<Candidate>,
    },
    Other,
}

#[derive(Debug, PartialEq, Clone)]
pub struct Candidate {
    pub chat_id: String,
    pub channel: String,
    pub thread_ts: String,
    pub user: String,
    pub text: String,
}

pub fn classify(body: &Value) -> Envelope {
    if body.get("type").and_then(Value::as_str) == Some("url_verification")
        && let Some(challenge) = body.get("challenge").and_then(Value::as_str)
    {
        return Envelope::UrlVerification {
            challenge: challenge.to_string(),
        };
    }

    if body.get("type").and_then(Value::as_str) == Some("event_callback") {
        let event = body.get("event");
        if event.and_then(|e| e.get("type")).and_then(Value::as_str) == Some("message")
            && let Some(event_id) = body.get("event_id").and_then(Value::as_str)
        {
            let candidate = event.and_then(candidate_from_event);
            return Envelope::Message {
                event_id: event_id.to_string(),
                candidate,
            };
        }
    }

    Envelope::Other
}

fn candidate_from_event(event: &Value) -> Option<Candidate> {
    if event.get("subtype").is_some() {
        return None;
    }
    if event.get("bot_id").is_some() {
        return None;
    }
    let thread_ts = event.get("thread_ts").and_then(Value::as_str)?;
    let ts = event.get("ts").and_then(Value::as_str)?;
    if thread_ts == ts {
        return None;
    }
    let channel = event.get("channel").and_then(Value::as_str)?;
    let user = event.get("user").and_then(Value::as_str)?;
    let text = event.get("text").and_then(Value::as_str).unwrap_or("");

    Some(Candidate {
        chat_id: format!("{channel}:{thread_ts}"),
        channel: channel.to_string(),
        thread_ts: thread_ts.to_string(),
        user: user.to_string(),
        text: text.to_string(),
    })
}

/// FIFO-bounded set of event ids, used to drop Slack's retried deliveries.
pub struct Dedupe {
    order: VecDeque<String>,
    seen: HashSet<String>,
    capacity: usize,
}

impl Dedupe {
    pub fn new(capacity: usize) -> Self {
        Self {
            order: VecDeque::with_capacity(capacity),
            seen: HashSet::with_capacity(capacity),
            capacity,
        }
    }

    /// Returns true only the first time a given event_id is seen.
    pub fn first_seen(&mut self, event_id: &str) -> bool {
        if !self.seen.insert(event_id.to_string()) {
            return false;
        }
        self.order.push_back(event_id.to_string());
        if self.order.len() > self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.seen.remove(&oldest);
        }
        true
    }
}

#[cfg(test)]
mod tests;
