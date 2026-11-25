use super::chain::{
    build_ordered_existence_group, ordered_existence_scan, OrderedExistenceProof, PartitionedChain,
};
use super::*;
use crate::executor::base::ExecutionContext;
use crate::executor::streaming::plan::arena_builder::PhysicalPlanBuilder;
use crate::executor::streaming::plan::context::PhysicalPlanBuildContext;
use crate::executor::streaming::plan::types::InputContract;
use crate::executor::streaming::{BlockingSpec, JoinSpec, OperatorKindSpec, SourceSpec, UnarySpec};
use crate::planning::plan::core::nodes::base::plan_node_traits::{MultipleInputNode, PlanNode};
use crate::planning::plan::core::nodes::graph_operations::DedupNode;
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::planning::plan::core::nodes::operation::sort_node::{LimitNode, SortItem, TopNNode};
use crate::planning::plan::core::nodes::traversal::ExpandAllNode;
use crate::planning::plan::core::nodes::ScanVerticesNode;
use crate::planning::plan::{PartitionSource, PartitionSpec};
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
use linkrs_core::types::expr::{ContextualExpression, ExpressionMeta};
use linkrs_core::types::graph_schema::OrderDirection;
use linkrs_core::types::operators::AggregateFunction;
use linkrs_core::{DataType, Expression};
use std::sync::Arc;

fn test_context() -> ExecutionContext {
    ExecutionContext::new(Arc::new(ExpressionAnalysisContext::new()))
}

fn spec_with_two_ranges() -> PartitionSpec {
    PartitionSpec::try_new(
        vec![0..5, 5..10],
        PartitionSource::VertexId {
            tag: "person".to_string(),
        },
        None,
    )
    .expect("valid spec")
}

fn tagged_scan() -> PlanNodeEnum {
    let mut scan = ScanVerticesNode::new(1, "space");
    scan.set_tag("person");
    scan.set_col_names(vec!["v".to_string()]);
    PlanNodeEnum::ScanVertices(scan)
}

fn distinct_topn_scan(types: Vec<DataType>) -> PlanNodeEnum {
    let mut dedup = DedupNode::new(tagged_scan()).expect("dedup plan should build");
    dedup.set_col_names(vec!["v".to_string()]);
    dedup.set_column_types(types);
    let topn = TopNNode::new(
        PlanNodeEnum::Dedup(dedup),
        vec![SortItem::column("v".to_string(), OrderDirection::Asc)],
        10,
    )
    .expect("topn plan should build");
    PlanNodeEnum::TopN(topn)
}

fn has_local_topk(plan: &crate::executor::streaming::plan::types::PhysicalPlan) -> bool {
    plan.operators.iter().any(|op| {
        matches!(
            op.spec,
            crate::executor::streaming::plan::types::OperatorKindSpec::Blocking(
                BlockingSpec::LocalTopKDistinct { .. } | BlockingSpec::LocalTopKRows { .. }
            )
        )
    })
}

fn simple_filter(input: PlanNodeEnum) -> PlanNodeEnum {
    let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
    let expr = Expression::Literal(linkrs_core::Value::Int(1));
    let id = expr_ctx.register_expression(ExpressionMeta::new(expr));
    let cond = ContextualExpression::new(id, expr_ctx);
    let filter = FilterNode::new(input, cond).expect("filter plan should build");
    PlanNodeEnum::Filter(filter)
}

#[test]
fn partitionable_scan_chain_produces_partition_and_exchange_fragments() {
    let node = simple_filter(tagged_scan());
    let mut ctx = PhysicalPlanBuildContext::new();
    ctx.partition_spec = Some(spec_with_two_ranges());
    let exec_ctx = test_context();

    let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).expect("build");

    // 2 partition fragments + 1 exchange fragment.
    assert_eq!(plan.fragment_count(), 3);
    assert_eq!(
        plan.root_fragment,
        crate::executor::streaming::plan::types::FragmentId(2)
    );

    // Every partition scan carries its own shard slice and no value range:
    // slices divide enumeration, ranges would repeat full-table walks.
    let mut shard_slices = Vec::new();
    for op in &plan.operators {
        if let crate::executor::streaming::plan::types::OperatorKindSpec::Source(
            SourceSpec::StorageScanVertices {
                partition_range,
                shard_slice,
                ..
            },
        ) = &op.spec
        {
            assert!(partition_range.is_none());
            shard_slices.push(*shard_slice);
        }
    }
    assert_eq!(shard_slices.len(), 2);
    for (index, slice) in shard_slices.iter().enumerate() {
        let slice = slice.expect("partition scan must carry a shard slice");
        assert_eq!(slice.partition_id, index);
        assert_eq!(slice.partition_count, 2);
    }

    // The exchange operator consumes both partitions through a
    // PartitionedInputs contract.
    let exchange = plan
        .operators
        .iter()
        .find(|op| {
            matches!(
                op.spec,
                crate::executor::streaming::plan::types::OperatorKindSpec::Exchange(_)
            )
        })
        .expect("exchange operator");
    match &exchange.input_contract {
        InputContract::PartitionedInputs { members, .. } => {
            assert_eq!(members.len(), 2);
            assert_eq!(members[0].partition_id, 0);
            assert_eq!(members[1].partition_id, 1);
        }
        other => panic!("expected PartitionedInputs, got {:?}", other),
    }

    crate::executor::streaming::plan::validator::PhysicalPlanValidator::validate(&Arc::new(plan))
        .expect("partitioned plan should validate");
}

#[test]
fn local_topk_unsupported_key_type_is_reported_in_plan_diagnostics() {
    let node = distinct_topn_scan(vec![DataType::String]);
    let mut ctx = PhysicalPlanBuildContext::new();
    ctx.partition_spec = Some(spec_with_two_ranges());
    let exec_ctx = test_context();

    let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).expect("build");

    assert!(
        ctx.diagnostics
            .notes
            .iter()
            .any(|note| note == "local_topk:unpushed,reason=unsupported-key-type"),
        "unsupported key type must be visible in EXPLAIN diagnostics: {:?}",
        ctx.diagnostics.notes
    );
    assert!(!has_local_topk(&plan));
}

#[test]
fn local_topk_pushed_shape_does_not_emit_unpushed_note() {
    let node = distinct_topn_scan(vec![DataType::BigInt]);
    let mut ctx = PhysicalPlanBuildContext::new();
    ctx.partition_spec = Some(spec_with_two_ranges());
    let exec_ctx = test_context();

    let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).expect("build");

    assert!(
        ctx.diagnostics
            .notes
            .iter()
            .all(|note| !note.contains("local_topk:unpushed")),
        "pushed shape must not claim it was unpushed"
    );
    assert!(has_local_topk(&plan));
}

#[test]
fn filter_above_distinct_topn_disables_local_topk() {
    let node = simple_filter(distinct_topn_scan(vec![DataType::BigInt]));
    let mut ctx = PhysicalPlanBuildContext::new();
    ctx.partition_spec = Some(spec_with_two_ranges());
    let exec_ctx = test_context();

    let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).expect("build");

    assert!(
        ctx.diagnostics
            .notes
            .iter()
            .any(|note| note == "local_topk:unpushed,reason=unsafe-global-tail"),
        "a filter above TopN must disable the local TopK push"
    );
    assert!(!has_local_topk(&plan));
}

#[test]
fn limit_above_distinct_topn_keeps_local_topk() {
    let node = LimitNode::new(distinct_topn_scan(vec![DataType::BigInt]), 0, 10)
        .expect("limit plan should build");
    let node = PlanNodeEnum::Limit(node);
    let mut ctx = PhysicalPlanBuildContext::new();
    ctx.partition_spec = Some(spec_with_two_ranges());
    let exec_ctx = test_context();

    let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).expect("build");

    assert!(
        ctx.diagnostics
            .notes
            .iter()
            .all(|note| !note.contains("local_topk:unpushed")),
        "a prefix limit must not disable the local TopK push"
    );
    assert!(has_local_topk(&plan));
}

#[test]
fn aggregate_is_split_into_partial_and_final_phases() {
    use crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;

    let input = tagged_scan();
    let mut agg = AggregateNode::new(input, vec![], vec![AggregateFunction::Count])
        .expect("aggregate plan should build");
    agg.set_col_names(vec!["count(*)".to_string()]);
    let node = PlanNodeEnum::Aggregate(agg);

    let mut ctx = PhysicalPlanBuildContext::new();
    ctx.partition_spec = Some(spec_with_two_ranges());
    let exec_ctx = test_context();

    let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).expect("build");

    // 2 partition fragments + exchange + final aggregate fragment.
    assert_eq!(plan.fragment_count(), 4);

    let mut partial_count = 0;
    let mut final_count = 0;
    for op in &plan.operators {
        if let crate::executor::streaming::plan::types::OperatorKindSpec::Blocking(spec) = &op.spec
        {
            match spec {
                BlockingSpec::PartialAggregate { .. } => partial_count += 1,
                BlockingSpec::FinalAggregate { .. } => final_count += 1,
                _ => {}
            }
        }
    }
    assert_eq!(partial_count, 2);
    assert_eq!(final_count, 1);

    crate::executor::streaming::plan::validator::PhysicalPlanValidator::validate(&Arc::new(plan))
        .expect("partitioned aggregate plan should validate");
}

#[test]
fn unsupported_shape_falls_back_to_serial() {
    use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;

    let start = PlanNodeEnum::Start(StartNode::new());
    let mut ctx = PhysicalPlanBuildContext::new();
    ctx.partition_spec = Some(spec_with_two_ranges());
    let exec_ctx = test_context();

    let plan = PhysicalPlanBuilder::build(&start, &mut ctx, &exec_ctx).expect("build");
    assert_eq!(plan.fragment_count(), 1);
    assert!(!ctx.diagnostics.fallback_reason.is_empty());
}

/// Build a keyed equality join plan node with the given hash/probe key
/// variable names over two tagged scans.
fn keyed_join(hash_key_name: &str, probe_key_name: &str) -> PlanNodeEnum {
    use crate::planning::plan::core::nodes::join::join_node::InnerJoinNode;

    let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
    let make_key = |name: &str| {
        let expr = Expression::Variable(name.to_string());
        let id = expr_ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, expr_ctx.clone())
    };
    let mut left_scan = ScanVerticesNode::new(1, "space");
    left_scan.set_tag("person");
    left_scan.set_col_names(vec!["a".to_string()]);
    let mut right_scan = ScanVerticesNode::new(2, "space");
    right_scan.set_tag("person");
    right_scan.set_col_names(vec!["b".to_string()]);
    PlanNodeEnum::InnerJoin(
        InnerJoinNode::new(
            PlanNodeEnum::ScanVertices(left_scan),
            PlanNodeEnum::ScanVertices(right_scan),
            vec![make_key(hash_key_name)],
            vec![make_key(probe_key_name)],
        )
        .expect("join plan should build"),
    )
}

#[test]
fn co_partitioned_equality_join_pairs_partition_fragments() {
    // A vid-key equality join over two simple scan chains takes the
    // co-partition direct-join path: one local join per vertex-id range,
    // gathered through a single Concatenate exchange.
    let node = keyed_join("a.vid", "b.vid");
    let mut ctx = PhysicalPlanBuildContext::new();
    ctx.partition_spec = Some(spec_with_two_ranges());
    let exec_ctx = test_context();

    let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).expect("build");
    assert!(
        ctx.diagnostics.fallback_reason.is_empty(),
        "vid-key join must not fall back, got: {}",
        ctx.diagnostics.fallback_reason
    );

    // 2 left scans + 2 right scans + 2 local joins + 1 exchange.
    assert_eq!(plan.fragment_count(), 7);
    assert_eq!(
        plan.fragments
            .fragments()
            .iter()
            .filter(|f| matches!(
                f.kind,
                crate::executor::streaming::plan::types::FragmentKind::Exchange
            ))
            .count(),
        1,
        "co-partitioned join must gather through exactly one exchange"
    );

    // One local join per partition, each carrying the equality condition.
    let mut join_count = 0;
    for op in &plan.operators {
        if let crate::executor::streaming::plan::types::OperatorKindSpec::Join(
            JoinSpec::InnerJoin { join_condition },
        ) = &op.spec
        {
            join_count += 1;
            assert!(
                join_condition.is_some(),
                "partitioned equality join must carry its key condition"
            );
        }
    }
    assert_eq!(join_count, 2, "one local join per partition");

    // The exchange consumes both per-partition joins.
    let exchange = plan
        .operators
        .iter()
        .find(|op| {
            matches!(
                op.spec,
                crate::executor::streaming::plan::types::OperatorKindSpec::Exchange(_)
            )
        })
        .expect("exchange operator");
    match &exchange.input_contract {
        InputContract::PartitionedInputs { members, .. } => {
            assert_eq!(members.len(), 2);
            assert_eq!(members[0].partition_id, 0);
            assert_eq!(members[1].partition_id, 1);
        }
        other => panic!("expected PartitionedInputs, got {:?}", other),
    }

    crate::executor::streaming::plan::validator::PhysicalPlanValidator::validate(&Arc::new(plan))
        .expect("co-partitioned join plan should validate");
}

#[test]
fn non_vid_equality_join_falls_back_to_global_gather_join() {
    // A join on a non-vid key uses the hash exchange path: both sides are
    // shuffled by join key, then partition-local joins run in parallel,
    // followed by a Concatenate exchange.
    let node = keyed_join("a.value", "b.value");
    let mut ctx = PhysicalPlanBuildContext::new();
    ctx.partition_spec = Some(spec_with_two_ranges());
    let exec_ctx = test_context();

    let plan = PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).expect("build");
    assert!(
        ctx.diagnostics.fallback_reason.is_empty(),
        "non-vid equality join must use hash exchange path, got: {}",
        ctx.diagnostics.fallback_reason
    );

    // Hash exchange path: 2 left frags + 1 left hash exchange +
    // 2 right frags + 1 right hash exchange + 2 local joins + 1 gather = 9
    assert_eq!(plan.fragment_count(), 9);
    assert_eq!(
        plan.fragments
            .fragments()
            .iter()
            .filter(|f| matches!(
                f.kind,
                crate::executor::streaming::plan::types::FragmentKind::Exchange
            ))
            .count(),
        3,
        "hash exchange path: 2 hash exchanges + 1 final gather"
    );

    let mut join_count = 0;
    for op in &plan.operators {
        if let crate::executor::streaming::plan::types::OperatorKindSpec::Join(
            JoinSpec::InnerJoin { join_condition },
        ) = &op.spec
        {
            join_count += 1;
            assert!(
                join_condition.is_some(),
                "partition-local equality join must carry its key condition"
            );
        }
    }
    assert_eq!(join_count, 2, "two partition-local join fragments");

    crate::executor::streaming::plan::validator::PhysicalPlanValidator::validate(&Arc::new(plan))
        .expect("hash-exchange join plan should validate");
}

fn distinct_chain(
    direction: OrderDirection,
    step_limit: u32,
) -> (PlanNodeEnum, PlanNodeEnum, PlanNodeEnum, PlanNodeEnum) {
    let scan = tagged_scan();
    let mut expand = ExpandAllNode::new(1, vec!["HASCREATOR".to_string()], "OUT");
    expand.set_col_names(vec!["n".to_string(), "h".to_string(), "p".to_string()]);
    expand.set_step_limit(step_limit);
    expand.set_include_empty_paths(false);
    expand.set_dst_tag("person".to_string());
    expand.set_edge_required_props(Some(vec![]));
    expand.set_dst_required_props(Some(vec!["id".to_string()]));
    expand.add_input(scan.clone());
    let expand = PlanNodeEnum::ExpandAll(expand);

    let mut dedup = DedupNode::new(expand.clone()).expect("dedup should build");
    dedup.set_col_names(vec!["p.id".to_string()]);
    let dedup = PlanNodeEnum::Dedup(dedup);
    let topn = PlanNodeEnum::TopN(
        crate::planning::plan::core::nodes::operation::sort_node::TopNNode::new(
            dedup.clone(),
            vec![SortItem::column("p.id".to_string(), direction)],
            10,
        )
        .expect("topn should build"),
    );
    (scan, expand, dedup, topn)
}

#[test]
fn ordered_existence_scan_rejects_descending_order() {
    let (scan, expand, dedup, topn) = distinct_chain(OrderDirection::Desc, 1);
    let chain = PartitionedChain {
        scan: &scan,
        local: vec![&expand],
        global: vec![&dedup, &topn],
        aggregate_split: None,
    };
    let result = ordered_existence_scan(&chain, &test_context());
    assert_eq!(result.err().as_deref(), Some("descending-order"));
}

#[test]
fn ordered_existence_scan_rejects_variable_length_hop() {
    let (scan, expand, dedup, topn) = distinct_chain(OrderDirection::Asc, 2);
    let chain = PartitionedChain {
        scan: &scan,
        local: vec![&expand],
        global: vec![&dedup, &topn],
        aggregate_split: None,
    };
    let result = ordered_existence_scan(&chain, &test_context());
    assert_eq!(result.err().as_deref(), Some("unsafe-expand-shape"));
}

#[test]
fn proven_ordered_existence_chain_drops_distinct_and_topn() {
    let (scan, expand, dedup, topn) = distinct_chain(OrderDirection::Asc, 1);
    let chain = PartitionedChain {
        scan: &scan,
        local: vec![&expand],
        global: vec![&dedup, &topn],
        aggregate_split: None,
    };
    let proof = OrderedExistenceProof {
        space_name: "space".to_string(),
        index_name: "person_id_idx".to_string(),
        index_id: 7,
        src_tag: "comment".to_string(),
        dst_tag: "person".to_string(),
        edge_type: "HASCREATOR".to_string(),
        key_property: "id".to_string(),
        key_column: "p.id".to_string(),
        key_type: DataType::BigInt,
        limit: 10,
        generation: 1,
        reason: "test".to_string(),
    };
    let mut operators = Vec::new();
    let mut fragments = Vec::new();
    let mut op_alloc = PhysicalOperatorIdAllocator::new();
    let mut frag_alloc = ArenaFragmentAllocator::new();
    let exec_ctx = test_context();
    build_ordered_existence_group(
        &mut operators,
        &mut fragments,
        &mut op_alloc,
        &mut frag_alloc,
        &chain,
        &proof,
        &exec_ctx,
    )
    .expect("group should build");

    assert!(!operators.iter().any(|op| {
        matches!(
            op.spec,
            OperatorKindSpec::Blocking(BlockingSpec::Distinct | BlockingSpec::TopN { .. })
        )
    }));
    assert!(operators.iter().any(|op| {
        matches!(
            op.spec,
            OperatorKindSpec::Unary(UnarySpec::OrderedExistenceContract { limit: 10 })
        )
    }));
}
