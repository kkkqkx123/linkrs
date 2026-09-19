//! Frozen serving-file sidecar cache for group base files.

use crate::edge::{
    frozen_serving::{serving_path_for, write_serving_file},
    CsrVariant, ImmutableCsr,
};
use graphdb_core::StorageResult;
use std::path::Path;

/// Sync the serving-file sidecar with a freshly written group base file.
///
/// Frozen bases gain (or refresh) a serving file; anything else removes a
/// stale one, so after every flush the sidecar exists exactly when the base
/// holds frozen bytes. Mapped bases already serve from their file and only
/// need a backfill when it went missing.
pub(crate) fn sync_serving_file(base_path: &Path, variant: &CsrVariant) -> StorageResult<()> {
    let serving = serving_path_for(base_path);
    match variant {
        CsrVariant::Frozen(csr) => write_serving_file(csr, &serving),
        CsrVariant::Mapped(csr) => {
            if serving.exists() {
                return Ok(());
            }
            let mut heap = ImmutableCsr::new();
            heap.load(&csr.dump())?;
            write_serving_file(&heap, &serving)
        }
        CsrVariant::Multiple(_) | CsrVariant::Single(_) | CsrVariant::None { .. } => {
            let _ = std::fs::remove_file(&serving);
            Ok(())
        }
    }
}

/// Backfill a missing serving file for an otherwise clean frozen group.
/// Missing files are the only trigger; existing ones are already in sync.
pub(crate) fn backfill_serving_file(base_path: &Path, variant: &CsrVariant) -> StorageResult<()> {
    match variant {
        CsrVariant::Frozen(_) | CsrVariant::Mapped(_) => {
            if serving_path_for(base_path).exists() {
                return Ok(());
            }
            sync_serving_file(base_path, variant)
        }
        CsrVariant::Multiple(_) | CsrVariant::Single(_) | CsrVariant::None { .. } => Ok(()),
    }
}
