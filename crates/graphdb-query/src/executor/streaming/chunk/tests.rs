use super::{set_typed_columns_enabled, DataChunk, RowPool, TypedColumn, TypedKind};
use crate::executor::streaming::slot::SlotLayout;
use graphdb_core::types::expr::Expression;
use graphdb_core::types::operators::BinaryOperator;
use graphdb_core::types::storage_ids::VertexId;
use graphdb_core::value::date_time::DateTimeValue;
use graphdb_core::value::decimal128::Decimal128Value;
use graphdb_core::value::DateValue;
use graphdb_core::Value;
use graphdb_core::Vertex;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[test]
fn test_data_chunk_creation() {
    let rows = vec![vec![Value::string("a"), Value::string("b")]];
    let chunk = DataChunk::from_rows(rows);
    assert_eq!(chunk.len(), 1);
    assert_eq!(chunk.num_columns(), 2);
}

#[test]
fn test_data_chunk_empty() {
    let chunk = DataChunk::from_rows(vec![]);
    assert!(chunk.is_empty());
    assert_eq!(chunk.num_columns(), 0);
}

#[test]
fn test_data_chunk_type_inference() {
    let rows = vec![vec![Value::BigInt(42), Value::string("hello")]];
    let chunk = DataChunk::from_rows(rows);
    assert_eq!(chunk.schema.columns[0].data_type, "bigint");
    assert_eq!(chunk.schema.columns[1].data_type, "string");
}

#[test]
fn flat_property_column_hits_columnar_path() {
    let layout = Arc::new(SlotLayout::from_names(&[
        "p".to_string(),
        "p.age".to_string(),
    ]));
    let rows = vec![
        vec![
            Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
            Value::BigInt(30),
        ],
        vec![
            Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(2)))),
            Value::BigInt(20),
        ],
    ];
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    let expr = Expression::binary(
        Expression::property(Expression::variable("p"), "age"),
        BinaryOperator::GreaterThan,
        Expression::literal(Value::BigInt(28)),
    );
    let results = chunk
        .evaluate_expression(&expr, None)
        .expect("evaluate should succeed");
    assert_eq!(results, vec![Value::Bool(true), Value::Bool(false)]);
}

#[cfg(debug_assertions)]
#[test]
fn flat_column_promise_holds_for_flat_layout() {
    let layout = Arc::new(SlotLayout::from_names(&[
        "p".to_string(),
        "p.age".to_string(),
        "p.name".to_string(),
    ]));
    let rows = vec![vec![
        Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
        Value::BigInt(30),
        Value::string("Alice"),
    ]];
    let chunk = DataChunk::new_with_layout(rows, layout);
    let expr = Expression::binary(
        Expression::property(Expression::variable("p"), "age"),
        BinaryOperator::GreaterThan,
        Expression::literal(Value::BigInt(28)),
    );
    assert!(chunk.columnar_promise_holds(&expr));
}

#[cfg(debug_assertions)]
#[test]
fn flat_column_promise_does_not_hold_without_compound_slot() {
    let layout = Arc::new(SlotLayout::from_names(&["p".to_string()]));
    let rows = vec![vec![Value::Vertex(Box::new(Vertex::with_vid(
        VertexId::from_int64(1),
    )))]];
    let chunk = DataChunk::new_with_layout(rows, layout);
    let expr = Expression::property(Expression::variable("p"), "age");
    assert!(!chunk.columnar_promise_holds(&expr));
}

#[cfg(debug_assertions)]
#[test]
fn flat_column_promise_excludes_unsupported_nodes() {
    let layout = Arc::new(SlotLayout::from_names(&[
        "p".to_string(),
        "p.age".to_string(),
    ]));
    let rows = vec![vec![
        Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
        Value::BigInt(30),
    ]];
    let chunk = DataChunk::new_with_layout(rows, layout);
    let expr = Expression::function(
        "abs".to_string(),
        vec![Expression::property(Expression::variable("p"), "age")],
    );
    assert!(!chunk.columnar_promise_holds(&expr));
}

#[test]
fn columnar_stats_record_hits_and_misses() {
    let stats = Arc::new(crate::executor::streaming::runtime::ColumnarStats::new());
    let layout = Arc::new(SlotLayout::from_names(&[
        "p".to_string(),
        "p.age".to_string(),
    ]));
    let rows = vec![vec![
        Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
        Value::BigInt(30),
    ]];
    let mut chunk = DataChunk::new_with_layout(rows, layout).with_columnar_stats(stats.clone());

    let simple = Expression::binary(
        Expression::property(Expression::variable("p"), "age"),
        BinaryOperator::GreaterThan,
        Expression::literal(Value::BigInt(28)),
    );
    chunk
        .evaluate_expression(&simple, None)
        .expect("columnar evaluation should succeed");
    assert_eq!(stats.columnar_hits.load(Ordering::Relaxed), 1);
    assert_eq!(stats.columnar_misses.load(Ordering::Relaxed), 0);
    assert_eq!(stats.hit_rate(), 1.0);

    let complex = Expression::function(
        "abs".to_string(),
        vec![Expression::property(Expression::variable("p"), "age")],
    );
    chunk
        .evaluate_expression(&complex, None)
        .expect("per-row evaluation should succeed");
    assert_eq!(stats.columnar_hits.load(Ordering::Relaxed), 1);
    assert_eq!(stats.columnar_misses.load(Ordering::Relaxed), 1);
    assert!((stats.hit_rate() - 0.5).abs() < 1e-9);
}

#[test]
fn column_cache_is_lazy_and_deferred_after_take_indices() {
    let layout = Arc::new(SlotLayout::from_names(&[
        "p".to_string(),
        "p.age".to_string(),
    ]));
    let rows = vec![
        vec![
            Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(1)))),
            Value::BigInt(30),
        ],
        vec![
            Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(2)))),
            Value::BigInt(20),
        ],
    ];
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    assert!(chunk.columns.is_none(), "no proactive materialisation");

    let col = chunk.get_column(1).expect("age column");
    assert_eq!(col, vec![Value::BigInt(30), Value::BigInt(20)]);
    assert!(chunk.columns.is_some(), "materialised and cached on demand");

    let mut selected = chunk.take_indices(&[1]);
    assert!(
        selected.columns.is_none(),
        "columnar cache rebuild is deferred after take_indices"
    );
    assert_eq!(
        selected.get_column(1).expect("age column"),
        vec![Value::BigInt(20)]
    );
}

#[test]
fn typed_columns_build_pure_bigint_column() {
    let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
    let rows: Vec<Vec<Value>> = (0..100)
        .map(|i| vec![Value::BigInt((i % 1000) as i64)])
        .collect();
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    let bytes = chunk.build_typed_columns(true);
    let typed = chunk.typed_column(0).expect("typed column built");
    assert!(matches!(typed, TypedColumn::I64(_)), "expected I64 layout");
    assert!(bytes > 0, "typed allocation must be accounted");
    assert_eq!(
        typed.value_at(5),
        Some(Value::BigInt(5)),
        "O(1) indexed materialization"
    );
}

#[test]
fn typed_columns_probe_null_leading_and_fallback_on_mixed() {
    let layout = Arc::new(SlotLayout::from_names(&[
        "n".to_string(),
        "mixed".to_string(),
    ]));
    let rows = vec![
        vec![
            Value::Null(graphdb_core::value::NullType::Null),
            Value::BigInt(1),
        ],
        vec![Value::BigInt(2), Value::Double(2.0)],
    ];
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.build_typed_columns(true);
    assert!(
        matches!(chunk.typed_column(0), Some(TypedColumn::NullableI64(..))),
        "leading NULL probes the first non-NULL value (NullableI64)"
    );
    assert_eq!(
        chunk.typed_column(0).and_then(|c| c.value_at(0)),
        Some(Value::Null(graphdb_core::value::NullType::Null)),
        "NULL placeholder preserved by the bitmap"
    );
    assert!(matches!(
        chunk.typed_column(1),
        Some(TypedColumn::Fallback(_))
    ));
}

#[test]
fn typed_columns_all_null_column_falls_back() {
    let layout = Arc::new(SlotLayout::from_names(&["n".to_string()]));
    let rows = vec![
        vec![Value::Null(graphdb_core::value::NullType::Null)],
        vec![Value::Null(graphdb_core::value::NullType::Null)],
    ];
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.build_typed_columns(true);
    assert!(matches!(
        chunk.typed_column(0),
        Some(TypedColumn::Fallback(_))
    ));
}

#[test]
fn typed_columns_build_datetime_and_decimal_columns() {
    let layout = Arc::new(SlotLayout::from_names(&[
        "dt".to_string(),
        "dec".to_string(),
    ]));
    let mut rows: Vec<Vec<Value>> = (0..10)
        .map(|i| {
            vec![
                Value::DateTime(DateTimeValue {
                    year: 2024,
                    month: 1,
                    day: i as u32 + 1,
                    hour: 0,
                    minute: 0,
                    sec: 0,
                    microsec: 0,
                }),
                Value::Decimal128(Decimal128Value::from_i64(i as i64 * 100)),
            ]
        })
        .collect();
    rows[2][0] = Value::Null(graphdb_core::value::NullType::Null);
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.build_typed_columns(true);
    let dt = chunk.typed_column(0).expect("datetime column typed");
    assert!(
        matches!(dt, TypedColumn::NullableDateTime(..)),
        "expected NullableDateTime layout (NULL at row 2)"
    );
    assert_eq!(
        dt.value_at(1),
        Some(Value::DateTime(DateTimeValue {
            year: 2024,
            month: 1,
            day: 2,
            hour: 0,
            minute: 0,
            sec: 0,
            microsec: 0,
        })),
        "DateTime round-trips through micros"
    );
    assert_eq!(
        dt.value_at(2),
        Some(Value::Null(graphdb_core::value::NullType::Null))
    );
    let dec = chunk.typed_column(1).expect("decimal column typed");
    assert!(
        matches!(dec, TypedColumn::Decimal(_)),
        "expected Decimal layout"
    );
    assert_eq!(
        dec.value_at(5),
        Some(Value::Decimal128(Decimal128Value::from_i64(500)))
    );
}

#[test]
fn typed_columns_build_date_and_string_columns() {
    let layout = Arc::new(SlotLayout::from_names(&[
        "d".to_string(),
        "s".to_string(),
        "mixed_str".to_string(),
    ]));
    let mut rows: Vec<Vec<Value>> = (0..10)
        .map(|i| {
            vec![
                Value::Date(DateValue {
                    year: 2024,
                    month: 1,
                    day: i as u32 + 1,
                }),
                Value::string(format!("s{i}")),
                Value::string("constant"),
            ]
        })
        .collect();
    rows[3][2] = Value::BigInt(1);
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.build_typed_columns(true);
    let date = chunk.typed_column(0).expect("date column typed");
    assert!(matches!(date, TypedColumn::Date(_)), "expected Date layout");
    assert_eq!(
        date.value_at(1),
        Some(Value::Date(DateValue {
            year: 2024,
            month: 1,
            day: 2
        })),
        "O(1) indexed materialization"
    );
    let str = chunk.typed_column(1).expect("string column typed");
    assert!(matches!(str, TypedColumn::Utf8(_)), "expected Utf8 layout");
    assert_eq!(str.value_at(2), Some(Value::string("s2")));
    assert!(
        matches!(chunk.typed_column(2), Some(TypedColumn::Fallback(_)),),
        "mixed string column falls back"
    );
}

#[test]
fn typed_columns_survive_take_indices() {
    let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
    let rows: Vec<Vec<Value>> = (0..10).map(|i| vec![Value::BigInt(i as i64)]).collect();
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.build_typed_columns(true);
    let mut selected = chunk.take_indices(&[0, 2, 4]);
    assert!(matches!(
        selected.typed_column(0),
        Some(TypedColumn::I64(_))
    ));
    assert_eq!(
        selected.get_column(0).expect("column"),
        vec![Value::BigInt(0), Value::BigInt(2), Value::BigInt(4)]
    );
}

#[test]
fn typed_eval_matches_value_path_semantics() {
    let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
    let rows: Vec<Vec<Value>> = (0..100)
        .map(|i| vec![Value::BigInt((i % 1000) as i64)])
        .collect();
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.build_typed_columns(true);

    let expr = Expression::binary(
        Expression::variable("k0"),
        BinaryOperator::GreaterThan,
        Expression::literal(Value::BigInt(500)),
    );
    let typed_result = chunk.evaluate_expression(&expr, None).expect("eval");
    assert_eq!(typed_result.len(), 100);

    let expected: Vec<Value> = (0..100).map(|i| Value::Bool((i % 1000) > 500)).collect();
    assert_eq!(typed_result, expected);
}

#[test]
fn typed_eval_matches_value_path_for_date_and_string() {
    let layout = Arc::new(SlotLayout::from_names(&["d".to_string(), "s".to_string()]));
    let rows: Vec<Vec<Value>> = (0..100)
        .map(|i| {
            vec![
                Value::Date(DateValue {
                    year: 2024,
                    month: 1,
                    day: (i % 28) as u32 + 1,
                }),
                Value::string(format!("v{:03}", i)),
            ]
        })
        .collect();
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.build_typed_columns(true);
    assert!(matches!(chunk.typed_column(0), Some(TypedColumn::Date(_))));
    assert!(matches!(chunk.typed_column(1), Some(TypedColumn::Utf8(_))));

    let pivot = DateValue {
        year: 2024,
        month: 1,
        day: 15,
    };
    let date_expr = Expression::binary(
        Expression::variable("d"),
        BinaryOperator::LessThan,
        Expression::literal(Value::Date(pivot)),
    );
    let typed = chunk.evaluate_expression(&date_expr, None).expect("eval");
    let expected: Vec<Value> = (0..100)
        .map(|i| Value::Bool(((i % 28) as u32) + 1 < 15))
        .collect();
    assert_eq!(typed, expected);

    let str_expr = Expression::binary(
        Expression::variable("s"),
        BinaryOperator::GreaterThanOrEqual,
        Expression::literal(Value::string("v050")),
    );
    let typed = chunk.evaluate_expression(&str_expr, None).expect("eval");
    let expected: Vec<Value> = (0..100).map(|i| Value::Bool(i >= 50)).collect();
    assert_eq!(typed, expected);
}

#[test]
fn typed_eval_matches_value_path_for_datetime_and_decimal() {
    let layout = Arc::new(SlotLayout::from_names(&[
        "dt".to_string(),
        "dec".to_string(),
    ]));
    let rows: Vec<Vec<Value>> = (0..100)
        .map(|i| {
            vec![
                Value::DateTime(DateTimeValue {
                    year: 2024,
                    month: 1,
                    day: (i % 28) as u32 + 1,
                    hour: 12,
                    minute: 0,
                    sec: 0,
                    microsec: 0,
                }),
                Value::Decimal128(Decimal128Value::from_i64(i as i64 * 7)),
            ]
        })
        .collect();
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.build_typed_columns(true);
    assert!(matches!(
        chunk.typed_column(0),
        Some(TypedColumn::DateTime(_))
    ));
    assert!(matches!(
        chunk.typed_column(1),
        Some(TypedColumn::Decimal(_))
    ));

    let pivot = DateTimeValue {
        year: 2024,
        month: 1,
        day: 15,
        hour: 12,
        minute: 0,
        sec: 0,
        microsec: 0,
    };
    let dt_expr = Expression::binary(
        Expression::variable("dt"),
        BinaryOperator::LessThan,
        Expression::literal(Value::DateTime(pivot)),
    );
    let typed = chunk.evaluate_expression(&dt_expr, None).expect("eval");
    let expected: Vec<Value> = (0..100)
        .map(|i| Value::Bool(((i % 28) as u32) + 1 < 15))
        .collect();
    assert_eq!(
        typed, expected,
        "DateTime typed ordering matches the field order"
    );

    let dec_expr = Expression::binary(
        Expression::variable("dec"),
        BinaryOperator::GreaterThanOrEqual,
        Expression::literal(Value::Decimal128(Decimal128Value::from_i64(350))),
    );
    let typed = chunk.evaluate_expression(&dec_expr, None).expect("eval");
    let expected: Vec<Value> = (0..100).map(|i| Value::Bool(i >= 50)).collect();
    assert_eq!(
        typed, expected,
        "Decimal typed comparison uses decimal semantics"
    );
}

#[test]
fn typed_eval_property_predicate_hits_typed_batch_path() {
    use std::sync::atomic::Ordering;
    let layout = Arc::new(SlotLayout::from_names(&[
        "p".to_string(),
        "p.age".to_string(),
    ]));
    let rows: Vec<Vec<Value>> = (0..100)
        .map(|i| {
            vec![
                Value::Vertex(Box::new(Vertex::with_vid(VertexId::from_int64(
                    i as i64 + 1,
                )))),
                Value::BigInt((i as i64 % 50) + 1),
            ]
        })
        .collect();
    let stats = Arc::new(crate::executor::streaming::runtime::ColumnarStats::new());
    let mut chunk = DataChunk::new_with_layout(rows, layout).with_columnar_stats(stats.clone());
    chunk.build_typed_columns(true);
    assert!(
        matches!(chunk.typed_column(1), Some(TypedColumn::I64(_))),
        "flat property column must be typed I64"
    );

    let expr = Expression::binary(
        Expression::property(Expression::variable("p"), "age"),
        BinaryOperator::GreaterThan,
        Expression::literal(Value::BigInt(20)),
    );
    let results = chunk.evaluate_expression(&expr, None).expect("eval");
    assert_eq!(results.len(), 100);
    let expected: Vec<Value> = (0..100)
        .map(|i| Value::Bool((i as i64 % 50) + 1 > 20))
        .collect();
    assert_eq!(results, expected);
    assert_eq!(
        stats.columnar_typed_hits.load(Ordering::Relaxed),
        1,
        "property predicate must be served by the typed batch path"
    );
}

#[test]
fn typed_eval_supports_arithmetic_and_cast() {
    let layout = Arc::new(SlotLayout::from_names(&["a".to_string(), "b".to_string()]));
    let rows = vec![
        vec![Value::BigInt(40), Value::BigInt(2)],
        vec![Value::BigInt(10), Value::BigInt(5)],
    ];
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.build_typed_columns(true);

    let add = Expression::binary(
        Expression::variable("a"),
        BinaryOperator::Add,
        Expression::variable("b"),
    );
    assert_eq!(
        chunk.evaluate_expression(&add, None).expect("add"),
        vec![Value::BigInt(42), Value::BigInt(15)]
    );

    let cast = Expression::TypeCast {
        expression: Box::new(Expression::variable("a")),
        target_type: graphdb_core::DataType::Double,
    };
    assert_eq!(
        chunk.evaluate_expression(&cast, None).expect("cast"),
        vec![Value::Double(40.0), Value::Double(10.0)]
    );
}

#[test]
fn typed_eval_promotes_mixed_int_kinds() {
    let stats = Arc::new(crate::executor::streaming::runtime::ColumnarStats::new());
    let layout = Arc::new(SlotLayout::from_names(&["a".to_string()]));
    let rows: Vec<Vec<Value>> = (0..10).map(|i| vec![Value::BigInt(i * 10)]).collect();
    let mut chunk = DataChunk::new_with_layout(rows, layout).with_columnar_stats(stats.clone());
    chunk.build_typed_columns(true);
    assert!(matches!(chunk.typed_column(0), Some(TypedColumn::I64(_))));

    let add = Expression::binary(
        Expression::variable("a"),
        BinaryOperator::Add,
        Expression::literal(Value::Int(1)),
    );
    assert_eq!(
        chunk.evaluate_expression(&add, None).expect("add"),
        (0..10)
            .map(|i| Value::BigInt(i * 10 + 1))
            .collect::<Vec<_>>()
    );

    let less = Expression::binary(
        Expression::variable("a"),
        BinaryOperator::LessThan,
        Expression::literal(Value::Int(45)),
    );
    assert_eq!(
        chunk.evaluate_expression(&less, None).expect("less"),
        (0..10)
            .map(|i| Value::Bool(i * 10 < 45))
            .collect::<Vec<_>>()
    );

    assert_eq!(
        stats.columnar_typed_hits.load(Ordering::Relaxed),
        2,
        "mixed int-kind expressions must be served by the typed batch path"
    );
}

#[test]
fn typed_eval_promotes_int_to_double() {
    let stats = Arc::new(crate::executor::streaming::runtime::ColumnarStats::new());
    let layout = Arc::new(SlotLayout::from_names(&["a".to_string()]));
    let rows: Vec<Vec<Value>> = (0..10).map(|i| vec![Value::Int(i * 10)]).collect();
    let mut chunk = DataChunk::new_with_layout(rows, layout).with_columnar_stats(stats.clone());
    chunk.build_typed_columns(true);
    assert!(matches!(chunk.typed_column(0), Some(TypedColumn::I32(_))));

    let add = Expression::binary(
        Expression::variable("a"),
        BinaryOperator::Add,
        Expression::literal(Value::Double(0.5)),
    );
    assert_eq!(
        chunk.evaluate_expression(&add, None).expect("add"),
        (0..10)
            .map(|i| Value::Double(i as f64 * 10.0 + 0.5))
            .collect::<Vec<_>>()
    );

    let mul = Expression::binary(
        Expression::literal(Value::Double(2.0)),
        BinaryOperator::Multiply,
        Expression::variable("a"),
    );
    assert_eq!(
        chunk.evaluate_expression(&mul, None).expect("mul"),
        (0..10)
            .map(|i| Value::Double(i as f64 * 20.0))
            .collect::<Vec<_>>()
    );

    let ge = Expression::binary(
        Expression::variable("a"),
        BinaryOperator::GreaterThanOrEqual,
        Expression::literal(Value::Double(25.0)),
    );
    assert_eq!(
        chunk.evaluate_expression(&ge, None).expect("ge"),
        (0..10)
            .map(|i| Value::Bool(i as f64 * 10.0 >= 25.0))
            .collect::<Vec<_>>()
    );

    assert_eq!(
        stats.columnar_typed_hits.load(Ordering::Relaxed),
        3,
        "int-vs-double expressions must be served by the typed batch path"
    );
}

#[test]
fn typed_eval_cross_kind_nan_matches_value_semantics() {
    let layout = Arc::new(SlotLayout::from_names(&["a".to_string()]));
    let rows: Vec<Vec<Value>> = (0..4).map(|i| vec![Value::Int(i)]).collect();
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.build_typed_columns(true);

    let nan = Expression::literal(Value::Double(f64::NAN));
    let cases = [
        (
            BinaryOperator::Equal,
            vec![false, false, false, false],
            "int == NaN is false",
        ),
        (
            BinaryOperator::NotEqual,
            vec![true, true, true, true],
            "int != NaN is true",
        ),
        (
            BinaryOperator::LessThan,
            vec![false, false, false, false],
            "int < NaN is false",
        ),
        (
            BinaryOperator::LessThanOrEqual,
            vec![true, true, true, true],
            "int <= NaN is true (partial_cmp unwraps to Equal)",
        ),
        (
            BinaryOperator::GreaterThan,
            vec![false, false, false, false],
            "int > NaN is false",
        ),
        (
            BinaryOperator::GreaterThanOrEqual,
            vec![true, true, true, true],
            "int >= NaN is true (partial_cmp unwraps to Equal)",
        ),
    ];
    for (op, expected, msg) in cases {
        let expr = Expression::binary(Expression::variable("a"), op, nan.clone());
        assert_eq!(
            chunk.evaluate_expression(&expr, None).expect("eval"),
            expected.iter().map(|&b| Value::Bool(b)).collect::<Vec<_>>(),
            "{}",
            msg
        );
    }

    let lt = Expression::binary(
        Expression::variable("a"),
        BinaryOperator::LessThan,
        Expression::literal(Value::Double(2.5)),
    );
    assert_eq!(
        chunk.evaluate_expression(&lt, None).expect("eval"),
        vec![
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(false)
        ]
    );
}

#[test]
fn typed_columns_disabled_falls_back() {
    let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
    let rows: Vec<Vec<Value>> = (0..10).map(|i| vec![Value::BigInt(i as i64)]).collect();
    set_typed_columns_enabled(false);
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    let bytes = chunk.build_typed_columns(true);
    assert_eq!(bytes, 0);
    assert!(chunk.typed_column(0).is_none());
    set_typed_columns_enabled(true);
}

#[test]
fn row_pool_recycles_typed_columns() {
    let pool = RowPool::new(64, 1);
    let col = pool.acquire_typed(TypedKind::I64);
    pool.release_typed(col);
    let col = pool.acquire_typed(TypedKind::I64);
    match col {
        TypedColumn::I64(v) => assert!(v.capacity() >= 64, "recycled capacity"),
        _ => panic!("expected I64 column"),
    }
}

#[test]
fn selection_attachment_and_materialization() {
    let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
    let rows: Vec<Vec<Value>> = (0..10).map(|i| vec![Value::BigInt(i as i64)]).collect();
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.build_typed_columns(true);

    let chunk = chunk.with_selection(vec![0, 2, 4]);
    assert_eq!(chunk.visible_count(), 3);
    assert_eq!(chunk.visible_indices(), vec![0, 2, 4]);
    assert!(chunk.is_visible(2));
    assert!(!chunk.is_visible(1));

    let mut chunk = chunk;
    assert!(chunk.materialize_selection());
    assert_eq!(chunk.visible_count(), 3);
    assert!(chunk.selection().is_none());
    let col = chunk.get_column(0).expect("column");
    assert_eq!(
        col,
        vec![Value::BigInt(0), Value::BigInt(2), Value::BigInt(4)]
    );
    assert!(!chunk.materialize_selection());
}

#[test]
fn selection_preserves_typed_columns_until_materialized() {
    let layout = Arc::new(SlotLayout::from_names(&["k0".to_string()]));
    let rows: Vec<Vec<Value>> = (0..6).map(|i| vec![Value::BigInt(i as i64)]).collect();
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.build_typed_columns(true);
    let mut chunk = chunk.with_selection(vec![1, 3]);
    assert!(chunk.typed_column(0).is_some(), "typed layout kept");
    chunk.materialize_selection();
    let typed = chunk.typed_column(0).expect("typed layout gathered");
    assert_eq!(typed.to_values(), vec![Value::BigInt(1), Value::BigInt(3)]);
}

#[test]
fn typed_eval_differential_random() {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let n = 1024;

    let layout = Arc::new(SlotLayout::from_names(&["a".to_string(), "b".to_string()]));
    let rows: Vec<Vec<Value>> = (0..n)
        .map(|_| {
            vec![
                Value::BigInt(rng.gen_range(-1000..1000)),
                Value::BigInt(rng.gen_range(-1000..1000)),
            ]
        })
        .collect();

    let mut chunk_typed = DataChunk::new_with_layout(rows.clone(), Arc::clone(&layout));
    chunk_typed.build_typed_columns(true);

    let mut chunk_row = DataChunk::new_with_layout(rows, layout);

    for (op_name, op) in [
        ("==", BinaryOperator::Equal),
        ("!=", BinaryOperator::NotEqual),
        ("<", BinaryOperator::LessThan),
        ("<=", BinaryOperator::LessThanOrEqual),
        (">", BinaryOperator::GreaterThan),
        (">=", BinaryOperator::GreaterThanOrEqual),
    ] {
        let expr = Expression::binary(Expression::variable("a"), op, Expression::variable("b"));

        let result_typed = chunk_typed
            .evaluate_expression(&expr, None)
            .expect("typed eval");
        let result_row = chunk_row
            .evaluate_expression(&expr, None)
            .expect("row eval");

        assert_eq!(result_typed, result_row, "comparison {} mismatch", op_name);
    }

    for (op_name, op) in [
        ("+", BinaryOperator::Add),
        ("-", BinaryOperator::Subtract),
        ("*", BinaryOperator::Multiply),
    ] {
        let expr = Expression::binary(Expression::variable("a"), op, Expression::variable("b"));

        let result_typed = chunk_typed
            .evaluate_expression(&expr, None)
            .expect("typed eval");
        let result_row = chunk_row
            .evaluate_expression(&expr, None)
            .expect("row eval");

        assert_eq!(result_typed, result_row, "arithmetic {} mismatch", op_name);
    }
}

#[test]
fn typed_bool_column_and_or() {
    let layout = Arc::new(SlotLayout::from_names(&["x".to_string(), "y".to_string()]));
    let rows = vec![
        vec![Value::Bool(true), Value::Bool(true)],
        vec![Value::Bool(true), Value::Bool(false)],
        vec![Value::Bool(false), Value::Bool(true)],
        vec![Value::Bool(false), Value::Bool(false)],
    ];
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.build_typed_columns(true);

    assert!(
        matches!(chunk.typed_column(0), Some(TypedColumn::Bool(_))),
        "column x should be typed Bool"
    );
    assert!(
        matches!(chunk.typed_column(1), Some(TypedColumn::Bool(_))),
        "column y should be typed Bool"
    );

    let and_expr = Expression::binary(
        Expression::variable("x"),
        BinaryOperator::And,
        Expression::variable("y"),
    );
    let and_result = chunk
        .evaluate_expression(&and_expr, None)
        .expect("and eval");
    assert_eq!(
        and_result,
        vec![
            Value::Bool(true),
            Value::Bool(false),
            Value::Bool(false),
            Value::Bool(false)
        ]
    );

    let or_expr = Expression::binary(
        Expression::variable("x"),
        BinaryOperator::Or,
        Expression::variable("y"),
    );
    let or_result = chunk.evaluate_expression(&or_expr, None).expect("or eval");
    assert_eq!(
        or_result,
        vec![
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(false)
        ]
    );
}

// ── Nullable (validity bitmap) typed columns ──

#[test]
fn nullable_column_builds_typed_with_bitmap() {
    let layout = Arc::new(SlotLayout::from_names(&["a".to_string()]));
    let rows = vec![
        vec![Value::BigInt(10)],
        vec![Value::Null(graphdb_core::value::NullType::Null)],
        vec![Value::BigInt(30)],
        vec![Value::Null(graphdb_core::value::NullType::Null)],
        vec![Value::BigInt(50)],
    ];
    let mut chunk = DataChunk::new_with_layout(rows.clone(), layout);
    chunk.build_typed_columns(true);
    assert!(
        matches!(chunk.typed_column(0), Some(TypedColumn::NullableI64(..))),
        "NULL-bearing homogeneous column must stay typed via the bitmap"
    );
    let col = chunk.typed_column(0).expect("typed column");
    assert_eq!(
        col.value_at(0),
        Some(Value::BigInt(10)),
        "valid rows materialize normally"
    );
    assert_eq!(
        col.value_at(1),
        Some(Value::Null(graphdb_core::value::NullType::Null)),
        "invalid rows materialize as NULL"
    );
    assert_eq!(
        col.to_values(),
        rows.iter().map(|r| r[0].clone()).collect::<Vec<_>>(),
        "to_values must preserve NULL rows"
    );
}

#[test]
fn nullable_eval_matches_row_path() {
    use graphdb_core::value::NullType;
    let layout = Arc::new(SlotLayout::from_names(&["a".to_string(), "b".to_string()]));
    let rows = vec![
        vec![Value::BigInt(10), Value::BigInt(2)],
        vec![Value::Null(NullType::Null), Value::BigInt(5)],
        vec![Value::BigInt(30), Value::Null(NullType::Null)],
        vec![Value::Null(NullType::Null), Value::Null(NullType::Null)],
        vec![Value::BigInt(50), Value::BigInt(10)],
    ];

    let mut chunk_typed = DataChunk::new_with_layout(rows.clone(), Arc::clone(&layout));
    chunk_typed.build_typed_columns(true);
    assert!(
        matches!(
            chunk_typed.typed_column(0),
            Some(TypedColumn::NullableI64(..))
        ),
        "column a must be NullableI64"
    );

    let mut chunk_row = DataChunk::new_with_layout(rows, layout);

    for (op_name, op) in [
        ("==", BinaryOperator::Equal),
        ("!=", BinaryOperator::NotEqual),
        ("<", BinaryOperator::LessThan),
        ("<=", BinaryOperator::LessThanOrEqual),
        (">", BinaryOperator::GreaterThan),
        (">=", BinaryOperator::GreaterThanOrEqual),
    ] {
        let expr = Expression::binary(Expression::variable("a"), op, Expression::variable("b"));
        let result_typed = chunk_typed
            .evaluate_expression(&expr, None)
            .expect("typed eval");
        let result_row = chunk_row
            .evaluate_expression(&expr, None)
            .expect("row eval");
        assert_eq!(result_typed, result_row, "comparison {} mismatch", op_name);
    }

    for (op_name, op) in [
        ("+", BinaryOperator::Add),
        ("-", BinaryOperator::Subtract),
        ("*", BinaryOperator::Multiply),
    ] {
        let expr = Expression::binary(Expression::variable("a"), op, Expression::variable("b"));
        let result_typed = chunk_typed.evaluate_expression(&expr, None);
        let result_row = chunk_row.evaluate_expression(&expr, None);
        assert_eq!(
            result_typed, result_row,
            "arithmetic {} must match the row path (including NULL errors)",
            op_name
        );
    }
}

#[test]
fn nullable_eval_literal_and_cast() {
    use graphdb_core::value::NullType;
    let layout = Arc::new(SlotLayout::from_names(&["a".to_string()]));
    let rows = vec![
        vec![Value::BigInt(1)],
        vec![Value::Null(NullType::Null)],
        vec![Value::BigInt(3)],
    ];
    let mut chunk = DataChunk::new_with_layout(rows, layout);
    chunk.build_typed_columns(true);

    // Comparisons treat NULL as the smallest value (Value type priority):
    // `NULL < 2` is true, mirroring the per-row evaluator.
    let less = Expression::binary(
        Expression::variable("a"),
        BinaryOperator::LessThan,
        Expression::literal(Value::BigInt(2)),
    );
    assert_eq!(
        chunk.evaluate_expression(&less, None).expect("less"),
        vec![Value::Bool(true), Value::Bool(true), Value::Bool(false),]
    );

    // Arithmetic on NULL rows errors in the value path; the typed path must
    // fall back and surface the same error.
    let add = Expression::binary(
        Expression::variable("a"),
        BinaryOperator::Add,
        Expression::literal(Value::BigInt(10)),
    );
    assert!(
        chunk.evaluate_expression(&add, None).is_err(),
        "NULL arithmetic must error exactly like the row path"
    );

    // Cast preserves NULL (NULL -> NULL in the value path).
    let cast = Expression::TypeCast {
        expression: Box::new(Expression::variable("a")),
        target_type: graphdb_core::DataType::Double,
    };
    assert_eq!(
        chunk.evaluate_expression(&cast, None).expect("cast"),
        vec![
            Value::Double(1.0),
            Value::Null(NullType::Null),
            Value::Double(3.0),
        ]
    );
}

#[test]
fn nullable_column_served_by_typed_batch_path() {
    use graphdb_core::value::NullType;
    let stats = Arc::new(crate::executor::streaming::runtime::ColumnarStats::new());
    let layout = Arc::new(SlotLayout::from_names(&["a".to_string()]));
    let rows: Vec<Vec<Value>> = (0..100)
        .map(|i| {
            if i % 50 == 0 {
                vec![Value::Null(NullType::Null)]
            } else {
                vec![Value::BigInt(i as i64)]
            }
        })
        .collect();
    let mut chunk = DataChunk::new_with_layout(rows, layout).with_columnar_stats(stats.clone());
    chunk.build_typed_columns(true);

    let expr = Expression::binary(
        Expression::variable("a"),
        BinaryOperator::GreaterThan,
        Expression::literal(Value::BigInt(80)),
    );
    let results = chunk.evaluate_expression(&expr, None).expect("eval");
    assert_eq!(results.len(), 100);
    assert_eq!(
        results[0],
        Value::Bool(false),
        "NULL compares as the smallest value, so NULL > 80 is false"
    );
    assert_eq!(results[99], Value::Bool(true));
    assert_eq!(
        stats.columnar_typed_hits.load(Ordering::Relaxed),
        1,
        "nullable predicate must be served by the typed batch path"
    );
}

#[test]
fn nullable_bool_and_or_errors_like_row_path() {
    use graphdb_core::value::NullType;
    let layout = Arc::new(SlotLayout::from_names(&["x".to_string(), "y".to_string()]));
    let rows = vec![
        vec![Value::Bool(true), Value::Bool(false)],
        vec![Value::Null(NullType::Null), Value::Bool(true)],
        vec![Value::Bool(false), Value::Null(NullType::Null)],
        vec![Value::Null(NullType::Null), Value::Null(NullType::Null)],
    ];
    let mut chunk_typed = DataChunk::new_with_layout(rows.clone(), Arc::clone(&layout));
    chunk_typed.build_typed_columns(true);
    assert!(
        matches!(
            chunk_typed.typed_column(0),
            Some(TypedColumn::NullableBool(..))
        ),
        "column x must be NullableBool"
    );
    let mut chunk_row = DataChunk::new_with_layout(rows, layout);

    for (op_name, op) in [("and", BinaryOperator::And), ("or", BinaryOperator::Or)] {
        let expr = Expression::binary(Expression::variable("x"), op, Expression::variable("y"));
        let result_typed = chunk_typed.evaluate_expression(&expr, None);
        let result_row = chunk_row.evaluate_expression(&expr, None);
        assert_eq!(
            result_typed, result_row,
            "{op_name} on NULL rows must error like the row path"
        );
        assert!(result_typed.is_err(), "{op_name} must error on NULL rows");
    }
}

#[test]
fn columnar_batch_keeps_nullable_typed_columns() {
    use crate::executor::streaming::chunk::columnar_batch::ColumnarBatch;
    use crate::executor::streaming::chunk::schema::{ColumnInfo, Schema};
    use graphdb_core::value::NullType;

    let mut chunk = DataChunk::new(
        vec![
            vec![Value::BigInt(1)],
            vec![Value::Null(NullType::Null)],
            vec![Value::BigInt(3)],
        ],
        Arc::new(Schema::new(vec![ColumnInfo {
            name: "a".to_string(),
            data_type: "bigint".to_string(),
        }])),
    );
    chunk.build_typed_columns(true);
    assert!(matches!(
        chunk.typed_column(0),
        Some(TypedColumn::NullableI64(..))
    ));

    let mut batch = ColumnarBatch::new(1);
    batch.append_chunk(&chunk);
    assert_eq!(batch.num_rows(), 3);
    assert!(batch.column(0).is_typed(), "nullable column stays typed");
    assert_eq!(
        batch.to_rows(),
        vec![
            vec![Value::BigInt(1)],
            vec![Value::Null(NullType::Null)],
            vec![Value::BigInt(3)],
        ]
    );
    // NULL sorts last (mirrors compare_values).
    assert_eq!(
        batch.column(0).compare_at(1, 0),
        std::cmp::Ordering::Greater
    );
    assert_eq!(
        batch.column(0).compare_at(1, 2),
        std::cmp::Ordering::Greater
    );
}

#[test]
fn columnar_and_row_paths_agree_fuzz() {
    use crate::executor::expression::evaluator::ExpressionEvaluator;
    use crate::executor::streaming::context::BorrowedRowContext;

    // Deterministic xorshift PRNG keeps failures reproducible.
    let mut seed: u64 = 0x1234_5678_9ABC_DEF0;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    for case in 0..64u32 {
        let n = 1 + (rand() % 48) as usize;
        let layout = Arc::new(SlotLayout::from_names(&[
            "p".to_string(),
            "p.age".to_string(),
            "p.score".to_string(),
        ]));
        let rows: Vec<Vec<Value>> = (0..n)
            .map(|_| {
                vec![
                    Value::Vertex(Box::new(graphdb_core::Vertex::with_vid(
                        VertexId::from_int64(rand() as i64),
                    ))),
                    Value::BigInt((rand() % 100) as i64),
                    Value::Double(((rand() % 1000) as f64) / 10.0),
                ]
            })
            .collect();

        // Expression pool mixing comparisons, arithmetic, And/Or with
        // randomized constants.
        let c1 = (rand() % 100) as i64;
        let c2 = ((rand() % 1000) as f64) / 10.0;
        let age = || Expression::property(Expression::variable("p"), "age");
        let score = || Expression::property(Expression::variable("p"), "score");
        let lit_i = |v: i64| Expression::literal(Value::BigInt(v));
        let lit_d = |v: f64| Expression::literal(Value::Double(v));
        let bin = |l: Expression, op: BinaryOperator, r: Expression| Expression::binary(l, op, r);
        let expressions = vec![
            bin(age(), BinaryOperator::GreaterThan, lit_i(c1)),
            bin(age(), BinaryOperator::LessThanOrEqual, lit_i(c1)),
            bin(score(), BinaryOperator::LessThan, lit_d(c2)),
            bin(score(), BinaryOperator::GreaterThanOrEqual, lit_d(c2)),
            bin(
                bin(age(), BinaryOperator::Add, lit_i(5)),
                BinaryOperator::GreaterThan,
                lit_i(c1 + 3),
            ),
            bin(
                bin(age(), BinaryOperator::GreaterThan, lit_i(c1)),
                BinaryOperator::And,
                bin(score(), BinaryOperator::LessThan, lit_d(c2)),
            ),
            bin(
                bin(age(), BinaryOperator::Equal, lit_i(c1)),
                BinaryOperator::Or,
                bin(score(), BinaryOperator::NotEqual, lit_d(c2)),
            ),
        ];

        let mut chunk = DataChunk::new_with_layout(rows.clone(), layout.clone());
        chunk.build_typed_columns(true);
        for expr in &expressions {
            let batched = chunk
                .evaluate_expression(expr, None)
                .unwrap_or_else(|e| panic!("case {case}: batch eval failed: {e}"));
            assert_eq!(batched.len(), n, "case {case}: result length");
            for (i, row) in rows.iter().enumerate() {
                let mut ctx = BorrowedRowContext::new(row, layout.clone());
                let reference = ExpressionEvaluator::evaluate(expr, &mut ctx)
                    .unwrap_or_else(|e| panic!("case {case} row {i}: row eval failed: {e}"));
                assert_eq!(batched[i], reference, "case {case} expr {:?} row {i}", expr);
            }
        }
    }
}
