use crate::planning::plan::factorization::FactorizedSchema;

use crate::planning::plan::logical::logical_nodes::graph_ops::LogicalUnwindNode;

use super::resolve_id;

pub(super) fn unwind(
    n: &LogicalUnwindNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    let list_id = resolve_id(&n.list_expression);
    let is_list_literal = n
        .list_expression
        .expression()
        .map(|meta| matches!(meta.inner(), graphdb_core::Expression::List(_)))
        .unwrap_or(false);
    if is_list_literal {
        let group = schema.create_group();
        if !schema.is_expression_in_scope(&list_id) {
            schema.insert_to_group_and_scope_with_name(list_id, Some(n.alias.clone()), group);
        } else {
            schema.insert_name_for_group(n.alias.clone(), group);
        }
    } else if let Some(pos) = schema.get_group_pos(&list_id) {
        // Baseline always builds a fresh group for the unwind output
        // (`logical_unwind.cpp:16-23`): flatten the input group first, then
        // create a new unflat group holding the element alias.
        schema.flatten_group(pos);
        let out = schema.create_group();
        schema.insert_name_for_group(n.alias.clone(), out);
    } else {
        // Unresolved list expression (bare variable, parameter or function
        // result with no tracked id). The output cardinality is unknown, so
        // conservatively flatten every unflat group, then create a fresh
        // unflat group for the alias instead of reusing a flat group.
        schema.flatten_all();
        let out = schema.create_group();
        schema.insert_name_for_group(n.alias.clone(), out);
    }
    schema.validate_at_most_one_unflat();
    schema
}
