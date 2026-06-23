//! Scan and Search Strategy
//!
//! The “full table scan” strategy serves as a backup option when indexes cannot be used.

use super::seek_strategy::SeekStrategy;
use super::seek_strategy_base::{NodePattern, SeekResult, SeekStrategyContext, SeekStrategyType};
use crate::core::{StorageError, Value, Vertex};
use crate::storage::StorageReader;

#[derive(Debug, Clone)]
pub struct ScanSeek {
    any_label: bool,
}

impl Default for ScanSeek {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanSeek {
    pub fn new() -> Self {
        Self { any_label: false }
    }

    pub fn with_any_label(mut self, any_label: bool) -> Self {
        self.any_label = any_label;
        self
    }
}

impl SeekStrategy for ScanSeek {
    fn execute<S: StorageReader>(
        &self,
        storage: &S,
        context: &SeekStrategyContext,
    ) -> Result<SeekResult, StorageError> {
        if self.any_label {
            self.scan_all_labels(storage, context)
        } else {
            self.scan_specific_labels(storage, context)
        }
    }

    fn supports(&self, _context: &SeekStrategyContext) -> bool {
        true
    }
}

impl ScanSeek {
    fn scan_all_labels<S: StorageReader>(
        &self,
        storage: &S,
        context: &SeekStrategyContext,
    ) -> Result<SeekResult, StorageError> {
        let all_tags = storage.list_tags("default")?;

        let mut vertex_ids = Vec::new();
        let mut rows_scanned = 0;

        for tag in all_tags {
            let vertices = storage.scan_vertices_by_tag("default", &tag.tag_name)?;
            for vertex in vertices {
                rows_scanned += 1;
                if self.vertex_matches_pattern(&vertex, &context.node_pattern, true) {
                    vertex_ids.push(Value::from(*vertex.vid()));
                }
            }
        }

        Ok(SeekResult {
            vertex_ids,
            strategy_used: SeekStrategyType::ScanSeek,
            rows_scanned,
        })
    }

    fn scan_specific_labels<S: StorageReader>(
        &self,
        storage: &S,
        context: &SeekStrategyContext,
    ) -> Result<SeekResult, StorageError> {
        let vertices = storage.scan_vertices("default")?;
        let mut vertex_ids = Vec::new();
        let mut rows_scanned = 0;

        for vertex in vertices {
            rows_scanned += 1;
            if self.vertex_matches_pattern(&vertex, &context.node_pattern, false) {
                vertex_ids.push(Value::from(*vertex.vid()));
            }
        }

        Ok(SeekResult {
            vertex_ids,
            strategy_used: SeekStrategyType::ScanSeek,
            rows_scanned,
        })
    }

    fn vertex_matches_pattern(
        &self,
        vertex: &Vertex,
        pattern: &NodePattern,
        any_label: bool,
    ) -> bool {
        if !pattern.labels.is_empty() {
            let has_all_labels = pattern
                .labels
                .iter()
                .all(|label| vertex.tags.iter().any(|tag| tag.name == *label));
            if !has_all_labels {
                return false;
            }
        } else if !any_label && vertex.tags.is_empty() {
            return false;
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
    use super::*;

    #[test]
    fn test_scan_seek_new() {
        let _seek = ScanSeek::new();
        // The test has been successful; reaching this point indicates that the goal has been achieved.
    }

    #[test]
    fn test_scan_seek_supports_always() {
        let seek = ScanSeek::new();
        let context = SeekStrategyContext::new(
            1,
            NodePattern {
                vid: None,
                labels: vec![],
                properties: vec![],
            },
            vec![],
        );
        assert!(seek.supports(&context));
    }
}
