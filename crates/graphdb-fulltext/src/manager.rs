use dashmap::DashMap;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;

use crate::engine::FulltextSearchEngine;
use crate::error::SearchError;
use crate::index_events::{IndexEvent, IndexEventCallback};
pub use crate::index_events::{RebuildPhase, RebuildProgress};
use crate::metadata::{IndexKey, IndexMetadata, IndexStatus};
use crate::metrics::MetricsSearchEngine;
use crate::result::{IndexStats, SearchResult};
use crate::tantivy_index::TantivySearchEngine;
use crate::ConsistencyState;
use graphdb_config::fulltext::{FulltextConfig, FulltextEngineType as EngineType};
use graphdb_core::event_dispatch::{EventFilter, EventSubscriptions, SubscriptionId};
use graphdb_core::metadata::SchemaManager;
use graphdb_metrics::StatsManager;

const METADATA_FILE_NAME: &str = "fulltext_metadata.json";
const INDEX_KEY_FILE_NAME: &str = "fulltext_key.json";

pub struct FulltextIndexManager {
    engines: DashMap<IndexKey, Arc<dyn FulltextSearchEngine>>,
    metadata: DashMap<IndexKey, IndexMetadata>,
    base_path: PathBuf,
    #[cfg(feature = "fulltext")]
    default_engine: EngineType,
    #[cfg(feature = "fulltext")]
    config: FulltextConfig,
    schema_manager: Option<Arc<SchemaManager>>,
    stats_manager: Mutex<Option<Arc<StatsManager>>>,
    index_callbacks: Arc<EventSubscriptions<IndexEvent>>,
    /// Per-index publish fence for online rebuild. Delivery paths hold the
    /// read guard while applying a batch; the rebuild publish phase holds
    /// the write guard across final catch-up replay and engine swap, so a
    /// commit is either fully before or fully after the swap.
    publish_fences: DashMap<IndexKey, Arc<tokio::sync::RwLock<()>>>,
    /// Live rebuild progress by index; present only while a rebuild runs
    /// (FAILED/COMPLETED retained until cleared by the next rebuild or startup).
    rebuild_progress: DashMap<IndexKey, RebuildProgress>,
}

impl std::fmt::Debug for FulltextIndexManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FulltextIndexManager")
            .field("engines_count", &self.engines.len())
            .field("index_callbacks", &self.index_callbacks.len())
            .finish()
    }
}

impl FulltextIndexManager {
    pub fn new(config: FulltextConfig) -> Result<Self, SearchError> {
        let base_path = config.index_path.clone();

        if !base_path.exists() {
            std::fs::create_dir_all(&base_path)?;
        }

        let manager = Self {
            engines: DashMap::new(),
            metadata: DashMap::new(),
            base_path,
            #[cfg(feature = "fulltext")]
            default_engine: config.default_engine,
            #[cfg(feature = "fulltext")]
            config,
            schema_manager: None,
            stats_manager: Mutex::new(None),
            index_callbacks: Arc::new(EventSubscriptions::new()),
            publish_fences: DashMap::new(),
            rebuild_progress: DashMap::new(),
        };

        manager.discover_existing_indexes()?;

        Ok(manager)
    }

    /// Shared index-event registry for the central `HookBus`.
    pub fn shared_index_callbacks(&self) -> Arc<EventSubscriptions<IndexEvent>> {
        Arc::clone(&self.index_callbacks)
    }

    /// Register a runtime observer for index lifecycle events.
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

    /// Number of registered index observers.
    pub fn index_callback_count(&self) -> usize {
        self.index_callbacks.len()
    }

    fn emit_index_event(&self, event: IndexEvent) {
        if self.index_callbacks.is_empty() {
            return;
        }
        self.index_callbacks.dispatch("index", &event);
    }

    fn discover_existing_indexes(&self) -> Result<(), SearchError> {
        #[cfg(feature = "fulltext")]
        if let Ok(loaded) = self.load_metadata_from_file() {
            for metadata in loaded {
                if self.restore_index_from_metadata(&metadata).is_ok() {
                    tracing::debug!(
                        index_id = %metadata.index_id,
                        "Restored index from metadata"
                    );
                }
            }
            return Ok(());
        }

        #[cfg(feature = "fulltext")]
        return self.discover_indexes_from_disk();

        #[cfg(not(feature = "fulltext"))]
        Ok(())
    }

    #[cfg(feature = "fulltext")]
    fn discover_indexes_from_disk(&self) -> Result<(), SearchError> {
        let entries = match std::fs::read_dir(&self.base_path) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!("Failed to read base path: {}", e);
                return Ok(());
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();

            // Rebuild scratch and backup directories are never live indexes.
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.contains(".rebuild-") || name.contains(".old-") {
                    continue;
                }
            }

            if path.is_dir() && path.join("meta.json").exists() {
                if let Some((key, engine, metadata)) = self.try_restore_bm25_index(&path) {
                    self.engines.insert(key.clone(), engine);
                    self.metadata.insert(key, metadata);
                }
            }
        }

        if !self.metadata.is_empty() {
            if let Err(e) = self.save_metadata_to_file() {
                tracing::warn!("Failed to save metadata cache: {}", e);
            }
        }

        Ok(())
    }

    #[cfg(feature = "fulltext")]
    fn try_restore_bm25_index(
        &self,
        path: &std::path::Path,
    ) -> Option<(IndexKey, Arc<dyn FulltextSearchEngine>, IndexMetadata)> {
        if let Some((space_id, tag_name, field_name)) = Self::read_key_sidecar(path) {
            let engine =
                TantivySearchEngine::open_or_create(path, self.config.tantivy.clone()).ok()?;

            let engine: Arc<dyn FulltextSearchEngine> = Arc::new(engine);
            let key = IndexKey::new(space_id, &tag_name, &field_name);
            let metadata = IndexMetadata {
                index_id: key.to_index_id(),
                index_name: format!("idx_{}_{}_{}", space_id, tag_name, field_name),
                space_id,
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                engine_type: EngineType::Bm25,
                storage_path: path.to_string_lossy().to_string(),
                created_at: chrono::Utc::now(),
                last_updated: chrono::Utc::now(),
                doc_count: 0,
                status: IndexStatus::Active,
                engine_config: None,
            };

            return Some((key, engine, metadata));
        }

        let dir_name = path.file_name()?.to_string_lossy();
        let (space_id, tag_name, field_name) = self.parse_index_id(&dir_name)?;

        let engine = TantivySearchEngine::open_or_create(
            &self.base_path.join(&*dir_name),
            self.config.tantivy.clone(),
        )
        .ok()?;

        let engine: Arc<dyn FulltextSearchEngine> = Arc::new(engine);
        let key = IndexKey::new(space_id, &tag_name, &field_name);
        let metadata = IndexMetadata {
            index_id: dir_name.to_string(),
            index_name: format!("idx_{}_{}_{}", space_id, tag_name, field_name),
            space_id,
            tag_name: tag_name.clone(),
            field_name: field_name.clone(),
            engine_type: EngineType::Bm25,
            storage_path: path.to_string_lossy().to_string(),
            created_at: chrono::Utc::now(),
            last_updated: chrono::Utc::now(),
            doc_count: 0,
            status: IndexStatus::Active,
            engine_config: None,
        };

        Some((key, engine, metadata))
    }

    /// Legacy directory-name fallback. The `space_ft_{space}_{tag}_{field}`
    /// form is ambiguous when tag/field names contain `_`, so new indexes
    /// write a `fulltext_key.json` sidecar (authoritative) and this parser
    /// only accepts the unambiguous five-segment form.
    #[cfg(feature = "fulltext")]
    fn parse_index_id(&self, index_id: &str) -> Option<(u64, String, String)> {
        let parts: Vec<&str> = index_id.split('_').collect();
        if parts.len() != 5 || parts[0] != "space" || parts[1] != "ft" {
            return None;
        }

        let space_id: u64 = parts[2].parse().ok()?;
        let tag_name = parts[3].to_string();
        let field_name = parts[4].to_string();

        Some((space_id, tag_name, field_name))
    }

    #[cfg(feature = "fulltext")]
    fn read_key_sidecar(path: &std::path::Path) -> Option<(u64, String, String)> {
        let content = std::fs::read_to_string(path.join(INDEX_KEY_FILE_NAME)).ok()?;
        let value: serde_json::Value = serde_json::from_str(&content).ok()?;
        Some((
            value.get("space_id")?.as_u64()?,
            value.get("tag_name")?.as_str()?.to_string(),
            value.get("field_name")?.as_str()?.to_string(),
        ))
    }

    #[cfg(feature = "fulltext")]
    fn write_key_sidecar(path: &std::path::Path, space_id: u64, tag: &str, field: &str) {
        let value = serde_json::json!({
            "space_id": space_id,
            "tag_name": tag,
            "field_name": field,
        });
        if let Ok(content) = serde_json::to_string_pretty(&value) {
            if let Err(e) = std::fs::write(path.join(INDEX_KEY_FILE_NAME), content) {
                tracing::warn!("Failed to write fulltext key sidecar: {}", e);
            }
        }
    }

    #[cfg(feature = "fulltext")]
    fn restore_index_from_metadata(&self, metadata: &IndexMetadata) -> Result<(), SearchError> {
        let key = IndexKey::new(metadata.space_id, &metadata.tag_name, &metadata.field_name);

        let stored = std::path::PathBuf::from(&metadata.storage_path);
        let index_path = if stored.is_absolute() || stored.exists() {
            stored
        } else {
            self.base_path.join(&metadata.index_id)
        };
        let engine = TantivySearchEngine::open_or_create(&index_path, self.config.tantivy.clone())?;

        self.engines.insert(
            key.clone(),
            Arc::new(engine) as Arc<dyn FulltextSearchEngine>,
        );
        self.metadata.insert(key, metadata.clone());

        Ok(())
    }

    #[cfg(feature = "fulltext")]
    fn load_metadata_from_file(&self) -> Result<Vec<IndexMetadata>, SearchError> {
        let metadata_path = self.base_path.join(METADATA_FILE_NAME);

        if !metadata_path.exists() {
            return Err(SearchError::Internal("Metadata file not found".to_string()));
        }

        let content = std::fs::read_to_string(&metadata_path)?;
        let metadata_list: Vec<IndexMetadata> = serde_json::from_str(&content)
            .map_err(|e| SearchError::SerializationError(e.to_string()))?;

        Ok(metadata_list)
    }

    fn save_metadata_to_file(&self) -> Result<(), SearchError> {
        let metadata_path = self.base_path.join(METADATA_FILE_NAME);

        let metadata_list: Vec<IndexMetadata> = self
            .metadata
            .iter()
            .map(|entry| entry.value().clone())
            .collect();

        let content = serde_json::to_string_pretty(&metadata_list)
            .map_err(|e| SearchError::SerializationError(e.to_string()))?;

        // Atomic write: write to a temp file then rename, so a crash mid-write
        // never leaves a corrupt metadata file.
        let tmp_path = metadata_path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &content)?;
        std::fs::rename(&tmp_path, &metadata_path)?;

        Ok(())
    }

    pub fn with_schema_manager(mut self, schema_manager: Arc<SchemaManager>) -> Self {
        self.schema_manager = Some(schema_manager);
        self
    }

    pub fn with_stats_manager(mut self, stats_manager: Arc<StatsManager>) -> Self {
        *self.stats_manager.get_mut() = Some(stats_manager);
        self
    }

    pub fn set_schema_manager(&mut self, schema_manager: Arc<SchemaManager>) {
        self.schema_manager = Some(schema_manager);
    }

    pub fn set_stats_manager(&self, stats_manager: Arc<StatsManager>) {
        *self.stats_manager.lock() = Some(stats_manager);
    }

    fn validate_space_exists(&self, space_id: u64) -> Result<(), SearchError> {
        if let Some(ref schema_manager) = self.schema_manager {
            let space_exists = schema_manager
                .get_space_by_id(space_id)
                .map_err(|e| SearchError::Internal(format!("Failed to validate space: {}", e)))?
                .is_some();

            if !space_exists {
                return Err(SearchError::SpaceNotFound(space_id));
            }
        }
        Ok(())
    }

    fn validate_tag_exists(&self, space_id: u64, tag_name: &str) -> Result<(), SearchError> {
        if let Some(ref schema_manager) = self.schema_manager {
            let space = schema_manager
                .get_space_by_id(space_id)
                .map_err(|e| SearchError::Internal(format!("Failed to validate tag: {}", e)))?;

            if let Some(space_info) = space {
                let tag_exists = space_info.tags.iter().any(|t| t.tag_name == tag_name);
                if !tag_exists {
                    return Err(SearchError::TagNotFound(format!(
                        "{}.{}",
                        space_id, tag_name
                    )));
                }
            }
        }
        Ok(())
    }

    #[cfg(feature = "fulltext")]
    fn get_space_storage_path(&self, space_id: u64) -> Result<PathBuf, SearchError> {
        if let Some(ref schema_manager) = self.schema_manager {
            if let Some(space_info) = schema_manager
                .get_space_by_id(space_id)
                .map_err(|e| SearchError::Internal(format!("Failed to get space: {}", e)))?
            {
                if let Some(ref custom_path) = space_info.storage_path {
                    let fulltext_path = custom_path.join("fulltext");
                    if !fulltext_path.exists() {
                        std::fs::create_dir_all(&fulltext_path)?;
                    }
                    return Ok(fulltext_path);
                }

                use graphdb_core::types::space::IsolationLevel;
                match space_info.isolation_level {
                    IsolationLevel::Directory => {
                        let space_path = self.base_path.join(format!("space_{}", space_id));
                        if !space_path.exists() {
                            std::fs::create_dir_all(&space_path)?;
                        }
                        return Ok(space_path);
                    }
                    IsolationLevel::Shared | IsolationLevel::Device => {}
                }
            }
        }
        Ok(self.base_path.clone())
    }

    pub async fn create_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        engine_type: Option<EngineType>,
    ) -> Result<String, SearchError> {
        let display_name = format!("idx_{}_{}_{}", space_id, tag_name, field_name);
        self.create_index_with_engine_config(
            space_id,
            tag_name,
            field_name,
            &display_name,
            engine_type,
            None,
        )
        .await
    }

    pub async fn create_index_with_engine_config(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        _user_index_name: &str,
        _engine_type: Option<EngineType>,
        engine_config: Option<serde_json::Value>,
    ) -> Result<String, SearchError> {
        self.validate_space_exists(space_id)?;
        self.validate_tag_exists(space_id, tag_name)?;
        #[cfg(not(feature = "fulltext"))]
        let _ = &engine_config;

        let key = IndexKey::new(space_id, tag_name, field_name);
        let index_id = key.to_index_id();

        if self.engines.contains_key(&key) {
            return Err(SearchError::IndexAlreadyExists(index_id));
        }

        #[cfg(feature = "fulltext")]
        {
            let engine_type = _engine_type.unwrap_or(self.default_engine);
            let storage_path = self.get_space_storage_path(space_id)?;

            self.emit_index_event(IndexEvent::FulltextBuildStarted {
                index_name: index_id.clone(),
            });

            let engine = TantivySearchEngine::open_or_create(
                &storage_path.join(&index_id),
                self.config.tantivy.clone(),
            )?;
            Self::write_key_sidecar(
                &storage_path.join(&index_id),
                space_id,
                tag_name,
                field_name,
            );
            let engine: Arc<dyn FulltextSearchEngine> = Arc::new(engine);

            let metadata = IndexMetadata {
                index_id: index_id.clone(),
                index_name: _user_index_name.to_string(),
                space_id,
                tag_name: tag_name.to_string(),
                field_name: field_name.to_string(),
                engine_type,
                storage_path: storage_path.join(&index_id).to_string_lossy().to_string(),
                created_at: chrono::Utc::now(),
                last_updated: chrono::Utc::now(),
                doc_count: 0,
                status: IndexStatus::Active,
                engine_config,
            };

            self.engines.insert(key.clone(), engine);
            self.metadata.insert(key, metadata);

            if let Err(e) = self.save_metadata_to_file() {
                tracing::warn!("Failed to save metadata after creating index: {}", e);
            }

            self.emit_index_event(IndexEvent::FulltextBuildCompleted {
                index_name: index_id.clone(),
                docs_count: 0,
            });

            Ok(index_id)
        }

        #[cfg(not(feature = "fulltext"))]
        {
            Err(SearchError::EngineUnavailable)
        }
    }

    pub fn get_engine(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<Arc<dyn FulltextSearchEngine>> {
        let key = IndexKey::new(space_id, tag_name, field_name);
        self.engines.get(&key).map(|e| Arc::clone(&*e))
    }

    pub fn get_metrics_engine(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<Arc<MetricsSearchEngine>> {
        let key = IndexKey::new(space_id, tag_name, field_name);
        let engine = self.engines.get(&key)?;
        let stats_manager = self.stats_manager.lock();
        let sm = stats_manager.as_ref()?;
        let index_name = format!("{}_{}_{}", space_id, tag_name, field_name);
        Some(Arc::new(MetricsSearchEngine::new(
            Arc::clone(&*engine),
            Arc::clone(sm),
            space_id,
            index_name,
        )))
    }

    pub fn get_metadata(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<IndexMetadata> {
        let key = IndexKey::new(space_id, tag_name, field_name);
        self.metadata.get(&key).map(|m| m.clone())
    }

    pub fn has_index(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        let key = IndexKey::new(space_id, tag_name, field_name);
        self.engines.contains_key(&key)
    }

    pub fn get_space_indexes(&self, space_id: u64) -> Vec<IndexMetadata> {
        self.metadata
            .iter()
            .filter(|entry| entry.value().space_id == space_id)
            .map(|entry| entry.value().clone())
            .collect()
    }

    pub async fn drop_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<(), SearchError> {
        let key = IndexKey::new(space_id, tag_name, field_name);

        if let Some((_, engine)) = self.engines.remove(&key) {
            engine.close().await?;
        }

        let stored_path = self
            .metadata
            .get(&key)
            .map(|m| std::path::PathBuf::from(&m.storage_path));
        self.metadata.remove(&key);

        if let Err(e) = self.save_metadata_to_file() {
            tracing::warn!("Failed to save metadata after dropping index: {}", e);
        }

        let index_id = key.to_index_id();
        let candidates = stored_path
            .into_iter()
            .chain(std::iter::once(self.base_path.join(&index_id)))
            .collect::<Vec<_>>();
        for index_path in candidates {
            if index_path.exists() {
                tokio::fs::remove_dir_all(&index_path).await?;
            }
        }

        self.emit_index_event(IndexEvent::FulltextDropped {
            index_name: index_id,
        });

        Ok(())
    }

    /// Notify observers that an existing index was refreshed/committed.
    pub fn notify_fulltext_refresh(&self, index_name: String) {
        self.emit_index_event(IndexEvent::FulltextRefresh { index_name });
    }

    /// Notify observers of a merge lifecycle for an index.
    ///
    /// Tantivy merges segments internally without an explicit maintenance
    /// entry point, so this is a manual hook for operators wrapping an
    /// explicit merge/optimize pass.
    pub fn notify_merge_started(&self, index_name: String, segments: usize) {
        self.emit_index_event(IndexEvent::IndexMergeStarted {
            index_name,
            segments,
        });
    }

    /// Notify observers that a merge completed for an index.
    pub fn notify_merge_completed(&self, index_name: String) {
        self.emit_index_event(IndexEvent::IndexMergeCompleted { index_name });
    }

    pub async fn search(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        let engine = self
            .get_engine(space_id, tag_name, field_name)
            .ok_or_else(|| {
                SearchError::IndexNotFound(format!("{}.{}.{}", space_id, tag_name, field_name))
            })?;

        engine.search(query, limit).await
    }

    pub async fn search_structured(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query: &crate::query::FulltextQuery,
        limit: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        let engine = self
            .get_engine(space_id, tag_name, field_name)
            .ok_or_else(|| {
                SearchError::IndexNotFound(format!("{}.{}.{}", space_id, tag_name, field_name))
            })?;

        engine.search_structured(query, limit).await
    }

    pub async fn get_stats(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<IndexStats, SearchError> {
        let engine = self
            .get_engine(space_id, tag_name, field_name)
            .ok_or_else(|| {
                SearchError::IndexNotFound(format!("{}.{}.{}", space_id, tag_name, field_name))
            })?;

        engine.stats().await
    }

    pub async fn commit_all(&self) -> Result<(), SearchError> {
        let keys: Vec<IndexKey> = self
            .engines
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        for key in &keys {
            if let Some(engine) = self.engines.get(key) {
                engine.value().commit().await?;
                self.emit_index_event(IndexEvent::FulltextRefresh {
                    index_name: key.to_index_id(),
                });
            }
        }
        Ok(())
    }

    pub async fn close_all(&self) -> Result<(), SearchError> {
        for entry in self.engines.iter() {
            entry.value().close().await?;
        }
        self.engines.clear();
        self.metadata.clear();
        Ok(())
    }

    pub fn list_indexes(&self) -> Vec<IndexMetadata> {
        self.metadata
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    pub async fn index_edge_property(
        &self,
        space_id: u64,
        edge_type: &str,
        field_name: &str,
        doc_id: &str,
        text: &str,
    ) -> Result<(), SearchError> {
        let key = IndexKey::new(space_id, edge_type, field_name);

        if let Some(engine) = self.engines.get(&key) {
            engine.index(doc_id, text).await?;
        }
        Ok(())
    }

    pub async fn delete_edge_index(
        &self,
        space_id: u64,
        edge_type: &str,
        doc_id: &str,
    ) -> Result<(), SearchError> {
        let edge_indexes: Vec<_> = self
            .metadata
            .iter()
            .filter(|entry| {
                entry.value().space_id == space_id && entry.value().tag_name == edge_type
            })
            .map(|entry| entry.key().clone())
            .collect();

        for key in edge_indexes {
            if let Some(engine) = self.engines.get(&key) {
                engine.delete(doc_id).await.ok();
            }
        }

        Ok(())
    }

    /// Indexes needing operator attention: engines in the `Inconsistent`
    /// state plus metadata stuck in `Error` (failed/discarded rebuilds whose
    /// previous data still serves). Both must be empty for a healthy index.
    pub fn get_inconsistent_indexes(&self) -> Vec<IndexMetadata> {
        self.metadata
            .iter()
            .filter(|entry| {
                entry.value().status == IndexStatus::Error
                    || self
                        .engines
                        .get(entry.key())
                        .is_some_and(|e| e.consistency_state() == ConsistencyState::Inconsistent)
            })
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Destructively clear a fulltext index: drop all documents and reset counters.
    ///
    /// This is an explicit destructive operation, not a rebuild: nothing is
    /// read back from primary storage, so indexed data is lost. The cleared
    /// index is empty and consistent, and subsequent writes proceed normally.
    /// To repair an inconsistent index without data loss, use the sync-side
    /// online rebuild driver (`SyncManager::rebuild_fulltext_index`) or drop
    /// and recreate the index. The `force` flag mirrors the vector
    /// `purge_index_data` guard and prevents accidental calls.
    pub async fn clear_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        force: bool,
    ) -> Result<(), SearchError> {
        if !force {
            return Err(SearchError::Internal(
                "Refusing to clear fulltext index without explicit confirmation: retry with force = true".to_string(),
            ));
        }
        tracing::warn!(
            "Clearing fulltext index explicitly confirmed: space {} tag {} field {}",
            space_id,
            tag_name,
            field_name
        );
        // Mutual exclusion with online rebuild and live delivery: refuse
        // while a rebuild holds the index, otherwise serialize the clear
        // under the publish fence write guard so no delivery apply straddles
        // the truncation.
        if let Some(progress) = self.rebuild_progress(space_id, tag_name, field_name) {
            if matches!(
                progress.phase,
                RebuildPhase::Preparing
                    | RebuildPhase::Backfilling
                    | RebuildPhase::CatchingUp
                    | RebuildPhase::Publishing
            ) {
                return Err(SearchError::RebuildBusy(format!(
                    "Refusing to clear fulltext index {space_id}.{tag_name}.{field_name} while a rebuild is {}",
                    progress.phase.as_str(),
                )));
            }
        }
        let fence = self.publish_fence_for(space_id, tag_name, field_name);
        let _fence_guard = fence.write().await;
        let key = IndexKey::new(space_id, tag_name, field_name);
        let engine = self.engines.get(&key).ok_or_else(|| {
            SearchError::IndexNotFound(format!("{}.{}.{}", space_id, tag_name, field_name))
        })?;

        engine.clear().await?;
        engine.mark_consistent();

        if let Some(mut metadata) = self.metadata.get_mut(&key) {
            metadata.last_updated = chrono::Utc::now();
            metadata.doc_count = 0;
            metadata.status = IndexStatus::Active;
        }

        self.save_metadata_to_file().map_err(|e| {
            tracing::warn!("Failed to save metadata after clearing index: {}", e);
            e
        })?;

        tracing::info!(
            "Cleared index {}.{}.{} - emptied and marked consistent",
            space_id,
            tag_name,
            field_name
        );
        Ok(())
    }

    /// Publish fence for an index. Delivery paths hold the read guard while
    /// applying; rebuild publish holds the write guard across final replay
    /// and engine swap.
    pub fn publish_fence_for(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Arc<tokio::sync::RwLock<()>> {
        let key = IndexKey::new(space_id, tag_name, field_name);
        self.publish_fences
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::RwLock::new(())))
            .value()
            .clone()
    }

    /// Current rebuild progress for an index, if a rebuild ran.
    pub fn rebuild_progress(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<RebuildProgress> {
        let key = IndexKey::new(space_id, tag_name, field_name);
        self.rebuild_progress.get(&key).map(|p| p.clone())
    }

    /// Overwrite rebuild progress (used by the rebuild driver).
    pub fn update_rebuild_progress(&self, progress: RebuildProgress) {
        let key = IndexKey::new(progress.space_id, &progress.tag_name, &progress.field_name);
        self.rebuild_progress.insert(key, progress);
    }

    /// Advance the rebuild phase and emit a phase-transition progress event.
    pub fn set_rebuild_phase(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        phase: RebuildPhase,
    ) {
        let key = IndexKey::new(space_id, tag_name, field_name);
        if let Some(mut entry) = self.rebuild_progress.get_mut(&key) {
            entry.phase = phase;
            let progress = entry.clone();
            drop(entry);
            self.emit_index_event(IndexEvent::FulltextRebuildProgress {
                index_name: key.to_index_id(),
                generation: progress.generation,
                phase: phase.as_str().to_string(),
                docs_applied: progress.docs_applied,
            });
        }
    }

    /// Scratch directory for a rebuild generation, next to the live directory
    /// so renames stay on one filesystem.
    fn rebuild_temp_dir(
        &self,
        live_path: &std::path::Path,
        index_id: &str,
        generation: u64,
    ) -> PathBuf {
        let parent = live_path.parent().unwrap_or(&self.base_path);
        parent.join(format!("{}.rebuild-{}", index_id, generation))
    }

    /// Create an empty rebuild engine for a new generation. The live engine
    /// keeps serving reads and writes. Any leftover scratch directory from a
    /// crashed rebuild of the same generation is removed for idempotent retry.
    /// Marks metadata `Rebuilding` and initializes progress tracking.
    #[cfg(feature = "fulltext")]
    pub async fn create_rebuild_engine(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        generation: u64,
    ) -> Result<Arc<dyn FulltextSearchEngine>, SearchError> {
        let key = IndexKey::new(space_id, tag_name, field_name);
        let live_path = self
            .metadata
            .get(&key)
            .map(|m| PathBuf::from(&m.storage_path))
            .ok_or_else(|| {
                SearchError::IndexNotFound(format!("{}.{}.{}", space_id, tag_name, field_name))
            })?;
        if !self.engines.contains_key(&key) {
            return Err(SearchError::IndexNotFound(format!(
                "{}.{}.{}",
                space_id, tag_name, field_name
            )));
        }

        let index_id = key.to_index_id();
        let temp_dir = self.rebuild_temp_dir(&live_path, &index_id, generation);
        if temp_dir.exists() {
            tracing::warn!(
                "Removing leftover rebuild scratch directory {} from a previous attempt",
                temp_dir.display()
            );
            // Async filesystem call: this runs while operators may poll
            // progress, so it must not block the executor.
            tokio::fs::remove_dir_all(&temp_dir).await?;
        }
        tokio::fs::create_dir_all(&temp_dir).await?;

        // No key sidecar on purpose: discovery must never adopt scratch dirs.
        let engine = TantivySearchEngine::open_or_create(&temp_dir, self.config.tantivy.clone())?;
        let engine: Arc<dyn FulltextSearchEngine> = Arc::new(engine);

        if let Some(mut metadata) = self.metadata.get_mut(&key) {
            metadata.status = IndexStatus::Rebuilding;
            metadata.last_updated = chrono::Utc::now();
        }
        self.save_metadata_to_file().map_err(|e| {
            tracing::warn!("Failed to save metadata after starting rebuild: {}", e);
            e
        })?;

        self.update_rebuild_progress(RebuildProgress {
            space_id,
            tag_name: tag_name.to_string(),
            field_name: field_name.to_string(),
            generation,
            phase: RebuildPhase::Preparing,
            docs_scanned: 0,
            docs_applied: 0,
            docs_skipped: 0,
            started_at: chrono::Utc::now(),
        });
        self.emit_index_event(IndexEvent::FulltextRebuildStarted {
            index_name: index_id,
            generation,
        });
        Ok(engine)
    }

    /// Publish a rebuilt engine: swap it into the live map, point metadata at
    /// the scratch directory (which becomes the live directory), and rename
    /// the previous directory aside as `<id>.old-<generation>`. The newest
    /// backup is retained for reverse recovery after a bad publish; older
    /// backups are pruned. Must be called under the publish fence write
    /// guard after final catch-up replay.
    #[cfg(feature = "fulltext")]
    pub async fn publish_rebuilt_engine(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        generation: u64,
        engine: Arc<dyn FulltextSearchEngine>,
    ) -> Result<u64, SearchError> {
        let key = IndexKey::new(space_id, tag_name, field_name);
        let index_id = key.to_index_id();
        let live_dir = engine.stats().await.map(|s| s.doc_count).unwrap_or(0);

        let previous_path = self
            .metadata
            .get(&key)
            .map(|m| m.storage_path.clone())
            .unwrap_or_default();
        let live_path = self
            .rebuild_temp_dir(&PathBuf::from(&previous_path), &index_id, generation)
            .to_string_lossy()
            .to_string();

        let old = self.engines.insert(key.clone(), Arc::clone(&engine));

        if let Some(mut metadata) = self.metadata.get_mut(&key) {
            metadata.storage_path = live_path;
            metadata.doc_count = live_dir;
            metadata.status = IndexStatus::Active;
            metadata.last_updated = chrono::Utc::now();
        }
        self.save_metadata_to_file().map_err(|e| {
            tracing::warn!("Failed to save metadata after publishing rebuild: {}", e);
            e
        })?;

        if let Some(old_engine) = old {
            old_engine.close().await.ok();
        }
        if !previous_path.is_empty()
            && previous_path
                != self
                    .rebuild_temp_dir(&PathBuf::from(&previous_path), &index_id, generation)
                    .to_string_lossy()
                    .to_string()
        {
            let backup = PathBuf::from(format!("{}.old-{}", previous_path, generation));
            if PathBuf::from(&previous_path).exists() {
                // Rename-then-retain keeps a crash window recoverable and
                // preserves one backup generation for reverse recovery after
                // a bad publish. Older backups are pruned below. A rename
                // failure keeps the previous directory beside the new live
                // one: no data is lost, but reverse recovery loses its
                // backup, so it is logged at error level for attention.
                // Async filesystem call: publish runs under the publish
                // fence, so blocking the executor here would stall delivery.
                if tokio::fs::rename(&previous_path, &backup).await.is_err() {
                    tracing::error!(
                        "Published rebuild for {} but failed to retain previous directory {} as backup {}: reverse recovery unavailable until the next successful rebuild",
                        index_id,
                        previous_path,
                        backup.display()
                    );
                }
            }
            Self::prune_old_backups_async(
                PathBuf::from(&previous_path)
                    .parent()
                    .unwrap_or(&self.base_path),
                &index_id,
                Some(&backup),
            )
            .await;
        }

        self.set_rebuild_phase(space_id, tag_name, field_name, RebuildPhase::Completed);
        self.emit_index_event(IndexEvent::FulltextRebuildCompleted {
            index_name: index_id,
            generation,
            docs_count: live_dir as u64,
        });
        Ok(live_dir as u64)
    }

    /// List backup directories `<index_id>.old-<generation>` for one index,
    /// newest generation first (unparseable suffixes sort last).
    fn list_old_backups(parent: &std::path::Path, index_id: &str) -> Vec<PathBuf> {
        let prefix = format!("{}.old-", index_id);
        let mut backups: Vec<(u64, PathBuf)> = Vec::new();
        let entries = match std::fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(suffix) = name.strip_prefix(&prefix) else {
                continue;
            };
            let generation = suffix.parse::<u64>().unwrap_or(0);
            backups.push((generation, path));
        }
        backups.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        backups.into_iter().map(|(_, path)| path).collect()
    }

    /// Prune `<index_id>.old-*` backups in `parent`, keeping only `keep`
    /// (usually the newest backup). Used after publish so exactly one backup
    /// generation survives for reverse recovery. Async filesystem calls keep
    /// the publish fence critical section off the blocking path.
    #[cfg(feature = "fulltext")]
    async fn prune_old_backups_async(
        parent: &std::path::Path,
        index_id: &str,
        keep: Option<&PathBuf>,
    ) {
        for backup in Self::list_old_backups(parent, index_id) {
            if keep.is_some_and(|keep| *keep == backup) {
                continue;
            }
            if let Err(e) = tokio::fs::remove_dir_all(&backup).await {
                tracing::warn!("Failed to prune rebuild backup {}: {}", backup.display(), e);
            }
        }
    }

    /// Reverse recovery after a bad publish: repoint the index at its newest
    /// `.old-<generation>` backup and reopen the engine from it. The suspect
    /// live directory is renamed aside as `.suspect-<millis>` for inspection.
    /// Metadata is marked `Error` so the operator runs a fresh rebuild.
    #[cfg(feature = "fulltext")]
    pub async fn restore_rebuild_backup(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<PathBuf, SearchError> {
        let key = IndexKey::new(space_id, tag_name, field_name);
        let index_id = key.to_index_id();
        let live_path = self
            .metadata
            .get(&key)
            .map(|m| PathBuf::from(&m.storage_path))
            .ok_or_else(|| {
                SearchError::IndexNotFound(format!("{}.{}.{}", space_id, tag_name, field_name))
            })?;
        let parent = live_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.base_path.clone());
        let backup = Self::list_old_backups(&parent, &index_id)
            .into_iter()
            .next()
            .ok_or_else(|| {
                SearchError::IndexNotFound(format!(
                    "No rebuild backup available for {}.{}.{}",
                    space_id, tag_name, field_name
                ))
            })?;

        if let Some((_, live_engine)) = self.engines.remove(&key) {
            live_engine.close().await.ok();
        }
        if live_path.exists() {
            let suspect = parent.join(format!(
                "{}.suspect-{}",
                index_id,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
            ));
            if tokio::fs::rename(&live_path, &suspect).await.is_err() {
                tracing::warn!(
                    "Restoring backup for {} but failed to move aside suspect directory {}",
                    index_id,
                    live_path.display()
                );
            }
        }

        let engine = TantivySearchEngine::open_or_create(&backup, self.config.tantivy.clone())?;
        self.engines.insert(
            key.clone(),
            Arc::new(engine) as Arc<dyn FulltextSearchEngine>,
        );
        if let Some(mut metadata) = self.metadata.get_mut(&key) {
            metadata.storage_path = backup.to_string_lossy().to_string();
            metadata.status = IndexStatus::Error;
            metadata.last_updated = chrono::Utc::now();
        }
        if let Err(e) = self.save_metadata_to_file() {
            tracing::warn!("Failed to save metadata after restoring backup: {}", e);
        }
        tracing::warn!(
            "Restored index {} from rebuild backup {}; marked Error pending a fresh rebuild",
            index_id,
            backup.display()
        );
        Ok(backup)
    }

    /// Discard a failed rebuild: remove the scratch directory, mark metadata
    /// `Error` (the previous engine keeps serving), record failure progress.
    #[cfg(feature = "fulltext")]
    pub async fn discard_rebuild_engine(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        generation: u64,
        reason: &str,
    ) {
        let key = IndexKey::new(space_id, tag_name, field_name);
        let index_id = key.to_index_id();
        if let Some(metadata) = self.metadata.get(&key) {
            let temp_dir = self.rebuild_temp_dir(
                &PathBuf::from(&metadata.storage_path),
                &index_id,
                generation,
            );
            if temp_dir.exists() && PathBuf::from(&metadata.storage_path) != temp_dir {
                if let Err(e) = tokio::fs::remove_dir_all(&temp_dir).await {
                    tracing::warn!(
                        "Failed to remove rebuild scratch directory {}: {}",
                        temp_dir.display(),
                        e
                    );
                }
            }
        }
        if let Some(mut metadata) = self.metadata.get_mut(&key) {
            metadata.status = IndexStatus::Error;
            metadata.last_updated = chrono::Utc::now();
        }
        if let Err(e) = self.save_metadata_to_file() {
            tracing::warn!("Failed to save metadata after discarding rebuild: {}", e);
        }
        self.set_rebuild_phase(space_id, tag_name, field_name, RebuildPhase::Failed);
        self.emit_index_event(IndexEvent::FulltextRebuildFailed {
            index_name: index_id,
            generation,
            reason: reason.to_string(),
        });
    }

    /// Crash recovery for rebuild scratch state. Deletes unreferenced
    /// `*.rebuild-*` directories, heals interrupted publishes from `*.old-*`
    /// backups (keeping the newest backup per index for reverse recovery),
    /// and moves metadata stuck in `Rebuilding` to `Error` (previous data
    /// still served). Idempotent.
    pub fn cleanup_stale_rebuild_state(&self) {
        use std::collections::HashSet;

        let referenced: HashSet<String> = self
            .metadata
            .iter()
            .map(|entry| entry.value().storage_path.clone())
            .collect();

        let mut scan_dirs: Vec<PathBuf> = vec![self.base_path.clone()];
        for path in referenced.iter() {
            let parent = PathBuf::from(path);
            if let Some(dir) = parent.parent() {
                let dir = dir.to_path_buf();
                if !scan_dirs.contains(&dir) {
                    scan_dirs.push(dir);
                }
            }
        }

        for dir in scan_dirs {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            // Backups superseded by a surviving live directory are pruned
            // after the scan, keeping the newest generation per index for
            // reverse recovery.
            let mut superseded: Vec<PathBuf> = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let path_string = path.to_string_lossy().to_string();
                if name.contains(".rebuild-") {
                    if !referenced.contains(&path_string) {
                        tracing::warn!(
                            "Removing stale rebuild scratch directory {}",
                            path.display()
                        );
                        if let Err(e) = std::fs::remove_dir_all(&path) {
                            tracing::warn!(
                                "Failed to remove stale rebuild directory {}: {}",
                                path.display(),
                                e
                            );
                        }
                    }
                } else if let Some(pos) = name.find(".old-") {
                    let candidate = dir.join(&name[..pos]);
                    let candidate_string = candidate.to_string_lossy().to_string();
                    if candidate.exists() || referenced.contains(&candidate_string) {
                        // Live directory survived: only the newest backup is
                        // kept (pruned below).
                        superseded.push(path);
                    } else if referenced.iter().any(|r| {
                        PathBuf::from(r).file_name().and_then(|n| n.to_str()) == Some(&name[..pos])
                    }) {
                        // Metadata points at a missing live directory whose
                        // backup survived: heal the interrupted rename.
                        tracing::warn!(
                            "Healing interrupted rebuild publish: restoring {} from backup",
                            candidate.display()
                        );
                        if let Err(e) = std::fs::rename(&path, &candidate) {
                            tracing::warn!(
                                "Failed to restore rebuild backup {}: {}",
                                path.display(),
                                e
                            );
                        }
                    } else if let Err(e) = std::fs::remove_dir_all(&path) {
                        tracing::warn!(
                            "Failed to remove orphaned rebuild backup {}: {}",
                            path.display(),
                            e
                        );
                    }
                }
            }
            // Group superseded backups by index prefix; keep newest per group.
            {
                use std::collections::HashMap;
                let mut groups: HashMap<String, Vec<PathBuf>> = HashMap::new();
                for backup in superseded {
                    if let Some(name) = backup.file_name().and_then(|n| n.to_str()) {
                        if let Some(pos) = name.find(".old-") {
                            groups
                                .entry(name[..pos].to_string())
                                .or_default()
                                .push(backup);
                        }
                    }
                }
                for (_, mut backups) in groups {
                    backups.sort_by(|a, b| {
                        let generation = |p: &PathBuf| {
                            p.file_name()
                                .and_then(|n| n.to_str())
                                .and_then(|n| n.rsplit(".old-").next())
                                .and_then(|s| s.parse::<u64>().ok())
                                .unwrap_or(0)
                        };
                        generation(b).cmp(&generation(a)).then_with(|| b.cmp(a))
                    });
                    for old in backups.into_iter().skip(1) {
                        if let Err(e) = std::fs::remove_dir_all(&old) {
                            tracing::warn!(
                                "Failed to prune superseded rebuild backup {}: {}",
                                old.display(),
                                e
                            );
                        }
                    }
                }
            }
        }

        for mut entry in self.metadata.iter_mut() {
            if entry.value().status == IndexStatus::Rebuilding {
                tracing::warn!(
                    "Index {} was interrupted mid-rebuild; marking Error (previous data still served)",
                    entry.value().index_id
                );
                entry.value_mut().status = IndexStatus::Error;
                entry.value_mut().last_updated = chrono::Utc::now();
            }
        }
        if let Err(e) = self.save_metadata_to_file() {
            tracing::warn!("Failed to save metadata after rebuild cleanup: {}", e);
        }
    }

    /// Flush pending writes so Tantivy can merge segments.
    /// Tantivy merges in the background without an explicit blocking
    /// optimize call, so optimize is a commit plus metadata refresh.
    pub async fn optimize_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<(), SearchError> {
        let key = IndexKey::new(space_id, tag_name, field_name);
        let engine = self.engines.get(&key).ok_or_else(|| {
            SearchError::IndexNotFound(format!("{}.{}.{}", space_id, tag_name, field_name))
        })?;

        engine.commit().await?;
        if let Some(mut metadata) = self.metadata.get_mut(&key) {
            metadata.last_updated = chrono::Utc::now();
            metadata.status = IndexStatus::Active;
        }

        if let Err(e) = self.save_metadata_to_file() {
            tracing::warn!("Failed to save metadata after optimizing index: {}", e);
        }

        Ok(())
    }

    pub async fn drop_space_indexes(&self, space_id: u64) -> Result<(), SearchError> {
        let space_indexes: Vec<(IndexKey, IndexMetadata)> = self
            .metadata
            .iter()
            .filter(|entry| entry.value().space_id == space_id)
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();

        for (key, metadata) in space_indexes {
            if let Some((_, engine)) = self.engines.remove(&key) {
                engine.close().await.ok();
            }
            self.metadata.remove(&key);

            let storage_path = PathBuf::from(&metadata.storage_path);
            if storage_path.exists() {
                tokio::fs::remove_dir_all(&storage_path).await.ok();
            }
        }

        if let Some(ref schema_manager) = self.schema_manager {
            if let Some(space_info) = schema_manager
                .get_space_by_id(space_id)
                .map_err(|e| SearchError::Internal(format!("Failed to get space: {}", e)))?
            {
                if let Some(ref custom_path) = space_info.storage_path {
                    let fulltext_path = custom_path.join("fulltext");
                    if fulltext_path.exists() {
                        tokio::fs::remove_dir_all(&fulltext_path).await.ok();
                    }
                } else if space_info.isolation_level
                    == graphdb_core::types::space::IsolationLevel::Directory
                {
                    let space_path = self.base_path.join(format!("space_{}", space_id));
                    if space_path.exists() {
                        tokio::fs::remove_dir_all(&space_path).await.ok();
                    }
                }
            }
        }

        if let Err(e) = self.save_metadata_to_file() {
            tracing::warn!(
                "Failed to save metadata after dropping space indexes: {}",
                e
            );
        }

        Ok(())
    }
}
