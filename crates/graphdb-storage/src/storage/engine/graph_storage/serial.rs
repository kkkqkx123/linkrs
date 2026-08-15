//! SERIAL column allocator.
//!
//! Each space + table (tag or edge type) with a `SERIAL` column owns one
//! monotonic 64-bit counter. Allocation is a single `AtomicU64` fetch-add, so
//! the auto-allocate path is lock-free. The counter is never rolled back by
//! transaction undo: allocated ids are consumed even when the containing
//! insert is rolled back (ids must not be reused after being exposed).
//!
//! Persistence hooks (`snapshot` / [`GraphStorageContext::serial_allocator`])
//! feed `SERIAL_NEXT` metadata; startup recovery re-seeds every counter with
//! `max(persisted_next, column_max + 1)`.

use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::core::types::LabelId;
use crate::core::{StorageResult, Value};

use super::context::GraphStorageContext;

/// Scan state of one serial column: the max present value and the sorted set
/// of present values. Used to seed counters at startup and to detect conflicts
/// for explicitly supplied SERIAL values.
#[derive(Debug, Clone)]
pub(crate) struct SerialColumnScan {
    max_value: Option<i64>,
    present: Vec<i64>,
}

impl SerialColumnScan {
    pub(crate) fn max(&self) -> Option<i64> {
        self.max_value
    }

    pub(crate) fn contains(&self, value: i64) -> bool {
        self.present.binary_search(&value).is_ok()
    }
}

fn value_as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::BigInt(v) => Some(*v),
        Value::Int(v) => Some(*v as i64),
        Value::SmallInt(v) => Some(*v as i64),
        _ => None,
    }
}

/// Scan the `prop_name` column of the vertex table `label` for its max value
/// and the set of present values. Returns `None` when the table is absent.
///
/// The scan runs at the highest reserved write timestamp so that every
/// committed row is visible (live rows end at `MAX_TIMESTAMP`, which
/// `Timestamp::MAX` itself would not pass the MVCC validity check).
pub(crate) fn scan_vertex_serial_column(
    ctx: &GraphStorageContext,
    label: LabelId,
    prop_name: &str,
) -> Option<SerialColumnScan> {
    let ts = ctx.version_manager().write_timestamp();
    ctx.data_store().with_vertex_tables(|tables| {
        let table = tables.get(&label)?;
        let ids = table.live_ids();
        let mut present: Vec<i64> = Vec::new();
        let mut max_value: Option<i64> = None;
        if !ids.is_empty() {
            let projection = [prop_name.to_string()];
            for record in table.get_projected_batch(&ids, ts, Some(&projection)) {
                let record = record?;
                if let Some((_, value)) = record
                    .properties
                    .iter()
                    .find(|(name, _)| name == prop_name)
                {
                    if let Some(integer) = value_as_i64(value) {
                        present.push(integer);
                        max_value = Some(max_value.map_or(integer, |m| m.max(integer)));
                    }
                }
            }
        }
        present.sort_unstable();
        Some(SerialColumnScan {
            max_value,
            present,
        })
    })
}

/// Scan the `prop_name` column of the edge tables labeled `label` for its max
/// value and the set of present values. Returns `None` when no edge table is
/// present.
pub(crate) fn scan_edge_serial_column(
    ctx: &GraphStorageContext,
    label: LabelId,
    prop_name: &str,
) -> Option<SerialColumnScan> {
    let ts = ctx.version_manager().write_timestamp();
    ctx.data_store().with_edge_tables(|tables| {
        let mut scan = SerialColumnScan {
            max_value: None,
            present: Vec::new(),
        };
        for (key, arc) in tables.iter() {
            if key.edge_label != label {
                continue;
            }
            let table = arc.read();
            for record in table.scan(ts) {
                if let Some((_, value)) = record
                    .properties
                    .iter()
                    .find(|(name, _)| name == prop_name)
                {
                    if let Some(integer) = value_as_i64(value) {
                        scan.present.push(integer);
                        scan.max_value =
                            Some(scan.max_value.map_or(integer, |m| m.max(integer)));
                    }
                }
            }
        }
        scan.present.sort_unstable();
        if scan.present.is_empty() && scan.max_value.is_none() {
            return None;
        }
        Some(scan)
    })
}

/// Serialize the live counters into the `SERIAL_NEXT` schema-metadata format:
/// `(space_id, table name, next value)` triples.
pub(crate) fn serial_next_snapshot(ctx: &GraphStorageContext) -> Vec<(u64, String, u64)> {
    ctx.serial_allocator()
        .snapshot()
        .into_iter()
        .map(|(key, next)| (key.space_id, key.table, next))
        .collect()
}

/// Seed every SERIAL counter with `max(persisted SERIAL_NEXT, column max + 1)`.
///
/// Must run after WAL recovery so the column max already reflects replayed
/// rows. `max + 1` covers allocations that were never persisted before a
/// crash; the persisted value covers ids allocated at a high water mark whose
/// rows were later deleted.
pub(crate) fn seed_serial_allocators(ctx: &GraphStorageContext) -> StorageResult<()> {
    let allocator = ctx.serial_allocator();
    for (space_id, table, next) in ctx.schema_manager().serial_next() {
        allocator.seed(&SerialKey::new(space_id, table), next);
    }

    for space in ctx.schema_manager().list_spaces()? {
        let space_id = space.space_id;
        for tag in ctx.schema_manager().list_tags(&space.space_name)? {
            for prop in &tag.properties {
                if !prop.serial {
                    continue;
                }
                let key = SerialKey::new(space_id, tag.tag_name.clone());
                if let Some(scan) = scan_vertex_serial_column(ctx, tag.tag_id, &prop.name) {
                    if let Some(max) = scan.max().filter(|max| *max >= 0) {
                        allocator.seed(&key, (max as u64).saturating_add(1));
                    }
                }
            }
        }
        for edge_type in ctx.schema_manager().list_edge_types(&space.space_name)? {
            for prop in &edge_type.properties {
                if !prop.serial {
                    continue;
                }
                let key = SerialKey::new(space_id, edge_type.edge_type_name.clone());
                if let Some(scan) =
                    scan_edge_serial_column(ctx, edge_type.edge_type_id, &prop.name)
                {
                    if let Some(max) = scan.max().filter(|max| *max >= 0) {
                        allocator.seed(&key, (max as u64).saturating_add(1));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Identity of one serial counter: a space and a table name (tag name or edge
/// type name).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SerialKey {
    pub space_id: u64,
    pub table: String,
}

impl SerialKey {
    pub fn new(space_id: u64, table: impl Into<String>) -> Self {
        Self {
            space_id,
            table: table.into(),
        }
    }
}

/// In-memory auto-increment counters keyed by [`SerialKey`].
#[derive(Clone, Default)]
pub struct SerialAllocator {
    /// Holds the *next* value each counter will allocate (starts at 1).
    next_values: Arc<DashMap<SerialKey, Arc<AtomicU64>>>,
}

impl SerialAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate the next value for `key` (1-based, monotonically increasing).
    pub fn next(&self, key: &SerialKey) -> u64 {
        self.counter(key).fetch_add(1, Ordering::SeqCst)
    }

    /// Advance the counter so that the next allocation is at least
    /// `value + 1` (used when an explicit value is inserted).
    pub fn advance_to(&self, key: &SerialKey, value: u64) {
        self.counter(key)
            .fetch_max(value.saturating_add(1), Ordering::SeqCst);
    }

    /// Raise the counter to at least `next_value` (startup recovery seeding;
    /// idempotent with respect to already-seeded counters).
    pub fn seed(&self, key: &SerialKey, next_value: u64) {
        self.counter(key).fetch_max(next_value, Ordering::SeqCst);
    }

    /// All live counters as `(key, next_value)` pairs for `SERIAL_NEXT`
    /// persistence.
    pub fn snapshot(&self) -> Vec<(SerialKey, u64)> {
        self.next_values
            .iter()
            .map(|entry| {
                (
                    entry.key().clone(),
                    entry.value().load(Ordering::SeqCst),
                )
            })
            .collect()
    }

    /// Drop every counter of `space_id` (DROP SPACE / CLEAR SPACE).
    pub fn clear_space(&self, space_id: u64) {
        self.next_values
            .retain(|key, _| key.space_id != space_id);
    }

    /// Drop one counter (DROP TAG / DROP EDGE).
    pub fn remove(&self, key: &SerialKey) {
        self.next_values.remove(key);
    }

    fn counter(&self, key: &SerialKey) -> Arc<AtomicU64> {
        self.next_values
            .entry(key.clone())
            .or_insert_with(|| Arc::new(AtomicU64::new(1)))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocations_are_monotonic_and_1_based() {
        let allocator = SerialAllocator::new();
        let key = SerialKey::new(1, "Person".to_string());
        assert_eq!(allocator.next(&key), 1);
        assert_eq!(allocator.next(&key), 2);
        assert_eq!(allocator.next(&key), 3);
    }

    #[test]
    fn counters_are_scoped_by_space_and_table() {
        let allocator = SerialAllocator::new();
        assert_eq!(allocator.next(&SerialKey::new(1, "Person")), 1);
        assert_eq!(allocator.next(&SerialKey::new(2, "Person")), 1);
        assert_eq!(allocator.next(&SerialKey::new(1, "City")), 1);
    }

    #[test]
    fn advance_to_raises_next_allocation() {
        let allocator = SerialAllocator::new();
        let key = SerialKey::new(1, "Person".to_string());
        allocator.advance_to(&key, 5);
        // Next allocation must be 6.
        assert_eq!(allocator.next(&key), 6);
        // Advancing below the high water mark is a no-op.
        allocator.advance_to(&key, 2);
        assert_eq!(allocator.next(&key), 7);
    }

    #[test]
    fn seed_raises_but_never_lowers() {
        let allocator = SerialAllocator::new();
        let key = SerialKey::new(1, "Person".to_string());
        allocator.seed(&key, 10);
        allocator.seed(&key, 5);
        assert_eq!(allocator.next(&key), 10);
    }

    #[test]
    fn snapshot_and_clear() {
        let allocator = SerialAllocator::new();
        allocator.next(&SerialKey::new(1, "Person"));
        allocator.next(&SerialKey::new(1, "City"));
        allocator.next(&SerialKey::new(2, "Person"));
        assert_eq!(allocator.snapshot().len(), 3);

        allocator.clear_space(1);
        assert_eq!(allocator.snapshot().len(), 1);

        allocator.remove(&SerialKey::new(2, "Person"));
        assert!(allocator.snapshot().is_empty());
    }
}
