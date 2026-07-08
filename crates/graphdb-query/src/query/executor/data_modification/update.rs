//! Update the executor.
//!
//! Responsible for updating the attributes of the existing vertices and edges.
//!
//! Functionality enhancements:
//! Support for upsert (inserting a record when the node does not exist).
//! The RETURN clause is supported to return the updated attributes.
//! Support for specifying the attribute to be returned using the YIELD keyword.
//! Support for conditional expressions
//! Better error handling and logging.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::core::error::DBError;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::VertexId;
use crate::core::{Expression, Value};
use crate::query::executor::base::{BaseExecutor, ExecutorStats};
use crate::query::executor::base::{DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::executor::expression::evaluation_context::DefaultExpressionContext;
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::{StorageReader, StorageWriter};
use parking_lot::RwLock;

/// Update the executor.
///
/// Responsible for updating the properties of vertices and edges.
pub struct UpdateExecutor<S: StorageReader + StorageWriter> {
    base: BaseExecutor<S>,
    vertex_updates: Option<Vec<VertexUpdate>>,
    edge_updates: Option<Vec<EdgeUpdate>>,
    condition: Option<ContextualExpression>,
    return_props: Option<Vec<String>>,
    yield_names: Vec<String>,
    insertable: bool,
    space_name: String,
}

/// Vertex update data structure
#[derive(Debug, Clone)]
pub struct VertexUpdate {
    pub vertex_id: Value,
    pub properties: HashMap<String, Value>,
    pub property_expressions: Option<HashMap<String, ContextualExpression>>,
    pub tags_to_add: Option<Vec<String>>,
    pub tags_to_remove: Option<Vec<String>>,
}

/// While updating the data structure…
#[derive(Debug, Clone)]
pub struct EdgeUpdate {
    pub src: Value,
    pub dst: Value,
    pub edge_type: String,
    pub rank: Option<i64>,
    pub properties: HashMap<String, Value>,
    pub property_expressions: Option<HashMap<String, ContextualExpression>>,
}

/// Update the result data structure
#[derive(Debug, Clone)]
pub struct UpdateResult {
    pub vertex_id: Option<Value>,
    pub src: Option<Value>,
    pub dst: Option<Value>,
    pub edge_type: Option<String>,
    pub returned_props: HashMap<String, Value>,
}

impl<S: StorageReader + StorageWriter> UpdateExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        vertex_updates: Option<Vec<VertexUpdate>>,
        edge_updates: Option<Vec<EdgeUpdate>>,
        condition: Option<ContextualExpression>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "UpdateExecutor".to_string(), storage, expr_context),
            vertex_updates,
            edge_updates,
            condition,
            return_props: None,
            yield_names: Vec::new(),
            insertable: false,
            space_name: "default".to_string(),
        }
    }

    pub fn with_return_props(mut self, return_props: Vec<String>) -> Self {
        self.return_props = Some(return_props);
        self
    }

    pub fn with_yield_names(mut self, yield_names: Vec<String>) -> Self {
        self.yield_names = yield_names;
        self
    }

    pub fn with_insertable(mut self, insertable: bool) -> Self {
        self.insertable = insertable;
        self
    }

    pub fn with_space(mut self, space_name: String) -> Self {
        self.space_name = space_name;
        self
    }
}

impl<S: StorageReader + StorageWriter + Send + Sync + 'static> Executor<S> for UpdateExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start = Instant::now();
        let result = self.do_execute();
        let elapsed = start.elapsed();
        self.base.get_stats_mut().add_total_time(elapsed);
        match result {
            Ok(_) => Ok(ExecutionResult::Empty),
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
        "UpdateExecutor"
    }

    fn description(&self) -> &str {
        "Update executor - updates vertices and edges in storage"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader + StorageWriter> HasStorage<S> for UpdateExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageReader + StorageWriter + Send + Sync + 'static> UpdateExecutor<S> {
    fn do_execute(&mut self) -> DBResult<Vec<UpdateResult>> {
        let mut results = Vec::new();

        let condition_expression = self.condition.as_ref().and_then(|c| c.get_expression());

        let mut storage = self.get_storage().write();

        if let Some(updates) = &self.vertex_updates {
            for update in updates {
                let mut update_result = UpdateResult {
                    vertex_id: Some(update.vertex_id.clone()),
                    src: None,
                    dst: None,
                    edge_type: None,
                    returned_props: HashMap::new(),
                };

                let vertex_vid = VertexId::try_from(&update.vertex_id).map_err(DBError::from)?;
                if let Some(mut vertex) = storage.get_vertex(&self.space_name, &vertex_vid)? {
                    // Build current vertex properties for condition evaluation
                    let mut current_props = HashMap::new();
                    for tag in &vertex.tags {
                        for (k, v) in &tag.properties {
                            current_props.insert(k.clone(), v.clone());
                        }
                    }
                    for (k, v) in &vertex.properties {
                        current_props.insert(k.clone(), v.clone());
                    }

                    let should_update = if let Some(ref expression) = condition_expression {
                        self.evaluate_condition(
                            expression,
                            update.vertex_id.clone(),
                            None,
                            None,
                            None,
                            &current_props,
                        )?
                    } else {
                        true
                    };

                    if should_update {
                        let evaluated_props = self.evaluate_property_expressions(
                            &update.properties,
                            update.property_expressions.as_ref(),
                            &vertex,
                        )?;

                        if !vertex.tags.is_empty() {
                            for (key, value) in &evaluated_props {
                                vertex.tags[0].properties.insert(key.clone(), value.clone());
                            }
                        } else {
                            for (key, value) in &evaluated_props {
                                vertex.properties.insert(key.clone(), value.clone());
                            }
                        }
                        storage.update_vertex(&self.space_name, vertex.clone())?;

                        update_result.returned_props = evaluated_props;
                    }
                } else if self.insertable {
                    let tags: Vec<crate::core::Tag> = update
                        .tags_to_add
                        .as_ref()
                        .map(|tag_names| {
                            tag_names
                                .iter()
                                .map(|name| {
                                    crate::core::Tag::new(name.clone(), update.properties.clone())
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let new_vertex = crate::core::Vertex::new_with_properties(
                        vertex_vid,
                        tags,
                        update.properties.clone(),
                    );
                    storage.insert_vertex(&self.space_name, new_vertex)?;
                    update_result.returned_props = update.properties.clone();
                } else {
                    return Err(DBError::query(format!(
                        "Vertex {} not found, cannot update",
                        vertex_vid
                    )));
                }

                results.push(update_result);
            }
        }

        if let Some(updates) = &self.edge_updates {
            for update in updates {
                let mut update_result = UpdateResult {
                    vertex_id: None,
                    src: Some(update.src.clone()),
                    dst: Some(update.dst.clone()),
                    edge_type: Some(update.edge_type.clone()),
                    returned_props: HashMap::new(),
                };

                let rank = update.rank.unwrap_or(0);
                let edge_src = VertexId::try_from(&update.src).map_err(DBError::from)?;
                let edge_dst = VertexId::try_from(&update.dst).map_err(DBError::from)?;
                if let Some(mut edge) = storage.get_edge(
                    &self.space_name,
                    &edge_src,
                    &edge_dst,
                    &update.edge_type,
                    rank,
                )? {
                    let should_update = if let Some(ref expression) = condition_expression {
                        self.evaluate_condition(
                            expression,
                            update.src.clone(),
                            Some(update.dst.clone()),
                            Some(&update.edge_type),
                            None,
                            &edge.props,
                        )?
                    } else {
                        true
                    };

                    if should_update {
                        let evaluated_props = self.evaluate_edge_property_expressions(
                            &update.properties,
                            update.property_expressions.as_ref(),
                            &edge,
                        )?;

                        for (key, value) in &evaluated_props {
                            edge.props.insert(key.clone(), value.clone());
                        }
                        storage.delete_edge(
                            &self.space_name,
                            &edge_src,
                            &edge_dst,
                            &update.edge_type,
                            rank,
                        )?;
                        storage.insert_edge(&self.space_name, edge)?;
                        update_result.returned_props = evaluated_props;
                    }
                } else if self.insertable {
                    let new_edge = crate::core::Edge::new(
                        edge_src,
                        edge_dst,
                        update.edge_type.clone(),
                        update.rank.unwrap_or(0),
                        update.properties.clone(),
                    );
                    storage.insert_edge(&self.space_name, new_edge)?;
                    update_result.returned_props = update.properties.clone();
                } else {
                    return Err(DBError::query(format!(
                        "Edge ({}, {}) of type '{}' not found, cannot update",
                        update.src, update.dst, update.edge_type
                    )));
                }

                results.push(update_result);
            }
        }

        Ok(results)
    }

    fn evaluate_property_expressions(
        &self,
        base_properties: &HashMap<String, Value>,
        property_expressions: Option<&HashMap<String, ContextualExpression>>,
        vertex: &crate::core::Vertex,
    ) -> DBResult<HashMap<String, Value>> {
        let mut result = HashMap::new();

        if let Some(expressions) = property_expressions {
            let mut context = DefaultExpressionContext::new();

            for tag in vertex.tags() {
                for (prop_name, prop_value) in &tag.properties {
                    context.set_variable(prop_name.clone(), prop_value.clone());
                }
            }
            for (prop_name, prop_value) in &vertex.properties {
                context.set_variable(prop_name.clone(), prop_value.clone());
            }

            for (key, ctx_expr) in expressions {
                if let Some(expr) = ctx_expr.get_expression() {
                    let value =
                        ExpressionEvaluator::evaluate(&expr, &mut context).map_err(|e| {
                            DBError::query(format!(
                                "Failed to evaluate expression for property '{}': {}",
                                key, e
                            ))
                        })?;
                    result.insert(key.clone(), value);
                } else if let Some(value) = base_properties.get(key) {
                    result.insert(key.clone(), value.clone());
                }
            }
        } else {
            result = base_properties.clone();
        }

        Ok(result)
    }

    fn evaluate_edge_property_expressions(
        &self,
        base_properties: &HashMap<String, Value>,
        property_expressions: Option<&HashMap<String, ContextualExpression>>,
        edge: &crate::core::Edge,
    ) -> DBResult<HashMap<String, Value>> {
        let mut result = HashMap::new();

        if let Some(expressions) = property_expressions {
            let mut context = DefaultExpressionContext::new();

            for (prop_name, prop_value) in &edge.props {
                context.set_variable(prop_name.clone(), prop_value.clone());
            }

            for (key, ctx_expr) in expressions {
                if let Some(expr) = ctx_expr.get_expression() {
                    let value =
                        ExpressionEvaluator::evaluate(&expr, &mut context).map_err(|e| {
                            DBError::query(format!(
                                "Failed to evaluate expression for edge property '{}': {}",
                                key, e
                            ))
                        })?;
                    result.insert(key.clone(), value);
                } else if let Some(value) = base_properties.get(key) {
                    result.insert(key.clone(), value.clone());
                }
            }
        } else {
            result = base_properties.clone();
        }

        Ok(result)
    }

    fn evaluate_condition(
        &self,
        expression: &Expression,
        vertex_id: Value,
        dst: Option<Value>,
        edge_type: Option<&str>,
        _rank: Option<i64>,
        properties: &HashMap<String, Value>,
    ) -> DBResult<bool> {
        let mut context = DefaultExpressionContext::new();
        context.set_variable("VID".to_string(), vertex_id.clone());
        if let Some(dst_val) = dst {
            context.set_variable("DST".to_string(), dst_val);
        }
        if let Some(etype) = edge_type {
            context.set_variable(
                "edge_type".to_string(),
                crate::core::Value::String(etype.to_string()),
            );
        }
        for (key, value) in properties {
            context.set_variable(key.clone(), value.clone());
        }

        let result = ExpressionEvaluator::evaluate(expression, &mut context)
            .map_err(|e| DBError::query(format!("Conditional evaluation failed: {}", e)))?;

        match result {
            crate::core::Value::Bool(b) => Ok(b),
            _ => Err(DBError::query(
                "Conditional expression must return a boolean value".to_string(),
            )),
        }
    }
}
