//! Index Data Manager
//!
//! Provide update, delete and query functions for indexed data
//! The management of index metadata is handled by the IndexMetadataManager.
//! All operations identify a space by its space_id, enabling multi-space data segregation.
//! Supports persistence through flush/load operations.
//! Supports MVCC (Multi-Version Concurrency Control) for snapshot isolation.

use crate::core::types::{
    CommitLsn, Index, IndexGeneration, IndexType, ManifestEpoch, SnapshotTimestamp, Timestamp,
    MAX_TIMESTAMP,
};
use crate::core::wal::EntityRef;
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::cursor::IndexScanPlan;
use crate::storage::index::edge_index_manager::EdgeIndexManager;
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::key_codec::{KeyBuilder, KeyParser};
use crate::storage::index::manifest::{
    GenerationBuildState, GenerationState, IndexManifest, IndexShard, ManifestCatalog,
};
use crate::storage::index::shard_runtime::{
    generation_from_maps, GenerationRuntime, IndexMaps, IndexRuntime,
};
use crate::storage::index::vertex_index_manager::VertexIndexManager;
use parking_lot::RwLock;
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct IndexIdentity {
    space_id: u64,
    index_id: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexRecord {
    pub created_ts: Timestamp,
    pub deleted_ts: Option<Timestamp>,
    pub entity_version: Option<Timestamp>,
    pub included_columns: Vec<(String, Value)>,
    pub entity_ref: Option<EntityRef>,
}

impl IndexRecord {
    pub fn new(created_ts: Timestamp) -> Self {
        Self {
            created_ts,
            deleted_ts: None,
            entity_version: None,
            included_columns: Vec::new(),
            entity_ref: None,
        }
    }

    pub fn new_with_columns(created_ts: Timestamp, included_columns: Vec<(String, Value)>) -> Self {
        Self {
            created_ts,
            deleted_ts: None,
            entity_version: None,
            included_columns,
            entity_ref: None,
        }
    }

    pub fn with_entity_ref(mut self, entity_ref: EntityRef) -> Self {
        self.entity_ref = Some(entity_ref);
        self
    }

    pub fn with_entity_version(mut self, version: Timestamp) -> Self {
        self.entity_version = Some(version);
        self
    }

    pub fn is_visible_at(&self, read_ts: Timestamp) -> bool {
        self.created_ts <= read_ts
            && self
                .deleted_ts
                .is_none_or(|deleted_ts| deleted_ts > read_ts)
    }

    pub fn mark_deleted(&mut self, deleted_ts: Timestamp) {
        self.deleted_ts = Some(deleted_ts);
    }
}

impl Default for IndexRecord {
    fn default() -> Self {
        Self::new(MAX_TIMESTAMP)
    }
}

/// Vertex index operations trait.
/// Provides update, delete, and lookup operations for vertex indexes.
pub trait VertexIndexOps: Send + Sync {
    fn update_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_name: &str,
        props: &[(String, Value)],
        write_ts: Timestamp,
    ) -> Result<(), StorageError>;

    fn delete_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_names: &[String],
        write_ts: Timestamp,
    ) -> Result<(), StorageError>;

    fn lookup_tag_index(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
    ) -> Result<Vec<Value>, StorageError> {
        self.lookup_tag_index_mvcc(space_id, index, value, MAX_TIMESTAMP)
    }

    fn lookup_tag_index_mvcc(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
        read_ts: Timestamp,
    ) -> Result<Vec<Value>, StorageError>;

    fn clear_tag_index(&self, space_id: u64, index_name: &str) -> Result<(), StorageError>;
}

/// Edge index operations trait.
/// Provides update, delete, and lookup operations for edge indexes.
pub trait EdgeIndexOps: Send + Sync {
    fn update_edge_indexes_mvcc(
        &self,
        space_id: u64,
        edge_src: &Value,
        edge_dst: &Value,
        edge_type: &str,
        ranking: i64,
        index_name: &str,
        props: &[(String, Value)],
        write_ts: Timestamp,
    ) -> Result<(), StorageError>;

    fn delete_edge_indexes_mvcc(
        &self,
        space_id: u64,
        edge_src: &Value,
        edge_dst: &Value,
        edge_type: &str,
        ranking: i64,
        index_names: &[String],
        write_ts: Timestamp,
    ) -> Result<(), StorageError>;

    fn lookup_edge_index(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
    ) -> Result<Vec<(Value, Value, String, i64)>, StorageError> {
        self.lookup_edge_index_mvcc(space_id, index, value, MAX_TIMESTAMP)
    }

    fn lookup_edge_index_mvcc(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
        read_ts: Timestamp,
    ) -> Result<Vec<(Value, Value, String, i64)>, StorageError>;

    fn clear_edge_index(&self, space_id: u64, index_name: &str) -> Result<(), StorageError>;
}

/// Index garbage collection operations trait.
pub trait IndexGcOps: Send + Sync {
    fn gc_tombstones(&self, safe_ts: Timestamp) -> Result<GcStats, StorageError>;
    fn gc_tombstones_incremental(
        &self,
        safe_ts: Timestamp,
        batch_size: usize,
    ) -> Result<GcStats, StorageError>;
    fn tombstone_count(&self) -> usize;
}

#[derive(Clone)]
pub struct IndexDataManagerImpl {
    manifest_catalogs: Arc<RwLock<HashMap<IndexIdentity, Arc<ManifestCatalog>>>>,
    runtimes: Arc<RwLock<HashMap<IndexIdentity, Arc<IndexRuntime>>>>,
    index_aliases: Arc<RwLock<HashMap<(u64, String), u64>>>,
    index_types: Arc<RwLock<HashMap<IndexIdentity, IndexType>>>,
    restored_generations: Arc<RwLock<HashMap<IndexIdentity, IndexGeneration>>>,
}

impl IndexDataManagerImpl {
    pub fn new() -> Self {
        Self {
            manifest_catalogs: Arc::new(RwLock::new(HashMap::new())),
            runtimes: Arc::new(RwLock::new(HashMap::new())),
            index_aliases: Arc::new(RwLock::new(HashMap::new())),
            index_types: Arc::new(RwLock::new(HashMap::new())),
            restored_generations: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register_native_index(&self, space_id: u64, index: &Index) -> StorageResult<()> {
        if index.space_id != space_id {
            return Err(StorageError::invalid_operation(format!(
                "Index {} belongs to space {}, not space {}",
                index.name, index.space_id, space_id
            )));
        }
        let index_id = u64::try_from(index.id)
            .map_err(|_| StorageError::invalid_operation("Index ID cannot be negative"))?;
        let identity = IndexIdentity { space_id, index_id };
        self.index_aliases
            .write()
            .insert((space_id, index.name.clone()), index_id);
        self.index_types
            .write()
            .insert(identity, index.index_type.clone());
        let mut catalogs = self.manifest_catalogs.write();
        if !catalogs.contains_key(&identity) {
            let manifest = IndexManifest::new(
                space_id,
                index_id,
                IndexGeneration::new(1),
                ManifestEpoch::new(1),
                vec![IndexShard {
                    shard_id: 0,
                    lower: None,
                    upper: None,
                    checkpoint_file: format!(
                        "native_index/{space_id}/{index_id}/generation-1/shard-0"
                    )
                    .into(),
                }],
            )
            .map_err(StorageError::db_error)?;
            catalogs.insert(
                identity,
                Arc::new(ManifestCatalog::new(manifest).map_err(StorageError::db_error)?),
            );
        }
        drop(catalogs);
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no manifest")))?;
        self.runtimes
            .write()
            .entry(identity)
            .or_insert_with(|| Arc::new(IndexRuntime::new(catalog.acquire().manifest())));
        self.restore_active_generation(identity, index)?;
        Ok(())
    }

    fn restore_active_generation(
        &self,
        identity: IndexIdentity,
        index: &Index,
    ) -> StorageResult<()> {
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
        let runtime = match index.index_type {
            IndexType::TagIndex => IndexRuntime::load::<
                crate::storage::index::key_codec::VertexIndexKeyGen,
            >(handle.manifest())?,
            IndexType::EdgeIndex => IndexRuntime::load::<
                crate::storage::index::key_codec::EdgeIndexKeyGen,
            >(handle.manifest())?,
        };
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
            self.restored_generations.write().remove(&identity);
        }
    }

    pub fn manifest_catalog(&self, space_id: u64, index_id: u64) -> Option<Arc<ManifestCatalog>> {
        self.manifest_catalogs
            .read()
            .get(&IndexIdentity { space_id, index_id })
            .cloned()
    }

    /// Look up the index_id for a (space_id, index_name) alias.
    pub fn index_alias(&self, space_id: u64, index_name: &str) -> Option<u64> {
        self.index_aliases
            .read()
            .get(&(space_id, index_name.to_string()))
            .copied()
    }

    fn runtime(&self, space_id: u64, index_id: u64) -> StorageResult<Arc<IndexRuntime>> {
        self.runtimes
            .read()
            .get(&IndexIdentity { space_id, index_id })
            .cloned()
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no runtime")))
    }

    fn active_generation(
        &self,
        space_id: u64,
        index_id: u64,
    ) -> StorageResult<(
        crate::storage::index::manifest::ManifestHandle,
        Arc<IndexRuntime>,
        Arc<GenerationRuntime>,
    )> {
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
        let (_handle, runtime, generation) = self.active_generation(space_id, index_id)?;
        let _fence = runtime.read_fence();
        let mut forward = BTreeMap::new();
        let mut reverse = BTreeMap::new();
        for shard in generation.shards() {
            let (shard_forward, shard_reverse) = shard.snapshot();
            forward.extend(shard_forward);
            reverse.extend(shard_reverse);
        }
        Ok((forward, reverse))
    }

    pub(crate) fn publish_generation_data(
        &self,
        manifest: IndexManifest,
        forward: BTreeMap<SecondaryIndexKey, IndexRecord>,
        reverse: BTreeMap<SecondaryIndexKey, IndexRecord>,
    ) -> StorageResult<()> {
        let runtime = self.runtime(manifest.space_id, manifest.index_id)?;
        let _fence = runtime.write_fence();
        let mut maps = HashMap::new();
        let shard = manifest
            .shards
            .first()
            .ok_or_else(|| StorageError::invalid_operation("Index manifest has no shards"))?;
        maps.insert(shard.shard_id, (forward, reverse));
        runtime.install_generation(generation_from_maps(&manifest, maps));
        self.manifest_catalog(manifest.space_id, manifest.index_id)
            .ok_or_else(|| StorageError::not_found("Index manifest catalog is unavailable"))?
            .publish(manifest)
            .map_err(StorageError::db_error)?;
        Ok(())
    }

    fn build_state_path(&self, index_root: &Path) -> PathBuf {
        index_root.join("generation_build.json")
    }

    fn save_build_state(
        &self,
        index_root: &Path,
        state: &GenerationBuildState,
    ) -> StorageResult<()> {
        std::fs::create_dir_all(index_root)?;
        let path = self.build_state_path(index_root);
        let temporary = path.with_extension("tmp");
        let serialized =
            serde_json::to_vec(state).map_err(|e| StorageError::serialize_error(e.to_string()))?;
        {
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(&serialized)?;
            file.sync_all()?;
        }
        std::fs::rename(&temporary, &path)?;
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    }

    fn load_build_state(&self, index_root: &Path) -> StorageResult<Option<GenerationBuildState>> {
        let path = self.build_state_path(index_root);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path)?;
        let state: GenerationBuildState = serde_json::from_slice(&bytes)
            .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
        Ok(Some(state))
    }

    fn remove_build_state(&self, index_root: &Path) -> StorageResult<()> {
        let path = self.build_state_path(index_root);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// Resolve orphaned split build state at startup or before a new split.
    /// Incomplete builds (Building) are discarded. Publishing builds with a
    /// published manifest are completed; otherwise the partial checkpoint
    /// directories are left for the next split to overwrite.
    pub fn resolve_split_crash_recovery(&self, index_root: &Path) -> StorageResult<()> {
        let Some(build_state) = self.load_build_state(index_root)? else {
            return Ok(());
        };
        if matches!(build_state.state, GenerationState::Building) {
            log::warn!(
                "Discarding incomplete split build state for gen {}",
                build_state.generation
            );
            self.remove_build_state(index_root)?;
        }
        if matches!(build_state.state, GenerationState::Publishing) {
            let manifest_path = index_root.join("manifest.json");
            if manifest_path.exists() {
                log::info!("Completing split build from Publishing state");
                let mut completed = build_state;
                completed.state = GenerationState::Active;
                self.save_build_state(index_root, &completed)?;
                self.remove_build_state(index_root)?;
            } else {
                log::warn!("Split in Publishing state but no manifest found; discarding");
                self.remove_build_state(index_root)?;
            }
        }
        Ok(())
    }

    pub fn split_native_index(
        &self,
        space_id: u64,
        index_id: u64,
        boundary: Vec<u8>,
        barrier_lsn: CommitLsn,
    ) -> StorageResult<()> {
        if boundary.is_empty() {
            return Err(StorageError::invalid_operation(
                "Index split boundary cannot be empty",
            ));
        }
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no manifest")))?;
        let runtime = self.runtime(space_id, index_id)?;
        // The exclusive fence is the publish barrier: no writer can observe a
        // partially materialized generation or keep writing the old one after
        // the manifest publication.
        let _fence = runtime.write_fence();
        let current = catalog.acquire().manifest().clone();
        let split_position = current
            .shards
            .iter()
            .position(|shard| shard.contains(&boundary))
            .ok_or_else(|| {
                StorageError::invalid_operation("Split boundary is outside key space")
            })?;
        let shard = &current.shards[split_position];
        if shard.lower.as_ref() == Some(&boundary) || shard.upper.as_ref() == Some(&boundary) {
            return Err(StorageError::invalid_operation(
                "Split boundary must be inside an existing shard",
            ));
        }

        let generation = IndexGeneration::new(current.generation.get().saturating_add(1));
        let epoch = ManifestEpoch::new(current.epoch.get().saturating_add(1));
        let next_shard_id = current
            .shards
            .iter()
            .map(|entry| entry.shard_id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let checkpoint_parent = shard.checkpoint_file.parent().ok_or_else(|| {
            StorageError::invalid_operation("Index shard checkpoint path has no parent")
        })?;
        let index_root = shard
            .checkpoint_file
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("shard-"))
            .then(|| {
                checkpoint_parent.parent().ok_or_else(|| {
                    StorageError::invalid_operation("Index shard generation path has no parent")
                })
            })
            .transpose()?
            .unwrap_or(checkpoint_parent);

        // ── Phase: Building ──────────────────────────────────────────────
        self.resolve_split_crash_recovery(index_root)?;
        let mut build_state = GenerationBuildState::new(
            generation,
            epoch,
            SnapshotTimestamp::new(0),
            CommitLsn::ZERO,
        );
        self.save_build_state(index_root, &build_state)?;

        let next_generation_dir = index_root.join(format!("generation-{}", generation.get()));
        let shard_a_path = next_generation_dir.join(format!("shard-{}", shard.shard_id));
        let shard_b_path = next_generation_dir.join(format!("shard-{next_shard_id}"));

        let index_type = self
            .index_types
            .read()
            .get(&IndexIdentity { space_id, index_id })
            .cloned()
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no type")))?;

        let mut shards = current.shards.clone();
        shards.splice(
            split_position..=split_position,
            [
                IndexShard {
                    shard_id: shard.shard_id,
                    lower: shard.lower.clone(),
                    upper: Some(boundary.clone()),
                    checkpoint_file: shard_a_path,
                },
                IndexShard {
                    shard_id: next_shard_id,
                    lower: Some(boundary.clone()),
                    upper: shard.upper.clone(),
                    checkpoint_file: shard_b_path,
                },
            ],
        );
        let next = IndexManifest::new(space_id, index_id, generation, epoch, shards)
            .map_err(StorageError::db_error)?;
        let active = runtime
            .generation(current.generation)
            .ok_or_else(|| StorageError::not_found("Active runtime generation is missing"))?;
        let mut maps: HashMap<u32, IndexMaps> = HashMap::new();
        for current_shard in &current.shards {
            let (forward, reverse) = active
                .shard(current_shard.shard_id)
                .ok_or_else(|| StorageError::not_found("Active runtime shard is missing"))?
                .snapshot();
            if current_shard.shard_id != shard.shard_id {
                maps.insert(current_shard.shard_id, (forward, reverse));
                continue;
            }
            let (forward_a, forward_b): (BTreeMap<_, _>, BTreeMap<_, _>) = forward
                .into_iter()
                .partition(|(key, _)| key.as_slice() < boundary.as_slice());
            let entities_a = index_entities(&forward_a);
            let entities_b = index_entities(&forward_b);
            let (reverse_a, reverse_b): (BTreeMap<_, _>, BTreeMap<_, _>) =
                reverse.into_iter().partition(|(_, entry)| {
                    entry
                        .entity_ref
                        .as_ref()
                        .is_some_and(|entity| entities_a.contains(entity))
                });
            // A reverse record has no value key. Its owning forward record
            // determines the shard; entries without an EntityRef are retained
            // with the upper partition so no data is silently discarded.
            let mut reverse_b = reverse_b;
            reverse_b.retain(|_, entry| {
                entry
                    .entity_ref
                    .as_ref()
                    .is_none_or(|entity| entities_b.contains(entity))
            });
            maps.insert(shard.shard_id, (forward_a, reverse_a));
            maps.insert(next_shard_id, (forward_b, reverse_b));
        }
        let next_runtime = generation_from_maps(&next, maps);
        match index_type {
            IndexType::TagIndex => {
                for entry in &next.shards {
                    let data = next_runtime.shard(entry.shard_id).ok_or_else(|| {
                        StorageError::not_found("Split generation shard is unavailable")
                    })?;
                    crate::storage::index::generic_index_manager::GenericIndexManager::<
                        crate::storage::index::key_codec::VertexIndexKeyGen,
                    >::flush_data(
                        &entry.checkpoint_file,
                        &data.forward().read(),
                        &data.reverse().read(),
                    )?;
                }
            }
            IndexType::EdgeIndex => {
                for entry in &next.shards {
                    let data = next_runtime.shard(entry.shard_id).ok_or_else(|| {
                        StorageError::not_found("Split generation shard is unavailable")
                    })?;
                    crate::storage::index::generic_index_manager::GenericIndexManager::<
                        crate::storage::index::key_codec::EdgeIndexKeyGen,
                    >::flush_data(
                        &entry.checkpoint_file,
                        &data.forward().read(),
                        &data.reverse().read(),
                    )?;
                }
            }
        }

        // ── Phase: Publishing ────────────────────────────────────────────
        build_state
            .transition_from_building_to_publishing(barrier_lsn)
            .map_err(StorageError::invalid_operation)?;
        self.save_build_state(index_root, &build_state)?;

        next.store(&index_root.join("manifest.json"))?;
        runtime.install_generation(next_runtime);
        catalog.publish(next).map_err(StorageError::db_error)?;

        // ── Phase: Active ────────────────────────────────────────────────
        build_state
            .transition_to_active()
            .map_err(StorageError::invalid_operation)?;
        self.remove_build_state(index_root)?;
        Ok(())
    }

    pub fn take_reclaimable_index_files(&self) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        for (index_id, catalog) in self.manifest_catalogs.read().iter() {
            let runtime = self.runtimes.read().get(index_id).cloned();
            for manifest in catalog.take_reclaimable_manifests() {
                if let Some(runtime) = &runtime {
                    runtime.remove_generation(manifest.generation);
                }
                files.extend(
                    manifest
                        .shards
                        .into_iter()
                        .map(|shard| shard.checkpoint_file),
                );
            }
        }
        files
    }

    pub fn flush<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        let manifest_dir = path.join("native_index_manifests");
        std::fs::create_dir_all(&manifest_dir)?;
        for (identity, catalog) in self.manifest_catalogs.read().iter() {
            let manifest = catalog.acquire();
            let runtime = self.runtime(identity.space_id, identity.index_id)?;
            match self.index_types.read().get(identity) {
                Some(IndexType::TagIndex) => runtime
                    .flush_generation::<crate::storage::index::key_codec::VertexIndexKeyGen>(
                    manifest.manifest(),
                )?,
                Some(IndexType::EdgeIndex) => runtime
                    .flush_generation::<crate::storage::index::key_codec::EdgeIndexKeyGen>(
                    manifest.manifest(),
                )?,
                None => continue,
            }
            manifest.manifest().store(
                &manifest_dir.join(format!("{}-{}.json", identity.space_id, identity.index_id)),
            )?;
        }
        Ok(())
    }

    pub fn load<P: AsRef<Path>>(&mut self, path: P) -> StorageResult<()> {
        let path = path.as_ref();
        let manifest_dir = path.join("native_index_manifests");
        if manifest_dir.is_dir() {
            let mut catalogs = self.manifest_catalogs.write();
            for entry in std::fs::read_dir(manifest_dir)? {
                let manifest_path = entry?.path();
                if manifest_path.extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let manifest = IndexManifest::load(&manifest_path)?;
                catalogs.insert(
                    IndexIdentity {
                        space_id: manifest.space_id,
                        index_id: manifest.index_id,
                    },
                    Arc::new(ManifestCatalog::new(manifest).map_err(StorageError::db_error)?),
                );
            }
        }
        for entry in std::fs::read_dir(path)? {
            let candidate = entry?.path().join("manifest.json");
            if !candidate.is_file() {
                continue;
            }
            let manifest = IndexManifest::load(&candidate)?;
            self.manifest_catalogs.write().insert(
                IndexIdentity {
                    space_id: manifest.space_id,
                    index_id: manifest.index_id,
                },
                Arc::new(ManifestCatalog::new(manifest).map_err(StorageError::db_error)?),
            );
        }
        Ok(())
    }

    pub fn open_tag_index_cursor(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
    ) -> StorageResult<crate::storage::index::vertex_index_manager::VertexIndexCursor> {
        self.register_native_index(space_id, index)?;
        self.open_tag_index_cursor_full(space_id, index, plan, None, None)
    }

    pub fn open_tag_index_cursor_with_checker(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
        stale_checker: Option<
            Arc<
                dyn Fn(&crate::core::wal::EntityRef, Option<crate::core::types::Timestamp>) -> bool
                    + Send
                    + Sync,
            >,
        >,
    ) -> StorageResult<crate::storage::index::vertex_index_manager::VertexIndexCursor> {
        self.open_tag_index_cursor_full(space_id, index, plan, stale_checker, None)
    }

    pub fn open_tag_index_cursor_full(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
        stale_checker: Option<
            Arc<
                dyn Fn(&crate::core::wal::EntityRef, Option<crate::core::types::Timestamp>) -> bool
                    + Send
                    + Sync,
            >,
        >,
        catalog: Option<&crate::storage::index::manifest::ManifestCatalog>,
    ) -> StorageResult<crate::storage::index::vertex_index_manager::VertexIndexCursor> {
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
        let _fence = runtime.read_fence();
        let handle = catalog.acquire();
        let generation = runtime
            .generation(handle.manifest().generation)
            .ok_or_else(|| StorageError::not_found("Index runtime generation is unavailable"))?;
        let temporary = VertexIndexManager::new();
        let mut forward = BTreeMap::new();
        for shard in generation.shards() {
            forward.extend(shard.forward().read().clone());
        }
        temporary.base().replace_data(forward, BTreeMap::new());
        let mut cursor = temporary.open_tag_index_cursor_full(
            space_id,
            index,
            plan,
            stale_checker,
            Some(catalog),
        )?;
        cursor.set_manifest_handle(handle);
        Ok(cursor)
    }

    pub fn open_edge_index_cursor(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
    ) -> StorageResult<crate::storage::index::edge_index_manager::EdgeIndexCursor> {
        self.register_native_index(space_id, index)?;
        self.open_edge_index_cursor_full(space_id, index, plan, None, None)
    }

    pub fn open_edge_index_cursor_with_checker(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
        stale_checker: Option<
            Arc<
                dyn Fn(&crate::core::wal::EntityRef, Option<crate::core::types::Timestamp>) -> bool
                    + Send
                    + Sync,
            >,
        >,
    ) -> StorageResult<crate::storage::index::edge_index_manager::EdgeIndexCursor> {
        self.open_edge_index_cursor_full(space_id, index, plan, stale_checker, None)
    }

    pub fn open_edge_index_cursor_full(
        &self,
        space_id: u64,
        index: &Index,
        plan: &IndexScanPlan,
        stale_checker: Option<
            Arc<
                dyn Fn(&crate::core::wal::EntityRef, Option<crate::core::types::Timestamp>) -> bool
                    + Send
                    + Sync,
            >,
        >,
        catalog: Option<&crate::storage::index::manifest::ManifestCatalog>,
    ) -> StorageResult<crate::storage::index::edge_index_manager::EdgeIndexCursor> {
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
        let _fence = runtime.read_fence();
        let handle = catalog.acquire();
        let generation = runtime
            .generation(handle.manifest().generation)
            .ok_or_else(|| StorageError::not_found("Index runtime generation is unavailable"))?;
        let temporary = EdgeIndexManager::new();
        let mut forward = BTreeMap::new();
        for shard in generation.shards() {
            forward.extend(shard.forward().read().clone());
        }
        temporary.base().replace_data(forward, BTreeMap::new());
        let mut cursor = temporary.open_edge_index_cursor_full(
            space_id,
            index,
            plan,
            stale_checker,
            Some(catalog),
        )?;
        cursor.set_manifest_handle(handle);
        Ok(cursor)
    }

    fn clear_vertex_entity(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_name: &str,
        write_ts: Timestamp,
    ) -> StorageResult<()> {
        let Some(index_id) = self.index_alias(space_id, index_name) else {
            return Ok(());
        };
        let (manifest, runtime, generation) = self.active_generation(space_id, index_id)?;
        let _fence = runtime.read_fence();
        let reverse = KeyBuilder::build_vertex_reverse_key_v2(space_id, vertex_id, index_name)?;
        let reverse_end = KeyBuilder::build_range_end(&reverse);
        for shard in manifest
            .manifest()
            .shards
            .iter()
            .filter_map(|shard| generation.shard(shard.shard_id))
        {
            let reverse_keys = shard
                .reverse()
                .read()
                .range(reverse.0.clone()..reverse_end.0.clone())
                .filter(|(_, entry)| entry.is_visible_at(write_ts))
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            for key in reverse_keys {
                if let Some(entry) = shard.reverse().write().get_mut(&key) {
                    entry.mark_deleted(write_ts);
                }
            }
            let forward_keys = shard
                .forward()
                .read()
                .iter()
                .filter(|(key, entry)| {
                    entry.is_visible_at(write_ts)
                        && KeyParser::parse_vertex_id_from_key(key)
                            .is_ok_and(|candidate| candidate == *vertex_id)
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            for key in forward_keys {
                if let Some(entry) = shard.forward().write().get_mut(&key) {
                    entry.mark_deleted(write_ts);
                }
            }
        }
        Ok(())
    }

    fn clear_edge_entity(
        &self,
        space_id: u64,
        src: &Value,
        dst: &Value,
        edge_type: &str,
        ranking: i64,
        index_name: &str,
        write_ts: Timestamp,
    ) -> StorageResult<()> {
        let Some(index_id) = self.index_alias(space_id, index_name) else {
            return Ok(());
        };
        let (manifest, runtime, generation) = self.active_generation(space_id, index_id)?;
        let _fence = runtime.read_fence();
        let reverse =
            KeyBuilder::build_edge_reverse_key(space_id, src, dst, edge_type, ranking, index_name)?;
        let reverse_end = KeyBuilder::build_range_end(&reverse);
        for shard in manifest
            .manifest()
            .shards
            .iter()
            .filter_map(|shard| generation.shard(shard.shard_id))
        {
            let reverse_keys = shard
                .reverse()
                .read()
                .range(reverse.0.clone()..reverse_end.0.clone())
                .filter(|(_, entry)| entry.is_visible_at(write_ts))
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            for key in reverse_keys {
                if let Some(entry) = shard.reverse().write().get_mut(&key) {
                    entry.mark_deleted(write_ts);
                }
            }
            let forward_keys = shard
                .forward()
                .read()
                .iter()
                .filter(|(key, entry)| {
                    entry.is_visible_at(write_ts)
                        && KeyParser::parse_edge_identity_from_key(key).is_ok_and(
                            |(candidate_src, candidate_dst, candidate_type, candidate_rank)| {
                                candidate_src == *src
                                    && candidate_dst == *dst
                                    && candidate_type == edge_type
                                    && candidate_rank == ranking
                            },
                        )
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            for key in forward_keys {
                if let Some(entry) = shard.forward().write().get_mut(&key) {
                    entry.mark_deleted(write_ts);
                }
            }
        }
        Ok(())
    }

    fn clear_index(
        &self,
        index_id: u64,
        space_id: u64,
        index_name: &str,
        index_type: IndexType,
        write_ts: Timestamp,
    ) -> StorageResult<()> {
        let (manifest, runtime, generation) = self.active_generation(space_id, index_id)?;
        let _fence = runtime.read_fence();
        let prefix = match index_type {
            IndexType::TagIndex => KeyBuilder::build_vertex_index_prefix(space_id, index_name),
            IndexType::EdgeIndex => KeyBuilder::build_edge_index_prefix(space_id, index_name),
        };
        let end = KeyBuilder::build_range_end(&prefix);
        for shard in manifest
            .manifest()
            .shards
            .iter()
            .filter_map(|shard| generation.shard(shard.shard_id))
        {
            let forward_keys = shard
                .forward()
                .read()
                .range(prefix.0.clone()..end.0.clone())
                .filter(|(_, entry)| entry.is_visible_at(write_ts))
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            for key in forward_keys {
                if let Some(entry) = shard.forward().write().get_mut(&key) {
                    entry.mark_deleted(write_ts);
                }
            }
            let reverse_keys = shard
                .reverse()
                .read()
                .iter()
                .filter(|(key, entry)| {
                    entry.is_visible_at(write_ts)
                        && match index_type {
                            IndexType::TagIndex => KeyParser::parse_vertex_reverse_key_v2(key)
                                .is_ok_and(|(_, name)| name == index_name),
                            IndexType::EdgeIndex => KeyParser::parse_edge_reverse_key(key)
                                .is_ok_and(|(_, _, _, _, name)| name == index_name),
                        }
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            for key in reverse_keys {
                if let Some(entry) = shard.reverse().write().get_mut(&key) {
                    entry.mark_deleted(write_ts);
                }
            }
        }
        Ok(())
    }

    fn gc_runtime(&self, safe_ts: Timestamp, batch_size: usize) -> StorageResult<GcStats> {
        let mut remaining = batch_size;
        let mut stats = GcStats::default();
        for (index_id, runtime) in self.runtimes.read().iter() {
            let index_type = self.index_types.read().get(index_id).cloned();
            for generation in runtime.generations() {
                for shard in generation.shards() {
                    let mut remove =
                        |map: &parking_lot::RwLock<BTreeMap<SecondaryIndexKey, IndexRecord>>| {
                            if remaining == 0 {
                                return 0;
                            }
                            let keys = map
                                .read()
                                .iter()
                                .filter(|(_, entry)| {
                                    entry.deleted_ts.is_some_and(|deleted| deleted < safe_ts)
                                })
                                .take(remaining)
                                .map(|(key, _)| key.clone())
                                .collect::<Vec<_>>();
                            let count = keys.len();
                            let mut data = map.write();
                            for key in keys {
                                data.remove(&key);
                            }
                            remaining = remaining.saturating_sub(count);
                            count
                        };
                    let removed = remove(shard.forward()) + remove(shard.reverse());
                    match index_type {
                        Some(IndexType::TagIndex) => stats.vertex_entries_removed += removed,
                        Some(IndexType::EdgeIndex) => stats.edge_entries_removed += removed,
                        None => {}
                    }
                    if remaining == 0 {
                        return Ok(stats);
                    }
                }
            }
        }
        Ok(stats)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GcStats {
    pub vertex_entries_removed: usize,
    pub edge_entries_removed: usize,
}

impl GcStats {
    pub fn total_removed(&self) -> usize {
        self.vertex_entries_removed + self.edge_entries_removed
    }

    pub fn is_empty(&self) -> bool {
        self.vertex_entries_removed == 0 && self.edge_entries_removed == 0
    }
}

impl Default for IndexDataManagerImpl {
    fn default() -> Self {
        Self::new()
    }
}

fn index_entities(entries: &BTreeMap<SecondaryIndexKey, IndexRecord>) -> Vec<EntityRef> {
    entries
        .values()
        .filter_map(|entry| entry.entity_ref.clone())
        .collect()
}

fn vertex_entity_ref(value: &Value) -> Option<EntityRef> {
    match value {
        Value::BigInt(id) => Some(EntityRef::Vertex(
            crate::core::types::storage_ids::VertexId::from_int64(*id),
        )),
        Value::Int(id) => Some(EntityRef::Vertex(
            crate::core::types::storage_ids::VertexId::from_int64(*id as i64),
        )),
        Value::String(id) => Some(EntityRef::Vertex(id.parse::<i64>().map_or_else(
            |_| crate::core::types::storage_ids::VertexId::from_string(id.clone()),
            crate::core::types::storage_ids::VertexId::from_int64,
        ))),
        Value::Vertex(vertex) => Some(EntityRef::Vertex(vertex.vid)),
        _ => None,
    }
}

fn edge_entity_ref(src: &Value, dst: &Value, edge_type: &str, ranking: i64) -> Option<EntityRef> {
    let EntityRef::Vertex(src) = vertex_entity_ref(src)? else {
        return None;
    };
    let EntityRef::Vertex(dst) = vertex_entity_ref(dst)? else {
        return None;
    };
    Some(EntityRef::Edge {
        src,
        dst,
        edge_type: edge_type.parse::<u32>().unwrap_or_default(),
        ranking,
    })
}

impl EdgeIndexOps for IndexDataManagerImpl {
    fn update_edge_indexes_mvcc(
        &self,
        space_id: u64,
        edge_src: &Value,
        edge_dst: &Value,
        edge_type: &str,
        ranking: i64,
        index_name: &str,
        props: &[(String, Value)],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        let Some(index_id) = self.index_alias(space_id, index_name) else {
            return Ok(());
        };
        let runtime = self.runtime(space_id, index_id)?;
        let _fence = runtime.read_fence();
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found("Index manifest catalog is unavailable"))?;
        let manifest = catalog.acquire();
        let generation = runtime
            .generation(manifest.manifest().generation)
            .ok_or_else(|| {
                StorageError::not_found("Active index runtime generation is unavailable")
            })?;
        for (_, value) in props {
            let forward = KeyBuilder::build_edge_index_key(
                space_id, index_name, value, edge_src, edge_dst, edge_type, ranking,
            )?;
            let reverse = KeyBuilder::build_edge_reverse_key(
                space_id, edge_src, edge_dst, edge_type, ranking, index_name,
            )?;
            let forward_end = KeyBuilder::build_range_end(&forward);
            let reverse_end = KeyBuilder::build_range_end(&reverse);
            for shard in generation.shards() {
                let keys = shard
                    .forward()
                    .read()
                    .range(forward.0.clone()..forward_end.0.clone())
                    .filter(|(_, entry)| entry.is_visible_at(write_ts))
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                let mut data = shard.forward().write();
                for key in keys {
                    if let Some(entry) = data.get_mut(&key) {
                        entry.mark_deleted(write_ts);
                    }
                }
                let keys = shard
                    .reverse()
                    .read()
                    .range(reverse.0.clone()..reverse_end.0.clone())
                    .filter(|(_, entry)| entry.is_visible_at(write_ts))
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                let mut data = shard.reverse().write();
                for key in keys {
                    if let Some(entry) = data.get_mut(&key) {
                        entry.mark_deleted(write_ts);
                    }
                }
            }
            let target = manifest
                .manifest()
                .route_key(&forward.0)
                .and_then(|shard| generation.shard(shard.shard_id))
                .ok_or_else(|| {
                    StorageError::invalid_operation("Index manifest does not cover the ordered key")
                })?;
            let entity_ref = edge_entity_ref(edge_src, edge_dst, edge_type, ranking);
            let mut entry = IndexRecord::new(write_ts).with_entity_version(write_ts);
            if let Some(entity) = entity_ref {
                entry = entry.with_entity_ref(entity);
            }
            target
                .forward()
                .write()
                .insert(target.physical_key(&forward.0), entry.clone());
            target
                .reverse()
                .write()
                .insert(target.physical_key(&reverse.0), entry);
        }
        Ok(())
    }

    fn delete_edge_indexes_mvcc(
        &self,
        space_id: u64,
        edge_src: &Value,
        edge_dst: &Value,
        edge_type: &str,
        ranking: i64,
        index_names: &[String],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        for index_name in index_names {
            self.clear_edge_entity(
                space_id, edge_src, edge_dst, edge_type, ranking, index_name, write_ts,
            )?;
        }
        Ok(())
    }

    fn lookup_edge_index_mvcc(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
        read_ts: Timestamp,
    ) -> Result<Vec<(Value, Value, String, i64)>, StorageError> {
        let Some(index_id) = self.index_alias(space_id, &index.name) else {
            return Ok(Vec::new());
        };
        let (manifest, runtime, generation) = self.active_generation(space_id, index_id)?;
        let _fence = runtime.read_fence();
        let prefix = KeyBuilder::build_edge_index_prefix(space_id, &index.name);
        let end = KeyBuilder::build_range_end(&prefix);
        let mut seen = std::collections::HashSet::new();
        let mut results = Vec::new();
        for shard in manifest
            .manifest()
            .shards
            .iter()
            .filter_map(|shard| generation.shard(shard.shard_id))
        {
            for (key, entry) in shard
                .forward()
                .read()
                .range(prefix.0.clone()..end.0.clone())
            {
                if !entry.is_visible_at(read_ts) {
                    continue;
                }
                if !KeyParser::parse_prop_value_from_edge_key(key)
                    .is_ok_and(|stored| stored == *value)
                {
                    continue;
                }
                if let Ok((src, dst, edge_type, ranking)) =
                    KeyParser::parse_edge_identity_from_key(key)
                {
                    if seen.insert((src.clone(), dst.clone(), edge_type.clone(), ranking)) {
                        results.push((src, dst, edge_type, ranking));
                    }
                }
            }
        }
        Ok(results)
    }

    fn clear_edge_index(&self, space_id: u64, index_name: &str) -> Result<(), StorageError> {
        let Some(index_id) = self.index_alias(space_id, index_name) else {
            return Ok(());
        };
        self.clear_index(
            index_id,
            space_id,
            index_name,
            IndexType::EdgeIndex,
            MAX_TIMESTAMP,
        )
    }
}

impl VertexIndexOps for IndexDataManagerImpl {
    fn update_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_name: &str,
        props: &[(String, Value)],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        let Some(index_id) = self.index_alias(space_id, index_name) else {
            return Ok(());
        };
        let runtime = self.runtime(space_id, index_id)?;
        let _fence = runtime.read_fence();
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found("Index manifest catalog is unavailable"))?;
        let manifest = catalog.acquire();
        let generation = runtime
            .generation(manifest.manifest().generation)
            .ok_or_else(|| {
                StorageError::not_found("Active index runtime generation is unavailable")
            })?;
        for (_, value) in props {
            let forward =
                KeyBuilder::build_vertex_index_key(space_id, index_name, value, vertex_id)?;
            let reverse = KeyBuilder::build_vertex_reverse_key_v2(space_id, vertex_id, index_name)?;
            let forward_end = KeyBuilder::build_range_end(&forward);
            let reverse_end = KeyBuilder::build_range_end(&reverse);
            for shard in generation.shards() {
                let keys = shard
                    .forward()
                    .read()
                    .range(forward.0.clone()..forward_end.0.clone())
                    .filter(|(_, entry)| entry.is_visible_at(write_ts))
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                let mut data = shard.forward().write();
                for key in keys {
                    if let Some(entry) = data.get_mut(&key) {
                        entry.mark_deleted(write_ts);
                    }
                }
                let keys = shard
                    .reverse()
                    .read()
                    .range(reverse.0.clone()..reverse_end.0.clone())
                    .filter(|(_, entry)| entry.is_visible_at(write_ts))
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                let mut data = shard.reverse().write();
                for key in keys {
                    if let Some(entry) = data.get_mut(&key) {
                        entry.mark_deleted(write_ts);
                    }
                }
            }
            let target = manifest
                .manifest()
                .route_key(&forward.0)
                .and_then(|shard| generation.shard(shard.shard_id))
                .ok_or_else(|| {
                    StorageError::invalid_operation("Index manifest does not cover the ordered key")
                })?;
            let mut entry = IndexRecord::new(write_ts).with_entity_version(write_ts);
            if let Some(entity) = vertex_entity_ref(vertex_id) {
                entry = entry.with_entity_ref(entity);
            }
            target
                .forward()
                .write()
                .insert(target.physical_key(&forward.0), entry.clone());
            target
                .reverse()
                .write()
                .insert(target.physical_key(&reverse.0), entry);
        }
        Ok(())
    }

    fn delete_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_names: &[String],
        write_ts: Timestamp,
    ) -> Result<(), StorageError> {
        for index_name in index_names {
            self.clear_vertex_entity(space_id, vertex_id, index_name, write_ts)?;
        }
        Ok(())
    }

    fn lookup_tag_index_mvcc(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
        read_ts: Timestamp,
    ) -> Result<Vec<Value>, StorageError> {
        let Some(index_id) = self.index_alias(space_id, &index.name) else {
            return Ok(Vec::new());
        };
        let (manifest, runtime, generation) = self.active_generation(space_id, index_id)?;
        let _fence = runtime.read_fence();
        let prefix = KeyBuilder::build_vertex_index_prefix(space_id, &index.name);
        let end = KeyBuilder::build_range_end(&prefix);
        let mut seen = std::collections::HashSet::new();
        let mut results = Vec::new();
        for shard in manifest
            .manifest()
            .shards
            .iter()
            .filter_map(|shard| generation.shard(shard.shard_id))
        {
            for (key, entry) in shard
                .forward()
                .read()
                .range(prefix.0.clone()..end.0.clone())
            {
                if !entry.is_visible_at(read_ts) {
                    continue;
                }
                if !KeyParser::parse_prop_value_from_key(key).is_ok_and(|stored| stored == *value) {
                    continue;
                }
                if let Ok(vertex_id) = KeyParser::parse_vertex_id_from_key(key) {
                    if seen.insert(vertex_id.clone()) {
                        results.push(vertex_id);
                    }
                }
            }
        }
        Ok(results)
    }

    fn clear_tag_index(&self, space_id: u64, index_name: &str) -> Result<(), StorageError> {
        let Some(index_id) = self.index_alias(space_id, index_name) else {
            return Ok(());
        };
        self.clear_index(
            index_id,
            space_id,
            index_name,
            IndexType::TagIndex,
            MAX_TIMESTAMP,
        )
    }
}

impl IndexGcOps for IndexDataManagerImpl {
    fn gc_tombstones(&self, safe_ts: Timestamp) -> Result<GcStats, StorageError> {
        self.gc_runtime(safe_ts, usize::MAX)
    }

    fn gc_tombstones_incremental(
        &self,
        safe_ts: Timestamp,
        batch_size: usize,
    ) -> Result<GcStats, StorageError> {
        self.gc_runtime(safe_ts, batch_size)
    }

    fn tombstone_count(&self) -> usize {
        let mut count = 0;
        for runtime in self.runtimes.read().values() {
            for generation in runtime.generations() {
                for shard in generation.shards() {
                    count += shard
                        .forward()
                        .read()
                        .values()
                        .filter(|entry| entry.deleted_ts.is_some())
                        .count();
                    count += shard
                        .reverse()
                        .read()
                        .values()
                        .filter(|entry| entry.deleted_ts.is_some())
                        .count();
                }
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use crate::core::types::{
        CommitLsn, Index, IndexConfig, IndexField, IndexGeneration, IndexType, ManifestEpoch,
        MAX_TIMESTAMP,
    };
    use crate::core::Value;
    use crate::storage::index::generic_index_manager::GenericIndexManager;
    use crate::storage::index::key_codec::{KeyBuilder, VertexIndexKeyGen};
    use crate::storage::index::manifest::{IndexManifest, IndexShard};
    use crate::storage::index::*;

    fn create_test_index(name: &str, schema_name: &str) -> Index {
        Index::new(IndexConfig {
            id: 1,
            name: name.to_string(),
            space_id: 1,
            schema_name: schema_name.to_string(),
            fields: vec![IndexField::new(
                "name".to_string(),
                Value::String("".to_string()),
                false,
            )],
            properties: vec![],
            index_type: IndexType::TagIndex,
            is_unique: false,
            partial_condition: None,
        })
    }

    #[test]
    fn test_serialize_deserialize_value() {
        let value = Value::String("test".to_string());
        let bytes = crate::storage::index::key_codec::key_types::serialize_value(&value)
            .expect("serialize should succeed");
        let decoded = crate::storage::index::key_codec::key_types::deserialize_value(&bytes)
            .expect("deserialize should succeed");
        assert_eq!(value, decoded);
    }

    #[test]
    fn test_update_and_lookup_vertex_index() {
        let manager = IndexDataManagerImpl::new();

        let space_id = 1u64;
        let vertex_id = Value::Int(1);
        let index_name = "idx_person_name";
        let props = vec![("name".to_string(), Value::String("Alice".to_string()))];
        let index = create_test_index(index_name, "person");

        manager
            .register_native_index(space_id, &index)
            .expect("register native index");
        manager
            .update_vertex_indexes_mvcc(space_id, &vertex_id, index_name, &props, MAX_TIMESTAMP)
            .expect("Failed to update vertex indexes");
        let results = manager
            .lookup_tag_index(space_id, &index, &Value::String("Alice".to_string()))
            .expect("Failed to lookup tag index");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], vertex_id);
    }

    #[test]
    fn same_index_id_in_different_spaces_uses_isolated_runtimes() {
        let manager = IndexDataManagerImpl::new();
        let first = create_test_index("person_name", "person");
        let mut second = create_test_index("person_name", "person");
        second.space_id = 2;

        manager
            .register_native_index(1, &first)
            .expect("register first space index");
        manager
            .register_native_index(2, &second)
            .expect("register second space index");
        manager
            .update_vertex_indexes_mvcc(
                1,
                &Value::Int(1),
                "person_name",
                &[("name".to_string(), Value::String("Alice".to_string()))],
                MAX_TIMESTAMP,
            )
            .expect("write first space index");
        manager
            .update_vertex_indexes_mvcc(
                2,
                &Value::Int(2),
                "person_name",
                &[("name".to_string(), Value::String("Bob".to_string()))],
                MAX_TIMESTAMP,
            )
            .expect("write second space index");

        assert_eq!(
            manager
                .lookup_tag_index(1, &first, &Value::String("Alice".to_string()))
                .expect("lookup first space"),
            vec![Value::Int(1)]
        );
        assert_eq!(
            manager
                .lookup_tag_index(2, &second, &Value::String("Bob".to_string()))
                .expect("lookup second space"),
            vec![Value::Int(2)]
        );
        assert!(manager.manifest_catalog(1, 1).is_some());
        assert!(manager.manifest_catalog(2, 1).is_some());
    }

    #[test]
    fn split_writes_only_the_selected_index_to_each_shard() {
        let directory = tempfile::tempdir().expect("create temporary index directory");
        let manager = IndexDataManagerImpl::new();
        let first_index = create_test_index("first", "person");
        let mut second_index = create_test_index("second", "person");
        second_index.id = 2;
        manager
            .register_native_index(1, &first_index)
            .expect("register first index");
        manager
            .register_native_index(1, &second_index)
            .expect("register second index");

        let catalog = manager
            .manifest_catalog(1, 1)
            .expect("first index manifest");
        catalog
            .publish(
                IndexManifest::new(
                    1,
                    1,
                    IndexGeneration::new(1),
                    ManifestEpoch::new(2),
                    vec![IndexShard {
                        shard_id: 0,
                        lower: None,
                        upper: None,
                        checkpoint_file: directory.path().join("first/generation-1"),
                    }],
                )
                .expect("valid manifest"),
            )
            .expect("publish test manifest");

        manager
            .update_vertex_indexes_mvcc(
                1,
                &Value::Int(1),
                "first",
                &[("name".to_string(), Value::String("Alice".to_string()))],
                1,
            )
            .expect("write first lower key");
        manager
            .update_vertex_indexes_mvcc(
                1,
                &Value::Int(2),
                "first",
                &[("name".to_string(), Value::String("Zoe".to_string()))],
                1,
            )
            .expect("write first upper key");
        manager
            .update_vertex_indexes_mvcc(
                1,
                &Value::Int(3),
                "second",
                &[("name".to_string(), Value::String("Other".to_string()))],
                1,
            )
            .expect("write unrelated index key");

        let boundary = KeyBuilder::build_vertex_index_value_prefix(
            1,
            "first",
            &Value::String("M".to_string()),
        )
        .expect("build split boundary")
        .0;
        manager
            .split_native_index(1, 1, boundary, CommitLsn::new(100))
            .expect("split first index");

        let manifest = catalog.acquire();
        assert_eq!(manifest.manifest().shards.len(), 2);
        let first_prefix = KeyBuilder::build_vertex_index_prefix(1, "first").0;
        let second_prefix = KeyBuilder::build_vertex_index_prefix(1, "second").0;
        let mut shard_entries = 0;
        for shard in &manifest.manifest().shards {
            let mut shard_manager = GenericIndexManager::<VertexIndexKeyGen>::new();
            shard_manager
                .load(&shard.checkpoint_file)
                .expect("load split shard");
            let (forward, _) = shard_manager.snapshot_data();
            shard_entries += forward.len();
            assert!(forward.keys().all(|key| key.starts_with(&first_prefix)));
            assert!(forward.keys().all(|key| !key.starts_with(&second_prefix)));
        }
        assert_eq!(shard_entries, 2);
    }
}
