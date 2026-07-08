//! Insert the actuator.
//!
//! Responsible for inserting vertex and edge data into the storage layer.

use std::sync::Arc;
use std::time::Instant;

use crate::core::{Edge, Value, Vertex};
use crate::query::executor::base::{BaseExecutor, ExecutorStats};
use crate::query::executor::base::{DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::{StorageReader, StorageWriter};
use parking_lot::RwLock;

/// Insert the actuator.
///
/// Responsible for inserting new vertex and edge data
pub struct InsertExecutor<S: StorageReader + StorageWriter> {
    base: BaseExecutor<S>,
    space_name: String,
    vertex_data: Option<Vec<Vertex>>,
    edge_data: Option<Vec<Edge>>,
    if_not_exists: bool,
}

impl<S: StorageReader + StorageWriter> InsertExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        vertex_data: Option<Vec<Vertex>>,
        edge_data: Option<Vec<Edge>>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "InsertExecutor".to_string(), storage, expr_context),
            space_name,
            vertex_data,
            edge_data,
            if_not_exists: false,
        }
    }

    pub fn with_vertices(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        vertex_data: Vec<Vertex>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "InsertExecutor".to_string(), storage, expr_context),
            space_name,
            vertex_data: Some(vertex_data),
            edge_data: None,
            if_not_exists: false,
        }
    }

    pub fn with_edges(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        edge_data: Vec<Edge>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "InsertExecutor".to_string(), storage, expr_context),
            space_name,
            vertex_data: None,
            edge_data: Some(edge_data),
            if_not_exists: false,
        }
    }

    /// Create an InsertExecutor with the IF NOT EXISTS option
    pub fn with_vertices_if_not_exists(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        vertex_data: Vec<Vertex>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "InsertExecutor".to_string(), storage, expr_context),
            space_name,
            vertex_data: Some(vertex_data),
            edge_data: None,
            if_not_exists: true,
        }
    }

    /// Create an InsertExecutor with the IF NOT EXISTS option (for edges)
    pub fn with_edges_if_not_exists(
        id: i64,
        storage: Arc<RwLock<S>>,
        space_name: String,
        edge_data: Vec<Edge>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "InsertExecutor".to_string(), storage, expr_context),
            space_name,
            vertex_data: None,
            edge_data: Some(edge_data),
            if_not_exists: true,
        }
    }
}

impl<S: StorageReader + StorageWriter + Send + Sync + 'static> Executor<S> for InsertExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start = Instant::now();
        let result = self.do_execute();
        let elapsed = start.elapsed();
        self.base.get_stats_mut().add_total_time(elapsed);
        match result {
            Ok(count) => {
                let dataset = DataSet::from_rows(
                    vec![vec![Value::BigInt(count as i64)]],
                    vec!["count".to_string()],
                );
                Ok(ExecutionResult::DataSet(dataset))
            }
            Err(e) => Err(e),
        }
    }

    fn open(&mut self) -> Result<(), crate::core::DBError> {
        Ok(())
    }

    fn close(&mut self) -> Result<(), crate::core::DBError> {
        Ok(())
    }

    fn is_open(&self) -> bool {
        true
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        "InsertExecutor"
    }

    fn description(&self) -> &str {
        "Insert executor - inserts vertices and edges into storage"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader + StorageWriter> HasStorage<S> for InsertExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageReader + StorageWriter + Send + Sync + 'static> InsertExecutor<S> {
    fn do_execute(&mut self) -> DBResult<usize> {
        let mut total_inserted = 0;

        if let Some(vertices) = &self.vertex_data {
            let mut storage = self.get_storage().write();
            for vertex in vertices.iter() {
                // If IF NOT EXISTS is enabled, check whether the vertex already exists.
                if self.if_not_exists
                    && storage.get_vertex(&self.space_name, &vertex.vid)?.is_some()
                {
                    // The vertex already exists; the insertion step will be skipped.
                    continue;
                }
                storage.insert_vertex(&self.space_name, vertex.clone())?;
                total_inserted += 1;
            }
        }

        if let Some(edges) = &self.edge_data {
            let mut storage = self.get_storage().write();
            for edge in edges {
                if self.if_not_exists
                    && storage
                        .get_edge(
                            &self.space_name,
                            &edge.src,
                            &edge.dst,
                            &edge.edge_type,
                            edge.ranking,
                        )?
                        .is_some()
                {
                    continue;
                }
                storage.insert_edge(&self.space_name, edge.clone())?;
                total_inserted += 1;
            }
        }

        Ok(total_inserted)
    }
}
