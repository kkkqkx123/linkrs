use std::collections::HashSet;

use crate::planning::plan::factorization::{FGroupPos, FactorizationError, FactorizedSchema};
use crate::planning::plan::logical::LogicalNodeEnum;

use super::FactorizationRewriter;

impl FactorizationRewriter {
    pub(super) fn barrier_groups(schema: &FactorizedSchema) -> HashSet<FGroupPos> {
        crate::optimizer::factorization::FlattenAll::get_groups_pos_to_flatten_for_groups(
            &schema.groups_pos_in_scope(),
            schema,
        )
    }

    pub(super) fn flatten_barrier_child(
        &mut self,
        child: &mut LogicalNodeEnum,
        schema: &FactorizedSchema,
    ) -> Result<(), FactorizationError> {
        let to_flatten = Self::barrier_groups(schema);
        self.replace_node_and_flatten(child, &to_flatten, schema)
    }

    pub(super) fn flatten_barrier_single(
        &mut self,
        input: Option<&mut Box<LogicalNodeEnum>>,
        schema: &FactorizedSchema,
    ) -> Result<(), FactorizationError> {
        if let Some(child) = input {
            self.flatten_barrier_child(child, schema)?;
        }
        Ok(())
    }

    pub(super) fn flatten_barrier_binary(
        &mut self,
        left: &mut LogicalNodeEnum,
        right: &mut LogicalNodeEnum,
        left_schema: &FactorizedSchema,
        right_schema: &FactorizedSchema,
    ) -> Result<(), FactorizationError> {
        self.flatten_barrier_child(left, left_schema)?;
        self.flatten_barrier_child(right, right_schema)?;
        Ok(())
    }

    pub(super) fn append_flattens(
        &mut self,
        plan: &mut LogicalNodeEnum,
        groups: &HashSet<FGroupPos>,
    ) -> Result<(), FactorizationError> {
        if groups.is_empty() {
            return Ok(());
        }
        let mut flattens = groups.iter().copied().collect::<Vec<_>>();
        flattens.sort_unstable();
        for pos in flattens {
            *plan = LogicalNodeEnum::Flatten(
                crate::planning::plan::logical::logical_nodes::flatten::LogicalFlattenNode {
                    id: crate::planning::plan::core::node_id_generator::next_node_id(),
                    input: Some(Box::new(plan.clone())),
                    group_pos: pos,
                    output_var: None,
                    col_names: vec![],
                    column_types: vec![],
                    expected_groups: None,
                    group_columns: vec![],
                },
            );
        }
        Ok(())
    }

    pub(super) fn replace_child_and_flatten(
        &mut self,
        child: &mut Box<LogicalNodeEnum>,
        groups: &HashSet<FGroupPos>,
        schema: &mut FactorizedSchema,
    ) -> Result<(), FactorizationError> {
        if groups.is_empty() {
            return Ok(());
        }
        let mut new_child = child.clone();
        self.append_flattens(&mut new_child, groups)?;
        for pos in groups {
            schema.flatten_group(*pos)?;
        }
        *child = new_child;
        Ok(())
    }

    pub(super) fn replace_node_and_flatten(
        &mut self,
        node: &mut LogicalNodeEnum,
        groups: &HashSet<FGroupPos>,
        _schema: &FactorizedSchema,
    ) -> Result<(), FactorizationError> {
        if groups.is_empty() {
            return Ok(());
        }
        let mut new_node = node.clone();
        self.append_flattens(&mut new_node, groups)?;
        *node = new_node;
        Ok(())
    }
}
