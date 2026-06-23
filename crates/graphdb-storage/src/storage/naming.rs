//! Name to Index Mapper
//!
//! Provides a reusable utility for mapping string names to numeric indices (PropertyId).
//! This replaces the duplicated `name_to_index: HashMap<String, usize>` pattern
//! found in property tables and schema-related storage code.
//!
//! Features:
//! - O(1) name-to-index lookup
//! - Supports adding new mappings dynamically
//! - Memory-efficient storage

use std::collections::HashMap;

use crate::storage::types::PropertyId;

/// Maps string names to PropertyId.
#[derive(Debug, Clone)]
pub struct NameIndexer {
    name_to_id: HashMap<String, PropertyId>,
    next_id: u16,
}

impl NameIndexer {
    pub fn new() -> Self {
        Self {
            name_to_id: HashMap::new(),
            next_id: 0,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            name_to_id: HashMap::with_capacity(capacity),
            next_id: 0,
        }
    }

    /// Register a new name and return its PropertyId.
    /// Returns the existing PropertyId if the name is already registered.
    pub fn register(&mut self, name: String) -> PropertyId {
        if let Some(&id) = self.name_to_id.get(&name) {
            return id;
        }

        let id = PropertyId::new(self.next_id);
        self.next_id = self.next_id.checked_add(1).expect("property id overflow");
        self.name_to_id.insert(name, id);

        id
    }

    /// Look up the PropertyId for a given name.
    #[inline]
    pub fn get_id(&self, name: &str) -> Option<PropertyId> {
        self.name_to_id.get(name).copied()
    }

    /// Check if a name is registered.
    #[inline]
    pub fn contains(&self, name: &str) -> bool {
        self.name_to_id.contains_key(name)
    }

    /// Clear all registered names.
    pub fn clear(&mut self) {
        self.name_to_id.clear();
        self.next_id = 0;
    }
}

impl Default for NameIndexer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_lookup() {
        let mut indexer = NameIndexer::new();

        let id1 = indexer.register("weight".to_string());
        let id2 = indexer.register("since".to_string());

        assert_eq!(id1.as_u16(), 0);
        assert_eq!(id2.as_u16(), 1);

        assert_eq!(indexer.get_id("weight"), Some(id1));
        assert_eq!(indexer.get_id("since"), Some(id2));
    }

    #[test]
    fn test_duplicate_register() {
        let mut indexer = NameIndexer::new();

        let id1 = indexer.register("weight".to_string());
        let id2 = indexer.register("weight".to_string());

        assert_eq!(id1, id2);
    }

    #[test]
    fn test_nonexistent_name() {
        let indexer = NameIndexer::new();

        assert_eq!(indexer.get_id("nonexistent"), None);
        assert!(!indexer.contains("nonexistent"));
    }

    #[test]
    fn test_clear() {
        let mut indexer = NameIndexer::new();

        indexer.register("weight".to_string());
        indexer.register("since".to_string());

        indexer.clear();

        assert_eq!(indexer.get_id("weight"), None);
        assert_eq!(indexer.get_id("since"), None);
    }
}
