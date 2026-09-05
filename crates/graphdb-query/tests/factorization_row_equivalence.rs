//! Factorization on/off row-equivalence tests.
//!
//! The streaming engine stores flat row batches, so `Flatten(group=)` is
//! row-preserving by contract: the same input drained with and without
//! the Flatten operator must yield identical rows in identical order.
//! These tests pin that contract by draining real operators (factorization
//! off = bare source, on = source plus Flatten), across both the
//! single-row and the batched flatten paths.
//!
//! Schema-level guards (at most one unflat group, `Flatten(group=)`
//! presence, out-of-range hard error) are asserted here through the
//! public API; deeper unit coverage lives in the crate (`operation.rs`,
//! `join.rs`, `factorization_rewriter.rs` tests).

use std::sync::Arc;

use graphdb_core::Value;
use graphdb_query::executor::streaming::executor::StreamingExecutor;
use graphdb_query::executor::streaming::operators::base::OperatorBase;
use graphdb_query::executor::streaming::operators::flatten::DEFAULT_FLATTEN_BATCH_SIZE;
use graphdb_query::executor::streaming::operators::source_operator::SourceOperator;
use graphdb_query::executor::streaming::operators::spec::{SourceSpec, UnarySpec};
use graphdb_query::executor::streaming::operators::unary_operator::{
    UnaryOperator, UnaryOperatorKind,
};
use graphdb_query::executor::streaming::slot::SlotLayout;

fn col_names() -> Vec<String> {
    vec!["id".to_string(), "name".to_string()]
}

fn input_rows() -> Vec<Vec<Value>> {
    vec![
        vec![Value::Int(1), Value::string("a")],
        vec![Value::Int(2), Value::string("b")],
        vec![Value::Int(3), Value::string("c")],
        vec![Value::Int(4), Value::string("d")],
        vec![Value::Int(5), Value::string("e")],
    ]
}

fn layout() -> Arc<SlotLayout> {
    Arc::new(SlotLayout::from_names(&col_names()))
}

/// Factorization off: bare in-memory source.
fn source_executor() -> StreamingExecutor {
    let spec = SourceSpec::ScanVertices {
        rows: input_rows(),
        col_names: col_names(),
    };
    let op = SourceOperator::from_spec(&spec, None, layout());
    StreamingExecutor::Source(OperatorBase::new(0).with_output_layout(layout()), op)
}

/// Factorization on: the same source plus one Flatten operator.
fn flatten_executor(batch_size: usize, group_columns: Vec<String>) -> StreamingExecutor {
    let kind = UnaryOperatorKind::Flatten {
        group_pos: 1,
        group_columns,
        expected_groups: Some(2),
        current_idx: 0,
        size_to_flatten: 0,
        saved_sel_vector: None,
        buffered_chunk: None,
        batch_size,
    };
    StreamingExecutor::Unary(
        OperatorBase::new(1).with_output_layout(layout()),
        Box::new(source_executor()),
        UnaryOperator::new(kind, layout()),
    )
}

fn drain(exec: &mut StreamingExecutor) -> Vec<Vec<Value>> {
    exec.open().expect("open");
    let mut out = Vec::new();
    while let Some(chunk) = exec.advance().expect("advance") {
        out.extend(chunk.rows.clone());
    }
    out
}

#[test]
fn flatten_off_matches_flatten_on_rows() {
    let expected = input_rows();
    let off = drain(&mut source_executor());
    assert_eq!(off, expected);
    // Batched path (production default morsel size).
    let on_batched = drain(&mut flatten_executor(
        DEFAULT_FLATTEN_BATCH_SIZE,
        vec!["name".to_string()],
    ));
    assert_eq!(on_batched, expected);
    // Single-row path must replay the same rows in the same order.
    let on_single = drain(&mut flatten_executor(1, vec!["name".to_string()]));
    assert_eq!(on_single, expected);
}

#[test]
fn flatten_preserves_output_column_names() {
    let mut exec = flatten_executor(DEFAULT_FLATTEN_BATCH_SIZE, vec!["id".to_string()]);
    exec.open().expect("open");
    let chunk = exec.advance().expect("advance").expect("chunk");
    assert_eq!(chunk.col_names(), col_names());
}

#[test]
fn flatten_spec_mapping_opens_and_stale_position_fails() {
    // Position within the snapshotted group count opens through the
    // immutable spec path.
    let valid = UnarySpec::Flatten {
        group_pos: 1,
        group_columns: vec!["name".to_string()],
        expected_groups: Some(2),
    };
    let mut op = UnaryOperator::from_spec(&valid, layout());
    let mut input = source_executor();
    assert!(op.open(&mut input).is_ok());

    // Stale position (plan/executor drift) fails loudly at open.
    let stale = UnarySpec::Flatten {
        group_pos: 5,
        group_columns: vec!["name".to_string()],
        expected_groups: Some(2),
    };
    let mut op = UnaryOperator::from_spec(&stale, layout());
    let mut input = source_executor();
    let err = op.open(&mut input).expect_err("stale position must fail");
    assert!(err.to_string().contains("out of range"));
}

// ── Schema-level guards through the public API ──────────────────────────

use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::expr::{ExpressionId, ExpressionMeta};
use graphdb_core::Expression;
use graphdb_query::optimizer::factorization::{FactorizationRewriter, RemoveFactorizationRewriter};
use graphdb_query::planning::plan::core::node_id_generator::next_node_id;
use graphdb_query::planning::plan::factorization::{FactorizedSchema, FactorizedSchemaCompute};
use graphdb_query::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use graphdb_query::planning::plan::logical::logical_nodes::access::{
    LogicalGetNeighborsNode, LogicalScanVerticesNode,
};
use graphdb_query::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode;

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

fn neighbors_with_output(out_expr: ContextualExpression) -> LogicalNodeEnum {
    LogicalNodeEnum::GetNeighbors(LogicalGetNeighborsNode {
        id: next_node_id(),
        space_id: 1,
        src_vids: "1".to_string(),
        edge_types: vec!["knows".to_string()],
        direction: "OUT".to_string(),
        edge_props: vec![],
        tag_props: vec![],
        expression: Some(out_expr),
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

fn ctx_var(var: &str) -> (ContextualExpression, ExpressionId) {
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let id = ctx.register_expression(ExpressionMeta::new(Expression::Variable(var.to_string())));
    (ContextualExpression::new(id.clone(), ctx), id)
}

#[test]
fn rewritten_full_outer_carries_flatten_on_both_sides_within_invariant() {
    use graphdb_query::planning::plan::logical::logical_nodes::join::LogicalFullOuterJoinNode;
    let (left_key, left_id) = ctx_var("lk");
    let (right_key, right_id) = ctx_var("rk");
    let mut join = LogicalNodeEnum::FullOuterJoin(LogicalFullOuterJoinNode {
        id: next_node_id(),
        left: Box::new(neighbors_with_output(left_key.clone())),
        right: Box::new(neighbors_with_output(right_key.clone())),
        hash_keys: vec![left_key],
        probe_keys: vec![right_key],
        deps: vec![],
        output_var: None,
        col_names: vec![],
        column_types: vec![],
    });
    let mut rewriter = FactorizationRewriter::new();
    rewriter.rewrite(&mut join);
    // `Flatten(group=)` presence on both key sides.
    if let LogicalNodeEnum::FullOuterJoin(n) = &join {
        assert!(RemoveFactorizationRewriter::has_flatten_public(&n.left));
        assert!(RemoveFactorizationRewriter::has_flatten_public(&n.right));
    } else {
        panic!("expected full outer join");
    }
    // The rewritten shape still honors at most one unflat group when its
    // schema is recomputed from matching child schemas.
    let mut left_schema = FactorizedSchema::new();
    let lg0 = left_schema.create_flat_group(false);
    let lg1 = left_schema.create_group();
    left_schema.insert_to_group_and_scope(ExpressionId::new(1), lg0);
    left_schema.insert_to_group_and_scope(left_id, lg1);
    let mut right_schema = FactorizedSchema::new();
    let rg0 = right_schema.create_flat_group(false);
    let rg1 = right_schema.create_group();
    right_schema.insert_to_group_and_scope(ExpressionId::new(2), rg0);
    right_schema.insert_to_group_and_scope(right_id, rg1);
    // Full-outer flattens both key sides, so the recomputed output holds
    // the invariant with no surviving unflat group.
    let mut check = join.clone();
    let out = check.compute_factorized_schema(&[left_schema, right_schema]);
    out.validate_at_most_one_unflat();
}

#[test]
fn right_join_multi_unflat_keeps_left_side_alive() {
    use graphdb_query::planning::plan::logical::logical_nodes::join::LogicalRightJoinNode;
    // Right key group is unflat: Right policy flattens the build (right)
    // side fully while the left side keeps its single unflat group.
    let mut left_schema = FactorizedSchema::new();
    let lg0 = left_schema.create_flat_group(false);
    let lg1 = left_schema.create_group();
    left_schema.insert_to_group_and_scope(ExpressionId::new(1), lg0);
    left_schema.insert_to_group_and_scope(ExpressionId::new(11), lg1);
    let mut right_schema = FactorizedSchema::new();
    let rg0 = right_schema.create_flat_group(false);
    let rg1 = right_schema.create_group();
    right_schema.insert_to_group_and_scope(ExpressionId::new(2), rg0);
    let (key_for_schema, key_id) = ctx_var("k");
    right_schema.insert_to_group_and_scope(key_id, rg1);
    let mut node = LogicalNodeEnum::RightJoin(LogicalRightJoinNode {
        id: next_node_id(),
        left: Box::new(scan()),
        right: Box::new(scan()),
        hash_keys: vec![],
        probe_keys: vec![key_for_schema],
        deps: vec![scan(), scan()],
        output_var: None,
        col_names: vec![],
        column_types: vec![],
    });
    let out = node.compute_factorized_schema(&[left_schema, right_schema]);
    out.validate_at_most_one_unflat();
    assert_eq!(out.unflat_group_pos(), Some(lg1));
}

#[test]
#[should_panic(expected = "out of range")]
fn flatten_group_out_of_range_is_hard_error() {
    let mut scan_node = scan();
    let schema = scan_node.compute_factorized_schema(&[]);
    let mut flatten = LogicalNodeEnum::Flatten(LogicalFlattenNode::new(99, scan()));
    let _ = flatten.compute_factorized_schema(&[schema]);
}
