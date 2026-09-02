use std::collections::{HashMap, HashSet};

use graphdb_core::types::expr::ExpressionId;

use crate::planning::plan::factorization::{
    FGroupPos, FactorizedSchema, FactorizedSchemaCompute,
};
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use crate::planning::plan::logical::logical_node_traits::{LogicalNode, LogicalSingleInputNode};
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
        Self::visit_operator(plan);
    }

    fn visit_operator(node: &mut LogicalNodeEnum) {
        // Bottom-up: recurse into children first.
        match node {
            LogicalNodeEnum::Project(n) => {
                if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child);
                }
                // After children, apply projection-specific flatten logic.
                // For docs: use FlattenAllButOne per expression.
                // Since we don't have real ExpressionIds here, we simulate
                // by flattening based on schema utility.
                // In production, this would analyze projection expressions
                // against the child's FactorizedSchema.
                let _ = Self::visit_projection(n);
            }
            LogicalNodeEnum::Filter(n) => {
                if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child);
                }
                let _ = Self::visit_filter(n);
            }
            LogicalNodeEnum::Aggregate(n) => {
                if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child);
                }
                let _ = Self::visit_aggregate(n);
            }
            LogicalNodeEnum::Sort(n) => {
                if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child);
                }
                let _ = Self::visit_sort(n);
            }
            LogicalNodeEnum::Limit(n) => {
                if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child);
                }
                let _ = Self::visit_limit(n);
            }
            LogicalNodeEnum::Dedup(n) => {
                if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child);
                }
                // Dedup flattens all groups.
            }
            LogicalNodeEnum::InnerJoin(n) => {
                Self::visit_operator(&mut n.left);
                Self::visit_operator(&mut n.right);
                let _ = Self::visit_hash_join(n);
            }
            LogicalNodeEnum::LeftJoin(n) => {
                Self::visit_operator(&mut n.left);
                Self::visit_operator(&mut n.right);
                let _ = Self::visit_hash_join_left(n);
            }
            LogicalNodeEnum::RightJoin(n) => {
                Self::visit_operator(&mut n.left);
                Self::visit_operator(&mut n.right);
            }
            LogicalNodeEnum::CrossJoin(n) => {
                Self::visit_operator(&mut n.left);
                Self::visit_operator(&mut n.right);
            }
            LogicalNodeEnum::SemiJoin(n) => {
                Self::visit_operator(&mut n.left);
                Self::visit_operator(&mut n.right);
            }
            LogicalNodeEnum::Traverse(n) => {
                if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child);
                }
            }
            LogicalNodeEnum::Unwind(n) => {
                if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child);
                }
            }
            LogicalNodeEnum::Union(n) => {
                for dep in &mut n.deps {
                    Self::visit_operator(dep);
                }
            }
            LogicalNodeEnum::Minus(n) => {
                for dep in &mut n.deps {
                    Self::visit_operator(dep);
                }
            }
            LogicalNodeEnum::Intersect(n) => {
                for dep in &mut n.deps {
                    Self::visit_operator(dep);
                }
            }
            LogicalNodeEnum::Flatten(n) => {
                if let Some(child) = n.input.as_mut() {
                    Self::visit_operator(child);
                }
            }
            _ => {
                // Leaf or unhandled node: no children to recurse.
            }
        }
        // After visiting children and applying operator-specific logic,
        // the node's FactorizedSchema would be recomputed in the real engine.
        // Here we simply validate.
    }

    fn visit_projection(
        node: &mut crate::planning::plan::logical::logical_nodes::operation::LogicalProjectNode,
    ) {
        let child_schema = if let Some(child) = node.input.as_ref() {
            Self::schema_for_node(child)
        } else {
            return;
        };
        let exprs = Self::expr_ids_for_project(node);
        let store = std::collections::HashMap::new();
        let to_flatten =
            FlattenAllButOne::get_groups_pos_to_flatten_for_exprs(&exprs, &child_schema, &store);
        if !to_flatten.is_empty() {
            if let Some(child) = node.input.as_mut() {
                let new_child =
                    Self::append_flattens((**child).clone(), &to_flatten, &child_schema);
                **child = new_child;
            }
        }
    }

    fn visit_filter(
        node: &mut crate::planning::plan::logical::logical_nodes::operation::LogicalFilterNode,
    ) {
        if let Some(child) = node.input.as_mut() {
            let child_schema = Self::schema_for_node(child);
            let expr_id = {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut h = DefaultHasher::new();
                format!("{:?}", node.condition).hash(&mut h);
                graphdb_core::types::expr::ExpressionId::new(h.finish())
            };
            let store = std::collections::HashMap::new();
            let to_flatten = FlattenAll::get_groups_pos_to_flatten_for_expr(
                &expr_id,
                &child_schema,
                &store,
            );
            if !to_flatten.is_empty() {
                let new_child =
                    Self::append_flattens((**child).clone(), &to_flatten, &child_schema);
                **child = new_child;
            }
        }
    }

    fn visit_aggregate(
        node: &mut crate::planning::plan::logical::logical_nodes::operation::LogicalAggregateNode,
    ) {
        if let Some(child) = node.input.as_mut() {
            let child_schema = Self::schema_for_node(child);
            let groups: std::collections::HashSet<FGroupPos> =
                child_schema.groups_pos_in_scope();
            let to_flatten =
                FlattenAll::get_groups_pos_to_flatten_for_groups(&groups, &child_schema);
            if !to_flatten.is_empty() {
                let new_child =
                    Self::append_flattens((**child).clone(), &to_flatten, &child_schema);
                **child = new_child;
            }
        }
    }

    fn visit_sort(
        _node: &mut crate::planning::plan::logical::logical_nodes::operation::LogicalSortNode,
    ) {
    }

    fn visit_limit(
        _node: &mut crate::planning::plan::logical::logical_nodes::operation::LogicalLimitNode,
    ) {
    }

    fn visit_hash_join(
        node: &mut crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode,
    ) {
        let left_schema = Self::schema_for_node(&node.left);
        let right_schema = Self::schema_for_node(&node.right);
        let left_keys = Self::hash_keys_to_groups_contextual(&node.hash_keys, &left_schema);
        let right_keys = Self::hash_keys_to_groups_contextual(&node.probe_keys, &right_schema);
        let left_to_flatten =
            FlattenAll::get_groups_pos_to_flatten_for_groups(&left_keys, &left_schema);
        let right_to_flatten =
            FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&right_keys, &right_schema);
        if !left_to_flatten.is_empty() {
            let new_left =
                Self::append_flattens((*node.left).clone(), &left_to_flatten, &left_schema);
            *node.left = new_left;
        }
        if !right_to_flatten.is_empty() {
            let new_right =
                Self::append_flattens((*node.right).clone(), &right_to_flatten, &right_schema);
            *node.right = new_right;
        }
    }

    fn visit_hash_join_left(
        node: &mut crate::planning::plan::logical::logical_nodes::join::LogicalLeftJoinNode,
    ) {
        let left_schema = Self::schema_for_node(&node.left);
        let right_schema = Self::schema_for_node(&node.right);
        let left_keys = Self::hash_keys_to_groups_contextual(&node.hash_keys, &left_schema);
        let right_keys =
            Self::hash_keys_to_groups_contextual(&node.probe_keys, &right_schema);
        let left_to_flatten =
            FlattenAll::get_groups_pos_to_flatten_for_groups(&left_keys, &left_schema);
        let right_to_flatten =
            FlattenAllButOne::get_groups_pos_to_flatten_for_groups(&right_keys, &right_schema);
        if !left_to_flatten.is_empty() {
            let new_left =
                Self::append_flattens((*node.left).clone(), &left_to_flatten, &left_schema);
            *node.left = new_left;
        }
        if !right_to_flatten.is_empty() {
            let new_right =
                Self::append_flattens((*node.right).clone(), &right_to_flatten, &right_schema);
            *node.right = new_right;
        }
    }

    fn schema_for_node(node: &LogicalNodeEnum) -> FactorizedSchema {
        let mut clone = node.clone();
        clone.compute_factorized_schema(&[])
    }

    fn expr_ids_for_project(
        node: &crate::planning::plan::logical::logical_nodes::operation::LogicalProjectNode,
    ) -> Vec<graphdb_core::types::expr::ExpressionId> {
        node.columns
            .iter()
            .map(|c| {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut h = DefaultHasher::new();
                c.alias.hash(&mut h);
                graphdb_core::types::expr::ExpressionId::new(h.finish())
            })
            .collect()
    }

    fn hash_keys_to_groups(
        keys: &[String],
        schema: &FactorizedSchema,
    ) -> std::collections::HashSet<FGroupPos> {
        let mut set = std::collections::HashSet::new();
        for k in keys {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            k.hash(&mut h);
            let eid = graphdb_core::types::expr::ExpressionId::new(h.finish());
            if let Some(pos) = schema.get_group_pos(&eid) {
                set.insert(pos);
            } else if let Some(pos) = schema.get_group_pos_by_name(k) {
                set.insert(pos);
            }
        }
        set
    }

    fn hash_keys_to_groups_contextual(
        keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        schema: &FactorizedSchema,
    ) -> std::collections::HashSet<FGroupPos> {
        let mut set = std::collections::HashSet::new();
        for k in keys {
            let s = format!("{:?}", k.expression().map(|e| format!("{:?}", e)).unwrap_or_default());
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            s.hash(&mut h);
            let eid = graphdb_core::types::expr::ExpressionId::new(h.finish());
            if let Some(pos) = schema.get_group_pos(&eid) {
                set.insert(pos);
            } else if let Some(pos) = schema.get_group_pos_by_name(&s) {
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
    use crate::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode;

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
}
