//! PhysicalPlanBuilder: builds an arena [`PhysicalPlan`] from planner nodes.
//!
//! This is the single builder path that replaces both the tree-based
//! [`PhysicalNode`] builder and the partitioned plan builder.
//!
//! The builder first produces a [`PhysicalNode`] tree using the existing
//! domain-specific builders, then converts it to an arena-based
//! [`PhysicalPlan`] with a proper fragment DAG.

use std::collections::HashMap;

use super::super::operators::spec::{
    ApplySpec, BlockingSpec, DdlSpec, ExchangeSpec, FulltextSpec, GraphSpec, JoinSpec,
    RecursiveFragmentSpec, SetSpec, SinkSpec, SourceSpec, TxnSpec, UnarySpec, VectorSpec,
};
use super::super::slot::{combine_layouts, SlotLayout};
use super::context::PhysicalPlanBuildContext;
use super::node::PhysicalNode;
use super::properties::{PhysicalProperties, PipelineKind};

/// Allowed call-sites for `PhysicalNode` construction and materialization.
/// Phase A audit: any production path hitting PhysicalNode outside this list
/// must be treated as a bug.
#[cfg(debug_assertions)]
const PHYSICAL_NODE_ALLOWED_SITES: &[&str] = &[
    "arena_builder.rs",       // from_physical_node conversion (legacy IR)
    "node.rs",                // definition + materialize
    // operator_plan_builder/*.rs — these construct PhysicalNode trees
    // Tests are always allowed.
];
use super::types::{
    CapabilitySet, FragmentGraph, FragmentId, FragmentKind, FragmentSpec, InputContract,
    LogicalNodeId, OperatorKindSpec, OutputContract, PhysicalOperatorId,
    PhysicalOperatorIdAllocator, PhysicalOperatorSpec, PhysicalPlan, PlanCompatibility,
};
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::executor::streaming::operator_plan_builder::build_plan_node;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

/// Builds an arena [`PhysicalPlan`] from a [`PlanNodeEnum`] tree.
pub struct PhysicalPlanBuilder;

/// Fragment ID allocator (separate from operator allocator).
struct ArenaFragmentAllocator {
    next: usize,
}

impl ArenaFragmentAllocator {
    fn new() -> Self {
        Self { next: 0 }
    }
    fn allocate(&mut self) -> FragmentId {
        let id = FragmentId(self.next);
        self.next += 1;
        id
    }
}

impl PhysicalPlanBuilder {
    /// Build a complete [`PhysicalPlan`] from a plan node.
    ///
    /// Uses the existing domain-specific `build_plan_node` to construct a
    /// [`PhysicalNode`] tree, then converts it into an arena-based plan
    /// with a proper fragment DAG.
    pub fn build(
        node: &PlanNodeEnum,
        ctx: &mut PhysicalPlanBuildContext,
        exec_ctx: &ExecutionContext,
    ) -> Result<PhysicalPlan, PlanBuildError> {
        let phys_node = build_plan_node(node, exec_ctx)?;
        Self::from_physical_node(&phys_node, ctx)
    }

    /// Convert a [`PhysicalNode`] tree into an arena [`PhysicalPlan`].
    ///
    /// Walks the tree recursively, creating operators in the arena and
    /// building a fragment DAG that mirrors the tree structure:
    /// - Source operators start new fragments
    /// - Unary operators chain in the same fragment as their child
    /// - Binary operators create new fragments with left/right inputs
    pub fn from_physical_node(
        node: &PhysicalNode,
        ctx: &PhysicalPlanBuildContext,
    ) -> Result<PhysicalPlan, PlanBuildError> {
        let mut operators: Vec<PhysicalOperatorSpec> = Vec::new();
        let mut fragments: Vec<FragmentSpec> = Vec::new();
        let mut op_alloc = PhysicalOperatorIdAllocator::new();
        let mut frag_alloc = ArenaFragmentAllocator::new();

        let (root_fid, root_op_id) = Self::convert_node(
            node,
            &mut operators,
            &mut fragments,
            &mut op_alloc,
            &mut frag_alloc,
        )?;

        Self::propagate_layouts(&mut operators, &fragments)?;

        let output = operators
            .iter()
            .find(|op| op.operator_id == root_op_id)
            .map(|op| Self::output_contract(&op.spec, op.output_layout.clone()))
            .unwrap_or_else(|| OutputContract {
                output_layout: super::super::slot::SlotLayout::new(vec![]),
                always_produces_row: false,
                nullability: Vec::new(),
                ordering: Vec::new(),
                delivery_streamable: true,
                pipeline_mode: super::types::PipelineMode::Pipelined,
            });

        let plan = PhysicalPlan {
            operators,
            logical_to_physical: HashMap::new(),
            fragments: FragmentGraph::new(fragments, root_fid),
            root_fragment: root_fid,
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

    /// Recursively convert a PhysicalNode tree to arena operators.
    ///
    /// Returns (fragment_id, root_operator_id) for the subtree.
    fn convert_node(
        node: &PhysicalNode,
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        match node {
            PhysicalNode::Source(id, spec, props) => {
                let op_id = op_alloc.allocate();
                let fid = frag_alloc.allocate();
                let output_layout = source_output_layout(spec);

                operators.push(PhysicalOperatorSpec {
                    operator_id: op_id,
                    logical_node_id: Some(LogicalNodeId(*id)),
                    spec: OperatorKindSpec::Source(spec.clone()),
                    input_contract: InputContract::NoInput,
                    input_layout: None,
                    output_layout,
                    properties: Self::convert_properties(props),
                    state_ownership: super::types::StateOwnership::TreeLocal,
                    estimated_cardinality: None,
                    explain_name: source_explain_name(spec),
                });

                fragments.push(FragmentSpec {
                    id: fid,
                    kind: fragment_kind_for_spec(spec, props),
                    operators: vec![op_id],
                    root_operator: op_id,
                    inputs: Vec::new(),
                    output: None,
                    exchange_layout: None,
                });

                Ok((fid, op_id))
            }

            PhysicalNode::Unary(id, child, spec, props) => {
                let (child_fid, _) =
                    Self::convert_node(child, operators, fragments, op_alloc, frag_alloc)?;
                let op_id = op_alloc.allocate();

                operators.push(PhysicalOperatorSpec {
                    operator_id: op_id,
                    logical_node_id: Some(LogicalNodeId(*id)),
                    spec: OperatorKindSpec::Unary(spec.clone()),
                    input_contract: InputContract::NoInput,
                    input_layout: None,
                    output_layout: SlotLayout::new(vec![]),
                    properties: Self::convert_properties(props),
                    state_ownership: super::types::StateOwnership::TreeLocal,
                    estimated_cardinality: None,
                    explain_name: unary_explain_name(spec),
                });

                let fragment = fragments.get_mut(child_fid.0).ok_or_else(|| {
                    PlanBuildError::unsupported("PhysicalPlan", 0, "fragment not found")
                })?;
                fragment.operators.push(op_id);
                fragment.root_operator = op_id;

                Ok((child_fid, op_id))
            }

            PhysicalNode::Blocking(id, child, spec, props) => {
                let (child_fid, _) =
                    Self::convert_node(child, operators, fragments, op_alloc, frag_alloc)?;
                let op_id = op_alloc.allocate();

                operators.push(PhysicalOperatorSpec {
                    operator_id: op_id,
                    logical_node_id: Some(LogicalNodeId(*id)),
                    spec: OperatorKindSpec::Blocking(spec.clone()),
                    input_contract: InputContract::NoInput,
                    input_layout: None,
                    output_layout: SlotLayout::new(vec![]),
                    properties: Self::convert_properties(props),
                    state_ownership: super::types::StateOwnership::TreeLocal,
                    estimated_cardinality: None,
                    explain_name: blocking_explain_name(spec),
                });

                let fragment = fragments.get_mut(child_fid.0).ok_or_else(|| {
                    PlanBuildError::unsupported("PhysicalPlan", 0, "fragment not found")
                })?;
                fragment.operators.push(op_id);
                fragment.root_operator = op_id;

                Ok((child_fid, op_id))
            }

            PhysicalNode::Join(id, left, right, spec, props) => {
                let (left_fid, _) =
                    Self::convert_node(left, operators, fragments, op_alloc, frag_alloc)?;
                let (right_fid, _) =
                    Self::convert_node(right, operators, fragments, op_alloc, frag_alloc)?;
                let op_id = op_alloc.allocate();
                let fid = frag_alloc.allocate();

                operators.push(PhysicalOperatorSpec {
                    operator_id: op_id,
                    logical_node_id: Some(LogicalNodeId(*id)),
                    spec: OperatorKindSpec::Join(spec.clone()),
                    input_contract: InputContract::NoInput,
                    input_layout: None,
                    output_layout: SlotLayout::new(vec![]),
                    properties: Self::convert_properties(props),
                    state_ownership: super::types::StateOwnership::TreeLocal,
                    estimated_cardinality: None,
                    explain_name: join_explain_name(spec),
                });

                fragments.push(FragmentSpec {
                    id: fid,
                    kind: FragmentKind::Streaming,
                    operators: vec![op_id],
                    root_operator: op_id,
                    inputs: vec![left_fid, right_fid],
                    output: None,
                    exchange_layout: None,
                });

                Ok((fid, op_id))
            }

            PhysicalNode::Graph(id, child, spec, props) => {
                let (child_fid, _) =
                    Self::convert_node(child, operators, fragments, op_alloc, frag_alloc)?;
                let op_id = op_alloc.allocate();
                let fid = frag_alloc.allocate();

                operators.push(PhysicalOperatorSpec {
                    operator_id: op_id,
                    logical_node_id: Some(LogicalNodeId(*id)),
                    spec: OperatorKindSpec::Graph(spec.clone()),
                    input_contract: InputContract::NoInput,
                    input_layout: None,
                    output_layout: SlotLayout::new(vec![]),
                    properties: Self::convert_properties(props),
                    state_ownership: super::types::StateOwnership::TreeLocal,
                    estimated_cardinality: None,
                    explain_name: graph_explain_name(spec),
                });

                fragments.push(FragmentSpec {
                    id: fid,
                    kind: FragmentKind::Streaming,
                    operators: vec![op_id],
                    root_operator: op_id,
                    inputs: vec![child_fid],
                    output: None,
                    exchange_layout: None,
                });

                Ok((fid, op_id))
            }

            PhysicalNode::RecursiveFragment(id, child, spec, props) => {
                let (child_fid, _) =
                    Self::convert_node(child, operators, fragments, op_alloc, frag_alloc)?;
                let op_id = op_alloc.allocate();
                let fid = frag_alloc.allocate();

                operators.push(PhysicalOperatorSpec {
                    operator_id: op_id,
                    logical_node_id: Some(LogicalNodeId(*id)),
                    spec: OperatorKindSpec::RecursiveFragment(spec.clone()),
                    input_contract: InputContract::NoInput,
                    input_layout: None,
                    output_layout: SlotLayout::new(vec![]),
                    properties: Self::convert_properties(props),
                    state_ownership: super::types::StateOwnership::TreeLocal,
                    estimated_cardinality: None,
                    explain_name: recursive_fragment_explain_name(spec),
                });

                fragments.push(FragmentSpec {
                    id: fid,
                    kind: FragmentKind::Streaming,
                    operators: vec![op_id],
                    root_operator: op_id,
                    inputs: vec![child_fid],
                    output: None,
                    exchange_layout: None,
                });

                Ok((fid, op_id))
            }

            PhysicalNode::Sink(id, child, spec, props) => {
                let (child_fid, _) =
                    Self::convert_node(child, operators, fragments, op_alloc, frag_alloc)?;
                let op_id = op_alloc.allocate();
                let fid = frag_alloc.allocate();

                operators.push(PhysicalOperatorSpec {
                    operator_id: op_id,
                    logical_node_id: Some(LogicalNodeId(*id)),
                    spec: OperatorKindSpec::Sink(spec.clone()),
                    input_contract: InputContract::NoInput,
                    input_layout: None,
                    output_layout: SlotLayout::new(vec![]),
                    properties: Self::convert_properties(props),
                    state_ownership: super::types::StateOwnership::TreeLocal,
                    estimated_cardinality: None,
                    explain_name: sink_explain_name(spec),
                });

                fragments.push(FragmentSpec {
                    id: fid,
                    kind: FragmentKind::Terminal,
                    operators: vec![op_id],
                    root_operator: op_id,
                    inputs: vec![child_fid],
                    output: None,
                    exchange_layout: None,
                });

                Ok((fid, op_id))
            }

            PhysicalNode::Set(id, left, right, spec, props) => {
                let (left_fid, _) =
                    Self::convert_node(left, operators, fragments, op_alloc, frag_alloc)?;
                let (right_fid, _) =
                    Self::convert_node(right, operators, fragments, op_alloc, frag_alloc)?;
                let op_id = op_alloc.allocate();
                let fid = frag_alloc.allocate();

                operators.push(PhysicalOperatorSpec {
                    operator_id: op_id,
                    logical_node_id: Some(LogicalNodeId(*id)),
                    spec: OperatorKindSpec::Set(spec.clone()),
                    input_contract: InputContract::NoInput,
                    input_layout: None,
                    output_layout: SlotLayout::new(vec![]),
                    properties: Self::convert_properties(props),
                    state_ownership: super::types::StateOwnership::TreeLocal,
                    estimated_cardinality: None,
                    explain_name: set_explain_name(spec),
                });

                fragments.push(FragmentSpec {
                    id: fid,
                    kind: FragmentKind::Streaming,
                    operators: vec![op_id],
                    root_operator: op_id,
                    inputs: vec![left_fid, right_fid],
                    output: None,
                    exchange_layout: None,
                });

                Ok((fid, op_id))
            }

            PhysicalNode::Apply(id, left, right, spec, props) => {
                let (left_fid, _) =
                    Self::convert_node(left, operators, fragments, op_alloc, frag_alloc)?;
                let (right_fid, _) =
                    Self::convert_node(right, operators, fragments, op_alloc, frag_alloc)?;
                let op_id = op_alloc.allocate();
                let fid = frag_alloc.allocate();

                operators.push(PhysicalOperatorSpec {
                    operator_id: op_id,
                    logical_node_id: Some(LogicalNodeId(*id)),
                    spec: OperatorKindSpec::Apply(spec.clone()),
                    input_contract: InputContract::NoInput,
                    input_layout: None,
                    output_layout: SlotLayout::new(vec![]),
                    properties: Self::convert_properties(props),
                    state_ownership: super::types::StateOwnership::TreeLocal,
                    estimated_cardinality: None,
                    explain_name: apply_explain_name(spec),
                });

                fragments.push(FragmentSpec {
                    id: fid,
                    kind: FragmentKind::Streaming,
                    operators: vec![op_id],
                    root_operator: op_id,
                    inputs: vec![left_fid, right_fid],
                    output: None,
                    exchange_layout: None,
                });

                Ok((fid, op_id))
            }

            PhysicalNode::Exchange(id, children, spec, props) => {
                let mut child_fids = Vec::new();
                for child in children {
                    let (fid, _) =
                        Self::convert_node(child, operators, fragments, op_alloc, frag_alloc)?;
                    child_fids.push(fid);
                }
                let op_id = op_alloc.allocate();
                let fid = frag_alloc.allocate();

                operators.push(PhysicalOperatorSpec {
                    operator_id: op_id,
                    logical_node_id: Some(LogicalNodeId(*id)),
                    spec: OperatorKindSpec::Exchange(spec.clone()),
                    input_contract: InputContract::NoInput,
                    input_layout: None,
                    output_layout: SlotLayout::new(vec![]),
                    properties: Self::convert_properties(props),
                    state_ownership: super::types::StateOwnership::TreeLocal,
                    estimated_cardinality: None,
                    explain_name: exchange_explain_name(spec),
                });

                fragments.push(FragmentSpec {
                    id: fid,
                    kind: FragmentKind::Exchange,
                    operators: vec![op_id],
                    root_operator: op_id,
                    inputs: child_fids,
                    output: None,
                    exchange_layout: None,
                });

                Ok((fid, op_id))
            }

            PhysicalNode::Ddl(id, child, spec, props) => {
                let (child_fid, _) =
                    Self::convert_node(child, operators, fragments, op_alloc, frag_alloc)?;
                let op_id = op_alloc.allocate();
                let fid = frag_alloc.allocate();

                operators.push(PhysicalOperatorSpec {
                    operator_id: op_id,
                    logical_node_id: Some(LogicalNodeId(*id)),
                    spec: OperatorKindSpec::Ddl(spec.clone()),
                    input_contract: InputContract::NoInput,
                    input_layout: None,
                    output_layout: SlotLayout::new(vec![]),
                    properties: Self::convert_properties(props),
                    state_ownership: super::types::StateOwnership::TreeLocal,
                    estimated_cardinality: None,
                    explain_name: ddl_explain_name(spec),
                });

                fragments.push(FragmentSpec {
                    id: fid,
                    kind: FragmentKind::Terminal,
                    operators: vec![op_id],
                    root_operator: op_id,
                    inputs: vec![child_fid],
                    output: None,
                    exchange_layout: None,
                });

                Ok((fid, op_id))
            }

            PhysicalNode::Fulltext(id, child, spec, props) => {
                let (child_fid, _) =
                    Self::convert_node(child, operators, fragments, op_alloc, frag_alloc)?;
                let op_id = op_alloc.allocate();
                let fid = frag_alloc.allocate();

                operators.push(PhysicalOperatorSpec {
                    operator_id: op_id,
                    logical_node_id: Some(LogicalNodeId(*id)),
                    spec: OperatorKindSpec::Fulltext(spec.clone()),
                    input_contract: InputContract::NoInput,
                    input_layout: None,
                    output_layout: SlotLayout::new(vec![]),
                    properties: Self::convert_properties(props),
                    state_ownership: super::types::StateOwnership::TreeLocal,
                    estimated_cardinality: None,
                    explain_name: fulltext_explain_name(spec),
                });

                fragments.push(FragmentSpec {
                    id: fid,
                    kind: FragmentKind::Terminal,
                    operators: vec![op_id],
                    root_operator: op_id,
                    inputs: vec![child_fid],
                    output: None,
                    exchange_layout: None,
                });

                Ok((fid, op_id))
            }

            PhysicalNode::Vector(id, child, spec, props) => {
                let (child_fid, _) =
                    Self::convert_node(child, operators, fragments, op_alloc, frag_alloc)?;
                let op_id = op_alloc.allocate();
                let fid = frag_alloc.allocate();

                operators.push(PhysicalOperatorSpec {
                    operator_id: op_id,
                    logical_node_id: Some(LogicalNodeId(*id)),
                    spec: OperatorKindSpec::Vector(spec.clone()),
                    input_contract: InputContract::NoInput,
                    input_layout: None,
                    output_layout: SlotLayout::new(vec![]),
                    properties: Self::convert_properties(props),
                    state_ownership: super::types::StateOwnership::TreeLocal,
                    estimated_cardinality: None,
                    explain_name: vector_explain_name(spec),
                });

                fragments.push(FragmentSpec {
                    id: fid,
                    kind: FragmentKind::Terminal,
                    operators: vec![op_id],
                    root_operator: op_id,
                    inputs: vec![child_fid],
                    output: None,
                    exchange_layout: None,
                });

                Ok((fid, op_id))
            }

            PhysicalNode::Txn(id, child, spec, props) => {
                let (child_fid, _) =
                    Self::convert_node(child, operators, fragments, op_alloc, frag_alloc)?;
                let op_id = op_alloc.allocate();
                let fid = frag_alloc.allocate();

                operators.push(PhysicalOperatorSpec {
                    operator_id: op_id,
                    logical_node_id: Some(LogicalNodeId(*id)),
                    spec: OperatorKindSpec::Txn(spec.clone()),
                    input_contract: InputContract::NoInput,
                    input_layout: None,
                    output_layout: SlotLayout::new(vec![]),
                    properties: Self::convert_properties(props),
                    state_ownership: super::types::StateOwnership::TreeLocal,
                    estimated_cardinality: None,
                    explain_name: txn_explain_name(spec),
                });

                fragments.push(FragmentSpec {
                    id: fid,
                    kind: FragmentKind::Terminal,
                    operators: vec![op_id],
                    root_operator: op_id,
                    inputs: vec![child_fid],
                    output: None,
                    exchange_layout: None,
                });

                Ok((fid, op_id))
            }
        }
    }

    fn convert_properties(props: &PhysicalProperties) -> super::properties::PhysicalProperties {
        props.clone()
    }

    fn output_contract(spec: &OperatorKindSpec, output_layout: SlotLayout) -> OutputContract {
        OutputContract {
            output_layout,
            always_produces_row: false,
            nullability: Vec::new(),
            ordering: Vec::new(),
            delivery_streamable: true,
            pipeline_mode: match spec {
                OperatorKindSpec::Blocking(_)
                | OperatorKindSpec::Exchange(
                    ExchangeSpec::Barrier | ExchangeSpec::Materialize { .. },
                ) => super::types::PipelineMode::Blocking,
                _ => super::types::PipelineMode::Pipelined,
            },
        }
    }

    /// Populate every arena operator's input and output layout after the
    /// fragment graph is assembled.  This keeps schema an immutable plan
    /// property instead of allowing executors to infer it from their first
    /// non-empty chunk.
    fn propagate_layouts(
        operators: &mut [PhysicalOperatorSpec],
        fragments: &[FragmentSpec],
    ) -> Result<(), PlanBuildError> {
        let mut layouts: HashMap<PhysicalOperatorId, SlotLayout> = operators
            .iter()
            .filter_map(|operator| match &operator.spec {
                OperatorKindSpec::Source(spec) => {
                    Some((operator.operator_id, source_output_layout(spec)))
                }
                _ => None,
            })
            .collect();

        for fragment in fragments {
            let fragment_inputs = fragment
                .inputs
                .iter()
                .map(|input_id| {
                    let root = fragments.get(input_id.0).ok_or_else(|| {
                        PlanBuildError::unsupported("PhysicalPlan", 0, "input fragment not found")
                    })?;
                    layouts.get(&root.root_operator).cloned().ok_or_else(|| {
                        PlanBuildError::unsupported("PhysicalPlan", 0, "input layout not available")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut previous = None;

            for operator_id in &fragment.operators {
                let operator = operators.get_mut(operator_id.0).ok_or_else(|| {
                    PlanBuildError::unsupported("PhysicalPlan", 0, "operator not found")
                })?;
                let input_layouts = previous
                    .clone()
                    .map(|layout| vec![layout])
                    .unwrap_or_else(|| fragment_inputs.clone());
                operator.input_layout = input_layouts.first().cloned();
                let output_layout = infer_output_layout(&operator.spec, &input_layouts);
                operator.output_layout = output_layout.clone();
                layouts.insert(*operator_id, output_layout.clone());
                previous = Some(output_layout);
            }
        }
        Ok(())
    }
}

fn input_layout(inputs: &[SlotLayout]) -> SlotLayout {
    inputs
        .first()
        .cloned()
        .unwrap_or_else(|| SlotLayout::new(Vec::new()))
}

fn layout_with_added_names(
    input: &SlotLayout,
    names: impl IntoIterator<Item = String>,
) -> SlotLayout {
    let mut all_names = input.names();
    all_names.extend(names);
    SlotLayout::from_names(&all_names)
}

fn infer_output_layout(spec: &OperatorKindSpec, inputs: &[SlotLayout]) -> SlotLayout {
    let input = input_layout(inputs);
    match spec {
        OperatorKindSpec::Source(spec) => source_output_layout(spec),
        OperatorKindSpec::Unary(UnarySpec::Project {
            output_col_names, ..
        }) => SlotLayout::from_names(output_col_names),
        OperatorKindSpec::Unary(UnarySpec::Assign { assignments }) => {
            layout_with_added_names(&input, assignments.iter().map(|(name, _)| name.clone()))
        }
        OperatorKindSpec::Unary(UnarySpec::Remove { columns_to_remove }) => {
            let names = input
                .names()
                .into_iter()
                .filter(|name| !columns_to_remove.contains(name))
                .collect::<Vec<_>>();
            SlotLayout::from_names(&names)
        }
        OperatorKindSpec::Unary(UnarySpec::AppendVertices { vertex_properties }) => {
            layout_with_added_names(
                &input,
                vertex_properties.iter().map(|(name, _)| name.clone()),
            )
        }
        OperatorKindSpec::Blocking(
            BlockingSpec::Aggregate {
                output_col_names, ..
            }
            | BlockingSpec::PartialAggregate {
                output_col_names, ..
            }
            | BlockingSpec::FinalAggregate {
                output_col_names, ..
            },
        ) => SlotLayout::from_names(output_col_names),
        OperatorKindSpec::Blocking(BlockingSpec::RollUpApply { rollup_expressions }) => {
            layout_with_added_names(
                &input,
                (0..rollup_expressions.len()).map(|index| format!("rollup_{index}")),
            )
        }
        OperatorKindSpec::Join(JoinSpec::SemiJoin { .. }) => input,
        OperatorKindSpec::Join(_) => inputs
            .get(1)
            .map(|right| combine_layouts(&input, right))
            .unwrap_or(input),
        OperatorKindSpec::Apply(ApplySpec::Apply {
            kind:
                super::super::operators::spec::ApplyKind::Standard
                | super::super::operators::spec::ApplyKind::Single,
            ..
        }) => inputs
            .get(1)
            .map(|right| combine_layouts(&input, right))
            .unwrap_or(input),
        OperatorKindSpec::Apply(ApplySpec::RollUpApply { collect_column, .. }) => {
            layout_with_added_names(
                &input,
                [collect_column
                    .clone()
                    .unwrap_or_else(|| "rollup".to_string())],
            )
        }
        OperatorKindSpec::Apply(ApplySpec::PatternApply { .. })
        | OperatorKindSpec::Apply(ApplySpec::Apply { .. })
        | OperatorKindSpec::Set(_)
        | OperatorKindSpec::Exchange(_)
        | OperatorKindSpec::Sink(_)
        | OperatorKindSpec::Ddl(_)
        | OperatorKindSpec::Txn(_)
        | OperatorKindSpec::Unary(_)
        | OperatorKindSpec::Blocking(_) => input,
        OperatorKindSpec::Graph(GraphSpec::Expand { col_names, .. } | GraphSpec::ExpandAll { col_names, .. }) => {
            if col_names.len() == 3 {
                SlotLayout::from_names(col_names)
            } else {
                layout_with_added_names(
                    &input,
                    [
                        "_expand_edge".to_string(),
                        "_expand_dst".to_string(),
                    ],
                )
            }
        }
        OperatorKindSpec::Graph(GraphSpec::Traverse { .. }) => layout_with_added_names(
            &input,
            [
                "_traverse_vertex".to_string(),
                "_traverse_edge_type".to_string(),
                "_traverse_direction".to_string(),
                "_traverse_depth".to_string(),
            ],
        ),
        OperatorKindSpec::Graph(GraphSpec::ShortestPath { .. }) => {
            layout_with_added_names(&input, ["_shortest_path".to_string()])
        }
        OperatorKindSpec::Graph(GraphSpec::BFSShortest { .. }) => {
            layout_with_added_names(&input, ["_bfs_shortest".to_string()])
        }
        OperatorKindSpec::Graph(GraphSpec::AllPaths { .. }) => {
            layout_with_added_names(&input, ["_all_paths".to_string()])
        }
        OperatorKindSpec::Graph(GraphSpec::MultiShortestPath { .. }) => {
            layout_with_added_names(&input, ["_multi_shortest_path".to_string()])
        }
        OperatorKindSpec::RecursiveFragment(RecursiveFragmentSpec::ShortestPath { .. }) => {
            layout_with_added_names(&input, ["_shortest_path".to_string()])
        }
        OperatorKindSpec::RecursiveFragment(RecursiveFragmentSpec::MultiShortestPath {
            ..
        }) => layout_with_added_names(&input, ["_multi_shortest_path".to_string()]),
        OperatorKindSpec::RecursiveFragment(RecursiveFragmentSpec::BFSShortest { .. }) => {
            layout_with_added_names(&input, ["_bfs_path".to_string()])
        }
        OperatorKindSpec::RecursiveFragment(RecursiveFragmentSpec::AllPaths { .. }) => {
            layout_with_added_names(&input, ["_all_paths".to_string()])
        }
        OperatorKindSpec::Graph(GraphSpec::BiExpand { .. } | GraphSpec::BiTraverse { .. }) => input,
        OperatorKindSpec::Fulltext(FulltextSpec::FulltextManage { .. })
        | OperatorKindSpec::Vector(VectorSpec::VectorManage { .. }) => SlotLayout::from_names(&[
            "action".to_string(),
            "name".to_string(),
            "status".to_string(),
        ]),
        OperatorKindSpec::Fulltext(
            FulltextSpec::FulltextSearch { .. }
            | FulltextSpec::FulltextLookup { .. }
            | FulltextSpec::MatchFulltext { .. },
        ) => SlotLayout::from_names(&["doc_id".to_string(), "score".to_string()]),
        OperatorKindSpec::Vector(
            VectorSpec::VectorSearch { .. } | VectorSpec::VectorMatch { .. },
        ) => SlotLayout::from_names(&["id".to_string(), "score".to_string()]),
        OperatorKindSpec::Vector(VectorSpec::VectorLookup { .. }) => input,
    }
}

// ── Helper functions ──

fn source_output_layout(spec: &SourceSpec) -> SlotLayout {
    match spec {
        SourceSpec::Start => SlotLayout::new(vec![]),
        SourceSpec::Argument => SlotLayout::new(vec![]),
        SourceSpec::ScanVertices { col_names, .. } => SlotLayout::from_names(col_names),
        SourceSpec::StorageScanVertices { col_names, .. } => SlotLayout::from_names(col_names),
        SourceSpec::ScanEdges { col_names, .. } => SlotLayout::from_names(col_names),
        SourceSpec::StorageScanEdges { col_names, .. } => SlotLayout::from_names(col_names),
        SourceSpec::GetVertices { .. } => SlotLayout::from_names(&["vertex".to_string()]),
        SourceSpec::GetEdges { .. } => SlotLayout::from_names(&["edge".to_string()]),
        SourceSpec::GetNeighbors { .. } => SlotLayout::from_names(&["vertex".to_string()]),
        SourceSpec::EdgeIndexScan { .. } => SlotLayout::new(vec![]),
        SourceSpec::IndexScan { output_layout, .. } => (**output_layout).clone(),
        SourceSpec::LookupIndex { output_layout, .. } => (**output_layout).clone(),
        SourceSpec::GetProp { output_layout, .. } => (**output_layout).clone(),
    }
}

fn source_explain_name(spec: &SourceSpec) -> &'static str {
    match spec {
        SourceSpec::Start => "Start",
        SourceSpec::Argument => "Argument",
        SourceSpec::ScanVertices { .. } => "ScanVertices",
        SourceSpec::StorageScanVertices { .. } => "StorageScanVertices",
        SourceSpec::ScanEdges { .. } => "ScanEdges",
        SourceSpec::StorageScanEdges { .. } => "StorageScanEdges",
        SourceSpec::GetVertices { .. } => "GetVertices",
        SourceSpec::GetEdges { .. } => "GetEdges",
        SourceSpec::GetNeighbors { .. } => "GetNeighbors",
        SourceSpec::EdgeIndexScan { .. } => "EdgeIndexScan",
        SourceSpec::IndexScan { .. } => "IndexScan",
        SourceSpec::LookupIndex { .. } => "LookupIndex",
        SourceSpec::GetProp { .. } => "GetProp",
    }
}

fn unary_explain_name(spec: &UnarySpec) -> &'static str {
    match spec {
        UnarySpec::Filter { .. } => "Filter",
        UnarySpec::Project { .. } => "Project",
        UnarySpec::Limit { .. } => "Limit",
        UnarySpec::Assign { .. } => "Assign",
        UnarySpec::Remove { .. } => "Remove",
        UnarySpec::Unwind { .. } => "Unwind",
        UnarySpec::AppendVertices { .. } => "AppendVertices",
        UnarySpec::Sample { .. } => "Sample",
    }
}

fn blocking_explain_name(spec: &BlockingSpec) -> &'static str {
    match spec {
        BlockingSpec::Sort { .. } => "Sort",
        BlockingSpec::Aggregate { .. } => "Aggregate",
        BlockingSpec::GroupBy { .. } => "GroupBy",
        BlockingSpec::WindowFunction { .. } => "WindowFunction",
        BlockingSpec::Window { .. } => "Window",
        BlockingSpec::TopN { .. } => "TopN",
        BlockingSpec::Distinct => "Distinct",
        BlockingSpec::Materialize => "Materialize",
        BlockingSpec::DataCollect => "DataCollect",
        BlockingSpec::RollUpApply { .. } => "RollUpApply",
        BlockingSpec::PartialAggregate { .. } => "PartialAggregate",
        BlockingSpec::FinalAggregate { .. } => "FinalAggregate",
    }
}

fn join_explain_name(spec: &JoinSpec) -> &'static str {
    match spec {
        JoinSpec::InnerJoin { .. } => "InnerJoin",
        JoinSpec::LeftJoin { .. } => "LeftJoin",
        JoinSpec::RightJoin { .. } => "RightJoin",
        JoinSpec::FullOuterJoin { .. } => "FullOuterJoin",
        JoinSpec::CrossJoin => "CrossJoin",
        JoinSpec::SemiJoin { .. } => "SemiJoin",
        JoinSpec::HashJoin { .. } => "HashJoin",
        JoinSpec::HashLeftJoin { .. } => "HashLeftJoin",
        JoinSpec::NestedLoopJoin { .. } => "NestedLoopJoin",
    }
}

fn graph_explain_name(spec: &GraphSpec) -> &'static str {
    match spec {
        GraphSpec::Expand { .. } => "Expand",
        GraphSpec::ExpandAll { .. } => "ExpandAll",
        GraphSpec::Traverse { .. } => "Traverse",
        GraphSpec::BiExpand { .. } => "BiExpand",
        GraphSpec::BiTraverse { .. } => "BiTraverse",
        GraphSpec::ShortestPath { .. } => "ShortestPath",
        GraphSpec::BFSShortest { .. } => "BFSShortest",
        GraphSpec::AllPaths { .. } => "AllPaths",
        GraphSpec::MultiShortestPath { .. } => "MultiShortestPath",
    }
}

fn recursive_fragment_explain_name(spec: &RecursiveFragmentSpec) -> &'static str {
    match spec {
        RecursiveFragmentSpec::ShortestPath { .. } => "RecursiveShortestPath",
        RecursiveFragmentSpec::MultiShortestPath { .. } => "RecursiveMultiShortestPath",
        RecursiveFragmentSpec::BFSShortest { .. } => "RecursiveBFSShortest",
        RecursiveFragmentSpec::AllPaths { .. } => "RecursiveAllPaths",
    }
}

fn sink_explain_name(spec: &SinkSpec) -> &'static str {
    match spec {
        SinkSpec::InsertVertices { .. } => "InsertVertices",
        SinkSpec::InsertEdges { .. } => "InsertEdges",
        SinkSpec::UpdateVertices { .. } => "UpdateVertices",
        SinkSpec::UpdateEdges { .. } => "UpdateEdges",
        SinkSpec::DeleteVertices { .. } => "DeleteVertices",
        SinkSpec::DeleteEdges { .. } => "DeleteEdges",
        SinkSpec::PipeDeleteVertices { .. } => "PipeDeleteVertices",
        SinkSpec::PipeDeleteEdges { .. } => "PipeDeleteEdges",
        SinkSpec::DeleteTags { .. } => "DeleteTags",
    }
}

fn set_explain_name(spec: &SetSpec) -> &'static str {
    match spec {
        SetSpec::Union => "Union",
        SetSpec::UnionAll => "UnionAll",
        SetSpec::Intersect => "Intersect",
        SetSpec::Except => "Except",
        SetSpec::Minus => "Minus",
    }
}

fn apply_explain_name(spec: &ApplySpec) -> &'static str {
    match spec {
        ApplySpec::Apply { .. } => "Apply",
        ApplySpec::PatternApply { .. } => "PatternApply",
        ApplySpec::RollUpApply { .. } => "RollUpApply",
    }
}

fn exchange_explain_name(spec: &ExchangeSpec) -> &'static str {
    match spec {
        ExchangeSpec::Concatenate { .. } => "Concatenate",
        ExchangeSpec::MergeSort { .. } => "MergeSort",
        ExchangeSpec::RepartitionHash { .. } => "RepartitionHash",
        ExchangeSpec::Broadcast { .. } => "Broadcast",
        ExchangeSpec::Barrier => "Barrier",
        ExchangeSpec::Materialize { .. } => "Materialize",
    }
}

fn ddl_explain_name(spec: &DdlSpec) -> &'static str {
    match spec {
        DdlSpec::SpaceManage { .. } => "SpaceManage",
        DdlSpec::TagManage { .. } => "TagManage",
        DdlSpec::EdgeManage { .. } => "EdgeManage",
        DdlSpec::IndexManage { .. } => "IndexManage",
        DdlSpec::DeleteIndex { .. } => "DeleteIndex",
        DdlSpec::UserManage { .. } => "UserManage",
        DdlSpec::ShowStats { .. } => "ShowStats",
        DdlSpec::Analyze { .. } => "Analyze",
        DdlSpec::Migrate { .. } => "Migrate",
    }
}

fn fulltext_explain_name(spec: &FulltextSpec) -> &'static str {
    match spec {
        FulltextSpec::FulltextManage { .. } => "FulltextManage",
        FulltextSpec::FulltextSearch { .. } => "FulltextSearch",
        FulltextSpec::FulltextLookup { .. } => "FulltextLookup",
        FulltextSpec::MatchFulltext { .. } => "MatchFulltext",
    }
}

fn vector_explain_name(spec: &VectorSpec) -> &'static str {
    match spec {
        VectorSpec::VectorManage { .. } => "VectorManage",
        VectorSpec::VectorSearch { .. } => "VectorSearch",
        VectorSpec::VectorLookup { .. } => "VectorLookup",
        VectorSpec::VectorMatch { .. } => "VectorMatch",
    }
}

fn txn_explain_name(spec: &TxnSpec) -> &'static str {
    match spec {
        TxnSpec::BeginTransaction => "BeginTransaction",
        TxnSpec::Commit => "Commit",
        TxnSpec::Rollback => "Rollback",
    }
}

fn fragment_kind_for_spec(_spec: &SourceSpec, props: &PhysicalProperties) -> FragmentKind {
    match props.pipeline_kind {
        PipelineKind::Blocking => FragmentKind::Blocking,
        _ => FragmentKind::Source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
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
        let source = PhysicalNode::Source(
            1,
            SourceSpec::ScanVertices {
                rows: vec![vec![crate::core::Value::Int(1)]],
                col_names: vec!["vertex_id".to_string()],
            },
            PhysicalProperties::single_streaming(),
        );
        let node = PhysicalNode::Blocking(
            2,
            Box::new(source),
            BlockingSpec::Materialize,
            PhysicalProperties::single_blocking_with_budget(),
        );
        let ctx = PhysicalPlanBuildContext::new();

        let plan = PhysicalPlanBuilder::from_physical_node(&node, &ctx).unwrap();
        let source = plan.operator(PhysicalOperatorId(0)).unwrap();
        let materialize = plan.operator(PhysicalOperatorId(1)).unwrap();

        assert_eq!(source.output_layout.names(), vec!["vertex_id"]);
        assert_eq!(
            materialize.input_layout.as_ref().unwrap().names(),
            vec!["vertex_id"]
        );
        assert_eq!(materialize.output_layout.names(), vec!["vertex_id"]);
        assert_eq!(plan.output.output_layout.names(), vec!["vertex_id"]);
        assert_eq!(
            plan.output.pipeline_mode,
            super::super::types::PipelineMode::Blocking
        );
    }
}
