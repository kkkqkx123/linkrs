use std::sync::Arc;
use std::time::Instant;

use super::super::base::{BaseExecutor, EdgeDirection, ExecutorStats};
use crate::core::error::DBError;
use crate::core::types::VertexId;
use crate::core::Value;
use crate::query::executor::base::{DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::StorageReader;
use parking_lot::RwLock;

pub struct GetNeighborsExecutor<S: StorageReader + 'static> {
    base: BaseExecutor<S>,
    vertex_ids: Vec<Value>,
    edge_direction: EdgeDirection,
    edge_types: Option<Vec<String>>,
    space_name: String,
}

impl<S: StorageReader> GetNeighborsExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        vertex_ids: Vec<Value>,
        edge_direction: EdgeDirection,
        edge_types: Option<Vec<String>>,
        expr_context: Arc<ExpressionAnalysisContext>,
        space_name: String,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "GetNeighborsExecutor".to_string(),
                storage,
                expr_context,
            ),
            vertex_ids,
            edge_direction,
            edge_types,
            space_name,
        }
    }
}

impl<S: StorageReader + 'static> Executor<S> for GetNeighborsExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start = Instant::now();
        let result = self.do_execute();
        let elapsed = start.elapsed();
        self.base.get_stats_mut().add_total_time(elapsed);
        match result {
            Ok(values) => {
                let dataset = DataSet::from_rows(
                    values.into_iter().map(|v| vec![v]).collect(),
                    vec!["value".to_string()],
                );
                Ok(ExecutionResult::DataSet(dataset))
            }
            Err(e) => Err(e),
        }
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
        "GetNeighborsExecutor"
    }

    fn description(&self) -> &str {
        "Get neighbors executor - retrieves neighboring vertices"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader> HasStorage<S> for GetNeighborsExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageReader + 'static> GetNeighborsExecutor<S> {
    fn do_execute(&mut self) -> DBResult<Vec<Value>> {
        if self.vertex_ids.is_empty() {
            return Ok(Vec::new());
        }

        let storage = self.get_storage().read();
        let mut neighbor_ids: Vec<VertexId> = Vec::new();
        let edge_types_filter = self.edge_types.as_ref();
        let direction = self.edge_direction;

        for vertex_id in &self.vertex_ids {
            let vid = VertexId::try_from(vertex_id).map_err(DBError::from)?;
            let edges = storage.get_node_edges(&self.space_name, &vid, direction)?;

            for edge in edges {
                if let Some(filter_types) = edge_types_filter {
                    if !filter_types.contains(&edge.edge_type) {
                        continue;
                    }
                }

                let neighbor_id = if edge.src == vid { edge.dst } else { edge.src };

                neighbor_ids.push(neighbor_id);
            }
        }

        neighbor_ids.sort();
        neighbor_ids.dedup();

        if neighbor_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut neighbors: Vec<Value> = Vec::new();
        let mut failed_count = 0;

        for neighbor_id in &neighbor_ids {
            match storage.get_vertex(&self.space_name, neighbor_id) {
                Ok(Some(vertex)) => {
                    neighbors.push(crate::core::Value::Vertex(Box::new(vertex)));
                }
                Ok(None) => {
                    failed_count += 1;
                }
                Err(_) => {
                    failed_count += 1;
                }
            }
        }

        if failed_count > 0 {
            log::warn!("Failed to get neighbor vertices: {} ", failed_count);
        }

        Ok(neighbors)
    }
}
