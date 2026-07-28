use crate::core::types::Timestamp;
use crate::storage::index::chunk::buffer_pool::BufferPool;
use crate::storage::index::chunk::chunk::{build_chunks, Chunk, ChunkId, CHUNK_TARGET_SIZE};
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::types::IndexRecord;
use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;

/// A segmented index backed by a chunked BTree and a cache-friendly sparse index.
///
/// Semantically equivalent to `BTreeMap<SecondaryIndexKey, IndexRecord>` but
/// internally split into fixed-size chunks for:
/// - Partial eviction (cold chunks dropped under memory pressure)
/// - Incremental persistence (dirty chunks only)
/// - Prefix compression (shared prefix stored once per chunk)
#[derive(Clone)]
pub(crate) struct ChunkedIndex {
    /// Shared prefix stripped from all stored keys (space_id + key_type + index_name).
    prefix: Vec<u8>,
    /// Chunk descriptors sorted by min_key: (chunk_id, min_key, max_key).
    chunks: Vec<(ChunkId, SecondaryIndexKey, SecondaryIndexKey)>,
    /// The buffer pool owning all chunk data.
    pool: Arc<BufferPool<Chunk>>,
}

impl ChunkedIndex {
    pub(crate) fn new(prefix: Vec<u8>) -> Self {
        Self {
            prefix,
            chunks: Vec::new(),
            pool: Arc::new(BufferPool::new(u64::MAX)),
        }
    }

    pub(crate) fn from_btree(
        prefix: Vec<u8>,
        map: &BTreeMap<SecondaryIndexKey, IndexRecord>,
        pool_capacity: u64,
    ) -> Self {
        let entries: Vec<_> = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let built_chunks = build_chunks(entries, CHUNK_TARGET_SIZE);
        let pool = Arc::new(BufferPool::new(pool_capacity));
        let mut chunks = Vec::with_capacity(built_chunks.len());
        for chunk in &built_chunks {
            let cid = chunk.id;
            let min_key = chunk.min_key.clone();
            let max_key = chunk.max_key.clone();
            let size = chunk.estimated_size;
            pool.insert(cid, chunk.clone(), size);
            chunks.push((cid, min_key, max_key));
        }
        Self {
            prefix,
            chunks,
            pool,
        }
    }

    pub(crate) fn empty(prefix: Vec<u8>, pool_capacity: u64) -> Self {
        Self {
            prefix,
            chunks: Vec::new(),
            pool: Arc::new(BufferPool::new(pool_capacity)),
        }
    }

    pub(crate) fn with_capacity(
        prefix: Vec<u8>,
        pool: Arc<BufferPool<Chunk>>,
        chunk_descs: Vec<(ChunkId, SecondaryIndexKey, SecondaryIndexKey)>,
    ) -> Self {
        Self {
            prefix,
            chunks: chunk_descs,
            pool,
        }
    }

    pub(crate) fn prefix(&self) -> &[u8] {
        &self.prefix
    }

    /// Find the chunk IDs that overlap with `[lower, upper)`.
    fn chunks_for_range(&self, lower: &[u8], upper: &[u8]) -> Vec<ChunkId> {
        if self.chunks.is_empty() {
            return Vec::new();
        }
        // Empty upper bound means unbounded — include all chunks whose max_key >= lower
        if upper.is_empty() {
            let start_idx = self
                .chunks
                .partition_point(|(_, _, max_key)| max_key.as_slice() < lower);
            return self.chunks[start_idx..]
                .iter()
                .map(|(id, _, _)| *id)
                .collect();
        }
        self.chunks
            .iter()
            .filter(|(_, min_key, max_key)| {
                max_key.as_slice() >= lower && min_key.as_slice() < upper
            })
            .map(|(id, _, _)| *id)
            .collect()
    }

    /// Return all entries in `[lower, upper)` by merging relevant chunks.
    /// Keys are stored as-is (suffix keys if prefix is used externally).
    pub(crate) fn range(
        &self,
        lower: &[u8],
        upper: &[u8],
    ) -> Vec<(SecondaryIndexKey, IndexRecord)> {
        let chunk_ids = self.chunks_for_range(lower, upper);
        if chunk_ids.is_empty() {
            return Vec::new();
        }
        let mut results = Vec::new();
        for chunk_id in chunk_ids {
            if let Some(cached) = self.pool.get_or_load(chunk_id) {
                cached.pin();
                let entries = cached.item.range(lower, upper);
                results.extend(entries);
                cached.unpin();
            }
        }
        results
    }

    /// Return visible entries in `[lower, upper)`, skipping chunks with zero
    /// live entries and filtering tombstones inline via `read_ts`.
    pub(crate) fn visible_range(
        &self,
        lower: &[u8],
        upper: &[u8],
        read_ts: Timestamp,
    ) -> Vec<(SecondaryIndexKey, IndexRecord)> {
        let chunk_ids = self.chunks_for_range(lower, upper);
        if chunk_ids.is_empty() {
            return Vec::new();
        }
        let mut results = Vec::new();
        for chunk_id in chunk_ids {
            if let Some(cached) = self.pool.get_or_load(chunk_id) {
                if cached.item.live_count == 0 {
                    continue;
                }
                cached.pin();
                let entries: Vec<_> = cached
                    .item
                    .visible_range_iter(lower, upper, read_ts)
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                results.extend(entries);
                cached.unpin();
            }
        }
        results
    }

    /// Reconstruct the full sorted map by iterating all chunks.
    /// This is an O(N) operation used only for compaction/split (non-hot-path).
    pub(crate) fn snapshot(&self) -> BTreeMap<SecondaryIndexKey, IndexRecord> {
        let mut map = BTreeMap::new();
        for (chunk_id, _, _) in &self.chunks {
            if let Some(cached) = self.pool.get_or_load(*chunk_id) {
                cached.pin();
                for (k, v) in &cached.item.entries {
                    map.insert(k.clone(), v.clone());
                }
                cached.unpin();
            }
        }
        map
    }

    /// Number of entries across all chunks (inexact, includes tombstones).
    pub(crate) fn entry_count(&self) -> usize {
        let mut count = 0;
        for (chunk_id, _, _) in &self.chunks {
            if let Some(cached) = self.pool.get_or_load(*chunk_id) {
                count += cached.item.len();
            }
        }
        count
    }

    pub(crate) fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub(crate) fn pool(&self) -> &Arc<BufferPool<Chunk>> {
        &self.pool
    }

    pub(crate) fn set_loader<F>(&self, loader: F)
    where
        F: Fn(ChunkId) -> Option<(Chunk, usize)> + Send + Sync + 'static,
    {
        self.pool.set_loader(loader);
    }

    pub(crate) fn set_writer<F>(&self, writer: F)
    where
        F: Fn(ChunkId, &Chunk) -> Result<(), Box<dyn Error + Send + Sync>> + Send + Sync + 'static,
    {
        self.pool.set_writer(writer);
    }

    pub(crate) fn chunk_descriptors(&self) -> &[(ChunkId, SecondaryIndexKey, SecondaryIndexKey)] {
        &self.chunks
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Memory used by the index (prefix + chunk descriptors + pool).
    pub(crate) fn memory_usage(&self) -> u64 {
        let desc_size: usize = self
            .chunks
            .iter()
            .map(|(_, min_k, max_k)| {
                std::mem::size_of::<ChunkId>() + min_k.len() + max_k.len()
            })
            .sum();
        self.prefix.capacity() as u64 + desc_size as u64 + self.pool.current_usage()
    }
}

/// Builder for constructing ChunkedIndex from a BTreeMap.
pub(crate) struct ChunkedIndexBuilder {
    prefix: Vec<u8>,
    pool_capacity: u64,
}

impl ChunkedIndexBuilder {
    pub(crate) fn new(prefix: Vec<u8>, pool_capacity: u64) -> Self {
        Self {
            prefix,
            pool_capacity,
        }
    }

    pub(crate) fn build(
        self,
        map: &BTreeMap<SecondaryIndexKey, IndexRecord>,
    ) -> ChunkedIndex {
        ChunkedIndex::from_btree(self.prefix, map, self.pool_capacity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(id: u8) -> SecondaryIndexKey {
        vec![id]
    }

    fn make_value(id: u8) -> IndexRecord {
        IndexRecord::new(id as u64)
    }

    fn make_map(entries: Vec<(u8, u8)>) -> BTreeMap<SecondaryIndexKey, IndexRecord> {
        let mut map = BTreeMap::new();
        for (k, v) in entries {
            map.insert(make_key(k), make_value(v));
        }
        map
    }

    #[test]
    fn empty_index_returns_empty_range() {
        let idx = ChunkedIndex::new(vec![]);
        assert!(idx.range(&[0], &[255]).is_empty());
        assert!(idx.is_empty());
    }

    #[test]
    fn from_btree_preserves_all_entries() {
        let map = make_map(vec![(1, 10), (2, 20), (3, 30)]);
        let idx = ChunkedIndex::from_btree(vec![], &map, u64::MAX);
        assert_eq!(idx.entry_count(), 3);
        assert_eq!(idx.chunk_count(), 1);
    }

    #[test]
    fn range_returns_correct_subset() {
        let map = make_map(vec![(1, 10), (2, 20), (3, 30), (4, 40), (5, 50)]);
        let idx = ChunkedIndex::from_btree(vec![], &map, u64::MAX);
        let results = idx.range(&[2], &[5]);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0, vec![2]);
        assert_eq!(results[2].0, vec![4]);
    }

    #[test]
    fn snapshot_equals_original() {
        let map = make_map(vec![(1, 10), (2, 20), (3, 30)]);
        let idx = ChunkedIndex::from_btree(vec![], &map, u64::MAX);
        let snap = idx.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap.get(&vec![2]).unwrap().created_ts, 20);
    }

    #[test]
    fn range_returns_empty_for_non_overlapping() {
        let map = make_map(vec![(10, 100), (20, 200)]);
        let idx = ChunkedIndex::from_btree(vec![], &map, u64::MAX);
        assert!(idx.range(&[1], &[9]).is_empty());
        assert!(idx.range(&[30], &[40]).is_empty());
    }

    #[test]
    fn memory_usage_tracks_pool() {
        let map = make_map(vec![(1, 10), (2, 20)]);
        let idx = ChunkedIndex::from_btree(vec![], &map, u64::MAX);
        let mem = idx.memory_usage();
        assert!(mem > 0);
        assert!(mem >= idx.pool.current_usage());
    }

    #[test]
    fn prefix_is_stored() {
        let idx = ChunkedIndex::new(vec![0xAB, 0xCD]);
        assert_eq!(idx.prefix(), &[0xAB, 0xCD]);
    }

    #[test]
    fn from_btree_with_multiple_chunks() {
        let mut map = BTreeMap::new();
        for i in 0u8..200 {
            map.insert(vec![i; 512], IndexRecord::new(i as u64));
        }
        let idx = ChunkedIndex::from_btree(vec![], &map, u64::MAX);
        assert!(idx.chunk_count() >= 2, "expected multiple chunks, got {}", idx.chunk_count());
        assert_eq!(idx.entry_count(), 200);
    }

    #[test]
    fn chunks_for_range_returns_correct_ids() {
        let map = make_map(vec![(1, 10), (2, 20), (3, 30), (10, 100), (20, 200)]);
        let idx = ChunkedIndex::from_btree(vec![], &map, u64::MAX);
        let ids = idx.chunks_for_range(&[2], &[5]);
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn range_with_empty_lower_returns_all() {
        let mut map = BTreeMap::new();
        // suffix key: OrderedCodec(Int(2020)) + entity_id bytes
        map.insert(vec![0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0xE4, 0x01], IndexRecord::new(10));
        map.insert(vec![0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0xE5, 0x02], IndexRecord::new(10));
        let idx = ChunkedIndex::from_btree(vec![], &map, 65536);
        // Simulate IndexPredicate::All with prefix stripping:
        // lower_suffix = &[], upper_suffix = &[last_byte+1] where last_byte is from build_range_end
        let results = idx.range(&[], &[0xFF]);
        assert_eq!(results.len(), 2, "should find all entries with empty lower bound");
        // upper bound that is larger than all keys
        let results = idx.range(&[], &[0x04]);
        assert_eq!(results.len(), 2, "0x03 < 0x04, so all entries should match");
    }

    #[test]
    fn builder_creates_valid_index() {
        let map = make_map(vec![(5, 50), (3, 30), (1, 10)]);
        let builder = ChunkedIndexBuilder::new(vec![], u64::MAX);
        let idx = builder.build(&map);
        assert_eq!(idx.entry_count(), 3);
    }
}
