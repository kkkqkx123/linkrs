use serde::{Deserialize, Serialize};

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
        ranking: i64,
    },
    CreateIndex {
        space_id: u64,
        index_name: String,
        schema_name: String,
        index_type: String,
        fields: Vec<(String, Value)>,
        properties: Vec<String>,
    },
    DropIndex {
        space_id: u64,
        index_name: String,
        schema_name: String,
        index_type: String,
        fields: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutboxStats {
    pub pending: usize,
    pub retries: u64,
    pub oldest_event_age_ms: u64,
    pub dead_lettered: usize,
    pub leased: usize,
    /// Current durable SQLite projection size.
    pub write_amplification_bytes: u64,
    /// Time spent waiting for the SQLite write lock.
    pub lock_wait_nanos: u64,
    /// Number of durable projection writes observed by the collector.
    pub persist_operations: u64,
}
