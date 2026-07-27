//! Generation-scoped native-index storage.
//!
//! Each manifest generation owns independent shard maps. This makes the
//! manifest handle a real data-generation pin instead of metadata only.

use arc_swap::ArcSwap;
use crate::core::types::{CommitLsn, IndexGeneration, Timestamp};
use crate::core::{StorageError, StorageResult};
use crate::storage::index::generic_index_manager::GenericIndexManager;
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::key_codec::IndexKeyGenerator;
use crate::storage::index::manifest::IndexManifest;
use crate::storage::index::types::IndexRecord;
use crate::storage::index::wal::{self, WalEntry};
use parking_lot::{Condvar, Mutex, RwLock};
use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

/// Shared publish barriers used by native indexes and WAL truncation.
pub(crate) type IndexBarrierRegistry = Arc<RwLock<HashMap<(u64, u64), CommitLsn>>>;

pub(crate) type IndexMaps = (
    BTreeMap<SecondaryIndexKey, IndexRecord>,
    BTreeMap<SecondaryIndexKey, IndexRecord>,
);

pub(crate) struct ShardRuntime {
    pub(crate) checkpoint_file: PathBuf,
    forward: ArcSwap<BTreeMap<SecondaryIndexKey, IndexRecord>>,
    reverse: ArcSwap<BTreeMap<SecondaryIndexKey, IndexRecord>>,
    dirty: AtomicBool,
    forward_prefix: Vec<u8>,
    reverse_prefix: Vec<u8>,
    prefix_forward_len: usize,
    prefix_reverse_len: usize,
    wal_file: PathBuf,
    wal_entries: Mutex<Vec<WalEntry>>,
    wal_size_bytes: AtomicU64,
}

impl ShardRuntime {
    pub(crate) fn empty(checkpoint_file: PathBuf) -> Self {
        let wal_file = checkpoint_file.join("index.wal");
        Self {
            checkpoint_file,
            forward: ArcSwap::new(Arc::new(BTreeMap::new())),
            reverse: ArcSwap::new(Arc::new(BTreeMap::new())),
            dirty: AtomicBool::new(false),
            forward_prefix: Vec::new(),
            reverse_prefix: Vec::new(),
            prefix_forward_len: 0,
            prefix_reverse_len: 0,
            wal_file,
            wal_entries: Mutex::new(Vec::new()),
            wal_size_bytes: AtomicU64::new(0),
        }
    }

    fn new_with_prefix(
        checkpoint_file: PathBuf,
        forward_prefix: Vec<u8>,
        reverse_prefix: Vec<u8>,
    ) -> Self {
        let prefix_forward_len = forward_prefix.len();
        let prefix_reverse_len = reverse_prefix.len();
        let wal_file = checkpoint_file.join("index.wal");
        Self {
            checkpoint_file,
            forward: ArcSwap::new(Arc::new(BTreeMap::new())),
            reverse: ArcSwap::new(Arc::new(BTreeMap::new())),
            dirty: AtomicBool::new(false),
            forward_prefix,
            reverse_prefix,
            prefix_forward_len,
            prefix_reverse_len,
            wal_file,
            wal_entries: Mutex::new(Vec::new()),
            wal_size_bytes: AtomicU64::new(0),
        }
    }

    pub(crate) fn load<K: IndexKeyGenerator>(checkpoint_file: PathBuf) -> StorageResult<Self> {
        let (forward, reverse) =
            GenericIndexManager::<K>::load_data(&checkpoint_file)?;
        let mut sr = Self {
            checkpoint_file,
            forward: ArcSwap::new(Arc::new(forward)),
            reverse: ArcSwap::new(Arc::new(reverse)),
            dirty: AtomicBool::new(false),
            forward_prefix: Vec::new(),
            reverse_prefix: Vec::new(),
            prefix_forward_len: 0,
            prefix_reverse_len: 0,
            wal_file: PathBuf::new(),
            wal_entries: Mutex::new(Vec::new()),
            wal_size_bytes: AtomicU64::new(0),
        };
        sr.wal_file = sr.checkpoint_file.join("index.wal");
        // Replay WAL entries if present
        sr.replay_wal()?;
        Ok(sr)
    }

    pub(crate) fn forward_prefix_len(&self) -> usize {
        self.prefix_forward_len
    }

    pub(crate) fn reverse_prefix_len(&self) -> usize {
        self.prefix_reverse_len
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

    fn prepend_forward_prefix(&self, key: &SecondaryIndexKey) -> SecondaryIndexKey {
        if self.prefix_forward_len > 0 {
            let mut full = self.forward_prefix.clone();
            full.extend_from_slice(key);
            full
        } else {
            key.clone()
        }
    }

    fn prepend_reverse_prefix(&self, key: &SecondaryIndexKey) -> SecondaryIndexKey {
        if self.prefix_reverse_len > 0 {
            let mut full = self.reverse_prefix.clone();
            full.extend_from_slice(key);
            full
        } else {
            key.clone()
        }
    }

    pub(crate) fn read_forward(
        &self,
    ) -> arc_swap::Guard<Arc<BTreeMap<SecondaryIndexKey, IndexRecord>>> {
        self.forward.load()
    }

    pub(crate) fn read_reverse(
        &self,
    ) -> arc_swap::Guard<Arc<BTreeMap<SecondaryIndexKey, IndexRecord>>> {
        self.reverse.load()
    }

    /// Iterate forward range with full-key semantics: accepts full-key bounds,
    /// returns entries with full forward keys (prefix reconstructed).
    pub(crate) fn forward_range<'a>(
        &'a self,
        lower: &[u8],
        upper: &[u8],
    ) -> impl Iterator<Item = (SecondaryIndexKey, IndexRecord)> + 'a {
        let lower_suffix = if self.prefix_forward_len > 0 && lower.len() >= self.prefix_forward_len
        {
            Bound::Included(lower[self.prefix_forward_len..].to_vec())
        } else {
            Bound::Included(lower.to_vec())
        };
        let upper_suffix = if self.prefix_forward_len > 0 && upper.len() >= self.prefix_forward_len
        {
            Bound::Excluded(upper[self.prefix_forward_len..].to_vec())
        } else {
            Bound::Excluded(upper.to_vec())
        };
        let map = self.forward.load();
        let plen = self.prefix_forward_len;
        let fwd_prefix = self.forward_prefix.clone();
        let range: Vec<_> = map
            .range((lower_suffix, upper_suffix))
            .map(move |(k, v)| {
                if plen > 0 {
                    let mut full = fwd_prefix.clone();
                    full.extend_from_slice(k);
                    (full, v.clone())
                } else {
                    (k.clone(), v.clone())
                }
            })
            .collect();
        range.into_iter()
    }

    /// Iterate reverse range with full-key semantics.
    pub(crate) fn reverse_range<'a>(
        &'a self,
        lower: &[u8],
        upper: &[u8],
    ) -> impl Iterator<Item = (SecondaryIndexKey, IndexRecord)> + 'a {
        let lower_suffix =
            if self.prefix_reverse_len > 0 && lower.len() >= self.prefix_reverse_len {
                Bound::Included(lower[self.prefix_reverse_len..].to_vec())
            } else {
                Bound::Included(lower.to_vec())
            };
        let upper_suffix =
            if self.prefix_reverse_len > 0 && upper.len() >= self.prefix_reverse_len {
                Bound::Excluded(upper[self.prefix_reverse_len..].to_vec())
            } else {
                Bound::Excluded(upper.to_vec())
            };
        let map = self.reverse.load();
        let plen = self.prefix_reverse_len;
        let rev_prefix = self.reverse_prefix.clone();
        let range: Vec<_> = map
            .range((lower_suffix, upper_suffix))
            .map(move |(k, v)| {
                if plen > 0 {
                    let mut full = rev_prefix.clone();
                    full.extend_from_slice(k);
                    (full, v.clone())
                } else {
                    (k.clone(), v.clone())
                }
            })
            .collect();
        range.into_iter()
    }

    pub(crate) fn snapshot(&self) -> IndexMaps {
        let fwd = self.forward.load();
        let rev = self.reverse.load();
        let plen_f = self.prefix_forward_len;
        let plen_r = self.prefix_reverse_len;
        let fwd_prefix = self.forward_prefix.clone();
        let rev_prefix = self.reverse_prefix.clone();
        let forward = if plen_f > 0 {
            fwd.iter()
                .map(|(k, v)| {
                    let mut full = fwd_prefix.clone();
                    full.extend_from_slice(k);
                    (full, v.clone())
                })
                .collect()
        } else {
            fwd.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        let reverse = if plen_r > 0 {
            rev.iter()
                .map(|(k, v)| {
                    let mut full = rev_prefix.clone();
                    full.extend_from_slice(k);
                    (full, v.clone())
                })
                .collect()
        } else {
            rev.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
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

        let fwd_map = if self.prefix_forward_len > 0 {
            forward
                .into_iter()
                .map(|(k, v)| {
                    let suffix = self.strip_forward_prefix(&k);
                    wal_size_delta += wal_record_size(&k, &v);
                    wal_guard.push(wal_entry_for(true, k.clone(), v.clone()));
                    (suffix, v)
                })
                .collect()
        } else {
            forward
                .into_iter()
                .map(|(k, v)| {
                    wal_size_delta += wal_record_size(&k, &v);
                    wal_guard.push(wal_entry_for(true, k.clone(), v.clone()));
                    (k, v)
                })
                .collect()
        };

        let rev_map = if self.prefix_reverse_len > 0 {
            reverse
                .into_iter()
                .map(|(k, v)| {
                    let suffix = self.strip_reverse_prefix(&k);
                    wal_size_delta += wal_record_size(&k, &v);
                    wal_guard.push(wal_entry_for(false, k.clone(), v.clone()));
                    (suffix, v)
                })
                .collect()
        } else {
            reverse
                .into_iter()
                .map(|(k, v)| {
                    wal_size_delta += wal_record_size(&k, &v);
                    wal_guard.push(wal_entry_for(false, k.clone(), v.clone()));
                    (k, v)
                })
                .collect()
        };

        self.wal_size_bytes
            .fetch_add(wal_size_delta, Ordering::Relaxed);
        drop(wal_guard);

        self.forward.store(Arc::new(fwd_map));
        self.reverse.store(Arc::new(rev_map));
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
    pub(crate) fn checkpoint<K: IndexKeyGenerator>(&self) -> StorageResult<()> {
        self.full_flush::<K>()?;
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

        let mut fwd = self.forward.load().as_ref().clone();
        let mut rev = self.reverse.load().as_ref().clone();

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

        self.forward.store(Arc::new(fwd));
        self.reverse.store(Arc::new(rev));
        Ok(())
    }

    /// Return current WAL size in bytes (for checkpoint threshold).
    pub(crate) fn wal_size(&self) -> u64 {
        self.wal_size_bytes.load(Ordering::Relaxed)
    }

    /// Iterate all forward entries returning full keys.
    pub(crate) fn iter_forward(&self) -> Vec<(SecondaryIndexKey, IndexRecord)> {
        let map = self.forward.load();
        let plen = self.prefix_forward_len;
        let fwd_prefix = self.forward_prefix.clone();
        map.iter()
            .map(|(k, v)| {
                if plen > 0 {
                    let mut full = fwd_prefix.clone();
                    full.extend_from_slice(k);
                    (full, v.clone())
                } else {
                    (k.clone(), v.clone())
                }
            })
            .collect()
    }

    /// Iterate all reverse entries returning full keys.
    pub(crate) fn iter_reverse(&self) -> Vec<(SecondaryIndexKey, IndexRecord)> {
        let map = self.reverse.load();
        let plen = self.prefix_reverse_len;
        let rev_prefix = self.reverse_prefix.clone();
        map.iter()
            .map(|(k, v)| {
                if plen > 0 {
                    let mut full = rev_prefix.clone();
                    full.extend_from_slice(k);
                    (full, v.clone())
                } else {
                    (k.clone(), v.clone())
                }
            })
            .collect()
    }

    /// Flush to disk. If WAL size is below threshold, only appends WAL entries.
    /// Otherwise performs a full checkpoint.
    pub(crate) fn flush<K: IndexKeyGenerator>(&self) -> StorageResult<()> {
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
            self.checkpoint::<K>()
        }
    }

    /// Full flush without WAL - used by checkpoint.
    fn full_flush<K: IndexKeyGenerator>(&self) -> StorageResult<()> {
        let fwd = self.forward.load();
        let rev = self.reverse.load();
        let plen_f = self.prefix_forward_len;
        let plen_r = self.prefix_reverse_len;
        let fwd_prefix = self.forward_prefix.clone();
        let rev_prefix = self.reverse_prefix.clone();
        let full_fwd: BTreeMap<_, _> = if plen_f > 0 {
            fwd.iter()
                .map(|(k, v)| {
                    let mut full = fwd_prefix.clone();
                    full.extend_from_slice(k);
                    (full, v.clone())
                })
                .collect()
        } else {
            fwd.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        let full_rev: BTreeMap<_, _> = if plen_r > 0 {
            rev.iter()
                .map(|(k, v)| {
                    let mut full = rev_prefix.clone();
                    full.extend_from_slice(k);
                    (full, v.clone())
                })
                .collect()
        } else {
            rev.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        GenericIndexManager::<K>::flush_data(
            &self.checkpoint_file,
            &full_fwd,
            &full_rev,
        )?;
        self.dirty.store(false, Ordering::Release);
        Ok(())
    }

    pub(crate) fn memory_usage_bytes(&self) -> u64 {
        fn map_size(map: &BTreeMap<SecondaryIndexKey, IndexRecord>) -> u64 {
            map.iter()
                .map(|(key, record)| {
                    let included_columns = record
                        .included_columns
                        .as_ref()
                        .map_or(0, |cols| {
                            cols.iter()
                                .map(|(name, value)| {
                                    name.capacity() as u64 + value.estimated_size() as u64
                                })
                                .sum::<u64>()
                        });
                    std::mem::size_of::<IndexRecord>() as u64
                        + key.capacity() as u64
                        + included_columns
                })
                .sum()
        }

        let fwd = self.forward.load();
        let rev = self.reverse.load();
        map_size(&fwd) + map_size(&rev)
    }
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
fn wal_record_size(key: &[u8], record: &IndexRecord) -> u64 {
    (key.len() + std::mem::size_of::<IndexRecord>()) as u64
}

pub(crate) struct GenerationRuntime {
    pub(crate) generation: IndexGeneration,
    shards: HashMap<u32, Arc<ShardRuntime>>,
    parent: Option<Weak<GenerationRuntime>>,
    pub(crate) max_ts: Timestamp,
}

impl GenerationRuntime {
    pub(crate) fn empty(manifest: &IndexManifest) -> Self {
        Self {
            generation: manifest.generation,
            shards: manifest
                .shards
                .iter()
                .map(|shard| {
                    (
                        shard.shard_id,
                        Arc::new(ShardRuntime::empty(shard.checkpoint_file.clone())),
                    )
                })
                .collect(),
            parent: None,
            max_ts: 0,
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
        let prefix_f_len = forward_prefix.len();
        let prefix_r_len = reverse_prefix.len();
        let mut shards = HashMap::new();
        for shard_def in &manifest.shards {
            let (forward, reverse) = maps.get(&shard_def.shard_id).cloned().unwrap_or_default();
            let stripped_fwd = if prefix_f_len > 0 {
                forward.into_iter().map(|(k, v)| {
                    let suffix = k[prefix_f_len..].to_vec();
                    (suffix, v)
                }).collect()
            } else {
                forward
            };
            let stripped_rev = if prefix_r_len > 0 {
                reverse.into_iter().map(|(k, v)| {
                    let suffix = k[prefix_r_len..].to_vec();
                    (suffix, v)
                }).collect()
            } else {
                reverse
            };
            let mut sr = ShardRuntime::empty(shard_def.checkpoint_file.clone());
            sr.forward_prefix = forward_prefix.clone();
            sr.reverse_prefix = reverse_prefix.clone();
            sr.prefix_forward_len = prefix_f_len;
            sr.prefix_reverse_len = prefix_r_len;
            sr.forward.store(Arc::new(stripped_fwd));
            sr.reverse.store(Arc::new(stripped_rev));
            sr.dirty.store(true, Ordering::Release);
            shards.insert(shard_def.shard_id, Arc::new(sr));
        }
        let mut gen = Self {
            generation: manifest.generation,
            shards,
            parent: None,
            max_ts,
        };
        if let Some(p) = parent {
            gen.set_parent(p);
        }
        gen
    }

    fn load<K: IndexKeyGenerator>(manifest: &IndexManifest) -> StorageResult<Self> {
        let mut shards = HashMap::new();
        for shard in &manifest.shards {
            let data = if shard.checkpoint_file.is_dir() {
                ShardRuntime::load::<K>(shard.checkpoint_file.clone())?
            } else {
                ShardRuntime::empty(shard.checkpoint_file.clone())
            };
            shards.insert(shard.shard_id, Arc::new(data));
        }
        Ok(Self {
            generation: manifest.generation,
            shards,
            parent: None,
            max_ts: 0,
        })
    }

    pub(crate) fn set_parent(&mut self, parent: &Arc<GenerationRuntime>) {
        self.parent = Some(Arc::downgrade(parent));
    }

    pub(crate) fn parent_gen(&self) -> Option<Arc<GenerationRuntime>> {
        self.parent.as_ref().and_then(|w| w.upgrade())
    }

    pub(crate) fn shard(&self, shard_id: u32) -> Option<Arc<ShardRuntime>> {
        self.shards.get(&shard_id).cloned()
    }

    pub(crate) fn shards(&self) -> impl Iterator<Item = Arc<ShardRuntime>> + '_ {
        self.shards.values().cloned()
    }

    pub(crate) fn memory_usage_bytes(&self) -> u64 {
        self.shards
            .values()
            .map(|shard| shard.memory_usage_bytes())
            .sum()
    }

    /// Walk the generation chain from this gen backward, yielding (gen, [shards]) tuples.
    pub(crate) fn chain_from(
        gen: Arc<GenerationRuntime>,
    ) -> Vec<Arc<GenerationRuntime>> {
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
    publish_fence: RwLock<()>,
    barrier_lsn: AtomicU64,
    barrier_wait: Mutex<()>,
    barrier_cv: Condvar,
}

impl IndexRuntime {
    pub(crate) fn new(manifest: &IndexManifest) -> Self {
        Self {
            generations: RwLock::new(HashMap::from([(
                manifest.generation,
                Arc::new(GenerationRuntime::empty(manifest)),
            )])),
            publish_fence: RwLock::new(()),
            barrier_lsn: AtomicU64::new(0),
            barrier_wait: Mutex::new(()),
            barrier_cv: Condvar::new(),
        }
    }

    pub(crate) fn load<K: IndexKeyGenerator>(manifest: &IndexManifest) -> StorageResult<Self> {
        Ok(Self {
            generations: RwLock::new(HashMap::from([(
                manifest.generation,
                Arc::new(GenerationRuntime::load::<K>(manifest)?),
            )])),
            publish_fence: RwLock::new(()),
            barrier_lsn: AtomicU64::new(0),
            barrier_wait: Mutex::new(()),
            barrier_cv: Condvar::new(),
        })
    }

    pub(crate) fn generation(&self, generation: IndexGeneration) -> Option<Arc<GenerationRuntime>> {
        self.generations.read().get(&generation).cloned()
    }

    pub(crate) fn install_generation(&self, generation: GenerationRuntime) {
        self.generations
            .write()
            .insert(generation.generation, Arc::new(generation));
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
        // Find the newest generation <= max_generation
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
        let newest_gen = gens.get(&newest).ok_or_else(|| {
            StorageError::not_found(format!("Generation {} not found", newest))
        })?;
        Ok(GenerationRuntime::chain_from(Arc::clone(newest_gen)))
    }

    pub(crate) fn memory_usage_bytes(&self) -> u64 {
        self.generations
            .read()
            .values()
            .map(|generation| generation.memory_usage_bytes())
            .sum()
    }

    pub(crate) fn read_fence(&self) -> parking_lot::RwLockReadGuard<'_, ()> {
        self.publish_fence.read()
    }

    pub(crate) fn write_fence(&self) -> parking_lot::RwLockWriteGuard<'_, ()> {
        self.publish_fence.write()
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

    pub(crate) fn flush_generation<K: IndexKeyGenerator>(
        &self,
        manifest: &IndexManifest,
    ) -> StorageResult<()> {
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
                .flush::<K>()?;
        }
        Ok(())
    }

    /// Checkpoint all generations: full flush + clear WAL.
    pub(crate) fn checkpoint_all<K: IndexKeyGenerator>(
        &self,
        manifest: &IndexManifest,
    ) -> StorageResult<()> {
        let generation = self.generation(manifest.generation).ok_or_else(|| {
            StorageError::not_found(format!(
                "Missing runtime generation {}",
                manifest.generation
            ))
        })?;
        for shard in &manifest.shards {
            if let Some(s) = generation.shard(shard.shard_id) {
                s.checkpoint::<K>()?;
            }
        }
        Ok(())
    }
}

pub(crate) fn generation_from_maps(
    manifest: &IndexManifest,
    maps: HashMap<u32, IndexMaps>,
    parent: Option<&Arc<GenerationRuntime>>,
    max_ts: Timestamp,
    forward_prefix: Vec<u8>,
    reverse_prefix: Vec<u8>,
) -> GenerationRuntime {
    if !forward_prefix.is_empty() || !reverse_prefix.is_empty() {
        return GenerationRuntime::empty_with_maps(
            manifest, forward_prefix, reverse_prefix, maps, parent, max_ts,
        );
    }
    let mut generation = GenerationRuntime::empty(manifest);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> IndexManifest {
        IndexManifest::new(
            1,
            1,
            IndexGeneration::new(1),
            vec![crate::storage::index::manifest::IndexShard {
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
