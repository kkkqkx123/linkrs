//! Graph Traversal Executor Builder
//!
//! Responsible for creating executors of graph traversal types (Expand, ExpandAll, Traverse, AllPaths, ShortestPath, MultiShortestPath)

use crate::core::error::query::QueryError;
use crate::core::types::{EdgeDirection, VertexId};
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::{
    AllPathsConfig, ExecutionContext, ExecutorConfig, MultiShortestPathConfig, ShortestPathConfig,
};
use crate::query::executor::graph_operations::graph_traversal::algorithms::bfs_shortest::BfsShortestPathConfig;
use crate::query::executor::graph_operations::graph_traversal::algorithms::MultiShortestPathExecutor;
use crate::query::executor::graph_operations::graph_traversal::{
    AllPathsExecutor, ExpandAllExecutor, ExpandAllExecutorParams, ExpandExecutor,
    ShortestPathExecutor, TraverseExecutor,
};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, PlanNode,
};
use crate::query::planning::plan::core::nodes::traversal::{
    AllPathsNode, BFSShortestNode, BiExpandNode, BiTraverseNode, MultiShortestPathNode,
    ShortestPathNode,
};
use crate::query::planning::plan::core::nodes::{ExpandAllNode, ExpandNode, TraverseNode};
use crate::storage::StorageClient;
use parking_lot::RwLock;
use std::sync::Arc;

/// Graph Traversal Executor Builder
pub struct TraversalBuilder<S: StorageClient + Send + 'static> {
    _phantom: std::marker::PhantomData<S>,
}

impl<S: StorageClient + Send + 'static> TraversalBuilder<S> {
    /// Create a new graph traversal builder.
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }

    /// Resolve space name from space_id using storage
    fn resolve_space_name(storage: &Arc<RwLock<S>>, space_id: u64) -> String {
        let storage_guard = storage.read();
        match storage_guard.get_space_by_id(space_id) {
            Ok(Some(space_info)) => space_info.space_name,
            _ => "default".to_string(),
        }
    }

    /// Constructing the Expand executor
    pub fn build_expand(
        node: &ExpandNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // Parameters of ExpandExecutor::new: id, storage, edge_direction, edge_types, max_depth, expr_context
        // direction() 返回 EdgeDirection 值，直接传递即可
        let executor = ExpandExecutor::new(
            node.id(),
            storage,
            node.direction(),
            Some(node.edge_types().to_vec()),
            node.step_limit().map(|s| s as usize),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::Expand(executor))
    }

    /// Building the ExpandAll executor
    pub fn build_expand_all(
        node: &ExpandAllNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // Parameters of ExpandAllExecutor::new: id, storage, edge_direction, edge_types, any_edge_type, max_depth, expr_context
        // ExpandAllNode 的 direction() 返回 &str，需要转换为 EdgeDirection
        let edge_direction = EdgeDirection::from(node.direction());

        // Get space name from storage using space_id
        let space_name = Self::resolve_space_name(&storage, node.space_id());

        let params = ExpandAllExecutorParams {
            id: node.id(),
            storage: storage.clone(),
            edge_direction,
            edge_types: Some(node.edge_types().to_vec()),
            any_edge_type: false,
            max_depth: node.step_limit().map(|s| s as usize),
            expr_context: context.expression_context().clone(),
            space_id: node.space_id(),
            space_name: space_name.clone(),
        };
        let src_vids: Vec<VertexId> = node
            .src_vids()
            .iter()
            .filter_map(|v| VertexId::try_from(v).ok())
            .collect();
        let mut executor = ExpandAllExecutor::with_context(params, context.clone())
            .with_src_vids(src_vids)
            .with_include_empty_paths(node.include_empty_paths())
            .with_filter(node.filter().cloned());

        // If input_var is set, use it to get input from ExecutionContext
        if let Some(input_var) = node.get_input_var() {
            executor = executor.with_input_var(input_var.to_string());
        } else {
            // If there are input nodes, get the input variable name from the first input node
            let inputs = node.inputs();
            if !inputs.is_empty() {
                if let Some(input_var) = inputs[0].output_var() {
                    executor = executor.with_input_var(input_var.to_string());
                }
            }
        }

        // Set column names from the node configuration
        // This allows custom dst column names for variable binding in multi-hop queries
        if !node.col_names().is_empty() {
            executor = executor.with_col_names(node.col_names().to_vec());
        }

        Ok(ExecutorEnum::ExpandAll(executor))
    }

    /// Building the Traverse executor
    pub fn build_traverse(
        node: &TraverseNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // TraverseExecutor::new parameters: id, storage, edge_direction, edge_types, max_depth, conditions, expr_context
        let executor = TraverseExecutor::new(
            node.id(),
            storage,
            node.direction(),
            Some(node.edge_types().to_vec()),
            Some(node.max_steps() as usize),
            None, // conditions
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::Traverse(executor))
    }

    /// Building the AllPaths executor
    pub fn build_all_paths(
        node: &AllPathsNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let space_name = Self::resolve_space_name(&storage, node.space_id());
        let executor = AllPathsExecutor::new(
            ExecutorConfig::new(node.id(), storage, context.expression_context().clone()),
            AllPathsConfig {
                left_start_ids: node.start_vertex_ids().to_vec(),
                right_start_ids: node.end_vertex_ids().to_vec(),
                max_hops: node.max_hop(),
                edge_types: Some(node.edge_types().to_vec()),
                direction: EdgeDirection::Out,
                space_name,
            },
        );
        Ok(ExecutorEnum::AllPaths(executor))
    }

    /// Building the ShortestPath executor
    pub fn build_shortest_path(
        node: &ShortestPathNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        use crate::query::executor::graph_operations::graph_traversal::algorithms::ShortestPathAlgorithmType;

        let start_vertex_ids: Vec<VertexId> = node
            .start_vertex_ids()
            .iter()
            .filter_map(|v| VertexId::try_from(v).ok())
            .collect();
        let end_vertex_ids: Vec<VertexId> = node
            .end_vertex_ids()
            .iter()
            .filter_map(|v| VertexId::try_from(v).ok())
            .collect();

        let space_name = Self::resolve_space_name(&storage, node.space_id());
        let mut executor = ShortestPathExecutor::new(
            ExecutorConfig::new(node.id(), storage, context.expression_context().clone()),
            ShortestPathConfig {
                start_vertex_ids,
                direction: EdgeDirection::Out,
                edge_types: Some(node.edge_types().to_vec()),
                space_name,
            },
            ShortestPathAlgorithmType::BFS,
        );
        executor.set_end_vertex_ids(end_vertex_ids);
        executor.max_depth = Some(node.max_step());
        Ok(ExecutorEnum::ShortestPath(executor))
    }

    /// Building the BFSShortest executor
    pub fn build_bfs_shortest(
        node: &BFSShortestNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        use crate::query::executor::graph_operations::graph_traversal::algorithms::BFSShortestExecutor;

        let space_name = Self::resolve_space_name(&storage, node.space_id());
        // BFSShortestExecutor::new parameters: id, storage, steps, edge_types, with_cycle, max_depth, single_shortest, limit, start_vertex, end_vertex, expr_context
        let executor = BFSShortestExecutor::new(
            ExecutorConfig::new(node.id(), storage, context.expression_context().clone()),
            BfsShortestPathConfig {
                steps: node.steps(),
                edge_types: node.edge_types().to_vec(),
                with_cycle: node.with_cycle(),
                max_depth: Some(node.steps()),
                single_shortest: false,
                limit: usize::MAX,
                start_vertex: VertexId::new(),
                end_vertex: VertexId::new(),
                space_name,
            },
        );
        Ok(ExecutorEnum::BFSShortest(executor))
    }

    /// Constructing the MultiShortestPath executor
    pub fn build_multi_shortest_path(
        node: &MultiShortestPathNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        //  Obtain the starting and ending point IDs from the input.
        let start_vids: Vec<crate::core::types::VertexId> = Vec::new();
        let _end_vids: Vec<crate::core::types::VertexId> = Vec::new();

        let space_name = Self::resolve_space_name(&storage, 0);
        let executor = MultiShortestPathExecutor::new(
            ExecutorConfig::new(node.id(), storage, context.expression_context().clone()),
            MultiShortestPathConfig {
                start_vids,
                direction: EdgeDirection::Out,
                edge_types: None,
                max_steps: node.steps(),
                space_name,
            },
        );
        Ok(ExecutorEnum::MultiShortestPath(executor))
    }

    /// Constructing the BiExpand executor
    /// Bidirectional expand from two input sources meeting at common vertices
    pub fn build_bi_expand(
        node: &BiExpandNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = ExpandExecutor::new(
            node.id(),
            storage,
            node.left_direction(),
            Some(node.edge_types().to_vec()),
            Some(node.max_hops()),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::BiExpand(executor))
    }

    /// Constructing the BiTraverse executor
    /// Bidirectional traverse from two input sources meeting at common vertices
    pub fn build_bi_traverse(
        node: &BiTraverseNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = ExpandExecutor::new(
            node.id(),
            storage,
            node.left_direction(),
            Some(node.edge_types().to_vec()),
            Some(node.max_hops()),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::BiTraverse(executor))
    }
}

impl<S: StorageClient + 'static> Default for TraversalBuilder<S> {
    fn default() -> Self {
        Self::new()
    }
}
