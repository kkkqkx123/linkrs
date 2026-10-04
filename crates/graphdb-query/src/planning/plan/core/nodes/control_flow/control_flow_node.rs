//! Implementation of control flow nodes
//!
//! Plan nodes related to control flow, such as Start, Argument, Select, Loop, etc.

mod argument;
mod r#loop;
mod recursive_cte;
mod select;
mod transaction;

#[cfg(test)]
mod tests;

pub use argument::ArgumentNode;
pub use r#loop::LoopNode;
pub use recursive_cte::RecursiveCteNode;
pub use select::SelectNode;
pub use transaction::{
    BeginTransactionNode, CommitNode, IsolationLevel, ReleaseSavepointNode, RollbackNode,
    SavepointNode,
};

use crate::define_plan_node;

define_plan_node! {
    pub struct PassThroughNode {
    }
    enum: PassThrough
    input: ZeroInputNode
}

impl PassThroughNode {
    pub fn new(id: i64) -> Self {
        Self {
            id,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }
}
