//! OperatorSpec: Immutable configuration descriptors for operator nodes.
//!
//! Each variant holds only the immutable fields of a corresponding operator
//! — expressions, configuration values, column names — but never cursors,
//! hash tables, buffers, or lifecycle state.  This makes an `OperatorSpec`
//! suitable for caching, EXPLAIN, and repeated instantiation without shared
//! mutable state.
//!
//! Phase 2 pilot: Source, Filter, Project, Limit, Sort, HashJoin.
//! Remaining operators will be migrated in follow-up phases.

use std::sync::Arc;

use crate::core::types::expr::{ContextualExpression, Expression};
use crate::core::types::operators::AggregateFunction;
use crate::core::types::user::PasswordInfo;
use crate::core::types::PropertyDef;
use crate::core::{EdgeDirection, Value};
use crate::query::executor::streaming::executor::SortDirection;
use crate::query::executor::streaming::plan::types::PhysicalPlan;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::query::parser::ast::vector::VectorDistance;
use crate::storage::ScanPredicate;

// ── Bound index predicate types ─────────────────────────────────────────────

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

// ── Source spec ──────────────────────────────────────────────────────────────

/// Immutable config for source operators.
///
/// Mutable state (`cursor`, `buffer`, `current_index`, `partition_id`,
/// `partition_range`) lives in [`SourceState`](super::state::SourceState).
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
        residual_filter: Option<crate::core::types::expr::Expression>,
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

// ── Unary spec ───────────────────────────────────────────────────────────────

/// Immutable config for unary (one-input) operators.
#[derive(Debug, Clone)]
pub enum UnarySpec {
    Filter {
        predicate: Expression,
        /// Expression-level subqueries compiled for this filter;
        /// the materializer turns them into a per-operator `SubqueryExecutor`.
        subquery_runners: Vec<crate::query::executor::streaming::subquery::SubqueryRunnerSpec>,
    },
    Project {
        output_expressions: Vec<Expression>,
        output_col_names: Vec<String>,
        /// Expression-level subqueries compiled for this project.
        subquery_runners: Vec<crate::query::executor::streaming::subquery::SubqueryRunnerSpec>,
    },
    Limit {
        offset: u32,
        limit: u32,
    },
    Assign {
        assignments: Vec<(String, Expression)>,
        /// Expression-level subqueries compiled for this assign.
        subquery_runners: Vec<crate::query::executor::streaming::subquery::SubqueryRunnerSpec>,
    },
    Remove {
        columns_to_remove: Vec<String>,
    },
    Unwind {
        unwind_column: String,
        list_expression: Option<Expression>,
    },
    /// Storage-backed vertex property append.
    ///
    /// Evaluates `entity_expr` per input row to resolve the vertex id, reads
    /// the vertex (full or projected) from storage, and appends the property
    /// columns to the row.  With a non-empty `prop_names` the appended
    /// columns are the flat `{entity_var}.{prop}` names; with an empty list
    /// the whole `Value::Vertex` is appended under `entity_var`.
    AppendVertices {
        /// Space the vertex is read from.
        space_name: String,
        /// Binding variable of the appended vertex (flat-column prefix).
        entity_var: String,
        /// Expression resolved per row to the vertex id.
        entity_expr: Expression,
        /// Property names to read; empty reads the full vertex.
        prop_names: Vec<String>,
    },
    Sample {
        count: u64,
    },
}

impl UnarySpec {
    /// Expression-level subquery runner specs of this operator (empty for
    /// kinds that do not host subqueries). The materializer instantiates a
    /// per-operator `SubqueryExecutor` from these.
    pub fn subquery_runners(
        &self,
    ) -> &[crate::query::executor::streaming::subquery::SubqueryRunnerSpec] {
        match self {
            Self::Filter {
                subquery_runners, ..
            }
            | Self::Project {
                subquery_runners, ..
            }
            | Self::Assign {
                subquery_runners, ..
            } => subquery_runners,
            _ => &[],
        }
    }
}

// ── Blocking spec ────────────────────────────────────────────────────────────

/// Immutable config for blocking operators.
#[derive(Debug, Clone)]
pub enum BlockingSpec {
    Sort {
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
    },
    Aggregate {
        group_by_expressions: Vec<Expression>,
        aggregate_functions: Vec<(AggregateFunction, Expression)>,
        output_col_names: Vec<String>,
    },
    GroupBy {
        group_by_expressions: Vec<Expression>,
    },
    WindowFunction {
        window_exprs: Vec<Expression>,
        partition_by_exprs: Vec<Expression>,
        order_by_exprs: Vec<Expression>,
        order_by_directions: Vec<SortDirection>,
    },
    Window {
        window_exprs: Vec<Expression>,
        partition_by_exprs: Vec<Expression>,
        order_by_exprs: Vec<Expression>,
        order_by_directions: Vec<SortDirection>,
    },
    TopN {
        n: u32,
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
    },
    Distinct,
    Materialize,
    DataCollect,
    RollUpApply {
        rollup_expressions: Vec<Expression>,
    },
    PartialAggregate {
        group_by_expressions: Vec<Expression>,
        aggregate_functions: Vec<AggregateFunction>,
        output_col_names: Vec<String>,
    },
    FinalAggregate {
        group_by_expressions: Vec<Expression>,
        aggregate_functions: Vec<AggregateFunction>,
        output_col_names: Vec<String>,
    },
}

// ── Join spec ────────────────────────────────────────────────────────────────

/// Which physical child of a hash join provides the build side.
///
/// The logical plan always builds from the right child (the default); a left
/// build side is a physical alternative selected by the plan conversion when
/// the right child is not hashable but the left child is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BuildSide {
    Left,
    #[default]
    Right,
}

/// Immutable config for binary join operators.
#[derive(Debug, Clone)]
pub enum JoinSpec {
    InnerJoin {
        join_condition: Option<Expression>,
    },
    LeftJoin {
        join_condition: Option<Expression>,
    },
    RightJoin {
        join_condition: Option<Expression>,
    },
    FullOuterJoin {
        join_condition: Option<Expression>,
    },
    CrossJoin,
    SemiJoin {
        join_condition: Option<Expression>,
    },
    HashJoin {
        join_condition: Option<Expression>,
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
        build_side: BuildSide,
    },
    HashLeftJoin {
        join_condition: Option<Expression>,
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
        build_side: BuildSide,
    },
    NestedLoopJoin {
        join_condition: Option<Expression>,
    },
}

// ── Graph spec ───────────────────────────────────────────────────────────────

/// Immutable config for graph traversal operators.
#[derive(Debug, Clone)]
pub enum GraphSpec {
    Expand {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        filter_expr: Option<Expression>,
        col_names: Vec<String>,
    },
    ExpandAll {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        filter_expr: Option<Expression>,
        col_names: Vec<String>,
        src_vids: Vec<Value>,
        step_limit: u32,
        /// When true, the expand operator only counts output rows instead of
        /// materializing them. Used when the downstream is a simple COUNT(*)
        /// aggregate with no GROUP BY or other aggregation functions.
        count_only: bool,
        /// When true, emit `Value::VertexId` / `Value::EdgeId` instead of
        /// full `Value::Vertex(Box)` / `Value::Edge(Box)` in the expand
        /// output.  Eliminates heap allocation for downstream operators that
        /// only need the identifier (e.g. another expand hop, count, join key).
        emit_raw_ids: bool,
        /// When true (always alongside `emit_raw_ids`), the hop's source
        /// column is also emitted as a `Value::VertexId` instead of cloning
        /// the full `Value::Vertex(Box)` carried in from upstream.
        lightweight_source: bool,
    },
    Traverse {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
        filter_expr: Option<Expression>,
    },
    BiExpand {
        edge_types: Vec<String>,
        direction: EdgeDirection,
    },
    BiTraverse {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
    },
}

// ── Sink spec ────────────────────────────────────────────────────────────────

/// Immutable config for sink (data modification) operators.
#[derive(Debug, Clone)]
pub enum SinkSpec {
    InsertVertices {
        space_name: String,
        vertex_properties: Vec<(String, Expression)>,
        tags: Vec<String>,
        /// Property column names for each tag, aligned with `tags`.
        tag_property_names: Vec<Vec<String>>,
        if_not_exists: bool,
    },
    InsertEdges {
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
        edge_properties: Vec<(String, Expression)>,
        if_not_exists: bool,
    },
    UpdateVertices {
        space_name: String,
        tag_name: String,
        updates: Vec<(String, Expression)>,
        condition: Option<Expression>,
        is_upsert: bool,
    },
    UpdateEdges {
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
        updates: Vec<(String, Expression)>,
        condition: Option<Expression>,
        is_upsert: bool,
    },
    DeleteVertices {
        space_name: String,
        vertex_id_col: String,
    },
    DeleteEdges {
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
    },
    PipeDeleteVertices {
        space_name: String,
        vertex_id_col: String,
    },
    PipeDeleteEdges {
        space_name: String,
        src_col: String,
        dst_col: String,
        edge_type: String,
    },
    DeleteTags {
        space_name: String,
        tag_names: Vec<String>,
        vertex_ids: Option<Vec<Value>>,
    },
}

// ── Exchange spec ────────────────────────────────────────────────────────────

/// Immutable config for exchange (gather / merge / repartition) operators.
///
/// M6: extended with RepartitionHash, Broadcast, Barrier, Materialize.
/// Workers in the shared engine-level scheduler execute partition tasks
/// dynamically via a morsel-style shared atomic counter.
#[derive(Debug, Clone)]
pub enum ExchangeSpec {
    /// Concatenate N partition outputs in partition order.
    Concatenate { partition_count: usize },
    /// N-way merge-sort of pre-sorted partition inputs.
    MergeSort {
        sort_expressions: Vec<Expression>,
        sort_directions: Vec<SortDirection>,
        limit: Option<usize>,
    },
    /// Hash-based repartition: partition rows by hash of keys into N buckets.
    ///
    /// Each child produces output for one partition; the operator collects,
    /// rehashes, and routes rows to the correct output bucket.  Used by hash
    /// join and hash aggregate to align partition boundaries.
    RepartitionHash {
        /// Number of output buckets / partitions.
        num_partitions: usize,
        /// Expressions whose hash determines the output partition.
        hash_expressions: Vec<Expression>,
        /// Column names / slot layout of the input rows.
        input_layout: Option<SlotLayout>,
        /// Column names / slot layout of the output rows.
        output_layout: Option<SlotLayout>,
    },
    /// Broadcast: replicate every input row to all consumers.
    ///
    /// Used to distribute a small build-side to all probe-side partitions.
    /// The input chunk is shallow-copied (Arc-like) or deep-cloned for each
    /// consumer depending on size.
    Broadcast {
        /// Number of output channels.
        num_consumers: usize,
    },
    /// Barrier: wait for all input fragments to complete before producing
    /// any output row.
    ///
    /// Used to sequence blocking stages (e.g. wait for build side before
    /// probe).  No data rearrangement; the first input's layout passes
    /// through.
    Barrier,
    /// Materialize: force an upstream fragment to fully materialise before
    /// the consumer fragment starts.
    ///
    /// Used for explicit spooling / break-fanout patterns and to isolate
    /// lifecycle across fragment boundaries.  Behaves like Concatenate but
    /// signals a pipeline break to the scheduler and validator.
    Materialize {
        /// Expected number of child inputs (all must be consumed).
        child_count: usize,
    },
}

// ── Set spec ─────────────────────────────────────────────────────────────────

/// Immutable config for set operators.
#[derive(Debug, Clone)]
pub enum SetSpec {
    Union,
    UnionAll,
    Intersect,
    Except,
    Minus,
}

// ── Apply spec ───────────────────────────────────────────────────────────────

/// Immutable config for apply operators.
#[derive(Debug, Clone)]
pub enum ApplySpec {
    Apply {
        kind: ApplyKind,
        correlated_columns: Vec<String>,
    },
    PatternApply {
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
        anti: bool,
    },
    CorrelatedApply {
        /// Self-contained right subtree (rooted at an `Argument` source),
        /// re-executed once per outer row with the outer row bound as the
        /// correlation frame.
        sub_plan: Arc<PhysicalPlan>,
        anti: bool,
    },
    RollUpApply {
        compare_columns: Vec<String>,
        collect_column: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyKind {
    Standard,
    Semi,
    Anti,
    Single,
    All,
}

/// Migrate action kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrateAction {
    MigrateSpace,
}

// ── Manage command payloads ──────────────────────────────────────────────────
// Self-contained value types for management DDL commands.
//
// These carry only the value fields consumed by the executor layer, so that
// the spec layer stays independent of planning-layer node types.

/// Space DDL command payload.
#[derive(Debug, Clone)]
pub enum SpaceManageCommand {
    Create {
        space_name: String,
        vid_type: String,
    },
    Drop {
        space_name: String,
    },
    Desc {
        space_name: String,
    },
    Show,
    ShowCreate {
        space_name: String,
    },
    Switch {
        space_name: String,
    },
    Alter {
        space_name: String,
    },
    Clear {
        space_name: String,
    },
}

/// Tag DDL command payload.
#[derive(Debug, Clone)]
pub enum TagManageCommand {
    Create {
        tag_name: String,
        properties: Vec<PropertyDef>,
        if_not_exists: bool,
    },
    Alter {
        tag_name: String,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
        changes: Vec<PropertyRename>,
    },
    Desc {
        tag_name: String,
    },
    Drop {
        tag_name: String,
        if_exists: bool,
    },
    Show,
    ShowCreate {
        tag_name: String,
    },
}

/// Edge DDL command payload.
#[derive(Debug, Clone)]
pub enum EdgeManageCommand {
    Create {
        edge_name: String,
        properties: Vec<PropertyDef>,
        src_tag_name: Option<String>,
        dst_tag_name: Option<String>,
        if_not_exists: bool,
    },
    Alter {
        edge_name: String,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
    },
    Desc {
        edge_name: String,
    },
    Drop {
        edge_name: String,
        if_exists: bool,
    },
    Show,
    ShowCreate {
        edge_name: String,
    },
}

/// Index DDL command payload.
#[derive(Debug, Clone)]
pub enum IndexManageCommand {
    CreateTagIndex {
        index_name: String,
        target_name: String,
        properties: Vec<String>,
    },
    DropTagIndex {
        index_name: String,
    },
    DescTagIndex {
        index_name: String,
    },
    ShowTagIndexes,
    RebuildTagIndex {
        index_name: String,
    },
    CreateEdgeIndex {
        index_name: String,
        target_name: String,
        properties: Vec<String>,
    },
    DropEdgeIndex {
        index_name: String,
    },
    DescEdgeIndex {
        index_name: String,
    },
    ShowEdgeIndexes,
    RebuildEdgeIndex {
        index_name: String,
    },
    ShowIndexes,
    ShowCreateIndex {
        index_name: String,
    },
}

/// User DDL command payload.
#[derive(Debug, Clone)]
pub enum UserManageCommand {
    Create {
        username: String,
        password: String,
        role: String,
    },
    Alter {
        username: String,
        new_password: Option<String>,
        new_role: Option<String>,
        is_locked: Option<bool>,
    },
    Drop {
        username: String,
        if_exists: bool,
    },
    ChangePassword {
        password_info: PasswordInfo,
    },
    GrantRole {
        username: String,
        space_name: String,
        role: String,
    },
    RevokeRole {
        username: String,
        space_name: String,
    },
    ShowUsers,
    ShowRoles,
    DescribeUser {
        username: String,
    },
}

/// Fulltext index DDL command payload.
#[derive(Debug, Clone)]
pub enum FulltextManageCommand {
    Create {
        index_name: String,
        schema_name: String,
        fields: Vec<String>,
        space_id: u64,
    },
    Drop {
        index_name: String,
        if_exists: bool,
    },
    Alter {
        index_name: String,
    },
    Show {
        pattern: Option<String>,
        from_schema: Option<String>,
    },
    Describe {
        index_name: String,
    },
}

/// Vector index DDL command payload.
#[derive(Debug, Clone)]
pub enum VectorManageCommand {
    Create {
        index_name: String,
        tag_name: String,
        field_name: String,
        vector_size: usize,
        distance: VectorDistance,
        space_id: u64,
    },
    Drop {
        index_name: String,
    },
}

/// Property rename within ALTER TAG (executor consumes only old/new names).
#[derive(Debug, Clone)]
pub struct PropertyRename {
    pub old_name: String,
    pub new_name: String,
}

// ── DDL spec ─────────────────────────────────────────────────────────────────

/// Immutable config for DDL operators.
#[derive(Debug, Clone)]
pub enum DdlSpec {
    SpaceManage {
        command: SpaceManageCommand,
    },
    TagManage {
        space_name: String,
        command: TagManageCommand,
    },
    EdgeManage {
        space_name: String,
        command: EdgeManageCommand,
    },
    IndexManage {
        space_name: String,
        command: IndexManageCommand,
    },
    DeleteIndex {
        space_name: String,
        index_name: String,
    },
    UserManage {
        command: UserManageCommand,
    },
    ShowStats {
        space_name: String,
    },
    ShowConfigs {
        space_name: String,
    },
    ShowQueries {
        space_name: String,
    },
    ShowSessions {
        space_name: String,
    },
    Analyze {
        space_name: String,
    },
    Migrate {
        space_name: String,
        action: MigrateAction,
        migration_data: Option<String>,
    },
}

// ── Fulltext spec ────────────────────────────────────────────────────────────

/// Immutable config for fulltext search operators.
#[derive(Debug, Clone)]
pub enum FulltextSpec {
    FulltextManage {
        space_name: String,
        command: FulltextManageCommand,
    },
    FulltextSearch {
        space_name: String,
        space_id: u64,
        index_name: String,
        search_query: String,
        tag_name: String,
        field_name: String,
    },
    FulltextLookup {
        space_name: String,
        space_id: u64,
        index_name: String,
        search_query: String,
        tag_name: String,
        field_name: String,
    },
    MatchFulltext {
        space_name: String,
        match_expr: Expression,
        match_field: Option<String>,
        tag_name: String,
        field_name: String,
    },
}

// ── Vector spec ──────────────────────────────────────────────────────────────

/// Immutable config for vector search operators.
#[derive(Debug, Clone)]
pub enum VectorSpec {
    VectorManage {
        space_name: String,
        command: VectorManageCommand,
    },
    VectorSearch {
        space_name: String,
        space_id: u64,
        index_name: String,
        query_vector: Vec<f32>,
        top_k: u32,
        tag_name: String,
        field_name: String,
    },
    VectorLookup {
        space_name: String,
        index_name: String,
        lookup_key: Expression,
    },
    VectorMatch {
        space_name: String,
        pattern: String,
        field: String,
        query_vector: Vec<f32>,
        threshold: Option<f32>,
        tag_name: String,
        field_name: String,
        space_id: u64,
    },
}

// ── Txn spec ─────────────────────────────────────────────────────────────────

/// Immutable config for transaction operators.
///
/// The actual transaction state transitions are performed by the
/// [`SessionTransactionController`] at execution time.
#[derive(Debug, Clone)]
pub enum TxnSpec {
    BeginTransaction,
    Commit,
    Rollback,
    /// Roll back to a savepoint: validates the controller is in `Active`
    /// state but does NOT transition out of it.
    RollbackToSavepoint {
        name: String,
    },
    /// Create a savepoint (validation only — the TransactionManager
    /// operation is performed by the API layer beforehand).
    Savepoint {
        name: String,
    },
    /// Release a savepoint (validation only — the TransactionManager
    /// operation is performed by the API layer beforehand).
    ReleaseSavepoint {
        name: String,
    },
}

// ── RecursiveFragment spec (M7) ──────────────────────────────────────────────

/// Immutable config for recursive fragment operators.
///
/// Variable-length path traversal, BFS, shortest-path, and multi-round
/// graph algorithms use this explicit recursive fragment spec.
///
/// Frontier, visited-set, path-predecessor, and result-queue allocations
/// are all accounted against the query memory pool.  Each round and
/// batch-expansion checks the cancellation token.
#[derive(Debug, Clone)]
pub enum RecursiveFragmentSpec {
    /// Bidirectional BFS shortest path between start and target vertices.
    ShortestPath {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        start_vertices: Vec<Value>,
        target_vertices: Vec<Value>,
    },
    /// Multi-source multi-target shortest path via bidirectional BFS.
    MultiShortestPath {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        left_vertex_column: String,
        right_vertex_column: String,
        single_shortest: bool,
    },
    /// BFS traversal with configurable depth and cycle policies.
    BFSShortest {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        allow_loops: bool,
    },
    /// Enumerate all paths between start and target vertices.
    AllPaths {
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: usize,
        max_depth: usize,
        acyclic: bool,
        limit: Option<usize>,
        offset: usize,
        start_vertices: Vec<Value>,
        target_vertices: Vec<Value>,
    },
}

// ── Cardinality shape keys ──────────────────────────────────────────────────

/// Normalized shape key for an operator's output cardinality.
///
/// Returns `"{space}:{Type}:{discriminator}"` for operators whose output row
/// count is estimated independently (sources, graph traversals, joins,
/// applies, aggregates).  Filter operators return `None`: they are corrected
/// per predicate via `condition_key` in the selectivity feedback loop.
///
/// The string format must stay in sync with the plan-side key generator in
/// `optimizer/cost_based/row_estimates.rs` (`cardinality_shape_key`), so
/// corrections recorded against physical operators are applied to the same
/// shapes during cost-based estimation.
pub fn operator_cardinality_shape_key(
    space: Option<&str>,
    spec: &crate::query::executor::streaming::plan::types::OperatorKindSpec,
) -> Option<String> {
    use crate::query::executor::streaming::plan::types::OperatorKindSpec;
    let prefix = space.unwrap_or("").to_string();
    let key = |kind: &str, discriminator: Option<&str>| {
        let mut key = format!("{prefix}:{kind}");
        if let Some(discriminator) = discriminator {
            if !discriminator.is_empty() {
                key.push(':');
                key.push_str(discriminator);
            }
        }
        Some(key)
    };
    let join_types = |kind: &str| key(kind, None);
    match spec {
        OperatorKindSpec::Source(source) => match source {
            SourceSpec::Start | SourceSpec::Argument { .. } => None,
            SourceSpec::ScanVertices { col_names, .. } => {
                key("ScanVertices", col_names.first().map(String::as_str))
            }
            SourceSpec::StandaloneValues { .. } => None,
            SourceSpec::StorageScanVertices { tag, .. } => key("ScanVertices", tag.as_deref()),
            SourceSpec::ScanEdges { col_names, .. } => {
                key("ScanEdges", col_names.first().map(String::as_str))
            }
            SourceSpec::StorageScanEdges { edge_type, .. } => {
                key("ScanEdges", edge_type.as_deref())
            }
            SourceSpec::GetVertices { .. } => key("GetVertices", None),
            SourceSpec::GetEdges { edge_type, .. } => key("GetEdges", edge_type.as_deref()),
            SourceSpec::GetNeighbors { direction, .. } => key("GetNeighbors", Some(direction)),
            SourceSpec::IndexScan { index_name, .. } => key("IndexScan", Some(index_name)),
            SourceSpec::GetProp { .. } => None,
        },
        OperatorKindSpec::Unary(UnarySpec::Filter { .. }) => None,
        OperatorKindSpec::Unary(UnarySpec::AppendVertices { entity_var, .. }) => {
            key("AppendVertices", Some(entity_var))
        }
        OperatorKindSpec::Unary(_) => None,
        OperatorKindSpec::Blocking(
            BlockingSpec::Aggregate { .. }
            | BlockingSpec::PartialAggregate { .. }
            | BlockingSpec::FinalAggregate { .. },
        ) => key("Aggregate", None),
        OperatorKindSpec::Blocking(_) => None,
        OperatorKindSpec::Join(spec) => match spec {
            JoinSpec::InnerJoin { .. } => join_types("InnerJoin"),
            JoinSpec::LeftJoin { .. } => join_types("LeftJoin"),
            JoinSpec::RightJoin { .. } => join_types("RightJoin"),
            JoinSpec::FullOuterJoin { .. } => join_types("FullOuterJoin"),
            JoinSpec::CrossJoin => join_types("CrossJoin"),
            JoinSpec::SemiJoin { .. } => join_types("SemiJoin"),
            JoinSpec::HashJoin { .. } => join_types("HashJoin"),
            JoinSpec::HashLeftJoin { .. } => join_types("HashLeftJoin"),
            JoinSpec::NestedLoopJoin { .. } => join_types("NestedLoopJoin"),
        },
        OperatorKindSpec::Graph(spec) => match spec {
            GraphSpec::Expand { edge_types, .. } => key("Expand", Some(&edge_types.join(","))),
            GraphSpec::ExpandAll { edge_types, .. } => {
                key("ExpandAll", Some(&edge_types.join(",")))
            }
            GraphSpec::Traverse { edge_types, .. } => key("Traverse", Some(&edge_types.join(","))),
            GraphSpec::BiExpand { edge_types, .. } => key("BiExpand", Some(&edge_types.join(","))),
            GraphSpec::BiTraverse { edge_types, .. } => {
                key("BiTraverse", Some(&edge_types.join(",")))
            }
        },
        OperatorKindSpec::Apply(spec) => match spec {
            ApplySpec::Apply { .. } => key("Apply", None),
            ApplySpec::PatternApply { .. } => key("PatternApply", None),
            ApplySpec::CorrelatedApply { .. } => key("CorrelatedApply", None),
            ApplySpec::RollUpApply { .. } => key("RollUpApply", None),
        },
        OperatorKindSpec::Set(spec) => match spec {
            SetSpec::Union | SetSpec::UnionAll => key("Union", None),
            SetSpec::Intersect => key("Intersect", None),
            SetSpec::Except | SetSpec::Minus => key("Minus", None),
        },
        _ => None,
    }
}
