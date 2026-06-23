//! Transaction Conflict Detection
//!
//! Provides conflict detection mechanisms for write transactions using write sets.

use super::types::WriteSet;

/// Check if two write sets have any conflicts
pub fn have_write_conflict(ws1: &WriteSet, ws2: &WriteSet) -> bool {
    ws1.has_conflict_with(ws2)
}

/// Conflict detection analyzer for transactions
pub struct WriteSetAnalyzer;

impl WriteSetAnalyzer {
    /// Analyze conflict intensity between two write sets
    ///
    /// Returns a score from 0.0 to 1.0 indicating how likely conflicts are:
    /// - 0.0: No conflict
    /// - 0.5: Medium conflict probability
    /// - 1.0: Definite conflict
    pub fn conflict_intensity(ws1: &WriteSet, ws2: &WriteSet) -> f64 {
        if !have_write_conflict(ws1, ws2) {
            return 0.0;
        }

        // Calculate intensity based on overlap size
        let vertex_overlap = ws1
            .vertices
            .intersection(&ws2.vertices)
            .count();
        let edge_overlap = ws1
            .edges
            .intersection(&ws2.edges)
            .count();

        let total_entities = ws1.size() + ws2.size();
        if total_entities == 0 {
            return 0.0;
        }

        let overlap_count = vertex_overlap + edge_overlap;
        (overlap_count as f64) / (total_entities as f64)
    }

    /// Check if conflict is due to vertex modification
    pub fn conflicts_on_vertex(ws1: &WriteSet, ws2: &WriteSet) -> bool {
        !ws1.vertices.is_disjoint(&ws2.vertices)
    }

    /// Check if conflict is due to edge modification
    pub fn conflicts_on_edge(ws1: &WriteSet, ws2: &WriteSet) -> bool {
        !ws1.edges.is_disjoint(&ws2.edges)
    }

    /// Check if conflict is due to shared vertex endpoints
    pub fn conflicts_on_shared_vertex(ws1: &WriteSet, ws2: &WriteSet) -> bool {
        for edge1 in &ws1.edges {
            for edge2 in &ws2.edges {
                if edge1.src_vid == edge2.src_vid || edge1.dst_vid == edge2.dst_vid {
                    return true;
                }
            }
        }
        false
    }

    /// Get a detailed conflict report
    pub fn analyze_conflict(ws1: &WriteSet, ws2: &WriteSet) -> ConflictReport {
        ConflictReport {
            has_conflict: have_write_conflict(ws1, ws2),
            vertex_conflict: Self::conflicts_on_vertex(ws1, ws2),
            edge_conflict: Self::conflicts_on_edge(ws1, ws2),
            shared_vertex_conflict: Self::conflicts_on_shared_vertex(ws1, ws2),
            intensity: Self::conflict_intensity(ws1, ws2),
        }
    }
}

/// Detailed conflict analysis report
#[derive(Debug, Clone)]
pub struct ConflictReport {
    /// Whether there is any conflict
    pub has_conflict: bool,
    /// Whether conflict is due to vertex modification
    pub vertex_conflict: bool,
    /// Whether conflict is due to edge modification
    pub edge_conflict: bool,
    /// Whether conflict is due to shared vertex endpoints
    pub shared_vertex_conflict: bool,
    /// Conflict intensity (0.0 to 1.0)
    pub intensity: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::VertexId;

    #[test]
    fn test_have_write_conflict_no_conflict() {
        let ws1 = WriteSet::new();
        let ws2 = WriteSet::new();
        assert!(!have_write_conflict(&ws1, &ws2));
    }

    #[test]
    fn test_conflict_intensity_no_conflict() {
        let ws1 = WriteSet::new();
        let ws2 = WriteSet::new();
        assert_eq!(WriteSetAnalyzer::conflict_intensity(&ws1, &ws2), 0.0);
    }

    #[test]
    fn test_conflict_intensity_full_conflict() {
        let vid = VertexId::from_int64(1);

        let mut ws1 = WriteSet::new();
        ws1.record_vertex(vid);

        let mut ws2 = WriteSet::new();
        ws2.record_vertex(vid);

        assert_eq!(WriteSetAnalyzer::conflict_intensity(&ws1, &ws2), 1.0);
    }

    #[test]
    fn test_analyze_conflict_vertex() {
        let vid = VertexId::from_int64(1);

        let mut ws1 = WriteSet::new();
        ws1.record_vertex(vid);

        let mut ws2 = WriteSet::new();
        ws2.record_vertex(vid);

        let report = WriteSetAnalyzer::analyze_conflict(&ws1, &ws2);
        assert!(report.has_conflict);
        assert!(report.vertex_conflict);
        assert!(!report.edge_conflict);
    }

    #[test]
    fn test_analyze_conflict_different_vertices() {
        let vid1 = VertexId::from_int64(1);
        let vid2 = VertexId::from_int64(2);

        let mut ws1 = WriteSet::new();
        ws1.record_vertex(vid1);

        let mut ws2 = WriteSet::new();
        ws2.record_vertex(vid2);

        let report = WriteSetAnalyzer::analyze_conflict(&ws1, &ws2);
        assert!(!report.has_conflict);
        assert!(!report.vertex_conflict);
    }
}
