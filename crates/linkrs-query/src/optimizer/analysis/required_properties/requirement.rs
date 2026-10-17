//! Property requirement for a single variable binding.

use std::collections::BTreeSet;

/// Property requirement for one variable binding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PropertyRequirement {
    /// Variable (alias) the properties belong to.
    pub alias: String,
    /// Resolved tag / edge type when the binding is single-typed.
    pub tag_name: Option<String>,
    /// Property names demanded of the binding.
    pub prop_names: BTreeSet<String>,
    /// The variable is consumed in a non-prunable position (bare reference,
    /// opaque object, function argument, ...): its whole value is needed,
    /// so the binding must not be narrowed. Sticky: once set it is never
    /// cleared by later property references.
    pub full_value: bool,
}

impl PropertyRequirement {
    pub fn new(alias: impl Into<String>) -> Self {
        Self {
            alias: alias.into(),
            ..Default::default()
        }
    }

    /// Merge another requirement (same alias) into this one.
    pub fn merge(&mut self, other: &PropertyRequirement) {
        self.tag_name = other.tag_name.clone().or_else(|| self.tag_name.clone());
        self.prop_names.extend(other.prop_names.iter().cloned());
        self.full_value |= other.full_value;
    }

    /// Whether the binding is narrowable at all.
    pub fn is_narrowable(&self) -> bool {
        !self.full_value && !self.prop_names.is_empty()
    }
}
