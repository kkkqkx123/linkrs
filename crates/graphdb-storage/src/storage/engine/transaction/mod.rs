//! Transaction Module
//!
//! Unified transaction module containing core transaction operations:
//! - ops: Core transaction operations for vertex/edge manipulation
//! - undo: Undo log execution for GraphStorageContext
//! - recovery: WAL recovery replay for GraphStorageContext
//! - compact: Compaction operations for GraphStorageContext

mod compact;
mod ops;
#[cfg(test)]
mod ops_test;
mod recovery;
pub mod undo;

pub use crate::core::types::UndoTarget;
pub use ops::{
    AddEdgeParams, DeleteEdgeParams, DeleteEdgeTypeParams, EdgeTypeLabelParams,
    RevertDeleteEdgeParams, TransactionOps, UpdateEdgePropertyUndoParams,
};
