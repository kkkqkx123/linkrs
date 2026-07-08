//! Definition of graph algorithm sharing types
//!
//! Shared data structures used by various graph traversal and pathfinding algorithms

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::core::types::VertexId;
use crate::core::{Edge, NPath, Path, Value};

/// Multi-source shortest path request
/// A pathfinding request that specifies a pair of starting and ending points.
#[derive(Debug, Clone)]
pub struct MultiPathRequest {
    /// Source vertex ID
    pub src: Value,
    /// Destination vertex ID
    pub dst: Value,
    /// Has the path been found?
    pub found: bool,
}

impl MultiPathRequest {
    pub fn new(src: Value, dst: Value) -> Self {
        Self {
            src,
            dst,
            found: false,
        }
    }

    pub fn mark_found(&mut self) {
        self.found = true;
    }
}

/// Multi-source shortest path termination mapping table
pub type TerminationMap = HashMap<VertexId, Vec<(VertexId, bool)>>;

/// Intermediate path mapping
pub type Interims = HashMap<VertexId, HashMap<VertexId, Vec<Path>>>;

/// Create a termination mapping table
pub fn create_termination_map(start_vids: &[VertexId], end_vids: &[VertexId]) -> TerminationMap {
    let mut map = HashMap::new();
    for src in start_vids {
        let pairs: Vec<(VertexId, bool)> = end_vids.iter().map(|dst| (*dst, true)).collect();
        map.insert(*src, pairs);
    }
    map
}

/// Check whether the termination of the mapping table has been completed in its entirety.
pub fn is_termination_complete(map: &TerminationMap) -> bool {
    map.is_empty()
}

/// The marked path has been found.
pub fn mark_path_found(map: &mut TerminationMap, src: &VertexId, dst: &VertexId) -> bool {
    if let Some(pairs) = map.get_mut(src) {
        for (d, found) in pairs.iter_mut() {
            if d == dst {
                *found = false;
                return true;
            }
        }
    }
    false
}

/// Clean up the found path pairs.
pub fn cleanup_termination_map(map: &mut TerminationMap) {
    map.retain(|_, pairs| {
        pairs.retain(|(_, found)| *found);
        !pairs.is_empty()
    });
}

/// Auxiliary structure for removing duplicates from self-loop edges
/// Used to track the processed self-loop edges during the traversal process.
#[derive(Debug, Default)]
pub struct SelfLoopDedup {
    seen: HashSet<(String, i64)>,
    with_loop: bool,
}

impl SelfLoopDedup {
    pub fn new() -> Self {
        Self {
            seen: HashSet::new(),
            with_loop: false,
        }
    }

    /// Create a structure that allows for the removal of duplicates, including self-looping edges.
    pub fn with_loop(with_loop: bool) -> Self {
        Self {
            seen: HashSet::new(),
            with_loop,
        }
    }

    /// Check and record the self-loop edges.
    /// Returning `true` indicates that the edge should be included (either as it appears for the first time or because self-loops are allowed).
    /// Returning `false` indicates that this edge should be skipped (it represents a duplicate, self-looping edge).
    pub fn should_include(&mut self, edge: &Edge) -> bool {
        let is_self_loop = edge.src == edge.dst;
        if is_self_loop {
            if self.with_loop {
                return true;
            }
            let key = (edge.edge_type.clone(), edge.ranking);
            self.seen.insert(key)
        } else {
            true
        }
    }
}

/// Dijkstra's algorithm for calculating distances between nodes
#[derive(Debug, Clone)]
pub struct DistanceNode {
    pub distance: f64,
    pub vertex_id: VertexId,
}

impl Eq for DistanceNode {}

impl PartialEq for DistanceNode {
    fn eq(&self, other: &Self) -> bool {
        self.distance == other.distance && self.vertex_id == other.vertex_id
    }
}

impl Ord for DistanceNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .distance
            .partial_cmp(&self.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

impl PartialOrd for DistanceNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Bidirectional BFS state
#[derive(Debug, Clone)]
pub struct BidirectionalBFSState {
    pub left_queue: VecDeque<(VertexId, Arc<NPath>)>,
    pub right_queue: VecDeque<(VertexId, Arc<NPath>)>,
    pub left_visited: HashMap<VertexId, (Arc<NPath>, f64)>,
    pub right_visited: HashMap<VertexId, (Arc<NPath>, f64)>,
    pub left_edges: Vec<HashMap<VertexId, Vec<(Edge, VertexId)>>>,
    pub right_edges: Vec<HashMap<VertexId, Vec<(Edge, VertexId)>>>,
}

impl BidirectionalBFSState {
    pub fn new() -> Self {
        Self {
            left_queue: VecDeque::new(),
            right_queue: VecDeque::new(),
            left_visited: HashMap::new(),
            right_visited: HashMap::new(),
            left_edges: Vec::new(),
            right_edges: Vec::new(),
        }
    }
}

impl Default for BidirectionalBFSState {
    fn default() -> Self {
        Self::new()
    }
}

/// Algorithm statistics information
#[derive(Debug, Clone, Default)]
pub struct AlgorithmStats {
    pub nodes_visited: usize,
    pub edges_traversed: usize,
    pub execution_time_ms: u64,
}

impl AlgorithmStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn increment_nodes_visited(&mut self) {
        self.nodes_visited += 1;
    }

    pub fn increment_edges_traversed(&mut self, count: usize) {
        self.edges_traversed += count;
    }

    pub fn set_execution_time(&mut self, time_ms: u64) {
        self.execution_time_ms = time_ms;
    }
}

/// Types of shortest path algorithms
#[derive(Debug, Clone)]
pub enum ShortestPathAlgorithmType {
    BFS,
    Dijkstra,
    AStar,
}

/// Edge weight configuration
#[derive(Debug, Clone, Default)]
pub enum EdgeWeightConfig {
    /// For the “No Permission Map” scenario, the number of steps taken is used as a measure of distance.
    #[default]
    Unweighted,
    /// Use the ranking of the edges as a weight.
    Ranking,
    /// Use the specified attributes as weights.
    Property(String),
}

/// Heuristic function type
/// Used in the A* algorithm to estimate the cost from the current node to the target node.
#[derive(Debug, Clone, Default)]
pub enum HeuristicFunction {
    /// Zero-heuristics approach; degenerates into the Dijkstra algorithm.
    #[default]
    Zero,
    /// Using vertex attributes to calculate heuristics (such as coordinates)
    /// The parameter is the name of an attribute, which is used to obtain the spatial coordinates.
    PropertyDistance(String, String), // (lat_prop, lon_prop)
    /// Use fixed weight factors
    ScaleFactor(f64),
}

impl HeuristicFunction {
    /// Calculate the heuristic value.
    ///
    /// # Arguments
    /// - * `current` – The value of the current node.
    /// - * `target` – The value of the target node.
    /// - `current_props` – Properties of the current node.
    /// - `target_props` – Properties of the target node
    ///
    /// # Returns
    /// Heuristic estimates (which must meet the requirement of admissibility: not overestimating the actual cost)
    pub fn evaluate(
        &self,
        _current: &Value,
        _target: &Value,
        current_props: Option<&std::collections::HashMap<String, crate::core::Value>>,
        target_props: Option<&std::collections::HashMap<String, crate::core::Value>>,
    ) -> f64 {
        match self {
            HeuristicFunction::Zero => 0.0,
            HeuristicFunction::PropertyDistance(lat_prop, lon_prop) => {
                let get_coords = |props: &std::collections::HashMap<String, crate::core::Value>| -> Option<(f64, f64)> {
                    let lat = props.get(lat_prop).and_then(|v| match v {
                        crate::core::Value::Float(f) => Some((*f).into()),
                        crate::core::Value::Double(f) => Some(*f),
                        crate::core::Value::SmallInt(i) => Some(*i as f64),
                        crate::core::Value::Int(i) => Some(*i as f64),
                        crate::core::Value::BigInt(i) => Some(*i as f64),
                        _ => None,
                    });
                    let lon = props.get(lon_prop).and_then(|v| match v {
                        crate::core::Value::Float(f) => Some((*f).into()),
                        crate::core::Value::Double(f) => Some(*f),
                        crate::core::Value::SmallInt(i) => Some(*i as f64),
                        crate::core::Value::Int(i) => Some(*i as f64),
                        crate::core::Value::BigInt(i) => Some(*i as f64),
                        _ => None,
                    });
                    match (lat, lon) {
                        (Some(la), Some(lo)) => Some((la, lo)),
                        _ => None,
                    }
                };

                if let (Some(c_props), Some(t_props)) = (current_props, target_props) {
                    if let (Some((c_lat, c_lon)), Some((t_lat, t_lon))) =
                        (get_coords(c_props), get_coords(t_props))
                    {
                        const EARTH_RADIUS_KM: f64 = 6371.0;
                        let to_rad = |deg: f64| deg * std::f64::consts::PI / 180.0;

                        let lat1 = to_rad(c_lat);
                        let lat2 = to_rad(t_lat);
                        let dlat = to_rad(t_lat - c_lat);
                        let dlon = to_rad(t_lon - c_lon);

                        let a = (dlat / 2.0).sin().powi(2)
                            + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
                        let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
                        EARTH_RADIUS_KM * c
                    } else {
                        0.0
                    }
                } else {
                    0.0
                }
            }
            HeuristicFunction::ScaleFactor(factor) => *factor,
        }
    }

    /// Is it a zero-heuristic approach?
    pub fn is_zero(&self) -> bool {
        matches!(self, HeuristicFunction::Zero)
    }
}

impl EdgeWeightConfig {
    /// Is it a weighted graph?
    pub fn is_weighted(&self) -> bool {
        !matches!(self, EdgeWeightConfig::Unweighted)
    }

    /// Obtain the name of the weight attribute.
    pub fn property_name(&self) -> Option<&str> {
        match self {
            EdgeWeightConfig::Property(name) => Some(name.as_str()),
            _ => None,
        }
    }
}

/// Path concatenation tool function
/// The left path goes from the starting point to the middle point, while the right path goes from the ending point to the middle point.
pub fn combine_npaths(left: &Arc<NPath>, right: &Arc<NPath>) -> Option<Path> {
    if left.vertex().vid != right.vertex().vid {
        return None;
    }

    let left_path = left.to_path();

    let mut right_path = right.to_path();
    right_path.reverse();

    let mut combined = left_path;
    combined.steps.extend(right_path.steps);

    Some(combined)
}

/// Check whether there are any duplicate edges in the path.
pub fn has_duplicate_edges(path: &Path) -> bool {
    let mut edge_set = HashSet::new();

    for step in &path.steps {
        let edge = &step.edge;
        let edge_key = format!("{}_{}_{}", edge.src, edge.dst, edge.ranking);
        if !edge_set.insert(edge_key) {
            return true;
        }
    }

    false
}
