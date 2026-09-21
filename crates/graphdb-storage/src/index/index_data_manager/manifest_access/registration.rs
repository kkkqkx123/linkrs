use crate::index::manifest::{IndexManifest, IndexShard, ManifestCatalog, ManifestHandle};
use crate::index::shard_runtime::{GenerationRuntime, IndexMaps, IndexRuntime};
use crate::index::types::IndexIdentity;
use graphdb_core::types::{Index, IndexGeneration};
use graphdb_core::{StorageError, StorageResult};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::super::remove_dir_if_empty;
use super::super::IndexDataManagerImpl;

impl IndexDataManagerImpl {
    pub fn register_native_index(&self, space_id: u64, index: &Index) -> StorageResult<()> {
        if index.space_id != space_id {
            return Err(StorageError::invalid_operation(format!(
                "Index {} belongs to space {}, not space {}",
                index.name, index.space_id, space_id
            )));
        }
        let index_id = index.id;
        let identity = IndexIdentity { space_id, index_id };
        self.index_aliases
            .write()
            .insert((space_id, index.name.clone()), index_id);
        self.index_types
            .write()
            .insert(identity, index.index_type.clone());
        self.index_definitions
            .write()
            .insert(identity, index.clone());
        let mut catalogs = self.manifest_catalogs.write();
        let catalog_already_loaded = catalogs.contains_key(&identity);
        if let std::collections::hash_map::Entry::Vacant(e) = catalogs.entry(identity) {
            let manifest = IndexManifest::new(
                space_id,
                index_id,
                IndexGeneration::new(1),
                vec![IndexShard {
                    shard_id: 0,
                    lower: None,
                    upper: None,
                    checkpoint_file: self.initial_checkpoint_path(space_id, index_id),
                    checksum: None,
                }],
            )?;
            e.insert(Arc::new(ManifestCatalog::new(manifest)?));
        }
        drop(catalogs);
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no manifest")))?;
        let manifest = catalog.acquire().manifest().clone();
        self.runtimes.write().entry(identity).or_insert_with(|| {
            // When the catalog was restored from disk, its manifest points to real
            // shard checkpoint files. Load the runtime from disk so that shard data
            // is actually available, instead of creating an empty generation that
            // satisfies the `restore_active_generation` check without any data.
            let has_disk_shards = catalog_already_loaded
                && manifest.shards.iter().any(|s| s.checkpoint_file.is_dir());
            if has_disk_shards {
                let pool_cap = self.pool_capacity.load(Ordering::Relaxed);
                let runtime = IndexRuntime::load_with_pool_capacity(&manifest, pool_cap)
                    .unwrap_or_else(|_| IndexRuntime::new(&manifest));
                Arc::new(runtime)
            } else {
                Arc::new(IndexRuntime::new(&manifest))
            }
        });
        self.restore_active_generation(identity)?;
        self.sync_memory_usage();
        Ok(())
    }

    pub(crate) fn restore_active_generation(&self, identity: IndexIdentity) -> StorageResult<()> {
        let catalog = self
            .manifest_catalog(identity.space_id, identity.index_id)
            .ok_or_else(|| {
                StorageError::not_found(format!("Index {} has no manifest", identity.index_id))
            })?;
        let handle = catalog.acquire();
        let generation = handle.manifest().generation;
        if self.restored_generations.read().get(&identity) == Some(&generation) {
            return Ok(());
        }
        // If the existing runtime already has this generation installed
        // (e.g. via publish_native_index during rebuild), don't overwrite
        // it by loading from the manifest's checkpoint file, which may be
        // empty in in-memory mode.
        if let Some(runtime) = self.runtimes.read().get(&identity) {
            let has_gen = runtime.generation(generation).is_some();
            if has_gen {
                self.restored_generations
                    .write()
                    .insert(identity, generation);
                return Ok(());
            }
        }
        let pool_cap = self.pool_capacity.load(Ordering::Relaxed);
        let runtime = IndexRuntime::load_with_pool_capacity(handle.manifest(), pool_cap)?;
        self.runtimes.write().insert(identity, Arc::new(runtime));
        self.restored_generations
            .write()
            .insert(identity, generation);
        Ok(())
    }

    pub fn unregister_native_index(&self, space_id: u64, index_name: &str) {
        if let Some(index_id) = self
            .index_aliases
            .write()
            .remove(&(space_id, index_name.to_string()))
        {
            let identity = IndexIdentity { space_id, index_id };
            self.manifest_catalogs.write().remove(&identity);
            self.runtimes.write().remove(&identity);
            self.index_types.write().remove(&identity);
            self.index_definitions.write().remove(&identity);
            self.restored_generations.write().remove(&identity);
            self.barrier_registry
                .write()
                .remove(&(identity.space_id, identity.index_id));
        }
    }

    /// Physically remove checkpoint directories of a dropped index.
    ///
    /// Tombstone `clear_index` alone leaves generation files behind for MVCC
    /// readers; once the index is unregistered no reader can pin them, so the
    /// directories under `{index_root}/{space_id}/{index_id}` plus the crash
    /// recovery marker `generation_build_state/{space_id}/{name}_*` are safe
    /// to delete. Best effort: missing root or already removed paths are ignored.
    pub fn remove_index_checkpoint_dirs(&self, space_id: u64, index_name: &str) {
        let index_id = self.index_alias(space_id, index_name);
        if let Some(index_id) = index_id {
            self.remove_checkpoint_dirs_by_id(space_id, index_id);
            return;
        }
        let candidates: Vec<u64> = self
            .index_definitions
            .read()
            .keys()
            .filter(|identity| identity.space_id == space_id)
            .map(|identity| identity.index_id)
            .collect();
        for candidate in candidates {
            self.remove_checkpoint_dirs_by_id(space_id, candidate);
        }
        if let Some(root) = self.index_root.as_ref() {
            let space_dir = root.join(format!("{space_id}"));
            remove_dir_if_empty(&space_dir);
        }
        self.remove_generation_build_markers(space_id, Some(index_name));
    }

    /// Remove checkpoint directories by resolved numeric index id.
    pub fn remove_checkpoint_dirs_by_id(&self, space_id: u64, index_id: u64) {
        if let Some(root) = self.index_root.as_ref() {
            let index_dir = root.join(format!("{space_id}/{index_id}"));
            if index_dir.exists() {
                if let Err(error) = std::fs::remove_dir_all(&index_dir) {
                    log::warn!(
                        "Failed to remove checkpoint dir {} for dropped index: {error}",
                        index_dir.display()
                    );
                }
            }
            remove_dir_if_empty(&root.join(format!("{space_id}")));
        }
        self.remove_generation_build_markers(space_id, None);
    }

    fn remove_generation_build_markers(&self, space_id: u64, index_name: Option<&str>) {
        if let Some(root) = self.index_root.as_ref() {
            let state_dir = root
                .join("generation_build_state")
                .join(format!("{space_id}"));
            if !state_dir.exists() {
                return;
            }
            let entries = std::fs::read_dir(&state_dir);
            let Ok(entries) = entries else {
                return;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let matched = match index_name {
                    Some(wanted) => name.starts_with(&format!("{wanted}_generation_build")),
                    None => name.ends_with("_generation_build.json"),
                };
                if matched {
                    if let Err(error) = std::fs::remove_file(entry.path()) {
                        log::warn!(
                            "Failed to remove generation build marker {}: {error}",
                            entry.path().display()
                        );
                    }
                }
            }
            remove_dir_if_empty(&state_dir);
        }
    }

    pub fn manifest_catalog(&self, space_id: u64, index_id: u64) -> Option<Arc<ManifestCatalog>> {
        self.manifest_catalogs
            .read()
            .get(&IndexIdentity { space_id, index_id })
            .cloned()
    }

    pub fn index_alias(&self, space_id: u64, index_name: &str) -> Option<u64> {
        self.index_aliases
            .read()
            .get(&(space_id, index_name.to_string()))
            .copied()
    }

    pub(crate) fn runtime(&self, space_id: u64, index_id: u64) -> StorageResult<Arc<IndexRuntime>> {
        self.runtimes
            .read()
            .get(&IndexIdentity { space_id, index_id })
            .cloned()
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no runtime")))
    }

    pub(crate) fn active_generation(
        &self,
        space_id: u64,
        index_id: u64,
    ) -> StorageResult<(ManifestHandle, Arc<IndexRuntime>, Arc<GenerationRuntime>)> {
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no manifest")))?;
        let handle = catalog.acquire();
        let runtime = self.runtime(space_id, index_id)?;
        let generation = runtime
            .generation(handle.manifest().generation)
            .ok_or_else(|| {
                StorageError::not_found(format!(
                    "Index {index_id} has no active runtime generation"
                ))
            })?;
        Ok((handle, runtime, generation))
    }

    pub(crate) fn active_index_data(
        &self,
        space_id: u64,
        index_id: u64,
    ) -> StorageResult<IndexMaps> {
        let (_handle, _runtime, generation) = self.active_generation(space_id, index_id)?;
        let mut forward = BTreeMap::new();
        let mut reverse = BTreeMap::new();
        for shard in generation.shards() {
            let (shard_forward, shard_reverse) = shard.snapshot();
            forward.extend(shard_forward);
            reverse.extend(shard_reverse);
        }
        Ok((forward, reverse))
    }
}
