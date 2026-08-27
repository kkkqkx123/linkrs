//! Unified Interface for Node Types
//!
//! Provides a unified trait interface for plan and executor nodes.
//! Used to ensure consistency and traceability across the system.

/// Classification of nodes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeCategory {
    /// scanning operation node
    Scan,
    /// Connection Operation Node
    Join,
    /// Filtering Operation Nodes
    Filter,
    /// Projection operation node
    Project,
    /// Aggregation Operation Node
    Aggregate,
    /// Sort Operation Node
    Sort,
    /// control flow node
    Control,
    /// Data collection nodes
    DataCollect,
    /// Iterative operation node
    Traversal,
    /// set operator node (computing)
    SetOp,
    /// Path Algorithm Node
    Path,
    /// Managing Operational Nodes
    Admin,
    /// Data Access Nodes (Full-text search, etc.)
    DataAccess,
    /// Other types
    Other,
}

impl NodeCategory {
    /// Get the name of the category
    pub fn name(&self) -> &'static str {
        match self {
            NodeCategory::Scan => "Scan",
            NodeCategory::Join => "Join",
            NodeCategory::Filter => "Filter",
            NodeCategory::Project => "Project",
            NodeCategory::Aggregate => "Aggregate",
            NodeCategory::Sort => "Sort",
            NodeCategory::Control => "Control",
            NodeCategory::DataCollect => "DataCollect",
            NodeCategory::Traversal => "Traversal",
            NodeCategory::SetOp => "SetOp",
            NodeCategory::Path => "Path",
            NodeCategory::Admin => "Admin",
            NodeCategory::DataAccess => "DataAccess",
            NodeCategory::Other => "Other",
        }
    }
}

/// Unified Interface for Node Types
///
/// This trait provides a common interface for nodes in the query plan
/// and execution pipeline, ensuring semantic consistency.
pub trait NodeType {
    /// Get a unique identifier for the node type
    ///
    /// The return value should be a globally unique string identifier.
    /// Examples: "cross_join", "index_scan", etc.
    fn node_type_id(&self) -> &'static str;

    /// Get the name of the node type
    ///
    /// The return value should be a human-readable name.
    /// Examples: "Cross Join", "Index Scan", etc.
    fn node_type_name(&self) -> &'static str;

    /// Get the classification to which the node belongs
    fn category(&self) -> NodeCategory;
}

/// Node type mapping trait
///
/// Used to map a plan node to its corresponding executor type.
pub trait NodeTypeMapping {
    /// Get the corresponding actuator type ID
    fn corresponding_executor_type(&self) -> Option<&'static str>;
}
