//! Logical node traits.
//!
//! Mirrors the physical node traits in `core::nodes::base::plan_node_traits`
//! but operates on `LogicalNodeEnum` instead of `PlanNodeEnum`.

use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

/// Base trait for all logical plan nodes.
pub trait LogicalNode {
    fn id(&self) -> i64;
    fn name(&self) -> &'static str;
    fn output_var(&self) -> Option<&str>;
    fn col_names(&self) -> &[String];
    fn set_output_var(&mut self, var: String);
    fn set_col_names(&mut self, names: Vec<String>);
    fn into_enum(self) -> LogicalNodeEnum;
}

/// Marker trait for nodes with no inputs.
pub trait LogicalZeroInputNode: LogicalNode {}

/// Trait for nodes with a single input.
pub trait LogicalSingleInputNode: LogicalNode {
    fn input(&self) -> &LogicalNodeEnum;
    fn input_mut(&mut self) -> &mut LogicalNodeEnum;
    fn set_input(&mut self, input: LogicalNodeEnum);
}

/// Trait for nodes with two inputs (joins, binary operations).
pub trait LogicalBinaryInputNode: LogicalNode {
    fn left_input(&self) -> &LogicalNodeEnum;
    fn right_input(&self) -> &LogicalNodeEnum;
    fn left_input_mut(&mut self) -> &mut LogicalNodeEnum;
    fn right_input_mut(&mut self) -> &mut LogicalNodeEnum;
    fn set_left_input(&mut self, input: LogicalNodeEnum);
    fn set_right_input(&mut self, input: LogicalNodeEnum);
}

/// Trait for join nodes with hash/probe keys.
pub trait LogicalJoinNode: LogicalBinaryInputNode {
    fn hash_keys(&self) -> &[graphdb_core::types::expr::contextual::ContextualExpression];
    fn probe_keys(&self) -> &[graphdb_core::types::expr::contextual::ContextualExpression];
}

/// Trait for nodes with multiple inputs.
pub trait LogicalMultipleInputNode: LogicalNode {
    fn inputs(&self) -> &[LogicalNodeEnum];
    fn inputs_mut(&mut self) -> &mut Vec<LogicalNodeEnum>;
    fn add_input(&mut self, input: LogicalNodeEnum);
    fn remove_input(&mut self, index: usize) -> Result<(), String>;
}
