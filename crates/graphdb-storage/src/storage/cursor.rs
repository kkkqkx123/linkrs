//! Cursor / batch-reader traits for vertex and edge scanning.
//!
//! These traits provide a cursor-based alternative to the Vec-returning
//! scan methods on [`StorageReader`](super::StorageReader).  The caller
//! pulls batches of rows on demand instead of having the entire result
//! materialized upfront.
//!
//! # Performance contract
//!
//! Implementations **should** be lazy (read only what the caller asks
//! for).  The default implementations shipped here are thin wrappers
//! over the existing Vec-based scans – they are *not* truly lazy yet.
//! True cursor implementations that thread the internal `VertexIterator`
//! / CSR iterators will replace them once the storage-internal plumbing
//! is in place.

use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::StorageError;
use crate::core::Value;
use crate::storage::StorageClient;

// ---------------------------------------------------------------------------
// Cursor traits
// ---------------------------------------------------------------------------

/// A cursor that yields vertices in batches.
pub trait VertexCursor: Send + std::fmt::Debug {
    /// Read the next batch of vertices (at most `batch_size` rows).
    ///
    /// Returns an empty `Vec` when the scan is exhausted.
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<crate::core::Vertex>, StorageError>;
}

/// A cursor that yields edges in batches.
pub trait EdgeCursor: Send + std::fmt::Debug {
    /// Read the next batch of edges (at most `batch_size` rows).
    ///
    /// Returns an empty `Vec` when the scan is exhausted.
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<crate::core::Edge>, StorageError>;
}

// ---------------------------------------------------------------------------
// Default (Vec-backed) implementations
// ---------------------------------------------------------------------------

/// Vertex cursor backed by a pre-materialized `Vec<Vertex>`.
///
/// This is the default implementation used when the storage backend does
/// not yet provide a native lazy cursor.  It is semantically identical
/// to calling [`StorageReader::scan_vertices`] upfront.
#[derive(Debug)]
pub struct VecVertexCursor {
    iter: std::vec::IntoIter<crate::core::Vertex>,
}

impl VecVertexCursor {
    pub fn new(vertices: Vec<crate::core::Vertex>) -> Self {
        Self {
            iter: vertices.into_iter(),
        }
    }
}

impl VertexCursor for VecVertexCursor {
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<crate::core::Vertex>, StorageError> {
        Ok(self.iter.by_ref().take(batch_size).collect())
    }
}

/// Edge cursor backed by a pre-materialized `Vec<Edge>`.
#[derive(Debug)]
pub struct VecEdgeCursor {
    iter: std::vec::IntoIter<crate::core::Edge>,
}

impl VecEdgeCursor {
    pub fn new(edges: Vec<crate::core::Edge>) -> Self {
        Self {
            iter: edges.into_iter(),
        }
    }
}

impl EdgeCursor for VecEdgeCursor {
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<crate::core::Edge>, StorageError> {
        Ok(self.iter.by_ref().take(batch_size).collect())
    }
}

// ---------------------------------------------------------------------------
// Cursor-opening helpers on StorageClient
// ---------------------------------------------------------------------------

/// Open a vertex scan cursor through a storage client.
///
/// Uses the default Vec-backed cursor unless the client provides a
/// native cursor implementation.
///
/// When `limit` is `Some(n)`, at most `n` vertices are returned.
pub fn open_vertex_scan(
    storage: &Arc<RwLock<dyn StorageClient>>,
    space: &str,
    limit: Option<usize>,
) -> Result<Box<dyn VertexCursor>, StorageError> {
    let reader = storage.read();
    let mut vertices = reader.scan_vertices(space)?;
    if let Some(limit) = limit {
        vertices.truncate(limit);
    }
    Ok(Box::new(VecVertexCursor::new(vertices)))
}

/// Open a vertex scan cursor bound by a limit.
///
/// Convenience wrapper – delegates to [`open_vertex_scan`] with a limit.
pub fn open_vertex_scan_with_limit(
    storage: &Arc<RwLock<dyn StorageClient>>,
    space: &str,
    limit: usize,
) -> Result<Box<dyn VertexCursor>, StorageError> {
    open_vertex_scan(storage, space, Some(limit))
}

/// Open an edge scan cursor through a storage client.
///
/// Uses the default Vec-backed cursor unless the client provides a
/// native cursor implementation.
///
/// When `limit` is `Some(n)`, at most `n` edges are returned.
pub fn open_edge_scan(
    storage: &Arc<RwLock<dyn StorageClient>>,
    space: &str,
    edge_type: Option<&str>,
    limit: Option<usize>,
) -> Result<Box<dyn EdgeCursor>, StorageError> {
    let reader = storage.read();
    let mut edges = if let Some(et) = edge_type {
        reader.scan_edges_by_type(space, et)?
    } else {
        reader.scan_all_edges(space)?
    };
    if let Some(limit) = limit {
        edges.truncate(limit);
    }
    Ok(Box::new(VecEdgeCursor::new(edges)))
}
