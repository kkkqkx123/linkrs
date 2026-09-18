//! Checkpoint wrappers, integrity audit and WAL replay.

use super::EdgeStore;
use graphdb_core::types::Timestamp;
use graphdb_core::StorageResult;
use std::collections::HashSet;

impl EdgeStore {
    pub fn flush<P: AsRef<std::path::Path>>(
        &mut self,
        path: P,
        compression: crate::compression::CompressionType,
    ) -> StorageResult<crate::edge::EdgeCheckpointKind> {
        let path = path.as_ref();
        let crate::compression::CompressionType::Zstd { level } = compression;
        let page_size = crate::compression::DEFAULT_PAGE_SIZE;
        self.flush_incremental(path, page_size, level)
    }

    pub fn load<P: AsRef<std::path::Path>>(&mut self, path: P) -> StorageResult<()> {
        self.load_incremental(path.as_ref())
    }

    /// Fail-closed cross-copy audit used by [`EdgeStore::load`].
    ///
    /// Damage detection only: returns `(orphan property mappings, orphan CSR
    /// rows, live authority orphans)`. A nonzero count signals corrupt files
    /// or a write-path regression; callers reject the load. Crash consistency
    /// itself comes from the checkpoint commit protocol (groups before
    /// metadata, manifest published last with its tail embedded in
    /// `meta.bin`), not from this audit.
    ///
    /// One CSR traversal feeds both the orphan-row count and the live
    /// authority check, so the load path pays a single pass over both
    /// directions instead of two.
    pub(crate) fn copy_audit(&self) -> (usize, usize, usize) {
        let mut csr_ids = HashSet::new();
        let mut orphan_csr_rows = 0;
        for (_, nbr) in self.out_csr.iter_all().chain(self.in_csr.iter_all()) {
            if !self.mvcc.edge_timestamps.contains_key(&nbr.edge_id) {
                orphan_csr_rows += 1;
            }
            csr_ids.insert(nbr.edge_id);
        }
        let orphan_mappings = self
            .properties
            .edge_ids()
            .filter(|edge_id| !self.mvcc.edge_timestamps.contains_key(edge_id))
            .count();
        let live_orphans = self
            .mvcc
            .edge_timestamps
            .iter()
            .filter(|(edge_id, ts)| ts.delete_ts == Timestamp::MAX && !csr_ids.contains(edge_id))
            .count();
        (orphan_mappings, orphan_csr_rows, live_orphans)
    }

    /// Orphan property mappings plus orphan CSR rows.
    /// Audit-only (load path plus tests); a nonzero count means corrupt files
    /// or a write-path regression.
    ///
    /// Live-authority orphans (a live authority entry with no CSR row, as
    /// produced by a silent Single-slot overwrite) are reported separately
    /// by [`EdgeStore::live_authority_orphans`] and also reject the load.
    pub fn loaded_copy_mismatches(&self) -> (usize, usize) {
        let (orphan_mappings, orphan_csr_rows, _) = self.copy_audit();
        (orphan_mappings, orphan_csr_rows)
    }

    /// Live authority entries with no CSR row in either direction.
    /// Audit-only (load path plus tests); a nonzero count means a past
    /// silent overwrite orphaned the authority record.
    pub fn live_authority_orphans(&self) -> usize {
        let (_, _, live_orphans) = self.copy_audit();
        live_orphans
    }

    /// Replay one write-ahead log operation idempotently.
    ///
    /// Called only during load recovery with `wal_dir` cleared so replayed
    /// commits never append back to the log. Inserts skip on
    /// `EdgeAlreadyExists`, deletes treat missing edges as done, updates
    /// overwrite the same value, and schema changes skip when already
    /// applied, so a repeated replay yields the same state.
    pub(crate) fn replay_one_wal_op(
        &mut self,
        op: super::super::wal::EdgeWalOp,
    ) -> StorageResult<()> {
        match op {
            super::super::wal::EdgeWalOp::Insert {
                src,
                dst,
                rank,
                properties,
                create_ts,
            } => match self.insert_edge(src, dst, rank, &properties, create_ts) {
                Ok(()) => Ok(()),
                Err(e)
                    if e.kind()
                        == graphdb_core::error::storage::StorageErrorKind::EdgeAlreadyExists =>
                {
                    Ok(())
                }
                Err(e) => Err(e),
            },
            super::super::wal::EdgeWalOp::Delete {
                src,
                dst,
                rank,
                delete_ts,
            } => match self.delete_edge(src, dst, rank, delete_ts) {
                Ok(_) => Ok(()),
                Err(e) => Err(e),
            },
            super::super::wal::EdgeWalOp::PropertyUpdate {
                src,
                dst,
                rank,
                prop_name,
                value,
                ts,
            } => {
                self.update_edge_property(src, dst, rank, &prop_name, &value, ts)?;
                Ok(())
            }
            super::super::wal::EdgeWalOp::SchemaAdd {
                name,
                data_type,
                nullable,
                default,
            } => {
                if self.properties.has_property(&name) {
                    return Ok(());
                }
                self.prepare_add_property(name.clone(), data_type, nullable, default)?;
                if let Err(e) = self.fill_pending_add_property() {
                    let _ = self.abort_pending_add_property();
                    return Err(e);
                }
                match self.publish_pending_add_property() {
                    Ok(()) => Ok(()),
                    Err(e)
                        if e.kind()
                            == graphdb_core::error::storage::StorageErrorKind::ColumnAlreadyExists =>
                    {
                        let _ = self.abort_pending_add_property();
                        Ok(())
                    }
                    Err(e) => {
                        let _ = self.abort_pending_add_property();
                        Err(e)
                    }
                }
            }
            super::super::wal::EdgeWalOp::SchemaDrop { name } => {
                if !self.properties.has_property(&name) {
                    return Ok(());
                }
                match self.remove_property(&name) {
                    Ok(()) => Ok(()),
                    Err(e)
                        if e.kind()
                            == graphdb_core::error::storage::StorageErrorKind::ColumnNotFound =>
                    {
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }
        }
    }
}
