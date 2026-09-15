//! Table of connected agents: session name -> outbound queue.

use std::collections::HashMap;

use protocol::RelayMsg;
use tokio::sync::{RwLock, mpsc};

#[derive(Debug, PartialEq)]
pub struct Duplicate;

#[derive(Debug, PartialEq)]
pub struct Offline;

#[derive(Default)]
pub struct Hub {
    agents: RwLock<HashMap<String, mpsc::Sender<RelayMsg>>>,
}

impl Hub {
    /// Adds a connection. A name that is already connected is rejected.
    pub async fn register(&self, name: &str, tx: mpsc::Sender<RelayMsg>) -> Result<(), Duplicate> {
        let mut agents = self.agents.write().await;
        if agents.contains_key(name) {
            return Err(Duplicate);
        }
        agents.insert(name.to_string(), tx);
        Ok(())
    }

    pub async fn unregister(&self, name: &str) {
        self.agents.write().await.remove(name);
    }

    /// Connected session names, sorted ascending.
    pub async fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.agents.read().await.keys().cloned().collect();
        names.sort();
        names
    }

    pub async fn send(&self, name: &str, msg: RelayMsg) -> Result<(), Offline> {
        let tx = self.agents.read().await.get(name).cloned().ok_or(Offline)?;
        tx.send(msg).await.map_err(|_| Offline)
    }
}

#[cfg(test)]
mod tests;
