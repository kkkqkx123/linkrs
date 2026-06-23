//! Path-related data structures

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::Expression;

/// Path information
#[derive(Debug, Clone)]
pub struct Path {
    pub alias: String,
    pub anonymous: bool,
    pub gen_path: bool, // Should a path be generated?
    pub path_type: PathYieldType,
    pub node_infos: Vec<NodeInfo>,
    pub edge_infos: Vec<EdgeInfo>,
    pub path_build: Option<Expression>, // Path construction expression
    pub is_pred: bool,                  // Whether it is a predicate
    pub is_anti_pred: bool,             // Is it a reverse predicate?
    pub compare_variables: Vec<String>, // Compare variables
    pub collect_variable: String,       // Collecting variables
    pub roll_up_apply: bool,            // Should the RollUp function be applied?
}

impl Path {
    /// Check whether it is the default path type.
    pub fn is_default_path(&self) -> bool {
        matches!(self.path_type, PathYieldType::Default)
    }

    /// Obtain a list of node information.
    pub fn node_infos(&self) -> &[NodeInfo] {
        &self.node_infos
    }

    /// Obtain the list of edge information.
    pub fn edge_infos(&self) -> &[EdgeInfo] {
        &self.edge_infos
    }
}

/// Path type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathYieldType {
    Default,
    Shortest,
    AllShortest,
    SingleSourceShortest,
    SingleSourceAllShortest,
}

/// Node information
#[derive(Debug, Clone, Default)]
pub struct NodeInfo {
    pub alias: String,
    pub labels: Vec<String>,
    pub props: Option<Expression>,
    pub anonymous: bool,
    pub filter: Option<Expression>, // Node filtering criteria
    pub tids: Vec<i32>,             // List of tag IDs
    pub label_props: Vec<Option<Expression>>, // Tag attributes
}

/// Edge Information
#[derive(Debug, Clone)]
pub struct EdgeInfo {
    pub alias: String,
    pub inner_alias: String, // Internal alias
    pub types: Vec<String>,
    pub props: Option<ContextualExpression>,
    pub anonymous: bool,
    pub filter: Option<ContextualExpression>, // Border filter criteria
    pub direction: Direction,                 // Side direction
    pub range: Option<MatchStepRange>,        // Range of steps
    pub edge_types: Vec<i32>,                 // Edge Type ID
}

/// The direction of the edge
#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Forward,       // ->
    Backward,      // <-
    Bidirectional, // -
}

/// Range of path step counts
#[derive(Debug, Clone)]
pub struct MatchStepRange {
    pub min: u32,
    pub max: u32,
}

impl MatchStepRange {
    pub fn new(min: u32, max: u32) -> Self {
        MatchStepRange { min, max }
    }

    pub fn min(&self) -> u32 {
        self.min
    }

    pub fn max(&self) -> u32 {
        self.max
    }
}
