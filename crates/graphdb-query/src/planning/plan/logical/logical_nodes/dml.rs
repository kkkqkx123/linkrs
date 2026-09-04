//! Logical data-modification nodes.
//!
//! Zero-input leaves carrying the same payload structs as their physical
//! counterparts, so lowering is a one-to-one rebuild.
//!
//! Single-input pipe variants carry an upstream input for streamed deletion.

use crate::define_logical_plan_node;
use crate::define_logical_plan_node_with_deps;
use crate::planning::plan::core::nodes::data_modification::{
    CopyTarget, EdgeDeleteInfo, EdgeInsertInfo, IndexDeleteInfo, TagDeleteInfo, UpdateTargetType,
    VertexDeleteInfo, VertexInsertInfo,
};

// ============================================================================
// Insert nodes
// ============================================================================

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

// ============================================================================
// Update node
// ============================================================================

define_logical_plan_node! {
    pub struct LogicalUpdateNode {
        info: UpdateTargetType,
    }
    enum: Update
    input: ZeroInputNode
}

// ============================================================================
// Delete nodes (zero-input, standalone)
// ============================================================================

define_logical_plan_node! {
    pub struct LogicalDeleteVerticesNode {
        info: VertexDeleteInfo,
    }
    enum: DeleteVertices
    input: ZeroInputNode
}

define_logical_plan_node! {
    pub struct LogicalDeleteEdgesNode {
        info: EdgeDeleteInfo,
    }
    enum: DeleteEdges
    input: ZeroInputNode
}

define_logical_plan_node! {
    pub struct LogicalDeleteTagsNode {
        info: TagDeleteInfo,
    }
    enum: DeleteTags
    input: ZeroInputNode
}

define_logical_plan_node! {
    pub struct LogicalDeleteIndexNode {
        info: IndexDeleteInfo,
    }
    enum: DeleteIndex
    input: ZeroInputNode
}

// ============================================================================
// Pipe delete nodes (single-input, streamed from upstream)
// ============================================================================

define_logical_plan_node_with_deps! {
    pub struct LogicalPipeDeleteVerticesNode {
        info: VertexDeleteInfo,
    }
    enum: PipeDeleteVertices
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalPipeDeleteEdgesNode {
        info: EdgeDeleteInfo,
    }
    enum: PipeDeleteEdges
    input: SingleInputNode
}

// ============================================================================
// Copy nodes (zero-input, bulk import/export)
// ============================================================================

define_logical_plan_node! {
    pub struct LogicalCopyFromNode {
        space_name: String,
        target: CopyTarget,
        file_path: String,
        header: bool,
        delimiter: char,
        batch_size: usize,
    }
    enum: CopyFrom
    input: ZeroInputNode
}

define_logical_plan_node! {
    pub struct LogicalCopyToNode {
        space_name: String,
        target: CopyTarget,
        file_path: String,
        header: bool,
        delimiter: char,
    }
    enum: CopyTo
    input: ZeroInputNode
}
