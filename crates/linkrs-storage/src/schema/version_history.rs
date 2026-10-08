//! Version history tracking for schema migrations
//!
//! Maintains a complete history of schema versions, including change logs
//! for each label.

use super::change::{ChangeLog, PropertyChange, SchemaObjectType};
use serde::{Deserialize, Serialize};

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

    /// Get all versions in order
    pub fn get_versions(&self) -> Vec<u64> {
        self.change_log.get_versions()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_label_version_history_creation() {
        let history = LabelVersionHistory::new(1, "User".to_string(), SchemaObjectType::Vertex);
        assert_eq!(history.label_id, 1);
        assert_eq!(history.label_name, "User");
        assert_eq!(history.latest_version(), 1);
    }
}
