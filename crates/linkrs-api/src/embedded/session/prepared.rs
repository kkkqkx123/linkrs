//! Prepared statements for the embedded API.
//!
//! A lightweight wrapper over `Session::execute_with_params`: the statement
//! text is validated once at `prepare` time and parameter bindings can be
//! reused across executions without re-parsing call sites.

use super::Session;
use crate::api_core::error::{CoreError, CoreResult};
use crate::embedded::result::QueryResult;
use linkrs_core::Value;
use linkrs_storage::StorageClient;
use std::collections::HashMap;

impl<S> Session<S>
where
    S: StorageClient + Clone + Send + Sync + 'static + linkrs_storage::UndoTarget,
{
    /// Prepare a statement for repeated execution with different bindings.
    ///
    /// Validates the query text once (same checks as `validate_query`) and
    /// returns a handle whose bindings can be set via `bind`/`bind_many`
    /// and run via `execute`/`execute_update`/`execute_batch`.
    /// Session variables (`$name`) still resolve from the live session
    /// snapshot at each execution.
    pub fn prepare(&self, query: &str) -> CoreResult<PreparedStatement<'_, S>> {
        self.validate_query(query)?;
        Ok(PreparedStatement::new(self, query.to_string()))
    }
}

/// A pre-validated query with reusable parameter bindings.
///
/// Bindings are `@name` parameters (see `Session::execute_with_params`);
/// session variables (`$name`) continue to come from the session snapshot.
pub struct PreparedStatement<'a, S>
where
    S: StorageClient + Clone + Send + Sync + 'static,
{
    session: &'a Session<S>,
    query: String,
    params: HashMap<String, Value>,
}

impl<'a, S> PreparedStatement<'a, S>
where
    S: StorageClient + Clone + Send + Sync + 'static + linkrs_storage::UndoTarget,
{
    pub(crate) fn new(session: &'a Session<S>, query: String) -> Self {
        Self {
            session,
            query,
            params: HashMap::new(),
        }
    }

    /// Query text this statement was prepared from.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Bind one `@name` parameter.
    pub fn bind(&mut self, name: &str, value: Value) -> CoreResult<()> {
        if name.trim().is_empty() {
            return Err(CoreError::InvalidParameter(
                "parameter name must not be empty".to_string(),
            ));
        }
        self.params.insert(name.to_string(), value);
        Ok(())
    }

    /// Bind many `@name` parameters at once (merges with existing bindings).
    pub fn bind_many(&mut self, params: HashMap<String, Value>) -> CoreResult<()> {
        for (name, value) in params {
            self.bind(&name, value)?;
        }
        Ok(())
    }

    /// Clear all parameter bindings.
    pub fn clear_bindings(&mut self) {
        self.params.clear();
    }

    /// Reset the statement (currently equivalent to clearing bindings).
    pub fn reset(&mut self) {
        self.clear_bindings();
    }

    /// Bound parameter snapshot (for debugging / logging).
    pub fn bindings(&self) -> &HashMap<String, Value> {
        &self.params
    }

    /// Execute the statement with the current bindings.
    pub fn execute(&self) -> CoreResult<QueryResult> {
        self.session
            .execute_with_params(&self.query, self.params.clone())
    }

    /// Execute an updating statement, returning the number of rows returned.
    pub fn execute_update(&self) -> CoreResult<usize> {
        Ok(self.execute()?.len())
    }

    /// Execute the statement once per parameter batch.
    pub fn execute_batch(
        &self,
        param_batches: &[HashMap<String, Value>],
    ) -> CoreResult<Vec<QueryResult>> {
        let mut out = Vec::with_capacity(param_batches.len());
        for batch in param_batches {
            out.push(
                self.session
                    .execute_with_params(&self.query, batch.clone())?,
            );
        }
        Ok(out)
    }
}
