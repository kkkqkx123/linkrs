//! API Core Layer Type Definitions
//!
//! Business types that are independent of the transport layer

use crate::core::types::TransactionId;
use crate::core::Value;
use crate::query::parser::ast::stmt::Ast;
use std::collections::HashMap;
use std::sync::Arc;

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
    /// Pre-parsed statement AST from the API-layer classification pass.
    ///
    /// When present, the query engine skips its own parse of the query text
    /// (single-parse pipeline for transaction / session commands). The AST
    /// carries its own expression analysis context, so expression ids stay
    /// consistent with the plan generated from it.
    pub parsed_statement: Option<Arc<Ast>>,
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
            parsed_statement: None,
        }
    }
}

/// Query results
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
    pub metadata: ExecutionMetadata,
}

impl QueryResult {
    /// Value of the first column of the first row, if any.
    ///
    /// Convenience accessor for single-value projection results (e.g.
    /// `RETURN COUNT(...) as total`), independent of map iteration order.
    pub fn first_value(&self) -> Option<&Value> {
        let col = self.columns.first()?;
        self.rows.first()?.values.get(col)
    }

    /// Values of the first column across all rows, in row order.
    pub fn first_column_values(&self) -> impl Iterator<Item = &Value> {
        let col = self.columns.first().cloned();
        self.rows
            .iter()
            .filter_map(move |row| col.as_ref().and_then(|c| row.get(c)))
    }
}

/// Result row
#[derive(Debug, Clone)]
pub struct Row {
    pub values: HashMap<String, Value>,
}

impl Default for Row {
    fn default() -> Self {
        Self::new()
    }
}

impl Row {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            values: HashMap::with_capacity(capacity),
        }
    }

    pub fn insert(&mut self, key: String, value: Value) {
        self.values.insert(key, value);
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
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
    pub data_type: crate::core::DataType,
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
    pub vid_type: crate::core::DataType,
    pub comment: Option<String>,
}

impl Default for SpaceConfig {
    fn default() -> Self {
        Self {
            partition_num: 100,
            replica_factor: 1,
            vid_type: crate::core::DataType::String,
            comment: None,
        }
    }
}
