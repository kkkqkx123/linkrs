//! DataChunk: Basic unit of streaming execution
//!
//! A DataChunk represents a fixed-size batch of rows processed in streaming mode.
//! Typical size: 1024 rows (~4MB)
//!
//! # Ownership & Memory Accounting Rules (M4)
//!
//! - **`rows`**: Owned `Vec<Vec<Value>>`. Deep-cloned on `Clone` (transitional).
//! - **`schema`**: `Arc<Schema>` — shared reference, cheap `Arc::clone` on `Clone`.
//! - **`layout`**: `Arc<SlotLayout>` — always present; shared reference, cheap on `Clone`.
//! - **`memory_reservation`**: `Option<MemoryReservation>` / `Option<MemoryPoolReservation>` —
//!   ownership stays with the original chunk on `Clone`; the clone gets `None`.
//!   Use `take_memory_reservation()` to transfer ownership explicitly.
//!
//! ## Clone removal (M4)
//!
//! `Clone` is provided for migration only.  New code should use one of:
//! - **`deep_copy(pool)`**: creates a new chunk with rows deep-copied into the
//!   given memory pool.  Properly accounts the new memory.
//! - **`view()`**: creates a zero-copy [`ChunkView`] borrowing the parent chunk.
//! - **`slice(range)`**: moves a subset of rows into a new chunk (efficient,
//!   uses `std::mem::take` per row).
//!
//! # Construction Paths
//!
//! - `new_with_layout(rows, layout)` — **Production path**. Schema is derived from
//!   layout slot metadata. Always preferred when a `SlotLayout` is available.
//! - `new(rows, schema)` — Schema-driven path. Layout auto-created from column names.
//! - `from_rows(rows)` / `from_rows_with_col_names(rows, col_names)` — Convenience
//!   constructors for tests and legacy code. Always produce a layout (auto-created).

mod columnar_batch;
mod core;
mod eval;
mod policy;
mod pool;
mod schema;
mod selection;
mod typed;
mod view;

#[cfg(test)]
mod tests;

// Re-export public API
pub use columnar_batch::{BatchColumn, ColumnarBatch};
pub use core::DataChunk;
pub use policy::ColumnarPolicy;
pub use pool::RowPool;
pub use schema::{ColumnInfo, Schema};
pub use typed::{TypedColumn, TypedKind};
pub use view::ChunkView;

// Runtime switches
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

/// Runtime switch: typed column layout for produced chunks.
static TYPED_COLUMNS_ENABLED: AtomicBool = AtomicBool::new(true);

/// Enable or disable the typed column layout (rollback switch).
pub fn set_typed_columns_enabled(enabled: bool) {
    TYPED_COLUMNS_ENABLED.store(enabled, AtomicOrdering::Relaxed);
}

/// Whether the typed column layout is currently enabled.
pub fn typed_columns_enabled() -> bool {
    TYPED_COLUMNS_ENABLED.load(AtomicOrdering::Relaxed)
}

/// Runtime switch: selection-vector propagation across operators.
static SELECTION_PROPAGATION_ENABLED: AtomicBool = AtomicBool::new(true);

/// Enable or disable selection-vector propagation (rollback switch).
pub fn set_selection_propagation_enabled(enabled: bool) {
    SELECTION_PROPAGATION_ENABLED.store(enabled, AtomicOrdering::Relaxed);
}

/// Whether selection-vector propagation is currently enabled.
pub fn selection_propagation_enabled() -> bool {
    SELECTION_PROPAGATION_ENABLED.load(AtomicOrdering::Relaxed)
}
