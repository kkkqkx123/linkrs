//! Integration tests for StreamingExecutor
//!
//! Tests complete streaming execution pipelines with multiple operators
//! chained together to verify end-to-end functionality.

use graphdb_query::core::types::expr::Expression;
use graphdb_query::core::Value;
use graphdb_query::query::executor::base::{MemoryBudget, MemoryTracker};
use graphdb_query::query::executor::streaming::executor::StreamingExecutor;

// ====== Test Helpers ======

fn create_simple_scan(size: usize) -> StreamingExecutor {
    let buffer = (0..size)
        .map(|i| vec![Value::Int(i as i32), Value::String(format!("item_{}", i))])
        .collect();

    StreamingExecutor::ScanVertices {
        partition_id: 0,
        buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    }
}

fn create_scan_with_data(data: Vec<Vec<Value>>) -> StreamingExecutor {
    StreamingExecutor::ScanVertices {
        partition_id: 0,
        buffer: data,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    }
}

// ====== Integration Test Cases ======

#[test]
fn test_filter_then_limit_pipeline() {
    // Create: Scan -> Filter -> Limit
    let scan = create_simple_scan(100);

    let filter = StreamingExecutor::Filter {
        input: Box::new(scan),
        predicate: Expression::Literal(Value::Bool(true)),
        opened: false,
        plan_node_id: 0,
    };

    let mut pipeline = StreamingExecutor::Limit {
        input: Box::new(filter),
        limit: 10,
        consumed: 0,
        opened: false,
        plan_node_id: 0,
    };

    pipeline.open().unwrap();
    let chunk = pipeline.next().unwrap();
    assert!(chunk.is_some());
    assert_eq!(chunk.unwrap().len(), 10);
    pipeline.close().unwrap();
}

#[test]
fn test_project_then_distinct_pipeline() {
    // Create: Scan -> Project -> Distinct
    let data = vec![
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(2), Value::String("b".to_string())],
    ];

    let scan = create_scan_with_data(data);

    let project = StreamingExecutor::Project {
        input: Box::new(scan),
        output_expressions: vec![Expression::Literal(Value::Int(0))],
        output_col_names: vec![],
        opened: false,
        plan_node_id: 0,
    };

    let mut pipeline = StreamingExecutor::Distinct {
        input: Box::new(project),
        seen_rows: std::collections::HashSet::new(),
        opened: false,
        plan_node_id: 0,
    };

    pipeline.open().unwrap();
    let chunk = pipeline.next().unwrap();
    assert!(chunk.is_some());
    pipeline.close().unwrap();
}

#[test]
fn test_join_with_small_inputs() {
    // Create: ScanLeft -> Join <- ScanRight
    let left = create_scan_with_data(vec![
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(2), Value::String("b".to_string())],
    ]);

    let right = create_scan_with_data(vec![
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(2), Value::String("b".to_string())],
    ]);

    let mut join = StreamingExecutor::HashJoin {
        left: Box::new(left),
        right: Box::new(right),
        join_condition: None,
        hash_keys: vec![],
        probe_keys: vec![],
        build_side_hash: std::collections::HashMap::new(),
        all_right_rows: Vec::new(),
        left_consumed: false,
        memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
        opened: false,
        right_col_names: vec![],
        plan_node_id: 0,
    };

    join.open().unwrap();
    let chunk = join.next().unwrap();
    assert!(chunk.is_some());
    // Cartesian product: 2 × 2 = 4
    assert_eq!(chunk.unwrap().len(), 4);
    join.close().unwrap();
}

#[test]
fn test_union_then_limit_pipeline() {
    // Create: (ScanLeft Union ScanRight) -> Limit
    let left = create_scan_with_data(vec![vec![Value::Int(1)], vec![Value::Int(2)]]);

    let right = create_scan_with_data(vec![vec![Value::Int(2)], vec![Value::Int(3)]]);

    let union = StreamingExecutor::Union {
        left: Box::new(left),
        right: Box::new(right),
        seen_rows: std::collections::HashSet::new(),
        left_consumed: false,
        opened: false,
        plan_node_id: 0,
    };

    let mut pipeline = StreamingExecutor::Limit {
        input: Box::new(union),
        limit: 2,
        consumed: 0,
        opened: false,
        plan_node_id: 0,
    };

    pipeline.open().unwrap();
    let chunk = pipeline.next().unwrap();
    assert!(chunk.is_some());
    assert!(chunk.unwrap().len() <= 2);
    pipeline.close().unwrap();
}

#[test]
fn test_except_then_filter_pipeline() {
    // Create: (ScanLeft Except ScanRight) -> Filter
    let left = create_scan_with_data(vec![
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(2), Value::String("b".to_string())],
        vec![Value::Int(3), Value::String("c".to_string())],
    ]);

    let right = create_scan_with_data(vec![vec![Value::Int(2), Value::String("b".to_string())]]);

    let except = StreamingExecutor::Except {
        left: Box::new(left),
        right: Box::new(right),
        exclude_rows: std::collections::HashSet::new(),
        right_buffered: false,
        opened: false,
        plan_node_id: 0,
    };

    let mut pipeline = StreamingExecutor::Filter {
        input: Box::new(except),
        predicate: Expression::Literal(Value::Bool(true)),
        opened: false,
        plan_node_id: 0,
    };

    pipeline.open().unwrap();
    let chunk = pipeline.next().unwrap();
    assert!(chunk.is_some());
    // Should have 2 rows (1 and 3, excluding 2)
    assert_eq!(chunk.unwrap().len(), 2);
    pipeline.close().unwrap();
}
