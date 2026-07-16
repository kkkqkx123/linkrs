//! Cursor / batch-reader traits for vertex and edge scanning.
//!
//! These traits provide a cursor-based alternative to the Vec-returning
//! scan methods on [`StorageReader`](super::StorageReader).  The caller
//! pulls batches of rows on demand instead of having the entire result
//! materialized upfront.
//!
//! # Performance contract
//!
//! Storage engines are expected to provide native lazy cursors. The
//! `Vec*Cursor` types remain available for adapters and test doubles, but
//! they are explicit materialized implementations rather than an implicit
//! fallback for production scans.

use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::types::storage_ids::VertexId;
use crate::core::types::Timestamp;
use crate::core::value::NullType;
use crate::core::StorageError;
use crate::storage::StorageClient;

// ---------------------------------------------------------------------------
// Scan target (type-safe scan intent)
// ---------------------------------------------------------------------------

/// Identifies what kind of scan is being performed.
///
/// Used alongside [`ScanOptions`] to make the scan intent explicit and
/// catch misconfiguration (e.g. passing `edge_type` with a vertex scan).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanTarget {
    Vertex,
    Edge { edge_type: Option<String> },
}

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
    /// Maximum number of rows to return (None = unlimited).
    pub limit: Option<usize>,
    /// Number of matching rows to skip before emitting the first row.
    pub offset: usize,
    /// Batch size for cursor reads.
    pub batch_size: usize,
    /// Optional vertex ID range filter. Only vertices whose `id` falls in
    /// this range (inclusive of start, exclusive of end) are returned.
    /// When set, this filter is applied at scan time, not as a post-filter.
    pub vertex_id_range: Option<std::ops::Range<i64>>,
    /// Optional edge source ID range filter. Only edges whose source ID
    /// (parsed as `i64`) falls in this range are returned.
    pub edge_src_id_range: Option<std::ops::Range<i64>>,
    /// Edge type filter (for edge scans only).
    pub edge_type: Option<String>,
    /// Optional property projection pushed into the physical scan.
    pub projection: Option<Vec<String>>,
    /// Read timestamp captured by the caller.
    pub read_timestamp: Option<Timestamp>,
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

    /// Builder: set vertex ID range filter.
    pub fn with_vertex_id_range(mut self, range: std::ops::Range<i64>) -> Self {
        self.vertex_id_range = Some(range);
        self
    }

    /// Builder: set edge source ID range filter.
    pub fn with_edge_src_id_range(mut self, range: std::ops::Range<i64>) -> Self {
        self.edge_src_id_range = Some(range);
        self
    }

    /// Builder: set row limit.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Builder: set the number of matching rows to skip.
    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }

    pub fn with_projection(mut self, projection: Vec<String>) -> Self {
        self.projection = Some(projection);
        self
    }

    pub fn with_read_timestamp(mut self, read_timestamp: Timestamp) -> Self {
        self.read_timestamp = Some(read_timestamp);
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

/// Predicate understood by native index cursors.
#[derive(Debug, Clone, PartialEq)]
pub enum IndexPredicate {
    Equal(crate::core::Value),
    Range {
        lower: Option<crate::core::Value>,
        upper: Option<crate::core::Value>,
        include_lower: bool,
        include_upper: bool,
    },
    Prefix(crate::core::Value),
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PartitionSelector {
    #[default]
    All,
    Shards(Vec<u32>),
    KeyRange {
        lower: Vec<u8>,
        upper: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum IndexRow {
    RowId(crate::core::wal::EntityRef),
    Covering {
        entity_ref: crate::core::wal::EntityRef,
        columns: Vec<(String, crate::core::Value)>,
    },
}

/// Immutable physical index scan contract.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexScanPlan {
    pub space: String,
    pub index_id: u64,
    pub predicate: IndexPredicate,
    pub projection: Option<Vec<String>>,
    pub limit: Option<usize>,
    pub offset: usize,
    pub read_timestamp: Timestamp,
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

/// A cursor that yields index entries (row IDs or covering rows) in batches.
///
/// Bound to a transaction snapshot at creation time.  Supports equality,
/// range, and prefix predicates as available in the storage engine.
/// Unsupported predicate types return an error at open time, not at runtime.
pub trait IndexCursor: Send + std::fmt::Debug {
    /// The type of row identifier this cursor yields.
    type Row: Send;

    /// Read the next batch of index entries (at most `batch_size`).
    ///
    /// Returns an empty `Vec` when exhausted.
    /// Stale or deleted row IDs are counted but skipped — they do not
    /// cause premature exhaustion.
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<Self::Row>, StorageError>;

    /// Return the total number of rows that matched the index predicate
    /// at cursor creation time (before stale filtering).
    fn estimated_match_count(&self) -> Option<u64> {
        None
    }

    /// Number of stale rows skipped so far (for diagnostics).
    fn stale_skipped(&self) -> u64 {
        0
    }

    /// Whether the cursor has reached the end of its physical scan.
    ///
    /// A batch may be empty even before exhaustion when all entries in that
    /// batch are invisible or stale, so callers that need to continue over
    /// such entries must inspect this flag.
    fn is_exhausted(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Property batch reader
// ---------------------------------------------------------------------------

/// A batch reader that reads properties for multiple entities at once.
///
/// Unlike row-at-a-time `get_vertex` / `get_edge`, this allows the storage
/// layer to amortise lookup overhead across many entities.
pub trait PropertyBatchReader: Send + std::fmt::Debug {
    /// Read a set of named properties for a batch of vertices.
    ///
    /// Returns one `Vec<Value>` per vertex in input order.
    /// Missing entities produce an all-null row (or error, depending on the
    /// `missing_policy` configuration at spec level).  Missing properties
    /// produce `Value::Null` for that slot.
    fn read_vertex_props(
        &self,
        ids: &[VertexId],
        prop_names: &[String],
    ) -> Result<Vec<Vec<crate::core::Value>>, StorageError>;

    /// Read a set of named properties for a batch of edges.
    ///
    /// Edges are identified by (src, dst, edge_type, rank).
    /// Returns one `Vec<Value>` per edge in input order.
    fn read_edge_props(
        &self,
        edges: &[(VertexId, VertexId, String, i64)],
        prop_names: &[String],
    ) -> Result<Vec<Vec<crate::core::Value>>, StorageError>;
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
/// Prefers the storage engine's native cursor when available (via
/// [`StorageReader::create_vertex_cursor`]), falling back to the default
/// Vec-backed cursor.
///
/// When `options.limit` is `Some(n)`, at most `n` vertices are returned.
pub fn open_vertex_scan<S: crate::storage::StorageReader + ?Sized>(
    storage: &Arc<RwLock<S>>,
    space: &str,
    options: &ScanOptions,
) -> Result<Box<dyn VertexCursor>, StorageError> {
    let reader = storage.read();
    reader.create_vertex_cursor(space, options)
}

/// Open a vertex scan cursor with a limit.
///
/// Convenience wrapper – delegates to [`open_vertex_scan`] with a limit.
pub fn open_vertex_scan_with_limit<S: crate::storage::StorageReader + ?Sized>(
    storage: &Arc<RwLock<S>>,
    space: &str,
    limit: usize,
) -> Result<Box<dyn VertexCursor>, StorageError> {
    let options = ScanOptions::new().with_limit(limit);
    open_vertex_scan(storage, space, &options)
}

/// Open an edge scan cursor through a storage client.
///
/// Prefers the storage engine's native cursor when available (via
/// [`StorageReader::create_edge_cursor`]), falling back to the default
/// Vec-backed cursor.
///
/// When `options.edge_type` is set, only edges of that type are scanned.
/// When `options.limit` is `Some(n)`, at most `n` edges are returned.
pub fn open_edge_scan<S: crate::storage::StorageReader + ?Sized>(
    storage: &Arc<RwLock<S>>,
    space: &str,
    options: &ScanOptions,
) -> Result<Box<dyn EdgeCursor>, StorageError> {
    let reader = storage.read();
    reader.create_edge_cursor(space, options)
}

/// Open a property batch reader through a storage client.
///
/// Returns a reader bound to the current transaction snapshot.
pub fn open_property_batch_reader(
    storage: &Arc<RwLock<dyn StorageClient>>,
    space: impl Into<String>,
    read_timestamp: Timestamp,
) -> Box<dyn PropertyBatchReader> {
    Box::new(DefaultPropertyBatchReader::new(
        storage.clone(),
        space.into(),
        read_timestamp,
    ))
}

/// Open an index scan cursor through a storage client.
///
/// Returns a cursor that yields row IDs for the given index and predicate.
/// When the index is covering, the cursor yields full rows directly.
///
/// # Note
/// This is a placeholder.  Storage engines should override
/// `StorageReader::create_index_cursor` when they support native index
/// cursors.  The default implementation returns a capability error.
pub fn open_index_cursor<S: crate::storage::StorageReader + ?Sized>(
    storage: &Arc<RwLock<S>>,
    plan: &IndexScanPlan,
) -> Result<Box<dyn IndexCursor<Row = IndexRow>>, StorageError> {
    let reader = storage.read();
    reader.create_index_cursor(plan)
}

// ---------------------------------------------------------------------------
// Default (Vec-backed) PropertyBatchReader implementation
// ---------------------------------------------------------------------------

/// Default property batch reader that performs sequential `get_vertex` /
/// `get_edge` calls through the storage client.
#[derive(Debug)]
pub struct DefaultPropertyBatchReader {
    storage: Arc<RwLock<dyn StorageClient>>,
    space: String,
    read_timestamp: Timestamp,
}

impl DefaultPropertyBatchReader {
    pub fn new(
        storage: Arc<RwLock<dyn StorageClient>>,
        space: impl Into<String>,
        read_timestamp: Timestamp,
    ) -> Self {
        Self {
            storage,
            space: space.into(),
            read_timestamp,
        }
    }
}

impl PropertyBatchReader for DefaultPropertyBatchReader {
    fn read_vertex_props(
        &self,
        ids: &[VertexId],
        prop_names: &[String],
    ) -> Result<Vec<Vec<crate::core::Value>>, StorageError> {
        let guard = self.storage.read();
        validate_property_reader_context(&*guard, self.read_timestamp)?;
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            match guard.get_vertex(&self.space, id) {
                Ok(Some(vertex)) => {
                    let props = prop_names
                        .iter()
                        .map(|name| {
                            vertex
                                .get_property_any(name)
                                .cloned()
                                .unwrap_or(crate::core::Value::Null(NullType::Null))
                        })
                        .collect();
                    results.push(props);
                }
                Ok(None) => {
                    results.push(vec![
                        crate::core::Value::Null(NullType::Null);
                        prop_names.len()
                    ]);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(results)
    }

    fn read_edge_props(
        &self,
        edges: &[(VertexId, VertexId, String, i64)],
        prop_names: &[String],
    ) -> Result<Vec<Vec<crate::core::Value>>, StorageError> {
        let guard = self.storage.read();
        validate_property_reader_context(&*guard, self.read_timestamp)?;
        let mut results = Vec::with_capacity(edges.len());
        for (src, dst, edge_type, rank) in edges {
            match guard.get_edge(&self.space, src, dst, edge_type, *rank) {
                Ok(Some(edge)) => {
                    let props = prop_names
                        .iter()
                        .map(|name| {
                            edge.get_property(name)
                                .cloned()
                                .unwrap_or(crate::core::Value::Null(NullType::Null))
                        })
                        .collect();
                    results.push(props);
                }
                Ok(None) => {
                    results.push(vec![
                        crate::core::Value::Null(NullType::Null);
                        prop_names.len()
                    ]);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(results)
    }
}

fn validate_property_reader_context(
    storage: &dyn StorageClient,
    read_timestamp: Timestamp,
) -> Result<(), StorageError> {
    let context = storage.operation_context().ok_or_else(|| {
        StorageError::invalid_operation(
            "property batch reader requires a storage operation context".to_string(),
        )
    })?;
    if context.read_timestamp != read_timestamp {
        return Err(StorageError::invalid_operation(format!(
            "property batch reader timestamp {} does not match storage snapshot {}",
            read_timestamp, context.read_timestamp
        )));
    }
    Ok(())
}
