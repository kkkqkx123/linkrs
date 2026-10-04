use super::*;
use crate::parser::ast::pattern::{EdgePattern, NodePattern, RepetitionType};
use crate::planning::plan::logical::LogicalNodeEnum;

use crate::binder::validation::ValidationInfo;
use crate::metadata::MetadataContext;
use crate::parser::ast::pattern::PathPattern;
use crate::QueryRequestContext;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::graph_schema::EdgeDirection;
use graphdb_core::types::Span;
use std::collections::HashMap;

#[allow(clippy::arc_with_non_send_sync)]
fn create_test_components() -> (
    Arc<QueryContext>,
    ValidationInfo,
    Option<MetadataContext>,
    Option<Arc<ExpressionAnalysisContext>>,
) {
    let rctx = Arc::new(QueryRequestContext {
        session_id: None,
        user_name: None,
        space_name: None,
        query: String::new(),
        parameters: HashMap::new(),
        ..Default::default()
    });
    let qctx = Arc::new(QueryContext::new(rctx));
    let validation_info = ValidationInfo::default();
    let metadata_context: Option<MetadataContext> = None;
    let expr_context: Option<Arc<ExpressionAnalysisContext>> =
        Some(Arc::new(ExpressionAnalysisContext::new()));
    (qctx, validation_info, metadata_context, expr_context)
}

fn create_test_ctx<'a>(
    qctx: &'a Arc<QueryContext>,
    validation_info: &'a ValidationInfo,
    metadata_context: &'a Option<MetadataContext>,
    expr_context: &'a Option<Arc<ExpressionAnalysisContext>>,
) -> PlanningContext<'a> {
    PlanningContext {
        space_id: 1,
        space_name: "test",
        validation_info,
        qctx,
        enable_index_optimization: false,
        metadata_context,
        expr_context,
        where_expression: None,
    }
}

fn node_pattern(var: &str) -> PathElement {
    PathElement::Node(NodePattern::new(
        Some(var.to_string()),
        vec!["Person".to_string()],
        None,
        vec![],
        Span::default(),
    ))
}

fn edge_pattern(var: &str, direction: EdgeDirection) -> PathElement {
    PathElement::Edge(EdgePattern::new(
        Some(var.to_string()),
        vec!["KNOWS".to_string()],
        None,
        vec![],
        direction,
        None,
        Span::default(),
    ))
}

fn node_path(var: &str) -> Pattern {
    Pattern::Path(PathPattern::new(vec![node_pattern(var)], Span::default()))
}

#[test]
fn test_plan_path_pattern_keeps_logical_root() {
    let (qctx, validation_info, metadata_context, expr_context) = create_test_components();
    let ctx = create_test_ctx(&qctx, &validation_info, &metadata_context, &expr_context);
    let pattern = Pattern::Path(PathPattern::new(
        vec![
            node_pattern("a"),
            edge_pattern("e", EdgeDirection::Out),
            node_pattern("b"),
        ],
        Span::default(),
    ));

    let plan = plan_path_pattern(&pattern, &ctx).expect("planning should succeed");
    let logical_root = plan
        .logical_root()
        .expect("logical root should be attached");

    match logical_root {
        LogicalNodeEnum::ExpandAll(expand) => {
            assert_eq!(expand.input_var.as_deref(), Some("a"));
            assert_eq!(expand.deps.len(), 1);
            match &expand.deps[0] {
                LogicalNodeEnum::Filter(filter) => {
                    assert!(
                        matches!(
                            filter.input.as_deref(),
                            Some(LogicalNodeEnum::ScanVertices(_))
                        ),
                        "labeled scan should be filtered: {:?}",
                        filter.input
                    );
                }
                other => panic!("expected label filter over scan, got: {:?}", other),
            }
        }
        other => panic!("unexpected logical root: {:?}", other),
    }
}

#[test]
fn test_plan_path_pattern_multi_edge_logical_chain() {
    let (qctx, validation_info, metadata_context, expr_context) = create_test_components();
    let ctx = create_test_ctx(&qctx, &validation_info, &metadata_context, &expr_context);
    let pattern = Pattern::Path(PathPattern::new(
        vec![
            node_pattern("a"),
            edge_pattern("e1", EdgeDirection::Out),
            node_pattern("b"),
            edge_pattern("e2", EdgeDirection::Out),
            node_pattern("c"),
        ],
        Span::default(),
    ));

    let plan = plan_path_pattern(&pattern, &ctx).expect("planning should succeed");
    let logical_root = plan
        .logical_root()
        .expect("logical root should be attached");

    match logical_root {
        LogicalNodeEnum::ExpandAll(second_expand) => {
            assert_eq!(second_expand.input_var.as_deref(), Some("b"));
            assert_eq!(second_expand.deps.len(), 1);
            match &second_expand.deps[0] {
                LogicalNodeEnum::ExpandAll(first_expand) => {
                    assert_eq!(first_expand.input_var.as_deref(), Some("a"));
                    assert_eq!(first_expand.deps.len(), 1);
                    match &first_expand.deps[0] {
                        LogicalNodeEnum::Filter(filter) => {
                            assert!(
                                matches!(
                                    filter.input.as_deref(),
                                    Some(LogicalNodeEnum::ScanVertices(_))
                                ),
                                "labeled scan should be filtered: {:?}",
                                filter.input
                            );
                        }
                        other => {
                            panic!("expected label filter over scan, got: {:?}", other)
                        }
                    }
                }
                other => panic!("unexpected middle logical node: {:?}", other),
            }
        }
        other => panic!("unexpected logical root: {:?}", other),
    }
}

#[test]
fn test_plan_path_pattern_optional_keeps_logical_root() {
    let (qctx, validation_info, metadata_context, expr_context) = create_test_components();
    let ctx = create_test_ctx(&qctx, &validation_info, &metadata_context, &expr_context);
    let pattern = Pattern::Path(PathPattern::new(
        vec![
            node_pattern("a"),
            PathElement::Optional(Box::new(edge_pattern("e", EdgeDirection::Out))),
            node_pattern("b"),
        ],
        Span::default(),
    ));

    let plan = plan_path_pattern(&pattern, &ctx).expect("planning should succeed");
    assert!(
        plan.logical_root().is_some(),
        "logical root should survive optional element combination"
    );
}

#[test]
fn test_plan_path_pattern_alternative_keeps_logical_root() {
    let (qctx, validation_info, metadata_context, expr_context) = create_test_components();
    let ctx = create_test_ctx(&qctx, &validation_info, &metadata_context, &expr_context);
    let pattern = Pattern::Path(PathPattern::new(
        vec![
            node_pattern("a"),
            PathElement::Alternative(vec![node_path("x"), node_path("y")]),
        ],
        Span::default(),
    ));

    let plan = plan_path_pattern(&pattern, &ctx).expect("planning should succeed");
    let logical_root = plan
        .logical_root()
        .expect("logical root should be attached");

    match logical_root {
        LogicalNodeEnum::CrossJoin(join) => {
            // Alternative patterns fan out to a two-way join.
            let children = [&join.left, &join.right];
            assert_eq!(children.len(), 2);
        }
        other => panic!("unexpected logical root: {:?}", other),
    }
}

#[test]
fn test_plan_path_pattern_repeated_keeps_logical_root() {
    let (qctx, validation_info, metadata_context, expr_context) = create_test_components();
    let ctx = create_test_ctx(&qctx, &validation_info, &metadata_context, &expr_context);
    let pattern = Pattern::Path(PathPattern::new(
        vec![
            node_pattern("a"),
            PathElement::Repeated(
                Box::new(edge_pattern("e", EdgeDirection::Out)),
                RepetitionType::OneOrMore,
            ),
        ],
        Span::default(),
    ));

    let plan = plan_path_pattern(&pattern, &ctx).expect("planning should succeed");
    let logical_root = plan
        .logical_root()
        .expect("logical root should be attached");

    match logical_root {
        LogicalNodeEnum::CrossJoin(join) => {
            let children = [&join.left, &join.right];
            assert_eq!(children.len(), 2);
            match &*join.right {
                LogicalNodeEnum::Loop(loop_node) => {
                    assert!(matches!(
                        loop_node.body(),
                        Some(LogicalNodeEnum::ExpandAll(_))
                    ));
                }
                other => panic!("unexpected loop node in join: {:?}", other),
            }
        }
        other => panic!("unexpected logical root: {:?}", other),
    }
}
