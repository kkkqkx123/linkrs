//! Integration tests for Streaming Executor
//!
//! Tests the workflow: StreamingExecutor construction → execution call chain
//! Focus: verify call chain integrity and executor lifecycle

use graphdb::core::error::QueryError;
use graphdb::core::types::expr::Expression;
use graphdb::core::types::operators::AggregateFunction;
use graphdb::core::Value;
use graphdb::query::executor::base::{MemoryBudget, MemoryTracker};
use graphdb::query::executor::streaming::executor::SortDirection;
use graphdb::query::executor::streaming::executor::StreamingExecutor;
use graphdb::query::executor::streaming::operators::base::OperatorBase;
use graphdb::query::executor::streaming::operators::blocking_operator::BlockingOperator;
use graphdb::query::executor::streaming::operators::join_operator::JoinOperator;
use graphdb::query::executor::streaming::operators::set_operator::SetOperator;
use graphdb::query::executor::streaming::operators::source_operator::SourceOperator;
use graphdb::query::executor::streaming::operators::unary_operator::UnaryOperator;

mod common;

// ============ Test Helpers ============

fn create_scan_executor(rows: usize) -> StreamingExecutor {
    let buffer: Vec<Vec<Value>> = (0..rows)
        .map(|i| vec![Value::Int(i as i32), Value::String(format!("item_{}", i))])
        .collect();

    StreamingExecutor::Source(
        OperatorBase::new(0),
        SourceOperator::ScanVertices {
            buffer,
            current_index: 0,
            col_names: vec![],
        },
    )
}

fn scan_vertices(data: Vec<Vec<Value>>) -> StreamingExecutor {
    StreamingExecutor::Source(
        OperatorBase::new(0),
        SourceOperator::ScanVertices {
            buffer: data,
            current_index: 0,
            col_names: vec![],
        },
    )
}

fn verify_executor_lifecycle(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    executor.open()?;
    let _result = executor.advance()?;
    executor.close()?;
    Ok(())
}

// ============ Executor Lifecycle Tests ============

#[test]
fn test_scan_vertices_lifecycle() {
    let mut executor = create_scan_executor(10);
    assert!(verify_executor_lifecycle(&mut executor).is_ok());
}

#[test]
fn test_scan_edges_lifecycle() {
    let buffer = vec![
        vec![
            Value::Int(1),
            Value::Int(2),
            Value::String("edge".to_string()),
        ],
        vec![
            Value::Int(2),
            Value::Int(3),
            Value::String("edge".to_string()),
        ],
    ];
    let mut executor = StreamingExecutor::Source(
        OperatorBase::new(0),
        SourceOperator::ScanEdges {
            buffer,
            current_index: 0,
            col_names: vec![],
        },
    );
    assert!(verify_executor_lifecycle(&mut executor).is_ok());
}

// ============ Single-Input Operator Tests ============

#[test]
fn test_filter_in_chain() {
    let scan = Box::new(create_scan_executor(10));
    let mut filter = StreamingExecutor::Unary(
        OperatorBase::new(0),
        scan,
        UnaryOperator::Filter {
            predicate: Expression::Literal(Value::Bool(true)),
        },
    );
    filter.open().unwrap();
    let chunk = filter.advance().unwrap();
    assert!(chunk.is_some());
    filter.close().unwrap();
}

#[test]
fn test_project_in_chain() {
    let scan = Box::new(create_scan_executor(5));
    let mut project = StreamingExecutor::Unary(
        OperatorBase::new(0),
        scan,
        UnaryOperator::Project {
            output_expressions: vec![Expression::Literal(Value::Int(0))],
            output_col_names: vec![],
        },
    );
    project.open().unwrap();
    let chunk = project.advance().unwrap();
    assert!(chunk.is_some());
    if let Some(ref chunk_data) = chunk {
        assert_eq!(chunk_data.rows[0].len(), 1);
    }
    project.close().unwrap();
}

#[test]
fn test_limit_in_chain() {
    let scan = Box::new(create_scan_executor(100));
    let mut limit = StreamingExecutor::Unary(
        OperatorBase::new(0),
        scan,
        UnaryOperator::Limit {
            offset: 0,
            limit: 10,
            skipped: 0,
            consumed: 0,
        },
    );
    limit.open().unwrap();
    let chunk = limit.advance().unwrap();
    assert!(chunk.is_some());
    if let Some(ref chunk_data) = chunk {
        assert_eq!(chunk_data.len(), 10);
    }
    let chunk2 = limit.advance().unwrap();
    assert!(chunk2.is_none());
    limit.close().unwrap();
}

#[test]
fn test_distinct_in_chain() {
    let buffer = vec![
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(2), Value::String("b".to_string())],
    ];
    let scan = Box::new(scan_vertices(buffer));

    let mut distinct = StreamingExecutor::Blocking(
        OperatorBase::new(0),
        scan,
        BlockingOperator::Distinct {
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            state: None,
        },
    );
    distinct.open().unwrap();
    let chunk = distinct.advance().unwrap();
    assert!(chunk.is_some());
    distinct.close().unwrap();
}

// ============ Chained Pipeline Tests ============

#[test]
fn test_pipeline_scan_filter() {
    let scan = Box::new(create_scan_executor(20));
    let mut pipeline = StreamingExecutor::Unary(
        OperatorBase::new(0),
        scan,
        UnaryOperator::Filter {
            predicate: Expression::Literal(Value::Bool(true)),
        },
    );
    pipeline.open().unwrap();
    let result = pipeline.advance().unwrap();
    assert!(result.is_some());
    pipeline.close().unwrap();
}

#[test]
fn test_pipeline_scan_project() {
    let scan = Box::new(create_scan_executor(15));
    let mut pipeline = StreamingExecutor::Unary(
        OperatorBase::new(0),
        scan,
        UnaryOperator::Project {
            output_expressions: vec![
                Expression::Literal(Value::Int(0)),
                Expression::Literal(Value::String("const".to_string())),
            ],
            output_col_names: vec![],
        },
    );
    pipeline.open().unwrap();
    let result = pipeline.advance().unwrap();
    assert!(result.is_some());
    pipeline.close().unwrap();
}

#[test]
fn test_pipeline_scan_limit() {
    let scan = Box::new(create_scan_executor(50));
    let mut pipeline = StreamingExecutor::Unary(
        OperatorBase::new(0),
        scan,
        UnaryOperator::Limit {
            offset: 0,
            limit: 5,
            skipped: 0,
            consumed: 0,
        },
    );
    pipeline.open().unwrap();
    let result = pipeline.advance().unwrap();
    assert!(result.is_some());
    if let Some(ref chunk) = result {
        assert_eq!(chunk.len(), 5);
    }
    pipeline.close().unwrap();
}

#[test]
fn test_pipeline_scan_filter_project() {
    let scan = Box::new(create_scan_executor(10));
    let filter = Box::new(StreamingExecutor::Unary(
        OperatorBase::new(0),
        scan,
        UnaryOperator::Filter {
            predicate: Expression::Literal(Value::Bool(true)),
        },
    ));
    let mut pipeline = StreamingExecutor::Unary(
        OperatorBase::new(0),
        filter,
        UnaryOperator::Project {
            output_expressions: vec![Expression::Literal(Value::Int(42))],
            output_col_names: vec![],
        },
    );
    pipeline.open().unwrap();
    let result = pipeline.advance().unwrap();
    assert!(result.is_some());
    pipeline.close().unwrap();
}

#[test]
fn test_pipeline_scan_filter_limit() {
    let scan = Box::new(create_scan_executor(100));
    let filter = Box::new(StreamingExecutor::Unary(
        OperatorBase::new(0),
        scan,
        UnaryOperator::Filter {
            predicate: Expression::Literal(Value::Bool(true)),
        },
    ));
    let mut pipeline = StreamingExecutor::Unary(
        OperatorBase::new(0),
        filter,
        UnaryOperator::Limit {
            offset: 0,
            limit: 8,
            skipped: 0,
            consumed: 0,
        },
    );
    pipeline.open().unwrap();
    let result = pipeline.advance().unwrap();
    assert!(result.is_some());
    if let Some(ref chunk) = result {
        assert_eq!(chunk.len(), 8);
    }
    pipeline.close().unwrap();
}

// ============ Stateful Operator Tests ============

#[test]
fn test_sort_in_chain() {
    let buffer = vec![
        vec![Value::Int(3)],
        vec![Value::Int(1)],
        vec![Value::Int(2)],
    ];
    let scan = Box::new(scan_vertices(buffer));

    let mut sort = StreamingExecutor::Blocking(
        OperatorBase::new(0),
        scan,
        BlockingOperator::Sort {
            sort_expressions: vec![Expression::Literal(Value::Int(0))],
            sort_directions: vec![SortDirection::Ascending],
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            state: None,
        },
    );
    sort.open().unwrap();
    let result = sort.advance().unwrap();
    assert!(result.is_some());
    sort.close().unwrap();
}

#[test]
fn test_aggregate_in_chain() {
    let buffer = vec![
        vec![Value::Int(1), Value::Int(10)],
        vec![Value::Int(1), Value::Int(20)],
        vec![Value::Int(2), Value::Int(15)],
    ];
    let scan = Box::new(scan_vertices(buffer));

    let mut agg = StreamingExecutor::Blocking(
        OperatorBase::new(0),
        scan,
        BlockingOperator::Aggregate {
            group_by_expressions: vec![Expression::Literal(Value::Int(0))],
            aggregate_functions: vec![(
                AggregateFunction::Count(None),
                Expression::Literal(Value::Int(1)),
            )],
            output_col_names: vec![],
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            state: None,
        },
    );
    agg.open().unwrap();
    let result = agg.advance().unwrap();
    assert!(result.is_some());
    agg.close().unwrap();
}

// ============ Binary Operator Tests ============

#[test]
fn test_hash_join_in_chain() {
    let left = Box::new(scan_vertices(vec![vec![
        Value::Int(1),
        Value::String("a".to_string()),
    ]]));
    let right = Box::new(scan_vertices(vec![vec![
        Value::Int(1),
        Value::String("x".to_string()),
    ]]));

    let mut join = StreamingExecutor::Join(
        OperatorBase::new(0),
        left,
        right,
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
    let _result = join.advance().unwrap();
    join.close().unwrap();
}

#[test]
fn test_nested_loop_join_in_chain() {
    let left = Box::new(scan_vertices(vec![vec![Value::Int(1)]]));
    let right = Box::new(scan_vertices(vec![vec![Value::Int(2)]]));

    let mut join = StreamingExecutor::Join(
        OperatorBase::new(0),
        left,
        right,
        JoinOperator::NestedLoopJoin {
            join_condition: None,
            build_side_tuples: vec![],
            left_consumed: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            right_col_names: vec![],
        },
    );
    join.open().unwrap();
    let _result = join.advance().unwrap();
    join.close().unwrap();
}

// ============ Set Operation Tests ============

#[test]
fn test_union_in_chain() {
    let left = Box::new(scan_vertices(vec![vec![Value::Int(1)]]));
    let right = Box::new(scan_vertices(vec![vec![Value::Int(2)]]));

    let mut union = StreamingExecutor::Set(
        OperatorBase::new(0),
        left,
        right,
        SetOperator::Union {
            seen_rows: std::collections::HashSet::new(),
            left_consumed: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
        },
    );
    union.open().unwrap();
    let result = union.advance().unwrap();
    assert!(result.is_some());
    union.close().unwrap();
}

#[test]
fn test_intersect_in_chain() {
    let left = Box::new(scan_vertices(vec![vec![Value::Int(1)]]));
    let right = Box::new(scan_vertices(vec![vec![Value::Int(1)]]));

    let mut intersect = StreamingExecutor::Set(
        OperatorBase::new(0),
        left,
        right,
        SetOperator::Intersect {
            left_rows: Vec::new(),
            right_rows: std::collections::HashSet::new(),
            left_buffered: false,
            right_buffered: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
        },
    );
    intersect.open().unwrap();
    let _result = intersect.advance().unwrap();
    intersect.close().unwrap();
}

#[test]
fn test_except_in_chain() {
    let left = Box::new(scan_vertices(vec![
        vec![Value::Int(1)],
        vec![Value::Int(2)],
    ]));
    let right = Box::new(scan_vertices(vec![vec![Value::Int(2)]]));

    let mut except = StreamingExecutor::Set(
        OperatorBase::new(0),
        left,
        right,
        SetOperator::Except {
            exclude_rows: std::collections::HashSet::new(),
            right_buffered: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
        },
    );
    except.open().unwrap();
    let result = except.advance().unwrap();
    assert!(result.is_some());
    except.close().unwrap();
}

// ============ Complex Pipeline Tests ============

#[test]
fn test_complex_pipeline_4step() {
    let scan = Box::new(create_scan_executor(50));
    let filter = Box::new(StreamingExecutor::Unary(
        OperatorBase::new(0),
        scan,
        UnaryOperator::Filter {
            predicate: Expression::Literal(Value::Bool(true)),
        },
    ));
    let project = Box::new(StreamingExecutor::Unary(
        OperatorBase::new(0),
        filter,
        UnaryOperator::Project {
            output_expressions: vec![Expression::Literal(Value::String("col".to_string()))],
            output_col_names: vec![],
        },
    ));
    let mut limit = StreamingExecutor::Unary(
        OperatorBase::new(0),
        project,
        UnaryOperator::Limit {
            offset: 0,
            limit: 5,
            skipped: 0,
            consumed: 0,
        },
    );
    limit.open().unwrap();
    let result = limit.advance().unwrap();
    assert!(result.is_some());
    limit.close().unwrap();
}

#[test]
fn test_union_of_filtered_scans() {
    let left_scan = Box::new(create_scan_executor(10));
    let left = Box::new(StreamingExecutor::Unary(
        OperatorBase::new(0),
        left_scan,
        UnaryOperator::Filter {
            predicate: Expression::Literal(Value::Bool(true)),
        },
    ));
    let right_scan = Box::new(create_scan_executor(10));
    let right = Box::new(StreamingExecutor::Unary(
        OperatorBase::new(0),
        right_scan,
        UnaryOperator::Filter {
            predicate: Expression::Literal(Value::Bool(true)),
        },
    ));

    let mut union = StreamingExecutor::Set(
        OperatorBase::new(0),
        left,
        right,
        SetOperator::Union {
            seen_rows: std::collections::HashSet::new(),
            left_consumed: false,
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
        },
    );
    union.open().unwrap();
    let result = union.advance().unwrap();
    assert!(result.is_some());
    union.close().unwrap();
}

// ============ Edge Case Tests ============

#[test]
fn test_filter_with_empty_input() {
    let empty_scan = Box::new(scan_vertices(vec![]));
    let mut filter = StreamingExecutor::Unary(
        OperatorBase::new(0),
        empty_scan,
        UnaryOperator::Filter {
            predicate: Expression::Literal(Value::Bool(true)),
        },
    );
    filter.open().unwrap();
    let result = filter.advance().unwrap();
    assert!(result.is_none());
    filter.close().unwrap();
}

#[test]
fn test_limit_zero() {
    let scan = Box::new(create_scan_executor(10));
    let mut limit = StreamingExecutor::Unary(
        OperatorBase::new(0),
        scan,
        UnaryOperator::Limit {
            offset: 0,
            limit: 0,
            skipped: 0,
            consumed: 0,
        },
    );
    limit.open().unwrap();
    let result = limit.advance().unwrap();
    assert!(result.is_none());
    limit.close().unwrap();
}

#[test]
fn test_distinct_all_same() {
    let buffer = vec![
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(1), Value::String("a".to_string())],
    ];
    let scan = Box::new(scan_vertices(buffer));

    let mut distinct = StreamingExecutor::Blocking(
        OperatorBase::new(0),
        scan,
        BlockingOperator::Distinct {
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            state: None,
        },
    );
    distinct.open().unwrap();
    let result = distinct.advance().unwrap();
    if let Some(ref chunk) = result {
        assert!(chunk.len() <= 3);
    }
    distinct.close().unwrap();
}

// ── Storage-backed integration tests (R5) ──

#[cfg(test)]
mod storage_backed {
    use super::common::TestStorage;
    use graphdb::core::stats::StatsManager;
    use graphdb::core::types::VertexId;
    use graphdb::core::types::{PropertyDef, SpaceInfo, TagInfo};
    use graphdb::core::vertex_edge_path::{Tag, Vertex};
    use graphdb::core::DataType;
    use graphdb::core::Value;
    use graphdb::query::executor::streaming::StreamingQueryResult;
    use graphdb::query::optimizer::OptimizerEngine;
    use graphdb::query::query_pipeline_manager::QueryPipelineManager;
    use graphdb::query::QueryRequestContext;
    use graphdb::storage::{
        StorageReader, StorageSchemaContextOps, StorageSchemaOps, StorageWriter,
    };
    use parking_lot::RwLock;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Set up a minimal graph space with a Person tag and a few vertices.
    fn setup_test_data(storage: &Arc<RwLock<graphdb::storage::GraphStorage>>) {
        let mut store = storage.write();
        let mut space = SpaceInfo::new("test".to_string()).with_vid_type(DataType::BigInt);
        store.create_space(&mut space).unwrap();
        let tag = TagInfo::new("Person".to_string()).with_properties(vec![
            PropertyDef::new("name".to_string(), DataType::String),
            PropertyDef::new("age".to_string(), DataType::BigInt),
        ]);
        store.create_tag("test", &tag).unwrap();
        for (i, (name, age)) in [
            ("Alice", 30i64),
            ("Bob", 25),
            ("Charlie", 35),
            ("Diana", 28),
        ]
        .iter()
        .enumerate()
        {
            let vid = VertexId::from_int64(i as i64 + 1);
            let mut props = HashMap::new();
            props.insert("name".to_string(), Value::String(name.to_string()));
            props.insert("age".to_string(), Value::BigInt(*age));
            let vertex = Vertex::new(vid, vec![Tag::new("Person".to_string(), props)]);
            store.insert_vertex("test", vertex).unwrap();
        }
    }

    #[test]
    fn test_streaming_query_with_storage_backed_scan() {
        let test_storage = TestStorage::new().expect("Failed to create test storage");
        let storage = test_storage.storage();

        let stats_manager = Arc::new(StatsManager::new());
        let schema_manager = {
            let guard = storage.read();
            StorageSchemaContextOps::get_schema_manager(&*guard)
                .expect("Schema manager not available")
        };
        let mut pipeline = QueryPipelineManager::with_optimizer(
            storage.clone(),
            stats_manager,
            Arc::new(OptimizerEngine::default()),
        )
        .with_schema_manager(schema_manager);

        setup_test_data(&storage);

        let rctx = Arc::new(QueryRequestContext::new(
            "MATCH (n:Person) RETURN n.name, n.age ORDER BY n.age".to_string(),
        ));
        let space_info = {
            let store = storage.read();
            store.get_space("test").unwrap()
        };

        let result: StreamingQueryResult = pipeline
            .execute_query_stream_with_request(
                "MATCH (n:Person) RETURN n.name, n.age ORDER BY n.age",
                rctx,
                space_info,
            )
            .expect("Streaming query should succeed");

        let mut total_rows = 0;
        let mut names: Vec<String> = Vec::new();
        while let Ok(Some(chunk)) = result.next_chunk() {
            for row in &chunk.rows {
                if let (Some(Value::String(name)), Some(Value::BigInt(_age))) =
                    (row.first(), row.get(1))
                {
                    names.push(name.clone());
                }
            }
            total_rows += chunk.len();
        }

        assert_eq!(total_rows, 4, "Expected 4 Person vertices");
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["Alice", "Bob", "Charlie", "Diana"]);
    }
}
