use super::*;
use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
use crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
use crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
use linkrs_core::types::expr::{ContextualExpression, ExpressionMeta};
use linkrs_core::types::operators::AggregateFunction;
use linkrs_core::Expression;
use linkrs_core::Value;
use std::sync::Arc;
use crate::planning::plan::core::nodes::base::plan_node_traits::MultipleInputNode;

fn ctx_expr(expr: Expression) -> ContextualExpression {
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let id = ctx.register_expression(ExpressionMeta::new(expr));
    ContextualExpression::new(id, ctx)
}

fn anchor_scan(var: &str) -> PlanNodeEnum {
    let mut scan = ScanVerticesNode::new(1, "space");
    scan.set_tag("Node");
    scan.set_col_names(vec![var.to_string()]);
    PlanNodeEnum::ScanVertices(scan)
}

fn hop(edge: &str, vars: [&str; 3], input: PlanNodeEnum) -> PlanNodeEnum {
    let mut expand = ExpandAllNode::new(1, vec![edge.to_string()], "OUT");
    expand.set_step_limit(1);
    expand.set_col_names(vars.iter().map(|s| s.to_string()).collect());
    expand.add_input(input);
    PlanNodeEnum::ExpandAll(expand)
}

fn project_pass_dst(input: PlanNodeEnum, dst: &str) -> PlanNodeEnum {
    let expr = Expression::Variable(dst.to_string());
    let col = linkrs_core::YieldColumn {
        expression: ctx_expr(expr),
        alias: dst.to_string(),
    };
    PlanNodeEnum::Project(ProjectNode::new(input, vec![col]).expect("project should build"))
}

fn count_agg(input: PlanNodeEnum) -> PlanNodeEnum {
    let agg = AggregateNode::new(input, vec![], vec![AggregateFunction::Count])
        .expect("aggregate should build");
    PlanNodeEnum::Aggregate(agg)
}

fn project_pass_var(input: PlanNodeEnum, var: &str) -> PlanNodeEnum {
    let expr = Expression::Variable(var.to_string());
    let col = linkrs_core::YieldColumn {
        expression: ctx_expr(expr),
        alias: var.to_string(),
    };
    PlanNodeEnum::Project(ProjectNode::new(input, vec![col]).expect("project should build"))
}

fn count_field_agg(input: PlanNodeEnum, field: &str) -> PlanNodeEnum {
    let mut agg = AggregateNode::new(input, vec![], vec![AggregateFunction::Count])
        .expect("aggregate should build");
    agg.set_aggregation_args(vec![vec![Expression::Variable(field.to_string())]]);
    PlanNodeEnum::Aggregate(agg)
}

fn expand_alls(root: &PlanNodeEnum) -> Vec<ExpandAllNode> {
    let mut out = Vec::new();
    collect_expand_alls(root, &mut Vec::new(), &mut out);
    out.into_iter().map(|(e, _)| e).collect()
}

fn hop_by_dst<'a>(hops: &'a [ExpandAllNode], dst: &str) -> &'a ExpandAllNode {
    hops.iter()
        .find(|h| h.col_names().get(2).map(|s| s.as_str()) == Some(dst))
        .expect("hop with dst")
}

#[test]
fn two_hop_count_chain_is_annotated() {
    // MATCH (a:Node)-[:Link]->(b)-[:Link]->(c) RETURN count(c)
    let chain = count_agg(project_pass_dst(
        hop(
            "Link",
            ["b", "e2", "c"],
            hop("Link", ["a", "e1", "b"], anchor_scan("a")),
        ),
        "c",
    ));
    let (annotated, changed) = annotate_expand_all(&chain);
    assert!(changed, "annotation must change the plan");
    let hops = expand_alls(&annotated);
    assert_eq!(hops.len(), 2);
    let hop_b = hop_by_dst(&hops, "b");
    let hop_c = hop_by_dst(&hops, "c");
    assert!(hop_b.id_only(), "hop1 (b) must be id_only");
    assert!(!hop_b.count_only(), "hop1 (b) must not be count_only");
    assert!(
        hop_b.lightweight_source(),
        "hop1 source (a) is unreferenced, so its source column may be lightweight"
    );
    assert!(hop_c.count_only(), "hop2 (c) must be count_only");
    assert!(
        !hop_c.id_only(),
        "hop2 (c) dst is referenced by the aggregate"
    );
}

#[test]
fn referenced_source_keeps_id_only_but_not_lightweight() {
    // MATCH (a:Node)-[:Link]->(b) RETURN a  -> the source `a` is projected
    // out, so hop1 stays id_only but must keep the full source vertex.
    let chain = project_pass_dst(hop("Link", ["a", "e1", "b"], anchor_scan("a")), "a");
    let (annotated, changed) = annotate_expand_all(&chain);
    assert!(changed, "annotation must change the plan");
    let hops = expand_alls(&annotated);
    let hop_b = hop_by_dst(&hops, "b");
    assert!(hop_b.id_only(), "dst (b) unreferenced so hop1 is id_only");
    assert!(
        !hop_b.lightweight_source(),
        "source (a) is referenced by the projection, so it must stay faithful"
    );
}

#[test]
fn dst_property_access_blocks_id_only() {
    // hop1's dst `b` is used by a property access on hop2's filter.
    let mut hop2 = ExpandAllNode::new(1, vec!["Link".to_string()], "OUT");
    hop2.set_step_limit(1);
    hop2.set_col_names(vec!["b".to_string(), "e2".to_string(), "c".to_string()]);
    hop2.set_filter(ctx_expr(Expression::Binary {
        left: Box::new(Expression::Property {
            object: Box::new(Expression::Variable("b".to_string())),
            property: "value".to_string(),
        }),
        op: linkrs_core::types::operators::BinaryOperator::LessThan,
        right: Box::new(Expression::Literal(Value::Int(5))),
    }));
    hop2.add_input(hop("Link", ["a", "e1", "b"], anchor_scan("a")));

    let (annotated, _) = annotate_expand_all(&PlanNodeEnum::ExpandAll(hop2));
    let hops = expand_alls(&annotated);
    assert_eq!(hops.len(), 2);
    assert!(
        !hop_by_dst(&hops, "b").id_only(),
        "hop1 dst referenced by hop2 filter property access"
    );
}

#[test]
fn projected_dst_blocks_id_only_and_count_only() {
    // MATCH (a)-[:R]->(b)-[:R]->(c) RETURN c  -> hop2 dst is projected out.
    let chain = project_pass_dst(
        hop(
            "Link",
            ["b", "e2", "c"],
            hop("Link", ["a", "e1", "b"], anchor_scan("a")),
        ),
        "c",
    );
    let (annotated, _) = annotate_expand_all(&chain);
    let hops = expand_alls(&annotated);
    assert!(hop_by_dst(&hops, "b").id_only(), "hop1 dst only feeds hop2");
    assert!(
        hop_by_dst(&hops, "b").lightweight_source(),
        "hop1 source (a) is unreferenced"
    );
    assert!(
        !hop_by_dst(&hops, "c").id_only(),
        "hop2 dst is the final projection"
    );
    assert!(
        !hop_by_dst(&hops, "c").count_only(),
        "no count-only aggregate above hop2"
    );
}

#[test]
fn count_of_edge_blocks_id_only() {
    // MATCH (a:Node)-[f:Link]->(b) RETURN count(f)  -> the edge `f` is
    // consumed by the aggregate, so id_only must not be applied (it would
    // nullify the edge column and turn count(f) into 0).
    let chain = count_field_agg(
        project_pass_var(hop("Link", ["a", "f", "b"], anchor_scan("a")), "f"),
        "f",
    );
    let (annotated, _) = annotate_expand_all(&chain);
    let hops = expand_alls(&annotated);
    let hop = hop_by_dst(&hops, "b");
    assert!(
        !hop.id_only(),
        "edge variable is referenced by count(f), so id_only must be blocked"
    );
}

fn hop_tagged(edge: &str, vars: [&str; 3], dst_tag: &str, input: PlanNodeEnum) -> PlanNodeEnum {
    let mut expand = ExpandAllNode::new(1, vec![edge.to_string()], "OUT");
    expand.set_step_limit(1);
    expand.set_col_names(vars.iter().map(|s| s.to_string()).collect());
    expand.set_dst_tag(dst_tag.to_string());
    expand.add_input(input);
    PlanNodeEnum::ExpandAll(expand)
}

fn project_prop_col(input: PlanNodeEnum, var: &str, prop: &str, alias: &str) -> PlanNodeEnum {
    let col = linkrs_core::YieldColumn {
        expression: ctx_expr(Expression::Property {
            object: Box::new(Expression::Variable(var.to_string())),
            property: prop.to_string(),
        }),
        alias: alias.to_string(),
    };
    PlanNodeEnum::Project(ProjectNode::new(input, vec![col]).expect("project should build"))
}

#[test]
fn prop_needs_empty_when_only_counted() {
    let chain = count_field_agg(
        project_pass_var(
            hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a")),
            "r",
        ),
        "r",
    );
    let (annotated, _) = annotate_expand_all(&chain);
    let hops = expand_alls(&annotated);
    let hop = hop_by_dst(&hops, "b");
    assert_eq!(
        hop.edge_required_props(),
        Some(&vec![]),
        "counted edge needs no properties"
    );
    assert_eq!(
        hop.dst_required_props(),
        Some(&vec![]),
        "uncounted destination needs no properties"
    );
}

#[test]
fn prop_needs_collects_edge_and_dst_props() {
    let proj = project_prop_col(
        hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a")),
        "r",
        "weight",
        "w",
    );
    let (annotated, _) = annotate_expand_all(&proj);
    let hops = expand_alls(&annotated);
    let hop = hop_by_dst(&hops, "b");
    assert_eq!(
        hop.edge_required_props(),
        Some(&vec!["weight".to_string()]),
        "edge property demand must be collected"
    );
    let proj2 = project_prop_col(
        hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a")),
        "b",
        "name",
        "n",
    );
    let (annotated2, _) = annotate_expand_all(&proj2);
    let hops2 = expand_alls(&annotated2);
    assert_eq!(
        hop_by_dst(&hops2, "b").dst_required_props(),
        Some(&vec!["name".to_string()]),
        "destination property demand must be collected"
    );
}

#[test]
fn full_edge_use_blocks_narrowing() {
    let chain = project_pass_var(
        hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a")),
        "r",
    );
    let (annotated, _) = annotate_expand_all(&chain);
    let hops = expand_alls(&annotated);
    assert_eq!(
        hop_by_dst(&hops, "b").edge_required_props(),
        None,
        "whole-edge return needs the full value"
    );
}

#[test]
fn closed_loop_needs_dst_tag_and_typed_edges() {
    let tagged = hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a"));
    let (annotated, _) = annotate_expand_all(&tagged);
    assert!(
        expand_alls(&annotated)[0].closed_loop(),
        "typed edges with a destination tag form a closed loop"
    );
    let untagged = hop("Link", ["a", "r", "b"], anchor_scan("a"));
    let (annotated2, _) = annotate_expand_all(&untagged);
    assert!(
        expand_alls(&annotated2)[0].closed_loop(),
        "typed edges stay closed-loop syntactically; storage re-verifies schemas"
    );
    let mut open = ExpandAllNode::new(1, vec![], "OUT");
    open.set_step_limit(1);
    open.set_col_names(vec!["a".to_string(), "r".to_string(), "b".to_string()]);
    open.set_dst_tag("Node".to_string());
    open.add_input(anchor_scan("a"));
    let (annotated3, _) = annotate_expand_all(&PlanNodeEnum::ExpandAll(open));
    assert!(
        !expand_alls(&annotated3)[0].closed_loop(),
        "untyped fanout is not a closed loop"
    );
}

#[test]
fn skip_rows_for_passthrough_and_count_but_not_sort() {
    let count_chain = count_field_agg(
        project_pass_var(
            hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a")),
            "r",
        ),
        "r",
    );
    let (annotated, _) = annotate_expand_all(&count_chain);
    assert!(
        expand_alls(&annotated)[0].skip_rows(),
        "bare count terminator is column-capable"
    );
    let sort_chain = PlanNodeEnum::Sort(
        crate::planning::plan::core::nodes::operation::sort_node::SortNode::new(
            hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a")),
            vec![
                crate::planning::plan::core::nodes::operation::sort_node::SortItem {
                    expression: Expression::Variable("r".to_string()),
                    direction: linkrs_core::types::graph_schema::OrderDirection::Asc,
                },
            ],
        )
        .expect("sort should build"),
    );
    let (annotated2, _) = annotate_expand_all(&sort_chain);
    assert!(
        !expand_alls(&annotated2)[0].skip_rows(),
        "sort needs rows and blocks the rowless path"
    );
}

#[test]
fn skip_rows_for_chained_seed_hop() {
    let chain = hop_tagged(
        "Link",
        ["b", "e2", "c"],
        "Node",
        hop_tagged("Link", ["a", "e1", "b"], "Node", anchor_scan("a")),
    );
    let (annotated, _) = annotate_expand_all(&chain);
    let hops = expand_alls(&annotated);
    assert!(
        hop_by_dst(&hops, "b").skip_rows(),
        "intermediate hop feeding a seed-tolerant hop may skip rows"
    );
}

#[test]
fn skip_rows_with_dst_property_passthrough() {
    let proj = project_prop_col(
        hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a")),
        "b",
        "name",
        "n",
    );
    let (annotated, _) = annotate_expand_all(&proj);
    let hops = expand_alls(&annotated);
    let hop = hop_by_dst(&hops, "b");
    assert_eq!(
        hop.dst_required_props(),
        Some(&vec!["name".to_string()]),
        "destination property demand must be collected"
    );
    assert!(
        hop.skip_rows(),
        "direct property passthrough may stay rowless via bypass columns"
    );
}

#[test]
fn skip_rows_with_edge_property_passthrough() {
    let proj = project_prop_col(
        hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a")),
        "r",
        "weight",
        "w",
    );
    let (annotated, _) = annotate_expand_all(&proj);
    let hops = expand_alls(&annotated);
    let hop = hop_by_dst(&hops, "b");
    assert_eq!(
        hop.edge_required_props(),
        Some(&vec!["weight".to_string()]),
        "edge property demand must be collected"
    );
    assert!(
        hop.skip_rows(),
        "direct edge property passthrough may stay rowless via bypass columns"
    );
}

#[test]
fn skip_rows_blocked_for_computed_property() {
    let expr = Expression::Binary {
        left: Box::new(Expression::Property {
            object: Box::new(Expression::Variable("b".to_string())),
            property: "name".to_string(),
        }),
        op: linkrs_core::types::operators::BinaryOperator::Add,
        right: Box::new(Expression::Literal(Value::Int(1))),
    };
    let col = linkrs_core::YieldColumn {
        expression: ctx_expr(expr),
        alias: "n".to_string(),
    };
    let proj = PlanNodeEnum::Project(
        ProjectNode::new(
            hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a")),
            vec![col],
        )
        .expect("project should build"),
    );
    let (annotated, _) = annotate_expand_all(&proj);
    assert!(
        !expand_alls(&annotated)[0].skip_rows(),
        "computed property projections need rows"
    );
}

#[test]
fn skip_rows_blocked_for_count_with_property_arg() {
    let hop = hop_tagged("Link", ["a", "r", "b"], "Node", anchor_scan("a"));
    let mut agg = AggregateNode::new(hop, vec![], vec![AggregateFunction::Count])
        .expect("aggregate should build");
    agg.set_aggregation_args(vec![vec![Expression::Property {
        object: Box::new(Expression::Variable("b".to_string())),
        property: "name".to_string(),
    }]]);
    let (annotated, _) = annotate_expand_all(&PlanNodeEnum::Aggregate(agg));
    let hops = expand_alls(&annotated);
    assert_eq!(
        hop_by_dst(&hops, "b").dst_required_props(),
        Some(&vec!["name".to_string()]),
        "count argument property must be audited"
    );
    assert!(
        !hop_by_dst(&hops, "b").skip_rows(),
        "count over a property keeps the row path"
    );
}

#[test]
fn skip_rows_chained_with_property_tail() {
    let tail_proj = project_prop_col(
        hop_tagged(
            "Link",
            ["b", "e2", "c"],
            "Node",
            hop_tagged("Link", ["a", "e1", "b"], "Node", anchor_scan("a")),
        ),
        "c",
        "name",
        "n",
    );
    let (annotated, _) = annotate_expand_all(&tail_proj);
    let hops = expand_alls(&annotated);
    assert!(
        hop_by_dst(&hops, "c").skip_rows(),
        "tail hop with property passthrough may skip rows"
    );
    assert!(
        hop_by_dst(&hops, "b").skip_rows(),
        "intermediate hop feeding a bypass-capable hop may skip rows"
    );
}
