//! Slack Web API boundary: chat.postMessage and response_url (spec 6.3, 6.5, 12.2).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use serde_json::{Value, json};

const POST_MESSAGE_URL: &str = "https://slack.com/api/chat.postMessage";

/// Slack error code (e.g. `not_in_channel`), or a short transport error.
#[derive(Debug, Clone, PartialEq)]
pub struct SlackError(pub String);

#[async_trait]
pub trait SlackApi: Send + Sync {
    /// Posts a message and returns its `ts`.
    async fn post_message(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        text: &str,
    ) -> Result<String, SlackError>;
    async fn respond(&self, response_url: &str, text: &str) -> Result<(), SlackError>;
}

/// Escapes a value inserted into mrkdwn text (spec 13.1).
pub fn escape_mrkdwn(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub struct HttpSlack {
    client: reqwest::Client,
    bot_token: String,
}

impl HttpSlack {
    pub fn new(bot_token: String) -> Result<Self, SlackError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| SlackError(e.to_string()))?;
        Ok(Self { client, bot_token })
    }
}

fn transport_error(e: reqwest::Error) -> SlackError {
    SlackError(e.without_url().to_string())
}

#[async_trait]
impl SlackApi for HttpSlack {
    async fn post_message(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        text: &str,
    ) -> Result<String, SlackError> {
        let body = match thread_ts {
            Some(ts) => json!({"channel": channel, "thread_ts": ts, "text": text}),
            None => json!({"channel": channel, "text": text}),
        };
        let resp: Value = self
            .client
            .post(POST_MESSAGE_URL)
            .bearer_auth(&self.bot_token)
            .header(CONTENT_TYPE, "application/json; charset=utf-8")
            .body(body.to_string())
            .send()
            .await
            .map_err(transport_error)?
            .json()
            .await
            .map_err(transport_error)?;

        if resp.get("ok").and_then(Value::as_bool) != Some(true) {
            let code = resp.get("error").and_then(Value::as_str);
            return Err(SlackError(code.unwrap_or("unknown_error").to_string()));
        }
        resp.get("ts")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| SlackError("missing_ts".to_string()))
    }

    async fn respond(&self, response_url: &str, text: &str) -> Result<(), SlackError> {
        let body = json!({"response_type": "ephemeral", "replace_original": false, "text": text});
        let resp = self
            .client
            .post(response_url)
            .json(&body)
            .send()
            .await
            .map_err(transport_error)?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(SlackError(format!("http_{}", resp.status().as_u16())))
        }
    }
}

/// Development client: prints each call to stdout (spec 12.2).
pub struct LogSlack {
    now: fn() -> i64,
    counter: AtomicU64,
}

impl LogSlack {
    pub fn new(now: fn() -> i64) -> Self {
        Self {
            now,
            counter: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl SlackApi for LogSlack {
    async fn post_message(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        text: &str,
    ) -> Result<String, SlackError> {
        let n = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let ts = format!("{}.{n:06}", (self.now)());
        match thread_ts {
            Some(thread) => {
                println!("[slack] post chat_id={channel}:{thread} ts={ts} text={text:?}")
            }
            None => println!("[slack] post channel={channel} ts={ts} text={text:?}"),
        }
        Ok(ts)
    }

    async fn respond(&self, _response_url: &str, text: &str) -> Result<(), SlackError> {
        println!("[slack] respond text={text:?}");
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    Post {
        channel: String,
        thread_ts: Option<String>,
        text: String,
    },
    Respond {
        response_url: String,
        text: String,
    },
}

/// Test double. Records every call; `post_message` returns
/// `"1700000000.{n:06}"` (n counts posts from 1) or the configured error.
#[derive(Default)]
pub struct FakeSlack {
    calls: Mutex<Vec<Call>>,
    posts: AtomicU64,
    fail_post: Option<SlackError>,
}

impl FakeSlack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every `post_message` fails with `err` (still recorded).
    pub fn failing(err: SlackError) -> Self {
        Self {
            fail_post: Some(err),
            ..Self::default()
        }
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn record(&self, call: Call) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call);
    }
}

#[async_trait]
impl SlackApi for FakeSlack {
    async fn post_message(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        text: &str,
    ) -> Result<String, SlackError> {
        self.record(Call::Post {
            channel: channel.to_string(),
            thread_ts: thread_ts.map(str::to_string),
            text: text.to_string(),
        });
        if let Some(err) = &self.fail_post {
            return Err(err.clone());
        }
        let n = self.posts.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(format!("1700000000.{n:06}"))
    }

    async fn respond(&self, response_url: &str, text: &str) -> Result<(), SlackError> {
        self.record(Call::Respond {
            response_url: response_url.to_string(),
            text: text.to_string(),
        });
        Ok(())
    }
}
