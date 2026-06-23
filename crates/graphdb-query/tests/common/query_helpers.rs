//! Query Execution Helper Module
//!
//! Provides convenient query execution and result extraction functions for tests

use crate::common::TestResult;
use graphdb_query::core::error::DBError;
use graphdb_query::core::Value;
use graphdb_query::query::executor::base::ExecutionResult;
use graphdb_query::query::query_pipeline_manager::QueryPipelineManager;
use parking_lot::RwLock;
use std::sync::Arc;

/// Query execution helper
pub struct QueryHelper<S: graphdb_query::storage::StorageClient + 'static> {
    pipeline: QueryPipelineManager<S>,
}

impl<S: graphdb_query::storage::StorageClient + 'static> QueryHelper<S> {
    /// Create a new query helper
    pub fn new(storage: Arc<RwLock<S>>) -> Self {
        use graphdb_query::core::stats::StatsManager;
        use graphdb_query::query::optimizer::OptimizerEngine;
        use std::sync::Arc;

        let stats_manager = Arc::new(StatsManager::new());
        let optimizer = Arc::new(OptimizerEngine::default());
        let pipeline = QueryPipelineManager::with_optimizer(storage, stats_manager, optimizer);

        Self { pipeline }
    }

    /// Execute a query and return the result
    pub fn execute(&mut self, query: &str) -> TestResult<ExecutionResult> {
        self.pipeline.execute_query(query).map_err(Box::new)
    }

    /// Execute a DDL statement (CREATE, ALTER, DROP)
    pub fn exec_ddl(&mut self, query: &str) -> TestResult<()> {
        let result = self.execute(query)?;
        #[allow(clippy::unreachable)]
        match result {
            ExecutionResult::Success | ExecutionResult::Empty => Ok(()),
            ExecutionResult::Error(msg) => Err(Box::new(DBError::query(msg))),
            _ => Ok(()),
        }
    }

    /// Execute a DML statement (INSERT, UPDATE, DELETE)
    /// Returns the number of affected rows
    #[allow(unreachable_patterns)]
    pub fn exec_dml(&mut self, query: &str) -> TestResult<usize> {
        let result = self.execute(query)?;
        match result {
            ExecutionResult::Success => Ok(1),
            ExecutionResult::Empty => Ok(0),
            ExecutionResult::DataSet(ds) => Ok(ds.row_count()),
            ExecutionResult::Error(msg) => Err(Box::new(DBError::query(msg))),
            _ => Ok(0),
        }
    }

    /// Execute a query and return the result as a Vec of rows
    pub fn query_rows(&mut self, query: &str) -> TestResult<Vec<Vec<Value>>> {
        let result = self.execute(query)?;
        match result {
            ExecutionResult::DataSet(ds) => Ok(ds.rows),
            ExecutionResult::Empty => Ok(vec![]),
            ExecutionResult::Error(msg) => Err(Box::new(DBError::query(msg))),
            _ => Ok(vec![]),
        }
    }

    /// Execute a query and return a single scalar value
    pub fn query_scalar<T: FromValue>(&mut self, query: &str) -> TestResult<Option<T>> {
        let rows = self.query_rows(query)?;
        if rows.is_empty() || rows[0].is_empty() {
            return Ok(None);
        }
        T::from_value(&rows[0][0]).map(Some)
    }

    /// Execute a query and return the first row
    pub fn query_first(&mut self, query: &str) -> TestResult<Option<Vec<Value>>> {
        let rows = self.query_rows(query)?;
        Ok(rows.into_iter().next())
    }

    /// Execute a query and return the count
    pub fn query_count(&mut self, query: &str) -> TestResult<usize> {
        let result = self.execute(query)?;
        Ok(result.count())
    }
}

/// Trait for converting Value to specific types
pub trait FromValue: Sized {
    fn from_value(value: &Value) -> TestResult<Self>;
}

impl FromValue for i64 {
    fn from_value(value: &Value) -> TestResult<Self> {
        match value {
            Value::Int(i) => Ok(*i as i64),
            _ => Err(Box::new(DBError::validation(format!(
                "Expected Int, got {:?}",
                value
            )))),
        }
    }
}

impl FromValue for String {
    fn from_value(value: &Value) -> TestResult<Self> {
        match value {
            Value::String(s) => Ok(s.clone()),
            _ => Err(Box::new(DBError::validation(format!(
                "Expected String, got {:?}",
                value
            )))),
        }
    }
}

impl FromValue for f64 {
    fn from_value(value: &Value) -> TestResult<Self> {
        match value {
            Value::Float(f) => Ok(*f as f64),
            _ => Err(Box::new(DBError::validation(format!(
                "Expected Float, got {:?}",
                value
            )))),
        }
    }
}

impl FromValue for bool {
    fn from_value(value: &Value) -> TestResult<Self> {
        match value {
            Value::Bool(b) => Ok(*b),
            _ => Err(Box::new(DBError::validation(format!(
                "Expected Bool, got {:?}",
                value
            )))),
        }
    }
}
