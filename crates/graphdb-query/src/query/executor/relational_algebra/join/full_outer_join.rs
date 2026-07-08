use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::types::ContextualExpression;
use crate::core::{Expression, Value};
use crate::query::executor::base::{ExecutionResult, Executor, JoinConfigWithDesc};
use crate::query::executor::relational_algebra::join::{
    base_join::BaseJoinExecutor,
    hash_table::{build_hash_table, extract_key_values, JoinKey},
    ExpressionContextStruct,
};
use crate::query::DataSet;
use crate::storage::StorageClient;

/// Full Outer Join Configuration
#[derive(Debug, Clone)]
pub struct FullOuterJoinConfig {
    pub hash_keys: Vec<ContextualExpression>,
    pub probe_keys: Vec<ContextualExpression>,
    pub left_var: String,
    pub right_var: String,
    pub output_columns: Vec<String>,
}

/// Full Outer Join Executor
/// Implement a full outer join operation: Retain all records from both the left and right tables, and fill in the unmatched parts with NULL.
pub struct FullOuterJoinExecutor<S: StorageClient + Send + 'static> {
    base: BaseJoinExecutor<S>,
}

impl<S: StorageClient + Send + 'static> FullOuterJoinExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionContextStruct>,
        config: FullOuterJoinConfig,
    ) -> Self {
        let hash_exprs = Self::extract_expressions(&config.hash_keys);
        let probe_exprs = Self::extract_expressions(&config.probe_keys);

        let join_config = JoinConfigWithDesc {
            left_var: config.left_var,
            right_var: config.right_var,
            hash_keys: hash_exprs,
            probe_keys: probe_exprs,
            col_names: config.output_columns,
            description: "Full outer join executor - performs full outer join".to_string(),
        };

        Self {
            base: BaseJoinExecutor::with_description(id, storage, expr_context, join_config),
        }
    }

    /// An auxiliary method for extracting the Expression list from the ContextualExpression list
    fn extract_expressions(ctx_exprs: &[ContextualExpression]) -> Vec<Expression> {
        ctx_exprs
            .iter()
            .filter_map(|ctx_expr| ctx_expr.expression().map(|meta| meta.inner().clone()))
            .collect()
    }

    fn execute_full_outer_join(&mut self) -> DBResult<ExecutionResult> {
        // Obtain the input results from the left and right sides.
        let left_result = self
            .base
            .base
            .context
            .get_result(self.base.left_var())
            .ok_or_else(|| {
                DBError::query(format!(
                    "Left input variable '{}' not found",
                    self.base.left_var()
                ))
            })?
            .clone();

        let right_result = self
            .base
            .base
            .context
            .get_result(self.base.right_var())
            .ok_or_else(|| {
                DBError::query(format!(
                    "Right input variable '{}' not found",
                    self.base.right_var()
                ))
            })?
            .clone();

        // Convert into a dataset
        let left_dataset = match left_result {
            ExecutionResult::DataSet(ds) => ds,
            _ => return Err(DBError::query("Left input must be a DataSet".to_string())),
        };

        let right_dataset = match right_result {
            ExecutionResult::DataSet(ds) => ds,
            _ => return Err(DBError::query("Right input must be a DataSet".to_string())),
        };

        // Pre-built mapping of column names to indexes
        let left_col_map: HashMap<&str, usize> = left_dataset
            .col_names
            .iter()
            .enumerate()
            .map(|(i, name)| (name.as_str(), i))
            .collect();

        let _right_col_map: HashMap<&str, usize> = right_dataset
            .col_names
            .iter()
            .enumerate()
            .map(|(i, name)| (name.as_str(), i))
            .collect();

        // Create a hash table for the left table: Use the join key from the left table as the key, and the row index as the value.
        let left_hash_table_indices = build_hash_table(&left_dataset, self.base.hash_keys())
            .map_err(|e| DBError::query(format!("Failed to build left hash table: {}", e)))?;

        // Convert to a hash table with matching indicators
        let mut left_hash_table: HashMap<JoinKey, Vec<(usize, bool)>> = HashMap::new();
        for (key, indices) in left_hash_table_indices {
            left_hash_table
                .entry(key)
                .or_default()
                .extend(indices.into_iter().map(|idx| (idx, false)));
        }

        // Construct a hash table for the right table: Use the join key from the right table as the key, and the row index as the value.
        let right_hash_table_indices = build_hash_table(&right_dataset, self.base.probe_keys())
            .map_err(|e| DBError::query(format!("Failed to build right hash table: {}", e)))?;

        // Convert to a hash table with matching indicators
        let mut right_hash_table: HashMap<JoinKey, Vec<(usize, bool)>> = HashMap::new();
        for (key, indices) in right_hash_table_indices {
            right_hash_table
                .entry(key)
                .or_default()
                .extend(indices.into_iter().map(|idx| (idx, false)));
        }

        // Constructing the resulting dataset
        let mut result_dataset = DataSet {
            col_names: self.base.col_names().clone(),
            rows: Vec::new(),
        };

        // Process each row of the left table.
        for row in left_dataset.rows.iter() {
            let key_parts = extract_key_values(
                row,
                &left_dataset.col_names,
                self.base.hash_keys(),
                &left_col_map,
            );

            let key = JoinKey::new(key_parts);

            // If there is a matching row in the right table…
            if let Some(right_indices) = right_hash_table.get_mut(&key) {
                for (right_idx, ref mut matched) in right_indices {
                    *matched = true; // Marked as matched
                    if *right_idx < right_dataset.rows.len() {
                        let right_row = &right_dataset.rows[*right_idx];
                        let mut joined_row = row.to_vec();
                        joined_row.extend(right_row.iter().cloned());
                        result_dataset.rows.push(joined_row);
                    }
                }
            } else {
                // No matching rows in the right table were found; the right table column should be filled with NULL.
                let mut null_right_row = Vec::new();
                for _ in 0..right_dataset.col_names.len() {
                    null_right_row.push(Value::Null(crate::core::value::NullType::Null));
                }

                let mut joined_row = row.to_vec();
                joined_row.extend(null_right_row);
                result_dataset.rows.push(joined_row);
            }
        }

        // Add rows that do not have a match in the right table.
        for (key, right_entries) in &right_hash_table {
            for (right_idx, matched) in right_entries {
                if !matched {
                    // Find the corresponding left table key; if there are any unprocessed rows in the left table, handle them accordingly.
                    if *right_idx < right_dataset.rows.len() {
                        let right_row = &right_dataset.rows[*right_idx];

                        // Check whether there is a row in the left table that matches the key of the current row in the right table.
                        let has_left_match = left_hash_table.get(key).is_some_and(|left_entries| {
                            left_entries.iter().any(|(_left_idx, matched)| !matched)
                        });

                        if !has_left_match {
                            // No matching rows in the left table were found; the left table column should be filled with NULL.
                            let mut null_left_row = Vec::new();
                            for _ in 0..left_dataset.col_names.len() {
                                null_left_row.push(Value::Null(crate::core::value::NullType::Null));
                            }

                            let mut joined_row = null_left_row;
                            joined_row.extend(right_row.iter().cloned());
                            result_dataset.rows.push(joined_row);
                        }
                    }
                }
            }
        }

        Ok(ExecutionResult::DataSet(result_dataset))
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for FullOuterJoinExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        self.execute_full_outer_join()
    }

    fn open(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.base.base.is_open()
    }

    fn id(&self) -> i64 {
        self.base.id()
    }

    fn name(&self) -> &str {
        self.base.name()
    }

    fn description(&self) -> &str {
        self.base.description()
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_base().get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_base_mut().get_stats_mut()
    }
}

impl<S: StorageClient + Send + 'static> crate::query::executor::base::HasStorage<S>
    for FullOuterJoinExecutor<S>
{
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base
            .get_base()
            .storage
            .as_ref()
            .expect("FullOuterJoinExecutor storage should be set")
    }
}
