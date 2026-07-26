//! Logical plan representation.
//!
//! This module provides the pure logical operator tree (`LogicalNodeEnum`)
//! and the `LogicalPlan` wrapper.  Physical execution choices (IndexScan,
//! HashInnerJoin, etc.) are excluded — they are introduced later by the
//! physical converter.

pub mod conversion;
pub mod logical_macros;
pub mod logical_node_enum;
pub mod logical_node_traits;
pub mod logical_nodes;

pub use conversion::{convert_plan, ConversionError};
pub use logical_node_enum::LogicalNodeEnum;
pub use logical_node_traits::{
    LogicalBinaryInputNode, LogicalJoinNode, LogicalMultipleInputNode, LogicalNode,
    LogicalSingleInputNode, LogicalZeroInputNode,
};
