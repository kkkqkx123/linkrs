use super::*;
use crate::executor::base::ExecutionContext;
use crate::executor::streaming::plan::arena_builder::PhysicalPlanBuilder;
use crate::executor::streaming::{BlockingSpec, JoinSpec, SourceSpec};
use linkrs_core::types::operators::AggregateFunction;
use crate::executor::streaming::plan::context::PhysicalPlanBuildContext;
use crate::executor::streaming::plan::types::InputContract;
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::planning::plan::core::nodes::ScanVerticesNode;
use crate::planning::plan::{PartitionSource, PartitionSpec};
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
use linkrs_core::types::expr::{ContextualExpression, ExpressionMeta};
use linkrs_core::Expression;
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

    // Every partition scan carries its own vertex-id range.
    let mut partition_ranges = Vec::new();
    for op in &plan.operators {
        if let crate::executor::streaming::plan::types::OperatorKindSpec::Source(
            SourceSpec::StorageScanVertices {
                partition_range, ..
            },
        ) = &op.spec
        {
            partition_ranges.push(partition_range.clone());
        }
    }
    assert_eq!(partition_ranges, vec![Some(0..5), Some(5..10)]);

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
    assert!(!ctx.parallel_fallback_reason.is_empty());
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
        ctx.parallel_fallback_reason.is_empty(),
        "vid-key join must not fall back, got: {}",
        ctx.parallel_fallback_reason
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
        ctx.parallel_fallback_reason.is_empty(),
        "non-vid equality join must use hash exchange path, got: {}",
        ctx.parallel_fallback_reason
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
