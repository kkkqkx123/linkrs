//! Implementation of AppendVerticesExecutor
//!
//! Responsible for handling the addition of vertices; retrieves vertex information based on the provided vertex ID and adds it to the result.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::types::VertexId;
use crate::core::Expression;
use crate::core::{Value, Vertex};
#[cfg(test)]
use crate::query::executor::base::Executor;
use crate::query::executor::base::{
    AppendVerticesConfig, BaseExecutor, ExecutionResult, ExecutorConfig, HasStorage,
};
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::{DefaultExpressionContext, ExpressionContext};
use crate::query::DataSet;
use crate::storage::StorageClient;

/// AppendVertices executor
/// Used to retrieve vertex information based on the vertex ID and append it to the result.
pub struct AppendVerticesExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    /// Input variable name
    input_var: String,
    /// Source expression used to obtain the vertex ID
    src_expression: Expression,
    /// Vertex filtering expression
    v_filter: Option<Expression>,
    /// Column names
    col_names: Vec<String>,
    /// Should duplicates be removed?
    dedup: bool,
    /// Is it necessary to obtain the attribute?
    need_fetch_prop: bool,
}

impl<S: StorageClient + Send + 'static> AppendVerticesExecutor<S> {
    /// Create a new AppendVerticesExecutor.
    pub fn new(base_config: ExecutorConfig<S>, config: AppendVerticesConfig) -> Self {
        Self {
            base: BaseExecutor::new(
                base_config.id,
                "AppendVerticesExecutor".to_string(),
                base_config.storage,
                base_config.expr_context,
            ),
            input_var: config.input_var,
            src_expression: config.src_expression,
            v_filter: config.v_filter,
            col_names: config.col_names,
            dedup: config.dedup,
            need_fetch_prop: config.need_fetch_prop,
        }
    }

    /// Create an AppendVerticesExecutor with context.
    pub fn with_context(
        id: i64,
        storage: Arc<RwLock<S>>,
        context: crate::query::executor::base::ExecutionContext,
        config: AppendVerticesConfig,
    ) -> Self {
        Self {
            base: BaseExecutor::with_context(
                id,
                "AppendVerticesExecutor".to_string(),
                storage,
                context,
            ),
            input_var: config.input_var,
            src_expression: config.src_expression,
            v_filter: config.v_filter,
            col_names: config.col_names,
            dedup: config.dedup,
            need_fetch_prop: config.need_fetch_prop,
        }
    }

    /// Constructing a request dataset
    fn build_request_dataset(&mut self) -> DBResult<Vec<Value>> {
        // Obtain the input result.
        let input_result = self
            .base
            .context
            .get_result(&self.input_var)
            .ok_or_else(|| {
                DBError::query(format!("Input variable '{}' not found", self.input_var))
            })?;

        // Create the context for the expression.
        let mut expr_context = DefaultExpressionContext::new();

        let mut vids = Vec::new();
        let mut seen = if self.dedup {
            Some(std::collections::HashMap::new())
        } else {
            None
        };

        // Process the input result based on its type
        match input_result {
            ExecutionResult::DataSet(dataset) => {
                for row in dataset.rows {
                    for value in row {
                        expr_context.set_variable("_".to_string(), value.clone());

                        let vid =
                            ExpressionEvaluator::evaluate(&self.src_expression, &mut expr_context)
                                .map_err(|e| DBError::query(e.to_string()))?;

                        if let Some(ref mut seen_map) = seen {
                            if !seen_map.contains_key(&vid) {
                                seen_map.insert(vid.clone(), true);
                                vids.push(vid);
                            }
                        } else {
                            vids.push(vid);
                        }
                    }
                }
            }
            ExecutionResult::Empty
            | ExecutionResult::Success
            | ExecutionResult::SpaceSwitched(_) => {}
            ExecutionResult::Error(msg) => {
                return Err(DBError::query(msg));
            }
        }

        Ok(vids)
    }

    /// Handling cases of empty attributes
    fn handle_null_prop(&mut self, vids: Vec<Value>) -> DBResult<DataSet> {
        let mut dataset = DataSet {
            col_names: self.col_names.clone(),
            rows: Vec::new(),
        };

        let _input_result = self
            .base
            .context
            .get_result(&self.input_var)
            .expect("Context should have input result");

        for vid in vids {
            if vid.is_empty() {
                continue;
            }

            // Create vertices
            let vertex = Vertex {
                vid: VertexId::try_from(&vid).map_err(|e| DBError::storage(e.to_string()))?,
                id: 0,
                tags: Vec::new(),
                properties: std::collections::HashMap::new(),
            };

            dataset.rows.push(vec![Value::Vertex(Box::new(vertex))]);
        }

        Ok(dataset)
    }

    /// Retrieve vertex attributes from storage.
    fn fetch_vertices(&mut self, vids: Vec<Value>) -> DBResult<Vec<Vertex>> {
        let mut vertices = Vec::new();

        let storage = self.get_storage().read();

        for vid in vids {
            if vid.is_empty() {
                continue;
            }

            let vid = VertexId::try_from(&vid).map_err(|e| DBError::storage(e.to_string()))?;
            let vertex = storage.get_vertex("default", &vid).map_err(DBError::from)?;

            if let Some(vertex) = vertex {
                vertices.push(vertex);
            }
        }

        Ok(vertices)
    }

    fn execute_append_vertices(&mut self) -> DBResult<DataSet> {
        if !self.need_fetch_prop {
            let vids = self.build_request_dataset()?;
            return self.handle_null_prop(vids);
        }

        let vids = self.build_request_dataset()?;

        if vids.is_empty() {
            return Ok(DataSet {
                col_names: self.col_names.clone(),
                rows: Vec::new(),
            });
        }

        let vertices = self.fetch_vertices(vids)?;

        let mut dataset = DataSet {
            col_names: self.col_names.clone(),
            rows: Vec::new(),
        };

        for vertex in vertices {
            let vertex_value = Value::Vertex(Box::new(vertex.clone()));
            let mut row_context = DefaultExpressionContext::new();

            if let Some(ref filter_expression) = self.v_filter {
                let filter_result =
                    ExpressionEvaluator::evaluate(filter_expression, &mut row_context)
                        .map_err(|e| DBError::query(e.to_string()))?;

                if let Value::Bool(false) = filter_result {
                    continue;
                }
            }

            dataset.rows.push(vec![vertex_value]);
        }

        Ok(dataset)
    }
}

impl_executor_with_execute!(AppendVerticesExecutor, execute_append_vertices);
impl_has_storage!(AppendVerticesExecutor);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Expression;
    use crate::core::Value;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use crate::storage::MockStorage;
    use parking_lot::RwLock;
    use std::sync::Arc;

    #[test]
    fn test_append_vertices_executor() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));

        let vids = vec![
            Value::String("vertex1".to_string()),
            Value::String("vertex2".to_string()),
        ];

        let input_dataset = DataSet::from_rows(vec![vids], vec!["_".to_string()]);
        let input_result = ExecutionResult::DataSet(input_dataset);

        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let context = crate::query::executor::base::ExecutionContext::new(expr_context.clone());
        context.set_result("input".to_string(), input_result);

        let src_expression = Expression::Variable("_".to_string());
        let config = AppendVerticesConfig {
            input_var: "input".to_string(),
            src_expression,
            v_filter: None,
            col_names: vec!["vertex".to_string()],
            dedup: false,
            need_fetch_prop: false,
        };

        let mut executor = AppendVerticesExecutor::with_context(1, storage, context, config);

        let result = executor
            .execute()
            .expect("Executor should execute successfully");

        match result {
            ExecutionResult::DataSet(dataset) => {
                assert_eq!(dataset.row_count(), 2);
                assert!(matches!(dataset.rows[0][0], Value::Vertex(_)));
                assert!(matches!(dataset.rows[1][0], Value::Vertex(_)));
            }
            _ => panic!("Expected DataSet result"),
        }
    }
}
