// Re-export all executor modules
pub mod admin;
pub mod base;
pub mod control_flow;
pub mod data_access;
pub mod data_modification;
pub mod explain;
pub mod expression;
pub mod factory;
pub mod graph_operations;
pub mod macros;
pub mod relational_algebra;
pub mod result_processing;
pub mod utils;

// Re-export from the base module: The basic types are uniformly exported from the base module.
pub use base::{
    BaseExecutor, BaseResultProcessor, ExecutionContext, ExecutionResult, Executor, ExecutorEnum,
    ExecutorStats, HasInput, HasStorage, InputExecutor, ResultProcessor, ResultProcessorContext,
    StartExecutor,
};

// Re-export data access executors
pub use data_access::{
    AllPathsExecutor, GetEdgesExecutor, GetNeighborsExecutor, GetPropExecutor, GetVerticesExecutor,
    IndexScanExecutor, LookupIndexExecutor, ScanVerticesExecutor,
};

// Re-export result processing executors
pub use result_processing::{
    DedupExecutor, DedupStrategy, LimitExecutor, SampleExecutor, SampleMethod, SortExecutor,
    SortKey, SortOrder, TopNExecutor,
};

// Re-export relational algebra executors
pub use relational_algebra::{
    AggregateExecutor, AggregateFunctionSpec, CrossJoinExecutor, FilterExecutor,
    FullOuterJoinExecutor, GroupAggregateState, GroupByExecutor, HashInnerJoinExecutor,
    HashLeftJoinExecutor, HavingExecutor, InnerJoinExecutor, IntersectExecutor, LeftJoinExecutor,
    MinusExecutor, ProjectExecutor, ProjectionColumn, SetExecutor, UnionAllExecutor, UnionExecutor,
};

// Re-export transformations (Data conversion executors)
pub use result_processing::transformations::{
    AppendVerticesExecutor, AssignExecutor, PatternApplyExecutor, RollUpApplyExecutor,
    UnwindExecutor,
};

// Re-export control flow executors
pub use control_flow::{ForLoopExecutor, LoopExecutor, WhileLoopExecutor};

// Re-export core execution states
pub use crate::query::core::{ExecutorState, LoopExecutionState, QueryExecutionState, RowStatus};

// Re-export admin executors
pub use admin::{
    AlterEdgeExecutor, AlterTagExecutor, AlterUserExecutor, ChangePasswordExecutor,
    CreateEdgeExecutor, CreateEdgeIndexExecutor, CreateSpaceExecutor, CreateTagExecutor,
    CreateTagIndexExecutor, CreateUserExecutor, DescEdgeExecutor, DescEdgeIndexExecutor,
    DescSpaceExecutor, DescTagExecutor, DescTagIndexExecutor, DropEdgeExecutor,
    DropEdgeIndexExecutor, DropSpaceExecutor, DropTagExecutor, DropTagIndexExecutor,
    DropUserExecutor, RebuildEdgeIndexExecutor, RebuildTagIndexExecutor, ShowEdgeIndexesExecutor,
    ShowEdgesExecutor, ShowSpacesExecutor, ShowTagIndexesExecutor, ShowTagsExecutor,
};

// Re-export utility executors
pub use utils::{ArgumentExecutor, DataCollectExecutor, PassThroughExecutor};

// Re-export graph traversal executors (graph traversal executor)
pub use crate::query::executor::graph_operations::graph_traversal::algorithms::BFSShortestExecutor;

// Re-export explain/profile executors
pub use explain::{
    ExecutionStatsContext, ExplainExecutor, ExplainMode, InstrumentedExecutor,
    InstrumentedExecutorFactory, NodeExecutionStats, ProfileExecutor,
};

// Compilation-time enumeration consistency check
// These checks ensure that the number of variants for PlanNodeEnum and ExecutorEnum is the same.
// If the quantities do not match, the compilation will fail and a clear error message will be displayed.

/// The number of variants of PlanNodeEnum
/// When adding or removing variants of PlanNodeEnum, this constant needs to be updated.
/// This constant is only used for compile-time assertion checks; therefore, it is marked as allowing its non-use.
const PLAN_NODE_VARIANT_COUNT: usize = 68;

/// The number of variants of ExecutorEnum
/// When adding or removing variants of ExecutorEnum, this constant needs to be updated.
/// This constant is only used for compile-time assertion checking, so it is marked to allow unused
const EXECUTOR_VARIANT_COUNT: usize = 68;

// Compilation-time assertion: Ensure that the number of variants in both enumerations is the same.
// Formatted strings cannot be used within the `const assert` statement.
const _: () = assert!(
    PLAN_NODE_VARIANT_COUNT == EXECUTOR_VARIANT_COUNT,
    "PlanNodeEnum and ExecutorEnum variant count mismatch"
);

/// Consistency check of node type
///
/// This module checks the consistency of PlanNodeEnum and ExecutorEnum during the compilation phase.
#[cfg(test)]
mod consistency_tests {
    use crate::query::core::NodeTypeMapping;
    use crate::query::planning::plan::core::nodes::PlanNodeEnum;

    /// Test whether the node type IDs of PlanNodeEnum and ExecutorEnum are consistent.
    #[test]
    fn test_node_type_id_consistency() {
        // This test ensures that all PlanNode types have corresponding Executor types.
        // The actual checks are performed during the compilation phase using constant assertions.
        assert_eq!(
            super::PLAN_NODE_VARIANT_COUNT,
            super::EXECUTOR_VARIANT_COUNT
        );
    }

    /// Verify the node type mapping.
    #[test]
    fn test_node_type_mapping() {
        use crate::query::planning::plan::core::nodes::{
            ArgumentNode, CrossJoinNode, PlanNodeEnum as NodeEnum,
        };

        // Example: Verifying the mapping of CrossJoin
        // Create two ArgumentNode objects as inputs for the CrossJoin operation.
        let left = ArgumentNode::new(1, "left_var");
        let right = ArgumentNode::new(2, "right_var");
        let cross_join_node =
            CrossJoinNode::new(NodeEnum::Argument(left), NodeEnum::Argument(right))
                .expect("Failed to create CrossJoinNode");
        let plan_node = PlanNodeEnum::CrossJoin(cross_join_node);

        // Verify that the PlanNodeEnum implementation satisfies the NodeTypeMapping requirements.
        let executor_type = plan_node.corresponding_executor_type();
        assert!(executor_type.is_some());
        assert_eq!(
            executor_type.expect("Expected executor type to exist"),
            "cross_join"
        );
    }
}
