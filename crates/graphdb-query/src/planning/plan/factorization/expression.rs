use std::collections::{HashMap, HashSet};

use super::{FGroupPos, FactorizationError, FactorizationGroup};
use graphdb_core::types::expr::ExpressionId;

/// Output schema with factorization structure.
///
/// Tracks flat/unflat groups and which expression belongs to which group.
/// Enforces the invariant that at most one group is unflat at any time.
#[derive(Debug, Clone, Default)]
pub struct FactorizedSchema {
    pub(super) groups: Vec<FactorizationGroup>,
    pub(super) expression_to_group: HashMap<ExpressionId, FGroupPos>,
    pub(super) expression_name_to_group: HashMap<String, FGroupPos>,
    pub(super) expression_id_to_name: HashMap<ExpressionId, String>,
    pub(super) expressions_in_scope: HashSet<ExpressionId>,
}

impl FactorizedSchema {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn num_groups(&self) -> usize {
        self.groups.len()
    }

    pub fn groups(&self) -> &[FactorizationGroup] {
        &self.groups
    }

    pub fn groups_mut(&mut self) -> &mut Vec<FactorizationGroup> {
        &mut self.groups
    }

    pub fn get_group(&self, pos: FGroupPos) -> Option<&FactorizationGroup> {
        self.groups.get(pos as usize)
    }

    pub fn get_group_mut(&mut self, pos: FGroupPos) -> Option<&mut FactorizationGroup> {
        self.groups.get_mut(pos as usize)
    }

    pub fn get_group_by_expression(&self, expr_id: &ExpressionId) -> Option<&FactorizationGroup> {
        let pos = self.get_group_pos(expr_id)?;
        self.get_group(pos)
    }

    #[cfg(test)]
    pub fn get_group_by_name(&self, name: &str) -> Option<&FactorizationGroup> {
        let pos = self.get_group_pos_by_name(name)?;
        self.get_group(pos)
    }

    pub fn create_group(&mut self) -> FGroupPos {
        let pos = self.groups.len() as FGroupPos;
        self.groups.push(FactorizationGroup::new());
        pos
    }

    pub fn create_flat_group(&mut self, single_state: bool) -> FGroupPos {
        let pos = self.groups.len() as FGroupPos;
        self.groups.push(FactorizationGroup::new_flat(single_state));
        pos
    }

    pub fn insert_to_scope(
        &mut self,
        expr_id: ExpressionId,
        group_pos: FGroupPos,
    ) -> Result<(), FactorizationError> {
        if (group_pos as usize) >= self.groups.len() {
            return Err(FactorizationError::GroupPosOutOfRange(group_pos));
        }
        if self.expression_to_group.contains_key(&expr_id) {
            return Err(FactorizationError::ExpressionAlreadyMapped(expr_id));
        }
        if self.expressions_in_scope.contains(&expr_id) {
            return Err(FactorizationError::ExpressionAlreadyInScope(expr_id));
        }
        self.expression_to_group.insert(expr_id.clone(), group_pos);
        self.expressions_in_scope.insert(expr_id);
        Ok(())
    }

    pub fn insert_to_scope_with_name(
        &mut self,
        expr_id: ExpressionId,
        name: String,
        group_pos: FGroupPos,
    ) -> Result<(), FactorizationError> {
        if (group_pos as usize) >= self.groups.len() {
            return Err(FactorizationError::GroupPosOutOfRange(group_pos));
        }
        self.expression_name_to_group
            .insert(name.clone(), group_pos);
        self.expression_id_to_name.insert(expr_id.clone(), name);
        self.insert_to_scope(expr_id, group_pos)?;
        Ok(())
    }

    pub fn insert_to_group_and_scope(
        &mut self,
        expr_id: ExpressionId,
        group_pos: FGroupPos,
    ) -> Result<(), FactorizationError> {
        self.insert_to_group_and_scope_with_name(expr_id, None, group_pos)
    }

    pub fn insert_to_group_and_scope_with_name(
        &mut self,
        expr_id: ExpressionId,
        name: Option<String>,
        group_pos: FGroupPos,
    ) -> Result<(), FactorizationError> {
        if (group_pos as usize) >= self.groups.len() {
            return Err(FactorizationError::GroupPosOutOfRange(group_pos));
        }
        if self.expression_to_group.contains_key(&expr_id) {
            return Err(FactorizationError::ExpressionAlreadyMapped(expr_id));
        }
        if self.expressions_in_scope.contains(&expr_id) {
            return Err(FactorizationError::ExpressionAlreadyInScope(expr_id));
        }
        let group = &mut self.groups[group_pos as usize];
        group.insert_expression_with_name(expr_id.clone(), name.clone())?;
        if let Some(n) = name {
            self.expression_name_to_group.insert(n.clone(), group_pos);
            self.expression_id_to_name.insert(expr_id.clone(), n);
        }
        self.expression_to_group.insert(expr_id.clone(), group_pos);
        self.expressions_in_scope.insert(expr_id);
        Ok(())
    }

    pub fn insert_to_group_and_scope_batch(
        &mut self,
        exprs: Vec<ExpressionId>,
        group_pos: FGroupPos,
    ) -> Result<(), FactorizationError> {
        for e in exprs {
            self.insert_to_group_and_scope(e, group_pos)?;
        }
        Ok(())
    }

    pub fn insert_to_scope_may_repeat(
        &mut self,
        expr_id: ExpressionId,
        group_pos: FGroupPos,
    ) -> Result<(), FactorizationError> {
        if (group_pos as usize) >= self.groups.len() {
            return Err(FactorizationError::GroupPosOutOfRange(group_pos));
        }
        self.expression_to_group.insert(expr_id.clone(), group_pos);
        self.expressions_in_scope.insert(expr_id);
        Ok(())
    }

    pub fn insert_to_group_and_scope_may_repeat(
        &mut self,
        expr_id: ExpressionId,
        group_pos: FGroupPos,
    ) -> Result<(), FactorizationError> {
        if (group_pos as usize) >= self.groups.len() {
            return Err(FactorizationError::GroupPosOutOfRange(group_pos));
        }
        let group = &mut self.groups[group_pos as usize];
        if !group.contains(&expr_id) {
            group.insert_expression(expr_id.clone())?;
        }
        self.expression_to_group.insert(expr_id.clone(), group_pos);
        self.expressions_in_scope.insert(expr_id);
        Ok(())
    }

    pub fn get_group_pos(&self, expr_id: &ExpressionId) -> Option<FGroupPos> {
        self.expression_to_group.get(expr_id).copied()
    }

    #[cfg(test)]
    pub fn get_group_pos_by_name(&self, name: &str) -> Option<FGroupPos> {
        self.get_group_pos_by_name_opt(name)
    }

    pub fn get_group_pos_by_name_opt(&self, name: &str) -> Option<FGroupPos> {
        self.expression_name_to_group.get(name).copied()
    }

    pub fn expression_name(&self, expr_id: &ExpressionId) -> Option<&str> {
        self.expression_id_to_name.get(expr_id).map(String::as_str)
    }

    pub fn insert_name_for_group(
        &mut self,
        name: String,
        group_pos: FGroupPos,
    ) -> Result<(), FactorizationError> {
        if (group_pos as usize) >= self.groups.len() {
            return Err(FactorizationError::GroupPosOutOfRange(group_pos));
        }
        self.expression_name_to_group.insert(name, group_pos);
        Ok(())
    }

    pub fn resolve_group_pos(
        &self,
        expr_id: Option<&ExpressionId>,
        name: Option<&str>,
    ) -> Option<FGroupPos> {
        if let Some(id) = expr_id {
            if let Some(pos) = self.get_group_pos(id) {
                return Some(pos);
            }
            if let Some(owned) = self.expression_name(id) {
                let owned = owned.to_string();
                if let Some(pos) = self.get_group_pos_by_name_opt(&owned) {
                    return Some(pos);
                }
            }
        }
        if let Some(n) = name {
            if let Some(pos) = self.get_group_pos_by_name_opt(n) {
                return Some(pos);
            }
        }
        None
    }

    pub fn member_names(&self, group_pos: FGroupPos) -> Vec<String> {
        let mut names: Vec<String> = self
            .expression_name_to_group
            .iter()
            .filter(|(_, pos)| **pos == group_pos)
            .map(|(name, _)| name.clone())
            .collect();
        names.sort();
        names
    }

    pub fn get_expression_pos(&self, expr_id: &ExpressionId) -> Option<(FGroupPos, usize)> {
        let gpos = self.get_group_pos(expr_id)?;
        let group = self.get_group(gpos)?;
        let pos = group.get_expression_pos(expr_id)?;
        Some((gpos, pos))
    }

    pub fn flatten_group(&mut self, pos: FGroupPos) -> Result<(), FactorizationError> {
        let group = self
            .get_group_mut(pos)
            .ok_or(FactorizationError::InvalidFlattenPos(pos))?;
        if !group.is_flat() {
            group.set_flat()?;
        }
        self.validate_at_most_one_unflat()
    }

    pub fn flatten_all(&mut self) -> Result<(), FactorizationError> {
        for i in 0..self.groups.len() {
            let pos = i as FGroupPos;
            if let Some(g) = self.get_group(pos) {
                if !g.is_flat() {
                    self.flatten_group(pos)?;
                }
            }
        }
        Ok(())
    }

    pub fn set_group_as_single_state(&mut self, pos: FGroupPos) -> Result<(), FactorizationError> {
        let group = self
            .get_group_mut(pos)
            .ok_or(FactorizationError::InvalidSingleStatePos(pos))?;
        if !group.is_single_state() {
            group.set_single_state()?;
        }
        Ok(())
    }

    pub fn is_expression_in_scope(&self, expr_id: &ExpressionId) -> bool {
        self.expression_to_group.contains_key(expr_id)
    }

    #[cfg(test)]
    pub fn is_name_in_scope(&self, name: &str) -> bool {
        self.expression_name_to_group.contains_key(name)
    }

    pub fn expressions_in_scope(&self) -> &HashSet<ExpressionId> {
        &self.expressions_in_scope
    }

    pub fn expressions_in_scope_for_group(&self, pos: FGroupPos) -> Vec<ExpressionId> {
        let group = match self.get_group(pos) {
            Some(g) => g,
            None => return Vec::new(),
        };
        group
            .expressions()
            .iter()
            .filter(|e| self.expressions_in_scope.contains(e))
            .cloned()
            .collect()
    }

    pub fn evaluable(&self, expr_id: &ExpressionId) -> bool {
        self.is_expression_in_scope(expr_id)
    }

    pub fn clear_expressions_in_scope(&mut self) {
        self.expression_to_group.clear();
        self.expression_name_to_group.clear();
        self.expression_id_to_name.clear();
        self.expressions_in_scope.clear();
    }

    pub fn groups_pos_in_scope(&self) -> HashSet<FGroupPos> {
        self.expression_to_group.values().copied().collect()
    }

    pub fn copy(&self) -> Self {
        self.clone()
    }

    pub fn clear(&mut self) {
        self.groups.clear();
        self.clear_expressions_in_scope();
    }

    pub fn has_unflat_group(&self) -> bool {
        self.groups.iter().any(|g| !g.is_flat())
    }

    pub fn unflat_group_pos(&self) -> Option<FGroupPos> {
        self.groups
            .iter()
            .enumerate()
            .find(|(_, g)| !g.is_flat())
            .map(|(i, _)| i as FGroupPos)
    }

    pub fn validate_at_most_one_unflat(&self) -> Result<(), FactorizationError> {
        let unflat = self.groups.iter().filter(|g| !g.is_flat()).count();
        if unflat > 1 {
            return Err(FactorizationError::TooManyUnflatGroups(unflat));
        }
        Ok(())
    }

    pub fn has_at_most_one_unflat(&self) -> bool {
        self.groups.iter().filter(|g| !g.is_flat()).count() <= 1
    }

    pub fn is_flat_schema(&self) -> bool {
        self.groups.iter().all(|g| g.is_flat())
    }

    pub fn flat_copy(&self) -> Result<Self, FactorizationError> {
        let mut copy = self.clone();
        copy.flatten_all()?;
        Ok(copy)
    }

    pub fn merge_groups_from(&mut self, other: &FactorizedSchema) -> HashMap<FGroupPos, FGroupPos> {
        let mut mapping = HashMap::new();
        for (idx, group) in other.groups.iter().enumerate() {
            let old_pos = idx as FGroupPos;
            let new_pos = self.groups.len() as FGroupPos;
            self.groups.push(group.clone());
            mapping.insert(old_pos, new_pos);
        }
        for (name, pos) in &other.expression_name_to_group {
            if let Some(new_pos) = mapping.get(pos) {
                self.expression_name_to_group.insert(name.clone(), *new_pos);
            }
        }
        for (expr_id, name) in &other.expression_id_to_name {
            self.expression_id_to_name
                .insert(expr_id.clone(), name.clone());
        }
        for (expr_id, pos) in &other.expression_to_group {
            if let Some(new_pos) = mapping.get(pos) {
                self.expression_to_group.insert(expr_id.clone(), *new_pos);
            }
        }
        for expr_id in &other.expressions_in_scope {
            if self.expression_to_group.contains_key(expr_id) {
                self.expressions_in_scope.insert(expr_id.clone());
            }
        }
        mapping
    }

    pub fn expression_to_group_iter(&self) -> impl Iterator<Item = (&ExpressionId, &FGroupPos)> {
        self.expression_to_group.iter()
    }

    pub fn expression_name_to_group_iter(&self) -> impl Iterator<Item = (&String, &FGroupPos)> {
        self.expression_name_to_group.iter()
    }
}
