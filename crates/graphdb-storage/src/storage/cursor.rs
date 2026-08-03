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

use crate::core::types::{DataType, Timestamp};
use crate::core::StorageError;

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
// Required property (typed projection)
// ---------------------------------------------------------------------------

/// Carries resolved metadata so that scan operators and storage cursors
/// no longer rely on alias/name heuristics.  The `schema_version` binds
/// the identity to a specific catalog generation, preventing stale reuse
/// after schema changes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequiredProperty {
    /// Property name (column name in storage).
    pub name: String,
    /// Resolved column index in the target `ColumnStore`, if known.
    pub column_id: Option<i32>,
    /// Data type from schema binding.
    pub data_type: Option<DataType>,
    /// Schema version at binding time.
    pub schema_version: u64,
}

impl RequiredProperty {
    pub fn new(name: String) -> Self {
        Self {
            name,
            column_id: None,
            data_type: None,
            schema_version: 0,
        }
    }

    pub fn with_metadata(
        name: String,
        column_id: Option<i32>,
        data_type: Option<DataType>,
        schema_version: u64,
    ) -> Self {
        Self {
            name,
            column_id,
            data_type,
            schema_version,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn column_id(&self) -> Option<i32> {
        self.column_id
    }

    pub fn data_type(&self) -> Option<&DataType> {
        self.data_type.as_ref()
    }

    pub fn schema_version(&self) -> u64 {
        self.schema_version
    }
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
    pub projection: Option<Vec<RequiredProperty>>,
    /// Read timestamp captured by the caller.
    pub read_timestamp: Option<Timestamp>,
    /// Optional conjunctive scan predicates pushed from the query layer.
    ///
    /// All predicates must match for a row to be emitted.  The query layer
    /// keeps the original filter on top, so the pushdown is a pure
    /// pre-filter (see [`ScanPredicate`]).
    pub predicate: Option<Vec<ScanPredicate>>,
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

    pub fn with_projection_named(mut self, projection: Vec<String>) -> Self {
        self.projection = Some(projection.into_iter().map(RequiredProperty::new).collect());
        self
    }

    pub fn with_projection(mut self, projection: Vec<RequiredProperty>) -> Self {
        self.projection = Some(projection);
        self
    }

    pub fn with_read_timestamp(mut self, read_timestamp: Timestamp) -> Self {
        self.read_timestamp = Some(read_timestamp);
        self
    }

    /// Builder: set pushed scan predicates (conjunction semantics).
    pub fn with_predicate(mut self, predicates: Vec<ScanPredicate>) -> Self {
        self.predicate = Some(predicates);
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

/// A single-column comparison predicate pushed from the query layer into a
/// physical scan.
///
/// This is the whitelist of filter conjuncts the planner can push into the
/// storage layer.  A list of predicates forms a conjunction (every predicate
/// must match).  Rows with a missing property never match, mirroring the
/// query engine's NULL semantics where comparisons against NULL are false.
/// The original filter expression still runs on top of the scan, so the
/// pushdown is a pure pre-filter and can never change results.
#[derive(Debug, Clone, PartialEq)]
pub enum ScanPredicate {
    /// `column = value`
    ColumnEqual {
        column: String,
        value: crate::core::Value,
    },
    /// `column` bounded by constants (either bound may be absent).
    ColumnRange {
        column: String,
        lower: Option<crate::core::Value>,
        upper: Option<crate::core::Value>,
        include_lower: bool,
        include_upper: bool,
    },
}

impl ScanPredicate {
    /// Whether the predicate matches the given property set.
    ///
    /// Properties are a `(name, value)` slice in projection order.  A
    /// missing column (or any non-scalar comparison) never matches.
    pub fn matches(&self, props: &[(String, crate::core::Value)]) -> bool {
        let Some(value) = props
            .iter()
            .find(|(name, _)| name == self.column())
            .map(|(_, v)| v)
        else {
            return false;
        };
        match self {
            ScanPredicate::ColumnEqual {
                value: expected, ..
            } => compare_scalar(value, expected) == std::cmp::Ordering::Equal,
            ScanPredicate::ColumnRange {
                lower,
                upper,
                include_lower,
                include_upper,
                ..
            } => {
                if let Some(lower) = lower {
                    let ord = compare_scalar(value, lower);
                    let passes = if *include_lower {
                        ord != std::cmp::Ordering::Less
                    } else {
                        ord == std::cmp::Ordering::Greater
                    };
                    if !passes {
                        return false;
                    }
                }
                if let Some(upper) = upper {
                    let ord = compare_scalar(value, upper);
                    let passes = if *include_upper {
                        ord != std::cmp::Ordering::Greater
                    } else {
                        ord == std::cmp::Ordering::Less
                    };
                    if !passes {
                        return false;
                    }
                }
                true
            }
        }
    }

    /// The property column this predicate compares.
    pub fn column(&self) -> &str {
        match self {
            ScanPredicate::ColumnEqual { column, .. } => column,
            ScanPredicate::ColumnRange { column, .. } => column,
        }
    }
}

/// Compare two scalar values for a pushed predicate.
///
/// Integer kinds are compared exactly as `i64`; any numeric pair involving a
/// float is compared as `f64` (mirroring the query engine's typed batch
/// evaluation); everything else falls back to `Value` ordering.
fn compare_scalar(a: &crate::core::Value, b: &crate::core::Value) -> std::cmp::Ordering {
    match (as_i64(a), as_i64(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        _ => match (as_f64(a), as_f64(b)) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
            _ => crate::core::Value::cmp(a, b),
        },
    }
}

fn as_i64(value: &crate::core::Value) -> Option<i64> {
    match value {
        crate::core::Value::SmallInt(v) => Some(*v as i64),
        crate::core::Value::Int(v) => Some(*v as i64),
        crate::core::Value::BigInt(v) => Some(*v),
        _ => None,
    }
}

fn as_f64(value: &crate::core::Value) -> Option<f64> {
    match value {
        crate::core::Value::Float(v) => Some(*v as f64),
        crate::core::Value::Double(v) => Some(*v),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PartitionSelector {
    #[default]
    All,
    Shards(Vec<u32>),
    KeyRange {
        lower: Option<Vec<u8>>,
        upper: Option<Vec<u8>>,
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
    pub partition: PartitionSelector,
    /// Optional vertex/edge ID range for partition-based shard selection.
    /// Forwarded from `PartitionView`; the storage layer converts this to
    /// precise key bounds using index metadata.
    pub partition_id_range: Option<std::ops::Range<i64>>,
    pub projection: Option<Vec<String>>,
    pub limit: Option<usize>,
    pub offset: usize,
    pub read_timestamp: Timestamp,
}

// ---------------------------------------------------------------------------
// Flat vertex record (scan boundary bypassing Vertex/HashMap boxing)
// ---------------------------------------------------------------------------

/// A vertex row read from the storage columns without `Vertex`/`HashMap`
/// boxing.
///
/// The properties are a plain `Vec<(String, Value)>` in storage projection
/// order. The query layer rebuilds the `Value::Vertex` slot 0 only when a
/// consumer actually needs the entity (graph operators, `RETURN p`, label
/// checks), skipping the per-row `HashMap` construction at the storage
/// boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct FlatVertexRecord {
    /// External vertex ID.
    pub vid: crate::core::types::VertexId,
    /// Internal (storage) vertex ID.
    pub internal_id: i64,
    /// Tag (label) name of the scanned table.
    pub tag_name: String,
    /// Projected properties in storage order.
    pub props: Vec<(String, crate::core::Value)>,
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

    /// Read the next batch as flat vertex records (at most `batch_size`
    /// rows), skipping `Vertex` construction and `HashMap` boxing.
    ///
    /// The default implementation materialises vertices via [`Self::next_batch`]
    /// and converts them. Storage engines should override this when they can
    /// produce records directly from the column store.
    ///
    /// Returns an empty `Vec` when the scan is exhausted.
    fn next_flat_batch(
        &mut self,
        batch_size: usize,
    ) -> Result<Vec<FlatVertexRecord>, StorageError> {
        Ok(self
            .next_batch(batch_size)?
            .into_iter()
            .map(|v| FlatVertexRecord {
                vid: v.vid,
                internal_id: v.id,
                tag_name: v.tags.first().map(|t| t.name.clone()).unwrap_or_default(),
                props: v.properties.into_iter().collect(),
            })
            .collect())
    }
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

    /// Number of stale rows skipped so far (for diagnostics).
    fn stale_skipped(&self) -> u64 {
        0
    }

    /// Number of invisible (MVCC-hidden) entries skipped so far.
    fn invisible_skipped(&self) -> u64 {
        0
    }

    /// Number of malformed/unparseable entries skipped so far.
    fn malformed_skipped(&self) -> u64 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::storage_ids::VertexId;
    use crate::core::{Tag, Value, Vertex};
    use std::collections::HashMap;

    #[test]
    fn next_flat_batch_default_converts_vertices() {
        let mut vertex = Vertex::with_vid(VertexId::from_int64(1));
        vertex.id = 7;
        let mut props = HashMap::new();
        props.insert("age".to_string(), Value::BigInt(30));
        vertex.add_tag(Tag::new("person".to_string(), props));
        vertex
            .properties
            .insert("age".to_string(), Value::BigInt(30));

        let mut cursor = VecVertexCursor::new(vec![vertex]);
        let batch = cursor
            .next_flat_batch(10)
            .expect("flat batch should succeed");
        assert_eq!(batch.len(), 1);
        let rec = &batch[0];
        assert_eq!(rec.vid, VertexId::from_int64(1));
        assert_eq!(rec.internal_id, 7);
        assert_eq!(rec.tag_name, "person");
        assert_eq!(rec.props, vec![("age".to_string(), Value::BigInt(30))]);
    }

    #[test]
    fn next_flat_batch_empty_when_exhausted() {
        let mut cursor = VecVertexCursor::new(Vec::new());
        let batch = cursor
            .next_flat_batch(10)
            .expect("flat batch should succeed");
        assert!(batch.is_empty());
    }
}
