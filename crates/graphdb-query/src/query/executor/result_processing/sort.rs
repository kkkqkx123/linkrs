//! Sorting Executor
//!
//! Provides high-performance sorting capabilities, supports sorting by multiple columns, and includes Top-N optimization features.
//!
//! CPU-intensive operations are parallelized using Rayon.

use parking_lot::RwLock;
use rayon::prelude::*;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::Expression;
use crate::core::Value;
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::InputExecutor;
use crate::query::executor::base::{BaseResultProcessor, ResultProcessor, ResultProcessorContext};
use crate::query::executor::base::{ExecutionResult, Executor, HasStorage};
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::{DefaultExpressionContext, ExpressionContext};
use crate::query::executor::utils::recursion_detector::ParallelConfig;
use crate::query::DataSet;
use crate::storage::StorageClient;

/// Sorting order enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}

/// Sorting key definition
#[derive(Debug, Clone)]
pub struct SortKey {
    pub expression: Expression,
    pub order: SortOrder,
    /// Optimized column index (if the expression can be parsed as a column index)
    pub column_index: Option<usize>,
}

impl SortKey {
    pub fn new(expression: Expression, order: SortOrder) -> Self {
        Self {
            expression,
            order,
            column_index: None,
        }
    }

    /// Create a sorting key based on a column index.
    pub fn from_column_index(column_index: usize, order: SortOrder) -> Self {
        Self {
            expression: Expression::Literal(Value::BigInt(column_index as i64)),
            order,
            column_index: Some(column_index),
        }
    }

    /// Check whether column indexing is used for sorting.
    pub fn uses_column_index(&self) -> bool {
        self.column_index.is_some()
    }
}

/// Sorting configuration
#[derive(Debug, Clone)]
pub struct SortConfig {
    /// Memory limit (in bytes), used for processing large datasets
    pub memory_limit: usize,
}

impl Default for SortConfig {
    fn default() -> Self {
        Self {
            memory_limit: 100 * 1024 * 1024, // Default memory limit of 100 MB
        }
    }
}

/// Optimized sorting executor
///
/// Refer to the implementation of SortExecutor in nebula-graph; it supports the Scatter-Gather parallel computing pattern.
pub struct SortExecutor<S: StorageClient + Send + 'static> {
    /// Basic processor
    base: BaseResultProcessor<S>,
    /// List of sorting keys
    sort_keys: Vec<SortKey>,
    /// Limit the quantity
    limit: Option<usize>,
    /// Input actuator
    input_executor: Option<Box<ExecutorEnum<S>>>,
    /// Sort Configuration
    config: SortConfig,
    /// Parallel computing configuration
    parallel_config: ParallelConfig,
}

impl<S: StorageClient + Send + 'static> SortExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        sort_keys: Vec<SortKey>,
        limit: Option<usize>,
        config: SortConfig,
    ) -> DBResult<Self> {
        let base = BaseResultProcessor::new(
            id,
            "SortExecutor".to_string(),
            "High-performance sorting".to_string(),
            storage,
        );

        Ok(Self {
            base,
            sort_keys,
            limit,
            input_executor: None,
            config,
            parallel_config: ParallelConfig::default(),
        })
    }

    /// Setting up parallel computing configuration
    pub fn with_parallel_config(mut self, config: ParallelConfig) -> Self {
        self.parallel_config = config;
        self
    }

    /// Process the input data and sort it.
    fn process_input(&mut self) -> DBResult<DataSet> {
        if let Some(ref mut input_exec) = self.input_executor {
            let input_result = input_exec.execute()?;

            match input_result {
                ExecutionResult::DataSet(mut data_set) => {
                    self.optimize_sort_keys(&data_set.col_names)?;
                    self.execute_sort(&mut data_set)?;
                    Ok(data_set)
                }
                _ => Err(DBError::query(
                    "Sort executor expects DataSet input".to_string(),
                )),
            }
        } else {
            Err(DBError::query(
                "Sort executor requires input executor".to_string(),
            ))
        }
    }

    /// Optimize the sorting keys and parse the expressions into column indices.
    fn optimize_sort_keys(&mut self, col_names: &[String]) -> DBResult<()> {
        // First, collect the expressions that need to be parsed.
        let mut expressions_to_parse = Vec::new();
        for (i, sort_key) in self.sort_keys.iter().enumerate() {
            if sort_key.column_index.is_none() {
                expressions_to_parse.push((i, sort_key.expression.clone()));
            }
        }

        // Parse the expression to obtain the column index.
        for (i, expression) in expressions_to_parse {
            if let Some(column_index) =
                self.parse_expression_to_column_index(&expression, col_names)?
            {
                self.sort_keys[i].column_index = Some(column_index);
            }
        }

        Ok(())
    }

    /// Parse the expression into column indices.
    fn parse_expression_to_column_index(
        &self,
        expression: &Expression,
        col_names: &[String],
    ) -> DBResult<Option<usize>> {
        match expression {
            Expression::Property {
                object: _,
                property,
            } => {
                // Find the column index corresponding to the attribute name.
                for (index, col_name) in col_names.iter().enumerate() {
                    if col_name == property {
                        return Ok(Some(index));
                    }
                }
                Ok(None)
            }
            Expression::Variable(name) => {
                // Find the column index corresponding to the variable name.
                for (index, col_name) in col_names.iter().enumerate() {
                    if col_name == name {
                        return Ok(Some(index));
                    }
                }
                Ok(None)
            }
            Expression::Literal(Value::Int(index)) => {
                // Use the column index directly.
                let idx = *index as usize;
                if idx < col_names.len() {
                    Ok(Some(idx))
                } else {
                    Ok(None)
                }
            }
            Expression::Literal(Value::BigInt(index)) => {
                // Use the column index directly.
                let idx = *index as usize;
                if idx < col_names.len() {
                    Ok(Some(idx))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None), // Other types of expressions are not currently supported for optimization.
        }
    }

    /// Implement a sorting algorithm
    ///
    /// Choose the sorting method based on the amount of data:
    /// - The amount of data is less than parallel_threshold: Single-threaded sorting
    /// - Large amount of data: Using the Scatter-Gather parallel sorting algorithm
    fn execute_sort(&mut self, data_set: &mut DataSet) -> DBResult<()> {
        if self.sort_keys.is_empty() || data_set.rows.is_empty() {
            return Ok(());
        }

        // Check whether the memory usage exceeds the limits.
        let estimated_memory = self.estimate_memory_usage(data_set);
        if estimated_memory > self.config.memory_limit {
            return Err(DBError::query(format!(
                "Sort operation memory usage limit exceeded: {} > {}",
                estimated_memory, self.config.memory_limit
            )));
        }

        let total_size = data_set.rows.len();

        // Determine whether to use parallel sorting based on the parallel configuration.
        if self.parallel_config.should_use_parallel(total_size) {
            return self.execute_parallel_sort(data_set);
        }

        // Check whether all sorting keys are using column indexes.
        let all_use_column_index = self.sort_keys.iter().all(|key| key.uses_column_index());

        if all_use_column_index {
            // Sort using column indexes
            return self.execute_column_index_sort(data_set);
        }

        // If there is a LIMIT and the amount of data is very large, the Top-N algorithm should be used.
        if let Some(limit) = self.limit {
            if data_set.rows.len() > limit * 10 {
                return self.execute_top_n_sort(data_set, limit);
            }
        }

        // Use the standard library to sort the data.
        self.execute_standard_sort(data_set)
    }

    /// Sort using column indexes
    fn execute_column_index_sort(&mut self, data_set: &mut DataSet) -> DBResult<()> {
        // Verify that all column indexes are within the valid range.
        for sort_key in &self.sort_keys {
            if let Some(column_index) = sort_key.column_index {
                if data_set.rows.iter().any(|row| column_index >= row.len()) {
                    return Err(DBError::query(format!(
                        "Column index out of range: {} (Maximum index: {})",
                        column_index,
                        data_set.rows[0].len() - 1
                    )));
                }
            }
        }

        // Using the standard library sorting functions directly provides the best performance.
        data_set.rows.sort_unstable_by(|a, b| {
            self.compare_by_column_indices(a, b)
                .unwrap_or(Ordering::Equal)
        });

        // Applying the “limit” function
        if let Some(limit) = self.limit {
            data_set.rows.truncate(limit);
        }

        Ok(())
    }

    /// Use the standard library to sort the data.
    fn execute_standard_sort(&mut self, data_set: &mut DataSet) -> DBResult<()> {
        // Use the standard library to sort the data.
        data_set
            .rows
            .sort_unstable_by(|a, b| self.compare_rows(a, b).unwrap_or(Ordering::Equal));

        // Applying the “limit” function
        if let Some(limit) = self.limit {
            data_set.rows.truncate(limit);
        }

        Ok(())
    }

    /// Comparing two data rows based on column indices
    fn compare_by_column_indices(&self, a: &[Value], b: &[Value]) -> DBResult<Ordering> {
        for sort_key in &self.sort_keys {
            if let Some(column_index) = sort_key.column_index {
                if column_index >= a.len() || column_index >= b.len() {
                    return Err(DBError::query(format!(
                        "Column index out of range: {} (Maximum index: {})",
                        column_index,
                        a.len().min(b.len()) - 1
                    )));
                }

                let a_val = &a[column_index];
                let b_val = &b[column_index];

                let cmp = a_val.partial_cmp(b_val).ok_or_else(|| {
                    DBError::query(format!(
                        "Value comparison failed with type mismatch: {:?} and {:?}",
                        a_val, b_val
                    ))
                })?;
                if cmp != Ordering::Equal {
                    return Ok(match sort_key.order {
                        SortOrder::Asc => cmp,
                        SortOrder::Desc => cmp.reverse(),
                    });
                }
            }
        }
        Ok(Ordering::Equal)
    }

    /// Estimating the memory usage of a dataset
    fn estimate_memory_usage(&self, data_set: &DataSet) -> usize {
        if data_set.rows.is_empty() {
            return 0;
        }

        // Estimate the memory usage per line.
        let sample_row = &data_set.rows[0];
        let mut row_size = std::mem::size_of::<Vec<Value>>();

        // Estimate the memory usage for each value.
        for value in sample_row {
            row_size += self.estimate_value_size(value);
        }

        // Estimating the memory usage of the sorting key
        let sort_key_size = self.sort_keys.len() * std::mem::size_of::<Value>();

        // Total memory usage = Number of rows × (Row size + Size of the sorting key)
        data_set.rows.len() * (row_size + sort_key_size)
    }

    /// Estimating the memory usage of a single value
    fn estimate_value_size(&self, value: &Value) -> usize {
        match value {
            Value::String(s) => std::mem::size_of::<String>() + s.len(),
            Value::Int(_) => std::mem::size_of::<i64>(),
            Value::Float(_) => std::mem::size_of::<f64>(),
            Value::Bool(_) => std::mem::size_of::<bool>(),
            Value::Null(_) => 0,
            Value::List(list) => {
                std::mem::size_of::<Vec<Value>>()
                    + list
                        .iter()
                        .map(|v| self.estimate_value_size(v))
                        .sum::<usize>()
            }
            Value::Map(map) => {
                std::mem::size_of::<std::collections::HashMap<String, Value>>()
                    + map
                        .iter()
                        .map(|(k, v)| k.len() + self.estimate_value_size(v))
                        .sum::<usize>()
            }
            _ => std::mem::size_of::<Value>(), // Default size
        }
    }

    /// Perform a Top-N sorting (using the select_nth_unstable optimization)
    ///
    /// Refer to the TopNExecutor implementation in nebula-graph; use heap sorting for optimization.
    /// 对于大数据集，select_nth_unstable的时间复杂度为O(n)，比完整排序O(n log n)更优
    fn execute_top_n_sort(&mut self, data_set: &mut DataSet, n: usize) -> DBResult<()> {
        if n == 0 || data_set.rows.is_empty() {
            return Ok(());
        }

        // If n is greater than or equal to the size of the dataset, simply sort the entire dataset.
        if n >= data_set.rows.len() {
            return self.execute_sort(data_set);
        }

        // Check whether all sorting keys are using column indexes.
        let all_use_column_index = self.sort_keys.iter().all(|key| key.uses_column_index());

        if all_use_column_index {
            // Performing a Top-N sorting using column indexes
            return self.execute_column_index_top_n_sort(data_set, n);
        }

        // Get the direction of all sorting keys
        let sort_orders: Vec<SortOrder> = self.sort_keys.iter().map(|key| key.order).collect();

        // Select the correct comparison logic based on the sorting direction.
        let is_ascending = sort_orders[0] == SortOrder::Asc;

        // Optimizing Top-N queries using select_nth_unstable
        // The `select_nth_unstable` function will place the first `n` elements on the left side, but the order of these elements is not guaranteed to be preserved.
        // For ascending order: The elements on the left are the first n smallest elements.
        // For descending order: The comparator needs to be reversed, so that the left side contains the first n largest elements.
        if is_ascending {
            // Ascending order: Select the first n smallest elements.
            let (left, _, _) = data_set.rows.select_nth_unstable_by(n, |a, b| {
                self.compare_rows(a, b).unwrap_or(Ordering::Equal)
            });
            left.sort_unstable_by(|a, b| self.compare_rows(a, b).unwrap_or(Ordering::Equal));
            data_set.rows.truncate(n);
        } else {
            // Descending order: Select the first n largest elements.
            // By using the reverse comparator, the function `select_nth_unstable` will place the largest `n` elements on the left side.
            let (left, _, _) = data_set.rows.select_nth_unstable_by(n, |a, b| {
                // Reverse the comparison result: Place the larger elements at the front.
                self.compare_rows(b, a).unwrap_or(Ordering::Equal)
            });
            // Sort the elements on the left side (the largest n elements) in descending order.
            left.sort_unstable_by(|a, b| self.compare_rows(a, b).unwrap_or(Ordering::Equal));
            data_set.rows.truncate(n);
        }

        Ok(())
    }

    /// Performing a Top-N sorting using column indexes
    fn execute_column_index_top_n_sort(
        &mut self,
        data_set: &mut DataSet,
        n: usize,
    ) -> DBResult<()> {
        // Obtain the direction of all sorting keys
        let sort_orders: Vec<SortOrder> = self.sort_keys.iter().map(|key| key.order).collect();
        let is_ascending = sort_orders[0] == SortOrder::Asc;

        if is_ascending {
            // Ascending order: Select the first n smallest elements.
            let (left, _, _) = data_set.rows.select_nth_unstable_by(n, |a, b| {
                self.compare_by_column_indices(a, b)
                    .unwrap_or(Ordering::Equal)
            });
            left.sort_unstable_by(|a, b| {
                self.compare_by_column_indices(a, b)
                    .unwrap_or(Ordering::Equal)
            });
            data_set.rows.truncate(n);
        } else {
            // Descending order: Select the first n largest elements.
            // By using the reverse comparator, the function `select_nth_unstable` will place the largest `n` elements on the left side.
            let (left, _, _) = data_set.rows.select_nth_unstable_by(n, |a, b| {
                // Reverse the comparison result: Place the larger elements at the front.
                self.compare_by_column_indices(b, a)
                    .unwrap_or(Ordering::Equal)
            });
            // Sort the elements on the left (the largest n elements) in descending order.
            left.sort_unstable_by(|a, b| {
                self.compare_by_column_indices(a, b)
                    .unwrap_or(Ordering::Equal)
            });
            data_set.rows.truncate(n);
        }

        Ok(())
    }

    /// Convert an Expression to a column name string for direct lookup
    fn expression_to_col_name(expr: &Expression) -> Option<String> {
        match expr {
            Expression::Property { object, property } => {
                if let Expression::Variable(var_name) = object.as_ref() {
                    Some(format!("{}.{}", var_name, property))
                } else {
                    None
                }
            }
            Expression::Variable(name) => Some(name.clone()),
            Expression::TagProperty { tag_name, property } => {
                Some(format!("{}.{}", tag_name, property))
            }
            Expression::EdgeProperty {
                edge_name,
                property,
            } => Some(format!("{}.{}", edge_name, property)),
            _ => None,
        }
    }

    /// The sorting key values for the calculation rows
    fn calculate_sort_values(&self, row: &[Value], col_names: &[String]) -> DBResult<Vec<Value>> {
        let mut sort_values = Vec::new();

        for sort_key in &self.sort_keys {
            // Handling special cases of sorting based on column indices
            if let Expression::Literal(Value::Int(index)) = &sort_key.expression {
                let idx = *index as usize;
                if idx < row.len() {
                    sort_values.push(row[idx].clone());
                } else {
                    return Err(DBError::query(format!(
                        "Column index {} out of range, row length:{}",
                        idx,
                        row.len()
                    )));
                }
            } else {
                // First try direct column name lookup
                let col_lookup = Self::expression_to_col_name(&sort_key.expression)
                    .and_then(|col_name| col_names.iter().position(|name| name == &col_name))
                    .filter(|&idx| idx < row.len())
                    .map(|idx| row[idx].clone());

                if let Some(value) = col_lookup {
                    sort_values.push(value);
                } else {
                    // Fall back to expression evaluator
                    let mut expr_context = DefaultExpressionContext::new();
                    for (i, col_name) in col_names.iter().enumerate() {
                        if i < row.len() {
                            expr_context.set_variable(col_name.clone(), row[i].clone());
                        }
                    }

                    let sort_value =
                        ExpressionEvaluator::evaluate(&sort_key.expression, &mut expr_context)
                            .map_err(|e| DBError::query(e.to_string()))?;
                    sort_values.push(sort_value);
                }
            }
        }

        Ok(sort_values)
    }

    /// Compare two vectors of sorted values
    fn compare_sort_items_vec(&self, a: &[Value], b: &[Value]) -> DBResult<Ordering> {
        for ((idx, sort_val_a), sort_val_b) in a.iter().enumerate().zip(b.iter()) {
            let comparison =
                self.compare_values(sort_val_a, sort_val_b, &self.sort_keys[idx].order)?;
            if !comparison.is_eq() {
                return Ok(comparison);
            }
        }
        Ok(Ordering::Equal)
    }

    /// Compare two values based on the sorting direction.
    fn compare_values(&self, a: &Value, b: &Value, order: &SortOrder) -> DBResult<Ordering> {
        let comparison = a.partial_cmp(b).ok_or_else(|| {
            DBError::query(format!(
                "Sorted value comparison failed with type mismatch: {:?} and {:?}",
                a, b
            ))
        })?;

        Ok(match order {
            SortOrder::Asc => comparison,
            SortOrder::Desc => comparison.reverse(),
        })
    }

    /// To compare two data rows, you can directly use a sorting key expression.
    /// This method has already correctly handled the sorting direction based on the `order` field of the `SortKey` internally.
    fn compare_rows(&self, a: &[Value], b: &[Value]) -> DBResult<Ordering> {
        // Create virtual column names (because the sorting key expression should be able to directly access the row data).
        let col_names: Vec<String> = (0..a.len()).map(|i| format!("col_{}", i)).collect();

        // Calculate the sorting value for each line.
        let sort_values_a = self.calculate_sort_values(a, &col_names)?;
        let sort_values_b = self.calculate_sort_values(b, &col_names)?;

        // Use the existing comparison logic.
        self.compare_sort_items_vec(&sort_values_a, &sort_values_b)
    }

    /// Parallel sorting
    ///
    /// Use the Scatter-Gather mode:
    /// ** Scatter:** Divides the data into multiple chunks, with each chunk being sorted in a separate thread.
    /// - Gather: Use the k-way merge sort to combine the sorted blocks.
    fn execute_parallel_sort(&mut self, data_set: &mut DataSet) -> DBResult<()> {
        let batch_size = self
            .parallel_config
            .calculate_batch_size(data_set.rows.len());
        let sort_keys = self.sort_keys.clone();

        // Check whether all sorting keys use column indexes (required for parallel sorting).
        let all_use_column_index = sort_keys.iter().all(|key| key.uses_column_index());

        // Split the data into multiple chunks.
        let chunks: Vec<Vec<Vec<Value>>> = data_set
            .rows
            .chunks(batch_size)
            .map(|c| c.to_vec())
            .collect();

        // Parallel sorting of each block
        let sorted_chunks: Vec<Vec<Vec<Value>>> = if all_use_column_index {
            // Sort using column indexes (faster).
            chunks
                .into_par_iter()
                .map(|mut chunk| {
                    chunk.par_sort_unstable_by(|a, b| {
                        for sort_key in &sort_keys {
                            if let Some(column_index) = sort_key.column_index {
                                if column_index < a.len() && column_index < b.len() {
                                    let a_val = &a[column_index];
                                    let b_val = &b[column_index];

                                    if let Some(cmp) = a_val.partial_cmp(b_val) {
                                        if cmp != Ordering::Equal {
                                            return match sort_key.order {
                                                SortOrder::Asc => cmp,
                                                SortOrder::Desc => cmp.reverse(),
                                            };
                                        }
                                    }
                                }
                            }
                        }
                        Ordering::Equal
                    });
                    chunk
                })
                .collect()
        } else {
            // Sort using the expression method (slower, as it requires the calculation of the sorting values).
            chunks
                .into_par_iter()
                .map(|chunk| {
                    let col_names: Vec<String> =
                        (0..chunk[0].len()).map(|i| format!("col_{}", i)).collect();

                    // Precomputing sort values
                    let mut rows_with_sort_values: Vec<(Vec<Value>, Vec<Value>)> = chunk
                        .into_iter()
                        .map(|row| {
                            let sort_values = self
                                .calculate_sort_values(&row, &col_names)
                                .unwrap_or_default();
                            (row, sort_values)
                        })
                        .collect();

                    // Sort according to the sorted values.
                    rows_with_sort_values.par_sort_unstable_by(|(_, a), (_, b)| {
                        for ((idx, sort_val_a), sort_val_b) in a.iter().enumerate().zip(b.iter()) {
                            if let Some(comparison) = sort_val_a.partial_cmp(sort_val_b) {
                                if comparison != Ordering::Equal {
                                    return match sort_keys[idx].order {
                                        SortOrder::Asc => comparison,
                                        SortOrder::Desc => comparison.reverse(),
                                    };
                                }
                            }
                        }
                        Ordering::Equal
                    });

                    rows_with_sort_values
                        .into_iter()
                        .map(|(row, _)| row)
                        .collect()
                })
                .collect()
        };

        // K-way merge
        data_set.rows = self.k_way_merge(sorted_chunks)?;

        // Applying the “limit” function
        if let Some(limit) = self.limit {
            data_set.rows.truncate(limit);
        }

        Ok(())
    }

    /// K-way merge
    ///
    /// Implement multi-way merge sorting using a heap, ensuring that the result is arranged in the order of the sorting keys.
    fn k_way_merge(&self, sorted_chunks: Vec<Vec<Vec<Value>>>) -> DBResult<Vec<Vec<Value>>> {
        if sorted_chunks.is_empty() {
            return Ok(Vec::new());
        }

        if sorted_chunks.len() == 1 {
            return Ok(sorted_chunks
                .into_iter()
                .next()
                .expect("sorted_chunks should contain exactly one element"));
        }

        // Check whether all sorting keys are using column indexes.
        let all_use_column_index = self.sort_keys.iter().all(|key| key.uses_column_index());

        // Implementing k-way merging using a priority queue
        struct HeapItem {
            row: Vec<Value>,
            chunk_idx: usize,
            row_idx: usize,
            sort_values: Vec<Value>, // Precomputed sorting values
        }

        impl Eq for HeapItem {}

        impl PartialEq for HeapItem {
            fn eq(&self, other: &Self) -> bool {
                self.chunk_idx == other.chunk_idx && self.row_idx == other.row_idx
            }
        }

        // In Rust, the `BinaryHeap` implementation represents a max-heap. However, we need a min-heap, so we need to reverse the outcome of the comparison operations.
        impl Ord for HeapItem {
            fn cmp(&self, other: &Self) -> Ordering {
                // First, compare the sorted values.
                match self.sort_values.partial_cmp(&other.sort_values) {
                    Some(Ordering::Equal) | None => {
                        // If the sort values are equal, use `chunk_idx` and `row_idx` as the basis for a stable sort.
                        other
                            .chunk_idx
                            .cmp(&self.chunk_idx)
                            .then_with(|| other.row_idx.cmp(&self.row_idx))
                    }
                    Some(ordering) => ordering.reverse(), // Reverse the order to implement a min-heap.
                }
            }
        }

        impl PartialOrd for HeapItem {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        let mut result = Vec::new();
        let mut chunk_iters: Vec<std::vec::IntoIter<Vec<Value>>> =
            sorted_chunks.into_iter().map(|c| c.into_iter()).collect();

        // Create virtual column names for calculating the sorting values.
        let col_names: Vec<String> = if !chunk_iters.is_empty() && all_use_column_index {
            // If a column index is used, the column names are not required.
            Vec::new()
        } else {
            // Get the length of the first row to determine the number of columns.
            let first_row_len = chunk_iters
                .iter()
                .filter_map(|iter| iter.as_slice().first().map(|row| row.len()))
                .next()
                .unwrap_or(0);
            (0..first_row_len).map(|i| format!("col_{}", i)).collect()
        };

        // Initialize the heap
        let mut heap = BinaryHeap::new();
        for (chunk_idx, iter) in chunk_iters.iter_mut().enumerate() {
            if let Some(row) = iter.next() {
                // Pre-compute the sorting values
                let sort_values = if all_use_column_index {
                    self.extract_column_sort_values(&row)?
                } else {
                    self.calculate_sort_values(&row, &col_names)?
                };

                heap.push(HeapItem {
                    row,
                    chunk_idx,
                    row_idx: 0,
                    sort_values,
                });
            }
        }

        // Merge
        while let Some(item) = heap.pop() {
            result.push(item.row);

            if let Some(next_row) = chunk_iters[item.chunk_idx].next() {
                // Pre calculating the sorting values for the next line
                let sort_values = if all_use_column_index {
                    self.extract_column_sort_values(&next_row)?
                } else {
                    self.calculate_sort_values(&next_row, &col_names)?
                };

                heap.push(HeapItem {
                    row: next_row,
                    chunk_idx: item.chunk_idx,
                    row_idx: item.row_idx + 1,
                    sort_values,
                });
            }
        }

        Ok(result)
    }

    /// Extract the sorting values corresponding to the column indices from the rows.
    fn extract_column_sort_values(&self, row: &[Value]) -> DBResult<Vec<Value>> {
        let mut sort_values = Vec::with_capacity(self.sort_keys.len());

        for sort_key in &self.sort_keys {
            if let Some(column_index) = sort_key.column_index {
                if column_index < row.len() {
                    sort_values.push(row[column_index].clone());
                } else {
                    return Err(DBError::query(format!(
                        "Column index out of range: {} (Row length: {})",
                        column_index,
                        row.len()
                    )));
                }
            } else {
                return Err(DBError::query(
                    "Extract_column_sort_values is not available for non-column index sort keys"
                        .to_string(),
                ));
            }
        }

        Ok(sort_values)
    }
}

impl<S: StorageClient + Send + 'static> ResultProcessor for SortExecutor<S> {
    fn process(&mut self, input: ExecutionResult) -> DBResult<ExecutionResult> {
        ResultProcessor::set_input(self, input);
        let dataset = self.process_input()?;
        Ok(ExecutionResult::DataSet(dataset))
    }

    fn set_input(&mut self, input: ExecutionResult) {
        self.base.input = Some(input);
    }

    fn get_input(&self) -> Option<&ExecutionResult> {
        self.base.input.as_ref()
    }

    fn context(&self) -> &ResultProcessorContext {
        &self.base.context
    }

    fn set_context(&mut self, context: ResultProcessorContext) {
        self.base.context = context;
    }

    fn memory_usage(&self) -> usize {
        self.base.memory_usage
    }

    fn reset(&mut self) {
        self.base.reset_state();
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for SortExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let input_result = if let Some(ref mut input_exec) = self.input_executor {
            input_exec.execute()?
        } else {
            self.base
                .input
                .clone()
                .unwrap_or(ExecutionResult::DataSet(DataSet::new()))
        };

        self.process(input_result)
    }

    fn open(&mut self) -> DBResult<()> {
        if let Some(ref mut input_exec) = self.input_executor {
            input_exec.open()?;
        }
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        if let Some(ref mut input_exec) = self.input_executor {
            input_exec.close()?;
        }
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.base.input.is_some()
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        &self.base.description
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for SortExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_deref()
    }
}

impl<S: StorageClient + Send + 'static> HasStorage<S> for SortExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        &self.base.storage
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;
    use crate::query::DataSet;
    use crate::storage::MockStorage;

    fn create_test_dataset() -> DataSet {
        let mut data_set = DataSet::new();
        data_set.col_names = vec!["name".to_string(), "age".to_string(), "score".to_string()];

        // Add test data
        data_set.rows = vec![
            vec![
                Value::String("Alice".to_string()),
                Value::Int(25),
                Value::Float(85.5),
            ],
            vec![
                Value::String("Bob".to_string()),
                Value::Int(30),
                Value::Float(92.0),
            ],
            vec![
                Value::String("Charlie".to_string()),
                Value::Int(22),
                Value::Float(78.5),
            ],
            vec![
                Value::String("David".to_string()),
                Value::Int(28),
                Value::Float(88.0),
            ],
            vec![
                Value::String("Eve".to_string()),
                Value::Int(26),
                Value::Float(95.5),
            ],
        ];

        data_set
    }

    #[test]
    fn test_sort_key_column_index() {
        // Testing the functionality of sorting key column indexes
        let sort_key = SortKey::new(Expression::Literal(Value::Int(1)), SortOrder::Asc);
        assert!(!sort_key.uses_column_index());

        // The test is based on the sorting key that uses column indexes.
        let column_index_sort_key = SortKey::from_column_index(1, SortOrder::Desc);
        assert!(column_index_sort_key.uses_column_index());
        assert_eq!(column_index_sort_key.column_index, Some(1));
    }

    #[test]
    fn test_column_index_sorting() {
        let mut data_set = create_test_dataset();

        // Sort using column indexes
        let sort_keys = vec![SortKey::from_column_index(2, SortOrder::Asc)]; // Sort in ascending order according to the score column.

        let config = SortConfig::default();

        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));

        let mut executor = SortExecutor::new(1, storage, sort_keys, None, config)
            .expect("SortExecutor::new should succeed");

        // Sort the content.
        executor
            .execute_sort(&mut data_set)
            .expect("execute_sort should succeed");

        // Verify the sorting result (ascending order: scores from low to high).
        assert_eq!(data_set.rows.len(), 5);
        assert_eq!(data_set.rows[0][2], Value::Float(78.5)); // Charlie (lowest score)
        assert_eq!(data_set.rows[1][2], Value::Float(85.5)); // Alice
        assert_eq!(data_set.rows[2][2], Value::Float(88.0)); // David
        assert_eq!(data_set.rows[3][2], Value::Float(92.0)); // Bob
        assert_eq!(data_set.rows[4][2], Value::Float(95.5)); // Eve (highest score)
    }

    #[test]
    fn test_top_n_sort() {
        let mut data_set = create_test_dataset();
        let sort_keys = vec![SortKey::from_column_index(2, SortOrder::Desc)]; // Sort in descending order according to the score column.

        let config = SortConfig::default();

        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));

        let mut executor = SortExecutor::new(1, storage, sort_keys, Some(3), config)
            .expect("SortExecutor::new should succeed");

        // Sort the content.
        executor
            .execute_sort(&mut data_set)
            .expect("execute_sort should succeed");

        // Verify the Top-N results
        assert_eq!(data_set.rows.len(), 3); // Verify that the Top-3 results are returned.
        assert_eq!(data_set.rows[0][2], Value::Float(95.5)); // Eve (highest score)
        assert_eq!(data_set.rows[1][2], Value::Float(92.0)); // Bob
        assert_eq!(data_set.rows[2][2], Value::Float(88.0)); // David (third highest score)
    }

    #[test]
    fn test_column_index_top_n_sort() {
        let mut data_set = create_test_dataset();

        // Sort using column indexes
        let sort_keys = vec![SortKey::from_column_index(2, SortOrder::Desc)]; // Sort in descending order by the score column.

        let config = SortConfig::default();

        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));

        let mut executor = SortExecutor::new(1, storage, sort_keys, Some(2), config)
            .expect("SortExecutor::new should succeed");

        // Sort the content.
        executor
            .execute_sort(&mut data_set)
            .expect("execute_sort should succeed");

        // Verify the Top-N results
        assert_eq!(data_set.rows.len(), 2); // Verify that the Top-2 results are returned.
        assert_eq!(data_set.rows[0][2], Value::Float(95.5)); // Eve (highest score)
        assert_eq!(data_set.rows[1][2], Value::Float(92.0)); // Bob (second-highest score)
    }

    #[test]
    fn test_multi_column_sorting() {
        let mut data_set = create_test_dataset();

        // Sorting by multiple columns: First, sort by age in ascending order, and then by score in descending order.
        let sort_keys = vec![
            SortKey::from_column_index(1, SortOrder::Asc), // Sort by age in ascending order.
            SortKey::from_column_index(2, SortOrder::Desc), // Sort the results in descending order based on the “score” value.
        ];

        let config = SortConfig::default();

        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));

        let mut executor = SortExecutor::new(1, storage, sort_keys, None, config)
            .expect("SortExecutor::new should succeed");
        executor
            .execute_sort(&mut data_set)
            .expect("execute_sort should succeed");

        // Verify the sorting results of multiple columns
        assert_eq!(data_set.rows.len(), 5);

        // Sort the first column: Age from lowest to highest.
        assert_eq!(data_set.rows[0][1], Value::Int(22)); // Charlie (the youngest)
        assert_eq!(data_set.rows[1][1], Value::Int(25)); // Alice
        assert_eq!(data_set.rows[2][1], Value::Int(26)); // Eve
        assert_eq!(data_set.rows[3][1], Value::Int(28)); // David
        assert_eq!(data_set.rows[4][1], Value::Int(30)); // Bob (the oldest)

        // Sort the rows of the same age in descending order based on the score.
        // There are no rows with the same age, so no additional verification is needed.
    }

    #[test]
    fn test_error_handling() {
        let mut data_set = create_test_dataset();

        // Testing invalid column indices
        let sort_keys = vec![SortKey::from_column_index(10, SortOrder::Asc)]; // Invalid column index

        let config = SortConfig::default();
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));

        let mut executor = SortExecutor::new(1, storage, sort_keys, None, config)
            .expect("SortExecutor::new should succeed");

        // Verifying the sorting results of multiple columns should return an error, because the column index is out of range.
        let result = executor.execute_sort(&mut data_set);
        assert!(result.is_err());

        // The verification error message contains column index information.
        let error = result.unwrap_err();
        assert!(
            format!("{:?}", error).contains("column index")
                || format!("{:?}", error).contains("Column index")
        );
    }

    #[test]
    fn test_compare_by_column_indices() {
        let data_set = create_test_dataset();
        let sort_keys = vec![SortKey::from_column_index(2, SortOrder::Asc)];

        let config = SortConfig::default();

        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));

        let executor = SortExecutor::new(1, storage, sort_keys, None, config)
            .expect("SortExecutor::new should succeed");

        // Testing the column index comparison function
        let row1 = &data_set.rows[0]; // Alice: 85.5
        let row2 = &data_set.rows[1]; // Bob: 92.0

        let result = executor
            .compare_by_column_indices(row1, row2)
            .expect("compare_by_column_indices should succeed");
        assert_eq!(result, Ordering::Less); // 85.5 < 92.0
    }
}
