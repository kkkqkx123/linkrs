use crate::core::types::{IndexType, Timestamp};
use crate::core::{StorageError, StorageResult};
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::traits::IndexGcOps;
use crate::storage::index::types::{GcStats, IndexRecord};
use crate::storage::index::IndexDataManagerImpl;
use std::collections::BTreeMap;

impl IndexDataManagerImpl {
    pub(crate) fn gc_runtime(
        &self,
        safe_ts: Timestamp,
        batch_size: usize,
    ) -> StorageResult<GcStats> {
        let mut remaining = batch_size;
        let mut stats = GcStats::default();
        for (index_id, runtime) in self.runtimes.read().iter() {
            let index_type = self.index_types.read().get(index_id).cloned();
            for generation in runtime.generations() {
                for shard in generation.shards() {
                    let mut remove =
                        |map: &parking_lot::RwLock<BTreeMap<SecondaryIndexKey, IndexRecord>>| {
                            if remaining == 0 {
                                return 0;
                            }
                            let keys = map
                                .read()
                                .iter()
                                .filter(|(_, entry)| {
                                    entry.deleted_ts.is_some_and(|deleted| deleted < safe_ts)
                                })
                                .take(remaining)
                                .map(|(key, _)| key.clone())
                                .collect::<Vec<_>>();
                            let count = keys.len();
                            let mut data = map.write();
                            for key in keys {
                                data.remove(&key);
                            }
                            remaining = remaining.saturating_sub(count);
                            count
                        };
                    let removed = remove(shard.forward()) + remove(shard.reverse());
                    match index_type {
                        Some(IndexType::TagIndex) => stats.vertex_entries_removed += removed,
                        Some(IndexType::EdgeIndex) => stats.edge_entries_removed += removed,
                        None => {}
                    }
                    if remaining == 0 {
                        return Ok(stats);
                    }
                }
            }
        }
        Ok(stats)
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
                        .forward()
                        .read()
                        .values()
                        .filter(|entry| entry.deleted_ts.is_some())
                        .count();
                    count += shard
                        .reverse()
                        .read()
                        .values()
                        .filter(|entry| entry.deleted_ts.is_some())
                        .count();
                }
            }
        }
        count
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
    fn tombstones_are_removed_after_gc() {
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

        let stats = manager
            .gc_tombstones_incremental(25, 100)
            .expect("gc should succeed");
        assert!(stats.total_removed() > 0, "expected removed entries");
        assert_eq!(manager.tombstone_count(), 0);
    }
}
