//! Variable-Length Path Planner
//!
//! Specialized planner for variable-length path patterns `[:TYPE*min..max]`.
//!
//! ## Features
//!
//! - Exact hop planning: `*n`
//! - Range hop planning: `*min..max`
//! - Unbounded planning: `*` or `*+`
//! - Path pruning strategies
//! - BFS-based shortest path optimization

use std::collections::HashSet;

use crate::core::types::graph_schema::EdgeDirection;
use crate::core::Value;
use crate::query::parser::ast::pattern::{EdgeRange, RepetitionType};

pub type PlannerError = String;

#[derive(Debug, Clone)]
pub struct VLPConfig {
    pub max_unbounded_depth: usize,
    pub enable_path_pruning: bool,
    pub use_bfs_for_shortest: bool,
    pub batch_size: usize,
    pub max_path_memory_mb: usize,
    pub unbounded_timeout_secs: Option<u64>,
}

impl Default for VLPConfig {
    fn default() -> Self {
        Self {
            max_unbounded_depth: 100,
            enable_path_pruning: true,
            use_bfs_for_shortest: true,
            batch_size: 1000,
            max_path_memory_mb: 512,
            unbounded_timeout_secs: Some(30),
        }
    }
}

impl VLPConfig {
    pub fn for_production() -> Self {
        Self {
            max_unbounded_depth: 100,
            enable_path_pruning: true,
            use_bfs_for_shortest: true,
            batch_size: 1000,
            max_path_memory_mb: 512,
            unbounded_timeout_secs: Some(30),
        }
    }

    pub fn for_analytics() -> Self {
        Self {
            max_unbounded_depth: 500,
            enable_path_pruning: true,
            use_bfs_for_shortest: true,
            batch_size: 5000,
            max_path_memory_mb: 2048,
            unbounded_timeout_secs: Some(300),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VariableLengthPathSpec {
    pub edge_types: Vec<String>,
    pub direction: EdgeDirection,
    pub range: EdgeRange,
    pub properties: Vec<(String, Value)>,
}

impl VariableLengthPathSpec {
    pub fn new(
        edge_types: Vec<String>,
        direction: EdgeDirection,
        range: EdgeRange,
        properties: Vec<(String, Value)>,
    ) -> Self {
        Self {
            edge_types,
            direction,
            range,
            properties,
        }
    }

    pub fn min_hops(&self) -> usize {
        self.range.min.unwrap_or(1)
    }

    pub fn max_hops(&self, default_max: usize) -> usize {
        self.range.max.unwrap_or(default_max)
    }

    pub fn is_bounded(&self) -> bool {
        self.range.max.is_some()
    }

    pub fn is_single_hop(&self) -> bool {
        self.range.min == Some(1) && self.range.max == Some(1)
    }

    pub fn is_zero_inclusive(&self) -> bool {
        self.range.min == Some(0)
    }
}

#[derive(Debug, Clone)]
pub struct VariableLengthPathPlan {
    pub strategy: VLPStrategy,
    pub edge_types: Vec<String>,
    pub direction: EdgeDirection,
    pub min_hops: usize,
    pub max_hops: usize,
    pub properties: Vec<(String, Value)>,
    pub pruning: Option<PruningConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VLPStrategy {
    Exact,
    Range,
    Unbounded,
    BFS,
    Iterative,
}

#[derive(Debug, Clone)]
pub struct PruningConfig {
    pub strategy: PruningStrategy,
    pub enable_cycle_detection: bool,
    pub max_visited_nodes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PruningStrategy {
    None,
    NoCycles,
    ShortestPath,
    PredicateBased,
}

pub struct VariableLengthPathPlanner {
    config: VLPConfig,
}

impl VariableLengthPathPlanner {
    pub fn new() -> Self {
        Self {
            config: VLPConfig::default(),
        }
    }

    pub fn with_config(config: VLPConfig) -> Self {
        Self { config }
    }

    pub fn plan(
        &self,
        spec: &VariableLengthPathSpec,
    ) -> Result<VariableLengthPathPlan, PlannerError> {
        let min_hops = spec.min_hops();
        let max_hops = spec.max_hops(self.config.max_unbounded_depth);

        if min_hops > max_hops {
            return Err(format!(
                "Invalid range: min ({}) > max ({})",
                min_hops, max_hops
            ));
        }

        let strategy = self.select_strategy(min_hops, max_hops, spec.is_bounded());

        let pruning = if self.config.enable_path_pruning {
            Some(self.create_pruning_config(&strategy))
        } else {
            None
        };

        Ok(VariableLengthPathPlan {
            strategy,
            edge_types: spec.edge_types.clone(),
            direction: spec.direction,
            min_hops,
            max_hops,
            properties: spec.properties.clone(),
            pruning,
        })
    }

    fn select_strategy(&self, min_hops: usize, max_hops: usize, is_bounded: bool) -> VLPStrategy {
        if min_hops == max_hops {
            return VLPStrategy::Exact;
        }

        if !is_bounded {
            return VLPStrategy::Unbounded;
        }

        if self.config.use_bfs_for_shortest && min_hops == 1 {
            return VLPStrategy::BFS;
        }

        if max_hops - min_hops <= 3 {
            return VLPStrategy::Range;
        }

        VLPStrategy::Iterative
    }

    fn create_pruning_config(&self, strategy: &VLPStrategy) -> PruningConfig {
        let pruning_strategy = match strategy {
            VLPStrategy::BFS => PruningStrategy::ShortestPath,
            VLPStrategy::Unbounded => PruningStrategy::NoCycles,
            _ => PruningStrategy::NoCycles,
        };

        PruningConfig {
            strategy: pruning_strategy,
            enable_cycle_detection: true,
            max_visited_nodes: self.config.batch_size * 10,
        }
    }

    pub fn plan_from_repetition(
        &self,
        rep_type: &RepetitionType,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        properties: Vec<(String, Value)>,
    ) -> Result<VariableLengthPathPlan, PlannerError> {
        let range = match rep_type {
            RepetitionType::ZeroOrMore => EdgeRange::at_least(0),
            RepetitionType::OneOrMore => EdgeRange::at_least(1),
            RepetitionType::ZeroOrOne => EdgeRange::range(0, 1),
            RepetitionType::Exactly(n) => EdgeRange::fixed(*n),
            RepetitionType::Range(min, max) => EdgeRange::range(*min, *max),
        };

        let spec = VariableLengthPathSpec::new(edge_types, direction, range, properties);
        self.plan(&spec)
    }
}

impl Default for VariableLengthPathPlanner {
    fn default() -> Self {
        Self::new()
    }
}

pub struct PathPruner {
    strategy: PruningStrategy,
    visited: HashSet<Value>,
    best_length: Option<usize>,
    max_visited: usize,
}

impl PathPruner {
    pub fn new(strategy: PruningStrategy) -> Self {
        Self {
            strategy,
            visited: HashSet::new(),
            best_length: None,
            max_visited: usize::MAX,
        }
    }

    pub fn with_max_visited(mut self, max: usize) -> Self {
        self.max_visited = max;
        self
    }

    pub fn should_prune(&mut self, path: &[Value]) -> bool {
        match self.strategy {
            PruningStrategy::None => false,

            PruningStrategy::NoCycles => {
                let mut seen = HashSet::new();
                for vertex in path {
                    if !seen.insert(vertex.clone()) {
                        return true;
                    }
                }
                false
            }

            PruningStrategy::ShortestPath => {
                if let Some(best) = self.best_length {
                    if path.len() > best {
                        return true;
                    }
                }
                false
            }

            PruningStrategy::PredicateBased => false,
        }
    }

    pub fn should_prune_vertex(&mut self, vertex: &Value) -> bool {
        if self.visited.len() >= self.max_visited {
            return true;
        }

        match self.strategy {
            PruningStrategy::NoCycles | PruningStrategy::ShortestPath => {
                !self.visited.insert(vertex.clone())
            }
            _ => false,
        }
    }

    pub fn update_best(&mut self, length: usize) {
        if let Some(ref mut best) = self.best_length {
            if length < *best {
                *best = length;
            }
        } else {
            self.best_length = Some(length);
        }
    }

    pub fn reset(&mut self) {
        self.visited.clear();
        self.best_length = None;
    }

    pub fn visited_count(&self) -> usize {
        self.visited.len()
    }
}

pub struct PathExpansionStats {
    pub paths_found: usize,
    pub paths_pruned: usize,
    pub max_depth_reached: usize,
    pub vertices_visited: usize,
}

impl PathExpansionStats {
    pub fn new() -> Self {
        Self {
            paths_found: 0,
            paths_pruned: 0,
            max_depth_reached: 0,
            vertices_visited: 0,
        }
    }

    pub fn record_path(&mut self, depth: usize) {
        self.paths_found += 1;
        if depth > self.max_depth_reached {
            self.max_depth_reached = depth;
        }
    }

    pub fn record_prune(&mut self) {
        self.paths_pruned += 1;
    }

    pub fn record_vertex(&mut self) {
        self.vertices_visited += 1;
    }
}

impl Default for PathExpansionStats {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vlp_config_default() {
        let config = VLPConfig::default();
        assert_eq!(config.max_unbounded_depth, 100);
        assert!(config.enable_path_pruning);
        assert!(config.use_bfs_for_shortest);
    }

    #[test]
    fn test_vlp_spec_min_max_hops() {
        let spec = VariableLengthPathSpec::new(
            vec!["KNOWS".to_string()],
            EdgeDirection::Out,
            EdgeRange::range(1, 5),
            vec![],
        );

        assert_eq!(spec.min_hops(), 1);
        assert_eq!(spec.max_hops(100), 5);
        assert!(spec.is_bounded());
        assert!(!spec.is_single_hop());
    }

    #[test]
    fn test_vlp_spec_single_hop() {
        let spec = VariableLengthPathSpec::new(
            vec!["KNOWS".to_string()],
            EdgeDirection::Out,
            EdgeRange::fixed(1),
            vec![],
        );

        assert!(spec.is_single_hop());
    }

    #[test]
    fn test_vlp_spec_zero_inclusive() {
        let spec = VariableLengthPathSpec::new(
            vec!["KNOWS".to_string()],
            EdgeDirection::Out,
            EdgeRange::at_least(0),
            vec![],
        );

        assert!(spec.is_zero_inclusive());
    }

    #[test]
    fn test_vlp_planner_exact() {
        let planner = VariableLengthPathPlanner::new();
        let spec = VariableLengthPathSpec::new(
            vec!["KNOWS".to_string()],
            EdgeDirection::Out,
            EdgeRange::fixed(3),
            vec![],
        );

        let plan = planner.plan(&spec).unwrap();
        assert_eq!(plan.strategy, VLPStrategy::Exact);
        assert_eq!(plan.min_hops, 3);
        assert_eq!(plan.max_hops, 3);
    }

    #[test]
    fn test_vlp_planner_range() {
        let planner = VariableLengthPathPlanner::new();
        let spec = VariableLengthPathSpec::new(
            vec!["KNOWS".to_string()],
            EdgeDirection::Out,
            EdgeRange::range(1, 3),
            vec![],
        );

        let plan = planner.plan(&spec).unwrap();
        assert_eq!(plan.strategy, VLPStrategy::BFS);
        assert_eq!(plan.min_hops, 1);
        assert_eq!(plan.max_hops, 3);
    }

    #[test]
    fn test_vlp_planner_unbounded() {
        let planner = VariableLengthPathPlanner::new();
        let spec = VariableLengthPathSpec::new(
            vec!["KNOWS".to_string()],
            EdgeDirection::Out,
            EdgeRange::at_least(1),
            vec![],
        );

        let plan = planner.plan(&spec).unwrap();
        assert_eq!(plan.strategy, VLPStrategy::Unbounded);
        assert_eq!(plan.min_hops, 1);
        assert_eq!(plan.max_hops, 100);
    }

    #[test]
    fn test_path_pruner_no_cycles() {
        let mut pruner = PathPruner::new(PruningStrategy::NoCycles);

        let path = vec![
            Value::String("a".to_string()),
            Value::String("b".to_string()),
            Value::String("a".to_string()),
        ];

        assert!(pruner.should_prune(&path));
    }

    #[test]
    fn test_path_pruner_shortest_path() {
        let mut pruner = PathPruner::new(PruningStrategy::ShortestPath);

        pruner.update_best(3);

        let short_path = vec![
            Value::String("a".to_string()),
            Value::String("b".to_string()),
        ];
        assert!(!pruner.should_prune(&short_path));

        let long_path = vec![
            Value::String("a".to_string()),
            Value::String("b".to_string()),
            Value::String("c".to_string()),
            Value::String("d".to_string()),
        ];
        assert!(pruner.should_prune(&long_path));
    }

    #[test]
    fn test_path_pruner_vertex_tracking() {
        let mut pruner = PathPruner::new(PruningStrategy::NoCycles);

        assert!(!pruner.should_prune_vertex(&Value::String("a".to_string())));
        assert!(pruner.should_prune_vertex(&Value::String("a".to_string())));
        assert!(!pruner.should_prune_vertex(&Value::String("b".to_string())));

        assert_eq!(pruner.visited_count(), 2);
    }

    #[test]
    fn test_path_expansion_stats() {
        let mut stats = PathExpansionStats::new();

        stats.record_path(3);
        stats.record_path(5);
        stats.record_prune();
        stats.record_vertex();

        assert_eq!(stats.paths_found, 2);
        assert_eq!(stats.paths_pruned, 1);
        assert_eq!(stats.max_depth_reached, 5);
        assert_eq!(stats.vertices_visited, 1);
    }
}
