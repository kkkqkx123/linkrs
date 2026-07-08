//! TopN Executor
//!
//! Implementing efficient TopN queries by optimizing performance using heap data structures
//! CPU-intensive operations are parallelized using Rayon.

use parking_lot::RwLock;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;

use rayon::prelude::*;

use crate::core::error::{DBError, DBResult};
use crate::core::types::OrderDirection;
use crate::core::Expression;
use crate::core::Value;
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::InputExecutor;
use crate::query::executor::base::{BaseResultProcessor, ResultProcessor, ResultProcessorContext};
use crate::query::executor::base::{ExecutionResult, Executor};
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::{DefaultExpressionContext, ExpressionContext};
use crate::query::executor::utils::recursion_detector::ParallelConfig;
use crate::query::DataSet;
use crate::storage::StorageClient;

/// Sorting column definition
#[derive(Debug, Clone)]
pub struct SortColumn {
    /// Column index
    pub column_index: usize,
    /// Data types
    pub data_type: crate::core::DataType,
    /// Does the NULL value appear at the beginning?
    pub nulls_first: bool,
}

impl SortColumn {
    pub fn new(column_index: usize, data_type: crate::core::DataType, nulls_first: bool) -> Self {
        Self {
            column_index,
            data_type,
            nulls_first,
        }
    }
}

/// Top N error types
#[derive(Debug, thiserror::Error)]
pub enum TopNError {
    #[error("Executor already open")]
    ExecutorAlreadyOpen,

    #[error("Memory limit exceeded")]
    MemoryLimitExceeded,

    #[error("Invalid column index: {0}")]
    InvalidColumnIndex(usize),

    #[error("Sort value extraction failed: {0}")]
    SortValueExtractionFailed(String),

    #[error("Heap operation failed: {0}")]
    HeapOperationFailed(String),

    #[error("Input executor error: {0}")]
    InputExecutorError(#[from] DBError),
}

/// TopNExecutor – The executor responsible for generating the top N results.
///
/// Return the first N sorted results; this is an optimized version of the “Sort + Limit” approach.
/// Implementing efficient TopN queries using the heap data structure
/// CPU-intensive operations are parallelized using Rayon.
pub struct TopNExecutor<S: StorageClient + Send + 'static> {
    /// Basic processor
    base: BaseResultProcessor<S>,
    /// Number of results returned
    n: usize,
    /// Offset
    offset: usize,
    /// List of sorting keys
    sort_keys: Vec<crate::query::executor::result_processing::sort::SortKey>,
    /// Input actuator
    input_executor: Option<Box<ExecutorEnum<S>>>,
    /// Sorting column definition
    sort_columns: Vec<SortColumn>,
    /// Sorting direction
    sort_direction: OrderDirection,
    /// Heap data structure (max heap or min heap)
    heap: Option<BinaryHeap<TopNItem>>,
    /// Has it been turned on?
    is_open: bool,
    /// Has it been turned off?
    is_closed: bool,
    /// Number of records processed
    processed_count: usize,
    /// Parallel computing configuration
    parallel_config: ParallelConfig,
}

impl<S: StorageClient> TopNExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        n: usize,
        sort_columns: Vec<String>,
        ascending: bool,
    ) -> Self {
        let base = BaseResultProcessor::new(
            id,
            "TopNExecutor".to_string(),
            "Returns the top N results using optimized heap algorithm".to_string(),
            storage,
        );

        // Convert the old sorting column format to the new sorting key format.
        let sort_keys = sort_columns
            .into_iter()
            .map(|col| {
                let order = if ascending {
                    crate::query::executor::result_processing::sort::SortOrder::Asc
                } else {
                    crate::query::executor::result_processing::sort::SortOrder::Desc
                };
                crate::query::executor::result_processing::sort::SortKey::new(
                    Expression::Variable(col),
                    order,
                )
            })
            .collect();

        Self {
            base,
            n,
            offset: 0,
            sort_keys,
            input_executor: None,
            sort_columns: Vec::new(),
            sort_direction: if ascending {
                OrderDirection::Asc
            } else {
                OrderDirection::Desc
            },
            heap: None,
            is_open: false,
            is_closed: false,
            processed_count: 0,
            parallel_config: ParallelConfig::default(),
        }
    }

    /// Create a TopN executor with the definition of sorted columns
    pub fn with_sort_columns(
        id: i64,
        storage: Arc<RwLock<S>>,
        n: usize,
        sort_columns: Vec<SortColumn>,
        sort_direction: OrderDirection,
    ) -> Self {
        let base = BaseResultProcessor::new(
            id,
            "TopNExecutor".to_string(),
            "Returns the top N results using optimized heap algorithm".to_string(),
            storage,
        );

        let sort_keys = sort_columns
            .iter()
            .map(|col| {
                let order = if sort_direction == OrderDirection::Asc {
                    crate::query::executor::result_processing::sort::SortOrder::Asc
                } else {
                    crate::query::executor::result_processing::sort::SortOrder::Desc
                };
                crate::query::executor::result_processing::sort::SortKey::new(
                    Expression::Variable(format!("col_{}", col.column_index)),
                    order,
                )
            })
            .collect();

        Self {
            base,
            n,
            offset: 0,
            sort_keys,
            input_executor: None,
            sort_columns,
            sort_direction,
            heap: None,
            is_open: false,
            is_closed: false,
            processed_count: 0,
            parallel_config: ParallelConfig::default(),
        }
    }

    /// Create a TopN executor with sort keys (supports expressions)
    pub fn with_sort_keys(
        id: i64,
        storage: Arc<RwLock<S>>,
        n: usize,
        sort_keys: Vec<crate::query::executor::result_processing::sort::SortKey>,
    ) -> Self {
        let base = BaseResultProcessor::new(
            id,
            "TopNExecutor".to_string(),
            "Returns the top N results using optimized heap algorithm".to_string(),
            storage,
        );

        let sort_direction = if sort_keys.first().map(|k| k.order)
            == Some(crate::query::executor::result_processing::sort::SortOrder::Asc)
        {
            OrderDirection::Asc
        } else {
            OrderDirection::Desc
        };

        Self {
            base,
            n,
            offset: 0,
            sort_keys,
            input_executor: None,
            sort_columns: Vec::new(),
            sort_direction,
            heap: None,
            is_open: false,
            is_closed: false,
            processed_count: 0,
            parallel_config: ParallelConfig::default(),
        }
    }

    /// Setting up parallel computing configuration
    pub fn with_parallel_config(mut self, config: ParallelConfig) -> Self {
        self.parallel_config = config;
        self
    }

    /// Set the offset value.
    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }

    /// Process the input data and perform the TopN operation.
    fn process_input(&mut self) -> DBResult<ExecutionResult> {
        if let Some(input) = self.base.input.take() {
            match input {
                ExecutionResult::DataSet(dataset) => {
                    let topn_result = self.execute_topn_dataset(dataset)?;
                    Ok(ExecutionResult::DataSet(topn_result))
                }
                ExecutionResult::Empty
                | ExecutionResult::Success
                | ExecutionResult::SpaceSwitched(_) => Ok(ExecutionResult::DataSet(DataSet::new())),
                ExecutionResult::Error(msg) => Err(DBError::query(msg)),
            }
        } else if let Some(ref mut input_exec) = self.input_executor {
            let input_result = input_exec.execute()?;

            match input_result {
                ExecutionResult::DataSet(dataset) => {
                    let topn_result = self.execute_topn_dataset(dataset)?;
                    Ok(ExecutionResult::DataSet(topn_result))
                }
                ExecutionResult::Empty
                | ExecutionResult::Success
                | ExecutionResult::SpaceSwitched(_) => Ok(ExecutionResult::DataSet(DataSet::new())),
                ExecutionResult::Error(msg) => Err(DBError::query(msg)),
            }
        } else {
            Err(DBError::query(
                "TopN executor requires input executor".to_string(),
            ))
        }
    }

    /// Performing a TopN operation on a dataset
    ///
    /// Select the execution method based on the amount of data:
    /// Data volume is below the threshold: Single-threaded heap sort
    /// Large amount of data: Rayon is used for parallel processing.
    fn execute_topn_dataset(&self, dataset: DataSet) -> DBResult<DataSet> {
        if self.sort_keys.is_empty() {
            return self.apply_limit_and_offset(dataset);
        }

        let total_size = dataset.rows.len();

        if self.parallel_config.should_use_parallel(total_size) {
            self.execute_topn_dataset_parallel(dataset)
        } else {
            self.execute_topn_dataset_sequential(dataset)
        }
    }

    /// TopN elements executed in order (using heap sorting)
    fn execute_topn_dataset_sequential(&self, mut dataset: DataSet) -> DBResult<DataSet> {
        if self.is_ascending() {
            self.heap_ascending(&mut dataset, self.n + self.offset)?;
        } else {
            self.heap_descending(&mut dataset, self.n + self.offset)?;
        }

        Ok(dataset)
    }

    /// Parallel execution of TopN (using Rayon)
    ///
    /// Use a two-stage strategy:
    /// Parallel computing of the sort key-value pairs for each row
    /// 2. Use Rayon partitioning for sorting, and then select N elements.
    fn execute_topn_dataset_parallel(&self, mut dataset: DataSet) -> DBResult<DataSet> {
        let _heap_size = self.n + self.offset;
        let sort_keys = self.sort_keys.clone();
        let col_names = dataset.col_names.clone();
        let is_ascending = self.is_ascending();

        let rows_with_values: Vec<(Vec<Value>, Vec<Value>)> = dataset
            .rows
            .into_par_iter()
            .map(|row| {
                let sort_value = Self::calculate_sort_value_parallel(&row, &col_names, &sort_keys);
                (sort_value, row)
            })
            .collect();

        let target_count = self.n + self.offset;

        if is_ascending {
            let mut items: Vec<TopNItemParallel> = rows_with_values
                .into_iter()
                .map(|(sort_value, row)| TopNItemParallel { sort_value, row })
                .collect();

            if items.len() > target_count {
                items.select_nth_unstable_by(target_count, |a, b| {
                    a.sort_value
                        .partial_cmp(&b.sort_value)
                        .unwrap_or(Ordering::Equal)
                });
                items.truncate(target_count);
            }

            items.sort_by(|a, b| {
                a.sort_value
                    .partial_cmp(&b.sort_value)
                    .unwrap_or(Ordering::Equal)
            });
            items.truncate(self.n);

            dataset.rows = items
                .into_iter()
                .skip(self.offset)
                .map(|item| item.row)
                .collect();
        } else {
            let mut items: Vec<TopNItemParallel> = rows_with_values
                .into_iter()
                .map(|(sort_value, row)| TopNItemParallel { sort_value, row })
                .collect();

            if items.len() > target_count {
                items.select_nth_unstable_by(target_count, |a, b| {
                    b.sort_value
                        .partial_cmp(&a.sort_value)
                        .unwrap_or(Ordering::Equal)
                });
                items.truncate(target_count);
            }

            items.sort_by(|a, b| {
                b.sort_value
                    .partial_cmp(&a.sort_value)
                    .unwrap_or(Ordering::Equal)
            });
            items.truncate(self.n);

            dataset.rows = items
                .into_iter()
                .skip(self.offset)
                .map(|item| item.row)
                .collect();
        }

        Ok(dataset)
    }

    /// Sorting of values in parallel computing rows
    fn calculate_sort_value_parallel(
        row: &[Value],
        col_names: &[String],
        sort_keys: &[crate::query::executor::result_processing::sort::SortKey],
    ) -> Vec<Value> {
        let mut context = DefaultExpressionContext::new();
        for (i, col_name) in col_names.iter().enumerate() {
            if i < row.len() {
                context.set_variable(col_name.clone(), row[i].clone());
            }
        }

        let mut sort_values = Vec::new();
        for sort_key in sort_keys {
            if let Some(column_index) = sort_key.column_index {
                if column_index < row.len() {
                    sort_values.push(row[column_index].clone());
                    continue;
                }
            }

            match ExpressionEvaluator::evaluate(&sort_key.expression, &mut context) {
                Ok(value) => sort_values.push(value),
                Err(_) => sort_values.push(Value::Null(crate::core::value::NullType::Null)),
            }
        }

        sort_values
    }

    /// Implementation of a heap for ascending order sorting
    fn heap_ascending(&self, dataset: &mut DataSet, heap_size: usize) -> DBResult<()> {
        let mut heap = BinaryHeap::with_capacity(heap_size);

        // Process all elements.
        for (i, row) in dataset.rows.iter().enumerate() {
            let sort_value = self.calculate_sort_value(row, &dataset.col_names)?;
            let new_item = TopNItem {
                sort_value,
                _original_index: i,
                row: row.to_vec(),
            };

            if heap.len() < heap_size {
                heap.push(new_item);
            } else {
                // For ascending order (selecting the smallest N elements): use a max heap.
                // If the new element is smaller than the top element of the heap (the top element of a max heap is always the largest element currently in the heap), then replace it.
                if let Some(peeked) = heap.peek() {
                    if new_item < *peeked {
                        heap.pop();
                        heap.push(new_item);
                    }
                }
            }
        }

        // Extract and sort the results (in ascending order).
        let mut items: Vec<TopNItem> = heap.into_iter().collect();
        items.sort_by(|a, b| a.sort_value.cmp(&b.sort_value));

        // Update the dataset
        dataset.rows = items
            .into_iter()
            .skip(self.offset)
            .map(|item| item.row)
            .collect();

        Ok(())
    }

    /// Implementation of a heap for descending order sorting
    fn heap_descending(&self, dataset: &mut DataSet, heap_size: usize) -> DBResult<()> {
        // For descending order (selecting the N largest elements): use a min-heap.
        // Implement the effect of a minimum heap using TopNItemDesc.
        let mut heap = BinaryHeap::with_capacity(heap_size);

        // Process all elements.
        for (i, row) in dataset.rows.iter().enumerate() {
            let sort_value = self.calculate_sort_value(row, &dataset.col_names)?;
            let new_item = TopNItemDesc {
                sort_value,
                _original_index: i,
                row: row.to_vec(),
            };

            if heap.len() < heap_size {
                heap.push(new_item);
            } else {
                // For the descending TopN (selecting the largest N elements):
                // Using a min-heap, if the new element is greater than the element at the top of the heap (the current smallest element), then replace it.
                if let Some(peeked) = heap.peek() {
                    // Since the comparison logic for TopNItemDesc is reverse,
                    // So, a less-than comparison should be used here, rather than a greater-than comparison.
                    if new_item < *peeked {
                        heap.pop();
                        heap.push(new_item);
                    }
                }
            }
        }

        // Extract and sort the results in descending order.
        let mut items: Vec<TopNItemDesc> = heap.into_iter().collect();
        items.sort_by(|a, b| b.sort_value.cmp(&a.sort_value));

        // Update the dataset
        dataset.rows = items
            .into_iter()
            .skip(self.offset)
            .map(|item| item.row)
            .collect();

        Ok(())
    }

    /// Calculate the sorting value for the row.
    fn calculate_sort_value(&self, row: &[Value], col_names: &[String]) -> DBResult<Vec<Value>> {
        let mut context = DefaultExpressionContext::new();
        for (i, col_name) in col_names.iter().enumerate() {
            if i < row.len() {
                context.set_variable(col_name.clone(), row[i].clone());
            }
        }

        let mut sort_values = Vec::new();
        for sort_key in &self.sort_keys {
            // First try direct column name lookup
            let col_lookup = Self::expression_to_col_name(&sort_key.expression)
                .and_then(|col_name| col_names.iter().position(|name| name == &col_name))
                .filter(|&idx| idx < row.len())
                .map(|idx| row[idx].clone());

            let value = if let Some(v) = col_lookup {
                v
            } else {
                ExpressionEvaluator::evaluate(&sort_key.expression, &mut context).map_err(|e| {
                    DBError::query(format!("Failed to evaluate sort expression: {}", e))
                })?
            };
            sort_values.push(value);
        }

        Ok(sort_values)
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

    /// Determine whether it is in ascending order.
    fn is_ascending(&self) -> bool {
        self.sort_keys
            .first()
            .map(|key| {
                matches!(
                    key.order,
                    crate::query::executor::result_processing::sort::SortOrder::Asc
                )
            })
            .unwrap_or(true)
    }

    /// Application restrictions and offsets
    fn apply_limit_and_offset(&self, mut dataset: DataSet) -> DBResult<DataSet> {
        // Application offset
        if self.offset > 0 {
            if self.offset < dataset.rows.len() {
                dataset.rows.drain(0..self.offset);
            } else {
                dataset.rows.clear();
            }
        }

        // Application restrictions
        dataset.rows.truncate(self.n);

        Ok(dataset)
    }

    /// Perform a TopN operation on the list of vertices.
    ///
    /// Sort the vertices using sorting keys; complex sorting based on attributes is also supported.
    /// Refer to the TopNExecutor implementation in nebula-graph; use heap sorting for optimization.
    fn _execute_topn_vertices(
        &self,
        vertices: Vec<crate::core::Vertex>,
    ) -> DBResult<Vec<crate::core::Vertex>> {
        if vertices.is_empty() || self.n == 0 {
            return Ok(Vec::new());
        }

        let total_size = vertices.len();
        let heap_size = self._calculate_heap_size(total_size);

        if heap_size == 0 {
            return Ok(Vec::new());
        }

        // Calculate maxCount: The number of elements that need to be retained in the end.
        let max_count = if total_size <= self.offset {
            0
        } else if total_size > self.offset + self.n {
            self.n
        } else {
            total_size - self.offset
        };

        if max_count == 0 {
            return Ok(Vec::new());
        }

        // Refer to the TopNExecutor implementation in nebula-graph.
        // 1. First, calculate the sorted values of all elements.
        let mut vertices_with_sort_values: Vec<(Vec<Value>, crate::core::Vertex)> = vertices
            .into_iter()
            .map(|vertex| {
                let sort_values = self._calculate_vertex_sort_values(&vertex)?;
                Ok((sort_values, vertex))
            })
            .collect::<DBResult<Vec<_>>>()?;

        // 2. Use the `select_nth_unstable` optimization to optimize TopN queries
        if vertices_with_sort_values.len() > heap_size {
            // Use `select_nth_unstable` to select the first `heap_size` elements.
            vertices_with_sort_values
                .select_nth_unstable_by(heap_size, |a, b| self._compare_sort_values(&a.0, &b.0));
            vertices_with_sort_values.truncate(heap_size);
        }

        // 3. Perform a complete sorting of the selected elements.
        vertices_with_sort_values.sort_by(|a, b| self._compare_sort_values(&a.0, &b.0));

        // 4. Apply the offset and limit parameters.
        let start = self.offset.min(vertices_with_sort_values.len());
        let end = (self.n + self.offset).min(vertices_with_sort_values.len());

        Ok(vertices_with_sort_values
            .into_iter()
            .skip(start)
            .take(end - start)
            .map(|(_, v)| v)
            .collect())
    }

    /// Perform a TopN operation on the list of opposite sides.
    ///
    /// Sort the edges using the sorting key; complex sorting based on attributes is also supported.
    /// Refer to the TopNExecutor implementation in nebula-graph, and optimize it using heap sorting.
    fn _execute_topn_edges(
        &self,
        edges: Vec<crate::core::Edge>,
    ) -> DBResult<Vec<crate::core::Edge>> {
        if edges.is_empty() || self.n == 0 {
            return Ok(Vec::new());
        }

        let total_size = edges.len();
        let heap_size = self._calculate_heap_size(total_size);

        if heap_size == 0 {
            return Ok(Vec::new());
        }

        // Calculate maxCount: The number of elements that ultimately need to be retained.
        let max_count = if total_size <= self.offset {
            0
        } else if total_size > self.offset + self.n {
            self.n
        } else {
            total_size - self.offset
        };

        if max_count == 0 {
            return Ok(Vec::new());
        }

        // Refer to the TopNExecutor implementation in nebula-graph.
        // 1. First, calculate the sorted values of all elements.
        let mut edges_with_sort_values: Vec<(Vec<Value>, crate::core::Edge)> = edges
            .into_iter()
            .map(|edge| {
                let sort_values = self._calculate_edge_sort_values(&edge)?;
                Ok((sort_values, edge))
            })
            .collect::<DBResult<Vec<_>>>()?;

        // 2. Use the `select_nth_unstable` optimization to optimize TopN queries
        if edges_with_sort_values.len() > heap_size {
            edges_with_sort_values
                .select_nth_unstable_by(heap_size, |a, b| self._compare_sort_values(&a.0, &b.0));
            edges_with_sort_values.truncate(heap_size);
        }

        // 3. Perform a complete sorting of the selected elements.
        edges_with_sort_values.sort_by(|a, b| self._compare_sort_values(&a.0, &b.0));

        // 4. Apply the offset and limit parameters.
        let start = self.offset.min(edges_with_sort_values.len());
        let end = (self.n + self.offset).min(edges_with_sort_values.len());

        Ok(edges_with_sort_values
            .into_iter()
            .skip(start)
            .take(end - start)
            .map(|(_, e)| e)
            .collect())
    }

    /// Perform a TopN operation on a list of values.
    ///
    /// Sort a list of key-value pairs using a sorting key.
    fn _execute_topn_values(&self, values: Vec<Value>) -> DBResult<Vec<Value>> {
        if values.is_empty() || self.n == 0 {
            return Ok(Vec::new());
        }

        let total_size = values.len();
        let heap_size = self._calculate_heap_size(total_size);

        if heap_size == 0 {
            return Ok(Vec::new());
        }

        // Wrap the “Value” data into a single-line format to allow for the reuse of the sorting logic.
        let mut rows: Vec<Vec<Value>> = values.into_iter().map(|v| vec![v]).collect();

        // Implementing TopN using Heap Sort
        let heap_size = self.n + self.offset;

        if rows.len() <= heap_size {
            rows.sort_by(|a, b| self._compare_rows(a, b).unwrap_or(Ordering::Equal));
        } else {
            rows.select_nth_unstable_by(heap_size, |a, b| {
                self._compare_rows(a, b).unwrap_or(Ordering::Equal)
            });
            rows.truncate(heap_size);
            rows.sort_by(|a, b| self._compare_rows(a, b).unwrap_or(Ordering::Equal));
        }

        // Apply the `offset` and `limit` parameters.
        let start = self.offset.min(rows.len());
        let end = (self.n + self.offset).min(rows.len());

        Ok(rows
            .into_iter()
            .skip(start)
            .take(end - start)
            .map(|row| row.into_iter().next().expect("row should not be empty"))
            .collect())
    }

    /// Calculate the size of the heap
    fn _calculate_heap_size(&self, total_size: usize) -> usize {
        if total_size <= self.offset {
            0
        } else if total_size > self.offset + self.n {
            self.offset + self.n
        } else {
            total_size
        }
    }

    /// Calculate the sorting values of the vertices
    fn _calculate_vertex_sort_values(&self, vertex: &crate::core::Vertex) -> DBResult<Vec<Value>> {
        let mut sort_values = Vec::with_capacity(self.sort_keys.len());

        for sort_key in &self.sort_keys {
            let value = self._extract_value_from_vertex(vertex, &sort_key.expression)?;
            sort_values.push(value);
        }

        Ok(sort_values)
    }

    /// Calculate the sorting value of the edges
    fn _calculate_edge_sort_values(&self, edge: &crate::core::Edge) -> DBResult<Vec<Value>> {
        let mut sort_values = Vec::with_capacity(self.sort_keys.len());

        for sort_key in &self.sort_keys {
            let value = self._extract_value_from_edge(edge, &sort_key.expression)?;
            sort_values.push(value);
        }

        Ok(sort_values)
    }

    /// Extract values from the vertices.
    fn _extract_value_from_vertex(
        &self,
        vertex: &crate::core::Vertex,
        expression: &Expression,
    ) -> DBResult<Value> {
        match expression {
            Expression::Variable(name) => {
                if let Some(value) = vertex.get_property_any(name) {
                    Ok(value.clone())
                } else if name == "vid" || name == "_vid" {
                    Ok(Value::from(vertex.vid))
                } else if name == "id" || name == "_id" {
                    Ok(Value::BigInt(vertex.id))
                } else {
                    Ok(Value::Null(crate::core::value::NullType::Null))
                }
            }
            Expression::Property { object, property } => {
                if object.as_ref() == &Expression::Variable("v".to_string())
                    || object.as_ref() == &Expression::Variable("vertex".to_string())
                {
                    if let Some(value) = vertex.get_property_any(property) {
                        Ok(value.clone())
                    } else {
                        Ok(Value::Null(crate::core::value::NullType::Null))
                    }
                } else {
                    Ok(Value::Null(crate::core::value::NullType::Null))
                }
            }
            Expression::Literal(value) => Ok(value.clone()),
            _ => {
                let mut context = DefaultExpressionContext::new();
                context.set_variable("vid".to_string(), Value::from(vertex.vid));
                context.set_variable("id".to_string(), Value::BigInt(vertex.id));

                for tag in &vertex.tags {
                    for (prop_name, prop_value) in &tag.properties {
                        context.set_variable(prop_name.clone(), prop_value.clone());
                    }
                }

                ExpressionEvaluator::evaluate(expression, &mut context)
                    .map_err(|e| DBError::query(e.to_string()))
            }
        }
    }

    /// Extract values from the edges.
    fn _extract_value_from_edge(
        &self,
        edge: &crate::core::Edge,
        expression: &Expression,
    ) -> DBResult<Value> {
        match expression {
            Expression::Variable(name) => {
                if name == "src" || name == "_src" {
                    Ok(Value::from(edge.src))
                } else if name == "dst" || name == "_dst" {
                    Ok(Value::from(edge.dst))
                } else if name == "ranking" || name == "_ranking" {
                    Ok(Value::BigInt(edge.ranking))
                } else if name == "edge_type" || name == "_type" {
                    Ok(Value::String(edge.edge_type.clone()))
                } else if let Some(value) = edge.get_property(name) {
                    Ok(value.clone())
                } else {
                    Ok(Value::Null(crate::core::value::NullType::Null))
                }
            }
            Expression::Property { object, property } => {
                if object.as_ref() == &Expression::Variable("e".to_string())
                    || object.as_ref() == &Expression::Variable("edge".to_string())
                {
                    if let Some(value) = edge.get_property(property) {
                        Ok(value.clone())
                    } else {
                        Ok(Value::Null(crate::core::value::NullType::Null))
                    }
                } else {
                    Ok(Value::Null(crate::core::value::NullType::Null))
                }
            }
            Expression::Literal(value) => Ok(value.clone()),
            _ => {
                // For complex expressions, use an expression evaluator.
                let mut context = DefaultExpressionContext::new();
                context.set_variable("src".to_string(), Value::from(edge.src));
                context.set_variable("dst".to_string(), Value::from(edge.dst));
                context.set_variable("ranking".to_string(), Value::BigInt(edge.ranking));
                context.set_variable(
                    "edge_type".to_string(),
                    Value::String(edge.edge_type.clone()),
                );

                // Add all attributes to the context.
                for (prop_name, prop_value) in &edge.props {
                    context.set_variable(prop_name.clone(), prop_value.clone());
                }

                ExpressionEvaluator::evaluate(expression, &mut context)
                    .map_err(|e| DBError::query(e.to_string()))
            }
        }
    }

    /// Compare the sorted values
    fn _compare_sort_values(&self, a: &[Value], b: &[Value]) -> Ordering {
        for (idx, (val_a, val_b)) in a.iter().zip(b.iter()).enumerate() {
            let order = if idx < self.sort_keys.len() {
                &self.sort_keys[idx].order
            } else {
                &crate::query::executor::result_processing::sort::SortOrder::Asc
            };

            let comparison = match val_a.partial_cmp(val_b) {
                Some(cmp) => cmp,
                None => continue,
            };

            if comparison != Ordering::Equal {
                return match order {
                    crate::query::executor::result_processing::sort::SortOrder::Asc => comparison,
                    crate::query::executor::result_processing::sort::SortOrder::Desc => {
                        comparison.reverse()
                    }
                };
            }
        }
        Ordering::Equal
    }

    /// Compare two rows of data (used for sorting value lists)
    fn _compare_rows(&self, a: &[Value], b: &[Value]) -> DBResult<Ordering> {
        // Create virtual column names
        let col_names: Vec<String> = (0..a.len().max(b.len()))
            .map(|i| format!("col_{}", i))
            .collect();

        // Calculate the sorted values
        let sort_values_a = self.calculate_sort_value(a, &col_names)?;
        let sort_values_b = self.calculate_sort_value(b, &col_names)?;

        Ok(self._compare_sort_values(&sort_values_a, &sort_values_b))
    }

    /// Obtain the heap size
    pub fn get_heap_size(&self) -> usize {
        self.heap.as_ref().map_or(0, |h| h.len())
    }

    /// Obtain the number of processed records.
    pub fn get_processed_count(&self) -> usize {
        self.processed_count
    }

    /// Configure sorting parameters
    pub fn configure_sorting(
        &mut self,
        sort_columns: Vec<SortColumn>,
        sort_direction: OrderDirection,
    ) -> Result<(), TopNError> {
        if self.is_open {
            return Err(TopNError::ExecutorAlreadyOpen);
        }
        self.sort_columns = sort_columns;
        self.sort_direction = sort_direction;
        Ok(())
    }

    /// Push into the heap
    pub fn push_to_heap(&mut self, item: TopNItem) -> Result<(), TopNError> {
        let heap = self
            .heap
            .get_or_insert_with(|| BinaryHeap::with_capacity(self.n + 1));

        heap.push(item);

        if heap.len() > self.n {
            heap.pop();
        }

        Ok(())
    }

    /// Pop from the heap
    pub fn pop_from_heap(&mut self) -> Option<TopNItem> {
        self.heap.as_mut()?.pop()
    }
}

/// TopN heap items
#[derive(Debug, Clone)]
pub struct TopNItem {
    sort_value: Vec<Value>,
    _original_index: usize,
    row: Vec<Value>,
}

impl PartialEq for TopNItem {
    fn eq(&self, other: &Self) -> bool {
        self.sort_value == other.sort_value
    }
}

impl Eq for TopNItem {}

impl PartialOrd for TopNItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TopNItem {
    fn cmp(&self, other: &Self) -> Ordering {
        // In a normal comparison, BinaryHeap is a max heap.
        self.sort_value.cmp(&other.sort_value)
    }
}

/// Heap items used for descending sorting (to implement a minimum heap)
#[derive(Debug, Clone)]
struct TopNItemDesc {
    sort_value: Vec<Value>,
    _original_index: usize,
    row: Vec<Value>,
}

impl PartialEq for TopNItemDesc {
    fn eq(&self, other: &Self) -> bool {
        self.sort_value == other.sort_value
    }
}

impl Eq for TopNItemDesc {}

impl PartialOrd for TopNItemDesc {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TopNItemDesc {
    fn cmp(&self, other: &Self) -> Ordering {
        other.sort_value.cmp(&self.sort_value)
    }
}

/// Items used by parallel TopN (partial_cmp is supported)
#[derive(Debug, Clone)]
struct TopNItemParallel {
    sort_value: Vec<Value>,
    row: Vec<Value>,
}

impl PartialEq for TopNItemParallel {
    fn eq(&self, other: &Self) -> bool {
        self.sort_value == other.sort_value
    }
}

impl Eq for TopNItemParallel {}

impl PartialOrd for TopNItemParallel {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TopNItemParallel {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sort_value.cmp(&other.sort_value)
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for TopNExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_deref()
    }
}

impl<S: StorageClient + Send + 'static> ResultProcessor for TopNExecutor<S> {
    fn process(&mut self, input: ExecutionResult) -> DBResult<ExecutionResult> {
        self.base.input = Some(input.clone());
        self.process_input()
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

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for TopNExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let input_result = if let Some(ref mut input_exec) = self.input_executor {
            input_exec.execute()?
        } else {
            self.base
                .input
                .clone()
                .unwrap_or(ExecutionResult::DataSet(DataSet::new()))
        };

        match input_result {
            ExecutionResult::DataSet(dataset) => {
                let topn_result = self.execute_topn_dataset(dataset)?;
                Ok(ExecutionResult::DataSet(topn_result))
            }
            _ => Err(DBError::query(
                "TopN executor expects DataSet input type".to_string(),
            )),
        }
    }

    fn open(&mut self) -> DBResult<()> {
        if self.is_open {
            return Err(DBError::query("Executor already open".to_string()));
        }

        if self.input_executor.is_none() {
            return Err(DBError::query("Missing input executor".to_string()));
        }

        if let Some(ref mut input_exec) = self.input_executor {
            input_exec.open()?;
        }

        self.is_open = true;
        self.is_closed = false;
        self.heap = Some(BinaryHeap::with_capacity(self.n + 1));
        self.processed_count = 0;
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        if !self.is_open || self.is_closed {
            return Ok(());
        }

        if let Some(ref mut input_exec) = self.input_executor {
            input_exec.close()?;
        }

        self.heap = None;
        self.is_closed = true;
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.base.id > 0
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

impl<S: StorageClient + Send + Sync + 'static> TopNExecutor<S> {
    pub fn execute_with_recovery(&mut self) -> DBResult<ExecutionResult> {
        match self.execute() {
            Ok(result) => Ok(result),
            Err(ref err) if err.message().contains("memory") || err.message().contains("limit") => {
                self.fallback_to_external_sort()
            }
            Err(e) => Err(e),
        }
    }

    fn fallback_to_external_sort(&mut self) -> DBResult<ExecutionResult> {
        Err(DBError::query(
            "Memory limit exceeded, consider reducing the dataset size or N value".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MockStorage;

    #[test]
    fn test_topn_executor_basic() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));

        // Create test data
        let mut dataset = DataSet::new();
        dataset.col_names = vec!["name".to_string(), "score".to_string()];
        for i in 1..=10 {
            dataset.rows.push(vec![
                Value::String(format!("User{}", i)),
                Value::Int(i * 10),
            ]);
        }

        // Create a TopN executor (to retrieve the top 3 items, sorted in descending order by score)
        let mut executor = TopNExecutor::new(1, storage, 3, vec!["score".to_string()], false);

        // Perform TopN
        let result = executor
            .process(ExecutionResult::DataSet(dataset))
            .expect("TopN executor should process successfully");

        // Verification results
        match result {
            ExecutionResult::DataSet(topn_dataset) => {
                assert_eq!(topn_dataset.rows.len(), 3);
                // Verify that the list is sorted in descending order by score.
                assert_eq!(topn_dataset.rows[0][1], Value::Int(100)); // User10
                assert_eq!(topn_dataset.rows[1][1], Value::Int(90)); // User9
                assert_eq!(topn_dataset.rows[2][1], Value::Int(80)); // User8
            }
            _ => panic!("Expected DataSet result"),
        }
    }

    #[test]
    fn test_topn_executor_with_offset() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));

        let values: Vec<Value> = (1..=10).map(Value::Int).collect();

        let input_dataset = DataSet::from_rows(
            values.iter().map(|v| vec![v.clone()]).collect(),
            vec!["value".to_string()],
        );

        let mut executor =
            TopNExecutor::new(1, storage, 3, vec!["value".to_string()], true).with_offset(2);

        let result = executor
            .process(ExecutionResult::DataSet(input_dataset))
            .expect("TopN executor should process successfully");

        match result {
            ExecutionResult::DataSet(topn_dataset) => {
                assert_eq!(topn_dataset.rows.len(), 3);
                assert_eq!(topn_dataset.rows[0][0], Value::Int(3));
                assert_eq!(topn_dataset.rows[1][0], Value::Int(4));
                assert_eq!(topn_dataset.rows[2][0], Value::Int(5));
            }
            _ => panic!("Expected DataSet result"),
        }
    }
}
