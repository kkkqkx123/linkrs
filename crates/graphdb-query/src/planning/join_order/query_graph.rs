//! Query graph representation for join ordering.
//!
//! A `QueryGraph` captures the pattern part of a MATCH query as a set of
//! query nodes and query rels with positional indexes. The join order
//! enumerator works on this graph instead of the AST so that DP enumeration
//! and WCO detection share one structural view.

use std::collections::HashMap;
use std::sync::Arc;

/// Traversal direction of a query rel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ExtendDirection {
    /// Outgoing: src -> dst.
    #[default]
    Out,
    /// Incoming: src <- dst.
    In,
    /// Bidirectional.
    Both,
}

impl ExtendDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExtendDirection::Out => "OUT",
            ExtendDirection::In => "IN",
            ExtendDirection::Both => "BOTH",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_uppercase().as_str() {
            "IN" => ExtendDirection::In,
            "BOTH" | "BIDIRECT" | "UNDIRECTED" => ExtendDirection::Both,
            _ => ExtendDirection::Out,
        }
    }
}

impl From<graphdb_core::types::graph_schema::EdgeDirection> for ExtendDirection {
    fn from(d: graphdb_core::types::graph_schema::EdgeDirection) -> Self {
        use graphdb_core::types::graph_schema::EdgeDirection as Core;
        match d {
            Core::Out => ExtendDirection::Out,
            Core::In => ExtendDirection::In,
            Core::Both => ExtendDirection::Both,
        }
    }
}

/// A single query node (pattern variable bound to vertices).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryNode {
    /// Positional name used for graph wiring (usually the variable).
    pub name: String,
    /// Query variable, e.g. `a` in `(a)`.
    pub variable: String,
    /// Label predicates on the node.
    pub labels: Vec<String>,
}

impl QueryNode {
    pub fn new(name: impl Into<String>, variable: impl Into<String>) -> Self {
        let variable = variable.into();
        let name = name.into();
        Self {
            name,
            variable,
            labels: Vec::new(),
        }
    }

    pub fn with_labels(mut self, labels: Vec<String>) -> Self {
        self.labels = labels;
        self
    }
}

/// A single query rel (pattern variable bound to edges).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRel {
    /// Positional name used for graph wiring (usually the variable).
    pub name: String,
    /// Query variable, e.g. `e` in `-[e]->`.
    pub variable: String,
    /// Edge type predicates.
    pub edge_types: Vec<String>,
    /// Source node name.
    pub src_node_name: String,
    /// Destination node name.
    pub dst_node_name: String,
    /// Traversal direction.
    pub direction: ExtendDirection,
}

impl QueryRel {
    pub fn new(
        name: impl Into<String>,
        variable: impl Into<String>,
        src_node_name: impl Into<String>,
        dst_node_name: impl Into<String>,
    ) -> Self {
        let variable = variable.into();
        Self {
            name: name.into(),
            variable,
            edge_types: Vec::new(),
            src_node_name: src_node_name.into(),
            dst_node_name: dst_node_name.into(),
            direction: ExtendDirection::Out,
        }
    }

    pub fn with_direction(mut self, direction: ExtendDirection) -> Self {
        self.direction = direction;
        self
    }

    pub fn with_edge_types(mut self, edge_types: Vec<String>) -> Self {
        self.edge_types = edge_types;
        self
    }

    /// The other endpoint of this rel given one endpoint name.
    pub fn other_node(&self, node_name: &str) -> Option<&str> {
        if self.src_node_name == node_name {
            Some(self.dst_node_name.as_str())
        } else if self.dst_node_name == node_name {
            Some(self.src_node_name.as_str())
        } else {
            None
        }
    }

    /// Whether this rel touches the given node name.
    pub fn touches(&self, node_name: &str) -> bool {
        self.src_node_name == node_name || self.dst_node_name == node_name
    }
}

/// Pattern graph used by the join order enumerator.
#[derive(Debug, Clone, Default)]
pub struct QueryGraph {
    /// All query nodes in positional order.
    pub query_nodes: Vec<Arc<QueryNode>>,
    /// All query rels in positional order.
    pub query_rels: Vec<Arc<QueryRel>>,
    /// Node name -> position index.
    pub node_name_to_pos: HashMap<String, usize>,
    /// Rel name -> position index.
    pub rel_name_to_pos: HashMap<String, usize>,
}

impl QueryGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a query node and return its position.
    pub fn add_node(&mut self, node: QueryNode) -> usize {
        let pos = self.query_nodes.len();
        self.node_name_to_pos.insert(node.name.clone(), pos);
        self.query_nodes.push(Arc::new(node));
        pos
    }

    /// Add a query rel and return its position.
    pub fn add_rel(&mut self, rel: QueryRel) -> usize {
        let pos = self.query_rels.len();
        self.rel_name_to_pos.insert(rel.name.clone(), pos);
        self.query_rels.push(Arc::new(rel));
        pos
    }

    pub fn num_nodes(&self) -> usize {
        self.query_nodes.len()
    }

    pub fn num_rels(&self) -> usize {
        self.query_rels.len()
    }

    /// Total pattern variables (nodes + rels), capped by the bitset width.
    pub fn num_variables(&self) -> usize {
        self.num_nodes() + self.num_rels()
    }

    pub fn node_pos(&self, name: &str) -> Option<usize> {
        self.node_name_to_pos.get(name).copied()
    }

    pub fn rel_pos(&self, name: &str) -> Option<usize> {
        self.rel_name_to_pos.get(name).copied()
    }

    /// Positions of rels incident to the given node position.
    pub fn incident_rel_positions(&self, node_pos: usize) -> Vec<usize> {
        let Some(node) = self.query_nodes.get(node_pos) else {
            return Vec::new();
        };
        self.query_rels
            .iter()
            .enumerate()
            .filter(|(_, r)| r.touches(&node.name))
            .map(|(i, _)| i)
            .collect()
    }

    /// Node positions connected to the given rel position.
    pub fn rel_endpoint_positions(&self, rel_pos: usize) -> Option<(usize, usize)> {
        let rel = self.query_rels.get(rel_pos)?;
        let src = self.node_name_to_pos.get(&rel.src_node_name).copied()?;
        let dst = self.node_name_to_pos.get(&rel.dst_node_name).copied()?;
        Some((src, dst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle_graph() -> QueryGraph {
        let mut qg = QueryGraph::new();
        qg.add_node(QueryNode::new("a", "a"));
        qg.add_node(QueryNode::new("b", "b"));
        qg.add_node(QueryNode::new("c", "c"));
        qg.add_rel(QueryRel::new("e1", "e1", "a", "b"));
        qg.add_rel(QueryRel::new("e2", "e2", "a", "c"));
        qg.add_rel(QueryRel::new("e3", "e3", "b", "c"));
        qg
    }

    #[test]
    fn node_and_rel_positions() {
        let qg = triangle_graph();
        assert_eq!(qg.num_nodes(), 3);
        assert_eq!(qg.num_rels(), 3);
        assert_eq!(qg.node_pos("a"), Some(0));
        assert_eq!(qg.rel_pos("e3"), Some(2));
        assert_eq!(qg.num_variables(), 6);
    }

    #[test]
    fn incident_rels() {
        let qg = triangle_graph();
        let incident = qg.incident_rel_positions(0);
        assert_eq!(incident.len(), 2);
        assert!(incident.contains(&0));
        assert!(incident.contains(&1));
    }

    #[test]
    fn rel_endpoints() {
        let qg = triangle_graph();
        assert_eq!(qg.rel_endpoint_positions(0), Some((0, 1)));
        assert_eq!(qg.rel_endpoint_positions(2), Some((1, 2)));
        assert_eq!(qg.rel_endpoint_positions(99), None);
    }

    #[test]
    fn direction_roundtrip() {
        assert_eq!(ExtendDirection::from_str("in"), ExtendDirection::In);
        assert_eq!(ExtendDirection::from_str("both"), ExtendDirection::Both);
        assert_eq!(ExtendDirection::Out.as_str(), "OUT");
    }
}
