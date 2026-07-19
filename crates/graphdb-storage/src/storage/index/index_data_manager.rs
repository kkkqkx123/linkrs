//! Index Data Manager
//!
//! Provide update, delete and query functions for indexed data
//! The management of index metadata is handled by the IndexMetadataManager.
//! All operations identify a space by its space_id, enabling multi-space data segregation.
//! Supports persistence through flush/load operations.
//! Supports MVCC (Multi-Version Concurrency Control) for snapshot isolation.

use crate::core::stats::StatsManager;
use crate::core::types::{
    CommitLsn, Index, IndexGeneration, IndexType, SnapshotTimestamp, Timestamp, MAX_TIMESTAMP,
};
use crate::core::wal::{EntityRef, OutboxIntent};
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::cursor::IndexScanPlan;
use crate::storage::index::edge_index_manager::EdgeIndexManager;
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::key_codec::{KeyBuilder, KeyParser};
use crate::storage::index::manifest::{
    GenerationBuildState, GenerationState, IndexManifest, IndexShard, ManifestCatalog,
};
use crate::storage::index::shard_runtime::{
    generation_from_maps, GenerationRuntime, IndexBarrierRegistry, IndexMaps, IndexRuntime,
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
    index_root: Option<PathBuf>,
    manifest_catalogs: Arc<RwLock<HashMap<IndexIdentity, Arc<ManifestCatalog>>>>,
    runtimes: Arc<RwLock<HashMap<IndexIdentity, Arc<IndexRuntime>>>>,
    index_aliases: Arc<RwLock<HashMap<(u64, String), u64>>>,
    index_types: Arc<RwLock<HashMap<IndexIdentity, IndexType>>>,
    index_definitions: Arc<RwLock<HashMap<IndexIdentity, Index>>>,
    restored_generations: Arc<RwLock<HashMap<IndexIdentity, IndexGeneration>>>,
    barrier_registry: IndexBarrierRegistry,
    rebuild_gate: Arc<RwLock<()>>,
    stats_manager: Option<Arc<StatsManager>>,
}

impl IndexDataManagerImpl {
    pub fn new() -> Self {
        Self::new_with_optional_root(None)
    }

    /// Create an index manager whose checkpoint files are rooted at `index_root`.
    ///
    /// Persistent storage must provide this root so index files never resolve
    /// relative to the process working directory. The root is also useful for
    /// filesystem-backed tests, which can point it at a `tempfile::TempDir`.
    pub fn new_with_root(index_root: impl Into<PathBuf>) -> Self {
        Self::new_with_optional_root(Some(index_root.into()))
    }

    fn new_with_optional_root(index_root: Option<PathBuf>) -> Self {
        Self {
            index_root,
            manifest_catalogs: Arc::new(RwLock::new(HashMap::new())),
            runtimes: Arc::new(RwLock::new(HashMap::new())),
            index_aliases: Arc::new(RwLock::new(HashMap::new())),
            index_types: Arc::new(RwLock::new(HashMap::new())),
            index_definitions: Arc::new(RwLock::new(HashMap::new())),
            restored_generations: Arc::new(RwLock::new(HashMap::new())),
            barrier_registry: Arc::new(RwLock::new(HashMap::new())),
            rebuild_gate: Arc::new(RwLock::new(())),
            stats_manager: None,
        }
    }

    pub fn set_stats_manager(&mut self, stats_manager: Arc<StatsManager>) {
        self.stats_manager = Some(stats_manager);
    }

    fn record_manifest_state(&self, catalog: &ManifestCatalog) {
        if let Some(stats) = &self.stats_manager {
            let state = catalog.stats();
            stats.set_manifest_state(state.active_readers, state.retired_generations);
        }
    }

    fn initial_checkpoint_path(&self, space_id: u64, index_id: u64) -> PathBuf {
        let relative = PathBuf::from(format!("{space_id}/{index_id}/generation-1/shard-0"));
        match self.index_root.as_ref() {
            Some(root) => root.join(relative),
            None => PathBuf::from("memory-index").join(relative),
        }
    }

    pub fn memory_usage_bytes(&self) -> u64 {
        self.runtimes
            .read()
            .values()
            .map(|runtime| runtime.memory_usage_bytes())
            .sum()
    }

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
        if !catalogs.contains_key(&identity) {
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
            self.index_definitions.write().remove(&identity);
            self.restored_generations.write().remove(&identity);
            self.barrier_registry
                .write()
                .remove(&(identity.space_id, identity.index_id));
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

    pub(crate) fn barrier_registry(&self) -> IndexBarrierRegistry {
        Arc::clone(&self.barrier_registry)
    }

    /// Return the gate shared by generation rebuilds and index writers.
    ///
    /// A rebuild holds its write side from snapshot acquisition through
    /// publication. Every index mutation holds the read side before it
    /// resolves the active generation, so a mutation cannot be stranded in a
    /// retired generation.
    pub(crate) fn rebuild_gate(&self) -> Arc<RwLock<()>> {
        Arc::clone(&self.rebuild_gate)
    }

    fn record_barrier_lsn(&self, identity: IndexIdentity, barrier_lsn: CommitLsn) {
        if barrier_lsn == CommitLsn::ZERO {
            return;
        }
        let mut barriers = self.barrier_registry.write();
        let entry = barriers
            .entry((identity.space_id, identity.index_id))
            .or_default();
        if barrier_lsn > *entry {
            *entry = barrier_lsn;
        }
    }

    /// Advance all published index barriers after a committed transaction.
    /// Index mutations are applied before the transaction commit is appended,
    /// so the commit LSN is a safe durable watermark for every affected index.
    pub(crate) fn advance_barriers(&self, commit_lsn: CommitLsn) {
        if commit_lsn == CommitLsn::ZERO {
            return;
        }
        for (identity, runtime) in self.runtimes.read().iter() {
            runtime.establish_barrier_lsn(commit_lsn);
            self.record_barrier_lsn(*identity, commit_lsn);
        }
    }

    fn wait_for_active_barrier(&self, runtime: &IndexRuntime) {
        let barrier_lsn = runtime.barrier_lsn();
        runtime.wait_for_barrier_lsn(barrier_lsn);
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
        let runtime = self.runtime(space_id, index_id)?;
        let _fence = runtime.read_fence();
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

    pub(crate) fn publish_native_index(
        &self,
        manifest: IndexManifest,
        forward: BTreeMap<SecondaryIndexKey, IndexRecord>,
        reverse: BTreeMap<SecondaryIndexKey, IndexRecord>,
        barrier_lsn: CommitLsn,
    ) -> StorageResult<()> {
        let runtime = self.runtime(manifest.space_id, manifest.index_id)?;
        let _fence = runtime.write_fence();
        let identity = IndexIdentity {
            space_id: manifest.space_id,
            index_id: manifest.index_id,
        };
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
        if let Some(stats) = &self.stats_manager {
            stats.record_generation_publish();
        }
        runtime.establish_barrier_lsn(barrier_lsn);
        self.record_barrier_lsn(identity, barrier_lsn);
        runtime.wait_for_barrier_lsn(barrier_lsn);
        if let Some(catalog) = self.manifest_catalog(identity.space_id, identity.index_id) {
            self.record_manifest_state(&catalog);
        }
        Ok(())
    }

    fn build_state_path(&self, index_root: &Path) -> PathBuf {
        index_root.join("generation_build.bin")
    }

    fn save_build_state(
        &self,
        index_root: &Path,
        state: &GenerationBuildState,
    ) -> StorageResult<()> {
        std::fs::create_dir_all(index_root)?;
        let path = self.build_state_path(index_root);
        let temporary = path.with_extension("tmp");
        let serialized = postcard::to_allocvec(state)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;
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
        let state: GenerationBuildState = postcard::from_bytes(&bytes)
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
    /// Incomplete builds are discarded. A Publishing build is accepted only
    /// when its generation is the generation recorded by the manifest.
    pub fn resolve_split_crash_recovery(&self, index_root: &Path) -> StorageResult<()> {
        let Some(build_state) = self.load_build_state(index_root)? else {
            return Ok(());
        };
        let partial_generation =
            index_root.join(format!("generation-{}", build_state.generation.get()));
        if matches!(
            build_state.state,
            GenerationState::Building
                | GenerationState::CatchingUp
                | GenerationState::Failed
                | GenerationState::Cancelled
        ) {
            log::warn!(
                "Discarding incomplete split build state for gen {} (state={:?})",
                build_state.generation,
                build_state.state
            );
            if partial_generation.exists() {
                std::fs::remove_dir_all(&partial_generation)?;
            }
            self.remove_build_state(index_root)?;
        }
        if matches!(build_state.state, GenerationState::Publishing) {
            let manifest_path = index_root.join("manifest.bin");
            let published_generation = manifest_path
                .is_file()
                .then(|| IndexManifest::load(&manifest_path))
                .transpose()?
                .map(|manifest| manifest.generation);
            if published_generation == Some(build_state.generation) {
                log::info!("Completing split build from Publishing state");
                self.remove_build_state(index_root)?;
            } else {
                log::warn!("Discarding split in Publishing state without its published generation");
                if partial_generation.exists() {
                    std::fs::remove_dir_all(&partial_generation)?;
                }
                self.remove_build_state(index_root)?;
            }
        }
        if matches!(build_state.state, GenerationState::Active) {
            self.remove_build_state(index_root)?;
        }
        Ok(())
    }

    pub fn split_native_index<F, G>(
        &self,
        space_id: u64,
        index_id: u64,
        boundary: Vec<u8>,
        snapshot_timestamp: SnapshotTimestamp,
        start_lsn: CommitLsn,
        barrier_lsn: F,
        wal_intents: G,
    ) -> StorageResult<()>
    where
        F: FnOnce() -> StorageResult<CommitLsn>,
        G: FnOnce(CommitLsn, CommitLsn) -> StorageResult<Vec<OutboxIntent>>,
    {
        if let Some(stats) = &self.stats_manager {
            stats.record_generation_build();
        }
        if self.index_root.is_none() {
            return Err(StorageError::invalid_operation(
                "Native index splitting requires a persistent index root",
            ));
        }
        if snapshot_timestamp.get() == 0 || start_lsn == CommitLsn::ZERO {
            return Err(StorageError::invalid_operation(
                "Native index splitting requires non-zero snapshot timestamp and start LSN",
            ));
        }
        if boundary.is_empty() {
            return Err(StorageError::invalid_operation(
                "Index split boundary cannot be empty",
            ));
        }
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found(format!("Index {index_id} has no manifest")))?;
        let runtime = self.runtime(space_id, index_id)?;
        let current = catalog.acquire().manifest().clone();
        let split_position = current
            .shards
            .iter()
            .position(|shard| shard.contains(&boundary))
            .ok_or_else(|| {
                StorageError::invalid_operation("Split boundary is outside key space")
            })?;
        let shard = current.shards[split_position].clone();
        if shard.lower.as_ref() == Some(&boundary) || shard.upper.as_ref() == Some(&boundary) {
            return Err(StorageError::invalid_operation(
                "Split boundary must be inside an existing shard",
            ));
        }

        let generation = IndexGeneration::new(current.generation.get().saturating_add(1));
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
        let mut build_state = GenerationBuildState::new(generation, snapshot_timestamp, start_lsn);
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
        let index_definition = self
            .index_definitions
            .read()
            .get(&IndexIdentity { space_id, index_id })
            .cloned()
            .ok_or_else(|| {
                StorageError::not_found(format!("Index {index_id} has no definition"))
            })?;

        let mut shards = current.shards.clone();
        shards.splice(
            split_position..=split_position,
            [
                IndexShard {
                    shard_id: shard.shard_id,
                    lower: shard.lower.clone(),
                    upper: Some(boundary.clone()),
                    checkpoint_file: shard_a_path,
                    checksum: None,
                },
                IndexShard {
                    shard_id: next_shard_id,
                    lower: Some(boundary.clone()),
                    upper: shard.upper.clone(),
                    checkpoint_file: shard_b_path,
                    checksum: None,
                },
            ],
        );
        let next = IndexManifest::new(space_id, index_id, generation, shards)
            .map_err(StorageError::db_error)?;
        let mut maps = {
            let _fence = runtime.read_fence();
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
                let mut entity_shards: Vec<(EntityRef, (Timestamp, u32))> = Vec::new();
                for entry in forward_a.values() {
                    if let Some(entity) = entry.entity_ref.clone() {
                        if let Some((_, (ts, shard_id))) = entity_shards
                            .iter_mut()
                            .find(|(candidate, _)| *candidate == entity)
                        {
                            if entry.created_ts >= *ts {
                                *ts = entry.created_ts;
                                *shard_id = shard.shard_id;
                            }
                        } else {
                            entity_shards.push((entity, (entry.created_ts, shard.shard_id)));
                        }
                    }
                }
                for entry in forward_b.values() {
                    if let Some(entity) = entry.entity_ref.clone() {
                        if let Some((_, (ts, shard_id))) = entity_shards
                            .iter_mut()
                            .find(|(candidate, _)| *candidate == entity)
                        {
                            if entry.created_ts >= *ts {
                                *ts = entry.created_ts;
                                *shard_id = next_shard_id;
                            }
                        } else {
                            entity_shards.push((entity, (entry.created_ts, next_shard_id)));
                        }
                    }
                }
                let mut reverse_a = BTreeMap::new();
                let mut reverse_b = BTreeMap::new();
                for (key, entry) in reverse {
                    let target_shard = entry
                        .entity_ref
                        .as_ref()
                        .and_then(|entity| {
                            entity_shards
                                .iter()
                                .find(|(candidate, _)| candidate == entity)
                        })
                        .map(|(_, (_, shard_id))| *shard_id);
                    if target_shard == Some(next_shard_id) || target_shard.is_none() {
                        reverse_b.insert(key, entry);
                    } else {
                        reverse_a.insert(key, entry);
                    }
                }
                maps.insert(shard.shard_id, (forward_a, reverse_a));
                maps.insert(next_shard_id, (forward_b, reverse_b));
            }
            maps
        };

        // ── Phase: CatchingUp ────────────────────────────────────────────
        build_state
            .transition_to_catching_up()
            .map_err(StorageError::invalid_operation)?;
        self.save_build_state(index_root, &build_state)?;

        // The production entry point holds the rebuild gate from snapshot
        // acquisition through publication. The publish fence still pins the
        // active generation while its maps and barrier are materialized.
        let _fence = runtime.write_fence();
        let active_manifest = catalog.acquire().manifest().clone();
        if active_manifest.generation != current.generation {
            return Err(StorageError::invalid_operation(
                "Index generation changed while splitting; retry the split",
            ));
        }
        let barrier_lsn = barrier_lsn()?;
        if barrier_lsn < start_lsn {
            return Err(StorageError::invalid_operation(
                "Split barrier LSN cannot precede its start LSN",
            ));
        }
        let intents = wal_intents(start_lsn, barrier_lsn)?;
        let active = runtime
            .generation(current.generation)
            .ok_or_else(|| StorageError::not_found("Active runtime generation is missing"))?;
        let mut active_forward = BTreeMap::new();
        let mut active_reverse = BTreeMap::new();
        for current_shard in &current.shards {
            let (forward, reverse) = active
                .shard(current_shard.shard_id)
                .ok_or_else(|| StorageError::not_found("Active runtime shard is missing"))?
                .snapshot();
            active_forward.extend(forward);
            active_reverse.extend(reverse);
        }
        match index_type {
            IndexType::TagIndex => {
                let forward_prefix =
                    KeyBuilder::build_vertex_index_prefix(space_id, &index_definition.name).0;
                merge_split_wal_changes(
                    &mut maps,
                    &next,
                    &active_forward,
                    &active_reverse,
                    &intents,
                    |key| key.starts_with(&forward_prefix),
                    |key| {
                        KeyParser::parse_vertex_reverse_key_v2(key)
                            .is_ok_and(|(_, name)| name == index_definition.name)
                    },
                )?;
            }
            IndexType::EdgeIndex => {
                let forward_prefix =
                    KeyBuilder::build_edge_index_prefix(space_id, &index_definition.name).0;
                merge_split_wal_changes(
                    &mut maps,
                    &next,
                    &active_forward,
                    &active_reverse,
                    &intents,
                    |key| key.starts_with(&forward_prefix),
                    |key| {
                        KeyParser::parse_edge_reverse_key(key)
                            .is_ok_and(|(_, _, _, _, name)| name == index_definition.name)
                    },
                )?;
            }
        }

        // ── Phase: Publishing ────────────────────────────────────────────
        build_state
            .transition_to_publishing(barrier_lsn)
            .map_err(StorageError::invalid_operation)?;
        self.save_build_state(index_root, &build_state)?;

        let next_runtime = generation_from_maps(&next, maps);
        match index_type {
            IndexType::TagIndex => flush_split_generation::<
                crate::storage::index::key_codec::VertexIndexKeyGen,
            >(&next, &next_runtime)?,
            IndexType::EdgeIndex => flush_split_generation::<
                crate::storage::index::key_codec::EdgeIndexKeyGen,
            >(&next, &next_runtime)?,
        }

        next.store(&index_root.join("manifest.bin"))?;
        runtime.install_generation(next_runtime);
        catalog.publish(next).map_err(StorageError::db_error)?;
        runtime.establish_barrier_lsn(barrier_lsn);
        self.record_barrier_lsn(IndexIdentity { space_id, index_id }, barrier_lsn);
        runtime.wait_for_barrier_lsn(barrier_lsn);
        if let Some(stats) = &self.stats_manager {
            stats.record_generation_publish();
        }
        self.record_manifest_state(&catalog);

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
            let files_before = files.len();
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
            if let Some(stats) = &self.stats_manager {
                stats.record_reclaimed_index_files((files.len() - files_before) as u64);
            }
            self.record_manifest_state(catalog);
        }
        files
    }

    pub fn flush<P: AsRef<Path>>(&self, path: P) -> StorageResult<()> {
        if self.index_root.is_none() && !self.manifest_catalogs.read().is_empty() {
            return Err(StorageError::invalid_operation(
                "Flushing native indexes requires a persistent index root",
            ));
        }
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
                &manifest_dir.join(format!("{}-{}.bin", identity.space_id, identity.index_id)),
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
                if manifest_path.extension().and_then(|value| value.to_str()) != Some("bin") {
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
        for space_entry in std::fs::read_dir(path)? {
            let space_path = space_entry?.path();
            if !space_path.is_dir() {
                continue;
            }
            for index_entry in std::fs::read_dir(space_path)? {
                let index_path = index_entry?.path();
                self.resolve_split_crash_recovery(&index_path)?;
                let candidate = index_path.join("manifest.bin");
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
        let runtime = self.runtime(space_id, index_id)?;
        let _fence = runtime.read_fence();
        self.wait_for_active_barrier(&runtime);
        let (manifest, _runtime, generation) = self.active_generation(space_id, index_id)?;
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
        let runtime = self.runtime(space_id, index_id)?;
        let _fence = runtime.read_fence();
        self.wait_for_active_barrier(&runtime);
        let (manifest, _runtime, generation) = self.active_generation(space_id, index_id)?;
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
        let runtime = self.runtime(space_id, index_id)?;
        let _fence = runtime.read_fence();
        self.wait_for_active_barrier(&runtime);
        let (manifest, _runtime, generation) = self.active_generation(space_id, index_id)?;
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

fn merge_split_wal_changes<F, R>(
    maps: &mut HashMap<u32, IndexMaps>,
    manifest: &IndexManifest,
    active_forward: &BTreeMap<SecondaryIndexKey, IndexRecord>,
    active_reverse: &BTreeMap<SecondaryIndexKey, IndexRecord>,
    intents: &[OutboxIntent],
    matches_forward: F,
    matches_reverse: R,
) -> StorageResult<()>
where
    F: Fn(&[u8]) -> bool,
    R: Fn(&[u8]) -> bool,
{
    let mut changed_entities = Vec::new();
    for entity in intents
        .iter()
        .map(|intent| entity_ref_from_intent(intent))
        .flatten()
    {
        if !changed_entities.contains(&entity) {
            changed_entities.push(entity);
        }
    }
    if changed_entities.is_empty() {
        return Ok(());
    }

    for (forward, reverse) in maps.values_mut() {
        forward.retain(|key, record| {
            !(matches_forward(key)
                && record
                    .entity_ref
                    .as_ref()
                    .is_some_and(|entity| changed_entities.contains(entity)))
        });
        reverse.retain(|key, record| {
            !(matches_reverse(key)
                && record
                    .entity_ref
                    .as_ref()
                    .is_some_and(|entity| changed_entities.contains(entity)))
        });
    }

    let mut entity_shards: Vec<(EntityRef, (Timestamp, u32))> = Vec::new();
    for (key, record) in active_forward {
        if !matches_forward(key) {
            continue;
        }
        let Some(entity) = record.entity_ref.clone() else {
            continue;
        };
        let shard_id = manifest
            .route_key(key)
            .ok_or_else(|| {
                StorageError::invalid_operation(
                    "WAL catch-up produced an index key outside the split manifest",
                )
            })?
            .shard_id;
        if let Some((_, (ts, current_shard))) = entity_shards
            .iter_mut()
            .find(|(candidate, _)| *candidate == entity)
        {
            if record.created_ts >= *ts {
                *ts = record.created_ts;
                *current_shard = shard_id;
            }
        } else {
            entity_shards.push((entity, (record.created_ts, shard_id)));
        }
    }

    for (key, record) in active_forward {
        if !matches_forward(key) {
            continue;
        }
        let shard = manifest.route_key(key).ok_or_else(|| {
            StorageError::invalid_operation(
                "WAL catch-up produced an index key outside the split manifest",
            )
        })?;
        maps.entry(shard.shard_id)
            .or_insert_with(|| (BTreeMap::new(), BTreeMap::new()))
            .0
            .insert(key.clone(), record.clone());
    }

    for (key, record) in active_reverse {
        if !matches_reverse(key) {
            continue;
        }
        let entity = record.entity_ref.as_ref().ok_or_else(|| {
            StorageError::invalid_operation("WAL catch-up reverse record has no owning entity")
        })?;
        let shard_id = entity_shards
            .iter()
            .find(|(candidate, _)| candidate == entity)
            .map(|(_, (_, shard_id))| *shard_id)
            .ok_or_else(|| {
                StorageError::invalid_operation(
                    "WAL catch-up reverse record has no routable forward record",
                )
            })?;
        maps.entry(shard_id)
            .or_insert_with(|| (BTreeMap::new(), BTreeMap::new()))
            .1
            .insert(key.clone(), record.clone());
    }

    Ok(())
}

fn entity_ref_from_intent(intent: &OutboxIntent) -> Option<EntityRef> {
    Some(intent.mutation.entity_ref.clone())
}

fn flush_split_generation<K: crate::storage::index::key_codec::IndexKeyGenerator>(
    manifest: &IndexManifest,
    runtime: &GenerationRuntime,
) -> StorageResult<()> {
    for entry in &manifest.shards {
        let data = runtime
            .shard(entry.shard_id)
            .ok_or_else(|| StorageError::not_found("Split generation shard is unavailable"))?;
        crate::storage::index::generic_index_manager::GenericIndexManager::<K>::flush_data(
            &entry.checkpoint_file,
            &data.forward().read(),
            &data.reverse().read(),
        )?;
    }
    Ok(())
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
        edge_type: stable_hash(edge_type.as_bytes()) as u32,
        ranking,
    })
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn effective_index_values(
    index_definition: Option<&Index>,
    props: &[(String, Value)],
    existing_values: Vec<Value>,
) -> Vec<Value> {
    let Some(index) = index_definition else {
        return props.iter().map(|(_, value)| value.clone()).collect();
    };
    let values = index
        .fields
        .iter()
        .filter_map(|field| {
            props
                .iter()
                .find(|(name, _)| name == &field.name)
                .map(|(_, value)| value.clone())
        })
        .collect::<Vec<_>>();
    if values.is_empty() {
        existing_values
    } else {
        values
    }
}

fn merged_included_columns(
    index_definition: Option<&Index>,
    mut existing: Vec<(String, Value)>,
    props: &[(String, Value)],
) -> Vec<(String, Value)> {
    let Some(index) = index_definition else {
        return props.to_vec();
    };
    for name in &index.properties {
        let Some((_, value)) = props.iter().find(|(candidate, _)| candidate == name) else {
            continue;
        };
        if let Some((_, existing_value)) =
            existing.iter_mut().find(|(candidate, _)| candidate == name)
        {
            *existing_value = value.clone();
        } else {
            existing.push((name.clone(), value.clone()));
        }
    }
    existing
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
        self.wait_for_active_barrier(&runtime);
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found("Index manifest catalog is unavailable"))?;
        let manifest = catalog.acquire();
        let generation = runtime
            .generation(manifest.manifest().generation)
            .ok_or_else(|| {
                StorageError::not_found("Active index runtime generation is unavailable")
            })?;
        let index_definition = self
            .index_definitions
            .read()
            .get(&IndexIdentity { space_id, index_id })
            .cloned();
        let prefix = KeyBuilder::build_edge_index_prefix(space_id, index_name);
        let end = KeyBuilder::build_range_end(&prefix);
        let expected_entity = edge_entity_ref(edge_src, edge_dst, edge_type, ranking);
        let mut existing_values = Vec::new();
        let mut existing_columns = Vec::new();
        let mut existing_columns_ts = 0;
        for shard in generation.shards() {
            for (key, record) in shard
                .forward()
                .read()
                .range(prefix.0.clone()..end.0.clone())
            {
                if !record.is_visible_at(write_ts) {
                    continue;
                }
                let matches_entity = record
                    .entity_ref
                    .as_ref()
                    .zip(expected_entity.as_ref())
                    .is_some_and(|(actual, expected)| actual == expected)
                    || KeyParser::parse_edge_identity_from_key(key).is_ok_and(
                        |(candidate_src, candidate_dst, candidate_type, candidate_rank)| {
                            candidate_src == *edge_src
                                && candidate_dst == *edge_dst
                                && candidate_type == edge_type
                                && candidate_rank == ranking
                        },
                    );
                if !matches_entity {
                    continue;
                }
                if let Ok(value) = KeyParser::parse_prop_value_from_edge_key(key) {
                    if !existing_values.contains(&value) {
                        existing_values.push(value);
                    }
                }
                if record.created_ts >= existing_columns_ts {
                    existing_columns_ts = record.created_ts;
                    existing_columns = record.included_columns.clone();
                }
            }
        }
        let indexed_values =
            effective_index_values(index_definition.as_ref(), props, existing_values);
        let included_columns =
            merged_included_columns(index_definition.as_ref(), existing_columns, props);
        if !indexed_values.is_empty() {
            self.clear_edge_entity(
                space_id, edge_src, edge_dst, edge_type, ranking, index_name, write_ts,
            )?;
        }
        for value in indexed_values {
            let forward = KeyBuilder::build_edge_index_key(
                space_id, index_name, &value, edge_src, edge_dst, edge_type, ranking,
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
            let mut entry = IndexRecord::new_with_columns(write_ts, included_columns.clone())
                .with_entity_version(write_ts);
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
        let runtime = self.runtime(space_id, index_id)?;
        let _fence = runtime.read_fence();
        let (manifest, _runtime, generation) = self.active_generation(space_id, index_id)?;
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
        self.wait_for_active_barrier(&runtime);
        let catalog = self
            .manifest_catalog(space_id, index_id)
            .ok_or_else(|| StorageError::not_found("Index manifest catalog is unavailable"))?;
        let manifest = catalog.acquire();
        let generation = runtime
            .generation(manifest.manifest().generation)
            .ok_or_else(|| {
                StorageError::not_found("Active index runtime generation is unavailable")
            })?;
        let index_definition = self
            .index_definitions
            .read()
            .get(&IndexIdentity { space_id, index_id })
            .cloned();
        let prefix = KeyBuilder::build_vertex_index_prefix(space_id, index_name);
        let end = KeyBuilder::build_range_end(&prefix);
        let expected_entity = vertex_entity_ref(vertex_id);
        let mut existing_values = Vec::new();
        let mut existing_columns = Vec::new();
        let mut existing_columns_ts = 0;
        for shard in generation.shards() {
            for (key, record) in shard
                .forward()
                .read()
                .range(prefix.0.clone()..end.0.clone())
            {
                if !record.is_visible_at(write_ts) {
                    continue;
                }
                let matches_entity = record
                    .entity_ref
                    .as_ref()
                    .zip(expected_entity.as_ref())
                    .is_some_and(|(actual, expected)| actual == expected)
                    || KeyParser::parse_vertex_id_from_key(key)
                        .is_ok_and(|candidate| candidate == *vertex_id);
                if !matches_entity {
                    continue;
                }
                if let Ok(value) = KeyParser::parse_prop_value_from_key(key) {
                    if !existing_values.contains(&value) {
                        existing_values.push(value);
                    }
                }
                if record.created_ts >= existing_columns_ts {
                    existing_columns_ts = record.created_ts;
                    existing_columns = record.included_columns.clone();
                }
            }
        }
        let indexed_values =
            effective_index_values(index_definition.as_ref(), props, existing_values);
        let included_columns =
            merged_included_columns(index_definition.as_ref(), existing_columns, props);
        if !indexed_values.is_empty() {
            self.clear_vertex_entity(space_id, vertex_id, index_name, write_ts)?;
        }
        for value in indexed_values {
            let forward =
                KeyBuilder::build_vertex_index_key(space_id, index_name, &value, vertex_id)?;
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
            let mut entry = IndexRecord::new_with_columns(write_ts, included_columns.clone())
                .with_entity_version(write_ts);
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
        let runtime = self.runtime(space_id, index_id)?;
        let _fence = runtime.read_fence();
        let (manifest, _runtime, generation) = self.active_generation(space_id, index_id)?;
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
        CommitLsn, Index, IndexConfig, IndexField, IndexGeneration, IndexType, SnapshotTimestamp,
        MAX_TIMESTAMP,
    };
    use crate::core::Value;
    use crate::storage::cursor::{
        IndexCursor, IndexPredicate, IndexRow, IndexScanPlan, PartitionSelector,
    };
    use crate::storage::index::generic_index_manager::GenericIndexManager;
    use crate::storage::index::key_codec::{KeyBuilder, VertexIndexKeyGen};
    use crate::storage::index::manifest::{
        GenerationBuildState, GenerationState, IndexManifest, IndexShard,
    };
    use crate::storage::index::*;
    use std::collections::BTreeMap;

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
        let manager = IndexDataManagerImpl::new_with_root(directory.path().join("indexes"));
        let first_index = create_test_index("first", "person");
        let mut second_index = create_test_index("second", "person");
        second_index.id = 2;
        manager
            .register_native_index(1, &first_index)
            .expect("register first index");
        manager
            .register_native_index(1, &second_index)
            .expect("register second index");

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
            .split_native_index(
                1,
                1,
                boundary,
                SnapshotTimestamp::new(1),
                CommitLsn::new(1),
                || Ok(CommitLsn::new(100)),
                |_, _| Ok(Vec::new()),
            )
            .expect("split first index");

        let catalog = manager.manifest_catalog(1, 1).expect("catalog exists");
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
    // --- Phase 3: Split crash recovery ---

    #[test]
    fn resolve_split_crash_recovery_discards_building_state() {
        let directory = tempfile::tempdir().expect("tempdir");
        let manager = IndexDataManagerImpl::new_with_root(directory.path().join("indexes"));
        let index = create_test_index("first", "person");
        manager
            .register_native_index(1, &index)
            .expect("register index");
        manager
            .update_vertex_indexes_mvcc(
                1,
                &Value::Int(1),
                "first",
                &[("name".to_string(), Value::String("Alice".to_string()))],
                1,
            )
            .expect("write");

        let index_root = directory.path().join("indexes").join("1").join("1");
        let build_state_path = index_root.join("generation_build.bin");

        let crashed = GenerationBuildState {
            generation: IndexGeneration::new(2),
            snapshot_timestamp: SnapshotTimestamp::new(1),
            start_lsn: CommitLsn::new(10),
            barrier_lsn: None,
            state: GenerationState::Building,
            terminal_reason: None,
        };
        let bytes = postcard::to_allocvec(&crashed).expect("serialize");
        std::fs::create_dir_all(&index_root).unwrap();
        std::fs::write(&build_state_path, &bytes).unwrap();

        manager
            .resolve_split_crash_recovery(&index_root)
            .expect("recovery should succeed");

        assert!(
            !build_state_path.exists(),
            "Building state must be discarded on recovery"
        );

        let results = manager
            .lookup_tag_index(1, &index, &Value::String("Alice".to_string()))
            .expect("lookup after recovery");
        assert_eq!(results, vec![Value::Int(1)]);
    }

    #[test]
    fn resolve_split_crash_recovery_discards_catching_up_state() {
        let directory = tempfile::tempdir().expect("tempdir");
        let manager = IndexDataManagerImpl::new_with_root(directory.path().join("indexes"));
        let index = create_test_index("first", "person");
        manager
            .register_native_index(1, &index)
            .expect("register index");

        let index_root = directory.path().join("indexes").join("1").join("1");
        let build_state_path = index_root.join("generation_build.bin");

        let crashed = GenerationBuildState {
            generation: IndexGeneration::new(2),
            snapshot_timestamp: SnapshotTimestamp::new(1),
            start_lsn: CommitLsn::new(10),
            barrier_lsn: None,
            state: GenerationState::CatchingUp,
            terminal_reason: None,
        };
        let bytes = postcard::to_allocvec(&crashed).expect("serialize");
        std::fs::create_dir_all(&index_root).unwrap();
        std::fs::write(&build_state_path, &bytes).unwrap();

        manager
            .resolve_split_crash_recovery(&index_root)
            .expect("recovery should succeed");

        assert!(
            !build_state_path.exists(),
            "CatchingUp state must be discarded on recovery"
        );
    }

    #[test]
    fn resolve_split_crash_recovery_completes_publishing_state_with_manifest() {
        let directory = tempfile::tempdir().expect("tempdir");
        let manager = IndexDataManagerImpl::new_with_root(directory.path().join("indexes"));
        let index = create_test_index("first", "person");
        manager
            .register_native_index(1, &index)
            .expect("register index");
        manager
            .update_vertex_indexes_mvcc(
                1,
                &Value::Int(1),
                "first",
                &[("name".to_string(), Value::String("Alice".to_string()))],
                1,
            )
            .expect("write");

        let index_root = directory.path().join("indexes").join("1").join("1");
        let build_state_path = index_root.join("generation_build.bin");

        let manifest = IndexManifest::new(
            1,
            1,
            IndexGeneration::new(2),
            vec![IndexShard {
                shard_id: 0,
                lower: None,
                upper: None,
                checkpoint_file: index_root.join("generation-2").join("shard-0"),
                checksum: None,
            }],
        )
        .expect("new manifest");
        manifest
            .store(&index_root.join("manifest.bin"))
            .expect("store manifest");

        let publishing = GenerationBuildState {
            generation: IndexGeneration::new(2),
            snapshot_timestamp: SnapshotTimestamp::new(1),
            start_lsn: CommitLsn::new(10),
            barrier_lsn: Some(CommitLsn::new(50)),
            state: GenerationState::Publishing,
            terminal_reason: None,
        };
        let bytes = postcard::to_allocvec(&publishing).expect("serialize");
        std::fs::create_dir_all(&index_root).unwrap();
        std::fs::write(&build_state_path, &bytes).unwrap();

        manager
            .resolve_split_crash_recovery(&index_root)
            .expect("recovery should succeed");

        assert!(
            !build_state_path.exists(),
            "Publishing build state removed after completion"
        );
        assert!(
            index_root.join("manifest.bin").exists(),
            "manifest preserved after Publishing completion"
        );

        let results = manager
            .lookup_tag_index(1, &index, &Value::String("Alice".to_string()))
            .expect("lookup after publishing recovery");
        assert_eq!(results, vec![Value::Int(1)]);
    }

    #[test]
    fn resolve_split_crash_recovery_discards_publishing_without_manifest() {
        let directory = tempfile::tempdir().expect("tempdir");
        let manager = IndexDataManagerImpl::new_with_root(directory.path().join("indexes"));
        let index = create_test_index("first", "person");
        manager
            .register_native_index(1, &index)
            .expect("register index");

        let index_root = directory.path().join("indexes").join("1").join("1");
        let build_state_path = index_root.join("generation_build.bin");

        let publishing = GenerationBuildState {
            generation: IndexGeneration::new(2),
            snapshot_timestamp: SnapshotTimestamp::new(1),
            start_lsn: CommitLsn::new(10),
            barrier_lsn: Some(CommitLsn::new(50)),
            state: GenerationState::Publishing,
            terminal_reason: None,
        };
        let bytes = postcard::to_allocvec(&publishing).expect("serialize");
        std::fs::create_dir_all(&index_root).unwrap();
        std::fs::write(&build_state_path, &bytes).unwrap();

        manager
            .resolve_split_crash_recovery(&index_root)
            .expect("recovery should succeed");

        assert!(
            !build_state_path.exists(),
            "Publishing state without manifest must be discarded"
        );
    }

    // --- Phase 4: Included columns MVCC ---

    fn create_edge_index_with_included_properties() -> Index {
        // `weight` is the indexed field (stable key), `since` is the included
        // property (changes on update without altering the index key).
        Index::new(IndexConfig {
            id: 1,
            name: "knows_weight_idx".to_string(),
            space_id: 1,
            schema_name: "Person".to_string(),
            fields: vec![IndexField::new("weight".to_string(), Value::Int(0), false)],
            properties: vec!["since".to_string()],
            index_type: IndexType::EdgeIndex,
            is_unique: false,
            partial_condition: None,
        })
    }

    #[test]
    fn included_columns_visible_in_covering_query_after_update() {
        let manager = IndexDataManagerImpl::new();
        let index = create_edge_index_with_included_properties();
        manager
            .register_native_index(1, &index)
            .expect("register edge index");

        // Initial write: weight=10 (indexed), since=2020 (included).
        manager
            .update_edge_indexes_mvcc(
                1,
                &Value::Int(1),
                &Value::Int(2),
                "KNOWS",
                0,
                "knows_weight_idx",
                &[
                    ("weight".to_string(), Value::Int(10)),
                    ("since".to_string(), Value::Int(2020)),
                ],
                10,
            )
            .expect("initial write");

        let covering_plan = IndexScanPlan {
            space: "space".to_string(),
            index_id: 1,
            predicate: IndexPredicate::All,
            partition: PartitionSelector::All,
            partition_id_range: None,
            projection: Some(vec!["since".to_string()]),
            limit: None,
            offset: 0,
            read_timestamp: 10,
        };
        let mut cursor = manager
            .open_edge_index_cursor(1, &index, &covering_plan)
            .expect("cursor");
        let rows: Vec<IndexRow> =
            std::iter::from_fn(|| cursor.next_batch(64).ok().filter(|b| !b.is_empty()))
                .flatten()
                .collect();
        assert_eq!(rows.len(), 1);
        match &rows[0] {
            IndexRow::Covering { columns, .. } => {
                assert_eq!(columns.len(), 1);
                assert_eq!(columns[0], ("since".to_string(), Value::Int(2020)));
            }
            IndexRow::RowId(_) => panic!("expected covering row"),
        }

        // Update only the included column: the index manager must retain the
        // existing indexed value and refresh the covering payload.
        manager
            .update_edge_indexes_mvcc(
                1,
                &Value::Int(1),
                &Value::Int(2),
                "KNOWS",
                0,
                "knows_weight_idx",
                &[("since".to_string(), Value::Int(2024))],
                20,
            )
            .expect("update");

        let after_update_plan = IndexScanPlan {
            space: "space".to_string(),
            index_id: 1,
            predicate: IndexPredicate::All,
            partition: PartitionSelector::All,
            partition_id_range: None,
            projection: Some(vec!["since".to_string()]),
            limit: None,
            offset: 0,
            read_timestamp: 20,
        };
        let mut cursor = manager
            .open_edge_index_cursor(1, &index, &after_update_plan)
            .expect("cursor after update");
        let rows: Vec<IndexRow> =
            std::iter::from_fn(|| cursor.next_batch(64).ok().filter(|b| !b.is_empty()))
                .flatten()
                .collect();
        assert_eq!(rows.len(), 1);
        match &rows[0] {
            IndexRow::Covering { columns, .. } => {
                assert_eq!(columns[0], ("since".to_string(), Value::Int(2024)));
            }
            IndexRow::RowId(_) => panic!("expected covering row after update"),
        }

        // Snapshot query at ts=10 still sees the old included value (MVCC).
        let snapshot_plan = IndexScanPlan {
            space: "space".to_string(),
            index_id: 1,
            predicate: IndexPredicate::All,
            partition: PartitionSelector::All,
            partition_id_range: None,
            projection: Some(vec!["since".to_string()]),
            limit: None,
            offset: 0,
            read_timestamp: 10,
        };
        let mut cursor = manager
            .open_edge_index_cursor(1, &index, &snapshot_plan)
            .expect("snapshot cursor");
        let rows: Vec<IndexRow> =
            std::iter::from_fn(|| cursor.next_batch(64).ok().filter(|b| !b.is_empty()))
                .flatten()
                .collect();
        assert_eq!(rows.len(), 1);
        match &rows[0] {
            IndexRow::Covering { columns, .. } => {
                assert_eq!(columns[0], ("since".to_string(), Value::Int(2020)));
            }
            IndexRow::RowId(_) => panic!("expected covering row at snapshot"),
        }
    }

    #[test]
    fn included_columns_not_visible_after_delete() {
        let manager = IndexDataManagerImpl::new();
        let index = create_edge_index_with_included_properties();
        manager
            .register_native_index(1, &index)
            .expect("register edge index");

        manager
            .update_edge_indexes_mvcc(
                1,
                &Value::Int(1),
                &Value::Int(2),
                "KNOWS",
                0,
                "knows_weight_idx",
                &[
                    ("weight".to_string(), Value::Int(10)),
                    ("since".to_string(), Value::Int(2020)),
                ],
                10,
            )
            .expect("write");

        let covering_plan = IndexScanPlan {
            space: "space".to_string(),
            index_id: 1,
            predicate: IndexPredicate::All,
            partition: PartitionSelector::All,
            partition_id_range: None,
            projection: Some(vec!["since".to_string()]),
            limit: None,
            offset: 0,
            read_timestamp: 10,
        };
        let mut cursor = manager
            .open_edge_index_cursor(1, &index, &covering_plan)
            .expect("cursor");
        let rows: Vec<IndexRow> =
            std::iter::from_fn(|| cursor.next_batch(64).ok().filter(|b| !b.is_empty()))
                .flatten()
                .collect();
        assert_eq!(rows.len(), 1, "one edge before delete");

        manager
            .delete_edge_indexes_mvcc(
                1,
                &Value::Int(1),
                &Value::Int(2),
                "KNOWS",
                0,
                &["knows_weight_idx".to_string()],
                20,
            )
            .expect("delete");

        let after_delete_plan = IndexScanPlan {
            space: "space".to_string(),
            index_id: 1,
            predicate: IndexPredicate::All,
            partition: PartitionSelector::All,
            partition_id_range: None,
            projection: Some(vec!["since".to_string()]),
            limit: None,
            offset: 0,
            read_timestamp: 20,
        };
        let mut cursor = manager
            .open_edge_index_cursor(1, &index, &after_delete_plan)
            .expect("cursor after delete");
        let rows: Vec<IndexRow> =
            std::iter::from_fn(|| cursor.next_batch(64).ok().filter(|b| !b.is_empty()))
                .flatten()
                .collect();
        assert!(
            rows.is_empty(),
            "covering query must not return deleted edge"
        );
    }

    #[test]
    fn included_columns_survive_rebuild_from_snapshot() {
        use crate::storage::index::generic_index_manager::GenericIndexManager;

        let directory = tempfile::tempdir().expect("tempdir");
        let manager = IndexDataManagerImpl::new_with_root(directory.path().join("indexes"));
        let index = create_edge_index_with_included_properties();
        manager
            .register_native_index(1, &index)
            .expect("register index");

        manager
            .update_edge_indexes_mvcc(
                1,
                &Value::Int(1),
                &Value::Int(2),
                "KNOWS",
                0,
                "knows_weight_idx",
                &[
                    ("weight".to_string(), Value::Int(10)),
                    ("since".to_string(), Value::Int(2020)),
                ],
                10,
            )
            .expect("write");

        let runtime = manager.runtime(1, 1).expect("runtime");
        let catalog = manager.manifest_catalog(1, 1).expect("catalog");
        let manifest = catalog.acquire();
        let generation = runtime
            .generation(manifest.manifest().generation)
            .expect("active generation");
        let mut forward = BTreeMap::new();
        let mut reverse = BTreeMap::new();
        for shard in generation.shards() {
            let (f, r) = shard.snapshot();
            forward.extend(f);
            reverse.extend(r);
        }
        drop(manifest);

        let checkpoint_dir = directory
            .path()
            .join("indexes")
            .join("1")
            .join("1")
            .join("generation-2")
            .join("shard-0");
        GenericIndexManager::<crate::storage::index::key_codec::EdgeIndexKeyGen>::flush_data(
            &checkpoint_dir,
            &forward,
            &reverse,
        )
        .expect("flush checkpoint");

        let next_gen = IndexGeneration::new(2);
        let next_manifest = IndexManifest::new(
            1,
            1,
            next_gen,
            vec![IndexShard {
                shard_id: 0,
                lower: None,
                upper: None,
                checkpoint_file: checkpoint_dir,
                checksum: None,
            }],
        )
        .expect("new manifest");
        manager
            .publish_native_index(next_manifest, forward, reverse, CommitLsn::ZERO)
            .expect("publish");

        let covering_plan = IndexScanPlan {
            space: "space".to_string(),
            index_id: 1,
            predicate: IndexPredicate::All,
            partition: PartitionSelector::All,
            partition_id_range: None,
            projection: Some(vec!["since".to_string()]),
            limit: None,
            offset: 0,
            read_timestamp: 10,
        };
        let mut cursor = manager
            .open_edge_index_cursor(1, &index, &covering_plan)
            .expect("cursor after rebuild");
        let rows: Vec<IndexRow> =
            std::iter::from_fn(|| cursor.next_batch(64).ok().filter(|b| !b.is_empty()))
                .flatten()
                .collect();
        assert_eq!(rows.len(), 1, "rebuilt index should have one entry");
        match &rows[0] {
            IndexRow::Covering { columns, .. } => {
                assert_eq!(columns[0], ("since".to_string(), Value::Int(2020)));
            }
            IndexRow::RowId(_) => panic!("expected covering row after rebuild"),
        }
    }
}
