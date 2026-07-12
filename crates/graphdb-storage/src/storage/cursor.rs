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
use crate::storage::StorageClient;

// ---------------------------------------------------------------------------
// Scan options
// ---------------------------------------------------------------------------

/// Unified scan options that configure cursor behavior.
///
/// This is the contract between the query planner/executor and the storage
/// layer.  Future phases will add predicate pushdown, projection, partition,
/// range, and snapshot support.
#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// Optional edge type filter (for edge scans only).
    pub edge_type: Option<String>,
    /// Maximum number of rows to return (None = unlimited).
    pub limit: Option<usize>,
    /// Batch size for cursor reads.
    pub batch_size: usize,
    /// Optional vertex ID range filter. Only vertices whose `id` falls in
    /// this range (inclusive of start, exclusive of end) are returned.
    /// When set, this filter is applied at scan time, not as a post-filter.
    pub vertex_id_range: Option<std::ops::Range<i64>>,
    /// Optional edge source ID range filter. Only edges whose source ID
    /// (parsed as `i64`) falls in this range are returned.
    pub edge_src_id_range: Option<std::ops::Range<i64>>,
}

impl ScanOptions {
    pub const DEFAULT_BATCH_SIZE: usize = 1024;

    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: set edge type filter.
    pub fn with_edge_type(mut self, edge_type: String) -> Self {
        self.edge_type = Some(edge_type);
        self
    }

    /// Builder: set row limit.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn batch_size(&self) -> usize {
        if self.batch_size == 0 {
            Self::DEFAULT_BATCH_SIZE
        } else {
            self.batch_size
        }
    }
}

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
/// When `options.limit` is `Some(n)`, at most `n` vertices are returned.
pub fn open_vertex_scan(
    storage: &Arc<RwLock<dyn StorageClient>>,
    space: &str,
    options: &ScanOptions,
) -> Result<Box<dyn VertexCursor>, StorageError> {
    let reader = storage.read();
    let mut vertices = reader.scan_vertices(space)?;
    if let Some(range) = &options.vertex_id_range {
        vertices.retain(|v| v.id >= range.start && v.id < range.end);
    }
    if let Some(limit) = options.limit {
        vertices.truncate(limit);
    }
    Ok(Box::new(VecVertexCursor::new(vertices)))
}

/// Open a vertex scan cursor with a limit.
///
/// Convenience wrapper – delegates to [`open_vertex_scan`] with a limit.
pub fn open_vertex_scan_with_limit(
    storage: &Arc<RwLock<dyn StorageClient>>,
    space: &str,
    limit: usize,
) -> Result<Box<dyn VertexCursor>, StorageError> {
    let options = ScanOptions::new().with_limit(limit);
    open_vertex_scan(storage, space, &options)
}

/// Open an edge scan cursor through a storage client.
///
/// Uses the default Vec-backed cursor unless the client provides a
/// native cursor implementation.
///
/// When `options.edge_type` is set, only edges of that type are scanned.
/// When `options.limit` is `Some(n)`, at most `n` edges are returned.
///
/// Phase 3 improvement: `edge_type` from the plan node is now passed
/// through `ScanOptions` instead of a separate parameter.
pub fn open_edge_scan(
    storage: &Arc<RwLock<dyn StorageClient>>,
    space: &str,
    options: &ScanOptions,
) -> Result<Box<dyn EdgeCursor>, StorageError> {
    let reader = storage.read();
    let mut edges = if let Some(ref et) = options.edge_type {
        reader.scan_edges_by_type(space, et)?
    } else {
        reader.scan_all_edges(space)?
    };
    if let Some(range) = &options.edge_src_id_range {
        edges.retain(|e| {
            e.src.to_string().parse::<i64>().is_ok_and(|id| id >= range.start && id < range.end)
        });
    }
    if let Some(limit) = options.limit {
        edges.truncate(limit);
    }
    Ok(Box::new(VecEdgeCursor::new(edges)))
}
