use super::*;
use crate::core::Value;
use crate::query::executor::base::MemoryBudget;
use crate::query::executor::streaming::executor::SortDirection;
use crate::query::executor::streaming::operators::source_operator::SourceOperator;
use crate::query::planning::plan::PartitionSpec;

fn scan_executor(
    rows: Vec<Vec<Value>>,
    _partition_id: usize,
    col_names: Vec<String>,
) -> StreamingExecutor {
    StreamingExecutor::Source(
        OperatorBase::new(0),
        SourceOperator::ScanVertices {
            buffer: rows,
            current_index: 0,
            col_names,
        },
    )
}

// ── Partial + Final Aggregate integration tests ──

#[test]
fn test_partial_aggregate_operator_counts_rows_per_partition() {
    let mut partial = StreamingExecutor::Blocking(
        OperatorBase::new(1),
        Box::new(scan_executor(
            vec![
                vec![Value::BigInt(1), Value::BigInt(10)],
                vec![Value::BigInt(2), Value::BigInt(20)],
            ],
            0,
            vec!["id".to_string(), "amount".to_string()],
        )),
        BlockingOperator::PartialAggregate {
            group_by_expressions: Vec::new(),
            aggregate_functions: vec![
                AggregateFunction::Count(None),
                AggregateFunction::Sum("amount".to_string()),
            ],
            output_col_names: vec!["COUNT".to_string(), "SUM".to_string()],
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            state: None,
        },
    );
    partial.open().expect("partial open");
    let chunk = partial.advance().expect("partial advance");
    partial.close().expect("partial close");

    let chunk = chunk.expect("partial should produce a chunk");
    assert_eq!(chunk.rows.len(), 1, "should have one group row");
    assert_eq!(chunk.rows[0].len(), 2);
    match &chunk.rows[0][0] {
        Value::BigInt(n) => assert_eq!(*n, 2),
        _ => panic!("expected BigInt for Count"),
    }
    match &chunk.rows[0][1] {
        Value::Double(n) => assert!((*n - 30.0).abs() < 1e-10),
        _ => panic!("expected Double for Sum"),
    }
}

#[test]
fn test_final_aggregate_merges_partial_results() {
    let p0 = StreamingExecutor::Blocking(
        OperatorBase::new(1),
        Box::new(scan_executor(
            vec![vec![Value::BigInt(1)], vec![Value::BigInt(2)]],
            0,
            vec!["val".to_string()],
        )),
        BlockingOperator::PartialAggregate {
            group_by_expressions: Vec::new(),
            aggregate_functions: vec![
                AggregateFunction::Count(None),
                AggregateFunction::Sum("val".to_string()),
            ],
            output_col_names: vec!["COUNT".to_string(), "SUM".to_string()],
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            state: None,
        },
    );
    let p1 = StreamingExecutor::Blocking(
        OperatorBase::new(1),
        Box::new(scan_executor(
            vec![vec![Value::BigInt(3)], vec![Value::BigInt(4)]],
            1,
            vec!["val".to_string()],
        )),
        BlockingOperator::PartialAggregate {
            group_by_expressions: Vec::new(),
            aggregate_functions: vec![
                AggregateFunction::Count(None),
                AggregateFunction::Sum("val".to_string()),
            ],
            output_col_names: vec!["COUNT".to_string(), "SUM".to_string()],
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            state: None,
        },
    );

    let mut final_agg = StreamingExecutor::Blocking(
        OperatorBase::new(3).with_global(true),
        Box::new(StreamingExecutor::Gather(
            OperatorBase::new(2).with_global(true),
            vec![p0, p1],
            GatherOperator::concatenate(),
        )),
        BlockingOperator::FinalAggregate {
            group_by_expressions: Vec::new(),
            aggregate_functions: vec![
                AggregateFunction::Count(None),
                AggregateFunction::Sum("val".to_string()),
            ],
            output_col_names: vec!["COUNT".to_string(), "SUM".to_string()],
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            state: None,
        },
    );

    final_agg.open().expect("final open");
    let chunk = final_agg.advance().expect("final advance");
    final_agg.close().expect("final close");

    let chunk = chunk.expect("final should produce a chunk");
    assert_eq!(chunk.rows.len(), 1, "should have one result row");
    assert_eq!(chunk.rows[0][0], Value::BigInt(4), "COUNT should be 4");
    match &chunk.rows[0][1] {
        Value::Double(n) => assert!((*n - 10.0).abs() < 1e-10, "SUM should be ~10"),
        Value::BigInt(n) => assert_eq!(*n, 10, "SUM should be 10"),
        other => panic!("expected numeric for SUM, got {:?}", other),
    }
}

#[test]
fn test_partial_aggregate_with_group_keys() {
    let mut partial = StreamingExecutor::Blocking(
        OperatorBase::new(1),
        Box::new(scan_executor(
            vec![
                vec![Value::String("a".to_string()), Value::BigInt(10)],
                vec![Value::String("a".to_string()), Value::BigInt(20)],
                vec![Value::String("b".to_string()), Value::BigInt(30)],
            ],
            0,
            vec!["group".to_string(), "amount".to_string()],
        )),
        BlockingOperator::PartialAggregate {
            group_by_expressions: vec![Expression::Variable("group".to_string())],
            aggregate_functions: vec![
                AggregateFunction::Count(None),
                AggregateFunction::Sum("amount".to_string()),
            ],
            output_col_names: vec!["group".to_string(), "COUNT".to_string(), "SUM".to_string()],
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            state: None,
        },
    );
    partial.open().expect("partial open");
    let chunk = partial.advance().expect("partial advance");
    partial.close().expect("partial close");

    let chunk = chunk.expect("partial should produce rows");
    assert_eq!(chunk.rows.len(), 2, "should have two group rows");
    let mut by_group: std::collections::HashMap<String, &Vec<Value>> =
        std::collections::HashMap::new();
    for row in &chunk.rows {
        let key = match &row[0] {
            Value::String(s) => s.clone(),
            _ => panic!("expected String group key"),
        };
        by_group.insert(key, row);
    }
    let row_a = by_group.get("a").expect("group 'a' should exist");
    let row_b = by_group.get("b").expect("group 'b' should exist");
    assert_eq!(row_a[1], Value::BigInt(2), "group a COUNT should be 2");
    assert_eq!(row_b[1], Value::BigInt(1), "group b COUNT should be 1");
}

#[test]
fn test_aggregate_split_via_partitioned_physical_plan() {
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
    use crate::query::planning::plan::{PartitionSource, PartitionedPhysicalPlan};

    let start = StartNode::new();
    let agg = AggregateNode::new(
        PlanNodeEnum::Start(start),
        vec![],
        vec![
            AggregateFunction::Count(None),
            AggregateFunction::Sum("amount".to_string()),
        ],
    )
    .expect("aggregate plan should build");
    let spec = PartitionSpec::try_new(
        vec![0..10, 10..20],
        PartitionSource::VertexId {
            tag: "test".to_string(),
        },
        None,
    )
    .expect("valid spec");
    let physical = PartitionedPhysicalPlan::from_logical(PlanNodeEnum::Aggregate(agg), spec);

    assert!(
        matches!(
            physical.root(),
            PartitionedPhysicalNode::AggregateSplit { .. }
        ),
        "Expected AggregateSplit, got {:?}",
        physical.root()
    );
}

#[test]
fn test_two_level_distinct_eliminates_duplicates_across_partitions() {
    let p0 = scan_executor(
        vec![
            vec![Value::BigInt(1)],
            vec![Value::BigInt(1)],
            vec![Value::BigInt(2)],
        ],
        0,
        vec!["val".to_string()],
    );
    let p1 = scan_executor(
        vec![
            vec![Value::BigInt(2)],
            vec![Value::BigInt(3)],
            vec![Value::BigInt(3)],
        ],
        1,
        vec!["val".to_string()],
    );

    let local_distincts: Vec<StreamingExecutor> = vec![p0, p1]
        .into_iter()
        .map(|tree| {
            StreamingExecutor::Blocking(
                OperatorBase::new(1),
                Box::new(tree),
                BlockingOperator::Distinct {
                    memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                    state: None,
                },
            )
        })
        .collect();

    let mut global_distinct = StreamingExecutor::Blocking(
        OperatorBase::new(3).with_global(true),
        Box::new(StreamingExecutor::Gather(
            OperatorBase::new(2).with_global(true),
            local_distincts,
            GatherOperator::concatenate(),
        )),
        BlockingOperator::Distinct {
            memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
            state: None,
        },
    );

    global_distinct.open().expect("distinct pipeline open");
    let chunk = global_distinct.advance().expect("distinct advance");
    global_distinct.close().expect("distinct close");

    let chunk = chunk.expect("distinct should produce rows");
    let mut values: Vec<i64> = chunk
        .rows
        .iter()
        .filter_map(|row| match row.first() {
            Some(Value::BigInt(n)) => Some(*n),
            _ => None,
        })
        .collect();
    values.sort();
    assert_eq!(
        values,
        vec![1, 2, 3],
        "two-level distinct should produce [1, 2, 3]"
    );
}

#[test]
fn test_two_level_topn_keeps_top_across_partitions() {
    let p0 = scan_executor(
        vec![
            vec![Value::BigInt(5)],
            vec![Value::BigInt(3)],
            vec![Value::BigInt(1)],
        ],
        0,
        vec!["val".to_string()],
    );
    let p1 = scan_executor(
        vec![
            vec![Value::BigInt(4)],
            vec![Value::BigInt(2)],
            vec![Value::BigInt(6)],
        ],
        1,
        vec!["val".to_string()],
    );

    let limit: u32 = 3;
    let sort_expressions = vec![Expression::Variable("val".to_string())];
    let sort_directions = vec![SortDirection::Ascending];

    let local_topns: Vec<StreamingExecutor> = vec![p0, p1]
        .into_iter()
        .map(|tree| {
            StreamingExecutor::Blocking(
                OperatorBase::new(1),
                Box::new(tree),
                BlockingOperator::TopN {
                    n: limit,
                    sort_expressions: sort_expressions.clone(),
                    sort_directions: sort_directions.clone(),
                    memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                    state: None,
                },
            )
        })
        .collect();

    let mut executor = StreamingExecutor::Gather(
        OperatorBase::new(2).with_global(true),
        local_topns,
        GatherOperator::merge_sort(sort_expressions, sort_directions, Some(limit as usize)),
    );

    executor.open().expect("topn pipeline open");
    let chunk = executor.advance().expect("topn advance");
    executor.close().expect("topn close");

    let chunk = chunk.expect("topn should produce rows");
    let values: Vec<i64> = chunk
        .rows
        .iter()
        .filter_map(|row| match row.first() {
            Some(Value::BigInt(n)) => Some(*n),
            _ => None,
        })
        .collect();
    assert_eq!(
        values,
        vec![1, 2, 3],
        "two-level TopN(3) should produce [1, 2, 3]"
    );
}

#[test]
fn test_two_level_topn_preserves_order_with_larger_limit() {
    let p0 = scan_executor(
        vec![vec![Value::BigInt(10)], vec![Value::BigInt(30)]],
        0,
        vec!["val".to_string()],
    );
    let p1 = scan_executor(
        vec![vec![Value::BigInt(20)], vec![Value::BigInt(40)]],
        1,
        vec!["val".to_string()],
    );

    let limit: u32 = 4;
    let sort_expressions = vec![Expression::Variable("val".to_string())];
    let sort_directions = vec![SortDirection::Ascending];

    let local_topns: Vec<StreamingExecutor> = vec![p0, p1]
        .into_iter()
        .map(|tree| {
            StreamingExecutor::Blocking(
                OperatorBase::new(1),
                Box::new(tree),
                BlockingOperator::TopN {
                    n: limit,
                    sort_expressions: sort_expressions.clone(),
                    sort_directions: sort_directions.clone(),
                    memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                    state: None,
                },
            )
        })
        .collect();

    let mut executor = StreamingExecutor::Gather(
        OperatorBase::new(2).with_global(true),
        local_topns,
        GatherOperator::merge_sort(sort_expressions, sort_directions, Some(limit as usize)),
    );

    executor.open().expect("topn pipeline open");
    let chunk = executor.advance().expect("topn advance");
    executor.close().expect("topn close");

    let chunk = chunk.expect("topn should produce rows");
    let values: Vec<i64> = chunk
        .rows
        .iter()
        .filter_map(|row| match row.first() {
            Some(Value::BigInt(n)) => Some(*n),
            _ => None,
        })
        .collect();
    assert_eq!(
        values,
        vec![10, 20, 30, 40],
        "two-level TopN(4) should produce all sorted values"
    );
}

// ── HashShuffleJoin integration tests ──

fn make_hash_shuffle_join(
    left_trees: Vec<StreamingExecutor>,
    right_trees: Vec<StreamingExecutor>,
    join_kind: HashJoinKind,
    left_key: &str,
    right_key: &str,
    left_schema: Vec<String>,
    right_schema: Vec<String>,
    bucket_count: usize,
) -> StreamingExecutor {
    let left_key_expr = if left_key.is_empty() {
        vec![]
    } else {
        vec![Expression::Variable(left_key.to_string())]
    };
    let right_key_expr = if right_key.is_empty() {
        vec![]
    } else {
        vec![Expression::Variable(right_key.to_string())]
    };
    let operator = HashShuffleJoinOperator::new(
        join_kind,
        left_key_expr,
        right_key_expr,
        None,
        bucket_count,
        left_schema,
        right_schema,
        MemoryTracker::new(MemoryBudget::default_budget()),
    );

    let node_id = 100;
    StreamingExecutor::HashShuffleJoin(
        OperatorBase::new(node_id).with_global(true),
        left_trees,
        right_trees,
        operator,
    )
}

#[test]
fn test_hash_shuffle_inner_join_matches_across_partitions() {
    let p0 = scan_executor(
        vec![
            vec![Value::BigInt(1), Value::String("a".to_string())],
            vec![Value::BigInt(2), Value::String("b".to_string())],
        ],
        0,
        vec!["id".to_string(), "val".to_string()],
    );
    let p1 = scan_executor(
        vec![vec![Value::BigInt(3), Value::String("c".to_string())]],
        1,
        vec!["id".to_string(), "val".to_string()],
    );

    let r0 = scan_executor(
        vec![
            vec![Value::BigInt(1), Value::BigInt(100)],
            vec![Value::BigInt(3), Value::BigInt(300)],
        ],
        0,
        vec!["id".to_string(), "score".to_string()],
    );
    let r1 = scan_executor(
        vec![vec![Value::BigInt(2), Value::BigInt(200)]],
        1,
        vec!["id".to_string(), "score".to_string()],
    );

    let mut join = make_hash_shuffle_join(
        vec![p0, p1],
        vec![r0, r1],
        HashJoinKind::Inner,
        "id",
        "id",
        vec!["id".to_string(), "val".to_string()],
        vec!["id".to_string(), "score".to_string()],
        4,
    );

    fn collect_all(join: &mut StreamingExecutor) -> Vec<Vec<Value>> {
        let mut all_rows = Vec::new();
        while let Ok(Some(chunk)) = join.advance() {
            all_rows.extend(chunk.rows);
        }
        all_rows
    }

    join.open().expect("hash shuffle inner join open");
    let mut rows = collect_all(&mut join);
    join.close().expect("hash shuffle inner join close");

    rows.sort_by(|a, b| {
        let a_id = match a.first() {
            Some(Value::BigInt(n)) => *n,
            _ => 0,
        };
        let b_id = match b.first() {
            Some(Value::BigInt(n)) => *n,
            _ => 0,
        };
        a_id.cmp(&b_id)
    });
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[0],
        vec![
            Value::BigInt(1),
            Value::String("a".to_string()),
            Value::BigInt(1),
            Value::BigInt(100)
        ]
    );
    assert_eq!(
        rows[1],
        vec![
            Value::BigInt(2),
            Value::String("b".to_string()),
            Value::BigInt(2),
            Value::BigInt(200)
        ]
    );
    assert_eq!(
        rows[2],
        vec![
            Value::BigInt(3),
            Value::String("c".to_string()),
            Value::BigInt(3),
            Value::BigInt(300)
        ]
    );
}

#[test]
fn test_hash_shuffle_left_join_pads_nulls_for_unmatched() {
    fn collect_all(join: &mut StreamingExecutor) -> Vec<Vec<Value>> {
        let mut all_rows = Vec::new();
        while let Ok(Some(chunk)) = join.advance() {
            all_rows.extend(chunk.rows);
        }
        all_rows
    }
    let p0 = scan_executor(
        vec![vec![Value::BigInt(1)], vec![Value::BigInt(2)]],
        0,
        vec!["id".to_string()],
    );
    let p1 = scan_executor(vec![vec![Value::BigInt(3)]], 1, vec!["id".to_string()]);

    let r0 = scan_executor(
        vec![vec![Value::BigInt(1), Value::BigInt(100)]],
        0,
        vec!["id".to_string(), "score".to_string()],
    );
    let r1 = scan_executor(vec![], 1, vec!["id".to_string(), "score".to_string()]);

    let mut join = make_hash_shuffle_join(
        vec![p0, p1],
        vec![r0, r1],
        HashJoinKind::Left,
        "id",
        "id",
        vec!["id".to_string()],
        vec!["id".to_string(), "score".to_string()],
        4,
    );

    join.open().expect("hash shuffle left join open");
    let mut rows = collect_all(&mut join);
    join.close().expect("hash shuffle left join close");

    rows.sort_by(|a, b| {
        let a_id = match a.first() {
            Some(Value::BigInt(n)) => *n,
            _ => 0,
        };
        let b_id = match b.first() {
            Some(Value::BigInt(n)) => *n,
            _ => 0,
        };
        a_id.cmp(&b_id)
    });
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[0],
        vec![Value::BigInt(1), Value::BigInt(1), Value::BigInt(100)]
    );
    assert_eq!(
        rows[1],
        vec![
            Value::BigInt(2),
            Value::Null(crate::core::value::NullType::Null),
            Value::Null(crate::core::value::NullType::Null)
        ]
    );
    assert_eq!(
        rows[2],
        vec![
            Value::BigInt(3),
            Value::Null(crate::core::value::NullType::Null),
            Value::Null(crate::core::value::NullType::Null)
        ]
    );
}

#[test]
fn test_hash_shuffle_inner_join_empty_left_partition() {
    fn collect_all(join: &mut StreamingExecutor) -> Vec<Vec<Value>> {
        let mut all_rows = Vec::new();
        while let Ok(Some(chunk)) = join.advance() {
            all_rows.extend(chunk.rows);
        }
        all_rows
    }
    let p0 = scan_executor(vec![], 0, vec!["id".to_string()]);
    let p1 = scan_executor(vec![vec![Value::BigInt(1)]], 1, vec!["id".to_string()]);
    let r0 = scan_executor(
        vec![vec![Value::BigInt(1), Value::BigInt(100)]],
        0,
        vec!["id".to_string(), "score".to_string()],
    );
    let r1 = scan_executor(vec![], 1, vec!["id".to_string(), "score".to_string()]);

    let mut join = make_hash_shuffle_join(
        vec![p0, p1],
        vec![r0, r1],
        HashJoinKind::Inner,
        "id",
        "id",
        vec!["id".to_string()],
        vec!["id".to_string(), "score".to_string()],
        4,
    );

    join.open().expect("empty left join open");
    let rows = collect_all(&mut join);
    join.close().expect("empty left join close");

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0],
        vec![Value::BigInt(1), Value::BigInt(1), Value::BigInt(100)]
    );
}

#[test]
fn test_hash_shuffle_join_duplicate_keys() {
    let p0 = scan_executor(
        vec![
            vec![Value::BigInt(1), Value::String("a".to_string())],
            vec![Value::BigInt(1), Value::String("b".to_string())],
        ],
        0,
        vec!["id".to_string(), "val".to_string()],
    );
    let p1 = scan_executor(
        vec![vec![Value::BigInt(2), Value::String("c".to_string())]],
        1,
        vec!["id".to_string(), "val".to_string()],
    );

    let r0 = scan_executor(
        vec![vec![Value::BigInt(1), Value::BigInt(100)]],
        0,
        vec!["id".to_string(), "score".to_string()],
    );
    let r1 = scan_executor(vec![], 1, vec!["id".to_string(), "score".to_string()]);

    let mut join = make_hash_shuffle_join(
        vec![p0, p1],
        vec![r0, r1],
        HashJoinKind::Inner,
        "id",
        "id",
        vec!["id".to_string(), "val".to_string()],
        vec!["id".to_string(), "score".to_string()],
        4,
    );

    fn collect_all(join: &mut StreamingExecutor) -> Vec<Vec<Value>> {
        let mut all_rows = Vec::new();
        while let Ok(Some(chunk)) = join.advance() {
            all_rows.extend(chunk.rows);
        }
        all_rows
    }
    join.open().expect("duplicate keys join open");
    let rows = collect_all(&mut join);
    join.close().expect("duplicate keys join close");

    assert_eq!(rows.len(), 2);
    for row in &rows {
        match row.first() {
            Some(Value::BigInt(1)) => {}
            _ => panic!("Expected all rows to have id=1"),
        }
    }
}

// ── Cross-chunk boundary tests for HashShuffleJoin ────────────

#[test]
fn hash_shuffle_join_emits_all_matches_when_single_left_row_exceeds_chunk() {
    let n_right = 1025;
    let left = scan_executor(
        vec![vec![Value::BigInt(1), Value::String("left".to_string())]],
        0,
        vec!["id".to_string(), "val".to_string()],
    );
    let right_rows: Vec<Vec<Value>> = (0..n_right)
        .map(|i| vec![Value::BigInt(1), Value::BigInt(i as i64)])
        .collect();
    let right = scan_executor(right_rows, 0, vec!["id".to_string(), "score".to_string()]);

    let mut join = make_hash_shuffle_join(
        vec![left],
        vec![right],
        HashJoinKind::Inner,
        "id",
        "id",
        vec!["id".to_string(), "val".to_string()],
        vec!["id".to_string(), "score".to_string()],
        1,
    );

    fn collect_all(join: &mut StreamingExecutor) -> Vec<Vec<Value>> {
        let mut all_rows = Vec::new();
        while let Ok(Some(chunk)) = join.advance() {
            all_rows.extend(chunk.rows);
        }
        all_rows
    }

    join.open().expect("cross-chunk join open");
    let rows = collect_all(&mut join);
    join.close().expect("cross-chunk join close");

    assert_eq!(
        rows.len(),
        n_right,
        "must produce all {} matches, got {}",
        n_right,
        rows.len()
    );
    for row in &rows {
        assert_eq!(row[0], Value::BigInt(1), "all rows must have id=1");
    }
}

#[test]
fn hash_shuffle_join_cross_chunk_left_join_pads_nulls_for_unmatched_left() {
    // Left rows: [id=1, id=2]; Right rows: [id=1]
    // Left join on id: id=1 should match, id=2 should be NULL-padded
    let left = scan_executor(
        vec![
            vec![Value::BigInt(1), Value::String("matched".to_string())],
            vec![Value::BigInt(2), Value::String("lonely".to_string())],
        ],
        0,
        vec!["id".to_string(), "val".to_string()],
    );
    let right = scan_executor(
        vec![vec![Value::BigInt(1), Value::BigInt(100)]],
        0,
        vec!["id".to_string(), "score".to_string()],
    );

    let mut join = make_hash_shuffle_join(
        vec![left],
        vec![right],
        HashJoinKind::Left,
        "id",
        "id",
        vec!["id".to_string(), "val".to_string()],
        vec!["id".to_string(), "score".to_string()],
        1,
    );

    fn collect_all(join: &mut StreamingExecutor) -> Vec<Vec<Value>> {
        let mut all_rows = Vec::new();
        while let Ok(Some(chunk)) = join.advance() {
            all_rows.extend(chunk.rows);
        }
        all_rows
    }

    join.open().expect("cross-chunk left join open");
    let rows = collect_all(&mut join);
    join.close().expect("cross-chunk left join close");

    assert_eq!(rows.len(), 2, "expected 2 rows, got {:?}", rows);
    let null = Value::Null(crate::core::value::NullType::Null);
    assert!(
        rows.iter()
            .any(|r| r[0] == Value::BigInt(1) && r[3] == Value::BigInt(100)),
        "expected row with id=1 and score=100, got {:?}",
        rows
    );
    assert!(rows
        .iter()
        .any(|r| r[0] == Value::BigInt(2) && r[3] == null));
}

#[test]
fn hash_shuffle_join_cross_join_no_keys_produces_cartesian_product() {
    let n_left = 3;
    let n_right = 300;
    let left_rows: Vec<Vec<Value>> = (0..n_left).map(|i| vec![Value::BigInt(i as i64)]).collect();
    let left = scan_executor(left_rows, 0, vec!["id".to_string()]);
    let right_rows: Vec<Vec<Value>> = (0..n_right)
        .map(|i| vec![Value::BigInt(100 + i as i64)])
        .collect();
    let right = scan_executor(right_rows, 0, vec!["rid".to_string()]);

    let mut join = make_hash_shuffle_join(
        vec![left],
        vec![right],
        HashJoinKind::Inner,
        "",
        "",
        vec!["id".to_string()],
        vec!["rid".to_string()],
        1,
    );

    fn collect_all(join: &mut StreamingExecutor) -> Vec<Vec<Value>> {
        let mut all_rows = Vec::new();
        while let Ok(Some(chunk)) = join.advance() {
            all_rows.extend(chunk.rows);
        }
        all_rows
    }

    join.open().expect("cross-join open");
    let rows = collect_all(&mut join);
    join.close().expect("cross-join close");

    assert_eq!(
        rows.len(),
        n_left * n_right,
        "cross join must produce {} rows",
        n_left * n_right
    );
}
