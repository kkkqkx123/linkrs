use std::collections::{HashMap, HashSet};

use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::ExpressionId;

use crate::optimizer::factorization::flatten_resolver::{FlattenAll, FlattenAllButOne};
use crate::planning::plan::factorization::{
    FGroupPos, FactorizedSchema, FactorizedSchemaCompute, SchemaUtils,
};

use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

fn resolve_id(expr: &ContextualExpression) -> ExpressionId {
    expr.id().clone()
}

/// Register bare output names on an expansion output group.
///
/// Traversal nodes fan out into a new unflat group but carry no tracked
/// `ExpressionId` for the output. Downstream `Variable` references resolve
/// by name through `expression_name_to_group`, so the aliases must be
/// recorded explicitly. Without this the output group stays empty and every
/// downstream use falls back to flatten-all via
/// `GroupDependencyAnalyzer::mark_unresolved`.
fn register_output_names(
    schema: &mut FactorizedSchema,
    output_var: Option<&str>,
    col_names: &[String],
    group: FGroupPos,
) {
    if let Some(var) = output_var {
        schema.insert_name_for_group(var.to_string(), group);
    }
    for name in col_names {
        schema.insert_name_for_group(name.clone(), group);
    }
}

/// Schema for bidirectional expansion over two child schemas.
///
/// Child order follows the node inputs: the first child is the probe side
/// and is flattened before fan-out, the second child is the build side
/// whose bindings are merged into scope. Any remaining nested group is
/// flattened so the expansion output group is the single unflat group.
/// `output_aliases` are registered on the new group via
/// `register_output_names` semantics so downstream variables resolve
/// precisely instead of via flatten-all fallback.
fn bi_expand_schema(child_schemas: &[FactorizedSchema], output_aliases: &[String]) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    if schema.has_unflat_group() {
        if let Some(pos) = schema.unflat_group_pos() {
            schema.flatten_group(pos);
        }
    }
    if let Some(build) = child_schemas.get(1) {
        let mapping = schema.merge_groups_from(build);
        for (expr_id, gpos) in build.expression_to_group_iter() {
            let new_pos = mapping.get(gpos).copied().unwrap_or(*gpos);
            schema.insert_to_scope_may_repeat(expr_id.clone(), new_pos);
        }
        if schema.has_unflat_group() {
            if let Some(pos) = schema.unflat_group_pos() {
                schema.flatten_group(pos);
            }
        }
    }
    // The expansion output carries no tracked expression identity, so
    // output aliases are registered as bare names for downstream
    // `Variable` resolution (see `register_output_names`).
    let out = schema.create_group();
    for alias in output_aliases {
        schema.insert_name_for_group(alias.clone(), out);
    }
    schema.validate_at_most_one_unflat();
    schema
}

impl FactorizedSchemaCompute for LogicalNodeEnum {
    fn compute_factorized_schema(
        &mut self,
        child_schemas: &[FactorizedSchema],
    ) -> FactorizedSchema {
        match self {
            LogicalNodeEnum::ScanVertices(n) => {
                let mut schema = FactorizedSchema::new();
                let g = schema.create_flat_group(false);
                if let Some(expr) = &n.expression {
                    let eid = resolve_id(expr);
                    let name = n
                        .col_names()
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "scan".to_string());
                    schema.insert_to_group_and_scope_with_name(eid, Some(name), g);
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::ScanEdges(n) => {
                let mut schema = FactorizedSchema::new();
                let g = schema.create_flat_group(false);
                if let Some(expr) = &n.expression {
                    let eid = resolve_id(expr);
                    let name = n
                        .col_names()
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "scan".to_string());
                    schema.insert_to_group_and_scope_with_name(eid, Some(name), g);
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::GetVertices(n) => {
                let mut schema = if let Some(cs) = child_schemas.first() {
                    cs.clone()
                } else {
                    FactorizedSchema::new()
                };
                if schema.num_groups() == 0 {
                    let g = schema.create_flat_group(false);
                    {
                        let eid = resolve_id(&n.src_ref);
                        let name = n
                            .col_names()
                            .first()
                            .cloned()
                            .unwrap_or_else(|| "getv".to_string());
                        schema.insert_to_group_and_scope_with_name(eid, Some(name), g);
                    }
                    if let Some(expr) = &n.expression {
                        let eid = resolve_id(expr);
                        if !schema.is_expression_in_scope(&eid) {
                            let name = expr.to_expression_string();
                            schema.insert_to_group_and_scope_with_name(eid, Some(name), g);
                        }
                    }
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::GetNeighbors(n) => {
                let mut schema = if let Some(cs) = child_schemas.first() {
                    cs.clone()
                } else {
                    FactorizedSchema::new()
                };
                if schema.num_groups() == 0 {
                    schema.create_flat_group(false);
                }
                if schema.has_unflat_group() {
                    if let Some(pos) = schema.unflat_group_pos() {
                        schema.flatten_group(pos);
                    }
                }
                // The input side is the probe side: fan-out over an unflat
                // probe would duplicate nested rows, so it is flattened
                // above. The new group holds the expansion output.
                let output_group = schema.create_group();
                // Register the output explicitly when the node carries an
                // output expression; otherwise register bare output names
                // so downstream variables resolve precisely instead of
                // via flatten-all fallback.
                if let Some(expr) = &n.expression {
                    let eid = resolve_id(expr);
                    if !schema.is_expression_in_scope(&eid) {
                        let name = n
                            .col_names()
                            .first()
                            .cloned()
                            .unwrap_or_else(|| "neighbors".to_string());
                        schema.insert_to_group_and_scope_with_name(eid, Some(name), output_group);
                    }
                } else {
                    register_output_names(
                        &mut schema,
                        n.output_var.as_deref(),
                        &n.col_names,
                        output_group,
                    );
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::Flatten(n) => {
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
            LogicalNodeEnum::Project(n) => {
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
                let mut expr_store: HashMap<ExpressionId, graphdb_core::Expression> =
                    HashMap::new();
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
                            g.insert_expression_with_name(
                                alias_id.clone(),
                                Some(alias_name.clone()),
                            );
                        }
                    }
                }
                out.validate_at_most_one_unflat();
                out
            }
            LogicalNodeEnum::Filter(filter) => {
                let mut schema = child_schemas.first().cloned().unwrap_or_default();
                let pred_id = filter.condition.id().clone();
                let mut store = HashMap::new();
                if let Some(expr) = filter.condition.get_expression() {
                    store.insert(pred_id.clone(), expr);
                }
                let mut analyzer =
                    crate::optimizer::factorization::GroupDependencyAnalyzer::with_expr_store(
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
            LogicalNodeEnum::Aggregate(n) => {
                let mut out = FactorizedSchema::new();
                let g = out.create_flat_group(false);
                for expr in &n.group_key_exprs {
                    let eid = resolve_id(expr);
                    let name = expr.to_expression_string();
                    out.insert_to_group_and_scope_with_name(eid, Some(name), g);
                }
                out.validate_at_most_one_unflat();
                out
            }
            LogicalNodeEnum::Sort(_) => {
                let mut schema = child_schemas.first().cloned().unwrap_or_default();
                let groups = schema.groups_pos_in_scope();
                let to_flatten =
                    FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, &schema);
                for pos in to_flatten {
                    schema.flatten_group(pos);
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::TopN(_) => {
                let mut schema = child_schemas.first().cloned().unwrap_or_default();
                let groups = schema.groups_pos_in_scope();
                let to_flatten =
                    FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, &schema);
                for pos in to_flatten {
                    schema.flatten_group(pos);
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::Window(_) => {
                let mut schema = child_schemas.first().cloned().unwrap_or_default();
                let groups = schema.groups_pos_in_scope();
                let to_flatten =
                    FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, &schema);
                for pos in to_flatten {
                    schema.flatten_group(pos);
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::Dedup(_) => {
                let mut schema = child_schemas.first().cloned().unwrap_or_default();
                let groups = schema.groups_pos_in_scope();
                let to_flatten = FlattenAll::get_groups_pos_to_flatten_for_groups(&groups, &schema);
                for pos in to_flatten {
                    schema.flatten_group(pos);
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::Limit(_) | LogicalNodeEnum::Sample(_) => {
                let schema = child_schemas.first().cloned().unwrap_or_default();
                schema
            }
            LogicalNodeEnum::InnerJoin(_)
            | LogicalNodeEnum::LeftJoin(_)
            | LogicalNodeEnum::RightJoin(_)
            | LogicalNodeEnum::CrossJoin(_)
            | LogicalNodeEnum::FullOuterJoin(_)
            | LogicalNodeEnum::SemiJoin(_) => {
                if child_schemas.len() >= 2 {
                    let left = &child_schemas[0];
                    let right = &child_schemas[1];
                    let mut merged = left.clone();
                    let mapping = merged.merge_groups_from(right);
                    for (expr_id, gpos) in right.expression_to_group_iter() {
                        let new_pos = mapping.get(gpos).copied().unwrap_or(*gpos);
                        merged.insert_to_scope_may_repeat(expr_id.clone(), new_pos);
                    }
                    if merged.has_unflat_group() {
                        let unflat_count = merged.groups().iter().filter(|g| !g.is_flat()).count();
                        if unflat_count > 1 {
                            let mut first = true;
                            for i in 0..merged.num_groups() {
                                let pos = i as FGroupPos;
                                if let Some(g) = merged.get_group(pos) {
                                    if !g.is_flat() {
                                        if first {
                                            first = false;
                                        } else {
                                            merged.flatten_group(pos);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    merged.validate_at_most_one_unflat();
                    merged
                } else {
                    child_schemas.first().cloned().unwrap_or_default()
                }
            }
            LogicalNodeEnum::Traverse(n) => {
                let mut schema = child_schemas.first().cloned().unwrap_or_default();
                // Single-input expansion: the input is the probe side and
                // is flattened before fan-out.
                if schema.has_unflat_group() {
                    if let Some(pos) = schema.unflat_group_pos() {
                        schema.flatten_group(pos);
                    }
                }
                let out = schema.create_group();
                if let Some(alias) = &n.vertex_alias {
                    schema.insert_name_for_group(alias.clone(), out);
                }
                if let Some(alias) = &n.edge_alias {
                    schema.insert_name_for_group(alias.clone(), out);
                }
                register_output_names(&mut schema, n.output_var.as_deref(), &n.col_names, out);
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::Expand(n) => {
                let mut schema = child_schemas.first().cloned().unwrap_or_default();
                if schema.has_unflat_group() {
                    if let Some(pos) = schema.unflat_group_pos() {
                        schema.flatten_group(pos);
                    }
                }
                let out = schema.create_group();
                register_output_names(&mut schema, n.output_var.as_deref(), &n.col_names, out);
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::ExpandAll(n) => {
                let mut schema = child_schemas.first().cloned().unwrap_or_default();
                if schema.has_unflat_group() {
                    if let Some(pos) = schema.unflat_group_pos() {
                        schema.flatten_group(pos);
                    }
                }
                let out = schema.create_group();
                register_output_names(&mut schema, n.output_var.as_deref(), &n.col_names, out);
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::AppendVertices(n) => {
                let mut schema = child_schemas.first().cloned().unwrap_or_default();
                if schema.has_unflat_group() {
                    if let Some(pos) = schema.unflat_group_pos() {
                        schema.flatten_group(pos);
                    }
                }
                let out = schema.create_group();
                if let Some(alias) = &n.node_alias {
                    schema.insert_name_for_group(alias.clone(), out);
                }
                register_output_names(&mut schema, n.output_var.as_deref(), &n.col_names, out);
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::BiExpand(n) => {
                let mut aliases = Vec::new();
                if let Some(var) = &n.meeting_point_var {
                    aliases.push(var.clone());
                }
                if let Some(var) = n.output_var.as_deref() {
                    aliases.push(var.to_string());
                }
                aliases.extend(n.col_names.iter().cloned());
                bi_expand_schema(child_schemas, &aliases)
            }
            LogicalNodeEnum::BiTraverse(n) => {
                let mut aliases = vec![n.path_var.clone()];
                if let Some(alias) = &n.edge_alias {
                    aliases.push(alias.clone());
                }
                if let Some(alias) = &n.vertex_alias {
                    aliases.push(alias.clone());
                }
                if let Some(var) = n.output_var.as_deref() {
                    aliases.push(var.to_string());
                }
                aliases.extend(n.col_names.iter().cloned());
                bi_expand_schema(child_schemas, &aliases)
            }
            LogicalNodeEnum::GetEdges(n) => {
                let mut schema = FactorizedSchema::new();
                let g = schema.create_flat_group(false);
                let eid = resolve_id(&n.edge_ref);
                schema.insert_to_group_and_scope_with_name(eid, Some(n.edge_type.clone()), g);
                if let Some(expr) = &n.expression {
                    let eid2 = resolve_id(expr);
                    if !schema.is_expression_in_scope(&eid2) {
                        let name = expr.to_expression_string();
                        schema.insert_to_group_and_scope_with_name(eid2, Some(name), g);
                    }
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::Start(_) => {
                let mut schema = FactorizedSchema::new();
                schema.create_flat_group(false);
                schema
            }
            LogicalNodeEnum::Union(_) | LogicalNodeEnum::Minus(_) => {
                if child_schemas.len() >= 2 {
                    let left = &child_schemas[0];
                    let right = &child_schemas[1];
                    let mut merged = left.clone();
                    let mapping = merged.merge_groups_from(right);
                    for (expr_id, gpos) in right.expression_to_group_iter() {
                        let new_pos = mapping.get(gpos).copied().unwrap_or(*gpos);
                        merged.insert_to_scope_may_repeat(expr_id.clone(), new_pos);
                    }
                    merged.flatten_all();
                    merged.validate_at_most_one_unflat();
                    merged
                } else {
                    child_schemas.first().cloned().unwrap_or_default()
                }
            }
            LogicalNodeEnum::Intersect(_) => {
                if child_schemas.len() > 2 {
                    let mut schema = child_schemas[0].clone();
                    if schema.has_unflat_group() {
                        schema.flatten_all();
                    }
                    let out_pos = schema.create_group();
                    for build_schema in &child_schemas[1..] {
                        for expr in build_schema.expressions_in_scope() {
                            if !schema.is_expression_in_scope(expr) {
                                schema.insert_to_group_and_scope(expr.clone(), out_pos);
                            }
                        }
                    }
                    schema.validate_at_most_one_unflat();
                    schema
                } else if child_schemas.len() >= 2 {
                    let left = &child_schemas[0];
                    let right = &child_schemas[1];
                    let mut merged = left.clone();
                    let mapping = merged.merge_groups_from(right);
                    for (expr_id, gpos) in right.expression_to_group_iter() {
                        let new_pos = mapping.get(gpos).copied().unwrap_or(*gpos);
                        merged.insert_to_scope_may_repeat(expr_id.clone(), new_pos);
                    }
                    merged.flatten_all();
                    merged.validate_at_most_one_unflat();
                    merged
                } else {
                    child_schemas.first().cloned().unwrap_or_default()
                }
            }
            LogicalNodeEnum::FulltextSearch(_)
            | LogicalNodeEnum::FulltextLookup(_)
            | LogicalNodeEnum::MatchFulltext(_) => {
                let mut schema = FactorizedSchema::new();
                schema.create_flat_group(false);
                schema.validate_at_most_one_unflat();
                schema
            }
            #[cfg(feature = "vector")]
            LogicalNodeEnum::VectorSearch(_)
            | LogicalNodeEnum::VectorLookup(_)
            | LogicalNodeEnum::VectorMatch(_) => {
                let mut schema = FactorizedSchema::new();
                schema.create_flat_group(false);
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::Unwind(n) => {
                let mut schema = child_schemas.first().cloned().unwrap_or_default();
                let list_id = resolve_id(&n.list_expression);
                let is_list_literal = n
                    .list_expression
                    .expression()
                    .map(|meta| matches!(meta.inner(), graphdb_core::Expression::List(_)))
                    .unwrap_or(false);
                if is_list_literal {
                    // A literal list fans out into a new nested group whose
                    // elements are named by the alias.
                    let group = schema.create_group();
                    if !schema.is_expression_in_scope(&list_id) {
                        schema.insert_to_group_and_scope_with_name(
                            list_id,
                            Some(n.alias.clone()),
                            group,
                        );
                    } else {
                        schema.insert_name_for_group(n.alias.clone(), group);
                    }
                } else if let Some(pos) = schema.get_group_pos(&list_id) {
                    // Unwinding an already-tracked column flattens its group
                    // first so each element lands in a flat row; the alias
                    // then refers to that group.
                    schema.flatten_group(pos);
                    schema.insert_name_for_group(n.alias.clone(), pos);
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::BeginTransaction(_)
            | LogicalNodeEnum::Commit(_)
            | LogicalNodeEnum::Rollback(_)
            | LogicalNodeEnum::PassThrough(_)
            | LogicalNodeEnum::Argument(_) => {
                let mut schema = FactorizedSchema::new();
                schema.create_flat_group(false);
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::Assign(n) => {
                // Write-path counterpart of Project: each assignment only
                // flattens the groups its right-hand side depends on, so an
                // assignment that does not touch unflat groups keeps their
                // factorization instead of a blind flatten-all.
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
                let mut expr_store: HashMap<ExpressionId, graphdb_core::Expression> =
                    HashMap::new();
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
                            g.insert_expression_with_name(
                                alias_id.clone(),
                                Some(alias.clone()),
                            );
                        }
                    }
                }
                out.validate_at_most_one_unflat();
                out
            }
            LogicalNodeEnum::Remove(_)
            | LogicalNodeEnum::DataCollect(_)
            | LogicalNodeEnum::Materialize(_) => {
                let mut schema = child_schemas.first().cloned().unwrap_or_default();
                schema.flatten_all();
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::Select(n) => {
                // Per-branch refinement: the branch condition decides
                // flattens through FlattenAllButOne on the effective (first
                // present branch) schema instead of a blind flatten-all, so
                // branches that do not touch unflat groups keep their
                // factorization. An unresolvable condition conservatively
                // flattens every unflat group via the analyzer fallback.
                let mut schema = child_schemas.first().cloned().unwrap_or_default();
                let cond_id = n.condition.id().clone();
                let mut store = HashMap::new();
                if let Some(expr) = n.condition.get_expression() {
                    store.insert(cond_id.clone(), expr);
                }
                let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_expr(
                    &cond_id,
                    &schema,
                    &store,
                );
                for pos in to_flatten {
                    schema.flatten_group(pos);
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            LogicalNodeEnum::Loop(n) => {
                // Body refinement mirrors Select: the loop condition drives
                // FlattenAllButOne on the body schema.
                let mut schema = child_schemas.first().cloned().unwrap_or_default();
                let cond_id = n.condition.id().clone();
                let mut store = HashMap::new();
                if let Some(expr) = n.condition.get_expression() {
                    store.insert(cond_id.clone(), expr);
                }
                let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_expr(
                    &cond_id,
                    &schema,
                    &store,
                );
                for pos in to_flatten {
                    schema.flatten_group(pos);
                }
                schema.validate_at_most_one_unflat();
                schema
            }
            _ => {
                let mut schema = child_schemas.first().cloned().unwrap_or_default();
                if schema.has_unflat_group() {
                    schema.flatten_all();
                }
                schema.validate_at_most_one_unflat();
                schema
            }
        }
    }

    fn compute_flat_schema(&mut self, child_schemas: &[FactorizedSchema]) -> FactorizedSchema {
        let flat_children: Vec<FactorizedSchema> =
            child_schemas.iter().map(|cs| cs.flat_copy()).collect();
        let mut result = self.compute_factorized_schema(&flat_children);
        result.flatten_all();
        result.validate_at_most_one_unflat();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::node_id_generator::next_node_id;
    use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
    use crate::planning::plan::logical::logical_nodes::control_flow::{
        LogicalLoopNode, LogicalSelectNode,
    };
    use crate::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode;
    use crate::planning::plan::logical::logical_nodes::graph_ops::LogicalAssignNode;
    use crate::planning::plan::logical::logical_nodes::join::LogicalSemiJoinNode;
    use graphdb_core::types::expr::contextual::ContextualExpression;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::ExpressionMeta;
    use std::sync::Arc;

    fn test_id(n: u64) -> ExpressionId {
        ExpressionId::new(n)
    }

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
            col_names: vec!["a.name".to_string()],
            column_types: vec![],
        })
    }

    fn scan_with_expr() -> (LogicalNodeEnum, Arc<ExpressionAnalysisContext>) {
        let raw_ctx = ExpressionAnalysisContext::new();
        let ctx = Arc::new(raw_ctx);
        let expr = graphdb_core::Expression::Variable("a".to_string());
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        let ctx_expr = ContextualExpression::new(id, ctx.clone());
        let node = LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
            id: next_node_id(),
            space_id: 1,
            space_name: "test".to_string(),
            tag: Some("p".to_string()),
            expression: Some(ctx_expr),
            limit: None,
            projected_properties: vec![],
            index_hint: None,
            estimated_cardinality: None,
            output_var: None,
            col_names: vec!["a.name".to_string()],
            column_types: vec![],
        });
        (node, ctx)
    }

    #[test]
    fn scan_schema_is_flat() {
        let mut n = scan();
        let s = n.compute_factorized_schema(&[]);
        assert!(s.is_flat_schema());
        assert_eq!(s.num_groups(), 1);
        let flat = n.compute_flat_schema(&[]);
        assert!(flat.is_flat_schema());
    }

    #[test]
    fn scan_schema_with_real_expr() {
        let (mut n, _ctx) = scan_with_expr();
        let s = n.compute_factorized_schema(&[]);
        assert!(s.is_flat_schema());
        assert_eq!(s.num_groups(), 1);
    }

    #[test]
    fn project_dependency() {
        let raw_ctx = ExpressionAnalysisContext::new();
        let ctx = Arc::new(raw_ctx);
        let expr = graphdb_core::Expression::Variable("a".to_string());
        let meta = ExpressionMeta::new(expr);
        let id_a = ctx.register_expression(meta);
        let ctx_a = ContextualExpression::new(id_a.clone(), ctx.clone());
        let mut scan_schema = FactorizedSchema::new();
        let g0 = scan_schema.create_flat_group(false);
        scan_schema.insert_to_group_and_scope(id_a.clone(), g0);
        scan_schema.insert_to_group_and_scope_with_name(
            test_id(999),
            Some("extra".to_string()),
            g0,
        );

        let col_expr = ctx_a.clone();
        let yield_col = graphdb_core::YieldColumn {
            expression: col_expr,
            alias: "a2".to_string(),
            is_matched: false,
        };
        let mut proj = LogicalNodeEnum::Project(
            crate::planning::plan::logical::logical_nodes::operation::LogicalProjectNode {
                id: next_node_id(),
                input: Some(Box::new(scan())),
                deps: vec![scan()],
                columns: vec![yield_col],
                output_var: None,
                col_names: vec!["a2".to_string()],
                column_types: vec![],
            },
        );
        let out = proj.compute_factorized_schema(&[scan_schema]);
        assert!(out.is_expression_in_scope(&id_a) || out.get_group_pos_by_name("a2").is_some());
    }

    #[test]
    fn filter_flatten() {
        let raw_ctx = ExpressionAnalysisContext::new();
        let ctx = Arc::new(raw_ctx);
        let expr = graphdb_core::Expression::Variable("b".to_string());
        let meta = ExpressionMeta::new(expr);
        let id_b = ctx.register_expression(meta);
        let ctx_b = ContextualExpression::new(id_b.clone(), ctx.clone());
        let mut child_schema = FactorizedSchema::new();
        let g0 = child_schema.create_flat_group(false);
        let g1 = child_schema.create_group();
        child_schema.insert_to_group_and_scope(test_id(100), g0);
        child_schema.insert_to_group_and_scope(id_b.clone(), g1);
        let filter = LogicalNodeEnum::Filter(
            crate::planning::plan::logical::logical_nodes::operation::LogicalFilterNode {
                id: next_node_id(),
                input: Some(Box::new(scan())),
                deps: vec![scan()],
                condition: ctx_b,
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            },
        );
        let mut filter = filter;
        let out = filter.compute_factorized_schema(&[child_schema]);
        assert!(out.is_flat_schema());
    }

    #[test]
    fn aggregate_keys() {
        let raw_ctx = ExpressionAnalysisContext::new();
        let ctx = Arc::new(raw_ctx);
        let expr = graphdb_core::Expression::Variable("a".to_string());
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        let ctx_a = ContextualExpression::new(id.clone(), ctx.clone());
        let mut child_schema = FactorizedSchema::new();
        let g0 = child_schema.create_flat_group(false);
        child_schema.insert_to_group_and_scope(id.clone(), g0);
        let mut agg = LogicalNodeEnum::Aggregate(
            crate::planning::plan::logical::logical_nodes::operation::LogicalAggregateNode {
                id: next_node_id(),
                input: Some(Box::new(scan())),
                deps: vec![scan()],
                group_key_exprs: vec![ctx_a],
                aggregation_functions: vec![],
                aggregation_distinct: vec![],
                aggregation_filters: vec![],
                grouping_sets: vec![],
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            },
        );
        let out = agg.compute_factorized_schema(&[child_schema]);
        assert!(out.is_flat_schema());
        assert!(out.num_groups() >= 1);
    }

    #[test]
    fn flatten_schema() {
        let mut scan_n = scan();
        let scan_schema = scan_n.compute_factorized_schema(&[]);
        let mut g = scan_schema.clone();
        let pos = g.create_group();
        g.insert_to_group_and_scope(test_id(555), pos);
        assert!(!g.get_group(pos).expect("g").is_flat());
        let mut flatten = LogicalNodeEnum::Flatten(LogicalFlattenNode::new(pos, scan()));
        let out = flatten.compute_factorized_schema(&[g]);
        assert!(out.is_flat_schema());
    }

    #[test]
    fn join_merges() {
        let mut left = scan();
        let ls = left.compute_factorized_schema(&[]);
        let mut right = scan();
        let mut rs = right.compute_factorized_schema(&[]);
        let pos = rs.create_group();
        rs.insert_to_group_and_scope(test_id(777), pos);
        let mut join = LogicalNodeEnum::InnerJoin(
            crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode {
                id: next_node_id(),
                left: Box::new(scan()),
                right: Box::new(scan()),
                hash_keys: vec![],
                probe_keys: vec![],
                deps: vec![scan(), scan()],
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            },
        );
        let out = join.compute_factorized_schema(&[ls, rs]);
        out.validate_at_most_one_unflat();
    }

    #[test]
    fn fulltext_leaf_is_flat_single_group() {
        use crate::parser::ast::fulltext::FulltextQueryExpr;
        use crate::planning::plan::logical::logical_nodes::search::LogicalFulltextSearchNode;
        let mut node = LogicalNodeEnum::FulltextSearch(LogicalFulltextSearchNode {
            id: next_node_id(),
            index_name: "idx".to_string(),
            query: FulltextQueryExpr::Simple("test".to_string()),
            yield_clause: None,
            where_clause: None,
            order_clause: None,
            limit: None,
            offset: None,
            space_id: 1,
            tag_name: "person".to_string(),
            field_name: "name".to_string(),
            output_var: None,
            col_names: vec!["a".to_string()],
            column_types: vec![],
        });
        let schema = node.compute_factorized_schema(&[]);
        assert_eq!(schema.num_groups(), 1);
        assert!(schema.is_flat_schema());
    }

    #[test]
    fn unwind_passthrough() {
        let mut child_schema = FactorizedSchema::new();
        let g0 = child_schema.create_flat_group(false);
        let g1 = child_schema.create_group();
        child_schema.insert_to_group_and_scope(test_id(1), g0);
        child_schema.insert_to_group_and_scope(test_id(2), g1);
        let mut unwind = LogicalNodeEnum::Unwind(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalUnwindNode {
                id: next_node_id(),
                input: Some(Box::new(scan())),
                deps: vec![scan()],
                alias: "x".to_string(),
                list_expression: {
                    let raw_ctx = ExpressionAnalysisContext::new();
                    let ctx = Arc::new(raw_ctx);
                    let meta =
                        ExpressionMeta::new(graphdb_core::Expression::Variable("list".to_string()));
                    let id = ctx.register_expression(meta);
                    ContextualExpression::new(id, ctx)
                },
                output_var: None,
                col_names: vec!["x".to_string()],
                column_types: vec![],
            },
        );
        let out = unwind.compute_factorized_schema(&[child_schema.clone()]);
        assert_eq!(out.num_groups(), child_schema.num_groups());
        assert_eq!(out.has_unflat_group(), child_schema.has_unflat_group());
        assert_eq!(out.unflat_group_pos(), child_schema.unflat_group_pos());
    }

    #[test]
    fn get_neighbors_chain_keeps_one_unflat() {
        let mut base = FactorizedSchema::new();
        let g0 = base.create_flat_group(false);
        base.insert_to_group_and_scope(test_id(1), g0);
        let mut prev = base;
        for _ in 0..3 {
            let mut node = LogicalNodeEnum::GetNeighbors(
                crate::planning::plan::logical::logical_nodes::access::LogicalGetNeighborsNode {
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
                },
            );
            let next = node.compute_factorized_schema(&[prev.clone()]);
            next.validate_at_most_one_unflat();
            prev = next;
        }
        prev.validate_at_most_one_unflat();
        assert_eq!(prev.groups().iter().filter(|g| !g.is_flat()).count(), 1);
    }

    fn unwind_node(alias: &str, list: graphdb_core::Expression) -> (LogicalNodeEnum, ExpressionId) {
        let raw_ctx = ExpressionAnalysisContext::new();
        let ctx = Arc::new(raw_ctx);
        let id = ctx.register_expression(ExpressionMeta::new(list));
        let list_expression = ContextualExpression::new(id.clone(), ctx);
        let node = LogicalNodeEnum::Unwind(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalUnwindNode {
                id: next_node_id(),
                input: Some(Box::new(scan())),
                deps: vec![scan()],
                alias: alias.to_string(),
                list_expression,
                output_var: None,
                col_names: vec![alias.to_string()],
                column_types: vec![],
            },
        );
        (node, id)
    }

    #[test]
    fn unwind_list_literal_creates_unflat_group() {
        let child_schema = FactorizedSchema::new();
        let list = graphdb_core::Expression::List(vec![graphdb_core::Expression::Literal(
            graphdb_core::value::Value::Int(1),
        )]);
        let (mut node, list_id) = unwind_node("x", list);
        let out = node.compute_factorized_schema(&[child_schema]);
        out.validate_at_most_one_unflat();
        assert!(out.has_unflat_group());
        assert_eq!(out.get_group_pos(&list_id), out.unflat_group_pos());
        assert_eq!(out.get_group_pos_by_name("x"), out.unflat_group_pos());
    }

    #[test]
    fn unwind_tracked_column_flattens_first() {
        let mut child_schema = FactorizedSchema::new();
        let g0 = child_schema.create_flat_group(false);
        let g1 = child_schema.create_group();
        child_schema.insert_to_group_and_scope(test_id(1), g0);
        let raw_ctx = ExpressionAnalysisContext::new();
        let ctx = Arc::new(raw_ctx);
        let list_id = ctx.register_expression(ExpressionMeta::new(
            graphdb_core::Expression::Variable("items".to_string()),
        ));
        child_schema.insert_to_group_and_scope(list_id.clone(), g1);
        assert!(child_schema.has_unflat_group());
        let list_expression = ContextualExpression::new(list_id.clone(), ctx);
        let mut node = LogicalNodeEnum::Unwind(
            crate::planning::plan::logical::logical_nodes::graph_ops::LogicalUnwindNode {
                id: next_node_id(),
                input: Some(Box::new(scan())),
                deps: vec![scan()],
                alias: "item".to_string(),
                list_expression,
                output_var: None,
                col_names: vec!["item".to_string()],
                column_types: vec![],
            },
        );
        let out = node.compute_factorized_schema(&[child_schema]);
        out.validate_at_most_one_unflat();
        assert!(out.is_flat_schema());
        assert!(out.is_expression_in_scope(&list_id));
        assert_eq!(
            out.get_group_pos_by_name("item"),
            out.get_group_pos(&list_id)
        );
    }

    #[test]
    fn get_neighbors_registers_output_expression() {
        let raw_ctx = ExpressionAnalysisContext::new();
        let ctx = Arc::new(raw_ctx);
        let out_id = ctx.register_expression(ExpressionMeta::new(
            graphdb_core::Expression::Variable("b".to_string()),
        ));
        let out_expr = ContextualExpression::new(out_id.clone(), ctx);
        let child_schema = FactorizedSchema::new();
        let mut node = LogicalNodeEnum::GetNeighbors(
            crate::planning::plan::logical::logical_nodes::access::LogicalGetNeighborsNode {
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
            },
        );
        let out = node.compute_factorized_schema(&[child_schema]);
        out.validate_at_most_one_unflat();
        assert!(out.has_unflat_group());
        assert_eq!(out.get_group_pos(&out_id), out.unflat_group_pos());
    }

    #[test]
    fn get_neighbors_registers_col_names_without_expression() {
        let child_schema = FactorizedSchema::new();
        let mut node = LogicalNodeEnum::GetNeighbors(
            crate::planning::plan::logical::logical_nodes::access::LogicalGetNeighborsNode {
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
                output_var: Some("b".to_string()),
                col_names: vec!["b".to_string()],
                column_types: vec![],
                deps: vec![scan()],
            },
        );
        let out = node.compute_factorized_schema(&[child_schema]);
        out.validate_at_most_one_unflat();
        assert!(out.has_unflat_group());
        let pos = out.unflat_group_pos().expect("unflat");
        assert_eq!(out.get_group_pos_by_name_opt("b"), Some(pos));
    }

    #[test]
    fn traverse_registers_vertex_and_edge_alias() {
        let child_schema = FactorizedSchema::new();
        let mut node = LogicalNodeEnum::Traverse(
            crate::planning::plan::logical::logical_nodes::traversal::LogicalTraverseNode {
                id: next_node_id(),
                space_id: 1,
                start_vids: "1".to_string(),
                end_vids: None,
                edge_types: vec!["knows".to_string()],
                direction: graphdb_core::types::graph_schema::EdgeDirection::Out,
                min_steps: 1,
                max_steps: 2,
                edge_alias: Some("e".to_string()),
                vertex_alias: Some("b".to_string()),
                e_filter: None,
                v_filter: None,
                first_step_filter: None,
                input: None,
                deps: vec![],
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            },
        );
        let out = node.compute_factorized_schema(&[child_schema]);
        out.validate_at_most_one_unflat();
        let pos = out.unflat_group_pos().expect("unflat");
        assert_eq!(out.get_group_pos_by_name_opt("b"), Some(pos));
        assert_eq!(out.get_group_pos_by_name_opt("e"), Some(pos));
    }

    #[test]
    fn expand_all_registers_col_names() {
        let child_schema = FactorizedSchema::new();
        let mut node = LogicalNodeEnum::ExpandAll(
            crate::planning::plan::logical::logical_nodes::traversal::LogicalExpandAllNode {
                id: next_node_id(),
                space_id: 1,
                edge_types: vec!["knows".to_string()],
                direction: "out".to_string(),
                any_edge_type: false,
                step_limit: Some(1),
                step_limits: None,
                join_input: false,
                sample: false,
                edge_props: vec![],
                vertex_props: vec![],
                filter: None,
                src_vids: vec![],
                include_empty_paths: false,
                input_var: Some("a".to_string()),
                deps: vec![],
                output_var: None,
                col_names: vec!["e".to_string()],
                column_types: vec![],
            },
        );
        let out = node.compute_factorized_schema(&[child_schema]);
        out.validate_at_most_one_unflat();
        let pos = out.unflat_group_pos().expect("unflat");
        assert_eq!(out.get_group_pos_by_name_opt("e"), Some(pos));
    }

    #[test]
    fn traverse_output_resolves_downstream_without_flatten_all() {
        use crate::optimizer::factorization::GroupDependencyAnalyzer;
        let child_schema = FactorizedSchema::new();
        let mut node = LogicalNodeEnum::Traverse(
            crate::planning::plan::logical::logical_nodes::traversal::LogicalTraverseNode {
                id: next_node_id(),
                space_id: 1,
                start_vids: "1".to_string(),
                end_vids: None,
                edge_types: vec!["knows".to_string()],
                direction: graphdb_core::types::graph_schema::EdgeDirection::Out,
                min_steps: 1,
                max_steps: 1,
                edge_alias: None,
                vertex_alias: Some("b".to_string()),
                e_filter: None,
                v_filter: None,
                first_step_filter: None,
                input: None,
                deps: vec![],
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            },
        );
        let schema = node.compute_factorized_schema(&[child_schema]);
        schema.validate_at_most_one_unflat();
        let pos = schema.unflat_group_pos().expect("unflat");
        let mut analyzer =
            GroupDependencyAnalyzer::with_expr_store(&schema, false, HashMap::new());
        analyzer.visit_expression(&graphdb_core::Expression::Variable("b".to_string()));
        assert!(!analyzer.has_unresolved());
        assert!(analyzer.dependent_groups().contains(&pos));
        let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_groups(
            analyzer.dependent_groups(),
            &schema,
        );
        assert!(to_flatten.is_empty());
    }

    #[test]
    fn bi_expand_merges_both_children() {        let mut left_schema = FactorizedSchema::new();
        let g0 = left_schema.create_flat_group(false);
        left_schema.insert_to_group_and_scope(test_id(1), g0);
        let mut right_schema = FactorizedSchema::new();
        let g1 = right_schema.create_flat_group(false);
        right_schema.insert_to_group_and_scope(test_id(2), g1);
        let mut node = LogicalNodeEnum::BiExpand(
            crate::planning::plan::logical::logical_nodes::traversal::LogicalBiExpandNode {
                id: next_node_id(),
                space_id: 1,
                left_direction: graphdb_core::types::graph_schema::EdgeDirection::Out,
                right_direction: graphdb_core::types::graph_schema::EdgeDirection::In,
                edge_types: vec!["knows".to_string()],
                max_hops: 2,
                meeting_point_var: None,
                left: Box::new(scan()),
                right: Box::new(scan()),
                deps: vec![scan(), scan()],
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            },
        );
        let out = node.compute_factorized_schema(&[left_schema, right_schema]);
        out.validate_at_most_one_unflat();
        assert!(out.is_expression_in_scope(&test_id(1)));
        assert!(out.is_expression_in_scope(&test_id(2)));
        assert!(out.has_unflat_group());
    }

    fn ctx_var(var: &str) -> (ContextualExpression, ExpressionId) {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(
            graphdb_core::Expression::Variable(var.to_string()),
        ));
        (ContextualExpression::new(id.clone(), ctx), id)
    }

    fn assign_node(rhs: ContextualExpression) -> LogicalNodeEnum {
        LogicalNodeEnum::Assign(LogicalAssignNode {
            id: next_node_id(),
            input: Some(Box::new(scan())),
            deps: vec![scan()],
            assignments: vec![("c".to_string(), rhs)],
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })
    }

    #[test]
    fn assign_on_flat_group_keeps_unflat() {
        let (rhs, rhs_id) = ctx_var("a");
        let mut child = FactorizedSchema::new();
        let g0 = child.create_flat_group(false);
        let g1 = child.create_group();
        child.insert_to_group_and_scope(test_id(1), g0);
        child.insert_to_group_and_scope(test_id(2), g1);
        child.insert_to_group_and_scope_with_name(test_id(3), Some("a".to_string()), g0);
        let mut node = assign_node(rhs);
        let out = node.compute_factorized_schema(&[child]);
        out.validate_at_most_one_unflat();
        assert_eq!(
            out.unflat_group_pos(),
            Some(g1),
            "assignment reading only a flat group must not flatten the unflat group"
        );
        assert!(
            out.is_expression_in_scope(&rhs_id),
            "assigned expression must be registered in scope"
        );
    }

    #[test]
    fn assign_on_unflat_group_targets_dependent_group() {
        let (rhs, rhs_id) = ctx_var("a");
        let mut child = FactorizedSchema::new();
        let g0 = child.create_flat_group(false);
        let g1 = child.create_group();
        child.insert_to_group_and_scope(test_id(1), g0);
        child.insert_to_group_and_scope(test_id(2), g1);
        child.insert_to_group_and_scope_with_name(test_id(3), Some("a".to_string()), g1);
        let mut node = assign_node(rhs);
        let out = node.compute_factorized_schema(&[child]);
        out.validate_at_most_one_unflat();
        assert_eq!(
            out.unflat_group_pos(),
            Some(g1),
            "assignment on an unflat group must target that group, not flatten-all"
        );
        assert!(
            out.is_expression_in_scope(&rhs_id),
            "assigned expression must be registered in scope"
        );
    }

    #[test]
    fn semi_join_compute_keeps_first_unflat_with_keys() {
        // Boundary documentation: with hand-built schemas the generic
        // join arm keeps the first unflat group even when it holds a hash
        // key, while a key-aware rule would flatten the probe key groups
        // first. This shape is unreachable through the rewriter, which
        // flattens probe key groups before compute (see
        // semi_join_rewriter_flattens_unflat_probe_keys).
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let key_id = ctx.register_expression(ExpressionMeta::new(
            graphdb_core::Expression::Variable("a".to_string()),
        ));
        let key = ContextualExpression::new(key_id.clone(), ctx);
        let mut left_schema = FactorizedSchema::new();
        let lg0 = left_schema.create_flat_group(false);
        let lg1 = left_schema.create_group();
        left_schema.insert_to_group_and_scope(test_id(1), lg0);
        left_schema.insert_to_group_and_scope(key_id.clone(), lg1);
        let mut right_schema = FactorizedSchema::new();
        let rg0 = right_schema.create_flat_group(false);
        let rg1 = right_schema.create_group();
        right_schema.insert_to_group_and_scope(test_id(2), rg0);
        right_schema.insert_to_group_and_scope(test_id(3), rg1);
        let mut node = LogicalNodeEnum::SemiJoin(LogicalSemiJoinNode {
            id: next_node_id(),
            left: Box::new(scan()),
            right: Box::new(scan()),
            hash_keys: vec![key],
            probe_keys: vec![],
            deps: vec![scan(), scan()],
            join_condition: None,
            anti: false,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        let out = node.compute_factorized_schema(&[left_schema, right_schema]);
        out.validate_at_most_one_unflat();
        // Generic rule keeps the first (left) unflat group, keys included.
        assert_eq!(out.unflat_group_pos(), Some(lg1));
        assert_eq!(out.get_group_pos(&key_id), Some(lg1));
    }

    fn select_node(cond: ContextualExpression) -> LogicalNodeEnum {
        LogicalNodeEnum::Select(LogicalSelectNode {
            id: next_node_id(),
            condition: cond,
            if_branch: Some(Box::new(scan())),
            else_branch: Some(Box::new(scan())),
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })
    }

    #[test]
    fn select_condition_on_flat_keeps_factorization() {
        let (cond, _) = ctx_var("a");
        let mut branch = FactorizedSchema::new();
        let g0 = branch.create_flat_group(false);
        let g1 = branch.create_group();
        branch.insert_to_group_and_scope(test_id(1), g0);
        branch.insert_to_group_and_scope(test_id(2), g1);
        branch.insert_to_group_and_scope_with_name(test_id(3), Some("a".to_string()), g0);
        let mut node = select_node(cond);
        let out = node.compute_factorized_schema(&[branch.clone(), branch]);
        out.validate_at_most_one_unflat();
        assert_eq!(
            out.unflat_group_pos(),
            Some(g1),
            "condition on a flat group must not flatten the unflat branch group"
        );
    }

    #[test]
    fn select_unresolvable_condition_falls_back_to_flat() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(
            graphdb_core::Expression::Variable("ghost".to_string()),
        ));
        let cond = ContextualExpression::new(id, ctx);
        let mut branch = FactorizedSchema::new();
        let g0 = branch.create_flat_group(false);
        let g1 = branch.create_group();
        branch.insert_to_group_and_scope(test_id(1), g0);
        branch.insert_to_group_and_scope(test_id(2), g1);
        let mut node = select_node(cond);
        let out = node.compute_factorized_schema(&[branch]);
        out.validate_at_most_one_unflat();
        assert!(
            out.is_flat_schema(),
            "unresolvable condition must conservatively flatten"
        );
    }

    #[test]
    fn loop_condition_on_flat_keeps_factorization() {
        let (cond, _) = ctx_var("a");
        let mut body = FactorizedSchema::new();
        let g0 = body.create_flat_group(false);
        let g1 = body.create_group();
        body.insert_to_group_and_scope(test_id(1), g0);
        body.insert_to_group_and_scope(test_id(2), g1);
        body.insert_to_group_and_scope_with_name(test_id(3), Some("a".to_string()), g0);
        let mut node =
            LogicalNodeEnum::Loop(LogicalLoopNode::new_with_body(cond, scan()));
        let out = node.compute_factorized_schema(&[body]);
        out.validate_at_most_one_unflat();
        assert_eq!(out.unflat_group_pos(), Some(g1));
    }
}
