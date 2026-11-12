use crate::executor::expression::ExpressionError;
use crate::executor::expression::evaluation_context::graph_storage::GraphStorageRef;
use linkrs_core::types::VertexId;
use linkrs_core::value::list::List;
use linkrs_core::value::NullType;
use linkrs_core::Value;

pub(crate) fn endpoint_vertex_with_storage(
    edge_type: &str,
    vid: VertexId,
    endpoint_src: bool,
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    let reader = storage.storage.read();
    let info = reader
        .get_edge_type(&storage.space, edge_type)
        .map_err(|e| ExpressionError::function_error(format!("Storage error: {}", e)))?
        .ok_or_else(|| {
            ExpressionError::function_error(format!("Edge type '{edge_type}' not found"))
        })?;
    let tag = if endpoint_src {
        &info.src_tag_name
    } else {
        &info.dst_tag_name
    };
    if tag.is_empty() {
        return Err(ExpressionError::function_error(format!(
            "Edge type '{edge_type}' declares no endpoint label"
        )));
    }
    let vertex = reader
        .get_vertex(&storage.space, tag, &vid)
        .map_err(|e| ExpressionError::function_error(format!("Storage error: {}", e)))?;
    match vertex {
        Some(vertex) => Ok(Value::Vertex(Box::new(vertex))),
        None => Ok(Value::Null(NullType::Null)),
    }
}

pub(crate) fn execute_startnode_with_storage(
    args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Edge(e) => endpoint_vertex_with_storage(&e.edge_type, e.src, true, storage),
        Value::EdgeHeader(h) => endpoint_vertex_with_storage(&h.edge_type, h.src, true, storage),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The startnode function requires the edge type",
        )),
    }
}

pub(crate) fn execute_endnode_with_storage(
    args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Edge(e) => endpoint_vertex_with_storage(&e.edge_type, e.dst, false, storage),
        Value::EdgeHeader(h) => endpoint_vertex_with_storage(&h.edge_type, h.dst, false, storage),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The endnode function requires an edge type",
        )),
    }
}

pub(crate) fn extract_vertex_id(value: &Value) -> Result<VertexId, ExpressionError> {
    match value {
        Value::Vertex(v) => Ok(v.vid),
        Value::BigInt(id) => {
            VertexId::try_from_int64(*id).map_err(|e| ExpressionError::type_error(e.to_string()))
        }
        Value::Int(id) => VertexId::try_from_int64(*id as i64)
            .map_err(|e| ExpressionError::type_error(e.to_string())),
        Value::Null(_) => Err(ExpressionError::type_error(
            "Expected a vertex or vertex ID, got null",
        )),
        _ => Err(ExpressionError::type_error(
            "Expected a vertex or vertex ID",
        )),
    }
}

pub(crate) fn execute_neighbors_with_storage(
    args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    let vid = extract_vertex_id(&args[0])?;
    let neighbors = storage
        .get_neighbors(&vid)
        .map_err(ExpressionError::function_error)?;
    let neighbor_ids: Vec<Value> = neighbors
        .into_iter()
        .map(|(nid, _)| Value::from(nid))
        .collect();
    Ok(Value::list(List {
        values: neighbor_ids,
    }))
}

pub(crate) fn execute_degree_with_storage(
    args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    let vid = extract_vertex_id(&args[0])?;
    let neighbors = storage
        .get_neighbors(&vid)
        .map_err(ExpressionError::function_error)?;
    Ok(Value::BigInt(neighbors.len() as i64))
}

pub(crate) fn execute_out_edges_with_storage(
    args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    use linkrs_core::types::EdgeDirection;
    let vid = extract_vertex_id(&args[0])?;
    let reader = storage.storage.read();
    let edges = reader
        .get_node_edges(&storage.space, &vid, EdgeDirection::Out, &[])
        .map_err(|e| ExpressionError::function_error(format!("Storage error: {}", e)))?;
    drop(reader);
    let edge_values: Vec<Value> = edges
        .into_iter()
        .map(|e| Value::Edge(Box::new(e)))
        .collect();
    Ok(Value::list(List {
        values: edge_values,
    }))
}

pub(crate) fn execute_in_edges_with_storage(
    args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    use linkrs_core::types::EdgeDirection;
    let vid = extract_vertex_id(&args[0])?;
    let reader = storage.storage.read();
    let edges = reader
        .get_node_edges(&storage.space, &vid, EdgeDirection::In, &[])
        .map_err(|e| ExpressionError::function_error(format!("Storage error: {}", e)))?;
    drop(reader);
    let edge_values: Vec<Value> = edges
        .into_iter()
        .map(|e| Value::Edge(Box::new(e)))
        .collect();
    Ok(Value::list(List {
        values: edge_values,
    }))
}

pub(crate) fn execute_shortest_path_with_storage(
    args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
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
            .map_err(ExpressionError::function_error)?;

        for (neighbor_id, _) in neighbors {
            if neighbor_id == end_vid {
                return Ok(Value::BigInt(distance + 1));
            }
            if let std::collections::hash_map::Entry::Vacant(e) = visited.entry(neighbor_id) {
                e.insert(distance + 1);
                queue.push_back(neighbor_id);
            }
        }
    }

    Ok(Value::BigInt(-1))
}

pub(crate) fn execute_bfs_with_storage(
    args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
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
    result.push(Value::from(start_vid));

    while let Some((current, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }

        let neighbors = storage
            .get_neighbors(&current)
            .map_err(ExpressionError::function_error)?;

        for (neighbor_id, _) in neighbors {
            if visited.insert(neighbor_id) {
                queue.push_back((neighbor_id, depth + 1));
                result.push(Value::from(neighbor_id));
            }
        }
    }

    Ok(Value::list(List { values: result }))
}

pub(crate) fn execute_connected_components_with_storage(
    _args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
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
        component.push(Value::from(*start_vid));

        while let Some(current) = queue.pop_front() {
            let neighbors = match storage.get_neighbors(&current) {
                Ok(n) => n,
                Err(_) => continue,
            };
            for (neighbor_id, _) in neighbors {
                if visited.insert(neighbor_id) {
                    queue.push_back(neighbor_id);
                    component.push(Value::from(neighbor_id));
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

pub(crate) fn execute_variable_length_path_with_storage(
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

    if start_vid == end_vid && min_depth <= 0 {
        return Ok(Value::list(List {
            values: vec![Value::list(List {
                values: vec![Value::from(start_vid)],
            })],
        }));
    }

    // DFS with depth limit to find all paths from start_vid to end_vid
    // within the depth range [min_depth, max_depth]
    use std::collections::VecDeque;
    // Each element: (current_vertex, path_so_far, depth)
    // path_so_far is the list of vertex IDs visited (excluding current)
    let mut results: Vec<Vec<Value>> = Vec::new();
    let mut queue: VecDeque<(VertexId, Vec<Value>, i64)> = VecDeque::new();

    queue.push_back((start_vid, vec![Value::from(start_vid)], 0));

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
            let neighbor_val = Value::from(neighbor_id);
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

pub(crate) fn execute_pagerank_with_storage(
    _args: &[Value],
    storage: &GraphStorageRef,
) -> Result<Value, ExpressionError> {
    let reader = storage.storage.read();
    let all_vertices = reader
        .scan_vertices(&storage.space)
        .map_err(|e| ExpressionError::function_error(format!("Storage error: {}", e)))?;
    drop(reader);

    let vertex_count = all_vertices.len();
    if vertex_count == 0 {
        return Ok(Value::map(std::collections::HashMap::new()));
    }

    use std::collections::HashMap;

    let vertex_ids: Vec<VertexId> = all_vertices.iter().map(|v| v.vid).collect();
    let mut vid_to_index: HashMap<VertexId, usize> = HashMap::new();
    for (i, vid) in vertex_ids.iter().enumerate() {
        vid_to_index.insert(*vid, i);
    }

    let damping_factor: f64 = 0.85;
    let max_iterations: usize = 100;
    let tolerance: f64 = 1e-6;

    let mut scores: Vec<f64> = vec![1.0 / vertex_count as f64; vertex_count];
    let mut out_degrees: Vec<usize> = vec![0; vertex_count];

    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); vertex_count];

    for vid in &vertex_ids {
        let reader = storage.storage.read();
        let edges = reader
            .get_node_edges(
                &storage.space,
                vid,
                linkrs_core::types::EdgeDirection::Out,
                &[],
            )
            .map_err(|e| ExpressionError::function_error(format!("Storage error: {}", e)))?;
        drop(reader);

        let src_index = vid_to_index[vid];
        out_degrees[src_index] = edges.len();

        for edge in edges {
            if let Some(&dst_index) = vid_to_index.get(&edge.dst) {
                adjacency[src_index].push(dst_index);
            }
        }
    }

    for _ in 0..max_iterations {
        let mut new_scores: Vec<f64> = vec![0.0; vertex_count];
        let mut dangling_sum: f64 = 0.0;

        for i in 0..vertex_count {
            if out_degrees[i] == 0 {
                dangling_sum += scores[i];
            }
        }

        let base = (1.0 - damping_factor) / vertex_count as f64
            + damping_factor * dangling_sum / vertex_count as f64;
        for score in new_scores.iter_mut() {
            *score = base;
        }

        for i in 0..vertex_count {
            if out_degrees[i] > 0 {
                let share = scores[i] / out_degrees[i] as f64;
                for &j in &adjacency[i] {
                    new_scores[j] += damping_factor * share;
                }
            }
        }

        let mut diff: f64 = 0.0;
        for i in 0..vertex_count {
            diff += (new_scores[i] - scores[i]).abs();
        }

        scores = new_scores;

        if diff < tolerance {
            break;
        }
    }

    let mut result: HashMap<String, Value> = HashMap::new();
    for (i, vid) in vertex_ids.iter().enumerate() {
        let vid_str = vid.to_string();
        result.insert(vid_str, Value::Double(scores[i]));
    }

    Ok(Value::string_map(result))
}
