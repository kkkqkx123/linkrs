use super::{
    BlockingOperator, ExecutionRuntime, OperatorBase, OperatorProfileKey, SetOperator,
    SortDirection, SourceOperator, StreamingExecutor, UnaryOperator,
};
use crate::executor::streaming::helpers::compare_values;
use crate::executor::streaming::operators::set_operator::SetOperatorKind;
use crate::executor::streaming::operators::source_operator::SourceOperatorKind;
use crate::executor::streaming::operators::unary_operator::UnaryOperatorKind;
use crate::executor::streaming::plan::types::PhysicalOperatorId;
use crate::executor::streaming::slot::SlotLayout;
use graphdb_core::Value;
use std::sync::Arc;

fn create_test_buffer() -> Vec<Vec<Value>> {
    (0..100)
        .map(|i| {
            vec![
                Value::BigInt(i as i64),
                Value::string(format!("vertex_{}", i)),
                Value::string(format!("label_{}", i % 10)),
                Value::string(format!("prop_{}", i % 100)),
                Value::BigInt((i % 1000) as i64),
            ]
        })
        .collect()
}

fn scan_executor(rows: Vec<Vec<Value>>, col_names: Vec<String>) -> StreamingExecutor {
    StreamingExecutor::Source(
        OperatorBase::new(0),
        SourceOperator::new(
            SourceOperatorKind::ScanVertices {
                buffer: rows,
                current_index: 0,
                col_names,
            },
            Arc::new(SlotLayout::new(vec![])),
        ),
    )
}

fn empty_layout() -> Arc<SlotLayout> {
    Arc::new(SlotLayout::new(vec![]))
}

#[test]
fn test_scan_vertices_with_buffer() {
    let buffer = create_test_buffer();
    let mut executor = scan_executor(buffer.clone(), vec![]);

    executor.open().unwrap();
    let chunk = executor.advance().unwrap();
    assert!(chunk.is_some());
    let chunk = chunk.unwrap();
    assert_eq!(chunk.len(), 100);
    executor.close().unwrap();
}

#[test]
fn test_limit_executor() {
    let buffer = create_test_buffer();
    let scan = Box::new(scan_executor(buffer, vec![]));
    let mut executor = StreamingExecutor::Unary(
        OperatorBase::new(0),
        scan,
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

    executor.open().unwrap();
    let mut total = 0;
    while let Some(mut chunk) = executor.advance().unwrap() {
        // Limit is selection-aware — materialize to count the rows
        // an API consumer would observe (engine does this at the root).
        chunk.materialize_selection_by("Test");
        total += chunk.len();
    }
    executor.close().unwrap();
    assert_eq!(total, 10);
}

#[test]
fn test_limit_executor_honors_offset() {
    let scan = Box::new(scan_executor(
        (0..6).map(|value| vec![Value::BigInt(value)]).collect(),
        vec!["id".to_string()],
    ));
    let mut executor = StreamingExecutor::Unary(
        OperatorBase::new(0),
        scan,
        UnaryOperator::new(
            UnaryOperatorKind::Limit {
                offset: 2,
                limit: 3,
                skipped: 0,
                consumed: 0,
            },
            empty_layout(),
        ),
    );

    executor.open().expect("limit should open");
    let mut values = Vec::new();
    while let Some(mut chunk) = executor.advance().expect("limit should advance") {
        // materialize the selection to observe the rows an API
        // consumer would see (engine does this at the root).
        chunk.materialize_selection_by("Test");
        values.extend(chunk.rows.into_iter().filter_map(|row| match row.first() {
            Some(Value::BigInt(value)) => Some(*value),
            _ => None,
        }));
    }
    executor.close().expect("limit should close");

    assert_eq!(values, vec![2, 3, 4]);
}

#[test]
fn test_dynamic_column_count() {
    let buffer: Vec<Vec<Value>> = vec![
        vec![
            Value::BigInt(1),
            Value::string("a"),
            Value::string("b"),
            Value::string("c"),
            Value::string("d"),
            Value::string("e"),
            Value::string("f"),
            Value::string("g"),
            Value::string("h"),
        ],
        vec![
            Value::BigInt(2),
            Value::string("i"),
            Value::string("j"),
            Value::string("k"),
            Value::string("l"),
            Value::string("m"),
            Value::string("n"),
            Value::string("o"),
            Value::string("p"),
        ],
    ];

    let col_names = (0..9).map(|i| format!("col_{}", i)).collect::<Vec<_>>();
    let mut executor = scan_executor(buffer.clone(), col_names);
    executor.open().unwrap();
    let chunk = executor.advance().unwrap();
    assert!(chunk.is_some());
    let chunk = chunk.unwrap();
    assert_eq!(chunk.len(), 2);
    assert_eq!(chunk.num_columns(), 9);
    executor.close().unwrap();
}

#[test]
fn failed_open_closes_children_opened_before_the_failure() {
    let left = StreamingExecutor::Source(
        OperatorBase::new(1),
        SourceOperator::new(
            SourceOperatorKind::ScanVertices {
                buffer: vec![vec![Value::BigInt(1)]],
                current_index: 0,
                col_names: vec!["id".to_string()],
            },
            empty_layout(),
        ),
    );
    let right = StreamingExecutor::Source(
        OperatorBase::new(2),
        SourceOperator::new(
            SourceOperatorKind::StorageScanVertices {
                storage: None,
                space_name: "test".to_string(),
                limit: None,
                partition_range: None,
                col_names: vec![],
                projected_properties: vec![],
                predicate: Vec::new(),
                tag: None,
                cursor: None,
            },
            empty_layout(),
        ),
    );
    let mut executor = StreamingExecutor::Set(
        OperatorBase::new(3),
        Box::new(left),
        Box::new(right),
        SetOperator::new(
            SetOperatorKind::UnionAll {
                left_consumed: false,
            },
            empty_layout(),
        ),
    );

    assert!(executor.open().is_err());
    assert!(!executor.opened());
    for child in executor.children_mut() {
        assert!(!child.opened());
    }
}

#[test]
fn test_sort_spill_records_profile_metrics() {
    use crate::executor::base::MemoryBudget;
    use crate::executor::streaming::operators::spec::BlockingSpec;
    use crate::executor::streaming::plan::types::PhysicalOperatorId;
    use crate::executor::streaming::slot::SlotLayout;
    use crate::executor::streaming::spill::{SpillConfig, SpillManager};
    use std::sync::Arc;

    // Build a runtime with a large memory budget (source chunk
    // reservations must not fail) while the Sort tracker uses a tiny
    // budget so the operator spills immediately.
    let runtime_budget = MemoryBudget::new(512 * 1024 * 1024);
    let tracker_budget = MemoryBudget::new(128); // ~3 rows before spill
    let rt = Arc::new(ExecutionRuntime::new(
        super::super::runtime::QueryIdentity {
            query_id: 999,
            session_id: None,
            space_name: None,
        },
        runtime_budget,
        None,
        crate::executor::base::SearchContext::default(),
    ));

    let sm = Arc::new(SpillManager::new(SpillConfig::default(), 999).unwrap());
    rt.set_spill_manager(Some(sm));

    // Build input scan
    let rows: Vec<Vec<Value>> = (0..50)
        .map(|i| vec![Value::BigInt(50 - i as i64)])
        .collect();
    let scan = Box::new(scan_executor(rows, vec!["val".to_string()]));

    let output_layout = Arc::new(SlotLayout::new(vec![]));
    let mut executor = StreamingExecutor::Blocking(
        OperatorBase::new(10)
            .with_runtime(Some(rt.clone()))
            .with_physical_operator_id(PhysicalOperatorId(42))
            .with_output_layout(output_layout.clone()),
        scan,
        BlockingOperator::from_spec(
            &BlockingSpec::Sort {
                sort_expressions: vec![graphdb_core::types::expr::Expression::variable(
                    "val".to_string(),
                )],
                sort_directions: vec![SortDirection::Ascending],
            },
            &tracker_budget,
            output_layout,
        ),
    );

    executor.open().unwrap();
    while let Some(_chunk) = executor.advance().unwrap() {}
    executor.close().unwrap();

    // Verify profile has spill metrics recorded.
    let prof = rt.profile().flush_to_collector();
    let key = OperatorProfileKey::new(PhysicalOperatorId(42), None);
    let entry = prof.operators.get(&key).expect("profile entry exists");
    assert!(
        entry.spilled_bytes > 0,
        "expected spilled_bytes > 0, got {}",
        entry.spilled_bytes
    );
    assert!(
        entry.spill_count > 0,
        "expected spill_count > 0, got {}",
        entry.spill_count
    );
    assert!(
        entry.peak_memory_bytes > 0,
        "expected peak_memory_bytes > 0, got {}",
        entry.peak_memory_bytes
    );
    assert_eq!(entry.output_rows, 50);
}

/// Run a Sort operator over `rows` and return all output rows plus the
/// runtime (for spill-metric inspection).
fn run_sort(
    rows: Vec<Vec<Value>>,
    col_names: Vec<String>,
    spill_budget_bytes: Option<usize>,
) -> (Vec<Vec<Value>>, Arc<ExecutionRuntime>) {
    use crate::executor::base::MemoryBudget;
    use crate::executor::streaming::operators::spec::BlockingSpec;
    use crate::executor::streaming::plan::types::PhysicalOperatorId;
    use crate::executor::streaming::slot::SlotLayout;
    use crate::executor::streaming::spill::{SpillConfig, SpillManager};

    let spill_budget = spill_budget_bytes.unwrap_or(512 * 1024 * 1024);
    let runtime_budget = MemoryBudget::new(512 * 1024 * 1024);
    let tracker_budget = MemoryBudget::new(spill_budget);
    let rt = Arc::new(ExecutionRuntime::new(
        super::super::runtime::QueryIdentity {
            query_id: 4243,
            session_id: None,
            space_name: None,
        },
        runtime_budget.clone(),
        None,
        crate::executor::base::SearchContext::default(),
    ));
    if spill_budget_bytes.is_some() {
        let sm = Arc::new(SpillManager::new(SpillConfig::default(), 4243).unwrap());
        rt.set_spill_manager(Some(sm));
    }

    let scan = Box::new(scan_executor(rows, col_names.clone()));
    let output_layout = Arc::new(SlotLayout::new(vec![]));
    let mut executor = StreamingExecutor::Blocking(
        OperatorBase::new(10)
            .with_runtime(Some(rt.clone()))
            .with_physical_operator_id(PhysicalOperatorId(44))
            .with_output_layout(output_layout.clone()),
        scan,
        BlockingOperator::from_spec(
            &BlockingSpec::Sort {
                sort_expressions: vec![graphdb_core::types::expr::Expression::variable(
                    col_names[0].clone(),
                )],
                sort_directions: vec![SortDirection::Ascending],
            },
            &tracker_budget,
            output_layout,
        ),
    );

    executor.open().unwrap();
    let mut result = Vec::new();
    while let Some(chunk) = executor.advance().unwrap() {
        result.extend(chunk.rows);
    }
    executor.close().unwrap();
    (result, rt)
}

#[test]
fn test_sort_multi_run_spill_output_sorted_and_complete() {
    // Many rows with a tiny budget produce multiple spill runs; the merge
    // must reconstruct the fully sorted output (write → read → merge →
    // cleanup closed loop).
    let rows: Vec<Vec<Value>> = (0..500)
        .map(|i| vec![Value::BigInt(499 - i as i64)])
        .collect();
    let col_names = vec!["val".to_string()];

    let (spilled, rt) = run_sort(rows, col_names, Some(64));
    assert_eq!(spilled.len(), 500, "all rows must survive spill");
    for (i, row) in spilled.iter().enumerate() {
        assert_eq!(
            row[0],
            Value::BigInt(i as i64),
            "merged output must be fully sorted at index {}",
            i
        );
    }

    let prof = rt.profile().flush_to_collector();
    let key = OperatorProfileKey::new(PhysicalOperatorId(44), None);
    let entry = prof.operators.get(&key).expect("profile entry exists");
    assert!(
        entry.spill_count >= 2,
        "expected multiple spill runs, got {}",
        entry.spill_count
    );
    assert!(entry.spilled_bytes > 0);

    // Spilled result must be identical to the in-memory baseline.
    let (in_memory, _) = run_sort(
        (0..500)
            .map(|i| vec![Value::BigInt(499 - i as i64)])
            .collect(),
        vec!["val".to_string()],
        None,
    );
    assert_eq!(spilled, in_memory, "spill and in-memory results differ");
}

/// Run an Aggregate operator over `rows` and return all result rows.
///
/// When `spill_budget_bytes` is `Some`, a tiny memory budget plus a spill
/// manager force the accumulator spill path; otherwise a large budget
/// keeps everything in memory.
fn run_aggregate(
    rows: Vec<Vec<Value>>,
    col_names: Vec<String>,
    spill_budget_bytes: Option<usize>,
) -> (Vec<Vec<Value>>, Arc<ExecutionRuntime>) {
    use crate::executor::base::MemoryBudget;
    use crate::executor::streaming::operators::spec::BlockingSpec;
    use crate::executor::streaming::slot::SlotLayout;
    use crate::executor::streaming::spill::{SpillConfig, SpillManager};
    use graphdb_core::types::expr::Expression;
    use graphdb_core::types::operators::AggregateFunction;

    let spill_budget = spill_budget_bytes.unwrap_or(512 * 1024 * 1024);
    let runtime_budget = MemoryBudget::new(512 * 1024 * 1024);
    let tracker_budget = MemoryBudget::new(spill_budget);
    let rt = Arc::new(ExecutionRuntime::new(
        super::super::runtime::QueryIdentity {
            query_id: 4242,
            session_id: None,
            space_name: None,
        },
        runtime_budget.clone(),
        None,
        crate::executor::base::SearchContext::default(),
    ));
    if spill_budget_bytes.is_some() {
        let sm = Arc::new(SpillManager::new(SpillConfig::default(), 4242).unwrap());
        rt.set_spill_manager(Some(sm));
    }

    let scan = Box::new(scan_executor(rows, col_names));
    let output_layout = Arc::new(SlotLayout::new(vec![]));
    let mut executor = StreamingExecutor::Blocking(
        OperatorBase::new(10)
            .with_runtime(Some(rt.clone()))
            .with_physical_operator_id(PhysicalOperatorId(43))
            .with_output_layout(output_layout.clone()),
        scan,
        BlockingOperator::from_spec(
            &BlockingSpec::Aggregate {
                group_by_expressions: vec![Expression::variable("g".to_string())],
                aggregate_functions: vec![
                    (
                        AggregateFunction::Count,
                        vec![Expression::Literal(Value::Int(1))],
                    ),
                    (
                        AggregateFunction::Sum,
                        vec![Expression::variable("v".to_string())],
                    ),
                    (
                        AggregateFunction::Min,
                        vec![Expression::variable("v".to_string())],
                    ),
                    (
                        AggregateFunction::Max,
                        vec![Expression::variable("v".to_string())],
                    ),
                    (
                        AggregateFunction::Collect,
                        vec![Expression::variable("v".to_string())],
                    ),
                ],
                output_col_names: vec![],
            },
            &tracker_budget,
            output_layout,
        ),
    );

    executor.open().unwrap();
    let mut result = Vec::new();
    while let Some(chunk) = executor.advance().unwrap() {
        result.extend(chunk.rows);
    }
    executor.close().unwrap();

    result.sort_by(|a, b| compare_values(&a[0], &b[0]));
    (result, rt)
}

#[test]
fn test_aggregate_spill_matches_in_memory() {
    let rows: Vec<Vec<Value>> = (0..2000)
        .map(|i| {
            vec![
                Value::BigInt((i % 40) as i64),
                Value::BigInt((i as i64) * 3 - 1000),
            ]
        })
        .collect();
    let col_names = vec!["g".to_string(), "v".to_string()];

    let (spilled, rt) = run_aggregate(rows.clone(), col_names.clone(), Some(4096));
    let (in_memory, _) = run_aggregate(rows, col_names, None);

    // Spilled results must match the in-memory baseline for every group.
    assert_eq!(spilled.len(), in_memory.len());
    for (spilled_row, in_mem_row) in spilled.iter().zip(in_memory.iter()) {
        assert_eq!(spilled_row.len(), in_mem_row.len());
        for (s, m) in spilled_row.iter().zip(in_mem_row.iter()) {
            match (s, m) {
                (Value::List(a), Value::List(b)) => {
                    assert_eq!(a.values, b.values);
                }
                _ => assert_eq!(s, m, "group {:?} value mismatch", spilled_row[0]),
            }
        }
    }

    // The spill path must actually have spilled to disk.
    let prof = rt.profile().flush_to_collector();
    let key = OperatorProfileKey::new(PhysicalOperatorId(43), None);
    let entry = prof.operators.get(&key).expect("profile entry exists");
    assert!(
        entry.spilled_bytes > 0,
        "expected spilled_bytes > 0, got {}",
        entry.spilled_bytes
    );
}

/// Run a GroupBy operator over `rows` and return all result rows.
///
/// When `spill_budget_bytes` is `Some`, a tiny memory budget plus a spill
/// manager force the partition-spill path; otherwise a large budget
/// keeps everything in memory.
fn run_groupby(
    rows: Vec<Vec<Value>>,
    col_names: Vec<String>,
    spill_budget_bytes: Option<usize>,
) -> (Vec<Vec<Value>>, Arc<ExecutionRuntime>) {
    use crate::executor::base::MemoryBudget;
    use crate::executor::streaming::operators::spec::BlockingSpec;
    use crate::executor::streaming::plan::types::PhysicalOperatorId;
    use crate::executor::streaming::slot::SlotLayout;
    use crate::executor::streaming::spill::{SpillConfig, SpillManager};

    let spill_budget = spill_budget_bytes.unwrap_or(512 * 1024 * 1024);
    let runtime_budget = MemoryBudget::new(512 * 1024 * 1024);
    let tracker_budget = MemoryBudget::new(spill_budget);
    let rt = Arc::new(ExecutionRuntime::new(
        super::super::runtime::QueryIdentity {
            query_id: 4244,
            session_id: None,
            space_name: None,
        },
        runtime_budget.clone(),
        None,
        crate::executor::base::SearchContext::default(),
    ));
    if spill_budget_bytes.is_some() {
        let sm = Arc::new(SpillManager::new(SpillConfig::default(), 4244).unwrap());
        rt.set_spill_manager(Some(sm));
    }

    let scan = Box::new(scan_executor(rows, col_names.clone()));
    let output_layout = Arc::new(SlotLayout::new(vec![]));
    let mut executor = StreamingExecutor::Blocking(
        OperatorBase::new(10)
            .with_runtime(Some(rt.clone()))
            .with_physical_operator_id(PhysicalOperatorId(45))
            .with_output_layout(output_layout.clone()),
        scan,
        BlockingOperator::from_spec(
            &BlockingSpec::GroupBy {
                group_by_expressions: vec![graphdb_core::types::expr::Expression::variable(
                    col_names[0].clone(),
                )],
            },
            &tracker_budget,
            output_layout,
        ),
    );

    executor.open().unwrap();
    let mut result = Vec::new();
    while let Some(chunk) = executor.advance().unwrap() {
        result.extend(chunk.rows);
    }
    executor.close().unwrap();

    // GroupBy is a grouping (not sorting) operator; normalize order for
    // comparison by sorting on the full row content.
    result.sort_by(|a, b| {
        for (x, y) in a.iter().zip(b.iter()) {
            let c = compare_values(x, y);
            if c != std::cmp::Ordering::Equal {
                return c;
            }
        }
        std::cmp::Ordering::Equal
    });
    (result, rt)
}

#[test]
fn test_groupby_spill_matches_in_memory() {
    let rows: Vec<Vec<Value>> = (0..2000)
        .map(|i| {
            vec![
                Value::BigInt((i % 40) as i64),
                Value::BigInt((i as i64) * 3 - 1000),
            ]
        })
        .collect();
    let col_names = vec!["g".to_string(), "v".to_string()];

    let (spilled, rt) = run_groupby(rows.clone(), col_names.clone(), Some(65536));
    let (in_memory, _) = run_groupby(rows, col_names, None);

    // Grouped output must be identical to the in-memory baseline.
    assert_eq!(spilled, in_memory, "spill and in-memory results differ");

    // The spill path must actually have spilled to disk.
    let prof = rt.profile().flush_to_collector();
    let key = OperatorProfileKey::new(PhysicalOperatorId(45), None);
    let entry = prof.operators.get(&key).expect("profile entry exists");
    assert!(
        entry.spilled_bytes > 0,
        "expected spilled_bytes > 0, got {}",
        entry.spilled_bytes
    );
    assert!(
        entry.spill_count > 0,
        "expected spill_count > 0, got {}",
        entry.spill_count
    );
}

/// Run a WindowFunction operator over `rows` and return all result rows.
///
/// When `spill_budget_bytes` is `Some`, a tiny memory budget plus a spill
/// manager force the partition-spill path; otherwise a large budget
/// keeps everything in memory.
fn run_window(
    rows: Vec<Vec<Value>>,
    col_names: Vec<String>,
    spill_budget_bytes: Option<usize>,
) -> (Vec<Vec<Value>>, Arc<ExecutionRuntime>) {
    use crate::executor::base::MemoryBudget;
    use crate::executor::streaming::operators::spec::BlockingSpec;
    use crate::executor::streaming::plan::types::PhysicalOperatorId;
    use crate::executor::streaming::slot::SlotLayout;
    use crate::executor::streaming::spill::{SpillConfig, SpillManager};
    use graphdb_core::types::expr::Expression;

    let spill_budget = spill_budget_bytes.unwrap_or(512 * 1024 * 1024);
    let runtime_budget = MemoryBudget::new(512 * 1024 * 1024);
    let tracker_budget = MemoryBudget::new(spill_budget);
    let rt = Arc::new(ExecutionRuntime::new(
        super::super::runtime::QueryIdentity {
            query_id: 4245,
            session_id: None,
            space_name: None,
        },
        runtime_budget.clone(),
        None,
        crate::executor::base::SearchContext::default(),
    ));
    if spill_budget_bytes.is_some() {
        let sm = Arc::new(SpillManager::new(SpillConfig::default(), 4245).unwrap());
        rt.set_spill_manager(Some(sm));
    }

    let scan = Box::new(scan_executor(rows, col_names.clone()));
    let output_layout = Arc::new(SlotLayout::new(vec![]));
    let mut executor = StreamingExecutor::Blocking(
        OperatorBase::new(10)
            .with_runtime(Some(rt.clone()))
            .with_physical_operator_id(PhysicalOperatorId(46))
            .with_output_layout(output_layout.clone()),
        scan,
        BlockingOperator::from_spec(
            &BlockingSpec::WindowFunction {
                window_exprs: vec![Expression::WindowFunction {
                    name: "row_number".to_string(),
                    args: vec![],
                    over_partition_by: vec![Expression::variable(col_names[0].clone())],
                    over_order_by: vec![Expression::variable(col_names[1].clone())],
                    over_order_desc: vec![false],
                }],
                partition_by_exprs: vec![Expression::variable(col_names[0].clone())],
                order_by_exprs: vec![Expression::variable(col_names[1].clone())],
                order_by_directions: vec![SortDirection::Ascending],
            },
            &tracker_budget,
            output_layout,
        ),
    );

    executor.open().unwrap();
    let mut result = Vec::new();
    while let Some(chunk) = executor.advance().unwrap() {
        result.extend(chunk.rows);
    }
    executor.close().unwrap();

    // Partitions are emitted in different orders on the two paths; sort
    // rows by content so the comparison is order-independent.
    result.sort_by(|a, b| {
        for (x, y) in a.iter().zip(b.iter()) {
            let c = compare_values(x, y);
            if c != std::cmp::Ordering::Equal {
                return c;
            }
        }
        std::cmp::Ordering::Equal
    });
    (result, rt)
}

#[test]
fn test_window_spill_matches_in_memory() {
    let rows: Vec<Vec<Value>> = (0..2000)
        .map(|i| {
            vec![
                Value::BigInt((i % 40) as i64),
                Value::BigInt(((i * 7) % 1000) as i64),
            ]
        })
        .collect();
    let col_names = vec!["p".to_string(), "v".to_string()];

    let (spilled, rt) = run_window(rows.clone(), col_names.clone(), Some(65536));
    let (in_memory, _) = run_window(rows, col_names, None);

    // Window output must be identical to the in-memory baseline.
    assert_eq!(spilled, in_memory, "spill and in-memory results differ");
    assert!(!spilled.is_empty(), "window produced no output rows");

    // The spill path must actually have spilled to disk.
    let prof = rt.profile().flush_to_collector();
    let key = OperatorProfileKey::new(PhysicalOperatorId(46), None);
    let entry = prof.operators.get(&key).expect("profile entry exists");
    assert!(
        entry.spilled_bytes > 0,
        "expected spilled_bytes > 0, got {}",
        entry.spilled_bytes
    );
    assert!(
        entry.spill_count > 0,
        "expected spill_count > 0, got {}",
        entry.spill_count
    );
}

// ── Reset protocol ──

fn pull_all(executor: &mut StreamingExecutor) -> Vec<Vec<Value>> {
    let mut rows = Vec::new();
    while let Some(mut chunk) = executor.advance().expect("advance should succeed") {
        chunk.materialize_selection_by("Test");
        rows.extend(chunk.rows);
    }
    rows
}

#[test]
fn stateless_filter_reset_repulls_identical_output() {
    use graphdb_core::types::expr::Expression;
    use graphdb_core::types::operators::BinaryOperator;

    let scan = Box::new(scan_executor(
        (1..=6).map(|v| vec![Value::BigInt(v)]).collect(),
        vec!["v".to_string()],
    ));
    let predicate = Expression::binary(
        Expression::variable("v"),
        BinaryOperator::GreaterThan,
        Expression::literal(Value::BigInt(3)),
    );
    let mut executor = StreamingExecutor::Unary(
        OperatorBase::new(0),
        scan,
        UnaryOperator::new(
            UnaryOperatorKind::Filter {
                predicate,
                state: Default::default(),
            },
            empty_layout(),
        ),
    );

    executor.open().expect("open should succeed");
    let first = pull_all(&mut executor);
    assert_eq!(
        first,
        vec![
            vec![Value::BigInt(4)],
            vec![Value::BigInt(5)],
            vec![Value::BigInt(6)]
        ]
    );

    executor.reset().expect("reset should succeed");
    let second = pull_all(&mut executor);
    assert_eq!(second, first, "stateless filter reset re-produces output");
    executor.close().expect("close should succeed");
}

#[test]
fn buffered_unary_reset_clears_counters_and_seen_rows() {
    let dedup_scan = Box::new(scan_executor(
        vec![
            vec![Value::BigInt(1)],
            vec![Value::BigInt(1)],
            vec![Value::BigInt(2)],
        ],
        vec!["v".to_string()],
    ));
    let mut dedup = StreamingExecutor::Unary(
        OperatorBase::new(0),
        dedup_scan,
        UnaryOperator::new(
            UnaryOperatorKind::Dedup {
                seen_rows: std::collections::HashSet::new(),
            },
            empty_layout(),
        ),
    );
    dedup.open().expect("open should succeed");
    let first = pull_all(&mut dedup);
    assert_eq!(first, vec![vec![Value::BigInt(1)], vec![Value::BigInt(2)]]);
    dedup.reset().expect("dedup reset should succeed");
    let second = pull_all(&mut dedup);
    assert_eq!(second, first, "dedup seen_rows must be cleared by reset");
    dedup.close().expect("close should succeed");

    let limit_scan = Box::new(scan_executor(
        (0..10).map(|v| vec![Value::BigInt(v)]).collect(),
        vec!["v".to_string()],
    ));
    let mut limit = StreamingExecutor::Unary(
        OperatorBase::new(0),
        limit_scan,
        UnaryOperator::new(
            UnaryOperatorKind::Limit {
                offset: 0,
                limit: 3,
                skipped: 0,
                consumed: 0,
            },
            empty_layout(),
        ),
    );
    limit.open().expect("open should succeed");
    let first = pull_all(&mut limit);
    assert_eq!(first.len(), 3, "limit applies on the first run");
    limit.reset().expect("limit reset should succeed");
    let second = pull_all(&mut limit);
    assert_eq!(second, first, "limit counters must be reset");
    limit.close().expect("close should succeed");
}

#[test]
fn blocking_sort_reset_falls_back_to_close_open_and_marks_flag() {
    use crate::executor::streaming::operators::spec::BlockingSpec;
    use crate::executor::streaming::slot::SlotLayout;
    use graphdb_core::types::expr::Expression;

    let rows: Vec<Vec<Value>> = (0..6).map(|v| vec![Value::BigInt(5 - v)]).collect();
    let scan = Box::new(scan_executor(rows, vec!["v".to_string()]));
    let output_layout = Arc::new(SlotLayout::new(vec![]));
    let mut executor = StreamingExecutor::Blocking(
        OperatorBase::new(10).with_output_layout(output_layout.clone()),
        scan,
        BlockingOperator::from_spec(
            &BlockingSpec::Sort {
                sort_expressions: vec![Expression::variable("v".to_string())],
                sort_directions: vec![SortDirection::Ascending],
            },
            &crate::executor::base::MemoryBudget::default_budget(),
            output_layout,
        ),
    );

    executor.open().expect("open should succeed");
    let first = pull_all(&mut executor);
    assert_eq!(first.len(), 6);
    executor.reset().expect("reset should succeed");
    assert!(
        executor.base().reset_used_fallback,
        "Blocking has no native reset yet; fallback must be flagged"
    );
    let second = pull_all(&mut executor);
    assert_eq!(
        second, first,
        "fallback reset re-produces the sorted stream"
    );
    executor.close().expect("close should succeed");
}

#[test]
fn correlation_frames_are_isolated_per_executor_instance() {
    use crate::executor::streaming::slot::SlotLayout;

    let layout = Arc::new(SlotLayout::from_names(&["id".to_string()]));
    let mut first = StreamingExecutor::Source(
        OperatorBase::new(0).with_output_layout(layout.clone()),
        SourceOperator::new(SourceOperatorKind::Argument, layout.clone()),
    );
    let mut second = StreamingExecutor::Source(
        OperatorBase::new(1).with_output_layout(layout.clone()),
        SourceOperator::new(SourceOperatorKind::Argument, layout.clone()),
    );
    first.open().expect("open should succeed");
    second.open().expect("open should succeed");

    first.inject_correlation_frame(layout.clone(), vec![Value::BigInt(10)]);
    second.inject_correlation_frame(layout.clone(), vec![Value::BigInt(20)]);

    let first_chunk = first.advance().expect("pull").expect("first frame row");
    let second_chunk = second.advance().expect("pull").expect("second frame row");
    assert_eq!(first_chunk.rows, vec![vec![Value::BigInt(10)]]);
    assert_eq!(
        second_chunk.rows,
        vec![vec![Value::BigInt(20)]],
        "frames must be private to each executor instance"
    );

    first.reset().expect("reset should succeed");
    second.reset().expect("reset should succeed");
    first.inject_correlation_frame(layout.clone(), vec![Value::BigInt(30)]);
    let again = first.advance().expect("pull").expect("third frame row");
    assert_eq!(again.rows, vec![vec![Value::BigInt(30)]]);
}
