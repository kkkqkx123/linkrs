//! API Core Layer Type Definitions
//!
//! Business types that are independent of the transport layer

use linkrs_core::types::{SpaceSummary, TransactionId};
use linkrs_core::Value;
use linkrs_query::executor::base::ExecutionResult;
use linkrs_query::parser::ast::stmt::Ast;
use std::collections::HashMap;
use std::sync::Arc;

/// Consistency level for reads that may lag behind the sync frontier.
///
/// - `Eventual` (default) – no waiting, may observe `frontier_lag`.
/// - `ReadYourWrites` – wait until the secondary index frontier has caught up
///   to the caller's `commit_lsn` or the timeout expires. Degraded frontiers
///   fail the read instead of returning stale data.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConsistencyLevel {
    #[default]
    Eventual,
    ReadYourWrites(linkrs_core::types::ReadYourWritesConfig),
}

impl ConsistencyLevel {
    /// Returns the RYW config if this is a `ReadYourWrites` level.
    pub fn read_your_writes(&self) -> Option<&linkrs_core::types::ReadYourWritesConfig> {
        match self {
            Self::ReadYourWrites(cfg) => Some(cfg),
            Self::Eventual => None,
        }
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
    pub isolation_level: Option<linkrs_core::types::TransactionIsolationLevel>,
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
        }
    }
}

/// Builder for [`QueryRequest`].
///
/// Callers assemble a request from a handful of fields and rely on
/// defaults for the rest, so adding a field to the request never requires
/// touching every construction site.
#[derive(Debug, Clone, Default)]
pub struct QueryRequestBuilder {
    request: QueryRequest,
}

impl QueryRequestBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn space_id(mut self, space_id: Option<u64>) -> Self {
        self.request.space_id = space_id;
        self
    }

    pub fn space_name(mut self, space_name: Option<String>) -> Self {
        self.request.space_name = space_name;
        self
    }

    pub fn auto_commit(mut self, auto_commit: bool) -> Self {
        self.request.auto_commit = auto_commit;
        self
    }

    pub fn transaction_id(mut self, transaction_id: Option<TransactionId>) -> Self {
        self.request.transaction_id = transaction_id;
        self
    }

    pub fn parameters(mut self, parameters: Option<HashMap<String, Value>>) -> Self {
        self.request.parameters = parameters;
        self
    }

    pub fn session_variables(mut self, session_variables: Option<HashMap<String, Value>>) -> Self {
        self.request.session_variables = session_variables;
        self
    }

    pub fn query_id(mut self, query_id: Option<u64>) -> Self {
        self.request.query_id = query_id;
        self
    }

    pub fn isolation_level(
        mut self,
        isolation_level: Option<linkrs_core::types::TransactionIsolationLevel>,
    ) -> Self {
        self.request.isolation_level = isolation_level;
        self
    }

    pub fn parsed_statement(mut self, parsed_statement: Option<Arc<Ast>>) -> Self {
        self.request.parsed_statement = parsed_statement;
        self
    }

    pub fn consistency(mut self, consistency: ConsistencyLevel) -> Self {
        self.request.consistency = consistency;
        self
    }

    pub fn build(self) -> QueryRequest {
        self.request
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
    ///
    /// This column-name sniffing is an adapter over the engine's current
    /// DataSet-shaped `USE` result; once the engine emits a structured
    /// `SpaceSwitched` variant, switch to that and delete the DataSet arm.
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
                    .unwrap_or(linkrs_core::DataType::String);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_starts_from_request_defaults() {
        let request = QueryRequestBuilder::new().build();
        assert!(request.space_id.is_none());
        assert!(request.space_name.is_none());
        assert!(request.auto_commit);
        assert!(request.transaction_id.is_none());
        assert!(request.parameters.is_none());
        assert!(request.session_variables.is_none());
        assert!(request.query_id.is_none());
        assert!(request.isolation_level.is_none());
        assert!(request.parsed_statement.is_none());
        assert_eq!(request.consistency, ConsistencyLevel::Eventual);
    }

    #[test]
    fn builder_overrides_only_named_fields() {
        let request = QueryRequestBuilder::new()
            .space_id(Some(7))
            .auto_commit(false)
            .query_id(Some(42))
            .build();
        assert_eq!(request.space_id, Some(7));
        assert!(!request.auto_commit);
        assert_eq!(request.query_id, Some(42));
        // Untouched fields keep their defaults.
        assert!(request.transaction_id.is_none());
        assert_eq!(request.consistency, ConsistencyLevel::Eventual);
    }
}

/// The Schema attribute is used for definition purposes.
#[derive(Debug, Clone)]
pub struct PropertyDef {
    pub name: String,
    pub data_type: linkrs_core::DataType,
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
    pub vid_type: linkrs_core::DataType,
    pub comment: Option<String>,
}

impl Default for SpaceConfig {
    fn default() -> Self {
        Self {
            partition_num: 100,
            replica_factor: 1,
            vid_type: linkrs_core::DataType::String,
            comment: None,
        }
    }
}
