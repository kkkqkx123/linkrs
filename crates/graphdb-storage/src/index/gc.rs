use graphdb_core::types::Timestamp;
use graphdb_core::{StorageError, StorageResult};
use crate::index::traits::IndexGcOps;
use crate::index::types::GcStats;
use crate::index::IndexDataManagerImpl;

impl IndexDataManagerImpl {
    pub(crate) fn gc_runtime(
        &self,
        _safe_ts: Timestamp,
        _batch_size: usize,
    ) -> StorageResult<GcStats> {
        // Tombstone cleanup is handled by generation retirement + compaction.
        // Per-entry tombstone removal is no longer needed because clear paths
        // now publish delta generations (tombstones live in sub-generations).
        Ok(GcStats::default())
    }
}

impl IndexGcOps for IndexDataManagerImpl {
    fn gc_tombstones(&self, safe_ts: Timestamp) -> Result<GcStats, StorageError> {
        self.gc_runtime(safe_ts, usize::MAX)
    }

    fn gc_tombstones_incremental(
        &self,
        safe_ts: Timestamp,
        batch_size: usize,
    ) -> Result<GcStats, StorageError> {
        self.gc_runtime(safe_ts, batch_size)
    }

    fn tombstone_count(&self) -> usize {
        self.resync_tombstone_count();
        self.cached_tombstone_count() as usize
    }
}

#[cfg(test)]
mod tests {
    use graphdb_core::types::{Index, IndexConfig, IndexField, IndexType};
    use graphdb_core::Value;
    use crate::index::traits::{IndexGcOps, VertexIndexOps};
    use crate::index::IndexDataManagerImpl;

    #[test]
    fn tombstone_count_zero_for_empty_manager() {
        let manager = IndexDataManagerImpl::new();
        assert_eq!(manager.tombstone_count(), 0);
    }

    #[test]
    fn tombstones_are_published_as_delta_generations() {
        let manager = IndexDataManagerImpl::new();

        let index = Index::new(IndexConfig {
            id: 1,
            name: "idx".to_string(),
            space_id: 1,
            schema_name: "person".to_string(),
            fields: vec![IndexField::new(
                "name".to_string(),
                Value::string(""),
                false,
            )],
            properties: vec![],
            index_type: IndexType::TagIndex,
            is_unique: false,
            covering: false,
            partial_condition: None,
        });
        manager.register_native_index(1, &index).expect("register");
        manager
            .update_vertex_indexes_mvcc(
                1,
                &Value::Int(1),
                "idx",
                &[("name".to_string(), Value::string("Alice"))],
                10,
            )
            .expect("write");
        manager
            .delete_vertex_indexes_mvcc(1, &Value::Int(1), &["idx".to_string()], 20)
            .expect("delete");

        assert!(
            manager.tombstone_count() > 0,
            "expected tombstones after delete"
        );

        // With delta generation approach, per-entry tombstone removal is
        // replaced by generation retirement + compaction.
        // gc_runtime returns 0 — tombstones live in sub-generations until
        // compaction or the entire generation is retired.
        let stats = manager
            .gc_tombstones_incremental(25, 100)
            .expect("gc should succeed");
        assert_eq!(stats.total_removed(), 0, "per-entry GC is a no-op");
        assert!(
            manager.tombstone_count() > 0,
            "tombstones remain until compaction retires the delta generation"
        );
    }
}
