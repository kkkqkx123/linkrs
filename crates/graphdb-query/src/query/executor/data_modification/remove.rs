//! Universal Deletion Executor
//!
//! Responsible for deleting attributes, labels, and other information associated with vertices and edges.

use std::sync::Arc;
use std::time::Instant;

use crate::core::error::DBError;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::VertexId;
use crate::core::{Expression, Value};
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::{BaseExecutor, ExecutorStats};
use crate::query::executor::base::{
    DBResult, ExecutionResult, Executor, HasStorage, InputExecutor,
};
use crate::query::executor::expression::evaluation_context::DefaultExpressionContext;
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::StorageClient;
use parking_lot::RwLock;

/// Deleted items
#[derive(Debug, Clone)]
pub struct RemoveItem {
    pub item_type: RemoveItemType,
    pub expression: ContextualExpression,
}

/// Type of item to be deleted
#[derive(Debug, Clone, PartialEq)]
pub enum RemoveItemType {
    Property,
    Tag,
}

/// Please provide the text you would like to have translated. I will then assist you with deleting the corresponding content from the system.
#[derive(Debug, Clone)]
pub struct RemoveResult {
    pub removed_count: i64,
}

/// Universal Deletion Executor
///
/// Responsible for deleting attributes, labels, and other information associated with vertices and edges.
pub struct RemoveExecutor<S: StorageClient + 'static> {
    base: BaseExecutor<S>,
    remove_items: Vec<RemoveItem>,
    input_executor: Option<Box<ExecutorEnum<S>>>,
}

impl<S: StorageClient + 'static> RemoveExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        remove_items: Vec<RemoveItem>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "RemoveExecutor".to_string(), storage, expr_context),
            remove_items,
            input_executor: None,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for RemoveExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start = Instant::now();
        let result = self.do_execute();
        let elapsed = start.elapsed();
        self.base.get_stats_mut().add_total_time(elapsed);
        match result {
            Ok(count) => {
                let dataset =
                    DataSet::from_rows(vec![vec![Value::BigInt(count)]], vec!["count".to_string()]);
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
        "RemoveExecutor"
    }

    fn description(&self) -> &str {
        "Remove executor - removes properties and tags from vertices and edges"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient + 'static> HasStorage<S> for RemoveExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for RemoveExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_ref().map(|v| v.as_ref())
    }
}

impl<S: StorageClient + Send + Sync + 'static> RemoveExecutor<S> {
    fn do_execute(&mut self) -> DBResult<i64> {
        let mut removed_count = 0i64;
        let mut storage = self.get_storage().write();

        for remove_item in &self.remove_items {
            let expression = remove_item
                .expression
                .get_expression()
                .ok_or_else(|| DBError::query("REMOVE expression does not exist".to_string()))?;

            match &remove_item.item_type {
                RemoveItemType::Property => {
                    if let Some((vertex_id, property_name)) =
                        self.extract_property_info(&expression)?
                    {
                        let vid = VertexId::try_from(&vertex_id).map_err(DBError::from)?;
                        if let Some(mut vertex) = storage.get_vertex("default", &vid)? {
                            vertex.properties.remove(&property_name);
                            storage.update_vertex("default", vertex)?;
                            removed_count += 1;
                        }
                    }
                }
                RemoveItemType::Tag => {
                    if let Some((vertex_id, tag_name)) = self.extract_tag_info(&expression)? {
                        let vid = VertexId::try_from(&vertex_id).map_err(DBError::from)?;
                        let count = storage.delete_tags("default", &vid, &[tag_name])?;
                        removed_count += count as i64;
                    }
                }
            }
        }

        Ok(removed_count)
    }

    fn extract_property_info(&self, expr: &Expression) -> DBResult<Option<(Value, String)>> {
        match expr {
            Expression::Property { object, property } => {
                let vertex_id = self.evaluate_to_vertex_id(object)?;
                let property_name = property.clone();
                Ok(Some((vertex_id, property_name)))
            }
            _ => Ok(None),
        }
    }

    fn extract_tag_info(&self, expr: &Expression) -> DBResult<Option<(Value, String)>> {
        match expr {
            Expression::Label(label) => {
                let vertex_id = self.evaluate_to_vertex_id(expr)?;
                let tag_name = label.clone();
                Ok(Some((vertex_id, tag_name)))
            }
            _ => Ok(None),
        }
    }

    fn evaluate_to_vertex_id(&self, expr: &Expression) -> DBResult<Value> {
        let mut context = DefaultExpressionContext::new();
        let value = ExpressionEvaluator::evaluate(expr, &mut context)
            .map_err(|e| DBError::query(e.to_string()))?;
        Ok(value)
    }
}
