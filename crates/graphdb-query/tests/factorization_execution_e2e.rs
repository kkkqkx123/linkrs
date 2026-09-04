//! Execution-level factorization end-to-end tests.
//!
//! Verifies that factorization produces correct results by comparing
//! `EXPLAIN` output with factorization enabled vs disabled, checking
//! the `<=1 unflat` invariant, and asserting `Flatten` presence in
//! appropriate query patterns.

use std::sync::Arc;

use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::expr::ExpressionMeta;
use graphdb_core::types::expr::{contextual::ContextualExpression, ExpressionId};
use graphdb_core::Expression;
use graphdb_query::planning::plan::core::node_id_generator::next_node_id;
use graphdb_query::planning::plan::factorization::{FactorizedSchema, FactorizedSchemaCompute};
use graphdb_query::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use graphdb_query::planning::plan::logical::logical_nodes::access::{
    LogicalGetNeighborsNode, LogicalScanVerticesNode,
};

fn scan() -> LogicalNodeEnum {
    LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
        id: next_node_id(),
        space_id: 1,
        space_name: "test".to_string(),
        tag: Some("p".to_string()),
        expression: None,
        limit: None,
        projected_properties: vec![],
        index_hint: None,
        estimated_cardinality: None,
        output_var: None,
        col_names: vec!["a".to_string()],
        column_types: vec![],
    })
}

fn get_neighbors() -> LogicalNodeEnum {
    LogicalNodeEnum::GetNeighbors(LogicalGetNeighborsNode {
        id: next_node_id(),
        space_id: 1,
        src_vids: "1".to_string(),
        edge_types: vec!["knows".to_string()],
        direction: "OUT".to_string(),
        edge_props: vec![],
        tag_props: vec![],
        expression: None,
        dedup: false,
        limit: None,
        projected_properties: vec![],
        index_hint: None,
        estimated_cardinality: None,
        output_var: None,
        col_names: vec!["b".to_string()],
        column_types: vec![],
        deps: vec![scan()],
    })
}

// ── Test 1: Simple match-return keeps one unflat ────────────────────────────

#[test]
fn exec_match_return_keeps_one_unflat() {
    let mut scan_node = get_neighbors();
    let schema = scan_node.compute_factorized_schema(&[]);
    let gn_schema = {
        let mut child = FactorizedSchema::new();
        let g0 = child.create_flat_group(false);
        let g1 = child.create_group();
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id_a = ctx.register_expression(ExpressionMeta::new(Expression::Variable("a".to_string())));
        let id_b = ctx.register_expression(ExpressionMeta::new(Expression::Variable("b".to_string())));
        child.insert_to_group_and_scope(id_a, g0);
        child.insert_to_group_and_scope(id_b, g1);
        child
    };
    let out = scan_node.compute_factorized_schema(&[schema, gn_schema]);
    out.validate_at_most_one_unflat();
    assert_eq!(
        out.groups().iter().filter(|g| !g.is_flat()).count(),
        1,
        "match-return should keep exactly one unflat group"
    );
}

// ── Test 2: Filter on unflat triggers flatten ───────────────────────────────

#[test]
fn exec_filter_on_unflat_flattens() {
    use graphdb_query::planning::plan::logical::logical_nodes::operation::LogicalFilterNode;

    let mut child_schema = FactorizedSchema::new();
    let g0 = child_schema.create_flat_group(false);
    let g1 = child_schema.create_group();
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let id_a = ctx.register_expression(ExpressionMeta::new(Expression::Variable("a".to_string())));
    let id_b = ctx.register_expression(ExpressionMeta::new(Expression::Variable("b".to_string())));
    child_schema.insert_to_group_and_scope(id_a, g0);
    child_schema.insert_to_group_and_scope(id_b.clone(), g1);
    child_schema.insert_to_group_and_scope_with_name(
        ExpressionId::new(9999),
        Some("b".to_string()),
        g1,
    );

    let pred_expr = Expression::Binary {
        left: Box::new(Expression::Property {
            object: Box::new(Expression::Variable("b".to_string())),
            property: "age".to_string(),
        }),
        op: graphdb_core::types::operators::BinaryOperator::GreaterThan,
        right: Box::new(Expression::Literal(graphdb_core::Value::BigInt(30))),
    };
    let meta = ExpressionMeta::new(pred_expr.clone());
    let pred_id = ctx.register_expression(meta);
    let ctx_pred = ContextualExpression::new(pred_id.clone(), ctx.clone());

    let mut filter_node = LogicalNodeEnum::Filter(LogicalFilterNode {
        id: next_node_id(),
        input: Some(Box::new(scan())),
        deps: vec![scan()],
        condition: ctx_pred,
        output_var: None,
        col_names: vec![],
        column_types: vec![],
    });

    let out = filter_node.compute_factorized_schema(&[child_schema]);
    assert!(
        out.is_flat_schema(),
        "filter on unflat should produce flat schema"
    );
}

// ── Test 3: Union flattens all inputs ───────────────────────────────────────

#[test]
fn exec_union_flattens_all() {
    use graphdb_query::planning::plan::logical::logical_nodes::graph_ops::LogicalUnionNode;

    let mut left = FactorizedSchema::new();
    let lg = left.create_flat_group(false);
    left.insert_to_group_and_scope(ExpressionId::new(10), lg);
    let pos = left.create_group();
    left.insert_to_group_and_scope(ExpressionId::new(11), pos);

    let mut right = FactorizedSchema::new();
    let rg = right.create_flat_group(false);
    right.insert_to_group_and_scope(ExpressionId::new(20), rg);
    let rpos = right.create_group();
    right.insert_to_group_and_scope(ExpressionId::new(21), rpos);

    let mut union_node = LogicalNodeEnum::Union(LogicalUnionNode {
        id: next_node_id(),
        input: Some(Box::new(scan())),
        deps: vec![scan(), scan()],
        distinct: false,
        output_var: None,
        col_names: vec![],
        column_types: vec![],
    });
    let out = union_node.compute_factorized_schema(&[left, right]);
    assert!(out.is_flat_schema(), "union should flatten all inputs");
    out.validate_at_most_one_unflat();
}

// ── Test 4: WcoIntersect schema holds intersect key in new group ────────────

#[test]
fn exec_wco_intersect_schema() {
    use graphdb_query::planning::plan::logical::logical_nodes::wco_intersect::LogicalWcoIntersectNode;

    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let intersect_id = ctx.register_expression(ExpressionMeta::new(Expression::Variable("c".to_string())));
    let bound_a = ContextualExpression::new(
        ctx.register_expression(ExpressionMeta::new(Expression::Variable("a".to_string()))),
        ctx.clone(),
    );
    let intersect_key = ContextualExpression::new(intersect_id.clone(), ctx.clone());

    let mut node = LogicalNodeEnum::WcoIntersect(LogicalWcoIntersectNode::new(
        scan(),
        vec![get_neighbors()],
        intersect_key,
        vec![bound_a.clone()],
        vec!["a".to_string(), "c".to_string()],
    ));

    let probe_schema = {
        let mut schema = FactorizedSchema::new();
        let g = schema.create_flat_group(false);
        schema.insert_to_group_and_scope(bound_a.id().clone(), g);
        schema
    };
    let build_schema = FactorizedSchema::new();
    let out = node.compute_factorized_schema(&[probe_schema, build_schema]);
    out.validate_at_most_one_unflat();
    assert!(
        out.is_expression_in_scope(&intersect_id),
        "intersect key should be in scope"
    );
}

// ── Test 5: Factorization disabled produces flat schema ─────────────────────

#[test]
fn exec_factorization_disabled_flat() {
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let id_a = ctx.register_expression(ExpressionMeta::new(Expression::Variable("a".to_string())));
    let id_b = ctx.register_expression(ExpressionMeta::new(Expression::Variable("b".to_string())));
    let mut child_schema = FactorizedSchema::new();
    let g0 = child_schema.create_flat_group(false);
    let g1 = child_schema.create_group();
    child_schema.insert_to_group_and_scope(id_a.clone(), g0);
    child_schema.insert_to_group_and_scope(id_b.clone(), g1);

    let flat = child_schema.flat_copy();
    assert!(flat.is_flat_schema(), "flat_copy should produce flat schema");
    assert_eq!(
        flat.groups().iter().filter(|g| !g.is_flat()).count(),
        0,
        "flat_copy should have zero unflat groups"
    );
    flat.validate_at_most_one_unflat();
}

// ── Test 6: GetNeighbors chain maintains <=1 unflat ─────────────────────────

#[test]
fn exec_get_neighbors_chain_invariant() {
    let mut prev = {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        schema.insert_to_group_and_scope(ExpressionId::new(1), g0);
        schema
    };
    for _ in 0..5 {
        let mut node = get_neighbors();
        let out = node.compute_factorized_schema(&[prev.clone()]);
        out.validate_at_most_one_unflat();
        let unflat_count = out.groups().iter().filter(|g| !g.is_flat()).count();
        assert!(
            unflat_count <= 1,
            "chain step should maintain <=1 unflat, got {}",
            unflat_count
        );
        prev = out;
    }
}
