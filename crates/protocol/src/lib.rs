//! Wire types shared between sccr-agent and sccr-relay.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentMsg {
    Hello {
        version: u32,
        name: String,
        host: String,
        cwd: String,
    },
    Reply {
        chat_id: String,
        text: String,
    },
    PermissionRequest {
        request_id: String,
        tool_name: String,
        description: String,
        input_preview: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RelayMsg {
    Welcome {
        name: String,
    },
    Inbound {
        chat_id: String,
        user: String,
        text: String,
    },
    PermissionVerdict {
        request_id: String,
        behavior: Behavior,
    },
    Error {
        message: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Behavior {
    Allow,
    Deny,
}

#[cfg(test)]
mod tests;
