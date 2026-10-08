//! Core Query Module
//!
//! Provide definitions of the basic types of query systems and their common functions.

mod execution_state;
mod node_type;

pub use execution_state::{
    ExecutorState, LoopExecutionState, OptimizationPhase, OptimizationState, QueryExecutionState,
    RowStatus,
};
pub use node_type::{NodeCategory, NodeType, NodeTypeMapping};
