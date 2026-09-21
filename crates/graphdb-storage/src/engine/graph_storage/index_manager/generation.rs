use std::collections::HashSet;
use std::sync::LazyLock;

use graphdb_core::types::{CommitLsn, IndexGeneration};
use graphdb_core::{StorageError, StorageResult};

use super::super::context::GraphStorageContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum GenerationFaultPoint {
    SnapshotBuild,
    IncrementalReplay,
    BarrierEstablished,
    GenerationFsync,
    ManifestRename,
    FenceRelease,
}

static GENERATION_FAULTS: LazyLock<parking_lot::RwLock<HashSet<GenerationFaultPoint>>> =
    LazyLock::new(|| parking_lot::RwLock::new(HashSet::new()));

pub(crate) fn fail_if_generation_fault_is_injected(
    point: GenerationFaultPoint,
) -> StorageResult<()> {
    if GENERATION_FAULTS.read().contains(&point) {
        return Err(StorageError::db_error(format!(
            "Injected generation rebuild failure at {point:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn inject_generation_fault(point: GenerationFaultPoint) {
    GENERATION_FAULTS.write().insert(point);
}

#[cfg(test)]
pub(crate) fn clear_generation_faults() {
    GENERATION_FAULTS.write().clear();
}

pub(crate) fn current_wal_lsn(ctx: &GraphStorageContext) -> CommitLsn {
    if let Some(persistence) = ctx.persistence() {
        let coordinator = persistence.read();
        if let Some(wal) = coordinator.wal_manager() {
            let lsn = wal.read().current_lsn();
            return CommitLsn::new(lsn.into());
        }
    }
    CommitLsn::ZERO
}

pub(crate) fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Index IDs are persisted in SQLite INTEGER columns, so keep the
    // deterministic hash within the signed 64-bit range.
    hash & (i64::MAX as u64)
}

/// Get a fresh generation number derived from the active manifest's generation + 1.
pub(crate) fn next_generation(
    ctx: &GraphStorageContext,
    space_id: u64,
    index_name: &str,
) -> StorageResult<IndexGeneration> {
    let index_id = {
        let mgr = ctx.index_data_manager().read();

        mgr.index_alias(space_id, index_name)
    };
    let Some(index_id) = index_id else {
        // No manifest catalog yet; start at generation 1.
        return Ok(IndexGeneration::new(1));
    };
    let catalog = ctx
        .index_data_manager()
        .read()
        .manifest_catalog(space_id, index_id);
    let Some(catalog) = catalog else {
        return Ok(IndexGeneration::new(1));
    };
    let stats = catalog.stats();
    Ok(IndexGeneration::new(
        stats.active_generation.get().saturating_add(1),
    ))
}
