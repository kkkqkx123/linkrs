use std::sync::Arc;

use crate::query::executor::base::{ExecutorConfig, ShortestPathConfig};
use crate::query::executor::graph_operations::graph_traversal::algorithms::ShortestPathAlgorithmType;
use crate::query::executor::graph_operations::graph_traversal::expand::{
    ExpandExecutor, ExpandExecutorParams,
};
use crate::query::executor::graph_operations::graph_traversal::expand_all::{
    ExpandAllExecutor, ExpandAllExecutorParams,
};
use crate::query::executor::graph_operations::graph_traversal::shortest_path::ShortestPathExecutor;
use crate::query::executor::graph_operations::graph_traversal::traverse::{
    TraverseExecutor, TraverseExecutorParams,
};
use crate::query::validator::context::ExpressionAnalysisContext;
use parking_lot::RwLock;

/// Graph Traversal Executor Factory
pub struct GraphTraversalExecutorFactory;

impl GraphTraversalExecutorFactory {
    /// Create an ExpandExecutor
    pub fn create_expand_executor<S: crate::storage::StorageClient>(
        params: ExpandExecutorParams<S>,
    ) -> ExpandExecutor<S> {
        ExpandExecutor::new(
            params.id,
            params.storage,
            params.edge_direction,
            params.edge_types,
            params.max_depth,
            params.expr_context,
        )
    }

    /// Create the ExpandAllExecutor
    pub fn create_expand_all_executor<S: crate::storage::StorageClient + std::marker::Send>(
        params: ExpandAllExecutorParams<S>,
    ) -> ExpandAllExecutor<S> {
        ExpandAllExecutor::new(params)
    }

    /// Create a TraverseExecutor
    pub fn create_traverse_executor<S: crate::storage::StorageClient>(
        params: TraverseExecutorParams<S>,
    ) -> TraverseExecutor<S> {
        TraverseExecutor::new(
            params.id,
            params.storage,
            params.edge_direction,
            params.edge_types,
            params.max_depth,
            params.conditions,
            params.expr_context,
        )
    }

    /// Create the ShortestPathExecutor
    pub fn create_shortest_path_executor<S: crate::storage::StorageClient>(
        id: i64,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionAnalysisContext>,
        config: ShortestPathConfig,
        algorithm: ShortestPathAlgorithmType,
    ) -> ShortestPathExecutor<S> {
        let base_config = ExecutorConfig::new(id, storage, expr_context);
        ShortestPathExecutor::new(base_config, config, algorithm)
    }
}
