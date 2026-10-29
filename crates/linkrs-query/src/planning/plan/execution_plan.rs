//! Structure definition of the execution plan
//! Contains the ExecutionPlan and SubPlan structures.

use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use crate::planning::plan::logical_plan::LogicalPlan;
use crate::planning::plan::PlanNodeEnum;
use crate::planning::planner::PlannerError;

// The `PartitionSpec` / `PartitionSource` / `PartitionSpecError` types now
// live in `partition_spec.rs`.  Re-export them here so the public
// `ExecutionPlan` API (which carries an optional `PartitionSpec`) keeps
// compiling without touching the new module path at every call site.
pub use crate::planning::plan::partition_spec::{
    PartitionSource, PartitionSpec, PartitionSpecError,
};

/// Unified cost diagnostics carried with the plan.
///
/// Groups the cost-based decision notes and the parallel fallback reason so
/// they travel as one record instead of scattered strings. Display layers
/// (`PlanDescription`, EXPLAIN output) read from here without changing text.
#[derive(Debug, Clone, Default)]
pub struct PlanDiagnostics {
    /// Why parallel partitioning was not selected (empty = active or not requested).
    pub fallback_reason: String,
    /// Cost-based decision notes (unnest, join order, index, TopN, aggregate).
    pub notes: Vec<String>,
}

impl PlanDiagnostics {
    pub fn push_note(&mut self, note: String) {
        self.notes.push(note);
    }

    pub fn extend_notes(&mut self, notes: impl IntoIterator<Item = String>) {
        self.notes.extend(notes);
    }

    pub fn append_fallback(&mut self, reason: String) {
        if self.fallback_reason.is_empty() {
            self.fallback_reason = reason;
        } else if !reason.is_empty() {
            self.fallback_reason.push_str("; ");
            self.fallback_reason.push_str(&reason);
        }
    }
}

/// Execution plan structure
/// Represents the complete executable plan, including the root node and the plan ID.
#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    /// The root node of the planning tree
    pub root: Option<PlanNodeEnum>,

    /// The unique ID of the plan
    pub id: i64,

    /// Optimized time (in microseconds)
    pub optimize_time_in_us: u64,

    /// Of course! Please provide the text you would like to have translated.
    pub format: String,

    /// Optional physical partition layout. A layout is consumed only by a
    /// partition-safe streaming plan; unsupported plans fail explicitly.
    pub partition_spec: Option<PartitionSpec>,

    /// Maximum worker threads for intra-query parallelism.
    /// 1 means fully serial. Propagated from PartitioningConfig.
    pub max_workers: usize,

    /// Per-partition output channel capacity for backpressure.
    pub max_buffered_chunks: usize,

    /// Unified cost diagnostics (fallback reason + CBO notes).
    pub diagnostics: PlanDiagnostics,

    /// The pure logical plan (if conversion succeeded).
    /// Used by cost-based optimization to make physical decisions.
    pub logical_plan: Option<LogicalPlan>,
}

impl ExecutionPlan {
    /// Create a new execution plan.
    pub fn new(root: Option<PlanNodeEnum>) -> Self {
        Self {
            root,
            id: -1,
            optimize_time_in_us: 0,
            format: "default".to_string(),
            partition_spec: None,
            max_workers: 1,
            max_buffered_chunks: 10,
            diagnostics: PlanDiagnostics::default(),
            logical_plan: None,
        }
    }

    /// Set the root node of the plan.
    pub fn set_root(&mut self, root: PlanNodeEnum) {
        self.root = Some(root);
    }

    /// Obtain the reference to the root node of the plan.
    pub fn root(&self) -> &Option<PlanNodeEnum> {
        &self.root
    }

    /// Obtain a reference to the variable root node.
    pub fn root_mut(&mut self) -> &mut Option<PlanNodeEnum> {
        &mut self.root
    }

    /// Set the ID for the plan.
    pub fn set_id(&mut self, id: i64) {
        self.id = id;
    }

    /// Set the optimization time
    pub fn set_optimize_time(&mut self, time_us: u64) {
        self.optimize_time_in_us = time_us;
    }

    /// Set the output format
    pub fn set_format(&mut self, format: String) {
        self.format = format;
    }

    /// Attach the physical partition layout chosen by the optimizer.
    pub fn set_partition_spec(&mut self, partition_spec: PartitionSpec) {
        self.partition_spec = Some(partition_spec);
    }

    /// Remove partitioning and execute the plan as a single tree.
    pub fn clear_partition_spec(&mut self) {
        self.partition_spec = None;
    }

    pub fn partition_spec(&self) -> Option<&PartitionSpec> {
        self.partition_spec.as_ref()
    }

    pub fn set_max_workers(&mut self, max_workers: usize) {
        self.max_workers = max_workers;
    }

    pub fn set_max_buffered_chunks(&mut self, max_buffered_chunks: usize) {
        self.max_buffered_chunks = max_buffered_chunks;
    }

    /// Attach a pure logical plan for cost-based optimization.
    pub fn set_logical_plan(&mut self, logical_plan: LogicalPlan) {
        self.logical_plan = Some(logical_plan);
    }

    /// Access the logical plan, if available.
    pub fn logical_plan(&self) -> Option<&LogicalPlan> {
        self.logical_plan.as_ref()
    }

    /// Calculate the number of nodes in the plan.
    /// Traverses the execution plan tree with an explicit stack instead of
    /// recursion so deeply nested plans cannot overflow the call stack.
    pub fn node_count(&self) -> usize {
        let Some(root) = self.root.as_ref() else {
            return 0;
        };
        let mut count = 0;
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            count += 1;
            stack.extend(node.children());
        }
        count
    }
}

/// SubPlan structure
/// Represents a sub-part of the execution plan, which contains the root node and the tail node.
/// Segmented planning for complex queries
#[derive(Debug, Clone)]
pub struct SubPlan {
    /// The root node of the sub-plan
    pub root: Option<PlanNodeEnum>,

    /// The end node of the sub-plan
    /// Used to connect multiple sub-plans
    pub tail: Option<PlanNodeEnum>,

    /// Native logical tree, when the planner produced one directly.
    ///
    /// Migrated planners build the pure [`LogicalNodeEnum`] tree first and
    /// convert it to the physical root exactly once at the plan exit; the
    /// logical tree is attached here so the compiler can build the
    /// [`crate::planning::plan::logical_plan::LogicalPlan`] natively
    /// instead of stripping it back out of the physical tree.
    pub logical_root: Option<crate::planning::plan::logical::LogicalNodeEnum>,
    /// The join order was priced with real statistics during planning.
    ///
    /// Set by the match planner when its enumerator ran against a non-empty
    /// statistics snapshot. The compiler copies this into the logical plan
    /// so the optimizer can skip redundant re-enumeration.
    pub join_order_stats_backed: bool,
}

impl SubPlan {
    /// Create a new SubPlan without a logical mirror.
    ///
    /// Reserved for planners whose operators stay physical-only by design
    /// (data modifications, administrative statements, search operators)
    /// where no `LogicalNodeEnum` counterpart exists. DQL planners build a
    /// native logical tree instead and attach it through
    /// [`SubPlan::from_dual`] or the `plan_combiner` mirror helpers.
    pub fn new(root: Option<PlanNodeEnum>, tail: Option<PlanNodeEnum>) -> Self {
        Self {
            root,
            tail,
            logical_root: None,
            join_order_stats_backed: false,
        }
    }

    /// Dual-tree exit for statement and clause planners.
    ///
    /// This is the single construction entry for every plan that carries
    /// both a physical tree (`root`/`tail`) and its native logical mirror
    /// (`logical_root`). Statement planners produce fresh dual trees here;
    /// clause planners wrap an input plan and preserve its tail through
    /// the same entry. Physical-only exits keep using [`SubPlan::new`];
    /// migrating a dual-tree planner to the single-conversion
    /// [`SubPlan::from_logical_root`] entry only ever touches this
    /// constructor's callers. Query operators must not gain new
    /// physical-only construction paths; `new` stays reserved for
    /// management, system, administrative, and search leaves plus
    /// transient wiring seeds.
    pub fn from_dual(
        root: Option<PlanNodeEnum>,
        tail: Option<PlanNodeEnum>,
        logical_root: Option<crate::planning::plan::logical::LogicalNodeEnum>,
    ) -> Self {
        Self {
            root,
            tail,
            logical_root,
            join_order_stats_backed: false,
        }
    }

    /// Create a SubPlan from a native logical tree.
    ///
    /// Builds the physical root through `PhysicalMapper::map` — the single
    /// statement-level conversion entry — exactly once, and retains the
    /// logical tree for the compiler.
    pub fn from_logical_root(
        logical_root: crate::planning::plan::logical::LogicalNodeEnum,
    ) -> Self {
        let physical_root =
            crate::planning::physical_mapper::PhysicalMapper::map(logical_root.clone());
        Self {
            root: Some(physical_root),
            tail: None,
            logical_root: Some(logical_root),
            join_order_stats_backed: false,
        }
    }

    /// Obtain the native logical tree, if the planner produced one.
    pub fn logical_root(&self) -> Option<&crate::planning::plan::logical::LogicalNodeEnum> {
        self.logical_root.as_ref()
    }

    /// Obtain a reference to the root node.
    pub fn root(&self) -> &Option<PlanNodeEnum> {
        &self.root
    }

    /// Obtain the reference to the tail node.
    pub fn tail(&self) -> &Option<PlanNodeEnum> {
        &self.tail
    }

    /// Setting the root node
    pub fn set_root(&mut self, root: PlanNodeEnum) {
        self.root = Some(root);
    }

    /// Setting the tail node
    pub fn set_tail(&mut self, tail: PlanNodeEnum) {
        self.tail = Some(tail);
    }

    /// Check whether SubPlan is empty.
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// Retrieve all nodes from the SubPlan.
    pub fn collect_nodes(&self) -> Vec<PlanNodeEnum> {
        let mut nodes = Vec::new();

        if let Some(root) = &self.root {
            nodes.push(root.clone());
        }

        if let Some(tail) = &self.tail {
            nodes.push(tail.clone());
        }

        nodes
    }

    /// Connect `upstream` as the input of `downstream`, returning a
    /// structurally closed plan.
    ///
    /// The upstream root is attached as the downstream root's input, so the
    /// resulting SubPlan is a complete tree that planners and executors
    /// observe identically.  This replaces the previous pattern of creating
    /// the downstream node first and manually injecting its input, which
    /// left the connection outside the SubPlan structure.
    ///
    /// The returned plan's tail is the upstream tail (the data flows from
    /// upstream into downstream).
    pub fn connect_upstream(
        mut downstream: SubPlan,
        upstream: SubPlan,
    ) -> Result<SubPlan, PlannerError> {
        let down_root = downstream.root.take().ok_or_else(|| {
            PlannerError::PlanGenerationFailed("downstream has no root".to_string())
        })?;
        let up_root = upstream.root.ok_or_else(|| {
            PlannerError::PlanGenerationFailed("upstream has no root".to_string())
        })?;

        let mut connected = down_root;
        connect_node_input(&mut connected, up_root)?;
        downstream.root = Some(connected);
        downstream.tail = upstream.tail;
        Ok(downstream)
    }
}

/// Attach `input` as the structural input of `node`.
///
/// Single-input nodes get their input slot and dependency list filled via
/// [`SingleInputNode::set_input`]; multi-input expansion nodes (Expand /
/// ExpandAll / AppendVertices) append to their dependency list via
/// [`MultipleInputNode::add_input`], which is the storage their tree
/// traversals read.  Binary and leaf nodes cannot host a unary upstream.
fn connect_node_input(node: &mut PlanNodeEnum, input: PlanNodeEnum) -> Result<(), PlannerError> {
    match node {
        PlanNodeEnum::Project(n) => n.set_input(input),
        PlanNodeEnum::Filter(n) => n.set_input(input),
        PlanNodeEnum::Sort(n) => n.set_input(input),
        PlanNodeEnum::Limit(n) => n.set_input(input),
        PlanNodeEnum::TopN(n) => n.set_input(input),
        PlanNodeEnum::Sample(n) => n.set_input(input),
        PlanNodeEnum::Dedup(n) => n.set_input(input),
        PlanNodeEnum::DataCollect(n) => n.set_input(input),
        PlanNodeEnum::Aggregate(n) => n.set_input(input),
        PlanNodeEnum::Window(n) => n.set_input(input),
        PlanNodeEnum::Unwind(n) => n.set_input(input),
        PlanNodeEnum::Assign(n) => n.set_input(input),
        PlanNodeEnum::PatternApply(n) => n.set_input(input),
        PlanNodeEnum::CorrelatedApply(n) => n.set_input(input),
        PlanNodeEnum::RollUpApply(n) => n.set_input(input),
        PlanNodeEnum::Materialize(n) => n.set_input(input),
        PlanNodeEnum::Traverse(n) => n.set_input(input),
        PlanNodeEnum::PipeDeleteVertices(n) => n.set_input(input),
        PlanNodeEnum::PipeDeleteEdges(n) => n.set_input(input),
        PlanNodeEnum::Union(n) => n.set_input(input),
        PlanNodeEnum::Minus(n) => n.set_input(input),
        PlanNodeEnum::Intersect(n) => n.set_input(input),
        PlanNodeEnum::Expand(n) => n.add_input(input),
        PlanNodeEnum::ExpandAll(n) => n.add_input(input),
        PlanNodeEnum::AppendVertices(n) => n.add_input(input),
        _ => {
            return Err(PlannerError::PlanGenerationFailed(format!(
                "Cannot connect upstream to node of type {}",
                node.name()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_plan_max_workers_defaults() {
        let plan = ExecutionPlan::new(None);
        assert_eq!(plan.max_workers, 1);
        assert_eq!(plan.max_buffered_chunks, 10);
    }

    #[test]
    fn execution_plan_set_max_workers() {
        let mut plan = ExecutionPlan::new(None);
        plan.set_max_workers(4);
        plan.set_max_buffered_chunks(20);
        assert_eq!(plan.max_workers, 4);
        assert_eq!(plan.max_buffered_chunks, 20);
    }

    #[test]
    fn connect_upstream_attaches_input_structurally() {
        use crate::planning::plan::core::nodes::{ExpandAllNode, StartNode};

        let downstream_seed = PlanNodeEnum::ExpandAll(ExpandAllNode::new(1, vec![], "out"));
        let downstream = SubPlan::new(Some(downstream_seed.clone()), Some(downstream_seed));
        let upstream_seed = PlanNodeEnum::Start(StartNode::new());
        let upstream = SubPlan::new(Some(upstream_seed.clone()), Some(upstream_seed));

        let connected = SubPlan::connect_upstream(downstream, upstream)
            .expect("connect_upstream should succeed");
        let root = connected.root.expect("connected plan should have a root");
        let children = root.children();
        assert_eq!(children.len(), 1, "expand node must own exactly one input");
        assert!(
            matches!(children[0], PlanNodeEnum::Start(_)),
            "expand node input should be the upstream root"
        );
        assert!(
            connected.tail.is_some(),
            "connected plan tail should carry the upstream tail"
        );
    }

    #[test]
    fn connect_upstream_requires_both_roots() {
        use crate::planning::plan::core::nodes::StartNode;

        let empty = SubPlan::new(None, None);
        let populated_seed = PlanNodeEnum::Start(StartNode::new());
        let populated = SubPlan::new(Some(populated_seed.clone()), Some(populated_seed));

        assert!(SubPlan::connect_upstream(empty.clone(), populated.clone()).is_err());
        assert!(SubPlan::connect_upstream(populated, empty).is_err());
    }

    #[test]
    fn connect_upstream_rejects_leaf_downstream() {
        use crate::planning::plan::core::nodes::StartNode;

        let downstream_seed = PlanNodeEnum::Start(StartNode::new());
        let downstream = SubPlan::new(Some(downstream_seed.clone()), Some(downstream_seed));
        let upstream_seed = PlanNodeEnum::Start(StartNode::new());
        let upstream = SubPlan::new(Some(upstream_seed.clone()), Some(upstream_seed));

        assert!(
            SubPlan::connect_upstream(downstream, upstream).is_err(),
            "a leaf node cannot host a unary upstream"
        );
    }
}
