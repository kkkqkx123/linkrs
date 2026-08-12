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
    /// Optional vertex ID range filter over the **external** vertex ID
    /// (the same i64 domain as `PartitionSpec` ranges). Only vertices whose
    /// external ID falls in this range (inclusive of start, exclusive of
    /// end) are returned. When set, this filter is applied at scan time,
    /// not as a post-filter.
    pub vertex_id_range: Option<std::ops::Range<i64>>,
    /// Optional edge source ID range filter. Only edges whose source ID
    /// (parsed as `i64`) falls in this range are returned.
    pub edge_src_id_range: Option<std::ops::Range<i64>>,
    /// Edge type filter (for edge scans only).
    pub edge_type: Option<String>,
    /// Optional tag filter for vertex scans: only rows whose tag matches
    /// this name are scanned.  The tag tables of all other tags are skipped
    /// at scan time (the query layer may therefore elide the matching
    /// `contains(labels(v), ...)` residual conjunct).
    pub tag: Option<String>,
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
    /// Column-block scan mode: pull column-major batches from the storage
    /// cursor via `VertexCursor::next_column_batch` instead of row-major
    /// batches. Default off — the row-based path stays the fallback.
    pub column_block_mode: bool,
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

    /// Builder: enable column-block scan mode.
    pub fn with_column_block_mode(mut self, enabled: bool) -> Self {
        self.column_block_mode = enabled;
        self
    }

    /// Builder: restrict a vertex scan to a single tag by name.
    pub fn with_tag(mut self, tag: String) -> Self {
        self.tag = Some(tag);
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

    /// Columnar variant of [`matches`](Self::matches): evaluate against one
    /// [`ColumnValues`] at row `idx`.  A null value never matches, mirroring
    /// the row-based NULL semantics.
    pub fn matches_column(&self, column: &ColumnValues, idx: usize) -> bool {
        let Some(value) = column.value_at(idx) else {
            return false;
        };
        self.matches_scalar(&value)
    }

    /// Evaluate the predicate against a single decoded value.
    fn matches_scalar(&self, value: &crate::core::Value) -> bool {
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
// Column-major vertex batch (A1 column-block path)
// ---------------------------------------------------------------------------

/// Raw decoded values for one property column, in column-major order.
///
/// Fixed-size scalar columns (BigInt/Double/Int) are returned as dense typed
/// vectors plus a validity bitmap (`valid[i] == 1` means the value is
/// present, not null).  Everything else (strings, mixed, other types) falls
/// back to per-row decoded `Option<Value>`.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnValues {
    I64 { values: Vec<i64>, valid: Vec<u8> },
    F64 { values: Vec<f64>, valid: Vec<u8> },
    I32 { values: Vec<i32>, valid: Vec<u8> },
    General(Vec<Option<crate::core::Value>>),
}

impl ColumnValues {
    /// Number of rows in this column.
    pub fn len(&self) -> usize {
        match self {
            ColumnValues::I64 { values, .. } => values.len(),
            ColumnValues::F64 { values, .. } => values.len(),
            ColumnValues::I32 { values, .. } => values.len(),
            ColumnValues::General(values) => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The decoded value at row `idx` (None for null / missing).
    pub fn value_at(&self, idx: usize) -> Option<crate::core::Value> {
        match self {
            ColumnValues::I64 { values, valid } => {
                if valid.get(idx).copied().unwrap_or(0) == 1 {
                    values.get(idx).map(|&v| crate::core::Value::BigInt(v))
                } else {
                    None
                }
            }
            ColumnValues::F64 { values, valid } => {
                if valid.get(idx).copied().unwrap_or(0) == 1 {
                    values.get(idx).map(|&v| crate::core::Value::Double(v))
                } else {
                    None
                }
            }
            ColumnValues::I32 { values, valid } => {
                if valid.get(idx).copied().unwrap_or(0) == 1 {
                    values.get(idx).map(|&v| crate::core::Value::Int(v))
                } else {
                    None
                }
            }
            ColumnValues::General(values) => values.get(idx).cloned().flatten(),
        }
    }

    /// Append another column's rows (same kind). Kind mismatches are resolved
    /// by degrading both sides to `General`, except when the target is an
    /// empty `General` — then the source's typed kind is adopted so the
    /// first table's decode stays typed.
    pub fn append(&mut self, other: ColumnValues) {
        // Adopt the source's typed kind when the target is an empty `General`
        // column so the first decoded run keeps its typed layout.
        if matches!(self, ColumnValues::General(values) if values.is_empty()) {
            *self = other;
            return;
        }
        match (self, other) {
            (
                ColumnValues::I64 { values, valid },
                ColumnValues::I64 {
                    values: v2,
                    valid: v2v,
                },
            ) => {
                values.extend(v2);
                valid.extend(v2v);
            }
            (
                ColumnValues::F64 { values, valid },
                ColumnValues::F64 {
                    values: v2,
                    valid: v2v,
                },
            ) => {
                values.extend(v2);
                valid.extend(v2v);
            }
            (
                ColumnValues::I32 { values, valid },
                ColumnValues::I32 {
                    values: v2,
                    valid: v2v,
                },
            ) => {
                values.extend(v2);
                valid.extend(v2v);
            }
            (ColumnValues::General(values), ColumnValues::General(values2)) => {
                values.extend(values2);
            }
            (self_col, other) => {
                let mut general = self_col.to_general();
                general.extend(other.to_general());
                *self_col = ColumnValues::General(general);
            }
        }
    }

    /// Append `n` null rows (used when merging columns across tables that
    /// lack a column).
    pub fn append_nulls(&mut self, n: usize) {
        match self {
            ColumnValues::I64 { values, valid } => {
                values.resize(values.len() + n, 0);
                valid.resize(valid.len() + n, 0);
            }
            ColumnValues::F64 { values, valid } => {
                values.resize(values.len() + n, 0.0);
                valid.resize(valid.len() + n, 0);
            }
            ColumnValues::I32 { values, valid } => {
                values.resize(values.len() + n, 0);
                valid.resize(valid.len() + n, 0);
            }
            ColumnValues::General(values) => {
                values.resize(values.len() + n, None);
            }
        }
    }

    /// Truncate the column to the first `n` rows.
    pub fn truncate(&mut self, n: usize) {
        match self {
            ColumnValues::I64 { values, valid } => {
                values.truncate(n);
                valid.truncate(n);
            }
            ColumnValues::F64 { values, valid } => {
                values.truncate(n);
                valid.truncate(n);
            }
            ColumnValues::I32 { values, valid } => {
                values.truncate(n);
                valid.truncate(n);
            }
            ColumnValues::General(values) => values.truncate(n),
        }
    }

    /// Compress the column to the rows where `keep[i]` is true, in order.
    pub fn compact(&mut self, keep: &[bool]) {
        match self {
            ColumnValues::I64 { values, valid } => {
                let mut write = 0;
                for (i, &k) in keep.iter().enumerate() {
                    if k {
                        values[write] = values[i];
                        valid[write] = valid[i];
                        write += 1;
                    }
                }
                values.truncate(write);
                valid.truncate(write);
            }
            ColumnValues::F64 { values, valid } => {
                let mut write = 0;
                for (i, &k) in keep.iter().enumerate() {
                    if k {
                        values[write] = values[i];
                        valid[write] = valid[i];
                        write += 1;
                    }
                }
                values.truncate(write);
                valid.truncate(write);
            }
            ColumnValues::I32 { values, valid } => {
                let mut write = 0;
                for (i, &k) in keep.iter().enumerate() {
                    if k {
                        values[write] = values[i];
                        valid[write] = valid[i];
                        write += 1;
                    }
                }
                values.truncate(write);
                valid.truncate(write);
            }
            ColumnValues::General(values) => {
                let mut write = 0;
                for (i, &k) in keep.iter().enumerate() {
                    if k {
                        values[write] = values[i].take();
                        write += 1;
                    }
                }
                values.truncate(write);
            }
        }
    }

    /// Convert to a `General` per-row `Option<Value>` column.
    pub fn to_general(&self) -> Vec<Option<crate::core::Value>> {
        (0..self.len()).map(|i| self.value_at(i)).collect()
    }

    /// Scatter this column's rows into `target` at the given output
    /// positions (used when merging per-shard decodes back into input order).
    /// `positions[i]` is `(out_idx, local_id)`; the local id is ignored here.
    /// `target` must be a `General` column pre-sized to the merged row count.
    pub fn scatter(&self, target: &mut ColumnValues, positions: &[(usize, u32)]) {
        let ColumnValues::General(target_rows) = target else {
            return;
        };
        for (i, &(out_idx, _)) in positions.iter().enumerate() {
            if let Some(value) = self.value_at(i) {
                target_rows[out_idx] = Some(value);
            }
        }
    }

    /// Convert a `General` per-row column into a typed column when every value
    /// matches the column's declared scalar kind (or is null).  Returns `None`
    /// when the declared type does not map to a typed kind or values disagree.
    pub fn from_general_with_type(
        values: Vec<Option<crate::core::Value>>,
        data_type: &DataType,
    ) -> Option<ColumnValues> {
        match data_type {
            DataType::BigInt => {
                let mut vs = Vec::with_capacity(values.len());
                let mut valid = vec![0u8; values.len()];
                for (i, value) in values.into_iter().enumerate() {
                    match value {
                        Some(crate::core::Value::BigInt(v)) => {
                            vs.push(v);
                            valid[i] = 1;
                        }
                        None => vs.push(0),
                        Some(_) => return None,
                    }
                }
                Some(ColumnValues::I64 { values: vs, valid })
            }
            DataType::Double => {
                let mut vs = Vec::with_capacity(values.len());
                let mut valid = vec![0u8; values.len()];
                for (i, value) in values.into_iter().enumerate() {
                    match value {
                        Some(crate::core::Value::Double(v)) => {
                            vs.push(v);
                            valid[i] = 1;
                        }
                        None => vs.push(0.0),
                        Some(_) => return None,
                    }
                }
                Some(ColumnValues::F64 { values: vs, valid })
            }
            DataType::Int => {
                let mut vs = Vec::with_capacity(values.len());
                let mut valid = vec![0u8; values.len()];
                for (i, value) in values.into_iter().enumerate() {
                    match value {
                        Some(crate::core::Value::Int(v)) => {
                            vs.push(v);
                            valid[i] = 1;
                        }
                        None => vs.push(0),
                        Some(_) => return None,
                    }
                }
                Some(ColumnValues::I32 { values: vs, valid })
            }
            _ => None,
        }
    }

    /// Whether every row is non-null (so the typed fast path can be used
    /// without a validity bitmap).
    pub fn all_valid(&self) -> bool {
        match self {
            ColumnValues::I64 { valid, .. }
            | ColumnValues::F64 { valid, .. }
            | ColumnValues::I32 { valid, .. } => valid.iter().all(|&v| v == 1),
            ColumnValues::General(values) => values.iter().all(|v| v.is_some()),
        }
    }
}

/// Default data type for a fallback `General` column: scan the decoded values
/// and pick the type of the first non-null / non-empty row; when every value
/// is missing keep [`DataType::Empty`] (the upstream typed-column path then
/// degrades to `TypedColumn::Fallback`, matching the pre-existing behavior).
///
/// This is the analog of what the storage-side `GraphVertexCursor` does in
/// `assemble_column_batch` (`cursor_impl.rs`); keeping the trait default
/// aligned with the real engine path means non-GraphStorage engines
/// (`VecVertexCursor`, `VecEdgeCursor`) get the same typed-column coverage
/// when they opt into the column-block scan path.
fn column_data_type(values: &[Option<crate::core::Value>]) -> DataType {
    for v in values.iter().flatten() {
        let ty = v.get_type();
        if !matches!(ty, DataType::Empty | DataType::Null) {
            return ty;
        }
    }
    DataType::Empty
}

/// One property column of a column-major vertex batch.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyColumn {
    pub name: String,
    pub data_type: DataType,
    pub values: ColumnValues,
}

/// A column-major vertex batch produced by `VertexCursor::next_column_batch`.
///
/// Rows are implicit: every column (and `vids`/`internal_ids`) has the same
/// length.  `columns` holds one entry per requested property in projection
/// order; when the scan requests a full-row decode (empty projection) it
/// holds every column of the scanned table(s).
#[derive(Debug, Clone, PartialEq)]
pub struct VertexColumnBatch {
    pub vids: Vec<crate::core::types::VertexId>,
    pub internal_ids: Vec<i64>,
    /// Tag (label) name per row (batches may span tables).
    pub tag_names: Vec<String>,
    pub columns: Vec<PropertyColumn>,
}

impl VertexColumnBatch {
    pub fn empty() -> Self {
        Self {
            vids: Vec::new(),
            internal_ids: Vec::new(),
            tag_names: Vec::new(),
            columns: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.vids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.vids.len()
    }
}

/// A column-major edge batch produced by `EdgeCursor::next_column_batch`.
///
/// Rows are implicit: every column (and `srcs`/`dsts`/`edge_types`/
/// `rankings`) has the same length.  `columns` holds one entry per requested
/// property in projection order.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeColumnBatch {
    pub srcs: Vec<crate::core::types::VertexId>,
    pub dsts: Vec<crate::core::types::VertexId>,
    pub edge_types: Vec<String>,
    pub rankings: Vec<i64>,
    pub columns: Vec<PropertyColumn>,
}

impl EdgeColumnBatch {
    pub fn empty() -> Self {
        Self {
            srcs: Vec::new(),
            dsts: Vec::new(),
            edge_types: Vec::new(),
            rankings: Vec::new(),
            columns: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.srcs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.srcs.len()
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

    /// Read the next batch as column-major data (at most `batch_size` rows),
    /// skipping per-row `VertexRecord` / `HashMap` / `Value` materialization.
    ///
    /// `prop_names` lists the properties to decode; an empty list means decode
    /// every column of the scanned table(s).  The returned batch carries one
    /// [`PropertyColumn`] per requested property (or per table column when
    /// `prop_names` is empty).
    ///
    /// The default implementation falls back to [`Self::next_flat_batch`] and
    /// transposes the rows into columns.  Storage engines override this when
    /// they can decode straight from the column store.
    ///
    /// Returns an empty batch when the scan is exhausted.
    fn next_column_batch(
        &mut self,
        prop_names: &[String],
        batch_size: usize,
    ) -> Result<VertexColumnBatch, StorageError> {
        let records = self.next_flat_batch(batch_size)?;
        let row_count = records.len();
        let mut batch = VertexColumnBatch {
            vids: Vec::with_capacity(row_count),
            internal_ids: Vec::with_capacity(row_count),
            tag_names: Vec::with_capacity(row_count),
            columns: Vec::with_capacity(prop_names.len()),
        };
        let mut per_name: Vec<Vec<Option<crate::core::Value>>> = prop_names
            .iter()
            .map(|_| Vec::with_capacity(row_count))
            .collect();
        for record in records {
            batch.tag_names.push(record.tag_name.clone());
            batch.vids.push(record.vid);
            batch.internal_ids.push(record.internal_id);
            for (i, name) in prop_names.iter().enumerate() {
                let value = record
                    .props
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v.clone());
                per_name[i].push(value);
            }
        }
        for (i, name) in prop_names.iter().enumerate() {
            batch.columns.push(PropertyColumn {
                name: name.clone(),
                data_type: column_data_type(&per_name[i]),
                values: ColumnValues::General(std::mem::take(&mut per_name[i])),
            });
        }
        Ok(batch)
    }
}

/// A cursor that yields edges in batches.
pub trait EdgeCursor: Send + std::fmt::Debug {
    /// Read the next batch of edges (at most `batch_size` rows).
    ///
    /// Returns an empty `Vec` when the scan is exhausted.
    fn next_batch(&mut self, batch_size: usize) -> Result<Vec<crate::core::Edge>, StorageError>;

    /// Read the next batch as column-major data (at most `batch_size` rows).
    ///
    /// `prop_names` lists the properties to decode; an empty list means decode
    /// every property of the edge.  The returned batch carries one
    /// [`PropertyColumn`] per requested property.
    ///
    /// The default implementation falls back to [`Self::next_batch`] and
    /// transposes the rows into columns.  Storage engines override this when
    /// they can decode straight from the column store.
    ///
    /// Returns an empty batch when the scan is exhausted.
    fn next_column_batch(
        &mut self,
        prop_names: &[String],
        batch_size: usize,
    ) -> Result<EdgeColumnBatch, StorageError> {
        let edges = self.next_batch(batch_size)?;
        let row_count = edges.len();
        let mut batch = EdgeColumnBatch {
            srcs: Vec::with_capacity(row_count),
            dsts: Vec::with_capacity(row_count),
            edge_types: Vec::with_capacity(row_count),
            rankings: Vec::with_capacity(row_count),
            columns: Vec::with_capacity(prop_names.len()),
        };
        let mut per_name: Vec<Vec<Option<crate::core::Value>>> = prop_names
            .iter()
            .map(|_| Vec::with_capacity(row_count))
            .collect();
        for edge in edges {
            batch.srcs.push(edge.src);
            batch.dsts.push(edge.dst);
            batch.edge_types.push(edge.edge_type);
            batch.rankings.push(edge.ranking);
            for (i, name) in prop_names.iter().enumerate() {
                per_name[i].push(edge.props.get(name).cloned());
            }
        }
        for (i, name) in prop_names.iter().enumerate() {
            batch.columns.push(PropertyColumn {
                name: name.clone(),
                data_type: column_data_type(&per_name[i]),
                values: ColumnValues::General(std::mem::take(&mut per_name[i])),
            });
        }
        Ok(batch)
    }
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
/// Requires the storage engine's native lazy cursor (via
/// [`StorageReader::create_vertex_cursor`]); storage engines without one
/// report a capability error.  The [`VecVertexCursor`] type is only used by
/// adapters and test doubles, not as an implicit fallback.
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
/// Requires the storage engine's native lazy cursor (via
/// [`StorageReader::create_edge_cursor`]); storage engines without one
/// report a capability error.  The [`VecEdgeCursor`] type is only used by
/// adapters and test doubles, not as an implicit fallback.
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
