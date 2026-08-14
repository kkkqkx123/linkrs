//! Integration tests for StreamingExecutor
//!
//! Tests complete streaming execution pipelines with multiple operators
//! chained together to verify end-to-end functionality.

use graphdb_query::core::types::expr::Expression;
use graphdb_query::core::Value;
use graphdb_query::query::executor::base::{MemoryBudget, MemoryTracker};
use graphdb_query::query::executor::streaming::executor::StreamingExecutor;
use graphdb_query::query::executor::streaming::operators::base::OperatorBase;
use graphdb_query::query::executor::streaming::operators::blocking::{
    BlockingOperator, BlockingOperatorKind,
};
use graphdb_query::query::executor::streaming::operators::join_operator::{
    HashJoinBuildSide, JoinOperator, JoinOperatorKind,
};
use graphdb_query::query::executor::streaming::operators::set_operator::{
    SetOperator, SetOperatorKind,
};
use graphdb_query::query::executor::streaming::operators::source_operator::{
    SourceOperator, SourceOperatorKind,
};
use graphdb_query::query::executor::streaming::operators::spec::BuildSide;
use graphdb_query::query::executor::streaming::operators::unary_operator::{
    UnaryOperator, UnaryOperatorKind,
};
use graphdb_query::query::executor::streaming::slot::SlotLayout;
use std::sync::Arc;

// ====== Test Helpers ======

fn empty_layout() -> Arc<SlotLayout> {
    Arc::new(SlotLayout::new(vec![]))
}

fn create_simple_scan(size: usize) -> StreamingExecutor {
    let buffer = (0..size)
        .map(|i| vec![Value::Int(i as i32), Value::string(format!("item_{}", i))])
        .collect();

    StreamingExecutor::Source(
        OperatorBase::new(0),
        SourceOperator::new(
            SourceOperatorKind::ScanVertices {
                buffer,
                current_index: 0,
                col_names: vec![],
            },
            empty_layout(),
        ),
    )
}

fn create_scan_with_data(data: Vec<Vec<Value>>) -> StreamingExecutor {
    StreamingExecutor::Source(
        OperatorBase::new(0),
        SourceOperator::new(
            SourceOperatorKind::ScanVertices {
                buffer: data,
                current_index: 0,
                col_names: vec![],
            },
            empty_layout(),
        ),
    )
}

// ====== Integration Test Cases ======

#[test]
fn test_filter_then_limit_pipeline() {
    let scan = create_simple_scan(100);

    let filter = StreamingExecutor::Unary(
        OperatorBase::new(0),
        Box::new(scan),
        UnaryOperator::new(
            UnaryOperatorKind::Filter {
                predicate: Expression::Literal(Value::Bool(true)),
                state: Default::default(),
            },
            empty_layout(),
        ),
    );

    let mut pipeline = StreamingExecutor::Unary(
        OperatorBase::new(0),
        Box::new(filter),
        UnaryOperator::new(
            UnaryOperatorKind::Limit {
                offset: 0,
                limit: 10,
                skipped: 0,
                consumed: 0,
            },
            empty_layout(),
        ),
    );

    pipeline.open().unwrap();
    let chunk = pipeline.advance().unwrap();
    assert!(chunk.is_some());
    let mut chunk = chunk.unwrap();
    // P2: Limit returns a compact chunk (selection vector). Materialize to
    // count the visible rows an API consumer would observe (the engine
    // materializes at the root).
    chunk.materialize_selection();
    assert_eq!(chunk.len(), 10);
    pipeline.close().unwrap();
}

#[test]
fn test_project_then_distinct_pipeline() {
    let data = vec![
        vec![Value::Int(1), Value::string("a")],
        vec![Value::Int(1), Value::string("a")],
        vec![Value::Int(2), Value::string("b")],
    ];

    let scan = create_scan_with_data(data);

    let project = StreamingExecutor::Unary(
        OperatorBase::new(0),
        Box::new(scan),
        UnaryOperator::new(
            UnaryOperatorKind::Project {
                output_expressions: vec![Expression::Literal(Value::Int(0))],
                output_col_names: vec![],
                state: Default::default(),
            },
            empty_layout(),
        ),
    );

    let mut pipeline = StreamingExecutor::Blocking(
        OperatorBase::new(0),
        Box::new(project),
        BlockingOperator::new(
            BlockingOperatorKind::Distinct {
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                state: None,
            },
            empty_layout(),
        ),
    );

    pipeline.open().unwrap();
    let chunk = pipeline.advance().unwrap();
    assert!(chunk.is_some());
    pipeline.close().unwrap();
}

#[test]
fn test_join_with_small_inputs() {
    let left = create_scan_with_data(vec![
        vec![Value::Int(1), Value::string("a")],
        vec![Value::Int(2), Value::string("b")],
    ]);

    let right = create_scan_with_data(vec![
        vec![Value::Int(1), Value::string("a")],
        vec![Value::Int(2), Value::string("b")],
    ]);

    let mut join = StreamingExecutor::Join(
        OperatorBase::new(0),
        Box::new(left),
        Box::new(right),
        JoinOperator::new(
            JoinOperatorKind::HashJoin {
                join_condition: None,
                hash_keys: vec![],
                probe_keys: vec![],
                build_side: HashJoinBuildSide::new(),
                build_done: false,
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                right_col_names: vec![],
                build_side_select: BuildSide::Left,
            },
            empty_layout(),
        ),
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
        SetOperator::new(
            SetOperatorKind::Union {
                seen_rows: std::collections::HashSet::new(),
                left_consumed: false,
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            },
            empty_layout(),
        ),
    );

    let mut pipeline = StreamingExecutor::Unary(
        OperatorBase::new(0),
        Box::new(union),
        UnaryOperator::new(
            UnaryOperatorKind::Limit {
                offset: 0,
                limit: 2,
                skipped: 0,
                consumed: 0,
            },
            empty_layout(),
        ),
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
        vec![Value::Int(1), Value::string("a")],
        vec![Value::Int(2), Value::string("b")],
        vec![Value::Int(3), Value::string("c")],
    ]);

    let right = create_scan_with_data(vec![vec![Value::Int(2), Value::string("b")]]);

    let except = StreamingExecutor::Set(
        OperatorBase::new(0),
        Box::new(left),
        Box::new(right),
        SetOperator::new(
            SetOperatorKind::Except {
                exclude_rows: std::collections::HashSet::new(),
                right_buffered: false,
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            },
            empty_layout(),
        ),
    );

    let mut pipeline = StreamingExecutor::Unary(
        OperatorBase::new(0),
        Box::new(except),
        UnaryOperator::new(
            UnaryOperatorKind::Filter {
                predicate: Expression::Literal(Value::Bool(true)),
                state: Default::default(),
            },
            empty_layout(),
        ),
    );

    pipeline.open().unwrap();
    let chunk = pipeline.advance().unwrap();
    assert!(chunk.is_some());
    assert_eq!(chunk.unwrap().len(), 2);
    pipeline.close().unwrap();
}
