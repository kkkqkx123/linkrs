mod index;

pub use index::IndexMetadata;

use std::sync::Arc;

use dashmap::{DashMap, DashSet};
use tracing::{debug, info, warn};

use crate::config::VectorClientConfig;
use crate::engine::{create_engine as build_engine, DisabledEngine, VectorEngine};
use crate::error::{Result, VectorClientError};
use crate::types::{CollectionConfig, SearchQuery, SearchResult, VectorFilter, VectorPoint};

pub struct VectorManager {
    engine: Arc<dyn VectorEngine>,
    /// Indexes created through this manager, with full metadata.
    indexes: DashMap<String, IndexMetadata>,
    /// Collections known to exist on the server (warmed from
    /// `list_collections` at startup). Keeps `index_exists` truthful for
    /// collections created by earlier processes that were never registered
    /// in this one.
    known_collections: DashSet<String>,
}

impl std::fmt::Debug for VectorManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VectorManager")
            .field("engine", &self.engine.name())
            .field("index_count", &self.indexes.len())
            .finish()
    }
}

impl VectorManager {
    pub async fn new(config: VectorClientConfig) -> Result<Self> {
        let enabled = config.enabled;

        let engine: Arc<dyn VectorEngine> = if enabled {
            let engine = build_engine(config).await?;
            engine
        } else {
            info!("Vector search is disabled, using no-op engine");
            Arc::new(DisabledEngine) as Arc<dyn VectorEngine>
        };

        if enabled {
            match engine.health_check().await {
                Ok(health) => {
                    if health.is_healthy {
                        info!(
                            "Vector engine health check passed: {} {}",
                            health.engine_name, health.engine_version
                        );
                    } else {
                        warn!("Vector engine health check failed: {:?}", health.message);
                    }
                }
                Err(e) => {
                    warn!("Vector engine health check failed: {}", e);
                }
            }
        }

        let manager = Self {
            engine,
            indexes: DashMap::new(),
            known_collections: DashSet::new(),
        };

        // Best-effort warmup: discover collections that already exist on the
        // server so existence checks survive process restarts. Failures are
        // logged and retried lazily on the next create.
        if enabled {
            let engine = manager.engine.clone();
            let known = manager.known_collections.clone();
            tokio::spawn(async move {
                match engine.list_collections().await {
                    Ok(names) => {
                        debug!(
                            "Warmed collection registry with {} existing collections",
                            names.len()
                        );
                        for name in names {
                            known.insert(name);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to prewarm collection registry: {}", e);
                    }
                }
            });
        }

        Ok(manager)
    }

    pub fn engine(&self) -> &Arc<dyn VectorEngine> {
        &self.engine
    }

    pub async fn create_index(&self, name: &str, config: CollectionConfig) -> Result<()> {
        if self.indexes.contains_key(name) {
            return Err(VectorClientError::IndexAlreadyExists(name.to_string()));
        }

        debug!("Creating vector collection: {}", name);
        self.engine.create_collection(name, config.clone()).await?;

        let metadata = IndexMetadata::new(name.to_string(), config);
        self.indexes.insert(name.to_string(), metadata);
        self.known_collections.insert(name.to_string());

        info!("Vector index created: {}", name);
        Ok(())
    }

    pub async fn drop_index(&self, name: &str) -> Result<()> {
        if let Some((_, metadata)) = self.indexes.remove(name) {
            debug!("Dropping vector collection: {}", metadata.name);
            self.engine.delete_collection(name).await?;
            info!("Vector index dropped: {}", name);
        }
        self.known_collections.remove(name);
        Ok(())
    }

    pub fn unregister_index(&self, name: &str) {
        if self.indexes.remove(name).is_some() {
            debug!("Unregistered logical index: {}", name);
        }
    }

    pub fn register_index(&self, name: &str, metadata: IndexMetadata) {
        self.indexes.insert(name.to_string(), metadata);
        self.known_collections.insert(name.to_string());
    }

    pub fn index_exists(&self, name: &str) -> bool {
        self.indexes.contains_key(name) || self.known_collections.contains(name)
    }

    pub fn get_index_metadata(&self, name: &str) -> Option<IndexMetadata> {
        self.indexes.get(name).map(|m| m.clone())
    }

    pub fn list_indexes(&self) -> Vec<IndexMetadata> {
        self.indexes.iter().map(|m| m.value().clone()).collect()
    }

    pub async fn upsert(&self, collection: &str, point: VectorPoint) -> Result<()> {
        self.engine.upsert(collection, point).await?;
        Ok(())
    }

    pub async fn upsert_batch(&self, collection: &str, points: Vec<VectorPoint>) -> Result<()> {
        self.engine.upsert_batch(collection, points).await?;
        Ok(())
    }

    pub async fn delete(&self, collection: &str, point_id: &str) -> Result<()> {
        self.engine.delete(collection, point_id).await?;
        Ok(())
    }

    pub async fn delete_batch(&self, collection: &str, point_ids: Vec<&str>) -> Result<()> {
        self.engine.delete_batch(collection, point_ids).await?;
        Ok(())
    }

    pub async fn delete_by_filter(&self, collection: &str, filter: VectorFilter) -> Result<()> {
        self.engine.delete_by_filter(collection, filter).await?;
        Ok(())
    }

    pub async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>> {
        self.engine.search(collection, query).await
    }

    pub async fn get(&self, collection: &str, point_id: &str) -> Result<Option<VectorPoint>> {
        self.engine.get(collection, point_id).await
    }

    pub async fn count(&self, collection: &str) -> Result<u64> {
        self.engine.count(collection).await
    }
}
