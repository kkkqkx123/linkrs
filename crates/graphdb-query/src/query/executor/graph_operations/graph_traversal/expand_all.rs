use std::collections::HashSet;
use std::sync::Arc;

use crate::core::error::DBResult;
use crate::core::types::{ContextualExpression, VertexId};
use crate::core::{Edge, Expression, NPath, Path, Value, Vertex};
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::{BaseExecutor, EdgeDirection, InputExecutor};
use crate::query::executor::base::{ExecutionResult, Executor, HasStorage};
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::{DefaultExpressionContext, ExpressionContext};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::query::QueryError;
use crate::storage::StorageClient;
use parking_lot::RwLock;

/// ExpandAllExecutor – An executor that performs full-path expansion
///
/// Return all possible paths starting from the current node, not just the next-hop node.
/// Usually used in path exploration queries
pub struct ExpandAllExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    pub edge_direction: EdgeDirection,
    pub edge_types: Option<Vec<String>>,
    pub any_edge_type: bool,
    pub max_depth: Option<usize>,
    input_executor: Option<Box<ExecutorEnum<S>>>,
    // Use the NPath cache to store intermediate results and reduce the amount of memory copying.
    npath_cache: Vec<Arc<NPath>>,
    // Path caching (converted during the final output process)
    path_cache: Vec<Path>,
    // Set of visited nodes, used to avoid loops.
    pub visited_nodes: HashSet<VertexId>,
    // Source vertex IDs for starting the expansion (from GO FROM clause)
    pub src_vids: Vec<VertexId>,
    // Whether to include empty paths (paths with no edges) in the result
    pub include_empty_paths: bool,
    // Input variable name for getting input from ExecutionContext
    pub input_var: Option<String>,
    // Column names for the output DataSet
    pub col_names: Vec<String>,
    // Space ID for the query
    pub space_id: u64,
    // Space name for storage operations
    pub space_name: String,
    // Filter condition pushed down from FilterNode
    filter: Option<Expression>,
    // Input dataset for joining with expansion results (for multi-hop MATCH)
    input_dataset: Option<DataSet>,
    // Mapping from input vertex VID to input row index
    input_vertex_to_row: std::collections::HashMap<VertexId, usize>,
}

// Manual Debug implementation for ExpandAllExecutor to avoid requiring Debug trait for Executor trait object
impl<S: StorageClient> std::fmt::Debug for ExpandAllExecutor<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExpandAllExecutor")
            .field("base", &"BaseExecutor")
            .field("edge_direction", &self.edge_direction)
            .field("edge_types", &self.edge_types)
            .field("max_depth", &self.max_depth)
            .field("input_executor", &"Option<Box<dyn Executor<S>>>")
            .field("path_cache", &self.path_cache)
            .field("visited_nodes", &self.visited_nodes)
            .finish()
    }
}

/// Parameters for creating ExpandAllExecutor
pub struct ExpandAllExecutorParams<S: StorageClient + Send> {
    pub id: i64,
    pub storage: Arc<RwLock<S>>,
    pub edge_direction: EdgeDirection,
    pub edge_types: Option<Vec<String>>,
    pub any_edge_type: bool,
    pub max_depth: Option<usize>,
    pub expr_context: Arc<ExpressionAnalysisContext>,
    pub space_id: u64,
    pub space_name: String,
}

impl<S: StorageClient + Send> ExpandAllExecutor<S> {
    pub fn new(params: ExpandAllExecutorParams<S>) -> Self {
        Self {
            base: BaseExecutor::new(
                params.id,
                "ExpandAllExecutor".to_string(),
                params.storage,
                params.expr_context,
            ),
            edge_direction: params.edge_direction,
            edge_types: params.edge_types,
            any_edge_type: params.any_edge_type,
            max_depth: params.max_depth,
            input_executor: None,
            npath_cache: Vec::new(),
            path_cache: Vec::new(),
            visited_nodes: HashSet::new(),
            src_vids: Vec::new(),
            include_empty_paths: true, // Default to true for backward compatibility
            input_var: None,
            col_names: vec!["src".to_string(), "edge".to_string(), "dst".to_string()],
            space_id: params.space_id,
            space_name: params.space_name,
            filter: None,
            input_dataset: None,
            input_vertex_to_row: std::collections::HashMap::new(),
        }
    }

    pub fn with_context(
        params: ExpandAllExecutorParams<S>,
        context: crate::query::executor::base::ExecutionContext,
    ) -> Self {
        Self {
            base: BaseExecutor::with_context(
                params.id,
                "ExpandAllExecutor".to_string(),
                params.storage,
                context,
            ),
            edge_direction: params.edge_direction,
            edge_types: params.edge_types,
            any_edge_type: params.any_edge_type,
            max_depth: params.max_depth,
            input_executor: None,
            npath_cache: Vec::new(),
            path_cache: Vec::new(),
            visited_nodes: HashSet::new(),
            src_vids: Vec::new(),
            include_empty_paths: true,
            input_var: None,
            col_names: vec!["src".to_string(), "edge".to_string(), "dst".to_string()],
            space_id: params.space_id,
            space_name: params.space_name,
            filter: None,
            input_dataset: None,
            input_vertex_to_row: std::collections::HashMap::new(),
        }
    }

    pub fn with_src_vids(mut self, src_vids: Vec<VertexId>) -> Self {
        self.src_vids = src_vids;
        self
    }

    pub fn with_include_empty_paths(mut self, include: bool) -> Self {
        self.include_empty_paths = include;
        self
    }

    pub fn with_input_var(mut self, input_var: String) -> Self {
        self.input_var = Some(input_var);
        self
    }

    pub fn with_col_names(mut self, col_names: Vec<String>) -> Self {
        self.col_names = col_names;
        self
    }

    pub fn with_filter(mut self, filter: Option<ContextualExpression>) -> Self {
        self.filter =
            filter.and_then(|ctx_expr| ctx_expr.expression().map(|meta| meta.inner().clone()));
        self
    }

    fn get_neighbors_with_edges(
        &self,
        node_id: &VertexId,
    ) -> Result<Vec<(VertexId, Edge)>, QueryError> {
        let storage = self.base.get_storage().clone();
        let edge_types = if self.any_edge_type {
            None
        } else {
            self.edge_types.clone()
        };
        super::traversal_utils::get_neighbors_with_edges(
            &storage,
            node_id,
            self.edge_direction,
            &edge_types,
            &self.space_name,
            false,
        )
        .map_err(|e| QueryError::storage(e.to_string()))
    }

    /// Recursive expansion of paths (synchronous version)
    fn expand_paths_recursive(
        &mut self,
        current_npath: &Arc<NPath>,
        current_depth: usize,
        max_depth: usize,
    ) -> Result<Vec<Arc<NPath>>, QueryError> {
        // Get the last node of the current path.
        let current_node = &current_npath.vertex().vid;

        // Check whether the maximum depth has been reached.
        if current_depth >= max_depth {
            // Return to the current path
            return Ok(vec![current_npath.clone()]);
        }

        // Obtaining neighbor nodes and edges
        let neighbors_with_edges = self.get_neighbors_with_edges(current_node)?;

        if neighbors_with_edges.is_empty() {
            // There are no more neighbors; return to the current path.
            return Ok(vec![current_npath.clone()]);
        }

        let mut all_npaths: Vec<Arc<NPath>> = Vec::new();

        // Create a new path for each neighbor.
        for (neighbor_id, edge) in neighbors_with_edges {
            // Check whether the node has already been visited (to avoid loops).
            if self.visited_nodes.contains(&neighbor_id) {
                // Create a path that contains loops.
                let path_with_cycle = Arc::new(NPath::extend(
                    current_npath.clone(),
                    Arc::new(edge),
                    Arc::new(Vertex::new(neighbor_id, Vec::new())),
                ));
                all_npaths.push(path_with_cycle);
                continue;
            }

            // Obtain the complete information of the neighboring nodes.
            let neighbor_vertex = {
                let storage = self.get_storage().read();
                storage
                    .get_vertex(&self.space_name, &neighbor_id)
                    .map_err(|e| QueryError::storage(e.to_string()))?
            };

            // Create a vertex object: If the vertex already exists, use the actual vertex; otherwise, create a suspended vertex (with an empty Tag list).
            let vertex = match neighbor_vertex {
                Some(v) => v,
                None => {
                    // Suspension edge processing: Create a vertex for an empty Tag, while retaining the VID (Video Identifier).
                    Vertex::new(neighbor_id, Vec::new())
                }
            };

            // Using NPath expansion, O(1) operation
            let new_npath = Arc::new(NPath::extend(
                current_npath.clone(),
                Arc::new(edge),
                Arc::new(vertex),
            ));

            // Marked as visited
            self.visited_nodes.insert(neighbor_id);

            // Recursive expansion (continuing to expand in order to obtain more edges, even if the vertex is "hanging"/not directly connected to other nodes in the graph).
            let mut expanded_npaths =
                self.expand_paths_recursive(&new_npath, current_depth + 1, max_depth)?;
            all_npaths.append(&mut expanded_npaths);

            // Unmark (allows access from other paths)
            self.visited_nodes.remove(&neighbor_id);
        }

        // Add the current path
        all_npaths.push(current_npath.clone());

        Ok(all_npaths)
    }

    /// Construct the extended result.
    ///
    /// Returns a DataSet with columns from self.col_names (typically ["src", "edge", "dst"] or
    /// with a custom dst column name like ["src", "edge", "b"]) for each path step.
    /// This format allows subsequent operations to easily access the source vertex,
    /// edge, and destination vertex separately.
    fn build_expansion_result(&self) -> ExecutionResult {
        // Convert NPath to Path for output.
        let paths: Vec<Path> = self.npath_cache.iter().map(|np| np.to_path()).collect();

        // Build a DataSet with separate columns for src, edge, and dst
        // Use the configured column names, which may include custom dst column names
        let mut dataset = crate::query::DataSet::new();

        // If we have an input dataset, we need to join the expansion results with input rows
        // This is for multi-hop MATCH queries where we need to preserve all intermediate variables
        let (input_cols, input_rows) = if let Some(ref input_ds) = self.input_dataset {
            (input_ds.col_names.clone(), input_ds.rows.clone())
        } else {
            (Vec::new(), Vec::new())
        };

        // Set column names: if we have input columns, combine them with our output columns
        // But we need to avoid duplicates - the input_var column is the same as our src column
        if !input_cols.is_empty() {
            // Find which input columns to include (exclude the input_var column as it's our src)
            let mut combined_cols = Vec::new();
            for col in &input_cols {
                // Skip the input_var column as it will be replaced by our src column
                if let Some(ref input_var) = self.input_var {
                    if col == input_var {
                        continue;
                    }
                }
                combined_cols.push(col.clone());
            }
            // Add our output columns
            combined_cols.extend(self.col_names.clone());
            dataset.col_names = combined_cols;
        } else {
            dataset.col_names = self.col_names.clone();
        }

        let target_depth = self.max_depth.unwrap_or(1);

        // Determine if we have additional columns beyond src/edge/dst
        // These are typically edge type aliases for property access (e.g., KNOWS.since)
        let has_edge_alias = self.col_names.len() > 3;
        let edge_alias_index = if has_edge_alias { Some(3) } else { None };

        for path in &paths {
            // Skip empty paths if include_empty_paths is false
            if !self.include_empty_paths && path.steps.is_empty() {
                continue;
            }

            // For GO queries (include_empty_paths is false), only return the last step of paths
            // with exactly the target depth
            // For other queries (include_empty_paths is true), return all steps
            if self.include_empty_paths {
                // For each step in the path, create a row with src, edge, dst
                for step in &path.steps {
                    let mut row = vec![
                        Value::Vertex(path.src.clone()),
                        Value::edge((*step.edge).clone()),
                        Value::Vertex(Box::new((*step.dst).clone())),
                    ];
                    // Add edge alias column if present (duplicates edge value for property access)
                    if let Some(idx) = edge_alias_index {
                        if idx < self.col_names.len() {
                            row.push(Value::edge((*step.edge).clone()));
                        }
                    }

                    // Join with input row if available
                    if !input_rows.is_empty() {
                        if let Some(row_idx) = self.input_vertex_to_row.get(&path.src.vid) {
                            if let Some(input_row) = input_rows.get(*row_idx) {
                                // Prepend input row values (excluding the input_var column)
                                let mut combined_row = Vec::new();
                                for (i, col) in input_cols.iter().enumerate() {
                                    if let Some(ref input_var) = self.input_var {
                                        if col == input_var {
                                            continue;
                                        }
                                    }
                                    if i < input_row.len() {
                                        combined_row.push(input_row[i].clone());
                                    }
                                }
                                combined_row.extend(row);
                                row = combined_row;
                            }
                        }
                    }

                    // Apply filter if present
                    if self.should_include_row(&row, &dataset.col_names) {
                        dataset.rows.push(row);
                    }
                }

                // If include_empty_paths is true and path has no steps, add a row with just src
                if path.steps.is_empty() {
                    let mut row = vec![
                        Value::Vertex(path.src.clone()),
                        Value::Null(crate::core::NullType::Null),
                        Value::Null(crate::core::NullType::Null),
                    ];
                    // Add null for edge alias column if present
                    if edge_alias_index.is_some() {
                        row.push(Value::Null(crate::core::NullType::Null));
                    }

                    // Join with input row if available
                    if !input_rows.is_empty() {
                        if let Some(row_idx) = self.input_vertex_to_row.get(&path.src.vid) {
                            if let Some(input_row) = input_rows.get(*row_idx) {
                                let mut combined_row = Vec::new();
                                for (i, col) in input_cols.iter().enumerate() {
                                    if let Some(ref input_var) = self.input_var {
                                        if col == input_var {
                                            continue;
                                        }
                                    }
                                    if i < input_row.len() {
                                        combined_row.push(input_row[i].clone());
                                    }
                                }
                                combined_row.extend(row);
                                row = combined_row;
                            }
                        }
                    }

                    // Apply filter if present
                    if self.should_include_row(&row, &dataset.col_names) {
                        dataset.rows.push(row);
                    }
                }
            } else if path.steps.len() == target_depth {
                // For GO queries, only add the last step
                if let Some(last_step) = path.steps.last() {
                    let mut row = vec![
                        Value::Vertex(path.src.clone()),
                        Value::edge((*last_step.edge).clone()),
                        Value::Vertex(Box::new((*last_step.dst).clone())),
                    ];
                    // Add edge alias column if present (duplicates edge value for property access)
                    if let Some(idx) = edge_alias_index {
                        if idx < self.col_names.len() {
                            row.push(Value::edge((*last_step.edge).clone()));
                        }
                    }

                    // Join with input row if available
                    if !input_rows.is_empty() {
                        if let Some(row_idx) = self.input_vertex_to_row.get(&path.src.vid) {
                            if let Some(input_row) = input_rows.get(*row_idx) {
                                let mut combined_row = Vec::new();
                                for (i, col) in input_cols.iter().enumerate() {
                                    if let Some(ref input_var) = self.input_var {
                                        if col == input_var {
                                            continue;
                                        }
                                    }
                                    if i < input_row.len() {
                                        combined_row.push(input_row[i].clone());
                                    }
                                }
                                combined_row.extend(row);
                                row = combined_row;
                            }
                        }
                    }

                    // Apply filter if present
                    if self.should_include_row(&row, &dataset.col_names) {
                        dataset.rows.push(row);
                    }
                }
            }
        }

        ExecutionResult::DataSet(dataset)
    }

    /// Check if a row should be included based on the filter condition
    fn should_include_row(&self, row: &[Value], col_names: &[String]) -> bool {
        if let Some(ref filter) = self.filter {
            let mut context = DefaultExpressionContext::new();

            // Set column values as variables
            for (i, col_name) in col_names.iter().enumerate() {
                if i < row.len() {
                    context.set_variable(col_name.clone(), row[i].clone());
                }
            }

            // Map GO query special variables: $$ -> dst, $^ -> src, target -> dst, edge -> edge
            if let Some(dst_idx) = col_names.iter().position(|c| c == "dst") {
                if dst_idx < row.len() {
                    context.set_variable("$$".to_string(), row[dst_idx].clone());
                    context.set_variable("target".to_string(), row[dst_idx].clone());
                }
            }
            if let Some(src_idx) = col_names.iter().position(|c| c == "src") {
                if src_idx < row.len() {
                    context.set_variable("$^".to_string(), row[src_idx].clone());
                }
            }
            if let Some(edge_idx) = col_names.iter().position(|c| c == "edge") {
                if edge_idx < row.len() {
                    context.set_variable("edge".to_string(), row[edge_idx].clone());
                    // Map edge type name to the edge value for GO queries like WHERE friend.strength > 5
                    if let Value::Edge(ref edge_val) = row[edge_idx] {
                        context
                            .set_variable(edge_val.edge_type().to_string(), row[edge_idx].clone());
                    }
                }
            }

            // Evaluate the filter condition
            matches!(
                ExpressionEvaluator::evaluate(filter, &mut context),
                Ok(Value::Bool(true))
            )
        } else {
            true
        }
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for ExpandAllExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_deref()
    }
}

impl<S: StorageClient + Send + 'static> Executor<S> for ExpandAllExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        // Clear caches to ensure fresh results for each execution
        // This prevents duplicate results when the executor is reused
        self.npath_cache.clear();
        self.path_cache.clear();
        self.visited_nodes.clear();
        self.input_dataset = None;
        self.input_vertex_to_row.clear();

        // First, execute the input executor (if it exists).
        let input_result = if let Some(ref mut input_exec) = self.input_executor {
            input_exec.execute()?
        } else if let Some(ref input_var) = self.input_var {
            // Try to get input from ExecutionContext
            self.base
                .context
                .get_result(input_var)
                .unwrap_or_else(|| ExecutionResult::DataSet(DataSet::new()))
        } else {
            ExecutionResult::DataSet(DataSet::new())
        };

        let mut input_nodes: Vec<Vertex> = match input_result {
            ExecutionResult::DataSet(dataset) => {
                let col_names = dataset.col_names.clone();
                let dst_idx = col_names.iter().position(|c| c == "dst");

                // Store the input dataset for later joining with expansion results
                // Only store if there are rows (for multi-hop MATCH)
                if !dataset.rows.is_empty() {
                    // Create mapping from input vertex VID to row index
                    for (row_idx, row) in dataset.rows.iter().enumerate() {
                        if let Some(ref input_var) = self.input_var {
                            if let Some(idx) = col_names.iter().position(|c| c == input_var) {
                                if idx < row.len() {
                                    if let Value::Vertex(vertex) = &row[idx] {
                                        self.input_vertex_to_row.insert(vertex.vid, row_idx);
                                    }
                                }
                            }
                        }
                    }
                    self.input_dataset = Some(dataset.clone());
                }

                dataset
                    .rows
                    .into_iter()
                    .filter_map(|row| {
                        if let Some(idx) = dst_idx {
                            if idx < row.len() {
                                if let Value::Vertex(vertex) = &row[idx] {
                                    return Some((**vertex).clone());
                                }
                            }
                        }
                        // When custom column names are used (e.g., in multi-hop MATCH),
                        // the dst column may be named after the input_var instead of "dst".
                        // Try to find a column matching the input_var name.
                        if let Some(ref input_var) = self.input_var {
                            if let Some(idx) = col_names.iter().position(|c| c == input_var) {
                                if idx < row.len() {
                                    if let Value::Vertex(vertex) = &row[idx] {
                                        return Some((**vertex).clone());
                                    }
                                }
                            }
                        }
                        for val in row {
                            if let Value::Vertex(vertex) = val {
                                return Some(*vertex);
                            }
                        }
                        None
                    })
                    .collect()
            }
            _ => Vec::new(),
        };

        if !self.src_vids.is_empty() {
            let storage = self.get_storage().read();
            for vid in &self.src_vids {
                if let Ok(Some(vertex)) = storage.get_vertex(&self.space_name, vid) {
                    input_nodes.push(vertex);
                }
            }
        }

        // Special case: for simple edge-only patterns (MATCH ()-[e]->()) with specific edge types,
        // directly scan edges by type instead of scanning all vertices and expanding.
        // This is more efficient and avoids potential issues with vertex-based expansion.
        // Skip for multi-hop chains (input has >1 column) — the normal expansion path handles
        // column combining correctly for those.
        let is_multi_hop = self
            .input_dataset
            .as_ref()
            .is_some_and(|ds| ds.col_names.len() > 1);
        let has_specific_edge_types =
            !self.any_edge_type && self.edge_types.as_ref().is_some_and(|t| !t.is_empty());
        let is_from_go_clause = !self.src_vids.is_empty();

        if has_specific_edge_types && !is_from_go_clause && !is_multi_hop {
            if let Some(ref edge_types) = self.edge_types {
                if !edge_types.is_empty() {
                    let storage = self.get_storage().read();
                    let mut all_edges = Vec::new();

                    // Scan edges for each specified edge type
                    for edge_type in edge_types {
                        match storage.scan_edges_by_type(&self.space_name, edge_type) {
                            Ok(edges) => all_edges.extend(edges),
                            Err(e) => {
                                log::warn!("Failed to scan edges for type '{}': {}", edge_type, e);
                            }
                        }
                    }

                    // If we found edges, create a dataset with edge values
                    if !all_edges.is_empty() {
                        let mut dataset = DataSet::new();
                        dataset.col_names = self.col_names.clone();

                        // Build a set of source vertex IDs from input nodes for filtering
                        // and a map from VID to full vertex for property lookup
                        let src_set: std::collections::HashSet<VertexId> =
                            if !input_nodes.is_empty() {
                                input_nodes.iter().map(|v| v.vid).collect()
                            } else {
                                std::collections::HashSet::new()
                            };
                        let src_vid_to_vertex: std::collections::HashMap<VertexId, Vertex> =
                            input_nodes.iter().map(|v| (v.vid, v.clone())).collect();

                        // Get storage handle for loading dst vertices
                        let storage = self.get_storage().read();

                        for edge in all_edges {
                            // Skip edges whose source is not in the input node set
                            if !input_nodes.is_empty() && !src_set.contains(&edge.src) {
                                continue;
                            }

                            // Create a row with the edge value
                            let mut row = Vec::new();

                            if self.col_names.len() >= 3 {
                                // Use the source vertex from input_nodes (has full properties)
                                let src_vertex = src_vid_to_vertex
                                    .get(&edge.src)
                                    .cloned()
                                    .unwrap_or_else(|| Vertex::new(edge.src, Vec::new()));
                                // Load the destination vertex from storage (needs properties for RETURN)
                                let dst_vertex = storage
                                    .get_vertex(&self.space_name, &edge.dst)
                                    .ok()
                                    .flatten()
                                    .unwrap_or_else(|| Vertex::new(edge.dst, Vec::new()));
                                row.push(Value::Vertex(Box::new(src_vertex)));
                                row.push(Value::edge(edge.clone()));
                                row.push(Value::Vertex(Box::new(dst_vertex)));
                            } else {
                                row.push(Value::edge(edge));
                            }

                            dataset.rows.push(row);
                        }

                        return Ok(ExecutionResult::DataSet(dataset));
                    }
                }
            }
        }

        // Fallback: if no input nodes from executor, src_vids, or context,
        // scan all vertices from the space. This handles MATCH ()-[e]->() patterns
        // where no explicit input source is provided.
        if input_nodes.is_empty() && self.input_executor.is_none() {
            let storage = self.get_storage().read();
            if let Ok(vertices) = storage.scan_vertices(&self.space_name) {
                input_nodes = vertices;
            }
        }

        // Determine the maximum depth.
        let max_depth = self.max_depth.unwrap_or(3); // The default depth is 3.

        for vertex in &input_nodes {
            self.visited_nodes.clear();
            self.visited_nodes.insert(vertex.vid);

            let initial_npath = Arc::new(NPath::new(Arc::new(vertex.clone())));

            let mut expanded_npaths = self.expand_paths_recursive(&initial_npath, 0, max_depth)?;
            self.npath_cache.append(&mut expanded_npaths);
        }

        Ok(self.build_expansion_result())
    }

    fn open(&mut self) -> DBResult<()> {
        self.npath_cache.clear();
        self.path_cache.clear();
        self.visited_nodes.clear();

        if let Some(ref mut input_exec) = self.input_executor {
            input_exec.open()?;
        }
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        self.npath_cache.clear();
        self.path_cache.clear();
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

impl<S: StorageClient + Send> HasStorage<S> for ExpandAllExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base
            .storage
            .as_ref()
            .expect("ExpandAllExecutor storage should be set")
    }
}
