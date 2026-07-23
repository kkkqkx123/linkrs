//! PhysicalPlan: immutable, verifiable, cacheable physical plan.
//!
//! The top-level plan object uses an arena of [`PhysicalOperatorSpec`] nodes
//! connected through a [`FragmentGraph`] DAG, replacing the ad-hoc
//! [`PhysicalNode`](super::PhysicalNode) tree as the build target.
//!
//! Construction flow (once unified):
//!
//! ```text
//! LogicalPlan → PhysicalPlanBuilder → PhysicalPlanValidator
//!             → Arc<PhysicalPlan> → QueryExecutionInstance::instantiate
//! ```
//!
//! Every operator carries a stable [`PhysicalOperatorId`] allocated from a
//! unified arena.  Logical node IDs are preserved for tracing but never
//! reused as physical IDs.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use super::super::operators::spec::{
    ApplySpec, BlockingSpec, DdlSpec, ExchangeSpec, FulltextSpec, GraphSpec, JoinSpec,
    RecursiveFragmentSpec, SetSpec, SinkSpec, SourceSpec, TxnSpec, UnarySpec, VectorSpec,
};
use super::super::parameters::ParameterSchema;
use super::super::slot::SlotLayout;
use super::properties::PhysicalProperties;

// ── Identity types ──────────────────────────────────────────────────────────

/// Stable identifier for a physical operator within a [`PhysicalPlan`].
///
/// Allocated from a unified arena.  Never equal to a logical node ID, even
/// when the operator originates from a single logical node (1:1 or 1:N split).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PhysicalOperatorId(pub usize);

impl PhysicalOperatorId {
    pub const fn new(id: usize) -> Self {
        Self(id)
    }
}

impl fmt::Display for PhysicalOperatorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Op#{}", self.0)
    }
}

/// Reference to the logical plan node that produced this physical operator.
///
/// Multiple physical operators can share the same `LogicalNodeId` when
/// a logical node is split (e.g., partial + final aggregate, local + global
/// distinct).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LogicalNodeId(pub i64);

impl fmt::Display for LogicalNodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "L{}", self.0)
    }
}

/// Fragment identifier within a [`FragmentGraph`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FragmentId(pub usize);

impl fmt::Display for FragmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "F{}", self.0)
    }
}

// ── Output contract ─────────────────────────────────────────────────────────

/// Sort ordering for a single column in the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

/// Whether an operator can produce output before consuming all of its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineMode {
    Pipelined,
    Blocking,
}

/// Describes the result shape that a plan (or fragment) delivers.
///
/// This is the immutable contract stored in the cached plan, not bound to
/// any particular sink or delivery mechanism.  Extended in M4 with
/// nullability and ordering metadata so that consumers can inspect the
/// schema without receiving data.
#[derive(Debug, Clone)]
pub struct OutputContract {
    /// The slot layout of the result columns.
    pub output_layout: SlotLayout,
    /// Whether at least one row is always produced (e.g., aggregate without
    /// group-by produces one row even from empty input).
    pub always_produces_row: bool,
    /// Per-column nullability (true = column may contain NULL).
    /// Length matches `output_layout.len()` when set.
    pub nullability: Vec<bool>,
    /// Per-column ordering guarantee (empty = unspecified).
    /// When non-empty, length matches `output_layout.len()`; columns not
    /// participating in the ordering are marked `None`.
    pub ordering: Vec<Option<SortOrder>>,
    /// Whether the result can be delivered through the pull-based chunk API.
    /// Blocking operators remain deliverable as a stream after their internal
    /// work completes.
    pub delivery_streamable: bool,
    /// Whether first output depends on complete input consumption.
    pub pipeline_mode: PipelineMode,
}

// ── Plan compatibility (cache validation) ───────────────────────────────────

/// Correctness-relevant metadata used to validate a cached plan.
///
/// A cache hit requires all fields to match.  Separated from
/// [`PhysicalPlan`] so it can be compared without loading the full plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanCompatibility {
    /// Hash of the normalised query text (fingerprint).
    pub query_fingerprint: u64,
    /// Schema / layout version at planning time.
    pub layout_version: Option<u64>,
    /// Feature / capability set that the plan requires.
    pub required_capabilities: CapabilitySet,
    /// Planning configuration hash (flags, thresholds, etc.).
    pub planning_config_hash: u64,
    /// Optimizer rule set / strategy version.
    pub optimizer_version: u64,
}

/// Bitmask-style set of capabilities that a plan may require.
///
/// Capability bits for progressive parallelism enablement (4 batches):
/// - `PARALLEL_BASIC`: partition-local Source and Unary operators (filter, project).
/// - `PARALLEL_BLOCKING`: blocking operators (sort, distinct, aggregate).
/// - `PARALLEL_JOIN`: multi-input operators (join, set, apply).
/// - `PARALLEL_FULL`: all operators including exchange, graph, DDL, sink, txn.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CapabilitySet {
    bits: u64,
}

impl CapabilitySet {
    pub const EMPTY: CapabilitySet = CapabilitySet { bits: 0 };

    /// Batch 1: Source and Unary operators.
    pub const PARALLEL_BASIC: CapabilitySet = CapabilitySet { bits: 1 << 0 };
    /// Batch 2: Blocking operators (sort, distinct, aggregate).
    pub const PARALLEL_BLOCKING: CapabilitySet = CapabilitySet { bits: 1 << 1 };
    /// Batch 3: Multi-input operators (join, set, apply).
    pub const PARALLEL_JOIN: CapabilitySet = CapabilitySet { bits: 1 << 2 };
    /// Batch 4: All remaining operators (exchange, graph, sink, ddl, txn, fulltext, vector).
    pub const PARALLEL_FULL: CapabilitySet = CapabilitySet { bits: 1 << 3 };

    /// All parallel capability bits OR'd together.
    pub const PARALLEL_ALL: CapabilitySet = CapabilitySet {
        bits: (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3),
    };

    pub const fn new(bits: u64) -> Self {
        Self { bits }
    }

    pub fn contains(&self, other: &Self) -> bool {
        self.bits & other.bits == other.bits
    }

    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }

    /// Returns true if this capability set includes any parallel execution
    /// capability.
    pub fn has_parallel(&self) -> bool {
        self.bits & Self::PARALLEL_ALL.bits != 0
    }

    /// Add a capability bit to this set.
    pub fn insert(&mut self, other: Self) {
        self.bits |= other.bits;
    }
}

// ── Fragment types ──────────────────────────────────────────────────────────

/// Classification of a fragment's execution role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragmentKind {
    /// Source fragment: contains scan / source operators.
    Source,
    /// Streaming pipeline fragment.
    Streaming,
    /// Blocking pipeline fragment (must consume all input before producing).
    Blocking,
    /// Exchange / gather fragment.
    Exchange,
    /// Result boundary fragment (delivers output to the sink).
    Result,
    /// Terminal fragment (DDL, DML, transaction command).
    Terminal,
}

/// A fragment spec describing one schedulable unit of work.
#[derive(Debug, Clone)]
pub struct FragmentSpec {
    pub id: FragmentId,
    pub kind: FragmentKind,
    /// Physical operators in this fragment (arena indices).
    pub operators: Vec<PhysicalOperatorId>,
    /// Root operator of this fragment.
    pub root_operator: PhysicalOperatorId,
    /// Input fragments (producers).
    pub inputs: Vec<FragmentId>,
    /// Output fragment (consumer), if any.
    pub output: Option<FragmentId>,
    /// Layout of the data flowing between fragments.
    pub exchange_layout: Option<SlotLayout>,
}

/// DAG of fragments that together form a complete physical plan.
#[derive(Debug, Clone)]
pub struct FragmentGraph {
    fragments: Vec<FragmentSpec>,
    root: FragmentId,
}

impl FragmentGraph {
    pub fn new(fragments: Vec<FragmentSpec>, root: FragmentId) -> Self {
        Self { fragments, root }
    }

    pub fn fragments(&self) -> &[FragmentSpec] {
        &self.fragments
    }

    pub fn root(&self) -> FragmentId {
        self.root
    }

    pub fn get(&self, id: FragmentId) -> Option<&FragmentSpec> {
        self.fragments.get(id.0)
    }
}

// ── Input contract (typed input ports) ──────────────────────────────────────

/// Label for a side of a partitioned or binary input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionSide {
    Left,
    Right,
    Unary,
}

/// Describes one input to a physical operator from another fragment.
///
/// Replaces the unlabeled `Vec<FragmentId>` approach.  Each input carries
/// its fragment reference, layout contract, and physical properties so that
/// left/right semantics are explicit at the plan level.
#[derive(Debug, Clone)]
pub struct FragmentInput {
    pub fragment: FragmentId,
    pub layout: Arc<SlotLayout>,
    pub properties: super::properties::PhysicalProperties,
}

/// A single partition within a partitioned input group.
#[derive(Debug, Clone)]
pub struct PartitionInput {
    pub partition_id: usize,
    pub fragment: FragmentId,
    pub layout: Arc<SlotLayout>,
    pub properties: super::properties::PhysicalProperties,
}

/// Typed input port specification for a physical operator.
///
/// This replaces the reliance on unlabeled `FragmentSpec.inputs` vectors
/// and stack-order guessing of left/right ports.
#[derive(Debug, Clone)]
pub enum InputContract {
    /// Operator produces data without consuming any input.
    NoInput,
    /// Single upstream input (source-like: Filter, Project, Sink, etc.).
    UnaryInput(FragmentInput),
    /// Two distinct labeled inputs (left and right) for Join, Set, Apply.
    BinaryInputs {
        left: FragmentInput,
        right: FragmentInput,
    },
    /// Multiple partitioned inputs (Gather, Exchange, HashShuffleJoin).
    PartitionedInputs {
        side: PartitionSide,
        members: Vec<PartitionInput>,
    },
}

// ── State ownership ─────────────────────────────────────────────────────────

/// Declares where an operator's mutable state is owned.
///
/// Used by the validator and runtime to ensure that state, profile,
/// reservations, spill files, and workers are managed by the correct owner
/// and never leaked or double-freed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateOwnership {
    /// State is embedded in the operator enum variant itself (inline).
    /// No runtime or shared state arena involvement.
    TreeLocal,
    /// State is owned by the global [`ExecutionRuntime`] state arena
    /// (shared across all operators of the same physical identity).
    GlobalRuntime,
    /// State is owned by a task-local scheduler (partition-local,
    /// worker-local, or fragment-local).
    TaskLocal,
}

// ── Operator spec (arena-stored) ────────────────────────────────────────────

/// Immutable configuration of a single physical operator in the arena.
///
/// This is the arena-stored equivalent of the tree-based [`PhysicalNode`],
/// carrying full metadata for validation, EXPLAIN, PROFILE, and execution
/// instantiation.
#[derive(Debug, Clone)]
pub struct PhysicalOperatorSpec {
    pub operator_id: PhysicalOperatorId,
    pub logical_node_id: Option<LogicalNodeId>,
    pub spec: OperatorKindSpec,
    /// Typed input port contract.  Replaces unlabeled `FragmentSpec.inputs`.
    /// When set, the materializer uses this instead of `fragment.inputs` order.
    pub input_contract: InputContract,
    /// Deprecated: use `input_contract` instead.  Kept for transition.
    pub input_layout: Option<SlotLayout>,
    pub output_layout: SlotLayout,
    pub properties: PhysicalProperties,
    /// Where this operator's mutable state is owned.
    pub state_ownership: StateOwnership,
    pub estimated_cardinality: Option<f64>,
    pub explain_name: &'static str,
}

/// Domain-specific operator configuration (mirrors the PhysicalNode variant
/// structure but without child references — children are implicit via
/// the fragment graph).
#[derive(Debug, Clone)]
pub enum OperatorKindSpec {
    Source(SourceSpec),
    Unary(UnarySpec),
    Blocking(BlockingSpec),
    Join(JoinSpec),
    Graph(GraphSpec),
    RecursiveFragment(RecursiveFragmentSpec),
    Sink(SinkSpec),
    Set(SetSpec),
    Apply(ApplySpec),
    Exchange(ExchangeSpec),
    Ddl(DdlSpec),
    Fulltext(FulltextSpec),
    Vector(VectorSpec),
    Txn(TxnSpec),
}

// ── PhysicalPlan ────────────────────────────────────────────────────────────

/// Immutable, verifiable, cacheable physical plan.
///
/// Once constructed by [`PhysicalPlanBuilder`], the plan is read-only
/// shared via `Arc<PhysicalPlan>`.  Multiple query instances can
/// concurrently instantiate from the same plan with different bindings.
#[derive(Debug, Clone)]
pub struct PhysicalPlan {
    /// Arena of operator specs, indexed by [`PhysicalOperatorId`].
    pub operators: Vec<PhysicalOperatorSpec>,
    /// Operator ID lookup by logical node ID (for EXPLAIN / PROFILE).
    pub logical_to_physical: HashMap<LogicalNodeId, Vec<PhysicalOperatorId>>,
    /// Fragment DAG.
    pub fragments: FragmentGraph,
    /// Root fragment.
    pub root_fragment: FragmentId,
    /// Output contract (schema, nullability, etc.).
    pub output: OutputContract,
    /// Compatibility metadata for cache validation.
    pub compatibility: PlanCompatibility,
    /// Capabilities that the runtime must support.
    pub required_capabilities: CapabilitySet,
    /// Parameter schema for this plan (empty if no parameters).
    pub parameter_schema: ParameterSchema,
}

impl PhysicalPlan {
    /// Look up an operator by its physical ID.
    pub fn operator(&self, id: PhysicalOperatorId) -> Option<&PhysicalOperatorSpec> {
        self.operators.get(id.0)
    }

    /// Look up all physical operators derived from a logical node.
    pub fn operators_from_logical(
        &self,
        logical_id: LogicalNodeId,
    ) -> Vec<&PhysicalOperatorSpec> {
        self.logical_to_physical
            .get(&logical_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.operator(*id))
            .collect()
    }

    /// Return the total number of physical operators.
    pub fn operator_count(&self) -> usize {
        self.operators.len()
    }

    /// Return the number of fragments.
    pub fn fragment_count(&self) -> usize {
        self.fragments.fragments().len()
    }
}

/// Build ID allocator that assigns monotonically increasing
/// [`PhysicalOperatorId`] values from a unified arena.
///
/// Ensures no hardcoded or synthetic ID ranges escape into the plan.
#[derive(Debug, Clone, Default)]
pub struct PhysicalOperatorIdAllocator {
    next: usize,
}

impl PhysicalOperatorIdAllocator {
    pub fn new() -> Self {
        Self { next: 0 }
    }

    /// Allocate the next physical operator ID.
    pub fn allocate(&mut self) -> PhysicalOperatorId {
        let id = PhysicalOperatorId(self.next);
        self.next += 1;
        id
    }

    /// Peek at the next ID without consuming it.
    pub fn peek(&self) -> PhysicalOperatorId {
        PhysicalOperatorId(self.next)
    }

    /// Return how many IDs have been allocated so far.
    pub fn allocated(&self) -> usize {
        self.next
    }
}

/// Fragment ID allocator.
#[derive(Debug, Clone, Default)]
pub struct FragmentIdAllocator {
    next: usize,
}

impl FragmentIdAllocator {
    pub fn new() -> Self {
        Self { next: 0 }
    }

    pub fn allocate(&mut self) -> FragmentId {
        let id = FragmentId(self.next);
        self.next += 1;
        id
    }

    pub fn allocated(&self) -> usize {
        self.next
    }
}

/// Allocator for synthetic node IDs used during partitioned execution.
///
/// Synthetic nodes (Gather, Start sources) currently use i64 sentinels.
/// This allocator assigns unique IDs from `i64::MIN` going downward so they
/// never collide with logical plan node IDs (which start from 0 and go up).
///
/// Migrate target: once PhysicalOperatorId is fully adopted, synthetic nodes
/// will use the same [`PhysicalOperatorIdAllocator`] as regular operators.
#[derive(Debug)]
pub struct SyntheticNodeIdAllocator {
    next: i64,
}

impl SyntheticNodeIdAllocator {
    /// Create a new allocator starting from the sentinel base.
    pub fn new() -> Self {
        Self { next: i64::MIN }
    }

    /// Allocate the next synthetic node ID (monotonically decreasing).
    pub fn allocate(&mut self) -> i64 {
        let id = self.next;
        // Saturating sub to avoid overflow in pathological cases.
        self.next = self.next.saturating_sub(1);
        id
    }

    /// Peek at the next ID without consuming it.
    pub fn peek(&self) -> i64 {
        self.next
    }
}

impl Default for SyntheticNodeIdAllocator {
    fn default() -> Self {
        Self::new()
    }
}
