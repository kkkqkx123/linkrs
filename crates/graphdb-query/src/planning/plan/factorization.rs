use std::collections::{HashMap, HashSet};

use graphdb_core::types::expr::ExpressionId;

/// Factorization group position identifier.
pub type FGroupPos = u32;

/// Invalid group position sentinel.
pub const INVALID_F_GROUP_POS: FGroupPos = u32::MAX;

/// A group of expressions sharing the same nesting level.
///
/// Mirrors `lbug::planner::FactorizationGroup` in
/// `ref/ladybug/src/include/planner/operator/schema.h`.
#[derive(Debug, Clone)]
pub struct FactorizationGroup {
    flat: bool,
    single_state: bool,
    cardinality_multiplier: f64,
    expressions: Vec<ExpressionId>,
    expression_id_to_pos: HashMap<ExpressionId, usize>,
    expression_name_to_pos: HashMap<String, usize>,
}

impl FactorizationGroup {
    pub fn new() -> Self {
        Self {
            flat: false,
            single_state: false,
            cardinality_multiplier: 1.0,
            expressions: Vec::new(),
            expression_id_to_pos: HashMap::new(),
            expression_name_to_pos: HashMap::new(),
        }
    }

    pub fn new_flat(single_state: bool) -> Self {
        Self {
            flat: true,
            single_state,
            cardinality_multiplier: 1.0,
            expressions: Vec::new(),
            expression_id_to_pos: HashMap::new(),
            expression_name_to_pos: HashMap::new(),
        }
    }

    pub fn is_flat(&self) -> bool {
        self.flat
    }

    pub fn is_single_state(&self) -> bool {
        self.single_state
    }

    pub fn set_flat(&mut self) {
        assert!(!self.flat, "group already flat");
        self.flat = true;
    }

    pub fn set_single_state(&mut self) {
        assert!(!self.single_state, "group already single state");
        self.single_state = true;
        // A single-state group holds one row by construction and is treated
        // as flat here. This is stricter than engines that keep an unflat
        // group under multiplicity one; the at-most-one-unflat invariant
        // depends on the flat interpretation.
        if !self.flat {
            self.flat = true;
        }
    }

    pub fn cardinality_multiplier(&self) -> f64 {
        self.cardinality_multiplier
    }

    pub fn set_multiplier(&mut self, multiplier: f64) {
        self.cardinality_multiplier = multiplier;
    }

    pub fn expressions(&self) -> &[ExpressionId] {
        &self.expressions
    }

    pub fn len(&self) -> usize {
        self.expressions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.expressions.is_empty()
    }

    pub fn insert_expression(&mut self, expr_id: ExpressionId) {
        self.insert_expression_with_name(expr_id, None);
    }

    pub fn insert_expression_with_name(&mut self, expr_id: ExpressionId, name: Option<String>) {
        assert!(
            !self.expression_id_to_pos.contains_key(&expr_id),
            "duplicate expression id {:?} in group",
            expr_id
        );
        if let Some(n) = name.clone() {
            assert!(
                !self.expression_name_to_pos.contains_key(&n),
                "duplicate expression name {} in group",
                n
            );
            self.expression_name_to_pos
                .insert(n, self.expressions.len());
        }
        self.expression_id_to_pos
            .insert(expr_id.clone(), self.expressions.len());
        self.expressions.push(expr_id);
    }

    pub fn get_expression_pos(&self, expr_id: &ExpressionId) -> Option<usize> {
        self.expression_id_to_pos.get(expr_id).copied()
    }

    #[cfg(test)]
    pub fn get_expression_pos_by_name(&self, name: &str) -> Option<usize> {
        self.expression_name_to_pos.get(name).copied()
    }

    pub fn contains(&self, expr_id: &ExpressionId) -> bool {
        self.expression_id_to_pos.contains_key(expr_id)
    }

    /// Whether the group already maps this output name to a position.
    pub fn contains_name(&self, name: &str) -> bool {
        self.expression_name_to_pos.contains_key(name)
    }
}

impl Default for FactorizationGroup {
    fn default() -> Self {
        Self::new()
    }
}

/// Output schema with factorization structure.
///
/// Tracks flat/unflat groups and which expression belongs to which group.
/// Enforces the invariant that at most one group is unflat at any time.
#[derive(Debug, Clone, Default)]
pub struct FactorizedSchema {
    groups: Vec<FactorizationGroup>,
    expression_to_group: HashMap<ExpressionId, FGroupPos>,
    expression_name_to_group: HashMap<String, FGroupPos>,
    /// Reverse alias lookup: expression id to its registered output name.
    /// Used to carry alias names across schema rebuilds (e.g. sink
    /// operators) and to name expressions in diagnostics. Bare names
    /// registered via `insert_name_for_group` have no entry here.
    expression_id_to_name: HashMap<ExpressionId, String>,
    expressions_in_scope: Vec<ExpressionId>,
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

    pub fn insert_to_scope(&mut self, expr_id: ExpressionId, group_pos: FGroupPos) {
        assert!(
            (group_pos as usize) < self.groups.len(),
            "group_pos {} out of range",
            group_pos
        );
        assert!(
            !self.expression_to_group.contains_key(&expr_id),
            "expression {:?} already in scope",
            expr_id
        );
        self.expression_to_group.insert(expr_id.clone(), group_pos);
        self.expressions_in_scope.push(expr_id);
    }

    pub fn insert_to_scope_with_name(
        &mut self,
        expr_id: ExpressionId,
        name: String,
        group_pos: FGroupPos,
    ) {
        assert!(
            (group_pos as usize) < self.groups.len(),
            "group_pos {} out of range",
            group_pos
        );
        self.expression_name_to_group
            .insert(name.clone(), group_pos);
        self.expression_id_to_name.insert(expr_id.clone(), name);
        self.insert_to_scope(expr_id, group_pos);
    }

    pub fn insert_to_group_and_scope(&mut self, expr_id: ExpressionId, group_pos: FGroupPos) {
        self.insert_to_group_and_scope_with_name(expr_id, None, group_pos);
    }

    pub fn insert_to_group_and_scope_with_name(
        &mut self,
        expr_id: ExpressionId,
        name: Option<String>,
        group_pos: FGroupPos,
    ) {
        assert!(
            (group_pos as usize) < self.groups.len(),
            "group_pos {} out of range",
            group_pos
        );
        let group = &mut self.groups[group_pos as usize];
        group.insert_expression_with_name(expr_id.clone(), name.clone());
        if let Some(n) = name {
            self.expression_name_to_group.insert(n.clone(), group_pos);
            self.expression_id_to_name.insert(expr_id.clone(), n);
        }
        assert!(
            !self.expression_to_group.contains_key(&expr_id),
            "expression {:?} already mapped to group",
            expr_id
        );
        self.expression_to_group.insert(expr_id.clone(), group_pos);
        self.expressions_in_scope.push(expr_id);
    }

    pub fn insert_to_group_and_scope_batch(
        &mut self,
        exprs: Vec<ExpressionId>,
        group_pos: FGroupPos,
    ) {
        for e in exprs {
            self.insert_to_group_and_scope(e, group_pos);
        }
    }

    pub fn insert_to_scope_may_repeat(&mut self, expr_id: ExpressionId, group_pos: FGroupPos) {
        assert!((group_pos as usize) < self.groups.len());
        self.expression_to_group.insert(expr_id.clone(), group_pos);
        if !self.expressions_in_scope.contains(&expr_id) {
            self.expressions_in_scope.push(expr_id);
        }
    }

    pub fn insert_to_group_and_scope_may_repeat(
        &mut self,
        expr_id: ExpressionId,
        group_pos: FGroupPos,
    ) {
        let group = &mut self.groups[group_pos as usize];
        if !group.contains(&expr_id) {
            group.insert_expression(expr_id.clone());
        }
        self.expression_to_group.insert(expr_id.clone(), group_pos);
        if !self.expressions_in_scope.contains(&expr_id) {
            self.expressions_in_scope.push(expr_id);
        }
    }

    pub fn get_group_pos(&self, expr_id: &ExpressionId) -> Option<FGroupPos> {
        self.expression_to_group.get(expr_id).copied()
    }

    #[cfg(test)]
    pub fn get_group_pos_by_name(&self, name: &str) -> Option<FGroupPos> {
        self.get_group_pos_by_name_opt(name)
    }

    pub fn get_group_pos_by_name_opt(&self, name: &str) -> Option<FGroupPos> {
        // Production fallback only for `Expression::Variable`, which carries
        // no `ExpressionId`. All `ExpressionId` resolution uses
        // `get_group_pos` / `is_expression_in_scope` as the single source;
        // unresolved names fall back to flatten-all via
        // `GroupDependencyAnalyzer::mark_unresolved` instead of silent miss.
        self.expression_name_to_group.get(name).copied()
    }

    /// Registered output name for an expression id, if any.
    ///
    /// Bare names from `insert_name_for_group` have no owning id and are
    /// absent here; use `member_names` to enumerate a group's aliases.
    pub fn expression_name(&self, expr_id: &ExpressionId) -> Option<&str> {
        self.expression_id_to_name.get(expr_id).map(String::as_str)
    }

    /// Register a bare alias name for a group.
    ///
    /// Some aliases (such as unwind element names) have no expression
    /// identity of their own; downstream variable references resolve them
    /// by name, so the name mapping must point at the producing group
    /// explicitly.
    pub fn insert_name_for_group(&mut self, name: String, group_pos: FGroupPos) {
        assert!(
            (group_pos as usize) < self.groups.len(),
            "group_pos {} out of range",
            group_pos
        );
        self.expression_name_to_group.insert(name, group_pos);
    }

    /// Alias names currently mapped to a group, in sorted order.
    ///
    /// Used to snapshot the group-to-column mapping onto inserted
    /// `LogicalFlatten` nodes so the executor can validate the plan
    /// position against the runtime layout. Anonymous groups (no tracked
    /// alias) yield an empty list, which means "mapping unknown" rather
    /// than "group is empty".
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

    pub fn flatten_group(&mut self, pos: FGroupPos) {
        let group = self.get_group_mut(pos).expect("flatten_group: invalid pos");
        if !group.is_flat() {
            group.set_flat();
        }
    }

    pub fn flatten_all(&mut self) {
        for i in 0..self.groups.len() {
            let pos = i as FGroupPos;
            if let Some(g) = self.get_group(pos) {
                if !g.is_flat() {
                    self.flatten_group(pos);
                }
            }
        }
    }

    pub fn set_group_as_single_state(&mut self, pos: FGroupPos) {
        let group = self
            .get_group_mut(pos)
            .expect("set_group_as_single_state: invalid pos");
        if !group.is_single_state() {
            group.set_single_state();
        }
    }

    pub fn is_expression_in_scope(&self, expr_id: &ExpressionId) -> bool {
        self.expression_to_group.contains_key(expr_id)
    }

    #[cfg(test)]
    pub fn is_name_in_scope(&self, name: &str) -> bool {
        self.expression_name_to_group.contains_key(name)
    }

    pub fn expressions_in_scope(&self) -> &[ExpressionId] {
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

    pub fn validate_at_most_one_unflat(&self) {
        let unflat = self.groups.iter().filter(|g| !g.is_flat()).count();
        assert!(
            unflat <= 1,
            "at most one unflat group allowed, found {}",
            unflat
        );
    }

    pub fn is_flat_schema(&self) -> bool {
        self.groups.iter().all(|g| g.is_flat())
    }

    /// Flat copy where all groups are flattened.
    pub fn flat_copy(&self) -> Self {
        let mut copy = self.clone();
        copy.flatten_all();
        copy
    }

    /// Merge another schema's groups into this one (for joins etc.).
    /// Returns mapping from old pos to new pos.
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
        mapping
    }

    pub fn expression_to_group_iter(&self) -> impl Iterator<Item = (&ExpressionId, &FGroupPos)> {
        self.expression_to_group.iter()
    }

    pub fn expression_name_to_group_iter(&self) -> impl Iterator<Item = (&String, &FGroupPos)> {
        self.expression_name_to_group.iter()
    }
}

/// Utilities for factorization invariants.
pub struct SchemaUtils;

impl SchemaUtils {
    pub fn get_leading_group_pos(
        group_positions: &HashSet<FGroupPos>,
        schema: &FactorizedSchema,
    ) -> FGroupPos {
        assert!(!group_positions.is_empty(), "groupPositions empty");
        Self::validate_at_most_one_unflat(group_positions, schema);
        for &pos in group_positions {
            if let Some(g) = schema.get_group(pos) {
                if !g.is_flat() {
                    return pos;
                }
            }
        }
        *group_positions.iter().next().expect("non-empty")
    }

    pub fn validate_at_most_one_unflat(
        group_positions: &HashSet<FGroupPos>,
        schema: &FactorizedSchema,
    ) {
        let mut unflat = 0;
        for &pos in group_positions {
            if let Some(g) = schema.get_group(pos) {
                if !g.is_flat() {
                    unflat += 1;
                }
            }
        }
        assert!(
            unflat <= 1,
            "at most one unflat group allowed in set, found {}",
            unflat
        );
    }

    pub fn validate_no_unflat(group_positions: &HashSet<FGroupPos>, schema: &FactorizedSchema) {
        for &pos in group_positions {
            if let Some(g) = schema.get_group(pos) {
                assert!(g.is_flat(), "group {} expected flat but is unflat", pos);
            }
        }
    }
}

/// Rebuilds factorized schemas across sink boundaries.
///
/// A sink operator collects its whole input before producing output, so the
/// output groups no longer share the input's nesting structure. Flat
/// payloads are gathered into one new group (marked single-state when unflat
/// payloads also exist); each contributing unflat input group is copied into
/// its own new group with the cardinality multiplier preserved.
///
/// Mirrors `lbug::planner::SinkOperatorUtil` in
/// `ref/ladybug/src/planner/operator/factorization/sink_util.cpp`.
pub struct SinkOperatorUtil;

impl SinkOperatorUtil {
    /// Merge payload expressions from the input schema into the result
    /// schema, regrouping them as described above. Alias names of merged
    /// expressions (and bare group names of contributing groups) are carried
    /// over so downstream name resolution keeps working.
    pub fn merge_schema(
        input_schema: &FactorizedSchema,
        expressions_to_merge: &[ExpressionId],
        result_schema: &mut FactorizedSchema,
    ) {
        let mut flat_payloads = Vec::new();
        let mut unflat_per_group: HashMap<FGroupPos, Vec<ExpressionId>> = HashMap::new();
        for expr_id in expressions_to_merge {
            let Some(pos) = input_schema.get_group_pos(expr_id) else {
                continue;
            };
            let Some(group) = input_schema.get_group(pos) else {
                continue;
            };
            if group.is_flat() {
                flat_payloads.push(expr_id.clone());
            } else {
                unflat_per_group
                    .entry(pos)
                    .or_default()
                    .push(expr_id.clone());
            }
        }
        let mut unflat_groups: Vec<FGroupPos> = unflat_per_group.keys().copied().collect();
        unflat_groups.sort_unstable();
        // Old group position to rebuilt group position, for alias remapping.
        let mut old_to_new: HashMap<FGroupPos, FGroupPos> = HashMap::new();
        // New position of the single group holding flat payloads, if any.
        let mut flat_new_pos: Option<FGroupPos> = None;
        if unflat_groups.is_empty() {
            if !flat_payloads.is_empty() {
                let new_pos = result_schema.create_group();
                for expr_id in &flat_payloads {
                    result_schema.insert_to_group_and_scope(expr_id.clone(), new_pos);
                }
                flat_new_pos = Some(new_pos);
            }
        } else {
            if !flat_payloads.is_empty() {
                let new_pos = result_schema.create_group();
                for expr_id in &flat_payloads {
                    result_schema.insert_to_group_and_scope(expr_id.clone(), new_pos);
                }
                result_schema.set_group_as_single_state(new_pos);
                flat_new_pos = Some(new_pos);
            }
            for old_pos in unflat_groups {
                let new_pos = result_schema.create_group();
                for expr_id in &unflat_per_group[&old_pos] {
                    result_schema.insert_to_group_and_scope(expr_id.clone(), new_pos);
                }
                if let Some(input_group) = input_schema.get_group(old_pos) {
                    let multiplier = input_group.cardinality_multiplier();
                    if let Some(new_group) = result_schema.get_group_mut(new_pos) {
                        new_group.set_multiplier(multiplier);
                    }
                }
                old_to_new.insert(old_pos, new_pos);
            }
        }
        // Every flat contributor group maps onto the single new flat group.
        if let Some(flat_new_pos) = flat_new_pos {
            let mut flat_old: Vec<FGroupPos> = flat_payloads
                .iter()
                .filter_map(|eid| input_schema.get_group_pos(eid))
                .collect();
            flat_old.sort_unstable();
            flat_old.dedup();
            for old_pos in flat_old {
                old_to_new.entry(old_pos).or_insert(flat_new_pos);
            }
        }
        Self::remap_names(
            input_schema,
            expressions_to_merge,
            &old_to_new,
            result_schema,
        );
        result_schema.validate_at_most_one_unflat();
    }

    /// Clear the result schema, then merge.
    pub fn recompute_schema(
        input_schema: &FactorizedSchema,
        expressions_to_merge: &[ExpressionId],
        result_schema: &mut FactorizedSchema,
    ) {
        result_schema.clear();
        Self::merge_schema(input_schema, expressions_to_merge, result_schema);
    }

    /// Carry alias names of merged expressions (plus bare group names of
    /// contributing groups) onto the rebuilt groups. A name owned by an
    /// expression outside the merge set is left behind so it can never
    /// resolve into a group that does not hold its data.
    fn remap_names(
        input_schema: &FactorizedSchema,
        expressions_to_merge: &[ExpressionId],
        old_to_new: &HashMap<FGroupPos, FGroupPos>,
        result_schema: &mut FactorizedSchema,
    ) {
        let mut owned_names: HashSet<&str> = HashSet::new();
        for expr_id in expressions_to_merge {
            if let Some(name) = input_schema.expression_name(expr_id) {
                owned_names.insert(name);
            }
        }
        let linked_names: HashSet<&str> = input_schema
            .expression_id_to_name
            .values()
            .map(String::as_str)
            .collect();
        let mut names: Vec<(&String, &FGroupPos)> =
            input_schema.expression_name_to_group_iter().collect();
        names.sort_by(|a, b| a.0.cmp(b.0));
        for (name, old_pos) in names {
            let Some(new_pos) = old_to_new.get(old_pos).copied() else {
                continue;
            };
            // An id-linked name is only carried over for its owning merged
            // expression; bare group names follow their contributing group.
            if !owned_names.contains(name.as_str()) && linked_names.contains(name.as_str()) {
                continue;
            }
            if result_schema.get_group_pos_by_name_opt(name).is_none() {
                result_schema.insert_name_for_group(name.clone(), new_pos);
            }
        }
    }
}

/// Trait for operators that can compute factorized schemas.
///
/// `child_schemas` must be the bottom-up computed results for the direct children;
/// passing an empty slice forces recomputation and violates the factorization invariant.
pub trait FactorizedSchemaCompute {
    fn compute_factorized_schema(&mut self, child_schemas: &[FactorizedSchema])
        -> FactorizedSchema;
    fn compute_flat_schema(&mut self, child_schemas: &[FactorizedSchema]) -> FactorizedSchema;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(id: u64) -> ExpressionId {
        ExpressionId::new(id)
    }

    #[test]
    fn group_basic() {
        let mut g = FactorizationGroup::new();
        assert!(!g.is_flat());
        g.set_flat();
        assert!(g.is_flat());
    }

    #[test]
    fn group_single_state_forces_flat() {
        let mut g = FactorizationGroup::new();
        g.set_single_state();
        assert!(g.is_flat());
        assert!(g.is_single_state());
    }

    #[test]
    fn schema_single_flat_group() {
        let mut schema = FactorizedSchema::new();
        let pos = schema.create_flat_group(false);
        assert_eq!(pos, 0);
        schema.insert_to_group_and_scope(expr(1), pos);
        schema.insert_to_group_and_scope(expr(2), pos);
        assert_eq!(schema.num_groups(), 1);
        assert!(schema.get_group(pos).expect("group").is_flat());
        assert_eq!(schema.get_group_pos(&expr(1)), Some(0));
    }

    #[test]
    fn schema_unflat_group_and_flatten() {
        let mut schema = FactorizedSchema::new();
        let flat_pos = schema.create_flat_group(false);
        let unflat_pos = schema.create_group();
        schema.insert_to_group_and_scope(expr(10), flat_pos);
        schema.insert_to_group_and_scope(expr(20), unflat_pos);
        assert!(!schema.get_group(unflat_pos).expect("unflat").is_flat());
        assert!(schema.has_unflat_group());
        assert_eq!(schema.unflat_group_pos(), Some(unflat_pos));
        schema.validate_at_most_one_unflat();
        schema.flatten_group(unflat_pos);
        assert!(schema.is_flat_schema());
        assert!(!schema.has_unflat_group());
    }

    #[test]
    fn schema_at_most_one_unflat_invariant() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_group();
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope(expr(1), g0);
        schema.insert_to_group_and_scope(expr(2), g1);
        // Two unflat groups should panic on validate.
        let result = std::panic::catch_unwind(|| schema.validate_at_most_one_unflat());
        assert!(result.is_err());
    }

    #[test]
    fn schema_copy_and_flat_copy() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope(expr(1), g0);
        schema.insert_to_group_and_scope(expr(2), g1);
        let flat = schema.flat_copy();
        assert!(flat.is_flat_schema());
        assert!(!schema.is_flat_schema());
        let copied = schema.copy();
        assert_eq!(copied.num_groups(), 2);
    }

    #[test]
    fn schema_utils_leading_group() {
        let mut schema = FactorizedSchema::new();
        let flat = schema.create_flat_group(false);
        let unflat = schema.create_group();
        schema.insert_to_group_and_scope(expr(1), flat);
        schema.insert_to_group_and_scope(expr(2), unflat);
        let mut set = HashSet::new();
        set.insert(flat);
        set.insert(unflat);
        let leading = SchemaUtils::get_leading_group_pos(&set, &schema);
        assert_eq!(leading, unflat);
        let mut flat_only = HashSet::new();
        flat_only.insert(flat);
        let leading2 = SchemaUtils::get_leading_group_pos(&flat_only, &schema);
        assert_eq!(leading2, flat);
    }

    #[test]
    fn extend_schema_simulation() {
        // Simulate Scan -> Extend pattern described in docs.
        let mut scan_schema = FactorizedSchema::new();
        let g0 = scan_schema.create_flat_group(false);
        scan_schema.insert_to_group_and_scope(expr(100), g0);
        scan_schema.insert_to_group_and_scope(expr(101), g0);

        // Extend: copy scan schema, flatten bound node group, create new unflat group.
        let mut extend_schema = scan_schema.copy();
        // Suppose bound node 100 is in g0 which is already flat; no op.
        // Create new unflat group for neighbors.
        let g1 = extend_schema.create_group();
        extend_schema.insert_to_group_and_scope(expr(200), g1);
        extend_schema.insert_to_group_and_scope(expr(201), g1);
        assert_eq!(extend_schema.num_groups(), 2);
        assert!(extend_schema.get_group(g0).expect("g0").is_flat());
        assert!(!extend_schema.get_group(g1).expect("g1").is_flat());
        extend_schema.validate_at_most_one_unflat();
    }

    #[test]
    fn member_names_reports_sorted_aliases_per_group() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope(expr(1), g0);
        schema.insert_to_group_and_scope_with_name(expr(2), Some("b".to_string()), g1);
        schema.insert_to_group_and_scope_with_name(expr(3), Some("a".to_string()), g1);
        assert_eq!(
            schema.member_names(g1),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(schema.member_names(g0).is_empty());
        assert!(schema.member_names(99).is_empty());
    }

    #[test]
    fn hash_join_merge_schema() {
        let mut left = FactorizedSchema::new();
        let lg = left.create_flat_group(false);
        left.insert_to_group_and_scope(expr(1), lg);

        let mut right = FactorizedSchema::new();
        let rg = right.create_group();
        right.insert_to_group_and_scope(expr(2), rg);

        let mut merged = left.copy();
        let mapping = merged.merge_groups_from(&right);
        // right group should be remapped to new pos 1
        assert_eq!(mapping.get(&0), Some(&1));
        assert_eq!(merged.num_groups(), 2);
    }

    #[test]
    fn aggregate_flattens_all_but_one() {
        let mut schema = FactorizedSchema::new();
        let g0 = schema.create_flat_group(false);
        let g1 = schema.create_group();
        schema.insert_to_group_and_scope(expr(1), g0);
        schema.insert_to_group_and_scope(expr(2), g1);
        // Aggregate flattens all groups, creates single output group
        let mut agg_schema = FactorizedSchema::new();
        let out = agg_schema.create_flat_group(false);
        agg_schema.insert_to_group_and_scope(expr(10), out);
        assert!(agg_schema.is_flat_schema());
    }

    #[test]
    fn expression_id_to_name_roundtrip() {
        let mut schema = FactorizedSchema::new();
        let g = schema.create_flat_group(false);
        schema.insert_to_group_and_scope_with_name(expr(1), Some("a".to_string()), g);
        assert_eq!(schema.expression_name(&expr(1)), Some("a"));
        assert_eq!(schema.expression_name(&expr(2)), None);
        // Merge carries the reverse map alongside the forward one.
        let mut other = FactorizedSchema::new();
        other.merge_groups_from(&schema);
        other.insert_to_scope_may_repeat(expr(1), 0);
        assert_eq!(other.expression_name(&expr(1)), Some("a"));
        // Clearing scope drops both directions.
        other.clear_expressions_in_scope();
        assert_eq!(other.expression_name(&expr(1)), None);
    }

    #[test]
    fn sink_merge_preserves_multiplier_and_single_state() {
        let mut input = FactorizedSchema::new();
        let flat_pos = input.create_flat_group(false);
        input.insert_to_group_and_scope_with_name(expr(1), Some("a".to_string()), flat_pos);
        let unflat_pos = input.create_group();
        input.insert_to_group_and_scope_with_name(expr(2), Some("b".to_string()), unflat_pos);
        input
            .get_group_mut(unflat_pos)
            .expect("unflat group")
            .set_multiplier(2.5);
        let mut out = FactorizedSchema::new();
        SinkOperatorUtil::recompute_schema(&input, &[expr(1), expr(2)], &mut out);
        out.validate_at_most_one_unflat();
        assert_eq!(out.num_groups(), 2);
        // Flat payload "a" lands in a single-state group.
        let a_pos = out.get_group_pos(&expr(1)).expect("a placed");
        let a_group = out.get_group(a_pos).expect("a group");
        assert!(a_group.is_flat());
        assert!(a_group.is_single_state());
        // Unflat payload "b" keeps its group and multiplier.
        let b_pos = out.get_group_pos(&expr(2)).expect("b placed");
        assert_ne!(a_pos, b_pos);
        let b_group = out.get_group(b_pos).expect("b group");
        assert!(!b_group.is_flat());
        assert_eq!(b_group.cardinality_multiplier(), 2.5);
        // Alias names survive the rebuild for downstream name resolution.
        assert_eq!(out.get_group_pos_by_name_opt("a"), Some(a_pos));
        assert_eq!(out.get_group_pos_by_name_opt("b"), Some(b_pos));
    }

    #[test]
    fn sink_recompute_all_flat_input() {
        let mut input = FactorizedSchema::new();
        let g0 = input.create_flat_group(false);
        input.insert_to_group_and_scope_with_name(expr(1), Some("a".to_string()), g0);
        let g1 = input.create_flat_group(false);
        input.insert_to_group_and_scope_with_name(expr(2), Some("b".to_string()), g1);
        let mut out = FactorizedSchema::new();
        SinkOperatorUtil::recompute_schema(&input, &[expr(1), expr(2)], &mut out);
        // All payloads gather into one new group; names follow.
        assert_eq!(out.num_groups(), 1);
        assert!(out.is_expression_in_scope(&expr(1)));
        assert!(out.is_expression_in_scope(&expr(2)));
        assert!(out.get_group_pos_by_name_opt("a").is_some());
        assert!(out.get_group_pos_by_name_opt("b").is_some());
        out.validate_at_most_one_unflat();
    }

    #[test]
    fn sink_merge_partial_scope_leaves_unmerged_names() {
        let mut input = FactorizedSchema::new();
        let flat_pos = input.create_flat_group(false);
        input.insert_to_group_and_scope_with_name(expr(1), Some("a".to_string()), flat_pos);
        let unflat_pos = input.create_group();
        input.insert_to_group_and_scope_with_name(expr(2), Some("b".to_string()), unflat_pos);
        input.insert_name_for_group("bare".to_string(), unflat_pos);
        let mut out = FactorizedSchema::new();
        SinkOperatorUtil::recompute_schema(&input, &[expr(1)], &mut out);
        // Only the merged payload and its name cross the sink boundary.
        assert!(out.is_expression_in_scope(&expr(1)));
        assert!(!out.is_expression_in_scope(&expr(2)));
        assert!(out.get_group_pos_by_name_opt("a").is_some());
        assert!(out.get_group_pos_by_name_opt("b").is_none());
        assert!(out.get_group_pos_by_name_opt("bare").is_none());
    }
}
