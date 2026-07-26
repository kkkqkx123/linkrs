//! Generation-scoped native-index storage.
//!
//! Each manifest generation owns independent shard maps. This makes the
//! manifest handle a real data-generation pin instead of metadata only.

use crate::core::types::{CommitLsn, IndexGeneration};
use crate::core::{StorageError, StorageResult};
use crate::storage::index::generic_index_manager::GenericIndexManager;
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::key_codec::IndexKeyGenerator;
use crate::storage::index::manifest::IndexManifest;
use crate::storage::index::types::IndexRecord;
use parking_lot::{Condvar, Mutex, RwLock};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

/// Shared publish barriers used by native indexes and WAL truncation.
pub(crate) type IndexBarrierRegistry = Arc<RwLock<HashMap<(u64, u64), CommitLsn>>>;

pub(crate) type IndexMaps = (
    BTreeMap<SecondaryIndexKey, IndexRecord>,
    BTreeMap<SecondaryIndexKey, IndexRecord>,
);

pub(crate) struct ShardRuntime {
    checkpoint_file: PathBuf,
    forward: RwLock<BTreeMap<SecondaryIndexKey, IndexRecord>>,
    reverse: RwLock<BTreeMap<SecondaryIndexKey, IndexRecord>>,
    version_counter: AtomicU64,
}

impl ShardRuntime {
    fn empty(checkpoint_file: PathBuf) -> Self {
        Self {
            checkpoint_file,
            forward: RwLock::new(BTreeMap::new()),
            reverse: RwLock::new(BTreeMap::new()),
            version_counter: AtomicU64::new(1),
        }
    }

    fn load<K: IndexKeyGenerator>(checkpoint_file: PathBuf) -> StorageResult<Self> {
        let (forward, reverse, next_version) =
            GenericIndexManager::<K>::load_data(&checkpoint_file)?;
        Ok(Self {
            checkpoint_file,
            forward: RwLock::new(forward),
            reverse: RwLock::new(reverse),
            version_counter: AtomicU64::new(next_version),
        })
    }

    pub(crate) fn physical_key(&self, logical: &[u8]) -> SecondaryIndexKey {
        let version = self.version_counter.fetch_add(1, Ordering::Relaxed);
        let mut key = Vec::with_capacity(logical.len() + std::mem::size_of::<u64>());
        key.extend_from_slice(logical);
        key.extend_from_slice(&version.to_le_bytes());
        key
    }

    pub(crate) fn forward(&self) -> &RwLock<BTreeMap<SecondaryIndexKey, IndexRecord>> {
        &self.forward
    }

    pub(crate) fn reverse(&self) -> &RwLock<BTreeMap<SecondaryIndexKey, IndexRecord>> {
        &self.reverse
    }

    pub(crate) fn snapshot(&self) -> IndexMaps {
        (self.forward.read().clone(), self.reverse.read().clone())
    }

    pub(crate) fn replace(
        &self,
        forward: BTreeMap<SecondaryIndexKey, IndexRecord>,
        reverse: BTreeMap<SecondaryIndexKey, IndexRecord>,
    ) {
        *self.forward.write() = forward;
        *self.reverse.write() = reverse;
    }

    pub(crate) fn flush<K: IndexKeyGenerator>(&self) -> StorageResult<()> {
        GenericIndexManager::<K>::flush_data(
            &self.checkpoint_file,
            &self.forward.read(),
            &self.reverse.read(),
        )
    }

    pub(crate) fn memory_usage_bytes(&self) -> u64 {
        fn map_size(map: &BTreeMap<SecondaryIndexKey, IndexRecord>) -> u64 {
            map.iter()
                .map(|(key, record)| {
                    let included_columns = record
                        .included_columns
                        .iter()
                        .map(|(name, value)| name.capacity() as u64 + value.estimated_size() as u64)
                        .sum::<u64>();
                    std::mem::size_of::<IndexRecord>() as u64
                        + key.capacity() as u64
                        + included_columns
                })
                .sum()
        }

        map_size(&self.forward.read()) + map_size(&self.reverse.read())
    }
}

pub(crate) struct GenerationRuntime {
    pub(crate) generation: IndexGeneration,
    shards: HashMap<u32, Arc<ShardRuntime>>,
}

impl GenerationRuntime {
    fn empty(manifest: &IndexManifest) -> Self {
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
        }
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
        })
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

    pub(crate) fn generations(&self) -> Vec<Arc<GenerationRuntime>> {
        self.generations.read().values().cloned().collect()
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
}

pub(crate) fn generation_from_maps(
    manifest: &IndexManifest,
    maps: HashMap<u32, IndexMaps>,
) -> GenerationRuntime {
    let generation = GenerationRuntime::empty(manifest);
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
