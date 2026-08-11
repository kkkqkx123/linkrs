use std::sync::Arc;

use super::sort::*;
use super::*;
use crate::core::value::NullType;
use crate::query::executor::base::MemoryBudget;
use crate::query::executor::streaming::spill::{RunReader, SpillConfig, SpillManager};

fn integer_rows(values: &[i64]) -> Vec<Vec<Value>> {
    values.iter().map(|&v| vec![Value::BigInt(v)]).collect()
}

fn make_sort_expr(col_name: &str) -> Expression {
    Expression::variable(col_name.to_string())
}

#[test]
fn test_sort_rows_ascending() {
    let mut rows = integer_rows(&[3, 1, 4, 1, 5, 9, 2, 6]);
    let col_names = vec!["val".to_string()];
    let exprs = vec![make_sort_expr("val")];
    let dirs = vec![SortDirection::Ascending];

    sort_rows(&mut rows, &col_names, &exprs, &dirs);

    let vals: Vec<i64> = rows
        .iter()
        .map(|r| match &r[0] {
            Value::BigInt(n) => *n,
            _ => panic!("expected BigInt"),
        })
        .collect();
    assert_eq!(vals, vec![1, 1, 2, 3, 4, 5, 6, 9]);
}

#[test]
fn test_sort_rows_descending() {
    let mut rows = integer_rows(&[3, 1, 4, 1, 5, 9, 2, 6]);
    let col_names = vec!["val".to_string()];
    let exprs = vec![make_sort_expr("val")];
    let dirs = vec![SortDirection::Descending];

    sort_rows(&mut rows, &col_names, &exprs, &dirs);

    let vals: Vec<i64> = rows
        .iter()
        .map(|r| match &r[0] {
            Value::BigInt(n) => *n,
            _ => panic!("expected BigInt"),
        })
        .collect();
    assert_eq!(vals, vec![9, 6, 5, 4, 3, 2, 1, 1]);
}

#[test]
fn test_sort_rows_empty() {
    let mut rows: Vec<Vec<Value>> = vec![];
    let col_names = vec!["val".to_string()];
    sort_rows(&mut rows, &col_names, &[], &[]);
    assert!(rows.is_empty());
}

#[test]
fn test_sort_rows_single_row() {
    let mut rows = integer_rows(&[42]);
    let col_names = vec!["val".to_string()];
    let exprs = vec![make_sort_expr("val")];
    let dirs = vec![SortDirection::Ascending];
    sort_rows(&mut rows, &col_names, &exprs, &dirs);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::BigInt(42));
}

#[test]
fn test_sort_rows_null_ordering() {
    let mut rows = vec![
        vec![Value::BigInt(3)],
        vec![Value::Null(NullType::Null)],
        vec![Value::BigInt(1)],
        vec![Value::BigInt(2)],
    ];
    let col_names = vec!["val".to_string()];
    let exprs = vec![make_sort_expr("val")];
    let dirs = vec![SortDirection::Ascending];

    sort_rows(&mut rows, &col_names, &exprs, &dirs);

    assert_eq!(rows[0][0], Value::BigInt(1));
    assert_eq!(rows[1][0], Value::BigInt(2));
    assert_eq!(rows[2][0], Value::BigInt(3));
    assert_eq!(rows[3][0], Value::Null(NullType::Null));
}

#[test]
fn test_spill_sorted_run_basic() {
    let manager = SpillManager::new(SpillConfig::default(), 301).expect("spill manager");
    let mut buffer = integer_rows(&[3, 1, 4, 1, 5]);
    let col_names = vec!["val".to_string()];
    let exprs = vec![make_sort_expr("val")];
    let dirs = vec![SortDirection::Ascending];
    let budget = MemoryBudget::new(1024 * 1024);
    let mut tracker = MemoryTracker::new(budget);
    let mut runs: Vec<crate::query::executor::streaming::spill::SpilledRun> = vec![];

    let count = spill_sorted_run(
        &mut buffer,
        &col_names,
        &exprs,
        &dirs,
        &manager,
        &mut tracker,
        &mut runs,
    )
    .expect("spill");

    assert_eq!(count, 5);
    assert_eq!(runs.len(), 1);
    assert!(buffer.is_empty());

    let mut reader = RunReader::open(&runs[0]).expect("open run");
    let read_rows = reader.read_all().expect("read all");
    let vals: Vec<i64> = read_rows
        .iter()
        .map(|r| match &r[0] {
            Value::BigInt(n) => *n,
            _ => panic!("expected BigInt"),
        })
        .collect();
    assert_eq!(vals, vec![1, 1, 3, 4, 5]);
}

#[test]
fn test_spill_sorted_run_empty_buffer() {
    use crate::query::executor::streaming::spill::SpilledRun;
    let manager = SpillManager::new(SpillConfig::default(), 302).expect("spill manager");
    let mut buffer: Vec<Vec<Value>> = vec![];
    let col_names = vec!["val".to_string()];
    let budget = MemoryBudget::new(1024 * 1024);
    let mut tracker = MemoryTracker::new(budget);
    let mut runs: Vec<SpilledRun> = vec![];

    let count = spill_sorted_run(
        &mut buffer,
        &col_names,
        &[],
        &[],
        &manager,
        &mut tracker,
        &mut runs,
    )
    .expect("spill");

    assert_eq!(count, 0);
    assert!(runs.is_empty());
}

#[test]
fn test_multi_run_merge_correctness() {
    use crate::query::executor::streaming::spill::SpilledRun;
    let manager = SpillManager::new(SpillConfig::default(), 303).expect("spill manager");
    let col_names = vec!["val".to_string()];
    let exprs = vec![make_sort_expr("val")];
    let dirs = vec![SortDirection::Ascending];
    let budget = MemoryBudget::new(10);
    let mut tracker = MemoryTracker::new(budget);

    let mut runs: Vec<SpilledRun> = vec![];

    let mut b1 = integer_rows(&[1, 4, 7]);
    spill_sorted_run(
        &mut b1,
        &col_names,
        &exprs,
        &dirs,
        &manager,
        &mut tracker,
        &mut runs,
    )
    .unwrap();

    let mut b2 = integer_rows(&[2, 5, 8]);
    spill_sorted_run(
        &mut b2,
        &col_names,
        &exprs,
        &dirs,
        &manager,
        &mut tracker,
        &mut runs,
    )
    .unwrap();

    let mut b3 = integer_rows(&[3, 6, 9]);
    spill_sorted_run(
        &mut b3,
        &col_names,
        &exprs,
        &dirs,
        &manager,
        &mut tracker,
        &mut runs,
    )
    .unwrap();

    let mut run_buffers: Vec<RunBuffer> = Vec::with_capacity(runs.len());
    for run in &runs {
        let reader = RunReader::open(run).expect("open run");
        run_buffers.push(RunBuffer {
            rows: vec![],
            index: 0,
            reader,
        });
    }
    for buf in &mut run_buffers {
        refill_run_buffer(buf, 1024).unwrap();
    }

    let mut merged: Vec<i64> = Vec::new();
    loop {
        let min_idx = find_min_run(&run_buffers, &col_names, &exprs, &dirs);
        match min_idx {
            None => break,
            Some(idx) => {
                let buf = &mut run_buffers[idx];
                if let Value::BigInt(n) = buf.rows[buf.index][0] {
                    merged.push(n);
                }
                buf.index += 1;
                if buf.index >= buf.rows.len() {
                    refill_run_buffer(buf, 1024).unwrap();
                }
            }
        }
    }

    assert_eq!(merged, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
}

#[test]
fn test_sort_rows_multi_column() {
    let mut rows = vec![
        vec![Value::BigInt(3), Value::BigInt(30)],
        vec![Value::BigInt(1), Value::BigInt(10)],
        vec![Value::BigInt(2), Value::BigInt(20)],
        vec![Value::BigInt(4), Value::BigInt(40)],
    ];
    let expr_first = Expression::variable("a");
    let expr_second = Expression::variable("a");
    let col_names = vec!["a".to_string(), "b".to_string()];
    let exprs = vec![expr_first, expr_second];
    let dirs = vec![SortDirection::Ascending, SortDirection::Ascending];

    sort_rows(&mut rows, &col_names, &exprs, &dirs);

    let vals: Vec<i64> = rows
        .iter()
        .map(|r| match r[0] {
            Value::BigInt(n) => n,
            _ => panic!("expected BigInt"),
        })
        .collect();
    assert_eq!(vals, vec![1, 2, 3, 4]);
}

#[test]
fn test_compare_rows_ascending() {
    let col_names = vec!["val".to_string()];
    let layout =
        Arc::new(crate::query::executor::streaming::slot::SlotLayout::from_names(&col_names));
    let exprs = vec![make_sort_expr("val")];
    let dirs = vec![SortDirection::Ascending];

    let a = vec![Value::BigInt(1)];
    let b = vec![Value::BigInt(2)];

    assert_eq!(
        compare_two_rows_for_merge(&a, &b, &layout, &exprs, &dirs),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        compare_two_rows_for_merge(&b, &a, &layout, &exprs, &dirs),
        std::cmp::Ordering::Greater
    );
    assert_eq!(
        compare_two_rows_for_merge(&a, &a, &layout, &exprs, &dirs),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn test_compare_rows_descending() {
    let col_names = vec!["val".to_string()];
    let layout =
        Arc::new(crate::query::executor::streaming::slot::SlotLayout::from_names(&col_names));
    let exprs = vec![make_sort_expr("val")];
    let dirs = vec![SortDirection::Descending];

    let a = vec![Value::BigInt(1)];
    let b = vec![Value::BigInt(2)];

    assert_eq!(
        compare_two_rows_for_merge(&a, &b, &layout, &exprs, &dirs),
        std::cmp::Ordering::Greater
    );
    assert_eq!(
        compare_two_rows_for_merge(&b, &a, &layout, &exprs, &dirs),
        std::cmp::Ordering::Less
    );
}

// ── Columnar batch sort (step B) ──

use crate::query::executor::streaming::chunk::{ColumnInfo, Schema};
use crate::query::executor::streaming::helpers::compare_values;

fn typed_chunk(rows: Vec<Vec<Value>>) -> DataChunk {
    let mut chunk = DataChunk::new(
        rows,
        Arc::new(Schema::new(vec![ColumnInfo {
            name: "val".to_string(),
            data_type: "bigint".to_string(),
        }])),
    );
    chunk.build_typed_columns(true);
    chunk
}

fn values_from(rows: &[Vec<Value>]) -> Vec<Value> {
    rows.iter().map(|r| r[0].clone()).collect()
}

/// Columnar sort must reproduce the row-based sort exactly for the same
/// input (typed columns, mixed kinds, NULLs, all supported types).
#[test]
fn test_columnar_sort_matches_row_sort() {
    use crate::core::types::expr::Expression;

    let cases: Vec<Vec<Vec<Value>>> = vec![
        // Typed i64.
        (0..30)
            .map(|i| vec![Value::BigInt((i * 7) % 30 - 15)])
            .collect(),
        // f64 with ties.
        vec![
            vec![Value::Double(1.5)],
            vec![Value::Double(-2.0)],
            vec![Value::Double(0.0)],
            vec![Value::Double(1.5)],
            vec![Value::Double(7.25)],
        ],
        // Utf8 strings.
        vec![
            vec![Value::String("pear".into())],
            vec![Value::String("apple".into())],
            vec![Value::String("fig".into())],
            vec![Value::String("Apple".into())],
        ],
        // Date values (stored as days in the typed column).
        vec![
            vec![Value::Date(
                crate::core::value::date_time::DateValue::from_days(20000),
            )],
            vec![Value::Date(
                crate::core::value::date_time::DateValue::from_days(100),
            )],
            vec![Value::Date(
                crate::core::value::date_time::DateValue::from_days(999),
            )],
        ],
        // NULLs (fallback column).
        vec![
            vec![Value::BigInt(3)],
            vec![Value::Null(crate::core::value::NullType::Null)],
            vec![Value::BigInt(1)],
            vec![Value::BigInt(2)],
        ],
        // Mixed kinds in the same column (fallback column).
        vec![
            vec![Value::BigInt(100)],
            vec![Value::Int(5)],
            vec![Value::BigInt(50)],
            vec![Value::Int(7)],
        ],
    ];

    for rows in cases {
        for direction in [SortDirection::Ascending, SortDirection::Descending] {
            let mut row_sort = rows.clone();
            sort_rows(
                &mut row_sort,
                &["val".to_string()],
                &[make_sort_expr("val")],
                &[direction],
            );

            let mut batch = ColumnarBatch::new(1);
            batch.append_chunk(&typed_chunk(rows.clone()));
            sort_columnar_batch(
                &mut batch,
                &["val".to_string()],
                &[make_sort_expr("val")],
                &[direction],
            );
            let columnar_rows = batch.to_rows();

            assert_eq!(
                values_from(&row_sort),
                values_from(&columnar_rows),
                "columnar sort diverges from row sort for {:?} ({:?})",
                rows,
                direction
            );
        }
    }
}

/// Columnar sort with a non-bare expression falls back to per-row key
/// evaluation and must still match the row-based result.
#[test]
fn test_columnar_sort_non_bare_expression() {
    use crate::core::types::operators::BinaryOperator;
    // Sort by `val + 1` — not a bare column reference.
    let expr = Expression::Binary {
        left: Box::new(make_sort_expr("val")),
        op: BinaryOperator::Add,
        right: Box::new(Expression::literal(Value::BigInt(1))),
    };
    let rows: Vec<Vec<Value>> = vec![
        vec![Value::BigInt(9)],
        vec![Value::BigInt(1)],
        vec![Value::BigInt(5)],
        vec![Value::BigInt(3)],
    ];
    let mut row_sort = rows.clone();
    sort_rows(
        &mut row_sort,
        &["val".to_string()],
        &[expr.clone()],
        &[SortDirection::Ascending],
    );

    let mut batch = ColumnarBatch::new(1);
    batch.append_chunk(&typed_chunk(rows.clone()));
    sort_columnar_batch(
        &mut batch,
        &["val".to_string()],
        &[expr],
        &[SortDirection::Ascending],
    );
    assert_eq!(values_from(&row_sort), values_from(&batch.to_rows()));
}

/// Multi-column columnar sort with mixed typed/fallback keys.
#[test]
fn test_columnar_sort_multi_column() {
    use crate::core::types::expr::Expression;
    let col_names = vec!["a".to_string(), "b".to_string()];
    let rows: Vec<Vec<Value>> = vec![
        vec![Value::BigInt(1), Value::String("z".into())],
        vec![Value::BigInt(2), Value::String("a".into())],
        vec![Value::BigInt(1), Value::String("m".into())],
        vec![Value::BigInt(3), Value::String("k".into())],
    ];
    let exprs = vec![Expression::variable("a"), Expression::variable("b")];
    let dirs = vec![SortDirection::Ascending, SortDirection::Ascending];

    let mut row_sort = rows.clone();
    sort_rows(&mut row_sort, &col_names, &exprs, &dirs);

    let mut chunk = DataChunk::new(
        rows.clone(),
        Arc::new(Schema::new(
            col_names
                .iter()
                .map(|n| ColumnInfo {
                    name: n.clone(),
                    data_type: "unknown".to_string(),
                })
                .collect(),
        )),
    );
    chunk.build_typed_columns(true);

    let mut batch = ColumnarBatch::new(2);
    batch.append_chunk(&chunk);
    sort_columnar_batch(&mut batch, &col_names, &exprs, &dirs);
    assert_eq!(row_sort, batch.to_rows());
}

/// A fallback column must keep `compare_values` semantics inside the batch
/// (mixed Int/BigInt compare by `to_string`).
#[test]
fn test_columnar_batch_fallback_compare_semantics() {
    use crate::core::value::NullType;
    let mut batch = ColumnarBatch::new(1);
    batch.append_chunk(&typed_chunk(vec![
        vec![Value::BigInt(100)],
        vec![Value::Int(5)],
    ]));
    assert!(!batch.column(0).is_typed(), "mixed kinds must fall back");
    // compare_values(Int(5), BigInt(100)) sorts "100" before "5".
    assert_eq!(
        batch.compare_rows_at(0, 0, 1),
        compare_values(&Value::BigInt(100), &Value::Int(5))
    );
    assert_eq!(batch.column(0).value_at(1), Value::Int(5));
    let _ = NullType::Null;
}
