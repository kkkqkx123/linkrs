//! Generation-scoped native-index storage.
//!
//! Each manifest generation owns independent shard maps. This makes the
//! manifest handle a real data-generation pin instead of metadata only.

use crate::index::chunk::chunked_index::ChunkedIndex;
use crate::index::chunk::serialize::{
    make_chunk_loader, make_chunk_writer, read_chunked_index_checkpoint_lazy,
    write_chunked_index_checkpoint,
};
use crate::index::key_codec::key_types::SecondaryIndexKey;
use crate::index::manifest::IndexManifest;
use crate::index::types::IndexRecord;
use crate::index::wal::{self, WalEntry};
use arc_swap::ArcSwap;
use graphdb_core::types::{CommitLsn, IndexGeneration, Timestamp};
use graphdb_core::{StorageError, StorageResult};
use parking_lot::{Condvar, Mutex, RwLock};
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::ops::Bound;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

/// Shared publish barriers used by native indexes and WAL truncation.
pub(crate) type IndexBarrierRegistry = Arc<RwLock<HashMap<(u64, u64), CommitLsn>>>;

pub(crate) type IndexMaps = (
    BTreeMap<SecondaryIndexKey, IndexRecord>,
    BTreeMap<SecondaryIndexKey, IndexRecord>,
);

pub(crate) struct ShardRuntime {
    pub(crate) checkpoint_file: PathBuf,
    pub(crate) pool_capacity: u64,
    base_forward: ArcSwap<ChunkedIndex>,
    base_reverse: ArcSwap<ChunkedIndex>,
    delta_forward: ArcSwap<BTreeMap<SecondaryIndexKey, IndexRecord>>,
    delta_reverse: ArcSwap<BTreeMap<SecondaryIndexKey, IndexRecord>>,
    dirty: AtomicBool,
    forward_prefix: Arc<[u8]>,
    reverse_prefix: Arc<[u8]>,
    prefix_forward_len: usize,
    prefix_reverse_len: usize,
    wal_file: PathBuf,
    wal_entries: Mutex<Vec<WalEntry>>,
    wal_size_bytes: AtomicU64,
    forward_bloom: Mutex<RangeBloom>,
    reverse_bloom: Mutex<RangeBloom>,
}

impl ShardRuntime {
    pub(crate) fn empty_with_capacity(checkpoint_file: PathBuf, pool_capacity: u64) -> Self {
        let wal_file = checkpoint_file.join("index.wal");
        Self {
            checkpoint_file,
            pool_capacity,
            base_forward: ArcSwap::new(Arc::new(ChunkedIndex::empty(vec![], pool_capacity))),
            base_reverse: ArcSwap::new(Arc::new(ChunkedIndex::empty(vec![], pool_capacity))),
            delta_forward: ArcSwap::new(Arc::new(BTreeMap::new())),
            delta_reverse: ArcSwap::new(Arc::new(BTreeMap::new())),
            dirty: AtomicBool::new(false),
            forward_prefix: Arc::from([] as [u8; 0]),
            reverse_prefix: Arc::from([] as [u8; 0]),
            prefix_forward_len: 0,
            prefix_reverse_len: 0,
            wal_file,
            wal_entries: Mutex::new(Vec::new()),
            wal_size_bytes: AtomicU64::new(0),
            forward_bloom: Mutex::new(RangeBloom::new()),
            reverse_bloom: Mutex::new(RangeBloom::new()),
        }
    }

    /// Build a ChunkedIndex from a BTreeMap and install it as the forward base.
    /// Used during load/init to populate the chunked index from legacy data.
    fn install_forward_from_btree(&self, map: BTreeMap<SecondaryIndexKey, IndexRecord>) {
        let idx = ChunkedIndex::from_btree(vec![], &map, self.pool_capacity);
        let dir = self.checkpoint_file.join("forward_chunks");
        idx.set_loader(make_chunk_loader(dir.clone()));
        idx.set_writer(make_chunk_writer(dir));
        self.base_forward.store(Arc::new(idx));
    }

    /// Build a ChunkedIndex from a BTreeMap and install it as the reverse base.
    fn install_reverse_from_btree(&self, map: BTreeMap<SecondaryIndexKey, IndexRecord>) {
        let idx = ChunkedIndex::from_btree(vec![], &map, self.pool_capacity);
        let dir = self.checkpoint_file.join("reverse_chunks");
        idx.set_loader(make_chunk_loader(dir.clone()));
        idx.set_writer(make_chunk_writer(dir));
        self.base_reverse.store(Arc::new(idx));
    }

    pub(crate) fn load_with_pool_capacity(
        checkpoint_file: PathBuf,
        pool_capacity: u64,
    ) -> StorageResult<Self> {
        // Load chunk checkpoint format
        let fwd_dir = checkpoint_file.join("forward_chunks");
        let rev_dir = checkpoint_file.join("reverse_chunks");
        let fwd = read_chunked_index_checkpoint_lazy(&fwd_dir, pool_capacity)?
            .unwrap_or_else(|| ChunkedIndex::empty(vec![], pool_capacity));
        let rev = read_chunked_index_checkpoint_lazy(&rev_dir, pool_capacity)?
            .unwrap_or_else(|| ChunkedIndex::empty(vec![], pool_capacity));
        // Install writer for dirty chunk write-back
        fwd.set_writer(make_chunk_writer(fwd_dir));
        rev.set_writer(make_chunk_writer(rev_dir));

        let mut sr = Self {
            checkpoint_file,
            pool_capacity,
            base_forward: ArcSwap::new(Arc::new(fwd)),
            base_reverse: ArcSwap::new(Arc::new(rev)),
            delta_forward: ArcSwap::new(Arc::new(BTreeMap::new())),
            delta_reverse: ArcSwap::new(Arc::new(BTreeMap::new())),
            dirty: AtomicBool::new(false),
            forward_prefix: Arc::from([] as [u8; 0]),
            reverse_prefix: Arc::from([] as [u8; 0]),
            prefix_forward_len: 0,
            prefix_reverse_len: 0,
            wal_file: PathBuf::new(),
            wal_entries: Mutex::new(Vec::new()),
            wal_size_bytes: AtomicU64::new(0),
            forward_bloom: Mutex::new(RangeBloom::new()),
            reverse_bloom: Mutex::new(RangeBloom::new()),
        };
        sr.wal_file = sr.checkpoint_file.join("index.wal");
        // Replay WAL entries if present
        sr.replay_wal()?;
        Ok(sr)
    }

    fn strip_forward_prefix(&self, key: &SecondaryIndexKey) -> SecondaryIndexKey {
        if self.prefix_forward_len > 0 && key.len() >= self.prefix_forward_len {
            key[self.prefix_forward_len..].to_vec()
        } else {
            key.clone()
        }
    }

    fn strip_reverse_prefix(&self, key: &SecondaryIndexKey) -> SecondaryIndexKey {
        if self.prefix_reverse_len > 0 && key.len() >= self.prefix_reverse_len {
            key[self.prefix_reverse_len..].to_vec()
        } else {
            key.clone()
        }
    }

    pub(crate) fn read_forward(&self) -> arc_swap::Guard<Arc<ChunkedIndex>> {
        self.base_forward.load()
    }

    pub(crate) fn read_reverse(&self) -> arc_swap::Guard<Arc<ChunkedIndex>> {
        self.base_reverse.load()
    }

    fn snapshot_forward(&self) -> BTreeMap<SecondaryIndexKey, IndexRecord> {
        let base = self.base_forward.load();
        let delta = self.delta_forward.load_full();
        let mut merged = base.snapshot();
        for (k, v) in delta.iter() {
            merged.insert(k.clone(), v.clone());
        }
        merged
    }

    fn snapshot_reverse(&self) -> BTreeMap<SecondaryIndexKey, IndexRecord> {
        let base = self.base_reverse.load();
        let delta = self.delta_reverse.load_full();
        let mut merged = base.snapshot();
        for (k, v) in delta.iter() {
            merged.insert(k.clone(), v.clone());
        }
        merged
    }

    fn merge_delta_into_base(&self) {
        let delta_fwd = self.delta_forward.load_full();
        let delta_rev = self.delta_reverse.load_full();
        if delta_fwd.is_empty() && delta_rev.is_empty() {
            return;
        }
        // Snapshot current ChunkedIndex, merge delta, rebuild
        let mut fwd = self.base_forward.load_full().snapshot();
        for (k, v) in delta_fwd.iter() {
            self.forward_bloom.lock().insert(k);
            fwd.insert(k.clone(), v.clone());
        }
        let mut rev = self.base_reverse.load_full().snapshot();
        for (k, v) in delta_rev.iter() {
            self.reverse_bloom.lock().insert(k);
            rev.insert(k.clone(), v.clone());
        }
        // Rebuild ChunkedIndex from merged BTreeMap
        let fwd_idx = ChunkedIndex::from_btree(vec![], &fwd, self.pool_capacity);
        let rev_idx = ChunkedIndex::from_btree(vec![], &rev, self.pool_capacity);
        self.base_forward.store(Arc::new(fwd_idx));
        self.base_reverse.store(Arc::new(rev_idx));
        self.delta_forward.store(Arc::new(BTreeMap::new()));
        self.delta_reverse.store(Arc::new(BTreeMap::new()));
    }

    /// Merge ChunkedIndex base with delta range into a single sorted Vec.
    fn chunked_range_with_delta(
        chunked: &ChunkedIndex,
        delta: &BTreeMap<SecondaryIndexKey, IndexRecord>,
        lower_suffix: &[u8],
        upper_suffix: &[u8],
        plen: usize,
        prefix: &[u8],
    ) -> Vec<(SecondaryIndexKey, IndexRecord)> {
        let base_results = chunked.range(lower_suffix, upper_suffix);
        let mut merged: BTreeMap<SecondaryIndexKey, IndexRecord> = base_results
            .into_iter()
            .map(|(k, v)| {
                if plen > 0 {
                    let mut full = Vec::with_capacity(prefix.len() + k.len());
                    full.extend_from_slice(prefix);
                    full.extend_from_slice(&k);
                    (full, v)
                } else {
                    (k, v)
                }
            })
            .collect();
        for (k, v) in delta.range((
            Bound::Included(lower_suffix.to_vec()),
            Bound::Excluded(upper_suffix.to_vec()),
        )) {
            if plen > 0 {
                let mut full = Vec::with_capacity(prefix.len() + k.len());
                full.extend_from_slice(prefix);
                full.extend_from_slice(k);
                merged.insert(full, v.clone());
            } else {
                merged.insert(k.clone(), v.clone());
            }
        }
        merged.into_iter().collect()
    }

    pub(crate) fn forward_range<'a>(
        &'a self,
        lower: &[u8],
        upper: &[u8],
    ) -> impl Iterator<Item = (SecondaryIndexKey, IndexRecord)> + 'a {
        let plen = self.prefix_forward_len;
        let prefix = Arc::clone(&self.forward_prefix);
        let (lower_suffix, upper_suffix) = self.strip_bounds(lower, upper, plen);
        let chunked = self.base_forward.load();
        let delta = self.delta_forward.load();
        Self::chunked_range_with_delta(&chunked, &delta, lower_suffix, upper_suffix, plen, &prefix)
            .into_iter()
    }

    pub(crate) fn reverse_range<'a>(
        &'a self,
        lower: &[u8],
        upper: &[u8],
    ) -> impl Iterator<Item = (SecondaryIndexKey, IndexRecord)> + 'a {
        let plen = self.prefix_reverse_len;
        let prefix = Arc::clone(&self.reverse_prefix);
        let (lower_suffix, upper_suffix) = self.strip_bounds(lower, upper, plen);
        let chunked = self.base_reverse.load();
        let delta = self.delta_reverse.load();
        Self::chunked_range_with_delta(&chunked, &delta, lower_suffix, upper_suffix, plen, &prefix)
            .into_iter()
    }

    fn strip_bounds<'s>(
        &self,
        lower: &'s [u8],
        upper: &'s [u8],
        plen: usize,
    ) -> (&'s [u8], &'s [u8]) {
        let ls = if plen > 0 && lower.len() >= plen {
            &lower[plen..]
        } else {
            lower
        };
        let us: &[u8] = if plen > 0 && upper.len() > plen {
            &upper[plen..]
        } else if plen > 0 && upper.len() == plen {
            &[] // unbounded
        } else {
            upper
        };
        (ls, us)
    }

    /// Return visible entries in reverse range with SUFFIX keys (no prefix reconstruction).
    /// Used by write-path to find existing indexed values per entity (avoids allocating
    /// the prefix bytes only to strip them again in the caller).
    pub(crate) fn reverse_range_suffix_visible(
        &self,
        lower: &[u8],
        upper: &[u8],
        read_ts: Timestamp,
    ) -> Vec<(SecondaryIndexKey, IndexRecord)> {
        let plen = self.prefix_reverse_len;
        let (lower_suffix, upper_suffix) = self.strip_bounds(lower, upper, plen);
        let chunked = self.base_reverse.load();
        let delta = self.delta_reverse.load();
        let mut merged: BTreeMap<SecondaryIndexKey, IndexRecord> = chunked
            .visible_range(lower_suffix, upper_suffix, read_ts)
            .into_iter()
            .collect();
        for (k, v) in delta.range((
            Bound::Included(lower_suffix.to_vec()),
            Bound::Excluded(upper_suffix.to_vec()),
        )) {
            if v.is_visible_at(read_ts) {
                merged.insert(k.clone(), v.clone());
            }
        }
        merged.into_iter().collect()
    }

    /// Quick check: does this shard possibly have forward entries in [lower, upper)?
    pub(crate) fn forward_may_have_range(&self, lower: &[u8], upper: &[u8]) -> bool {
        let (lower_suffix, upper_suffix) = self.strip_bounds(lower, upper, self.prefix_forward_len);
        // Bloom pre-check on lower bound
        if !self.forward_bloom.lock().might_contain(lower_suffix) {
            let base = self.base_forward.load();
            if !base.range(lower_suffix, upper_suffix).is_empty() {
                return true;
            }
            let delta = self.delta_forward.load();
            return delta
                .range((
                    Bound::Included(lower_suffix.to_vec()),
                    Bound::Excluded(upper_suffix.to_vec()),
                ))
                .next()
                .is_some();
        }
        true
    }

    /// Quick check: does this shard possibly have reverse entries in [lower, upper)?
    pub(crate) fn reverse_may_have_range(&self, lower: &[u8], upper: &[u8]) -> bool {
        let (lower_suffix, upper_suffix) = self.strip_bounds(lower, upper, self.prefix_reverse_len);
        if !self.reverse_bloom.lock().might_contain(lower_suffix) {
            let base = self.base_reverse.load();
            if !base.range(lower_suffix, upper_suffix).is_empty() {
                return true;
            }
            let delta = self.delta_reverse.load();
            return delta
                .range((
                    Bound::Included(lower_suffix.to_vec()),
                    Bound::Excluded(upper_suffix.to_vec()),
                ))
                .next()
                .is_some();
        }
        true
    }

    pub(crate) fn snapshot(&self) -> IndexMaps {
        let fwd = self.snapshot_forward();
        let rev = self.snapshot_reverse();
        let plen_f = self.prefix_forward_len;
        let plen_r = self.prefix_reverse_len;
        let fwd_prefix = Arc::clone(&self.forward_prefix);
        let rev_prefix = Arc::clone(&self.reverse_prefix);
        let forward = if plen_f > 0 {
            fwd.into_iter()
                .map(|(k, v)| {
                    let mut full = Vec::with_capacity(fwd_prefix.len() + k.len());
                    full.extend_from_slice(&fwd_prefix);
                    full.extend_from_slice(&k);
                    (full, v)
                })
                .collect()
        } else {
            fwd
        };
        let reverse = if plen_r > 0 {
            rev.into_iter()
                .map(|(k, v)| {
                    let mut full = Vec::with_capacity(rev_prefix.len() + k.len());
                    full.extend_from_slice(&rev_prefix);
                    full.extend_from_slice(&k);
                    (full, v)
                })
                .collect()
        } else {
            rev
        };
        (forward, reverse)
    }

    pub(crate) fn replace(
        &self,
        forward: BTreeMap<SecondaryIndexKey, IndexRecord>,
        reverse: BTreeMap<SecondaryIndexKey, IndexRecord>,
    ) {
        let mut wal_guard = self.wal_entries.lock();
        let mut wal_size_delta: u64 = 0;

        // COW: clone current delta, merge new entries, store atomically
        let old_fwd = self.delta_forward.load_full();
        let mut new_fwd = (*old_fwd).clone();
        for (k, v) in forward {
            let suffix = if self.prefix_forward_len > 0 {
                self.strip_forward_prefix(&k)
            } else {
                k.clone()
            };
            wal_size_delta += wal_record_size(&k, &v);
            wal_guard.push(wal_entry_for(true, k.clone(), v.clone()));
            self.forward_bloom.lock().insert(&suffix);
            new_fwd.insert(suffix, v);
        }
        self.delta_forward.store(Arc::new(new_fwd));

        let old_rev = self.delta_reverse.load_full();
        let mut new_rev = (*old_rev).clone();
        for (k, v) in reverse {
            let suffix = if self.prefix_reverse_len > 0 {
                self.strip_reverse_prefix(&k)
            } else {
                k.clone()
            };
            wal_size_delta += wal_record_size(&k, &v);
            wal_guard.push(wal_entry_for(false, k.clone(), v.clone()));
            self.reverse_bloom.lock().insert(&suffix);
            new_rev.insert(suffix, v);
        }
        self.delta_reverse.store(Arc::new(new_rev));

        self.wal_size_bytes
            .fetch_add(wal_size_delta, Ordering::Relaxed);
        drop(wal_guard);

        self.dirty.store(true, Ordering::Release);
    }

    /// Write buffered WAL entries to disk.
    pub(crate) fn flush_wal(&self) -> StorageResult<()> {
        let entries = {
            let mut wal_guard = self.wal_entries.lock();
            if wal_guard.is_empty() {
                return Ok(());
            }
            std::mem::take(&mut *wal_guard)
        };

        for entry in &entries {
            wal::append_wal_entry(&self.wal_file, entry)?;
        }
        Ok(())
    }

    /// Checkpoint: write full state to data files and clear WAL.
    pub(crate) fn checkpoint(&self) -> StorageResult<()> {
        self.full_flush()?;
        self.wal_entries.lock().clear();
        self.wal_size_bytes.store(0, Ordering::Relaxed);
        wal::truncate_wal(&self.wal_file)?;
        Ok(())
    }

    /// Replay WAL entries into in-memory maps.
    fn replay_wal(&self) -> StorageResult<()> {
        let entries = wal::read_wal_entries(&self.wal_file)?;
        if entries.is_empty() {
            return Ok(());
        }

        let mut fwd = self.base_forward.load_full().snapshot();
        let mut rev = self.base_reverse.load_full().snapshot();

        for entry in &entries {
            match entry {
                WalEntry::Insert {
                    is_forward,
                    key,
                    record,
                } => {
                    let key = if *is_forward && self.prefix_forward_len > 0 {
                        self.strip_forward_prefix(key)
                    } else if !*is_forward && self.prefix_reverse_len > 0 {
                        self.strip_reverse_prefix(key)
                    } else {
                        key.clone()
                    };
                    if *is_forward {
                        fwd.insert(key, record.clone());
                    } else {
                        rev.insert(key, record.clone());
                    }
                }
                WalEntry::MarkDeleted {
                    is_forward,
                    key,
                    deleted_ts,
                } => {
                    let key = if *is_forward && self.prefix_forward_len > 0 {
                        self.strip_forward_prefix(key)
                    } else if !*is_forward && self.prefix_reverse_len > 0 {
                        self.strip_reverse_prefix(key)
                    } else {
                        key.clone()
                    };
                    let map = if *is_forward { &mut fwd } else { &mut rev };
                    if let Some(record) = map.get_mut(&key) {
                        record.deleted_ts = Some(*deleted_ts);
                    } else {
                        let mut tombstone = IndexRecord::new(0);
                        tombstone.deleted_ts = Some(*deleted_ts);
                        map.insert(key, tombstone);
                    }
                }
            }
        }

        let fwd_idx = ChunkedIndex::from_btree(vec![], &fwd, self.pool_capacity);
        let rev_idx = ChunkedIndex::from_btree(vec![], &rev, self.pool_capacity);
        self.base_forward.store(Arc::new(fwd_idx));
        self.base_reverse.store(Arc::new(rev_idx));
        Ok(())
    }

    /// Return current WAL size in bytes (for checkpoint threshold).
    pub(crate) fn wal_size(&self) -> u64 {
        self.wal_size_bytes.load(Ordering::Relaxed)
    }

    pub(crate) fn iter_reverse(&self) -> Vec<(SecondaryIndexKey, IndexRecord)> {
        let merged = self.snapshot_reverse();
        let plen = self.prefix_reverse_len;
        let prefix = Arc::clone(&self.reverse_prefix);
        merged
            .into_iter()
            .map(|(k, v)| {
                if plen > 0 {
                    let mut full = Vec::with_capacity(prefix.len() + k.len());
                    full.extend_from_slice(&prefix);
                    full.extend_from_slice(&k);
                    (full, v)
                } else {
                    (k, v)
                }
            })
            .collect()
    }

    /// Flush to disk. If WAL size is below threshold, only appends WAL entries.
    /// Otherwise performs a full checkpoint.
    pub(crate) fn flush(&self) -> StorageResult<()> {
        if !self.dirty.load(Ordering::Acquire) {
            return Ok(());
        }
        // If WAL is small, just append entries; otherwise checkpoint
        const WAL_CHECKPOINT_THRESHOLD: u64 = 1024 * 1024; // 1MB
        if self.wal_size() < WAL_CHECKPOINT_THRESHOLD {
            self.flush_wal()?;
            self.dirty.store(false, Ordering::Release);
            Ok(())
        } else {
            self.checkpoint()
        }
    }

    fn full_flush(&self) -> StorageResult<()> {
        self.merge_delta_into_base();

        // Write forward chunks
        let fwd_dir = self.checkpoint_file.join("forward_chunks");
        let fwd_base = self.base_forward.load();
        write_chunked_index_checkpoint(&fwd_dir, &fwd_base)?;

        // Write reverse chunks
        let rev_dir = self.checkpoint_file.join("reverse_chunks");
        let rev_base = self.base_reverse.load();
        write_chunked_index_checkpoint(&rev_dir, &rev_base)?;

        self.dirty.store(false, Ordering::Release);
        Ok(())
    }

    pub(crate) fn memory_usage_bytes(&self) -> u64 {
        let fwd = self.base_forward.load();
        let rev = self.base_reverse.load();
        let delta_fwd = self.delta_forward.load_full();
        let delta_rev = self.delta_reverse.load_full();
        fwd.memory_usage() + rev.memory_usage() + delta_size(&delta_fwd) + delta_size(&delta_rev)
    }
}

fn delta_size(map: &BTreeMap<SecondaryIndexKey, IndexRecord>) -> u64 {
    map.iter()
        .map(|(key, record)| {
            let included = record.included_columns.as_ref().map_or(0, |cols| {
                cols.iter()
                    .map(|(name, value)| name.capacity() as u64 + value.estimated_size() as u64)
                    .sum::<u64>()
            });
            std::mem::size_of::<IndexRecord>() as u64 + key.capacity() as u64 + included
        })
        .sum()
}

/// Create a WAL entry from a key-value pair.
fn wal_entry_for(is_forward: bool, key: SecondaryIndexKey, record: IndexRecord) -> WalEntry {
    if let Some(deleted_ts) = record.deleted_ts {
        WalEntry::MarkDeleted {
            is_forward,
            key,
            deleted_ts,
        }
    } else {
        WalEntry::Insert {
            is_forward,
            key,
            record,
        }
    }
}

/// Estimate WAL entry size in bytes.
fn wal_record_size(key: &[u8], _record: &IndexRecord) -> u64 {
    (key.len() + std::mem::size_of::<IndexRecord>()) as u64
}

pub(crate) struct GenerationRuntime {
    pub(crate) generation: IndexGeneration,
    shards: HashMap<u32, Arc<ShardRuntime>>,
    parent: Option<Weak<GenerationRuntime>>,
    pub(crate) max_ts: Timestamp,
    pub(crate) last_access: AtomicU64,
}

fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

impl GenerationRuntime {
    pub(crate) fn empty_with_pool_capacity(manifest: &IndexManifest, pool_capacity: u64) -> Self {
        Self {
            generation: manifest.generation,
            shards: manifest
                .shards
                .iter()
                .map(|shard| {
                    (
                        shard.shard_id,
                        Arc::new(ShardRuntime::empty_with_capacity(
                            shard.checkpoint_file.clone(),
                            pool_capacity,
                        )),
                    )
                })
                .collect(),
            parent: None,
            max_ts: 0,
            last_access: AtomicU64::new(now_nanos()),
        }
    }

    /// Create generation where shard maps use prefix-stripped keys.
    /// Each shard is created with the given prefix and populated with `maps` data.
    pub(crate) fn empty_with_maps(
        manifest: &IndexManifest,
        forward_prefix: Vec<u8>,
        reverse_prefix: Vec<u8>,
        maps: HashMap<u32, IndexMaps>,
        parent: Option<&Arc<GenerationRuntime>>,
        max_ts: Timestamp,
    ) -> Self {
        Self::empty_with_maps_and_pool_capacity(
            manifest,
            forward_prefix,
            reverse_prefix,
            maps,
            parent,
            max_ts,
            64 * 1024 * 1024,
        )
    }

    pub(crate) fn empty_with_maps_and_pool_capacity(
        manifest: &IndexManifest,
        forward_prefix: Vec<u8>,
        reverse_prefix: Vec<u8>,
        maps: HashMap<u32, IndexMaps>,
        parent: Option<&Arc<GenerationRuntime>>,
        max_ts: Timestamp,
        pool_capacity: u64,
    ) -> Self {
        let prefix_f_len = forward_prefix.len();
        let prefix_r_len = reverse_prefix.len();
        let fwd_prefix: Arc<[u8]> = forward_prefix.into();
        let rev_prefix: Arc<[u8]> = reverse_prefix.into();
        let mut shards = HashMap::new();
        for shard_def in &manifest.shards {
            let (forward, reverse) = maps.get(&shard_def.shard_id).cloned().unwrap_or_default();
            let stripped_fwd = if prefix_f_len > 0 {
                forward
                    .into_iter()
                    .map(|(k, v)| {
                        let suffix = k[prefix_f_len..].to_vec();
                        (suffix, v)
                    })
                    .collect()
            } else {
                forward
            };
            let stripped_rev = if prefix_r_len > 0 {
                reverse
                    .into_iter()
                    .map(|(k, v)| {
                        let suffix = k[prefix_r_len..].to_vec();
                        (suffix, v)
                    })
                    .collect()
            } else {
                reverse
            };
            let mut sr =
                ShardRuntime::empty_with_capacity(shard_def.checkpoint_file.clone(), pool_capacity);
            sr.forward_prefix = Arc::clone(&fwd_prefix);
            sr.reverse_prefix = Arc::clone(&rev_prefix);
            sr.prefix_forward_len = prefix_f_len;
            sr.prefix_reverse_len = prefix_r_len;
            for k in stripped_fwd.keys() {
                sr.forward_bloom.lock().insert(k);
            }
            for k in stripped_rev.keys() {
                sr.reverse_bloom.lock().insert(k);
            }
            sr.install_forward_from_btree(stripped_fwd);
            sr.install_reverse_from_btree(stripped_rev);
            sr.dirty.store(true, Ordering::Release);
            shards.insert(shard_def.shard_id, Arc::new(sr));
        }
        let mut gen = Self {
            generation: manifest.generation,
            shards,
            parent: None,
            max_ts,
            last_access: AtomicU64::new(now_nanos()),
        };
        if let Some(p) = parent {
            gen.set_parent(p);
        }
        gen
    }

    fn load_with_pool_capacity(
        manifest: &IndexManifest,
        pool_capacity: u64,
    ) -> StorageResult<Self> {
        let mut shards = HashMap::new();
        for shard in &manifest.shards {
            let data = if shard.checkpoint_file.is_dir() {
                Arc::new(ShardRuntime::load_with_pool_capacity(
                    shard.checkpoint_file.clone(),
                    pool_capacity,
                )?)
            } else {
                Arc::new(ShardRuntime::empty_with_capacity(
                    shard.checkpoint_file.clone(),
                    pool_capacity,
                ))
            };
            shards.insert(shard.shard_id, data);
        }
        Ok(Self {
            generation: manifest.generation,
            shards,
            parent: None,
            max_ts: 0,
            last_access: AtomicU64::new(now_nanos()),
        })
    }

    pub(crate) fn set_parent(&mut self, parent: &Arc<GenerationRuntime>) {
        self.parent = Some(Arc::downgrade(parent));
    }

    pub(crate) fn parent_gen(&self) -> Option<Arc<GenerationRuntime>> {
        self.parent.as_ref().and_then(|w| w.upgrade())
    }

    pub(crate) fn shard(&self, shard_id: u32) -> Option<Arc<ShardRuntime>> {
        self.last_access.store(now_nanos(), Ordering::Relaxed);
        self.shards.get(&shard_id).cloned()
    }

    pub(crate) fn shards(&self) -> impl Iterator<Item = Arc<ShardRuntime>> + '_ {
        self.last_access.store(now_nanos(), Ordering::Relaxed);
        self.shards.values().cloned()
    }

    pub(crate) fn memory_usage_bytes(&self) -> u64 {
        self.shards.values().map(|sr| sr.memory_usage_bytes()).sum()
    }

    /// Evict cold chunks from all shards in this generation.
    /// Returns total bytes evicted.
    pub(crate) fn evict_cold_chunks(&self, target_bytes: u64) -> u64 {
        let per_shard = target_bytes / self.shards.len() as u64;
        self.shards
            .values()
            .map(|sr| {
                let fwd = sr.base_forward.load();
                let rev = sr.base_reverse.load();
                let half = per_shard / 2;
                fwd.pool().evict(half) + rev.pool().evict(half)
            })
            .sum()
    }

    /// Walk the generation chain from this gen backward, yielding (gen, [shards]) tuples.
    pub(crate) fn chain_from(gen: Arc<GenerationRuntime>) -> Vec<Arc<GenerationRuntime>> {
        let mut chain = Vec::new();
        let mut current = Some(gen);
        while let Some(g) = current.take() {
            chain.push(Arc::clone(&g));
            current = g.parent_gen();
        }
        chain
    }
}

/// The sole mutable native-index data owner for one index.
pub(crate) struct IndexRuntime {
    generations: RwLock<HashMap<IndexGeneration, Arc<GenerationRuntime>>>,
    barrier_lsn: AtomicU64,
    barrier_wait: Mutex<()>,
    barrier_cv: Condvar,
}

impl IndexRuntime {
    pub(crate) fn new(manifest: &IndexManifest) -> Self {
        const DEFAULT_POOL_CAPACITY: u64 = 64 * 1024 * 1024;
        Self::new_with_pool_capacity(manifest, DEFAULT_POOL_CAPACITY)
    }

    pub(crate) fn new_with_pool_capacity(manifest: &IndexManifest, pool_capacity: u64) -> Self {
        Self {
            generations: RwLock::new(HashMap::from([(
                manifest.generation,
                Arc::new(GenerationRuntime::empty_with_pool_capacity(
                    manifest,
                    pool_capacity,
                )),
            )])),
            barrier_lsn: AtomicU64::new(0),
            barrier_wait: Mutex::new(()),
            barrier_cv: Condvar::new(),
        }
    }

    pub(crate) fn load_with_pool_capacity(
        manifest: &IndexManifest,
        pool_capacity: u64,
    ) -> StorageResult<Self> {
        Ok(Self {
            generations: RwLock::new(HashMap::from([(
                manifest.generation,
                Arc::new(GenerationRuntime::load_with_pool_capacity(
                    manifest,
                    pool_capacity,
                )?),
            )])),
            barrier_lsn: AtomicU64::new(0),
            barrier_wait: Mutex::new(()),
            barrier_cv: Condvar::new(),
        })
    }

    pub(crate) fn generation(&self, generation: IndexGeneration) -> Option<Arc<GenerationRuntime>> {
        self.generations.read().get(&generation).cloned()
    }

    pub(crate) fn install_generation(&self, generation: GenerationRuntime) {
        let gen = generation.generation;
        self.generations.write().insert(gen, Arc::new(generation));
    }

    pub(crate) fn remove_generation(&self, generation: IndexGeneration) -> bool {
        self.generations.write().remove(&generation).is_some()
    }

    pub(crate) fn generations(&self) -> Vec<Arc<GenerationRuntime>> {
        self.generations.read().values().cloned().collect()
    }

    /// Return all generations in chain order (newest first, up to `max_generation`).
    /// The newest generation is the one with the highest generation number in the
    /// `generations` map that is <= `max_generation`.
    pub(crate) fn generation_chain_until(
        &self,
        max_generation: IndexGeneration,
    ) -> StorageResult<Vec<Arc<GenerationRuntime>>> {
        let gens = self.generations.read();
        let newest = gens
            .keys()
            .filter(|g| **g <= max_generation)
            .max()
            .copied()
            .ok_or_else(|| {
                StorageError::not_found(format!(
                    "No generation <= {} found in runtime",
                    max_generation
                ))
            })?;
        let newest_gen = gens
            .get(&newest)
            .ok_or_else(|| StorageError::not_found(format!("Generation {} not found", newest)))?;
        Ok(GenerationRuntime::chain_from(Arc::clone(newest_gen)))
    }

    pub(crate) fn memory_usage_bytes(&self) -> u64 {
        self.generations
            .read()
            .values()
            .map(|generation| generation.memory_usage_bytes())
            .sum()
    }

    /// Evict cold chunks from old (non-active) generations only.
    /// The active generation (highest number) is never evicted to avoid data loss.
    pub(crate) fn evict_cold_chunks(
        &self,
        current_usage: u64,
        high_water: u64,
        low_water: u64,
    ) -> StorageResult<()> {
        let target = if current_usage <= high_water {
            return Ok(());
        } else {
            low_water
        };
        let mut remaining = current_usage.saturating_sub(target);
        let gens = self.generations.read();
        // Find the active (highest) generation — never evict from it
        let active_gen = gens.keys().max().copied();
        for (gen_id, gen) in gens.iter() {
            if remaining == 0 {
                break;
            }
            // Skip the active generation
            if active_gen == Some(*gen_id) {
                continue;
            }
            let evicted = gen.evict_cold_chunks(remaining);
            remaining = remaining.saturating_sub(evicted);
        }
        Ok(())
    }

    pub(crate) fn barrier_lsn(&self) -> CommitLsn {
        CommitLsn::new(self.barrier_lsn.load(Ordering::Acquire))
    }

    /// Publish a barrier and wake writers or maintenance operations waiting
    /// for the corresponding generation boundary.
    pub(crate) fn establish_barrier_lsn(&self, barrier_lsn: CommitLsn) {
        let mut current = self.barrier_lsn.load(Ordering::Acquire);
        while barrier_lsn.get() > current {
            match self.barrier_lsn.compare_exchange(
                current,
                barrier_lsn.get(),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
        self.barrier_cv.notify_all();
    }

    /// Wait until this runtime has published at least `barrier_lsn`.
    pub(crate) fn wait_for_barrier_lsn(&self, barrier_lsn: CommitLsn) {
        if self.barrier_lsn() >= barrier_lsn {
            return;
        }
        let mut guard = self.barrier_wait.lock();
        while self.barrier_lsn() < barrier_lsn {
            self.barrier_cv.wait(&mut guard);
        }
    }

    pub(crate) fn flush_generation(&self, manifest: &IndexManifest) -> StorageResult<()> {
        let generation = self.generation(manifest.generation).ok_or_else(|| {
            StorageError::not_found(format!(
                "Missing runtime generation {}",
                manifest.generation
            ))
        })?;
        for shard in &manifest.shards {
            generation
                .shard(shard.shard_id)
                .ok_or_else(|| {
                    StorageError::not_found(format!("Missing runtime shard {}", shard.shard_id))
                })?
                .flush()?;
        }
        Ok(())
    }
}

pub(crate) fn generation_from_maps_with_pool_capacity(
    manifest: &IndexManifest,
    maps: HashMap<u32, IndexMaps>,
    parent: Option<&Arc<GenerationRuntime>>,
    max_ts: Timestamp,
    forward_prefix: Vec<u8>,
    reverse_prefix: Vec<u8>,
    pool_capacity: u64,
) -> GenerationRuntime {
    if !forward_prefix.is_empty() || !reverse_prefix.is_empty() {
        return GenerationRuntime::empty_with_maps_and_pool_capacity(
            manifest,
            forward_prefix,
            reverse_prefix,
            maps,
            parent,
            max_ts,
            pool_capacity,
        );
    }
    let mut generation = GenerationRuntime::empty_with_pool_capacity(manifest, pool_capacity);
    if let Some(p) = parent {
        generation.set_parent(p);
    }
    generation.max_ts = max_ts;
    for (shard_id, (forward, reverse)) in maps {
        if let Some(shard) = generation.shard(shard_id) {
            shard.replace(forward, reverse);
        }
    }
    generation
}

/// Simple bloom filter for range-scan skip optimization.
/// Uses a fixed-size bit array with 3 hash functions.
struct RangeBloom {
    bits: bitvec::vec::BitVec,
    seeds: [u64; 3],
}

impl RangeBloom {
    fn new() -> Self {
        // 65536 bits = 8 KB per filter, handles ~5000 entries with ~1% FP rate
        Self {
            bits: bitvec::vec::BitVec::repeat(false, 65536),
            seeds: [0x1234, 0x5678, 0x9abc],
        }
    }

    fn hash_index(&self, key: &[u8], seed: u64) -> usize {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        seed.hash(&mut hasher);
        key.hash(&mut hasher);
        hasher.finish() as usize % self.bits.len()
    }

    fn insert(&mut self, key: &[u8]) {
        for seed in &self.seeds {
            let idx = self.hash_index(key, *seed);
            self.bits.set(idx, true);
        }
    }

    fn might_contain(&self, key: &[u8]) -> bool {
        for seed in &self.seeds {
            let idx = self.hash_index(key, *seed);
            if !self.bits[idx] {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> IndexManifest {
        IndexManifest::new(
            1,
            1,
            IndexGeneration::new(1),
            vec![crate::index::manifest::IndexShard {
                shard_id: 0,
                lower: None,
                upper: None,
                checkpoint_file: PathBuf::from("memory-index"),
                checksum: None,
            }],
        )
        .expect("manifest should be valid")
    }

    #[test]
    fn barrier_wait_is_released_by_publish_notification() {
        let runtime = Arc::new(IndexRuntime::new(&manifest()));
        let waiting = Arc::clone(&runtime);
        let handle = std::thread::spawn(move || {
            waiting.wait_for_barrier_lsn(CommitLsn::new(42));
            waiting.barrier_lsn()
        });

        std::thread::sleep(std::time::Duration::from_millis(10));
        runtime.establish_barrier_lsn(CommitLsn::new(42));
        assert_eq!(
            handle.join().expect("barrier waiter should finish"),
            CommitLsn::new(42)
        );
    }

    #[test]
    fn barrier_lsn_is_monotonic() {
        let runtime = IndexRuntime::new(&manifest());
        runtime.establish_barrier_lsn(CommitLsn::new(100));
        runtime.establish_barrier_lsn(CommitLsn::new(50));
        assert_eq!(runtime.barrier_lsn(), CommitLsn::new(100));
    }
}
