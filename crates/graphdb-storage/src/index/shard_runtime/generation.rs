use crate::index::manifest::IndexManifest;
use crate::index::shard_runtime::shard::ShardRuntime;
use crate::index::shard_runtime::IndexMaps;
use graphdb_core::types::{IndexGeneration, Timestamp};
use graphdb_core::StorageResult;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

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

    pub(crate) fn load_with_pool_capacity(
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
