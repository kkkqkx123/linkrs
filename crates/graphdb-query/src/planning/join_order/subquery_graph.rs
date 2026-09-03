//! Bitset-encoded subquery graph for DP join enumeration.
//!
//! A `SubqueryGraph` selects a subset of the query graph nodes and rels with
//! two `u64` selectors. All DP operations (neighbor lookup, subgraph union,
//! connected-key extraction) work on these selectors without cloning the
//! underlying pattern.
//!
//! Variables form a bipartite adjacency: a rel touches its two endpoint
//! nodes, and a node touches its incident rels. Base scans seed the DP table
//! with singletons (one node or one rel), and larger subgraphs grow by
//! joining connected pairs, so every table entry is a connected subgraph.

use std::sync::Arc;

use super::query_graph::QueryGraph;

/// Maximum number of query variables addressable by the bitset selectors.
pub const MAX_NUM_QUERY_VARIABLES: usize = 64;

/// Bitset-encoded subgraph of a [`QueryGraph`].
#[derive(Debug, Clone)]
pub struct SubqueryGraph {
    query_graph: Arc<QueryGraph>,
    /// Bit `i` set when query node `i` is in the subgraph.
    pub query_nodes_selector: u64,
    /// Bit `i` set when query rel `i` is in the subgraph.
    pub query_rels_selector: u64,
}

impl SubqueryGraph {
    pub fn new(query_graph: Arc<QueryGraph>) -> Self {
        Self {
            query_graph,
            query_nodes_selector: 0,
            query_rels_selector: 0,
        }
    }

    pub fn with_selectors(
        query_graph: Arc<QueryGraph>,
        query_nodes_selector: u64,
        query_rels_selector: u64,
    ) -> Self {
        Self {
            query_graph,
            query_nodes_selector,
            query_rels_selector,
        }
    }

    /// Subgraph holding exactly one query node.
    pub fn single_node(query_graph: &Arc<QueryGraph>, node_pos: usize) -> Self {
        assert!(node_pos < 64, "node position out of bitset range");
        Self {
            query_graph: Arc::clone(query_graph),
            query_nodes_selector: 1u64 << node_pos,
            query_rels_selector: 0,
        }
    }

    /// Subgraph holding exactly one query rel (endpoints excluded).
    ///
    /// Rel-only singletons keep the DP lattice connected: larger subgraphs
    /// grow by attaching endpoint nodes (extend) or by hash-joining on
    /// shared nodes once both sides carry them.
    pub fn single_rel(query_graph: &Arc<QueryGraph>, rel_pos: usize) -> Self {
        assert!(rel_pos < 64, "rel position out of bitset range");
        Self {
            query_graph: Arc::clone(query_graph),
            query_nodes_selector: 0,
            query_rels_selector: 1u64 << rel_pos,
        }
    }

    pub fn query_graph(&self) -> &Arc<QueryGraph> {
        &self.query_graph
    }

    pub fn add_query_node(&mut self, node_pos: usize) {
        assert!(node_pos < 64, "node position out of bitset range");
        self.query_nodes_selector |= 1u64 << node_pos;
    }

    pub fn add_query_rel(&mut self, rel_pos: usize) {
        assert!(rel_pos < 64, "rel position out of bitset range");
        self.query_rels_selector |= 1u64 << rel_pos;
    }

    pub fn add_subquery_graph(&mut self, other: &SubqueryGraph) {
        debug_assert!(Arc::ptr_eq(&self.query_graph, &other.query_graph));
        self.query_nodes_selector |= other.query_nodes_selector;
        self.query_rels_selector |= other.query_rels_selector;
    }

    pub fn contains_node(&self, node_pos: usize) -> bool {
        node_pos < 64 && (self.query_nodes_selector >> node_pos) & 1 == 1
    }

    pub fn contains_rel(&self, rel_pos: usize) -> bool {
        rel_pos < 64 && (self.query_rels_selector >> rel_pos) & 1 == 1
    }

    pub fn is_empty(&self) -> bool {
        self.query_nodes_selector == 0 && self.query_rels_selector == 0
    }

    /// Number of selected nodes plus selected rels.
    pub fn total_num_variables(&self) -> usize {
        self.query_nodes_selector.count_ones() as usize
            + self.query_rels_selector.count_ones() as usize
    }

    pub fn num_nodes(&self) -> usize {
        self.query_nodes_selector.count_ones() as usize
    }

    pub fn num_rels(&self) -> usize {
        self.query_rels_selector.count_ones() as usize
    }

    pub fn node_positions(&self) -> Vec<usize> {
        (0..self.query_graph.num_nodes())
            .filter(|p| self.contains_node(*p))
            .collect()
    }

    pub fn rel_positions(&self) -> Vec<usize> {
        (0..self.query_graph.num_rels())
            .filter(|p| self.contains_rel(*p))
            .collect()
    }

    /// Endpoint node positions of every rel contained in this subgraph.
    fn contained_endpoint_positions(&self) -> Vec<usize> {
        let mut out = Vec::new();
        for rel_pos in self.rel_positions() {
            if let Some((src, dst)) = self.query_graph.rel_endpoint_positions(rel_pos) {
                if !out.contains(&src) {
                    out.push(src);
                }
                if !out.contains(&dst) {
                    out.push(dst);
                }
            }
        }
        out
    }

    /// Node positions adjacent to this subgraph: endpoints of contained rels
    /// plus endpoints of frontier rels, excluding nodes already inside.
    pub fn get_node_neighbor_positions(&self) -> Vec<usize> {
        let mut out = Vec::new();
        for node_pos in self
            .contained_endpoint_positions()
            .into_iter()
            .chain(self.frontier_endpoint_positions())
        {
            if !self.contains_node(node_pos) && !out.contains(&node_pos) {
                out.push(node_pos);
            }
        }
        out.sort_unstable();
        out
    }

    /// Endpoints of frontier rels (rels touching this subgraph from outside).
    fn frontier_endpoint_positions(&self) -> Vec<usize> {
        let mut out = Vec::new();
        for rel_pos in self.get_rel_neighbor_positions() {
            if let Some((src, dst)) = self.query_graph.rel_endpoint_positions(rel_pos) {
                if !self.contains_node(src) && !out.contains(&src) {
                    out.push(src);
                }
                if !self.contains_node(dst) && !out.contains(&dst) {
                    out.push(dst);
                }
            }
        }
        out
    }

    /// Rel positions adjacent to this subgraph: outside rels sharing at
    /// least one endpoint node with the subgraph (directly or through a
    /// contained rel).
    pub fn get_rel_neighbor_positions(&self) -> Vec<usize> {
        let mut out = Vec::new();
        for rel_pos in 0..self.query_graph.num_rels() {
            if self.contains_rel(rel_pos) {
                continue;
            }
            if self.touches_subgraph(rel_pos) {
                out.push(rel_pos);
            }
        }
        out.sort_unstable();
        out
    }

    fn touches_subgraph(&self, rel_pos: usize) -> bool {
        let Some((src, dst)) = self.query_graph.rel_endpoint_positions(rel_pos) else {
            return false;
        };
        if self.contains_node(src) || self.contains_node(dst) {
            return true;
        }
        let contained = self.contained_endpoint_positions();
        contained.contains(&src) || contained.contains(&dst)
    }

    /// Enumerate connected neighbor subgraphs of exactly `target_size`
    /// fresh variables (disjoint from this subgraph) that touch it.
    ///
    /// Seeds grow one variable at a time from the bipartite frontier of
    /// (self union seed): outside rels touching the combined node set and
    /// outside endpoint nodes of combined rels.
    pub fn get_neighbor_subgraphs(&self, target_size: usize) -> Vec<SubqueryGraph> {
        if target_size == 0 || target_size > MAX_NUM_QUERY_VARIABLES {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut seeds = self.frontier_singletons();
        while let Some(seed) = seeds.pop() {
            let size = seed.total_num_variables();
            if size == target_size {
                if !out.iter().any(|s: &SubqueryGraph| s.key() == seed.key()) {
                    out.push(seed);
                }
            } else if size < target_size {
                seeds.extend(self.grown_seeds(&seed));
            }
        }
        out
    }

    /// Single-variable frontier subgraphs disjoint from this subgraph.
    fn frontier_singletons(&self) -> Vec<SubqueryGraph> {
        let mut out = Vec::new();
        for rel_pos in self.get_rel_neighbor_positions() {
            let mut seed = SubqueryGraph::new(Arc::clone(&self.query_graph));
            seed.add_query_rel(rel_pos);
            out.push(seed);
        }
        for node_pos in self.get_node_neighbor_positions() {
            let mut seed = SubqueryGraph::new(Arc::clone(&self.query_graph));
            seed.add_query_node(node_pos);
            if !out.iter().any(|s: &SubqueryGraph| s.key() == seed.key()) {
                out.push(seed);
            }
        }
        out
    }

    /// One-variable extensions of `seed` from the frontier of
    /// (self union seed), excluding variables already in either side.
    fn grown_seeds(&self, seed: &SubqueryGraph) -> Vec<SubqueryGraph> {
        let mut combined = self.clone();
        combined.add_subquery_graph(seed);
        let mut out = Vec::new();
        for rel_pos in combined.get_rel_neighbor_positions() {
            if seed.contains_rel(rel_pos) || self.contains_rel(rel_pos) {
                continue;
            }
            let mut next = seed.clone();
            next.add_query_rel(rel_pos);
            out.push(next);
        }
        for node_pos in combined.get_node_neighbor_positions() {
            if seed.contains_node(node_pos) || self.contains_node(node_pos) {
                continue;
            }
            let mut next = seed.clone();
            next.add_query_node(node_pos);
            if !out.iter().any(|s: &SubqueryGraph| s.key() == next.key()) {
                out.push(next);
            }
        }
        out
    }

    /// Node positions present in both subgraphs (binary join keys).
    pub fn get_connected_node_positions(&self, other: &SubqueryGraph) -> Vec<usize> {
        let mut out = Vec::new();
        for pos in 0..self.query_graph.num_nodes() {
            if self.contains_node(pos) && other.contains_node(pos) {
                out.push(pos);
            }
        }
        out.sort_unstable();
        out
    }

    /// Whether the two subgraphs touch through at least one shared node.
    pub fn is_connected(&self, other: &SubqueryGraph) -> bool {
        !self.get_connected_node_positions(other).is_empty()
    }

    /// Whether a rel on either side has an endpoint node on the other side.
    ///
    /// Extendable pairs without shared nodes are planned as index-nested-loop
    /// extends rather than hash joins.
    pub fn is_extendable_with(&self, other: &SubqueryGraph) -> bool {
        for rel_pos in self.rel_positions() {
            if let Some((src, dst)) = self.query_graph.rel_endpoint_positions(rel_pos) {
                if other.contains_node(src) || other.contains_node(dst) {
                    return true;
                }
            }
        }
        for rel_pos in other.rel_positions() {
            if let Some((src, dst)) = self.query_graph.rel_endpoint_positions(rel_pos) {
                if self.contains_node(src) || self.contains_node(dst) {
                    return true;
                }
            }
        }
        false
    }

    /// Whether the pair can be joined at all: shared join keys or an
    /// extend step in either direction.
    pub fn is_joinable_with(&self, other: &SubqueryGraph) -> bool {
        self.is_connected(other) || self.is_extendable_with(other)
    }

    /// Selector key used by the DP table.
    pub fn key(&self) -> (u64, u64) {
        (self.query_nodes_selector, self.query_rels_selector)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::join_order::query_graph::{QueryNode, QueryRel};

    fn triangle() -> Arc<QueryGraph> {
        let mut qg = QueryGraph::new();
        qg.add_node(QueryNode::new("a", "a"));
        qg.add_node(QueryNode::new("b", "b"));
        qg.add_node(QueryNode::new("c", "c"));
        qg.add_rel(QueryRel::new("e1", "e1", "a", "b"));
        qg.add_rel(QueryRel::new("e2", "e2", "a", "c"));
        qg.add_rel(QueryRel::new("e3", "e3", "b", "c"));
        Arc::new(qg)
    }

    #[test]
    fn add_and_count() {
        let qg = triangle();
        let mut sg = SubqueryGraph::new(Arc::clone(&qg));
        assert!(sg.is_empty());
        sg.add_query_node(0);
        sg.add_query_rel(0);
        assert_eq!(sg.total_num_variables(), 2);
        assert!(sg.contains_node(0));
        assert!(!sg.contains_node(1));
    }

    #[test]
    fn single_rel_is_rel_only() {
        let qg = triangle();
        let sg = SubqueryGraph::single_rel(&qg, 0);
        assert!(sg.contains_rel(0));
        assert_eq!(sg.total_num_variables(), 1);
        assert_eq!(sg.num_nodes(), 0);
    }

    #[test]
    fn rel_only_neighbors_are_endpoints() {
        let qg = triangle();
        let sg = SubqueryGraph::single_rel(&qg, 0);
        // Direct endpoints plus nodes behind frontier rels.
        assert_eq!(sg.get_node_neighbor_positions(), vec![0, 1, 2]);
        let mut rel_nbrs = sg.get_rel_neighbor_positions();
        rel_nbrs.sort_unstable();
        assert_eq!(rel_nbrs, vec![1, 2]);
    }

    #[test]
    fn rel_neighbors() {
        let qg = triangle();
        let mut sg = SubqueryGraph::new(Arc::clone(&qg));
        sg.add_query_node(0);
        sg.add_query_node(1);
        sg.add_query_rel(0);
        let mut nbrs = sg.get_rel_neighbor_positions();
        nbrs.sort_unstable();
        assert_eq!(nbrs, vec![1, 2]);
    }

    #[test]
    fn connected_nodes_are_join_keys() {
        let qg = triangle();
        let mut left = SubqueryGraph::single_rel(&qg, 0);
        left.add_query_node(0);
        let mut right = SubqueryGraph::single_rel(&qg, 1);
        right.add_query_node(0);
        assert_eq!(left.get_connected_node_positions(&right), vec![0]);
        assert!(left.is_connected(&right));
    }

    #[test]
    fn rel_node_pair_is_extendable_but_not_connected() {
        let qg = triangle();
        let rel = SubqueryGraph::single_rel(&qg, 0);
        let node = SubqueryGraph::single_node(&qg, 0);
        assert!(!rel.is_connected(&node));
        assert!(rel.is_extendable_with(&node));
        assert!(rel.is_joinable_with(&node));
        let far = SubqueryGraph::single_node(&qg, 2);
        assert!(!rel.is_joinable_with(&far));
    }

    #[test]
    fn neighbor_subgraphs_size_one() {
        let qg = triangle();
        let left = SubqueryGraph::single_rel(&qg, 0);
        let nbrs = left.get_neighbor_subgraphs(1);
        assert!(!nbrs.is_empty());
        for n in &nbrs {
            assert_eq!(n.total_num_variables(), 1);
        }
        // Endpoints a and b must be reachable as size-1 neighbors.
        assert!(nbrs.iter().any(|n| n.contains_node(0)));
        assert!(nbrs.iter().any(|n| n.contains_node(1)));
    }

    #[test]
    fn neighbor_subgraphs_grow_to_target_size() {
        let qg = triangle();
        let mut sg = SubqueryGraph::new(Arc::clone(&qg));
        sg.add_query_node(0);
        let nbrs = sg.get_neighbor_subgraphs(2);
        assert!(!nbrs.is_empty());
        for n in &nbrs {
            assert_eq!(n.total_num_variables(), 2);
        }
    }
}
