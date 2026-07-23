//! PhysicalPlanBuilder: builds an arena [`PhysicalPlan`] directly from planner nodes.
//!
//! This builder walks the [`PlanNodeEnum`] tree directly, creating arena operators
//! and a fragment DAG without the intermediate [`PhysicalNode`] representation.

use std::collections::HashMap;
use super::context::PhysicalPlanBuildContext;

use super::types::{
    CapabilitySet, FragmentGraph, FragmentSpec, LogicalNodeId, OutputContract,
    PhysicalOperatorId, PhysicalOperatorIdAllocator, PhysicalOperatorSpec, PhysicalPlan,
    PlanCompatibility,
};
use crate::query::executor::streaming::SourceSpec;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

mod assembler;
mod metadata;
mod specs;

use assembler::{ArenaFragmentAllocator, ArenaPlanAssembler};

/// Builds an arena [`PhysicalPlan`] from a [`PlanNodeEnum`] tree.
pub struct PhysicalPlanBuilder;

impl PhysicalPlanBuilder {
    /// Build a complete [`PhysicalPlan`] from a plan node.
    ///
    /// Walks the [`PlanNodeEnum`] tree directly, creating arena operators
    /// and a fragment DAG without the intermediate [`PhysicalNode`] representation.
    pub fn build(
        node: &PlanNodeEnum,
        ctx: &mut PhysicalPlanBuildContext,
        exec_ctx: &ExecutionContext,
    ) -> Result<PhysicalPlan, PlanBuildError> {
        let mut operators: Vec<PhysicalOperatorSpec> = Vec::new();
        let mut fragments: Vec<FragmentSpec> = Vec::new();
        let mut op_alloc = PhysicalOperatorIdAllocator::new();
        let mut frag_alloc = ArenaFragmentAllocator::new();

        let (root_fid, root_op_id) = ArenaPlanAssembler::convert_node(
            node,
            &mut operators,
            &mut fragments,
            &mut op_alloc,
            &mut frag_alloc,
            exec_ctx,
        )?;

        metadata::propagate_layouts(&mut operators, &fragments)?;
        metadata::populate_input_contracts(&mut operators, &fragments)?;
        metadata::populate_runtime_metadata(&mut operators);

        let output = operators
            .iter()
            .find(|op| op.operator_id == root_op_id)
            .map(|op| metadata::output_contract(&op.spec, op.output_layout.clone()))
            .unwrap_or_else(|| OutputContract {
                output_layout: super::super::slot::SlotLayout::new(vec![]),
                always_produces_row: false,
                nullability: Vec::new(),
                ordering: Vec::new(),
                delivery_streamable: true,
                pipeline_mode: super::types::PipelineMode::Pipelined,
            });

        let mut logical_to_physical: HashMap<LogicalNodeId, Vec<PhysicalOperatorId>> =
            HashMap::new();
        for operator in &operators {
            if let Some(logical_id) = operator.logical_node_id {
                logical_to_physical
                    .entry(logical_id)
                    .or_default()
                    .push(operator.operator_id);
            }
        }
        let mut fingerprint_hasher = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        format!("{:?}", operators).hash(&mut fingerprint_hasher);

        let mut required_capabilities = CapabilitySet::EMPTY;
        for operator in &operators {
            required_capabilities.insert(metadata::capability_for_operator(&operator.spec));
        }

        let plan = PhysicalPlan {
            operators,
            logical_to_physical,
            fragments: FragmentGraph::new(fragments, root_fid),
            root_fragment: root_fid,
            output,
            compatibility: PlanCompatibility {
                query_fingerprint: fingerprint_hasher.finish(),
                layout_version: ctx.schema.as_ref().map(|s| s.layout_version),
                required_capabilities: required_capabilities.clone(),
                planning_config_hash: ctx.config.config_hash,
                optimizer_version: ctx.config.optimizer_version,
            },
            required_capabilities,
            parameter_schema: ctx.parameter_schema.clone(),
        };

        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
    use crate::query::executor::streaming::plan::types::{InputContract, OperatorKindSpec};
    use crate::query::executor::base::ExecutionContext;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use std::sync::Arc;

    fn test_context() -> ExecutionContext {
        ExecutionContext::new(Arc::new(ExpressionAnalysisContext::new()))
    }

    #[test]
    fn test_build_start() {
        let start = StartNode::new();
        let node = PlanNodeEnum::Start(start);
        let mut ctx = PhysicalPlanBuildContext::new();
        let exec_ctx = test_context();

        let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).unwrap();
        assert_eq!(plan.operator_count(), 1);
        assert!(matches!(
            plan.operator(PhysicalOperatorId(0)).unwrap().spec,
            OperatorKindSpec::Source(SourceSpec::Start)
        ));
    }

    #[test]
    fn test_build_start_has_explain_name() {
        let start = StartNode::new();
        let node = PlanNodeEnum::Start(start);
        let mut ctx = PhysicalPlanBuildContext::new();
        let exec_ctx = test_context();

        let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).unwrap();
        let op = plan.operator(PhysicalOperatorId(0)).unwrap();
        assert_eq!(op.explain_name, "Start");
    }

    #[test]
    fn test_build_then_materialize_start() {
        use crate::query::executor::base::MemoryBudget;
        use crate::query::executor::streaming::instance::{
            QueryBindings, QueryExecutionInstance, ResultSink,
        };
        use crate::query::executor::streaming::transaction_scope::TransactionScope;
        use std::sync::Arc;

        let start = StartNode::new();
        let node = PlanNodeEnum::Start(start);
        let mut ctx = PhysicalPlanBuildContext::new();
        let exec_ctx = test_context();

        let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).unwrap();
        let plan_arc = Arc::new(plan);

        crate::query::executor::streaming::plan::validator::PhysicalPlanValidator::validate(
            &plan_arc,
        )
        .unwrap();

        let bindings = QueryBindings {
            parameters: Arc::new(std::collections::HashMap::new()),
            parameter_frame: None,
            space_name: None,
            storage: None,
            memory_budget: MemoryBudget::default_budget(),
            max_workers: 1,
            chunk_size: 1024,
            max_buffered_chunks: 4,
            query_id: 1,
            session_id: None,
            user_name: None,
            query_text: None,
            transaction: TransactionScope::None,
            shared_scheduler: None,
            #[cfg(feature = "fulltext-search")]
            fulltext_manager: None,
            #[cfg(feature = "qdrant")]
            vector_coordinator: None,
        };

        let result =
            QueryExecutionInstance::instantiate_plan(plan_arc, bindings, ResultSink::Discard, None);
        assert!(result.is_ok());
    }

    #[test]
    fn propagates_layout_through_blocking_pipeline_and_output_contract() {
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        use crate::query::planning::plan::core::nodes::operation::sort_node::LimitNode;

        let start = StartNode::new();
        let limit =
            LimitNode::new(PlanNodeEnum::Start(start), 0, 10).expect("limit plan should build");
        let node = PlanNodeEnum::Limit(limit);
        let mut ctx = PhysicalPlanBuildContext::new();
        let exec_ctx = test_context();

        let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).unwrap();
        let start_op = plan.operator(PhysicalOperatorId(0)).unwrap();
        let limit_op = plan.operator(PhysicalOperatorId(1)).unwrap();

        assert_eq!(start_op.explain_name, "Start");
        assert_eq!(limit_op.explain_name, "Limit");
        assert_eq!(plan.fragment_count(), 1);
    }

    #[test]
    fn transaction_command_has_start_input_fragment() {
        use crate::query::planning::plan::core::nodes::control_flow::BeginTransactionNode;

        let node = PlanNodeEnum::BeginTransaction(BeginTransactionNode::new(42));
        let mut ctx = PhysicalPlanBuildContext::new();
        let exec_ctx = test_context();

        let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx)
            .expect("transaction plan should build");
        assert_eq!(plan.operator_count(), 2);
        assert_eq!(plan.fragment_count(), 2);

        let root = plan
            .fragments
            .get(plan.root_fragment)
            .expect("root fragment should exist");
        assert_eq!(root.inputs.len(), 1);
        assert!(matches!(
            plan.operator(root.root_operator)
                .expect("transaction root operator should exist")
                .input_contract,
            InputContract::UnaryInput(_)
        ));
        crate::query::executor::streaming::plan::validator::PhysicalPlanValidator::validate(
            &Arc::new(plan),
        )
        .expect("transaction plan should validate");
    }
}
