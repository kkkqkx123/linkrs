//! API Core Layer Type Definitions
//!
//! Business types that are independent of the transport layer

use graphdb_core::types::{SpaceSummary, TransactionId};
use graphdb_core::Value;
use graphdb_query::executor::base::ExecutionResult;
use graphdb_query::parser::ast::stmt::Ast;
use std::collections::HashMap;
use std::sync::Arc;

/// Consistency level for reads that may lag behind the sync frontier.
///
/// - `Eventual` (default) – no waiting, may observe `frontier_lag`.
/// - `ReadYourWrites` – wait until the secondary index frontier has caught up
///   to the caller's `commit_lsn` or the timeout expires. Degraded frontiers
///   fail the read instead of returning stale data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsistencyLevel {
    Eventual,
    ReadYourWrites { timeout_ms: u64 },
}

impl Default for ConsistencyLevel {
    fn default() -> Self {
        Self::Eventual
    }
}

/// Query request
#[derive(Debug, Clone)]
pub struct QueryRequest {
    pub space_id: Option<u64>,
    pub space_name: Option<String>,
    pub auto_commit: bool,
    pub transaction_id: Option<TransactionId>,
    pub parameters: Option<HashMap<String, Value>>,
    /// Session variable snapshot (`$name` references), captured once per
    /// statement. Distinct from `parameters` (`@name` references).
    pub session_variables: Option<HashMap<String, Value>>,
    /// Optional server-assigned query ID threaded to the execution runtime.
    pub query_id: Option<u64>,
    /// Transaction isolation level for executions inside an explicit
    /// transaction (injected by the API layer from `TransactionExecution`).
    /// `None` = auto-commit statement-level snapshot semantics.
    pub isolation_level: Option<graphdb_core::types::TransactionIsolationLevel>,
    /// Pre-parsed statement AST from the API-layer classification pass.
    ///
    /// When present, the query engine skips its own parse of the query text
    /// (single-parse pipeline for transaction / session commands). The AST
    /// carries its own expression analysis context, so expression ids stay
    /// consistent with the plan generated from it.
    pub parsed_statement: Option<Arc<Ast>>,
    /// Consistency requirement for secondary-index reads (vector/fulltext).
    /// `Eventual` is the default for backward compatibility; `ReadYourWrites`
    /// makes a `SEARCH VECTOR` block until the outbox frontier catches up.
    pub consistency: ConsistencyLevel,
    /// Minimum LSN to wait for when `consistency` is `ReadYourWrites`. When
    /// `None`, the current outbox `materialized_lsn` is used.
    pub minimum_lsn: Option<graphdb_core::types::CommitLsn>,
}

impl Default for QueryRequest {
    fn default() -> Self {
        Self {
            space_id: None,
            space_name: None,
            auto_commit: true,
            transaction_id: None,
            parameters: None,
            session_variables: None,
            query_id: None,
            isolation_level: None,
            parsed_statement: None,
            consistency: ConsistencyLevel::default(),
            minimum_lsn: None,
        }
    }
}

/// Query results
///
/// Wraps the engine-level [`ExecutionResult`] (the single source of truth for
/// result rows) together with API-layer execution metadata. No row-level
/// copy or re-shaping happens at this boundary.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Engine execution result (DataSet / Empty / Success / SpaceSwitched).
    pub execution: ExecutionResult,
    /// API-layer execution metadata (timing, scanned/returned counts).
    pub metadata: ExecutionMetadata,
}

impl QueryResult {
    /// Create a query result from an engine execution result.
    pub fn new(execution: ExecutionResult, metadata: ExecutionMetadata) -> Self {
        Self {
            execution,
            metadata,
        }
    }

    /// Create an empty successful query result.
    pub fn empty() -> Self {
        Self::new(ExecutionResult::Empty, ExecutionMetadata::default())
    }

    /// Column names of the dataset, empty for non-dataset results.
    pub fn columns(&self) -> &[String] {
        self.execution
            .to_data_set()
            .map(|data| data.col_names.as_slice())
            .unwrap_or(&[])
    }

    /// Row values in column order, empty for non-dataset results.
    pub fn rows(&self) -> &[Vec<Value>] {
        self.execution
            .to_data_set()
            .map(|data| data.rows.as_slice())
            .unwrap_or(&[])
    }

    /// Value of the first column of the first row, if any.
    ///
    /// Convenience accessor for single-value projection results (e.g.
    /// `RETURN COUNT(...) as total`).
    pub fn first_value(&self) -> Option<&Value> {
        self.rows().first().and_then(|row| row.first())
    }

    /// Values of the first column across all rows, in row order.
    pub fn first_column_values(&self) -> impl Iterator<Item = &Value> {
        self.rows().iter().filter_map(|row| row.first())
    }

    /// Space summary of a USE-statement result, if any.
    ///
    /// The engine executes `USE` as a DataSet with `space_name` / `space_id` /
    /// `vid_type` columns (the `SpaceSwitched` variant is never produced), so
    /// both representations are recognized here.
    pub fn space_summary(&self) -> Option<SpaceSummary> {
        match &self.execution {
            ExecutionResult::SpaceSwitched(summary) => Some(summary.clone()),
            ExecutionResult::DataSet { data } => {
                let row = data.rows.first()?;
                let name = match data
                    .col_names
                    .iter()
                    .position(|c| c == "space_name")
                    .and_then(|idx| row.get(idx))?
                {
                    Value::String(s) => s.to_string(),
                    _ => return None,
                };
                let id = match data
                    .col_names
                    .iter()
                    .position(|c| c == "space_id")
                    .and_then(|idx| row.get(idx))?
                {
                    Value::BigInt(id) => *id as u64,
                    _ => return None,
                };
                let vid_type = data
                    .col_names
                    .iter()
                    .position(|c| c == "vid_type")
                    .and_then(|idx| row.get(idx))
                    .and_then(|v| match v {
                        Value::String(s) => s.parse().ok(),
                        _ => None,
                    })
                    .unwrap_or(graphdb_core::DataType::String);
                Some(SpaceSummary::new(id, name, vid_type))
            }
            _ => None,
        }
    }
}

/// Metadata of the executor
#[derive(Debug, Clone, Default)]
pub struct ExecutionMetadata {
    pub execution_time_ms: u64,
    pub rows_scanned: u64,
    pub rows_returned: u64,
    pub cache_hit: bool,
}

/// Transaction handler
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransactionHandle(pub TransactionId);

impl TransactionHandle {
    pub fn id(&self) -> u64 {
        self.0.as_u64()
    }

    pub fn transaction_id(&self) -> TransactionId {
        self.0
    }
}

impl From<u64> for TransactionHandle {
    fn from(id: u64) -> Self {
        Self(TransactionId::from(id))
    }
}

/// Save Point ID
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavepointId(pub u64);

/// The Schema attribute is used for definition purposes.
#[derive(Debug, Clone)]
pub struct PropertyDef {
    pub name: String,
    pub data_type: graphdb_core::DataType,
    pub nullable: bool,
    pub default_value: Option<Value>,
    pub comment: Option<String>,
}

/// Index target type
#[derive(Debug, Clone)]
pub enum IndexTarget {
    Tag { name: String, fields: Vec<String> },
    Edge { name: String, fields: Vec<String> },
}

/// Space configuration
#[derive(Debug, Clone)]
pub struct SpaceConfig {
    pub partition_num: i32,
    pub replica_factor: i32,
    pub vid_type: graphdb_core::DataType,
    pub comment: Option<String>,
}

impl Default for SpaceConfig {
    fn default() -> Self {
        Self {
            partition_num: 100,
            replica_factor: 1,
            vid_type: graphdb_core::DataType::String,
            comment: None,
        }
    }
}
