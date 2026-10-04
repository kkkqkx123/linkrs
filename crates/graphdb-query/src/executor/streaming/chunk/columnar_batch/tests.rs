use std::cmp::Ordering;
use std::sync::Arc;

use super::*;
use crate::executor::streaming::chunk::core::DataChunk;
use crate::executor::streaming::chunk::schema::{ColumnInfo, Schema};
use crate::executor::streaming::helpers::compare_values;
use graphdb_core::Value;

fn chunk_of(rows: Vec<Vec<Value>>) -> DataChunk {
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

#[test]
fn test_append_chunk_typed_i64() {
    let mut batch = ColumnarBatch::new(1);
    let chunk = chunk_of(vec![
        vec![Value::BigInt(1)],
        vec![Value::BigInt(2)],
        vec![Value::BigInt(3)],
    ]);
    batch.append_chunk(&chunk);
    assert_eq!(batch.num_rows(), 3);
    assert!(batch.column(0).is_typed());
    assert_eq!(batch.column(0).value_at(1), Value::BigInt(2));
    assert_eq!(batch.column(0).compare_at(0, 2), Ordering::Less);
    assert_eq!(
        batch.to_rows(),
        vec![
            vec![Value::BigInt(1)],
            vec![Value::BigInt(2)],
            vec![Value::BigInt(3)],
        ]
    );
}

#[test]
fn test_append_chunk_degrades_on_kind_mismatch() {
    let mut batch = ColumnarBatch::new(1);
    let c1 = chunk_of(vec![vec![Value::BigInt(1)], vec![Value::BigInt(2)]]);
    batch.append_chunk(&c1);
    assert!(batch.column(0).is_typed());
    // A later chunk carries an Int (typed I32) in the same column:
    // degrade to Fallback, preserving accumulated values.
    let mut c2 = DataChunk::new(
        vec![vec![Value::Int(7)], vec![Value::Int(8)]],
        Arc::new(Schema::new(vec![ColumnInfo {
            name: "val".to_string(),
            data_type: "int".to_string(),
        }])),
    );
    c2.build_typed_columns(true);
    batch.append_chunk(&c2);
    assert!(!batch.column(0).is_typed());
    assert_eq!(batch.num_rows(), 4);
    let rows = batch.to_rows();
    assert_eq!(rows[0][0], Value::BigInt(1));
    assert_eq!(rows[2][0], Value::Int(7));
    assert_eq!(rows[3][0], Value::Int(8));
}

#[test]
fn test_permute_and_truncate() {
    let mut batch = ColumnarBatch::new(1);
    batch.append_chunk(&chunk_of(vec![
        vec![Value::BigInt(3)],
        vec![Value::BigInt(1)],
        vec![Value::BigInt(2)],
    ]));
    // perm[i] = source index for position i → ascending order.
    batch.permute(&[1, 2, 0]);
    assert_eq!(batch.column(0).value_at(0), Value::BigInt(1));
    assert_eq!(batch.column(0).value_at(1), Value::BigInt(2));
    assert_eq!(batch.column(0).value_at(2), Value::BigInt(3));
    batch.truncate(2);
    assert_eq!(batch.num_rows(), 2);
    assert_eq!(batch.to_rows()[1][0], Value::BigInt(2));
}

#[test]
fn test_utf8_column_ordering() {
    let mut batch = ColumnarBatch::new(1);
    batch.append_chunk(&chunk_of(vec![
        vec![Value::String("pear".into())],
        vec![Value::String("apple".into())],
        vec![Value::String("fig".into())],
    ]));
    assert!(batch.column(0).is_typed());
    assert_eq!(
        batch.column(0).compare_at(1, 0),
        Ordering::Less,
        "apple < pear"
    );
    assert_eq!(
        batch.column(0).compare_at(2, 0),
        Ordering::Less,
        "fig < pear"
    );
}

#[test]
fn test_datetime_and_decimal_columns() {
    use graphdb_core::value::date_time::DateTimeValue;
    use graphdb_core::value::decimal128::Decimal128Value;
    let dt = |day: u32| {
        Value::DateTime(DateTimeValue {
            year: 2024,
            month: 1,
            day,
            hour: 0,
            minute: 0,
            sec: 0,
            microsec: 0,
        })
    };
    let mut batch = ColumnarBatch::new(2);
    let mut chunk = DataChunk::new(
        vec![
            vec![dt(3), Value::Decimal128(Decimal128Value::from_i64(30))],
            vec![dt(1), Value::Decimal128(Decimal128Value::from_i64(10))],
            vec![dt(2), Value::Decimal128(Decimal128Value::from_i64(20))],
        ],
        Arc::new(Schema::new(vec![
            ColumnInfo {
                name: "dt".to_string(),
                data_type: "datetime".to_string(),
            },
            ColumnInfo {
                name: "dec".to_string(),
                data_type: "decimal128".to_string(),
            },
        ])),
    );
    chunk.build_typed_columns(true);
    batch.append_chunk(&chunk);
    assert!(batch.column(0).is_typed());
    assert!(batch.column(1).is_typed());
    assert_eq!(
        batch.column(0).compare_at(0, 1),
        Ordering::Greater,
        "row 0 (day 3) > row 1 (day 1)"
    );
    assert_eq!(
        batch.column(1).compare_at(0, 2),
        Ordering::Greater,
        "30 > 20"
    );
    assert_eq!(
        batch.column(0).value_at(1),
        dt(1),
        "DateTime materializes from micros"
    );
    assert_eq!(
        batch.column(1).value_at(2),
        Value::Decimal128(Decimal128Value::from_i64(20))
    );
}

#[test]
fn test_compare_value_at_mixed_kind_falls_back() {
    let mut batch = ColumnarBatch::new(1);
    batch.append_chunk(&chunk_of(vec![vec![Value::BigInt(100)]]));
    // Int(5) vs BigInt(100): cross-type compare falls back to
    // compare_values semantics.
    assert_eq!(
        batch.compare_value_at(0, &Value::Int(5), 0),
        compare_values(&Value::Int(5), &Value::BigInt(100))
    );
    // Same-kind BigInt uses the raw fast path.
    assert_eq!(
        batch.compare_value_at(0, &Value::BigInt(50), 0),
        Ordering::Less
    );
}

#[test]
fn test_estimated_size() {
    let mut batch = ColumnarBatch::new(1);
    let chunk = chunk_of(vec![
        vec![Value::BigInt(1)],
        vec![Value::BigInt(2)],
        vec![Value::BigInt(3)],
    ]);
    batch.append_chunk(&chunk);
    assert!(batch.estimated_size() >= 3 * std::mem::size_of::<i64>());
}
