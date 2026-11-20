use crate::cursor::IndexScanPlan;
use crate::index::edge_index_manager::{compute_edge_index_scan_range, EdgeIndexCursor};
use crate::index::key_codec::key_types::SecondaryIndexKey;
use crate::index::manifest::{ManifestCatalog, ManifestHandle};
use crate::index::shard_runtime::GenerationRuntime;
use crate::index::types::IndexRecord;
use crate::index::types::StaleChecker;
use crate::index::vertex_index_manager::{compute_vertex_index_scan_range, VertexIndexCursor};
use crate::index::IndexDataManagerImpl;
use linkrs_core::types::Index;
use linkrs_core::{StorageError, StorageResult};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::sync::Arc;

/// Forward iterator over a generation chain for a single shard.
///
/// Each generation is already sorted by key. A k-way merge keeps the output
/// globally sorted while preserving MVCC chain semantics: the newest
/// generation appears first for a duplicate key, so it decides visibility
/// and tombstones before older generations are considered.
type EntryStream = std::vec::IntoIter<(SecondaryIndexKey, IndexRecord)>;

pub(crate) struct ChainForwardIterator {
    streams: Vec<EntryStream>,
    heap: BinaryHeap<Reverse<(SecondaryIndexKey, usize)>>,
    read_ts: u64,
    seen: HashSet<SecondaryIndexKey>,
    tombstoned: HashSet<SecondaryIndexKey>,
}

impl ChainForwardIterator {
    pub(crate) fn new(
        chain: Vec<Arc<GenerationRuntime>>,
        shard_id: u32,
        read_ts: u64,
        range_start: Vec<u8>,
        range_end: Vec<u8>,
    ) -> Self {
        let mut streams: Vec<EntryStream> = Vec::new();
        for generation in chain {
            let Some(shard) = generation.shard(shard_id) else {
                continue;
            };
            if !shard.forward_may_have_range(&range_start, &range_end) {
                continue;
            }
            let entries: Vec<_> = shard.forward_range(&range_start, &range_end).collect();
            if !entries.is_empty() {
                streams.push(entries.into_iter());
            }
        }
        let mut heap = BinaryHeap::new();
        for (generation_index, stream) in streams.iter_mut().enumerate() {
            if let Some((key, _)) = stream.as_slice().first() {
                heap.push(Reverse((key.clone(), generation_index)));
            }
        }
        Self {
            streams,
            heap,
            read_ts,
            seen: HashSet::new(),
            tombstoned: HashSet::new(),
        }
    }
}

impl Iterator for ChainForwardIterator {
    type Item = (SecondaryIndexKey, IndexRecord);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(Reverse((key, generation_index))) = self.heap.pop() {
            let Some((entry_key, entry)) = self.streams[generation_index].next() else {
                continue;
            };
            debug_assert_eq!(entry_key, key);
            if let Some((next_key, _)) = self.streams[generation_index].as_slice().first() {
                self.heap
                    .push(Reverse((next_key.clone(), generation_index)));
            }

            if entry.created_ts > self.read_ts {
                continue;
            }
            if entry.deleted_ts.is_some_and(|d| d <= self.read_ts) {
                self.tombstoned.insert(entry_key);
                continue;
            }
            if self.tombstoned.contains(&entry_key) {
                continue;
            }
            if !self.seen.insert(entry_key.clone()) {
                continue;
            }

            return Some((entry_key, entry));
        }
        None
    }
}

impl IndexDataManagerImpl {
    #[cfg(test)]
    pub fn open_edge_index_cursor(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
    ) -> StorageResult<EdgeIndexCursor> {
        self.open_edge_index_cursor_full(space_id, index, plan, None, None)
    }

    pub fn open_tag_index_cursor_full(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
        stale_checker: Option<StaleChecker>,
        catalog: Option<&ManifestCatalog>,
    ) -> StorageResult<VertexIndexCursor> {
        let identity = crate::index::types::IndexIdentity {
            space_id,
            index_id: plan.index_id,
        };
        // make any pending (unpublished) writes visible to this scan.
        self.publish_pending_delta(identity)?;
        let owned_catalog = if catalog.is_none() {
            self.register_native_index(space_id, index)?;
            self.manifest_catalog(space_id, plan.index_id)
        } else {
            None
        };
        let catalog = catalog
            .or(owned_catalog.as_deref())
            .ok_or_else(|| StorageError::not_found("Index manifest is unavailable"))?;
        let runtime = self.runtime(space_id, plan.index_id)?;
        let handle = catalog.acquire();
        let manifest = handle.manifest();
        let chain = runtime.generation_chain_until(manifest.generation)?;
        let chain_handles = pin_chain_manifests(catalog, &chain);

        let (start, end) = compute_vertex_index_scan_range(space_id, index, plan)?;
        let shard_iterators: Vec<ChainForwardIterator> = manifest
            .scan_ranges_with_shard(&plan.partition, &start, &end)
            .into_iter()
            .filter_map(|(shard_id, lower, upper)| {
                // Fast check: skip if no data exists for this shard across any generation
                let has_data = chain.iter().any(|gen| gen.shard(shard_id).is_some());
                has_data.then(|| {
                    ChainForwardIterator::new(
                        chain.clone(),
                        shard_id,
                        plan.read_timestamp,
                        lower,
                        upper,
                    )
                })
            })
            .collect();

        Ok(VertexIndexCursor::new(
            shard_iterators,
            plan,
            stale_checker,
            chain_handles,
        ))
    }

    pub fn open_edge_index_cursor_full(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
        stale_checker: Option<StaleChecker>,
        catalog: Option<&ManifestCatalog>,
    ) -> StorageResult<EdgeIndexCursor> {
        let identity = crate::index::types::IndexIdentity {
            space_id,
            index_id: plan.index_id,
        };
        // make any pending (unpublished) writes visible to this scan.
        self.publish_pending_delta(identity)?;
        let owned_catalog = if catalog.is_none() {
            self.register_native_index(space_id, index)?;
            self.manifest_catalog(space_id, plan.index_id)
        } else {
            None
        };
        let catalog = catalog
            .or(owned_catalog.as_deref())
            .ok_or_else(|| StorageError::not_found("Index manifest is unavailable"))?;
        let runtime = self.runtime(space_id, plan.index_id)?;
        let handle = catalog.acquire();
        let manifest = handle.manifest();
        let chain = runtime.generation_chain_until(manifest.generation)?;
        let chain_handles = pin_chain_manifests(catalog, &chain);

        let (start, end) = compute_edge_index_scan_range(space_id, index, plan)?;
        let shard_iterators: Vec<ChainForwardIterator> = manifest
            .scan_ranges_with_shard(&plan.partition, &start, &end)
            .into_iter()
            .filter_map(|(shard_id, lower, upper)| {
                let has_data = chain.iter().any(|gen| gen.shard(shard_id).is_some());
                has_data.then(|| {
                    ChainForwardIterator::new(
                        chain.clone(),
                        shard_id,
                        plan.read_timestamp,
                        lower,
                        upper,
                    )
                })
            })
            .collect();

        Ok(EdgeIndexCursor::new(
            shard_iterators,
            plan,
            stale_checker,
            chain_handles,
        ))
    }
}

/// Acquire a manifest pin for every generation in a runtime chain (newest
/// first). Holding these handles fences each chain generation's physical files
/// from reclamation for as long as the cursor is alive, so a cursor can never
/// outlive the checkpoint data it may lazily reload after chunk eviction.
fn pin_chain_manifests(
    catalog: &ManifestCatalog,
    chain: &[Arc<GenerationRuntime>],
) -> Vec<ManifestHandle> {
    chain
        .iter()
        .filter_map(|gen| catalog.acquire_generation(gen.generation))
        .collect()
}

#[cfg(test)]
mod chain_merge_tests {
    use super::*;
    use crate::cursor::{IndexCursor, IndexPredicate, PartitionSelector};
    use linkrs_core::types::VertexId;
    use linkrs_core::wal::EntityRef;

    fn stream(keys: &[&str], created_ts: u64) -> EntryStream {
        keys.iter()
            .map(|key| (key.as_bytes().to_vec(), IndexRecord::new(created_ts)))
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn generations_merge_into_global_key_order() {
        let mut streams = vec![
            stream(&["c", "d"], 20),
            stream(&["a", "c"], 10),
            stream(&["b", "e"], 5),
        ];
        let mut heap = BinaryHeap::new();
        for (generation_index, stream) in streams.iter_mut().enumerate() {
            if let Some((key, _)) = stream.as_slice().first() {
                heap.push(Reverse((key.clone(), generation_index)));
            }
        }
        let iterator = ChainForwardIterator {
            streams,
            heap,
            read_ts: 20,
            seen: HashSet::new(),
            tombstoned: HashSet::new(),
        };
        let keys: Vec<String> = iterator
            .map(|(key, _)| String::from_utf8(key).expect("utf8 key"))
            .collect();
        assert_eq!(keys, ["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn cursor_limit_holds_across_batches_and_shards() {
        let make_shard = |ids: &[u64]| {
            let entries = ids
                .iter()
                .map(|id| {
                    (
                        id.to_be_bytes().to_vec(),
                        IndexRecord::new(10)
                            .with_entity_ref(EntityRef::Vertex(VertexId::from(*id))),
                    )
                })
                .collect::<Vec<_>>()
                .into_iter();
            let streams = vec![entries];
            let mut heap = BinaryHeap::new();
            if let Some((key, _)) = streams[0].as_slice().first() {
                heap.push(Reverse((key.clone(), 0)));
            }
            ChainForwardIterator {
                streams,
                heap,
                read_ts: 20,
                seen: HashSet::new(),
                tombstoned: HashSet::new(),
            }
        };
        let plan = IndexScanPlan {
            space: "space".into(),
            index_id: 1,
            predicate: IndexPredicate::All,
            partition: PartitionSelector::All,
            partition_id_range: None,
            projection: None,
            limit: Some(2),
            offset: 0,
            read_timestamp: 20,
        };
        let mut cursor = VertexIndexCursor::new(
            vec![make_shard(&[1, 2]), make_shard(&[3, 4])],
            &plan,
            None,
            vec![],
        );
        let mut rows = 0;
        for _ in 0..4 {
            rows += cursor.next_batch(1).expect("batch").len();
        }
        assert_eq!(rows, 2);
        assert!(cursor.is_exhausted());
    }

    #[test]
    fn newest_generation_decides_duplicate_visibility() {
        let mut streams = vec![stream(&["c"], 30), stream(&["c"], 10)];
        let mut heap = BinaryHeap::new();
        for (generation_index, stream) in streams.iter_mut().enumerate() {
            if let Some((key, _)) = stream.as_slice().first() {
                heap.push(Reverse((key.clone(), generation_index)));
            }
        }
        let iterator = ChainForwardIterator {
            streams,
            heap,
            read_ts: 20,
            seen: HashSet::new(),
            tombstoned: HashSet::new(),
        };
        let keys: Vec<String> = iterator
            .map(|(key, entry)| {
                assert_eq!(entry.created_ts, 10, "older visible version is used");
                String::from_utf8(key).expect("utf8 key")
            })
            .collect();
        assert_eq!(keys, ["c"]);
    }
}
