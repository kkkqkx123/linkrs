use dashmap::DashMap;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;

use crate::core::metadata::SchemaManager;
use crate::core::stats::StatsManager;
use crate::search::config::FulltextConfig;
use crate::search::engine::{ConsistencyState, EngineType};
use crate::search::error::SearchError;
use crate::search::metadata::{IndexKey, IndexMetadata, IndexStatus};
use crate::search::metrics::MetricsSearchEngine;
use crate::search::result::{IndexStats, SearchResult};
use crate::search::tantivy_index::TantivySearchEngine;

const METADATA_FILE_NAME: &str = "fulltext_metadata.json";

#[derive(Debug)]
pub struct FulltextIndexManager {
    engines: DashMap<IndexKey, Arc<TantivySearchEngine>>,
    metadata: DashMap<IndexKey, IndexMetadata>,
    base_path: PathBuf,
    #[cfg(feature = "fulltext-search")]
    default_engine: EngineType,
    #[cfg(feature = "fulltext-search")]
    config: FulltextConfig,
    schema_manager: Option<Arc<SchemaManager>>,
    stats_manager: Mutex<Option<Arc<StatsManager>>>,
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
            #[cfg(feature = "fulltext-search")]
            default_engine: config.default_engine,
            #[cfg(feature = "fulltext-search")]
            config,
            schema_manager: None,
            stats_manager: Mutex::new(None),
        };

        manager.discover_existing_indexes()?;

        Ok(manager)
    }

    fn discover_existing_indexes(&self) -> Result<(), SearchError> {
        #[cfg(feature = "fulltext-search")]
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

        #[cfg(feature = "fulltext-search")]
        return self.discover_indexes_from_disk();

        #[cfg(not(feature = "fulltext-search"))]
        Ok(())
    }

    #[cfg(feature = "fulltext-search")]
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

    #[cfg(feature = "fulltext-search")]
    fn try_restore_bm25_index(
        &self,
        path: &std::path::Path,
    ) -> Option<(IndexKey, Arc<TantivySearchEngine>, IndexMetadata)> {
        let dir_name = path.file_name()?.to_string_lossy();
        let (space_id, tag_name, field_name) = self.parse_index_id(&dir_name)?;

        let engine = TantivySearchEngine::open_or_create(
            &self.base_path.join(&*dir_name),
            self.config.tantivy.clone(),
        )
        .ok()?;

        let engine = Arc::new(engine);
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

    #[cfg(feature = "fulltext-search")]
    fn parse_index_id(&self, index_id: &str) -> Option<(u64, String, String)> {
        let parts: Vec<&str> = index_id.split('_').collect();
        if parts.len() < 4 || parts[0] != "space" || parts[1] != "ft" {
            return None;
        }

        let space_id: u64 = parts[2].parse().ok()?;
        let tag_name = parts.get(3)?.to_string();
        let field_name = parts.get(4)?.to_string();

        Some((space_id, tag_name, field_name))
    }

    #[cfg(feature = "fulltext-search")]
    fn restore_index_from_metadata(&self, metadata: &IndexMetadata) -> Result<(), SearchError> {
        let key = IndexKey::new(metadata.space_id, &metadata.tag_name, &metadata.field_name);

        let engine = TantivySearchEngine::open_or_create(
            &self.base_path.join(&metadata.index_id),
            self.config.tantivy.clone(),
        )?;

        self.engines.insert(key.clone(), Arc::new(engine));
        self.metadata.insert(key, metadata.clone());

        Ok(())
    }

    #[cfg(feature = "fulltext-search")]
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
        std::fs::write(&metadata_path, content)?;

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

    #[cfg(feature = "fulltext-search")]
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

                use crate::core::types::space::IsolationLevel;
                match space_info.isolation_level {
                    IsolationLevel::Device => {
                        if let Some(ref custom_path) = space_info.storage_path {
                            let fulltext_path = custom_path.join("fulltext");
                            if !fulltext_path.exists() {
                                std::fs::create_dir_all(&fulltext_path)?;
                            }
                            return Ok(fulltext_path);
                        }
                    }
                    IsolationLevel::Directory => {
                        let space_path = self.base_path.join(format!("space_{}", space_id));
                        if !space_path.exists() {
                            std::fs::create_dir_all(&space_path)?;
                        }
                        return Ok(space_path);
                    }
                    IsolationLevel::Shared => {}
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
        #[cfg(not(feature = "fulltext-search"))]
        let _ = &engine_config;

        let key = IndexKey::new(space_id, tag_name, field_name);
        let index_id = key.to_index_id();

        if self.engines.contains_key(&key) {
            return Err(SearchError::IndexAlreadyExists(index_id));
        }

        #[cfg(feature = "fulltext-search")]
        {
            let engine_type = _engine_type.unwrap_or(self.default_engine);
            let storage_path = self.get_space_storage_path(space_id)?;

            let engine = TantivySearchEngine::open_or_create(
                &storage_path.join(&index_id),
                self.config.tantivy.clone(),
            )?;
            let engine = Arc::new(engine);

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

            Ok(index_id)
        }

        #[cfg(not(feature = "fulltext-search"))]
        {
            Err(SearchError::EngineUnavailable)
        }
    }

    pub fn get_engine(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<Arc<TantivySearchEngine>> {
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

        self.metadata.remove(&key);

        if let Err(e) = self.save_metadata_to_file() {
            tracing::warn!("Failed to save metadata after dropping index: {}", e);
        }

        let index_id = key.to_index_id();
        let index_path = self.base_path.join(&index_id);
        if index_path.exists() {
            tokio::fs::remove_dir_all(&index_path).await?;
        }

        Ok(())
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
        for entry in self.engines.iter() {
            entry.value().commit().await?;
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

    pub fn get_inconsistent_indexes(&self) -> Vec<IndexMetadata> {
        self.metadata
            .iter()
            .filter(|entry| {
                self.engines
                    .get(entry.key())
                    .is_some_and(|e| e.consistency_state() == ConsistencyState::Inconsistent)
            })
            .map(|entry| entry.value().clone())
            .collect()
    }

    pub async fn rebuild_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<(), SearchError> {
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

        if let Err(e) = self.save_metadata_to_file() {
            tracing::warn!("Failed to save metadata after rebuilding index: {}", e);
        }

        tracing::info!(
            "Rebuilt index {}.{}.{} - cleared and marked consistent",
            space_id,
            tag_name,
            field_name
        );
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
                    == crate::core::types::space::IsolationLevel::Directory
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
