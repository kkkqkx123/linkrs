//! Basic Module of Search Strategy Development
//!
//! Define the basic types of search strategies and the selectors used for their selection.
//!
//! ## Cost-Based Optimization
//!
//! This module uses cost-based optimization (CBO) to select the optimal seek strategy.
//! It considers I/O costs, CPU costs, and selectivity estimates to choose between:
//! - VertexSeek: Direct vertex ID lookup
//! - IndexSeek: Tag/label index scan
//! - PropIndexSeek: Property index scan
//! - ScanSeek: Full table scan

use std::sync::Arc;

use crate::core::types::expr::visitor_checkers::PropertyContainsChecker;
use crate::core::types::Expression;
use crate::core::Value;
use crate::query::optimizer::cost::{CostModelConfig, SelectivityEstimator};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekStrategyType {
    VertexSeek,
    IndexSeek,
    PropIndexSeek,
    VariablePropIndexSeek,
    EdgeSeek,
    ScanSeek,
}

#[derive(Debug)]
pub struct SeekStrategyContext {
    pub space_id: u64,
    pub node_pattern: NodePattern,
    pub predicates: Vec<Expression>,
    pub estimated_rows: usize,
    pub available_indexes: Vec<IndexInfo>,
    pub total_vertices: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodePattern {
    pub vid: Option<Value>,
    pub labels: Vec<String>,
    pub properties: Vec<(String, Value)>,
}

#[derive(Debug, Clone)]
pub struct IndexInfo {
    pub name: String,
    pub target_type: String,
    pub target_name: String,
    pub properties: Vec<String>,
    pub selectivity: f32,
    pub field_count: usize,
    pub is_composite: bool,
    pub composite_targets: Vec<String>,
}

impl IndexInfo {
    pub fn new(
        name: String,
        target_type: String,
        target_name: String,
        properties: Vec<String>,
    ) -> Self {
        let field_count = properties.len();
        Self {
            name,
            target_type,
            target_name,
            properties,
            selectivity: 0.5,
            field_count,
            is_composite: false,
            composite_targets: Vec::new(),
        }
    }

    pub fn with_selectivity(mut self, selectivity: f32) -> Self {
        self.selectivity = selectivity;
        self
    }

    pub fn with_composite(mut self, targets: Vec<String>) -> Self {
        self.is_composite = true;
        self.composite_targets = targets;
        self
    }

    pub fn covers_labels(&self, labels: &[String]) -> bool {
        if self.is_composite {
            self.composite_targets.iter().all(|t| labels.contains(t))
        } else {
            labels.contains(&self.target_name)
        }
    }

    pub fn coverage_score(&self, labels: &[String]) -> f32 {
        if labels.is_empty() {
            return 0.0;
        }

        let covered = if self.is_composite {
            self.composite_targets
                .iter()
                .filter(|t| labels.contains(t))
                .count()
        } else {
            if labels.contains(&self.target_name) {
                1
            } else {
                0
            }
        };

        covered as f32 / labels.len() as f32
    }
}

#[derive(Debug)]
pub struct SeekResult {
    pub vertex_ids: Vec<Value>,
    pub strategy_used: SeekStrategyType,
    pub rows_scanned: usize,
}

impl SeekStrategyContext {
    pub fn new(space_id: u64, node_pattern: NodePattern, predicates: Vec<Expression>) -> Self {
        Self {
            space_id,
            node_pattern,
            predicates,
            estimated_rows: 0,
            available_indexes: Vec::new(),
            total_vertices: 10000,
        }
    }

    pub fn with_estimated_rows(mut self, rows: usize) -> Self {
        self.estimated_rows = rows;
        self
    }

    pub fn with_indexes(mut self, indexes: Vec<IndexInfo>) -> Self {
        self.available_indexes = indexes;
        self
    }

    pub fn with_total_vertices(mut self, total: usize) -> Self {
        self.total_vertices = total;
        self
    }

    pub fn has_explicit_vid(&self) -> bool {
        self.node_pattern.vid.is_some()
    }

    pub fn has_labels(&self) -> bool {
        !self.node_pattern.labels.is_empty()
    }

    pub fn has_predicates(&self) -> bool {
        !self.predicates.is_empty()
    }

    pub fn get_index_for_labels(&self, labels: &[String]) -> Option<&IndexInfo> {
        self.available_indexes
            .iter()
            .find(|idx| idx.target_type == "tag" && labels.contains(&idx.target_name))
    }

    pub fn get_index_for_property(&self, property: &str) -> Option<&IndexInfo> {
        self.available_indexes
            .iter()
            .find(|idx| idx.properties.contains(&property.to_string()))
    }

    pub fn has_property_predicates(&self) -> bool {
        self.predicates
            .iter()
            .any(|pred| matches!(pred, Expression::Binary { .. }))
    }

    pub fn has_index_for_properties(&self) -> bool {
        !self.available_indexes.is_empty() && !self.predicates.is_empty()
    }
}

#[derive(Debug)]
pub struct SeekStrategySelector {
    use_index_threshold: usize,
    scan_threshold: usize,
    cost_config: CostModelConfig,
    selectivity_estimator: Option<Arc<SelectivityEstimator>>,
}

impl SeekStrategySelector {
    pub fn new() -> Self {
        Self {
            use_index_threshold: 1000,
            scan_threshold: 10000,
            cost_config: CostModelConfig::default(),
            selectivity_estimator: None,
        }
    }

    pub fn with_thresholds(mut self, use_index: usize, scan: usize) -> Self {
        self.use_index_threshold = use_index;
        self.scan_threshold = scan;
        self
    }

    pub fn with_cost_config(mut self, config: CostModelConfig) -> Self {
        self.cost_config = config;
        self
    }

    pub fn with_selectivity_estimator(mut self, estimator: Arc<SelectivityEstimator>) -> Self {
        self.selectivity_estimator = Some(estimator);
        self
    }

    pub fn select_best_index<'a>(
        &self,
        indexes: &'a [IndexInfo],
        predicates: &[Expression],
    ) -> Option<&'a IndexInfo> {
        if indexes.is_empty() {
            return None;
        }

        let candidate_indexes: Vec<&IndexInfo> = indexes
            .iter()
            .filter(|idx| {
                idx.properties.iter().any(|prop| {
                    predicates.iter().any(|pred| {
                        PropertyContainsChecker::check(pred, std::slice::from_ref(prop))
                    })
                })
            })
            .collect();

        if candidate_indexes.is_empty() {
            return indexes.iter().min_by_key(|idx| idx.field_count);
        }

        candidate_indexes.into_iter().min_by(|a, b| {
            let field_cmp = a.field_count.cmp(&b.field_count);
            if field_cmp != std::cmp::Ordering::Equal {
                return field_cmp;
            }
            b.selectivity
                .partial_cmp(&a.selectivity)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    pub fn select_strategy(&self, context: &SeekStrategyContext) -> SeekStrategyType {
        self.select_strategy_with_estimation(context).strategy_type
    }

    pub fn select_strategy_with_estimation(
        &self,
        context: &SeekStrategyContext,
    ) -> StrategySelection {
        let candidates = self.evaluate_all_strategies(context);

        candidates
            .into_iter()
            .flatten()
            .min_by(|a, b| {
                a.cost
                    .partial_cmp(&b.cost)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|e| StrategySelection {
                strategy_type: e.strategy_type,
                estimated_rows: e.estimated_rows,
            })
            .unwrap_or(StrategySelection {
                strategy_type: SeekStrategyType::ScanSeek,
                estimated_rows: context.total_vertices,
            })
    }

    fn evaluate_all_strategies(
        &self,
        context: &SeekStrategyContext,
    ) -> Vec<Option<StrategyEvaluation>> {
        vec![
            self.evaluate_vertex_seek(context),
            self.evaluate_index_seek(context),
            self.evaluate_prop_index_seek(context),
            self.evaluate_scan_seek(context),
        ]
    }

    fn evaluate_vertex_seek(&self, context: &SeekStrategyContext) -> Option<StrategyEvaluation> {
        if !context.has_explicit_vid() {
            return None;
        }

        let cost = self.cost_config.random_page_cost;
        Some(StrategyEvaluation {
            strategy_type: SeekStrategyType::VertexSeek,
            cost,
            estimated_rows: 1,
        })
    }

    fn evaluate_index_seek(&self, context: &SeekStrategyContext) -> Option<StrategyEvaluation> {
        let index = context.get_index_for_labels(&context.node_pattern.labels)?;
        let total_rows = context.total_vertices;
        let selectivity = index.selectivity as f64;

        let cost = self.cost_config.random_page_cost * (total_rows as f64 * selectivity)
            + self.cost_config.cpu_index_tuple_cost * total_rows as f64 * selectivity;
        let estimated_rows = (total_rows as f64 * selectivity) as usize;

        Some(StrategyEvaluation {
            strategy_type: SeekStrategyType::IndexSeek,
            cost,
            estimated_rows,
        })
    }

    fn evaluate_prop_index_seek(
        &self,
        context: &SeekStrategyContext,
    ) -> Option<StrategyEvaluation> {
        if !context.has_property_predicates() || !context.has_index_for_properties() {
            return None;
        }

        let index = self.select_best_index(&context.available_indexes, &context.predicates)?;
        let selectivity = self.estimate_predicate_selectivity(context);
        let total_rows = context.total_vertices;

        let cost = self.cost_config.random_page_cost * (total_rows as f64 * selectivity)
            + self.cost_config.cpu_operator_cost
                * index.field_count as f64
                * total_rows as f64
                * selectivity;
        let estimated_rows = (total_rows as f64 * selectivity) as usize;

        Some(StrategyEvaluation {
            strategy_type: SeekStrategyType::PropIndexSeek,
            cost,
            estimated_rows,
        })
    }

    fn evaluate_scan_seek(&self, context: &SeekStrategyContext) -> Option<StrategyEvaluation> {
        let total_rows = context.total_vertices;
        let cost = self.cost_config.seq_page_cost * total_rows as f64
            + self.cost_config.cpu_tuple_cost * total_rows as f64;

        Some(StrategyEvaluation {
            strategy_type: SeekStrategyType::ScanSeek,
            cost,
            estimated_rows: total_rows,
        })
    }

    fn estimate_predicate_selectivity(&self, context: &SeekStrategyContext) -> f64 {
        if let Some(ref estimator) = self.selectivity_estimator {
            let mut total_selectivity = 1.0;
            for pred in &context.predicates {
                let sel = estimator.estimate_from_expression(pred, None);
                total_selectivity *= sel;
            }
            return total_selectivity;
        }

        if context.predicates.len() == 1 {
            return 0.1;
        }

        0.1_f64.powi(context.predicates.len() as i32)
    }
}

#[derive(Debug, Clone)]
struct StrategyEvaluation {
    strategy_type: SeekStrategyType,
    cost: f64,
    estimated_rows: usize,
}

#[derive(Debug, Clone)]
pub struct StrategySelection {
    pub strategy_type: SeekStrategyType,
    pub estimated_rows: usize,
}

impl Default for SeekStrategySelector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_info_creation() {
        let index = IndexInfo::new(
            "person_name_idx".to_string(),
            "tag".to_string(),
            "person".to_string(),
            vec!["name".to_string()],
        );
        assert_eq!(index.name, "person_name_idx");
        assert_eq!(index.field_count, 1);
        assert!(!index.is_composite);
    }

    #[test]
    fn test_index_info_with_composite() {
        let index = IndexInfo::new(
            "composite_idx".to_string(),
            "tag".to_string(),
            "person".to_string(),
            vec!["name".to_string(), "age".to_string()],
        )
        .with_composite(vec!["person".to_string(), "employee".to_string()]);

        assert!(index.is_composite);
        assert!(index.covers_labels(&["person".to_string(), "employee".to_string()]));
        assert!(!index.covers_labels(&["person".to_string(), "other".to_string()]));
    }

    #[test]
    fn test_seek_strategy_context() {
        let node_pattern = NodePattern {
            vid: Some(Value::Int(1)),
            labels: vec!["person".to_string()],
            properties: vec![],
        };
        let context = SeekStrategyContext::new(1, node_pattern, vec![]);

        assert!(context.has_explicit_vid());
        assert!(context.has_labels());
        assert!(!context.has_predicates());
    }

    #[test]
    fn test_seek_strategy_selector_vertex_seek() {
        let selector = SeekStrategySelector::new();

        let node_pattern = NodePattern {
            vid: Some(Value::Int(1)),
            labels: vec![],
            properties: vec![],
        };
        let context = SeekStrategyContext::new(1, node_pattern, vec![]);

        let strategy = selector.select_strategy(&context);
        assert_eq!(strategy, SeekStrategyType::VertexSeek);
    }

    #[test]
    fn test_seek_strategy_selector_scan_seek() {
        let selector = SeekStrategySelector::new();

        let node_pattern = NodePattern {
            vid: None,
            labels: vec![],
            properties: vec![],
        };
        let context = SeekStrategyContext::new(1, node_pattern, vec![]);

        let strategy = selector.select_strategy(&context);
        assert_eq!(strategy, SeekStrategyType::ScanSeek);
    }
}
