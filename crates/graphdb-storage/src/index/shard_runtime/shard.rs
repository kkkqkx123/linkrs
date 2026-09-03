use crate::index::chunk::chunked_index::ChunkedIndex;
use crate::index::chunk::serialize::{
    make_chunk_loader, make_chunk_writer, read_chunked_index_checkpoint_lazy,
    write_chunked_index_checkpoint,
};
use crate::index::key_codec::key_types::SecondaryIndexKey;
use crate::index::shard_runtime::bloom::RangeBloom;
use crate::index::shard_runtime::IndexMaps;
use crate::index::types::IndexRecord;
use crate::index::wal::{self, WalEntry};
use arc_swap::ArcSwap;
use graphdb_core::types::Timestamp;
use graphdb_core::StorageResult;
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::ops::Bound;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

pub(crate) struct ShardRuntime {
    pub(crate) checkpoint_file: PathBuf,
    pub(crate) pool_capacity: u64,
    pub(crate) base_forward: ArcSwap<ChunkedIndex>,
    pub(crate) base_reverse: ArcSwap<ChunkedIndex>,
    pub(crate) delta_forward: ArcSwap<BTreeMap<SecondaryIndexKey, IndexRecord>>,
    pub(crate) delta_reverse: ArcSwap<BTreeMap<SecondaryIndexKey, IndexRecord>>,
    pub(crate) dirty: AtomicBool,
    pub(crate) forward_prefix: Arc<[u8]>,
    pub(crate) reverse_prefix: Arc<[u8]>,
    pub(crate) prefix_forward_len: usize,
    pub(crate) prefix_reverse_len: usize,
    pub(crate) wal_file: PathBuf,
    pub(crate) wal_entries: Mutex<Vec<WalEntry>>,
    pub(crate) wal_size_bytes: AtomicU64,
    pub(crate) forward_bloom: Mutex<RangeBloom>,
    pub(crate) reverse_bloom: Mutex<RangeBloom>,
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
    /// Used during load/init to populate the chunked index.
    pub(crate) fn install_forward_from_btree(&self, map: BTreeMap<SecondaryIndexKey, IndexRecord>) {
        let idx = ChunkedIndex::from_btree(vec![], &map, self.pool_capacity);
        let dir = self.checkpoint_file.join("forward_chunks");
        idx.set_loader(make_chunk_loader(dir.clone()));
        idx.set_writer(make_chunk_writer(dir));
        self.base_forward.store(Arc::new(idx));
    }

    /// Build a ChunkedIndex from a BTreeMap and install it as the reverse base.
    pub(crate) fn install_reverse_from_btree(&self, map: BTreeMap<SecondaryIndexKey, IndexRecord>) {
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
