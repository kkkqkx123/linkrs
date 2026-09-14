//! Explicit projection set for graph scan nodes.
//!
//! Replaces the bare `Vec<String>` convention where an empty vector means
//! "read all properties" (see `RequiredPropertiesMap::narrowable_properties`
//! and the `projected_properties` fields of the graph scan nodes). New code
//! should take or return [`ProjectionSet`]; existing `Vec<String>` call sites
//! migrate incrementally via [`ProjectionSet::from_vec`] / [`ProjectionSet::into_vec`].

/// Property projection of a graph scan: all properties or an explicit subset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionSet {
    /// Read all properties (previously `vec![]`).
    All,
    /// Read exactly these properties.
    Some(Vec<String>),
}

impl ProjectionSet {
    /// Build from the legacy representation (`vec![]` means [`ProjectionSet::All`]).
    pub fn from_vec(properties: Vec<String>) -> Self {
        if properties.is_empty() {
            Self::All
        } else {
            Self::Some(properties)
        }
    }

    /// Convert back to the legacy representation (`All` becomes `vec![]`).
    pub fn into_vec(self) -> Vec<String> {
        match self {
            Self::All => Vec::new(),
            Self::Some(properties) => properties,
        }
    }

    /// Whether this set reads all properties.
    pub fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }
}

impl From<Vec<String>> for ProjectionSet {
    fn from(properties: Vec<String>) -> Self {
        Self::from_vec(properties)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_vec_means_all() {
        assert_eq!(ProjectionSet::from_vec(Vec::new()), ProjectionSet::All);
        assert!(ProjectionSet::from_vec(Vec::new()).is_all());
    }

    #[test]
    fn test_round_trip() {
        let set = ProjectionSet::from_vec(vec!["age".to_string()]);
        assert!(!set.is_all());
        assert_eq!(set.into_vec(), vec!["age".to_string()]);
        assert!(ProjectionSet::All.into_vec().is_empty());
    }
}
