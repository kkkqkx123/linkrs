//! Optimize the definition of decision types
//!
//! Defining the intermediate representation that converts from the AST (Abstract Syntax Tree) to the physical execution plan – the basis for optimization decisions.
//! These decisions are based on cost-optimized choices, but they do not include the specific structure of the plan tree.

use std::time::Instant;

/// The complete optimization decision
#[derive(Debug, Clone, PartialEq)]
pub struct OptimizationDecision {
    /// Traverse the starting point and make a decision
    pub traversal_start: TraversalStartDecision,
    /// Index selection decision
    pub index_selection: IndexSelectionDecision,
    /// Decision on the order of connections
    pub join_order: JoinOrderDecision,
    /// Sequence of applicable rewriting rules
    pub rewrite_rules: Vec<RewriteRuleId>,
    /// Statistical information version for decision-making
    pub stats_version: u64,
    /// Index version used during decision-making
    pub index_version: u64,
    /// Decision Timestamp
    pub created_at: Instant,
}

impl OptimizationDecision {
    /// Create new optimization decisions
    pub fn new(
        traversal_start: TraversalStartDecision,
        index_selection: IndexSelectionDecision,
        join_order: JoinOrderDecision,
        stats_version: u64,
        index_version: u64,
    ) -> Self {
        Self {
            traversal_start,
            index_selection,
            join_order,
            rewrite_rules: Vec::new(),
            stats_version,
            index_version,
            created_at: Instant::now(),
        }
    }

    /// Check whether the decision is still valid.
    pub fn is_valid(&self, current_stats_version: u64, current_index_version: u64) -> bool {
        self.stats_version == current_stats_version && self.index_version == current_index_version
    }

    /// Obtain the decision-making age (in seconds)
    pub fn age_secs(&self) -> u64 {
        self.created_at.elapsed().as_secs()
    }
}

/// Iterative Starting Point Selection Decision
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraversalStartDecision {
    /// Name of the starting node variable
    pub start_variable: String,
    /// Access path type
    pub access_path: AccessPath,
    /// Estimated selectivity (represented as an integer to avoid issues with floating-point precision)
    pub estimated_selectivity_scaled: u64, // Actual value = This value / 1,000,000
    /// Estimated cost (expressed as an integer)
    pub estimated_cost_scaled: u64, // Actual value = This value / 1,000,000
}

impl TraversalStartDecision {
    /// Making a decision to establish a new starting point for the traversal process
    pub fn new(
        start_variable: String,
        access_path: AccessPath,
        estimated_selectivity: f64,
        estimated_cost: f64,
    ) -> Self {
        Self {
            start_variable,
            access_path,
            estimated_selectivity_scaled: (estimated_selectivity * 1_000_000.0) as u64,
            estimated_cost_scaled: (estimated_cost * 1_000_000.0) as u64,
        }
    }

    /// Obtain an estimate of the selectivity
    pub fn estimated_selectivity(&self) -> f64 {
        self.estimated_selectivity_scaled as f64 / 1_000_000.0
    }

    /// Obtain the estimated cost
    pub fn estimated_cost(&self) -> f64 {
        self.estimated_cost_scaled as f64 / 1_000_000.0
    }
}

/// Access Path Type
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AccessPath {
    /// Explicit VID specification
    ExplicitVid {
        /// Description of the VID expression (simplified representation)
        vid_description: String,
    },
    /// Index scan
    IndexScan {
        /// Index name
        index_name: String,
        /// Attribute name
        property_name: String,
        /// Predicate description
        predicate_description: String,
    },
    /// Tag Index
    TagIndex {
        /// Tag name
        tag_name: String,
    },
    /// Full table scan
    FullScan {
        /// Entity type
        entity_type: EntityType,
    },
    /// Variable binding
    VariableBinding {
        /// Name of the source variable
        source_variable: String,
    },
}

/// Entity type
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntityType {
    /// vertex
    Vertex {
        /// Tag name (optional)
        tag_name: Option<String>,
    },
    /// edge
    Edge {
        /// Edge type (optional)
        edge_type: Option<String>,
    },
}

/// Index Selection Decision
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IndexSelectionDecision {
    /// Index selection for each entity type
    pub entity_indexes: Vec<EntityIndexChoice>,
}

impl IndexSelectionDecision {
    /// Create an empty index selection decision
    pub fn empty() -> Self {
        Self {
            entity_indexes: Vec::new(),
        }
    }

    /// Add entity index selection option
    pub fn add_choice(&mut self, choice: EntityIndexChoice) {
        self.entity_indexes.push(choice);
    }
}

/// Entity index selection
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntityIndexChoice {
    /// Entity type (label or edge type)
    pub entity_name: String,
    /// Selected index
    pub selected_index: IndexChoice,
    /// Estimated selectiveivity (scaling values)
    pub selectivity_scaled: u64,
}

impl EntityIndexChoice {
    /// Option to create a new entity index
    pub fn new(entity_name: String, selected_index: IndexChoice, selectivity: f64) -> Self {
        Self {
            entity_name,
            selected_index,
            selectivity_scaled: (selectivity * 1_000_000.0) as u64,
        }
    }

    /// Obtain selectivity
    pub fn selectivity(&self) -> f64 {
        self.selectivity_scaled as f64 / 1_000_000.0
    }
}

/// Index selection
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IndexChoice {
    /// Primary key index
    PrimaryKey,
    /// Attribute index
    PropertyIndex {
        /// attribute name
        property_name: String,
        /// Index name
        index_name: String,
    },
    /// Composite index
    CompositeIndex {
        /// List of attribute names
        property_names: Vec<String>,
        /// Index name
        index_name: String,
    },
    /// No available index.
    None,
}

/// Connection Sequential Decision Making
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JoinOrderDecision {
    /// Connection order (sequence of variable names)
    pub join_order: Vec<String>,
    /// The choice of algorithm for each connection
    pub join_algorithms: Vec<JoinAlgorithm>,
}

impl JoinOrderDecision {
    /// Create an empty decision-making process for determining the order of connections.
    pub fn empty() -> Self {
        Self {
            join_order: Vec::new(),
            join_algorithms: Vec::new(),
        }
    }

    /// Add the connection steps
    pub fn add_join_step(&mut self, variable: String, algorithm: JoinAlgorithm) {
        self.join_order.push(variable);
        self.join_algorithms.push(algorithm);
    }
}

/// Connection algorithms
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JoinAlgorithm {
    /// Hash link
    HashJoin {
        /// Constructing the names for the side variables
        build_side: String,
        /// Variable name on the detection side
        probe_side: String,
    },
    /// Nested loop connection
    NestedLoopJoin {
        /// Variable name for the appearance
        outer: String,
        /// Internal table variable name
        inner: String,
    },
    /// Index join
    IndexJoin {
        /// Variable name on the side with the index
        indexed_side: String,
    },
}

/// Rewrite Rule ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RewriteRuleId {
    /// Predicate Pushdown
    PushFilterDown,
    /// Projection pushdown
    PushProjectDown,
    /// LIMIT push-down
    PushLimitDown,
    /// Operation merge
    MergeOperations,
    /// Redundancy elimination
    EliminateRedundancy,
    /// Aggregation optimization
    AggregateOptimization,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_traversal_start_decision() {
        let decision = TraversalStartDecision::new(
            "n".to_string(),
            AccessPath::TagIndex {
                tag_name: "Person".to_string(),
            },
            0.1,
            100.0,
        );

        assert_eq!(decision.start_variable, "n");
        assert!((decision.estimated_selectivity() - 0.1).abs() < 0.0001);
        assert!((decision.estimated_cost() - 100.0).abs() < 0.0001);
    }

    #[test]
    fn test_optimization_decision_validity() {
        let decision = OptimizationDecision::new(
            TraversalStartDecision::new(
                "n".to_string(),
                AccessPath::FullScan {
                    entity_type: EntityType::Vertex { tag_name: None },
                },
                1.0,
                1000.0,
            ),
            IndexSelectionDecision::empty(),
            JoinOrderDecision::empty(),
            1,
            1,
        );

        assert!(decision.is_valid(1, 1));
        assert!(!decision.is_valid(2, 1));
        assert!(!decision.is_valid(1, 2));
    }
}
