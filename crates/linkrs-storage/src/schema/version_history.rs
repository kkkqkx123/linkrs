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

    /// Render the history as the JSON object used by the schema version API.
    ///
    /// Both the HTTP handler and the embedded API call this projection so a
    /// label's history has one serialized shape across transports.
    pub fn summary_json(&self, space: &str, is_edge: bool) -> serde_json::Value {
        let versions = self
            .change_log
            .get_versions()
            .iter()
            .map(|&version| {
                let version_changes = self
                    .change_log
                    .get_version_changes(version)
                    .cloned()
                    .unwrap_or_default();
                let timestamp_ms = version_changes
                    .iter()
                    .map(|c| c.timestamp_ms)
                    .max()
                    .unwrap_or(0);
                let changes: Vec<serde_json::Value> = version_changes
                    .into_iter()
                    .map(|change| {
                        serde_json::json!({
                            "change_type": format!("{:?}", change.details),
                            "description": change.details.description(),
                            "details": {
                                "description": change.details.description(),
                            },
                        })
                    })
                    .collect();

                serde_json::json!({
                    "version": version,
                    "timestamp_ms": timestamp_ms,
                    "changes": changes,
                })
            })
            .collect::<Vec<_>>();

        serde_json::json!({
            "space": space,
            "label": self.label_name,
            "is_edge": is_edge,
            "versions": versions,
        })
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
