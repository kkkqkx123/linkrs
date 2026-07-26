//! Recursive assembly of operators and fragment DAG edges.

use super::super::super::super::operators::spec::{
    ApplySpec, BlockingSpec, DdlSpec, FulltextSpec, GraphSpec, JoinSpec, RecursiveFragmentSpec,
    SetSpec, SinkSpec, SourceSpec, TxnSpec, UnarySpec, VectorSpec,
};
use super::super::super::super::slot::SlotLayout;
use super::super::super::properties::PhysicalProperties;
use super::super::super::types::{
    FragmentId, FragmentKind, FragmentSpec, InputContract, LogicalNodeId, OperatorKindSpec,
    PhysicalOperatorId, PhysicalOperatorIdAllocator, PhysicalOperatorSpec, StateOwnership,
};
use crate::query::executor::build_error::PlanBuildError;

use super::{ArenaFragmentAllocator, ArenaPlanAssembler};

impl ArenaPlanAssembler {
    pub(super) fn push_source_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        node_id: i64,
        spec: SourceSpec,
    ) -> (FragmentId, PhysicalOperatorId) {
        let op_id = op_alloc.allocate();
        let fid = frag_alloc.allocate();
        let output_layout = super::super::metadata::source_output_layout(&spec);
        let estimated_cardinality = super::super::metadata::estimate_source_cardinality(&spec);
        let explain_name = super::super::metadata::source_explain_name(&spec);
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: OperatorKindSpec::Source(spec),
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout,
            properties: PhysicalProperties::single_streaming(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality,
            explain_name,
        });
        fragments.push(FragmentSpec {
            id: fid,
            kind: FragmentKind::Source,
            operators: vec![op_id],
            root_operator: op_id,
            inputs: Vec::new(),
            output: None,
            exchange_layout: None,
        });
        (fid, op_id)
    }

    pub(super) fn push_unary_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut [FragmentSpec],
        op_alloc: &mut PhysicalOperatorIdAllocator,
        child_fid: FragmentId,
        node_id: i64,
        spec: UnarySpec,
        explain_name: &'static str,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let op_id = op_alloc.allocate();
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: OperatorKindSpec::Unary(spec),
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties: PhysicalProperties::single_streaming(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            explain_name,
        });
        let fragment = fragments
            .get_mut(child_fid.0)
            .ok_or_else(|| PlanBuildError::unsupported("PhysicalPlan", 0, "fragment not found"))?;
        fragment.operators.push(op_id);
        fragment.root_operator = op_id;
        Ok((child_fid, op_id))
    }

    pub(super) fn push_blocking_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut [FragmentSpec],
        op_alloc: &mut PhysicalOperatorIdAllocator,
        child_fid: FragmentId,
        node_id: i64,
        spec: BlockingSpec,
        explain_name: &'static str,
        properties: PhysicalProperties,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let op_id = op_alloc.allocate();
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: OperatorKindSpec::Blocking(spec),
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties,
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            explain_name,
        });
        let fragment = fragments
            .get_mut(child_fid.0)
            .ok_or_else(|| PlanBuildError::unsupported("PhysicalPlan", 0, "fragment not found"))?;
        fragment.operators.push(op_id);
        fragment.root_operator = op_id;
        Ok((child_fid, op_id))
    }

    pub(super) fn push_graph_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        child_fid: FragmentId,
        node_id: i64,
        spec: GraphSpec,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let op_id = op_alloc.allocate();
        let fid = frag_alloc.allocate();
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: OperatorKindSpec::Graph(spec),
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties: PhysicalProperties::single_streaming(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            explain_name: "Graph",
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

    pub(super) fn push_recursive_fragment_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        child_fid: FragmentId,
        node_id: i64,
        spec: RecursiveFragmentSpec,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let op_id = op_alloc.allocate();
        let fid = frag_alloc.allocate();
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: OperatorKindSpec::RecursiveFragment(spec),
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties: PhysicalProperties::single_streaming(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            explain_name: "RecursiveFragment",
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

    pub(super) fn push_binary_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        left_fid: FragmentId,
        right_fid: FragmentId,
        node_id: i64,
        spec: impl Into<BinaryOperatorSpec>,
        explain_name: &'static str,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let op_id = op_alloc.allocate();
        let fid = frag_alloc.allocate();
        let (op_spec, fragment_kind) = spec.into().into_parts();
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: op_spec,
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties: PhysicalProperties::single_blocking_with_budget(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            explain_name,
        });
        fragments.push(FragmentSpec {
            id: fid,
            kind: fragment_kind,
            operators: vec![op_id],
            root_operator: op_id,
            inputs: vec![left_fid, right_fid],
            output: None,
            exchange_layout: None,
        });
        Ok((fid, op_id))
    }

    pub(super) fn push_sink_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        child_fid: FragmentId,
        node_id: i64,
        spec: SinkSpec,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let op_id = op_alloc.allocate();
        let fid = frag_alloc.allocate();
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: OperatorKindSpec::Sink(spec),
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties: PhysicalProperties::single_blocking(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            explain_name: "Sink",
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

    pub(super) fn push_ddl_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        node_id: i64,
        spec: DdlSpec,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let (input_fid, _) = Self::push_source_op(
            operators,
            fragments,
            op_alloc,
            frag_alloc,
            node_id,
            SourceSpec::Start,
        );
        let op_id = op_alloc.allocate();
        let fid = frag_alloc.allocate();
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: OperatorKindSpec::Ddl(spec),
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties: PhysicalProperties::single_blocking(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            explain_name: "DDL",
        });
        fragments.push(FragmentSpec {
            id: fid,
            kind: FragmentKind::Terminal,
            operators: vec![op_id],
            root_operator: op_id,
            inputs: vec![input_fid],
            output: None,
            exchange_layout: None,
        });
        Ok((fid, op_id))
    }

    pub(super) fn push_fulltext_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        child_fid: FragmentId,
        node_id: i64,
        spec: FulltextSpec,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let op_id = op_alloc.allocate();
        let fid = frag_alloc.allocate();
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: OperatorKindSpec::Fulltext(spec),
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties: PhysicalProperties::single_streaming(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            explain_name: "Fulltext",
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

    pub(super) fn push_vector_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        child_fid: FragmentId,
        node_id: i64,
        spec: VectorSpec,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let op_id = op_alloc.allocate();
        let fid = frag_alloc.allocate();
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: OperatorKindSpec::Vector(spec),
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties: PhysicalProperties::single_streaming(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            explain_name: "Vector",
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

    pub(super) fn push_txn_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        node_id: i64,
        spec: TxnSpec,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let (input_fid, _) = Self::push_source_op(
            operators,
            fragments,
            op_alloc,
            frag_alloc,
            node_id,
            SourceSpec::Start,
        );
        let op_id = op_alloc.allocate();
        let fid = frag_alloc.allocate();
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: OperatorKindSpec::Txn(spec),
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties: PhysicalProperties::single_blocking(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            explain_name: "Txn",
        });
        fragments.push(FragmentSpec {
            id: fid,
            kind: FragmentKind::Terminal,
            operators: vec![op_id],
            root_operator: op_id,
            inputs: vec![input_fid],
            output: None,
            exchange_layout: None,
        });
        Ok((fid, op_id))
    }
}
pub(super) enum BinaryOperatorSpec {
    Join(JoinSpec),
    Set(SetSpec),
    Apply(ApplySpec),
}

impl BinaryOperatorSpec {
    fn into_parts(self) -> (OperatorKindSpec, FragmentKind) {
        match self {
            Self::Join(spec) => (OperatorKindSpec::Join(spec), FragmentKind::Streaming),
            Self::Set(spec) => (OperatorKindSpec::Set(spec), FragmentKind::Streaming),
            Self::Apply(spec) => (OperatorKindSpec::Apply(spec), FragmentKind::Streaming),
        }
    }
}

impl From<JoinSpec> for BinaryOperatorSpec {
    fn from(spec: JoinSpec) -> Self {
        Self::Join(spec)
    }
}

impl From<SetSpec> for BinaryOperatorSpec {
    fn from(spec: SetSpec) -> Self {
        Self::Set(spec)
    }
}

impl From<ApplySpec> for BinaryOperatorSpec {
    fn from(spec: ApplySpec) -> Self {
        Self::Apply(spec)
    }
}
