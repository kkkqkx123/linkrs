use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet, VecDeque};

use crate::executor::streaming::query_registry::CancelToken;
use crate::executor::traversal::config::{TraversalConfig, TraversalOrder, VisitedPolicy};
use crate::executor::traversal::graph_reader::TraversalGraphReader;
use crate::executor::traversal::stats::TraversalStats;
use crate::parser::ast::pattern::PathSemantic;
use graphdb_core::error::QueryError;
use graphdb_core::types::storage_ids::VertexId;
use graphdb_core::{Edge, Vertex};

type EdgeKey = (VertexId, VertexId, String, i64);

#[derive(Debug, Clone)]
pub struct TraversalItem {
    pub vertex_id: VertexId,
    pub vertex: Vertex,
    pub depth: u32,
    pub edge: Option<Edge>,
    path_vertices: HashSet<VertexId>,
    path_edges: HashSet<EdgeKey>,
}

#[derive(Debug, Clone)]
pub struct TraversalEvent {
    pub vertex: Vertex,
    pub depth: u32,
    pub edge: Option<Edge>,
}

pub struct TraversalRuntime<'a> {
    pub reader: TraversalGraphReader<'a>,
    pub config: TraversalConfig,
    pub stats: TraversalStats,

    frontier: VecDeque<TraversalItem>,
    visited: HashSet<VertexId>,
    results: VecDeque<TraversalEvent>,
    exhausted: bool,
    total_emitted: usize,

    /// Optional cancel token for cooperative cancellation.
    /// When set, `expand_frontier` checks the token at each iteration
    /// boundary and returns early if cancelled.
    cancel_token: Option<CancelToken>,
}

impl<'a> TraversalRuntime<'a> {
    pub fn new(reader: TraversalGraphReader<'a>, config: TraversalConfig) -> Self {
        Self {
            reader,
            config,
            stats: TraversalStats::default(),
            frontier: VecDeque::new(),
            visited: HashSet::new(),
            results: VecDeque::new(),
            exhausted: false,
            total_emitted: 0,
            cancel_token: None,
        }
    }

    /// Attach an optional cancel token for cooperative cancellation.
    pub fn with_cancel_token(mut self, token: CancelToken) -> Self {
        self.cancel_token = Some(token);
        self
    }

    /// Set the cancel token after creation.
    pub fn set_cancel_token(&mut self, token: CancelToken) {
        self.cancel_token = Some(token);
    }

    /// Check whether a cancellation has been requested.
    /// Returns `QueryError::execution` if cancelled.
    pub fn check_cancel(&self) -> Result<(), QueryError> {
        if let Some(ref token) = self.cancel_token {
            if token.is_cancelled() {
                return Err(QueryError::execution(
                    "Query cancelled during traversal".to_string(),
                ));
            }
        }
        Ok(())
    }

    pub fn reset(&mut self) {
        self.frontier.clear();
        self.visited.clear();
        self.results.clear();
        self.exhausted = false;
        self.total_emitted = 0;
        self.stats = TraversalStats::default();
    }

    fn check_limit(&self) -> bool {
        self.total_emitted >= self.config.limit
    }

    fn should_emit(&self, depth: u32) -> bool {
        depth >= self.config.min_depth && depth <= self.config.max_depth
    }

    fn should_visit(&self, vertex_id: &VertexId) -> bool {
        match self.config.visited_policy {
            VisitedPolicy::None => true,
            VisitedPolicy::PerSeed | VisitedPolicy::Global => !self.visited.contains(vertex_id),
        }
    }

    pub fn seed_from_vertex(&mut self, vertex: Vertex) {
        let vid = *vertex.vid();

        if self.config.visited_policy != VisitedPolicy::None {
            self.visited.insert(vid);
        }

        self.frontier.push_back(TraversalItem {
            vertex_id: vid,
            vertex,
            depth: 0,
            edge: None,
            path_vertices: HashSet::from([vid]),
            path_edges: HashSet::new(),
        });
    }

    fn expand_frontier(&mut self) -> Result<(), QueryError> {
        if matches!(
            self.config.path_semantic,
            Some(PathSemantic::WeightedShortest(_))
        ) {
            return self.expand_weighted();
        }
        while let Some(item) = match self.config.order {
            TraversalOrder::Bfs => self.frontier.pop_front(),
            TraversalOrder::Dfs => self.frontier.pop_back(),
        } {
            // Check cancel at each frontier item boundary
            self.check_cancel()?;

            if self.exhausted || self.check_limit() {
                self.exhausted = true;
                return Ok(());
            }

            if item.depth >= self.config.max_depth {
                continue;
            }

            let edges = self.reader.get_edges(
                &self.config.space_name,
                &item.vertex_id,
                self.config.direction,
            );
            self.stats.record_edge_scan(edges.len());

            let filtered = self.reader.filter_edges(&edges, &self.config.edge_types);

            for edge in filtered {
                self.check_cancel()?;

                if self.check_limit() {
                    self.exhausted = true;
                    return Ok(());
                }

                let neighbor_id =
                    self.reader
                        .get_neighbor_id(edge, &item.vertex_id, self.config.direction);

                let edge_key = (
                    *edge.src(),
                    *edge.dst(),
                    edge.edge_type().to_string(),
                    edge.ranking(),
                );
                let parent_vertices = &item.path_vertices;
                let parent_edges = &item.path_edges;
                // Per-path repeat rules: Trail forbids revisiting a vertex
                // within the same path, Acyclic forbids reusing an edge
                // within the same path. Walk and Shortest impose no
                // per-path check (Shortest relies on BFS global dedup).
                let path_rejected = is_path_rejected(
                    self.config.path_semantic.clone(),
                    parent_vertices,
                    parent_edges,
                    &neighbor_id,
                    &edge_key,
                );
                if path_rejected || !self.should_visit(&neighbor_id) {
                    continue;
                }

                if self.config.visited_policy != VisitedPolicy::None {
                    self.visited.insert(neighbor_id);
                }

                if let Some(vertex) = self.reader.get_neighbor_vertex(
                    &self.config.space_name,
                    edge,
                    &neighbor_id,
                    &self.config.vertex_tag,
                ) {
                    self.stats.record_vertex_visit();
                    let new_depth = item.depth + 1;
                    self.stats.update_depth(new_depth);

                    if self.should_emit(new_depth) {
                        self.results.push_back(TraversalEvent {
                            vertex: vertex.clone(),
                            depth: new_depth,
                            edge: Some(edge.clone()),
                        });
                        self.total_emitted += 1;
                        self.stats.record_path_emitted();
                    }

                    let mut path_vertices = item.path_vertices.clone();
                    path_vertices.insert(neighbor_id);
                    let mut path_edges = item.path_edges.clone();
                    path_edges.insert(edge_key);
                    self.frontier.push_back(TraversalItem {
                        vertex_id: neighbor_id,
                        vertex,
                        depth: new_depth,
                        edge: Some(edge.clone()),
                        path_vertices,
                        path_edges,
                    });
                }
            }

            self.stats.update_frontier(self.frontier.len());
        }

        self.exhausted = true;
        Ok(())
    }

    /// Weighted-shortest expansion (Dijkstra).
    ///
    /// Runs from each seeded vertex, finalizing the cheapest path to
    /// every reachable vertex within `[min_depth, max_depth]` hops. The edge
    /// weight is read from the property named by the semantic; a missing or
    /// non-numeric property is treated as weight `1.0`, and negative weights
    /// are clamped to `0.0` so Dijkstra's non-negative assumption holds.
    fn expand_weighted(&mut self) -> Result<(), QueryError> {
        let weight_prop = match &self.config.path_semantic {
            Some(PathSemantic::WeightedShortest(p)) => p.clone(),
            _ => String::new(),
        };

        // Drain every seeded source. Callers normally seed exactly one
        // vertex per runtime, but draining avoids silently dropping
        // additional seeds when several are queued.
        let seeds: Vec<TraversalItem> = std::mem::take(&mut self.frontier).into_iter().collect();
        if seeds.is_empty() {
            self.exhausted = true;
            return Ok(());
        }

        for seed in seeds {
            self.check_cancel()?;
            if self.exhausted || self.check_limit() {
                self.exhausted = true;
                return Ok(());
            }
            // Seeds are pre-marked in `visited` by `seed_from_vertex`;
            // allow the source itself to be processed so its neighborhood
            // is expanded instead of being skipped as already finalized.
            let seed_id = seed.vertex_id;
            self.visited.remove(&seed_id);

            let mut dist: std::collections::HashMap<VertexId, f64> =
                std::collections::HashMap::new();
            dist.insert(seed_id, 0.0);
            let mut heap: BinaryHeap<WeightedItem> = BinaryHeap::new();
            heap.push(WeightedItem {
                cost: 0.0,
                depth: 0,
                vertex: seed.vertex,
                vertex_id: seed_id,
                edge: None,
            });

            while let Some(top) = heap.pop() {
                self.check_cancel()?;
                if self.exhausted || self.check_limit() {
                    self.exhausted = true;
                    return Ok(());
                }
                // Skip stale heap entries superseded by a cheaper finalized cost.
                if self.visited.contains(&top.vertex_id) {
                    continue;
                }
                self.visited.insert(top.vertex_id);

                if self.should_emit(top.depth) {
                    self.results.push_back(TraversalEvent {
                        vertex: top.vertex.clone(),
                        depth: top.depth,
                        edge: top.edge.clone(),
                    });
                    self.total_emitted += 1;
                    self.stats.record_path_emitted();
                }

                if top.depth >= self.config.max_depth {
                    continue;
                }

                let edges = self.reader.get_edges(
                    &self.config.space_name,
                    &top.vertex_id,
                    self.config.direction,
                );
                self.stats.record_edge_scan(edges.len());
                let filtered = self.reader.filter_edges(&edges, &self.config.edge_types);

                for edge in filtered {
                    self.check_cancel()?;
                    let neighbor_id =
                        self.reader
                            .get_neighbor_id(edge, &top.vertex_id, self.config.direction);
                    let weight = edge_weight(edge, &weight_prop);
                    let next_cost = top.cost + weight;
                    if self.visited.contains(&neighbor_id) {
                        continue;
                    }
                    let better = match dist.get(&neighbor_id) {
                        Some(old) => next_cost < *old,
                        None => true,
                    };
                    if !better {
                        continue;
                    }
                    dist.insert(neighbor_id, next_cost);
                    if let Some(vertex) = self.reader.get_neighbor_vertex(
                        &self.config.space_name,
                        edge,
                        &neighbor_id,
                        &self.config.vertex_tag,
                    ) {
                        self.stats.record_vertex_visit();
                        let new_depth = top.depth + 1;
                        self.stats.update_depth(new_depth);
                        heap.push(WeightedItem {
                            cost: next_cost,
                            depth: new_depth,
                            vertex,
                            vertex_id: neighbor_id,
                            edge: Some(edge.clone()),
                        });
                    }
                }
                self.stats.update_frontier(heap.len());
            }
        }

        self.exhausted = true;
        Ok(())
    }

    pub fn next_event(&mut self) -> Option<TraversalEvent> {
        if let Some(event) = self.results.pop_front() {
            return Some(event);
        }

        if self.exhausted {
            return None;
        }

        if self.expand_frontier().is_err() {
            self.exhausted = true;
            return None;
        }

        self.results.pop_front()
    }

    pub fn stats(&self) -> &TraversalStats {
        &self.stats
    }
}

/// Heap entry for the weighted (Dijkstra) traversal. Ordered by ascending
/// cost; the `Ord` impl inverts cost so `BinaryHeap` (a max-heap) pops the
/// cheapest entry first.
#[derive(Debug, Clone)]
struct WeightedItem {
    cost: f64,
    depth: u32,
    vertex: Vertex,
    vertex_id: VertexId,
    edge: Option<Edge>,
}

impl PartialEq for WeightedItem {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost
    }
}

impl Eq for WeightedItem {}

impl PartialOrd for WeightedItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for WeightedItem {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .partial_cmp(&self.cost)
            .unwrap_or(Ordering::Equal)
    }
}

/// Read a numeric edge weight from `prop`, defaulting to `1.0` when the
/// property is absent or non-numeric, and clamping negatives to `0.0`.
fn edge_weight(edge: &Edge, prop: &str) -> f64 {
    let raw = match edge.get_property(prop) {
        Some(v) => v,
        None => return 1.0,
    };
    let value = match raw {
        graphdb_core::Value::Float(v) => f64::from(*v),
        graphdb_core::Value::Double(v) => *v,
        graphdb_core::Value::Int(v) => f64::from(*v),
        graphdb_core::Value::BigInt(v) => *v as f64,
        graphdb_core::Value::SmallInt(v) => f64::from(*v),
        _ => return 1.0,
    };
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

fn is_path_rejected(
    semantic: Option<PathSemantic>,
    parent_vertices: &HashSet<VertexId>,
    parent_edges: &HashSet<EdgeKey>,
    neighbor: &VertexId,
    edge_key: &EdgeKey,
) -> bool {
    match semantic {
        Some(PathSemantic::Trail) => parent_vertices.contains(neighbor),
        Some(PathSemantic::Acyclic) => parent_edges.contains(edge_key),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vid(n: i64) -> VertexId {
        VertexId::try_from_int64(n).expect("valid vertex id")
    }

    fn edge_key(src: i64, dst: i64) -> EdgeKey {
        (vid(src), vid(dst), "KNOWS".to_string(), 0)
    }

    #[test]
    fn trail_rejects_repeated_vertex() {
        let parents: HashSet<VertexId> = HashSet::from([vid(1), vid(2)]);
        let edges: HashSet<EdgeKey> = HashSet::new();
        assert!(is_path_rejected(
            Some(PathSemantic::Trail),
            &parents,
            &edges,
            &vid(2),
            &edge_key(2, 3)
        ));
        assert!(!is_path_rejected(
            Some(PathSemantic::Trail),
            &parents,
            &edges,
            &vid(3),
            &edge_key(2, 3)
        ));
    }

    #[test]
    fn acyclic_rejects_repeated_edge_only() {
        let parents: HashSet<VertexId> = HashSet::from([vid(1), vid(2)]);
        let edges: HashSet<EdgeKey> = HashSet::from([edge_key(1, 2)]);
        // Same edge reused is rejected even though the neighbor is new.
        assert!(is_path_rejected(
            Some(PathSemantic::Acyclic),
            &parents,
            &edges,
            &vid(3),
            &edge_key(1, 2)
        ));
        // Revisiting a vertex via a fresh edge is allowed under Acyclic.
        assert!(!is_path_rejected(
            Some(PathSemantic::Acyclic),
            &parents,
            &edges,
            &vid(1),
            &edge_key(2, 1)
        ));
        // Trail would reject that vertex revisit.
        assert!(is_path_rejected(
            Some(PathSemantic::Trail),
            &parents,
            &edges,
            &vid(1),
            &edge_key(2, 1)
        ));
    }

    #[test]
    fn walk_and_shortest_impose_no_per_path_check() {
        let parents: HashSet<VertexId> = HashSet::from([vid(1)]);
        let edges: HashSet<EdgeKey> = HashSet::from([edge_key(1, 2)]);
        for semantic in [
            None,
            Some(PathSemantic::Walk),
            Some(PathSemantic::Shortest),
            Some(PathSemantic::AllShortest),
        ] {
            assert!(
                !is_path_rejected(semantic.clone(), &parents, &edges, &vid(1), &edge_key(1, 2)),
                "unexpected per-path rejection for {semantic:?}"
            );
        }
    }
}
