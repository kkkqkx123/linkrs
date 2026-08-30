use super::super::operators::base::OperatorBase;
use super::super::operators::gather_operator::GatherOperator;
use super::super::operators::join_operator::JoinOperator;
use super::super::operators::join_operator::JoinOperatorKind;
use super::super::operators::source_operator::SourceOperator;
use super::super::operators::source_operator::SourceOperatorKind;
use super::super::operators::spec::BuildSide;
use super::super::operators::unary_operator::UnaryOperator;
use super::super::operators::unary_operator::UnaryOperatorKind;
use super::super::slot::SlotLayout;
use super::*;
use graphdb_core::Value;

fn operator_base(plan_node_id: i64, col_names: &[String]) -> OperatorBase {
    OperatorBase::new(plan_node_id).with_output_layout(Arc::new(SlotLayout::from_names(col_names)))
}

fn create_test_buffer(count: usize) -> Vec<Vec<Value>> {
    (0..count)
        .map(|i| {
            vec![
                Value::BigInt(i as i64),
                Value::string(format!("item_{}", i)),
            ]
        })
        .collect()
}

fn scan_executor(rows: Vec<Vec<Value>>, col_names: Vec<String>) -> StreamingExecutor {
    StreamingExecutor::Source(
        operator_base(0, &col_names),
        SourceOperator::new(
            SourceOperatorKind::ScanVertices {
                buffer: rows,
                current_index: 0,
                col_names: col_names.clone(),
            },
            Arc::new(SlotLayout::from_names(&col_names)),
        ),
    )
}

#[test]
fn test_engine_creation() {
    let engine = StreamingExecutionEngine::new();
    assert!(engine.root_executor.is_none());
}

#[test]
fn test_engine_with_runtime() {
    let mut engine = StreamingExecutionEngine::new();
    let runtime = Arc::new(ExecutionRuntime::default_budget());
    engine.set_runtime(runtime);

    let buffer = create_test_buffer(50);
    let scan = scan_executor(buffer, vec![]);
    engine.register_executor(0, scan);

    let result = engine.execute_collected();
    assert!(result.is_ok());
    let chunks = result.unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].len(), 50);
}

#[test]
fn test_into_stream() {
    let mut engine = StreamingExecutionEngine::new();
    let runtime = Arc::new(ExecutionRuntime::default_budget());
    engine.set_runtime(runtime);

    let buffer = create_test_buffer(10);
    let scan = scan_executor(buffer, vec![]);
    engine.register_executor(0, scan);

    let mut stream = engine.into_stream().unwrap();
    let chunk = stream.next_chunk().unwrap();
    assert!(chunk.is_some());
    assert_eq!(chunk.unwrap().len(), 10);
    let done = stream.next_chunk().unwrap();
    assert!(done.is_none());
}

#[test]
fn test_cancel_during_execution() {
    let mut engine = StreamingExecutionEngine::new();
    let runtime = Arc::new(ExecutionRuntime::default_budget());
    engine.set_runtime(runtime.clone());

    let buffer = create_test_buffer(100);
    let scan = scan_executor(buffer, vec![]);
    engine.register_executor(0, scan);

    // Cancel before execution
    runtime.cancel();
    let result = engine.execute_collected();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("killed") || err.contains("cancelled"));
}

#[test]
fn test_stream_collect() {
    let mut engine = StreamingExecutionEngine::new();
    let runtime = Arc::new(ExecutionRuntime::default_budget());
    engine.set_runtime(runtime);

    let buffer = create_test_buffer(25);
    let scan = scan_executor(buffer, vec!["id".to_string(), "name".to_string()]);
    engine.register_executor(0, scan);

    let stream = engine.into_stream().unwrap();
    let ds = stream.collect().unwrap();
    assert_eq!(ds.row_count(), 25);
    assert_eq!(ds.col_count(), 2);
}

#[test]
fn test_single_scan_executor() {
    let mut engine = StreamingExecutionEngine::new();

    let buffer = create_test_buffer(100);
    let scan = scan_executor(buffer, vec![]);
    engine.register_executor(0, scan);

    let result = engine.execute_collected();
    assert!(result.is_ok());
    let chunks = result.unwrap();
    // 100 rows with chunk size 1024 → single chunk
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].len(), 100);
}

#[test]
fn test_filter_limit_pipeline() {
    let mut engine = StreamingExecutionEngine::new();

    let buffer = create_test_buffer(100);
    let scan = Box::new(scan_executor(buffer, vec![]));

    let limit = StreamingExecutor::Unary(
        OperatorBase::new(0),
        scan,
        UnaryOperator::new(
            UnaryOperatorKind::Limit {
                offset: 0,
                limit: 10,
                skipped: 0,
                consumed: 0,
            },
            Arc::new(SlotLayout::from_names(&[
                "id".to_string(),
                "name".to_string(),
            ])),
        ),
    );

    engine.register_executor(0, limit);

    let result = engine.execute_collected();
    assert!(result.is_ok());
    let chunks = result.unwrap();
    let total: usize = chunks.iter().map(|c| c.len()).sum();
    assert_eq!(total, 10);
}

#[test]
fn hash_join_skips_unmatched_probe_chunks_before_later_match() {
    use graphdb_core::types::expr::Expression;

    let left = StreamingExecutor::Source(
        operator_base(1, &["id".to_string()]).with_chunk_size(1),
        SourceOperator::new(
            SourceOperatorKind::ScanVertices {
                buffer: vec![vec![Value::BigInt(1)], vec![Value::BigInt(2)]],
                current_index: 0,
                col_names: vec!["id".to_string()],
            },
            Arc::new(SlotLayout::from_names(&["id".to_string()])),
        ),
    );
    let right = StreamingExecutor::Source(
        operator_base(2, &["id".to_string()]).with_chunk_size(1),
        SourceOperator::new(
            SourceOperatorKind::ScanVertices {
                buffer: vec![vec![Value::BigInt(2)]],
                current_index: 0,
                col_names: vec!["id".to_string()],
            },
            Arc::new(SlotLayout::from_names(&["id".to_string()])),
        ),
    );
    let join = StreamingExecutor::Join(
        operator_base(3, &["left_id".to_string(), "right_id".to_string()]),
        Box::new(left),
        Box::new(right),
        JoinOperator::new(
            JoinOperatorKind::HashJoin {
                join_condition: None,
                hash_keys: vec![Expression::Variable("id".to_string())],
                probe_keys: vec![Expression::Variable("id".to_string())],
                build_side:
                    crate::executor::streaming::operators::join_operator::HashJoinBuildSide::new(),
                build_done: false,
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                right_col_names: Vec::new(),
                build_side_select: BuildSide::default(),
            },
            Arc::new(SlotLayout::from_names(&[
                "left_id".to_string(),
                "right_id".to_string(),
            ])),
        ),
    );

    let mut engine = StreamingExecutionEngine::new();
    engine.register_executor(0, join);
    let chunks = engine
        .execute_collected()
        .expect("hash join should execute");

    assert_eq!(
        chunks
            .iter()
            .flat_map(|chunk| chunk.rows.iter())
            .collect::<Vec<_>>(),
        vec![&vec![Value::BigInt(2), Value::BigInt(2)]]
    );
}

// ── Partition execution tests ──

fn partitioned_scan_executor(
    rows: Vec<Vec<Value>>,
    _partition_id: usize,
    col_names: Vec<String>,
) -> StreamingExecutor {
    StreamingExecutor::Source(
        operator_base(0, &col_names),
        SourceOperator::new(
            SourceOperatorKind::ScanVertices {
                buffer: rows,
                current_index: 0,
                col_names: col_names.clone(),
            },
            Arc::new(SlotLayout::from_names(&col_names)),
        ),
    )
}

/// Build a Concatenate gather whose output layout mirrors the first
/// partition tree (test helper for partitioned executor tests).
fn concatenate_gather(local_trees: &[StreamingExecutor]) -> GatherOperator {
    let layout = local_trees
        .first()
        .map(|tree| Arc::clone(&tree.base().output_layout))
        .unwrap_or_else(|| Arc::new(SlotLayout::new(vec![])));
    GatherOperator::concatenate(layout)
}

fn extract_ids(chunks: &[DataChunk]) -> Vec<i64> {
    chunks
        .iter()
        .flat_map(|c| c.rows.iter())
        .filter_map(|row| {
            row.first().and_then(|v| {
                if let Value::BigInt(id) = v {
                    Some(*id)
                } else {
                    None
                }
            })
        })
        .collect()
}

#[test]
fn test_partitioned_execution_two_partitions() {
    // 100 rows split into 2 partitions: 0-49 and 50-99
    let all_data = create_test_buffer(100);

    let p0_data: Vec<Vec<Value>> = all_data[0..50].to_vec();
    let p1_data: Vec<Vec<Value>> = all_data[50..100].to_vec();

    let mut engine = StreamingExecutionEngine::new();
    engine.register_partition_executors(vec![
        partitioned_scan_executor(p0_data, 0, vec![]),
        partitioned_scan_executor(p1_data, 1, vec![]),
    ]);

    let result = engine.execute_collected().unwrap();
    let total_rows: usize = result.iter().map(|c| c.len()).sum();
    assert_eq!(total_rows, 100);

    let ids = extract_ids(&result);
    assert_eq!(ids.len(), 100);
    // Verify all 0..99 are present
    for i in 0..100i64 {
        assert!(ids.contains(&i), "Missing id {}", i);
    }
}

#[test]
fn test_partitioned_execution_three_partitions() {
    let all_data = create_test_buffer(99);

    let p0_data: Vec<Vec<Value>> = all_data[0..33].to_vec();
    let p1_data: Vec<Vec<Value>> = all_data[33..66].to_vec();
    let p2_data: Vec<Vec<Value>> = all_data[66..99].to_vec();

    let mut engine = StreamingExecutionEngine::new();
    engine.register_partition_executors(vec![
        partitioned_scan_executor(p0_data, 0, vec![]),
        partitioned_scan_executor(p1_data, 1, vec![]),
        partitioned_scan_executor(p2_data, 2, vec![]),
    ]);

    let result = engine.execute_collected().unwrap();
    let total_rows: usize = result.iter().map(|c| c.len()).sum();
    assert_eq!(total_rows, 99);

    let ids = extract_ids(&result);
    assert_eq!(ids.len(), 99);
    for i in 0..99i64 {
        assert!(ids.contains(&i), "Missing id {}", i);
    }
}

#[test]
fn test_partitioned_execution_with_runtime() {
    let all_data = create_test_buffer(50);
    let p0_data: Vec<Vec<Value>> = all_data[0..25].to_vec();
    let p1_data: Vec<Vec<Value>> = all_data[25..50].to_vec();

    let mut engine = StreamingExecutionEngine::new();
    let runtime = Arc::new(ExecutionRuntime::default_budget());
    engine.set_runtime(runtime);
    engine.register_partition_executors(vec![
        partitioned_scan_executor(p0_data, 0, vec![]),
        partitioned_scan_executor(p1_data, 1, vec![]),
    ]);

    let result = engine.execute_collected().unwrap();
    let total_rows: usize = result.iter().map(|c| c.len()).sum();
    assert_eq!(total_rows, 50);
}

#[test]
fn test_partitioned_execution_equal_to_single() {
    // Verify that partitioned execution produces the same result as single execution
    let all_data = create_test_buffer(100);

    // Single execution
    let mut single_engine = StreamingExecutionEngine::new();
    single_engine.register_executor(0, scan_executor(all_data.clone(), vec![]));
    let single_result = single_engine.execute_collected().unwrap();
    let single_ids = extract_ids(&single_result);

    // Partitioned execution (4 partitions)
    let chunk_size = 25;
    let partition_executors: Vec<StreamingExecutor> = (0..4)
        .map(|p| {
            let start = p * chunk_size;
            let end = ((p + 1) * chunk_size).min(100);
            partitioned_scan_executor(all_data[start..end].to_vec(), p, vec![])
        })
        .collect();

    let mut part_engine = StreamingExecutionEngine::new();
    part_engine.register_partition_executors(partition_executors);
    let part_result = part_engine.execute_collected().unwrap();
    let part_ids = extract_ids(&part_result);

    // Both should contain all 0..99
    assert_eq!(single_ids.len(), part_ids.len());
    let mut sorted_single = single_ids.clone();
    let mut sorted_part = part_ids.clone();
    sorted_single.sort();
    sorted_part.sort();
    assert_eq!(sorted_single, sorted_part);
}

#[test]
fn test_partition_count() {
    let mut engine = StreamingExecutionEngine::new();
    assert_eq!(engine.partition_count(), 0);

    let all_data = create_test_buffer(10);
    engine.register_partition_executors(vec![
        partitioned_scan_executor(all_data[0..5].to_vec(), 0, vec![]),
        partitioned_scan_executor(all_data[5..10].to_vec(), 1, vec![]),
    ]);
    assert_eq!(engine.partition_count(), 2);
}

#[test]
fn gather_root_keeps_partition_count_and_separate_profiles() {
    let mut engine = StreamingExecutionEngine::new();
    let runtime = Arc::new(ExecutionRuntime::default_budget());
    engine.set_runtime(runtime.clone());
    let partitions = vec![
        partitioned_scan_executor(
            create_test_buffer(2),
            0,
            vec!["id".to_string(), "name".to_string()],
        ),
        partitioned_scan_executor(
            create_test_buffer(3),
            1,
            vec!["id".to_string(), "name".to_string()],
        ),
    ];
    let gather = concatenate_gather(&partitions);
    engine
        .build_partitioned_executor(partitions, gather, None)
        .expect("gather tree should be registered");

    assert_eq!(engine.partition_count(), 2);
    let chunks = engine
        .execute_collected()
        .expect("gather execution should succeed");
    assert_eq!(chunks.iter().map(DataChunk::len).sum::<usize>(), 5);

    let profile = runtime.profile().flush_to_collector();
    assert!(profile
        .operators
        .contains_key(&super::super::runtime::OperatorProfileKey::new(
            super::super::plan::types::PhysicalOperatorId(0),
            Some(0),
        )));
    assert!(profile
        .operators
        .contains_key(&super::super::runtime::OperatorProfileKey::new(
            super::super::plan::types::PhysicalOperatorId(0),
            Some(1),
        )));
    assert!(profile
        .operators
        .contains_key(&super::super::runtime::OperatorProfileKey::new(
            super::super::plan::types::PhysicalOperatorId(i64::MIN.unsigned_abs() as usize),
            None
        )));
}

#[test]
fn p8_parallel_gather_preserves_partition_order_and_bounds_buffers() {
    let runtime = Arc::new(ExecutionRuntime::default_budget());
    let mut engine = StreamingExecutionEngine::new();
    engine.set_runtime(runtime.clone());
    engine.set_max_workers(2);
    engine.set_max_buffered_chunks(1);
    let partitions = vec![
        partitioned_scan_executor(
            create_test_buffer(1_500),
            0,
            vec!["id".to_string(), "name".to_string()],
        ),
        partitioned_scan_executor(
            (1_500..3_000)
                .map(|value| {
                    vec![
                        Value::BigInt(value as i64),
                        Value::string(format!("item_{value}")),
                    ]
                })
                .collect(),
            1,
            vec!["id".to_string(), "name".to_string()],
        ),
    ];
    let gather = concatenate_gather(&partitions);
    engine
        .build_partitioned_executor(partitions, gather, None)
        .expect("build parallel gather");

    let chunks = engine.execute_collected().expect("parallel gather execute");
    assert_eq!(
        extract_ids(&chunks),
        (0..3_000).map(|value| value as i64).collect::<Vec<_>>()
    );

    let board = runtime.profile();
    assert_eq!(
        board
            .parallel_workers
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
    assert!(
        board
            .parallel_wall_time_us
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0
    );
    assert!(
        board
            .parallel_work_time_us
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0
    );
    assert!(
        board
            .parallel_buffered_chunks_peak
            .load(std::sync::atomic::Ordering::Relaxed)
            <= 2,
        "one bounded channel per partition must cap queued chunks"
    );
}

#[test]
fn p8_parallel_gather_cancellation_joins_workers() {
    let runtime = Arc::new(ExecutionRuntime::default_budget());
    let mut engine = StreamingExecutionEngine::new();
    engine.set_runtime(runtime.clone());
    engine.set_max_workers(2);
    engine.set_max_buffered_chunks(1);
    let partitions = vec![
        partitioned_scan_executor(
            create_test_buffer(5_000),
            0,
            vec!["id".to_string(), "name".to_string()],
        ),
        partitioned_scan_executor(
            create_test_buffer(5_000),
            1,
            vec!["id".to_string(), "name".to_string()],
        ),
    ];
    let gather = concatenate_gather(&partitions);
    engine
        .build_partitioned_executor(partitions, gather, None)
        .expect("build parallel gather");

    let mut stream = engine.into_stream().expect("create stream");
    assert!(stream.next_chunk().expect("first chunk").is_some());
    runtime.cancel();
    assert!(stream.next_chunk().is_err());
    assert!(stream.close().is_ok());
    assert!(
        runtime
            .profile()
            .parallel_workers
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0
    );
}

#[test]
fn p8_parallel_merge_gather_preserves_global_sort_order() {
    let runtime = Arc::new(ExecutionRuntime::default_budget());
    let mut engine = StreamingExecutionEngine::new();
    engine.set_runtime(runtime.clone());
    engine.set_max_workers(2);
    engine.set_max_buffered_chunks(1);
    let partitions = vec![
        partitioned_scan_executor(
            vec![vec![Value::BigInt(1)], vec![Value::BigInt(3)]],
            0,
            vec!["id".to_string()],
        ),
        partitioned_scan_executor(
            vec![vec![Value::BigInt(2)], vec![Value::BigInt(4)]],
            1,
            vec!["id".to_string()],
        ),
    ];
    let gather = GatherOperator::merge_sort(
        vec![graphdb_core::types::expr::Expression::Variable(
            "id".to_string(),
        )],
        vec![SortDirection::Ascending],
        None,
        Arc::clone(&partitions[0].base().output_layout),
    );
    engine
        .build_partitioned_executor(partitions, gather, None)
        .expect("build parallel merge gather");

    let chunks = engine
        .execute_collected()
        .expect("parallel merge gather execute");
    assert_eq!(extract_ids(&chunks), vec![1, 2, 3, 4]);
    assert_eq!(
        runtime
            .profile()
            .parallel_workers
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
}

#[test]
fn partitioned_sort_builds_local_sorts_before_merging() {
    let mut engine = StreamingExecutionEngine::new();
    engine
        .build_partitioned_sort_executor(
            vec![
                partitioned_scan_executor(
                    vec![vec![Value::BigInt(3)], vec![Value::BigInt(1)]],
                    0,
                    vec!["id".to_string()],
                ),
                partitioned_scan_executor(
                    vec![vec![Value::BigInt(4)], vec![Value::BigInt(2)]],
                    1,
                    vec!["id".to_string()],
                ),
            ],
            vec![graphdb_core::types::expr::Expression::Variable(
                "id".to_string(),
            )],
            vec![SortDirection::Ascending],
            Some(3),
        )
        .expect("partitioned sort should build");

    let chunks = engine
        .execute_collected()
        .expect("partitioned sort should execute");
    assert_eq!(extract_ids(&chunks), vec![1, 2, 3]);
    assert_eq!(chunks[0].col_names(), vec!["id"]);
}

#[test]
fn partitioned_aggregate_runs_once_after_gathering_all_partitions() {
    let mut engine = StreamingExecutionEngine::new();
    let result_layout = Arc::new(SlotLayout::from_names(&[
        "COUNT".to_string(),
        "SUM".to_string(),
    ]));
    let global = StreamingExecutor::Blocking(
        OperatorBase::new(40).with_output_layout(result_layout.clone()),
        Box::new(scan_executor(Vec::new(), vec!["amount".to_string()])),
        BlockingOperator::new(
            BlockingOperatorKind::Aggregate {
                group_by_expressions: Vec::new(),
                aggregate_functions: vec![
                    (
                        graphdb_core::types::operators::AggregateFunction::Count,
                        vec![graphdb_core::types::expr::Expression::Literal(Value::Int(
                            1,
                        ))],
                    ),
                    (
                        graphdb_core::types::operators::AggregateFunction::Sum,
                        vec![graphdb_core::types::expr::Expression::Variable(
                            "amount".to_string(),
                        )],
                    ),
                ],
                output_col_names: vec!["COUNT".to_string(), "SUM".to_string()],
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                state: None,
            },
            result_layout,
        ),
    );
    let partitions = vec![
        partitioned_scan_executor(
            vec![vec![Value::BigInt(1)], vec![Value::BigInt(2)]],
            0,
            vec!["amount".to_string()],
        ),
        partitioned_scan_executor(
            vec![vec![Value::BigInt(3)], vec![Value::BigInt(4)]],
            1,
            vec!["amount".to_string()],
        ),
    ];
    let gather = concatenate_gather(&partitions);
    engine
        .build_partitioned_executor(partitions, gather, Some(global))
        .expect("partitioned aggregate tree should build");

    let chunks = engine
        .execute_collected()
        .expect("partitioned aggregate should execute");
    assert_eq!(chunks.len(), 1);
    assert_eq!(
        chunks[0].rows,
        vec![vec![Value::BigInt(4), Value::BigInt(10)]]
    );
    assert_eq!(chunks[0].col_names(), vec!["COUNT", "SUM"]);
}

#[test]
fn partitioned_dedup_removes_duplicates_across_partitions() {
    let mut engine = StreamingExecutionEngine::new();
    let result_layout = Arc::new(SlotLayout::from_names(&["id".to_string()]));
    let global = StreamingExecutor::Blocking(
        OperatorBase::new(41).with_output_layout(result_layout.clone()),
        Box::new(scan_executor(Vec::new(), vec!["id".to_string()])),
        BlockingOperator::new(
            BlockingOperatorKind::Distinct {
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                state: None,
            },
            result_layout,
        ),
    );
    let partitions = vec![
        partitioned_scan_executor(
            vec![vec![Value::BigInt(1)], vec![Value::BigInt(2)]],
            0,
            vec!["id".to_string()],
        ),
        partitioned_scan_executor(
            vec![vec![Value::BigInt(2)], vec![Value::BigInt(3)]],
            1,
            vec!["id".to_string()],
        ),
    ];
    let gather = concatenate_gather(&partitions);
    engine
        .build_partitioned_executor(partitions, gather, Some(global))
        .expect("partitioned dedup tree should build");

    let chunks = engine
        .execute_collected()
        .expect("partitioned dedup should execute");
    let mut ids = extract_ids(&chunks);
    ids.sort();
    assert_eq!(ids, vec![1, 2, 3]);
    assert_eq!(chunks[0].col_names(), vec!["id"]);
}

#[test]
fn partitioned_limit_applies_offset_and_count_globally() {
    let mut engine = StreamingExecutionEngine::new();
    let global = StreamingExecutor::Unary(
        OperatorBase::new(42),
        Box::new(scan_executor(Vec::new(), vec!["id".to_string()])),
        UnaryOperator::new(
            UnaryOperatorKind::Limit {
                offset: 2,
                limit: 3,
                skipped: 0,
                consumed: 0,
            },
            Arc::new(SlotLayout::from_names(&["id".to_string()])),
        ),
    );
    let partitions = vec![
        partitioned_scan_executor(
            vec![vec![Value::BigInt(0)], vec![Value::BigInt(1)]],
            0,
            vec!["id".to_string()],
        ),
        partitioned_scan_executor(
            vec![
                vec![Value::BigInt(2)],
                vec![Value::BigInt(3)],
                vec![Value::BigInt(4)],
            ],
            1,
            vec!["id".to_string()],
        ),
        partitioned_scan_executor(vec![vec![Value::BigInt(5)]], 2, vec!["id".to_string()]),
    ];
    let gather = concatenate_gather(&partitions);
    engine
        .build_partitioned_executor(partitions, gather, Some(global))
        .expect("partitioned limit tree should build");

    let chunks = engine
        .execute_collected()
        .expect("partitioned limit should execute");
    assert_eq!(extract_ids(&chunks), vec![2, 3, 4]);
}

#[test]
fn partitioned_hash_join_matches_rows_across_partition_boundaries() {
    let mut engine = StreamingExecutionEngine::new();
    let join_layout = Arc::new(SlotLayout::from_names(&[
        "id".to_string(),
        "left".to_string(),
        "id".to_string(),
        "right".to_string(),
    ]));
    let global_join = StreamingExecutor::Join(
        OperatorBase::new(43).with_output_layout(join_layout.clone()),
        Box::new(scan_executor(
            Vec::new(),
            vec!["id".to_string(), "left".to_string()],
        )),
        Box::new(scan_executor(
            Vec::new(),
            vec!["id".to_string(), "right".to_string()],
        )),
        JoinOperator::new(
            JoinOperatorKind::HashJoin {
                join_condition: None,
                hash_keys: vec![graphdb_core::types::expr::Expression::Variable(
                    "id".to_string(),
                )],
                probe_keys: vec![graphdb_core::types::expr::Expression::Variable(
                    "id".to_string(),
                )],
                build_side:
                    crate::executor::streaming::operators::join_operator::HashJoinBuildSide::new(),
                build_done: false,
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                right_col_names: Vec::new(),
                build_side_select: BuildSide::default(),
            },
            join_layout,
        ),
    );
    engine
        .build_partitioned_join_executor(
            vec![
                partitioned_scan_executor(
                    vec![vec![Value::BigInt(1), Value::string("left-1")]],
                    0,
                    vec!["id".to_string(), "left".to_string()],
                ),
                partitioned_scan_executor(
                    vec![vec![Value::BigInt(2), Value::string("left-2")]],
                    1,
                    vec!["id".to_string(), "left".to_string()],
                ),
            ],
            vec![
                partitioned_scan_executor(
                    vec![vec![Value::BigInt(2), Value::string("right-2")]],
                    0,
                    vec!["id".to_string(), "right".to_string()],
                ),
                partitioned_scan_executor(
                    vec![vec![Value::BigInt(1), Value::string("right-1")]],
                    1,
                    vec!["id".to_string(), "right".to_string()],
                ),
            ],
            global_join,
        )
        .expect("partitioned hash join tree should build");

    let chunks = engine
        .execute_collected()
        .expect("partitioned hash join should execute");
    assert_eq!(
        chunks
            .iter()
            .flat_map(|chunk| chunk.rows.iter().cloned())
            .collect::<Vec<_>>(),
        vec![
            vec![
                Value::BigInt(1),
                Value::string("left-1"),
                Value::BigInt(1),
                Value::string("right-1"),
            ],
            vec![
                Value::BigInt(2),
                Value::string("left-2"),
                Value::BigInt(2),
                Value::string("right-2"),
            ],
        ]
    );
    for chunk in chunks {
        assert_eq!(chunk.col_names(), vec!["id", "left", "id", "right"]);
    }
}

#[test]
fn partitioned_join_rejects_mismatched_input_partition_counts() {
    let mut engine = StreamingExecutionEngine::new();
    let global_join = StreamingExecutor::Join(
        OperatorBase::new(44),
        Box::new(scan_executor(Vec::new(), vec!["id".to_string()])),
        Box::new(scan_executor(Vec::new(), vec!["id".to_string()])),
        JoinOperator::new(
            JoinOperatorKind::InnerJoin {
                join_condition: None,
                build_side_tuples: Vec::new(),
                build_done: false,
                memory_tracker: MemoryTracker::new(MemoryBudget::default_budget()),
                right_col_names: Vec::new(),
            },
            Arc::new(SlotLayout::from_names(&[
                "id".to_string(),
                "id".to_string(),
            ])),
        ),
    );
    let error = engine
        .build_partitioned_join_executor(
            vec![partitioned_scan_executor(
                vec![vec![Value::BigInt(1)]],
                0,
                vec!["id".to_string()],
            )],
            vec![
                partitioned_scan_executor(vec![vec![Value::BigInt(1)]], 0, vec!["id".to_string()]),
                partitioned_scan_executor(vec![vec![Value::BigInt(2)]], 1, vec!["id".to_string()]),
            ],
            global_join,
        )
        .expect_err("mismatched partition counts must be rejected");

    assert!(error.to_string().contains("partition counts differ"));
}

#[test]
fn test_register_partition_replaces_root() {
    let mut engine = StreamingExecutionEngine::new();
    let buffer = create_test_buffer(10);
    engine.register_executor(0, scan_executor(buffer, vec![]));
    assert!(engine.root_executor.is_some());

    // Registering partitions should clear root
    engine.register_partition_executors(vec![]);
    assert!(engine.root_executor.is_none());
    assert_eq!(engine.partition_count(), 0);
}
