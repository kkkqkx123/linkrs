use std::collections::{HashMap, HashSet};

use linkrs_core::types::expr::ExpressionId;

use super::flatten_resolver::{FlattenAll, FlattenAllButOne};
use crate::planning::plan::factorization::{
    FGroupPos, FactorizationError, FactorizedSchema, FactorizedSchemaCompute,
};
use crate::planning::plan::logical::LogicalNodeEnum;

mod flatten;
mod join;

pub struct FactorizationRewriter {
    pub enabled: bool,
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

    pub fn rewrite(&mut self, plan: &mut LogicalNodeEnum) -> Result<(), FactorizationError> {
        if !self.enabled {
            return Ok(());
        }
        self.visit_operator(plan)?;
        Ok(())
    }

    pub fn take_skipped_flat_groups(&mut self) -> Vec<FGroupPos> {
        std::mem::take(&mut self.skipped_flat_groups)
    }

    fn visit_operator(
        &mut self,
        node: &mut LogicalNodeEnum,
    ) -> Result<FactorizedSchema, FactorizationError> {
        match node {
            LogicalNodeEnum::Project(n) => {
                let mut child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
                let store = Self::build_store_for_project(n);
                let has_random = store
                    .values()
                    .any(crate::optimizer::analysis::expression::NondeterministicChecker::contains_random);
                if has_random {
                    let groups = child_schema.groups_pos_in_scope();
                    let to_flatten =
                        FlattenAll::get_groups_pos_to_flatten_for_groups(&groups, &child_schema);
                    if !to_flatten.is_empty() {
                        if let Some(child) = n.input.as_mut() {
                            self.replace_child_and_flatten(child, &to_flatten, &mut child_schema)?;
                        }
                        for pos in &to_flatten {
                            child_schema.flatten_group(*pos)?;
                        }
                    }
                } else {
                    for expr_id in Self::expr_ids_for_project(n) {
                        let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_expr(
                            &expr_id,
                            &child_schema,
                            &store,
                        );
                        if !to_flatten.is_empty() {
                            if let Some(child) = n.input.as_mut() {
                                self.replace_child_and_flatten(
                                    child,
                                    &to_flatten,
                                    &mut child_schema,
                                )?;
                            }
                            for pos in &to_flatten {
                                child_schema.flatten_group(*pos)?;
                            }
                        }
                    }
                }
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Filter(n) => {
                let mut child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
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
                        self.replace_child_and_flatten(child, &to_flatten, &mut child_schema)?;
                    }
                    for pos in &to_flatten {
                        child_schema.flatten_group(*pos)?;
                    }
                    return node.compute_factorized_schema(&[child_schema]);
                }
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Aggregate(n) => {
                let mut child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
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
                        self.replace_child_and_flatten(child, &to_flatten, &mut child_schema)?;
                    }
                    for pos in &to_flatten {
                        child_schema.flatten_group(*pos)?;
                    }
                    return node.compute_factorized_schema(&[child_schema]);
                }
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Sort(n) => {
                let mut child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
                let to_flatten = if child_schema.num_groups() > 1 {
                    let (expr_ids, store) = Self::sort_key_store(n, &child_schema);
                    let mut dependent = HashSet::new();
                    let mut unresolved = false;
                    for expr_id in &expr_ids {
                        let mut analyzer =
                            super::group_dependency_analyzer::GroupDependencyAnalyzer::with_expr_store(
                                &child_schema,
                                false,
                                &store,
                            );
                        analyzer.visit(expr_id);
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
                        self.replace_child_and_flatten(child, &to_flatten, &mut child_schema)?;
                    }
                    for pos in &to_flatten {
                        child_schema.flatten_group(*pos)?;
                    }
                    return node.compute_factorized_schema(&[child_schema]);
                }
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Window(n) => {
                let mut child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
                let groups = child_schema.groups_pos_in_scope();
                let to_flatten =
                    FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, &child_schema);
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        self.replace_child_and_flatten(child, &to_flatten, &mut child_schema)?;
                    }
                    for pos in &to_flatten {
                        child_schema.flatten_group(*pos)?;
                    }
                    return node.compute_factorized_schema(&[child_schema]);
                }
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Limit(n) => {
                let mut child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
                let groups = child_schema.groups_pos_in_scope();
                let to_flatten =
                    FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, &child_schema);
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        self.replace_child_and_flatten(child, &to_flatten, &mut child_schema)?;
                    }
                    for pos in &to_flatten {
                        child_schema.flatten_group(*pos)?;
                    }
                    return node.compute_factorized_schema(&[child_schema]);
                }
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Skip(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::TopN(n) => {
                let mut child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
                let groups = child_schema.groups_pos_in_scope();
                let to_flatten =
                    FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&groups, &child_schema);
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        self.replace_child_and_flatten(child, &to_flatten, &mut child_schema)?;
                    }
                    for pos in &to_flatten {
                        child_schema.flatten_group(*pos)?;
                    }
                    return node.compute_factorized_schema(&[child_schema]);
                }
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Dedup(n) => {
                let mut child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
                let groups = child_schema.groups_pos_in_scope();
                let to_flatten =
                    FlattenAll::get_groups_pos_to_flatten_for_groups(&groups, &child_schema);
                if !to_flatten.is_empty() {
                    if let Some(child) = n.input.as_mut() {
                        self.replace_child_and_flatten(child, &to_flatten, &mut child_schema)?;
                    }
                    for pos in &to_flatten {
                        child_schema.flatten_group(*pos)?;
                    }
                    return node.compute_factorized_schema(&[child_schema]);
                }
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::InnerJoin(n) => {
                let mut left_schema = self.visit_operator(&mut n.left)?;
                let mut right_schema = self.visit_operator(&mut n.right)?;
                self.visit_hash_join_inner(n, &mut left_schema, &mut right_schema)?;
                node.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::LeftJoin(n) => {
                let mut left_schema = self.visit_operator(&mut n.left)?;
                let mut right_schema = self.visit_operator(&mut n.right)?;
                self.visit_hash_join_left(n, &mut left_schema, &mut right_schema)?;
                node.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::RightJoin(n) => {
                let mut left_schema = self.visit_operator(&mut n.left)?;
                let mut right_schema = self.visit_operator(&mut n.right)?;
                self.visit_hash_join_right(
                    &mut left_schema,
                    &mut right_schema,
                    &n.hash_keys,
                    &n.probe_keys,
                    &mut n.left,
                    &mut n.right,
                )?;
                node.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::CrossJoin(n) => {
                let mut left_schema = self.visit_operator(&mut n.left)?;
                let mut right_schema = self.visit_operator(&mut n.right)?;
                if n.hash_keys.is_empty() && n.probe_keys.is_empty() {
                } else {
                    self.visit_hash_join_generic_inner(
                        &mut left_schema,
                        &mut right_schema,
                        &n.hash_keys,
                        &n.probe_keys,
                        &mut n.left,
                        &mut n.right,
                        false,
                    )?;
                }
                node.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::FullOuterJoin(n) => {
                let mut left_schema = self.visit_operator(&mut n.left)?;
                let mut right_schema = self.visit_operator(&mut n.right)?;
                self.visit_hash_join_full_outer(
                    &mut left_schema,
                    &mut right_schema,
                    &n.hash_keys,
                    &n.probe_keys,
                    &mut n.left,
                    &mut n.right,
                )?;
                node.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::SemiJoin(n) => {
                let mut left_schema = self.visit_operator(&mut n.left)?;
                let mut right_schema = self.visit_operator(&mut n.right)?;
                self.visit_hash_join_generic_inner(
                    &mut left_schema,
                    &mut right_schema,
                    &n.hash_keys,
                    &n.probe_keys,
                    &mut n.left,
                    &mut n.right,
                    false,
                )?;
                node.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::Traverse(n) => {
                let effective_schema = if let Some(child) = n.input.as_mut() {
                    let mut schema = self.visit_operator(child)?;
                    if let Some(pos) = schema.unflat_group_pos() {
                        let mut to_flatten = HashSet::new();
                        to_flatten.insert(pos);
                        self.replace_child_and_flatten(child, &to_flatten, &mut schema)?;
                        schema.flatten_group(pos)?;
                        schema
                    } else {
                        schema
                    }
                } else {
                    FactorizedSchema::new()
                };
                node.compute_factorized_schema(&[effective_schema])
            }
            LogicalNodeEnum::Expand(n) => {
                let mut child_schema = FactorizedSchema::new();
                for dep in &mut n.deps {
                    let mut schema = self.visit_operator(dep)?;
                    if let Some(pos) = schema.unflat_group_pos() {
                        let mut to_flatten = HashSet::new();
                        to_flatten.insert(pos);
                        self.replace_node_and_flatten(dep, &to_flatten, &schema)?;
                        schema.flatten_group(pos)?;
                    }
                    if child_schema.num_groups() == 0 {
                        child_schema = schema;
                    }
                }
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::ExpandAll(n) => {
                let mut child_schema = FactorizedSchema::new();
                for dep in &mut n.deps {
                    let mut schema = self.visit_operator(dep)?;
                    if let Some(pos) = schema.unflat_group_pos() {
                        let mut to_flatten = HashSet::new();
                        to_flatten.insert(pos);
                        self.replace_node_and_flatten(dep, &to_flatten, &schema)?;
                        schema.flatten_group(pos)?;
                    }
                    if child_schema.num_groups() == 0 {
                        child_schema = schema;
                    }
                }
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::BiExpand(n) => {
                let mut left_schema = self.visit_operator(&mut n.left)?;
                let mut right_schema = self.visit_operator(&mut n.right)?;
                if let Some(pos) = left_schema.unflat_group_pos() {
                    let mut to_flatten = HashSet::new();
                    to_flatten.insert(pos);
                    self.replace_child_and_flatten(&mut n.left, &to_flatten, &mut left_schema)?;
                    left_schema.flatten_group(pos)?;
                }
                if let Some(pos) = right_schema.unflat_group_pos() {
                    let mut to_flatten = HashSet::new();
                    to_flatten.insert(pos);
                    self.replace_child_and_flatten(&mut n.right, &to_flatten, &mut right_schema)?;
                    right_schema.flatten_group(pos)?;
                }
                node.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::BiTraverse(n) => {
                let mut left_schema = self.visit_operator(&mut n.left)?;
                let mut right_schema = self.visit_operator(&mut n.right)?;
                if let Some(pos) = left_schema.unflat_group_pos() {
                    let mut to_flatten = HashSet::new();
                    to_flatten.insert(pos);
                    self.replace_child_and_flatten(&mut n.left, &to_flatten, &mut left_schema)?;
                    left_schema.flatten_group(pos)?;
                }
                if let Some(pos) = right_schema.unflat_group_pos() {
                    let mut to_flatten = HashSet::new();
                    to_flatten.insert(pos);
                    self.replace_child_and_flatten(&mut n.right, &to_flatten, &mut right_schema)?;
                    right_schema.flatten_group(pos)?;
                }
                node.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::AppendVertices(n) => {
                let mut child_schema = FactorizedSchema::new();
                for dep in &mut n.deps {
                    let mut schema = self.visit_operator(dep)?;
                    if let Some(pos) = schema.unflat_group_pos() {
                        let mut to_flatten = HashSet::new();
                        to_flatten.insert(pos);
                        self.replace_node_and_flatten(dep, &to_flatten, &schema)?;
                        schema.flatten_group(pos)?;
                    }
                    if child_schema.num_groups() == 0 {
                        child_schema = schema;
                    }
                }
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Unwind(n) => {
                let mut child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
                let list_id = n.list_expression.id().clone();
                let mut store = HashMap::new();
                if let Some(expr) = n.list_expression.get_expression() {
                    store.insert(list_id.clone(), expr);
                }
                let to_flatten =
                    FlattenAll::get_groups_pos_to_flatten_for_expr(&list_id, &child_schema, &store);
                let to_flatten = if to_flatten.is_empty() && !child_schema.is_flat_schema() {
                    let mut analyzer =
                        crate::optimizer::factorization::GroupDependencyAnalyzer::with_expr_store(
                            &child_schema,
                            false,
                            &store,
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
                        self.replace_child_and_flatten(child, &to_flatten, &mut child_schema)?;
                    }
                    for pos in &to_flatten {
                        child_schema.flatten_group(*pos)?;
                    }
                    return node.compute_factorized_schema(&[child_schema]);
                }
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Union(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    let mut schema = self.visit_operator(dep)?;
                    self.flatten_barrier_child(dep, &schema)?;
                    schema.flatten_all()?;
                    child_schemas.push(schema);
                }
                node.compute_factorized_schema(&child_schemas)
            }
            LogicalNodeEnum::Minus(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    let mut schema = self.visit_operator(dep)?;
                    self.flatten_barrier_child(dep, &schema)?;
                    schema.flatten_all()?;
                    child_schemas.push(schema);
                }
                node.compute_factorized_schema(&child_schemas)
            }
            LogicalNodeEnum::Intersect(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    let mut schema = self.visit_operator(dep)?;
                    self.flatten_barrier_child(dep, &schema)?;
                    schema.flatten_all()?;
                    child_schemas.push(schema);
                }
                node.compute_factorized_schema(&child_schemas)
            }
            LogicalNodeEnum::WcoIntersect(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    child_schemas.push(self.visit_operator(dep)?);
                }
                let probe_to_flatten = child_schemas
                    .first()
                    .map(|probe_schema| n.get_groups_to_flatten_on_probe_side(probe_schema))
                    .unwrap_or_default();
                if !probe_to_flatten.is_empty() {
                    self.replace_node_and_flatten(
                        &mut n.deps[0],
                        &probe_to_flatten,
                        &child_schemas[0],
                    )?;
                    for pos in &probe_to_flatten {
                        child_schemas[0].flatten_group(*pos)?;
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
                        self.replace_node_and_flatten(
                            &mut n.deps[child_idx],
                            &to_flatten,
                            &child_schemas[child_idx],
                        )?;
                        for pos in &to_flatten {
                            child_schemas[child_idx].flatten_group(*pos)?;
                        }
                    }
                }
                node.compute_factorized_schema(&child_schemas)
            }
            LogicalNodeEnum::Flatten(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::GetVertices(n) => {
                let mut child_schema = FactorizedSchema::new();
                for dep in &mut n.deps {
                    let schema = self.visit_operator(dep)?;
                    if child_schema.num_groups() == 0 {
                        child_schema = schema;
                    }
                }
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::GetNeighbors(n) => {
                let mut child_schema = FactorizedSchema::new();
                for dep in &mut n.deps {
                    let mut schema = self.visit_operator(dep)?;
                    if let Some(pos) = schema.unflat_group_pos() {
                        let mut to_flatten = HashSet::new();
                        to_flatten.insert(pos);
                        self.replace_node_and_flatten(dep, &to_flatten, &schema)?;
                        schema.flatten_group(pos)?;
                    }
                    if child_schema.num_groups() == 0 {
                        child_schema = schema;
                    }
                }
                node.compute_factorized_schema(&[child_schema])
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
            | LogicalNodeEnum::DeleteIndex(_)
            | LogicalNodeEnum::PipeDeleteVertices(_)
            | LogicalNodeEnum::PipeDeleteEdges(_)
            | LogicalNodeEnum::CopyFrom(_)
            | LogicalNodeEnum::CopyTo(_)
            | LogicalNodeEnum::FulltextSearch(_)
            | LogicalNodeEnum::FulltextLookup(_)
            | LogicalNodeEnum::MatchFulltext(_) => node.compute_factorized_schema(&[]),
            #[cfg(feature = "vector")]
            LogicalNodeEnum::VectorSearch(_)
            | LogicalNodeEnum::VectorLookup(_)
            | LogicalNodeEnum::VectorMatch(_) => node.compute_factorized_schema(&[]),
            LogicalNodeEnum::Assign(n) => {
                let mut child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
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
                            self.replace_child_and_flatten(child, &to_flatten, &mut child_schema)?;
                        }
                        for pos in &to_flatten {
                            child_schema.flatten_group(*pos)?;
                        }
                    }
                }
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Remove(n) => {
                let mut child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
                self.flatten_barrier_single(n.input.as_mut(), &child_schema)?;
                child_schema.flatten_all()?;
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::DataCollect(n) => {
                let mut child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
                self.flatten_barrier_single(n.input.as_mut(), &child_schema)?;
                child_schema.flatten_all()?;
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Materialize(n) => {
                let mut child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
                self.flatten_barrier_single(n.input.as_mut(), &child_schema)?;
                child_schema.flatten_all()?;
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::RollUpApply(n) => {
                let mut child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
                self.flatten_barrier_single(n.input.as_mut(), &child_schema)?;
                child_schema.flatten_all()?;
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Sample(n) => {
                let child_schema = if let Some(child) = n.input.as_mut() {
                    self.visit_operator(child)?
                } else {
                    FactorizedSchema::new()
                };
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Select(n) => {
                let cond_id = n.condition.id().clone();
                let mut store = HashMap::new();
                if let Some(expr) = n.condition.get_expression() {
                    store.insert(cond_id.clone(), expr);
                }
                let mut branch_schemas = Vec::new();
                if let Some(branch) = n.if_branch.as_mut() {
                    let mut schema = self.visit_operator(branch)?;
                    let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_expr(
                        &cond_id, &schema, &store,
                    );
                    if !to_flatten.is_empty() {
                        self.replace_child_and_flatten(branch, &to_flatten, &mut schema)?;
                        for pos in &to_flatten {
                            schema.flatten_group(*pos)?;
                        }
                        branch_schemas.push(schema);
                    } else {
                        branch_schemas.push(schema);
                    }
                }
                if let Some(branch) = n.else_branch.as_mut() {
                    let mut schema = self.visit_operator(branch)?;
                    let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_expr(
                        &cond_id, &schema, &store,
                    );
                    if !to_flatten.is_empty() {
                        self.replace_child_and_flatten(branch, &to_flatten, &mut schema)?;
                        for pos in &to_flatten {
                            schema.flatten_group(*pos)?;
                        }
                        branch_schemas.push(schema);
                    } else {
                        branch_schemas.push(schema);
                    }
                }
                if let Some(effective) = branch_schemas.into_iter().next() {
                    node.compute_factorized_schema(&[effective])
                } else {
                    node.compute_factorized_schema(&[])
                }
            }
            LogicalNodeEnum::Loop(n) => {
                let cond_id = n.condition.id().clone();
                let mut store = HashMap::new();
                if let Some(expr) = n.condition.get_expression() {
                    store.insert(cond_id.clone(), expr);
                }
                let child_schema = if let Some(body) = n.body.as_mut() {
                    let mut schema = self.visit_operator(body)?;
                    let to_flatten = FlattenAllButOne::get_groups_pos_to_flatten_for_expr(
                        &cond_id, &schema, &store,
                    );
                    if !to_flatten.is_empty() {
                        self.replace_child_and_flatten(body, &to_flatten, &mut schema)?;
                        for pos in &to_flatten {
                            schema.flatten_group(*pos)?;
                        }
                        schema
                    } else {
                        schema
                    }
                } else {
                    FactorizedSchema::new()
                };
                node.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::PatternApply(n) => {
                let mut left_schema = self.visit_operator(&mut n.left)?;
                let mut right_schema = self.visit_operator(&mut n.right)?;
                self.flatten_barrier_binary(
                    &mut n.left,
                    &mut n.right,
                    &left_schema,
                    &right_schema,
                )?;
                left_schema.flatten_all()?;
                right_schema.flatten_all()?;
                node.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::CorrelatedApply(n) => {
                let mut left_schema = self.visit_operator(&mut n.left)?;
                let mut right_schema = self.visit_operator(&mut n.right)?;
                self.flatten_barrier_binary(
                    &mut n.left,
                    &mut n.right,
                    &left_schema,
                    &right_schema,
                )?;
                left_schema.flatten_all()?;
                right_schema.flatten_all()?;
                node.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::Apply(n) => {
                let mut left_schema = self.visit_operator(n.left_input_mut())?;
                let mut right_schema = self.visit_operator(n.right_input_mut())?;
                {
                    let left = n.left_input_mut();
                    self.flatten_barrier_child(left, &left_schema)?;
                }
                {
                    let right = n.right_input_mut();
                    self.flatten_barrier_child(right, &right_schema)?;
                }
                left_schema.flatten_all()?;
                right_schema.flatten_all()?;
                node.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::MultiShortestPath(n) => {
                let mut left_schema = self.visit_operator(&mut n.left)?;
                let mut right_schema = self.visit_operator(&mut n.right)?;
                self.flatten_barrier_binary(
                    &mut n.left,
                    &mut n.right,
                    &left_schema,
                    &right_schema,
                )?;
                left_schema.flatten_all()?;
                right_schema.flatten_all()?;
                node.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::BFSShortest(n) => {
                let mut left_schema = self.visit_operator(&mut n.left)?;
                let mut right_schema = self.visit_operator(&mut n.right)?;
                self.flatten_barrier_binary(
                    &mut n.left,
                    &mut n.right,
                    &left_schema,
                    &right_schema,
                )?;
                left_schema.flatten_all()?;
                right_schema.flatten_all()?;
                node.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::AllPaths(n) => {
                let mut left_schema = self.visit_operator(&mut n.left)?;
                let mut right_schema = self.visit_operator(&mut n.right)?;
                self.flatten_barrier_binary(
                    &mut n.left,
                    &mut n.right,
                    &left_schema,
                    &right_schema,
                )?;
                left_schema.flatten_all()?;
                right_schema.flatten_all()?;
                node.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::ShortestPath(n) => {
                let mut left_schema = self.visit_operator(&mut n.left)?;
                let mut right_schema = self.visit_operator(&mut n.right)?;
                self.flatten_barrier_binary(
                    &mut n.left,
                    &mut n.right,
                    &left_schema,
                    &right_schema,
                )?;
                left_schema.flatten_all()?;
                right_schema.flatten_all()?;
                node.compute_factorized_schema(&[left_schema, right_schema])
            }
        }
    }

    fn build_store_for_project(
        node: &crate::planning::plan::logical::logical_nodes::operation::LogicalProjectNode,
    ) -> HashMap<ExpressionId, linkrs_core::Expression> {
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
    ) -> Vec<linkrs_core::types::expr::ExpressionId> {
        node.columns
            .iter()
            .map(|c| c.expression.id().clone())
            .collect()
    }

    fn sort_key_store(
        node: &crate::planning::plan::logical::logical_nodes::operation::LogicalSortNode,
        schema: &FactorizedSchema,
    ) -> (
        Vec<linkrs_core::types::expr::ExpressionId>,
        HashMap<ExpressionId, linkrs_core::Expression>,
    ) {
        let mut ids = Vec::with_capacity(node.sort_items.len());
        let mut store = HashMap::new();
        let mut candidate: u64 = 1 << 63;
        for item in &node.sort_items {
            while schema.is_expression_in_scope(&ExpressionId::new(candidate))
                || store.contains_key(&ExpressionId::new(candidate))
            {
                candidate = candidate.wrapping_add(1);
            }
            let id = ExpressionId::new(candidate);
            candidate = candidate.wrapping_add(1);
            store.insert(id.clone(), item.expression.clone());
            ids.push(id);
        }
        (ids, store)
    }
}

impl Default for FactorizationRewriter {
    fn default() -> Self {
        Self::new()
    }
}
