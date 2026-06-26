//! Implementation of graph-related functions
//!
//! Provide functions for manipulating vertices and edges, including id, tags, labels, properties, type, src, dst, and rank.
//! Also includes graph traversal functions: neighbors, degree, shortest_path.

use crate::core::types::VertexId;
use crate::core::value::list::List;
use crate::core::value::NullType;
use crate::core::vertex_edge_path::Vertex;
use crate::core::Value;
use crate::query::executor::expression::evaluation_context::graph_storage::GraphStorageRef;
use crate::query::executor::expression::ExpressionError;

/// Graph function enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphFunction {
    Id,
    Tags,
    Labels,
    Properties,
    EdgeType,
    Src,
    Dst,
    Rank,
    StartNode,
    EndNode,
    Neighbors,
    Degree,
    OutEdges,
    InEdges,
    ShortestPath,
    Bfs,
    ConnectedComponents,
    VariableLengthPath,
}

impl GraphFunction {
    /// Obtain the function name
    pub fn name(&self) -> &str {
        match self {
            Self::Id => "id",
            Self::Tags => "tags",
            Self::Labels => "labels",
            Self::Properties => "properties",
            Self::EdgeType => "type",
            Self::Src => "src",
            Self::Dst => "dst",
            Self::Rank => "rank",
            Self::StartNode => "startnode",
            Self::EndNode => "endnode",
            Self::Neighbors => "neighbors",
            Self::Degree => "degree",
            Self::OutEdges => "out_edges",
            Self::InEdges => "in_edges",
            Self::ShortestPath => "shortest_path",
            Self::Bfs => "bfs",
            Self::ConnectedComponents => "connected_components",
            Self::VariableLengthPath => "variable_length_path",
        }
    }

    /// Determine the number of parameters
    pub fn arity(&self) -> usize {
        match self {
            Self::Id => 1,
            Self::Tags => 1,
            Self::Labels => 1,
            Self::Properties => 1,
            Self::EdgeType => 1,
            Self::Src => 1,
            Self::Dst => 1,
            Self::Rank => 1,
            Self::StartNode => 1,
            Self::EndNode => 1,
            Self::Neighbors => 1,
            Self::Degree => 1,
            Self::OutEdges => 1,
            Self::InEdges => 1,
            Self::ShortestPath => 2,
            Self::Bfs => 2,
            Self::ConnectedComponents => 0,
            Self::VariableLengthPath => 3,
        }
    }

    /// Is it a function with variable parameters?
    pub fn is_variadic(&self) -> bool {
        matches!(self, Self::VariableLengthPath)
    }

    /// Obtain the function description
    pub fn description(&self) -> &str {
        match self {
            Self::Id => "Get the ID of the vertex",
            Self::Tags => "Get all labels of a vertex",
            Self::Labels => "Get all labels (aliases) of a vertex",
            Self::Properties => "Get all attributes of a vertex or edge",
            Self::EdgeType => "Get the type of the edge",
            Self::Src => "Get the starting vertex ID of the edge",
            Self::Dst => "Get the target vertex ID of the edge",
            Self::Rank => "Get the edge's rank value",
            Self::StartNode => "Get the starting vertex of the edge",
            Self::EndNode => "Get the target vertex of the edge",
            Self::Neighbors => "Get all neighbor vertex IDs of a vertex",
            Self::Degree => "Get the degree (number of edges) of a vertex",
            Self::OutEdges => "Get all outgoing edges of a vertex",
            Self::InEdges => "Get all incoming edges of a vertex",
            Self::ShortestPath => "Find the shortest path between two vertices (returns path length)",
            Self::Bfs => "Breadth-first search traversal (start_vid, max_depth)",
            Self::ConnectedComponents => "Find all connected components in the graph",
            Self::VariableLengthPath => "Find all paths between two vertices within a depth range (start_vid, end_vid, max_depth) or (start_vid, end_vid, min_depth, max_depth)",
        }
    }

    pub fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        match self {
            Self::Id => execute_id(args),
            Self::Tags => execute_tags(args),
            Self::Labels => execute_labels(args),
            Self::Properties => execute_properties(args),
            Self::EdgeType => execute_edge_type(args),
            Self::Src => execute_src(args),
            Self::Dst => execute_dst(args),
            Self::Rank => execute_rank(args),
            Self::StartNode => execute_startnode(args),
            Self::EndNode => execute_endnode(args),
            Self::Neighbors => execute_neighbors(args),
            Self::Degree => execute_degree(args),
            Self::OutEdges => execute_out_edges(args),
            Self::InEdges => execute_in_edges(args),
            Self::ShortestPath => execute_shortest_path(args),
            Self::Bfs => execute_bfs(args),
            Self::ConnectedComponents => execute_connected_components(args),
            Self::VariableLengthPath => execute_variable_length_path(args),
        }
    }

    /// Execute with graph storage access.
    /// Falls back to execute() if no results can be obtained from storage.
    pub fn execute_with_storage(
        &self,
        args: &[Value],
        storage: &GraphStorageRef,
    ) -> Result<Value, ExpressionError> {
        match self {
            Self::Neighbors => execute_neighbors_with_storage(args, storage),
            Self::Degree => execute_degree_with_storage(args, storage),
            Self::OutEdges => execute_out_edges_with_storage(args, storage),
            Self::InEdges => execute_in_edges_with_storage(args, storage),
            Self::ShortestPath => execute_shortest_path_with_storage(args, storage),
            Self::Bfs => execute_bfs_with_storage(args, storage),
            Self::ConnectedComponents => execute_connected_components_with_storage(args, storage),
            Self::VariableLengthPath => execute_variable_length_path_with_storage(args, storage),
            _ => self.execute(args),
        }
    }
}

fn execute_id(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The id function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Vertex(v) => Ok(Value::BigInt(v.vid.as_int64().unwrap_or(0))),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The id function requires a vertex type",
        )),
    }
}

fn execute_tags(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The tags function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Vertex(v) => {
            let tags: Vec<Value> = v
                .tags
                .iter()
                .map(|tag| Value::String(tag.name.clone()))
                .collect();
            Ok(Value::list(List { values: tags }))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("tags requires vertex type")),
    }
}

fn execute_labels(args: &[Value]) -> Result<Value, ExpressionError> {
    execute_tags(args)
}

fn execute_properties(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "properties requires 1 argument",
        ));
    }
    match &args[0] {
        Value::Vertex(v) => {
            let mut props = std::collections::HashMap::new();
            for tag in &v.tags {
                props.extend(tag.properties.clone());
            }
            props.extend(v.properties.clone());
            Ok(Value::map(props))
        }
        Value::Edge(e) => Ok(Value::map(e.props.clone())),
        Value::Map(m) => Ok(Value::map((**m).clone())),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "properties requires vertex, edge or map type",
        )),
    }
}

fn execute_edge_type(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The type function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Edge(e) => Ok(Value::String(e.edge_type.clone())),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The type function requires an edge type",
        )),
    }
}

fn execute_src(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The src function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Edge(e) => Ok(Value::BigInt(e.src.as_int64().unwrap_or(0))),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The src function requires the edge type",
        )),
    }
}

fn execute_dst(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The dst function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Edge(e) => Ok(Value::BigInt(e.dst.as_int64().unwrap_or(0))),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The dst function requires an edge type",
        )),
    }
}

fn execute_rank(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The rank function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Edge(e) => Ok(Value::BigInt(e.ranking)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The rank function requires an edge type",
        )),
    }
}

fn execute_startnode(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The startnode function takes 1 argument.",
        ));
    }
    match &args[0] {
        Value::Edge(e) => {
            let vertex = Vertex::new(e.src, vec![]);
            Ok(Value::Vertex(Box::new(vertex)))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The startnode function requires the edge type",
        )),
    }
}

fn execute_endnode(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The endnode function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Edge(e) => {
            let vertex = Vertex::new(e.dst, vec![]);
            Ok(Value::Vertex(Box::new(vertex)))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The endnode function requires an edge type",
        )),
    }
}

fn extract_vertex_id(value: &Value) -> Result<VertexId, ExpressionError> {
    match value {
        Value::Vertex(v) => Ok(v.vid),
        Value::BigInt(id) => Ok(VertexId::from_int64(*id)),
        Value::Int(id) => Ok(VertexId::from_int64(*id as i64)),
        Value::Null(_) => Err(ExpressionError::type_error(
            "Expected a vertex or vertex ID, got null",
        )),
        _ => Err(ExpressionError::type_error(
            "Expected a vertex or vertex ID",
        )),
    }
}

fn execute_neighbors(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The neighbors function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Vertex(v) => {
            let vid = v.vid.as_int64().unwrap_or(0);
            Ok(Value::BigInt(vid))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The neighbors function requires a vertex type",
        )),
    }
}

fn execute_degree(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The degree function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Vertex(v) => {
            let degree = v
                .properties
                .iter()
                .filter(|(k, _)| k.starts_with("neighbor_"))
                .count();
            Ok(Value::BigInt(degree as i64))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The degree function requires a vertex type",
        )),
    }
}

fn execute_out_edges(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The out_edges function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Vertex(v) => {
            let edges: Vec<Value> = v
                .properties
                .iter()
                .filter(|(k, _)| k.starts_with("out_"))
                .map(|(_, v)| v.clone())
                .collect();
            Ok(Value::list(List { values: edges }))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The out_edges function requires a vertex type",
        )),
    }
}

fn execute_in_edges(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The in_edges function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::Vertex(v) => {
            let edges: Vec<Value> = v
                .properties
                .iter()
                .filter(|(k, _)| k.starts_with("in_"))
                .map(|(_, v)| v.clone())
                .collect();
            Ok(Value::list(List { values: edges }))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The in_edges function requires a vertex type",
        )),
    }
}

fn execute_shortest_path(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "The shortest_path function takes 2 arguments (start_vid, end_vid)",
        ));
    }
    let start_vid = match &args[0] {
        Value::BigInt(id) => *id,
        Value::Int(id) => *id as i64,
        _ => {
            return Err(ExpressionError::type_error(
                "shortest_path requires integer vertex IDs",
            ))
        }
    };
    let end_vid = match &args[1] {
        Value::BigInt(id) => *id,
        Value::Int(id) => *id as i64,
        _ => {
            return Err(ExpressionError::type_error(
                "shortest_path requires integer vertex IDs",
            ))
        }
    };
    if start_vid == end_vid {
        return Ok(Value::BigInt(0));
    }
    Ok(Value::BigInt(-1))
}

fn execute_bfs(_args: &[Value]) -> Result<Value, ExpressionError> {
    Err(ExpressionError::function_error(
        "bfs() requires graph storage access; use within a query context".to_string(),
    ))
}

fn execute_connected_components(_args: &[Value]) -> Result<Value, ExpressionError> {
    Err(ExpressionError::function_error(
        "connected_components() requires graph storage access; use within a query context"
            .to_string(),
    ))
}

// --- Storage-backed implementations ---

fn execute_neighbors_with_storage(
    args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The neighbors function takes 1 argument",
        ));
    }
    let vid = extract_vertex_id(&args[0])?;
    let neighbors = storage
        .get_neighbors(&vid)
        .map_err(|e| ExpressionError::function_error(e))?;
    let neighbor_ids: Vec<Value> = neighbors
        .into_iter()
        .map(|(nid, _)| Value::BigInt(nid.as_int64().unwrap_or(0)))
        .collect();
    Ok(Value::list(List { values: neighbor_ids }))
}

fn execute_degree_with_storage(
    args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The degree function takes 1 argument",
        ));
    }
    let vid = extract_vertex_id(&args[0])?;
    let neighbors = storage
        .get_neighbors(&vid)
        .map_err(|e| ExpressionError::function_error(e))?;
    Ok(Value::BigInt(neighbors.len() as i64))
}

fn execute_out_edges_with_storage(
    args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The out_edges function takes 1 argument",
        ));
    }
    use crate::core::types::EdgeDirection;
    let vid = extract_vertex_id(&args[0])?;
    let reader = storage.storage.read();
    let edges = reader
        .get_node_edges(&storage.space, &vid, EdgeDirection::Out)
        .map_err(|e| ExpressionError::function_error(format!("Storage error: {}", e)))?;
    drop(reader);
    let edge_values: Vec<Value> = edges.into_iter().map(|e| Value::Edge(Box::new(e))).collect();
    Ok(Value::list(List {
        values: edge_values,
    }))
}

fn execute_in_edges_with_storage(
    args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The in_edges function takes 1 argument",
        ));
    }
    use crate::core::types::EdgeDirection;
    let vid = extract_vertex_id(&args[0])?;
    let reader = storage.storage.read();
    let edges = reader
        .get_node_edges(&storage.space, &vid, EdgeDirection::In)
        .map_err(|e| ExpressionError::function_error(format!("Storage error: {}", e)))?;
    drop(reader);
    let edge_values: Vec<Value> = edges.into_iter().map(|e| Value::Edge(Box::new(e))).collect();
    Ok(Value::list(List {
        values: edge_values,
    }))
}

fn execute_shortest_path_with_storage(
    args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "The shortest_path function takes 2 arguments (start_vid, end_vid)",
        ));
    }
    let start_vid = extract_vertex_id(&args[0])?;
    let end_vid = extract_vertex_id(&args[1])?;

    if start_vid == end_vid {
        return Ok(Value::BigInt(0));
    }

    // Simple BFS to find shortest path length
    use std::collections::{HashMap, VecDeque};
    let mut visited: HashMap<VertexId, i64> = HashMap::new();
    let mut queue: VecDeque<VertexId> = VecDeque::new();

    visited.insert(start_vid, 0);
    queue.push_back(start_vid);

    while let Some(current) = queue.pop_front() {
        let distance = visited[&current];

        let neighbors = storage
            .get_neighbors(&current)
            .map_err(|e| ExpressionError::function_error(e))?;

        for (neighbor_id, _) in neighbors {
            if neighbor_id == end_vid {
                return Ok(Value::BigInt(distance + 1));
            }
            if !visited.contains_key(&neighbor_id) {
                visited.insert(neighbor_id, distance + 1);
                queue.push_back(neighbor_id);
            }
        }
    }

    Ok(Value::BigInt(-1))
}

fn execute_bfs_with_storage(
    args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "The bfs function takes 2 arguments (start_vid, max_depth)",
        ));
    }
    let start_vid = extract_vertex_id(&args[0])?;
    let max_depth = match &args[1] {
        Value::BigInt(d) => *d,
        Value::Int(d) => *d as i64,
        _ => {
            return Err(ExpressionError::type_error(
                "bfs max_depth must be an integer",
            ))
        }
    };

    use std::collections::{HashSet, VecDeque};
    let mut visited: HashSet<VertexId> = HashSet::new();
    let mut queue: VecDeque<(VertexId, i64)> = VecDeque::new();
    let mut result: Vec<Value> = Vec::new();

    visited.insert(start_vid);
    queue.push_back((start_vid, 0));
    result.push(Value::BigInt(start_vid.as_int64().unwrap_or(0)));

    while let Some((current, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }

        let neighbors = storage
            .get_neighbors(&current)
            .map_err(|e| ExpressionError::function_error(e))?;

        for (neighbor_id, _) in neighbors {
            if visited.insert(neighbor_id) {
                queue.push_back((neighbor_id, depth + 1));
                result.push(Value::BigInt(neighbor_id.as_int64().unwrap_or(0)));
            }
        }
    }

    Ok(Value::list(List { values: result }))
}

fn execute_connected_components_with_storage(
    _args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    if !_args.is_empty() {
        return Err(ExpressionError::type_error(
            "The connected_components function takes no arguments",
        ));
    }

    // Get all vertices from storage
    let reader = storage.storage.read();
    let all_vertices = reader
        .scan_vertices(&storage.space)
        .map_err(|e| ExpressionError::function_error(format!("Storage error: {}", e)))?;
    drop(reader);

    use std::collections::{HashSet, VecDeque};

    let all_vids: HashSet<VertexId> = all_vertices.iter().map(|v| v.vid).collect();
    let mut visited: HashSet<VertexId> = HashSet::new();
    let mut components: Vec<Vec<Value>> = Vec::new();

    for start_vid in &all_vids {
        if visited.contains(start_vid) {
            continue;
        }

        let mut component: Vec<Value> = Vec::new();
        let mut queue: VecDeque<VertexId> = VecDeque::new();
        visited.insert(*start_vid);
        queue.push_back(*start_vid);
        component.push(Value::BigInt(start_vid.as_int64().unwrap_or(0)));

        while let Some(current) = queue.pop_front() {
            let neighbors = match storage.get_neighbors(&current) {
                Ok(n) => n,
                Err(_) => continue,
            };
            for (neighbor_id, _) in neighbors {
                if visited.insert(neighbor_id) {
                    queue.push_back(neighbor_id);
                    component.push(Value::BigInt(neighbor_id.as_int64().unwrap_or(0)));
                }
            }
        }

        components.push(component);
    }

    let component_lists: Vec<Value> = components
        .into_iter()
        .map(|comp| Value::list(List { values: comp }))
        .collect();

    Ok(Value::list(List {
        values: component_lists,
    }))
}

fn execute_variable_length_path(_args: &[Value]) -> Result<Value, ExpressionError> {
    Err(ExpressionError::type_error(
        "variable_length_path() requires graph storage access; use within a query context",
    ))
}

fn execute_variable_length_path_with_storage(
    args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    let (start_vid, end_vid, min_depth, max_depth) = match args.len() {
        3 => {
            let sv = extract_vertex_id(&args[0])?;
            let ev = extract_vertex_id(&args[1])?;
            let md = match &args[2] {
                Value::BigInt(d) => *d,
                Value::Int(d) => *d as i64,
                _ => {
                    return Err(ExpressionError::type_error(
                        "variable_length_path max_depth must be an integer",
                    ))
                }
            };
            (sv, ev, 1i64, md)
        }
        4 => {
            let sv = extract_vertex_id(&args[0])?;
            let ev = extract_vertex_id(&args[1])?;
            let mind = match &args[2] {
                Value::BigInt(d) => *d,
                Value::Int(d) => *d as i64,
                _ => {
                    return Err(ExpressionError::type_error(
                        "variable_length_path min_depth must be an integer",
                    ))
                }
            };
            let maxd = match &args[3] {
                Value::BigInt(d) => *d,
                Value::Int(d) => *d as i64,
                _ => {
                    return Err(ExpressionError::type_error(
                        "variable_length_path max_depth must be an integer",
                    ))
                }
            };
            (sv, ev, mind, maxd)
        }
        _ => {
            return Err(ExpressionError::type_error(
                "variable_length_path takes 3 or 4 arguments (start_vid, end_vid, max_depth) or (start_vid, end_vid, min_depth, max_depth)",
            ))
        }
    };

    if min_depth < 1 || max_depth < min_depth {
        return Err(ExpressionError::type_error(
            "variable_length_path: min_depth must be >= 1 and max_depth must be >= min_depth",
        ));
    }

    if start_vid == end_vid {
        if min_depth <= 0 {
            return Ok(Value::list(List {
                values: vec![Value::list(List { values: vec![
                    Value::BigInt(start_vid.as_int64().unwrap_or(0)),
                ] })],
            }));
        }
    }

    // DFS with depth limit to find all paths from start_vid to end_vid
    // within the depth range [min_depth, max_depth]
    use std::collections::VecDeque;
    // Each element: (current_vertex, path_so_far, depth)
    // path_so_far is the list of vertex IDs visited (excluding current)
    let mut results: Vec<Vec<Value>> = Vec::new();
    let mut queue: VecDeque<(VertexId, Vec<Value>, i64)> = VecDeque::new();

    queue.push_back((
        start_vid,
        vec![Value::BigInt(start_vid.as_int64().unwrap_or(0))],
        0,
    ));

    while let Some((current, path, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }

        let neighbors = match storage.get_neighbors(&current) {
            Ok(n) => n,
            Err(_) => continue,
        };

        for (neighbor_id, _) in neighbors {
            // Avoid cycles - skip if neighbor already in path
            let neighbor_val = Value::BigInt(neighbor_id.as_int64().unwrap_or(0));
            if path.contains(&neighbor_val) {
                continue;
            }

            let new_depth = depth + 1;
            let mut new_path = path.clone();
            new_path.push(neighbor_val.clone());

            if neighbor_id == end_vid && new_depth >= min_depth {
                results.push(new_path.clone());
            }

            // Limit total results to avoid excessive memory usage
            if results.len() >= 1000 {
                break;
            }

            queue.push_back((neighbor_id, new_path, new_depth));
        }

        if results.len() >= 1000 {
            break;
        }
    }

    let path_values: Vec<Value> = results
        .into_iter()
        .map(|path| Value::list(List { values: path }))
        .collect();

    Ok(Value::list(List {
        values: path_values,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::VertexId;
    use crate::core::vertex_edge_path::{Edge, Tag};
    use std::collections::HashMap;

    fn create_test_vertex() -> Vertex {
        let tag1 = Tag::new(
            "person".to_string(),
            HashMap::from([
                ("name".to_string(), Value::String("Alice".to_string())),
                ("age".to_string(), Value::Int(30)),
            ]),
        );
        let tag2 = Tag::new(
            "employee".to_string(),
            HashMap::from([("dept".to_string(), Value::String("Engineering".to_string()))]),
        );
        Vertex::new(VertexId::from_int64(1), vec![tag1, tag2])
    }

    fn create_test_edge() -> Edge {
        Edge::new(
            VertexId::from_int64(1),
            VertexId::from_int64(2),
            "knows".to_string(),
            0,
            HashMap::from([("since".to_string(), Value::Int(2020))]),
        )
    }

    #[test]
    fn test_id_function() {
        let vertex = create_test_vertex();
        let result = GraphFunction::Id
            .execute(&[Value::Vertex(Box::new(vertex))])
            .expect("The id function execution should succeed");
        assert_eq!(result, Value::BigInt(1));
    }

    #[test]
    fn test_tags_function() {
        let vertex = create_test_vertex();
        let result = GraphFunction::Tags
            .execute(&[Value::Vertex(Box::new(vertex))])
            .expect("The tags function should be executed successfully");
        if let Value::List(tags) = result {
            assert_eq!(tags.values.len(), 2);
        } else {
            panic!("The `tags` function should return a list.");
        }
    }

    #[test]
    fn test_properties_vertex() {
        let vertex = create_test_vertex();
        let result = GraphFunction::Properties
            .execute(&[Value::Vertex(Box::new(vertex))])
            .expect("The properties function should execute successfully");
        if let Value::Map(props) = result {
            assert!(props.contains_key("name"));
            assert!(props.contains_key("age"));
            assert!(props.contains_key("dept"));
        } else {
            panic!("The `properties` function should return a map.");
        }
    }

    #[test]
    fn test_type_function() {
        let edge = create_test_edge();
        let result = GraphFunction::EdgeType
            .execute(&[Value::Edge(Box::new(edge))])
            .expect("The type function execution should succeed");
        assert_eq!(result, Value::String("knows".to_string()));
    }

    #[test]
    fn test_src_function() {
        let edge = create_test_edge();
        let result = GraphFunction::Src
            .execute(&[Value::Edge(Box::new(edge))])
            .expect("The src function execution should succeed");
        assert_eq!(result, Value::BigInt(1));
    }

    #[test]
    fn test_dst_function() {
        let edge = create_test_edge();
        let result = GraphFunction::Dst
            .execute(&[Value::Edge(Box::new(edge))])
            .expect("The dst function execution should succeed");
        assert_eq!(result, Value::BigInt(2));
    }

    #[test]
    fn test_rank_function() {
        let edge = create_test_edge();
        let result = GraphFunction::Rank
            .execute(&[Value::Edge(Box::new(edge))])
            .expect("The execution of the rank function should succeed");
        assert_eq!(result, Value::BigInt(0));
    }

    #[test]
    fn test_null_handling() {
        let null_value = Value::Null(NullType::Null);

        assert_eq!(
            GraphFunction::Id
                .execute(std::slice::from_ref(&null_value))
                .expect("The id function should handle NULL"),
            Value::Null(NullType::Null)
        );
        assert_eq!(
            GraphFunction::Tags
                .execute(std::slice::from_ref(&null_value))
                .expect("The tags function should handle NULL"),
            Value::Null(NullType::Null)
        );
        assert_eq!(
            GraphFunction::Properties
                .execute(std::slice::from_ref(&null_value))
                .expect("The properties function should handle NULL."),
            Value::Null(NullType::Null)
        );
    }
}
