//! Logical data-modification nodes: InsertVertices, InsertEdges, Update.
//!
//! Zero-input leaves carrying the same payload structs as their physical
//! counterparts, so lowering is a one-to-one rebuild.

use crate::define_logical_plan_node;
use crate::planning::plan::core::nodes::data_modification::{
    EdgeInsertInfo, UpdateTargetType, VertexInsertInfo,
};

define_logical_plan_node! {
    pub struct LogicalInsertVerticesNode {
        info: VertexInsertInfo,
    }
    enum: InsertVertices
    input: ZeroInputNode
}

define_logical_plan_node! {
    pub struct LogicalInsertEdgesNode {
        info: EdgeInsertInfo,
    }
    enum: InsertEdges
    input: ZeroInputNode
}

define_logical_plan_node! {
    pub struct LogicalUpdateNode {
        info: UpdateTargetType,
    }
    enum: Update
    input: ZeroInputNode
}
