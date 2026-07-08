//! Streaming Execution Decision Logic
//!
//! Determines whether a query plan is suitable for streaming execution
//! and provides utilities to convert between streaming and materialized modes.

use crate::query::planning::plan::PlanNodeEnum;

/// Streaming Execution Mode Selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Use traditional materialized execution (default)
    Materialized,
    /// Use streaming pull-based execution
    Streaming,
}

/// Configuration for streaming execution decisions
#[derive(Debug, Clone)]
pub struct StreamingDecisionConfig {
    /// Enable streaming execution mode
    pub enabled: bool,
    /// Maximum plan depth to support streaming
    pub max_depth: usize,
    /// Supported operations for streaming mode
    pub supported_ops: SupportedOps,
}

/// Set of operations supported in streaming mode
#[derive(Debug, Clone)]
pub struct SupportedOps {
    pub scan: bool,
    pub filter: bool,
    pub project: bool,
    pub limit: bool,
    pub aggregate: bool,
    pub sort: bool,
    pub join: bool,
    pub distinct: bool,
    pub set_ops: bool,
}

impl Default for SupportedOps {
    fn default() -> Self {
        Self {
            scan: true,
            filter: true,
            project: true,
            limit: true,
            aggregate: true,
            sort: true,
            join: true,
            distinct: true,
            set_ops: true,
        }
    }
}

impl Default for StreamingDecisionConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Default to materialized mode
            max_depth: 100,
            supported_ops: SupportedOps::default(),
        }
    }
}

impl StreamingDecisionConfig {
    /// Create a config that enables streaming mode
    pub fn streaming_enabled() -> Self {
        Self {
            enabled: true,
            ..Default::default()
        }
    }

    /// Check if a plan node type is supported in streaming mode
    pub fn is_supported_node(&self, node: &PlanNodeEnum) -> bool {
        match node {
            // Supported source operations
            PlanNodeEnum::ScanVertices(_) | PlanNodeEnum::ScanEdges(_) => self.supported_ops.scan,

            // Supported single-input operations
            PlanNodeEnum::Filter(_) => self.supported_ops.filter,
            PlanNodeEnum::Project(_) => self.supported_ops.project,
            PlanNodeEnum::Limit(_) => self.supported_ops.limit,
            PlanNodeEnum::Dedup(_) => self.supported_ops.distinct,

            // Supported stateful operations
            PlanNodeEnum::Aggregate(_) => self.supported_ops.aggregate,
            PlanNodeEnum::Sort(_) => self.supported_ops.sort,
            PlanNodeEnum::Window(_) => self.supported_ops.aggregate,

            // Supported join operations
            PlanNodeEnum::InnerJoin(_)
            | PlanNodeEnum::LeftJoin(_)
            | PlanNodeEnum::CrossJoin(_) => self.supported_ops.join,

            // Supported set operations
            PlanNodeEnum::Union(_)
            | PlanNodeEnum::Intersect(_)
            | PlanNodeEnum::Minus(_) => self.supported_ops.set_ops,

            // Unsupported for streaming
            _ => false,
        }
    }
}

/// Determine whether a query plan should use streaming execution
///
/// # Arguments
/// * `plan_root` - Root node of the execution plan
/// * `config` - Streaming decision configuration
///
/// # Returns
/// * ExecutionMode::Streaming if the plan is suitable and config allows
/// * ExecutionMode::Materialized otherwise
pub fn decide_execution_mode(plan_root: &PlanNodeEnum, config: &StreamingDecisionConfig) -> ExecutionMode {
    if !config.enabled {
        return ExecutionMode::Materialized;
    }

    if is_plan_streaming_compatible(plan_root, config, 0) {
        ExecutionMode::Streaming
    } else {
        ExecutionMode::Materialized
    }
}

/// Recursively check if a plan tree is compatible with streaming execution
fn is_plan_streaming_compatible(
    node: &PlanNodeEnum,
    config: &StreamingDecisionConfig,
    depth: usize,
) -> bool {
    // Check depth limit
    if depth > config.max_depth {
        return false;
    }

    // Check if this node type is supported
    if !config.is_supported_node(node) {
        return false;
    }

    // Recursively check children
    for child in node.children() {
        if !is_plan_streaming_compatible(child, config, depth + 1) {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_disabled() {
        let config = StreamingDecisionConfig::default();
        assert!(!config.enabled);
    }

    #[test]
    fn test_streaming_enabled_config() {
        let config = StreamingDecisionConfig::streaming_enabled();
        assert!(config.enabled);
        assert!(config.supported_ops.scan);
        assert!(config.supported_ops.filter);
    }

    #[test]
    fn test_default_supported_ops() {
        let ops = SupportedOps::default();
        assert!(ops.scan);
        assert!(ops.filter);
        assert!(ops.join);
        assert!(ops.aggregate);
    }
}
