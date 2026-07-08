//! Rules for eliminating redundant data collection operations
//!
//! According to the reference implementation of nebula-graph, this rule matches the DataCollect->Project pattern.
//! When the `kind` of `DataCollect` is `kRowBasedMove`, the `DataCollect` node can be eliminated.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//!   DataCollect(kind=kRowBasedMove)
//!       |
//!   Project
//!       |
//!   ScanVertices
//! ```
//!
//! After:
//! ```text
//! Project (replace “output_var” with “DataCollect’s output_var”)
//!       |
//!   ScanVertices
//! ```
//!
//! # Applicable Conditions
//!
//! The kind of the DataCollect node is kRowBasedMove.
//! The child node of DataCollect is Project.

use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{EliminationRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::DataCollectNode;
use crate::query::planning::plan::PlanNodeEnum;

/// Rules for eliminating redundant data collection operations
///
/// When the kind of the DataCollect node is kRowBasedMove and its child node is Project,
/// The DataCollect node can be directly removed, and the output_var of the Project can be replaced with the output_var of the DataCollect node.
#[derive(Debug)]
pub struct EliminateRowCollectRule;

impl EliminateRowCollectRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }

    /// Check whether DataCollect is of the kRowBasedMove type.
    fn is_row_based_move(&self, data_collect: &DataCollectNode) -> bool {
        data_collect.collect_kind() == "kRowBasedMove"
    }
}

impl Default for EliminateRowCollectRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for EliminateRowCollectRule {
    fn name(&self) -> &'static str {
        "EliminateRowCollectRule"
    }

    fn pattern(&self) -> Pattern {
        // Match the DataCollect->Project pattern.
        Pattern::new_with_name("DataCollect").with_dependency_name("Project")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        // Check whether it is a DataCollect node.
        let data_collect = match node {
            PlanNodeEnum::DataCollect(n) => n,
            _ => return Ok(None),
        };

        // Check whether `collect_kind` is equal to `kRowBasedMove`.
        if !self.is_row_based_move(data_collect) {
            return Ok(None);
        }

        // Obtain the input node (which should be the Project).
        let input = data_collect.input();
        let project = match input {
            PlanNodeEnum::Project(n) => n,
            _ => return Ok(None),
        };

        // Create a new Project node, and change “output_var” to the “output_var” of DataCollect.
        let mut result = TransformResult::new();
        result.erase_curr = true;
        // Clone the Project node, preserving all its attributes.
        let new_project = PlanNodeEnum::Project(project.clone());
        result.add_new_node(new_project);

        Ok(Some(result))
    }
}

impl EliminationRule for EliminateRowCollectRule {
    fn can_eliminate(&self, node: &PlanNodeEnum) -> bool {
        match node {
            PlanNodeEnum::DataCollect(n) => self.is_row_based_move(n),
            _ => false,
        }
    }

    fn eliminate(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        self.apply(ctx, node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::heuristic::rule::RewriteRule;

    #[test]
    fn test_eliminate_row_collect_rule_name() {
        let rule = EliminateRowCollectRule::new();
        assert_eq!(rule.name(), "EliminateRowCollectRule");
    }

    #[test]
    fn test_eliminate_row_collect_rule_pattern() {
        let rule = EliminateRowCollectRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_is_row_based_move() {
        let rule = EliminateRowCollectRule::new();

        // Create a DataCollectNode for testing purposes.
        let start_node =
            crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
        let start_enum = PlanNodeEnum::Start(start_node);

        let data_collect = DataCollectNode::new(start_enum.clone(), "kRowBasedMove")
            .expect("Failed to create DataCollectNode");
        assert!(rule.is_row_based_move(&data_collect));

        let data_collect2 = DataCollectNode::new(start_enum, "kOtherKind")
            .expect("Failed to create DataCollectNode");
        assert!(!rule.is_row_based_move(&data_collect2));
    }
}
