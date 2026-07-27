use crate::core::types::Timestamp;
use crate::core::{StorageError, StorageResult};
use crate::storage::index::traits::IndexGcOps;
use crate::storage::index::types::GcStats;
use crate::storage::index::IndexDataManagerImpl;

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
        let mut count = 0;
        for runtime in self.runtimes.read().values() {
            for generation in runtime.generations() {
                for shard in generation.shards() {
                    count += shard
                        .read_forward()
                        .values()
                        .filter(|entry| entry.deleted_ts.is_some())
                        .count();
                    count += shard
                        .read_reverse()
                        .values()
                        .filter(|entry| entry.deleted_ts.is_some())
                        .count();
                }
            }
        }
        count
    }

    fn retire_generations(&self, safe_ts: Timestamp) -> usize {
        self.retire_generations(safe_ts)
    }
}

#[cfg(test)]
mod tests {
    use crate::core::types::{Index, IndexConfig, IndexField, IndexType};
    use crate::core::Value;
    use crate::storage::index::traits::{IndexGcOps, VertexIndexOps};
    use crate::storage::index::IndexDataManagerImpl;

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
