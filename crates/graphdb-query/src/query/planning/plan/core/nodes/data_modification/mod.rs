//! Data Modification Plan Nodes Module
//!
//! Provides unified plan nodes for INSERT, UPDATE, DELETE operations.
//! This module consolidates the previously separate delete, insert, and update modules.

pub mod delete_nodes;
pub mod info;
pub mod insert_nodes;
pub mod update_nodes;

// Re-export info structures
pub use info::{
    EdgeDeleteInfo, EdgeInsertInfo, EdgeUpdateInfo, IndexDeleteInfo, TagDeleteInfo, TagInsertSpec,
    UpdateTargetType, VertexDeleteInfo, VertexInsertInfo, VertexUpdateInfo,
};

pub use delete_nodes::{
    DeleteEdgesNode, DeleteIndexNode, DeleteTagsNode, DeleteVerticesNode, PipeDeleteEdgesNode,
    PipeDeleteVerticesNode,
};
pub use insert_nodes::{InsertEdgesNode, InsertVerticesNode};
pub use update_nodes::{UpdateEdgesNode, UpdateNode, UpdateVerticesNode};
