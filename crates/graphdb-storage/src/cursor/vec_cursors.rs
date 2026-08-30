use graphdb_core::StorageError;

/// Vertex cursor backed by a pre-materialized `Vec<Vertex>`.
///
/// This is the default implementation used when the storage backend does
/// not yet provide a native lazy cursor.  It is semantically identical
/// to calling [`StorageReader::scan_vertices`] upfront.
#[derive(Debug)]
pub struct VecVertexCursor {
    iter: std::vec::IntoIter<graphdb_core::Vertex>,
}

impl VecVertexCursor {
    pub fn new(vertices: Vec<graphdb_core::Vertex>) -> Self {
        Self {
            iter: vertices.into_iter(),
        }
    }
}

impl crate::cursor::VertexCursor for VecVertexCursor {
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<graphdb_core::Vertex>, StorageError> {
        Ok(self.iter.by_ref().take(batch_size).collect())
    }
}

/// Edge cursor backed by a pre-materialized `Vec<Edge>`.
#[derive(Debug)]
pub struct VecEdgeCursor {
    iter: std::vec::IntoIter<graphdb_core::Edge>,
}

impl VecEdgeCursor {
    pub fn new(edges: Vec<graphdb_core::Edge>) -> Self {
        Self {
            iter: edges.into_iter(),
        }
    }
}

impl crate::cursor::EdgeCursor for VecEdgeCursor {
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<graphdb_core::Edge>, StorageError> {
        Ok(self.iter.by_ref().take(batch_size).collect())
    }
}
