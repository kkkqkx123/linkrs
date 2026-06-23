//! Vector Index Management Executors
//!
//! This module implements executors for vector index DDL operations.

use std::sync::Arc;

use crate::core::error::DBError;
use crate::query::executor::base::{
    BaseExecutor, DBResult, ExecutionResult, Executor, ExecutorStats, HasStorage,
};
use crate::query::planning::plan::core::nodes::search::vector::management::{
    CreateVectorIndexNode, DropVectorIndexNode,
};
use crate::storage::StorageReader;
use crate::sync::vector_sync::VectorSyncCoordinator;
use parking_lot::RwLock;

fn convert_distance(
    dist: crate::query::parser::ast::vector::VectorDistance,
) -> vector_client::DistanceMetric {
    match dist {
        crate::query::parser::ast::vector::VectorDistance::Cosine => {
            vector_client::DistanceMetric::Cosine
        }
        crate::query::parser::ast::vector::VectorDistance::Euclidean => {
            vector_client::DistanceMetric::Euclid
        }
        crate::query::parser::ast::vector::VectorDistance::Dot => {
            vector_client::DistanceMetric::Dot
        }
    }
}

/// Create vector index executor
pub struct CreateVectorIndexExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
    node: CreateVectorIndexNode,
    coordinator: Arc<VectorSyncCoordinator>,
    _phantom: std::marker::PhantomData<S>,
}

impl<S: StorageReader> CreateVectorIndexExecutor<S> {
    /// Create a new create vector index executor
    pub fn new(
        id: i64,
        node: CreateVectorIndexNode,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<crate::query::validator::context::ExpressionAnalysisContext>,
        coordinator: Arc<VectorSyncCoordinator>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "CreateVectorIndexExecutor".to_string(),
                storage,
                expr_context,
            ),
            node,
            coordinator,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<S: StorageReader> Executor<S> for CreateVectorIndexExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        // Use space_id from plan node
        let space_id = self.node.space_id;

        // Check if index already exists
        let exists =
            self.coordinator
                .index_exists(space_id, &self.node.tag_name, &self.node.field_name);

        if exists {
            if !self.node.if_not_exists {
                return Err(DBError::validation(format!(
                    "Vector index '{}' already exists on {}.{}",
                    self.node.index_name, self.node.tag_name, self.node.field_name
                )));
            }
            return Ok(ExecutionResult::Success);
        }

        // Build vector index config
        let config = vector_client::CollectionConfig {
            vector_size: self.node.vector_size,
            distance: convert_distance(self.node.distance),
            index_type: Some(vector_client::IndexType::HNSW),
            hnsw_config: Some(vector_client::HnswConfig {
                m: self.node.hnsw_m.unwrap_or(16),
                ef_construct: self.node.hnsw_ef_construct.unwrap_or(100),
                full_scan_threshold: None,
                max_indexing_threads: None,
                on_disk: None,
                payload_m: Some(self.node.hnsw_m.unwrap_or(16)),
            }),
            quantization_config: None,
            replication_factor: None,
            write_consistency_factor: None,
            on_disk_payload: None,
            shard_number: None,
        };

        // Create vector index using coordinator's runtime (only if engine is not disabled)
        if !self.coordinator.is_disabled_engine() {
            let coordinator = self.coordinator.clone();
            let tag_name = self.node.tag_name.clone();
            let field_name = self.node.field_name.clone();

            self.coordinator
                .runtime()
                .block_on(async move {
                    coordinator
                        .create_index_with_config(space_id, &tag_name, &field_name, config)
                        .await
                })
                .map_err(|e| DBError::internal(format!("Failed to create vector index: {}", e)))?;
        } else {
            // Engine disabled: register logical index so metadata provider can find it
            let collection_name = format!(
                "space_{}_{}_{}",
                space_id, self.node.tag_name, self.node.field_name
            );
            self.coordinator.register_logical_index(
                space_id,
                &self.node.tag_name,
                &self.node.field_name,
                collection_name,
                config,
                Some(self.node.index_name.clone()),
            );
        }

        Ok(ExecutionResult::Success)
    }

    fn open(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn is_open(&self) -> bool {
        true
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        "Create vector index executor"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.stats_mut()
    }
}

impl<S: StorageReader> HasStorage<S> for CreateVectorIndexExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base
            .storage
            .as_ref()
            .expect("storage should be initialized")
    }
}

/// Drop vector index executor
pub struct DropVectorIndexExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
    node: DropVectorIndexNode,
    coordinator: Arc<VectorSyncCoordinator>,
    _phantom: std::marker::PhantomData<S>,
}

impl<S: StorageReader> DropVectorIndexExecutor<S> {
    /// Create a new drop vector index executor
    pub fn new(
        id: i64,
        node: DropVectorIndexNode,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<crate::query::validator::context::ExpressionAnalysisContext>,
        coordinator: Arc<VectorSyncCoordinator>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "DropVectorIndexExecutor".to_string(),
                storage,
                expr_context,
            ),
            node,
            coordinator,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<S: StorageReader> Executor<S> for DropVectorIndexExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        // Get space_id from execution context
        let space_id = self.base.context.current_space_id().unwrap_or(0);

        // Find index metadata by name
        let indexes = self.coordinator.list_indexes();
        let index_metadata = indexes
            .iter()
            .find(|idx| idx.collection_name == self.node.index_name);

        if index_metadata.is_none() {
            if !self.node.if_exists {
                return Err(DBError::validation(format!(
                    "Vector index '{}' does not exist",
                    self.node.index_name
                )));
            }
            return Ok(ExecutionResult::Success);
        }

        let metadata = index_metadata.unwrap();

        // Drop vector index using coordinator's runtime
        let coordinator = self.coordinator.clone();
        let tag_name = metadata.tag_name.clone();
        let field_name = metadata.field_name.clone();

        self.coordinator
            .runtime()
            .block_on(async move {
                coordinator
                    .drop_vector_index(space_id, &tag_name, &field_name)
                    .await
            })
            .map_err(|e| DBError::internal(format!("Failed to drop vector index: {}", e)))?;

        Ok(ExecutionResult::Success)
    }

    fn open(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn is_open(&self) -> bool {
        true
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        "Drop vector index executor"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.stats_mut()
    }
}

impl<S: StorageReader> HasStorage<S> for DropVectorIndexExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base
            .storage
            .as_ref()
            .expect("storage should be initialized")
    }
}
