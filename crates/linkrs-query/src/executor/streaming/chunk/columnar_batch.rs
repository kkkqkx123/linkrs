//! Columnar batch accumulation for blocking operators
//!
//! Blocking operators (Sort/TopN) buffer all input before producing output.
//! Instead of buffering `Vec<Vec<Value>>` rows, rows are accumulated
//! column-major: homogeneous columns stay raw
//! (`Vec<i64>`/`Vec<f64>`/`Vec<i32>`/`Vec<bool>`/`Vec<i64>` days/`Vec<Arc<str>>`)
//! so that ordering operates on scalars without constructing one `Value`
//! per row. Columns that mix kinds (or hit NULLs) degrade to
//! [`BatchColumn::Fallback`], keeping the exact `Value` semantics of the
//! row-based path.
//!
//! ## Relationship to `TypedColumn`
//!
//! `TypedColumn` models a fixed-kind column inside a `DataChunk` for batch
//! expression evaluation, while `BatchColumn` models a column whose kind is
//! decided by its first append (`BatchColumn::Empty` until then) and which
//! supports accumulation (append/gather/permute). The two intentionally
//! share a variant set so `append_typed` can move a chunk's typed columns
//! into a batch without re-inspecting values; keep them in sync when adding
//! a kind.
//!
//! ## Relationship to `MaterializedBatch`
//!
//! Other blocking operators (aggregate, distinct, materialize, window) keep
//! their state in `linkrs_core::columnar::MaterializedBatch`, which is
//! column-oriented but stores one `Vec<Value>` per column and therefore does
//! not get the typed-scalar treatment. Treat `MaterializedBatch` as the
//! spill and persistence form; the in-memory streaming fast path uses
//! `TypedColumn` and `ColumnarBatch`.

use std::sync::Arc;

use crate::executor::streaming::chunk::columnar_common::column_variants;
use linkrs_core::value::decimal128::Decimal128Value;
use linkrs_core::Value;

mod collector;
mod operations;

#[cfg(test)]
mod tests;

pub use collector::ColumnarBatch;

// Column-major accumulation of one output column across chunks.
//
// The variant set is declared once by `column_variants!` in
// `columnar_common` and shared with `TypedColumn`; `len`, `is_empty`, and
// `estimated_size` are generated from that list.
// Accumulable blocking-operator state; see `columnar_common` for the list.
column_variants!(BatchColumn, Empty);
