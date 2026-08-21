//! Recursive assembly of operators and fragment DAG edges.

use super::super::super::super::operators::spec::{
    ApplySpec, BlockingSpec, DdlSpec, ExchangeSpec, FactorizedSpec, FulltextSpec, GraphSpec,
    JoinSpec, RecursiveFragmentSpec, SetSpec, SinkSpec, SourceSpec, TxnSpec, UnarySpec, VectorSpec,
};
use super::super::super::super::slot::SlotLayout;
use super::super::super::properties::PhysicalProperties;
use super::super::super::types::{
    FragmentId, FragmentKind, FragmentSpec, InputContract, LogicalNodeId, OperatorKindSpec,
    PhysicalOperatorId, PhysicalOperatorIdAllocator, PhysicalOperatorSpec, StateOwnership,
};
use crate::query::executor::build_error::PlanBuildError;

use super::{ArenaFragmentAllocator, ArenaPlanAssembler};

/// Mutable builder handles shared by the per-fragment push helpers.
pub(crate) struct FragmentCtx<'a> {
    pub(crate) operators: &'a mut Vec<PhysicalOperatorSpec>,
    pub(crate) fragments: &'a mut Vec<FragmentSpec>,
    pub(crate) op_alloc: &'a mut PhysicalOperatorIdAllocator,
}

/// Parameters for creating a hash-repartition exchange fragment.
#[allow(dead_code)]
pub(crate) struct HashExchangeParams {
    pub(crate) partition_fids: Vec<FragmentId>,
    pub(crate) partition_count: usize,
    pub(crate) hash_expressions: Vec<crate::core::types::expr::Expression>,
    pub(crate) input_layout: Option<SlotLayout>,
    pub(crate) output_layout: Option<SlotLayout>,
}

impl ArenaPlanAssembler {
    pub(crate) fn push_source_op(
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
            choice_reason: None,
            has_folded_expressions: false,
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

    pub(crate) fn push_unary_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut [FragmentSpec],
        op_alloc: &mut PhysicalOperatorIdAllocator,
        child_fid: FragmentId,
        node_id: i64,
        spec: UnarySpec,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let op_id = op_alloc.allocate();
        let explain_name = super::super::metadata::unary_explain_name(&spec);
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
            choice_reason: None,
            has_folded_expressions: false,
            explain_name,
        });
        let fragment = fragments
            .get_mut(child_fid.0)
            .ok_or_else(|| PlanBuildError::unsupported("PhysicalPlan", 0, "fragment not found"))?;
        fragment.operators.push(op_id);
        fragment.root_operator = op_id;
        Ok((child_fid, op_id))
    }

    pub(crate) fn push_blocking_op(
        ctx: &mut FragmentCtx,
        child_fid: FragmentId,
        node_id: i64,
        spec: BlockingSpec,
        properties: PhysicalProperties,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let op_id = ctx.op_alloc.allocate();
        let explain_name = super::super::metadata::blocking_explain_name(&spec);
        ctx.operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: OperatorKindSpec::Blocking(spec),
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties,
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            choice_reason: None,
            has_folded_expressions: false,
            explain_name,
        });
        let fragment = ctx
            .fragments
            .get_mut(child_fid.0)
            .ok_or_else(|| PlanBuildError::unsupported("PhysicalPlan", 0, "fragment not found"))?;
        fragment.operators.push(op_id);
        fragment.root_operator = op_id;
        Ok((child_fid, op_id))
    }

    /// Create the gather/exchange fragment that consumes one executor per
    /// partition through a `PartitionedInputs` contract.
    pub(crate) fn push_exchange_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        partition_fids: Vec<FragmentId>,
        partition_count: usize,
    ) -> (FragmentId, PhysicalOperatorId) {
        let op_id = op_alloc.allocate();
        let fid = frag_alloc.allocate();
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: None,
            spec: OperatorKindSpec::Exchange(ExchangeSpec::Concatenate { partition_count }),
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties: PhysicalProperties::single_blocking(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            choice_reason: None,
            has_folded_expressions: false,
            explain_name: "Exchange",
        });
        fragments.push(FragmentSpec {
            id: fid,
            kind: FragmentKind::Exchange,
            operators: vec![op_id],
            root_operator: op_id,
            inputs: partition_fids,
            output: None,
            exchange_layout: None,
        });
        (fid, op_id)
    }

    /// Create a hash-repartition exchange fragment that redistributes rows
    /// by hash of key expressions into `partition_count` buckets.
    ///
    /// Used by E1b for co-partition joins where the join key differs from
    /// the partition key, requiring a shuffle to align partitions.
    #[allow(dead_code)]
    pub(crate) fn push_hash_exchange_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        params: HashExchangeParams,
    ) -> (FragmentId, PhysicalOperatorId) {
        let op_id = op_alloc.allocate();
        let fid = frag_alloc.allocate();
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: None,
            spec: OperatorKindSpec::Exchange(ExchangeSpec::RepartitionHash {
                num_partitions: params.partition_count,
                hash_expressions: params.hash_expressions,
                input_layout: params.input_layout,
                output_layout: params.output_layout,
            }),
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties: PhysicalProperties::single_blocking(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            choice_reason: None,
            has_folded_expressions: false,
            explain_name: "HashExchange",
        });
        fragments.push(FragmentSpec {
            id: fid,
            kind: FragmentKind::Exchange,
            operators: vec![op_id],
            root_operator: op_id,
            inputs: params.partition_fids,
            output: None,
            exchange_layout: None,
        });
        (fid, op_id)
    }

    /// Create a new fragment holding one global unary operator that consumes
    /// `child_fid` (the partition exchange fragment).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_global_unary_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        child_fid: FragmentId,
        node_id: i64,
        spec: UnarySpec,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let op_id = op_alloc.allocate();
        let fid = frag_alloc.allocate();
        let explain_name = super::super::metadata::unary_explain_name(&spec);
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
            choice_reason: None,
            has_folded_expressions: false,
            explain_name,
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

    /// Create a new fragment holding one global blocking operator that
    /// consumes `child_fid` (the partition exchange fragment).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_global_blocking_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        child_fid: FragmentId,
        node_id: i64,
        spec: BlockingSpec,
        properties: PhysicalProperties,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let op_id = op_alloc.allocate();
        let fid = frag_alloc.allocate();
        let explain_name = super::super::metadata::blocking_explain_name(&spec);
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
            choice_reason: None,
            has_folded_expressions: false,
            explain_name,
        });
        fragments.push(FragmentSpec {
            id: fid,
            kind: FragmentKind::Blocking,
            operators: vec![op_id],
            root_operator: op_id,
            inputs: vec![child_fid],
            output: None,
            exchange_layout: None,
        });
        Ok((fid, op_id))
    }

    pub(crate) fn push_graph_op(
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
        let explain_name = super::super::metadata::graph_explain_name(&spec);
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
            choice_reason: None,
            has_folded_expressions: false,
            explain_name,
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
        let explain_name = super::super::metadata::recursive_fragment_explain_name(&spec);
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
            choice_reason: None,
            has_folded_expressions: false,
            explain_name,
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

    pub(crate) fn push_binary_op(
        ctx: &mut FragmentCtx,
        frag_alloc: &mut ArenaFragmentAllocator,
        left_fid: FragmentId,
        right_fid: FragmentId,
        node_id: i64,
        spec: impl Into<BinaryOperatorSpec>,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let op_id = ctx.op_alloc.allocate();
        let fid = frag_alloc.allocate();
        let binary_spec: BinaryOperatorSpec = spec.into();
        let explain_name = match &binary_spec {
            BinaryOperatorSpec::Join(spec) => super::super::metadata::join_explain_name(spec),
            BinaryOperatorSpec::Set(spec) => super::super::metadata::set_explain_name(spec),
            BinaryOperatorSpec::Apply(spec) => super::super::metadata::apply_explain_name(spec),
        };
        let (op_spec, fragment_kind) = binary_spec.into_parts();
        ctx.operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: op_spec,
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties: PhysicalProperties::single_blocking_with_budget(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            choice_reason: None,
            has_folded_expressions: false,
            explain_name,
        });
        ctx.fragments.push(FragmentSpec {
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

    /// Create a new fragment holding one apply operator that consumes only
    /// `child_fid` (the left/outer input).  Used by unary-input apply
    /// operators such as `CorrelatedApply`, whose right subtree is embedded
    /// in the spec and rebuilt per row at runtime.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_apply_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        child_fid: FragmentId,
        node_id: i64,
        spec: ApplySpec,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let op_id = op_alloc.allocate();
        let fid = frag_alloc.allocate();
        let explain_name = super::super::metadata::apply_explain_name(&spec);
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: OperatorKindSpec::Apply(spec),
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties: PhysicalProperties::single_streaming(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            choice_reason: None,
            has_folded_expressions: false,
            explain_name,
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
        let explain_name = super::super::metadata::sink_explain_name(&spec);
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
            choice_reason: None,
            has_folded_expressions: false,
            explain_name,
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
        let explain_name = super::super::metadata::ddl_explain_name(&spec);
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
            choice_reason: None,
            has_folded_expressions: false,
            explain_name,
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
        let explain_name = super::super::metadata::fulltext_explain_name(&spec);
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
            choice_reason: None,
            has_folded_expressions: false,
            explain_name,
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
        let explain_name = super::super::metadata::vector_explain_name(&spec);
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
            choice_reason: None,
            has_folded_expressions: false,
            explain_name,
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
        let explain_name = super::super::metadata::txn_explain_name(&spec);
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: OperatorKindSpec::Txn(spec),
            input_contract: InputContract::NoInput,
            input_layout: None,
            // Transaction commands produce a structured two-column result
            // (command / result) through [`TransactionCommandResult`].
            output_layout: SlotLayout::from_names(&["command".to_string(), "result".to_string()]),
            properties: PhysicalProperties::single_blocking(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            choice_reason: None,
            has_folded_expressions: false,
            explain_name,
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

    pub(crate) fn push_factorized_op(
        operators: &mut Vec<PhysicalOperatorSpec>,
        fragments: &mut Vec<FragmentSpec>,
        op_alloc: &mut PhysicalOperatorIdAllocator,
        frag_alloc: &mut ArenaFragmentAllocator,
        child_fid: FragmentId,
        node_id: i64,
        spec: FactorizedSpec,
    ) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
        let op_id = op_alloc.allocate();
        let fid = frag_alloc.allocate();
        let explain_name = super::super::metadata::factorized_explain_name(&spec);
        let kind = match &spec {
            FactorizedSpec::MultiplicityReducer { .. } => FragmentKind::Blocking,
            _ => FragmentKind::Streaming,
        };
        operators.push(PhysicalOperatorSpec {
            operator_id: op_id,
            logical_node_id: Some(LogicalNodeId(node_id)),
            spec: OperatorKindSpec::Factorized(spec),
            input_contract: InputContract::NoInput,
            input_layout: None,
            output_layout: SlotLayout::new(vec![]),
            properties: PhysicalProperties::single_streaming(),
            state_ownership: StateOwnership::TreeLocal,
            estimated_cardinality: None,
            choice_reason: None,
            has_folded_expressions: false,
            explain_name,
        });
        fragments.push(FragmentSpec {
            id: fid,
            kind,
            operators: vec![op_id],
            root_operator: op_id,
            inputs: vec![child_fid],
            output: None,
            exchange_layout: None,
        });
        Ok((fid, op_id))
    }
}
pub(crate) enum BinaryOperatorSpec {
    Join(JoinSpec),
    Set(SetSpec),
    Apply(ApplySpec),
}

impl BinaryOperatorSpec {
    pub(super) fn into_parts(self) -> (OperatorKindSpec, FragmentKind) {
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
