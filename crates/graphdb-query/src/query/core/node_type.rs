//! Unified Interface for Node Types
//!
//! provides a unified trait interface for PlanNodeEnum and ExecutorEnum.
//! Used to ensure consistency and traceability between two enumerations.

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
/// This trait is used to unify the interfaces of PlanNodeEnum and ExecutorEnum.
/// Ensure that the two enumerations are semantically consistent.
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
/// Used to map a PlanNodeEnum to a corresponding ExecutorEnum.
pub trait NodeTypeMapping {
    /// Get the corresponding actuator type ID
    fn corresponding_executor_type(&self) -> Option<&'static str>;
}
