//! NPath - path representation of a linked list structure
//!
//! Refer to nebula-graph's NPath design for prefix sharing using shared ownership.
//! For scenarios where frequent path expansion is required in graph traversal.
//!
//! # Core strengths
//!
//! 1. **Shared prefix**: multiple paths share the same prefix portion to save memory
//! 2. **O(1) Extension **: New path simply creates a new node pointing to parent path
//! 3. **Fast splicing**: bi-directional BFS path splicing by simply finding the point of intersection
//!

use std::collections::HashSet;
use std::sync::Arc;

use crate::core::types::VertexId;
use crate::core::vertex_edge_path::Step;
use crate::core::{Edge, Path, Vertex};

/// NPath - path representation of a linked list structure
///
/// Shared ownership via Arc using immutable data structures.
/// Each node contains a vertex and an edge (other than the starting point) that reaches that vertex.
#[derive(Debug, Clone)]
pub struct NPath {
    /// Parent path node (None indicates the starting point)
    parent: Option<Arc<NPath>>,
    /// current vertex
    vertex: Arc<Vertex>,
    /// Edge to current vertex (starting point None)
    edge: Option<Arc<Edge>>,
    /// Path length (cached to avoid recursive calculations)
    length: usize,
    /// Path hashing (caching, for fast comparisons)
    hash: u64,
}

impl NPath {
    /// Creating a Starting Path
    ///
    /// # Parameters
    ///
    /// * `vertex` - starting vertex
    pub fn new(vertex: Arc<Vertex>) -> Self {
        let hash = Self::compute_hash(&vertex, None, None);
        Self {
            parent: None,
            vertex,
            edge: None,
            length: 0,
            hash,
        }
    }

    /// Extended Path - O(1) Operation
    ///
    /// Creates a new path node that points to the parent path for prefix sharing.
    ///
    /// # Parameters
    ///
    /// * `parent` - parent path
    /// * `edge` - the edge that reaches the new vertex
    /// * `vertex` - new vertex
    pub fn extend(parent: Arc<NPath>, edge: Arc<Edge>, vertex: Arc<Vertex>) -> Self {
        let length = parent.length + 1;
        let hash = Self::compute_hash(&vertex, Some(&edge), Some(parent.hash));
        Self {
            parent: Some(parent),
            vertex,
            edge: Some(edge),
            length,
            hash,
        }
    }

    /// Extend path and check for loops - O(1) averaging time
    ///
    /// Fast detection of loop formation using HashSet, suitable for early pruning in DFS exploration.
    ///
    /// # Parameters
    ///
    /// * `parent` - parent path
    /// * `edge` - the edge that reaches the new vertex
    /// * `vertex` - new vertex
    /// * :: `seen_vertices` - set of visited vertices
    ///
    /// # Back
    ///
    /// * `Some(NPath)` - extension successful
    /// * `None` - loop detected
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut seen = HashSet::new();
    /// seen.insert(parent.vertex.vid.as_ref().clone());
    /// if let Some(extended) = NPath::extend_with_set(parent, edge, vertex, &mut seen) {
    // Continue to explore
    /// }
    /// ```
    pub fn extend_with_set(
        parent: Arc<NPath>,
        edge: Arc<Edge>,
        vertex: Arc<Vertex>,
        seen_vertices: &mut HashSet<VertexId>,
    ) -> Option<Self> {
        if seen_vertices.contains(&vertex.vid) {
            return None;
        }

        let new_path = Self::extend(parent, edge, vertex);
        seen_vertices.insert(new_path.vertex.vid);
        Some(new_path)
    }

    /// Create NPath from Path
    ///
    /// Facilitates compatibility with existing interfaces, converting traditional Path to NPath.
    ///
    /// # Parameters
    ///
    /// * :: `path` -- traditional Path structure
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let npath = NPath::from_path(&path);
    /// ```
    pub fn from_path(path: &Path) -> Arc<Self> {
        let start_vertex = Arc::new((*path.src).clone());
        let mut current = Arc::new(Self::new(start_vertex));

        for step in &path.steps {
            let edge = Arc::new((*step.edge).clone());
            let vertex = Arc::new((*step.dst).clone());
            current = Arc::new(Self::extend(current, edge, vertex));
        }

        current
    }

    /// Get path length (number of edges)
    pub fn len(&self) -> usize {
        self.length
    }

    /// Check if the path is empty (contains only the starting point)
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Get current vertex
    pub fn vertex(&self) -> &Arc<Vertex> {
        &self.vertex
    }

    /// Get the edge that reaches the current vertex
    pub fn edge(&self) -> Option<&Arc<Edge>> {
        self.edge.as_ref()
    }

    /// Get Parent Path
    pub fn parent(&self) -> Option<&Arc<NPath>> {
        self.parent.as_ref()
    }

    /// Get start vertex
    pub fn start_vertex(&self) -> &Arc<Vertex> {
        let mut current = self;
        while let Some(ref parent) = current.parent {
            current = parent;
        }
        &current.vertex
    }

    /// Get end vertex (current vertex)
    pub fn end_vertex(&self) -> &Arc<Vertex> {
        &self.vertex
    }

    /// Translate the text into "Path" (and convert it further if necessary):
    ///
    /// Time complexity: O(n), n is the path length
    pub fn to_path(&self) -> Path {
        let mut steps = Vec::with_capacity(self.length);
        let mut current = self;

        // Collect all the steps (from the end point to the starting point).
        while let Some(ref parent) = current.parent {
            if let Some(ref edge) = current.edge {
                steps.push(Step {
                    dst: Box::new((*current.vertex).clone()),
                    edge: Box::new((**edge).clone()),
                });
            }
            current = parent;
        }

        // Reverse the steps (from the starting point to the ending point)
        steps.reverse();

        Path {
            src: Box::new((*current.vertex).clone()),
            steps,
        }
    }

    pub fn contains_vertex(&self, vid: &VertexId) -> bool {
        if &self.vertex.vid == vid {
            return true;
        }
        if let Some(ref parent) = self.parent {
            return parent.contains_vertex(vid);
        }
        false
    }

    pub fn contains_edge(&self, edge_key: &(VertexId, VertexId, String)) -> bool {
        if let Some(ref edge) = self.edge {
            let key = (edge.src, edge.dst, edge.edge_type.clone());
            if &key == edge_key {
                return true;
            }
        }
        if let Some(ref parent) = self.parent {
            return parent.contains_edge(edge_key);
        }
        false
    }

    pub fn has_common_vertices(&self, other: &NPath) -> bool {
        let self_vertices: HashSet<_> = self.iter_vertices().map(|v| &v.vid).collect();
        other
            .iter_vertices()
            .any(|v| self_vertices.contains(&v.vid))
    }

    pub fn collect_vertex_ids(&self) -> Vec<VertexId> {
        self.iter_vertices().map(|v| v.vid).collect()
    }

    /// Collect all the edges.
    pub fn collect_edges(&self) -> Vec<Arc<Edge>> {
        self.iter_edges().cloned().collect()
    }

    /// Iterator: All vertices from the starting point to the current node.
    pub fn iter_vertices(&self) -> NPathVertexIter<'_> {
        NPathVertexIter::new(self)
    }

    /// Iterator: All edges from the starting point to the current node
    pub fn iter_edges(&self) -> NPathEdgeIter<'_> {
        NPathEdgeIter::new(self)
    }

    /// Iterator: All nodes from the starting point to the current node.
    pub fn iter(&self) -> NPathIter<'_> {
        NPathIter::new(self)
    }

    /// Calculating the path hash
    fn compute_hash(
        vertex: &Arc<Vertex>,
        edge: Option<&Arc<Edge>>,
        parent_hash: Option<u64>,
    ) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();

        if let Some(ph) = parent_hash {
            ph.hash(&mut hasher);
        }

        vertex.vid.hash(&mut hasher);

        if let Some(e) = edge {
            e.edge_type.hash(&mut hasher);
            e.src.hash(&mut hasher);
            e.dst.hash(&mut hasher);
        }

        hasher.finish()
    }

    /// Obtain the path hash
    pub fn hash(&self) -> u64 {
        self.hash
    }
}

impl PartialEq for NPath {
    fn eq(&self, other: &Self) -> bool {
        if self.hash != other.hash || self.length != other.length {
            return false;
        }

        // The hashes are the same; a further comparison of the content is necessary.
        self.vertex.vid == other.vertex.vid
            && self.edge == other.edge
            && self.parent == other.parent
    }
}

impl Eq for NPath {}

/// NPath iterator – Traverses all nodes (from the end point to the start point)
///
/// Use inert summation, each time .next() jumps up one step to avoid preallocating Vecs
pub struct NPathIter<'a> {
    current: Option<&'a NPath>,
}

impl<'a> NPathIter<'a> {
    fn new(path: &'a NPath) -> Self {
        Self {
            current: Some(path),
        }
    }
}

impl<'a> Iterator for NPathIter<'a> {
    type Item = &'a NPath;

    fn next(&mut self) -> Option<Self::Item> {
        let curr = self.current?;
        self.current = curr.parent.as_deref();
        Some(curr)
    }
}

/// NPath vertex iterator – Lazy traversal of all vertices
///
/// Optimization: no pre-allocated Vec, each time .next() jumps up one step
pub struct NPathVertexIter<'a> {
    current: Option<&'a NPath>,
}

impl<'a> NPathVertexIter<'a> {
    fn new(path: &'a NPath) -> Self {
        Self {
            current: Some(path),
        }
    }
}

impl<'a> Iterator for NPathVertexIter<'a> {
    type Item = &'a Arc<Vertex>;

    fn next(&mut self) -> Option<Self::Item> {
        let curr = self.current?;
        let vertex = &curr.vertex;
        self.current = curr.parent.as_deref();
        Some(vertex)
    }
}

/// NPath edge iterator – Lazy traversal of all edges
///
/// Optimization: no pre-allocated Vec, each time .next() jumps up one step
pub struct NPathEdgeIter<'a> {
    current: Option<&'a NPath>,
}

impl<'a> NPathEdgeIter<'a> {
    fn new(path: &'a NPath) -> Self {
        Self {
            current: Some(path),
        }
    }
}

impl<'a> Iterator for NPathEdgeIter<'a> {
    type Item = &'a Arc<Edge>;

    fn next(&mut self) -> Option<Self::Item> {
        let curr = self.current?;
        let edge = curr.edge.as_ref()?;
        self.current = curr.parent.as_deref();
        Some(edge)
    }
}

/// NPath utility functions
pub mod utils {
    use super::*;

    /// Concatenating two paths (for bidirectional BFS)
    ///
    /// The left path goes from the starting point to the middle point, while the right path goes from the ending point to the middle point.
    /// The result path goes from the starting point to the ending point.
    pub fn combine_paths(left: &Arc<NPath>, right: &Arc<NPath>) -> Option<Path> {
        // Check whether the two paths intersect at the same vertex.
        if left.vertex.vid != right.vertex.vid {
            return None;
        }

        // Construct a path from the starting point on the left to the intersection point.
        let left_path = left.to_path();

        // Construct a path from the starting point on the right to the intersection point, and then reverse it.
        let mut right_path = right.to_path();
        right_path.reverse();

        // Merge the two paths
        let mut combined = left_path;
        combined.steps.extend(right_path.steps);

        Some(combined)
    }

    /// Convert NPaths to Paths in batches
    pub fn batch_to_paths(npaths: &[Arc<NPath>]) -> Vec<Path> {
        npaths.iter().map(|np| np.to_path()).collect()
    }

    /// Check whether there are any duplicates in the set of paths.
    pub fn has_duplicates(npaths: &[Arc<NPath>]) -> bool {
        let mut seen = HashSet::new();
        for np in npaths {
            if !seen.insert(np.hash()) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::VertexId;

    fn create_test_vertex(id: i64) -> Arc<Vertex> {
        Arc::new(Vertex::new(VertexId::from_int64(id), vec![]))
    }

    fn create_test_edge(src_id: i64, dst_id: i64, edge_type: &str) -> Arc<Edge> {
        use std::collections::HashMap;
        Arc::new(Edge::new(
            VertexId::from_int64(src_id),
            VertexId::from_int64(dst_id),
            edge_type.to_string(),
            0,
            HashMap::new(),
        ))
    }

    #[test]
    fn test_npath_new() {
        let v = create_test_vertex(1);
        let path = NPath::new(v.clone());

        assert_eq!(path.len(), 0);
        assert!(path.is_empty());
        assert_eq!(path.vertex().vid, VertexId::from_int64(1));
        assert!(path.parent().is_none());
        assert!(path.edge().is_none());
    }

    #[test]
    fn test_npath_extend() {
        let v1 = create_test_vertex(1);
        let v2 = create_test_vertex(2);
        let e = create_test_edge(1, 2, "friend");

        let start = Arc::new(NPath::new(v1));
        let extended = NPath::extend(start, e, v2);

        assert_eq!(extended.len(), 1);
        assert!(!extended.is_empty());
        assert_eq!(extended.vertex().vid, VertexId::from_int64(2));
        assert!(extended.parent().is_some());
        assert!(extended.edge().is_some());
    }

    #[test]
    fn test_npath_to_path() {
        let v1 = create_test_vertex(1);
        let v2 = create_test_vertex(2);
        let v3 = create_test_vertex(3);
        let e1 = create_test_edge(1, 2, "friend");
        let e2 = create_test_edge(2, 3, "friend");

        let start = Arc::new(NPath::new(v1));
        let p2 = Arc::new(NPath::extend(start, e1, v2));
        let p3 = Arc::new(NPath::extend(p2, e2, v3));

        let path = p3.to_path();

        assert_eq!(path.len(), 2);
        assert_eq!(path.src.vid, VertexId::from_int64(1));
    }

    #[test]
    fn test_npath_contains_vertex() {
        let v1 = create_test_vertex(1);
        let v2 = create_test_vertex(2);
        let v3 = create_test_vertex(3);
        let e1 = create_test_edge(1, 2, "friend");
        let e2 = create_test_edge(2, 3, "friend");

        let start = Arc::new(NPath::new(v1));
        let p2 = Arc::new(NPath::extend(start, e1, v2));
        let p3 = Arc::new(NPath::extend(p2, e2, v3));

        assert!(p3.contains_vertex(&VertexId::from_int64(1)));
        assert!(p3.contains_vertex(&VertexId::from_int64(2)));
        assert!(p3.contains_vertex(&VertexId::from_int64(3)));
        assert!(!p3.contains_vertex(&VertexId::from_int64(4)));
    }

    #[test]
    fn test_npath_iter_vertices() {
        let v1 = create_test_vertex(1);
        let v2 = create_test_vertex(2);
        let v3 = create_test_vertex(3);
        let e1 = create_test_edge(1, 2, "friend");
        let e2 = create_test_edge(2, 3, "friend");

        let start = Arc::new(NPath::new(v1));
        let p2 = Arc::new(NPath::extend(start, e1, v2));
        let p3 = Arc::new(NPath::extend(p2, e2, v3));

        let vertices: Vec<_> = p3.iter_vertices().collect();
        assert_eq!(vertices.len(), 3);
    }

    #[test]
    fn test_npath_equality() {
        let v1 = create_test_vertex(1);
        let v2 = create_test_vertex(2);
        let e = create_test_edge(1, 2, "friend");

        let start1 = Arc::new(NPath::new(v1.clone()));
        let path1 = Arc::new(NPath::extend(start1, e.clone(), v2.clone()));

        let start2 = Arc::new(NPath::new(v1));
        let path2 = Arc::new(NPath::extend(start2, e, v2));

        assert_eq!(path1.hash(), path2.hash());
    }
}
