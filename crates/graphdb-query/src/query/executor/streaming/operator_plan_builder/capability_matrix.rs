//! Capability matrix: PlanNodeEnum variants map to physical specs or structured
//! errors.  Adding a new variant causes a compile or test failure here.

use std::sync::Arc;

use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::executor::streaming::operator_plan_builder;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::validator::context::ExpressionAnalysisContext;

/// Unsupported nodes (PassThrough) produce UnsupportedNode errors.
#[test]
fn unsupported_node_produces_structured_error() {
    let ctx = ExecutionContext::new(Arc::new(ExpressionAnalysisContext::new()));
    let node = PlanNodeEnum::PassThrough(
        crate::query::planning::plan::core::nodes::control_flow::PassThroughNode::new(42),
    );
    let result = operator_plan_builder::build_plan_node(&node, &ctx);
    assert!(
        matches!(result, Err(PlanBuildError::UnsupportedNode { .. })),
        "PassThrough must produce UnsupportedNode: {result:?}"
    );
}

/// Start node builds successfully.
#[test]
fn start_node_builds_successfully() {
    let ctx = ExecutionContext::new(Arc::new(ExpressionAnalysisContext::new()));
    let result = operator_plan_builder::build_plan_node(&PlanNodeEnum::default(), &ctx);
    assert!(result.is_ok(), "Start must build: {result:?}");
}

/// IndexScan without scan_limits is rejected with MissingRequiredValue.
#[test]
fn index_scan_without_predicate_is_rejected() {
    use crate::query::planning::plan::core::nodes::access::IndexScanNode;

    let ctx = ExecutionContext::new(Arc::new(ExpressionAnalysisContext::new()));
    let mut idx_node = IndexScanNode::new(1, 1, 1, "test_idx".into(), "test_schema".into(), Default::default());
    idx_node.set_scan_limits(vec![]);

    let result = operator_plan_builder::build_plan_node(
        &PlanNodeEnum::IndexScan(idx_node),
        &ctx,
    );
    assert!(
        matches!(result, Err(PlanBuildError::MissingRequiredValue { .. })),
        "IndexScan without predicate must fail: {result:?}"
    );
}

/// Sample with zero count is rejected with MissingRequiredValue.
#[test]
fn sample_with_zero_count_is_rejected() {
    use crate::query::planning::plan::core::nodes::operation::sample_node::SampleNode;

    let ctx = ExecutionContext::new(Arc::new(ExpressionAnalysisContext::new()));
    let sample = SampleNode::new(PlanNodeEnum::default(), 0)
        .expect("SampleNode construction should succeed");
    let result = operator_plan_builder::build_plan_node(
        &PlanNodeEnum::Sample(sample),
        &ctx,
    );
    assert!(
        matches!(result, Err(PlanBuildError::MissingRequiredValue { .. })),
        "Sample with zero count must fail: {result:?}"
    );
}

/// Weighted shortest path is rejected with CapabilityUnavailable.
#[test]
fn weighted_shortest_path_is_rejected() {
    use crate::query::planning::plan::core::nodes::traversal::path_algorithms::ShortestPathNode;

    let ctx = ExecutionContext::new(Arc::new(ExpressionAnalysisContext::new()));
    let mut sp = ShortestPathNode::new(
        PlanNodeEnum::default(),
        PlanNodeEnum::default(),
        1,
        vec!["ROAD".to_string()],
        4,
    );
    sp.set_weight_expression("weight".to_string());
    let result = operator_plan_builder::build_plan_node(
        &PlanNodeEnum::ShortestPath(sp),
        &ctx,
    );
    assert!(
        matches!(result, Err(PlanBuildError::CapabilityUnavailable { .. })),
        "Weighted shortest path must fail: {result:?}"
    );
}
