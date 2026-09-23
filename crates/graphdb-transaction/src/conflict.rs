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
        let vertex_overlap = ws1.vertices.intersection(&ws2.vertices).count();
        let edge_overlap = ws1.edges.intersection(&ws2.edges).count();

        let max_size = ws1.size().max(ws2.size());
        if max_size == 0 {
            return 0.0;
        }

        let overlap_count = vertex_overlap + edge_overlap;
        (overlap_count as f64) / (max_size as f64)
    }

    /// Check if conflict is due to vertex modification
    pub fn conflicts_on_vertex(ws1: &WriteSet, ws2: &WriteSet) -> bool {
        !ws1.vertices.is_disjoint(&ws2.vertices)
    }

    /// Check if conflict is due to edge modification
    pub fn conflicts_on_edge(ws1: &WriteSet, ws2: &WriteSet) -> bool {
        !ws1.edges.is_disjoint(&ws2.edges)
    }

    /// Read-your-own-writes check: whether `writer` locally wrote an entity
    /// that a snapshot read at the writer's own timestamp would otherwise
    /// miss. Read paths merge locally covered entities over the snapshot.
    pub fn has_local_write(writer: &WriteSet, vid: &graphdb_core::types::VertexId) -> bool {
        writer.covers_vertex(vid)
    }

    /// Edge counterpart of [`Self::has_local_write`]: whether `writer`
    /// locally wrote `edge`. Same merge-over-snapshot contract.
    pub fn has_local_edge_write(
        writer: &WriteSet,
        edge: &graphdb_core::types::EdgeIdentifier,
    ) -> bool {
        writer.covers_edge(edge)
    }

    /// Get a detailed conflict report
    pub fn analyze_conflict(ws1: &WriteSet, ws2: &WriteSet) -> ConflictReport {
        ConflictReport {
            has_conflict: have_write_conflict(ws1, ws2),
            vertex_conflict: Self::conflicts_on_vertex(ws1, ws2),
            edge_conflict: Self::conflicts_on_edge(ws1, ws2),
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
    /// Conflict intensity (0.0 to 1.0)
    pub intensity: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::types::VertexId;

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
        let vid = VertexId::try_from_int64(1).expect("test vertex id");

        let mut ws1 = WriteSet::new();
        ws1.record_vertex(vid);

        let mut ws2 = WriteSet::new();
        ws2.record_vertex(vid);

        assert_eq!(WriteSetAnalyzer::conflict_intensity(&ws1, &ws2), 1.0);
    }

    #[test]
    fn test_analyze_conflict_vertex() {
        let vid = VertexId::try_from_int64(1).expect("test vertex id");

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
        let vid1 = VertexId::try_from_int64(1).expect("test vertex id");
        let vid2 = VertexId::try_from_int64(2).expect("test vertex id");

        let mut ws1 = WriteSet::new();
        ws1.record_vertex(vid1);

        let mut ws2 = WriteSet::new();
        ws2.record_vertex(vid2);

        let report = WriteSetAnalyzer::analyze_conflict(&ws1, &ws2);
        assert!(!report.has_conflict);
        assert!(!report.vertex_conflict);
    }

    #[test]
    fn test_shared_endpoint_not_conflict() {
        let vid1 = VertexId::try_from_int64(1).expect("test vertex id");
        let vid2 = VertexId::try_from_int64(2).expect("test vertex id");
        let vid3 = VertexId::try_from_int64(3).expect("test vertex id");

        let mut ws1 = WriteSet::new();
        let edge1 = graphdb_core::types::EdgeIdentifier::new(1, vid1, 1, vid2, 1, 0);
        ws1.record_edge(edge1);

        let mut ws2 = WriteSet::new();
        let edge2 = graphdb_core::types::EdgeIdentifier::new(1, vid1, 1, vid3, 1, 0);
        ws2.record_edge(edge2);

        assert!(
            !ws1.has_conflict_with(&ws2),
            "edges sharing a source vertex should not conflict"
        );
    }

    #[test]
    fn test_has_local_write_covers_recorded_vertices() {
        let vid = VertexId::try_from_int64(7).expect("test vertex id");
        let other = VertexId::try_from_int64(8).expect("test vertex id");

        let mut ws = WriteSet::new();
        assert!(!WriteSetAnalyzer::has_local_write(&ws, &vid));
        ws.record_vertex(vid);
        assert!(WriteSetAnalyzer::has_local_write(&ws, &vid));
        assert!(!WriteSetAnalyzer::has_local_write(&ws, &other));

        let mut deleted = WriteSet::new();
        deleted.record_vertex_delete(vid);
        assert!(WriteSetAnalyzer::has_local_write(&deleted, &vid));
    }

    #[test]
    fn test_has_local_edge_write_covers_recorded_edges() {
        use graphdb_core::types::EdgeIdentifier;
        let vid1 = VertexId::try_from_int64(1).expect("test vertex id");
        let vid2 = VertexId::try_from_int64(2).expect("test vertex id");
        let vid3 = VertexId::try_from_int64(3).expect("test vertex id");
        let edge = EdgeIdentifier::new(1, vid1, 1, vid2, 1, 0);
        let other = EdgeIdentifier::new(1, vid1, 1, vid3, 1, 0);

        let mut ws = WriteSet::new();
        assert!(!WriteSetAnalyzer::has_local_edge_write(&ws, &edge));
        ws.record_edge(edge);
        assert!(WriteSetAnalyzer::has_local_edge_write(&ws, &edge));
        assert!(!WriteSetAnalyzer::has_local_edge_write(&ws, &other));
    }
}
