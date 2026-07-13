//! Integration tests for StreamingExecutor
//!
//! Tests complete streaming execution pipelines with multiple operators
//! chained together to verify end-to-end functionality.

use graphdb_query::core::types::expr::Expression;
use graphdb_query::core::Value;
use graphdb_query::query::executor::base::{MemoryBudget, MemoryTracker};
use graphdb_query::query::executor::streaming::executor::StreamingExecutor;
use graphdb_query::query::executor::streaming::operators::base::OperatorBase;
use graphdb_query::query::executor::streaming::operators::blocking_operator::BlockingOperator;
use graphdb_query::query::executor::streaming::operators::join_operator::JoinOperator;
use graphdb_query::query::executor::streaming::operators::set_operator::SetOperator;
use graphdb_query::query::executor::streaming::operators::source_operator::SourceOperator;
use graphdb_query::query::executor::streaming::operators::unary_operator::UnaryOperator;

// ====== Test Helpers ======

fn create_simple_scan(size: usize) -> StreamingExecutor {
    let buffer = (0..size)
        .map(|i| vec![Value::Int(i as i32), Value::String(format!("item_{}", i))])
        .collect();

    StreamingExecutor::Source(
        OperatorBase::new(0),
        SourceOperator::ScanVertices {
            partition_id: 0,
            buffer,
            current_index: 0,
            col_names: vec![],
        },
    )
}

fn create_scan_with_data(data: Vec<Vec<Value>>) -> StreamingExecutor {
    StreamingExecutor::Source(
        OperatorBase::new(0),
        SourceOperator::ScanVertices {
            partition_id: 0,
            buffer: data,
            current_index: 0,
            col_names: vec![],
        },
    )
}

// ====== Integration Test Cases ======

#[test]
fn test_filter_then_limit_pipeline() {
    let scan = create_simple_scan(100);

    let filter = StreamingExecutor::Unary(
        OperatorBase::new(0),
        Box::new(scan),
        UnaryOperator::Filter {
            predicate: Expression::Literal(Value::Bool(true)),
        },
    );

    let mut pipeline = StreamingExecutor::Unary(
        OperatorBase::new(0),
        Box::new(filter),
        UnaryOperator::Limit {
            offset: 0,
            limit: 10,
            skipped: 0,
            consumed: 0,
        },
    );

    pipeline.open().unwrap();
    let chunk = pipeline.advance().unwrap();
    assert!(chunk.is_some());
    assert_eq!(chunk.unwrap().len(), 10);
    pipeline.close().unwrap();
}

#[test]
fn test_project_then_distinct_pipeline() {
    let data = vec![
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(2), Value::String("b".to_string())],
    ];

    let scan = create_scan_with_data(data);

    let project = StreamingExecutor::Unary(
        OperatorBase::new(0),
        Box::new(scan),
        UnaryOperator::Project {
            output_expressions: vec![Expression::Literal(Value::Int(0))],
            output_col_names: vec![],
        },
    );

    let mut pipeline = StreamingExecutor::Blocking(
        OperatorBase::new(0),
        Box::new(project),
        BlockingOperator::Distinct {
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            state: None,
        },
    );

    pipeline.open().unwrap();
    let chunk = pipeline.advance().unwrap();
    assert!(chunk.is_some());
    pipeline.close().unwrap();
}

#[test]
fn test_join_with_small_inputs() {
    let left = create_scan_with_data(vec![
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(2), Value::String("b".to_string())],
    ]);

    let right = create_scan_with_data(vec![
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(2), Value::String("b".to_string())],
    ]);

    let mut join = StreamingExecutor::Join(
        OperatorBase::new(0),
        Box::new(left),
        Box::new(right),
        JoinOperator::HashJoin {
            join_condition: None,
            hash_keys: vec![],
            probe_keys: vec![],
            build_side_hash: std::collections::HashMap::new(),
            all_right_rows: Vec::new(),
            left_consumed: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            right_col_names: vec![],
        },
    );

    join.open().unwrap();
    let chunk = join.advance().unwrap();
    assert!(chunk.is_some());
    assert_eq!(chunk.unwrap().len(), 4);
    join.close().unwrap();
}

#[test]
fn test_union_then_limit_pipeline() {
    let left = create_scan_with_data(vec![vec![Value::Int(1)], vec![Value::Int(2)]]);
    let right = create_scan_with_data(vec![vec![Value::Int(2)], vec![Value::Int(3)]]);

    let union = StreamingExecutor::Set(
        OperatorBase::new(0),
        Box::new(left),
        Box::new(right),
        SetOperator::Union {
            seen_rows: std::collections::HashSet::new(),
            left_consumed: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
        },
    );

    let mut pipeline = StreamingExecutor::Unary(
        OperatorBase::new(0),
        Box::new(union),
        UnaryOperator::Limit {
            offset: 0,
            limit: 2,
            skipped: 0,
            consumed: 0,
        },
    );

    pipeline.open().unwrap();
    let chunk = pipeline.advance().unwrap();
    assert!(chunk.is_some());
    assert!(chunk.unwrap().len() <= 2);
    pipeline.close().unwrap();
}

#[test]
fn test_except_then_filter_pipeline() {
    let left = create_scan_with_data(vec![
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(2), Value::String("b".to_string())],
        vec![Value::Int(3), Value::String("c".to_string())],
    ]);

    let right = create_scan_with_data(vec![vec![Value::Int(2), Value::String("b".to_string())]]);

    let except = StreamingExecutor::Set(
        OperatorBase::new(0),
        Box::new(left),
        Box::new(right),
        SetOperator::Except {
            exclude_rows: std::collections::HashSet::new(),
            right_buffered: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
        },
    );

    let mut pipeline = StreamingExecutor::Unary(
        OperatorBase::new(0),
        Box::new(except),
        UnaryOperator::Filter {
            predicate: Expression::Literal(Value::Bool(true)),
        },
    );

    pipeline.open().unwrap();
    let chunk = pipeline.advance().unwrap();
    assert!(chunk.is_some());
    assert_eq!(chunk.unwrap().len(), 2);
    pipeline.close().unwrap();
}
