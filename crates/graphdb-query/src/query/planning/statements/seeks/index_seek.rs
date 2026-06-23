//! Index lookup strategy
//!
//! Use tag or attribute indexes for efficient searching.

use super::seek_strategy::SeekStrategy;
use super::seek_strategy_base::{NodePattern, SeekResult, SeekStrategyContext, SeekStrategyType};
use crate::core::{StorageError, Value, Vertex};
use crate::storage::StorageReader;

#[derive(Debug, Clone)]
pub struct IndexSeek;

impl Default for IndexSeek {
    fn default() -> Self {
        Self::new()
    }
}

impl IndexSeek {
    pub fn new() -> Self {
        Self
    }
}

impl SeekStrategy for IndexSeek {
    fn execute<S: StorageReader>(
        &self,
        storage: &S,
        context: &SeekStrategyContext,
    ) -> Result<SeekResult, StorageError> {
        let mut vertex_ids = Vec::new();
        let mut rows_scanned = 0;

        if let Some(index_info) = context.get_index_for_labels(&context.node_pattern.labels) {
            let vertices = storage.scan_vertices_by_tag("default", &index_info.target_name)?;
            rows_scanned = vertices.len();
            for vertex in vertices {
                if self.vertex_matches_pattern(&vertex, &context.node_pattern) {
                    vertex_ids.push(Value::from(*vertex.vid()));
                }
            }
        }

        if vertex_ids.is_empty() {
            rows_scanned = 0;
        }

        Ok(SeekResult {
            vertex_ids,
            strategy_used: SeekStrategyType::IndexSeek,
            rows_scanned,
        })
    }

    fn supports(&self, context: &SeekStrategyContext) -> bool {
        context
            .get_index_for_labels(&context.node_pattern.labels)
            .is_some()
    }
}

impl IndexSeek {
    fn vertex_matches_pattern(&self, vertex: &Vertex, pattern: &NodePattern) -> bool {
        if !pattern.labels.is_empty() {
            let has_all_labels = pattern
                .labels
                .iter()
                .all(|label| vertex.tags.iter().any(|tag| tag.name == *label));
            if !has_all_labels {
                return false;
            }
        }

        for (prop_name, prop_value) in &pattern.properties {
            let found = vertex
                .get_all_properties()
                .iter()
                .any(|(name, value)| name == prop_name && **value == *prop_value);
            if !found {
                return false;
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::seek_strategy_base::IndexInfo;
    use super::*;

    #[test]
    fn test_index_seek_new() {
        let _ = IndexSeek::new();
        // If the test is successful and you have reached this point, it means that everything has gone well.
    }

    #[test]
    fn test_index_seek_supports_with_index() {
        let seek = IndexSeek::new();
        let context = SeekStrategyContext::new(
            1,
            NodePattern {
                vid: None,
                labels: vec!["person".to_string()],
                properties: vec![],
            },
            vec![],
        )
        .with_indexes(vec![IndexInfo::new(
            "idx_person_name".to_string(),
            "tag".to_string(),
            "person".to_string(),
            vec!["name".to_string()],
        )]);
        assert!(seek.supports(&context));
    }

    #[test]
    fn test_index_seek_supports_without_index() {
        let seek = IndexSeek::new();
        let context = SeekStrategyContext::new(
            1,
            NodePattern {
                vid: None,
                labels: vec!["person".to_string()],
                properties: vec![],
            },
            vec![],
        );
        assert!(!seek.supports(&context));
    }
}
