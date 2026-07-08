//! Remove the rule that adds a vertex at the bottom of the connection.
//!
//! According to the reference implementation of nebula-graph, this rule matches the following patterns:
//! HashInnerJoin/HashLeftJoin -> ... -> Project -> AppendVertices -> Traverse
//! The AppendVertices node can be removed when certain conditions are met.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//!   HashInnerJoin({id(v)}, {id(v)})
//!    /         \
//!   /           Project
//!  /               \
//! Left           AppendVertices(v)
//!                     \
//!                   Traverse(e)
//! ```
//!
//! After:
//! ```text
//!   HashInnerJoin({id(v)}, {$-.v})
//!    /         \
//!   /     Project(..., none_direct_dst(e) AS v)
//!  /               \
//! Left          Traverse(e)
//! ```
//!
//! # Applicable Conditions
//!
//! The right branch of “Join” is “Project->AppendVertices->Traverse”.
//! The `nodeAlias` of the `AppendVertices` function is only referenced once.
//! - Join 的 hash keys 匹配 id() 或 _joinkey() 模式

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::visitor::ExpressionVisitor;
use crate::core::types::expr::visitor_checkers::VariableContainsChecker;
use crate::core::types::expr::visitor_collectors::PropertyCollector;
use crate::core::types::expr::ExpressionMeta;
use crate::core::types::YieldColumn;
use crate::core::Expression;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteError, RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::RewriteRule;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use crate::query::planning::plan::core::nodes::join::join_node::{
    HashInnerJoinNode, HashLeftJoinNode,
};
use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
use crate::query::planning::plan::PlanNodeEnum;
use crate::query::validator::context::ExpressionAnalysisContext;
use std::sync::Arc;

/// Remove the rule that adds a vertex at the bottom of the connection.
///
/// When the right branch of the Join operation contains the AppendVertices function and certain conditions are met, the AppendVertices function should be removed.
#[derive(Debug)]
pub struct RemoveAppendVerticesBelowJoinRule;

impl RemoveAppendVerticesBelowJoinRule {
    /// Create a rule instance
    pub fn new() -> Self {
        Self
    }

    /// Collect all attribute names from the expression.
    fn collect_all_property_names(&self, expr: &Expression) -> Vec<String> {
        let mut collector = PropertyCollector::new();
        ExpressionVisitor::visit(&mut collector, expr);
        collector.properties
    }

    /// 检查表达式是否为 id() 或 _joinkey() 函数调用，返回参数表达式
    fn is_id_or_joinkey_function(
        &self,
        expr: &ContextualExpression,
    ) -> Option<ContextualExpression> {
        if let Some(expr_meta) = expr.expression() {
            let inner_expr = expr_meta.inner();
            match inner_expr {
                Expression::Function { name, args }
                    if (name == "id" || name == "_joinkey") && args.len() == 1 =>
                {
                    // Create new ContextualExpression packaging parameters.
                    let ctx = expr.context().clone();
                    let meta = ExpressionMeta::new(args[0].clone());
                    let id = ctx.register_expression(meta);
                    Some(ContextualExpression::new(id, ctx))
                }
                _ => None,
            }
        } else {
            None
        }
    }

    /// Check whether the expression references the specified attribute.
    fn expr_references_alias(&self, expr: &ContextualExpression, alias: &str) -> bool {
        if let Some(expr_meta) = expr.expression() {
            let inner_expr = expr_meta.inner();
            let properties = self.collect_all_property_names(inner_expr);
            properties.iter().any(|p| p == alias)
        } else {
            false
        }
    }

    /// Count the number of occurrences of `avNodeAlias` in the list of expressions.
    fn count_alias_references(&self, exprs: &[ContextualExpression], alias: &str) -> usize {
        exprs
            .iter()
            .filter(|e| self.expr_references_alias(e, alias))
            .count()
    }

    /// Count the number of occurrences of avNodeAlias in the YieldColumn list.
    fn count_alias_references_in_columns(&self, columns: &[YieldColumn], alias: &str) -> usize {
        columns
            .iter()
            .filter(|c| self.expr_references_alias(&c.expression, alias))
            .count()
    }

    /// Find the column index that contains the specified alias.
    fn find_column_with_alias(&self, columns: &[YieldColumn], alias: &str) -> Option<usize> {
        for (idx, col) in columns.iter().enumerate() {
            if let Some(expr_meta) = col.expression.expression() {
                if let Expression::Variable(var_name) = expr_meta.inner() {
                    if var_name == alias {
                        return Some(idx);
                    }
                }
            }
        }
        None
    }

    /// 查找 probe keys 中匹配 id()/_joinkey() 模式的索引
    fn find_matching_probe_key(
        &self,
        probe_keys: &[ContextualExpression],
        av_node_alias: &str,
    ) -> Option<usize> {
        for (idx, expr) in probe_keys.iter().enumerate() {
            if let Some(arg) = self.is_id_or_joinkey_function(expr) {
                if self.expr_contains_variable(&arg, av_node_alias) {
                    return Some(idx);
                }
            }
        }
        None
    }

    /// Check whether the expression contains references to the specified variables.
    fn expr_contains_variable(&self, expr: &ContextualExpression, var_name: &str) -> bool {
        if let Some(expr_meta) = expr.expression() {
            let inner_expr = expr_meta.inner();
            VariableContainsChecker::check(inner_expr, var_name)
        } else {
            false
        }
    }

    /// Create the expression for calling the none_direct_dst function.
    fn create_none_direct_dst_expr(
        &self,
        edge_alias: &str,
        vertex_alias: &str,
    ) -> ContextualExpression {
        let expr = Expression::Function {
            name: "none_direct_dst".to_string(),
            args: vec![
                Expression::Variable(edge_alias.to_string()),
                Expression::Variable(vertex_alias.to_string()),
            ],
        };
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    /// Create a variable reference expression.
    fn create_variable_expr(&self, var_name: &str) -> ContextualExpression {
        let expr = Expression::Variable(var_name.to_string());
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }
}

impl Default for RemoveAppendVerticesBelowJoinRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for RemoveAppendVerticesBelowJoinRule {
    fn name(&self) -> &'static str {
        "RemoveAppendVerticesBelowJoinRule"
    }

    fn pattern(&self) -> Pattern {
        // Match either HashInnerJoin or HashLeftJoin.
        Pattern::multi(vec!["HashInnerJoin", "HashLeftJoin"])
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        // Check whether it is a hash-linked node.
        let (hash_keys, probe_keys, left_input, right_input) = match node {
            PlanNodeEnum::HashInnerJoin(n) => (
                n.hash_keys().to_vec(),
                n.probe_keys().to_vec(),
                n.left_input().clone(),
                n.right_input().clone(),
            ),
            PlanNodeEnum::HashLeftJoin(n) => (
                n.hash_keys().to_vec(),
                n.probe_keys().to_vec(),
                n.left_input().clone(),
                n.right_input().clone(),
            ),
            _ => return Ok(None),
        };

        // Check whether the content entered on the right is a Project.
        let project = match right_input {
            PlanNodeEnum::Project(n) => n,
            _ => return Ok(None),
        };

        // Obtain the input nodes of the Project
        let project_input = project.input();

        // Check whether it is an operation to AppendVertices.
        let append_vertices = match project_input {
            PlanNodeEnum::AppendVertices(n) => n,
            _ => return Ok(None),
        };

        // Obtain the `node_alias` for the `AppendVertices` method.
        let av_node_alias = match append_vertices.node_alias() {
            Some(alias) => alias,
            None => return Ok(None),
        };

        // Obtaining the input nodes for the AppendVertices function
        let append_inputs = append_vertices.inputs();
        if append_inputs.is_empty() {
            return Ok(None);
        }

        // Check whether it is Traverse.
        let traverse = match &append_inputs[0] {
            PlanNodeEnum::Traverse(n) => n,
            _ => return Ok(None),
        };

        // Obtain the `edge_alias` and `vertex_alias` for Traverse.
        let tv_edge_alias = match traverse.edge_alias() {
            Some(alias) => alias,
            None => return Ok(None),
        };
        let _tv_node_alias = match traverse.vertex_alias() {
            Some(alias) => alias,
            None => return Ok(None),
        };

        // Check the number of occurrences of avNodeAlias in the probe keys.
        let probe_ref_count = self.count_alias_references(&probe_keys, av_node_alias);
        if probe_ref_count > 1 {
            // If it is referenced multiple times, the AppendVertices function cannot be removed.
            return Ok(None);
        }

        // Search for the index that matches the probe key.
        let probe_key_idx = match self.find_matching_probe_key(&probe_keys, av_node_alias) {
            Some(idx) => idx,
            None => return Ok(None),
        };

        // Check whether the corresponding hash key matches.
        if probe_key_idx >= hash_keys.len() {
            return Ok(None);
        }
        let corresponding_hash_key = &hash_keys[probe_key_idx];
        let probe_key = &probe_keys[probe_key_idx];
        if corresponding_hash_key != probe_key {
            return Ok(None);
        }

        // Check the number of occurrences of avNodeAlias in the Project columns.
        let columns = project.columns();
        let col_ref_count = self.count_alias_references_in_columns(columns, av_node_alias);
        if col_ref_count > 1 {
            return Ok(None);
        }

        // Find the column index in the Project that contains the `avNodeAlias`.
        let prj_idx = match self.find_column_with_alias(columns, av_node_alias) {
            Some(idx) => idx,
            None => return Ok(None),
        };

        // Create a new Project column
        let mut new_columns: Vec<YieldColumn> = columns.to_vec();
        let none_direct_dst_expr = self.create_none_direct_dst_expr(tv_edge_alias, _tv_node_alias);
        new_columns[prj_idx] = YieldColumn {
            expression: none_direct_dst_expr,
            alias: av_node_alias.clone(),
            is_matched: false,
        };

        // Create a new Project node.
        let new_project = ProjectNode::new(append_inputs[0].clone(), new_columns)
            .map_err(|e| RewriteError::InvalidPlanStructure(e.to_string()))?;

        // Create new probe keys.
        let mut new_probe_keys: Vec<ContextualExpression> = probe_keys.clone();
        new_probe_keys[probe_key_idx] = self.create_variable_expr(av_node_alias);

        // Create a new Join node.
        let new_join: PlanNodeEnum = match node {
            PlanNodeEnum::HashInnerJoin(_) => PlanNodeEnum::HashInnerJoin(
                HashInnerJoinNode::new(
                    left_input.clone(),
                    PlanNodeEnum::Project(new_project),
                    hash_keys.to_vec(),
                    new_probe_keys,
                )
                .map_err(|e| RewriteError::InvalidPlanStructure(e.to_string()))?,
            ),
            PlanNodeEnum::HashLeftJoin(_) => PlanNodeEnum::HashLeftJoin(
                HashLeftJoinNode::new(
                    left_input.clone(),
                    PlanNodeEnum::Project(new_project),
                    hash_keys.to_vec(),
                    new_probe_keys,
                )
                .map_err(|e| RewriteError::InvalidPlanStructure(e.to_string()))?,
            ),
            _ => unreachable!(),
        };

        // Construct the translation result.
        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(new_join);

        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::heuristic::rule::RewriteRule;

    #[test]
    fn test_remove_append_vertices_below_join_rule_name() {
        let rule = RemoveAppendVerticesBelowJoinRule::new();
        assert_eq!(rule.name(), "RemoveAppendVerticesBelowJoinRule");
    }

    #[test]
    fn test_remove_append_vertices_below_join_rule_pattern() {
        let rule = RemoveAppendVerticesBelowJoinRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }
}
