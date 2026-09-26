use std::sync::Arc;

use graphdb_core::types::{CommitLsn, CompactConfig, LabelId, Timestamp};
use graphdb_core::{StorageError, StorageResult, Value};

use crate::vertex::ShardedVertexTable;
use crate::StoragePersistenceOps;

use super::persistence;
use super::GraphStorage;

impl GraphStorage {
    /// Offline inspection of the vertex store under `vertices_dir`.
    ///
    /// Read-only patrol entry for half-damaged stores: reuses the recovery
    /// path manifest decoding per label and validates baseline plus
    /// incremental epoch chain continuity. Returns a one-line summary;
    /// cleanup stays with startup recovery.
    pub fn offline_inspect_vertex_store(
        &self,
        vertices_dir: &std::path::Path,
    ) -> StorageResult<String> {
        let health = ShardedVertexTable::inspect_store_health(vertices_dir)?;
        let summary = format!(
            "vertex store health: tables={} chain_ok={} healthy={} issues={:?}",
            health.tables.len(),
            health.chain_ok,
            health.is_healthy(),
            health.issues,
        );
        log::info!("{}", summary);
        Ok(summary)
    }

    /// Pre-switch baseline probe for an offline reshard: flushes the rebuilt
    /// table into a scratch directory beside the live vertex store, strictly
    /// health-checks the written checkpoint, and always removes the probe.
    /// Runs while the old table is still authoritative, so a rebuild that
    /// cannot durably flush a healthy baseline fails the reshard before any
    /// catalog switch. An engine without persistent paths has no on-disk
    /// baseline to protect, so the probe is a no-op there.
    fn probe_rebuilt_vertex_baseline(
        &self,
        rebuilt: &ShardedVertexTable,
        label: LabelId,
    ) -> StorageResult<()> {
        let Some(paths) = self.ctx.storage_paths() else {
            return Ok(());
        };
        let probe_dir = paths
            .vertices_dir()
            .join(format!("label_{label}.reshard_probe.tmp"));
        let _ = std::fs::remove_dir_all(&probe_dir);
        let verdict = rebuilt
            .flush(&probe_dir, self.ctx.flush_compression())
            .and_then(|()| {
                let report = ShardedVertexTable::inspect_commit_health(&probe_dir)?;
                if report.is_healthy() {
                    return Ok(());
                }
                let mut issues = report.missing_files.clone();
                issues.extend(report.pk_issues.clone());
                issues.extend(report.lineage_issues.clone());
                if !report.manifest_present || !report.manifest_decodable {
                    issues.push("commit manifest missing or undecodable".to_string());
                }
                Err(StorageError::invalid_operation(format!(
                    "reshard probe step failed: rebuilt baseline for vertex label {} \
                     is unhealthy at {}: {:?}",
                    label,
                    probe_dir.display(),
                    issues
                )))
            });
        // The probe is scratch content either way; a removal failure only
        // leaves orphans that the tolerant orphan cleanup clears later.
        let _ = std::fs::remove_dir_all(&probe_dir);
        verdict
    }

    /// Strict health check of the freshly published checkpoint baseline for
    /// one reshard label, run before old snapshots retire. Inspects the
    /// label's vertex directory inside the published checkpoint itself;
    /// failure refuses the reshard after naming the health step, keeping the
    /// retirement of recoverable history for a verified baseline.
    fn verify_resharded_baseline(&self, label: LabelId, checkpoint_seq: u64) -> StorageResult<()> {
        let persistence = self.ctx.persistence().as_ref().ok_or_else(|| {
            StorageError::invalid_operation(
                "reshard health step failed: no persistence coordinator for the checkpoint",
            )
        })?;
        let table_dir = persistence
            .read()
            .checkpoint_dir()
            .join(format!("checkpoint_{checkpoint_seq}"))
            .join("data")
            .join("vertices")
            .join(format!("label_{label}"));
        let report = ShardedVertexTable::inspect_commit_health(&table_dir).map_err(|e| {
            StorageError::db_error(format!(
                "reshard health step failed: cannot inspect the published vertex baseline \
                 for label {} at {}: {}",
                label,
                table_dir.display(),
                e
            ))
        })?;
        if !report.is_healthy() {
            return Err(StorageError::invalid_operation(format!(
                "reshard health step failed: published checkpoint {} for vertex label {} \
                 is unhealthy at {}: missing={:?} pk={:?} lineage={:?} manifest_present={} \
                 manifest_decodable={}. Old snapshots are kept; restore the previous \
                 checkpoint before retrying.",
                checkpoint_seq,
                label,
                table_dir.display(),
                report.missing_files,
                report.pk_issues,
                report.lineage_issues,
                report.manifest_present,
                report.manifest_decodable,
            )));
        }
        Ok(())
    }

    /// Offline redistribution of one vertex label to a new shard count.
    ///
    /// The edge-free adjustment outlet for the shard-count change, run as
    /// five ordered steps: rebuild every live row into a fresh table, probe
    /// the rebuilt baseline on disk and health-check it, switch the catalog,
    /// force the checkpoint that makes the new state the durable baseline,
    /// verify the published baseline, then retire old snapshots. Any failure
    /// before the switch keeps the old table authoritative and names the
    /// failing step; online shard count changes stay rejected by the table
    /// manifest.
    ///
    /// The tool holds the offline maintenance barrier itself, so no concurrent
    /// writes can tear the rebuild-swap sequence. Vertex IDs are rehashed and
    /// re-encoded, while edge endpoint IDs are not rewritten here: any live
    /// edge partition referencing this label (including wildcard label 0)
    /// refuses the rebuild so a graph is never silently mis-linked. Drain or
    /// migrate those edges first, or use
    /// [`Self::offline_reshard_vertex_table_with_edge_translation`].
    pub fn offline_reshard_vertex_table(
        &self,
        label: LabelId,
        new_num_shards: usize,
    ) -> StorageResult<usize> {
        let _barrier = self.ctx.offline_maintenance_barrier();
        let blockers: Vec<String> = self.ctx.data_store().with_edge_tables(|tables| {
            let mut blockers = Vec::new();
            for (key, table) in tables.iter() {
                if key.src_label != label
                    && key.dst_label != label
                    && key.src_label != 0
                    && key.dst_label != 0
                {
                    continue;
                }
                if table.read().edge_count() > 0 {
                    blockers.push(format!(
                        "edge partition ({},{},{}) holds live edges",
                        key.src_label, key.dst_label, key.edge_label
                    ));
                }
            }
            blockers
        });
        if !blockers.is_empty() {
            return Err(StorageError::invalid_operation(format!(
                "reshard refused: vertex label {} is referenced by {}: \
                 drain or migrate those edges before offline redistribution, \
                 or use the edge-aware redistribution entry",
                label,
                blockers.join("; ")
            )));
        }
        let (rebuilt, _) = self.ctx.data_store().with_vertex_tables(|tables| {
            let table = tables.get(&label).ok_or_else(|| {
                StorageError::label_not_found(format!("vertex label {} not found", label))
            })?;
            table.reshard_to(new_num_shards)
        })?;
        let rows = rebuilt.approximate_total_count();
        let shards = rebuilt.num_shards();
        // Persist and check the rebuilt baseline before switching anything:
        // a rebuild that cannot flush healthy refuses the reshard here.
        self.probe_rebuilt_vertex_baseline(&rebuilt, label)?;
        self.ctx.data_store().with_vertex_tables_mut(|tables| {
            tables.insert(label, Arc::new(rebuilt));
            Ok::<(), StorageError>(())
        })?;
        self.ctx.invalidate_vertex_cache(label);
        self.ctx.mark_vertex_modified(label);
        self.ctx.bump_layout_version();
        // Forced checkpoint fence: the swapped state must reach a durable
        // baseline before this call reports success.
        let stats = self.create_checkpoint()?.ok_or_else(|| {
            StorageError::invalid_operation(format!(
                "reshard finalize step failed: vertex label {} was rebuilt to {} shards \
                 but no checkpoint baseline was written: refusing success; the next open \
                 still sees the old baseline",
                label, shards,
            ))
        })?;
        // Check the published baseline before retiring old snapshots.
        self.verify_resharded_baseline(label, stats.checkpoint_id)?;
        log::info!(
            "Resharded vertex label {} to {} shards, {} rows carried over, checkpoint {} published",
            label,
            shards,
            rows,
            stats.checkpoint_id
        );
        self.cleanup_snapshots()?;
        Ok(rows)
    }

    /// Edge-aware offline redistribution of one vertex label.
    ///
    /// Same fence as the edge-free entry, but live edge references are
    /// translated instead of refused: the vertex rebuild produces the
    /// old-to-new internal id mapping, every edge partition referencing the
    /// label (including wildcard label 0) is remapped through the edge
    /// table's own remap machinery, and the vertex swap plus edge rewrite
    /// commit as one unit under the held barrier while readers continue on
    /// their pre-tool snapshots. A forced checkpoint baseline is written
    /// before returning; an unwritten checkpoint counts as not done and the
    /// next open still sees the old baseline. The new baseline is health
    /// checked before the call reports success; any step failing keeps the
    /// old catalog entry authoritative with the failing step named.
    pub fn offline_reshard_vertex_table_with_edge_translation(
        &self,
        label: LabelId,
        new_num_shards: usize,
    ) -> StorageResult<usize> {
        use std::collections::HashMap;
        let _barrier = self.ctx.offline_maintenance_barrier();
        let (rebuilt, id_mapping) = self.ctx.data_store().with_vertex_tables(|tables| {
            let table = tables.get(&label).ok_or_else(|| {
                StorageError::label_not_found(format!("vertex label {} not found", label))
            })?;
            table.reshard_to(new_num_shards)
        })?;
        let rows = rebuilt.approximate_total_count();
        let shards = rebuilt.num_shards();
        // Persist and check the rebuilt baseline before switching anything:
        // a rebuild that cannot flush healthy refuses the reshard here.
        self.probe_rebuilt_vertex_baseline(&rebuilt, label)?;
        self.ctx.data_store().with_vertex_tables_mut(|tables| {
            tables.insert(label, Arc::new(rebuilt));
            Ok::<(), StorageError>(())
        })?;

        // Translate every edge partition referencing the label through the
        // edge remap machinery. Wildcard endpoints (label 0) resolve against
        // this label's mapping; partitions touching neither endpoint nor a
        // wildcard are skipped. Unmapped endpoints keep their value, and an
        // empty mapping remaps nothing.
        let per_label: HashMap<LabelId, HashMap<u32, u32>> =
            [(label, id_mapping)].into_iter().collect();
        let mapping_for = |endpoint: LabelId| -> Option<&HashMap<u32, u32>> {
            if let Some(mapping) = per_label.get(&endpoint) {
                return Some(mapping);
            }
            if endpoint == 0 {
                return per_label.get(&label);
            }
            None
        };
        let remapped: Vec<(crate::engine::data_store::EdgeTableKey, bool)> = self
            .ctx
            .data_store()
            .for_all_edge_partitions_mut(|key, table| {
                let src_mapping = mapping_for(key.src_label);
                let dst_mapping = mapping_for(key.dst_label);
                if src_mapping.is_none() && dst_mapping.is_none() {
                    return Ok((key, false));
                }
                table.remap_vertex_ids(src_mapping, dst_mapping)?;
                for endpoint in [key.src_label, key.dst_label] {
                    if endpoint == label || endpoint == 0 {
                        self.ctx.mark_edge_modified(key.edge_label);
                    }
                }
                Ok((key, true))
            })?;

        self.ctx.invalidate_vertex_cache(label);
        self.ctx.mark_vertex_modified(label);
        self.ctx.bump_layout_version();

        // Forced checkpoint fence: the translated state must reach a durable
        // baseline before this call reports success.
        let stats = self.create_checkpoint()?.ok_or_else(|| {
            StorageError::db_error(format!(
                "reshard of vertex label {} to {} shards translated {} edge partition(s) \
                 but no checkpoint baseline was written: refusing success; the next open \
                 still sees the old baseline",
                label,
                shards,
                remapped.iter().filter(|(_, did)| *did).count(),
            ))
        })?;
        // Check the published baseline before retiring old snapshots.
        self.verify_resharded_baseline(label, stats.checkpoint_id)?;
        log::info!(
            "Resharded vertex label {} to {} shards with edge translation: {} rows, \
             {} edge partition(s) translated, checkpoint {} published",
            label,
            shards,
            rows,
            remapped.iter().filter(|(_, did)| *did).count(),
            stats.checkpoint_id,
        );
        self.cleanup_snapshots()?;
        Ok(rows)
    }

    /// Offline bulk import into an empty vertex label.
    ///
    /// Empty-table fast path for initial loads: capacity reserved once, rows
    /// applied under one lock hold per shard. Non-empty tables are rejected
    /// with guidance toward incremental batches. `sorted` declares strictly
    /// ordered unique keys, verified upfront.
    pub fn offline_bulk_import_vertices_str(
        &self,
        label: LabelId,
        rows: &[(String, Vec<(String, Value)>)],
        ts: Timestamp,
        sorted: bool,
    ) -> StorageResult<usize> {
        let table = self.ctx.data_store().with_vertex_tables(|tables| {
            tables.get(&label).cloned().ok_or_else(|| {
                StorageError::label_not_found(format!("vertex label {} not found", label))
            })
        })?;
        let borrowed: Vec<(&str, &[(String, Value)])> = rows
            .iter()
            .map(|(id, props)| (id.as_str(), props.as_slice()))
            .collect();
        table.bulk_import_str(&borrowed, ts, sorted)
    }

    /// Integer-keyed offline bulk import. Same contract as
    /// [`Self::offline_bulk_import_vertices_str`].
    pub fn offline_bulk_import_vertices_i64(
        &self,
        label: LabelId,
        rows: &[(i64, Vec<(String, Value)>)],
        ts: Timestamp,
        sorted: bool,
    ) -> StorageResult<usize> {
        let table = self.ctx.data_store().with_vertex_tables(|tables| {
            tables.get(&label).cloned().ok_or_else(|| {
                StorageError::label_not_found(format!("vertex label {} not found", label))
            })
        })?;
        let borrowed: Vec<(i64, &[(String, Value)])> = rows
            .iter()
            .map(|(id, props)| (*id, props.as_slice()))
            .collect();
        table.bulk_import_i64(&borrowed, ts, sorted)
    }
}

impl StoragePersistenceOps for GraphStorage {
    fn flush(&self) -> StorageResult<()> {
        persistence::flush(&self.ctx)
    }

    fn create_checkpoint(&self) -> StorageResult<Option<crate::CheckpointStats>> {
        persistence::create_checkpoint(&self.ctx)
    }

    fn verify_snapshot(&self, snapshot_id: u64) -> StorageResult<bool> {
        persistence::verify_snapshot(&self.ctx, snapshot_id)
    }

    fn cleanup_snapshots(&self) -> StorageResult<usize> {
        persistence::cleanup_snapshots(&self.ctx)
    }

    fn snapshot_stats(&self) -> crate::SnapshotStats {
        persistence::snapshot_stats(&self.ctx)
    }

    fn persistence_diagnostics(&self) -> Option<crate::PersistenceDiagnostics> {
        persistence::persistence_diagnostics(&self.ctx)
    }

    fn compact(&self, config: &CompactConfig) -> StorageResult<()> {
        persistence::compact_transactional(&self.ctx, config)
    }

    fn save_data(&self) -> StorageResult<()> {
        persistence::save_data(&self.ctx)
    }

    fn save_data_to_dir(&self, dir: &std::path::Path) -> StorageResult<()> {
        persistence::save_data_to_dir(&self.ctx, dir)
    }

    fn auto_flush_if_needed(&self) -> StorageResult<bool> {
        persistence::auto_flush_if_needed(&self.ctx)
    }

    fn auto_checkpoint_if_needed(&self) -> StorageResult<Option<crate::CheckpointStats>> {
        persistence::auto_checkpoint_if_needed(&self.ctx)
    }

    fn should_flush(&self) -> bool {
        persistence::should_flush(&self.ctx)
    }

    fn should_checkpoint(&self) -> bool {
        persistence::should_checkpoint(&self.ctx)
    }

    fn set_outbox_materialized_lsn_provider(
        &self,
        provider: Arc<dyn Fn() -> StorageResult<Option<CommitLsn>> + Send + Sync>,
    ) {
        if let Some(persistence) = self.ctx.persistence() {
            persistence
                .read()
                .set_outbox_materialized_lsn_provider(provider);
        }
    }
}
