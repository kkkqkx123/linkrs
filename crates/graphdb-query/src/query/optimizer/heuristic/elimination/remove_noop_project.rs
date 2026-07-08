//! Remove the rule that applies to projections with no operations.
//!
//! Based on the reference implementation of nebula-graph, this rule checks whether the Project node simply passes on the columns of its child nodes.
//! If that is the case, the Project node can be removed.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//!   Project(v1, v2, v3)  // 列名和子节点输出列名相同
//!       |
//! `ScanVertices` (outputs `v1`, `v2`, `v3`)
//! ```
//!
//! After:
//! ```text
//!   ScanVertices
//! ```
//!
//! # Applicable Conditions
//!
//! The output column of the Project node is exactly the same as the output column of its child nodes.
//! The list expression for the project represents a simple property reference (either VarProperty or InputProperty).
//! The child node is in the allowed list (removing a project is not allowed for certain types of nodes).

use crate::core::Expression;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{EliminationRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
use crate::query::planning::plan::PlanNodeEnum;
use std::collections::HashSet;

/// Remove the rule that applies to projections with no operations.
///
/// When the Project node simply passes on the columns of its child nodes, it is sufficient to remove the Project node directly.
#[derive(Debug)]
pub struct RemoveNoopProjectRule {
    /// Allow the removal of the set of sub-node types from the Project.
    allowed_child_types: HashSet<&'static str>,
}

impl RemoveNoopProjectRule {
    /// Create a rule instance
    pub fn new() -> Self {
        let mut allowed_child_types = HashSet::new();

        // Allow the removal of sub-node types from the Project.
        // Refer to the kQueries collection of nebula-graph.
        allowed_child_types.insert("GetNeighbors");
        allowed_child_types.insert("GetVertices");
        allowed_child_types.insert("GetEdges");
        allowed_child_types.insert("Traverse");
        allowed_child_types.insert("AppendVertices");
        allowed_child_types.insert("IndexScan");
        allowed_child_types.insert("ScanVertices");
        allowed_child_types.insert("ScanEdges");
        allowed_child_types.insert("EdgeIndexScan");
        allowed_child_types.insert("Union");
        allowed_child_types.insert("Project");
        allowed_child_types.insert("Unwind");
        allowed_child_types.insert("Sort");
        allowed_child_types.insert("TopN");
        allowed_child_types.insert("Sample");
        allowed_child_types.insert("Aggregate");
        allowed_child_types.insert("Assign");
        allowed_child_types.insert("InnerJoin");
        allowed_child_types.insert("HashInnerJoin");
        allowed_child_types.insert("HashLeftJoin");
        allowed_child_types.insert("CrossJoin");
        allowed_child_types.insert("DataCollect");
        allowed_child_types.insert("Argument");

        Self {
            allowed_child_types,
        }
    }

    /// Check whether the type of the child node allows the removal of the Project.
    fn is_allowed_child_type(&self, node: &PlanNodeEnum) -> bool {
        self.allowed_child_types.contains(node.name())
    }

    /// Check whether it is a projection with no operations (i.e., a projection that does not perform any computational tasks).
    fn is_noop_projection(&self, project: &ProjectNode, child_col_names: &[String]) -> bool {
        let proj_col_names = project.col_names();

        // The number of columns must be the same.
        if proj_col_names.len() != child_col_names.len() {
            return false;
        }

        let columns = project.columns();

        // Check each column.
        for (i, col) in columns.iter().enumerate() {
            let expr = &col.expression;

            // The expression must be a simple attribute reference.
            if let Some(expr_meta) = expr.expression() {
                let inner_expr = expr_meta.inner();
                match inner_expr {
                    Expression::Variable(var_name) => {
                        // The variable names must match the column names in the Project.
                        if var_name != &proj_col_names[i] {
                            return false;
                        }
                    }
                    Expression::Property { property, .. } => {
                        // The property names must match the column names in the Project.
                        if property != &proj_col_names[i] {
                            return false;
                        }
                    }
                    _ => {
                        // Other types of expressions are not equivalent to “operationally neutral projections” (i.e., projections that do not perform any specific mathematical operation).
                        return false;
                    }
                }
            } else {
                return false;
            }

            // Check whether the column names match the input column names.
            if proj_col_names[i] != child_col_names[i] {
                return false;
            }
        }

        true
    }
}

impl Default for RemoveNoopProjectRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for RemoveNoopProjectRule {
    fn name(&self) -> &'static str {
        "RemoveNoopProjectRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Project")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        // Check whether it is a Project node.
        let project = match node {
            PlanNodeEnum::Project(n) => n,
            _ => return Ok(None),
        };

        // Obtain the input node
        let input = project.input();

        // Check whether the type of the child node is allowed.
        if !self.is_allowed_child_type(input) {
            return Ok(None);
        }

        // Obtain the column names of the child nodes
        let child_col_names = input.col_names();

        // Check whether it is a projection with no operations (i.e., no specific actions or transformations being performed).
        if !self.is_noop_projection(project, child_col_names) {
            return Ok(None);
        }

        // Create a conversion result that replaces the current Project node with the input node.
        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(input.clone());

        Ok(Some(result))
    }
}

impl EliminationRule for RemoveNoopProjectRule {
    fn can_eliminate(&self, node: &PlanNodeEnum) -> bool {
        match node {
            PlanNodeEnum::Project(n) => {
                let input = n.input();
                if !self.is_allowed_child_type(input) {
                    return false;
                }
                self.is_noop_projection(n, input.col_names())
            }
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
    fn test_remove_noop_project_rule_name() {
        let rule = RemoveNoopProjectRule::new();
        assert_eq!(rule.name(), "RemoveNoopProjectRule");
    }

    #[test]
    fn test_remove_noop_project_rule_pattern() {
        let rule = RemoveNoopProjectRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_is_allowed_child_type() {
        let rule = RemoveNoopProjectRule::new();

        // The test determines the allowed types of child nodes.
        let start_node =
            crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
        // “The ‘Start’ option is not included in the allowed list.”
        assert!(!rule.is_allowed_child_type(&PlanNodeEnum::Start(start_node.clone())));
    }
}
