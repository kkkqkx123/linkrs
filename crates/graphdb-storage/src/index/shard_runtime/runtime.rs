use crate::index::manifest::IndexManifest;
use crate::index::shard_runtime::generation::GenerationRuntime;
use graphdb_core::types::{CommitLsn, IndexGeneration};
use graphdb_core::{StorageError, StorageResult};
use parking_lot::{Condvar, Mutex, RwLock};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
