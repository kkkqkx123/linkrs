//! Write-set and SSI tracking

use std::collections::HashSet;

use graphdb_core::types::{EdgeIdentifier, VertexId};

/// Write Set - tracks entities modified by a transaction for conflict detection
#[derive(Debug, Clone, Default)]
pub struct WriteSet {
    /// Vertices modified (insert/update/delete)
    pub vertices: HashSet<VertexId>,
    /// Edges modified (insert/update/delete)
    pub edges: HashSet<EdgeIdentifier>,
    /// Vertex IDs used as edge endpoints (source/destination).
    /// Collected for O(1) endpoint lookup.
    pub edge_endpoints: HashSet<VertexId>,
    /// Vertices deleted by this transaction. This is narrower than `vertices`
    /// and is used for vertex-delete versus edge-write certification.
    pub deleted_vertices: HashSet<VertexId>,
    /// Schema resources changed by this transaction.
    pub schema_resources: HashSet<String>,
    /// Index resources changed by this transaction.
    pub index_resources: HashSet<String>,
    /// Predicate-based read ranges for Serializable phantom detection.
    pub read_ranges: Vec<ReadRange>,
}

impl WriteSet {
    /// Create an empty write set
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a vertex write
    pub fn record_vertex(&mut self, vid: VertexId) {
        self.vertices.insert(vid);
    }

    /// Record a vertex deletion and retain the deletion kind for certification.
    pub fn record_vertex_delete(&mut self, vid: VertexId) {
        self.vertices.insert(vid);
        self.deleted_vertices.insert(vid);
    }

    /// Record an edge write
    pub fn record_edge(&mut self, edge: EdgeIdentifier) {
        self.edge_endpoints.insert(edge.src_vid);
        self.edge_endpoints.insert(edge.dst_vid);
        self.edges.insert(edge);
    }

    pub fn record_schema_resource(&mut self, resource: impl Into<String>) {
        self.schema_resources.insert(resource.into());
    }

    pub fn record_index_resource(&mut self, resource: impl Into<String>) {
        self.index_resources.insert(resource.into());
    }

    /// Record a predicate-based read range for Serializable phantom detection.
    pub fn record_read_range(&mut self, range: ReadRange) {
        self.read_ranges.push(range);
    }

    /// Check whether any committed write falls within a recorded read range.
    pub fn has_read_range_conflict_with(&self, committed: &WriteSet) -> bool {
        for range in &self.read_ranges {
            for vid in &committed.vertices {
                if range.contains(vid) {
                    return true;
                }
            }
        }
        false
    }

    /// Whether the set carries no certification-relevant resources.
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
            && self.edges.is_empty()
            && self.schema_resources.is_empty()
            && self.index_resources.is_empty()
            && self.read_ranges.is_empty()
    }

    pub fn is_empty_for_certification(&self) -> bool {
        self.is_empty()
    }

    /// Get the number of modified entities
    pub fn size(&self) -> usize {
        self.vertices.len() + self.edges.len()
    }

    /// Convert write set entities into `ResourceId`s for SSI tracking.
    pub fn ssi_resources(&self) -> Vec<ResourceId> {
        let mut resources = Vec::new();
        for vid in &self.vertices {
            resources.push(ResourceId::Vertex(*vid));
        }
        for edge in &self.edges {
            resources.push(ResourceId::Edge(*edge));
        }
        for res in &self.schema_resources {
            resources.push(ResourceId::Schema(res.clone()));
        }
        for res in &self.index_resources {
            resources.push(ResourceId::Index(res.clone()));
        }
        resources
    }

    /// Check if two write sets have any conflicting entities.
    ///
    /// Conflict is defined as: same vertex modified OR same edge modified.
    /// Edges sharing endpoints (source/destination) without actually modifying
    /// the same entity are NOT considered conflicting.
    pub fn has_conflict_with(&self, other: &WriteSet) -> bool {
        if !self.vertices.is_disjoint(&other.vertices) {
            return true;
        }
        if !self.edges.is_disjoint(&other.edges) {
            return true;
        }
        if !self.deleted_vertices.is_disjoint(&other.edge_endpoints)
            || !other.deleted_vertices.is_disjoint(&self.edge_endpoints)
        {
            return true;
        }
        if !self.schema_resources.is_disjoint(&other.schema_resources)
            || !self.index_resources.is_disjoint(&other.index_resources)
        {
            return true;
        }
        // Schema changes affect the physical data layout. Certify them
        // against concurrent data writes even when the entity keys differ.
        if (!self.schema_resources.is_empty()
            && (!other.vertices.is_empty() || !other.edges.is_empty()))
            || (!other.schema_resources.is_empty()
                && (!self.vertices.is_empty() || !self.edges.is_empty()))
        {
            return true;
        }
        false
    }
}

/// A predicate-based range of vertex IDs read by a Serializable transaction.
///
/// Used for phantom detection: if a concurrent write creates a vertex whose
/// ID falls within this range and matches the label, the Serializable
/// transaction is aborted to prevent phantoms.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadRange {
    /// Vertex label (vertex type name).
    pub label: String,
    /// Optional property column name for the indexed predicate.
    pub column: Option<String>,
    /// Lower bound (inclusive when `start_inclusive` is true).
    pub start: Option<VertexId>,
    /// Upper bound (inclusive when `end_inclusive` is true).
    pub end: Option<VertexId>,
}

impl ReadRange {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            column: None,
            start: None,
            end: None,
        }
    }

    pub fn with_column(mut self, column: impl Into<String>) -> Self {
        self.column = Some(column.into());
        self
    }

    pub fn with_start(mut self, start: VertexId) -> Self {
        self.start = Some(start);
        self
    }

    pub fn with_end(mut self, end: VertexId) -> Self {
        self.end = Some(end);
        self
    }

    /// Check whether the given `VertexId` falls within this range.
    pub fn contains(&self, vid: &VertexId) -> bool {
        if let Some(ref start) = self.start {
            let cmp = vid.as_bytes().cmp(start.as_bytes());
            if cmp == std::cmp::Ordering::Less {
                return false;
            }
        }
        if let Some(ref end) = self.end {
            let cmp = vid.as_bytes().cmp(end.as_bytes());
            if cmp == std::cmp::Ordering::Greater {
                return false;
            }
        }
        true
    }
}

/// Unified resource identifier for SSI rw-dependency tracking.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResourceId {
    Vertex(VertexId),
    Edge(graphdb_core::types::EdgeIdentifier),
    Schema(String),
    Index(String),
}

/// Per-transaction SSI (Serializable Snapshot Isolation) state.
///
/// Tracks which resources this transaction has read and written, enabling
/// O(1) dangerous-structure detection instead of O(N) committed write-set scanning.
#[derive(Debug, Clone, Default)]
pub struct SsiState {
    /// Resources read by this transaction (populated via `record_ssi_read`).
    read_resources: HashSet<ResourceId>,
    /// Resources written by this transaction (populated via `record_ssi_write`).
    write_resources: HashSet<ResourceId>,
}

impl SsiState {
    pub fn new() -> Self {
        Self {
            read_resources: HashSet::new(),
            write_resources: HashSet::new(),
        }
    }

    pub fn record_read(&mut self, resource: ResourceId) {
        self.read_resources.insert(resource);
    }

    pub fn record_write(&mut self, resource: ResourceId) {
        self.write_resources.insert(resource);
    }

    pub fn read_resources(&self) -> &HashSet<ResourceId> {
        &self.read_resources
    }

    pub fn write_resources(&self) -> &HashSet<ResourceId> {
        &self.write_resources
    }

    pub fn is_empty(&self) -> bool {
        self.read_resources.is_empty() && self.write_resources.is_empty()
    }
}
