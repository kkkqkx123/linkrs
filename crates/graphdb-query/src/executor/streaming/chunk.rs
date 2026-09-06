//! DataChunk: Basic unit of streaming execution
//!
//! A DataChunk represents a fixed-size batch of rows processed in streaming mode.
//! Typical size: 2048 rows (~8MB, OLAP vectorized batch)
//!
//! # Ownership & Memory Accounting Rules (M4)
//!
//! - **`rows`**: Owned `Vec<Vec<Value>>`. `Clone` performs an explicit deep
//!   copy (there is no shallow-clone path).
//! - **`schema`**: `Arc<Schema>` — shared reference, cheap `Arc::clone` on `Clone`.
//! - **`layout`**: `Arc<SlotLayout>` — always present; shared reference, cheap on `Clone`.
//! - **`memory_reservation`**: `Option<MemoryReservation>` / `Option<MemoryPoolReservation>` —
//!   ownership stays with the original chunk on `Clone`; the clone gets `None`.
//!   Use `take_memory_reservation()` to transfer ownership explicitly.
//!
//! ## Move-first discipline
//!
//! New code moves rows instead of cloning them:
//! - **`expand_visible_rows()`**: the single terminal move (visible rows out,
//!   expanded by `multiplicity`); used by `LocalChunkCollector` and every
//!   `collect` path.
//! - **`visible_rows()`**: the single borrow of visible rows (selection aware).
//! - **`take_indices`/`slice`**: move a subset of rows into a new chunk
//!   (efficient, uses `std::mem::take` per row), preserving `multiplicity`.
//! - **`Clone`**: the single explicit deep copy. It drops memory reservations
//!   and derived column caches; the new owner must re-account memory via its
//!   own pool/tracker.
//!
//! # Construction Paths
//!
//! - `new_with_layout(rows, layout)` — **Production path**. Schema is derived from
//!   layout slot metadata. Always preferred when a `SlotLayout` is available.
//! - `new(rows, schema)` — Schema-driven path. Layout auto-created from column names.
//! - `from_rows(rows)` / `from_rows_with_col_names(rows, col_names)` — Convenience
//!   constructors for tests. Always produce a layout (auto-created).

mod collector;
mod columnar_batch;
mod core;
mod eval;
mod kind;
mod policy;
mod schema;
mod selection;
mod typed;

#[cfg(test)]
mod tests;

// Re-export public API
pub use collector::LocalChunkCollector;
pub use columnar_batch::{BatchColumn, ColumnarBatch};
pub use core::DataChunk;
pub use policy::ColumnarPolicy;
pub use schema::{ColumnInfo, Schema};
pub(crate) use typed::gather_typed_column;
pub use typed::{TypedColumn, TypedKind};

/// OLAP vectorized batch size.
pub const VECTORIZED_BATCH_SIZE: usize = 2048;
/// Alias for the default chunk size (kept in sync with `ExecutionContext::DEFAULT_CHUNK_SIZE`).
pub const DEFAULT_CHUNK_SIZE: usize = VECTORIZED_BATCH_SIZE;
