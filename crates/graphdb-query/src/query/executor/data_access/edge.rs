use std::sync::Arc;
use std::time::Instant;

use super::super::base::{BaseExecutor, ExecutorStats};
use crate::core::types::storage_ids::VertexId;
use crate::core::vertex_edge_path;
use crate::core::Value;
use crate::query::executor::base::{DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::StorageReader;
use parking_lot::RwLock;

pub struct GetEdgesExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
    edge_type: Option<String>,
    src: Option<String>,
    dst: Option<String>,
    rank: i64,
    space_name: String,
}

impl<S: StorageReader> GetEdgesExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        edge_type: Option<String>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "GetEdgesExecutor".to_string(), storage, expr_context),
            edge_type,
            src: None,
            dst: None,
            rank: 0,
            space_name: "default".to_string(),
        }
    }

    pub fn with_src(mut self, src: String) -> Self {
        self.src = Some(src);
        self
    }

    pub fn with_dst(mut self, dst: String) -> Self {
        self.dst = Some(dst);
        self
    }

    pub fn with_rank(mut self, rank: i64) -> Self {
        self.rank = rank;
        self
    }

    pub fn with_space_name(mut self, space_name: String) -> Self {
        self.space_name = space_name;
        self
    }
}

impl<S: StorageReader> Executor<S> for GetEdgesExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start = Instant::now();
        let result = self.do_execute();
        let elapsed = start.elapsed();
        self.base.get_stats_mut().add_total_time(elapsed);
        match result {
            Ok(edges) => {
                let rows: Vec<Vec<Value>> =
                    edges.into_iter().map(|e| vec![Value::edge(e)]).collect();
                let dataset = DataSet::from_rows(rows, vec!["edge".to_string()]);
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
        "GetEdgesExecutor"
    }

    fn description(&self) -> &str {
        "Get edges executor - retrieves edges from storage"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader> HasStorage<S> for GetEdgesExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageReader> GetEdgesExecutor<S> {
    fn do_execute(&mut self) -> DBResult<Vec<vertex_edge_path::Edge>> {
        let storage = self.get_storage().read();

        if let (Some(src_str), Some(dst_str), Some(edge_type)) =
            (&self.src, &self.dst, &self.edge_type)
        {
            let src_vid = if let Ok(id) = src_str.parse::<i64>() {
                VertexId::from_int64(id)
            } else {
                VertexId::from_string(src_str.clone())
            };
            let dst_vid = if let Ok(id) = dst_str.parse::<i64>() {
                VertexId::from_int64(id)
            } else {
                VertexId::from_string(dst_str.clone())
            };

            if let Some(edge) =
                storage.get_edge(&self.space_name, &src_vid, &dst_vid, edge_type, self.rank)?
            {
                Ok(vec![edge])
            } else {
                Ok(Vec::new())
            }
        } else {
            let edges = if let Some(ref edge_type) = self.edge_type {
                storage.scan_edges_by_type(&self.space_name, edge_type)?
            } else {
                storage.scan_all_edges(&self.space_name)?
            };
            Ok(edges)
        }
    }
}

pub struct ScanEdgesExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
    edge_type: Option<String>,
    filter: Option<crate::core::Expression>,
    limit: Option<usize>,
    space_name: String,
}

impl<S: StorageReader> ScanEdgesExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        edge_type: Option<String>,
        filter: Option<crate::core::Expression>,
        limit: Option<usize>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "ScanEdgesExecutor".to_string(), storage, expr_context),
            edge_type,
            filter,
            limit,
            space_name: "default".to_string(),
        }
    }

    pub fn with_space_name(mut self, space_name: String) -> Self {
        self.space_name = space_name;
        self
    }
}

impl<S: StorageReader> Executor<S> for ScanEdgesExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start = Instant::now();
        let result = self.do_execute();
        let elapsed = start.elapsed();
        self.base.get_stats_mut().add_total_time(elapsed);
        match result {
            Ok(edges) => {
                let rows: Vec<Vec<Value>> =
                    edges.into_iter().map(|e| vec![Value::edge(e)]).collect();
                let dataset = DataSet::from_rows(rows, vec!["edge".to_string()]);
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
        "ScanEdgesExecutor"
    }

    fn description(&self) -> &str {
        "Scan edges executor - scans all edges from storage"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader> HasStorage<S> for ScanEdgesExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageReader> ScanEdgesExecutor<S> {
    fn do_execute(&mut self) -> DBResult<Vec<vertex_edge_path::Edge>> {
        let storage = self.get_storage().read();

        let mut edges: Vec<vertex_edge_path::Edge> = if let Some(ref edge_type) = self.edge_type {
            storage.scan_edges_by_type(&self.space_name, edge_type)?
        } else {
            storage.scan_all_edges(&self.space_name)?
        };

        if let Some(ref filter_expr) = self.filter {
            let mut context = crate::query::executor::expression::DefaultExpressionContext::new();
            edges.retain(|edge| {
                context.set_variable("edge".to_string(), crate::core::Value::edge(edge.clone()));
                match crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator::evaluate(filter_expr, &mut context) {
                    Ok(value) => match value {
                        crate::core::Value::Bool(b) => b,
                        crate::core::Value::Int(i) => i != 0,
                        crate::core::Value::Float(f) => f != 0.0,
                        _ => true,
                    },
                    Err(_) => true,
                }
            });
        }

        if let Some(limit) = self.limit {
            edges.truncate(limit);
        }

        Ok(edges)
    }
}
