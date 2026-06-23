//! Pattern matching definition
//!
//! Provide a pattern matching function for the planning nodes, which is used to rewrite rules in order to identify specific planning structures.
//! This is a simplified version that has been separated from the optimizer layer, focusing on the requirements for heuristic rewriting rules.

use crate::query::planning::plan::PlanNodeEnum;

/// Macro for generating node matching methods
///
/// Generate a convenient node matching constructor for the Pattern structure.
macro_rules! define_matcher {
    ($($method:ident => $name:expr),+ $(,)?) => {
        $(
            /// Create a pattern for matching nodes.
            pub fn $method() -> Self {
                Self::new_with_name($name)
            }
        )+
    };
}

/// Pattern Structure
///
/// Used to match the specific structure of the planning tree.
/// The matching criteria for the current node, as well as the patterns for the child nodes.
#[derive(Debug, Clone, Default)]
pub struct Pattern {
    /// The matching criteria for the current node
    pub node: Option<MatchNode>,
    /// List of child node patterns
    pub dependencies: Vec<Pattern>,
}

impl Pattern {
    /// Create an empty pattern (that matches any node).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a pattern using the specified nodes.
    pub fn with_node(node: MatchNode) -> Self {
        Self {
            node: Some(node),
            dependencies: Vec::new(),
        }
    }

    /// Create a pattern using the node names.
    pub fn new_with_name(name: &'static str) -> Self {
        Self::with_node(MatchNode::Single(name))
    }

    /// Create a pattern using multiple possible node names.
    pub fn multi(node_names: Vec<&'static str>) -> Self {
        Self::with_node(MatchNode::Multi(node_names))
    }

    /// Add child node mode
    pub fn with_dependency(mut self, dependency: Pattern) -> Self {
        self.dependencies.push(dependency);
        self
    }

    /// Use the node name to add a child node pattern.
    pub fn with_dependency_name(mut self, name: &'static str) -> Self {
        self.dependencies.push(Self::new_with_name(name));
        self
    }

    /// Add dependency pattern (variable reference version)
    pub fn add_dependency(&mut self, dependency: Pattern) {
        self.dependencies.push(dependency);
    }

    /// Check whether the mode matches the given scheduled node.
    pub fn matches(&self, plan_node: &PlanNodeEnum) -> bool {
        // Check the current node.
        if let Some(ref node) = self.node {
            if !node.matches(plan_node.name()) {
                return false;
            }
        }

        // If there is no dependency pattern, a direct match will be successful.
        if self.dependencies.is_empty() {
            return true;
        }

        // Obtain the names of all dependent nodes.
        let dep_names: Vec<&str> = self
            .dependencies
            .iter()
            .filter_map(|d| d.node.as_ref())
            .filter_map(|n| n.as_single())
            .collect();

        if dep_names.is_empty() {
            return true;
        }

        // Check whether each dependency pattern matches.
        // Use dependencies_ref() to avoid cloning nodes
        let deps = plan_node.dependencies_ref();
        for dep_pattern in &self.dependencies {
            let dep_matches = deps.iter().any(|input| dep_pattern.matches(input));

            if !dep_matches {
                return false;
            }
        }

        true
    }

    // ==================== Convenient Construction Methods ====================

    define_matcher! {
        with_project_matcher => "Project",
        with_filter_matcher => "Filter",
        with_scan_vertices_matcher => "ScanVertices",
        with_get_vertices_matcher => "GetVertices",
        with_limit_matcher => "Limit",
        with_sort_matcher => "Sort",
        with_aggregate_matcher => "Aggregate",
        with_dedup_matcher => "Dedup",
        with_get_neighbors_matcher => "GetNeighbors",
        with_traverse_matcher => "Traverse",
    }

    /// Create a pattern that matches the Join nodes (matching any type of connection).
    pub fn with_join_matcher() -> Self {
        Self::multi(vec![
            "HashInnerJoin",
            "HashLeftJoin",
            "InnerJoin",
            "LeftJoin",
            "CrossJoin",
            "FullOuterJoin",
        ])
    }
}

/// Node matching enumeration
///
/// Define how to match a single plan node.
#[derive(Debug, Clone)]
pub enum MatchNode {
    /// Match a node with a specific single name.
    Single(&'static str),
    /// Match any one of the multiple possible names.
    Multi(Vec<&'static str>),
    /// Match any node
    Any,
}

impl MatchNode {
    /// Check whether the node names match.
    pub fn matches(&self, node_name: &str) -> bool {
        match self {
            MatchNode::Single(name) => *name == node_name,
            MatchNode::Multi(names) => names.contains(&node_name),
            MatchNode::Any => true,
        }
    }

    /// Retrieve a single name (in the case of the Single variant).
    pub fn as_single(&self) -> Option<&'static str> {
        match self {
            MatchNode::Single(name) => Some(name),
            _ => None,
        }
    }

    /// Obtain multiple names (in the case of a Multi variant).
    pub fn as_multi(&self) -> Option<&Vec<&'static str>> {
        match self {
            MatchNode::Multi(names) => Some(names),
            _ => None,
        }
    }
}

/// Plan Node Matcher (Complex Conditions)
#[derive(Debug, Clone)]
pub enum PlanNodeMatcher {
    /// Match a specific name
    MatchNode(&'static str),
    /// Does not match.
    Not(Box<PlanNodeMatcher>),
    /// All conditions are met.
    And(Vec<PlanNodeMatcher>),
    /// Any condition matching
    Or(Vec<PlanNodeMatcher>),
}

impl PlanNodeMatcher {
    /// Check whether it matches the planned nodes.
    pub fn matches(&self, plan_node: &PlanNodeEnum) -> bool {
        match self {
            PlanNodeMatcher::MatchNode(name) => plan_node.name() == *name,
            PlanNodeMatcher::Not(matcher) => !matcher.matches(plan_node),
            PlanNodeMatcher::And(matchers) => matchers.iter().all(|m| m.matches(plan_node)),
            PlanNodeMatcher::Or(matchers) => matchers.iter().any(|m| m.matches(plan_node)),
        }
    }

    /// Combine with another matcher using the AND operator
    pub fn and(self, other: PlanNodeMatcher) -> Self {
        PlanNodeMatcher::And(vec![self, other])
    }

    /// Combine with another matcher using the OR operator.
    pub fn or(self, other: PlanNodeMatcher) -> Self {
        PlanNodeMatcher::Or(vec![self, other])
    }
}

/// Pattern Builder trait
///
/// Allowing the customization of type implementation patterns for construction
pub trait PatternBuilder {
    /// Constructing patterns
    fn build(&self) -> Pattern;
}

/// `NodeVisitor` trait
///
/// Used for traversing the planning tree
pub trait NodeVisitor {
    /// Access node
    /// Return `true` to continue the iteration; return `false` to stop the iteration.
    fn visit(&mut self, node: &PlanNodeEnum) -> bool;
}

/// The node records the visitors.
///
/// Record all visited nodes.
#[derive(Debug, Default)]
pub struct NodeVisitorRecorder {
    pub nodes: Vec<PlanNodeEnum>,
}

impl NodeVisitorRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, node: &PlanNodeEnum) {
        self.nodes.push(node.clone());
    }
}

impl NodeVisitor for NodeVisitorRecorder {
    fn visit(&mut self, node: &PlanNodeEnum) -> bool {
        self.record(node);
        true
    }
}

/// Node that searches for visitors
///
/// Find a node with a specific name.
#[derive(Debug)]
pub struct NodeVisitorFinder {
    pub target_name: String,
    pub found_node: Option<PlanNodeEnum>,
}

impl NodeVisitorFinder {
    pub fn new(target_name: &str) -> Self {
        Self {
            target_name: target_name.to_string(),
            found_node: None,
        }
    }
}

impl NodeVisitor for NodeVisitorFinder {
    fn visit(&mut self, node: &PlanNodeEnum) -> bool {
        if node.name() == self.target_name {
            self.found_node = Some(node.clone());
            return false; // Stop the iteration once the item is found.
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::ContextualExpression;
    use crate::core::Expression;
    use crate::core::Value;
    use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
    use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use std::sync::Arc;

    #[test]
    fn test_pattern_matches() {
        let pattern = Pattern::new_with_name("Project");
        let input_node = PlanNodeEnum::ScanVertices(ScanVerticesNode::new(1, "default"));
        let project_node = PlanNodeEnum::Project(
            ProjectNode::new(input_node.clone(), Vec::new())
                .expect("Creating the ProjectNode should succeed"),
        );
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta =
            crate::core::types::expr::ExpressionMeta::new(Expression::Literal(Value::Bool(true)));
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);
        let filter_node = PlanNodeEnum::Filter(
            FilterNode::new(input_node, ctx_expr).expect("Creating the FilterNode should succeed"),
        );

        assert!(pattern.matches(&project_node));
        assert!(!pattern.matches(&filter_node));
    }

    #[test]
    fn test_match_node_single() {
        let matcher = MatchNode::Single("Project");
        assert!(matcher.matches("Project"));
        assert!(!matcher.matches("Filter"));
    }

    #[test]
    fn test_match_node_multi() {
        let matcher = MatchNode::Multi(vec!["Project", "Filter"]);
        assert!(matcher.matches("Project"));
        assert!(matcher.matches("Filter"));
        assert!(!matcher.matches("ScanVertices"));
    }

    #[test]
    fn test_match_node_any() {
        let matcher = MatchNode::Any;
        assert!(matcher.matches("Project"));
        assert!(matcher.matches("Filter"));
        assert!(matcher.matches("ScanVertices"));
    }

    #[test]
    fn test_pattern_with_dependency() {
        let pattern = Pattern::new_with_name("Filter").with_dependency_name("Project");

        let scan = PlanNodeEnum::ScanVertices(ScanVerticesNode::new(1, "default"));
        let project = PlanNodeEnum::Project(
            ProjectNode::new(scan.clone(), Vec::new())
                .expect("Creating the ProjectNode should succeed"),
        );
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta =
            crate::core::types::expr::ExpressionMeta::new(Expression::Literal(Value::Bool(true)));
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx.clone());
        let filter = PlanNodeEnum::Filter(
            FilterNode::new(project.clone(), ctx_expr)
                .expect("Creating the FilterNode should succeed"),
        );

        assert!(pattern.matches(&filter));

        // “The ‘Filter’ -> ‘Scan’ functions should not match each other.”
        let expr_meta2 =
            crate::core::types::expr::ExpressionMeta::new(Expression::Literal(Value::Bool(true)));
        let id2 = ctx.register_expression(expr_meta2);
        let ctx_expr2 = ContextualExpression::new(id2, ctx);
        let filter2 = PlanNodeEnum::Filter(
            FilterNode::new(scan, ctx_expr2).expect("Creating the FilterNode should succeed"),
        );
        assert!(!pattern.matches(&filter2));
    }
}
