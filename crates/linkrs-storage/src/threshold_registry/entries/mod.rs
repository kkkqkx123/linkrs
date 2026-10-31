//! Domain-grouped registry slices assembled into one iteration surface.

mod batch_capacity;
mod cache;
mod columnar;
mod compaction;
mod flush_wal;
mod index;
mod memory;
mod snapshot_gc;
mod txn_write;

use super::model::ThresholdEntry;

/// All domain slices in registry display order.
pub(crate) const DOMAIN_SLICES: &[&[ThresholdEntry]] = &[
    memory::ENTRIES,
    cache::ENTRIES,
    flush_wal::ENTRIES,
    compaction::ENTRIES,
    snapshot_gc::ENTRIES,
    columnar::ENTRIES,
    txn_write::ENTRIES,
    index::ENTRIES,
    batch_capacity::ENTRIES,
];

/// Total registered threshold count across domain slices.
pub(crate) const REGISTRY_LEN: usize = DOMAIN_SLICES[0].len()
    + DOMAIN_SLICES[1].len()
    + DOMAIN_SLICES[2].len()
    + DOMAIN_SLICES[3].len()
    + DOMAIN_SLICES[4].len()
    + DOMAIN_SLICES[5].len()
    + DOMAIN_SLICES[6].len()
    + DOMAIN_SLICES[7].len()
    + DOMAIN_SLICES[8].len();

/// Flattened view over every registered threshold.
pub(crate) fn all_entries() -> impl Iterator<Item = &'static ThresholdEntry> {
    DOMAIN_SLICES.iter().flat_map(|slice| slice.iter())
}

/// Locate a registry entry by id.
pub(crate) fn entry(id: &str) -> &'static ThresholdEntry {
    all_entries()
        .find(|entry| entry.id == id)
        .unwrap_or_else(|| panic!("threshold {id} missing from the registry"))
}
