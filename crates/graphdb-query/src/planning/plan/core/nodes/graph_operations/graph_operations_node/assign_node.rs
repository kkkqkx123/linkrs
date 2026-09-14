//! Implementation of data processing nodes
//!
//! Plan nodes related to data processing, including Union, Unwind, Dedup, etc.

use crate::define_plan_node_with_deps;
use crate::planning::statements::clauses::exists_planner::PlannedSubquery;
use graphdb_core::types::expr::contextual::ContextualExpression;

define_plan_node_with_deps! {
    pub struct AssignNode {
        assignments: Vec<(String, ContextualExpression)>,
        // Expression-level EXISTS / IN subqueries compiled for this assign
        // Pre-execution only; never serialized.
        subqueries: Vec<PlannedSubquery>,
        // Whether constant folding replaced part of the assignments.
        has_folded_expressions: bool,
    }
    enum: Assign
    input: SingleInputNode
}

impl AssignNode {
    /// Derive output names/types after assignments.
    ///
    /// Assignments overwrite same-named input columns (documented cover
    /// semantics, unlike `Unwind` which rejects conflicts); genuinely new
    /// aliases are appended with an `Unknown` type placeholder.
    fn derive_outputs(
        input_col_names: &[String],
        assignments: &[(String, ContextualExpression)],
    ) -> (Vec<String>, Vec<graphdb_core::DataType>) {
        let mut col_names = input_col_names.to_vec();
        for (alias, _) in assignments {
            if !col_names.iter().any(|name| name == alias) {
                col_names.push(alias.clone());
            }
        }
        let column_types = vec![graphdb_core::DataType::Unknown; col_names.len()];
        (col_names, column_types)
    }

    pub fn new(
        input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        assignments: Vec<(String, ContextualExpression)>,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let aliases: Vec<String> = assignments.iter().map(|(alias, _)| alias.clone()).collect();
        crate::planning::statements::projection_util::validate_output_aliases(&aliases)?;
        let (col_names, column_types) = Self::derive_outputs(input.col_names(), &assignments);

        Ok(Self {
            id: -1,
            input: Some(Box::new(input)),
            assignments,
            subqueries: Vec::new(),
            has_folded_expressions: false,
            output_var: None,
            col_names,
            column_types,
        })
    }

    pub fn assignments(&self) -> &[(String, ContextualExpression)] {
        &self.assignments
    }

    /// Replace the assignments (preserving subqueries).
    pub fn set_assignments(&mut self, assignments: Vec<(String, ContextualExpression)>) {
        let base: Vec<String> = self
            .input
            .as_ref()
            .map(|input| input.col_names().to_vec())
            .unwrap_or_else(|| self.col_names.clone());
        let (col_names, column_types) = Self::derive_outputs(&base, &assignments);
        self.col_names = col_names;
        self.column_types = column_types;
        self.assignments = assignments;
    }

    /// Attach expression-level subqueries to this assign.
    pub fn with_subqueries(mut self, subqueries: Vec<PlannedSubquery>) -> Self {
        self.subqueries = subqueries;
        self
    }

    /// Expression-level subqueries compiled for this assign.
    pub fn subqueries(&self) -> &[PlannedSubquery] {
        &self.subqueries
    }

    /// Whether constant folding replaced part of the assignments.
    pub fn has_folded_expressions(&self) -> bool {
        self.has_folded_expressions
    }

    /// Mark whether constant folding replaced part of the assignments.
    pub fn set_has_folded_expressions(&mut self, val: bool) {
        self.has_folded_expressions = val;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::ExpressionMeta;
    use graphdb_core::Expression;
    use std::sync::Arc;

    fn start_enum() -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
            crate::planning::plan::core::nodes::control_flow::start_node::StartNode::new(),
        )
    }

    fn ctx_expr(expr: Expression) -> ContextualExpression {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, ctx)
    }

    #[test]
    fn test_assign_rejects_duplicate_alias() {
        let e = ctx_expr(Expression::Variable("n".to_string()));
        let err = AssignNode::new(
            start_enum(),
            vec![("a".to_string(), e.clone()), ("a".to_string(), e)],
        )
        .expect_err("duplicate must fail");
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn test_assign_appends_new_column_and_syncs_types() {
        let e = ctx_expr(Expression::Variable("n".to_string()));
        let node = AssignNode::new(start_enum(), vec![("a".to_string(), e)]).expect("builds");
        assert_eq!(node.col_names, vec!["a".to_string()]);
        assert_eq!(node.column_types.len(), node.col_names.len());
    }
}
