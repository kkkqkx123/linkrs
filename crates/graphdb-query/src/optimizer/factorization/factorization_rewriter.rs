use std::collections::{HashMap, HashSet};

use graphdb_core::types::expr::ExpressionId;

use crate::planning::plan::factorization::{FGroupPos, FactorizedSchema, FactorizedSchemaCompute};
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use crate::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode;

use super::flatten_resolver::{FlattenAll, FlattenAllButOne};

/// FactorizationRewriter: re-inserts LogicalFlatten operators after optimization.
///
/// Runs at the very end of the optimizer pipeline. Traverses the plan
/// bottom-up; for each operator it decides which groups must be flattened
/// and wraps the child's output with `LogicalFlatten` nodes.
///
/// Mirrors `lbug::optimizer::FactorizationRewriter` in
/// `ref/ladybug/src/optimizer/factorization_rewriter.cpp`.
pub struct FactorizationRewriter {
    /// When false, rewriting is disabled (always-flatten fallback).
    pub enabled: bool,
    /// Group positions where a requested flatten was skipped because the
    /// group was already flat. Intentional no-ops stay visible: the engine
    /// copies them into `cbo_notes` as
    /// `factorization: flatten_noop_flat(group=N)` instead of dropping
    /// them silently.
    skipped_flat_groups: Vec<FGroupPos>,
}

impl FactorizationRewriter {
    pub fn new() -> Self {
        Self {
            enabled: true,
            skipped_flat_groups: Vec::new(),
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            skipped_flat_groups: Vec::new(),
        }
    }

    /// Rewrite the plan in place.
    pub fn rewrite(&mut self, plan: &mut LogicalNodeEnum) {
        if !self.enabled {
            return;
        }
        let _ = self.visit_operator(plan);
    }

    /// Drain the recorded already-flat no-op positions.
    pub fn take_skipped_flat_groups(&mut self) -> Vec<FGroupPos> {
        std::mem::take(&mut self.skipped_flat_groups)
    }

    fn visit_operator(&mut self, node: &mut LogicalNodeEnum) -> FactorizedSchema {
        match node {
            LogicalNodeEnum::Project(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let exprs = Self::expr_ids_for_project(n);
                let store = Self::build_store_for_project(n);
                // Non-deterministic functions (rand, uuid, now, ...) must be
                // evaluated tuple-at-a-time: keeping any group unflat would
                // share one generated value across rows. Fall back to
                // flatten-all instead of the usual flatten-all-but-one.
                let has_nondeterministic = store
                    .values()
                    .any(crate::optimizer::analysis::expression::NondeterministicChecker::contains_nondeterministic);
                let to_flatten = if has_nondeterministic {
                    let groups = child_schema.groups_pos_in_scope();
                    FlattenAll::get_groups_pos_to_flatten_for_groups(&groups, &child_schema)
                } else {
                    FlattenAllButOne::get_groups_pos_to_flatten_for_exprs(
                        &exprs,
                        &child_schema,
                        &store,
                    )
                };
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            self.append_flattens((**child).clone(), &to_flatten, &child_schema);
                        **child = new_child;
                    }
                    let mut flattened_child = child_schema.clone();
                    for pos in &to_flatten {
                        flattened_child.flatten_group(*pos);
                    }
                    let mut tmp = node.clone();
                    return tmp.compute_factorized_schema(&[flattened_child]);
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Filter(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let expr_id = n.condition.id().clone();
                let mut store = HashMap::new();
                if let Some(expr) = n.condition.get_expression() {
                    store.insert(expr_id.clone(), expr);
                }
                let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_expr(
                    &expr_id,
                    &child_schema,
                    &store,
                );
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            self.append_flattens((**child).clone(), &to_flatten, &child_schema);
                        **child = new_child;
                    }
                    let mut flattened_child = child_schema.clone();
                    for pos in &to_flatten {
                        flattened_child.flatten_group(*pos);
                    }
                    let mut tmp = node.clone();
                    return tmp.compute_factorized_schema(&[flattened_child]);
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Aggregate(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                // Same two-stage rule as the compute path: keys decide the
                // surviving group, distinct payloads off the leading group
                // are flattened additionally.
                let key_ids: Vec<ExpressionId> =
                    n.group_key_exprs.iter().map(|e| e.id().clone()).collect();
                let mut store = HashMap::new();
                for expr in &n.group_key_exprs {
                    if let Some(inner) = expr.get_expression() {
                        store.insert(expr.id().clone(), inner);
                    }
                }
                let (_leading, to_flatten) = super::flatten_resolver::aggregate_groups_to_flatten(
                    &key_ids,
                    &store,
                    &n.aggregation_args,
                    &n.aggregation_distinct,
                    &child_schema,
                );
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            self.append_flattens((**child).clone(), &to_flatten, &child_schema);
                        **child = new_child;
                    }
                    let mut flattened_child = child_schema.clone();
                    for pos in &to_flatten {
                        flattened_child.flatten_group(*pos);
                    }
                    let mut tmp = node.clone();
                    return tmp.compute_factorized_schema(&[flattened_child]);
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Sort(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                // Only sort-key groups are flattened, and a single group
                // needs no flattening at all: with one group the factorized
                // table can be sorted and scanned back without changing the
                // schema shape.
                let to_flatten = if child_schema.num_groups() > 1 {
                    let mut dependent = HashSet::new();
                    let mut unresolved = false;
                    for item in &n.sort_items {
                        let mut analyzer =
                            super::group_dependency_analyzer::GroupDependencyAnalyzer::new(
                                &child_schema,
                                false,
                            );
                        analyzer.visit_expression(&item.expression);
                        dependent.extend(analyzer.dependent_groups().iter().copied());
                        unresolved |= analyzer.has_unresolved();
                    }
                    if unresolved {
                        FlattenAll::get_groups_pos_to_flatten_for_groups(
                            &child_schema.groups_pos_in_scope(),
                            &child_schema,
                        )
                    } else {
                        FlattenAll::get_groups_pos_to_flatten_for_groups(&dependent, &child_schema)
                    }
                } else {
                    HashSet::new()
                };
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            self.append_flattens((**child).clone(), &to_flatten, &child_schema);
                        **child = new_child;
                    }
                    let mut flattened_child = child_schema.clone();
                    for pos in &to_flatten {
                        flattened_child.flatten_group(*pos);
                    }
                    let mut tmp = node.clone();
                    return tmp.compute_factorized_schema(&[flattened_child]);
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Window(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let groups = child_schema.groups_pos_in_scope();
                let to_flatten =
                    FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, &child_schema);
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            self.append_flattens((**child).clone(), &to_flatten, &child_schema);
                        **child = new_child;
                    }
                    let mut flattened_child = child_schema.clone();
                    for pos in &to_flatten {
                        flattened_child.flatten_group(*pos);
                    }
                    let mut tmp = node.clone();
                    return tmp.compute_factorized_schema(&[flattened_child]);
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Limit(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                // Limit counts flattened rows, so at most one group may stay
                // unflat across the limit boundary.
                let groups = child_schema.groups_pos_in_scope();
                let to_flatten =
                    FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, &child_schema);
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            self.append_flattens((**child).clone(), &to_flatten, &child_schema);
                        **child = new_child;
                    }
                    let mut flattened_child = child_schema.clone();
                    for pos in &to_flatten {
                        flattened_child.flatten_group(*pos);
                    }
                    let mut tmp = node.clone();
                    return tmp.compute_factorized_schema(&[flattened_child]);
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Skip(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::TopN(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let groups = child_schema.groups_pos_in_scope();
                let to_flatten =
                    FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, &child_schema);
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            self.append_flattens((**child).clone(), &to_flatten, &child_schema);
                        **child = new_child;
                    }
                    let mut flattened_child = child_schema.clone();
                    for pos in &to_flatten {
                        flattened_child.flatten_group(*pos);
                    }
                    let mut tmp = node.clone();
                    return tmp.compute_factorized_schema(&[flattened_child]);
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Dedup(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let groups = child_schema.groups_pos_in_scope();
                let to_flatten =
                    FlattenAll::get_groups_pos_to_flatten_for_groups(&groups, &child_schema);
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            self.append_flattens((**child).clone(), &to_flatten, &child_schema);
                        **child = new_child;
                    }
                    let mut flattened_child = child_schema.clone();
                    for pos in &to_flatten {
                        flattened_child.flatten_group(*pos);
                    }
                    let mut tmp = node.clone();
                    return tmp.compute_factorized_schema(&[flattened_child]);
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::InnerJoin(n) => {
                let mut left_schema = self.visit_operator(&mut n.left);
                let mut right_schema = self.visit_operator(&mut n.right);
                self.visit_hash_join_inner(n, &mut left_schema, &mut right_schema);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::LeftJoin(n) => {
                let mut left_schema = self.visit_operator(&mut n.left);
                let mut right_schema = self.visit_operator(&mut n.right);
                self.visit_hash_join_left(n, &mut left_schema, &mut right_schema);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::RightJoin(n) => {
                let mut left_schema = self.visit_operator(&mut n.left);
                let mut right_schema = self.visit_operator(&mut n.right);
                self.visit_hash_join_right(
                    &mut left_schema,
                    &mut right_schema,
                    &n.hash_keys,
                    &n.probe_keys,
                    &mut n.left,
                    &mut n.right,
                );
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::CrossJoin(n) => {
                let mut left_schema = self.visit_operator(&mut n.left);
                let mut right_schema = self.visit_operator(&mut n.right);
                if n.hash_keys.is_empty() && n.probe_keys.is_empty() {
                    // No join keys: cross product needs no flatten; preserve factorization.
                } else {
                    self.visit_hash_join_generic_inner(
                        &mut left_schema,
                        &mut right_schema,
                        &n.hash_keys,
                        &n.probe_keys,
                        &mut n.left,
                        &mut n.right,
                    );
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::FullOuterJoin(n) => {
                let mut left_schema = self.visit_operator(&mut n.left);
                let mut right_schema = self.visit_operator(&mut n.right);
                self.visit_hash_join_full_outer(
                    &mut left_schema,
                    &mut right_schema,
                    &n.hash_keys,
                    &n.probe_keys,
                    &mut n.left,
                    &mut n.right,
                );
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::SemiJoin(n) => {
                let mut left_schema = self.visit_operator(&mut n.left);
                let mut right_schema = self.visit_operator(&mut n.right);
                self.visit_hash_join_generic_inner(
                    &mut left_schema,
                    &mut right_schema,
                    &n.hash_keys,
                    &n.probe_keys,
                    &mut n.left,
                    &mut n.right,
                );
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::Traverse(n) => {
                let effective_schema = if let Some(child) = n.input.as_mut() {
                    let schema = self.visit_operator(child);
                    if let Some(pos) = schema.unflat_group_pos() {
                        let mut to_flatten = HashSet::new();
                        to_flatten.insert(pos);
                        let new_child =
                            self.append_flattens((**child).clone(), &to_flatten, &schema);
                        **child = new_child;
                        let mut flattened = schema.clone();
                        flattened.flatten_group(pos);
                        flattened
                    } else {
                        schema
                    }
                } else {
                    FactorizedSchema::new()
                };
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[effective_schema])
            }
            LogicalNodeEnum::Expand(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    let schema = self.visit_operator(dep);
                    if let Some(pos) = schema.unflat_group_pos() {
                        let mut to_flatten = HashSet::new();
                        to_flatten.insert(pos);
                        let new_dep = self.append_flattens(dep.clone(), &to_flatten, &schema);
                        *dep = new_dep;
                        let mut flattened = schema.clone();
                        flattened.flatten_group(pos);
                        child_schemas.push(flattened);
                    } else {
                        child_schemas.push(schema);
                    }
                }
                let child_schema = child_schemas.first().cloned().unwrap_or_default();
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::ExpandAll(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    let schema = self.visit_operator(dep);
                    if let Some(pos) = schema.unflat_group_pos() {
                        let mut to_flatten = HashSet::new();
                        to_flatten.insert(pos);
                        let new_dep = self.append_flattens(dep.clone(), &to_flatten, &schema);
                        *dep = new_dep;
                        let mut flattened = schema.clone();
                        flattened.flatten_group(pos);
                        child_schemas.push(flattened);
                    } else {
                        child_schemas.push(schema);
                    }
                }
                let child_schema = child_schemas.first().cloned().unwrap_or_default();
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::BiExpand(n) => {
                let mut left_schema = self.visit_operator(&mut n.left);
                let mut right_schema = self.visit_operator(&mut n.right);
                if let Some(pos) = left_schema.unflat_group_pos() {
                    let mut to_flatten = HashSet::new();
                    to_flatten.insert(pos);
                    let new_left =
                        self.append_flattens((*n.left).clone(), &to_flatten, &left_schema);
                    *n.left = new_left;
                    left_schema.flatten_group(pos);
                }
                if let Some(pos) = right_schema.unflat_group_pos() {
                    let mut to_flatten = HashSet::new();
                    to_flatten.insert(pos);
                    let new_right =
                        self.append_flattens((*n.right).clone(), &to_flatten, &right_schema);
                    *n.right = new_right;
                    right_schema.flatten_group(pos);
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::BiTraverse(n) => {
                let mut left_schema = self.visit_operator(&mut n.left);
                let mut right_schema = self.visit_operator(&mut n.right);
                if let Some(pos) = left_schema.unflat_group_pos() {
                    let mut to_flatten = HashSet::new();
                    to_flatten.insert(pos);
                    let new_left =
                        self.append_flattens((*n.left).clone(), &to_flatten, &left_schema);
                    *n.left = new_left;
                    left_schema.flatten_group(pos);
                }
                if let Some(pos) = right_schema.unflat_group_pos() {
                    let mut to_flatten = HashSet::new();
                    to_flatten.insert(pos);
                    let new_right =
                        self.append_flattens((*n.right).clone(), &to_flatten, &right_schema);
                    *n.right = new_right;
                    right_schema.flatten_group(pos);
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::AppendVertices(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    let schema = self.visit_operator(dep);
                    if let Some(pos) = schema.unflat_group_pos() {
                        let mut to_flatten = HashSet::new();
                        to_flatten.insert(pos);
                        let new_dep = self.append_flattens(dep.clone(), &to_flatten, &schema);
                        *dep = new_dep;
                        let mut flattened = schema.clone();
                        flattened.flatten_group(pos);
                        child_schemas.push(flattened);
                    } else {
                        child_schemas.push(schema);
                    }
                }
                let child_schema = child_schemas.first().cloned().unwrap_or_default();
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Unwind(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                // Baseline `LogicalUnwind::getGroupsPosToFlatten` is
                // `FlattenAll(inExpr)`: flatten every dependent unflat group
                // before fan-out. The compute path then builds a fresh group
                // for the alias, so both sides stay in sync.
                let list_id = n.list_expression.id().clone();
                let mut store = HashMap::new();
                if let Some(expr) = n.list_expression.get_expression() {
                    store.insert(list_id.clone(), expr);
                }
                let to_flatten =
                    FlattenAll::get_groups_pos_to_flatten_for_expr(&list_id, &child_schema, &store);
                // Untracked list expressions (bare variable/parameter) miss
                // the id lookup and yield an empty dependent set; fall back
                // to flatten-all so the rewriter stays as conservative as
                // the compute path instead of inserting nothing.
                let to_flatten = if to_flatten.is_empty() && !child_schema.is_flat_schema() {
                    let mut analyzer =
                        crate::optimizer::factorization::GroupDependencyAnalyzer::with_expr_store(
                            &child_schema,
                            false,
                            store,
                        );
                    analyzer.visit(&list_id);
                    if analyzer.has_unresolved() {
                        FlattenAll::get_groups_pos_to_flatten_for_groups(
                            &child_schema.groups_pos_in_scope(),
                            &child_schema,
                        )
                    } else {
                        to_flatten
                    }
                } else {
                    to_flatten
                };
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            self.append_flattens((**child).clone(), &to_flatten, &child_schema);
                        **child = new_child;
                    }
                    let mut flattened = child_schema.clone();
                    for pos in &to_flatten {
                        flattened.flatten_group(*pos);
                    }
                    let mut tmp = node.clone();
                    return tmp.compute_factorized_schema(&[flattened]);
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Union(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    let schema = self.visit_operator(dep);
                    self.flatten_barrier_child(dep, &schema);
                    child_schemas.push(Self::flattened_schema(&schema));
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&child_schemas)
            }
            LogicalNodeEnum::Minus(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    let schema = self.visit_operator(dep);
                    self.flatten_barrier_child(dep, &schema);
                    child_schemas.push(Self::flattened_schema(&schema));
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&child_schemas)
            }
            LogicalNodeEnum::Intersect(n) => {
                // Binary set-operation intersect carries no join keys (unlike
                // Ladybug's keyed LogicalIntersect, whose counterpart here is
                // WcoIntersect with precise per-side handling below), so both
                // inputs must be fully flat before the set comparison.
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    let schema = self.visit_operator(dep);
                    self.flatten_barrier_child(dep, &schema);
                    child_schemas.push(Self::flattened_schema(&schema));
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&child_schemas)
            }
            LogicalNodeEnum::WcoIntersect(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    child_schemas.push(self.visit_operator(dep));
                }
                if let Some(probe_schema) = child_schemas.first() {
                    let to_flatten = n.get_groups_to_flatten_on_probe_side(probe_schema);
                    if !to_flatten.is_empty() {
                        let new_probe =
                            self.append_flattens(n.deps[0].clone(), &to_flatten, probe_schema);
                        n.deps[0] = new_probe;
                        let mut flattened = probe_schema.clone();
                        for pos in &to_flatten {
                            flattened.flatten_group(*pos);
                        }
                        child_schemas[0] = flattened;
                    }
                }
                for build_idx in 0..n.num_builds() {
                    let child_idx = build_idx + 1;
                    if child_idx >= child_schemas.len() {
                        break;
                    }
                    let to_flatten =
                        n.get_groups_to_flatten_on_build_side(build_idx, &child_schemas[child_idx]);
                    if !to_flatten.is_empty() {
                        let schema = child_schemas[child_idx].clone();
                        let new_build =
                            self.append_flattens(n.deps[child_idx].clone(), &to_flatten, &schema);
                        n.deps[child_idx] = new_build;
                        let mut flattened = schema;
                        for pos in &to_flatten {
                            flattened.flatten_group(*pos);
                        }
                        child_schemas[child_idx] = flattened;
                    }
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&child_schemas)
            }
            LogicalNodeEnum::Flatten(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::GetVertices(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    child_schemas.push(self.visit_operator(dep));
                }
                let child_schema = child_schemas.first().cloned().unwrap_or_default();
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::GetNeighbors(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    let schema = self.visit_operator(dep);
                    if let Some(pos) = schema.unflat_group_pos() {
                        let mut to_flatten = HashSet::new();
                        to_flatten.insert(pos);
                        let new_dep = self.append_flattens(dep.clone(), &to_flatten, &schema);
                        *dep = new_dep;
                        let mut flattened = schema.clone();
                        flattened.flatten_group(pos);
                        child_schemas.push(flattened);
                    } else {
                        child_schemas.push(schema);
                    }
                }
                let child_schema = child_schemas.first().cloned().unwrap_or_default();
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::GetEdges(_)
            | LogicalNodeEnum::ScanVertices(_)
            | LogicalNodeEnum::ScanEdges(_)
            | LogicalNodeEnum::Start(_)
            | LogicalNodeEnum::Argument(_)
            | LogicalNodeEnum::PassThrough(_)
            | LogicalNodeEnum::BeginTransaction(_)
            | LogicalNodeEnum::Commit(_)
            | LogicalNodeEnum::Rollback(_)
            | LogicalNodeEnum::InsertVertices(_)
            | LogicalNodeEnum::InsertEdges(_)
            | LogicalNodeEnum::Update(_)
            | LogicalNodeEnum::DeleteVertices(_)
            | LogicalNodeEnum::DeleteEdges(_)
            | LogicalNodeEnum::DeleteTags(_)
            | LogicalNodeEnum::DeleteIndex(_)
            | LogicalNodeEnum::PipeDeleteVertices(_)
            | LogicalNodeEnum::PipeDeleteEdges(_)
            | LogicalNodeEnum::CopyFrom(_)
            | LogicalNodeEnum::CopyTo(_)
            | LogicalNodeEnum::FulltextSearch(_)
            | LogicalNodeEnum::FulltextLookup(_)
            | LogicalNodeEnum::MatchFulltext(_) => {
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[])
            }
            #[cfg(feature = "vector")]
            LogicalNodeEnum::VectorSearch(_)
            | LogicalNodeEnum::VectorLookup(_)
            | LogicalNodeEnum::VectorMatch(_) => {
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[])
            }
            LogicalNodeEnum::Assign(n) => {
                let mut child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                for dep in &mut n.deps {
                    self.visit_operator(dep);
                }
                // Per-assignment granularity: each right-hand side flattens
                // only the groups it depends on, against the running schema
                // that already reflects earlier flattens. A bulk pass over
                // all right-hand sides would over-flatten when assignments
                // touch disjoint groups.
                for (_, rhs) in &n.assignments {
                    let rhs_id = rhs.id().clone();
                    let mut store = HashMap::new();
                    if let Some(inner) = rhs.get_expression() {
                        store.insert(rhs_id.clone(), inner);
                    }
                    let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_expr(
                        &rhs_id,
                        &child_schema,
                        &store,
                    );
                    if !to_flatten.is_empty() {
                        if let Some(child) = n.input.as_mut() {
                            let new_child =
                                self.append_flattens((**child).clone(), &to_flatten, &child_schema);
                            **child = new_child;
                        }
                        for pos in &to_flatten {
                            child_schema.flatten_group(*pos);
                        }
                    }
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Remove(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                self.flatten_barrier_single(n.input.as_mut(), &child_schema);
                let child_schema = Self::flattened_schema(&child_schema);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::DataCollect(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                self.flatten_barrier_single(n.input.as_mut(), &child_schema);
                let child_schema = Self::flattened_schema(&child_schema);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Materialize(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                self.flatten_barrier_single(n.input.as_mut(), &child_schema);
                let child_schema = Self::flattened_schema(&child_schema);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::RollUpApply(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                self.flatten_barrier_single(n.input.as_mut(), &child_schema);
                let child_schema = Self::flattened_schema(&child_schema);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Sample(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Select(n) => {
                // Per-branch refinement: the branch condition drives
                // FlattenAllButOne on each branch schema separately, so a
                // branch that does not touch unflat groups keeps its
                // factorization instead of being flattened wholesale.
                let cond_id = n.condition.id().clone();
                let mut store = HashMap::new();
                if let Some(expr) = n.condition.get_expression() {
                    store.insert(cond_id.clone(), expr);
                }
                let mut branch_schemas = Vec::new();
                if let Some(branch) = n.if_branch.as_mut() {
                    let schema = self.visit_operator(branch);
                    let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_expr(
                        &cond_id, &schema, &store,
                    );
                    if !to_flatten.is_empty() {
                        let new_branch =
                            self.append_flattens((**branch).clone(), &to_flatten, &schema);
                        **branch = new_branch;
                        let mut flattened = schema.clone();
                        for pos in &to_flatten {
                            flattened.flatten_group(*pos);
                        }
                        branch_schemas.push(flattened);
                    } else {
                        branch_schemas.push(schema);
                    }
                }
                if let Some(branch) = n.else_branch.as_mut() {
                    let schema = self.visit_operator(branch);
                    let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_expr(
                        &cond_id, &schema, &store,
                    );
                    if !to_flatten.is_empty() {
                        let new_branch =
                            self.append_flattens((**branch).clone(), &to_flatten, &schema);
                        **branch = new_branch;
                        let mut flattened = schema.clone();
                        for pos in &to_flatten {
                            flattened.flatten_group(*pos);
                        }
                        branch_schemas.push(flattened);
                    } else {
                        branch_schemas.push(schema);
                    }
                }
                let effective = branch_schemas.first().cloned().unwrap_or_default();
                if branch_schemas.is_empty() {
                    let mut tmp = node.clone();
                    tmp.compute_factorized_schema(&[])
                } else {
                    let mut tmp = node.clone();
                    tmp.compute_factorized_schema(&[effective])
                }
            }
            LogicalNodeEnum::Loop(n) => {
                // Body refinement mirrors Select: the loop condition drives
                // FlattenAllButOne on the body schema.
                let cond_id = n.condition.id().clone();
                let mut store = HashMap::new();
                if let Some(expr) = n.condition.get_expression() {
                    store.insert(cond_id.clone(), expr);
                }
                let child_schema = if let Some(body) = n.body.as_mut() {
                    let schema = self.visit_operator(body);
                    let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_expr(
                        &cond_id, &schema, &store,
                    );
                    if !to_flatten.is_empty() {
                        let new_body = self.append_flattens((**body).clone(), &to_flatten, &schema);
                        **body = new_body;
                        let mut flattened = schema.clone();
                        for pos in &to_flatten {
                            flattened.flatten_group(*pos);
                        }
                        flattened
                    } else {
                        schema
                    }
                } else {
                    FactorizedSchema::new()
                };
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::PatternApply(n) => {
                let left_schema = self.visit_operator(&mut n.left);
                let right_schema = self.visit_operator(&mut n.right);
                self.flatten_barrier_binary(&mut n.left, &mut n.right, &left_schema, &right_schema);
                let left_schema = Self::flattened_schema(&left_schema);
                let right_schema = Self::flattened_schema(&right_schema);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::CorrelatedApply(n) => {
                let left_schema = self.visit_operator(&mut n.left);
                let right_schema = self.visit_operator(&mut n.right);
                self.flatten_barrier_binary(&mut n.left, &mut n.right, &left_schema, &right_schema);
                let left_schema = Self::flattened_schema(&left_schema);
                let right_schema = Self::flattened_schema(&right_schema);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::Apply(n) => {
                let left_schema = self.visit_operator(n.left_input_mut());
                let right_schema = self.visit_operator(n.right_input_mut());
                {
                    let left = n.left_input_mut();
                    self.flatten_barrier_child(left, &left_schema);
                }
                {
                    let right = n.right_input_mut();
                    self.flatten_barrier_child(right, &right_schema);
                }
                let left_schema = Self::flattened_schema(&left_schema);
                let right_schema = Self::flattened_schema(&right_schema);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::MultiShortestPath(n) => {
                let left_schema = self.visit_operator(&mut n.left);
                let right_schema = self.visit_operator(&mut n.right);
                self.flatten_barrier_binary(&mut n.left, &mut n.right, &left_schema, &right_schema);
                let left_schema = Self::flattened_schema(&left_schema);
                let right_schema = Self::flattened_schema(&right_schema);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::BFSShortest(n) => {
                let left_schema = self.visit_operator(&mut n.left);
                let right_schema = self.visit_operator(&mut n.right);
                self.flatten_barrier_binary(&mut n.left, &mut n.right, &left_schema, &right_schema);
                let left_schema = Self::flattened_schema(&left_schema);
                let right_schema = Self::flattened_schema(&right_schema);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::AllPaths(n) => {
                let left_schema = self.visit_operator(&mut n.left);
                let right_schema = self.visit_operator(&mut n.right);
                self.flatten_barrier_binary(&mut n.left, &mut n.right, &left_schema, &right_schema);
                let left_schema = Self::flattened_schema(&left_schema);
                let right_schema = Self::flattened_schema(&right_schema);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::ShortestPath(n) => {
                let left_schema = self.visit_operator(&mut n.left);
                let right_schema = self.visit_operator(&mut n.right);
                self.flatten_barrier_binary(&mut n.left, &mut n.right, &left_schema, &right_schema);
                let left_schema = Self::flattened_schema(&left_schema);
                let right_schema = Self::flattened_schema(&right_schema);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
        }
    }

    fn build_store_for_project(
        node: &crate::planning::plan::logical::logical_nodes::operation::LogicalProjectNode,
    ) -> HashMap<ExpressionId, graphdb_core::Expression> {
        let mut store = HashMap::new();
        for col in &node.columns {
            if let Some(expr) = col.expression.get_expression() {
                store.insert(col.expression.id().clone(), expr);
            }
        }
        store
    }

    fn expr_ids_for_project(
        node: &crate::planning::plan::logical::logical_nodes::operation::LogicalProjectNode,
    ) -> Vec<graphdb_core::types::expr::ExpressionId> {
        node.columns
            .iter()
            .map(|c| c.expression.id().clone())
            .collect()
    }

    fn visit_hash_join_inner(
        &mut self,
        node: &mut crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode,
        left_schema: &mut FactorizedSchema,
        right_schema: &mut FactorizedSchema,
    ) {
        self.visit_hash_join_generic_inner(
            left_schema,
            right_schema,
            &node.hash_keys,
            &node.probe_keys,
            &mut node.left,
            &mut node.right,
        );
    }

    fn visit_hash_join_left(
        &mut self,
        node: &mut crate::planning::plan::logical::logical_nodes::join::LogicalLeftJoinNode,
        left_schema: &mut FactorizedSchema,
        right_schema: &mut FactorizedSchema,
    ) {
        self.visit_hash_join_generic_inner(
            left_schema,
            right_schema,
            &node.hash_keys,
            &node.probe_keys,
            &mut node.left,
            &mut node.right,
        );
    }

    fn visit_hash_join_generic_inner(
        &mut self,
        left_schema: &mut FactorizedSchema,
        right_schema: &mut FactorizedSchema,
        hash_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        probe_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        left: &mut Box<LogicalNodeEnum>,
        right: &mut Box<LogicalNodeEnum>,
    ) {
        let left_keys = Self::contextual_keys_to_groups(hash_keys, left_schema);
        let right_keys = Self::contextual_keys_to_groups(probe_keys, right_schema);
        let left_to_flatten =
            FlattenAll::get_groups_pos_to_flatten_for_groups(&left_keys, left_schema);
        let right_to_flatten =
            FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&right_keys, right_schema);
        if !left_to_flatten.is_empty() {
            let new_left = self.append_flattens((**left).clone(), &left_to_flatten, left_schema);
            **left = new_left;
            for pos in &left_to_flatten {
                left_schema.flatten_group(*pos);
            }
        }
        if !right_to_flatten.is_empty() {
            let new_right =
                self.append_flattens((**right).clone(), &right_to_flatten, right_schema);
            **right = new_right;
            for pos in &right_to_flatten {
                right_schema.flatten_group(*pos);
            }
        }
    }

    fn visit_hash_join_right(
        &mut self,
        left_schema: &mut FactorizedSchema,
        right_schema: &mut FactorizedSchema,
        hash_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        probe_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        left: &mut Box<LogicalNodeEnum>,
        right: &mut Box<LogicalNodeEnum>,
    ) {
        let left_keys = Self::contextual_keys_to_groups(hash_keys, left_schema);
        let right_keys = Self::contextual_keys_to_groups(probe_keys, right_schema);
        let left_to_flatten =
            FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&left_keys, left_schema);
        let right_to_flatten =
            FlattenAll::get_groups_pos_to_flatten_for_groups(&right_keys, right_schema);
        if !left_to_flatten.is_empty() {
            let new_left = self.append_flattens((**left).clone(), &left_to_flatten, left_schema);
            **left = new_left;
            for pos in &left_to_flatten {
                left_schema.flatten_group(*pos);
            }
        }
        if !right_to_flatten.is_empty() {
            let new_right =
                self.append_flattens((**right).clone(), &right_to_flatten, right_schema);
            **right = new_right;
            for pos in &right_to_flatten {
                right_schema.flatten_group(*pos);
            }
        }
    }

    /// Full-outer join key policy, mirroring `binary_join_full_outer` in
    /// the compute path: both sides can emit unmatched rows, so both key
    /// sides flatten fully instead of keeping one side alive.
    fn visit_hash_join_full_outer(
        &mut self,
        left_schema: &mut FactorizedSchema,
        right_schema: &mut FactorizedSchema,
        hash_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        probe_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        left: &mut Box<LogicalNodeEnum>,
        right: &mut Box<LogicalNodeEnum>,
    ) {
        let left_keys = Self::contextual_keys_to_groups(hash_keys, left_schema);
        let right_keys = Self::contextual_keys_to_groups(probe_keys, right_schema);
        let left_to_flatten =
            FlattenAll::get_groups_pos_to_flatten_for_groups(&left_keys, left_schema);
        let right_to_flatten =
            FlattenAll::get_groups_pos_to_flatten_for_groups(&right_keys, right_schema);
        if !left_to_flatten.is_empty() {
            let new_left = self.append_flattens((**left).clone(), &left_to_flatten, left_schema);
            **left = new_left;
            for pos in &left_to_flatten {
                left_schema.flatten_group(*pos);
            }
        }
        if !right_to_flatten.is_empty() {
            let new_right =
                self.append_flattens((**right).clone(), &right_to_flatten, right_schema);
            **right = new_right;
            for pos in &right_to_flatten {
                right_schema.flatten_group(*pos);
            }
        }
    }

    fn contextual_keys_to_groups(
        keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        schema: &FactorizedSchema,
    ) -> HashSet<FGroupPos> {
        let mut set = HashSet::new();
        for k in keys {
            let eid = k.id().clone();
            if let Some(pos) = schema.get_group_pos(&eid) {
                set.insert(pos);
            }
        }
        set
    }

    /// Barrier helpers: the compute path for barrier operators flattens
    /// everything, so the rewriter inserts the matching `FlattenAll` nodes
    /// explicitly instead of leaving the flattening implicit.
    fn flattened_schema(schema: &FactorizedSchema) -> FactorizedSchema {
        let mut out = schema.clone();
        out.flatten_all();
        out
    }

    fn barrier_groups(schema: &FactorizedSchema) -> HashSet<FGroupPos> {
        FlattenAll::get_groups_pos_to_flatten_for_groups(&schema.groups_pos_in_scope(), schema)
    }

    fn flatten_barrier_child(&mut self, child: &mut LogicalNodeEnum, schema: &FactorizedSchema) {
        let to_flatten = Self::barrier_groups(schema);
        if !to_flatten.is_empty() {
            let new_child = self.append_flattens(child.clone(), &to_flatten, schema);
            *child = new_child;
        }
    }

    fn flatten_barrier_single(
        &mut self,
        input: Option<&mut Box<LogicalNodeEnum>>,
        schema: &FactorizedSchema,
    ) {
        if let Some(child) = input {
            self.flatten_barrier_child(child, schema);
        }
    }

    fn flatten_barrier_binary(
        &mut self,
        left: &mut LogicalNodeEnum,
        right: &mut LogicalNodeEnum,
        left_schema: &FactorizedSchema,
        right_schema: &FactorizedSchema,
    ) {
        self.flatten_barrier_child(left, left_schema);
        self.flatten_barrier_child(right, right_schema);
    }

    /// Append Flatten nodes for each group position.
    pub fn append_flattens(
        &mut self,
        mut child: LogicalNodeEnum,
        groups_pos: &HashSet<FGroupPos>,
        schema: &FactorizedSchema,
    ) -> LogicalNodeEnum {
        let mut sorted: Vec<FGroupPos> = groups_pos.iter().copied().collect();
        sorted.sort_unstable();
        for pos in sorted {
            child = self.append_flatten_if_necessary(child, pos, schema);
        }
        child
    }

    pub fn append_flatten_if_necessary(
        &mut self,
        child: LogicalNodeEnum,
        group_pos: FGroupPos,
        schema: &FactorizedSchema,
    ) -> LogicalNodeEnum {
        // Out-of-range positions are rewriter bugs: fail loudly in every
        // build profile so a stale decision never degrades into a silent
        // row-shape change. Flat groups are intentional no-ops: skip the
        // node but record the position so EXPLAIN/`cbo_notes` keep the
        // decision visible instead of dropping it silently.
        assert!(
            schema.get_group(group_pos).is_some(),
            "append_flatten: group {} out of range for {} groups",
            group_pos,
            schema.num_groups()
        );
        if let Some(group) = schema.get_group(group_pos) {
            if group.is_flat() {
                self.skipped_flat_groups.push(group_pos);
                return child;
            }
        } else {
            return child;
        }
        let mut flatten = LogicalFlattenNode::new(group_pos, child);
        flatten.set_group_columns(schema.member_names(group_pos));
        flatten.set_expected_groups(schema.num_groups() as FGroupPos);
        LogicalNodeEnum::Flatten(flatten)
    }

    /// Helper: compute flatten groups for a set of expression ids using the resolver.
    pub fn groups_for_projection(
        exprs: &[ExpressionId],
        schema: &FactorizedSchema,
        store: &HashMap<ExpressionId, graphdb_core::Expression>,
    ) -> HashSet<FGroupPos> {
        FlattenAllButOne::get_groups_pos_to_flatten_for_exprs(exprs, schema, store)
    }
}

impl Default for FactorizationRewriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::node_id_generator::next_node_id;
    use crate::planning::plan::factorization::FactorizedSchema;
    use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
    use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;

    fn scan() -> LogicalNodeEnum {
        LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
            id: next_node_id(),
            space_id: 1,
            space_name: "test".to_string(),
            tag: None,
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

    #[test]
    fn append_flatten_if_necessary_flat_noop() {
        let mut schema = FactorizedSchema::new();
        let g = schema.create_flat_group(false);
        let child = scan();
        let mut rewriter = FactorizationRewriter::new();
        let out = rewriter.append_flatten_if_necessary(child, g, &schema);
        assert_eq!(out.type_name(), "ScanVertices");
    }

    #[test]
    fn append_flatten_flat_noop_is_recorded_for_diagnostics() {
        let mut schema = FactorizedSchema::new();
        let g = schema.create_flat_group(false);
        let child = scan();
        let mut rewriter = FactorizationRewriter::new();
        let out = rewriter.append_flatten_if_necessary(child, g, &schema);
        assert_eq!(out.type_name(), "ScanVertices");
        assert_eq!(rewriter.take_skipped_flat_groups(), vec![g]);
        // Drained exactly once; a second take observes no residue.
        assert!(rewriter.take_skipped_flat_groups().is_empty());
    }

    #[test]
    fn append_flatten_snapshots_group_column_mapping() {
        use graphdb_core::types::expr::ExpressionId;
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope(ExpressionId::new(1), g0);
        schema.insert_to_group_and_scope_with_name(ExpressionId::new(2), Some("b".to_string()), g1);
        let child = scan();
        let mut rewriter = FactorizationRewriter::new();
        let out = rewriter.append_flatten_if_necessary(child, g1, &schema);
        if let LogicalNodeEnum::Flatten(f) = out {
            assert_eq!(f.group_pos(), g1);
            assert_eq!(f.group_columns(), &["b".to_string()]);
            assert_eq!(f.expected_groups(), Some(2));
        } else {
            panic!("expected flatten");
        }
    }

    #[test]
    fn append_flatten_if_necessary_unflat() {
        let mut schema = FactorizedSchema::new();
        let g = schema.create_group();
        let child = scan();
        let mut rewriter = FactorizationRewriter::new();
        let out = rewriter.append_flatten_if_necessary(child, g, &schema);
        assert_eq!(out.type_name(), "Flatten");
        if let LogicalNodeEnum::Flatten(f) = out {
            assert_eq!(f.group_pos(), g);
        } else {
            panic!("expected flatten");
        }
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn append_flatten_out_of_range_reports() {
        let mut schema = FactorizedSchema::new();
        schema.create_flat_group(false);
        let child = scan();
        let mut rewriter = FactorizationRewriter::new();
        let _ = rewriter.append_flatten_if_necessary(child, 99, &schema);
    }

    #[test]
    fn rewrite_disabled_noop() {
        let mut root = scan();
        let mut rewriter = FactorizationRewriter::disabled();
        let before = root.type_name();
        rewriter.rewrite(&mut root);
        assert_eq!(root.type_name(), before);
    }

    #[test]
    fn aggregate_keys_flatten() {
        use graphdb_core::types::expr::contextual::ContextualExpression;
        use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
        use graphdb_core::types::expr::ExpressionMeta;
        use std::sync::Arc;
        let raw_ctx = ExpressionAnalysisContext::new();
        let ctx = Arc::new(raw_ctx);
        let expr = graphdb_core::Expression::Variable("a".to_string());
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        let ctx_a = ContextualExpression::new(id.clone(), ctx.clone());
        let mut child_schema = FactorizedSchema::new();
        let g0 = child_schema.create_flat_group(false);
        let g1 = child_schema.create_group();
        child_schema.insert_to_group_and_scope(id.clone(), g1);
        let _ = g0;
        let scan = scan();
        let mut agg = LogicalNodeEnum::Aggregate(
            crate::planning::plan::logical::logical_nodes::operation::LogicalAggregateNode {
                id: next_node_id(),
                input: Some(Box::new(scan.clone())),
                deps: vec![scan],
                group_key_exprs: vec![ctx_a],
                aggregation_functions: vec![],
                aggregation_args: vec![],
                aggregation_distinct: vec![],
                aggregation_filters: vec![],
                grouping_sets: vec![],
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            },
        );
        let mut rewriter = FactorizationRewriter::new();
        rewriter.rewrite(&mut agg);
        // After rewrite, if child had unflat dependency, a Flatten should be inserted
        // Our child_schema has unflat g1 containing a, and aggregate should flatten AllButOne (single -> no flatten)
        // So no flatten.
        assert!(
            !matches!(agg, LogicalNodeEnum::Aggregate(ref n) if n.input.as_ref().map(|c| matches!(c.as_ref(), LogicalNodeEnum::Flatten(_))).unwrap_or(false))
        );
    }

    #[test]
    fn window_partition_flatten() {
        let mut child_schema = FactorizedSchema::new();
        let g0 = child_schema.create_flat_group(false);
        let g1 = child_schema.create_group();
        child_schema
            .insert_to_group_and_scope(graphdb_core::types::expr::ExpressionId::new(10), g0);
        child_schema
            .insert_to_group_and_scope(graphdb_core::types::expr::ExpressionId::new(20), g1);
        // Window with partition_by referencing b (g1) should not panic
        let scan = scan();
        let mut window = LogicalNodeEnum::Window(
            crate::planning::plan::logical::logical_nodes::operation::LogicalWindowNode {
                id: next_node_id(),
                input: Some(Box::new(scan.clone())),
                deps: vec![scan],
                window_functions: vec![],
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            },
        );
        let mut rewriter = FactorizationRewriter::new();
        rewriter.rewrite(&mut window);
        assert_eq!(window.type_name(), "Window");
    }

    fn ctx_var(
        var: &str,
    ) -> (
        graphdb_core::types::expr::contextual::ContextualExpression,
        graphdb_core::types::expr::ExpressionId,
    ) {
        use graphdb_core::types::expr::contextual::ContextualExpression;
        use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
        use graphdb_core::types::expr::ExpressionMeta;
        use std::sync::Arc;
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(graphdb_core::Expression::Variable(
            var.to_string(),
        )));
        (ContextualExpression::new(id.clone(), ctx), id)
    }

    fn get_neighbors_with_output(
        out_expr: graphdb_core::types::expr::contextual::ContextualExpression,
    ) -> LogicalNodeEnum {
        use crate::planning::plan::logical::logical_nodes::access::LogicalGetNeighborsNode;
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

    #[test]
    fn select_rewriter_keeps_branch_factorization() {
        use crate::optimizer::factorization::RemoveFactorizationRewriter;
        use crate::planning::plan::logical::logical_nodes::control_flow::LogicalSelectNode;
        // Branch output "b" is unflat; the condition only reads "b", so a
        // single unflat group survives FlattenAllButOne and no Flatten is
        // inserted: refined per-branch handling, not a blind flatten-all.
        let (out_expr, _) = ctx_var("b");
        let (cond, _) = ctx_var("b");
        let nbr = get_neighbors_with_output(out_expr);
        let mut select = LogicalNodeEnum::Select(LogicalSelectNode {
            id: next_node_id(),
            condition: cond,
            if_branch: Some(Box::new(nbr)),
            else_branch: None,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        FactorizationRewriter::new().rewrite(&mut select);
        assert!(
            !RemoveFactorizationRewriter::has_flatten_public(&select),
            "condition on a single unflat branch group must not insert Flatten"
        );
    }

    #[test]
    fn assign_rewriter_no_flatten_for_single_unflat_dependent() {
        use crate::optimizer::factorization::RemoveFactorizationRewriter;
        use crate::planning::plan::logical::logical_nodes::graph_ops::LogicalAssignNode;
        // Neighbor output "b" is unflat; the assignment only reads "b", so
        // the single unflat group survives FlattenAllButOne and no Flatten
        // is inserted: write-path counterpart of the Project rule.
        // NOTE: one shared context with distinct ids: the rhs must resolve
        // by variable *name*, not by accidental id collision.
        let ctx = std::sync::Arc::new(
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let out_id = ctx.register_expression(graphdb_core::types::expr::ExpressionMeta::new(
            graphdb_core::Expression::Variable("b".to_string()),
        ));
        let rhs_id = ctx.register_expression(graphdb_core::types::expr::ExpressionMeta::new(
            graphdb_core::Expression::Variable("b".to_string()),
        ));
        assert_ne!(out_id, rhs_id);
        let out_expr =
            graphdb_core::types::expr::contextual::ContextualExpression::new(out_id, ctx.clone());
        let rhs =
            graphdb_core::types::expr::contextual::ContextualExpression::new(rhs_id, ctx.clone());
        let nbr = get_neighbors_with_output(out_expr);
        let mut assign = LogicalNodeEnum::Assign(LogicalAssignNode {
            id: next_node_id(),
            input: Some(Box::new(nbr)),
            deps: vec![],
            assignments: vec![("c".to_string(), rhs)],
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        FactorizationRewriter::new().rewrite(&mut assign);
        assert!(
            !RemoveFactorizationRewriter::has_flatten_public(&assign),
            "assignment on a single unflat group must not insert Flatten"
        );
    }

    #[test]
    fn assign_rewriter_flattens_for_unresolvable_rhs() {
        use crate::optimizer::factorization::RemoveFactorizationRewriter;
        use crate::planning::plan::logical::logical_nodes::graph_ops::LogicalAssignNode;
        // Unresolvable right-hand side conservatively flattens the unflat
        // neighbor group, mirroring the Project fallback.
        let ctx = std::sync::Arc::new(
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let out_id = ctx.register_expression(graphdb_core::types::expr::ExpressionMeta::new(
            graphdb_core::Expression::Variable("b".to_string()),
        ));
        let rhs_id = ctx.register_expression(graphdb_core::types::expr::ExpressionMeta::new(
            graphdb_core::Expression::Variable("ghost".to_string()),
        ));
        assert_ne!(out_id, rhs_id);
        let out_expr =
            graphdb_core::types::expr::contextual::ContextualExpression::new(out_id, ctx.clone());
        let rhs =
            graphdb_core::types::expr::contextual::ContextualExpression::new(rhs_id, ctx.clone());
        let nbr = get_neighbors_with_output(out_expr);
        let mut assign = LogicalNodeEnum::Assign(LogicalAssignNode {
            id: next_node_id(),
            input: Some(Box::new(nbr)),
            deps: vec![],
            assignments: vec![("c".to_string(), rhs)],
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        FactorizationRewriter::new().rewrite(&mut assign);
        assert!(
            RemoveFactorizationRewriter::has_flatten_public(&assign),
            "unresolvable assignment rhs must flatten the unflat group"
        );
    }

    #[test]
    fn semi_join_rewriter_flattens_unflat_probe_keys() {
        use crate::planning::plan::logical::logical_nodes::join::LogicalSemiJoinNode;
        // Guard for keeping the generic join arm as is: probe-side key
        // groups are flattened by the rewriter before compute, so the
        // keep-first rule never sees unflat keys on real plans and a
        // key-aware compute arm would be unobservable.
        // NOTE: the hash key intentionally shares the output expression
        // (same id), matching production id-threading from exists_planner.
        let (out_expr, _) = ctx_var("b");
        let nbr = get_neighbors_with_output(out_expr.clone());
        let mut join = LogicalNodeEnum::SemiJoin(LogicalSemiJoinNode {
            id: next_node_id(),
            left: Box::new(nbr),
            right: Box::new(scan()),
            hash_keys: vec![out_expr],
            probe_keys: vec![],
            deps: vec![],
            join_condition: None,
            anti: false,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        FactorizationRewriter::new().rewrite(&mut join);
        if let LogicalNodeEnum::SemiJoin(n) = &join {
            assert!(
                matches!(n.left.as_ref(), LogicalNodeEnum::Flatten(_)),
                "unflat probe key must be flattened before the membership test"
            );
        } else {
            panic!("expected semi join");
        }
    }

    #[test]
    fn full_outer_rewriter_flattens_both_key_sides() {
        use crate::optimizer::factorization::RemoveFactorizationRewriter;
        use crate::planning::plan::logical::logical_nodes::join::LogicalFullOuterJoinNode;
        // Each side carries its join key in an unflat GetNeighbors group;
        // full-outer must flatten both (either side can emit unmatched
        // rows), unlike Inner which would keep the right side alive.
        let (left_key, _) = ctx_var("lk");
        let (right_key, _) = ctx_var("rk");
        let left = get_neighbors_with_output(left_key.clone());
        let right = get_neighbors_with_output(right_key.clone());
        let mut join = LogicalNodeEnum::FullOuterJoin(LogicalFullOuterJoinNode {
            id: next_node_id(),
            left: Box::new(left),
            right: Box::new(right),
            hash_keys: vec![left_key],
            probe_keys: vec![right_key],
            deps: vec![],
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        FactorizationRewriter::new().rewrite(&mut join);
        if let LogicalNodeEnum::FullOuterJoin(n) = &join {
            assert!(
                RemoveFactorizationRewriter::has_flatten_public(&n.left),
                "full-outer left key side must be flattened"
            );
            assert!(
                RemoveFactorizationRewriter::has_flatten_public(&n.right),
                "full-outer right key side must be flattened"
            );
        } else {
            panic!("expected full outer join");
        }
    }

    #[test]
    fn project_with_rand_falls_back_to_flatten_all() {
        use crate::optimizer::factorization::RemoveFactorizationRewriter;
        use crate::planning::plan::logical::logical_nodes::operation::LogicalProjectNode;
        use graphdb_core::types::expr::contextual::ContextualExpression;
        use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
        use graphdb_core::types::expr::ExpressionMeta;
        use std::sync::Arc;
        // Neighbor output "b" lives in a single unflat group. A
        // deterministic projection keeps it factorized, but rand() must be
        // evaluated tuple-at-a-time, so the rewriter falls back to
        // flatten-all and inserts a Flatten even for the single group.
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let out_id = ctx.register_expression(ExpressionMeta::new(
            graphdb_core::Expression::Variable("b".to_string()),
        ));
        let rand_id =
            ctx.register_expression(ExpressionMeta::new(graphdb_core::Expression::Function {
                name: "rand".to_string(),
                args: vec![],
            }));
        let out_expr = ContextualExpression::new(out_id, ctx.clone());
        let rand_expr = ContextualExpression::new(rand_id, ctx);
        let nbr = get_neighbors_with_output(out_expr);
        let mut project = LogicalNodeEnum::Project(LogicalProjectNode {
            id: next_node_id(),
            input: Some(Box::new(nbr.clone())),
            deps: vec![nbr],
            columns: vec![graphdb_core::YieldColumn::new(rand_expr, "r".to_string())],
            output_var: None,
            col_names: vec!["r".to_string()],
            column_types: vec![],
        });
        FactorizationRewriter::new().rewrite(&mut project);
        assert!(
            RemoveFactorizationRewriter::has_flatten_public(&project),
            "rand() projection must flatten the single unflat group"
        );
    }

    #[test]
    fn project_deterministic_keeps_single_unflat_group() {
        use crate::optimizer::factorization::RemoveFactorizationRewriter;
        use crate::planning::plan::logical::logical_nodes::operation::LogicalProjectNode;
        use graphdb_core::types::expr::contextual::ContextualExpression;
        use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
        use graphdb_core::types::expr::ExpressionMeta;
        use std::sync::Arc;
        // Control case for the rand() fallback: projecting the unflat
        // variable itself keeps the single group factorized.
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let out_id = ctx.register_expression(ExpressionMeta::new(
            graphdb_core::Expression::Variable("b".to_string()),
        ));
        let col_id = ctx.register_expression(ExpressionMeta::new(
            graphdb_core::Expression::Variable("b".to_string()),
        ));
        assert_ne!(out_id, col_id);
        let out_expr = ContextualExpression::new(out_id, ctx.clone());
        let col_expr = ContextualExpression::new(col_id, ctx);
        let nbr = get_neighbors_with_output(out_expr);
        let mut project = LogicalNodeEnum::Project(LogicalProjectNode {
            id: next_node_id(),
            input: Some(Box::new(nbr.clone())),
            deps: vec![nbr],
            columns: vec![graphdb_core::YieldColumn::new(col_expr, "b".to_string())],
            output_var: None,
            col_names: vec!["b".to_string()],
            column_types: vec![],
        });
        FactorizationRewriter::new().rewrite(&mut project);
        assert!(
            !RemoveFactorizationRewriter::has_flatten_public(&project),
            "deterministic projection on one unflat group must stay factorized"
        );
    }

    #[test]
    fn limit_keeps_single_unflat_group() {
        use crate::optimizer::factorization::RemoveFactorizationRewriter;
        use crate::planning::plan::logical::logical_nodes::operation::LogicalLimitNode;
        // FlattenAllButOne over a single unflat group inserts nothing (a
        // FlattenAll policy would flatten it): limit counts flattened rows
        // but keeps the one surviving group alive.
        let (out_expr, _) = ctx_var("b");
        let nbr = get_neighbors_with_output(out_expr);
        let mut limit = LogicalNodeEnum::Limit(LogicalLimitNode {
            id: next_node_id(),
            input: Some(Box::new(nbr.clone())),
            deps: vec![nbr],
            offset: 0,
            count: 10,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        FactorizationRewriter::new().rewrite(&mut limit);
        assert!(
            !RemoveFactorizationRewriter::has_flatten_public(&limit),
            "limit over a single unflat group must not insert Flatten"
        );
    }

    #[test]
    fn sort_flattens_unflat_key_group() {
        use crate::optimizer::factorization::RemoveFactorizationRewriter;
        use crate::planning::plan::core::nodes::operation::sort_node::SortItem;
        use crate::planning::plan::logical::logical_nodes::operation::LogicalSortNode;
        // Sort keys drive FlattenAll (not AllButOne): the single unflat
        // group holding key "b" is flattened so the sort can materialize
        // key order.
        let (out_expr, _) = ctx_var("b");
        let nbr = get_neighbors_with_output(out_expr);
        let mut sort = LogicalNodeEnum::Sort(LogicalSortNode {
            id: next_node_id(),
            input: Some(Box::new(nbr.clone())),
            deps: vec![nbr],
            sort_items: vec![SortItem::column_asc("b".to_string())],
            limit: None,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        FactorizationRewriter::new().rewrite(&mut sort);
        assert!(
            RemoveFactorizationRewriter::has_flatten_public(&sort),
            "sort over an unflat key group must insert Flatten"
        );
    }

    #[test]
    fn sort_single_group_fast_path() {
        use crate::optimizer::factorization::RemoveFactorizationRewriter;
        use crate::planning::plan::core::nodes::operation::sort_node::SortItem;
        use crate::planning::plan::logical::logical_nodes::operation::LogicalSortNode;
        // A single group needs no flattening: the factorized table can be
        // sorted and scanned back without changing the schema shape.
        let mut sort = LogicalNodeEnum::Sort(LogicalSortNode {
            id: next_node_id(),
            input: Some(Box::new(scan())),
            deps: vec![scan()],
            sort_items: vec![SortItem::column_asc("a".to_string())],
            limit: None,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        FactorizationRewriter::new().rewrite(&mut sort);
        assert!(
            !RemoveFactorizationRewriter::has_flatten_public(&sort),
            "sort over a single group must not insert Flatten"
        );
    }

    #[test]
    fn sort_unresolved_key_falls_back_to_flatten_all() {
        use crate::optimizer::factorization::RemoveFactorizationRewriter;
        use crate::planning::plan::core::nodes::operation::sort_node::SortItem;
        use crate::planning::plan::logical::logical_nodes::operation::LogicalSortNode;
        // An unresolvable sort key conservatively flattens every unflat
        // group rather than silently keeping factorization.
        let (out_expr, _) = ctx_var("b");
        let nbr = get_neighbors_with_output(out_expr);
        let mut sort = LogicalNodeEnum::Sort(LogicalSortNode {
            id: next_node_id(),
            input: Some(Box::new(nbr.clone())),
            deps: vec![nbr],
            sort_items: vec![SortItem::column_asc("ghost".to_string())],
            limit: None,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        FactorizationRewriter::new().rewrite(&mut sort);
        assert!(
            RemoveFactorizationRewriter::has_flatten_public(&sort),
            "sort with an unresolvable key must flatten the unflat group"
        );
    }

    #[test]
    fn assign_two_rhs_on_single_group_no_flatten() {
        use crate::optimizer::factorization::RemoveFactorizationRewriter;
        use crate::planning::plan::logical::logical_nodes::graph_ops::LogicalAssignNode;
        // Per-assignment handling: two right-hand sides reading the same
        // single unflat group each keep it alive, so no Flatten is inserted.
        let ctx = std::sync::Arc::new(
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let out_id = ctx.register_expression(graphdb_core::types::expr::ExpressionMeta::new(
            graphdb_core::Expression::Variable("b".to_string()),
        ));
        let rhs1_id = ctx.register_expression(graphdb_core::types::expr::ExpressionMeta::new(
            graphdb_core::Expression::Variable("b".to_string()),
        ));
        let rhs2_id = ctx.register_expression(graphdb_core::types::expr::ExpressionMeta::new(
            graphdb_core::Expression::Variable("b".to_string()),
        ));
        let out_expr =
            graphdb_core::types::expr::contextual::ContextualExpression::new(out_id, ctx.clone());
        let rhs1 =
            graphdb_core::types::expr::contextual::ContextualExpression::new(rhs1_id, ctx.clone());
        let rhs2 =
            graphdb_core::types::expr::contextual::ContextualExpression::new(rhs2_id, ctx.clone());
        let nbr = get_neighbors_with_output(out_expr);
        let mut assign = LogicalNodeEnum::Assign(LogicalAssignNode {
            id: next_node_id(),
            input: Some(Box::new(nbr)),
            deps: vec![],
            assignments: vec![("c".to_string(), rhs1), ("d".to_string(), rhs2)],
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        });
        FactorizationRewriter::new().rewrite(&mut assign);
        assert!(
            !RemoveFactorizationRewriter::has_flatten_public(&assign),
            "two assignments on one unflat group must stay factorized"
        );
    }
}
