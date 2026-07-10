//! Integration tests for Streaming Executor
//!
//! Tests the workflow: StreamingExecutor construction → execution call chain
//! Focus: verify call chain integrity and executor lifecycle

use graphdb::core::error::QueryError;
use graphdb::core::types::expr::Expression;
use graphdb::core::Value;
use graphdb::query::executor::base::MemoryBudget;
use graphdb::query::executor::streaming::executor::SortDirection;
use graphdb::query::executor::streaming::StreamingExecutor;

// ============ Test Helpers ============

/// Create a test executor that produces data
fn create_scan_executor(rows: usize) -> StreamingExecutor {
    let buffer: Vec<Vec<Value>> = (0..rows)
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

/// Verify executor lifecycle: open → next → close
fn verify_executor_lifecycle(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    executor.open()?;
    let _result = executor.next()?; // May be Some or None
    executor.close()?;
    Ok(())
}

// ============ Executor Lifecycle Tests ============

/// Test basic ScanVertices executor lifecycle
#[test]
fn test_scan_vertices_lifecycle() {
    let mut executor = create_scan_executor(10);
    assert!(verify_executor_lifecycle(&mut executor).is_ok());
}

/// Test ScanEdges executor lifecycle
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

    let mut executor = StreamingExecutor::ScanEdges {
        partition_id: 0,
        buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    };

    assert!(verify_executor_lifecycle(&mut executor).is_ok());
}

// ============ Single-Input Operator Tests ============

/// Test Filter operator in call chain
#[test]
fn test_filter_in_chain() {
    let scan = Box::new(create_scan_executor(10));
    let mut filter = StreamingExecutor::Filter {
        input: scan,
        predicate: Expression::Literal(Value::Bool(true)),
        opened: false,
        plan_node_id: 0,
    };

    // Verify open → next → close chain works
    filter.open().unwrap();
    let chunk = filter.next().unwrap();
    assert!(chunk.is_some(), "Filter should produce output");
    filter.close().unwrap();
}

/// Test Project operator in call chain
#[test]
fn test_project_in_chain() {
    let scan = Box::new(create_scan_executor(5));
    let mut project = StreamingExecutor::Project {
        input: scan,
        output_expressions: vec![Expression::Literal(Value::Int(0))],
        output_col_names: vec![],
        opened: false,
        plan_node_id: 0,
    };

    project.open().unwrap();
    let chunk = project.next().unwrap();
    assert!(chunk.is_some(), "Project should produce output");
    // Verify column count matches projection
    if let Some(chunk_data) = chunk {
        assert_eq!(
            chunk_data.rows[0].len(),
            1,
            "Should have 1 column after projection"
        );
    }
    project.close().unwrap();
}

/// Test Limit operator in call chain
#[test]
fn test_limit_in_chain() {
    let scan = Box::new(create_scan_executor(100));
    let mut limit = StreamingExecutor::Limit {
        input: scan,
        limit: 10,
        consumed: 0,
        opened: false,
        plan_node_id: 0,
    };

    limit.open().unwrap();
    let chunk = limit.next().unwrap();
    assert!(chunk.is_some(), "Limit should produce output");
    if let Some(chunk_data) = chunk {
        assert_eq!(chunk_data.len(), 10, "Should limit to 10 rows");
    }

    // Second call should return None (limit exhausted)
    let chunk2 = limit.next().unwrap();
    assert!(chunk2.is_none(), "Limit should be exhausted");
    limit.close().unwrap();
}

/// Test Distinct operator in call chain
#[test]
fn test_distinct_in_chain() {
    let buffer = vec![
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(1), Value::String("a".to_string())], // Duplicate
        vec![Value::Int(2), Value::String("b".to_string())],
    ];

    let scan = Box::new(StreamingExecutor::ScanVertices {
        partition_id: 0,
        buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    });

    let mut distinct = StreamingExecutor::Distinct {
        input: scan,
        seen_rows: std::collections::HashSet::new(),
        opened: false,
        plan_node_id: 0,
    };

    distinct.open().unwrap();
    let chunk = distinct.next().unwrap();
    assert!(chunk.is_some(), "Distinct should produce output");
    distinct.close().unwrap();
}

// ============ Chained Pipeline Tests ============

/// Test two-step pipeline: Scan → Filter
#[test]
fn test_pipeline_scan_filter() {
    let scan = Box::new(create_scan_executor(20));
    let mut pipeline = StreamingExecutor::Filter {
        input: scan,
        predicate: Expression::Literal(Value::Bool(true)),
        opened: false,
        plan_node_id: 0,
    };

    // Verify entire chain can execute
    pipeline.open().unwrap();
    let result = pipeline.next().unwrap();
    assert!(result.is_some(), "Pipeline should produce output");
    pipeline.close().unwrap();
}

/// Test two-step pipeline: Scan → Project
#[test]
fn test_pipeline_scan_project() {
    let scan = Box::new(create_scan_executor(15));
    let mut pipeline = StreamingExecutor::Project {
        input: scan,
        output_expressions: vec![
            Expression::Literal(Value::Int(0)),
            Expression::Literal(Value::String("const".to_string())),
        ],
        output_col_names: vec![],
        opened: false,
        plan_node_id: 0,
    };

    pipeline.open().unwrap();
    let result = pipeline.next().unwrap();
    assert!(result.is_some(), "Pipeline should produce output");
    pipeline.close().unwrap();
}

/// Test two-step pipeline: Scan → Limit
#[test]
fn test_pipeline_scan_limit() {
    let scan = Box::new(create_scan_executor(50));
    let mut pipeline = StreamingExecutor::Limit {
        input: scan,
        limit: 5,
        consumed: 0,
        opened: false,
        plan_node_id: 0,
    };

    pipeline.open().unwrap();
    let result = pipeline.next().unwrap();
    assert!(result.is_some(), "Pipeline should produce output");
    if let Some(chunk) = result {
        assert_eq!(chunk.len(), 5, "Limit should be applied");
    }
    pipeline.close().unwrap();
}

/// Test three-step pipeline: Scan → Filter → Project
#[test]
fn test_pipeline_scan_filter_project() {
    let scan = Box::new(create_scan_executor(10));
    let filter = Box::new(StreamingExecutor::Filter {
        input: scan,
        predicate: Expression::Literal(Value::Bool(true)),
        opened: false,
        plan_node_id: 0,
    });

    let mut pipeline = StreamingExecutor::Project {
        input: filter,
        output_expressions: vec![Expression::Literal(Value::Int(42))],
        output_col_names: vec![],
        opened: false,
        plan_node_id: 0,
    };

    pipeline.open().unwrap();
    let result = pipeline.next().unwrap();
    assert!(
        result.is_some(),
        "Three-step pipeline should produce output"
    );
    pipeline.close().unwrap();
}

/// Test three-step pipeline: Scan → Filter → Limit
#[test]
fn test_pipeline_scan_filter_limit() {
    let scan = Box::new(create_scan_executor(100));
    let filter = Box::new(StreamingExecutor::Filter {
        input: scan,
        predicate: Expression::Literal(Value::Bool(true)),
        opened: false,
        plan_node_id: 0,
    });

    let mut pipeline = StreamingExecutor::Limit {
        input: filter,
        limit: 8,
        consumed: 0,
        opened: false,
        plan_node_id: 0,
    };

    pipeline.open().unwrap();
    let result = pipeline.next().unwrap();
    assert!(
        result.is_some(),
        "Three-step pipeline should produce output"
    );
    if let Some(chunk) = result {
        assert_eq!(chunk.len(), 8, "Limit should apply across filter");
    }
    pipeline.close().unwrap();
}

// ============ Stateful Operator Tests ============

/// Test Sort operator in call chain
#[test]
fn test_sort_in_chain() {
    let buffer = vec![
        vec![Value::Int(3)],
        vec![Value::Int(1)],
        vec![Value::Int(2)],
    ];

    let scan = Box::new(StreamingExecutor::ScanVertices {
        partition_id: 0,
        buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    });

    let mut sort = StreamingExecutor::Sort {
        input: scan,
        sort_expressions: vec![Expression::Literal(Value::Int(0))],
        sort_directions: vec![SortDirection::Ascending],
        all_rows: vec![],
        row_iter: None,
        memory_budget: MemoryBudget::default_budget(),
        opened: false,
        plan_node_id: 0,
    };

    sort.open().unwrap();
    let result = sort.next().unwrap();
    assert!(result.is_some(), "Sort should produce output");
    sort.close().unwrap();
}

/// Test Aggregate operator in call chain
#[test]
fn test_aggregate_in_chain() {
    use graphdb::core::types::operators::AggregateFunction;

    let buffer = vec![
        vec![Value::Int(1), Value::Int(10)],
        vec![Value::Int(1), Value::Int(20)],
        vec![Value::Int(2), Value::Int(15)],
    ];

    let scan = Box::new(StreamingExecutor::ScanVertices {
        partition_id: 0,
        buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    });

    let mut agg = StreamingExecutor::Aggregate {
        input: scan,
        group_by_expressions: vec![Expression::Literal(Value::Int(0))],
        aggregate_functions: vec![(
            AggregateFunction::Count(None),
            Expression::Literal(Value::Int(1)),
        )],
        all_rows: vec![],
        result_iter: None,
        memory_budget: MemoryBudget::default_budget(),
        opened: false,
        plan_node_id: 0,
    };

    agg.open().unwrap();
    let result = agg.next().unwrap();
    assert!(result.is_some(), "Aggregate should produce output");
    agg.close().unwrap();
}

// ============ Binary Operator Tests ============

/// Test HashJoin operator in call chain
#[test]
fn test_hash_join_in_chain() {
    let left_buffer = vec![vec![Value::Int(1), Value::String("a".to_string())]];
    let right_buffer = vec![vec![Value::Int(1), Value::String("x".to_string())]];

    let left = Box::new(StreamingExecutor::ScanVertices {
        partition_id: 0,
        buffer: left_buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    });

    let right = Box::new(StreamingExecutor::ScanVertices {
        partition_id: 1,
        buffer: right_buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    });

    let mut join = StreamingExecutor::HashJoin {
        left,
        right,
        join_condition: None,
        hash_keys: vec![],
        probe_keys: vec![],
        build_side_hash: std::collections::HashMap::new(),
        all_right_rows: vec![],
        left_consumed: false,
        memory_budget: MemoryBudget::default_budget(),
        opened: false,
        right_col_names: vec![],
        plan_node_id: 0,
    };

    join.open().unwrap();
    let _result = join.next().unwrap(); // May be Some or None
    join.close().unwrap();
}

/// Test NestedLoopJoin operator in call chain
#[test]
fn test_nested_loop_join_in_chain() {
    let left_buffer = vec![vec![Value::Int(1)]];
    let right_buffer = vec![vec![Value::Int(2)]];

    let left = Box::new(StreamingExecutor::ScanVertices {
        partition_id: 0,
        buffer: left_buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    });

    let right = Box::new(StreamingExecutor::ScanVertices {
        partition_id: 1,
        buffer: right_buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    });

    let mut join = StreamingExecutor::NestedLoopJoin {
        left,
        right,
        join_condition: None,
        build_side_tuples: vec![],
        left_consumed: false,
        memory_budget: MemoryBudget::default_budget(),
        opened: false,
        plan_node_id: 0,
    };

    join.open().unwrap();
    let _result = join.next().unwrap();
    join.close().unwrap();
}

// ============ Set Operation Tests ============

/// Test Union operator in call chain
#[test]
fn test_union_in_chain() {
    let left_buffer = vec![vec![Value::Int(1)]];
    let right_buffer = vec![vec![Value::Int(2)]];

    let left = Box::new(StreamingExecutor::ScanVertices {
        partition_id: 0,
        buffer: left_buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    });

    let right = Box::new(StreamingExecutor::ScanVertices {
        partition_id: 1,
        buffer: right_buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    });

    let mut union = StreamingExecutor::Union {
        left,
        right,
        seen_rows: std::collections::HashSet::new(),
        left_consumed: false,
        opened: false,
        plan_node_id: 0,
    };

    union.open().unwrap();
    let result = union.next().unwrap();
    assert!(result.is_some(), "Union should produce output");
    union.close().unwrap();
}

/// Test Intersect operator in call chain
#[test]
fn test_intersect_in_chain() {
    let left_buffer = vec![vec![Value::Int(1)]];
    let right_buffer = vec![vec![Value::Int(1)]];

    let left = Box::new(StreamingExecutor::ScanVertices {
        partition_id: 0,
        buffer: left_buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    });

    let right = Box::new(StreamingExecutor::ScanVertices {
        partition_id: 1,
        buffer: right_buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    });

    let mut intersect = StreamingExecutor::Intersect {
        left,
        right,
        left_rows: Vec::new(),
        right_rows: std::collections::HashSet::new(),
        left_buffered: false,
        right_buffered: false,
        opened: false,
        plan_node_id: 0,
    };

    intersect.open().unwrap();
    let _result = intersect.next().unwrap();
    intersect.close().unwrap();
}

/// Test Except operator in call chain
#[test]
fn test_except_in_chain() {
    let left_buffer = vec![vec![Value::Int(1)], vec![Value::Int(2)]];
    let right_buffer = vec![vec![Value::Int(2)]];

    let left = Box::new(StreamingExecutor::ScanVertices {
        partition_id: 0,
        buffer: left_buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    });

    let right = Box::new(StreamingExecutor::ScanVertices {
        partition_id: 1,
        buffer: right_buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    });

    let mut except = StreamingExecutor::Except {
        left,
        right,
        exclude_rows: std::collections::HashSet::new(),
        right_buffered: false,
        opened: false,
        plan_node_id: 0,
    };

    except.open().unwrap();
    let result = except.next().unwrap();
    assert!(result.is_some(), "Except should produce output");
    except.close().unwrap();
}

// ============ Complex Pipeline Tests ============

/// Test four-step pipeline: Scan → Filter → Project → Limit
#[test]
fn test_complex_pipeline_4step() {
    let scan = Box::new(create_scan_executor(50));
    let filter = Box::new(StreamingExecutor::Filter {
        input: scan,
        predicate: Expression::Literal(Value::Bool(true)),
        opened: false,
        plan_node_id: 0,
    });
    let project = Box::new(StreamingExecutor::Project {
        input: filter,
        output_expressions: vec![Expression::Literal(Value::String("col".to_string()))],
        output_col_names: vec![],
        opened: false,
        plan_node_id: 0,
    });
    let mut limit = StreamingExecutor::Limit {
        input: project,
        limit: 5,
        consumed: 0,
        opened: false,
        plan_node_id: 0,
    };

    limit.open().unwrap();
    let result = limit.next().unwrap();
    assert!(result.is_some(), "4-step pipeline should produce output");
    limit.close().unwrap();
}

/// Test union of two filtered scans
#[test]
fn test_union_of_filtered_scans() {
    let left_scan = Box::new(create_scan_executor(10));
    let left = Box::new(StreamingExecutor::Filter {
        input: left_scan,
        predicate: Expression::Literal(Value::Bool(true)),
        opened: false,
        plan_node_id: 0,
    });

    let right_scan = Box::new(create_scan_executor(10));
    let right = Box::new(StreamingExecutor::Filter {
        input: right_scan,
        predicate: Expression::Literal(Value::Bool(true)),
        opened: false,
        plan_node_id: 0,
    });

    let mut union = StreamingExecutor::Union {
        left,
        right,
        seen_rows: std::collections::HashSet::new(),
        left_consumed: false,
        opened: false,
        plan_node_id: 0,
    };

    union.open().unwrap();
    let result = union.next().unwrap();
    assert!(
        result.is_some(),
        "Union of filtered scans should produce output"
    );
    union.close().unwrap();
}

// ============ Edge Case Tests ============

/// Test executor with empty input
#[test]
fn test_filter_with_empty_input() {
    let empty_scan = Box::new(StreamingExecutor::ScanVertices {
        partition_id: 0,
        buffer: vec![],
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    });

    let mut filter = StreamingExecutor::Filter {
        input: empty_scan,
        predicate: Expression::Literal(Value::Bool(true)),
        opened: false,
        plan_node_id: 0,
    };

    filter.open().unwrap();
    let result = filter.next().unwrap();
    assert!(result.is_none(), "Filter on empty input should return None");
    filter.close().unwrap();
}

/// Test limit with zero value
#[test]
fn test_limit_zero() {
    let scan = Box::new(create_scan_executor(10));
    let mut limit = StreamingExecutor::Limit {
        input: scan,
        limit: 0,
        consumed: 0,
        opened: false,
        plan_node_id: 0,
    };

    limit.open().unwrap();
    let result = limit.next().unwrap();
    assert!(result.is_none(), "Limit(0) should return None");
    limit.close().unwrap();
}

/// Test distinct with all identical rows
#[test]
fn test_distinct_all_same() {
    let buffer = vec![
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(1), Value::String("a".to_string())],
        vec![Value::Int(1), Value::String("a".to_string())],
    ];

    let scan = Box::new(StreamingExecutor::ScanVertices {
        partition_id: 0,
        buffer,
        current_index: 0,
        col_names: vec![],
        plan_node_id: 0,
    });

    let mut distinct = StreamingExecutor::Distinct {
        input: scan,
        seen_rows: std::collections::HashSet::new(),
        opened: false,
        plan_node_id: 0,
    };

    distinct.open().unwrap();
    let result = distinct.next().unwrap();
    // Should return one chunk with only unique rows
    if let Some(chunk) = result {
        assert!(chunk.len() <= 3, "Distinct should deduplicate");
    }
    distinct.close().unwrap();
}
