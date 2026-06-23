//! Definition of Plan Node Classification
//!
//! Classify PlanNodes based on their functional characteristics to facilitate decision-making by the optimizer and to improve the organization of the code.

/// Plan node classification enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanNodeCategory {
    /// Access layer – Reads data from the storage layer
    Access,
    /// Operation Layer – Data Conversion and Filtering
    Operation,
    /// Connection layers – Multi-data stream connections
    Join,
    /// Traversal layer – Image traversal and expansion
    Traversal,
    /// Controlling the stratosphere – Managing the execution of processes
    ControlFlow,
    /// Data processing layer – Complex data operations
    DataProcessing,
    /// Algorithm Layer – Execution of Graph Algorithms
    Algorithm,
    /// Management/DDL Layer – Metadata Management
    Management,
    /// Data Access Layer – Full-text search and other data access operations
    DataAccess,
}

impl PlanNodeCategory {
    /// Obtain the category names
    pub fn name(&self) -> &'static str {
        match self {
            PlanNodeCategory::Access => "Access",
            PlanNodeCategory::Operation => "Operation",
            PlanNodeCategory::Join => "Join",
            PlanNodeCategory::Traversal => "Traversal",
            PlanNodeCategory::ControlFlow => "ControlFlow",
            PlanNodeCategory::DataProcessing => "DataProcessing",
            PlanNodeCategory::Algorithm => "Algorithm",
            PlanNodeCategory::Management => "Management",
            PlanNodeCategory::DataAccess => "DataAccess",
        }
    }

    /// Please provide the text that needs to be translated into Chinese.
    pub fn description(&self) -> &'static str {
        match self {
            PlanNodeCategory::Access => "Access layer - reads data from the storage layer",
            PlanNodeCategory::Operation => "Operational Layer - Data Conversion and Filtering",
            PlanNodeCategory::Join => "Connection Layer - Multi-Stream Connectivity",
            PlanNodeCategory::Traversal => "Traversal Layer - Graph Traversal and Extension",
            PlanNodeCategory::ControlFlow => "Control Flow Layer - Performs process control",
            PlanNodeCategory::DataProcessing => "Data Processing Layer - Complex Data Manipulation",
            PlanNodeCategory::Algorithm => "Algorithm Layer - Graph Algorithm Execution",
            PlanNodeCategory::Management => "Management/DDL Layer - Metadata Management",
            PlanNodeCategory::DataAccess => {
                "Data access layer - full text search and other data access operations"
            }
        }
    }

    /// Determine whether it is a leaf node (with no data dependencies).
    pub fn is_leaf(&self) -> bool {
        matches!(
            self,
            PlanNodeCategory::Access | PlanNodeCategory::DataAccess
        )
    }

    /// Determine whether it is a root node (with no downstream dependencies).
    pub fn is_root(&self) -> bool {
        matches!(
            self,
            PlanNodeCategory::ControlFlow | PlanNodeCategory::DataProcessing
        )
    }

    /// Determine whether parallel execution is supported.
    pub fn supports_parallelism(&self) -> bool {
        matches!(
            self,
            PlanNodeCategory::Operation
                | PlanNodeCategory::DataProcessing
                | PlanNodeCategory::Algorithm
        )
    }
}

impl std::fmt::Display for PlanNodeCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}
