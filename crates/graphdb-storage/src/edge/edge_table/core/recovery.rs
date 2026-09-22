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

    /// Read-only WAL diagnosis for an offline table directory.
    ///
    /// Never mutates the log. Reports the last valid entry boundary and the
    /// salvageable operation count. The load path stays fail-closed; run this
    /// after a torn-tail load failure, then trigger repair explicitly.
    pub fn diagnose_edge_wal_at<P: AsRef<std::path::Path>>(
        path: P,
    ) -> StorageResult<super::super::wal::EdgeWalDiagnosis> {
        super::super::wal::diagnose_ops(path.as_ref())
    }

    /// Explicit offline WAL repair for a table directory.
    ///
    /// Truncates the log at the last valid entry and returns the salvaged
    /// operation count. Only run while no writer holds the table, under the
    /// same single-writer discipline as every other mutation. Never called by
    /// the load path.
    pub fn repair_edge_wal_at<P: AsRef<std::path::Path>>(path: P) -> StorageResult<usize> {
        super::super::wal::discard_torn_tail(path.as_ref())
    }

    /// Explicit offline WAL repair with a full before/after report.
    ///
    /// Same fail-closed contract as [`Self::repair_edge_wal_at`]: the load
    /// path defaults to reject, only this explicit call truncates, and the
    /// returned report carries the salvaged prefix plus the discarded tail
    /// byte counts with diagnostic logs on both sides.
    pub fn repair_edge_wal_at_reported<P: AsRef<std::path::Path>>(
        path: P,
    ) -> StorageResult<super::super::wal::EdgeWalRepairReport> {
        super::super::wal::discard_torn_tail_reported(path.as_ref())
    }

    /// Read-only WAL diagnosis for this table's log directory.
    ///
    /// Fails when the table has no checkpoint directory yet, when redo has no
    /// home and there is nothing to diagnose.
    pub fn diagnose_wal(&self) -> StorageResult<super::super::wal::EdgeWalDiagnosis> {
        let Some(dir) = self.wal_dir.clone() else {
            return Err(graphdb_core::StorageError::invalid_operation(
                "edge table has no WAL directory before the first checkpoint".to_string(),
            ));
        };
        Self::diagnose_edge_wal_at(dir)
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
    /// Live-authority orphans (a live authority entry with no CSR row) are
    /// reported separately by [`EdgeStore::live_authority_orphans`] and also
    /// reject the load. The single-slot form rejects conflicting overwrites,
    /// so any such orphan indicates a regression, never an expected overwrite.
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

    /// Authority-versus-projection drift report.
    ///
    /// Single audit entry for the version-truth contract: the authority map
    /// owns every creation/deletion stamp, CSR cold halves and property row
    /// stamps are projections for collection only. Walks both CSR directions
    /// plus the property mappings and reports every divergence as a string:
    /// orphan rows/mappings, live authority orphans, and per-edge
    /// `delete_ts` (or deleted-state) mismatches between a projection and
    /// the authority. Empty means no drift. Load and reclaim paths fail
    /// closed on any nonzero presence subset; this report adds the stamp
    /// comparison so maintenance-time drift fails loudly instead of
    /// continuing silently.
    pub fn audit_copy_drift(&self) -> Vec<String> {
        let mut drift = Vec::new();
        let (orphan_mappings, orphan_csr_rows, live_orphans) = self.copy_audit();
        if orphan_mappings > 0 {
            drift.push(format!("orphan property mappings={}", orphan_mappings));
        }
        if orphan_csr_rows > 0 {
            drift.push(format!("orphan CSR rows={}", orphan_csr_rows));
        }
        if live_orphans > 0 {
            drift.push(format!("live authority orphans={}", live_orphans));
        }
        let mut seen_orphan_reported = HashSet::new();
        for (_, nbr) in self.out_csr.iter_all().chain(self.in_csr.iter_all()) {
            match self.mvcc.edge_timestamps.get(&nbr.edge_id) {
                None => {
                    if seen_orphan_reported.insert(nbr.edge_id) {
                        drift.push(format!(
                            "edge {:?} has CSR row but no authority",
                            nbr.edge_id
                        ));
                    }
                }
                Some(info) => {
                    if info.delete_ts != nbr.delete_ts {
                        drift.push(format!(
                            "edge {:?} delete_ts drift: authority={} csr={}",
                            nbr.edge_id, info.delete_ts, nbr.delete_ts
                        ));
                    }
                }
            }
        }
        if !self.properties.is_inline_stub() {
            for edge_id in self.properties.edge_ids() {
                let authority_deleted = self
                    .mvcc
                    .edge_timestamps
                    .get(&edge_id)
                    .map(|info| info.delete_ts != graphdb_core::types::Timestamp::MAX);
                match authority_deleted {
                    None => {
                        drift.push(format!(
                            "edge {:?} has property row but no authority",
                            edge_id
                        ));
                    }
                    Some(expected) => {
                        let row_deleted = self
                            .properties
                            .get_row_for_edge(edge_id)
                            .map(|row| self.properties.is_deleted_at_row(row));
                        if row_deleted != Some(expected) {
                            drift.push(format!("edge {:?} property deleted-state drift", edge_id));
                        }
                    }
                }
            }
        }
        drift
    }

    /// Periodic authority-versus-projection audit with metric emission.
    ///
    /// Runs the same [`EdgeStore::audit_copy_drift`] report the load,
    /// freeze and reclaim paths enforce, and additionally emits the three
    /// orphan counters through the table metrics registry so drift is
    /// observable between those gates. Read-only: never mutates table
    /// state. Callers are the watermark-driven maintenance passes; the
    /// write path itself stays free of full-table walks.
    pub fn audit_and_report(&self) -> Vec<String> {
        let drift = self.audit_copy_drift();
        if let Some(stats) = &self.stats_manager {
            let (orphan_mappings, orphan_csr_rows, live_orphans) = self.copy_audit();
            stats.add_value_with_amount(
                graphdb_metrics::MetricType::EdgeOrphanMappings,
                orphan_mappings as u64,
            );
            stats.add_value_with_amount(
                graphdb_metrics::MetricType::EdgeOrphanRows,
                orphan_csr_rows as u64,
            );
            stats.add_value_with_amount(
                graphdb_metrics::MetricType::EdgeLiveAuthorityOrphans,
                live_orphans as u64,
            );
        }
        drift
    }

    /// Replay one write-ahead log operation idempotently.
    ///
    /// Called only during load recovery with `wal_dir` cleared so replayed
    /// commits never append back to the log. Inserts skip on
    /// `EdgeAlreadyExists`, deletes treat a missing edge (no match) as done
    /// and keep cross-timestamp conflicts as errors, updates overwrite the
    /// same value, and schema changes skip when already applied, so a
    /// repeated replay yields the same state.
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
            super::super::wal::EdgeWalOp::SchemaRename { old_name, new_name } => {
                // Already-applied (published then truncated window) skips;
                // any other missing column is genuine damage and propagates.
                if self.properties.has_property(&new_name)
                    && !self.properties.has_property(&old_name)
                {
                    return Ok(());
                }
                self.rename_property(&old_name, &new_name)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EdgeStore;
    use crate::edge::edge_table::config::EdgeTableConfig;
    use crate::edge::{EdgeSchema, EdgeStrategy, RecordForm};
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::{CommitLsn, DataType, EdgeId, Timestamp};
    use graphdb_core::Value;

    fn audit_table() -> EdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef {
                name: "weight".to_string(),
                data_type: DataType::Double,
                nullable: false,
                default_value: Some(Value::Double(0.0)),
            }],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
            record_form: RecordForm::default(),
        };
        EdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    #[test]
    fn audit_covers_write_delete_rollback_compaction_freeze_serving_checkpoint() {
        let mut table = audit_table();
        assert!(table.audit_copy_drift().is_empty());

        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
            .unwrap();
        table
            .insert_edge(1, 2, 0, &[("weight".to_string(), Value::Double(3.0))], 100)
            .unwrap();
        assert!(table.audit_copy_drift().is_empty());

        assert!(table.delete_edge(0, 1, 0, 150).unwrap());
        assert!(table.audit_copy_drift().is_empty());

        assert!(table.revert_delete_edge(0, 1, 0, 150).unwrap());
        assert!(table.has_edge(0, 1, 0, 200));
        assert!(table.audit_copy_drift().is_empty());

        assert!(table.delete_edge(0, 1, 0, 160).unwrap());
        let watermarks =
            graphdb_transaction::MvccWatermarks::from_parts(300, 300, None, CommitLsn::ZERO);
        table.compact_csr_only_with_watermarks(&watermarks, 0, 0.2);
        assert!(table.audit_copy_drift().is_empty());

        table.freeze_group(true, 0, Timestamp::MAX, 0.0).unwrap();
        table.freeze_group(false, 0, Timestamp::MAX, 0.0).unwrap();
        assert!(table.audit_copy_drift().is_empty());
        table.unfreeze_group(true, 0).unwrap();
        table.unfreeze_group(false, 0).unwrap();
        assert!(table.audit_copy_drift().is_empty());

        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush succeeds");
        let mut loaded = audit_table();
        loaded.load(dir.path()).expect("load succeeds");
        assert!(loaded.audit_copy_drift().is_empty());
        assert_eq!(loaded.out_edges(0, 500).len(), 1);
        assert!(loaded.has_edge(0, 2, 0, 500));
        assert!(!loaded.has_edge(0, 1, 0, 500));
    }

    #[test]
    fn audit_and_report_emits_orphan_counters() {
        use graphdb_metrics::{MetricType, StatsManager};
        let mut table = audit_table();
        let stats = std::sync::Arc::new(StatsManager::new());
        table.set_stats_manager(stats.clone());
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        assert!(table.audit_and_report().is_empty());
        assert_eq!(
            stats.get_value(MetricType::EdgeOrphanMappings).unwrap_or(0),
            0
        );
        assert_eq!(stats.get_value(MetricType::EdgeOrphanRows).unwrap_or(0), 0);
        assert_eq!(
            stats
                .get_value(MetricType::EdgeLiveAuthorityOrphans)
                .unwrap_or(0),
            0
        );

        // Injected authority-only deletion surfaces as drift and reaches
        // the counters through the same report.
        table.mvcc.record_deletion(EdgeId(0), 50);
        assert!(!table.audit_and_report().is_empty());
    }

    #[test]
    fn reclaim_refusal_reports_orphan_counters() {
        use graphdb_metrics::{MetricType, StatsManager};
        let mut table = audit_table();
        let stats = std::sync::Arc::new(StatsManager::new());
        table.set_stats_manager(stats.clone());
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        // The refusal gate counts orphans, not stamp drift: drop the
        // authority record so both CSR directions and the property row
        // dangle.
        table.mvcc.edge_timestamps.remove(&EdgeId(0));
        let watermarks =
            graphdb_transaction::MvccWatermarks::from_parts(300, 300, None, CommitLsn::ZERO);
        assert!(table
            .reclaim_authority_with_watermarks(&watermarks, 0)
            .is_err());
        let reported = stats.get_value(MetricType::EdgeOrphanMappings).unwrap_or(0)
            + stats.get_value(MetricType::EdgeOrphanRows).unwrap_or(0)
            + stats
                .get_value(MetricType::EdgeLiveAuthorityOrphans)
                .unwrap_or(0);
        assert!(reported > 0, "reclaim refusal must emit orphan counters");
        // Storage refusal never flows through the import discard counters:
        // the two layers keep distinct metric names by construction.
        assert_eq!(
            stats.get_value(MetricType::ImportAcceptedRows).unwrap_or(0),
            0
        );
        assert_eq!(
            stats.get_value(MetricType::ImportDroppedRows).unwrap_or(0),
            0
        );
    }

    #[test]
    fn injected_authority_drift_is_reported_not_silent() {
        let mut table = audit_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        assert!(table.audit_copy_drift().is_empty());

        table.mvcc.record_deletion(EdgeId(0), 50);
        let drift = table.audit_copy_drift();
        assert!(
            !drift.is_empty(),
            "authority-only deletion must surface as drift"
        );
    }

    #[test]
    fn injected_property_drift_is_reported_not_silent() {
        let mut table = audit_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        assert!(table.audit_copy_drift().is_empty());

        assert!(table.properties.mark_deleted(EdgeId(0), 120));
        let drift = table.audit_copy_drift();
        assert!(
            !drift.is_empty(),
            "property-only deletion must surface as drift"
        );
    }

    #[test]
    fn reuse_cutoff_only_refreshes_from_watermarks() {
        let mut table = audit_table();
        assert_eq!(
            table.out_csr.tombstone_reuse_cutoff(),
            Timestamp::MAX,
            "fresh tables disable reuse"
        );

        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        assert!(table.delete_edge(0, 1, 0, 150).unwrap());
        table.maybe_run_auto_maintenance();
        assert_eq!(
            table.out_csr.tombstone_reuse_cutoff(),
            Timestamp::MAX,
            "pin-cache maintenance must never refresh the reuse hint"
        );

        let watermarks =
            graphdb_transaction::MvccWatermarks::from_parts(200, 200, None, CommitLsn::ZERO);
        table.maybe_run_auto_maintenance_with_watermarks(&watermarks, 0);
        assert_eq!(table.out_csr.tombstone_reuse_cutoff(), 200);
        assert_eq!(table.in_csr.tombstone_reuse_cutoff(), 200);

        table.out_csr.clear_tombstone_reuse_cutoff();
        assert_eq!(
            table.out_csr.tombstone_reuse_cutoff(),
            Timestamp::MAX,
            "unrefreshed tables degrade to no reuse"
        );

        table.out_csr.set_tombstone_reuse_cutoff(200);
        table.out_csr.set_tombstone_reuse_cutoff(150);
        assert_eq!(
            table.out_csr.tombstone_reuse_cutoff(),
            150,
            "stale values only narrow reuse"
        );
    }
}
