//! Logical WCO intersect node: N-way worst-case optimal join.
//!
//! While [`LogicalIntersectNode`](super::graph_ops::LogicalIntersectNode) is
//! the binary set-operation intersect, this node is the multi-way pattern
//! intersect from worst-case optimal join planning: one probe side plus N
//! build sides sharing a single intersect variable. Each build side is keyed
//! by its bound node (already in the probe scope); execution intersects the
//! adjacency lists of all build sides per probe row.

use graphdb_core::types::expr::contextual::ContextualExpression;

use crate::define_logical_plan_node;
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

define_logical_plan_node! {
    pub struct LogicalWcoIntersectNode {
        intersect_key: ContextualExpression,
        bound_keys: Vec<ContextualExpression>,
    }
    enum: WcoIntersect
    input: MultipleInputNode
}

impl LogicalWcoIntersectNode {
    /// Build an N-way intersect over one probe side and at least one build
    /// side. `bound_keys[i]` is the bound node of build side `deps[i + 1]`.
    pub fn new(
        probe: LogicalNodeEnum,
        builds: Vec<LogicalNodeEnum>,
        intersect_key: ContextualExpression,
        bound_keys: Vec<ContextualExpression>,
        col_names: Vec<String>,
    ) -> Self {
        use crate::planning::plan::core::node_id_generator::next_node_id;
        assert!(
            !builds.is_empty(),
            "WCO intersect needs at least one build side"
        );
        assert_eq!(
            builds.len(),
            bound_keys.len(),
            "each build side needs one bound key"
        );
        let mut deps = Vec::with_capacity(builds.len() + 1);
        deps.push(probe);
        deps.extend(builds);
        Self {
            id: next_node_id(),
            deps,
            intersect_key,
            bound_keys,
            output_var: None,
            col_names,
            column_types: vec![],
        }
    }

    /// The shared intersect variable produced by this node.
    pub fn intersect_key(&self) -> &ContextualExpression {
        &self.intersect_key
    }

    /// Bound node key per build side (`bound_keys[i]` for `deps[i + 1]`).
    pub fn bound_keys(&self) -> &[ContextualExpression] {
        &self.bound_keys
    }

    /// Probe side (`deps[0]`).
    pub fn probe_side(&self) -> &LogicalNodeEnum {
        &self.deps[0]
    }

    /// Build sides (`deps[1..]`).
    pub fn build_sides(&self) -> &[LogicalNodeEnum] {
        &self.deps[1..]
    }

    pub fn num_builds(&self) -> usize {
        self.deps.len().saturating_sub(1)
    }

    /// Probe-side groups holding the bound keys. Mirrors Ladybug
    /// `LogicalIntersect::getGroupsPosToFlattenOnProbeSide`: every bound
    /// key present in the probe schema must be flattened before the
    /// intersect fans out.
    pub fn get_groups_to_flatten_on_probe_side(
        &self,
        probe_schema: &crate::planning::plan::factorization::FactorizedSchema,
    ) -> std::collections::HashSet<crate::planning::plan::factorization::FGroupPos> {
        let mut out = std::collections::HashSet::new();
        for key in &self.bound_keys {
            if let Some(pos) = probe_schema.get_group_pos(key.id()) {
                out.insert(pos);
            }
        }
        out
    }

    /// Build-side group holding the bound key of one build side. Mirrors
    /// Ladybug `getGroupsPosToFlattenOnBuildSide`.
    pub fn get_groups_to_flatten_on_build_side(
        &self,
        build_idx: usize,
        build_schema: &crate::planning::plan::factorization::FactorizedSchema,
    ) -> std::collections::HashSet<crate::planning::plan::factorization::FGroupPos> {
        let mut out = std::collections::HashSet::new();
        if let Some(key) = self.bound_keys.get(build_idx) {
            if let Some(pos) = build_schema.get_group_pos(key.id()) {
                out.insert(pos);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::ExpressionMeta;
    use graphdb_core::Expression;

    use crate::planning::plan::core::node_id_generator::next_node_id;
    use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;

    fn scan_node(var: &str) -> LogicalNodeEnum {
        LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
            id: next_node_id(),
            space_id: 1,
            space_name: "default".to_string(),
            tag: None,
            expression: None,
            limit: None,
            projected_properties: vec![],
            index_hint: None,
            estimated_cardinality: None,
            output_var: Some(var.to_string()),
            col_names: vec![var.to_string()],
            column_types: vec![],
        })
    }

    fn key(ctx: &Arc<ExpressionAnalysisContext>, var: &str) -> ContextualExpression {
        let id =
            ctx.register_expression(ExpressionMeta::new(Expression::Variable(var.to_string())));
        ContextualExpression::new(id, Arc::clone(ctx))
    }

    #[test]
    fn probe_plus_builds_layout() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let node = LogicalWcoIntersectNode::new(
            scan_node("a"),
            vec![scan_node("e1"), scan_node("e2")],
            key(&ctx, "c"),
            vec![key(&ctx, "a"), key(&ctx, "b")],
            vec!["a".to_string(), "c".to_string()],
        );
        assert_eq!(node.num_builds(), 2);
        assert_eq!(node.build_sides().len(), 2);
        assert_eq!(node.bound_keys().len(), 2);
        assert!(matches!(
            node.clone_logical_node(),
            LogicalNodeEnum::WcoIntersect(_)
        ));
    }

    #[test]
    #[should_panic(expected = "at least one build side")]
    fn rejects_empty_builds() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let _ =
            LogicalWcoIntersectNode::new(scan_node("a"), vec![], key(&ctx, "c"), vec![], vec![]);
    }

    #[test]
    fn physical_lowering_is_dedicated_wco_node() {
        use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
        use crate::planning::plan::logical::logical_node_traits::LogicalNode;
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let node = LogicalWcoIntersectNode::new(
            scan_node("a"),
            vec![scan_node("e1"), scan_node("e2")],
            key(&ctx, "c"),
            vec![key(&ctx, "a"), key(&ctx, "b")],
            vec!["a".to_string(), "e1".to_string(), "e2".to_string()],
        );
        let physical =
            crate::planning::physical_planner::convert_logical_to_physical(node.into_enum());
        // The logical N-way intersect lowers to the dedicated physical
        // node (probe plus one build side per bound key), not to a
        // binary join chain.
        let PlanNodeEnum::WcoIntersect(wco) = &physical else {
            panic!("expected physical WcoIntersect, got {:?}", physical.name());
        };
        assert_eq!(wco.num_builds(), 2);
        assert_eq!(wco.intersect_key().as_variable().as_deref(), Some("c"));
        let bound_names: Vec<_> = wco
            .bound_keys()
            .iter()
            .map(|k| k.as_variable().expect("bound key is a variable"))
            .collect();
        assert_eq!(bound_names, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            wco.col_names(),
            &["a".to_string(), "e1".to_string(), "e2".to_string()]
        );
    }

    #[test]
    fn factorized_schema_holds_intersect_in_new_group() {
        use crate::planning::plan::factorization::FactorizedSchemaCompute;
        use crate::planning::plan::logical::LogicalNodeEnum;
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let intersect_key = key(&ctx, "c");
        let intersect_id = intersect_key.id().clone();
        let bound_a = key(&ctx, "a");
        let mut node = LogicalNodeEnum::WcoIntersect(LogicalWcoIntersectNode::new(
            scan_node("a"),
            vec![scan_node("e1")],
            intersect_key,
            vec![bound_a.clone()],
            vec!["a".to_string(), "c".to_string()],
        ));
        let probe_schema = {
            let mut schema = crate::planning::plan::factorization::FactorizedSchema::new();
            let g = schema.create_flat_group(false);
            schema.insert_to_group_and_scope(bound_a.id().clone(), g);
            schema
        };
        let build_schema = crate::planning::plan::factorization::FactorizedSchema::new();
        let out = node.compute_factorized_schema(&[probe_schema, build_schema]);
        out.validate_at_most_one_unflat();
        assert!(out.is_expression_in_scope(&intersect_id));
    }
}
