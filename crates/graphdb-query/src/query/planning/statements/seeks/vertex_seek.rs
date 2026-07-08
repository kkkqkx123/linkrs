//! Vertex search strategy
//!
//! Direct search strategy based on vertex IDs

use super::seek_strategy::SeekStrategy;
use super::seek_strategy_base::{NodePattern, SeekResult, SeekStrategyContext, SeekStrategyType};
use crate::core::types::VertexId;
use crate::core::{StorageError, Value, Vertex};
use crate::storage::StorageReader;

#[derive(Debug, Clone)]
pub struct VertexSeek;

impl Default for VertexSeek {
    fn default() -> Self {
        Self::new()
    }
}

impl VertexSeek {
    pub fn new() -> Self {
        Self
    }
}

impl SeekStrategy for VertexSeek {
    fn execute<S: StorageReader>(
        &self,
        storage: &S,
        context: &SeekStrategyContext,
    ) -> Result<SeekResult, StorageError> {
        let mut vertex_ids = Vec::new();
        let mut rows_scanned = 0;

        if let Some(ref vid) = context.node_pattern.vid {
            let vid =
                VertexId::try_from(vid).map_err(|e| StorageError::invalid_input(e.to_string()))?;
            if let Some(vertex) = storage.get_vertex("default", &vid)? {
                rows_scanned = 1;
                if self.vertex_matches_pattern(&vertex, &context.node_pattern) {
                    vertex_ids.push(Value::from(*vertex.vid()));
                }
            }
        } else {
            for vid_val in &self.resolve_vertex_ids(context)? {
                let vid = VertexId::try_from(vid_val)
                    .map_err(|e| StorageError::invalid_input(e.to_string()))?;
                if let Some(vertex) = storage.get_vertex("default", &vid)? {
                    rows_scanned += 1;
                    if self.vertex_matches_pattern(&vertex, &context.node_pattern) {
                        vertex_ids.push(vid_val.clone());
                    }
                }
            }
        }

        Ok(SeekResult {
            vertex_ids,
            strategy_used: SeekStrategyType::VertexSeek,
            rows_scanned,
        })
    }

    fn supports(&self, context: &SeekStrategyContext) -> bool {
        context.has_explicit_vid()
            || (!context.node_pattern.labels.is_empty() && context.estimated_rows < 1000)
    }
}

impl VertexSeek {
    fn resolve_vertex_ids(
        &self,
        context: &SeekStrategyContext,
    ) -> Result<Vec<Value>, StorageError> {
        let mut ids = Vec::new();

        for pred in &context.predicates {
            if let Some(vid) = self.extract_vid_from_predicate(pred) {
                ids.push(vid);
            }
        }

        if ids.is_empty() && context.node_pattern.vid.is_some() {
            if let Some(vid) = context.node_pattern.vid.as_ref() {
                ids.push(vid.clone());
            }
        }

        Ok(ids)
    }

    fn extract_vid_from_predicate(
        &self,
        predicate: &crate::core::types::expr::Expression,
    ) -> Option<Value> {
        use crate::core::types::expr::Expression;

        match predicate {
            Expression::Literal(value) => Some(value.clone()),
            Expression::Variable(name) => Some(Value::String(name.clone())),
            Expression::Property { object, property } => {
                if let Expression::Variable(var_name) = object.as_ref() {
                    Some(Value::String(format!("{}.{}", var_name, property)))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

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
    use super::*;

    #[test]
    fn test_vertex_seek_new() {
        let _ = VertexSeek::new();
        // If the test is successful and you have reached this point, it means that everything has gone well.
    }

    #[test]
    fn test_vertex_seek_supports_explicit_vid() {
        let seek = VertexSeek::new();
        let context = SeekStrategyContext::new(
            1,
            NodePattern {
                vid: Some(Value::String("test_vid".to_string())),
                labels: vec![],
                properties: vec![],
            },
            vec![],
        );
        assert!(seek.supports(&context));
    }

    #[test]
    fn test_vertex_seek_supports_labeled() {
        let seek = VertexSeek::new();
        let context = SeekStrategyContext::new(
            1,
            NodePattern {
                vid: None,
                labels: vec!["person".to_string()],
                properties: vec![],
            },
            vec![],
        )
        .with_estimated_rows(100);
        assert!(seek.supports(&context));
    }
}
