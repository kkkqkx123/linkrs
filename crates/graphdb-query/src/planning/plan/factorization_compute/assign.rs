use std::collections::HashMap;

use graphdb_core::types::expr::ExpressionId;

use crate::planning::plan::factorization::{FGroupPos, FactorizedSchema, SchemaUtils};

use crate::planning::plan::logical::logical_nodes::graph_ops::LogicalAssignNode;

pub(super) fn assign(
    n: &LogicalAssignNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let schema = child_schemas.first().cloned().unwrap_or_default();
    if schema.num_groups() == 0 {
        let mut out = FactorizedSchema::new();
        let g = out.create_flat_group(false);
        for (alias, expr) in &n.assignments {
            let eid = expr.id().clone();
            out.insert_to_group_and_scope_with_name(eid, Some(alias.clone()), g);
        }
        out.validate_at_most_one_unflat();
        return out;
    }
    let mut expr_store: HashMap<ExpressionId, graphdb_core::Expression> = HashMap::new();
    for (_, expr) in &n.assignments {
        if let Some(rhs) = expr.get_expression() {
            expr_store.insert(expr.id().clone(), rhs);
        }
    }
    let mut out = schema.clone();
    for (alias, expr) in &n.assignments {
        let alias_id = expr.id().clone();
        if out.is_expression_in_scope(&alias_id) {
            continue;
        }
        let mut analyzer =
            crate::optimizer::factorization::GroupDependencyAnalyzer::with_expr_store(
                &out,
                false,
                expr_store.clone(),
            );
        analyzer.visit(&alias_id);
        let dependent = analyzer.dependent_groups().clone();
        let required_flat = analyzer.required_flat_groups().clone();
        for pos in required_flat.iter() {
            if let Some(g) = out.get_group(*pos) {
                if !g.is_flat() {
                    out.flatten_group(*pos);
                }
            }
        }
        let target = if dependent.is_empty() {
            out.groups()
                .iter()
                .enumerate()
                .find(|(_, g)| g.is_flat())
                .map(|(i, _)| i as FGroupPos)
                .unwrap_or_else(|| out.create_flat_group(false))
        } else if dependent.len() == 1 {
            *dependent.iter().next().unwrap()
        } else {
            let mut candidates: Vec<FGroupPos> = dependent
                .iter()
                .filter(|pos| {
                    out.get_group(**pos)
                        .map(|g| !g.is_flat() && !required_flat.contains(pos))
                        .unwrap_or(false)
                })
                .copied()
                .collect();
            candidates.sort_unstable();
            if candidates.is_empty() {
                SchemaUtils::get_leading_group_pos(&dependent, &out)
            } else {
                candidates[0]
            }
        };
        out.insert_to_scope_with_name(alias_id.clone(), alias.clone(), target);
        if let Some(g) = out.get_group_mut(target) {
            if !g.contains(&alias_id) {
                g.insert_expression_with_name(alias_id.clone(), Some(alias.clone()));
            }
        }
    }
    out.validate_at_most_one_unflat();
    out
}
