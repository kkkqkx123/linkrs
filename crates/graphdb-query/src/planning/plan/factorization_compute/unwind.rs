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
        schema.flatten_group(pos);
        schema.insert_name_for_group(n.alias.clone(), pos);
    }
    schema.validate_at_most_one_unflat();
    schema
}
