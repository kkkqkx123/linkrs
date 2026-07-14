//! PhysicalPlanBuilder: builds an arena [`PhysicalPlan`] from planner nodes.
//!
//! This is the single builder path that replaces both the tree-based
//! [`PhysicalNode`] builder and the partitioned plan builder.
//!
//! Switching order (per M3.3):
//! 1. scan / filter / project / limit
//! 2. blocking relational operators
//! 3. join / set / apply
//! 4. graph traversal / path
//! 5. DML / DDL / transaction / fulltext / vector
//! 6. production facade
//! 7. delete old PartitionedPhysicalPlan and PhysicalNode paths

use std::collections::HashMap;
use std::sync::Arc;

use super::context::PhysicalPlanBuildContext;
use super::properties::{PhysicalProperties, PipelineKind};
use super::types::{
    CapabilitySet, FragmentGraph, FragmentId, FragmentKind, FragmentSpec, LogicalNodeId,
    OperatorKindSpec, OutputContract, PhysicalOperatorId, PhysicalOperatorSpec, PhysicalPlan,
    PlanCompatibility,
};
use super::super::operators::spec::SourceSpec;
use super::super::slot::SlotLayout;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

/// Builds an arena [`PhysicalPlan`] from a [`PlanNodeEnum`] tree.
pub struct PhysicalPlanBuilder;

impl PhysicalPlanBuilder {
    /// Build a complete [`PhysicalPlan`] from a plan node.
    pub fn build(
        node: &PlanNodeEnum,
        ctx: &mut PhysicalPlanBuildContext,
    ) -> Result<PhysicalPlan, PlanBuildError> {
        let (operators, fragments, root_fragment_id) =
            Self::build_fragment_graph(node, ctx)?;

        let root_op = operators.last().ok_or_else(|| {
            PlanBuildError::unsupported(node.name(), node.id(), "empty plan")
        })?;
        let output = OutputContract {
            output_layout: root_op.output_layout.clone(),
            always_produces_row: false,
        };

        let plan = PhysicalPlan {
            operators,
            logical_to_physical: HashMap::new(),
            fragments,
            root_fragment: root_fragment_id,
            output,
            compatibility: PlanCompatibility {
                query_fingerprint: 0,
                layout_version: ctx.schema.as_ref().map(|s| s.layout_version),
                required_capabilities: CapabilitySet::EMPTY,
                planning_config_hash: ctx.config.config_hash,
                optimizer_version: ctx.config.optimizer_version,
            },
            required_capabilities: CapabilitySet::EMPTY,
            parameter_schema: ctx.parameter_schema.clone(),
        };

        Ok(plan)
    }

    fn build_fragment_graph(
        node: &PlanNodeEnum,
        ctx: &mut PhysicalPlanBuildContext,
    ) -> Result<(Vec<PhysicalOperatorSpec>, FragmentGraph, FragmentId), PlanBuildError> {
        let mut operators: Vec<PhysicalOperatorSpec> = Vec::new();
        let mut fragments: Vec<FragmentSpec> = Vec::new();

        let fid = ctx.allocate_fragment_id();
        let ops = Self::build_operator_chain(node, ctx)?;
        let root_op_id = ops.last().map(|op| op.operator_id).ok_or_else(|| {
            PlanBuildError::unsupported(node.name(), node.id(), "no operators produced")
        })?;

        let operator_ids: Vec<PhysicalOperatorId> = ops.iter().map(|op| op.operator_id).collect();
        let fragment = FragmentSpec {
            id: fid,
            kind: Self::fragment_kind_for_node(node),
            operators: operator_ids,
            root_operator: root_op_id,
            inputs: Vec::new(),
            output: None,
            exchange_layout: None,
        };

        operators.extend(ops);
        fragments.push(fragment);

        let graph = FragmentGraph::new(fragments, fid);
        Ok((operators, graph, fid))
    }

    fn build_operator_chain(
        node: &PlanNodeEnum,
        ctx: &mut PhysicalPlanBuildContext,
    ) -> Result<Vec<PhysicalOperatorSpec>, PlanBuildError> {
        let op = Self::node_to_operator(node, ctx)?;
        Ok(vec![op])
    }

    fn node_to_operator(
        node: &PlanNodeEnum,
        ctx: &mut PhysicalPlanBuildContext,
    ) -> Result<PhysicalOperatorSpec, PlanBuildError> {
        let id = ctx.allocate_operator_id();
        let logical_id = LogicalNodeId(node.id());

        match node {
            PlanNodeEnum::Start(_) => Ok(PhysicalOperatorSpec {
                operator_id: id,
                logical_node_id: Some(logical_id),
                spec: OperatorKindSpec::Source(SourceSpec::Start),
                input_layout: None,
                output_layout: SlotLayout::new(vec![]),
                properties: PhysicalProperties::single_streaming(),
                estimated_cardinality: None,
                explain_name: "Start",
            }),

            PlanNodeEnum::GetVertices(gv) => {
                let col_names: Vec<String> = gv.col_names().to_vec();
                let output_layout = SlotLayout::from_names(&col_names);
                Ok(PhysicalOperatorSpec {
                    operator_id: id,
                    logical_node_id: Some(logical_id),
                    spec: OperatorKindSpec::Source(SourceSpec::GetVertices {
                        space_name: gv.space_name().to_string(),
                        vertex_ids: None,
                    }),
                    input_layout: None,
                    output_layout,
                    properties: PhysicalProperties::single_streaming(),
                    estimated_cardinality: None,
                    explain_name: "GetVertices",
                })
            }

            // Unsupported: structured error
            PlanNodeEnum::Loop(_) => Err(PlanBuildError::unsupported(
                node.name(), node.id(), "Loop not supported",
            )),
            PlanNodeEnum::PassThrough(_) => Err(PlanBuildError::unsupported(
                node.name(), node.id(), "PassThrough not supported",
            )),
            PlanNodeEnum::Select(_) => Err(PlanBuildError::unsupported(
                node.name(), node.id(), "Select not supported",
            )),
            PlanNodeEnum::AppendVertices(_) => Err(PlanBuildError::unsupported(
                node.name(), node.id(), "AppendVertices not supported",
            )),

            other => Err(PlanBuildError::unsupported(
                other.name(),
                other.id(),
                "arena builder: not yet implemented for this node type",
            )),
        }
    }

    fn fragment_kind_for_node(node: &PlanNodeEnum) -> FragmentKind {
        match node {
            PlanNodeEnum::Start(_)
            | PlanNodeEnum::GetVertices(_)
            | PlanNodeEnum::GetEdges(_)
            | PlanNodeEnum::GetNeighbors(_)
            | PlanNodeEnum::ScanVertices(_)
            | PlanNodeEnum::ScanEdges(_)
            | PlanNodeEnum::EdgeIndexScan(_)
            | PlanNodeEnum::IndexScan(_) => FragmentKind::Source,

            PlanNodeEnum::Sort(_)
            | PlanNodeEnum::TopN(_)
            | PlanNodeEnum::Dedup(_)
            | PlanNodeEnum::Aggregate(_)
            | PlanNodeEnum::Window(_) => FragmentKind::Blocking,

            _ => FragmentKind::Streaming,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::executor::streaming::plan::types::PhysicalOperatorId;

    #[test]
    fn test_build_start() {
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        let start = StartNode::new();
        let node = PlanNodeEnum::Start(start);
        let mut ctx = PhysicalPlanBuildContext::new();

        let plan = PhysicalPlanBuilder::build(&node, &mut ctx).unwrap();
        assert_eq!(plan.operator_count(), 1);
        assert!(matches!(
            plan.operator(PhysicalOperatorId(0)).unwrap().spec,
            OperatorKindSpec::Source(SourceSpec::Start)
        ));
    }

    #[test]
    fn test_build_get_vertices() {
        use crate::query::planning::plan::core::nodes::access::graph_scan_node::GetVerticesNode;
        let gv = GetVerticesNode::new(0, "test", "tag");
        let node = PlanNodeEnum::GetVertices(gv);
        let mut ctx = PhysicalPlanBuildContext::new();

        let plan = PhysicalPlanBuilder::build(&node, &mut ctx).unwrap();
        assert_eq!(plan.operator_count(), 1);
    }

    #[test]
    fn test_build_start_has_explain_name() {
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        let start = StartNode::new();
        let node = PlanNodeEnum::Start(start);
        let mut ctx = PhysicalPlanBuildContext::new();

        let plan = PhysicalPlanBuilder::build(&node, &mut ctx).unwrap();
        let op = plan.operator(PhysicalOperatorId(0)).unwrap();
        assert_eq!(op.explain_name, "Start");
    }

    #[test]
    fn test_build_unsupported_returns_error() {
        use crate::query::planning::plan::core::nodes::control_flow::control_flow_node::PassThroughNode;
        let pt = PassThroughNode::new(42);
        let node = PlanNodeEnum::PassThrough(pt);
        let mut ctx = PhysicalPlanBuildContext::new();

        let err = PhysicalPlanBuilder::build(&node, &mut ctx).unwrap_err();
        assert!(err.to_string().contains("not supported"));
    }

    #[test]
    fn test_build_then_materialize_start() {
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        use crate::query::executor::streaming::instance::{QueryBindings, QueryExecutionInstance, ResultSink};
        use std::collections::HashMap;
        use crate::query::executor::base::MemoryBudget;
        use crate::query::executor::streaming::transaction_scope::TransactionScope;

        let start = StartNode::new();
        let node = PlanNodeEnum::Start(start);
        let mut ctx = PhysicalPlanBuildContext::new();

        let plan = PhysicalPlanBuilder::build(&node, &mut ctx).unwrap();
        let plan_arc = Arc::new(plan);

        crate::query::executor::streaming::plan::validator::PhysicalPlanValidator::validate(
            &plan_arc,
        )
        .unwrap();

        let bindings = QueryBindings {
            parameters: Arc::new(HashMap::new()),
            space_name: None,
            storage: None,
            memory_budget: MemoryBudget::default_budget(),
            max_workers: 1,
            chunk_size: 1024,
            max_buffered_chunks: 4,
            query_id: 1,
            transaction: TransactionScope::None,
            #[cfg(feature = "fulltext-search")]
            fulltext_manager: None,
            #[cfg(feature = "qdrant")]
            vector_coordinator: None,
        };

        let result = QueryExecutionInstance::instantiate_plan(
            plan_arc,
            bindings,
            ResultSink::Discard,
        );
        assert!(result.is_ok());
    }
}
