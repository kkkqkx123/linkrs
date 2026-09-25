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

    /// Offline redistribution of one vertex label to a new shard count.
    ///
    /// The edge-free adjustment outlet for the shard-count change: rebuilds
    /// every live row into a fresh table and swaps it into the catalog, then
    /// checkpoints the rebuilt table as the new baseline and retires the old
    /// checkpoint directory; online shard count changes stay rejected by the
    /// table manifest.
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
        self.ctx.data_store().with_vertex_tables_mut(|tables| {
            tables.insert(label, Arc::new(rebuilt));
            Ok::<(), StorageError>(())
        })?;
        self.ctx.invalidate_vertex_cache(label);
        self.ctx.mark_vertex_modified(label);
        self.ctx.bump_layout_version();
        log::info!(
            "Resharded vertex label {} to {} shards, {} rows carried over",
            label,
            shards,
            rows
        );
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
        self.create_checkpoint()?.ok_or_else(|| {
            StorageError::db_error(format!(
                "reshard of vertex label {} to {} shards translated {} edge partition(s) \
                 but no checkpoint baseline was written: refusing success; the next open \
                 still sees the old baseline",
                label,
                shards,
                remapped.iter().filter(|(_, did)| *did).count(),
            ))
        })?;
        let vertices_dir = self.ctx.work_dir().as_ref().map(|dir| {
            crate::engine::paths::StoragePaths::new(dir.clone()).vertices_dir()
        });
        if let Some(vertices_dir) = vertices_dir {
            let summary = self.offline_inspect_vertex_store(&vertices_dir)?;
            log::info!(
                "Resharded vertex label {} to {} shards with edge translation: {} rows, summary: {}",
                label,
                shards,
                rows,
                summary
            );
        }
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
