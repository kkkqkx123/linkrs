//! Memory Budget Allocator Module
//!
//! Allocates memory budgets to different stages of query execution based on
//! the cost model configuration and plan structure.
//!
//! ## Usage Examples
//!
//! ```rust
//! use graphdb::query::optimizer::strategy::MemoryBudgetAllocator;
//! use graphdb::query::optimizer::cost::CostModelConfig;
//!
//! let allocator = MemoryBudgetAllocator::with_config(
//!     100 * 1024 * 1024, // 100MB total budget
//!     CostModelConfig::default(),
//! );
//!
//! let budgets = allocator.allocate_budget(&plan_root);
//! ```

use std::collections::HashMap;

use crate::query::optimizer::cost::CostModelConfig;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::PlanNodeEnum;

/// Unique identifier for plan nodes
pub type NodeId = u64;

/// Memory budget allocation result
#[derive(Debug, Clone)]
pub struct MemoryBudgetAllocation {
    /// Node identifier
    pub node_id: NodeId,
    /// Allocated memory budget in bytes
    pub budget_bytes: usize,
    /// Estimated memory requirement in bytes
    pub estimated_requirement: usize,
    /// Allocation priority (higher = more important)
    pub priority: u32,
}

/// Operator implementation strategy based on memory budget
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorImplementation {
    /// Standard in-memory implementation
    InMemory,
    /// External/memory-efficient implementation
    External,
    /// Hybrid approach (partial in-memory, partial external)
    Hybrid,
}

/// Memory Budget Allocator
///
/// Allocates memory budgets to query plan nodes based on their requirements
/// and the global memory constraints.
#[derive(Debug, Clone)]
pub struct MemoryBudgetAllocator {
    /// Total memory budget in bytes
    total_budget: usize,
    /// Cost model configuration
    config: CostModelConfig,
    /// Default row size for memory estimation (bytes)
    default_row_size: usize,
}

impl MemoryBudgetAllocator {
    /// Create a new memory budget allocator
    pub fn new(total_budget: usize) -> Self {
        Self {
            total_budget,
            config: CostModelConfig::default(),
            default_row_size: 64,
        }
    }

    /// Create with specific configuration
    pub fn with_config(total_budget: usize, config: CostModelConfig) -> Self {
        Self {
            total_budget,
            config,
            default_row_size: 64,
        }
    }

    /// Set default row size for estimation
    pub fn with_row_size(mut self, row_size: usize) -> Self {
        self.default_row_size = row_size;
        self
    }

    /// Allocate memory budget to all nodes in the plan
    ///
    /// Returns a map from node ID to allocated budget
    pub fn allocate_budget(&self, plan: &PlanNodeEnum) -> HashMap<NodeId, MemoryBudgetAllocation> {
        let mut allocations = HashMap::new();
        let mut requirements = Vec::new();

        // First pass: collect all nodes and their requirements
        self.collect_requirements(plan, &mut requirements);

        // Calculate total requirement
        let total_required: usize = requirements.iter().map(|r| r.estimated_requirement).sum();

        // Second pass: allocate budgets
        if total_required <= self.total_budget {
            // Sufficient memory - allocate full requirements
            for req in requirements {
                allocations.insert(
                    req.node_id,
                    MemoryBudgetAllocation {
                        node_id: req.node_id,
                        budget_bytes: req.estimated_requirement,
                        estimated_requirement: req.estimated_requirement,
                        priority: req.priority,
                    },
                );
            }
        } else {
            // Insufficient memory - proportional allocation with priority weighting
            allocations = self.allocate_with_constraints(requirements, total_required);
        }

        allocations
    }

    /// Collect memory requirements from all nodes
    fn collect_requirements(&self, plan: &PlanNodeEnum, requirements: &mut Vec<MemoryRequirement>) {
        let node_id = self.get_node_id(plan);
        let (requirement, priority) = self.estimate_node_memory(plan);

        requirements.push(MemoryRequirement {
            node_id,
            estimated_requirement: requirement,
            priority,
        });

        // Recursively collect from children
        for child in self.get_children(plan) {
            self.collect_requirements(child, requirements);
        }
    }

    /// Estimate memory requirement for a single node
    fn estimate_node_memory(&self, plan: &PlanNodeEnum) -> (usize, u32) {
        match plan {
            PlanNodeEnum::Sort(_node) => {
                // Sorting needs to buffer all input rows
                let rows = self.estimate_input_rows(plan);
                let memory = rows * self.default_row_size;
                (memory, 100) // High priority - sorting is memory-intensive
            }
            PlanNodeEnum::HashInnerJoin(_) | PlanNodeEnum::HashLeftJoin(_) => {
                // Hash join needs hash table for left input
                let rows = self.estimate_input_rows(plan);
                let memory = rows * self.default_row_size * 2; // Hash table overhead
                (memory, 90)
            }
            PlanNodeEnum::Aggregate(_) => {
                // Aggregation needs hash table or sort buffer
                let rows = self.estimate_input_rows(plan);
                // Estimate number of groups (heuristic: 10% of input)
                let groups = (rows / 10).max(10);
                let memory = groups * self.default_row_size * 2;
                (memory, 80)
            }
            PlanNodeEnum::Limit(node) => {
                // Limit only needs to buffer limited rows
                let limit = node.count() as usize;
                let memory = limit * self.default_row_size;
                (memory, 50) // Lower priority
            }
            PlanNodeEnum::Filter(_) => {
                // Filter is streaming, minimal memory
                (self.default_row_size * 10, 30)
            }
            PlanNodeEnum::Project(_) => {
                // Projection is streaming, minimal memory
                (self.default_row_size * 10, 30)
            }
            PlanNodeEnum::ScanVertices(_) | PlanNodeEnum::ScanEdges(_) => {
                // Scan is streaming
                (self.default_row_size * 10, 20)
            }
            PlanNodeEnum::IndexScan(_) => {
                // Index scan is streaming
                (self.default_row_size * 10, 20)
            }
            _ => {
                // Default estimate
                (self.default_row_size * 100, 50)
            }
        }
    }

    /// Estimate input rows for a node
    fn estimate_input_rows(&self, plan: &PlanNodeEnum) -> usize {
        // This is a simplified estimation
        // In practice, this would use statistics
        match plan {
            PlanNodeEnum::Sort(_) => 10000,
            PlanNodeEnum::InnerJoin(_) => 10000,
            PlanNodeEnum::LeftJoin(_) => 10000,
            PlanNodeEnum::Aggregate(_) => 10000,
            _ => 1000,
        }
    }

    /// Get children of a node
    fn get_children<'a>(&self, plan: &'a PlanNodeEnum) -> Vec<&'a PlanNodeEnum> {
        match plan {
            PlanNodeEnum::Sort(node) => vec![node.input()],
            PlanNodeEnum::InnerJoin(node) => {
                vec![node.left_input(), node.right_input()]
            }
            PlanNodeEnum::LeftJoin(node) => {
                vec![node.left_input(), node.right_input()]
            }
            PlanNodeEnum::Aggregate(node) => vec![node.input()],
            PlanNodeEnum::Limit(node) => vec![node.input()],
            PlanNodeEnum::Filter(node) => vec![node.input()],
            PlanNodeEnum::Project(node) => vec![node.input()],
            _ => vec![],
        }
    }

    /// Get unique identifier for a node
    ///
    /// Uses the plan node's unique ID (i64) converted to u64.
    /// This is stable across moves unlike pointer addresses.
    fn get_node_id(&self, plan: &PlanNodeEnum) -> NodeId {
        // Use the plan node's unique ID instead of pointer address
        // This ensures stability even when the plan is moved
        plan.id() as u64
    }

    /// Allocate budgets with constraints (when total requirement > budget)
    fn allocate_with_constraints(
        &self,
        requirements: Vec<MemoryRequirement>,
        _total_required: usize,
    ) -> HashMap<NodeId, MemoryBudgetAllocation> {
        let mut allocations = HashMap::new();

        // Calculate priority-weighted allocation
        let total_priority: u32 = requirements.iter().map(|r| r.priority).sum();

        for req in requirements {
            // Weight by priority
            let priority_ratio = req.priority as f64 / total_priority as f64;
            let allocated = (self.total_budget as f64 * priority_ratio) as usize;

            // Ensure minimum allocation for critical operations
            let min_allocation = if req.priority >= 80 {
                req.estimated_requirement.min(self.total_budget / 4)
            } else {
                1024 // Minimum 1KB
            };

            let final_allocation = allocated.max(min_allocation);

            allocations.insert(
                req.node_id,
                MemoryBudgetAllocation {
                    node_id: req.node_id,
                    budget_bytes: final_allocation,
                    estimated_requirement: req.estimated_requirement,
                    priority: req.priority,
                },
            );
        }

        allocations
    }

    /// Select operator implementation based on budget
    pub fn select_operator_implementation(
        &self,
        plan: &PlanNodeEnum,
        budget: usize,
    ) -> OperatorImplementation {
        match plan {
            PlanNodeEnum::Sort(_) => {
                let required = self.estimate_input_rows(plan) * self.default_row_size;
                if budget < required / 4 {
                    OperatorImplementation::External
                } else if budget < required {
                    OperatorImplementation::Hybrid
                } else {
                    OperatorImplementation::InMemory
                }
            }
            PlanNodeEnum::InnerJoin(_) | PlanNodeEnum::LeftJoin(_) => {
                let required = self.estimate_input_rows(plan) * self.default_row_size * 2;
                if budget < required / 2 {
                    // Fall back to nested loop join
                    OperatorImplementation::External
                } else {
                    OperatorImplementation::InMemory
                }
            }
            PlanNodeEnum::Aggregate(_) => {
                let required = self.estimate_input_rows(plan) * self.default_row_size;
                if budget < required / 4 {
                    OperatorImplementation::External
                } else {
                    OperatorImplementation::InMemory
                }
            }
            _ => OperatorImplementation::InMemory,
        }
    }

    /// Check if plan can execute within budget
    pub fn can_execute_within_budget(&self, plan: &PlanNodeEnum) -> bool {
        let allocations = self.allocate_budget(plan);
        allocations
            .values()
            .all(|a| a.budget_bytes >= a.estimated_requirement / 4)
    }

    /// Get total budget
    pub fn total_budget(&self) -> usize {
        self.total_budget
    }

    /// Get memory pressure threshold from config
    pub fn memory_pressure_threshold(&self) -> usize {
        self.config.memory_pressure_threshold
    }
}

/// Internal structure for memory requirements
#[derive(Debug, Clone)]
struct MemoryRequirement {
    node_id: NodeId,
    estimated_requirement: usize,
    priority: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allocator_creation() {
        let allocator = MemoryBudgetAllocator::new(100 * 1024 * 1024);
        assert_eq!(allocator.total_budget(), 100 * 1024 * 1024);
    }

    #[test]
    fn test_allocator_with_config() {
        let config = CostModelConfig::default();
        let allocator = MemoryBudgetAllocator::with_config(50 * 1024 * 1024, config);
        assert_eq!(allocator.total_budget(), 50 * 1024 * 1024);
    }

    #[test]
    fn test_memory_pressure_threshold() {
        let config = CostModelConfig::default();
        let allocator = MemoryBudgetAllocator::with_config(100 * 1024 * 1024, config);
        assert_eq!(
            allocator.memory_pressure_threshold(),
            config.memory_pressure_threshold
        );
    }

    #[test]
    fn test_select_operator_implementation() {
        let allocator = MemoryBudgetAllocator::new(100 * 1024 * 1024);

        // Create a simple start node for testing
        let start_node = crate::query::planning::plan::core::nodes::StartNode::new();
        let plan = PlanNodeEnum::Start(start_node);

        // Start node should use in-memory
        let impl_choice = allocator.select_operator_implementation(&plan, 1024 * 1024);
        assert_eq!(impl_choice, OperatorImplementation::InMemory);
    }

    #[test]
    fn test_can_execute_within_budget() {
        let allocator = MemoryBudgetAllocator::new(100 * 1024 * 1024);

        let start_node = crate::query::planning::plan::core::nodes::StartNode::new();
        let plan = PlanNodeEnum::Start(start_node);

        // Simple plan should be executable
        assert!(allocator.can_execute_within_budget(&plan));
    }
}
