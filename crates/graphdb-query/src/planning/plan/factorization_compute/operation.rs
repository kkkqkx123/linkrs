use std::collections::{HashMap, HashSet};

use graphdb_core::types::expr::ExpressionId;

use crate::optimizer::factorization::flatten_resolver::{FlattenAll, FlattenAllButOne};
use crate::planning::plan::factorization::{FGroupPos, FactorizedSchema, SchemaUtils};

use crate::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode;
use crate::planning::plan::logical::logical_nodes::operation::{
    LogicalAggregateNode, LogicalDedupNode, LogicalFilterNode, LogicalLimitNode,
    LogicalProjectNode, LogicalSampleNode, LogicalSkipNode, LogicalSortNode, LogicalTopNNode,
    LogicalWindowNode,
};

pub(super) fn project(
    n: &LogicalProjectNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let schema = child_schemas.first().cloned().unwrap_or_default();
    if schema.num_groups() == 0 {
        let mut out = FactorizedSchema::new();
        let g = out.create_flat_group(false);
        for col in &n.columns {
            let alias = col.alias.clone();
            let eid = col.expression.id().clone();
            out.insert_to_group_and_scope_with_name(eid, Some(alias), g);
        }
        out.validate_at_most_one_unflat();
        return out;
    }
    let mut expr_store: HashMap<ExpressionId, graphdb_core::Expression> = HashMap::new();
    for col in &n.columns {
        if let Some(expr) = col.expression.get_expression() {
            expr_store.insert(col.expression.id().clone(), expr);
        }
    }
    let mut out = schema.clone();
    for col in &n.columns {
        let alias_id = col.expression.id().clone();
        let alias_name = col.alias.clone();
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
        out.insert_to_scope_with_name(alias_id.clone(), alias_name.clone(), target);
        if let Some(g) = out.get_group_mut(target) {
            if !g.contains(&alias_id) {
                if !g.contains_name(&alias_name) {
                    g.insert_expression_with_name(alias_id.clone(), Some(alias_name.clone()));
                } else {
                    // The alias shadows a name already present in the group
                    // (e.g. an aggregate argument re-projected over a child
                    // column with the same output name). Keep the first name
                    // mapping and register only the id so scope lookups stay
                    // consistent with the runtime shadowing.
                    g.insert_expression(alias_id.clone());
                }
            }
        }
    }
    out.validate_at_most_one_unflat();
    out
}

pub(super) fn filter(
    n: &LogicalFilterNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    let pred_id = n.condition.id().clone();
    let mut store = HashMap::new();
    if let Some(expr) = n.condition.get_expression() {
        store.insert(pred_id.clone(), expr);
    }
    let mut analyzer = crate::optimizer::factorization::GroupDependencyAnalyzer::with_expr_store(
        &schema, false, store,
    );
    analyzer.visit(&pred_id);
    let dependent = analyzer.dependent_groups().clone();
    let required = analyzer.required_flat_groups().clone();
    let mut to_flatten: HashSet<FGroupPos> = HashSet::new();
    for pos in dependent.iter().chain(required.iter()) {
        if let Some(g) = schema.get_group(*pos) {
            if !g.is_flat() {
                to_flatten.insert(*pos);
            }
        }
    }
    for pos in to_flatten {
        schema.flatten_group(pos);
    }
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn aggregate(
    n: &LogicalAggregateNode,
    _child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut out = FactorizedSchema::new();
    let g = out.create_flat_group(false);
    for expr in &n.group_key_exprs {
        let eid = super::resolve_id(expr);
        let name = expr.to_expression_string();
        out.insert_to_group_and_scope_with_name(eid, Some(name), g);
    }
    out.validate_at_most_one_unflat();
    out
}

pub(super) fn flatten(
    n: &LogicalFlattenNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut schema = if let Some(cs) = child_schemas.first() {
        cs.clone()
    } else {
        FactorizedSchema::new()
    };
    if (n.group_pos as usize) < schema.num_groups() {
        schema.flatten_group(n.group_pos);
    }
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn sort(_n: &LogicalSortNode, child_schemas: &[FactorizedSchema]) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    let groups = schema.groups_pos_in_scope();
    let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, &schema);
    for pos in to_flatten {
        schema.flatten_group(pos);
    }
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn top_n(_n: &LogicalTopNNode, child_schemas: &[FactorizedSchema]) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    let groups = schema.groups_pos_in_scope();
    let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, &schema);
    for pos in to_flatten {
        schema.flatten_group(pos);
    }
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn window(
    _n: &LogicalWindowNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    let groups = schema.groups_pos_in_scope();
    let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, &schema);
    for pos in to_flatten {
        schema.flatten_group(pos);
    }
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn dedup(_n: &LogicalDedupNode, child_schemas: &[FactorizedSchema]) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    let groups = schema.groups_pos_in_scope();
    let to_flatten = FlattenAll::get_groups_pos_to_flatten_for_groups(&groups, &schema);
    for pos in to_flatten {
        schema.flatten_group(pos);
    }
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn limit(_n: &LogicalLimitNode, child_schemas: &[FactorizedSchema]) -> FactorizedSchema {
    child_schemas.first().cloned().unwrap_or_default()
}

pub(super) fn skip(_n: &LogicalSkipNode, child_schemas: &[FactorizedSchema]) -> FactorizedSchema {
    child_schemas.first().cloned().unwrap_or_default()
}

pub(super) fn sample(
    _n: &LogicalSampleNode,
    child_schemas: &[FactorizedSchema],
) -> FactorizedSchema {
    child_schemas.first().cloned().unwrap_or_default()
}
