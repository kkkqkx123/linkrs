//! Delete the executor.
//!
//! Responsible for deleting vertex and edge data.
//! Supports both standalone deletion and pipe-based deletion (e.g., GO ... | DELETE VERTEX $-.id).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::core::error::DBError;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::VertexId;
use crate::core::Value;
use crate::query::executor::base::{BaseExecutor, ExecutorStats};
use crate::query::executor::base::{DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::executor::expression::evaluation_context::DefaultExpressionContext;
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::{StorageClient, StorageReader, StorageSchemaOps, StorageWriter};
use parking_lot::RwLock;

/// Delete the executor.
///
/// Responsible for deleting vertices and edges
pub struct DeleteExecutor<S: StorageReader + StorageWriter + StorageSchemaOps> {
    base: BaseExecutor<S>,
    vertex_ids: Option<Vec<Value>>,
    edge_ids: Option<Vec<(Value, Value, String)>>,
    condition: Option<ContextualExpression>,
    with_edge: bool,
    space_name: String,
    tag_names: Option<Vec<String>>,
    is_all_tags: bool,
    index_name: Option<String>,
}

impl<S: StorageReader + StorageWriter + StorageSchemaOps> DeleteExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        vertex_ids: Option<Vec<Value>>,
        edge_ids: Option<Vec<(Value, Value, String)>>,
        condition: Option<ContextualExpression>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "DeleteExecutor".to_string(), storage, expr_context),
            vertex_ids,
            edge_ids,
            condition,
            with_edge: false,
            space_name: "default".to_string(),
            tag_names: None,
            is_all_tags: false,
            index_name: None,
        }
    }

    pub fn with_edge(mut self, with_edge: bool) -> Self {
        self.with_edge = with_edge;
        self
    }

    pub fn with_space(mut self, space_name: String) -> Self {
        self.space_name = space_name;
        self
    }

    pub fn with_tag_names(mut self, tag_names: Vec<String>) -> Self {
        self.tag_names = Some(tag_names);
        self
    }

    pub fn with_all_tags(mut self, is_all_tags: bool) -> Self {
        self.is_all_tags = is_all_tags;
        self
    }

    pub fn with_index_name(mut self, index_name: String) -> Self {
        self.index_name = Some(index_name);
        self
    }
}

impl<S: StorageReader + StorageWriter + StorageSchemaOps + Send + Sync + 'static> Executor<S>
    for DeleteExecutor<S>
{
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
        "DeleteExecutor"
    }

    fn description(&self) -> &str {
        "Delete executor - deletes vertices and edges from storage"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader + StorageWriter + StorageSchemaOps> HasStorage<S> for DeleteExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageReader + StorageWriter + StorageSchemaOps + Send + Sync + 'static>
    DeleteExecutor<S>
{
    fn do_execute(&mut self) -> DBResult<usize> {
        let mut total_deleted = 0;

        let condition_expression = self.condition.as_ref().and_then(|c| c.get_expression());

        if let Some(ids) = &self.vertex_ids {
            let mut storage = self.get_storage().write();
            for id in ids {
                let vid = VertexId::try_from(id).map_err(DBError::from)?;
                let should_delete = if let Some(ref expression) = condition_expression {
                    if let Ok(Some(vertex)) = storage.get_vertex(&self.space_name, &vid) {
                        let mut context = DefaultExpressionContext::new();
                        context.set_variable("VID".to_string(), id.clone());
                        for (key, value) in &vertex.properties {
                            context.set_variable(key.clone(), value.clone());
                        }

                        let result = ExpressionEvaluator::evaluate(expression, &mut context)
                            .map_err(|e| {
                                crate::core::error::DBError::query(format!(
                                    "Condition evaluation failed: {}",
                                    e
                                ))
                            })?;

                        match result {
                            crate::core::Value::Bool(b) => b,
                            _ => true,
                        }
                    } else {
                        true
                    }
                } else {
                    true
                };

                if should_delete {
                    if self.with_edge {
                        let edges = storage
                            .get_node_edges(
                                &self.space_name,
                                &vid,
                                crate::core::EdgeDirection::Both,
                            )
                            .map_err(|e| {
                                crate::core::error::DBError::storage(format!(
                                    "Failed to retrieve associated edges: {}",
                                    e
                                ))
                            })?;
                        for edge in edges {
                            StorageWriter::delete_edge(
                                &mut *storage,
                                &self.space_name,
                                &edge.src,
                                &edge.dst,
                                &edge.edge_type,
                                edge.ranking,
                            )
                            .map_err(|e| {
                                crate::core::error::DBError::storage(format!(
                                    "Failed to delete the associated edge: {}",
                                    e
                                ))
                            })?;
                            total_deleted += 1;
                        }
                    }

                    if StorageWriter::delete_vertex(&mut *storage, &self.space_name, &vid).is_ok() {
                        total_deleted += 1;
                    }
                }
            }
        }

        if let Some(edges) = &self.edge_ids {
            let mut storage = self.get_storage().write();
            for (src, dst, edge_type) in edges {
                // Handle Value::Edge: extract src/dst from the edge value
                // This is needed for MATCH ... DELETE EDGE e where e is an edge variable
                let src_vid = match src {
                    Value::Edge(ref e) => e.src,
                    other => VertexId::try_from(other).map_err(DBError::from)?,
                };
                let dst_vid = match dst {
                    Value::Edge(ref e) => e.dst,
                    other => VertexId::try_from(other).map_err(DBError::from)?,
                };
                let should_delete = if let Some(ref expression) = condition_expression {
                    if let Ok(Some(edge)) =
                        storage.get_edge(&self.space_name, &src_vid, &dst_vid, edge_type, 0)
                    {
                        let mut context = DefaultExpressionContext::new();
                        context.set_variable("SRC".to_string(), src.clone());
                        context.set_variable("DST".to_string(), dst.clone());
                        context.set_variable(
                            "edge_type".to_string(),
                            crate::core::Value::String(edge_type.clone()),
                        );
                        for (key, value) in &edge.props {
                            context.set_variable(key.clone(), value.clone());
                        }

                        let result = ExpressionEvaluator::evaluate(expression, &mut context)
                            .map_err(|e| {
                                crate::core::error::DBError::query(format!(
                                    "Condition evaluation failed: {}",
                                    e
                                ))
                            })?;

                        match result {
                            crate::core::Value::Bool(b) => b,
                            _ => true,
                        }
                    } else {
                        true
                    }
                } else {
                    true
                };

                if should_delete {
                    let edges = storage
                        .scan_edges_by_type(&self.space_name, edge_type)
                        .map_err(DBError::from)?;
                    for edge in edges {
                        if edge.src == src_vid && edge.dst == dst_vid {
                            storage
                                .delete_edge(
                                    &self.space_name,
                                    &src_vid,
                                    &dst_vid,
                                    edge_type,
                                    edge.ranking,
                                )
                                .map_err(DBError::from)?;
                            total_deleted += 1;
                            break;
                        }
                    }
                }
            }
        }

        if let Some(tag_names) = &self.tag_names {
            if let Some(ids) = &self.vertex_ids {
                let mut storage = self.get_storage().write();
                for id in ids {
                    let vid = VertexId::try_from(id).map_err(DBError::from)?;
                    if self.is_all_tags {
                        if let Ok(Some(vertex)) = storage.get_vertex(&self.space_name, &vid) {
                            for tag in &vertex.tags {
                                if storage.drop_tag(&self.space_name, &tag.name).is_ok() {
                                    total_deleted += 1;
                                }
                            }
                        }
                    } else {
                        for tag_name in tag_names {
                            if storage.drop_tag(&self.space_name, tag_name).is_ok() {
                                total_deleted += 1;
                            }
                        }
                    }
                }
            }
        }

        if let Some(index_name) = &self.index_name {
            let mut storage = self.get_storage().write();
            if storage.drop_tag_index(&self.space_name, index_name).is_ok() {
                total_deleted += 1;
            }
        }

        Ok(total_deleted)
    }
}

/// Pipe Delete Executor
///
/// Handles DELETE statements that receive input from a pipe.
/// Evaluates expressions against input rows to determine what to delete.
pub struct PipeDeleteExecutor<S: StorageClient + 'static> {
    base: BaseExecutor<S>,
    vertex_id_expressions: Vec<ContextualExpression>,
    edge_expressions: Vec<(
        ContextualExpression,
        ContextualExpression,
        Option<ContextualExpression>,
    )>,
    edge_type: Option<String>,
    condition: Option<ContextualExpression>,
    with_edge: bool,
    space_name: String,
    input_data: Option<DataSet>,
    input_executor: Option<Box<crate::query::executor::ExecutorEnum<S>>>,
}

impl<S: StorageClient + 'static> PipeDeleteExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "PipeDeleteExecutor".to_string(), storage, expr_context),
            vertex_id_expressions: vec![],
            edge_expressions: vec![],
            edge_type: None,
            condition: None,
            with_edge: false,
            space_name: "default".to_string(),
            input_data: None,
            input_executor: None,
        }
    }

    pub fn with_vertex_expressions(mut self, expressions: Vec<ContextualExpression>) -> Self {
        self.vertex_id_expressions = expressions;
        self
    }

    pub fn with_edge_expressions(
        mut self,
        expressions: Vec<(
            ContextualExpression,
            ContextualExpression,
            Option<ContextualExpression>,
        )>,
    ) -> Self {
        self.edge_expressions = expressions;
        self
    }

    pub fn with_edge_type(mut self, edge_type: Option<String>) -> Self {
        self.edge_type = edge_type;
        self
    }

    pub fn with_edge_flag(mut self, with_edge: bool) -> Self {
        self.with_edge = with_edge;
        self
    }

    pub fn with_space(mut self, space_name: String) -> Self {
        self.space_name = space_name;
        self
    }

    pub fn with_condition(mut self, condition: Option<ContextualExpression>) -> Self {
        self.condition = condition;
        self
    }

    pub fn with_input_data(mut self, data: DataSet) -> Self {
        self.input_data = Some(data);
        self
    }
}

impl<S: StorageClient + Send + Sync + 'static> crate::query::executor::base::InputExecutor<S>
    for PipeDeleteExecutor<S>
{
    fn set_input(&mut self, input: crate::query::executor::ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&crate::query::executor::ExecutorEnum<S>> {
        self.input_executor.as_ref().map(|b| b.as_ref())
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for PipeDeleteExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start = Instant::now();

        if let Some(mut input_exec) = self.input_executor.take() {
            let input_result = input_exec.execute()?;
            if let crate::query::executor::base::ExecutionResult::DataSet(data) = input_result {
                self.input_data = Some(data);
            }
        }

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
        "PipeDeleteExecutor"
    }

    fn description(&self) -> &str {
        "Pipe delete executor - deletes vertices and edges based on pipe input"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> HasStorage<S> for PipeDeleteExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageClient + Send + Sync + 'static> PipeDeleteExecutor<S> {
    fn do_execute(&mut self) -> DBResult<usize> {
        let mut total_deleted = 0;

        let input_data = self
            .input_data
            .as_ref()
            .ok_or_else(|| DBError::query("PipeDeleteExecutor requires input data".to_string()))?;

        let col_names = &input_data.col_names;

        if !self.vertex_id_expressions.is_empty() {
            let mut storage = self.get_storage().write();
            for row in &input_data.rows {
                for vid_expr in &self.vertex_id_expressions {
                    let id = self.evaluate_expression_with_row(vid_expr, col_names, row)?;
                    let vid = VertexId::try_from(&id).map_err(DBError::from)?;

                    let should_delete = self.check_condition(&storage, &id)?;

                    if should_delete {
                        if self.with_edge {
                            let edges = storage
                                .get_node_edges(
                                    &self.space_name,
                                    &vid,
                                    crate::core::EdgeDirection::Both,
                                )
                                .map_err(|e| {
                                    DBError::storage(format!(
                                        "Failed to retrieve associated edges: {}",
                                        e
                                    ))
                                })?;
                            for edge in edges {
                                StorageWriter::delete_edge(
                                    &mut *storage,
                                    &self.space_name,
                                    &edge.src,
                                    &edge.dst,
                                    &edge.edge_type,
                                    edge.ranking,
                                )
                                .map_err(|e| {
                                    DBError::storage(format!(
                                        "Failed to delete the associated edge: {}",
                                        e
                                    ))
                                })?;
                                total_deleted += 1;
                            }
                        }

                        match StorageWriter::delete_vertex(&mut *storage, &self.space_name, &vid) {
                            Ok(_) => {
                                total_deleted += 1;
                            }
                            Err(e) => {
                                log::warn!("PipeDeleteExecutor: delete_vertex failed: {:?}", e);
                            }
                        }
                    }
                }
            }
        }

        if !self.edge_expressions.is_empty() {
            let mut storage = self.get_storage().write();

            for row in &input_data.rows {
                for (src_expr, dst_expr, _rank_expr) in &self.edge_expressions {
                    let src = self.evaluate_expression_with_row(src_expr, col_names, row)?;
                    let dst = self.evaluate_expression_with_row(dst_expr, col_names, row)?;

                    // Determine edge type: first try explicit edge_type, then try to extract
                    // from Value::Edge (needed for MATCH ... DELETE EDGE e)
                    let edge_type = self
                        .edge_type
                        .clone()
                        .or_else(|| match &src {
                            Value::Edge(e) => Some(e.edge_type.clone()),
                            _ => None,
                        })
                        .or_else(|| match &dst {
                            Value::Edge(e) => Some(e.edge_type.clone()),
                            _ => None,
                        })
                        .unwrap_or_else(|| "UNKNOWN".to_string());

                    // Handle Value::Edge: extract src/dst from the edge value
                    // This is needed for MATCH ... DELETE EDGE e where e is an edge variable
                    let src_vid = match &src {
                        Value::Edge(e) => e.src,
                        other => VertexId::try_from(other).map_err(DBError::from)?,
                    };
                    let dst_vid = match &dst {
                        Value::Edge(e) => e.dst,
                        other => VertexId::try_from(other).map_err(DBError::from)?,
                    };

                    let edges = storage
                        .scan_edges_by_type(&self.space_name, &edge_type)
                        .map_err(DBError::from)?;

                    for edge in &edges {
                        if edge.src == src_vid && edge.dst == dst_vid {
                            StorageWriter::delete_edge(
                                &mut *storage,
                                &self.space_name,
                                &src_vid,
                                &dst_vid,
                                &edge_type,
                                edge.ranking,
                            )
                            .map_err(DBError::from)?;
                            total_deleted += 1;
                            break;
                        }
                    }
                }
            }
        }

        Ok(total_deleted)
    }

    fn evaluate_expression_with_row(
        &self,
        expr: &ContextualExpression,
        col_names: &[String],
        row: &[Value],
    ) -> DBResult<Value> {
        let expression = expr.get_expression().ok_or_else(|| {
            DBError::query("Expression not found in ContextualExpression".to_string())
        })?;

        let mut context = DefaultExpressionContext::new();

        for (i, col_name) in col_names.iter().enumerate() {
            if i < row.len() {
                context.set_variable(col_name.clone(), row[i].clone());
            }
        }

        // Set $- as a map of all column values for pipe references like $-.id
        if !col_names.is_empty() {
            let mut pipe_map = HashMap::new();
            for (i, col_name) in col_names.iter().enumerate() {
                if i < row.len() {
                    pipe_map.insert(col_name.clone(), row[i].clone());
                }
            }
            context.set_variable("$-".to_string(), Value::Map(Box::new(pipe_map)));
        }

        ExpressionEvaluator::evaluate(&expression, &mut context)
            .map_err(|e| DBError::query(format!("Expression evaluation failed: {}", e)))
    }

    fn check_condition(&self, _storage: &S, _id: &Value) -> DBResult<bool> {
        Ok(true)
    }
}
