//! The basic structure and common functions of the Join executor
//!
//! Provide the basic implementations for all join operations, including core functions such as hash table construction and detection.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::{Expression, Value};
use crate::query::executor::base::{BaseExecutor, ExecutionResult, JoinConfig, JoinConfigWithDesc};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::evaluation_context::row_context::RowExpressionContext;
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::relational_algebra::join::hash_table::JoinKey;
use crate::query::executor::relational_algebra::join::join_key_evaluator::JoinKeyEvaluator;
use crate::query::executor::relational_algebra::join::ExpressionContextStruct;
use crate::query::DataSet;
use crate::query::QueryError;
use crate::storage::StorageClient;

/// Probe result type alias
type ProbeResult = Result<Vec<(Vec<Value>, Vec<Vec<Value>>)>, QueryError>;

/// The basic structure of the Join executor
pub struct BaseJoinExecutor<S: StorageClient> {
    pub base: BaseExecutor<S>,
    /// Left input variable name
    left_var: String,
    /// Enter the variable name on the right.
    right_var: String,
    /// List of connection key expressions
    hash_keys: Vec<Expression>,
    /// List of detection key expressions
    probe_keys: Vec<Expression>,
    /// Column names
    col_names: Vec<String>,
    /// Description
    description: String,
    /// Should we swap the left and right inputs (for optimization purposes)?
    exchange: bool,
    /// Index of the output column on the right (used for natural joins)
    rhs_output_col_idxs: Option<Vec<usize>>,
    /// Cached single-key hash table (built once in open phase)
    cached_single_key_hash_table: Option<HashMap<Value, Vec<Vec<Value>>>>,
    /// Cached multi-key hash table (built once in open phase)
    cached_multi_key_hash_table: Option<HashMap<JoinKey, Vec<Vec<Value>>>>,
    /// Flag indicating whether hash table has been built
    is_hash_table_built: bool,
}

impl<S: StorageClient> BaseJoinExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionContextStruct>,
        config: JoinConfig,
    ) -> Self {
        Self::with_description(
            id,
            storage,
            expr_context,
            JoinConfigWithDesc {
                left_var: config.left_var,
                right_var: config.right_var,
                hash_keys: config.hash_keys,
                probe_keys: config.probe_keys,
                col_names: config.col_names,
                description: String::new(),
            },
        )
    }

    /// Initialize cache fields for a new instance
    fn init_cache() -> (Option<HashMap<Value, Vec<Vec<Value>>>>, Option<HashMap<JoinKey, Vec<Vec<Value>>>>, bool) {
        (None, None, false)
    }

    pub fn with_description(
        id: i64,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionContextStruct>,
        config: JoinConfigWithDesc,
    ) -> Self {
        let (cached_single_key, cached_multi_key, is_built) = Self::init_cache();
        Self {
            base: BaseExecutor::new(id, "BaseJoinExecutor".to_string(), storage, expr_context),
            left_var: config.left_var,
            right_var: config.right_var,
            hash_keys: config.hash_keys,
            probe_keys: config.probe_keys,
            col_names: config.col_names,
            description: config.description,
            exchange: false,
            rhs_output_col_idxs: None,
            cached_single_key_hash_table: cached_single_key,
            cached_multi_key_hash_table: cached_multi_key,
            is_hash_table_built: is_built,
        }
    }

    pub fn with_context(
        id: i64,
        storage: Arc<RwLock<S>>,
        context: crate::query::executor::base::ExecutionContext,
        config: JoinConfigWithDesc,
    ) -> Self {
        let (cached_single_key, cached_multi_key, is_built) = Self::init_cache();
        Self {
            base: BaseExecutor::with_context(id, "BaseJoinExecutor".to_string(), storage, context),
            left_var: config.left_var,
            right_var: config.right_var,
            hash_keys: config.hash_keys,
            probe_keys: config.probe_keys,
            col_names: config.col_names,
            description: config.description,
            exchange: false,
            rhs_output_col_idxs: None,
            cached_single_key_hash_table: cached_single_key,
            cached_multi_key_hash_table: cached_multi_key,
            is_hash_table_built: is_built,
        }
    }

    /// Check the input dataset (returns references, no cloning).
    pub fn check_input_datasets(&self) -> Result<(&DataSet, &DataSet), QueryError> {
        let left_result = self
            .base
            .context
            .get_result(&self.left_var)
            .ok_or_else(|| {
                QueryError::execution(format!("Left input variable not found: {}", self.left_var))
            })?;

        let right_result = self
            .base
            .context
            .get_result(&self.right_var)
            .ok_or_else(|| {
                QueryError::execution(format!(
                    "Right input variable not found: {}",
                    self.right_var
                ))
            })?;

        let left_dataset = match left_result {
            ExecutionResult::DataSet(dataset) => dataset,
            _ => {
                return Err(QueryError::execution(
                    "Left input must be a DataSet".to_string(),
                ))
            }
        };

        let right_dataset = match right_result {
            ExecutionResult::DataSet(dataset) => dataset,
            _ => {
                return Err(QueryError::execution(
                    "Right input must be a DataSet".to_string(),
                ))
            }
        };

        Ok((left_dataset, right_dataset))
    }

    /// Constructing a single-key hash table using JoinKeyEvaluator
    pub fn build_single_key_hash_table_with_evaluator<C: ExpressionContext>(
        &self,
        dataset: &DataSet,
        hash_key_expression: &Expression,
        _evaluator: &JoinKeyEvaluator,
        context: &mut C,
    ) -> Result<HashMap<Value, Vec<Vec<Value>>>, QueryError> {
        let mut hash_table = HashMap::new();

        for row in &dataset.rows {
            let key = JoinKeyEvaluator::evaluate_key(hash_key_expression, context)
                .map_err(|e| QueryError::execution(format!("Key evaluation failed: {}", e)))?;

            hash_table
                .entry(key)
                .or_insert_with(Vec::new)
                .push(row.clone());
        }

        Ok(hash_table)
    }

    /// Constructing a multi-key hash table using JoinKeyEvaluator
    pub fn build_multi_key_hash_table_with_evaluator<C: ExpressionContext>(
        &self,
        dataset: &DataSet,
        hash_key_exprs: &[Expression],
        _evaluator: &JoinKeyEvaluator,
        context: &mut C,
    ) -> Result<HashMap<JoinKey, Vec<Vec<Value>>>, QueryError> {
        let mut hash_table = HashMap::new();

        for row in &dataset.rows {
            let key_values = JoinKeyEvaluator::evaluate_keys(hash_key_exprs, context)
                .map_err(|e| QueryError::execution(format!("Key evaluation failed: {}", e)))?;

            let join_key = JoinKey::new(key_values);
            hash_table
                .entry(join_key)
                .or_insert_with(Vec::new)
                .push(row.clone());
        }

        Ok(hash_table)
    }

    /// Detecting a single-key hash table (using JoinKeyEvaluator)
    pub fn probe_single_key_hash_table_with_evaluator<C: ExpressionContext>(
        &self,
        probe_dataset: &DataSet,
        hash_table: &HashMap<Value, Vec<Vec<Value>>>,
        probe_key_expression: &Expression,
        _evaluator: &JoinKeyEvaluator,
        context: &mut C,
    ) -> ProbeResult {
        let mut results = Vec::new();

        for probe_row in &probe_dataset.rows {
            let key =
                JoinKeyEvaluator::evaluate_key(probe_key_expression, context).map_err(|e| {
                    QueryError::execution(format!("Probe key evaluation failed: {}", e))
                })?;

            if let Some(matching_rows) = hash_table.get(&key) {
                results.push((probe_row.clone(), matching_rows.clone()));
            }
        }

        Ok(results)
    }

    /// Detecting multi-key hash tables (using JoinKeyEvaluator)
    pub fn probe_multi_key_hash_table_with_evaluator<C: ExpressionContext>(
        &self,
        probe_dataset: &DataSet,
        hash_table: &HashMap<JoinKey, Vec<Vec<Value>>>,
        probe_key_exprs: &[Expression],
        _evaluator: &JoinKeyEvaluator,
        context: &mut C,
    ) -> ProbeResult {
        let mut results = Vec::new();

        for probe_row in &probe_dataset.rows {
            let key_values =
                JoinKeyEvaluator::evaluate_keys(probe_key_exprs, context).map_err(|e| {
                    QueryError::execution(format!("Probe key evaluation failed: {}", e))
                })?;

            let join_key = JoinKey::new(key_values);

            if let Some(matching_rows) = hash_table.get(&join_key) {
                results.push((probe_row.clone(), matching_rows.clone()));
            }
        }

        Ok(results)
    }

    /// Constructing a single-key hash table
    pub fn build_single_key_hash_table(
        hash_key: &str,
        dataset: &DataSet,
        hash_table: &mut HashMap<Value, Vec<Vec<Value>>>,
    ) -> Result<(), QueryError> {
        for row in &dataset.rows {
            let key_idx = hash_key
                .parse::<usize>()
                .map_err(|_| QueryError::execution("Invalid key index".to_string()))?;

            if key_idx < row.len() {
                let key = row[key_idx].clone();
                hash_table.entry(key).or_default().push(row.clone());
            }
        }
        Ok(())
    }

    /// Constructing a multi-key hash table
    pub fn build_multi_key_hash_table(
        hash_keys: &[String],
        dataset: &DataSet,
        hash_table: &mut HashMap<JoinKey, Vec<Vec<Value>>>,
    ) -> Result<(), QueryError> {
        for row in &dataset.rows {
            let mut key_values = Vec::new();
            for hash_key in hash_keys {
                let key_idx = hash_key
                    .parse::<usize>()
                    .map_err(|_| QueryError::execution("Invalid key index".to_string()))?;

                if key_idx < row.len() {
                    key_values.push(row[key_idx].clone());
                } else {
                    return Err(QueryError::execution("Key index out of range".to_string()));
                }
            }

            let join_key = JoinKey::new(key_values);
            hash_table.entry(join_key).or_default().push(row.clone());
        }
        Ok(())
    }

    /// Create a new row (by connecting the left and right rows, and selecting values based on the column names in the output).
    pub fn new_row(
        &self,
        left_row: Vec<Value>,
        right_row: Vec<Value>,
        left_col_names: &[String],
        right_col_names: &[String],
    ) -> Vec<Value> {
        let mut result = Vec::with_capacity(self.col_names.len());

        for col_name in &self.col_names {
            if let Some(idx) = left_col_names.iter().position(|c| c == col_name) {
                if let Some(val) = left_row.get(idx) {
                    result.push(val.clone());
                }
            } else if let Some(idx) = right_col_names.iter().position(|c| c == col_name) {
                if let Some(val) = right_row.get(idx) {
                    result.push(val.clone());
                }
            }
        }

        result
    }

    /// Decide whether to swap the left and right inputs in order to optimize performance.
    pub fn should_exchange(&self, left_size: usize, right_size: usize) -> bool {
        // If the left table is much larger than the right table, swap them to reduce the size of the hash table.
        left_size > right_size * 2
    }

    /// Optimize the swapping of left and right inputs.
    pub fn optimize_join_order(&mut self, left_dataset: &DataSet, right_dataset: &DataSet) {
        let left_size = left_dataset.rows.len();
        let right_size = right_dataset.rows.len();

        if self.should_exchange(left_size, right_size) {
            self.exchange = true;
        }
    }

    /// Calculate the index of the output column on the right (used for the natural join).
    pub fn calculate_rhs_output_col_idxs(
        &mut self,
        left_col_names: &[String],
        right_col_names: &[String],
    ) {
        let mut rhs_output_col_idxs = Vec::new();

        for (i, right_col) in right_col_names.iter().enumerate() {
            if !left_col_names.contains(right_col) {
                rhs_output_col_idxs.push(i);
            }
        }

        if !rhs_output_col_idxs.is_empty() && rhs_output_col_idxs.len() != right_col_names.len() {
            self.rhs_output_col_idxs = Some(rhs_output_col_idxs);
        }
    }

    /// Obtain column names
    pub fn get_col_names(&self) -> &Vec<String> {
        &self.col_names
    }

    /// Obtain the hash key
    pub fn get_hash_keys(&self) -> &Vec<Expression> {
        &self.hash_keys
    }

    /// Obtain the detection key
    pub fn get_probe_keys(&self) -> &Vec<Expression> {
        &self.probe_keys
    }

    /// Obtaining the basic executor
    pub fn get_base(&self) -> &BaseExecutor<S> {
        &self.base
    }

    /// Obtaining a variable basic executor
    pub fn get_base_mut(&mut self) -> &mut BaseExecutor<S> {
        &mut self.base
    }

    /// Obtain the executor ID.
    pub fn id(&self) -> i64 {
        self.base.id
    }

    /// Obtain the name of the executor.
    pub fn name(&self) -> &str {
        &self.base.name
    }

    /// Obtain a variable reference to the execution context
    pub fn context_mut(&mut self) -> &mut crate::query::executor::base::ExecutionContext {
        &mut self.base.context
    }

    /// Obtain the name of the variable from the left table.
    pub fn left_var(&self) -> &str {
        &self.left_var
    }

    /// Obtain the names of the variables from the right table.
    pub fn right_var(&self) -> &str {
        &self.right_var
    }

    /// Obtain a list of hash keys
    pub fn hash_keys(&self) -> &Vec<Expression> {
        &self.hash_keys
    }

    /// Obtain the list of detection keys.
    pub fn probe_keys(&self) -> &Vec<Expression> {
        &self.probe_keys
    }

    /// Obtain a list of column names
    pub fn col_names(&self) -> &Vec<String> {
        &self.col_names
    }

    /// Obtain the description.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Check whether the left and right inputs have been swapped.
    pub fn is_exchanged(&self) -> bool {
        self.exchange
    }

    /// Check if hash table has been built and cached.
    pub fn is_hash_table_built(&self) -> bool {
        self.is_hash_table_built
    }

    /// Build and cache single-key hash table for the given dataset.
    pub fn build_and_cache_single_key_hash_table(
        &mut self,
        dataset: &DataSet,
        hash_key_expression: &Expression,
        context: &mut ExpressionContextStruct,
    ) -> Result<(), QueryError> {
        if !self.is_hash_table_built {
            let mut hash_table = HashMap::new();

            for row in &dataset.rows {
                let mut row_context = RowExpressionContext::from_dataset(row, &dataset.col_names);
                let key = ExpressionEvaluator::evaluate(hash_key_expression, &mut row_context)
                    .map_err(|e| QueryError::execution(format!("Key evaluation failed: {}", e)))?;

                hash_table
                    .entry(key)
                    .or_insert_with(Vec::new)
                    .push(row.clone());
            }

            self.cached_single_key_hash_table = Some(hash_table);
            self.is_hash_table_built = true;
        }
        Ok(())
    }

    /// Build and cache multi-key hash table for the given dataset.
    pub fn build_and_cache_multi_key_hash_table(
        &mut self,
        dataset: &DataSet,
        hash_key_exprs: &[Expression],
        context: &mut ExpressionContextStruct,
    ) -> Result<(), QueryError> {
        if !self.is_hash_table_built {
            let mut hash_table = HashMap::new();

            for row in &dataset.rows {
                let mut row_context = RowExpressionContext::from_dataset(row, &dataset.col_names);
                let mut key_values = Vec::with_capacity(hash_key_exprs.len());

                for hash_key in hash_key_exprs {
                    let key = ExpressionEvaluator::evaluate(hash_key, &mut row_context)
                        .map_err(|e| QueryError::execution(format!("Key evaluation failed: {}", e)))?;
                    key_values.push(key);
                }

                let join_key = JoinKey::new(key_values);
                hash_table
                    .entry(join_key)
                    .or_insert_with(Vec::new)
                    .push(row.clone());
            }

            self.cached_multi_key_hash_table = Some(hash_table);
            self.is_hash_table_built = true;
        }
        Ok(())
    }

    /// Retrieve the cached single-key hash table (must be built first).
    pub fn get_cached_single_key_hash_table(
        &self,
    ) -> Option<&HashMap<Value, Vec<Vec<Value>>>> {
        self.cached_single_key_hash_table.as_ref()
    }

    /// Retrieve the cached multi-key hash table (must be built first).
    pub fn get_cached_multi_key_hash_table(
        &self,
    ) -> Option<&HashMap<JoinKey, Vec<Vec<Value>>>> {
        self.cached_multi_key_hash_table.as_ref()
    }

    /// Clear cached hash tables.
    pub fn clear_hash_table_cache(&mut self) {
        self.cached_single_key_hash_table = None;
        self.cached_multi_key_hash_table = None;
        self.is_hash_table_built = false;
    }
}

impl<S: StorageClient + Send + 'static> crate::query::executor::base::HasStorage<S>
    for BaseJoinExecutor<S>
{
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base
            .storage
            .as_ref()
            .expect("BaseJoinExecutor storage should be set")
    }
}
