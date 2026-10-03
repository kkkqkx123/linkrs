use std::collections::HashSet;

use crate::planning::plan::factorization::{FGroupPos, FactorizationError, FactorizedSchema};
use crate::planning::plan::logical::LogicalNodeEnum;

use super::FactorizationRewriter;

impl FactorizationRewriter {
    pub(super) fn visit_hash_join_inner(
        &mut self,
        node: &mut crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode,
        left_schema: &mut FactorizedSchema,
        right_schema: &mut FactorizedSchema,
    ) -> Result<(), FactorizationError> {
        self.visit_hash_join_generic_inner(
            left_schema,
            right_schema,
            &node.hash_keys,
            &node.probe_keys,
            &mut node.left,
            &mut node.right,
            true,
        )?;
        Ok(())
    }

    pub(super) fn visit_hash_join_left(
        &mut self,
        node: &mut crate::planning::plan::logical::logical_nodes::join::LogicalLeftJoinNode,
        left_schema: &mut FactorizedSchema,
        right_schema: &mut FactorizedSchema,
    ) -> Result<(), FactorizationError> {
        self.visit_hash_join_generic_inner(
            left_schema,
            right_schema,
            &node.hash_keys,
            &node.probe_keys,
            &mut node.left,
            &mut node.right,
            false,
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn visit_hash_join_generic_inner(
        &mut self,
        left_schema: &mut FactorizedSchema,
        right_schema: &mut FactorizedSchema,
        hash_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        probe_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        left: &mut Box<LogicalNodeEnum>,
        right: &mut Box<LogicalNodeEnum>,
        allow_probe_skip: bool,
    ) -> Result<(), FactorizationError> {
        let left_keys = Self::contextual_keys_to_groups(hash_keys, left_schema);
        let right_keys = Self::contextual_keys_to_groups(probe_keys, right_schema);
        let left_to_flatten =
            if allow_probe_skip && !Self::require_flat_probe_keys(hash_keys, probe_keys, right) {
                HashSet::new()
            } else {
                crate::optimizer::factorization::FlattenAll::get_groups_pos_to_flatten_for_groups(
                    &left_keys,
                    left_schema,
                )
            };
        let right_to_flatten =
            crate::optimizer::factorization::FlattenAllButOne::get_groups_pos_to_flatten_for_groups(
                &right_keys,
                right_schema,
            );
        if !left_to_flatten.is_empty() {
            self.replace_child_and_flatten(left, &left_to_flatten, &mut *left_schema)?;
            for pos in &left_to_flatten {
                left_schema.flatten_group(*pos)?;
            }
        }
        if !right_to_flatten.is_empty() {
            self.replace_child_and_flatten(right, &right_to_flatten, &mut *right_schema)?;
            for pos in &right_to_flatten {
                right_schema.flatten_group(*pos)?;
            }
        }
        Ok(())
    }

    pub(super) fn require_flat_probe_keys(
        hash_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        probe_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        right: &LogicalNodeEnum,
    ) -> bool {
        if hash_keys.len() > 1 || probe_keys.len() > 1 {
            return true;
        }
        let (Some(probe_key), Some(build_key)) = (hash_keys.first(), probe_keys.first()) else {
            return true;
        };
        let (Some(probe_var), Some(build_var)) = (probe_key.as_variable(), build_key.as_variable())
        else {
            return true;
        };
        if probe_var != build_var {
            return true;
        }
        !Self::build_side_yields_unique_node_id(right, &build_var)
    }

    pub(super) fn build_side_yields_unique_node_id(node: &LogicalNodeEnum, var: &str) -> bool {
        match node {
            LogicalNodeEnum::Filter(n) => n
                .input
                .as_deref()
                .is_some_and(|child| Self::build_side_yields_unique_node_id(child, var)),
            LogicalNodeEnum::Flatten(n) => n
                .input
                .as_deref()
                .is_some_and(|child| Self::build_side_yields_unique_node_id(child, var)),
            LogicalNodeEnum::Limit(n) => n
                .input
                .as_deref()
                .is_some_and(|child| Self::build_side_yields_unique_node_id(child, var)),
            LogicalNodeEnum::Project(n) => n
                .input
                .as_deref()
                .is_some_and(|child| Self::build_side_yields_unique_node_id(child, var)),
            LogicalNodeEnum::ScanVertices(n) => n.output_var.as_deref() == Some(var),
            _ => false,
        }
    }

    pub(super) fn visit_hash_join_right(
        &mut self,
        left_schema: &mut FactorizedSchema,
        right_schema: &mut FactorizedSchema,
        hash_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        probe_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        left: &mut Box<LogicalNodeEnum>,
        right: &mut Box<LogicalNodeEnum>,
    ) -> Result<(), FactorizationError> {
        let left_keys = Self::contextual_keys_to_groups(hash_keys, left_schema);
        let right_keys = Self::contextual_keys_to_groups(probe_keys, right_schema);
        let left_to_flatten =
            crate::optimizer::factorization::FlattenAllButOne::get_groups_pos_to_flatten_for_groups(
                &left_keys,
                left_schema,
            );
        let right_to_flatten =
            crate::optimizer::factorization::FlattenAll::get_groups_pos_to_flatten_for_groups(
                &right_keys,
                right_schema,
            );
        if !left_to_flatten.is_empty() {
            self.replace_child_and_flatten(left, &left_to_flatten, &mut *left_schema)?;
            for pos in &left_to_flatten {
                left_schema.flatten_group(*pos)?;
            }
        }
        if !right_to_flatten.is_empty() {
            self.replace_child_and_flatten(right, &right_to_flatten, &mut *right_schema)?;
            for pos in &right_to_flatten {
                right_schema.flatten_group(*pos)?;
            }
        }
        Ok(())
    }

    pub(super) fn visit_hash_join_full_outer(
        &mut self,
        left_schema: &mut FactorizedSchema,
        right_schema: &mut FactorizedSchema,
        hash_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        probe_keys: &[graphdb_core::types::expr::contextual::ContextualExpression],
        left: &mut Box<LogicalNodeEnum>,
        right: &mut Box<LogicalNodeEnum>,
    ) -> Result<(), FactorizationError> {
        let left_keys = Self::contextual_keys_to_groups(hash_keys, left_schema);
        let right_keys = Self::contextual_keys_to_groups(probe_keys, right_schema);
        let left_to_flatten =
            crate::optimizer::factorization::FlattenAll::get_groups_pos_to_flatten_for_groups(
                &left_keys,
                left_schema,
            );
        let right_to_flatten =
            crate::optimizer::factorization::FlattenAll::get_groups_pos_to_flatten_for_groups(
                &right_keys,
                right_schema,
            );
        if !left_to_flatten.is_empty() {
            self.replace_child_and_flatten(left, &left_to_flatten, &mut *left_schema)?;
            for pos in &left_to_flatten {
                left_schema.flatten_group(*pos)?;
            }
        }
        if !right_to_flatten.is_empty() {
            self.replace_child_and_flatten(right, &right_to_flatten, &mut *right_schema)?;
            for pos in &right_to_flatten {
                right_schema.flatten_group(*pos)?;
            }
        }
        Ok(())
    }

    pub(super) fn contextual_keys_to_groups(
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
}
