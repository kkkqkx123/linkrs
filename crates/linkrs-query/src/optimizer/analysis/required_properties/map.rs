//! Per-node requirement map produced by the required-property analysis.

use std::collections::HashMap;

use super::requirement::PropertyRequirement;

/// Result of the required-property analysis: one requirement list per plan
/// node id, holding the properties its consumers demand.
#[derive(Debug, Clone, Default)]
pub struct RequiredPropertiesMap {
    requirements: HashMap<i64, Vec<PropertyRequirement>>,
}

impl RequiredPropertiesMap {
    /// Record (replace) the requirement list for a node.
    pub fn record(&mut self, node_id: i64, requirements: &[PropertyRequirement]) {
        self.requirements.insert(node_id, requirements.to_vec());
    }

    /// All requirements recorded for a node.
    pub fn get(&self, node_id: i64) -> Option<&[PropertyRequirement]> {
        self.requirements.get(&node_id).map(Vec::as_slice)
    }

    /// The merged requirement for `var` at `node_id`, if any.
    pub fn requirement_for_var(&self, node_id: i64, var: &str) -> Option<PropertyRequirement> {
        let mut merged: Option<PropertyRequirement> = None;
        for req in self.requirements.get(&node_id)? {
            if req.alias == var {
                match merged.as_mut() {
                    Some(acc) => acc.merge(req),
                    None => merged = Some(req.clone()),
                }
            }
        }
        merged
    }

    /// Properties safe to narrow `node_id`'s source to for `var`.
    ///
    /// Returns `Some(sorted props)` only when the requirement is narrowable:
    /// the variable is not consumed in a full-value position and at least
    /// one property is provably needed. An empty or full-value requirement
    /// yields `None`, so callers keep the full read (empty
    /// `projected_properties` means "read everything").
    pub fn narrowable_properties(&self, node_id: i64, var: &str) -> Option<Vec<String>> {
        let req = self.requirement_for_var(node_id, var)?;
        if !req.is_narrowable() {
            return None;
        }
        Some(req.prop_names.into_iter().collect())
    }
}
