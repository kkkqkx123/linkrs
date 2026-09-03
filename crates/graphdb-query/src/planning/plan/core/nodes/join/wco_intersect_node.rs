//! Physical N-way WCO intersect node.
//!
//! While binary joins pair one probe side with one build side, this node
//! carries one probe side plus N build sides sharing a single intersect
//! variable. Each build side is keyed by its bound node (already present in
//! the probe scope); execution intersects the per-bound adjacency lists.
//! The node mirrors
//! [`LogicalWcoIntersectNode`](crate::planning::plan::logical::logical_nodes::wco_intersect::LogicalWcoIntersectNode):
//! `input` is the probe side and `deps[0]`/`deps[1..]` are the probe/build
//! sides, so plan children enumerate every input.

use crate::define_plan_node_with_deps;
use graphdb_core::types::ContextualExpression;

define_plan_node_with_deps! {
    pub struct WcoIntersectNode {
        intersect_key: ContextualExpression,
        bound_keys: Vec<ContextualExpression>,
    }
    enum: WcoIntersect
    input: SingleInputNode
}

impl WcoIntersectNode {
    /// Build an N-way intersect over one probe side and at least one build
    /// side. `bound_keys[i]` is the bound node of build side `deps[i + 1]`.
    pub fn new(
        probe: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        builds: Vec<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
        intersect_key: ContextualExpression,
        bound_keys: Vec<ContextualExpression>,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        if builds.is_empty() {
            return Err(crate::planning::planner::PlannerError::InvalidOperation(
                "WCO intersect needs at least one build side".to_string(),
            ));
        }
        if builds.len() != bound_keys.len() {
            return Err(crate::planning::planner::PlannerError::InvalidOperation(
                "each WCO build side needs one bound key".to_string(),
            ));
        }
        // Column names mirror the logical node: probe columns, then the
        // intersect variable, then every new build column.
        let mut col_names = probe.col_names().to_vec();
        if let Some(name) = intersect_key.as_variable() {
            if !col_names.iter().any(|c| c == &name) {
                col_names.push(name);
            }
        }
        for build in &builds {
            for col in build.col_names() {
                if !col_names.contains(col) {
                    col_names.push(col.clone());
                }
            }
        }
        let mut deps = Vec::with_capacity(builds.len() + 1);
        deps.push(probe.clone());
        deps.extend(builds);
        Ok(Self {
            id: -1,
            input: Some(Box::new(probe)),
            deps,
            intersect_key,
            bound_keys,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    /// The shared intersect variable produced by this node.
    pub fn intersect_key(&self) -> &ContextualExpression {
        &self.intersect_key
    }

    /// Bound node key per build side (`bound_keys[i]` for `deps[i + 1]`).
    pub fn bound_keys(&self) -> &[ContextualExpression] {
        &self.bound_keys
    }

    /// Probe side (`input`, also `deps[0]`).
    pub fn probe_input(
        &self,
    ) -> &crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.deps[0]
    }

    /// Build sides (`deps[1..]`).
    pub fn build_inputs(
        &self,
    ) -> &[crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
        &self.deps[1..]
    }

    pub fn num_builds(&self) -> usize {
        self.deps.len().saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::{Expression, ExpressionMeta};
    use std::sync::Arc;

    fn key(name: &str) -> ContextualExpression {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id =
            ctx.register_expression(ExpressionMeta::new(Expression::Variable(name.to_string())));
        ContextualExpression::new(id, ctx)
    }

    #[test]
    fn wco_node_merges_probe_intersect_and_build_columns() {
        let probe = PlanNodeEnum::Start(StartNode::new());
        let build = PlanNodeEnum::Start(StartNode::new());
        let node = WcoIntersectNode::new(probe, vec![build], key("c"), vec![key("a")])
            .expect("node should build");
        assert_eq!(node.num_builds(), 1);
        assert!(node.col_names().contains(&"c".to_string()));
        assert_eq!(node.type_name(), "WcoIntersectNode");
    }

    #[test]
    fn wco_node_rejects_empty_builds() {
        let probe = PlanNodeEnum::Start(StartNode::new());
        let err = WcoIntersectNode::new(probe, vec![], key("c"), vec![]).unwrap_err();
        assert!(err.to_string().contains("at least one build side"));
    }

    #[test]
    fn wco_node_rejects_key_arity_mismatch() {
        let probe = PlanNodeEnum::Start(StartNode::new());
        let build = PlanNodeEnum::Start(StartNode::new());
        let err = WcoIntersectNode::new(probe, vec![build], key("c"), vec![]).unwrap_err();
        assert!(err.to_string().contains("one bound key"));
    }
}
