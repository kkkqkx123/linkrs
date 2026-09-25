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
    pub fn offline_inspect_vertex_store(&self, vertices_dir: &std::path::Path) -> StorageResult<String> {
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
    /// The only adjustment outlet for the shard-count change: rebuilds every
    /// live row into a fresh table and swaps it into the catalog. The caller
    /// checkpoints afterwards as the new baseline; online shard count changes
    /// stay rejected by the table manifest.
    pub fn offline_reshard_vertex_table(
        &self,
        label: LabelId,
        new_num_shards: usize,
    ) -> StorageResult<usize> {
        let rebuilt = self.ctx.data_store().with_vertex_tables(|tables| {
            let table = tables.get(&label).ok_or_else(|| {
                StorageError::label_not_found(format!("vertex label {} not found", label))
            })?;
            table.reshard_to(new_num_shards)
        })?;
        let rows = rebuilt.total_count();
        let shards = rebuilt.num_shards();
        self.ctx.data_store().with_vertex_tables_mut(|tables| {
            tables.insert(label, Arc::new(rebuilt));
            Ok::<(), StorageError>(())
        })?;
        log::info!(
            "Resharded vertex label {} to {} shards, {} rows carried over",
            label,
            shards,
            rows
        );
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
            tables
                .get(&label)
                .cloned()
                .ok_or_else(|| {
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
            tables
                .get(&label)
                .cloned()
                .ok_or_else(|| {
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
