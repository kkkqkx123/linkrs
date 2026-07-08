//! StreamingExecutor: Enum-based pull executor with modular operator implementation
//!
//! This file contains:
//! - StreamingExecutor enum definition (all 15 operator variants)
//! - SortDirection enum
//! - Coordination methods (open, next, stop, close) that dispatch to operator modules
//!
//! Operator implementations are in submodules:
//! - context - Expression evaluation context
//! - operators/ - Operator implementations (sources, single_input, stateful, binary, set_ops)
//! - helpers/ - Helper functions (comparison, aggregation, conversion)

use super::chunk::DataChunk;
use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::core::Value;

pub mod context;
pub mod helpers;
pub mod operators;

pub use context::ValueRowContext;
pub use helpers::{aggregation, comparison, conversion};

/// Sort direction for ORDER BY clause
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// Pull-based streaming executor
///
/// Each variant handles different operation types:
/// - Data sources: ScanVertices, ScanEdges
/// - Single input: Filter, Project, Limit, Distinct
/// - Stateful: Aggregate, Sort, GroupBy, WindowFunction
/// - Binary input: HashJoin, NestedLoopJoin
/// - Set operations: Union, UnionAll, Intersect, Except
#[derive(Debug)]
pub enum StreamingExecutor {
    // ============ Data Sources ============
    /// Scan vertices from a partition
    /// Input data is pre-loaded into buffer (from storage layer)
    ScanVertices {
        partition_id: usize,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
    },

    /// Scan edges from a partition
    /// Input data is pre-loaded into buffer (from storage layer)
    ScanEdges {
        partition_id: usize,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
    },

    // ============ Single Input ============
    /// Filter executor with expression-based predicates
    Filter {
        input: Box<StreamingExecutor>,
        predicate: Expression,
        opened: bool,
    },

    /// Project executor with expression-based column selection
    Project {
        input: Box<StreamingExecutor>,
        output_expressions: Vec<Expression>,
        opened: bool,
    },

    /// Limit executor
    Limit {
        input: Box<StreamingExecutor>,
        limit: u32,
        consumed: u32,
        opened: bool,
    },

    // ============ Stateful ============
    /// Aggregate executor with GROUP BY and aggregate functions
    Aggregate {
        input: Box<StreamingExecutor>,
        /// GROUP BY expressions to compute group keys
        group_by_expressions: Vec<Expression>,
        /// (AggregateFunction, field_expression) pairs for aggregation
        aggregate_functions: Vec<(AggregateFunction, Expression)>,
        /// Buffer for collecting all input rows before aggregation
        all_rows: Vec<Vec<Value>>,
        /// Iterator for result chunks
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
    },

    /// Sort executor with ORDER BY support
    Sort {
        input: Box<StreamingExecutor>,
        /// ORDER BY expressions
        sort_expressions: Vec<Expression>,
        /// Sort direction for each expression
        sort_directions: Vec<SortDirection>,
        all_rows: Vec<Vec<Value>>,
        row_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
    },

    // ============ Binary Input ============
    /// HashJoin executor with join condition support
    HashJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        /// Join condition expression
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
        opened: bool,
    },

    /// GroupBy executor for independent grouping before aggregation
    GroupBy {
        input: Box<StreamingExecutor>,
        /// GROUP BY expressions to compute group keys
        group_by_expressions: Vec<Expression>,
        /// Buffer for collecting all input rows before grouping
        all_rows: Vec<Vec<Value>>,
        /// Iterator for result chunks
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
    },

    /// Distinct executor to eliminate duplicate rows
    Distinct {
        input: Box<StreamingExecutor>,
        /// Set of already-seen rows (as serialized strings)
        seen_rows: std::collections::HashSet<String>,
        opened: bool,
    },

    /// NestedLoopJoin for theta-joins and non-equi joins
    NestedLoopJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        /// Join condition expression (can be any comparison)
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
        opened: bool,
    },

    /// WindowFunction executor for analytic functions
    /// Buffers input by PARTITION BY clause, computes window functions
    WindowFunction {
        input: Box<StreamingExecutor>,
        /// Window function expressions
        window_exprs: Vec<Expression>,
        /// PARTITION BY expressions (empty means all rows in one partition)
        partition_by_exprs: Vec<Expression>,
        /// ORDER BY expressions
        order_by_exprs: Vec<Expression>,
        /// Sort directions for ORDER BY
        order_by_directions: Vec<SortDirection>,
        /// All input rows buffered
        all_rows: Vec<Vec<Value>>,
        /// Iterator for result chunks
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        opened: bool,
    },

    /// Set Union operation (combines all rows from left and right)
    Union {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        /// Already-seen rows to eliminate duplicates
        seen_rows: std::collections::HashSet<String>,
        left_consumed: bool,
        opened: bool,
    },

    /// Set UnionAll operation (combines all rows without deduplication)
    UnionAll {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        left_consumed: bool,
        opened: bool,
    },

    /// Set Intersect operation (returns rows present in both inputs)
    Intersect {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        /// Rows from left side
        left_rows: std::collections::HashSet<String>,
        /// Rows from right side
        right_rows: std::collections::HashSet<String>,
        left_buffered: bool,
        right_buffered: bool,
        opened: bool,
    },

    /// Set Except/Minus operation (returns rows from left not in right)
    Except {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        /// Rows to exclude (from right side)
        exclude_rows: std::collections::HashSet<String>,
        right_buffered: bool,
        opened: bool,
    },
}

impl StreamingExecutor {
    /// Initialize the executor
    pub fn open(&mut self) -> Result<(), QueryError> {
        match self {
            Self::ScanVertices { .. } => operators::sources::open_scanvertices(self),
            Self::ScanEdges { .. } => operators::sources::open_scanedges(self),
            Self::Filter { .. } => operators::single_input::open_filter(self),
            Self::Project { .. } => operators::single_input::open_project(self),
            Self::Limit { .. } => operators::single_input::open_limit(self),
            Self::Distinct { .. } => operators::single_input::open_distinct(self),
            Self::Aggregate { .. } => operators::stateful::open_aggregate(self),
            Self::Sort { .. } => operators::stateful::open_sort(self),
            Self::GroupBy { .. } => operators::stateful::open_groupby(self),
            Self::WindowFunction { .. } => operators::stateful::open_windowfunction(self),
            Self::HashJoin { .. } => operators::binary::open_hashjoin(self),
            Self::NestedLoopJoin { .. } => operators::binary::open_nestedloopjoin(self),
            Self::Union { .. } => operators::set_ops::open_union(self),
            Self::UnionAll { .. } => operators::set_ops::open_unionall(self),
            Self::Intersect { .. } => operators::set_ops::open_intersect(self),
            Self::Except { .. } => operators::set_ops::open_except(self),
        }
    }

    /// Pull next chunk from the executor
    pub fn next(&mut self) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::ScanVertices { .. } => operators::sources::next_scanvertices(self),
            Self::ScanEdges { .. } => operators::sources::next_scanedges(self),
            Self::Filter { .. } => operators::single_input::next_filter(self),
            Self::Project { .. } => operators::single_input::next_project(self),
            Self::Limit { .. } => operators::single_input::next_limit(self),
            Self::Distinct { .. } => operators::single_input::next_distinct(self),
            Self::Aggregate { .. } => operators::stateful::next_aggregate(self),
            Self::Sort { .. } => operators::stateful::next_sort(self),
            Self::GroupBy { .. } => operators::stateful::next_groupby(self),
            Self::WindowFunction { .. } => operators::stateful::next_windowfunction(self),
            Self::HashJoin { .. } => operators::binary::next_hashjoin(self),
            Self::NestedLoopJoin { .. } => operators::binary::next_nestedloopjoin(self),
            Self::Union { .. } => operators::set_ops::next_union(self),
            Self::UnionAll { .. } => operators::set_ops::next_unionall(self),
            Self::Intersect { .. } => operators::set_ops::next_intersect(self),
            Self::Except { .. } => operators::set_ops::next_except(self),
        }
    }

    /// Stop execution (for LIMIT)
    pub fn stop(&mut self) -> Result<(), QueryError> {
        match self {
            Self::ScanVertices { .. } => operators::sources::stop_scanvertices(self),
            Self::ScanEdges { .. } => operators::sources::stop_scanedges(self),
            Self::Filter { .. } => operators::single_input::stop_filter(self),
            Self::Project { .. } => operators::single_input::stop_project(self),
            Self::Limit { .. } => operators::single_input::stop_limit(self),
            Self::Distinct { .. } => operators::single_input::stop_distinct(self),
            Self::Aggregate { .. } => operators::stateful::stop_aggregate(self),
            Self::Sort { .. } => operators::stateful::stop_sort(self),
            Self::GroupBy { .. } => operators::stateful::stop_groupby(self),
            Self::WindowFunction { .. } => operators::stateful::stop_windowfunction(self),
            Self::HashJoin { .. } => operators::binary::stop_hashjoin(self),
            Self::NestedLoopJoin { .. } => operators::binary::stop_nestedloopjoin(self),
            Self::Union { .. } => operators::set_ops::stop_union(self),
            Self::UnionAll { .. } => operators::set_ops::stop_unionall(self),
            Self::Intersect { .. } => operators::set_ops::stop_intersect(self),
            Self::Except { .. } => operators::set_ops::stop_except(self),
        }
    }

    /// Clean up resources
    pub fn close(&mut self) -> Result<(), QueryError> {
        match self {
            Self::ScanVertices { .. } => operators::sources::close_scanvertices(self),
            Self::ScanEdges { .. } => operators::sources::close_scanedges(self),
            Self::Filter { .. } => operators::single_input::close_filter(self),
            Self::Project { .. } => operators::single_input::close_project(self),
            Self::Limit { .. } => operators::single_input::close_limit(self),
            Self::Distinct { .. } => operators::single_input::close_distinct(self),
            Self::Aggregate { .. } => operators::stateful::close_aggregate(self),
            Self::Sort { .. } => operators::stateful::close_sort(self),
            Self::GroupBy { .. } => operators::stateful::close_groupby(self),
            Self::WindowFunction { .. } => operators::stateful::close_windowfunction(self),
            Self::HashJoin { .. } => operators::binary::close_hashjoin(self),
            Self::NestedLoopJoin { .. } => operators::binary::close_nestedloopjoin(self),
            Self::Union { .. } => operators::set_ops::close_union(self),
            Self::UnionAll { .. } => operators::set_ops::close_unionall(self),
            Self::Intersect { .. } => operators::set_ops::close_intersect(self),
            Self::Except { .. } => operators::set_ops::close_except(self),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_buffer() -> Vec<Vec<Value>> {
        (0..100)
            .map(|i| {
                vec![
                    Value::BigInt(i as i64),
                    Value::String(format!("vertex_{}", i)),
                    Value::String(format!("label_{}", i % 10)),
                    Value::String(format!("prop_{}", i % 100)),
                    Value::BigInt((i % 1000) as i64),
                ]
            })
            .collect()
    }

    #[test]
    fn test_scan_vertices_with_buffer() {
        let buffer = create_test_buffer();
        let mut executor = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: buffer.clone(),
            current_index: 0,
        };

        executor.open().unwrap();
        let chunk = executor.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 100);
        executor.close().unwrap();
    }

    #[test]
    fn test_scan_edges_with_buffer() {
        let buffer: Vec<Vec<Value>> = (0..50)
            .map(|i| {
                vec![
                    Value::BigInt((i % 1000) as i64),
                    Value::BigInt(((i + 1) % 1000) as i64),
                    Value::String(format!("edge_type_{}", i % 5)),
                    Value::BigInt((i % 100) as i64),
                    Value::BigInt((1000 + i) as i64),
                ]
            })
            .collect();

        let mut executor = StreamingExecutor::ScanEdges {
            partition_id: 0,
            buffer: buffer.clone(),
            current_index: 0,
        };

        executor.open().unwrap();
        let chunk = executor.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 50);
        executor.close().unwrap();
    }

    #[test]
    fn test_limit_executor() {
        let buffer = create_test_buffer();
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
        });

        let mut limit = StreamingExecutor::Limit {
            input: scan,
            limit: 10,
            consumed: 0,
            opened: false,
        };

        limit.open().unwrap();
        let mut total = 0;
        while let Some(chunk) = limit.next().unwrap() {
            total += chunk.len();
        }
        limit.close().unwrap();

        assert_eq!(total, 10);
    }

    #[test]
    fn test_dynamic_column_count() {
        // Test with more than 5 columns to verify dynamic column support
        let buffer: Vec<Vec<Value>> = vec![
            vec![
                Value::BigInt(1),
                Value::String("a".to_string()),
                Value::String("b".to_string()),
                Value::String("c".to_string()),
                Value::String("d".to_string()),
                Value::String("e".to_string()),
                Value::String("f".to_string()),
                Value::String("g".to_string()),
                Value::String("h".to_string()),
            ],
            vec![
                Value::BigInt(2),
                Value::String("i".to_string()),
                Value::String("j".to_string()),
                Value::String("k".to_string()),
                Value::String("l".to_string()),
                Value::String("m".to_string()),
                Value::String("n".to_string()),
                Value::String("o".to_string()),
                Value::String("p".to_string()),
            ],
        ];

        let mut executor = StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: buffer.clone(),
            current_index: 0,
        };

        executor.open().unwrap();
        let chunk = executor.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 2);
        assert_eq!(chunk.num_columns(), 9);

        executor.close().unwrap();
    }
}
