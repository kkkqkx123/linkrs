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
}

impl FactorizationRewriter {
    pub fn new() -> Self {
        Self { enabled: true }
    }

    pub fn disabled() -> Self {
        Self { enabled: false }
    }

    /// Rewrite the plan in place.
    pub fn rewrite(&self, plan: &mut LogicalNodeEnum) {
        if !self.enabled {
            return;
        }
        let _ = Self::visit_operator(plan);
    }

    fn visit_operator(node: &mut LogicalNodeEnum) -> FactorizedSchema {
        match node {
            LogicalNodeEnum::Project(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let exprs = Self::expr_ids_for_project(n);
                let store = Self::build_store_for_project(n);
                let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_exprs(
                    &exprs,
                    &child_schema,
                    &store,
                );
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            Self::append_flattens((**child).clone(), &to_flatten, &child_schema);
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
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let expr_id = n.condition.id().clone();
                let mut store = HashMap::new();
                if let Some(expr) = n.condition.get_expression() {
                    store.insert(expr_id.clone(), expr);
                }
                let to_flatten =
                    FlattenAll::get_groups_pos_to_flatten_for_expr(&expr_id, &child_schema, &store);
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            Self::append_flattens((**child).clone(), &to_flatten, &child_schema);
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
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let mut dependent: HashSet<FGroupPos> = HashSet::new();
                if !n.group_key_exprs.is_empty() {
                    for expr in &n.group_key_exprs {
                        let eid = expr.id().clone();
                        if let Some(pos) = child_schema.get_group_pos(&eid) {
                            dependent.insert(pos);
                        }
                    }
                } else {
                    // No group keys: global aggregation
                }
                let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_groups(
                    &dependent,
                    &child_schema,
                );
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            Self::append_flattens((**child).clone(), &to_flatten, &child_schema);
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
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let groups = child_schema.groups_pos_in_scope();
                let to_flatten =
                    FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, &child_schema);
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            Self::append_flattens((**child).clone(), &to_flatten, &child_schema);
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
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let groups = child_schema.groups_pos_in_scope();
                let to_flatten =
                    FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, &child_schema);
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            Self::append_flattens((**child).clone(), &to_flatten, &child_schema);
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
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::TopN(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let groups = child_schema.groups_pos_in_scope();
                let to_flatten =
                    FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, &child_schema);
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            Self::append_flattens((**child).clone(), &to_flatten, &child_schema);
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
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let groups = child_schema.groups_pos_in_scope();
                let to_flatten =
                    FlattenAll::get_groups_pos_to_flatten_for_groups(&groups, &child_schema);
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            Self::append_flattens((**child).clone(), &to_flatten, &child_schema);
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
                let left_schema = Self::visit_operator(&mut n.left);
                let right_schema = Self::visit_operator(&mut n.right);
                Self::visit_hash_join_inner(n, &left_schema, &right_schema);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::LeftJoin(n) => {
                let left_schema = Self::visit_operator(&mut n.left);
                let right_schema = Self::visit_operator(&mut n.right);
                Self::visit_hash_join_left(n, &left_schema, &right_schema);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::RightJoin(n) => {
                let left_schema = Self::visit_operator(&mut n.left);
                let right_schema = Self::visit_operator(&mut n.right);
                Self::visit_hash_join_right(
                    &left_schema,
                    &right_schema,
                    &n.hash_keys,
                    &n.probe_keys,
                    &mut n.left,
                    &mut n.right,
                );
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::CrossJoin(n) => {
                let left_schema = Self::visit_operator(&mut n.left);
                let right_schema = Self::visit_operator(&mut n.right);
                if n.hash_keys.is_empty() && n.probe_keys.is_empty() {
                    // No join keys: cross product needs no flatten; preserve factorization.
                } else {
                    Self::visit_hash_join_generic_inner(
                        &left_schema,
                        &right_schema,
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
                let left_schema = Self::visit_operator(&mut n.left);
                let right_schema = Self::visit_operator(&mut n.right);
                Self::visit_hash_join_generic_inner(
                    &left_schema,
                    &right_schema,
                    &n.hash_keys,
                    &n.probe_keys,
                    &mut n.left,
                    &mut n.right,
                );
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::SemiJoin(n) => {
                let left_schema = Self::visit_operator(&mut n.left);
                let right_schema = Self::visit_operator(&mut n.right);
                Self::visit_hash_join_generic_inner(
                    &left_schema,
                    &right_schema,
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
                    let schema = Self::visit_operator(child);
                    if let Some(pos) = schema.unflat_group_pos() {
                        let mut to_flatten = HashSet::new();
                        to_flatten.insert(pos);
                        let new_child =
                            Self::append_flattens((**child).clone(), &to_flatten, &schema);
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
                    let schema = Self::visit_operator(dep);
                    if let Some(pos) = schema.unflat_group_pos() {
                        let mut to_flatten = HashSet::new();
                        to_flatten.insert(pos);
                        let new_dep = Self::append_flattens(dep.clone(), &to_flatten, &schema);
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
                    let schema = Self::visit_operator(dep);
                    if let Some(pos) = schema.unflat_group_pos() {
                        let mut to_flatten = HashSet::new();
                        to_flatten.insert(pos);
                        let new_dep = Self::append_flattens(dep.clone(), &to_flatten, &schema);
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
                let mut left_schema = Self::visit_operator(&mut n.left);
                let mut right_schema = Self::visit_operator(&mut n.right);
                if let Some(pos) = left_schema.unflat_group_pos() {
                    let mut to_flatten = HashSet::new();
                    to_flatten.insert(pos);
                    let new_left =
                        Self::append_flattens((*n.left).clone(), &to_flatten, &left_schema);
                    *n.left = new_left;
                    left_schema.flatten_group(pos);
                }
                if let Some(pos) = right_schema.unflat_group_pos() {
                    let mut to_flatten = HashSet::new();
                    to_flatten.insert(pos);
                    let new_right =
                        Self::append_flattens((*n.right).clone(), &to_flatten, &right_schema);
                    *n.right = new_right;
                    right_schema.flatten_group(pos);
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::BiTraverse(n) => {
                let mut left_schema = Self::visit_operator(&mut n.left);
                let mut right_schema = Self::visit_operator(&mut n.right);
                if let Some(pos) = left_schema.unflat_group_pos() {
                    let mut to_flatten = HashSet::new();
                    to_flatten.insert(pos);
                    let new_left =
                        Self::append_flattens((*n.left).clone(), &to_flatten, &left_schema);
                    *n.left = new_left;
                    left_schema.flatten_group(pos);
                }
                if let Some(pos) = right_schema.unflat_group_pos() {
                    let mut to_flatten = HashSet::new();
                    to_flatten.insert(pos);
                    let new_right =
                        Self::append_flattens((*n.right).clone(), &to_flatten, &right_schema);
                    *n.right = new_right;
                    right_schema.flatten_group(pos);
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::AppendVertices(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    let schema = Self::visit_operator(dep);
                    if let Some(pos) = schema.unflat_group_pos() {
                        let mut to_flatten = HashSet::new();
                        to_flatten.insert(pos);
                        let new_dep = Self::append_flattens(dep.clone(), &to_flatten, &schema);
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
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Union(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    child_schemas.push(Self::visit_operator(dep));
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&child_schemas)
            }
            LogicalNodeEnum::Minus(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    child_schemas.push(Self::visit_operator(dep));
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&child_schemas)
            }
            LogicalNodeEnum::Intersect(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    child_schemas.push(Self::visit_operator(dep));
                }
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&child_schemas)
            }
            LogicalNodeEnum::Flatten(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::GetVertices(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    child_schemas.push(Self::visit_operator(dep));
                }
                let child_schema = child_schemas.first().cloned().unwrap_or_default();
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::GetNeighbors(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    let schema = Self::visit_operator(dep);
                    if let Some(pos) = schema.unflat_group_pos() {
                        let mut to_flatten = HashSet::new();
                        to_flatten.insert(pos);
                        let new_dep = Self::append_flattens(dep.clone(), &to_flatten, &schema);
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
                let child_schema = if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                for dep in &mut n.deps {
                    Self::visit_operator(dep);
                }
                // Write-path counterpart of Project: only the groups the
                // assigned right-hand sides depend on are flattened.
                let exprs = Self::expr_ids_for_assign(n);
                let store = Self::build_store_for_assign(n);
                let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_exprs(
                    &exprs,
                    &child_schema,
                    &store,
                );
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        let new_child =
                            Self::append_flattens((**child).clone(), &to_flatten, &child_schema);
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
            LogicalNodeEnum::Remove(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::DataCollect(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Materialize(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::RollUpApply(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Sample(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child)
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
                    let schema = Self::visit_operator(branch);
                    let to_flatten =
                        FlattenAllButOne::get_groups_pos_to_flatten_for_expr(
                            &cond_id,
                            &schema,
                            &store,
                        );
                    if !to_flatten.is_empty() {
                        let new_branch =
                            Self::append_flattens((**branch).clone(), &to_flatten, &schema);
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
                    let schema = Self::visit_operator(branch);
                    let to_flatten =
                        FlattenAllButOne::get_groups_pos_to_flatten_for_expr(
                            &cond_id,
                            &schema,
                            &store,
                        );
                    if !to_flatten.is_empty() {
                        let new_branch =
                            Self::append_flattens((**branch).clone(), &to_flatten, &schema);
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
                    let schema = Self::visit_operator(body);
                    let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_expr(
                        &cond_id,
                        &schema,
                        &store,
                    );
                    if !to_flatten.is_empty() {
                        let new_body =
                            Self::append_flattens((**body).clone(), &to_flatten, &schema);
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
                let left_schema = Self::visit_operator(&mut n.left);
                let right_schema = Self::visit_operator(&mut n.right);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::CorrelatedApply(n) => {
                let left_schema = Self::visit_operator(&mut n.left);
                let right_schema = Self::visit_operator(&mut n.right);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::Apply(n) => {
                let left_schema = Self::visit_operator(n.left_input_mut());
                let right_schema = Self::visit_operator(n.right_input_mut());
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::MultiShortestPath(n) => {
                let left_schema = Self::visit_operator(&mut n.left);
                let right_schema = Self::visit_operator(&mut n.right);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::BFSShortest(n) => {
                let left_schema = Self::visit_operator(&mut n.left);
                let right_schema = Self::visit_operator(&mut n.right);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::AllPaths(n) => {
                let left_schema = Self::visit_operator(&mut n.left);
                let right_schema = Self::visit_operator(&mut n.right);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::ShortestPath(n) => {
                let left_schema = Self::visit_operator(&mut n.left);
                let right_schema = Self::visit_operator(&mut n.right);
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

    fn build_store_for_assign(
        node: &crate::planning::plan::logical::logical_nodes::graph_ops::LogicalAssignNode,
    ) -> HashMap<ExpressionId, graphdb_core::Expression> {
        let mut store = HashMap::new();
        for (_, expr) in &node.assignments {
            if let Some(rhs) = expr.get_expression() {
                store.insert(expr.id().clone(), rhs);
            }
        }
        store
    }

    fn expr_ids_for_assign(
        node: &crate::planning::plan::logical::logical_nodes::graph_ops::LogicalAssignNode,
    ) -> Vec<graphdb_core::types::expr::ExpressionId> {
        node.assignments
            .iter()
            .map(|(_, expr)| expr.id().clone())
            .collect()
    }

    fn visit_hash_join_inner(
        node: &mut crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode,
        left_schema: &FactorizedSchema,
        right_schema: &FactorizedSchema,
    ) {
        Self::visit_hash_join_generic_inner(
            left_schema,
            right_schema,
            &node.hash_keys,
            &node.probe_keys,
            &mut node.left,
            &mut node.right,
        );
    }

    fn visit_hash_join_left(
        node: &mut crate::planning::plan::logical::logical_nodes::join::LogicalLeftJoinNode,
        left_schema: &FactorizedSchema,
        right_schema: &FactorizedSchema,
    ) {
        Self::visit_hash_join_generic_inner(
            left_schema,
            right_schema,
            &node.hash_keys,
            &node.probe_keys,
            &mut node.left,
            &mut node.right,
        );
    }

    fn visit_hash_join_generic_inner(
        left_schema: &FactorizedSchema,
        right_schema: &FactorizedSchema,
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
            let new_left = Self::append_flattens((**left).clone(), &left_to_flatten, left_schema);
            **left = new_left;
        }
        if !right_to_flatten.is_empty() {
            let new_right =
                Self::append_flattens((**right).clone(), &right_to_flatten, right_schema);
            **right = new_right;
        }
    }

    fn visit_hash_join_right(
        left_schema: &FactorizedSchema,
        right_schema: &FactorizedSchema,
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
            let new_left = Self::append_flattens((**left).clone(), &left_to_flatten, left_schema);
            **left = new_left;
        }
        if !right_to_flatten.is_empty() {
            let new_right =
                Self::append_flattens((**right).clone(), &right_to_flatten, right_schema);
            **right = new_right;
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

    /// Append Flatten nodes for each group position.
    pub fn append_flattens(
        mut child: LogicalNodeEnum,
        groups_pos: &HashSet<FGroupPos>,
        schema: &FactorizedSchema,
    ) -> LogicalNodeEnum {
        let mut sorted: Vec<FGroupPos> = groups_pos.iter().copied().collect();
        sorted.sort_unstable();
        for pos in sorted {
            child = Self::append_flatten_if_necessary(child, pos, schema);
        }
        child
    }

    pub fn append_flatten_if_necessary(
        child: LogicalNodeEnum,
        group_pos: FGroupPos,
        schema: &FactorizedSchema,
    ) -> LogicalNodeEnum {
        if let Some(group) = schema.get_group(group_pos) {
            if group.is_flat() {
                return child;
            }
        } else {
            return child;
        }
        let flatten = LogicalFlattenNode::new(group_pos, child);
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

    pub fn groups_for_filter(
        expr: &ExpressionId,
        schema: &FactorizedSchema,
        store: &HashMap<ExpressionId, graphdb_core::Expression>,
    ) -> HashSet<FGroupPos> {
        FlattenAll::get_groups_pos_to_flatten_for_expr(expr, schema, store)
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
        let out = FactorizationRewriter::append_flatten_if_necessary(child, g, &schema);
        assert_eq!(out.type_name(), "ScanVertices");
    }

    #[test]
    fn append_flatten_if_necessary_unflat() {
        let mut schema = FactorizedSchema::new();
        let g = schema.create_group();
        let child = scan();
        let out = FactorizationRewriter::append_flatten_if_necessary(child, g, &schema);
        assert_eq!(out.type_name(), "Flatten");
        if let LogicalNodeEnum::Flatten(f) = out {
            assert_eq!(f.group_pos(), g);
        } else {
            panic!("expected flatten");
        }
    }

    #[test]
    fn rewrite_disabled_noop() {
        let mut root = scan();
        let rewriter = FactorizationRewriter::disabled();
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
                deps: vec![scan.clone()],
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
        let rewriter = FactorizationRewriter::new();
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
        let rewriter = FactorizationRewriter::new();
        rewriter.rewrite(&mut window);
        assert_eq!(window.type_name(), "Window");
    }

    fn ctx_var(var: &str) -> (
        graphdb_core::types::expr::contextual::ContextualExpression,
        graphdb_core::types::expr::ExpressionId,
    ) {
        use graphdb_core::types::expr::contextual::ContextualExpression;
        use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
        use graphdb_core::types::expr::ExpressionMeta;
        use std::sync::Arc;
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(
            graphdb_core::Expression::Variable(var.to_string()),
        ));
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
        use crate::planning::plan::logical::logical_nodes::control_flow::LogicalSelectNode;
        use crate::optimizer::factorization::RemoveFactorizationRewriter;
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
}
