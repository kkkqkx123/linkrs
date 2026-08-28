//! PhysicalPlanBuilder: builds an arena [`PhysicalPlan`] directly from planner nodes.
//!
//! This builder walks the [`PlanNodeEnum`] tree directly, creating arena operators
//! and a fragment DAG.

use super::context::PhysicalPlanBuildContext;
use std::collections::HashMap;

use super::types::{
    CapabilitySet, FragmentGraph, FragmentSpec, LogicalNodeId, OutputContract, PhysicalOperatorId,
    PhysicalOperatorIdAllocator, PhysicalOperatorSpec, PhysicalPlan, PlanCompatibility,
    PlanFingerprint,
};
use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

mod assembler;
mod metadata;
mod partition;
mod specs;

use assembler::{ArenaFragmentAllocator, ArenaPlanAssembler};

/// Builds an arena [`PhysicalPlan`] from a [`PlanNodeEnum`] tree.
pub struct PhysicalPlanBuilder;

impl PhysicalPlanBuilder {
    /// Build a complete [`PhysicalPlan`] from a plan node.
    ///
    /// Walks the [`PlanNodeEnum`] tree directly, creating arena operators
    /// and a fragment DAG.  When `ctx.partition_spec` is set and the tree is
    /// partitionable, a partitioned plan (N local source fragments + one
    /// exchange fragment + global fragments) is produced instead; otherwise
    /// the single-tree path is used and the reason is recorded in
    /// `ctx.parallel_fallback_reason`.
    pub fn build(
        node: &PlanNodeEnum,
        ctx: &mut PhysicalPlanBuildContext,
        exec_ctx: &ExecutionContext,
    ) -> Result<PhysicalPlan, PlanBuildError> {
        let mut partitioned = None;
        if let Some(spec) = ctx.partition_spec.clone() {
            match partition::build_partitioned(node, &spec, exec_ctx) {
                Ok(Some(result)) => {
                    ctx.parallel_fallback_reason.clear();
                    partitioned = Some(result);
                }
                Ok(None) => {
                    if ctx.parallel_fallback_reason.is_empty() {
                        ctx.parallel_fallback_reason =
                            "plan shape does not support partitioned execution".to_string();
                    }
                }
                Err(error) => {
                    if ctx.parallel_fallback_reason.is_empty() {
                        ctx.parallel_fallback_reason =
                            format!("partitioned execution unavailable ({error})");
                    }
                }
            }
        }

        let (mut operators, mut fragments, root_fid, root_op_id) = match partitioned {
            Some((operators, fragments, root_fid, root_op_id)) => {
                (operators, fragments, root_fid, root_op_id)
            }
            None => Self::build_serial(node, exec_ctx)?,
        };

        Self::finalize(&mut operators, &mut fragments, root_fid, root_op_id, ctx)
    }

    /// Build the single linear fragment DAG for an unpartitioned plan.
    fn build_serial(
        node: &PlanNodeEnum,
        exec_ctx: &ExecutionContext,
    ) -> Result<
        (
            Vec<PhysicalOperatorSpec>,
            Vec<FragmentSpec>,
            super::types::FragmentId,
            PhysicalOperatorId,
        ),
        PlanBuildError,
    > {
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

        Ok((operators, fragments, root_fid, root_op_id))
    }

    /// Run the shared metadata passes and assemble the final plan.
    fn finalize(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        root_fid: super::types::FragmentId,
        root_op_id: PhysicalOperatorId,
        ctx: &PhysicalPlanBuildContext,
    ) -> Result<PhysicalPlan, PlanBuildError> {
        metadata::propagate_layouts(operators, fragments)?;
        metadata::populate_input_contracts(operators, fragments)?;
        metadata::populate_runtime_metadata(operators);
        metadata::populate_choice_reasons(operators);
        metadata::populate_estimated_rows(
            operators,
            fragments,
            &ctx.statistics.per_node_row_estimates,
        );

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
        for operator in &*operators {
            if let Some(logical_id) = operator.logical_node_id {
                logical_to_physical
                    .entry(logical_id)
                    .or_default()
                    .push(operator.operator_id);
            }
        }
        let fingerprint = PlanFingerprint::compute(operators);

        let mut required_capabilities = CapabilitySet::EMPTY;
        for operator in &*operators {
            required_capabilities.insert(metadata::capability_for_operator(&operator.spec));
        }

        let plan = PhysicalPlan {
            operators: std::mem::take(operators),
            logical_to_physical,
            fragments: FragmentGraph::new(std::mem::take(fragments), root_fid),
            root_fragment: root_fid,
            output,
            compatibility: PlanCompatibility {
                fingerprint,
                layout_version: ctx.schema.as_ref().map(|s| s.layout_version),
                required_capabilities: required_capabilities.clone(),
                planning_config_hash: ctx.config.config_hash,
                optimizer_version: ctx.config.optimizer_version,
            },
            required_capabilities,
            parameter_schema: ctx.parameter_schema.clone(),
            parallel_fallback_reason: ctx.parallel_fallback_reason.clone(),
            cbo_notes: ctx.cbo_notes.clone(),
            partition_spec: ctx.partition_spec.clone(),
        };

        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::base::ExecutionContext;
    use crate::executor::streaming::plan::types::{InputContract, OperatorKindSpec};
    use crate::executor::streaming::SourceSpec;
    use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
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
        use crate::executor::base::MemoryBudget;
        use crate::executor::streaming::instance::{
            QueryBindings, QueryExecutionInstance, ResultSink,
        };
        use crate::executor::streaming::transaction_scope::TransactionScope;
        use std::sync::Arc;

        let start = StartNode::new();
        let node = PlanNodeEnum::Start(start);
        let mut ctx = PhysicalPlanBuildContext::new();
        let exec_ctx = test_context();

        let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).unwrap();
        let plan_arc = Arc::new(plan);

        crate::executor::streaming::plan::validator::PhysicalPlanValidator::validate(&plan_arc)
            .unwrap();

        let bindings = QueryBindings {
            parameters: Arc::new(std::collections::HashMap::new()),
            session_variables: Arc::new(std::collections::HashMap::new()),
            parameter_frame: None,
            space_name: None,
            storage: None,
            bound_snapshot: None,
            memory_budget: MemoryBudget::default_budget(),
            max_workers: 1,
            chunk_size: 2048,
            max_buffered_chunks: 4,
            query_id: 1,
            cancel_token: None,
            session_id: None,
            user_name: None,
            query_text: None,
            transaction: TransactionScope::None,
            shared_scheduler: None,
            partition_count: 0,
            arena: None,
            feedback_history: None,
            columnar_policy: None,
            search: crate::executor::base::SearchContext::default(),
        };

        let result =
            QueryExecutionInstance::instantiate_plan(plan_arc, bindings, ResultSink::Discard, None);
        assert!(result.is_ok());
    }

    #[test]
    fn propagates_layout_through_blocking_pipeline_and_output_contract() {
        use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;
        use crate::planning::plan::core::nodes::operation::sort_node::LimitNode;

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
        use crate::planning::plan::core::nodes::control_flow::BeginTransactionNode;

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
        crate::executor::streaming::plan::validator::PhysicalPlanValidator::validate(&Arc::new(
            plan,
        ))
        .expect("transaction plan should validate");
    }

    #[test]
    fn correlated_apply_builds_nested_sub_plan_rooted_at_argument() {
        use crate::executor::streaming::operators::spec::ApplySpec;
        use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
        use crate::planning::plan::core::nodes::control_flow::ArgumentNode;
        use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::CorrelatedApplyNode;
        use crate::planning::plan::core::nodes::join::CrossJoinNode;
        use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
        use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
        use graphdb_core::types::{ContextualExpression, ExpressionMeta};

        // Outer (left) plan.
        let left = PlanNodeEnum::Start(StartNode::new());

        // Right subtree: Filter(CrossJoin(Argument(col_names = outer), Start),
        // true) — the self-contained plan rebuilt per outer row at runtime.
        let mut argument = ArgumentNode::new(-2, "_correlated_apply");
        argument.set_col_names(vec!["t".to_string()]);
        let cross = CrossJoinNode::new(PlanNodeEnum::Argument(argument), left.clone())
            .expect("cross join should build");
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_id = expr_ctx.register_expression(ExpressionMeta::new(
            graphdb_core::Expression::Literal(graphdb_core::Value::Bool(true)),
        ));
        let condition = ContextualExpression::new(expr_id, expr_ctx);
        let filter = FilterNode::new(cross.into_enum(), condition).expect("filter should build");
        let right = PlanNodeEnum::Filter(filter);

        let apply =
            CorrelatedApplyNode::new(left, right, false).expect("correlated apply should build");
        let node = PlanNodeEnum::CorrelatedApply(apply);

        let mut ctx = PhysicalPlanBuildContext::new();
        let exec_ctx = test_context();
        let plan =
            PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).expect("plan should build");

        let apply_op = plan
            .operators
            .iter()
            .find(|op| {
                matches!(
                    op.spec,
                    OperatorKindSpec::Apply(ApplySpec::CorrelatedApply { .. })
                )
            })
            .expect("plan contains a CorrelatedApply operator");

        let sub_plan = match &apply_op.spec {
            OperatorKindSpec::Apply(ApplySpec::CorrelatedApply { sub_plan, anti }) => {
                assert!(!anti, "correlated apply defaults to semi");
                sub_plan
            }
            _ => unreachable!("spec match earlier guarantees the variant"),
        };

        // The nested sub-plan is self-contained and contains an Argument
        // source whose layout mirrors the outer columns.
        let argument_source = sub_plan
            .operators
            .iter()
            .find(|op| {
                matches!(
                    op.spec,
                    OperatorKindSpec::Source(SourceSpec::Argument { .. })
                )
            })
            .expect("sub-plan must be rooted at an Argument source");
        match &argument_source.spec {
            OperatorKindSpec::Source(SourceSpec::Argument { col_names }) => {
                assert_eq!(
                    col_names,
                    &["t".to_string()],
                    "Argument col_names mirror the outer layout"
                );
            }
            _ => unreachable!(),
        }
    }
}
