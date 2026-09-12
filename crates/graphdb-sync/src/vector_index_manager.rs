//! Vector Index Manager
//!
//! Manages vector index metadata, lifecycle (create/drop), and search
//! operations.  Extracted from `VectorSyncCoordinator` to separate index
//! management from synchronization concerns.
//!
//! The query layer depends on `VectorIndexManager` for index CRUD and search,
//! while the sync layer (`VectorSyncCoordinator`) wraps it and adds outbox /
//! change-batching / embedding capabilities.

use std::collections::HashMap;

use dashmap::DashMap;
use tracing::info;

use crate::backend::VectorBackend;
use crate::vector_error::{VectorCoordinatorError, VectorCoordinatorResult, VectorError};
use graphdb_core::event_dispatch::{EventFilter, EventSubscriptions, SubscriptionId};
pub use graphdb_fulltext::{IndexEvent, IndexEventCallback};
pub use vector_search::types::{DistanceMetric, PointId, SearchQuery, SearchResult, VectorPoint};
use vector_search::{
    types::validate_distance_metric, CollectionConfig, FilterCondition, IndexMetadata,
    PayloadSchemaType, VectorFilter,
};

use super::vector_sync::{CollectionGranularity, SearchOptions, VectorIndexLocation};

/// Validate a distance metric at the index-creation entry points.
fn validate_metric(distance: DistanceMetric) -> VectorCoordinatorResult<()> {
    validate_distance_metric(distance)
        .map_err(|e| VectorCoordinatorError::Vector(VectorError::ConfigError(e)))
}

fn validate_metric_for_backend(
    backend: &VectorBackend,
    distance: DistanceMetric,
) -> VectorCoordinatorResult<()> {
    validate_metric(distance)?;
    let _ = backend;
    Ok(())
}

/// Manages vector index metadata, lifecycle, and search operations.
///
/// This struct owns the vector backend handle and the logical index registry.
/// It provides all index CRUD and search methods without any synchronization
/// (outbox, change batching, embedding) concerns.
pub struct VectorIndexManager {
    backend: VectorBackend,
    /// Tracks registered logical indexes by key `(space_id, tag, field)`.
    logical_indexes: DashMap<VectorIndexLocation, IndexMetadata>,
    /// Rebuild scratch collections by key `(space_id, tag, field, generation)`.
    /// Temps are invisible to search: they never enter `logical_indexes` and
    /// collection discovery never adopts `*.rebuild-*` names as logical
    /// indexes (see startup recovery in `vector_rebuild.rs`).
    temp_collections: DashMap<TempKey, TempCollection>,
    /// Qdrant Field-granularity publish remaps: the remote backend has no
    /// atomic rename, so publish switches the logical collection pointer to
    /// the built temp instead of moving data. Local publishes rename
    /// directories, so this map stays empty for local backends.
    collection_overrides: DashMap<VectorIndexLocation, String>,
    /// Retained pre-publish collections by derived live name (at most one
    /// per live: Field granularity only; Space converges by rebuilding).
    promote_backups: DashMap<String, String>,
    /// Per-index publish fences shared with the sync coordinator. Delivery
    /// applies hold the read guard; rebuild publish and explicit purges hold
    /// the write guard, so a commit is either fully before or fully after
    /// each fenced window.
    publish_fences: DashMap<VectorIndexLocation, std::sync::Arc<tokio::sync::RwLock<()>>>,
    /// Collection granularity. Space-level is default for backward
    /// compatibility; Field-level gives physical isolation per (tag,field).
    granularity: parking_lot::RwLock<CollectionGranularity>,
    index_callbacks: EventSubscriptions<IndexEvent>,
}

/// Scratch rebuild collection invisible to the query path.
#[derive(Debug, Clone)]
pub struct TempCollection {
    /// Physical temp collection name (`{live}.rebuild-{gen}[__{tag}_{field}]`).
    pub temp_name: String,
    /// Derived live collection name the temp will publish over.
    pub live_name: String,
    /// Effective collection config the temp was created with.
    pub config: CollectionConfig,
    /// When the temp was registered.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Registry key for one rebuild scratch collection.
pub type TempKey = (u64, String, String, u64);

/// Outcome of publishing one temp collection.
#[derive(Debug, Clone)]
pub struct PublishOutcome {
    /// Retained pre-publish collection (one backup generation for Field
    /// granularity; `None` for Space slice搬迁 which keeps no backup).
    pub backup: Option<String>,
    /// Points copied back into live (Space granularity only; Field
    /// publishes switch pointers without copying).
    pub copied_points: u64,
}

impl std::fmt::Debug for VectorIndexManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VectorIndexManager")
            .field("backend", &self.backend)
            .field("logical_index_count", &self.logical_indexes.len())
            .field("granularity", &*self.granularity.read())
            .field("index_callbacks", &self.index_callbacks.len())
            .finish()
    }
}

impl VectorIndexManager {
    /// Create a new index manager with the given backend.
    pub fn new(backend: VectorBackend) -> Self {
        Self {
            backend,
            logical_indexes: DashMap::new(),
            temp_collections: DashMap::new(),
            collection_overrides: DashMap::new(),
            promote_backups: DashMap::new(),
            publish_fences: DashMap::new(),
            granularity: parking_lot::RwLock::new(CollectionGranularity::default()),
            index_callbacks: EventSubscriptions::new(),
        }
    }

    /// Publish fence for one vector index. Delivery applies hold the read
    /// guard; rebuild publish and explicit purges hold the write guard.
    /// The coordinator delegates here so both paths share one lock per index.
    pub fn publish_fence_for(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> std::sync::Arc<tokio::sync::RwLock<()>> {
        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        self.publish_fences
            .entry(loc)
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::RwLock::new(())))
            .clone()
    }

    /// True while a rebuild scratch collection is registered for the index
    /// (from temp creation until publish or discard).
    pub fn has_rebuild_temp(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        self.temp_collections.iter().any(|entry| {
            entry.key().0 == space_id && entry.key().1 == tag_name && entry.key().2 == field_name
        })
    }

    /// Register a runtime observer for vector index lifecycle events.
    pub fn register_index_callback(&self, callback: IndexEventCallback) -> SubscriptionId {
        self.index_callbacks.add(callback)
    }

    /// Register a filtered observer invoked only when `filter` returns true.
    pub fn register_index_callback_filtered(
        &self,
        callback: IndexEventCallback,
        filter: EventFilter<IndexEvent>,
    ) -> SubscriptionId {
        self.index_callbacks.add_filtered(callback, Some(filter))
    }

    /// Remove a previously registered observer. Returns true if present.
    pub fn unregister_index_callback(&self, id: SubscriptionId) -> bool {
        self.index_callbacks.remove(id)
    }

    /// Number of registered vector index observers.
    pub fn index_callback_count(&self) -> usize {
        self.index_callbacks.len()
    }

    fn emit_index_event(&self, event: IndexEvent) {
        self.index_callbacks.dispatch("index", &event);
    }

    /// Emit a rebuild lifecycle event to registered observers.
    pub fn emit_rebuild_event(&self, event: IndexEvent) {
        self.emit_index_event(event);
    }

    /// Get a reference to the underlying vector backend.
    pub fn backend(&self) -> &VectorBackend {
        &self.backend
    }

    pub fn granularity(&self) -> CollectionGranularity {
        *self.granularity.read()
    }

    pub fn set_granularity(&self, granularity: CollectionGranularity) {
        *self.granularity.write() = granularity;
    }

    /// Resolve collection name respecting the configured granularity and
    /// any Qdrant publish remap. Reads always resolve through here, so a
    /// published temp serves traffic atomically after the pointer switch.
    pub fn collection_name_for(&self, loc: &VectorIndexLocation) -> String {
        if let Some(target) = self.collection_overrides.get(loc) {
            return target.clone();
        }
        loc.to_collection_name_with(self.granularity())
    }

    /// Derived live collection name ignoring publish remaps.
    pub fn live_collection_name(&self, loc: &VectorIndexLocation) -> String {
        loc.to_collection_name_with(self.granularity())
    }

    /// Resolve group_id respecting granularity. `None` means field-level
    /// physical isolation, no group filter needed.
    pub fn group_id_for(&self, loc: &VectorIndexLocation) -> Option<String> {
        loc.group_id_with(self.granularity())
    }

    /// Whether the engine is disabled.
    pub fn is_disabled_engine(&self) -> bool {
        self.backend.is_disabled()
    }

    // ── Index lifecycle ───────────────────────────────────────────────

    /// Create a vector index (logical index in shared collection).
    pub async fn create_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector_size: usize,
        distance: DistanceMetric,
    ) -> VectorCoordinatorResult<String> {
        validate_metric_for_backend(&self.backend, distance)?;

        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        let collection_name = self.collection_name_for(&loc);
        let logical_index_name = format!("vec_{space_id}_{tag_name}_{field_name}");
        self.emit_index_event(IndexEvent::VectorBuildStarted {
            index_name: logical_index_name.clone(),
        });

        if self.is_disabled_engine() {
            let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
            let meta = IndexMetadata::new(
                collection_name.clone(),
                CollectionConfig::new(vector_size, distance),
            );
            self.logical_indexes.insert(logical_key, meta);
            info!(
                "Logical vector index created in disabled mode: space={} tag={} field={} in collection {}",
                space_id, tag_name, field_name, collection_name
            );
            self.emit_index_event(IndexEvent::VectorBuildCompleted {
                index_name: logical_index_name,
                vectors_count: 0,
            });
            return Ok(collection_name);
        }

        let config = if self.backend.is_local() {
            CollectionConfig::new(vector_size, distance)
        } else {
            let hnsw_config = vector_search::HnswConfig::new(16, 100).with_payload_m(16);
            CollectionConfig::new(vector_size, distance).with_hnsw(hnsw_config)
        };
        // Register and compare the effective (stored-equivalent) config:
        // the engine/store fill tier defaults at creation, so comparing the
        // raw request against a stored config false-conflicts on idempotent
        // re-creation (e.g. sibling logical indexes sharing one collection).
        let config = self.backend.effective_collection_config(&config);

        if !self.backend.index_exists(&collection_name) {
            self.backend
                .create_index(&collection_name, &config)
                .await
                .map_err(|e| VectorCoordinatorError::IndexCreationFailed {
                    tag_name: tag_name.to_string(),
                    field_name: field_name.to_string(),
                    reason: e.to_string(),
                })?;

            if self.granularity() == CollectionGranularity::Space {
                if let Err(e) = self
                    .backend
                    .create_payload_index(&collection_name, "group_id", PayloadSchemaType::Keyword)
                    .await
                {
                    tracing::warn!(
                        "Failed to create payload index for group_id in collection '{}': {}",
                        collection_name,
                        e
                    );
                }
            }
        } else {
            if let Some(existing_meta) = self.backend.get_index_metadata(&collection_name) {
                let existing = &existing_meta.config;
                if existing.vector_size != vector_size
                    || existing.distance != distance
                    || existing.index_type != config.index_type
                    || format!("{:?}", existing.hnsw_config) != format!("{:?}", config.hnsw_config)
                    || existing.quantization_config != config.quantization_config
                    || format!("{:?}", existing.ivf_config) != format!("{:?}", config.ivf_config)
                {
                    return Err(VectorCoordinatorError::CollectionConfigConflict {
                        collection_name: collection_name.clone(),
                        existing_size: existing.vector_size,
                        existing_dist: format!(
                            "{:?}/{:?}/{:?}/{:?}",
                            existing.distance,
                            existing.index_type,
                            existing.hnsw_config,
                            existing.quantization_config
                        ),
                        requested_size: vector_size,
                        requested_dist: format!(
                            "{:?}/{:?}/{:?}/{:?}",
                            distance,
                            config.index_type,
                            config.hnsw_config,
                            config.quantization_config
                        ),
                    });
                }
            }
        }

        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        let meta = IndexMetadata::new(collection_name.clone(), config);
        self.logical_indexes.insert(logical_key, meta);

        info!(
            "Logical vector index created: space={} tag={} field={} in collection {}",
            space_id, tag_name, field_name, collection_name
        );
        self.emit_index_event(IndexEvent::VectorBuildCompleted {
            index_name: logical_index_name,
            vectors_count: 0,
        });
        Ok(collection_name)
    }

    /// Create vector index with config (logical index in shared collection).
    pub async fn create_index_with_config(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        config: CollectionConfig,
    ) -> VectorCoordinatorResult<String> {
        validate_metric_for_backend(&self.backend, config.distance)?;

        // See `create_vector_index`: compare and register the effective
        // config so engine/store-applied defaults do not false-conflict.
        let config = self.backend.effective_collection_config(&config);

        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        let collection_name = self.collection_name_for(&loc);
        let logical_index_name = format!("vec_{space_id}_{tag_name}_{field_name}");
        self.emit_index_event(IndexEvent::VectorBuildStarted {
            index_name: logical_index_name.clone(),
        });

        if !self.backend.index_exists(&collection_name) {
            self.backend
                .create_index(&collection_name, &config)
                .await
                .map_err(|e| VectorCoordinatorError::IndexCreationFailed {
                    tag_name: tag_name.to_string(),
                    field_name: field_name.to_string(),
                    reason: e.to_string(),
                })?;

            if self.granularity() == CollectionGranularity::Space {
                if let Err(e) = self
                    .backend
                    .create_payload_index(&collection_name, "group_id", PayloadSchemaType::Keyword)
                    .await
                {
                    tracing::warn!(
                        "Failed to create payload index for group_id in collection '{}': {}",
                        collection_name,
                        e
                    );
                }
            }
        } else {
            if let Some(existing_meta) = self.backend.get_index_metadata(&collection_name) {
                let existing = &existing_meta.config;
                if existing.vector_size != config.vector_size
                    || existing.distance != config.distance
                    || existing.index_type != config.index_type
                    || format!("{:?}", existing.hnsw_config) != format!("{:?}", config.hnsw_config)
                    || existing.quantization_config != config.quantization_config
                    || format!("{:?}", existing.ivf_config) != format!("{:?}", config.ivf_config)
                {
                    return Err(VectorCoordinatorError::CollectionConfigConflict {
                        collection_name: collection_name.clone(),
                        existing_size: existing.vector_size,
                        existing_dist: format!(
                            "{:?}/{:?}/{:?}/{:?}",
                            existing.distance,
                            existing.index_type,
                            existing.hnsw_config,
                            existing.quantization_config
                        ),
                        requested_size: config.vector_size,
                        requested_dist: format!(
                            "{:?}/{:?}/{:?}/{:?}",
                            config.distance,
                            config.index_type,
                            config.hnsw_config,
                            config.quantization_config
                        ),
                    });
                }
            }
        }

        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        let meta = IndexMetadata::new(collection_name.clone(), config);
        self.logical_indexes.insert(logical_key, meta);

        info!(
            "Logical vector index created with config: space={} tag={} field={} in collection {}",
            space_id, tag_name, field_name, collection_name
        );
        self.emit_index_event(IndexEvent::VectorBuildCompleted {
            index_name: logical_index_name,
            vectors_count: 0,
        });
        Ok(collection_name)
    }

    /// Drop a vector index (remove logical index, physical collection remains).
    ///
    /// Any rebuild temps for this index are discarded as well, and a Qdrant
    /// publish remap is cleared (the remap target is reclaimed under Field
    /// granularity together with its retained backup).
    pub async fn drop_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> VectorCoordinatorResult<()> {
        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        let live_name = self.live_collection_name(&logical_key);
        let resolved_name = self.collection_name_for(&logical_key);
        self.logical_indexes.remove(&logical_key);
        self.collection_overrides.remove(&logical_key);
        self.discard_temps_for(space_id, tag_name, field_name).await;

        if self.granularity() == CollectionGranularity::Field {
            for name in [resolved_name, live_name] {
                if name.is_empty() || !self.backend.index_exists(&name) {
                    continue;
                }
                if let Err(error) = self.backend.delete_collection(&name).await {
                    tracing::error!(
                        "Failed to reclaim vector collection '{}' (field granularity): {}",
                        name,
                        error
                    );
                }
            }
            if let Some((_, backup)) =
                self.promote_backups
                    .remove(&self.live_collection_name(&VectorIndexLocation::new(
                        space_id, tag_name, field_name,
                    )))
            {
                if self.backend.index_exists(&backup) {
                    if let Err(error) = self.backend.delete_collection(&backup).await {
                        tracing::error!(
                            "Failed to reclaim vector promote backup '{}': {}",
                            backup,
                            error
                        );
                    }
                }
            }
        } else if self.backend.is_local() && self.backend.index_exists(&live_name) {
            let remaining_siblings = self
                .logical_indexes
                .iter()
                .filter(|entry| entry.value().name == live_name)
                .count();
            if remaining_siblings == 0 {
                if let Err(error) = self.backend.delete_collection(&live_name).await {
                    tracing::error!(
                        "Failed to reclaim vector collection '{}': {}",
                        live_name,
                        error
                    );
                }
            } else if let Err(error) = self
                .backend
                .delete_by_filter(
                    &live_name,
                    VectorFilter::new().must(FilterCondition::match_value(
                        "group_id",
                        format!("{tag_name}_{field_name}"),
                    )),
                )
                .await
            {
                tracing::error!(
                    "Failed to purge dropped vector group '{tag_name}_{field_name}' from collection '{}': {}",
                    live_name,
                    error
                );
            }
        }

        info!(
            "Logical vector index dropped: space={} tag={} field={}",
            space_id, tag_name, field_name
        );
        self.emit_index_event(IndexEvent::VectorDropped {
            index_name: format!("vec_{space_id}_{tag_name}_{field_name}"),
        });
        Ok(())
    }

    /// Drop every logical vector index of one space (space-level cascade).
    ///
    /// Collects matching keys first, then drops each through
    /// `drop_vector_index` so Field/Space granularity physical reclaim keeps
    /// its semantics. Missing indexes are skipped; directly created
    /// collections without a logical registration are left for operator
    /// cleanup.
    pub async fn drop_space_indexes(&self, space_id: u64) -> VectorCoordinatorResult<()> {
        let keys: Vec<(u64, String, String)> = self
            .logical_indexes
            .iter()
            .filter(|entry| entry.key().space_id == space_id)
            .map(|entry| {
                (
                    entry.key().space_id,
                    entry.key().tag_name.clone(),
                    entry.key().field_name.clone(),
                )
            })
            .collect();
        for (sid, tag, field) in keys {
            self.drop_vector_index(sid, &tag, &field).await?;
        }
        Ok(())
    }

    /// Drop every logical vector index bound to one tag (tag-level cascade).
    pub async fn drop_tag_indexes(
        &self,
        space_id: u64,
        tag_name: &str,
    ) -> VectorCoordinatorResult<()> {
        let keys: Vec<(u64, String, String)> = self
            .logical_indexes
            .iter()
            .filter(|entry| entry.key().space_id == space_id && entry.key().tag_name == tag_name)
            .map(|entry| {
                (
                    entry.key().space_id,
                    entry.key().tag_name.clone(),
                    entry.key().field_name.clone(),
                )
            })
            .collect();
        for (sid, tag, field) in keys {
            self.drop_vector_index(sid, &tag, &field).await?;
        }
        Ok(())
    }

    /// Purge all points of one logical vector index without dropping the
    /// logical registration. Field granularity recreates the physical
    /// collection with the stored config; space granularity deletes the
    /// `group_id` slice so sibling indexes in the shared collection survive.
    ///
    /// Serves explicit operator clears only (`force = true`): rebuilds build
    /// a temp collection and swap it in, and never call this.
    pub async fn purge_index_data(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> VectorCoordinatorResult<()> {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        // Mutual exclusion with online rebuild: refuse while a rebuild
        // scratch collection is registered, otherwise serialize the purge
        // under the publish fence write guard so no delivery apply straddles
        // the truncation.
        if self.has_rebuild_temp(space_id, tag_name, field_name) {
            return Err(VectorCoordinatorError::RebuildBusy(format!(
                "Refusing to purge vector index {space_id}.{tag_name}.{field_name} while a rebuild is running"
            )));
        }
        let fence = self.publish_fence_for(space_id, tag_name, field_name);
        let _fence_guard = fence.write().await;
        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        let logical_key = loc.clone();
        let Some(meta) = self
            .logical_indexes
            .get(&logical_key)
            .map(|entry| entry.clone())
        else {
            return Err(VectorCoordinatorError::FieldNotIndexed {
                tag_name: tag_name.to_string(),
                field_name: field_name.to_string(),
            });
        };
        let collection_name = self.collection_name_for(&loc);
        if self.granularity() == CollectionGranularity::Field {
            if self.backend.index_exists(&collection_name) {
                self.backend.delete_collection(&collection_name).await?;
            }
            self.backend
                .create_index(&collection_name, &meta.config)
                .await?;
        } else {
            let Some(group_id) = self.group_id_for(&loc) else {
                return Err(VectorCoordinatorError::Vector(VectorError::Internal(format!(
                    "space-granularity vector index {space_id}.{tag_name}.{field_name} has no group_id"
                ))));
            };
            self.backend
                .delete_by_filter(
                    &collection_name,
                    VectorFilter::new().must(FilterCondition::match_value("group_id", group_id)),
                )
                .await?;
            if let Err(error) = self
                .backend
                .create_payload_index(&collection_name, "group_id", PayloadSchemaType::Keyword)
                .await
            {
                tracing::warn!(
                    "Failed to ensure payload index for group_id in collection '{}': {}",
                    collection_name,
                    error
                );
            }
        }
        Ok(())
    }

    // ── Rebuild temps (temp-swap publishes) ─────────────────────────────

    /// Temp owner suffix distinguishing sibling Space-granularity slices.
    fn temp_owner_suffix(tag_name: &str, field_name: &str) -> String {
        format!("{tag_name}_{field_name}")
    }

    /// Temp collection name for one rebuild generation. Field granularity
    /// uses `{live}.rebuild-{gen}`; Space appends the logical owner so
    /// sibling slices never share a temp.
    pub fn temp_name_for(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        generation: u64,
    ) -> String {
        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        let live = self.live_collection_name(&loc);
        let owner = (self.granularity() == CollectionGranularity::Space)
            .then(|| Self::temp_owner_suffix(tag_name, field_name));
        VectorBackend::temp_collection_name(&live, generation, owner.as_deref())
    }

    /// Create and register the scratch collection for one rebuild generation.
    ///
    /// The config is snapshotted from the live logical registration and
    /// normalized through `effective_collection_config`, never rebuilt from
    /// request defaults, so sibling conflict detection keeps its semantics.
    /// A leftover temp of the same name (crashed retry of the same
    /// generation) is dropped first. Live data is untouched.
    pub async fn create_temp_collection(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        generation: u64,
    ) -> VectorCoordinatorResult<String> {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        let Some(meta) = self.logical_indexes.get(&loc).map(|entry| entry.clone()) else {
            return Err(VectorCoordinatorError::FieldNotIndexed {
                tag_name: tag_name.to_string(),
                field_name: field_name.to_string(),
            });
        };
        let live = self.live_collection_name(&loc);
        let temp = self.temp_name_for(space_id, tag_name, field_name, generation);
        let config = self.backend.effective_collection_config(&meta.config);
        self.backend.drop_temp_collection(&temp).await?;
        self.backend.create_temp_collection(&temp, &config).await?;
        if self.granularity() == CollectionGranularity::Space {
            if let Err(error) = self
                .backend
                .create_payload_index(&temp, "group_id", PayloadSchemaType::Keyword)
                .await
            {
                tracing::warn!(
                    "Failed to create payload index for group_id in temp collection '{}': {}",
                    temp,
                    error
                );
            }
        }
        self.temp_collections.insert(
            (
                space_id,
                tag_name.to_string(),
                field_name.to_string(),
                generation,
            ),
            TempCollection {
                temp_name: temp.clone(),
                live_name: live,
                config,
                created_at: chrono::Utc::now(),
            },
        );
        Ok(temp)
    }

    /// Look up the registered temp for one rebuild generation.
    pub fn get_temp(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        generation: u64,
    ) -> Option<TempCollection> {
        self.temp_collections
            .get(&(
                space_id,
                tag_name.to_string(),
                field_name.to_string(),
                generation,
            ))
            .map(|entry| entry.clone())
    }

    /// All registered temps (for diagnostics and recovery).
    pub fn list_temps(&self) -> Vec<((u64, String, String, u64), TempCollection)> {
        self.temp_collections
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }

    /// Write target for rebuild backfill/replay: the registered temp, or the
    /// live collection when no temp is registered. The query path never calls
    /// this; reads always resolve through `collection_name_for`.
    pub fn resolve_write_target(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        generation: u64,
    ) -> String {
        if let Some(temp) = self.get_temp(space_id, tag_name, field_name, generation) {
            return temp.temp_name;
        }
        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        self.collection_name_for(&loc)
    }

    /// Discard one registered temp: drop the physical collection (idempotent)
    /// and unregister it. Live data is untouched. Unregistered names are a
    /// no-op (never fall back to deleting by derived name: after a Field
    /// publish the temp name may serve live traffic).
    pub async fn drop_temp_collection(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        generation: u64,
    ) -> VectorCoordinatorResult<()> {
        let key = (
            space_id,
            tag_name.to_string(),
            field_name.to_string(),
            generation,
        );
        let Some((_, temp)) = self.temp_collections.remove(&key) else {
            return Ok(());
        };
        self.backend.drop_temp_collection(&temp.temp_name).await
    }

    /// Drop a temp by physical name (startup recovery for registry-less
    /// orphans) without touching the registry.
    pub async fn drop_temp_by_name(&self, temp: &str) -> VectorCoordinatorResult<()> {
        self.backend.drop_temp_collection(temp).await
    }

    /// Discard every registered temp of one logical index (index drop path).
    async fn discard_temps_for(&self, space_id: u64, tag_name: &str, field_name: &str) {
        let keys: Vec<TempKey> = self
            .temp_collections
            .iter()
            .filter(|entry| {
                entry.key().0 == space_id
                    && entry.key().1 == tag_name
                    && entry.key().2 == field_name
            })
            .map(|entry| entry.key().clone())
            .collect();
        for key in keys {
            if let Some((_, temp)) = self.temp_collections.remove(&key) {
                if let Err(error) = self.backend.drop_temp_collection(&temp.temp_name).await {
                    tracing::warn!(
                        "Failed to discard rebuild temp '{}': {}",
                        temp.temp_name,
                        error
                    );
                }
            }
        }
    }

    /// Override the resolved collection for one logical index (Qdrant
    /// publish remap) or clear it back to the derived live name.
    pub fn set_collection_override(&self, loc: &VectorIndexLocation, target: String) {
        self.collection_overrides.insert(loc.clone(), target);
    }

    /// Point the stored logical registration at another physical collection
    /// (Qdrant publish remap / restart re-adoption). Routing follows the
    /// override map; this keeps `index_info` consistent with it.
    pub fn set_logical_collection_name(&self, loc: &VectorIndexLocation, target: String) {
        if let Some(mut meta) = self.logical_indexes.get_mut(loc) {
            meta.name = target;
        }
    }

    /// Clear a publish remap override.
    pub fn clear_collection_override(&self, loc: &VectorIndexLocation) {
        self.collection_overrides.remove(loc);
    }

    /// Retained pre-publish collection for one derived live name, if any.
    pub fn promote_backup_for(&self, live: &str) -> Option<String> {
        self.promote_backups.get(live).map(|entry| entry.clone())
    }

    /// Publish one temp collection. Must run under the per-index publish
    /// fence write guard; callers activate the generation afterwards, after
    /// which failure is no longer an option.
    ///
    /// Field granularity switches collection pointers without copying:
    /// local backends rename directories (logical mapping unchanged), the
    /// remote backend remaps the logical pointer to the temp. Space
    /// granularity copies the temp slice back into the shared live
    /// collection (`delete_by_filter` + chunked `scroll`/`upsert_batch`),
    /// so sibling `group_id` slices are never disturbed.
    pub async fn publish_temp_collection(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        generation: u64,
        copy_batch: usize,
    ) -> VectorCoordinatorResult<PublishOutcome> {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        let Some(temp) = self.get_temp(space_id, tag_name, field_name, generation) else {
            return Err(VectorCoordinatorError::Vector(VectorError::Internal(
                format!("no rebuild temp registered for {space_id}.{tag_name}.{field_name} generation {generation}"),
            )));
        };
        if self.granularity() == CollectionGranularity::Field {
            return self.publish_temp_field(&loc, &temp, generation).await;
        }
        self.publish_temp_slice(&loc, &temp, generation, copy_batch)
            .await
    }

    /// Field-granularity publish: collection pointer switch.
    async fn publish_temp_field(
        &self,
        loc: &VectorIndexLocation,
        temp: &TempCollection,
        generation: u64,
    ) -> VectorCoordinatorResult<PublishOutcome> {
        if self.backend.is_local() {
            let backup_suffix = VectorBackend::promote_backup_suffix(generation);
            self.backend
                .promote_temp_collection(&temp.live_name, &temp.temp_name, &backup_suffix)
                .await?;
            let backup = format!("{}{}", temp.live_name, backup_suffix);
            self.promote_backups
                .insert(temp.live_name.clone(), backup.clone());
            self.temp_collections.remove(&(
                loc.space_id,
                loc.tag_name.clone(),
                loc.field_name.clone(),
                generation,
            ));
            return Ok(PublishOutcome {
                backup: Some(backup),
                copied_points: 0,
            });
        }
        // Remote backend: switch the logical pointer to the built temp. The
        // previous live collection is retained as the one backup generation.
        let previous = self.collection_name_for(loc);
        if let Some(mut meta) = self.logical_indexes.get_mut(loc) {
            meta.name = temp.temp_name.clone();
        }
        self.collection_overrides
            .insert(loc.clone(), temp.temp_name.clone());
        let rate_backup = if previous != temp.temp_name {
            if VectorBackend::is_temp_collection(&previous) {
                if let Some((_, older)) = self.promote_backups.remove(&temp.live_name) {
                    if older != previous && self.backend.index_exists(&older) {
                        if let Err(error) = self.backend.delete_collection(&older).await {
                            tracing::warn!(
                                "Failed to prune superseded promote backup '{}': {}",
                                older,
                                error
                            );
                        }
                    }
                }
            }
            self.promote_backups
                .insert(temp.live_name.clone(), previous.clone());
            Some(previous)
        } else {
            None
        };
        self.temp_collections.remove(&(
            loc.space_id,
            loc.tag_name.clone(),
            loc.field_name.clone(),
            generation,
        ));
        Ok(PublishOutcome {
            backup: rate_backup,
            copied_points: 0,
        })
    }

    /// Space-granularity publish: copy the temp slice back into the shared
    /// live collection, then drop the temp.
    ///
    /// Crash safety: the live slice is snapshotted into a backup temp
    /// (`{live}.rebuild-{gen}__slicebak`) before the destructive
    /// `delete_by_filter`. If the copy fails afterwards, the backup is
    /// restored over the live slice before returning the error, so a crash
    /// or failure leaves the previous data recoverable instead of a
    /// half-deleted slice. The backup uses the rebuild-temp naming scheme so
    /// startup orphan recovery reclaims it when no rebuild runs.
    async fn publish_temp_slice(
        &self,
        loc: &VectorIndexLocation,
        temp: &TempCollection,
        generation: u64,
        copy_batch: usize,
    ) -> VectorCoordinatorResult<PublishOutcome> {
        let Some(group_id) = self.group_id_for(loc) else {
            return Err(VectorCoordinatorError::Vector(VectorError::Internal(
                format!(
                    "space-granularity vector index {}.{}.{} has no group_id",
                    loc.space_id, loc.tag_name, loc.field_name
                ),
            )));
        };
        let batch = copy_batch.max(1);
        let backup =
            VectorBackend::temp_collection_name(&temp.live_name, generation, Some("slicebak"));
        self.backend.drop_temp_collection(&backup).await?;
        self.backend
            .create_temp_collection(&backup, &temp.config)
            .await?;
        if let Err(error) = self
            .backend
            .create_payload_index(&backup, "group_id", PayloadSchemaType::Keyword)
            .await
        {
            tracing::warn!(
                "Failed to create payload index for group_id in slice backup '{}': {}",
                backup,
                error
            );
        }
        if let Err(error) = self
            .copy_group_slice(&temp.live_name, &backup, &group_id, batch)
            .await
        {
            self.backend.drop_temp_collection(&backup).await.ok();
            return Err(VectorCoordinatorError::Vector(VectorError::Internal(
                format!("space-granularity publish backup of slice '{group_id}' failed: {error}"),
            )));
        }
        self.backend
            .delete_by_filter(
                &temp.live_name,
                VectorFilter::new()
                    .must(FilterCondition::match_value("group_id", group_id.clone())),
            )
            .await?;
        let copied = match self
            .copy_group_slice(&temp.temp_name, &temp.live_name, &group_id, batch)
            .await
        {
            Ok(copied) => copied,
            Err(error) => {
                tracing::warn!(
                    "Space-granularity publish copy failed; restoring slice '{group_id}' from backup: {error}"
                );
                if let Err(restore) = self
                    .copy_group_slice(&backup, &temp.live_name, &group_id, batch)
                    .await
                {
                    tracing::warn!(
                        "Failed to restore space-granularity slice '{group_id}' from backup '{}': {}",
                        backup,
                        restore
                    );
                }
                self.backend.drop_temp_collection(&backup).await.ok();
                return Err(VectorCoordinatorError::Vector(VectorError::Internal(format!(
                    "space-granularity publish copy of slice '{group_id}' failed (backup restored): {error}"
                ))));
            }
        };
        // Temp is fully published: drop both scratch collections. The temp
        // unregister makes the post-publish discard in the rebuild driver a
        // no-op, so a later failure cannot delete serving data.
        self.backend.drop_temp_collection(&temp.temp_name).await?;
        self.backend.drop_temp_collection(&backup).await.ok();
        self.temp_collections.remove(&(
            loc.space_id,
            loc.tag_name.clone(),
            loc.field_name.clone(),
            generation,
        ));
        Ok(PublishOutcome {
            backup: None,
            copied_points: copied,
        })
    }

    /// Copy every point of one `group_id` slice from one collection to
    /// another via chunked `scroll`/`upsert_batch`. The scroll API has no
    /// server-side filter, so points are filtered in memory by their
    /// `group_id` payload (siblings sharing the collection are skipped).
    /// Returns the copied count.
    async fn copy_group_slice(
        &self,
        from: &str,
        to: &str,
        group_id: &str,
        batch: usize,
    ) -> VectorCoordinatorResult<u64> {
        let mut copied = 0u64;
        let mut offset: Option<String> = None;
        loop {
            let (points, next) = self
                .backend
                .scroll(from, batch, offset.as_deref(), Some(true), Some(true))
                .await?;
            let slice_points: Vec<_> = points
                .into_iter()
                .filter(|point| {
                    point.payload.as_ref().is_some_and(|payload| {
                        payload.get("group_id")
                            == Some(&serde_json::Value::String(group_id.to_string()))
                    })
                })
                .collect();
            copied += slice_points.len() as u64;
            if !slice_points.is_empty() {
                self.backend.upsert_batch(to, slice_points).await?;
            }
            offset = next;
            if offset.is_none() {
                break;
            }
        }
        Ok(copied)
    }

    /// Reverse recovery after a bad Field-granularity publish: restore the
    /// retained `.old-*` backup over the live collection (local) or remap
    /// the logical pointer back (remote). Space granularity keeps no backup;
    /// converge by rebuilding instead.
    pub async fn restore_promote_backup(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> VectorCoordinatorResult<String> {
        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        if self.granularity() != CollectionGranularity::Field {
            return Err(VectorCoordinatorError::Vector(VectorError::Internal(
                "space-granularity publishes keep no backup; rebuild to converge".to_string(),
            )));
        }
        let live = self.live_collection_name(&loc);
        if self.backend.is_local() {
            let restored = self.backend.restore_promote_backup(&live).await?;
            self.promote_backups.remove(&live);
            return Ok(restored);
        }
        let Some(backup) = self.promote_backups.get(&live).map(|entry| entry.clone()) else {
            return Err(VectorCoordinatorError::Vector(VectorError::Internal(
                format!("no promote backup retained for {space_id}.{tag_name}.{field_name}"),
            )));
        };
        if !self.backend.index_exists(&backup) {
            return Err(VectorCoordinatorError::Vector(VectorError::Internal(
                format!("promote backup '{backup}' is gone; rebuild to converge"),
            )));
        }
        if let Some(mut meta) = self.logical_indexes.get_mut(&loc) {
            meta.name = backup.clone();
        }
        self.collection_overrides.insert(loc, backup.clone());
        Ok(backup)
    }

    /// Check if index exists (logical index).
    pub fn index_exists(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        self.logical_indexes.contains_key(&logical_key)
    }

    /// Attach a statement-level logical index name to an existing index.
    pub fn set_index_name(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        index_name: &str,
    ) {
        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        if let Some(mut meta) = self.logical_indexes.get_mut(&logical_key) {
            meta.index_name = Some(index_name.to_string());
        }
    }

    /// Get logical index metadata for a tag/field combination.
    pub fn index_info(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<IndexMetadata> {
        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        self.logical_indexes.get(&logical_key).map(|v| v.clone())
    }

    /// List all logical indexes.
    pub fn list_indexes(&self) -> Vec<super::vector_sync::IndexMetadataWrapper> {
        self.logical_indexes
            .iter()
            .map(|pair| {
                let location = pair.key();
                super::vector_sync::IndexMetadataWrapper {
                    collection_name: pair.value().name.clone(),
                    space_id: location.space_id,
                    tag_name: location.tag_name.clone(),
                    field_name: location.field_name.clone(),
                    index_name: pair.value().index_name.clone(),
                }
            })
            .collect()
    }

    /// Register a logical index (for disabled-engine mode or external registration).
    pub fn register_logical_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        collection_name: String,
        config: CollectionConfig,
        user_index_name: Option<String>,
    ) {
        let logical_key = VectorIndexLocation::new(space_id, tag_name, field_name);
        let meta = if let Some(idx_name) = user_index_name {
            IndexMetadata::with_index_name(collection_name, config, idx_name)
        } else {
            IndexMetadata::new(collection_name, config)
        };
        self.logical_indexes.insert(logical_key, meta);
    }

    // ── Search ────────────────────────────────────────────────────────

    /// Search for similar vectors.
    pub async fn search(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let results = self.backend.search(collection, query).await?;
        Ok(results)
    }

    /// Streaming search.
    pub async fn search_stream(
        &self,
        collection: &str,
        query: SearchQuery,
    ) -> VectorCoordinatorResult<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = VectorCoordinatorResult<SearchResult>> + Send>,
        >,
    > {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let stream = self.backend.search_stream(collection, query).await?;
        Ok(stream)
    }

    /// Streaming scroll.
    pub async fn scroll_stream(
        &self,
        collection: &str,
        batch_size: usize,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> VectorCoordinatorResult<
        std::pin::Pin<Box<dyn futures::Stream<Item = VectorCoordinatorResult<VectorPoint>> + Send>>,
    > {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let stream = self
            .backend
            .scroll_stream(collection, batch_size, with_payload, with_vector)
            .await?;
        Ok(stream)
    }

    /// Search with options (the primary search entry point).
    pub async fn search_with_options(
        &self,
        options: SearchOptions,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let loc =
            VectorIndexLocation::new(options.space_id, &options.tag_name, &options.field_name);
        let collection_name = self.collection_name_for(&loc);

        let mut query = SearchQuery::new(options.query_vector, options.limit);

        if let Some(threshold) = options.threshold {
            query = query.with_score_threshold(threshold);
        }

        let mut filter = options.filter.unwrap_or_default();
        if let Some(gid) = self.group_id_for(&loc) {
            filter = filter.must(FilterCondition::match_value("group_id", gid));
        }
        query = query.with_filter(filter);

        let results = self.search(&collection_name, query).await?;
        Ok(results)
    }

    /// Search with space_id and tag/field names.
    pub async fn search_by_location(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        let collection_name = self.collection_name_for(&loc);
        let mut query = SearchQuery::new(query_vector, limit);
        if let Some(gid) = self.group_id_for(&loc) {
            let filter = VectorFilter::new().must(FilterCondition::match_value("group_id", gid));
            query = query.with_filter(filter);
        }
        self.search(&collection_name, query).await
    }

    /// Search with threshold.
    pub async fn search_with_threshold(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
        threshold: f32,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        let collection_name = self.collection_name_for(&loc);
        let mut query = SearchQuery::new(query_vector, limit).with_score_threshold(threshold);
        if let Some(gid) = self.group_id_for(&loc) {
            let filter = VectorFilter::new().must(FilterCondition::match_value("group_id", gid));
            query = query.with_filter(filter);
        }
        self.search(&collection_name, query).await
    }

    /// Search with filter.
    pub async fn search_with_filter(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        if self.is_disabled_engine() {
            return Err(VectorCoordinatorError::EngineDisabled);
        }
        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        let collection_name = self.collection_name_for(&loc);
        let mut enriched = filter;
        if let Some(gid) = self.group_id_for(&loc) {
            enriched = enriched.must(FilterCondition::match_value("group_id", gid));
        }
        let query = SearchQuery::new(query_vector, limit).with_filter(enriched);
        self.search(&collection_name, query).await
    }

    /// Search with threshold and filter.
    pub async fn search_with_threshold_and_filter(
        &self,
        mut options: SearchOptions,
        threshold: f32,
        filter: VectorFilter,
    ) -> VectorCoordinatorResult<Vec<SearchResult>> {
        options.threshold = Some(threshold);
        options.filter = Some(filter);
        self.search_with_options(options).await
    }

    // ── Sync helpers (used by VectorSyncCoordinator) ──────────────────

    /// Prepare a batch forced into one physical collection (rebuild temps).
    /// `group_id` injection follows the context location granularity exactly
    /// as the live path, so Space temps carry the original `group_id` and
    /// copy-back filtering verifies cleanly.
    pub(crate) fn prepare_change_batch_for_collection(
        &self,
        collection_override: Option<&str>,
        contexts: Vec<super::vector_sync::VectorChangeContext>,
    ) -> (
        HashMap<String, Vec<VectorPoint>>,
        HashMap<String, Vec<String>>,
    ) {
        use super::vector_sync::VectorChangeType;

        let mut upsert_by_collection: HashMap<String, Vec<VectorPoint>> = HashMap::new();
        let mut delete_by_collection: HashMap<String, Vec<String>> = HashMap::new();

        for ctx in contexts {
            let collection_name = collection_override
                .map(str::to_string)
                .unwrap_or_else(|| self.collection_name_for(&ctx.location));
            let point_id = ctx.data.id.to_string();

            match ctx.change_type {
                VectorChangeType::Insert => {
                    let vector = ctx.data.vector;
                    let mut json_payload: HashMap<String, serde_json::Value> = ctx
                        .data
                        .payload
                        .into_iter()
                        .filter_map(|(k, v)| serde_json::to_value(&v).ok().map(|json| (k, json)))
                        .collect();

                    if let Some(gid) = self.group_id_for(&ctx.location) {
                        json_payload.insert(
                            "group_id".to_string(),
                            serde_json::to_value(gid).unwrap_or(serde_json::Value::Null),
                        );
                    }

                    let point = VectorPoint::new(point_id, vector).with_payload(json_payload);

                    upsert_by_collection
                        .entry(collection_name)
                        .or_default()
                        .push(point);
                }
                VectorChangeType::Delete => {
                    delete_by_collection
                        .entry(collection_name)
                        .or_default()
                        .push(point_id);
                }
            }
        }

        (upsert_by_collection, delete_by_collection)
    }
}
