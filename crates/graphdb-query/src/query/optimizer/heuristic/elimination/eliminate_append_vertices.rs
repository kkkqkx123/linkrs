//! Rules for eliminating redundancy and adding vertex operations
//!
//! According to the reference implementation of nebula-graph, this rule matches the Project->AppendVertices mode.
//! The AppendVertices node can be eliminated when it has no filtering criteria and the output column is an anonymous variable.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//!   Project
//!       |
//!   AppendVertices (vFilter=null, filter=null, 匿名列)
//!       |
//!   GetNeighbors
//! ```
//!
//! After:
//! ```text
//! Project (input should be replaced with “GetNeighbors”):
//!       |
//!   GetNeighbors
//! ```
//!
//! # Applicable Conditions
//!
//! The child nodes of a Project node are AppendVertices.
//! The `AppendVertices` method does not support the `vFilter` or `filter` parameters.
//! The output column of the AppendVertices function is an anonymous variable.
//! The list expression of the project does not contain the PathBuild expression.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::visitor_checkers::PathBuildContainsChecker;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::RewriteRule;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
use crate::query::planning::plan::core::nodes::traversal::traversal_node::AppendVerticesNode;
use crate::query::planning::plan::PlanNodeEnum;

/// Rules for removing redundancy and adding vertex operations
///
/// When the AppendVertices node meets certain conditions, it is directly removed from the planning tree.
#[derive(Debug)]
pub struct EliminateAppendVerticesRule;

impl EliminateAppendVerticesRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }

    /// Check whether the column names are anonymous variables (starting with __anon_).
    fn is_anonymous_var(&self, name: &str) -> bool {
        name.starts_with("__anon_")
    }

    /// Check whether the expression contains PathBuild.
    fn contains_path_build(&self, expr: &ContextualExpression) -> bool {
        let expr_meta = match expr.expression() {
            Some(e) => e,
            None => return false,
        };
        PathBuildContainsChecker::check(expr_meta.inner())
    }

    /// Check whether it is possible to eliminate the AppendVertices operation.
    fn can_eliminate_append_vertices(
        &self,
        project: &ProjectNode,
        append_vertices: &AppendVerticesNode,
    ) -> bool {
        // Check whether the list expressions of the Project contain PathBuild.
        for col in project.columns() {
            if self.contains_path_build(&col.expression) {
                return false;
            }
        }

        // Check whether AppendVertices has any filtering criteria.
        if append_vertices.v_filter().is_some() {
            return false;
        }

        // Check whether the last column of AppendVertices contains an anonymous variable.
        let col_names = append_vertices.col_names();
        if let Some(last_col) = col_names.last() {
            if !self.is_anonymous_var(last_col) {
                return false;
            }
        } else {
            return false;
        }

        true
    }
}

impl Default for EliminateAppendVerticesRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for EliminateAppendVerticesRule {
    fn name(&self) -> &'static str {
        "EliminateAppendVerticesRule"
    }

    fn pattern(&self) -> Pattern {
        // Match the Project->AppendVertices mode.
        Pattern::new_with_name("Project").with_dependency_name("AppendVertices")
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

        // Retrieve the input node (which should be AppendVertices).
        let input = project.input();
        let append_vertices = match input {
            PlanNodeEnum::AppendVertices(n) => n,
            _ => return Ok(None),
        };

        // Check whether it is possible to eliminate this element.
        if !self.can_eliminate_append_vertices(project, append_vertices) {
            return Ok(None);
        }

        // Obtaining the input for the AppendVertices method
        let append_inputs = append_vertices.inputs();
        if append_inputs.is_empty() {
            return Ok(None);
        }

        // Create a new Project node and enter the input parameters for the AppendVertices operation.
        let mut result = TransformResult::new();
        result.erase_curr = true;
        // Add a new Project node, with the input being the original input of the AppendVertices function.
        let new_project = PlanNodeEnum::Project(project.clone());
        result.add_new_node(new_project);

        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::heuristic::rule::RewriteRule;

    #[test]
    fn test_eliminate_append_vertices_rule_name() {
        let rule = EliminateAppendVerticesRule::new();
        assert_eq!(rule.name(), "EliminateAppendVerticesRule");
    }

    #[test]
    fn test_eliminate_append_vertices_rule_pattern() {
        let rule = EliminateAppendVerticesRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_is_anonymous_var() {
        let rule = EliminateAppendVerticesRule::new();
        assert!(rule.is_anonymous_var("__anon_123"));
        assert!(rule.is_anonymous_var("__anon_var"));
        assert!(!rule.is_anonymous_var("normal_var"));
        assert!(!rule.is_anonymous_var("v"));
    }
}
