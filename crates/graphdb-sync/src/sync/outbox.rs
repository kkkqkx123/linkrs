use serde::{Deserialize, Serialize};

use crate::core::types::TransactionId;
use crate::core::{Edge, Value};
use crate::sync::types::ChangeType;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutboxPayload {
    Vertex {
        space_id: u64,
        tag_name: String,
        vertex_id: Value,
        properties: Vec<(String, Value)>,
        change_type: ChangeType,
    },
    EdgeInsert {
        space_id: u64,
        edge: Edge,
    },
    EdgeDelete {
        space_id: u64,
        src: Value,
        dst: Value,
        edge_type: String,
    },
    CreateIndex {
        space_id: u64,
        index_name: String,
        index_type: String,
        fields: Vec<(String, Value)>,
        properties: Vec<String>,
    },
    DropIndex {
        space_id: u64,
        index_name: String,
        index_type: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxEvent {
    pub id: String,
    pub transaction_id: Option<TransactionId>,
    pub sequence: u64,
    pub committed: bool,
    pub retries: u64,
    pub created_at_ms: u64,
    pub payload: OutboxPayload,
    #[serde(default = "default_target")]
    pub target: String,
    #[serde(default)]
    pub partition: String,
    #[serde(default)]
    pub idempotency_key: String,
    #[serde(default)]
    pub enqueue_sequence: u64,
    #[serde(default)]
    pub ordering_key: String,
    #[serde(default)]
    pub next_attempt_at_ms: u64,
    #[serde(default)]
    pub lease_owner: Option<String>,
    #[serde(default)]
    pub lease_until_ms: u64,
    #[serde(default)]
    pub lease_epoch: u64,
    #[serde(default)]
    pub dead_lettered: bool,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutboxStats {
    pub pending: usize,
    pub retries: u64,
    pub oldest_event_age_ms: u64,
    pub dead_lettered: usize,
    pub leased: usize,
    /// Bytes written to the durable event file over its lifetime.
    pub write_amplification_bytes: u64,
    /// Time spent waiting for the cross-process lock.
    pub lock_wait_nanos: u64,
    /// Number of full-file persistence operations.
    pub persist_operations: u64,
}

pub fn default_target() -> String {
    "sync".to_string()
}
