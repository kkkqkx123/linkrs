//! Frozen snapshot-file sidecar cache for group base files.

use crate::edge::edge_table::checkpoint::snapshot::{snapshot_path_for, write_snapshot_file};
use crate::edge::{CsrVariant, ImmutableCsr};
use graphdb_core::StorageResult;
use std::path::Path;

/// Sync the snapshot-file sidecar with a freshly written group base file.
///
/// Frozen bases gain (or refresh) a snapshot file; anything else removes a
/// stale one, so after every flush the sidecar exists exactly when the base
/// holds frozen bytes. Mapped bases already serve from their file and only
/// need a backfill when it went missing.
pub(crate) fn sync_snapshot_file(base_path: &Path, variant: &CsrVariant) -> StorageResult<()> {
    let snapshot = snapshot_path_for(base_path);
    match variant {
        CsrVariant::Frozen(csr) => write_snapshot_file(csr, &snapshot),
        CsrVariant::Mapped(csr) => {
            if snapshot.exists() {
                return Ok(());
            }
            let mut heap = ImmutableCsr::new();
            heap.load(&csr.dump())?;
            write_snapshot_file(&heap, &snapshot)
        }
        CsrVariant::Multiple(_)
        | CsrVariant::Single(_)
        | CsrVariant::Pure(_)
        | CsrVariant::Bundled(_)
        | CsrVariant::None { .. } => {
            let _ = std::fs::remove_file(&snapshot);
            Ok(())
        }
    }
}

/// Backfill a missing snapshot file for an otherwise clean frozen group.
/// Missing files are the only trigger; existing ones are already in sync.
pub(crate) fn backfill_snapshot_file(base_path: &Path, variant: &CsrVariant) -> StorageResult<()> {
    match variant {
        CsrVariant::Frozen(_) | CsrVariant::Mapped(_) => {
            if snapshot_path_for(base_path).exists() {
                return Ok(());
            }
            sync_snapshot_file(base_path, variant)
        }
        CsrVariant::Multiple(_)
        | CsrVariant::Single(_)
        | CsrVariant::Pure(_)
        | CsrVariant::Bundled(_)
        | CsrVariant::None { .. } => Ok(()),
    }
}
