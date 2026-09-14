//! Shared types for the sync manager: error, configs, edge views.
#[cfg(feature = "fulltext")]
use crate::coordinator::CoordinatorError;
use graphdb_core::Value;
#[derive(Debug, Clone)]
pub struct IndexCreateRequest {
    pub space_id: u64,
    pub index_name: String,
    pub schema_name: String,
    pub index_type: String,
    pub fields: Vec<(String, Value)>,
    pub properties: Vec<String>,
}
/// Delivery policy shared by one or more sync manager workers.
#[derive(Debug, Clone)]
pub struct OutboxConsumerConfig {
    pub consumer_id: String,
    pub batch_size: usize,
    pub lease_duration_ms: u64,
    pub max_retries: u64,
    /// Max concurrent `claim → apply → ack` workers per target.
    /// `1` = single-threaded delivery (Local); `4` = Qdrant concurrent.
    pub max_concurrency: usize,
}
impl Default for OutboxConsumerConfig {
    fn default() -> Self {
        Self {
            consumer_id: format!("sync-manager-{}", uuid::Uuid::new_v4()),
            batch_size: 128,
            lease_duration_ms: 30_000,
            max_retries: 16,
            max_concurrency: 1,
        }
    }
}
/// Backpressure limits for the transactional outbox staging path.
#[derive(Debug, Clone)]
pub struct OutboxBackpressureConfig {
    /// Maximum intents staged for a single transaction.
    pub max_pending_per_txn: usize,
    /// Maximum intents staged across all in-flight transactions plus the
    /// durable `pending` backlog (queried lazily). Exceeding either limit
    /// makes `stage_intent` return `OutboxBackpressure`.
    pub max_pending_total: usize,
}
impl Default for OutboxBackpressureConfig {
    fn default() -> Self {
        Self {
            max_pending_per_txn: 10_000,
            max_pending_total: 100_000,
        }
    }
}
#[derive(Debug, Clone)]
pub struct EdgeRef<'a> {
    pub src: &'a Value,
    pub dst: &'a Value,
    pub edge_type: &'a str,
}
impl<'a> EdgeRef<'a> {
    pub fn new(src: &'a Value, dst: &'a Value, edge_type: &'a str) -> Self {
        Self {
            src,
            dst,
            edge_type,
        }
    }

    pub fn id(&self) -> String {
        format!("{}->{}", self.src, self.dst)
    }
}
#[derive(Debug, Clone)]
pub struct EdgeProps<'a> {
    pub old: &'a [(String, Value)],
    pub new: &'a [(String, Value)],
}
impl<'a> EdgeProps<'a> {
    pub fn new(old: &'a [(String, Value)], new: &'a [(String, Value)]) -> Self {
        Self { old, new }
    }
}
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[cfg(feature = "fulltext")]
    #[error("Coordinator error: {0}")]
    CoordinatorError(#[from] CoordinatorError),

    #[cfg(feature = "fulltext")]
    #[error("Sync coordinator error: {0}")]
    SyncCoordinatorError(#[from] crate::coordinator::SyncCoordinatorError),

    #[error("Buffer error: {0}")]
    BufferError(String),

    #[error("Vector error: {0}")]
    VectorError(String),

    #[error("Persistence error: {0}")]
    PersistenceError(String),

    #[error("Outbox backpressure: {0}")]
    OutboxBackpressure(String),

    #[error("Rebuild busy: {0}")]
    RebuildBusy(String),

    #[error("Internal error: {0}")]
    Internal(String),
}
