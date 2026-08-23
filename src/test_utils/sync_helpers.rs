//! Sync Module Test Helpers
//!
//! Test utilities for sync module integration tests

#![cfg(feature = "fulltext-search")]

use crate::core::types::TransactionId;
use crate::core::types::VertexId;
use crate::core::types::{DataType, PropertyDef, SpaceInfo, TagInfo};
use crate::core::vertex_edge_path::Tag;
use crate::core::{Value, Vertex};
use crate::search::{
    EngineType, FulltextConfig, FulltextIndexManager, SyncConfig, TantivyConfig, TokenizerKind,
};
use crate::storage::GraphStorage;
use crate::storage::{StorageCommitOps, StorageReader, StorageSchemaOps, StorageWriter};
use crate::sync::batch::BatchConfig;
use crate::sync::coordinator::{ChangeType, SyncCoordinator};
use crate::sync::manager::SyncManager;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

/// Test harness for sync module testing
pub struct SyncTestHarness {
    pub storage: GraphStorage,
    pub sync_manager: Arc<SyncManager>,
    pub sync_coordinator: Arc<SyncCoordinator>,
    pub temp_dir: TempDir,
    pub current_txn_id: Option<u64>,
    pub current_txn_seq: u64,
    pub rt: tokio::runtime::Runtime,
}

impl SyncTestHarness {
    /// Create a new test harness with default configuration
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test_sync.db");
        let index_path = temp_dir.path().join("test_index");

        // Create storage
        let storage = GraphStorage::new_with_path(db_path)?;

        // Create fulltext index manager
        let config = FulltextConfig {
            enabled: true,
            index_path: index_path.clone(),
            default_engine: EngineType::Bm25,
            sync: SyncConfig::default(),
            cache_size: 100,
            max_result_cache: 1000,
            result_cache_ttl_secs: 60,
            tantivy: TantivyConfig {
                tokenizer: TokenizerKind::Default,
                ..Default::default()
            },
        };

        let fulltext_manager = Arc::new(FulltextIndexManager::new(config)?);

        // Create sync coordinator
        let batch_config = BatchConfig {
            batch_size: 100,
            flush_interval: Duration::from_millis(100),
            max_buffer_size: 1000,
            enable_persistence: false,
            persistence_path: None,
            failure_policy: crate::search::SyncFailurePolicy::FailOpen,
        };

        let sync_coordinator =
            Arc::new(SyncCoordinator::new(fulltext_manager.clone(), batch_config));

        // Create sync manager
        let mut sync_manager = SyncManager::new(sync_coordinator.clone());
        sync_manager.configure_outbox(temp_dir.path().join("outbox"))?;
        let sync_manager = Arc::new(sync_manager);

        // Create runtime for async operations
        let rt = tokio::runtime::Runtime::new()?;

        // Start target processors. The test commit helper drives one durable
        // outbox claim synchronously so the runtime owns no long-lived worker.
        rt.block_on(sync_coordinator.start_background_tasks());

        Ok(Self {
            storage,
            sync_manager,
            sync_coordinator,
            temp_dir,
            current_txn_id: None,
            current_txn_seq: 0,
            rt,
        })
    }

    /// Create test harness with vector support
    ///
    /// Vector tests run against the built-in local engine (default backend);
    /// no external service is required.
    pub fn with_vector() -> Result<Self, Box<dyn std::error::Error>> {
        Self::new()
    }

    /// Create test space
    pub fn create_space(&mut self, space_name: &str) -> Result<(), Box<dyn std::error::Error>> {
        let mut space_info = SpaceInfo::new(space_name.to_string()).with_vid_type(DataType::BigInt);
        self.storage.create_space(&mut space_info)?;
        Ok(())
    }

    /// Create tag with fulltext index
    pub fn create_tag_with_fulltext(
        &mut self,
        space_name: &str,
        tag_name: &str,
        properties: Vec<(&str, DataType)>,
        fulltext_fields: Vec<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let props: Vec<PropertyDef> = properties
            .iter()
            .map(|(name, dtype)| PropertyDef::new(name.to_string(), dtype.clone()))
            .collect();

        let tag_info = TagInfo::new(tag_name.to_string()).with_properties(props);

        self.storage.create_tag(space_name, &tag_info)?;

        // Create fulltext index for specified fields
        let space_id = self.storage.get_space_id(space_name)?;
        self.rt.block_on(async {
            let coordinator = self.sync_manager.sync_coordinator();
            for field in fulltext_fields {
                let _ = coordinator
                    .fulltext_manager()
                    .create_index(space_id, tag_name, field, Some(EngineType::Bm25))
                    .await;
            }
        });

        Ok(())
    }

    /// Create tag with vector index
    #[cfg(feature = "vector")]
    pub fn create_tag_with_vector(
        &mut self,
        space_name: &str,
        tag_name: &str,
        vector_field: &str,
        vector_dim: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let props = vec![PropertyDef::new(
            vector_field.to_string(),
            DataType::VectorDense(vector_dim),
        )];

        let tag_info = TagInfo::new(tag_name.to_string()).with_properties(props);

        self.storage.create_tag(space_name, &tag_info)?;

        // Create vector index
        let space_id = self.storage.get_space_id(space_name)?;
        if let Some(vector_coord) = self.sync_manager.vector_coordinator() {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                vector_coord
                    .create_vector_index(
                        space_id,
                        tag_name,
                        vector_field,
                        vector_dim,
                        crate::sync::vector_sync::DistanceMetric::Cosine,
                    )
                    .await
            })?;
        }

        Ok(())
    }

    /// Begin a new transaction
    pub fn begin_transaction(&mut self) -> Result<u64, Box<dyn std::error::Error>> {
        self.current_txn_seq += 1;
        let txn_id = self.current_txn_seq;
        self.current_txn_id = Some(txn_id);
        Ok(txn_id)
    }

    /// Commit current transaction
    pub fn commit_transaction(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(txn_id) = self.current_txn_id.take() {
            let transaction_id = TransactionId(txn_id);
            let intents = self
                .sync_manager
                .pending_transaction_intents(transaction_id)?;
            let commit_lsn = self
                .storage
                .commit_staged_writes(transaction_id, &intents)?;
            self.sync_manager.materialize_committed_transaction(
                transaction_id,
                commit_lsn,
                &intents,
            )?;
            self.sync_manager.retry_outbox_sync()?;
            self.sync_manager.clear_transaction_intents(transaction_id);
        }
        Ok(())
    }

    /// Rollback current transaction
    pub fn rollback_transaction(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(txn_id) = self.current_txn_id.take() {
            self.rt.block_on(
                self.sync_manager
                    .rollback_transaction(TransactionId(txn_id)),
            )?;
        }
        Ok(())
    }

    /// Insert vertex
    pub fn insert_vertex(
        &mut self,
        space_name: &str,
        vertex: Vertex,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.storage.insert_vertex(space_name, vertex.clone())?;

        let space_id = self.storage.get_space_id(space_name)?;
        self.rt.block_on(async {
            for tag in &vertex.tags {
                let tag_name = &tag.name;
                let properties: Vec<(String, Value)> = tag
                    .properties
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                if !properties.is_empty() {
                    self.sync_manager
                        .sync_coordinator()
                        .on_vertex_change(
                            space_id,
                            tag_name,
                            &Value::from(vertex.vid),
                            &properties,
                            ChangeType::Insert,
                        )
                        .await?;
                }
            }
            Ok::<_, Box<dyn std::error::Error>>(())
        })?;

        Ok(())
    }

    /// Insert vertex with transaction
    pub fn insert_vertex_with_txn(
        &mut self,
        space_name: &str,
        vertex: Vertex,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if self.current_txn_id.is_none() {
            return Err("No active transaction".into());
        }

        let space_id = self.storage.get_space_id(space_name)?;
        let txn_id = TransactionId(self.current_txn_id.unwrap());

        let vertex_id = vertex.vid;
        let exists = self.storage.get_vertex(space_name, &vertex_id)?.is_some();

        if exists {
            if let Some(old_vertex) = self.storage.get_vertex(space_name, &vertex_id)? {
                for tag in &old_vertex.tags {
                    let tag_name = &tag.name;
                    for (field_name, value) in &tag.properties {
                        self.sync_manager.on_vertex_change_with_txn(
                            txn_id,
                            space_id,
                            tag_name,
                            &Value::from(vertex_id),
                            &[(field_name.clone(), value.clone())],
                            crate::sync::coordinator::ChangeType::Delete,
                        )?;
                    }
                }
            }

            for tag in &vertex.tags {
                let tag_name = &tag.name;
                for (field_name, value) in &tag.properties {
                    self.sync_manager.on_vertex_change_with_txn(
                        txn_id,
                        space_id,
                        tag_name,
                        &Value::from(vertex_id),
                        &[(field_name.clone(), value.clone())],
                        crate::sync::coordinator::ChangeType::Insert,
                    )?;
                }
            }

            self.storage.update_vertex(space_name, vertex)?;
        } else {
            for tag in &vertex.tags {
                let tag_name = &tag.name;
                for (field_name, value) in &tag.properties {
                    self.sync_manager.on_vertex_change_with_txn(
                        txn_id,
                        space_id,
                        tag_name,
                        &Value::from(vertex_id),
                        &[(field_name.clone(), value.clone())],
                        crate::sync::coordinator::ChangeType::Insert,
                    )?;
                }
            }

            self.storage.insert_vertex(space_name, vertex)?;
        }

        Ok(())
    }

    /// Delete vertex with transaction (sync-aware)
    pub fn delete_vertex_with_txn(
        &mut self,
        space_name: &str,
        vid: i64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if self.current_txn_id.is_none() {
            return Err("No active transaction".into());
        }

        let space_id = self.storage.get_space_id(space_name)?;
        let txn_id = TransactionId(self.current_txn_id.unwrap());
        let vertex_id = crate::core::types::VertexId::from_int64(vid);
        let vid_value = Value::Int(vid as i32);

        // Get the vertex to extract tag and field info for index cleanup
        if let Some(existing) = self.storage.get_vertex(space_name, &vertex_id)? {
            for tag in &existing.tags {
                let tag_name = &tag.name;
                for (field_name, value) in &tag.properties {
                    self.sync_manager.on_vertex_change_with_txn(
                        txn_id,
                        space_id,
                        tag_name,
                        &vid_value,
                        &[(field_name.clone(), value.clone())],
                        crate::sync::coordinator::ChangeType::Delete,
                    )?;
                }
            }
        }

        self.storage.delete_vertex(space_name, &vertex_id)?;
        Ok(())
    }

    /// Search fulltext
    pub fn search_fulltext(
        &self,
        space_name: &str,
        tag_name: &str,
        field_name: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::search::SearchResult>, Box<dyn std::error::Error>> {
        let space_id = self.storage.get_space_id(space_name)?;
        let results = self.rt.block_on(async {
            self.sync_manager
                .sync_coordinator()
                .fulltext_manager()
                .search(space_id, tag_name, field_name, query, limit)
                .await
        })?;
        Ok(results)
    }

    /// Search vector
    #[cfg(feature = "vector")]
    pub fn search_vector(
        &self,
        space_name: &str,
        tag_name: &str,
        _field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<crate::sync::vector_sync::SearchResult>, Box<dyn std::error::Error>> {
        if let Some(vector_coord) = self.sync_manager.vector_coordinator() {
            let _space_id = self.storage.get_space_id(space_name)?;
            let rt = tokio::runtime::Runtime::new()?;
            let results = rt.block_on(async {
                // Create search query
                let query = crate::sync::vector_sync::SearchQuery::new(query_vector, limit);

                vector_coord.search(tag_name, query).await
            })?;
            Ok(results)
        } else {
            Err("Vector coordinator not initialized".into())
        }
    }

    /// Get vertex
    pub fn get_vertex(
        &self,
        space_name: &str,
        vid: &Value,
    ) -> Result<Option<Vertex>, Box<dyn std::error::Error>> {
        let vertex_id = VertexId::try_from(vid)?;
        Ok(self.storage.get_vertex(space_name, &vertex_id)?)
    }

    /// Assert vertex exists
    pub fn assert_vertex_exists(
        &self,
        space_name: &str,
        vid: &Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let vertex = self.get_vertex(space_name, vid)?;
        assert!(vertex.is_some(), "Vertex {:?} should exist", vid);
        Ok(())
    }

    /// Assert vertex properties
    pub fn assert_vertex_props(
        &self,
        space_name: &str,
        vid: &Value,
        tag_name: &str,
        expected_props: HashMap<&str, Value>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let vertex = self
            .get_vertex(space_name, vid)?
            .ok_or_else(|| format!("Vertex {:?} not found", vid))?;

        let tag = vertex
            .get_tag(tag_name)
            .ok_or_else(|| format!("Tag {} not found", tag_name))?;

        for (prop_name, expected_value) in expected_props {
            let actual_value = tag.properties.get(prop_name);
            assert_eq!(
                actual_value,
                Some(expected_value).as_ref(),
                "Property {} mismatch",
                prop_name
            );
        }

        Ok(())
    }

    /// Wait for async processing
    pub fn wait_for_async(&self, duration_ms: u64) {
        self.rt.block_on(async {
            tokio::time::sleep(Duration::from_millis(duration_ms)).await;
        });
    }
}

impl Default for SyncTestHarness {
    fn default() -> Self {
        Self::new().unwrap()
    }
}

impl Clone for SyncTestHarness {
    fn clone(&self) -> Self {
        // For cloning, we create a new harness with same configuration
        // Note: This is a simplified clone for testing purposes
        Self::new().unwrap()
    }
}

/// Helper function to create test vertex
pub fn create_test_vertex(vid: i64, tag_name: &str, props: Vec<(&str, Value)>) -> Vertex {
    let mut properties = HashMap::new();
    for (k, v) in props {
        properties.insert(k.to_string(), v);
    }
    let tag = Tag::new(tag_name.to_string(), properties);
    Vertex::new(VertexId::from_int64(vid), vec![tag])
}

/// Helper function to create test vertex with vector
pub fn create_test_vertex_with_vector(
    vid: i64,
    tag_name: &str,
    string_prop: (&str, &str),
    vector_prop: (&str, Vec<f32>),
) -> Vertex {
    let mut properties = HashMap::new();
    properties.insert(string_prop.0.to_string(), Value::string(string_prop.1));

    // Convert Vec<f32> to VectorValue
    use crate::core::VectorValue;
    let vector_value = VectorValue::Dense(vector_prop.1);
    properties.insert(vector_prop.0.to_string(), Value::Vector(vector_value));

    let tag = Tag::new(tag_name.to_string(), properties);
    Vertex::new(VertexId::from_int64(vid), vec![tag])
}

/// Helper function to generate random vector
pub fn generate_random_vector(dim: usize) -> Vec<f32> {
    (0..dim).map(|_| rand::random::<f32>()).collect()
}
