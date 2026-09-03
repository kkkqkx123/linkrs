//! MATCH pattern to [`QueryGraph`](super::query_graph::QueryGraph) conversion.
//!
//! This is the seam between Cypher planning and join-order enumeration:
//! conjunctive single-hop patterns become a query graph for
//! [`JoinOrderEnumerator`](super::plan_join_order::JoinOrderEnumerator).
//! Anything the graph cannot represent (optional branches, alternatives,
//! variable-length edges, standalone variables) yields `None` and the
//! caller falls back to the legacy `ExpandAll` chain.

use super::query_graph::{ExtendDirection, QueryGraph, QueryNode, QueryRel};
use crate::parser::ast::pattern::{EdgePattern, NodePattern, PathElement, Pattern};

/// Convert MATCH patterns into a query graph.
///
/// Supports `Path` patterns of alternating `Node`/`Edge` elements with
/// single-hop edges, plus top-level `Node` patterns. Anonymous pattern
/// variables are synthesized as `__anon_n{i}` / `__anon_e{i}`, matching the
/// binder convention. Repeated variable names across patterns reuse the
/// first node entry (labels merged); a duplicated rel variable is
/// unsupported and yields `None`.
pub fn query_graph_from_match_patterns(patterns: &[Pattern]) -> Option<QueryGraph> {
    let mut builder = QueryGraphBuilder::default();
    for pattern in patterns {
        match pattern {
            Pattern::Path(path) => {
                if !builder.push_path(&path.elements) {
                    return None;
                }
            }
            Pattern::Node(node) => {
                builder.push_node(node);
            }
            Pattern::Edge(_) | Pattern::Variable(_) => return None,
        }
    }
    let graph = builder.finish()?;
    if graph.num_nodes() == 0 {
        return None;
    }
    Some(graph)
}

#[derive(Debug, Default)]
struct QueryGraphBuilder {
    graph: QueryGraph,
    anon_nodes: usize,
    anon_rels: usize,
}

impl QueryGraphBuilder {
    fn node_var(&mut self, node: &NodePattern) -> String {
        match &node.variable {
            Some(var) => var.clone(),
            None => {
                let var = format!("__anon_n{}", self.anon_nodes);
                self.anon_nodes += 1;
                var
            }
        }
    }

    fn rel_var(&mut self, edge: &EdgePattern) -> String {
        match &edge.variable {
            Some(var) => var.clone(),
            None => {
                let var = format!("__anon_e{}", self.anon_rels);
                self.anon_rels += 1;
                var
            }
        }
    }

    fn push_node(&mut self, node: &NodePattern) {
        let var = self.node_var(node);
        self.ensure_node(&var, &node.labels);
    }

    fn ensure_node(&mut self, var: &str, labels: &[String]) {
        if let Some(pos) = self.graph.node_pos(var) {
            let existing = self.graph.query_nodes[pos].clone();
            if existing.labels.len() < labels.len() {
                let mut merged = existing.labels.clone();
                for label in labels {
                    if !merged.contains(label) {
                        merged.push(label.clone());
                    }
                }
                self.graph.query_nodes[pos] = std::sync::Arc::new(
                    QueryNode::new(&existing.name, &existing.variable).with_labels(merged),
                );
            }
            return;
        }
        self.graph
            .add_node(QueryNode::new(var, var).with_labels(labels.to_vec()));
    }

    /// Push one path's elements. Expects `Node (Edge Node)*`.
    fn push_path(&mut self, elements: &[PathElement]) -> bool {
        if elements.is_empty() {
            return false;
        }
        let mut iter = elements.iter().peekable();
        let mut prev_node: Option<String> = None;
        let mut expect_node = true;
        // Pending edge between two nodes: (var, types, direction).
        let mut pending: Option<(String, Vec<String>, ExtendDirection)> = None;

        while let Some(element) = iter.next() {
            match element {
                PathElement::Node(node) => {
                    if !expect_node {
                        return false;
                    }
                    let var = self.node_var(node);
                    self.ensure_node(&var, &node.labels);
                    if let Some((rel_var, edge_types, direction)) = pending.take() {
                        let prev = prev_node.clone().unwrap_or_else(|| var.clone());
                        if self.graph.rel_pos(&rel_var).is_some() {
                            return false;
                        }
                        self.graph.add_rel(
                            QueryRel::new(&rel_var, &rel_var, &prev, &var)
                                .with_direction(direction)
                                .with_edge_types(edge_types),
                        );
                    }
                    prev_node = Some(var);
                    expect_node = false;
                }
                PathElement::Edge(edge) => {
                    if expect_node || pending.is_some() {
                        return false;
                    }
                    // Variable-length edges need loop expansion, not joins.
                    if edge.range.is_some() {
                        return false;
                    }
                    if prev_node.is_none() {
                        return false;
                    }
                    let var = self.rel_var(edge);
                    pending = Some((
                        var,
                        edge.edge_types.clone(),
                        ExtendDirection::from(edge.direction),
                    ));
                    expect_node = true;
                }
                PathElement::Alternative(_)
                | PathElement::Optional(_)
                | PathElement::Repeated(_, _) => return false,
            }
        }
        // A trailing edge without its endpoint is not plannable here.
        if pending.is_some() || expect_node {
            return false;
        }
        true
    }

    fn finish(self) -> Option<QueryGraph> {
        // Rel endpoints must all resolve to known nodes.
        for rel in &self.graph.query_rels {
            if self.graph.node_pos(&rel.src_node_name).is_none()
                || self.graph.node_pos(&rel.dst_node_name).is_none()
            {
                return None;
            }
        }
        Some(self.graph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ast::pattern::{EdgeRange, PathPattern};
    use graphdb_core::types::graph_schema::EdgeDirection;
    use graphdb_core::types::Span;

    fn node(var: &str) -> PathElement {
        PathElement::Node(NodePattern::new(
            Some(var.to_string()),
            Vec::new(),
            None,
            Vec::new(),
            Span::default(),
        ))
    }

    fn labeled_node(var: &str, label: &str) -> PathElement {
        PathElement::Node(NodePattern::new(
            Some(var.to_string()),
            vec![label.to_string()],
            None,
            Vec::new(),
            Span::default(),
        ))
    }

    fn edge(var: &str) -> PathElement {
        PathElement::Edge(EdgePattern::new(
            Some(var.to_string()),
            Vec::new(),
            None,
            Vec::new(),
            EdgeDirection::Out,
            None,
            Span::default(),
        ))
    }

    fn path(elements: Vec<PathElement>) -> Pattern {
        Pattern::Path(PathPattern::new(elements, Span::default()))
    }

    #[test]
    fn triangle_path_converts() {
        let patterns = vec![path(vec![
            node("a"),
            edge("e1"),
            node("b"),
            edge("e3"),
            node("c"),
            edge("e2"),
            node("a"),
        ])];
        // Note: linear paths cannot close a triangle (that needs two
        // patterns); this checks the linear conversion shape instead.
        let graph = query_graph_from_match_patterns(&patterns).expect("graph");
        assert_eq!(graph.num_nodes(), 3);
        assert_eq!(graph.num_rels(), 3);
        assert_eq!(graph.rel_endpoint_positions(0), Some((0, 1)));
    }

    #[test]
    fn comma_patterns_share_nodes() {
        let patterns = vec![
            path(vec![labeled_node("a", "person"), edge("e1"), node("b")]),
            path(vec![node("a"), edge("e2"), node("c")]),
        ];
        let graph = query_graph_from_match_patterns(&patterns).expect("graph");
        assert_eq!(graph.num_nodes(), 3);
        assert_eq!(graph.num_rels(), 2);
        let a = &graph.query_nodes[graph.node_pos("a").expect("a")];
        assert_eq!(a.labels, vec!["person".to_string()]);
    }

    #[test]
    fn unsupported_shapes_yield_none() {
        // Optional branch.
        let optional = vec![Pattern::Path(PathPattern::new(
            vec![PathElement::Optional(Box::new(node("a")))],
            Span::default(),
        ))];
        assert!(query_graph_from_match_patterns(&optional).is_none());
        // Variable-length edge.
        let ranged = vec![path(vec![
            node("a"),
            PathElement::Edge(EdgePattern::new(
                Some("e".to_string()),
                Vec::new(),
                None,
                Vec::new(),
                EdgeDirection::Out,
                Some(EdgeRange::at_least(1)),
                Span::default(),
            )),
            node("b"),
        ])];
        assert!(query_graph_from_match_patterns(&ranged).is_none());
        // Trailing edge without endpoint.
        let trailing = vec![path(vec![node("a"), edge("e")])];
        assert!(query_graph_from_match_patterns(&trailing).is_none());
        // Standalone variable.
        let variable = vec![Pattern::Variable(
            crate::parser::ast::pattern::VariablePattern::new("x".to_string(), Span::default()),
        )];
        assert!(query_graph_from_match_patterns(&variable).is_none());
        // Empty input.
        assert!(query_graph_from_match_patterns(&[]).is_none());
    }

    #[test]
    fn anonymous_variables_are_synthesized() {
        let patterns = vec![path(vec![
            PathElement::Node(NodePattern::new(
                None,
                Vec::new(),
                None,
                Vec::new(),
                Span::default(),
            )),
            edge("e1"),
            PathElement::Node(NodePattern::new(
                None,
                Vec::new(),
                None,
                Vec::new(),
                Span::default(),
            )),
        ])];
        let graph = query_graph_from_match_patterns(&patterns).expect("graph");
        assert_eq!(graph.num_nodes(), 2);
        assert!(graph.node_pos("__anon_n0").is_some());
        assert!(graph.node_pos("__anon_n1").is_some());
    }

    #[test]
    fn incoming_direction_is_preserved() {
        let patterns = vec![path(vec![
            node("a"),
            PathElement::Edge(EdgePattern::new(
                Some("e".to_string()),
                Vec::new(),
                None,
                Vec::new(),
                EdgeDirection::In,
                None,
                Span::default(),
            )),
            node("b"),
        ])];
        let graph = query_graph_from_match_patterns(&patterns).expect("graph");
        assert_eq!(graph.query_rels[0].direction, ExtendDirection::In);
    }
}
