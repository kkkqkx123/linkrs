use std::collections::HashMap;

use crate::optimizer::factorization::flatten_resolver::FlattenAllButOne;
use crate::planning::plan::factorization::FactorizedSchema;

use crate::planning::plan::logical::logical_nodes::control_flow::{
    LogicalLoopNode, LogicalSelectNode,
};

pub(super) fn select(
    n: &LogicalSelectNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    let cond_id = n.condition.id().clone();
    let mut store = HashMap::new();
    if let Some(expr) = n.condition.get_expression() {
        store.insert(cond_id.clone(), expr);
    }
    let to_flatten =
        FlattenAllButOne::get_groups_pos_to_flatten_for_expr(&cond_id, &schema, &store);
    for pos in to_flatten {
        schema.flatten_group(pos);
    }
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn loop_node(
    n: &LogicalLoopNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    let cond_id = n.condition.id().clone();
    let mut store = HashMap::new();
    if let Some(expr) = n.condition.get_expression() {
        store.insert(cond_id.clone(), expr);
    }
    let to_flatten =
        FlattenAllButOne::get_groups_pos_to_flatten_for_expr(&cond_id, &schema, &store);
    for pos in to_flatten {
        schema.flatten_group(pos);
    }
    schema.validate_at_most_one_unflat();
    schema
}

/// Transaction and argument nodes carry no factorized state across their
/// boundary, hence they start a fresh empty flat schema instead of forwarding
/// child scope.
pub(super) fn passthrough(_child_schemas: &[FactorizedSchema]) -> FactorizedSchema {
    let mut schema = FactorizedSchema::new();
    schema.create_flat_group(false);
    schema.validate_at_most_one_unflat();
    schema
}
