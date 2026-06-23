use std::collections::HashSet;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::types::VertexId;
use crate::core::{Edge, Expression, NPath, Path, Value, Vertex};
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::{BaseExecutor, EdgeDirection, InputExecutor};
use crate::query::executor::base::{ExecutionResult, Executor, HasStorage};
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::DefaultExpressionContext;
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::query::QueryError;
use crate::storage::StorageClient;
use parking_lot::RwLock;

/// Parameters for creating a TraverseExecutor
pub struct TraverseExecutorParams<S: StorageClient + Send + 'static> {
    pub id: i64,
    pub storage: Arc<RwLock<S>>,
    pub edge_direction: EdgeDirection,
    pub edge_types: Option<Vec<String>>,
    pub max_depth: Option<usize>,
    pub conditions: Option<String>,
    pub expr_context: Arc<ExpressionAnalysisContext>,
}

/// TraverseExecutor – A complete executor for traversing and executing graphs
///
/// Perform a complete graph traversal operation, supporting multiple jumps and conditional filtering.
/// Combines the functionality of ExpandExecutor, supporting more complex traversal requirements.
pub struct TraverseExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    pub edge_direction: EdgeDirection,
    pub edge_types: Option<Vec<String>>,
    pub max_depth: Option<usize>,
    pub space_name: String,
    conditions: Option<String>,
    input_executor: Option<Box<ExecutorEnum<S>>>,
    /// Use NPath to store the current traversal path, in order to reduce memory copying.
    current_npaths: Vec<Arc<NPath>>,
    /// Use NPath to store the completed paths.
    completed_npaths: Vec<Arc<NPath>>,
    /// Final output should be in the format of "Path".
    current_paths: Vec<Path>,
    completed_paths: Vec<Path>,
    pub visited_nodes: HashSet<Value>,
    track_prev_path: bool,
    generate_path: bool,
    v_filter: Option<Expression>,
    e_filter: Option<Expression>,
    filter: Option<Expression>,
}

// Manual Debug implementation for TraverseExecutor to avoid requiring Debug trait for Executor trait object
impl<S: StorageClient> std::fmt::Debug for TraverseExecutor<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraverseExecutor")
            .field("base", &"BaseExecutor")
            .field("edge_direction", &self.edge_direction)
            .field("edge_types", &self.edge_types)
            .field("max_depth", &self.max_depth)
            .field("conditions", &self.conditions)
            .field("input_executor", &"Option<Box<dyn Executor<S>>>")
            .field("current_paths", &self.current_paths)
            .field("completed_paths", &self.completed_paths)
            .field("visited_nodes", &self.visited_nodes)
            .field("track_prev_path", &self.track_prev_path)
            .field("generate_path", &self.generate_path)
            .finish()
    }
}

impl<S: StorageClient> TraverseExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        edge_direction: EdgeDirection,
        edge_types: Option<Vec<String>>,
        max_depth: Option<usize>,
        conditions: Option<String>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "TraverseExecutor".to_string(), storage, expr_context),
            edge_direction,
            edge_types,
            max_depth,
            space_name: "default".to_string(),
            conditions,
            input_executor: None,
            current_npaths: Vec::new(),
            completed_npaths: Vec::new(),
            current_paths: Vec::new(),
            completed_paths: Vec::new(),
            visited_nodes: HashSet::new(),
            track_prev_path: true,
            generate_path: true,
            v_filter: None,
            e_filter: None,
            filter: None,
        }
    }

    /// Set whether to track the previous path.
    pub fn with_track_prev_path(mut self, track_prev_path: bool) -> Self {
        self.track_prev_path = track_prev_path;
        self
    }

    /// Set whether to generate paths.
    pub fn with_generate_path(mut self, generate_path: bool) -> Self {
        self.generate_path = generate_path;
        self
    }

    fn get_neighbors_with_edges(&self, node_id: &Value) -> Result<Vec<(Value, Edge)>, QueryError> {
        let storage = self.base.get_storage().clone();
        let node_vid =
            VertexId::try_from(node_id).map_err(|e| QueryError::storage(e.to_string()))?;
        let neighbors = super::traversal_utils::get_neighbors_with_edges(
            &storage,
            &node_vid,
            self.edge_direction,
            &self.edge_types,
            &self.space_name,
            false,
        )
        .map_err(|e| QueryError::storage(e.to_string()))?;
        Ok(neighbors
            .into_iter()
            .map(|(vid, edge)| (Value::from(vid), edge))
            .collect())
    }

    /// Check whether the conditions are met.
    ///
    /// Refer to the implementation of TraverseExecutor::expand in nebula-graph.
    /// 支持顶点过滤(vFilter)和边过滤(eFilter)
    fn check_conditions(&self, path: &Path, edge: &Edge, vertex: &Vertex) -> bool {
        // Check the edge filtering conditions.
        if let Some(ref e_filter) = self.e_filter {
            let mut context = DefaultExpressionContext::new();
            context.set_variable("edge".to_string(), Value::edge(edge.clone()));
            context.set_variable(
                "vertex".to_string(),
                Value::Vertex(Box::new(vertex.clone())),
            );

            // If the path is not empty, add the relevant context.
            if !path.steps.is_empty() {
                let last_step = path.steps.last().expect("Path should have steps");
                context.set_variable("src".to_string(), Value::Vertex(last_step.dst.clone()));
            } else {
                context.set_variable("src".to_string(), Value::Vertex(path.src.clone()));
            }
            context.set_variable("dst".to_string(), Value::Vertex(Box::new(vertex.clone())));

            match ExpressionEvaluator::evaluate(e_filter, &mut context) {
                Ok(Value::Bool(true)) => {}
                _ => return false,
            }
        }

        // Check the vertex filtering conditions (applied only in the first step).
        if path.steps.is_empty() {
            if let Some(ref v_filter) = self.v_filter {
                let mut context = DefaultExpressionContext::new();
                context.set_variable(
                    "vertex".to_string(),
                    Value::Vertex(Box::new(vertex.clone())),
                );

                match ExpressionEvaluator::evaluate(v_filter, &mut context) {
                    Ok(Value::Bool(true)) => {}
                    _ => return false,
                }
            }
        }

        // Check the general filtering criteria.
        if let Some(ref filter) = self.filter {
            let mut context = DefaultExpressionContext::new();
            context.set_variable("edge".to_string(), Value::edge(edge.clone()));
            context.set_variable(
                "vertex".to_string(),
                Value::Vertex(Box::new(vertex.clone())),
            );

            if !path.steps.is_empty() {
                let last_step = path.steps.last().expect("Path should have steps");
                context.set_variable("src".to_string(), Value::Vertex(last_step.dst.clone()));
            } else {
                context.set_variable("src".to_string(), Value::Vertex(path.src.clone()));
            }
            context.set_variable("dst".to_string(), Value::Vertex(Box::new(vertex.clone())));

            match ExpressionEvaluator::evaluate(filter, &mut context) {
                Ok(Value::Bool(true)) => {}
                _ => return false,
            }
        }

        true
    }

    /// Set vertex filtering criteria
    pub fn with_v_filter(mut self, filter: Expression) -> Self {
        self.v_filter = Some(filter);
        self
    }

    /// Set edge filtering criteria
    pub fn with_e_filter(mut self, filter: Expression) -> Self {
        self.e_filter = Some(filter);
        self
    }

    /// Set common filtering criteria
    pub fn with_filter(mut self, filter: Expression) -> Self {
        self.filter = Some(filter);
        self
    }
}

impl<S: StorageClient> TraverseExecutor<S> {
    /// Perform a step-by-step traversal.
    fn traverse_step(&mut self, current_depth: usize, max_depth: usize) -> Result<(), QueryError> {
        if current_depth >= max_depth {
            // Move the remaining elements from `current_npaths` to `completed_npaths`.
            self.completed_npaths.extend(self.current_npaths.clone());
            self.current_npaths.clear();
            return Ok(());
        }

        self.traverse_step_serial(current_depth, max_depth)
    }

    fn traverse_step_serial(
        &mut self,
        current_depth: usize,
        max_depth: usize,
    ) -> Result<(), QueryError> {
        let mut next_npaths: Vec<Arc<NPath>> = Vec::new();
        let mut completed_this_step: Vec<Arc<NPath>> = Vec::new();

        for npath in &self.current_npaths {
            // Get the last node of the current path.
            let current_node = &npath.vertex().vid;
            let current_node_value = Value::from(*current_node);

            // Obtaining neighbor nodes and edges
            let neighbors_with_edges = self.get_neighbors_with_edges(&current_node_value)?;

            for (neighbor_id, edge) in neighbors_with_edges {
                // Obtain the complete information of the neighboring nodes.
                let neighbor_vid = VertexId::try_from(&neighbor_id)
                    .map_err(|e| QueryError::storage(e.to_string()))?;
                let storage = self.get_storage().read();
                let neighbor_vertex = storage
                    .get_vertex("default", &neighbor_vid)
                    .map_err(|e| QueryError::storage(e.to_string()))?;

                if let Some(vertex) = neighbor_vertex {
                    // Convert NPath to Path for use in conditional checks
                    let path = npath.to_path();
                    // Check conditions
                    if !self.check_conditions(&path, &edge, &vertex) {
                        continue;
                    }

                    // 使用 NPath 扩展，O(1) 操作
                    let new_npath = Arc::new(NPath::extend(
                        npath.clone(),
                        Arc::new(edge),
                        Arc::new(vertex),
                    ));

                    // Check whether the maximum depth has been reached.
                    if current_depth + 1 >= max_depth {
                        completed_this_step.push(new_npath);
                    } else {
                        next_npaths.push(new_npath);
                    }
                }
            }
        }

        self.completed_npaths.extend(completed_this_step);
        self.current_npaths = next_npaths;
        Ok(())
    }

    fn initialize_traversal(&mut self, input_nodes: Vec<Vertex>) -> Result<(), QueryError> {
        self.current_npaths.clear();
        self.completed_npaths.clear();
        self.current_paths.clear();
        self.completed_paths.clear();
        self.visited_nodes.clear();

        for vertex in input_nodes {
            let vid = vertex.vid;
            let initial_npath = Arc::new(NPath::new(Arc::new(vertex)));
            self.current_npaths.push(initial_npath);
            self.visited_nodes.insert(Value::from(vid));
        }

        Ok(())
    }

    /// Constructing the traversal results
    fn build_traversal_result(&self) -> ExecutionResult {
        // Convert NPath to Path for output.
        let completed_paths: Vec<Path> = self
            .completed_npaths
            .iter()
            .map(|np| np.to_path())
            .collect();

        if self.generate_path {
            // Return path result
            let rows: Vec<Vec<Value>> = completed_paths
                .into_iter()
                .map(|p| vec![Value::path(p)])
                .collect();
            let dataset = DataSet::from_rows(rows, vec!["path".to_string()]);
            ExecutionResult::DataSet(dataset)
        } else {
            // Return the vertex results.
            let mut vertices = Vec::new();
            let mut visited_vertices = HashSet::new();

            for path in &completed_paths {
                // Add a starting node.
                if !visited_vertices.contains(&path.src.vid) {
                    vertices.push((*path.src).clone());
                    visited_vertices.insert(path.src.vid);
                }

                // Add all nodes in the path.
                for step in &path.steps {
                    if !visited_vertices.contains(&step.dst.vid) {
                        vertices.push((*step.dst).clone());
                        visited_vertices.insert(step.dst.vid);
                    }
                }
            }

            let rows: Vec<Vec<Value>> = vertices
                .into_iter()
                .map(|v| vec![Value::Vertex(Box::new(v))])
                .collect();
            let dataset = DataSet::from_rows(rows, vec!["vertex".to_string()]);
            ExecutionResult::DataSet(dataset)
        }
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for TraverseExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_deref()
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for TraverseExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        // First, execute the input executor (if it exists).
        let input_result = if let Some(ref mut input_exec) = self.input_executor {
            input_exec.execute()?
        } else {
            // If no actuator is specified, return an empty result.
            ExecutionResult::DataSet(DataSet::new())
        };

        // Extract the input nodes.
        let input_nodes = match input_result {
            ExecutionResult::DataSet(dataset) => dataset
                .rows
                .into_iter()
                .flat_map(|row| row.into_iter())
                .filter_map(|v| match v {
                    Value::Vertex(vertex) => Some(*vertex),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };

        if input_nodes.is_empty() {
            return Ok(ExecutionResult::DataSet(DataSet::new()));
        }

        // Initialize the traversal
        self.initialize_traversal(input_nodes)
            .map_err(DBError::from)?;

        // Determine the maximum depth.
        let max_depth = self.max_depth.unwrap_or(3); // The default depth is 3.

        // Perform the traversal.
        for current_depth in 0..max_depth {
            self.traverse_step(current_depth, max_depth)
                .map_err(DBError::from)?;

            // If there are no additional paths available for expansion, the process should be terminated in advance.
            if self.current_paths.is_empty() {
                break;
            }
        }

        // Add the remaining parts of the current path to the complete path.
        self.completed_paths.extend(self.current_paths.clone());

        // Build the results.
        Ok(self.build_traversal_result())
    }

    fn open(&mut self) -> DBResult<()> {
        self.current_paths.clear();
        self.completed_paths.clear();
        self.visited_nodes.clear();

        if let Some(ref mut input_exec) = self.input_executor {
            input_exec.open()?;
        }
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        self.current_paths.clear();
        self.completed_paths.clear();
        self.visited_nodes.clear();

        if let Some(ref mut input_exec) = self.input_executor {
            input_exec.close()?;
        }
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.base.is_open()
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        &self.base.description
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient + Send> HasStorage<S> for TraverseExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base
            .storage
            .as_ref()
            .expect("TraverseExecutor storage should be set")
    }
}
