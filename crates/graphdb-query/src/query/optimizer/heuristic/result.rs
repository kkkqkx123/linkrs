//! Rewrite the definition of the result.
//!
//! Define the return type of the rule that rewrites the content.
//! This is a simplified version that has been separated from the optimizer layer.

/// Rewrite the error type
#[derive(Debug, thiserror::Error)]
pub enum RewriteError {
    #[error("Invalid plan node: {0}")]
    InvalidNode(String),

    #[error("Rewrite failed: {0}")]
    RewriteFailed(String),

    #[error("Unsupported node types: {0}")]
    UnsupportedNodeType(String),

    #[error("Optimizer error: {0}")]
    OptimizerError(String),

    #[error("Loop detection: node {0}")]
    CycleDetected(usize),

    #[error("Invalid program structure: {0}")]
    InvalidPlanStructure(String),
}

impl RewriteError {
    pub fn invalid_node(msg: impl Into<String>) -> Self {
        Self::InvalidNode(msg.into())
    }

    pub fn rewrite_failed(msg: impl Into<String>) -> Self {
        Self::RewriteFailed(msg.into())
    }

    pub fn unsupported_node_type(name: impl Into<String>) -> Self {
        Self::UnsupportedNodeType(name.into())
    }

    pub fn optimizer_error(msg: impl Into<String>) -> Self {
        Self::OptimizerError(msg.into())
    }

    pub fn cycle_detected(node_id: usize) -> Self {
        Self::CycleDetected(node_id)
    }

    pub fn invalid_plan_structure(msg: impl Into<String>) -> Self {
        Self::InvalidPlanStructure(msg.into())
    }
}

/// Rewrite the result in the desired format.
pub type RewriteResult<T> = std::result::Result<T, RewriteError>;

/// Of course! Please provide the text you would like to have translated.
///
/// Record the results after the application of the rule rewriting rules.
#[derive(Debug, Default, Clone)]
pub struct TransformResult {
    /// Should the current node be deleted?
    pub erase_curr: bool,
    /// Should all related nodes be deleted?
    pub erase_all: bool,
    /// New list of planned project milestones
    pub new_nodes: Vec<crate::query::planning::plan::PlanNodeEnum>,
    /// New dependencies
    pub new_dependencies: Vec<usize>,
}

impl TransformResult {
    /// Create a new conversion result.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set whether to delete the current node.
    pub fn with_erase_curr(mut self, erase_curr: bool) -> Self {
        self.erase_curr = erase_curr;
        self
    }

    /// Set whether to delete all nodes.
    pub fn with_erase_all(mut self, erase_all: bool) -> Self {
        self.erase_all = erase_all;
        self
    }

    /// Add a new planning node
    pub fn add_new_node(&mut self, node: crate::query::planning::plan::PlanNodeEnum) {
        self.new_nodes.push(node);
    }

    /// Add a new dependency
    pub fn add_new_dependency(&mut self, dep_id: usize) {
        self.new_dependencies.push(dep_id);
    }

    /// Set the deleted marker
    pub fn with_erased(mut self) -> Self {
        self.erase_curr = true;
        self
    }

    /// Check whether there are any new nodes.
    pub fn has_new_nodes(&self) -> bool {
        !self.new_nodes.is_empty()
    }

    /// Retrieve the first new node (if it exists).
    pub fn first_new_node(&self) -> Option<&crate::query::planning::plan::PlanNodeEnum> {
        self.new_nodes.first()
    }
}

/// Matching results
///
/// Results of the record mode matching
#[derive(Debug, Default, Clone)]
pub struct MatchedResult {
    /// List of matching nodes
    pub nodes: Vec<crate::query::planning::plan::PlanNodeEnum>,
    /// List of dependent nodes
    pub dependencies: Vec<crate::query::planning::plan::PlanNodeEnum>,
    /// Root node
    pub root_node: Option<crate::query::planning::plan::PlanNodeEnum>,
}

impl MatchedResult {
    /// Create new matching results.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add matching nodes
    pub fn add_node(&mut self, node: crate::query::planning::plan::PlanNodeEnum) {
        self.nodes.push(node);
    }

    /// Add dependency nodes
    pub fn add_dependency(&mut self, node: crate::query::planning::plan::PlanNodeEnum) {
        self.dependencies.push(node);
    }

    /// Setting the root node
    pub fn set_root_node(&mut self, node: crate::query::planning::plan::PlanNodeEnum) {
        self.root_node = Some(node);
    }

    /// Check whether there are any matching nodes.
    pub fn has_matches(&self) -> bool {
        !self.nodes.is_empty()
    }

    /// Retrieve the first matching node.
    pub fn first_node(&self) -> Option<&crate::query::planning::plan::PlanNodeEnum> {
        self.nodes.first()
    }

    /// Obtain the first dependent node.
    pub fn first_dependency(&self) -> Option<&crate::query::planning::plan::PlanNodeEnum> {
        self.dependencies.first()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;

    #[test]
    fn test_transform_result() {
        let mut result = TransformResult::new();
        assert!(!result.has_new_nodes());

        let node = crate::query::planning::plan::PlanNodeEnum::ScanVertices(ScanVerticesNode::new(
            1, "default",
        ));
        result.add_new_node(node);

        assert!(result.has_new_nodes());
        assert!(result.first_new_node().is_some());
    }

    #[test]
    fn test_matched_result() {
        let mut result = MatchedResult::new();
        assert!(!result.has_matches());

        let node = crate::query::planning::plan::PlanNodeEnum::ScanVertices(ScanVerticesNode::new(
            1, "default",
        ));
        result.add_node(node);

        assert!(result.has_matches());
        assert!(result.first_node().is_some());
    }

    #[test]
    fn test_rewrite_error() {
        let err = RewriteError::invalid_node("test node");
        assert!(err.to_string().contains("test node"));

        let err = RewriteError::cycle_detected(42);
        assert!(err.to_string().contains("42"));
    }
}
