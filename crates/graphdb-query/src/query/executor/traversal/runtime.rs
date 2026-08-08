use std::collections::{HashSet, VecDeque};

use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::{Edge, Vertex};
use crate::query::executor::streaming::query_registry::CancelToken;
use crate::query::executor::traversal::config::{TraversalConfig, TraversalOrder, VisitedPolicy};
use crate::query::executor::traversal::graph_reader::TraversalGraphReader;
use crate::query::executor::traversal::stats::TraversalStats;

#[derive(Debug, Clone)]
pub struct TraversalItem {
    pub vertex_id: VertexId,
    pub vertex: Vertex,
    pub depth: u32,
    pub edge: Option<Edge>,
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
        });
    }

    fn expand_frontier(&mut self) -> Result<(), QueryError> {
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

                if !self.should_visit(&neighbor_id) {
                    continue;
                }

                if self.config.visited_policy != VisitedPolicy::None {
                    self.visited.insert(neighbor_id);
                }

                if let Some(vertex) = self
                    .reader
                    .get_vertex(&self.config.space_name, &neighbor_id)
                {
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

                    self.frontier.push_back(TraversalItem {
                        vertex_id: neighbor_id,
                        vertex,
                        depth: new_depth,
                        edge: Some(edge.clone()),
                    });
                }
            }

            self.stats.update_frontier(self.frontier.len());
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
