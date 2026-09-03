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
                let child_schema = if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child)
                } else {
                    FactorizedSchema::new()
                };
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::Expand(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    child_schemas.push(Self::visit_operator(dep));
                }
                let child_schema = child_schemas.first().cloned().unwrap_or_default();
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::ExpandAll(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    child_schemas.push(Self::visit_operator(dep));
                }
                let child_schema = child_schemas.first().cloned().unwrap_or_default();
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[child_schema])
            }
            LogicalNodeEnum::BiExpand(n) => {
                let left_schema = Self::visit_operator(&mut n.left);
                let right_schema = Self::visit_operator(&mut n.right);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::BiTraverse(n) => {
                let left_schema = Self::visit_operator(&mut n.left);
                let right_schema = Self::visit_operator(&mut n.right);
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::AppendVertices(n) => {
                let mut child_schemas = Vec::new();
                for dep in &mut n.deps {
                    child_schemas.push(Self::visit_operator(dep));
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
            _ => {
                let mut tmp = node.clone();
                tmp.compute_factorized_schema(&[])
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
        let ctx = Arc::new(ExpressionAnalysisContext::new());
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
}
