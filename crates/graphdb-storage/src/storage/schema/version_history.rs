//! Version history tracking for schema migrations
//!
//! Maintains a complete history of schema versions, including change logs
//! for each label.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::core::StorageResult;
use super::change::{ChangeLog, PropertyChange, SchemaObjectType};

/// Version history for a single label (vertex or edge type)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelVersionHistory {
    /// Label ID
    pub label_id: u32,
    /// Label name
    pub label_name: String,
    /// Object type (vertex or edge)
    pub object_type: SchemaObjectType,
    /// Change log for this label
    pub change_log: ChangeLog,
}

impl LabelVersionHistory {
    /// Create a new version history
    pub fn new(label_id: u32, label_name: String, object_type: SchemaObjectType) -> Self {
        Self {
            label_id,
            label_name: label_name.clone(),
            object_type,
            change_log: ChangeLog::new(object_type, label_id, label_name),
        }
    }

    /// Add a change to the history
    pub fn add_change(&mut self, change: PropertyChange) {
        self.change_log.add_change(change);
    }

    /// Get the latest version
    pub fn latest_version(&self) -> u64 {
        self.change_log.latest_version().unwrap_or(1)
    }

    /// Check if a migration path exists (no breaking changes between versions)
    pub fn can_migrate(&self, from_version: u64, to_version: u64) -> bool {
        if from_version >= to_version {
            return true; // No forward migration needed
        }

        // Check if there are breaking changes between versions
        let breaking_between = self
            .change_log
            .changes
            .iter()
            .filter(|(v, _)| **v > from_version && **v <= to_version)
            .any(|(_, changes)| changes.iter().any(|c| c.is_breaking()));

        !breaking_between
    }

    /// Get all versions in order
    pub fn get_versions(&self) -> Vec<u64> {
        self.change_log.get_versions()
    }

    /// Get breaking changes between two versions
    pub fn get_breaking_changes(&self, from_version: u64, to_version: u64) -> Vec<PropertyChange> {
        self.change_log
            .changes
            .iter()
            .filter(|(v, _)| **v > from_version && **v <= to_version)
            .flat_map(|(_, changes)| {
                changes
                    .iter()
                    .filter(|c| c.is_breaking())
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

/// Complete schema version history for all labels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaVersionHistory {
    /// Vertex label histories
    pub vertex_histories: HashMap<u32, LabelVersionHistory>,
    /// Edge label histories
    pub edge_histories: HashMap<u32, LabelVersionHistory>,
}

impl SchemaVersionHistory {
    /// Create a new schema history
    pub fn new() -> Self {
        Self {
            vertex_histories: HashMap::new(),
            edge_histories: HashMap::new(),
        }
    }

    /// Add or update a vertex label history
    pub fn add_vertex_history(&mut self, history: LabelVersionHistory) {
        self.vertex_histories.insert(history.label_id, history);
    }

    /// Add or update an edge label history
    pub fn add_edge_history(&mut self, history: LabelVersionHistory) {
        self.edge_histories.insert(history.label_id, history);
    }

    /// Get vertex history
    pub fn get_vertex_history(&self, label_id: u32) -> Option<&LabelVersionHistory> {
        self.vertex_histories.get(&label_id)
    }

    /// Get vertex history (mutable)
    pub fn get_vertex_history_mut(&mut self, label_id: u32) -> Option<&mut LabelVersionHistory> {
        self.vertex_histories.get_mut(&label_id)
    }

    /// Get edge history
    pub fn get_edge_history(&self, label_id: u32) -> Option<&LabelVersionHistory> {
        self.edge_histories.get(&label_id)
    }

    /// Get edge history (mutable)
    pub fn get_edge_history_mut(&mut self, label_id: u32) -> Option<&mut LabelVersionHistory> {
        self.edge_histories.get_mut(&label_id)
    }

    /// Get or create vertex history
    pub fn get_or_create_vertex_history(
        &mut self,
        label_id: u32,
        label_name: String,
    ) -> &mut LabelVersionHistory {
        self.vertex_histories
            .entry(label_id)
            .or_insert_with(|| {
                LabelVersionHistory::new(label_id, label_name, SchemaObjectType::Vertex)
            })
    }

    /// Get or create edge history
    pub fn get_or_create_edge_history(
        &mut self,
        label_id: u32,
        label_name: String,
    ) -> &mut LabelVersionHistory {
        self.edge_histories
            .entry(label_id)
            .or_insert_with(|| {
                LabelVersionHistory::new(label_id, label_name, SchemaObjectType::Edge)
            })
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> StorageResult<String> {
        serde_json::to_string(self)
            .map_err(|e| crate::core::StorageError::serialize_error(e.to_string()))
    }

    /// Deserialize from JSON
    pub fn from_json(json: &str) -> StorageResult<Self> {
        serde_json::from_str(json)
            .map_err(|e| crate::core::StorageError::deserialize_error(e.to_string()))
    }
}

impl Default for SchemaVersionHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::DataType;

    #[test]
    fn test_label_version_history_creation() {
        let history =
            LabelVersionHistory::new(1, "User".to_string(), SchemaObjectType::Vertex);
        assert_eq!(history.label_id, 1);
        assert_eq!(history.label_name, "User");
        assert_eq!(history.latest_version(), 1);
    }

    #[test]
    fn test_schema_version_history() {
        let mut schema_history = SchemaVersionHistory::new();
        let vertex_history =
            LabelVersionHistory::new(1, "User".to_string(), SchemaObjectType::Vertex);

        schema_history.add_vertex_history(vertex_history);
        assert!(schema_history.get_vertex_history(1).is_some());
    }

    #[test]
    fn test_can_migrate() {
        let mut history =
            LabelVersionHistory::new(1, "User".to_string(), SchemaObjectType::Vertex);

        // No breaking changes, migration should be allowed
        assert!(history.can_migrate(1, 2));
    }
}
