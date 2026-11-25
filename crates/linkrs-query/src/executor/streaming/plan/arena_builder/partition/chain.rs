use super::super::super::super::operators::spec::{BlockingSpec, SourceSpec, UnarySpec};
use super::super::super::properties::PhysicalProperties;
use super::super::super::types::{
    FragmentId, FragmentSpec, PhysicalOperatorId, PhysicalOperatorIdAllocator, PhysicalOperatorSpec,
};
use super::super::assembler::{ArenaFragmentAllocator, ArenaPlanAssembler, FragmentCtx};
use super::super::specs::{
    build_expand_all_spec_with_flags, build_filter_spec, build_flatten_spec, build_project_spec,
    build_source_spec, build_topn_spec, count_only_expand_below, is_count_only_aggregate,
    COUNT_ONLY_COLUMN,
};
use super::global::push_global_op;
use std::sync::Arc;

use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::executor::streaming::slot::SlotLayout;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, PlanNode, SingleInputNode,
};
use crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
use linkrs_core::types::expr::Expression;
use linkrs_core::types::operators::AggregateFunction;
use linkrs_core::{DataType, EdgeDirection};

/// Result of decomposing the logical root into a partitionable chain.
pub(super) struct PartitionedChain<'a> {
    /// The single tagged vertex scan at the bottom of the chain.
    pub(super) scan: &'a PlanNodeEnum,
    /// Filter/Project operators between the scan and the first global
    /// operator, ordered scan-first.  Copied into every partition fragment.
    pub(super) local: Vec<&'a PlanNodeEnum>,
    /// Operators from the first global operator up to the root, ordered
    /// scan-first.  Applied above the partition exchange.
    pub(super) global: Vec<&'a PlanNodeEnum>,
    /// The Aggregate node that should be split into partial+final phases,
    /// when it is the first global operator and all its functions support
    /// partial accumulation.
    pub(super) aggregate_split: Option<&'a AggregateNode>,
}
/// How one partition-local scan copy divides the table among fragments.
///
/// Value ranges filter by external id; every copy still walks the whole
/// table, so ranges divide output rows but never enumeration work. Shard
/// slices divide the physical enumeration space, so the union of copies
/// costs a single full walk. Co-partitioned joins need value ranges (both
/// sides must observe the same key range); every other partitioned scan
/// uses shard slices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScanDivision {
    ByValueRange,
    ByShardSlice,
}

/// Build one partition group for a linear scan chain: one local fragment per
/// partition (scan + local Filter/Project + optional PartialAggregate), a
/// Concatenate exchange, then the chain's global operators (including a
/// FinalAggregate for split aggregates). Returns the group root.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_chain_group(
    operators: &mut Vec<PhysicalOperatorSpec>,
    fragments: &mut Vec<FragmentSpec>,
    op_alloc: &mut PhysicalOperatorIdAllocator,
    frag_alloc: &mut ArenaFragmentAllocator,
    chain: &PartitionedChain,
    scan: &PlanNodeEnum,
    spec: &crate::planning::plan::PartitionSpec,
    exec_ctx: &ExecutionContext,
) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
    // 1. One local fragment per partition: StorageScanVertices bound to the
    //    partition's shard slice (or StorageScanEdges bound to a source-id
    //    shard slice for edge scans), followed by the local Filter/Project
    //    pipeline and, for split aggregates, the PartialAggregate phase.
    let partition_fids = build_partition_local_fragments(
        operators,
        fragments,
        op_alloc,
        frag_alloc,
        chain,
        scan,
        spec,
        ScanDivision::ByShardSlice,
        exec_ctx,
    )?;

    // 2. Exchange fragment: Concatenate over all partition fragments.
    let mut child_fid = ArenaPlanAssembler::push_exchange_op(
        operators,
        fragments,
        op_alloc,
        frag_alloc,
        partition_fids,
        spec.partition_count(),
    )
    .0;

    // 3. Global operators above the exchange, scan-first order.
    for (index, op) in chain.global.iter().enumerate() {
        if index == 0 {
            if let Some(agg) = chain.aggregate_split {
                let (_, final_spec) = split_aggregate(agg);
                child_fid = ArenaPlanAssembler::push_global_blocking_op(
                    operators,
                    fragments,
                    op_alloc,
                    frag_alloc,
                    child_fid,
                    agg.id(),
                    final_spec,
                    PhysicalProperties::single_blocking_with_budget(),
                )?
                .0;
                continue;
            }
        }
        child_fid = push_global_op(
            operators, fragments, op_alloc, frag_alloc, child_fid, op, exec_ctx,
        )?
        .0;
    }

    let root_operator = fragments
        .get(child_fid.0)
        .map(|f| f.root_operator)
        .ok_or_else(|| PlanBuildError::unsupported("PhysicalPlan", 0, "root fragment missing"))?;

    Ok((child_fid, root_operator))
}

/// Build one partition-local scan chain fragment per configured range: the
/// storage scan bound to that partition plus the chain's local operators
/// (Filter/Project/ExpandAll) and, for split aggregates, the PartialAggregate
/// phase. Returns the fragment id per range in partition order.
///
/// Shared by the single-chain [`build_chain_group`], the E1b co-partition
/// direct join (which pairs the left/right partition fragments by index and
/// needs value ranges on both sides), and the hash-exchange join (whose
/// exchange repartitions by key, so any disjoint division works).
#[allow(clippy::too_many_arguments)]
pub(super) fn build_partition_local_fragments(
    operators: &mut Vec<PhysicalOperatorSpec>,
    fragments: &mut Vec<FragmentSpec>,
    op_alloc: &mut PhysicalOperatorIdAllocator,
    frag_alloc: &mut ArenaFragmentAllocator,
    chain: &PartitionedChain,
    scan: &PlanNodeEnum,
    spec: &crate::planning::plan::PartitionSpec,
    division: ScanDivision,
    exec_ctx: &ExecutionContext,
) -> Result<Vec<FragmentId>, PlanBuildError> {
    let mut partition_fids = Vec::with_capacity(spec.partition_count());
    let packed_key = packed_expand_key(chain, exec_ctx);
    let packed_key_name = packed_key.as_ref().map(|key| key.name.clone());
    let packed_identity_type = packed_key.and_then(|key| key.identity_type);
    let packed_expand_id = chain
        .local
        .iter()
        .rev()
        .find(|op| matches!(op, PlanNodeEnum::ExpandAll(_)))
        .map(|op| op.id());
    for (index, range) in spec.ranges().iter().enumerate() {
        let mut scan_spec = build_source_spec(scan, exec_ctx)?;
        match &mut scan_spec {
            SourceSpec::StorageScanVertices {
                partition_range,
                shard_slice,
                ..
            } => match division {
                ScanDivision::ByValueRange => *partition_range = Some(range.clone()),
                ScanDivision::ByShardSlice => {
                    *shard_slice = Some(crate::storage::ShardSlice::new(
                        index,
                        spec.partition_count(),
                    ));
                }
            },
            SourceSpec::StorageScanEdges { shard_slice, .. } => {
                *shard_slice = Some(crate::storage::ShardSlice::new(
                    index,
                    spec.partition_count(),
                ));
            }
            _ => {
                return Err(PlanBuildError::unsupported(
                    "PhysicalPlan",
                    scan.id(),
                    "partitioned scan must lower to a storage vertex or edge scan",
                ));
            }
        }
        let (mut fid, _) = ArenaPlanAssembler::push_source_op(
            operators,
            fragments,
            op_alloc,
            frag_alloc,
            scan.id(),
            scan_spec,
        );
        for op in &chain.local {
            match op {
                PlanNodeEnum::Filter(filter) => {
                    let subquery_runners = super::super::assembler::build_subquery_runner_specs(
                        filter.subqueries(),
                        exec_ctx,
                    )?;
                    let spec = build_filter_spec(filter, subquery_runners)?;
                    let (new_fid, op_id) = ArenaPlanAssembler::push_unary_op(
                        operators,
                        fragments,
                        op_alloc,
                        fid,
                        op.id(),
                        spec,
                    )?;
                    operators[op_id.0].has_folded_expressions = filter.has_folded_expressions();
                    fid = new_fid;
                }
                PlanNodeEnum::Project(project) => {
                    let subquery_runners = super::super::assembler::build_subquery_runner_specs(
                        project.subqueries(),
                        exec_ctx,
                    )?;
                    let spec = build_project_spec(project, subquery_runners)?;
                    let (new_fid, op_id) = ArenaPlanAssembler::push_unary_op(
                        operators,
                        fragments,
                        op_alloc,
                        fid,
                        op.id(),
                        spec,
                    )?;
                    operators[op_id.0].has_folded_expressions = project.has_folded_expressions();
                    fid = new_fid;
                }
                PlanNodeEnum::ExpandAll(expand) => {
                    // A1.4: drive count_only from the node annotation so only
                    // the chain-tail expand skips row materialization; middle
                    // hops keep emitting raw destination ids for the next hop.
                    let direct_key = if packed_expand_id == Some(expand.id()) {
                        packed_key_name.clone()
                    } else {
                        None
                    };
                    let spec = build_expand_all_spec_with_flags(
                        expand,
                        exec_ctx,
                        expand.count_only(),
                        direct_key,
                        packed_identity_type.clone(),
                    )?;
                    fid = ArenaPlanAssembler::push_graph_op(
                        operators,
                        fragments,
                        op_alloc,
                        frag_alloc,
                        fid,
                        op.id(),
                        spec,
                    )?
                    .0;
                }
                PlanNodeEnum::Flatten(flatten) => {
                    let spec = build_flatten_spec(flatten)?;
                    let (new_fid, _) = ArenaPlanAssembler::push_unary_op(
                        operators,
                        fragments,
                        op_alloc,
                        fid,
                        op.id(),
                        spec,
                    )?;
                    fid = new_fid;
                }
                _ => unreachable!("local chain holds filter/project/expand/flatten operators only"),
            }
        }
        // DISTINCT keyed by one column followed by TopN on that same key is
        // monotone across partitions: the global top k can only draw from the
        // per-partition top k. Truncating here keeps the global operators in
        // place for correctness and shrinks the exchange payload.
        let decision = local_distinct_topn(chain);
        if let Some((dedup, local_spec)) = decision.pushed {
            ArenaPlanAssembler::push_blocking_op(
                &mut FragmentCtx {
                    operators,
                    fragments,
                    op_alloc,
                },
                fid,
                dedup.id(),
                local_spec,
                PhysicalProperties::single_blocking_with_budget(),
            )?;
        }
        if let Some(agg) = chain.aggregate_split {
            let (partial, _) = split_aggregate(agg);
            ArenaPlanAssembler::push_blocking_op(
                &mut FragmentCtx {
                    operators,
                    fragments,
                    op_alloc,
                },
                fid,
                agg.id(),
                partial,
                PhysicalProperties::single_blocking_with_budget(),
            )?;
        }
        if let Some((topn, spec)) = local_group_topn(chain) {
            ArenaPlanAssembler::push_blocking_op(
                &mut FragmentCtx {
                    operators,
                    fragments,
                    op_alloc,
                },
                fid,
                topn.id(),
                spec,
                PhysicalProperties::single_blocking_with_budget(),
            )?;
        }
        partition_fids.push(fid);
    }
    Ok(partition_fids)
}
/// Outcome of checking the partition-local Distinct + TopN shape.
///
/// The optional spec is pushed into each partition fragment. The note is
/// emitted into plan diagnostics only when a Distinct shape was seen but not
/// proven, so ordinary queries do not get a misleading unpushed marker.
pub(super) struct LocalTopKDecision<'a> {
    pub pushed: Option<(
        &'a crate::planning::plan::core::nodes::graph_operations::DedupNode,
        BlockingSpec,
    )>,
    pub note: Option<String>,
}

fn local_topk_unproven<'a>(reason: &str) -> LocalTopKDecision<'a> {
    LocalTopKDecision {
        pushed: None,
        note: Some(format!("local_topk:unpushed,reason={reason}")),
    }
}

/// Destination property consumed directly by a partition-local blocking
/// operator. Only single-key count and local distinct TopK shapes qualify.
struct PackedKeyHandoff {
    name: String,
    identity_type: Option<DataType>,
}

fn packed_expand_key(
    chain: &PartitionedChain<'_>,
    exec_ctx: &ExecutionContext,
) -> Option<PackedKeyHandoff> {
    let name = if let Some((_, spec)) = local_distinct_topn(chain).pushed {
        let BlockingSpec::LocalTopKDistinct { key_name, .. } = spec else {
            return None;
        };
        key_name
    } else {
        let (partial, _) = split_aggregate(chain.aggregate_split?);
        let BlockingSpec::PartialAggregate {
            group_by_expressions,
            aggregate_functions,
            aggregate_filters,
            ..
        } = partial
        else {
            return None;
        };
        if aggregate_filters.iter().any(Option::is_some)
            || aggregate_functions.len() != 1
            || !matches!(aggregate_functions[0].0, AggregateFunction::Count)
        {
            return None;
        }
        match group_by_expressions.as_slice() {
            [Expression::Variable(name)] => name.clone(),
            _ => return None,
        }
    };
    Some(PackedKeyHandoff {
        identity_type: packed_identity_type(chain, exec_ctx, &name),
        name,
    })
}

/// Prove that the handed-off key is the destination's declared primary key
/// and that it is the only demanded destination property. The primary key
/// mirrors the external vertex id by DDL contract, so the packed endpoint id
/// can then stand in for the property read. No proof means no bypass.
fn packed_identity_type(
    chain: &PartitionedChain<'_>,
    exec_ctx: &ExecutionContext,
    key_name: &str,
) -> Option<DataType> {
    let expand = chain.local.iter().rev().find_map(|op| match *op {
        PlanNodeEnum::ExpandAll(expand) => Some(expand),
        _ => None,
    })?;
    if expand.any_edge_type()
        || expand.edge_types().len() != 1
        || expand.step_limit().unwrap_or(1) != 1
        || expand.step_limits().is_some()
        || expand.filter().is_some()
        || expand.path_semantic().is_some()
    {
        return None;
    }
    let dst_var = expand.col_names().get(2)?.as_str();
    let dst_tag = expand.dst_tag()?;
    if dst_tag.is_empty() {
        return None;
    }
    let key_expr = key_name_expr(chain, key_name)?;
    let (variable, property) = match &key_expr {
        Expression::Property { object, property } => {
            let Expression::Variable(variable) = object.as_ref() else {
                return None;
            };
            (variable.as_str(), property.as_str())
        }
        _ => return None,
    };
    if variable != dst_var {
        return None;
    }
    let dst_props = expand.dst_required_props()?;
    if dst_props.len() != 1 || dst_props[0].as_str() != property {
        return None;
    }
    let space = exec_ctx.space_name.as_deref()?;
    let reader = exec_ctx.storage.as_ref()?.read();
    let tag = reader.get_tag(space, dst_tag).ok()??;
    let primary = tag.properties.first()?;
    if primary.name != property {
        return None;
    }
    match primary.data_type {
        DataType::SmallInt | DataType::Int | DataType::BigInt => Some(primary.data_type.clone()),
        _ => None,
    }
}

/// Resolve the grouping key back to its projected expression so an alias
/// cannot be mistaken for a declared identity column.
fn key_name_expr(chain: &PartitionedChain<'_>, key_name: &str) -> Option<Expression> {
    for op in chain.local.iter().rev() {
        let PlanNodeEnum::Project(project) = op else {
            continue;
        };
        for column in project.columns() {
            if column.alias != key_name {
                continue;
            }
            return column
                .expression
                .expression()
                .map(|meta| meta.inner().clone());
        }
    }
    None
}

/// Operators above a TopN are safe to keep only when they cannot need rows
/// the TopN discarded: projections preserve rows, and a zero-offset Limit no
/// larger than the TopN only trims its ordered prefix.
fn safe_global_tail_after_topn(chain: &PartitionedChain<'_>, topn_limit: usize) -> bool {
    chain.global.iter().skip(2).all(|node| match node {
        PlanNodeEnum::Project(_) => true,
        PlanNodeEnum::Limit(limit) => {
            limit.offset() == 0
                && u64::try_from(limit.count())
                    .ok()
                    .is_some_and(|count| count <= topn_limit as u64)
        }
        _ => false,
    })
}

/// Single-key group aggregate whose following TopN sorts by that key. Every
/// group key is unique after the partial aggregate, so keeping the best k
/// rows per partition cannot remove a key needed by the global top k.
fn local_group_topn<'a>(
    chain: &'a PartitionedChain<'a>,
) -> Option<(&'a PlanNodeEnum, BlockingSpec)> {
    let agg = chain.aggregate_split?;
    let topn_node = chain.global.get(1)?;
    let PlanNodeEnum::TopN(topn) = topn_node else {
        return None;
    };
    let BlockingSpec::TopN {
        n,
        sort_expressions,
        sort_directions,
    } = build_topn_spec(topn).ok()?
    else {
        return None;
    };
    if n == 0 || !safe_global_tail_after_topn(chain, n as usize) {
        return None;
    }
    let key_name = match sort_expressions.as_slice() {
        [Expression::Variable(name)] => name.clone(),
        [Expression::Property { object, property }] => {
            let Expression::Variable(object) = object.as_ref() else {
                return None;
            };
            format!("{object}.{property}")
        }
        _ => return None,
    };
    if agg.group_keys().len() != 1 || agg.group_keys()[0] != key_name {
        return None;
    }
    let (partial, _) = split_aggregate(agg);
    let BlockingSpec::PartialAggregate {
        group_by_expressions,
        aggregate_functions,
        aggregate_filters,
        ..
    } = partial
    else {
        return None;
    };
    if group_by_expressions.len() != 1
        || aggregate_functions.len() != 1
        || !matches!(aggregate_functions[0].0, AggregateFunction::Count)
        || aggregate_filters.iter().any(Option::is_some)
    {
        return None;
    }
    let direction = match sort_directions.as_slice() {
        [direction] => *direction,
        _ => return None,
    };
    Some((
        topn_node,
        BlockingSpec::LocalTopKRows {
            key_name,
            limit: n,
            direction,
            push_reason: "group-key".to_string(),
        },
    ))
}

/// Narrow shape where Distinct and its following TopN can be truncated per
/// partition without changing the final answer.
fn local_distinct_topn<'a>(chain: &'a PartitionedChain<'a>) -> LocalTopKDecision<'a> {
    let Some(first) = chain.global.first() else {
        return LocalTopKDecision {
            pushed: None,
            note: None,
        };
    };
    if !matches!(first, PlanNodeEnum::Dedup(_)) {
        return LocalTopKDecision {
            pushed: None,
            note: None,
        };
    }
    let dedup = match first {
        PlanNodeEnum::Dedup(dedup) if dedup.col_names().len() == 1 => dedup,
        PlanNodeEnum::Dedup(_) => return local_topk_unproven("multi-column-distinct"),
        _ => unreachable!("dedup shape checked above"),
    };
    let topn = match chain.global.get(1) {
        Some(PlanNodeEnum::TopN(topn)) => topn,
        _ => return local_topk_unproven("missing-topn"),
    };
    let spec = match build_topn_spec(topn) {
        Ok(spec) => spec,
        Err(_) => return local_topk_unproven("invalid-topn-limit"),
    };
    let BlockingSpec::TopN {
        n,
        sort_expressions,
        sort_directions,
    } = &spec
    else {
        return local_topk_unproven("unsupported-topn-spec");
    };
    if !safe_global_tail_after_topn(chain, *n as usize) {
        return local_topk_unproven("unsafe-global-tail");
    }
    let BlockingSpec::TopN {
        n,
        sort_expressions,
        sort_directions,
    } = &spec
    else {
        return local_topk_unproven("unsupported-topn-spec");
    };
    let key_column = match (sort_expressions.as_slice(), n) {
        ([Expression::Variable(name)], n) if *n > 0 => name.clone(),
        ([Expression::Property { object, property }], n) if *n > 0 => {
            let Expression::Variable(object) = object.as_ref() else {
                return local_topk_unproven("complex-sort-expression");
            };
            format!("{object}.{property}")
        }
        _ => return local_topk_unproven("unsupported-sort-shape"),
    };
    let direction = match sort_directions.as_slice() {
        [direction] => *direction,
        _ => return local_topk_unproven("multi-column-sort"),
    };
    if key_column != dedup.col_names()[0] {
        return local_topk_unproven("sort-key-mismatch");
    }
    let push_reason = match dedup.column_types() {
        [] => "runtime".to_string(),
        [linkrs_core::DataType::BigInt] => "typed".to_string(),
        _ => return local_topk_unproven("unsupported-key-type"),
    };
    LocalTopKDecision {
        pushed: Some((
            dedup,
            BlockingSpec::LocalTopKDistinct {
                key_name: key_column,
                limit: *n,
                direction,
                push_reason,
            },
        )),
        note: None,
    }
}

/// Proven shape for the ordered primary-key existence scan.
pub(super) struct OrderedExistenceProof {
    pub space_name: String,
    pub index_name: String,
    pub index_id: u64,
    pub src_tag: String,
    pub dst_tag: String,
    pub edge_type: String,
    pub key_property: String,
    pub key_column: String,
    pub key_type: DataType,
    pub limit: usize,
    pub generation: u64,
    pub reason: String,
}

/// Lower a proven chain directly into one ordered source, a zero-buffer
/// contract boundary and the projections above the TopN. The source already
/// emits at most `limit` ordered distinct keys, so Distinct and TopN are
/// redundant on the proven path.
pub(super) fn build_ordered_existence_group(
    operators: &mut Vec<PhysicalOperatorSpec>,
    fragments: &mut Vec<FragmentSpec>,
    op_alloc: &mut PhysicalOperatorIdAllocator,
    frag_alloc: &mut ArenaFragmentAllocator,
    chain: &PartitionedChain<'_>,
    proof: &OrderedExistenceProof,
    exec_ctx: &ExecutionContext,
) -> Result<(FragmentId, PhysicalOperatorId), PlanBuildError> {
    let layout = Arc::new(SlotLayout::from_names(&[proof.key_column.clone()]));
    let spec = SourceSpec::OrderedExistenceScan {
        space_name: proof.space_name.clone(),
        index_name: proof.index_name.clone(),
        index_id: proof.index_id,
        src_tag: proof.src_tag.clone(),
        dst_tag: proof.dst_tag.clone(),
        edge_type: proof.edge_type.clone(),
        key_property: proof.key_property.clone(),
        key_column: proof.key_column.clone(),
        key_type: proof.key_type.clone(),
        limit: proof.limit,
        batch_size: proof.limit.clamp(1, 16),
        proof: proof.reason.clone(),
        generation: proof.generation,
        output_layout: layout.clone(),
    };
    let (mut fid, _) = ArenaPlanAssembler::push_source_op(
        operators,
        fragments,
        op_alloc,
        frag_alloc,
        chain.scan.id(),
        spec,
    );
    let contract_node = chain.global[1].id();
    fid = ArenaPlanAssembler::push_global_unary_op(
        operators,
        fragments,
        op_alloc,
        frag_alloc,
        fid,
        contract_node,
        UnarySpec::OrderedExistenceContract {
            limit: u32::try_from(proof.limit).unwrap_or(u32::MAX),
        },
    )?
    .0;
    for op in chain.global.iter().skip(2) {
        fid = push_global_op(
            operators, fragments, op_alloc, frag_alloc, fid, op, exec_ctx,
        )?
        .0;
    }
    let root_operator = fragments
        .get(fid.0)
        .map(|fragment| fragment.root_operator)
        .ok_or_else(|| PlanBuildError::unsupported("PhysicalPlan", 0, "root fragment missing"))?;
    Ok((fid, root_operator))
}

/// Prove the narrow `DISTINCT key ORDER BY key LIMIT k` shape where walking
/// the destination primary-key index can replace a full source scan.
///
/// `Ok(None)` means the query did not present the target shape at all.
/// `Err(reason)` means it did but a safety condition failed, so EXPLAIN can
/// report why the existing plan path was kept.
pub(super) fn ordered_existence_scan(
    chain: &PartitionedChain<'_>,
    exec_ctx: &ExecutionContext,
) -> Result<Option<OrderedExistenceProof>, String> {
    if std::env::var("LINKRS_ORDERED_EXISTENCE_SCAN")
        .is_ok_and(|value| matches!(value.as_str(), "off" | "0" | "false"))
    {
        return Err("disabled".to_string());
    }
    let dedup = match chain.global.first() {
        Some(PlanNodeEnum::Dedup(dedup)) if dedup.col_names().len() == 1 => dedup,
        Some(PlanNodeEnum::Dedup(_)) => return Err("multi-column-distinct".to_string()),
        _ => return Ok(None),
    };
    let topn = match chain.global.get(1) {
        Some(PlanNodeEnum::TopN(topn)) => topn,
        _ => return Ok(None),
    };
    let spec = build_topn_spec(topn).map_err(|_| "invalid-topn".to_string())?;
    let BlockingSpec::TopN {
        n,
        sort_expressions,
        sort_directions,
    } = spec
    else {
        return Err("unsupported-topn".to_string());
    };
    if n == 0 {
        return Err("zero-limit".to_string());
    }
    if !matches!(
        sort_directions.as_slice(),
        [crate::executor::streaming::executor::SortDirection::Ascending]
    ) {
        return Err("descending-order".to_string());
    }
    if !safe_global_tail_after_topn(chain, n as usize) {
        return Err("unsafe-global-tail".to_string());
    }
    let key_column = match sort_expressions.as_slice() {
        [Expression::Variable(name)] => name.clone(),
        [Expression::Property { object, property }] => {
            let Expression::Variable(variable) = object.as_ref() else {
                return Err("complex-sort-expression".to_string());
            };
            format!("{variable}.{property}")
        }
        _ => return Err("unsupported-sort-shape".to_string()),
    };
    if key_column != dedup.col_names()[0] {
        return Err("sort-key-mismatch".to_string());
    }

    let expands: Vec<_> = chain
        .local
        .iter()
        .filter_map(|op| match op {
            PlanNodeEnum::ExpandAll(expand) => Some(expand),
            _ => None,
        })
        .collect();
    let [expand] = expands.as_slice() else {
        return Ok(None);
    };
    if chain
        .local
        .iter()
        .any(|op| matches!(op, PlanNodeEnum::Filter(_) | PlanNodeEnum::Flatten(_)))
    {
        return Err("local-filter-or-flatten".to_string());
    }
    if expand.any_edge_type()
        || expand.edge_types().len() != 1
        || expand.step_limit().unwrap_or(1) != 1
        || expand.step_limits().is_some()
        || expand.filter().is_some()
        || expand.path_semantic().is_some()
        || expand.sample()
        || expand.include_empty_paths()
        || EdgeDirection::parse(expand.direction()) != Ok(EdgeDirection::Out)
    {
        return Err("unsafe-expand-shape".to_string());
    }
    if !matches!(expand.edge_required_props().as_deref(), Some([])) {
        return Err("edge-properties-required".to_string());
    }
    let dst_var = expand
        .col_names()
        .get(2)
        .map(String::as_str)
        .ok_or("missing-destination")?;
    let dst_tag = expand.dst_tag().ok_or("missing-destination-tag")?;
    if dst_tag.is_empty() {
        return Err("missing-destination-tag".to_string());
    }
    let key_property = key_property_for(chain, &key_column, dst_var)?;
    if !matches!(expand.dst_required_props().as_deref(), Some([property]) if *property == key_property)
    {
        return Err("extra-destination-properties".to_string());
    }
    let PlanNodeEnum::ScanVertices(scan) = chain.scan else {
        return Ok(None);
    };
    if scan.filter().is_some() || scan.limit().is_some() || !scan.projected_properties().is_empty()
    {
        return Err("source-scan-predicate".to_string());
    }
    let src_tag = scan.tag().ok_or("missing-source-tag")?;

    let space_name = exec_ctx.space_name.as_deref().ok_or("missing-space")?;
    let storage = exec_ctx.storage.as_ref().ok_or("missing-storage")?;
    let guard = storage.read();
    if guard.snapshot_handle().is_none() {
        return Err("unbound-snapshot".to_string());
    }
    let edge_type = &expand.edge_types()[0];
    let edge_info = guard
        .get_edge_type(space_name, edge_type)
        .map_err(|_| "edge-schema-unavailable".to_string())?
        .ok_or("edge-schema-missing".to_string())?;
    if edge_info.src_tag_name != *src_tag || edge_info.dst_tag_name != *dst_tag {
        return Err("endpoint-schema-mismatch".to_string());
    }
    let tag = guard
        .get_tag(space_name, dst_tag)
        .map_err(|_| "tag-schema-unavailable".to_string())?
        .ok_or("tag-schema-missing".to_string())?;
    let primary = tag.properties.first().ok_or("no-primary-key")?;
    if primary.name != key_property {
        return Err("not-primary-key".to_string());
    }
    let key_type = match primary.data_type {
        DataType::SmallInt | DataType::Int | DataType::BigInt => primary.data_type.clone(),
        _ => return Err("unsupported-key-type".to_string()),
    };
    let proof = guard
        .ordered_index_proof(space_name, dst_tag, &key_property)
        .map_err(|_| "index-proof-unavailable".to_string())?
        .ok_or("no-ordered-index".to_string())?;

    Ok(Some(OrderedExistenceProof {
        space_name: space_name.to_string(),
        index_name: proof.index_name.clone(),
        index_id: proof.index_id,
        src_tag: src_tag.to_string(),
        dst_tag: dst_tag.to_string(),
        edge_type: edge_type.clone(),
        key_property,
        key_column,
        key_type,
        limit: n as usize,
        generation: proof.generation,
        reason: format!(
            "{}; edge {} from {} to {}; limit {}",
            proof.reason, edge_type, src_tag, dst_tag, n
        ),
    }))
}

fn key_property_for(
    chain: &PartitionedChain<'_>,
    key_column: &str,
    dst_var: &str,
) -> Result<String, String> {
    let expression = key_name_expr(chain, key_column)
        .or_else(|| {
            let (variable, property) = key_column.split_once('.')?;
            (variable == dst_var).then(|| Expression::Property {
                object: Box::new(Expression::Variable(variable.to_string())),
                property: property.to_string(),
            })
        })
        .ok_or("key-expression-unavailable".to_string())?;
    match expression {
        Expression::Property { object, property } => {
            let Expression::Variable(variable) = object.as_ref() else {
                return Err("complex-key-expression".to_string());
            };
            if variable != dst_var {
                return Err("key-not-destination".to_string());
            }
            Ok(property)
        }
        _ => Err("key-not-property".to_string()),
    }
}

/// Diagnose the local TopK decision for a partitioned logical plan.
pub(crate) fn local_topk_decision(node: &PlanNodeEnum) -> Option<String> {
    let chain = decompose(node)?;
    local_distinct_topn(&chain).note
}

/// Decompose the logical root into a partitionable chain, or return `None`
/// when the tree is not a linear chain ending in a vertex scan.
pub(super) fn decompose(node: &PlanNodeEnum) -> Option<PartitionedChain<'_>> {
    let mut chain: Vec<&PlanNodeEnum> = Vec::new();
    if !collect_chain(node, &mut chain) {
        return None;
    }
    // chain is root-first, ending with the scan.
    let scan_index = chain.len() - 1;

    // Local operators: Filter/Project/ExpandAll directly above the scan, up
    // to the first global operator. ExpandAll must be the outermost local op
    // (its expansion is partition-local in E4 anchored traversals).
    let mut i = scan_index;
    let mut local: Vec<&PlanNodeEnum> = Vec::new();
    while i > 0 {
        let op = chain[i - 1];
        if matches!(
            op,
            PlanNodeEnum::Filter(_)
                | PlanNodeEnum::Project(_)
                | PlanNodeEnum::Flatten(_)
                | PlanNodeEnum::ExpandAll(_)
        ) {
            local.push(op);
            i -= 1;
        } else {
            break;
        }
    }

    // Global operators: the first global operator up to the root.
    let mut global: Vec<&PlanNodeEnum> = Vec::new();
    for j in (0..i).rev() {
        global.push(chain[j]);
    }

    let aggregate_split = match global.first() {
        Some(PlanNodeEnum::Aggregate(agg))
            if all_functions_support_partial(agg.aggregation_functions()) =>
        {
            Some(agg)
        }
        _ => None,
    };

    Some(PartitionedChain {
        scan: chain[scan_index],
        local,
        global,
        aggregate_split,
    })
}

/// Walk the linear chain from `node` down to its scan. Returns `false` when
/// an operator outside the supported set is encountered. An `ExpandAll` hop
/// is allowed (E4 anchored bounded traversal): it stays partition-local.
fn collect_chain<'a>(node: &'a PlanNodeEnum, chain: &mut Vec<&'a PlanNodeEnum>) -> bool {
    chain.push(node);
    match node {
        PlanNodeEnum::ScanVertices(_) | PlanNodeEnum::ScanEdges(_) => true,
        PlanNodeEnum::Filter(filter) => collect_chain(filter.input(), chain),
        PlanNodeEnum::Project(project) => collect_chain(project.input(), chain),
        PlanNodeEnum::Flatten(flatten) => collect_chain(flatten.input(), chain),
        PlanNodeEnum::Limit(limit) => collect_chain(limit.input(), chain),
        PlanNodeEnum::Sort(sort) => collect_chain(sort.input(), chain),
        PlanNodeEnum::Aggregate(agg) => collect_chain(agg.input(), chain),
        PlanNodeEnum::TopN(topn) => collect_chain(topn.input(), chain),
        PlanNodeEnum::Dedup(dedup) => collect_chain(dedup.input(), chain),
        PlanNodeEnum::Window(window) => collect_chain(window.input(), chain),
        PlanNodeEnum::ExpandAll(expand) => {
            if let Some(input) = expand.inputs().first() {
                collect_chain(input, chain)
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Build the partial and final aggregate specs for a split aggregate node.
///
/// When the aggregate consumes a `count_only` expand (through its Project
/// pass-through), the `COUNT` functions are rewritten to `SUM(_expand_count)`
/// so the per-chunk edge counts are summed instead of counted as rows.
fn split_aggregate(node: &AggregateNode) -> (BlockingSpec, BlockingSpec) {
    let group_by_expressions: Vec<Expression> = node
        .group_keys()
        .iter()
        .map(|key| Expression::Variable(key.clone()))
        .collect();
    let count_only =
        is_count_only_aggregate(node) && count_only_expand_below(node.input()).is_some();
    let agg_args = node.aggregation_args();
    let aggregate_functions: Vec<(AggregateFunction, Vec<Expression>)> = node
        .aggregation_functions()
        .iter()
        .enumerate()
        .map(|(i, func)| {
            if count_only {
                (
                    AggregateFunction::Sum,
                    vec![Expression::Variable(COUNT_ONLY_COLUMN.to_string())],
                )
            } else {
                let args = agg_args.get(i).cloned().unwrap_or_default();
                (*func, args)
            }
        })
        .collect();
    let output_col_names = node.col_names().to_vec();
    // Partial aggregates evaluate FILTER per input row; the final merge
    // combines already-filtered partials and carries no predicate.
    let aggregate_filters = node.aggregation_filters().to_vec();
    (
        BlockingSpec::PartialAggregate {
            group_by_expressions: group_by_expressions.clone(),
            aggregate_functions: aggregate_functions.clone(),
            aggregate_filters,
            output_col_names: output_col_names.clone(),
        },
        BlockingSpec::FinalAggregate {
            group_by_expressions,
            aggregate_functions,
            output_col_names,
        },
    )
}

/// Aggregate functions that support per-partition partial accumulation
/// followed by a global merge (mirrors the predicate previously used by the
/// removed planner-side `PartitionedPhysicalPlan` decomposition).
fn all_functions_support_partial(funcs: &[AggregateFunction]) -> bool {
    funcs.iter().all(|f| {
        matches!(
            f,
            AggregateFunction::Count
                | AggregateFunction::Sum
                | AggregateFunction::Min
                | AggregateFunction::Max
                | AggregateFunction::Avg
        )
    })
}
