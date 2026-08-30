pub mod apply_node;
pub mod assign_node;
pub mod correlated_apply_node;
pub mod data_collect_node;
pub mod dedup_node;
pub mod materialize_node;
pub mod pattern_apply_node;
pub mod remove_node;
pub mod rollup_apply_node;
pub mod union_node;
pub mod unwind_node;

pub use apply_node::{ApplyKind, ApplyNode};
pub use assign_node::AssignNode;
pub use correlated_apply_node::CorrelatedApplyNode;
pub use data_collect_node::DataCollectNode;
pub use dedup_node::DedupNode;
pub use materialize_node::MaterializeNode;
pub use pattern_apply_node::PatternApplyNode;
pub use remove_node::RemoveNode;
pub use rollup_apply_node::RollUpApplyNode;
pub use union_node::UnionNode;
pub use unwind_node::UnwindNode;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;

    #[test]
    fn test_union_node_creation() {
        let start_node =
            crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                StartNode::new(),
            );
        let start_node2 =
            crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                StartNode::new(),
            );

        let union_node = UnionNode::new(start_node, start_node2, true)
            .expect("Union node should be created successfully");

        assert_eq!(union_node.type_name(), "UnionNode");
        assert_eq!(union_node.dependencies().len(), 2);
        assert!(union_node.distinct());
    }

    #[test]
    fn test_unwind_node_creation() {
        let start_node =
            crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                StartNode::new(),
            );

        use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
        use graphdb_core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
        use std::sync::Arc;

        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let list_expr = Expression::Variable("list".to_string());
        let list_meta = ExpressionMeta::new(list_expr);
        let list_id = expr_ctx.register_expression(list_meta);
        let list_contextual = ContextualExpression::new(list_id, expr_ctx);

        let unwind_node = UnwindNode::new(start_node, "item", list_contextual)
            .expect("Unwind node should be created successfully");

        assert_eq!(unwind_node.type_name(), "UnwindNode");
        assert_eq!(unwind_node.dependencies().len(), 1);
        assert_eq!(unwind_node.alias(), "item");
    }

    #[test]
    fn test_dedup_node_creation() {
        let start_node =
            crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                StartNode::new(),
            );

        let dedup_node =
            DedupNode::new(start_node).expect("Dedup node should be created successfully");

        assert_eq!(dedup_node.type_name(), "DedupNode");
        assert_eq!(dedup_node.dependencies().len(), 1);
    }
}
