//! Immutable configuration for source operators.

use std::sync::Arc;

use crate::executor::streaming::slot::SlotLayout;
use crate::storage::ScanPredicate;
use graphdb_core::types::expr::ContextualExpression;
use graphdb_core::Value;

/// A predicate that has been validated and bound to an index schema.
///
/// Created at plan-build time from the logical plan's `IndexLimit` / filter.
#[derive(Debug, Clone)]
pub enum BoundIndexPredicate {
    /// Exact equality: `column = value`
    Equal { column: String, value: Value },
    /// Range scan: `column BETWEEN begin AND end`
    Range {
        column: String,
        begin: Option<Value>,
        end: Option<Value>,
        include_begin: bool,
        include_end: bool,
    },
    /// Prefix scan (for string columns): `column STARTS WITH prefix`
    Prefix { column: String, prefix: Value },
    /// Full scan (no predicate, all entries)
    Full,
}

/// Describes which columns the index should return.
///
/// A covering index can satisfy the projection directly; a non-covering
/// index requires back-to-table fetches.
#[derive(Debug, Clone)]
pub enum IndexProjection {
    /// Return only the row ID (back-to-table required).
    RowIdOnly,
    /// Return specific columns from the index (covering if the index
    /// includes all of them).
    Columns(Vec<String>),
    /// Return all indexed columns.
    AllColumns,
}

/// Immutable config for source operators.
///
/// Mutable state (`cursor`, `buffer`, `current_index`, `partition_id`,
/// `partition_range`) lives in [`SourceState`](super::super::state::SourceState).
#[derive(Debug, Clone)]
pub enum SourceSpec {
    ScanVertices {
        rows: Vec<Vec<Value>>,
        col_names: Vec<String>,
    },
    /// Standalone DML values — evaluated once per execution in the source
    /// operator so that volatile expressions (e.g. `now()`) are resolved at
    /// execution time, not at plan build time.
    StandaloneValues {
        values: Vec<Vec<ContextualExpression>>,
        col_names: Vec<String>,
    },
    StorageScanVertices {
        space_name: String,
        limit: Option<usize>,
        col_names: Vec<String>,
        projected_properties: Vec<String>,
        /// Scan predicates pushed into the storage layer (pure pre-filter;
        /// the original filter still runs on top).
        predicate: Vec<ScanPredicate>,
        /// Tag-restricted scan: only rows of this tag are scanned at the
        /// storage layer (the matching `contains(labels(v), ...)` residual
        /// conjunct may then be elided).
        tag: Option<String>,
        /// Optional vertex-id range restricting the scan to one partition.
        /// Set only on per-partition copies produced by the partitioned
        /// physical-plan builder; `None` means a full scan.
        partition_range: Option<std::ops::Range<i64>>,
    },
    ScanEdges {
        rows: Vec<Vec<Value>>,
        col_names: Vec<String>,
    },
    StorageScanEdges {
        space_name: String,
        limit: Option<usize>,
        edge_type: Option<String>,
        col_names: Vec<String>,
        projected_properties: Vec<String>,
        /// Scan predicates pushed into the storage layer (pure pre-filter;
        /// the original filter still runs on top).
        predicate: Vec<ScanPredicate>,
        /// Optional source-id range restricting the scan to one partition.
        partition_range: Option<std::ops::Range<i64>>,
    },
    GetVertices {
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
        projected_properties: Vec<String>,
        /// Entity variable name for the fetched vertex (e.g. the tag name).
        col_names: Vec<String>,
    },
    GetEdges {
        space_name: String,
        edge_type: Option<String>,
        src: Option<String>,
        dst: Option<String>,
        rank: i64,
        /// Property names to keep on the materialized edge; empty reads all.
        projected_properties: Vec<String>,
    },
    GetNeighbors {
        space_name: String,
        direction: String,
        projected_properties: Vec<String>,
    },
    /// Index scan with typed predicate and projection.
    ///
    /// The predicate and projection are validated at build time against
    /// the index schema.  Stale row IDs are skipped, not treated as EOF.
    IndexScan {
        space_name: String,
        index_name: String,
        index_id: u64,
        predicate: Box<BoundIndexPredicate>,
        projection: IndexProjection,
        residual_filter: Option<graphdb_core::types::expr::Expression>,
        output_layout: Arc<SlotLayout>,
    },
    /// Produces one singleton row from which correlated apply can pull.
    /// `col_names` mirrors the outer (left) layout so the emitted chunk slots
    /// align with the correlation frame read at runtime.
    Argument { col_names: Vec<String> },
    /// Unary property retrieval: reads properties from the input entity IDs.
    ///
    /// Unlike the source-variant GetProp (which is a zero-input stub),
    /// this unary variant takes an input child and produces one output
    /// row per input row with additional property columns.
    GetProp {
        space_name: String,
        /// Slot number of the entity (vertex or edge) column in the input chunk.
        entity_slot: usize,
        /// Property names to read.
        prop_names: Vec<String>,
        /// Whether the entity is a vertex or edge.
        is_vertex: bool,
        /// Output layout (input columns + new property columns).
        output_layout: Arc<SlotLayout>,
    },
    /// Produces one zero-column row on first `next()`, then `None`.
    /// Used as the seed for command-type operators (DDL, DML, etc.)
    /// that require a source but have no real input.
    Start,
}
