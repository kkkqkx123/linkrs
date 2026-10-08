//! Join hint support: user-directed binary and multi-way joins.
//!
//! Hints arrive as [`JoinHintAst`](crate::parser::ast::hint::JoinHintAst)
//! from `USING JOIN` clauses and lower to the standalone [`JoinHint`] tree
//! over query variables via [`JoinHint::from_ast`].
//! [`JoinTreeConstructor`] resolves the hint against a
//! [`QueryGraph`](super::query_graph::QueryGraph) into an executable
//! [`JoinTree`], validating that multi-way joins share exactly one common
//! neighbor node.

use std::sync::Arc;

use super::query_graph::QueryGraph;
use super::subquery_graph::SubqueryGraph;

/// Scan vs join shape of a hint tree node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeNodeType {
    NodeScan,
    RelScan,
    BinaryJoin,
    MultiwayJoin,
}

/// User-supplied join hint over query variables.
///
/// ```text
/// MATCH (a)-[e1]->(b), (a)-[e2]->(c), (b)-[e3]->(c)
/// JOIN HINT multiway(probe=binary(e1, e2), builds=[e3]) on c
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinHintNode {
    /// Leaf scan of one node or rel variable.
    Scan { variable: String },
    /// Binary join of two sub-hints.
    Binary {
        left: Box<JoinHintNode>,
        right: Box<JoinHintNode>,
    },
    /// Multi-way (WCO) join: first child probes, the rest build.
    Multiway {
        probe: Box<JoinHintNode>,
        builds: Vec<JoinHintNode>,
    },
}

/// A complete user join hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinHint {
    pub root: JoinHintNode,
}

impl JoinHint {
    pub fn scan(variable: impl Into<String>) -> Self {
        Self {
            root: JoinHintNode::Scan {
                variable: variable.into(),
            },
        }
    }

    /// Lower a parsed `USING JOIN` hint to a hint tree.
    ///
    /// `BINARY(a, b)` becomes a binary join of two scans; `MULTIWAY(p,
    /// b1, ..)` becomes a multi-way join with `p` probing. Returns `None`
    /// for degenerate inputs (empty build list); unknown variables are
    /// reported later by [`JoinTreeConstructor`] as `UnknownVariable`.
    pub fn from_ast(hint: &crate::parser::ast::hint::JoinHintAst) -> Option<Self> {
        use crate::parser::ast::hint::JoinHintAst as Ast;
        match hint {
            Ast::Binary { left, right } => Some(Self {
                root: JoinHintNode::Binary {
                    left: Box::new(JoinHintNode::Scan {
                        variable: left.clone(),
                    }),
                    right: Box::new(JoinHintNode::Scan {
                        variable: right.clone(),
                    }),
                },
            }),
            Ast::Multiway { probe, builds } => {
                if builds.is_empty() {
                    return None;
                }
                Some(Self {
                    root: JoinHintNode::Multiway {
                        probe: Box::new(JoinHintNode::Scan {
                            variable: probe.clone(),
                        }),
                        builds: builds
                            .iter()
                            .map(|var| JoinHintNode::Scan {
                                variable: var.clone(),
                            })
                            .collect(),
                    },
                })
            }
        }
    }
}

/// Extra join metadata carried by inner hint tree nodes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JoinTreeExtraInfo {
    /// Shared node variables joining the children.
    pub join_nodes: Vec<String>,
}

/// A resolved node in the user-hinted join tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinTreeNode {
    pub node_type: TreeNodeType,
    pub extra_info: JoinTreeExtraInfo,
    pub children: Vec<JoinTreeNode>,
}

impl JoinTreeNode {
    pub fn node_scan(variable: impl Into<String>) -> Self {
        Self {
            node_type: TreeNodeType::NodeScan,
            extra_info: JoinTreeExtraInfo {
                join_nodes: vec![variable.into()],
            },
            children: Vec::new(),
        }
    }

    pub fn rel_scan(variable: impl Into<String>) -> Self {
        Self {
            node_type: TreeNodeType::RelScan,
            extra_info: JoinTreeExtraInfo {
                join_nodes: vec![variable.into()],
            },
            children: Vec::new(),
        }
    }

    pub fn binary_join(left: JoinTreeNode, right: JoinTreeNode, join_nodes: Vec<String>) -> Self {
        Self {
            node_type: TreeNodeType::BinaryJoin,
            extra_info: JoinTreeExtraInfo { join_nodes },
            children: vec![left, right],
        }
    }

    pub fn multiway_join(
        probe: JoinTreeNode,
        builds: Vec<JoinTreeNode>,
        intersect_node: String,
    ) -> Self {
        let mut children = Vec::with_capacity(builds.len() + 1);
        children.push(probe);
        children.extend(builds);
        Self {
            node_type: TreeNodeType::MultiwayJoin,
            extra_info: JoinTreeExtraInfo {
                join_nodes: vec![intersect_node],
            },
            children,
        }
    }

    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }

    pub fn is_binary(&self) -> bool {
        self.node_type == TreeNodeType::BinaryJoin
    }

    pub fn is_multiway(&self) -> bool {
        self.node_type == TreeNodeType::MultiwayJoin
    }
}

/// A resolved user-hinted join tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinTree {
    pub root: JoinTreeNode,
}

impl JoinTree {
    /// Variables covered by the tree, as a subgraph of the query graph.
    pub fn covered_subgraph(&self, query_graph: &Arc<QueryGraph>) -> SubqueryGraph {
        covered_subgraph(&self.root, query_graph)
    }
}

fn covered_subgraph(node: &JoinTreeNode, query_graph: &Arc<QueryGraph>) -> SubqueryGraph {
    let mut subgraph = SubqueryGraph::new(Arc::clone(query_graph));
    match node.node_type {
        TreeNodeType::NodeScan => {
            for var in &node.extra_info.join_nodes {
                if let Some(pos) = query_graph.node_pos(var) {
                    subgraph.add_query_node(pos);
                }
            }
        }
        TreeNodeType::RelScan => {
            // Pattern coverage: scanning a rel binds its endpoints in the
            // hint view, so both are covered. This differs from the DP
            // lattice seed `SubqueryGraph::single_rel` (rel-only for
            // connectivity); the solver maps a RelScan to the DP entry
            // covering rel plus endpoints (built via extend steps).
            for var in &node.extra_info.join_nodes {
                if let Some(pos) = query_graph.rel_pos(var) {
                    subgraph.add_query_rel(pos);
                    if let Some((src, dst)) = query_graph.rel_endpoint_positions(pos) {
                        subgraph.add_query_node(src);
                        subgraph.add_query_node(dst);
                    }
                }
            }
        }
        TreeNodeType::BinaryJoin | TreeNodeType::MultiwayJoin => {
            for child in &node.children {
                subgraph.add_subquery_graph(&covered_subgraph(child, query_graph));
            }
            // Multi-way joins additionally cover the intersect node.
            if node.node_type == TreeNodeType::MultiwayJoin {
                for var in &node.extra_info.join_nodes {
                    if let Some(pos) = query_graph.node_pos(var) {
                        subgraph.add_query_node(pos);
                    }
                }
            }
        }
    }
    subgraph
}

/// Hint resolution failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JoinHintError {
    #[error("unknown variable in join hint: {0}")]
    UnknownVariable(String),
    #[error("multi-way join needs at least one build side")]
    MissingBuildSide,
    #[error("multi-way join has no single common neighbor node")]
    NoCommonIntersectNode,
}

/// Builds a [`JoinTree`] from a user [`JoinHint`].
#[derive(Debug, Default, Clone, Copy)]
pub struct JoinTreeConstructor;

impl JoinTreeConstructor {
    pub fn construct(
        query_graph: &Arc<QueryGraph>,
        hint: &JoinHint,
    ) -> Result<JoinTree, JoinHintError> {
        Ok(JoinTree {
            root: Self::construct_tree_node(query_graph, &hint.root)?,
        })
    }

    fn construct_tree_node(
        query_graph: &Arc<QueryGraph>,
        hint_node: &JoinHintNode,
    ) -> Result<JoinTreeNode, JoinHintError> {
        match hint_node {
            JoinHintNode::Scan { variable } => Self::construct_scan_node(query_graph, variable),
            JoinHintNode::Binary { left, right } => {
                let left = Self::construct_tree_node(query_graph, left)?;
                let right = Self::construct_tree_node(query_graph, right)?;
                let left_sg = covered_subgraph(&left, query_graph);
                let right_sg = covered_subgraph(&right, query_graph);
                let mut join_nodes: Vec<String> = left_sg
                    .get_connected_node_positions(&right_sg)
                    .into_iter()
                    .filter_map(|pos| query_graph.query_nodes.get(pos))
                    .map(|n| n.variable.clone())
                    .collect();
                join_nodes.sort();
                Ok(JoinTreeNode::binary_join(left, right, join_nodes))
            }
            JoinHintNode::Multiway { probe, builds } => {
                Self::construct_multiway_join(query_graph, probe, builds)
            }
        }
    }

    fn construct_scan_node(
        query_graph: &Arc<QueryGraph>,
        variable: &str,
    ) -> Result<JoinTreeNode, JoinHintError> {
        if query_graph.node_pos(variable).is_some() {
            return Ok(JoinTreeNode::node_scan(variable));
        }
        if query_graph.rel_pos(variable).is_some() {
            return Ok(JoinTreeNode::rel_scan(variable));
        }
        Err(JoinHintError::UnknownVariable(variable.to_string()))
    }

    fn construct_multiway_join(
        query_graph: &Arc<QueryGraph>,
        probe: &JoinHintNode,
        builds: &[JoinHintNode],
    ) -> Result<JoinTreeNode, JoinHintError> {
        if builds.is_empty() {
            return Err(JoinHintError::MissingBuildSide);
        }
        let probe = Self::construct_tree_node(query_graph, probe)?;
        let probe_subgraph = covered_subgraph(&probe, query_graph);
        let mut build_nodes = Vec::with_capacity(builds.len());
        let mut build_subgraphs = Vec::with_capacity(builds.len());
        for child in builds {
            let build = Self::construct_tree_node(query_graph, child)?;
            build_subgraphs.push(covered_subgraph(&build, query_graph));
            build_nodes.push(build);
        }
        let intersect = Self::common_neighbor(query_graph, &build_subgraphs, &probe_subgraph)?;
        Ok(JoinTreeNode::multiway_join(probe, build_nodes, intersect))
    }

    /// The shared neighbor of all build subgraphs: node positions adjacent
    /// to every build side. The WCO freshness constraint is enforced:
    /// the intersect node must be absent from the probe scope, otherwise
    /// planning would duplicate an already-bound variable. Returns
    /// `NoCommonIntersectNode` when no fresh common neighbor exists.
    fn common_neighbor(
        query_graph: &Arc<QueryGraph>,
        build_subgraphs: &[SubqueryGraph],
        probe_subgraph: &SubqueryGraph,
    ) -> Result<String, JoinHintError> {
        let mut common: Option<Vec<usize>> = None;
        for subgraph in build_subgraphs {
            let mut adjacent = subgraph.get_node_neighbor_positions();
            adjacent.extend(subgraph.node_positions());
            adjacent.sort_unstable();
            adjacent.dedup();
            common = Some(match common {
                None => adjacent,
                Some(prev) => prev.into_iter().filter(|p| adjacent.contains(p)).collect(),
            });
        }
        let common = common.unwrap_or_default();
        let chosen = common
            .iter()
            .filter(|p| !probe_subgraph.contains_node(**p))
            .min()
            .copied();
        chosen
            .and_then(|pos| query_graph.query_nodes.get(pos))
            .map(|n| n.variable.clone())
            .ok_or(JoinHintError::NoCommonIntersectNode)
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
    fn scan_resolves_node_vs_rel() {
        let qg = triangle();
        let node = JoinTreeConstructor::construct(&qg, &JoinHint::scan("a")).expect("node");
        assert_eq!(node.root.node_type, TreeNodeType::NodeScan);
        let rel = JoinTreeConstructor::construct(&qg, &JoinHint::scan("e1")).expect("rel");
        assert_eq!(rel.root.node_type, TreeNodeType::RelScan);
    }

    #[test]
    fn unknown_variable_is_error() {
        let qg = triangle();
        let err = JoinTreeConstructor::construct(&qg, &JoinHint::scan("zzz")).unwrap_err();
        assert_eq!(err, JoinHintError::UnknownVariable("zzz".to_string()));
    }

    #[test]
    fn binary_join_collects_shared_nodes() {
        let qg = triangle();
        let hint = JoinHint {
            root: JoinHintNode::Binary {
                left: Box::new(JoinHintNode::Scan {
                    variable: "e1".to_string(),
                }),
                right: Box::new(JoinHintNode::Scan {
                    variable: "e2".to_string(),
                }),
            },
        };
        let tree = JoinTreeConstructor::construct(&qg, &hint).expect("binary");
        assert_eq!(tree.root.node_type, TreeNodeType::BinaryJoin);
        // e1 covers {a, b, e1}, e2 covers {a, c, e2}: shared node a.
        assert_eq!(tree.root.extra_info.join_nodes, vec!["a".to_string()]);
    }

    #[test]
    fn multiway_join_finds_intersect_node() {
        let qg = triangle();
        // probe covers a and b; builds e2 (a-c) and e3 (b-c) share c.
        let hint = JoinHint {
            root: JoinHintNode::Multiway {
                probe: Box::new(JoinHintNode::Binary {
                    left: Box::new(JoinHintNode::Scan {
                        variable: "a".to_string(),
                    }),
                    right: Box::new(JoinHintNode::Scan {
                        variable: "b".to_string(),
                    }),
                }),
                builds: vec![
                    JoinHintNode::Scan {
                        variable: "e2".to_string(),
                    },
                    JoinHintNode::Scan {
                        variable: "e3".to_string(),
                    },
                ],
            },
        };
        let tree = JoinTreeConstructor::construct(&qg, &hint).expect("multiway");
        assert_eq!(tree.root.node_type, TreeNodeType::MultiwayJoin);
        assert_eq!(tree.root.extra_info.join_nodes, vec!["c".to_string()]);
        assert!(tree.root.is_multiway());
        // Covered: probe {a, b} + builds {a, c, e2} and {b, c, e3}.
        let covered = tree.covered_subgraph(&qg);
        assert_eq!(covered.total_num_variables(), 5);
    }

    #[test]
    fn from_ast_lowers_binary_and_multiway() {
        use crate::parser::ast::hint::JoinHintAst as Ast;
        let binary = JoinHint::from_ast(&Ast::Binary {
            left: "e1".to_string(),
            right: "e2".to_string(),
        })
        .expect("binary");
        assert!(matches!(binary.root, JoinHintNode::Binary { .. }));
        let multiway = JoinHint::from_ast(&Ast::Multiway {
            probe: "e1".to_string(),
            builds: vec!["e2".to_string(), "e3".to_string()],
        })
        .expect("multiway");
        match &multiway.root {
            JoinHintNode::Multiway { builds, .. } => assert_eq!(builds.len(), 2),
            other => panic!("expected multiway, got {other:?}"),
        }
        let qg = triangle();
        let tree = JoinTreeConstructor::construct(&qg, &binary).expect("tree");
        assert_eq!(tree.root.node_type, TreeNodeType::BinaryJoin);
    }

    #[test]
    fn multiway_without_builds_is_error() {
        let qg = triangle();
        let hint = JoinHint {
            root: JoinHintNode::Multiway {
                probe: Box::new(JoinHintNode::Scan {
                    variable: "a".to_string(),
                }),
                builds: vec![],
            },
        };
        assert_eq!(
            JoinTreeConstructor::construct(&qg, &hint).unwrap_err(),
            JoinHintError::MissingBuildSide
        );
    }
}
