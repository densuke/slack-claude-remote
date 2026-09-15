//! Shared application state handed to every route.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

use crate::config::Config;
use crate::hub::Hub;
use crate::slack::api::SlackApi;
use crate::slack::events::Dedupe;
use crate::store::Store;

const DEDUPE_CAPACITY: usize = 1000;

/// Unanswered permission request, keyed by request_id (spec 7.4). Used by T6-2.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub chat_id: String,
    pub session: String,
    pub created: i64,
}

pub struct AppState {
    pub cfg: Config,
    pub hub: Hub,
    pub store: Mutex<Store>,
    pub slack: Arc<dyn SlackApi>,
    pub dedupe: Mutex<Dedupe>,
    pub pending: Mutex<HashMap<String, Pending>>,
    /// Unix seconds; injectable for tests.
    pub now: fn() -> i64,
}

impl AppState {
    pub fn new(cfg: Config, store: Store, slack: Arc<dyn SlackApi>, now: fn() -> i64) -> Self {
        Self {
            cfg,
            hub: Hub::default(),
            store: Mutex::new(store),
            slack,
            dedupe: Mutex::new(Dedupe::new(DEDUPE_CAPACITY)),
            pending: Mutex::new(HashMap::new()),
            now,
        }
    }
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
