//! Generation-scoped native-index storage.
//!
//! Each manifest generation owns independent shard maps. This makes the
//! manifest handle a real data-generation pin instead of metadata only.

use crate::core::types::IndexGeneration;
use crate::core::{StorageError, StorageResult};
use crate::storage::index::generic_index_manager::GenericIndexManager;
use crate::storage::index::index_data_manager::IndexRecord;
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::key_codec::IndexKeyGenerator;
use crate::storage::index::manifest::IndexManifest;
use parking_lot::RwLock;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

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
}

/// The sole mutable native-index data owner for one index.
pub(crate) struct IndexRuntime {
    generations: RwLock<HashMap<IndexGeneration, Arc<GenerationRuntime>>>,
    publish_fence: RwLock<()>,
}

impl IndexRuntime {
    pub(crate) fn new(manifest: &IndexManifest) -> Self {
        Self {
            generations: RwLock::new(HashMap::from([(
                manifest.generation,
                Arc::new(GenerationRuntime::empty(manifest)),
            )])),
            publish_fence: RwLock::new(()),
        }
    }

    pub(crate) fn load<K: IndexKeyGenerator>(manifest: &IndexManifest) -> StorageResult<Self> {
        Ok(Self {
            generations: RwLock::new(HashMap::from([(
                manifest.generation,
                Arc::new(GenerationRuntime::load::<K>(manifest)?),
            )])),
            publish_fence: RwLock::new(()),
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

    pub(crate) fn remove_generation(&self, generation: IndexGeneration) {
        self.generations.write().remove(&generation);
    }

    pub(crate) fn generations(&self) -> Vec<Arc<GenerationRuntime>> {
        self.generations.read().values().cloned().collect()
    }

    pub(crate) fn read_fence(&self) -> parking_lot::RwLockReadGuard<'_, ()> {
        self.publish_fence.read()
    }

    pub(crate) fn write_fence(&self) -> parking_lot::RwLockWriteGuard<'_, ()> {
        self.publish_fence.write()
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
