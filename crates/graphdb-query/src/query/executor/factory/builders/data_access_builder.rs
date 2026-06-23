//! Data Access Executor Builder
//!
//! Responsible for creating executors for different data access types (ScanVertices, ScanEdges, GetVertices, GetNeighbors, IndexScan, GetEdges)

use crate::core::error::QueryError;
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::{ExecutionContext, ExecutorConfig, IndexScanConfig};
use crate::query::executor::data_access::{
    GetEdgesExecutor, GetNeighborsExecutor, GetVerticesExecutor, GetVerticesParams,
    IndexScanExecutor, ScanEdgesExecutor,
};
use crate::query::executor::factory::param_parsing::{parse_edge_direction, parse_vertex_ids};
use crate::query::planning::plan::core::nodes::access::IndexScanNode;
use crate::query::planning::plan::core::nodes::{
    EdgeIndexScanNode, GetEdgesNode, GetNeighborsNode, GetVerticesNode, ScanEdgesNode,
    ScanVerticesNode,
};
use crate::storage::StorageClient;
use parking_lot::RwLock;
use std::sync::Arc;

/// Data Access Executor Builder
pub struct DataAccessBuilder<S: StorageClient + Send + 'static> {
    _phantom: std::marker::PhantomData<S>,
}

impl<S: StorageClient + Send + 'static> DataAccessBuilder<S> {
    /// Create a new data access builder.
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }

    /// Building the ScanVertices executor
    pub fn build_scan_vertices(
        node: &ScanVerticesNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let col_names = if !node.col_names().is_empty() {
            node.col_names().to_vec()
        } else if let Some(output_var) = node.output_var() {
            vec![output_var.to_string()]
        } else {
            vec!["vertex".to_string()]
        };
        let params = GetVerticesParams {
            space_name: node.space_name().to_string(),
            vertex_ids: None,
            tag_filter: None,
            vertex_filter: node.vertex_filter().and_then(|f| f.get_expression()),
            limit: node.limit().map(|l| l as usize),
            col_names,
        };
        let executor = GetVerticesExecutor::new(
            node.id(),
            storage,
            params,
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::GetVertices(executor))
    }

    /// Building the ScanEdges executor
    pub fn build_scan_edges(
        node: &ScanEdgesNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let space_name = {
            let storage_guard = storage.read();
            match storage_guard.get_space_by_id(node.space_id()) {
                Ok(Some(space_info)) => space_info.space_name,
                _ => "default".to_string(),
            }
        };
        let executor = ScanEdgesExecutor::new(
            node.id(),
            storage,
            node.edge_type(),
            node.filter().and_then(|f| f.get_expression()),
            node.limit().map(|l| l as usize),
            context.expression_context().clone(),
        )
        .with_space_name(space_name);
        Ok(ExecutorEnum::ScanEdges(executor))
    }

    /// Constructing the GetVertices executor
    pub fn build_get_vertices(
        node: &GetVerticesNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let vertex_ids = parse_vertex_ids(node.src_vids());
        let col_names = if node.col_names().is_empty() {
            vec!["vertex".to_string()]
        } else {
            node.col_names().to_vec()
        };
        let params = GetVerticesParams {
            space_name: node.space_name().to_string(),
            vertex_ids: if vertex_ids.is_empty() {
                None
            } else {
                Some(vertex_ids)
            },
            tag_filter: None,
            vertex_filter: node.expression().and_then(|e| e.get_expression()),
            limit: node.limit().map(|l| l as usize),
            col_names,
        };
        let executor = GetVerticesExecutor::new(
            node.id(),
            storage,
            params,
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::GetVertices(executor))
    }

    /// Constructing the GetNeighbors executor
    pub fn build_get_neighbors(
        node: &GetNeighborsNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let vertex_ids = parse_vertex_ids(node.src_vids());
        let edge_direction = parse_edge_direction(node.direction());
        let edge_types = if node.edge_types().is_empty() {
            None
        } else {
            Some(node.edge_types().to_vec())
        };
        let space_name = {
            let storage_guard = storage.read();
            match storage_guard.get_space_by_id(node.space_id()) {
                Ok(Some(space_info)) => space_info.space_name,
                _ => "default".to_string(),
            }
        };
        let executor = GetNeighborsExecutor::new(
            node.id(),
            storage,
            vertex_ids,
            edge_direction,
            edge_types,
            context.expression_context().clone(),
            space_name,
        );
        Ok(ExecutorEnum::GetNeighbors(executor))
    }

    /// Building the EdgeIndexScan executor
    pub fn build_edge_index_scan(
        node: &EdgeIndexScanNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = IndexScanExecutor::new(
            ExecutorConfig::new(node.id(), storage, context.expression_context().clone()),
            IndexScanConfig {
                space_id: node.space_id(),
                tag_id: node
                    .edge_type()
                    .chars()
                    .fold(0i32, |acc, c| acc.wrapping_mul(31).wrapping_add(c as i32)),
                index_id: node
                    .index_name()
                    .chars()
                    .fold(0i32, |acc, c| acc.wrapping_mul(31).wrapping_add(c as i32)),
                index_name: node.index_name().to_string(),
                schema_name: node.schema_name().to_string(),
                scan_type: node.scan_type().as_str().to_string(),
                scan_limits: node.scan_limits().to_vec(),
                filter: node.filter().and_then(|f| f.get_expression()),
                return_columns: node.return_columns().to_vec(),
                limit: node.limit().map(|l| l as usize),
                is_edge: true,
            },
        );
        Ok(ExecutorEnum::IndexScan(executor))
    }

    /// Constructing the GetEdges executor
    pub fn build_get_edges(
        node: &GetEdgesNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let edge_type = if node.edge_type().is_empty() {
            None
        } else {
            Some(node.edge_type().to_string())
        };

        let space_name = {
            let storage_guard = storage.read();
            match storage_guard.get_space_by_id(node.space_id()) {
                Ok(Some(space_info)) => space_info.space_name,
                _ => "default".to_string(),
            }
        };

        let rank: i64 = node.rank().parse().unwrap_or(0);

        let executor = GetEdgesExecutor::new(
            node.id(),
            storage,
            edge_type,
            context.expression_context().clone(),
        )
        .with_src(node.src().to_string())
        .with_dst(node.dst().to_string())
        .with_rank(rank)
        .with_space_name(space_name);

        Ok(ExecutorEnum::GetEdges(executor))
    }

    /// Building the IndexScan executor (for scanning tag indexes)
    pub fn build_index_scan(
        node: &IndexScanNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = IndexScanExecutor::new(
            ExecutorConfig::new(node.id(), storage, context.expression_context().clone()),
            IndexScanConfig {
                space_id: node.space_id(),
                tag_id: node.tag_id(),
                index_id: node.index_id(),
                index_name: node.index_name().to_string(),
                schema_name: node.schema_name().to_string(),
                scan_type: node.scan_type().as_str().to_string(),
                scan_limits: node.scan_limits().to_vec(),
                filter: node.filter().and_then(|f| f.get_expression()),
                return_columns: node.return_columns().to_vec(),
                limit: node.limit().map(|l| l as usize),
                is_edge: false,
            },
        );
        Ok(ExecutorEnum::IndexScan(executor))
    }
}

impl<S: StorageClient + 'static> Default for DataAccessBuilder<S> {
    fn default() -> Self {
        Self::new()
    }
}
