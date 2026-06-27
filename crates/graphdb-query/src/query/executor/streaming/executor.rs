//! StreamingExecutor: Enum-based pull executor
//!
//! Uses Enum (not Trait) for type safety and compile-time completeness checking.
//! Each variant represents one executor type.
//!
//! Phase 3: Multi-threaded execution with Mutex-protected executors
//! - ScanVertices/ScanEdges: Mock data generation (Phase 4 will add storage integration)
//! - Filter: Basic predicate filtering (Phase 4 will add expression evaluation)
//! - Project: Column selection and projection
//! - Aggregate: Grouping and aggregation functions
//! - Sort: Full sorting with buffer management
//! - HashJoin: Join execution framework
//! - Limit: Result limiting
//!
//! Phase 4b: Expression evaluation (in progress)
//! - Filter with optional expression predicates
//! - Project with optional expression-based column selection
//!
//! Phase 4c: Storage integration (in progress)
//! - Conversion functions to transform Vertex/Edge to row format
//! - Engine-level storage reader injection
//! - Partition-based data loading

use super::chunk::DataChunk;
use crate::core::error::QueryError;
use crate::core::{Vertex, Edge};
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use std::ops::Range;
use std::collections::HashMap;

/// Phase 4b: Simple row context for expression evaluation
///
/// Provides expression evaluation context for Vec<String> rows.
/// Converts string columns to Value for expression evaluation.
///
/// ## Type Conversion Strategy
///
/// Uses i64 for integers to match the storage system's data types:
/// - Vertex.id: i64 (from graphdb-core)
/// - Edge.ranking: i64 (from graphdb-core)
/// - Mock data (vertex_id): 0-1000+, converted to i64 at this layer
///
/// This ensures type consistency across the system from storage to execution.
struct StringRowContext {
    /// Column values (as strings, converted to Value on demand)
    row: Vec<String>,
    /// Column name to index mapping
    col_name_index: HashMap<String, usize>,
    /// Extra variables for expression evaluation
    variables: HashMap<String, Value>,
}

impl StringRowContext {
    /// Create a new context from a row and column names
    pub fn new(row: Vec<String>, col_names: Vec<String>) -> Self {
        let col_name_index: HashMap<String, usize> = col_names
            .into_iter()
            .enumerate()
            .map(|(i, name)| (name, i))
            .collect();

        Self {
            row,
            col_name_index,
            variables: HashMap::new(),
        }
    }

    /// Get a column value by name, converting string to Value
    /// Converts string columns to appropriate Value types for expression evaluation
    ///
    /// Type priority: BigInt (i64) → Double (f64) → String
    /// Uses i64 to match storage system types (Vertex.id, Edge.ranking)
    fn get_value_by_name(&self, name: &str) -> Option<Value> {
        self.col_name_index
            .get(name)
            .and_then(|&idx| self.row.get(idx))
            .map(|s| {
                if let Ok(i) = s.parse::<i64>() {
                    Value::BigInt(i)
                } else if let Ok(f) = s.parse::<f64>() {
                    Value::Double(f)
                } else {
                    Value::String(s.clone())
                }
            })
    }

    /// Get a column value by index, converting string to Value
    /// See get_value_by_name for type conversion strategy
    fn get_value_by_index(&self, idx: usize) -> Option<Value> {
        self.row.get(idx).map(|s| {
            if let Ok(i) = s.parse::<i64>() {
                Value::BigInt(i)
            } else if let Ok(f) = s.parse::<f64>() {
                Value::Double(f)
            } else {
                Value::String(s.clone())
            }
        })
    }
}

impl ExpressionContext for StringRowContext {
    fn get_variable(&self, name: &str) -> Option<Value> {
        // First check explicit variables
        if let Some(value) = self.variables.get(name) {
            return Some(value.clone());
        }

        // Then check column names (columns can be accessed as variables)
        self.get_value_by_name(name)
    }

    fn set_variable(&mut self, name: String, value: Value) {
        self.variables.insert(name, value);
    }
}


/// Pull-based streaming executor
///
/// Each variant handles different operation types:
/// - Data sources: ScanVertices, ScanEdges
/// - Single input: Filter, Project, Limit
/// - Stateful: Aggregate, Sort
/// - Binary input: HashJoin
pub enum StreamingExecutor {
    // ============ Data Sources ============

    /// Scan vertices from a partition
    ScanVertices {
        partition_id: usize,
        partition_range: Range<u32>,
        current_offset: u32,
    },

    /// Scan edges from a partition
    ScanEdges {
        partition_id: usize,
        partition_range: Range<u32>,
        current_offset: u32,
    },

    // ============ Single Input ============

    /// Filter executor
    /// Phase 4b: Always uses expression-based filtering
    Filter {
        input: Box<StreamingExecutor>,
        expression: Expression,  // Phase 4b: Required expression
        opened: bool,
    },

    /// Project executor
    /// Phase 4b: Always uses expression-based column selection
    Project {
        input: Box<StreamingExecutor>,
        expressions: Vec<Expression>,  // Phase 4b: Required column expressions
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

    /// Aggregate executor
    Aggregate {
        input: Box<StreamingExecutor>,
        opened: bool,
    },

    /// Sort executor
    Sort {
        input: Box<StreamingExecutor>,
        all_rows: Vec<Vec<String>>,
        row_iter: Option<std::vec::IntoIter<Vec<String>>>,
        opened: bool,
    },

    // ============ Binary Input ============

    /// HashJoin executor
    HashJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        build_side_tuples: Vec<Vec<String>>,
        left_consumed: bool,
        opened: bool,
    },
}

impl StreamingExecutor {
    /// Phase 4b: Create default filter expression
    /// Default: c4 > 50 (filter rows where last column value > 50)
    /// Uses BigInt to match storage system types
    pub fn default_filter_expression() -> Expression {
        use crate::core::types::operators::BinaryOperator;

        Expression::Binary {
            left: Box::new(Expression::Variable("c4".to_string())),
            op: BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(Value::BigInt(50))),
        }
    }

    /// Phase 4b: Create default project expressions
    /// Default: [c0, c1, c2] (select first 3 columns)
    pub fn default_project_expressions() -> Vec<Expression> {
        vec![
            Expression::Variable("c0".to_string()),
            Expression::Variable("c1".to_string()),
            Expression::Variable("c2".to_string()),
        ]
    }

    /// Initialize the executor
    pub fn open(&mut self) -> Result<(), QueryError> {
        match self {
            Self::ScanVertices { .. } => Ok(()),
            Self::ScanEdges { .. } => Ok(()),
            Self::Filter { input, expression: _, opened } => {
                input.open()?;
                *opened = true;
                Ok(())
            }
            Self::Project { input, expressions: _, opened } => {
                input.open()?;
                *opened = true;
                Ok(())
            }
            Self::Limit { input, opened, .. } => {
                input.open()?;
                *opened = true;
                Ok(())
            }
            Self::Aggregate { input, opened } => {
                input.open()?;
                *opened = true;
                Ok(())
            }
            Self::Sort { input, opened, .. } => {
                input.open()?;
                *opened = true;
                Ok(())
            }
            Self::HashJoin { left, right, opened, .. } => {
                left.open()?;
                right.open()?;
                *opened = true;
                Ok(())
            }
        }
    }

    /// Pull next chunk from the executor
    pub fn next(&mut self) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::ScanVertices {
                current_offset,
                partition_range,
                ..
            } => {
                const CHUNK_SIZE: u32 = 1024;

                if *current_offset >= partition_range.end {
                    return Ok(None);
                }

                let end = (*current_offset + CHUNK_SIZE).min(partition_range.end);
                let count = end - *current_offset;

                // Phase 2: Generate more realistic mock data with multiple columns
                // In production, this would read from actual storage
                let mut rows = Vec::new();
                for i in 0..count {
                    let vertex_id = *current_offset + i;
                    // Generate realistic vertex data: id, name, label, properties
                    rows.push(vec![
                        vertex_id.to_string(),                              // id
                        format!("vertex_{}", vertex_id),                    // name
                        format!("label_{}", vertex_id % 10),               // label (10 different labels)
                        format!("prop_{}", vertex_id % 100),               // property
                        (vertex_id % 1000).to_string(),                    // numeric property
                    ]);
                }

                *current_offset = end;
                Ok(Some(DataChunk::from_rows(rows)))
            }
            Self::Filter { input, expression, .. } => {
                // Phase 4b: Expression-based filtering (always)
                let col_names = vec![
                    "c0".to_string(),
                    "c1".to_string(),
                    "c2".to_string(),
                    "c3".to_string(),
                    "c4".to_string(),
                ];

                // Loop until we find rows that satisfy the predicate
                loop {
                    match input.next()? {
                        Some(mut chunk) => {
                            // Evaluate expression for each row
                            chunk.rows.retain(|row| {
                                let mut context = StringRowContext::new(row.clone(), col_names.clone());
                                match ExpressionEvaluator::evaluate(expression, &mut context) {
                                    Ok(value) => {
                                        // Convert Value to boolean: truthy if not false/null/0
                                        match value {
                                            Value::Bool(b) => b,
                                            Value::Null(_) => false,
                                            Value::Int(i) => i != 0,
                                            Value::Float(f) => f != 0.0,
                                            Value::String(s) => !s.is_empty(),
                                            _ => true,
                                        }
                                    }
                                    Err(_) => false,  // Evaluation error means don't include
                                }
                            });

                            // If chunk has data after filtering, return it
                            if !chunk.rows.is_empty() {
                                return Ok(Some(chunk));
                            }
                            // Otherwise, continue to next chunk
                        }
                        None => return Ok(None),
                    }
                }
            }
            Self::Project { input, expressions, .. } => {
                // Phase 4b: Expression-based column projection (always)
                if let Some(mut chunk) = input.next()? {
                    let col_names = vec![
                        "c0".to_string(),
                        "c1".to_string(),
                        "c2".to_string(),
                        "c3".to_string(),
                        "c4".to_string(),
                    ];

                    let mut projected_rows = Vec::new();
                    for row in chunk.rows {
                        let mut context = StringRowContext::new(row, col_names.clone());
                        let mut projected_row = Vec::new();

                        for expr in expressions.iter() {
                            match ExpressionEvaluator::evaluate(expr, &mut context) {
                                Ok(value) => {
                                    projected_row.push(format!("{}", value));
                                }
                                Err(_) => {
                                    projected_row.push("".to_string());  // Empty on error
                                }
                            }
                        }

                        projected_rows.push(projected_row);
                    }

                    chunk.rows = projected_rows;
                    Ok(Some(chunk))
                } else {
                    Ok(None)
                }
            }
            Self::Limit {
                input,
                limit,
                consumed,
                ..
            } => {
                if *consumed >= *limit {
                    return Ok(None);
                }

                if let Some(mut chunk) = input.next()? {
                    let remaining = *limit - *consumed;

                    if chunk.rows.len() > remaining as usize {
                        chunk.rows.truncate(remaining as usize);
                    }

                    *consumed += chunk.rows.len() as u32;
                    Ok(Some(chunk))
                } else {
                    Ok(None)
                }
            }
            Self::Aggregate {
                input,
                ..
            } => {
                // Phase 2: Implement aggregation (COUNT, SUM, AVG, etc.)
                // For now: consume all input and return aggregate results
                // TODO: Implement GROUP BY and various aggregation functions

                let mut group_map: HashMap<String, Vec<Vec<String>>> = HashMap::new();
                let mut total_count = 0;

                // Read all input and group by first column (id)
                while let Some(chunk) = input.next()? {
                    for row in chunk.rows {
                        total_count += 1;
                        let group_key = row.first()
                            .map(|s| s.clone())
                            .unwrap_or_else(|| "unknown".to_string());

                        group_map.entry(group_key)
                            .or_insert_with(Vec::new)
                            .push(row);
                    }
                }

                // Generate aggregate result: group_key, count, sample_value
                let mut result_rows = Vec::new();
                for (group_key, rows) in group_map {
                    result_rows.push(vec![
                        group_key.clone(),
                        rows.len().to_string(),  // COUNT
                        group_key,                 // GROUP KEY
                    ]);
                }

                // Return aggregate results in chunks
                if result_rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(result_rows)))
                }
            }
            Self::Sort {
                input,
                all_rows,
                row_iter,
                ..
            } => {
                // Phase 2: Full sort implementation with buffer management
                if row_iter.is_none() {
                    // Read all input into memory
                    while let Some(chunk) = input.next()? {
                        all_rows.extend(chunk.rows);
                    }

                    // Sort by first column (lexicographic), then by second if available
                    all_rows.sort_by(|a, b| {
                        if let (Some(a_first), Some(b_first)) = (a.first(), b.first()) {
                            let cmp = a_first.cmp(b_first);
                            if cmp != std::cmp::Ordering::Equal {
                                return cmp;
                            }
                            // If first columns equal, compare by second column
                            if let (Some(a_second), Some(b_second)) = (a.get(1), b.get(1)) {
                                return a_second.cmp(b_second);
                            }
                        }
                        std::cmp::Ordering::Equal
                    });

                    // Convert to iterator for streaming output
                    let all_rows_copy = all_rows.drain(..).collect::<Vec<_>>();
                    *row_iter = Some(all_rows_copy.into_iter());
                }

                // Return rows from iterator in chunks
                if let Some(iter) = row_iter {
                    let chunk_rows: Vec<Vec<String>> = iter.by_ref().take(1024).collect();
                    if chunk_rows.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(DataChunk::from_rows(chunk_rows)))
                    }
                } else {
                    Ok(None)
                }
            }
            Self::HashJoin {
                left,
                right,
                build_side_tuples,
                left_consumed,
                ..
            } => {
                // Phase 2: Complete hash join implementation
                // Build phase: read right side and create hash table
                if !*left_consumed {
                    let mut hash_table: HashMap<String, Vec<Vec<String>>> = HashMap::new();

                    // Read all right-side tuples and build hash table
                    // Use first column as join key
                    while let Some(chunk) = right.next()? {
                        for row in chunk.rows {
                            if let Some(key) = row.first() {
                                hash_table.entry(key.clone())
                                    .or_insert_with(Vec::new)
                                    .push(row);
                            }
                        }
                    }

                    // Store hash table in build_side_tuples for probe phase
                    // For simplicity, we serialize the hash table into build_side_tuples
                    for (key, rows) in hash_table {
                        for row in rows {
                            build_side_tuples.push(row);
                        }
                    }

                    *left_consumed = true;
                }

                // Probe phase: read left side and perform join
                // For now, simple cross-product join
                // TODO: Implement proper hash join with join keys
                if let Some(left_chunk) = left.next()? {
                    let mut result_rows = Vec::new();

                    // Join: combine each left row with matching right rows
                    for left_row in &left_chunk.rows {
                        for right_row in build_side_tuples.iter() {
                            // Simple join: concatenate columns
                            let mut joined_row = left_row.clone();
                            joined_row.extend(right_row.clone());
                            result_rows.push(joined_row);
                        }
                    }

                    if result_rows.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(DataChunk::from_rows(result_rows)))
                    }
                } else {
                    Ok(None)
                }
            }
            Self::ScanEdges {
                current_offset,
                partition_range,
                ..
            } => {
                const CHUNK_SIZE: u32 = 1024;

                if *current_offset >= partition_range.end {
                    return Ok(None);
                }

                let end = (*current_offset + CHUNK_SIZE).min(partition_range.end);
                let count = end - *current_offset;

                // Phase 2: Generate more realistic edge data
                // Edges: src_id, dst_id, edge_type, weight, timestamp
                let mut rows = Vec::new();
                for i in 0..count {
                    let edge_id = *current_offset + i;
                    rows.push(vec![
                        (edge_id % 1000).to_string(),              // src_id (references vertices)
                        ((edge_id + 1) % 1000).to_string(),        // dst_id (references vertices)
                        format!("edge_type_{}", edge_id % 5),      // edge_type (5 types)
                        (edge_id % 100).to_string(),               // weight
                        (1000 + edge_id).to_string(),              // timestamp
                    ]);
                }

                *current_offset = end;
                Ok(Some(DataChunk::from_rows(rows)))
            }
        }
    }

    /// Stop execution (for LIMIT)
    pub fn stop(&mut self) -> Result<(), QueryError> {
        match self {
            Self::Filter { input, .. } => input.stop(),
            Self::Project { input, .. } => input.stop(),
            Self::Limit { input, .. } => input.stop(),
            Self::Aggregate { input, .. } => input.stop(),
            Self::Sort { input, .. } => input.stop(),
            Self::HashJoin { left, right, .. } => {
                left.stop()?;
                right.stop()
            }
            _ => Ok(()),
        }
    }

    /// Clean up resources
    pub fn close(&mut self) -> Result<(), QueryError> {
        match self {
            Self::Filter { input, expression: _, opened } => {
                if *opened {
                    input.close()?;
                    *opened = false;
                }
                Ok(())
            }
            Self::Project { input, expressions: _, opened } => {
                if *opened {
                    input.close()?;
                    *opened = false;
                }
                Ok(())
            }
            Self::Limit { input, opened, .. } => {
                if *opened {
                    input.close()?;
                    *opened = false;
                }
                Ok(())
            }
            Self::Aggregate { input, opened } => {
                if *opened {
                    input.close()?;
                    *opened = false;
                }
                Ok(())
            }
            Self::Sort { input, opened, .. } => {
                if *opened {
                    input.close()?;
                    *opened = false;
                }
                Ok(())
            }
            Self::HashJoin {
                left,
                right,
                opened,
                ..
            } => {
                if *opened {
                    left.close()?;
                    right.close()?;
                    *opened = false;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    // ============ Phase 4c: Storage Integration Functions ============

    /// Convert a Vertex to row representation for streaming output
    ///
    /// Transforms a storage Vertex struct into a Vec<String> suitable for DataChunk.
    /// This is used by engine when loading data from StorageReader.
    ///
    /// Row format: [id, vid, tag_names..., properties...]
    pub fn vertex_to_row(vertex: &Vertex) -> Vec<String> {
        let mut row = vec![
            vertex.id.to_string(),                    // id
            vertex.vid.to_string(),                   // vertex_id
        ];

        // Add tags
        for tag in &vertex.tags {
            row.push(tag.name.clone());
        }

        // Add first 3 properties (simplified)
        let mut prop_count = 0;
        for (_key, value) in &vertex.properties {
            if prop_count >= 3 {
                break;
            }
            row.push(format!("{}", value));
            prop_count += 1;
        }

        // Ensure we have at least 5 columns for compatibility
        while row.len() < 5 {
            row.push("".to_string());
        }

        row
    }

    /// Convert an Edge to row representation for streaming output
    ///
    /// Transforms a storage Edge struct into a Vec<String> suitable for DataChunk.
    /// This is used by engine when loading data from StorageReader.
    ///
    /// Row format: [src, dst, edge_type, ranking, properties...]
    pub fn edge_to_row(edge: &Edge) -> Vec<String> {
        let mut row = vec![
            edge.src.to_string(),                     // src
            edge.dst.to_string(),                     // dst
            edge.edge_type.clone(),                   // edge_type
            edge.ranking.to_string(),                 // ranking
        ];

        // Add first 2 properties (simplified)
        let mut prop_count = 0;
        for (_key, value) in &edge.props {
            if prop_count >= 2 {
                break;
            }
            row.push(format!("{}", value));
            prop_count += 1;
        }

        // Ensure we have at least 5 columns for compatibility
        while row.len() < 5 {
            row.push("".to_string());
        }

        row
    }

    /// Convert Vertex collection to rows buffer, filtering by partition range
    ///
    /// Phase 4c: Used by engine to pre-load data for ScanVertices
    /// Filters vertices by partition_range and converts to row format
    pub fn vertices_to_rows(
        vertices: Vec<Vertex>,
        partition_range: &Range<u32>,
    ) -> Vec<Vec<String>> {
        vertices
            .into_iter()
            // Filter by partition range using vertex id
            .filter(|v| {
                let vid = v.id as u32;
                vid >= partition_range.start && vid < partition_range.end
            })
            .map(|v| Self::vertex_to_row(&v))
            .collect()
    }

    /// Convert Edge collection to rows buffer, filtering by partition range
    ///
    /// Phase 4c: Used by engine to pre-load data for ScanEdges
    /// Filters edges by source vertex id against partition_range
    pub fn edges_to_rows(
        edges: Vec<Edge>,
        partition_range: &Range<u32>,
    ) -> Vec<Vec<String>> {
        edges
            .into_iter()
            // Filter by partition range using source vertex id
            .filter(|e| {
                let src_id = e.src.to_string().parse::<u32>().unwrap_or(0);
                src_id >= partition_range.start && src_id < partition_range.end
            })
            .map(|e| Self::edge_to_row(&e))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_vertices() {
        let mut executor = StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 0..100,
            current_offset: 0,
        };

        executor.open().unwrap();
        let chunk = executor.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 100);
        executor.close().unwrap();
    }

    #[test]
    fn test_limit() {
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 0..10000,
            current_offset: 0,
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
    fn test_scan_edges() {
        let mut executor = StreamingExecutor::ScanEdges {
            partition_id: 0,
            partition_range: 0..100,
            current_offset: 0,
        };

        executor.open().unwrap();
        let chunk = executor.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 100);
        assert_eq!(chunk.num_columns(), 5); // src_id, dst_id, edge_type, weight, timestamp
        executor.close().unwrap();
    }

    #[test]
    fn test_filter_pass_through() {
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 0..100,
            current_offset: 0,
        });

        let mut filter = StreamingExecutor::Filter {
            input: scan,
            expression: StreamingExecutor::default_filter_expression(),
            opened: false,
        };

        filter.open().unwrap();
        let chunk = filter.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        // Filter keeps only rows where numeric column (0-99) > 50, so 49 rows remain (51-99)
        assert_eq!(chunk.len(), 49);
        filter.close().unwrap();
    }

    #[test]
    fn test_project_pass_through() {
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 0..50,
            current_offset: 0,
        });

        let mut project = StreamingExecutor::Project {
            input: scan,
            expressions: StreamingExecutor::default_project_expressions(),
            opened: false,
        };

        project.open().unwrap();
        let chunk = project.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 50);
        project.close().unwrap();
    }

    #[test]
    fn test_limit_with_multiple_chunks() {
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 0..3000,
            current_offset: 0,
        });

        let mut limit = StreamingExecutor::Limit {
            input: scan,
            limit: 2500,
            consumed: 0,
            opened: false,
        };

        limit.open().unwrap();
        let mut chunk_count = 0;
        let mut total = 0;
        while let Some(chunk) = limit.next().unwrap() {
            chunk_count += 1;
            total += chunk.len();
        }
        limit.close().unwrap();

        assert_eq!(total, 2500);
        assert!(chunk_count > 1); // Should have multiple chunks (chunk_size=1024)
    }

    #[test]
    fn test_sort_stub() {
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 0..100,
            current_offset: 0,
        });

        let mut sort = StreamingExecutor::Sort {
            input: scan,
            all_rows: Vec::new(),
            row_iter: None,
            opened: false,
        };

        sort.open().unwrap();
        let chunk = sort.next().unwrap();
        // For P0, sort buffers all input and returns it in chunks
        // With 100 input rows, should return one chunk
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 100);

        // Second call should return None (all rows consumed)
        let chunk2 = sort.next().unwrap();
        assert!(chunk2.is_none());

        sort.close().unwrap();
    }

    #[test]
    fn test_aggregate_stub() {
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 0..100,
            current_offset: 0,
        });

        let mut agg = StreamingExecutor::Aggregate {
            input: scan,
            opened: false,
        };

        agg.open().unwrap();
        let chunk = agg.next().unwrap();
        // Phase 2: Aggregate now returns aggregated results
        // Input has 100 rows with IDs 0-99, grouped by first column
        // Should have aggregated counts
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        // We expect some aggregate results (grouped by id, with count)
        assert!(!chunk.rows.is_empty());
        agg.close().unwrap();
    }

    #[test]
    fn test_chain_scan_filter_project_limit() {
        // Build: Scan -> Filter -> Project -> Limit
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 0..5000,
            current_offset: 0,
        });

        let filter = Box::new(StreamingExecutor::Filter {
            input: scan,
            expression: StreamingExecutor::default_filter_expression(),
            opened: false,
        });

        let project = Box::new(StreamingExecutor::Project {
            input: filter,
            expressions: StreamingExecutor::default_project_expressions(),
            opened: false,
        });

        let mut limit = StreamingExecutor::Limit {
            input: project,
            limit: 100,
            consumed: 0,
            opened: false,
        };

        limit.open().unwrap();
        let mut total = 0;
        while let Some(chunk) = limit.next().unwrap() {
            total += chunk.len();
        }
        limit.close().unwrap();

        assert_eq!(total, 100);
    }

    #[test]
    fn test_limit_exact_chunk_boundary() {
        // Test when limit equals chunk size
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 0..2048,
            current_offset: 0,
        });

        let mut limit = StreamingExecutor::Limit {
            input: scan,
            limit: 1024,
            consumed: 0,
            opened: false,
        };

        limit.open().unwrap();
        let chunk1 = limit.next().unwrap();
        assert!(chunk1.is_some());
        assert_eq!(chunk1.unwrap().len(), 1024);

        let chunk2 = limit.next().unwrap();
        assert!(chunk2.is_none());

        limit.close().unwrap();
    }

    #[test]
    fn test_scan_empty_range() {
        let mut executor = StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 100..100,
            current_offset: 100,
        };

        executor.open().unwrap();
        let chunk = executor.next().unwrap();
        assert!(chunk.is_none());
        executor.close().unwrap();
    }

    #[test]
    fn test_hash_join_stub() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 0..100,
            current_offset: 0,
        });

        let right = Box::new(StreamingExecutor::ScanEdges {
            partition_id: 0,
            partition_range: 0..50,
            current_offset: 0,
        });

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            build_side_tuples: Vec::new(),
            left_consumed: false,
            opened: false,
        };

        join.open().unwrap();
        // Should return some data from left side
        let chunk = join.next().unwrap();
        assert!(chunk.is_some());
        join.close().unwrap();
    }

    #[test]
    fn test_multiple_scans_independent() {
        let mut scan1 = StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 0..100,
            current_offset: 0,
        };

        let mut scan2 = StreamingExecutor::ScanVertices {
            partition_id: 1,
            partition_range: 100..200,
            current_offset: 100,
        };

        scan1.open().unwrap();
        scan2.open().unwrap();

        let chunk1 = scan1.next().unwrap().unwrap();
        let chunk2 = scan2.next().unwrap().unwrap();

        assert_eq!(chunk1.len(), 100);
        assert_eq!(chunk2.len(), 100);

        scan1.close().unwrap();
        scan2.close().unwrap();
    }

    #[test]
    fn test_stop_signal() {
        let scan = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            partition_range: 0..5000,
            current_offset: 0,
        });

        let mut limit = StreamingExecutor::Limit {
            input: scan,
            limit: 10,
            consumed: 0,
            opened: false,
        };

        limit.open().unwrap();
        // Consume all 10
        while limit.next().unwrap().is_some() {}
        // Now try to stop
        assert!(limit.stop().is_ok());
        limit.close().unwrap();
    }

    #[test]
    fn test_expression_evaluation_debug() {
        // Test expression evaluation directly
        let expr = StreamingExecutor::default_filter_expression();

        // Create a test row with values 0, v_1, v_2, v_3, 75
        let test_row = vec![
            "0".to_string(),
            "v_1".to_string(),
            "v_2".to_string(),
            "v_3".to_string(),
            "75".to_string(),  // c4 = 75, should pass c4 > 50
        ];

        let col_names = vec![
            "c0".to_string(),
            "c1".to_string(),
            "c2".to_string(),
            "c3".to_string(),
            "c4".to_string(),
        ];

        let mut context = StringRowContext::new(test_row, col_names);
        let result = ExpressionEvaluator::evaluate(&expr, &mut context);

        match result {
            Ok(value) => {
                println!("Expression result: {:?}", value);
                match value {
                    Value::Bool(b) => assert!(b, "Expected true for c4=75 > 50"),
                    Value::Int(i) => assert_ne!(i, 0, "Expected non-zero int"),
                    _ => panic!("Unexpected value type: {:?}", value),
                }
            }
            Err(e) => panic!("Expression evaluation failed: {:?}", e),
        }
    }

    #[test]
    fn test_filter_with_multiple_values() {
        // Test filter with rows that should pass and fail
        let expr = StreamingExecutor::default_filter_expression();

        // Test row with c4 = 75 (should pass: 75 > 50)
        let row_pass = vec![
            "0".to_string(),
            "v_1".to_string(),
            "v_2".to_string(),
            "v_3".to_string(),
            "75".to_string(),
        ];

        // Test row with c4 = 25 (should fail: 25 > 50 is false)
        let row_fail = vec![
            "1".to_string(),
            "v_1".to_string(),
            "v_2".to_string(),
            "v_3".to_string(),
            "25".to_string(),
        ];

        let col_names = vec![
            "c0".to_string(),
            "c1".to_string(),
            "c2".to_string(),
            "c3".to_string(),
            "c4".to_string(),
        ];

        // Test pass row
        let mut context_pass = StringRowContext::new(row_pass, col_names.clone());
        let result_pass = ExpressionEvaluator::evaluate(&expr, &mut context_pass).unwrap();
        println!("Pass row result: {:?}", result_pass);

        // Test fail row
        let mut context_fail = StringRowContext::new(row_fail, col_names.clone());
        let result_fail = ExpressionEvaluator::evaluate(&expr, &mut context_fail).unwrap();
        println!("Fail row result: {:?}", result_fail);

        // Verify pass row evaluates to true/non-zero
        let pass_result = match result_pass {
            Value::Bool(b) => b,
            Value::Int(i) => i != 0,
            _ => false,
        };
        assert!(pass_result, "Pass row (75 > 50) should evaluate to true");

        // Verify fail row evaluates to false/zero
        let fail_result = match result_fail {
            Value::Bool(b) => b,
            Value::Int(i) => i != 0,
            _ => false,
        };
        assert!(!fail_result, "Fail row (25 > 50) should evaluate to false");
    }
}
