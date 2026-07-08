use std::sync::Arc;

use super::super::base::{BaseExecutor, EdgeDirection, ExecutorStats, PathConfig};
use crate::core::error::DBError;
use crate::core::types::VertexId;
use crate::core::{Path, Step, Value};
use crate::query::executor::base::{DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::StorageReader;
use parking_lot::RwLock;

#[derive(Debug)]
pub struct AllPathsExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
    start_vertex: Value,
    end_vertex: Option<Value>,
    max_hops: usize,
    edge_types: Option<Vec<String>>,
    direction: EdgeDirection,
    space_name: String,
}

impl<S: StorageReader> AllPathsExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionAnalysisContext>,
        config: PathConfig,
        space_name: String,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "AllPathsExecutor".to_string(), storage, expr_context),
            start_vertex: config.start_vertex,
            end_vertex: config.end_vertex,
            max_hops: config.max_hops,
            edge_types: config.edge_types,
            direction: config.direction,
            space_name,
        }
    }
}

impl<S: StorageReader> Executor<S> for AllPathsExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let storage = self.get_storage().read();

        let mut all_paths: Vec<Path> = Vec::new();

        let start_vid = VertexId::try_from(&self.start_vertex).map_err(DBError::from)?;

        let start_vertex_obj =
            if let Some(vertex) = storage.get_vertex(&self.space_name, &start_vid)? {
                vertex
            } else {
                return Ok(ExecutionResult::DataSet(DataSet::new()));
            };

        let mut current_paths: Vec<Path> = vec![Path {
            src: Box::new(start_vertex_obj.clone()),
            steps: Vec::new(),
        }];

        for _hop in 0..self.max_hops {
            let mut next_paths: Vec<Path> = Vec::new();

            for path in &current_paths {
                let direction = self.direction;

                let edges = storage.get_node_edges(&self.space_name, &start_vid, direction)?;

                for edge in edges {
                    let neighbor_id = edge.dst;

                    if let Some(ref _end_vertex) = self.end_vertex {
                        continue;
                    }

                    if let Some(ref edge_types) = self.edge_types {
                        if !edge_types.contains(&edge.edge_type) {
                            continue;
                        }
                    }

                    if let Some(neighbor) = storage.get_vertex(&self.space_name, &neighbor_id)? {
                        let mut new_path = path.clone();
                        new_path.steps.push(Step {
                            dst: Box::new(neighbor),
                            edge: Box::new(edge),
                        });

                        next_paths.push(new_path.clone());
                        all_paths.push(new_path);
                    }
                }
            }

            current_paths = next_paths;
            if current_paths.is_empty() {
                break;
            }
        }

        let rows: Vec<Vec<Value>> = all_paths
            .into_iter()
            .map(|path| vec![Value::path(path)])
            .collect();
        let dataset = DataSet::from_rows(rows, vec!["path".to_string()]);
        Ok(ExecutionResult::DataSet(dataset))
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
        "All paths executor - finds all paths between vertices"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader> HasStorage<S> for AllPathsExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.storage.as_ref().expect("Storage not initialized")
    }
}
