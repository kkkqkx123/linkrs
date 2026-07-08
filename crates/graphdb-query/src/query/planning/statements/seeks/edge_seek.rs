//! Edge search strategy
//!
//! A search strategy that starts from the edge mode, used for MATCH operations where the search begins from an edge.
//!
//! Applicable scenarios:
//! - MATCH ()-[e:KNOWS]->() WHERE e.since > 2020
//! - MATCH (a)-[e]->(b) WHERE e.weight > 5
//! - Start the search from the edge index.

use super::seek_strategy::SeekStrategy;
use super::seek_strategy_base::{SeekResult, SeekStrategyContext, SeekStrategyType};
use crate::core::{StorageError, Value};
use crate::storage::StorageReader;

/// Edge pattern information
#[derive(Debug, Clone, PartialEq)]
pub struct EdgePattern {
    pub edge_types: Vec<String>,
    pub direction: EdgeDirection,
    pub src_vid: Option<Value>,
    pub dst_vid: Option<Value>,
    pub properties: Vec<(String, Value)>,
}

/// edgewise
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeDirection {
    Outgoing, // ->
    Incoming, // <-
    Both,     // -
}

impl EdgeDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeDirection::Outgoing => "OUT",
            EdgeDirection::Incoming => "IN",
            EdgeDirection::Both => "BOTH",
        }
    }
}

impl std::str::FromStr for EdgeDirection {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "OUT" | "OUTGOING" | "->" => Ok(EdgeDirection::Outgoing),
            "IN" | "INCOMING" | "<-" => Ok(EdgeDirection::Incoming),
            "BOTH" | "-" => Ok(EdgeDirection::Both),
            _ => Err(format!("Invalid edge direction: {}", s)),
        }
    }
}

/// Edge search strategy
#[derive(Debug, Clone)]
pub struct EdgeSeek {
    pub edge_pattern: EdgePattern,
}

impl EdgeSeek {
    pub fn new(edge_pattern: EdgePattern) -> Self {
        Self { edge_pattern }
    }

    /// Evaluating whether a border matches a pattern
    fn edge_matches_pattern(&self, edge: &crate::core::Edge) -> bool {
        // Check edge type
        if !self.edge_pattern.edge_types.is_empty()
            && !self.edge_pattern.edge_types.contains(&edge.edge_type)
        {
            return false;
        }

        if let Some(ref src_vid) = self.edge_pattern.src_vid {
            if Value::from(edge.src) != *src_vid {
                return false;
            }
        }

        if let Some(ref dst_vid) = self.edge_pattern.dst_vid {
            if Value::from(edge.dst) != *dst_vid {
                return false;
            }
        }

        // Check properties
        for (prop_name, prop_value) in &self.edge_pattern.properties {
            let found = edge
                .get_property(prop_name)
                .map(|v| v == prop_value)
                .unwrap_or(false);
            if !found {
                return false;
            }
        }

        true
    }
}

impl SeekStrategy for EdgeSeek {
    fn execute<S: StorageReader>(
        &self,
        storage: &S,
        _context: &SeekStrategyContext,
    ) -> Result<SeekResult, StorageError> {
        let mut edge_ids = Vec::new();
        let mut rows_scanned = 0;

        let space_name = "default";

        // Strategy 1: Use edge type scan if edge types are specified
        if !self.edge_pattern.edge_types.is_empty() {
            for edge_type in &self.edge_pattern.edge_types {
                let edges = storage.scan_edges_by_type(space_name, edge_type)?;
                rows_scanned += edges.len();

                for edge in edges {
                    let edge_id = format!("{}->{}@{}", edge.src, edge.dst, edge.edge_type);
                    edge_ids.push(Value::String(edge_id));
                }
            }
        } else {
            // Strategy 2: Scan all edges in the table
            let edges = storage.scan_all_edges(space_name)?;
            rows_scanned = edges.len();

            for edge in edges {
                if self.edge_matches_pattern(&edge) {
                    let edge_id = format!("{}->{}@{}", edge.src, edge.dst, edge.edge_type);
                    edge_ids.push(Value::String(edge_id));
                }
            }
        }

        // Remove duplicates
        edge_ids.sort();
        edge_ids.dedup();

        Ok(SeekResult {
            vertex_ids: edge_ids,
            strategy_used: SeekStrategyType::EdgeSeek,
            rows_scanned,
        })
    }

    fn supports(&self, _context: &SeekStrategyContext) -> bool {
        // Supported as long as there is a side mode
        true
    }
}

/// Expand the search results as needed.
#[derive(Debug)]
pub struct EdgeSeekResult {
    pub base: SeekResult,
    pub start_vids: Vec<Value>,
    pub end_vids: Vec<Value>,
}

impl EdgeSeekResult {
    pub fn from_seek_result(
        result: SeekResult,
        start_vids: Vec<Value>,
        end_vids: Vec<Value>,
    ) -> Self {
        Self {
            base: result,
            start_vids,
            end_vids,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::VertexId;

    #[test]
    fn test_edge_direction_from_str() {
        assert_eq!(
            "OUT".parse::<EdgeDirection>().ok(),
            Some(EdgeDirection::Outgoing)
        );
        assert_eq!(
            "->".parse::<EdgeDirection>().ok(),
            Some(EdgeDirection::Outgoing)
        );
        assert_eq!(
            "IN".parse::<EdgeDirection>().ok(),
            Some(EdgeDirection::Incoming)
        );
        assert_eq!(
            "<-".parse::<EdgeDirection>().ok(),
            Some(EdgeDirection::Incoming)
        );
        assert_eq!(
            "BOTH".parse::<EdgeDirection>().ok(),
            Some(EdgeDirection::Both)
        );
        assert_eq!("-".parse::<EdgeDirection>().ok(), Some(EdgeDirection::Both));
        assert!("unknown".parse::<EdgeDirection>().is_err());
    }

    #[test]
    fn test_edge_pattern_matching() {
        let seek = EdgeSeek::new(EdgePattern {
            edge_types: vec!["KNOWS".to_string()],
            direction: EdgeDirection::Outgoing,
            src_vid: None,
            dst_vid: None,
            properties: vec![],
        });

        // Test edge type matching
        let edge = crate::core::Edge::new_empty(
            VertexId::from_int64(1),
            VertexId::from_int64(2),
            "KNOWS".to_string(),
            0,
        );
        assert!(seek.edge_matches_pattern(&edge));

        // Test edge type mismatch
        let edge2 = crate::core::Edge::new_empty(
            VertexId::from_int64(1),
            VertexId::from_int64(2),
            "FOLLOWS".to_string(),
            0,
        );
        assert!(!seek.edge_matches_pattern(&edge2));
    }

    #[test]
    fn test_edge_pattern_with_src_vid() {
        let seek = EdgeSeek::new(EdgePattern {
            edge_types: vec![],
            direction: EdgeDirection::Outgoing,
            src_vid: Some(Value::Int(1)),
            dst_vid: None,
            properties: vec![],
        });

        let edge = crate::core::Edge::new_empty(
            VertexId::from_int64(1),
            VertexId::from_int64(2),
            "KNOWS".to_string(),
            0,
        );
        assert!(seek.edge_matches_pattern(&edge));

        let edge2 = crate::core::Edge::new_empty(
            VertexId::from_int64(3),
            VertexId::from_int64(2),
            "KNOWS".to_string(),
            0,
        );
        assert!(!seek.edge_matches_pattern(&edge2));
    }
}
