use std::sync::Arc;
use std::time::Instant;

use super::super::base::{BaseExecutor, ExecutorStats};
use crate::core::error::DBError;
use crate::core::types::VertexId;
use crate::core::{vertex_edge_path, Value};
use crate::query::executor::base::{DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::StorageReader;
use parking_lot::RwLock;

/// Parameters for creating GetVerticesExecutor
pub struct GetVerticesParams {
    pub space_name: String,
    pub vertex_ids: Option<Vec<Value>>,
    pub tag_filter: Option<crate::core::Expression>,
    pub vertex_filter: Option<crate::core::Expression>,
    pub limit: Option<usize>,
    pub col_names: Vec<String>,
}

impl GetVerticesParams {
    pub fn new(space_name: String) -> Self {
        Self {
            space_name,
            vertex_ids: None,
            tag_filter: None,
            vertex_filter: None,
            limit: None,
            col_names: vec!["vertex".to_string()],
        }
    }
}

pub struct GetVerticesExecutor<S: StorageReader + 'static> {
    base: BaseExecutor<S>,
    space_name: String,
    vertex_ids: Option<Vec<Value>>,
    tag_filter: Option<crate::core::Expression>,
    vertex_filter: Option<crate::core::Expression>,
    limit: Option<usize>,
    col_names: Vec<String>,
}

impl<S: StorageReader + 'static> GetVerticesExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        params: GetVerticesParams,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        let col_names = if params.col_names.is_empty() {
            vec!["vertex".to_string()]
        } else {
            params.col_names.clone()
        };
        Self {
            base: BaseExecutor::new(id, "GetVerticesExecutor".to_string(), storage, expr_context),
            space_name: params.space_name,
            vertex_ids: params.vertex_ids,
            tag_filter: params.tag_filter,
            vertex_filter: params.vertex_filter,
            limit: params.limit,
            col_names,
        }
    }
}

impl<S: StorageReader + 'static> Executor<S> for GetVerticesExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start = Instant::now();

        let result = self.do_execute();

        let elapsed = start.elapsed();
        self.base.get_stats_mut().add_total_time(elapsed);

        match result {
            Ok(vertices) => {
                let rows: Vec<Vec<Value>> = vertices
                    .into_iter()
                    .map(|v| vec![Value::Vertex(Box::new(v))])
                    .collect();
                let dataset = DataSet::from_rows(rows, self.col_names.clone());
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
        "GetVerticesExecutor"
    }

    fn description(&self) -> &str {
        "Get vertices executor - retrieves vertices from storage"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader> HasStorage<S> for GetVerticesExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageReader + 'static> GetVerticesExecutor<S> {
    fn do_execute(&mut self) -> DBResult<Vec<vertex_edge_path::Vertex>> {
        match &self.vertex_ids {
            Some(ids) if ids.len() > 1 => {
                let storage = self.get_storage().read();
                let mut result_vertices: Vec<vertex_edge_path::Vertex> = Vec::new();
                let mut failed_count = 0;

                for id in ids {
                    let vid = VertexId::try_from(id).map_err(DBError::from)?;
                    match storage.get_vertex(&self.space_name, &vid) {
                        Ok(Some(vertex)) => {
                            let include_vertex =
                                if let Some(ref tag_filter_expression) = self.tag_filter {
                                    crate::query::executor::utils::tag_filter::TagFilterProcessor
                                    ::process_tag_filter(tag_filter_expression, &vertex)
                                } else {
                                    true
                                };

                            if include_vertex {
                                result_vertices.push(vertex);
                            }
                        }
                        Ok(None) => {
                            failed_count += 1;
                        }
                        Err(_) => {
                            failed_count += 1;
                        }
                    }

                    if let Some(limit) = self.limit {
                        if result_vertices.len() >= limit {
                            break;
                        }
                    }
                }

                if failed_count > 0 {
                    log::warn!("Failed to get vertices: {} ", failed_count);
                }

                Ok(result_vertices)
            }
            Some(ids) if ids.len() == 1 => {
                let storage = self.get_storage().read();

                let vid = VertexId::try_from(&ids[0]).map_err(DBError::from)?;
                match storage.get_vertex(&self.space_name, &vid) {
                    Ok(Some(vertex)) => Ok(vec![vertex]),
                    Ok(None) => Ok(vec![]),
                    Err(e) => Err(crate::core::error::DBError::from(e)),
                }
            }
            Some(_) => Ok(Vec::new()),
            None => {
                let storage = self.get_storage().read();

                let vertices = storage.scan_vertices(&self.space_name)?
                    .into_iter()
                    .filter(|vertex| {
                        if let Some(ref tag_filter_expression) = self.tag_filter {
                            crate::query::executor::utils::tag_filter::TagFilterProcessor
                                ::process_tag_filter(tag_filter_expression, vertex)
                        } else {
                            true
                        }
                    })
                    .filter(|vertex| {
                        if let Some(ref filter_expression) = self.vertex_filter {
                            let mut context =
                                crate::query::executor::expression::DefaultExpressionContext::new();
                            context.set_variable(
                                "vertex".to_string(),
                                crate::core::Value::Vertex(Box::new(vertex.clone())),
                            );

                            match crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator::evaluate(filter_expression, &mut context) {
                                Ok(value) => {
                                    match value {
                                        crate::core::Value::Bool(b) => b,
                                        crate::core::Value::SmallInt(i) => i != 0,
                                        crate::core::Value::Int(i) => i != 0,
                                        crate::core::Value::BigInt(i) => i != 0,
                                        crate::core::Value::Float(f) => f != 0.0,
                                        crate::core::Value::Double(f) => f != 0.0,
                                        crate::core::Value::Decimal128(d) => !d.is_zero(),
                                        crate::core::Value::String(s) => !s.is_empty(),
                                        crate::core::Value::FixedString { data, .. } => !data.is_empty(),
                                        crate::core::Value::Blob(b) => !b.is_empty(),
                                        crate::core::Value::List(l) => !l.is_empty(),
                                        crate::core::Value::Map(m) => !m.is_empty(),
                                        crate::core::Value::Set(s) => !s.is_empty(),
                                        crate::core::Value::Vertex(_) => true,
                                        crate::core::Value::Edge(_) => true,
                                        crate::core::Value::Path(_) => true,
                                        crate::core::Value::Null(_) => false,
                                        crate::core::Value::Empty => false,
                                        crate::core::Value::Date(_) => true,
                                        crate::core::Value::Time(_) => true,
                                        crate::core::Value::DateTime(_) => true,
                                        crate::core::Value::Geography(_) => true,
                                        crate::core::Value::Vector(_) => true,
                                        crate::core::Value::DataSet(_) => true,
                                        crate::core::Value::Json(_) => true,
                                        crate::core::Value::JsonB(_) => true,
                                        crate::core::Value::Uuid(_) => true,
                                        crate::core::Value::Interval(_) => true,
                                    }
                                }
                                Err(e) => {
                                    log::warn!("Vertex filter expression evaluation failed: {}", e);
                                    false
                                }
                            }
                        } else {
                            true
                        }
                    })
                    .take(self.limit.unwrap_or(usize::MAX))
                    .collect();
                Ok(vertices)
            }
        }
    }
}

pub struct ScanVerticesExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
    tag_filter: Option<crate::core::Expression>,
    vertex_filter: Option<crate::core::Expression>,
    limit: Option<usize>,
    col_names: Vec<String>,
}

impl<S: StorageReader> ScanVerticesExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        tag_filter: Option<crate::core::Expression>,
        vertex_filter: Option<crate::core::Expression>,
        limit: Option<usize>,
        expr_context: Arc<ExpressionAnalysisContext>,
        col_names: Vec<String>,
    ) -> Self {
        let col_names = if col_names.is_empty() {
            vec!["vertex".to_string()]
        } else {
            col_names
        };
        Self {
            base: BaseExecutor::new(
                id,
                "ScanVerticesExecutor".to_string(),
                storage,
                expr_context,
            ),
            tag_filter,
            vertex_filter,
            limit,
            col_names,
        }
    }
}

impl<S: StorageReader> Executor<S> for ScanVerticesExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start = Instant::now();
        let result = self.do_execute();
        let elapsed = start.elapsed();
        self.base.get_stats_mut().add_total_time(elapsed);
        match result {
            Ok(vertices) => {
                let rows: Vec<Vec<Value>> = vertices
                    .into_iter()
                    .map(|v| vec![Value::Vertex(Box::new(v))])
                    .collect();
                let dataset = DataSet::from_rows(rows, self.col_names.clone());
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
        "ScanVerticesExecutor"
    }

    fn description(&self) -> &str {
        "Scan vertices executor - scans all vertices from storage"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader> HasStorage<S> for ScanVerticesExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageReader> ScanVerticesExecutor<S> {
    fn do_execute(&mut self) -> DBResult<Vec<vertex_edge_path::Vertex>> {
        let storage = self.get_storage().read();

        let mut vertices: Vec<vertex_edge_path::Vertex> = storage.scan_vertices("default")?
            .into_iter()
            .filter(|vertex| {
                if let Some(ref tag_filter_expression) = self.tag_filter {
                    crate::query::executor::utils::tag_filter::TagFilterProcessor
                        ::process_tag_filter(tag_filter_expression, vertex)
                } else {
                    true
                }
            })
            .filter(|vertex| {
                if let Some(ref filter_expression) = self.vertex_filter {
                    let mut context = crate::query::executor::expression::DefaultExpressionContext::new();
                    context.set_variable(
                        "vertex".to_string(),
                        crate::core::Value::Vertex(Box::new(vertex.clone())),
                    );

                    match crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator::evaluate(filter_expression, &mut context) {
                        Ok(value) => {
                            match value {
                                crate::core::Value::Bool(b) => b,
                                _ => false,
                            }
                        }
                        Err(_) => false,
                    }
                } else {
                    true
                }
            })
            .collect();

        if let Some(limit) = self.limit {
            vertices.truncate(limit);
        }

        Ok(vertices)
    }
}
