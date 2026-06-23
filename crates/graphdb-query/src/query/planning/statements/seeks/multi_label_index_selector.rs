//! Multi-Label Index Selector
//!
//! Handles index selection for multi-label node patterns like `(:Label1:Label2)`.
//!
//! ## Strategies
//!
//! 1. **Composite Index**: Use an index covering all labels
//! 2. **Single Index with Filter**: Use best single-label index, filter others
//! 3. **Index Intersection**: Intersect results from multiple indexes
//! 4. **Full Scan with Filter**: Scan all and filter by labels

use std::collections::HashMap;

use crate::core::types::Expression;
use crate::query::optimizer::cost::CostModelConfig;
use crate::query::planning::statements::seeks::seek_strategy_base::IndexInfo;

pub type SelectorError = String;

#[derive(Debug, Clone)]
pub enum MultiLabelStrategy {
    CompositeIndex {
        index: IndexInfo,
        covered_labels: Vec<String>,
    },
    SingleIndexWithFilter {
        index: IndexInfo,
        primary_label: String,
        filter_labels: Vec<String>,
        estimated_selectivity: f64,
    },
    IndexIntersection {
        indexes: Vec<IndexInfo>,
        labels: Vec<String>,
    },
    FullScanWithFilter {
        labels: Vec<String>,
    },
}

impl MultiLabelStrategy {
    pub fn estimate_cost(&self, cost_model: &CostModelConfig, total_vertices: usize) -> f64 {
        match self {
            Self::CompositeIndex {
                index,
                covered_labels: _,
            } => {
                let selectivity = index.selectivity as f64;
                cost_model.random_page_cost * (total_vertices as f64 * selectivity)
                    + cost_model.cpu_index_tuple_cost * total_vertices as f64 * selectivity
            }

            Self::SingleIndexWithFilter {
                index,
                filter_labels,
                estimated_selectivity,
                ..
            } => {
                let base_cost = cost_model.random_page_cost
                    * (total_vertices as f64 * index.selectivity as f64)
                    + cost_model.cpu_index_tuple_cost
                        * total_vertices as f64
                        * index.selectivity as f64;
                let filter_cost = cost_model.cpu_tuple_cost
                    * filter_labels.len() as f64
                    * (total_vertices as f64 * estimated_selectivity);
                base_cost + filter_cost
            }

            Self::IndexIntersection { indexes, .. } => {
                let scan_cost: f64 = indexes
                    .iter()
                    .map(|idx| {
                        cost_model.random_page_cost
                            * (total_vertices as f64 * idx.selectivity as f64)
                            + cost_model.cpu_index_tuple_cost
                                * total_vertices as f64
                                * idx.selectivity as f64
                    })
                    .sum();
                let intersection_cost =
                    cost_model.cpu_tuple_cost * indexes.len() as f64 * total_vertices as f64;
                scan_cost + intersection_cost
            }

            Self::FullScanWithFilter { labels } => {
                cost_model.seq_page_cost * total_vertices as f64
                    + cost_model.cpu_tuple_cost * labels.len() as f64 * total_vertices as f64
            }
        }
    }

    pub fn uses_index(&self) -> bool {
        !matches!(self, Self::FullScanWithFilter { .. })
    }

    pub fn get_primary_index(&self) -> Option<&IndexInfo> {
        match self {
            Self::CompositeIndex { index, .. } => Some(index),
            Self::SingleIndexWithFilter { index, .. } => Some(index),
            Self::IndexIntersection {
                indexes: index_infos,
                ..
            } => index_infos.first(),
            Self::FullScanWithFilter { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LabelStats {
    pub label: String,
    pub row_count: usize,
    pub selectivity: f64,
}

impl LabelStats {
    pub fn new(label: String, row_count: usize, total_vertices: usize) -> Self {
        let selectivity = if total_vertices > 0 {
            row_count as f64 / total_vertices as f64
        } else {
            0.5
        };
        Self {
            label,
            row_count,
            selectivity,
        }
    }
}

pub struct MultiLabelIndexSelector {
    label_stats: HashMap<String, LabelStats>,
    cost_model: CostModelConfig,
    total_vertices: usize,
}

impl MultiLabelIndexSelector {
    pub fn new() -> Self {
        Self {
            label_stats: HashMap::new(),
            cost_model: CostModelConfig::default(),
            total_vertices: 10000,
        }
    }

    pub fn with_cost_model(mut self, config: CostModelConfig) -> Self {
        self.cost_model = config;
        self
    }

    pub fn with_total_vertices(mut self, total: usize) -> Self {
        self.total_vertices = total;
        self
    }

    pub fn with_label_stats(mut self, stats: Vec<LabelStats>) -> Self {
        for stat in stats {
            self.label_stats.insert(stat.label.clone(), stat);
        }
        self
    }

    pub fn add_label_stats(&mut self, stats: LabelStats) {
        self.label_stats.insert(stats.label.clone(), stats);
    }

    pub fn select_strategy(
        &self,
        labels: &[String],
        indexes: &[IndexInfo],
        predicates: &[Expression],
    ) -> Result<MultiLabelStrategy, SelectorError> {
        if labels.is_empty() {
            return Ok(MultiLabelStrategy::FullScanWithFilter {
                labels: labels.to_vec(),
            });
        }

        if labels.len() == 1 {
            return self.select_single_label_strategy(&labels[0], indexes, predicates);
        }

        let strategies = self.evaluate_all_strategies(labels, indexes, predicates);

        strategies
            .into_iter()
            .min_by(|a, b| {
                let cost_a = a.estimate_cost(&self.cost_model, self.total_vertices);
                let cost_b = b.estimate_cost(&self.cost_model, self.total_vertices);
                cost_a
                    .partial_cmp(&cost_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .ok_or_else(|| "No valid strategy found".to_string())
    }

    fn select_single_label_strategy(
        &self,
        label: &str,
        indexes: &[IndexInfo],
        _predicates: &[Expression],
    ) -> Result<MultiLabelStrategy, SelectorError> {
        let index = indexes
            .iter()
            .find(|idx| idx.target_type == "tag" && idx.target_name == label)
            .cloned();

        if let Some(index) = index {
            return Ok(MultiLabelStrategy::SingleIndexWithFilter {
                index,
                primary_label: label.to_string(),
                filter_labels: vec![],
                estimated_selectivity: self.get_label_selectivity(label),
            });
        }

        Ok(MultiLabelStrategy::FullScanWithFilter {
            labels: vec![label.to_string()],
        })
    }

    fn evaluate_all_strategies(
        &self,
        labels: &[String],
        indexes: &[IndexInfo],
        _predicates: &[Expression],
    ) -> Vec<MultiLabelStrategy> {
        let mut strategies = Vec::new();

        if let Some(strategy) = self.try_composite_index(labels, indexes) {
            strategies.push(strategy);
        }

        if let Some(strategy) = self.try_single_index_with_filter(labels, indexes) {
            strategies.push(strategy);
        }

        if let Some(strategy) = self.try_index_intersection(labels, indexes) {
            strategies.push(strategy);
        }

        strategies.push(MultiLabelStrategy::FullScanWithFilter {
            labels: labels.to_vec(),
        });

        strategies
    }

    fn try_composite_index(
        &self,
        labels: &[String],
        indexes: &[IndexInfo],
    ) -> Option<MultiLabelStrategy> {
        indexes
            .iter()
            .find(|idx| idx.is_composite && idx.covers_labels(labels))
            .map(|index| MultiLabelStrategy::CompositeIndex {
                index: index.clone(),
                covered_labels: labels.to_vec(),
            })
    }

    fn try_single_index_with_filter(
        &self,
        labels: &[String],
        indexes: &[IndexInfo],
    ) -> Option<MultiLabelStrategy> {
        let mut best: Option<(IndexInfo, String, f64)> = None;

        for label in labels {
            if let Some(index) = indexes
                .iter()
                .find(|idx| idx.target_type == "tag" && idx.target_name == *label)
            {
                let selectivity = self.get_label_selectivity(label);
                let score = selectivity * index.coverage_score(labels) as f64;

                if best.as_ref().is_none_or(|(_, _, s)| score < *s) {
                    best = Some((index.clone(), label.clone(), score));
                }
            }
        }

        best.map(|(index, primary_label, score)| {
            let filter_labels: Vec<String> = labels
                .iter()
                .filter(|l| *l != &primary_label)
                .cloned()
                .collect();

            MultiLabelStrategy::SingleIndexWithFilter {
                index,
                primary_label,
                filter_labels,
                estimated_selectivity: score,
            }
        })
    }

    fn try_index_intersection(
        &self,
        labels: &[String],
        indexes: &[IndexInfo],
    ) -> Option<MultiLabelStrategy> {
        let matching_indexes: Vec<IndexInfo> = labels
            .iter()
            .filter_map(|label| {
                indexes
                    .iter()
                    .find(|idx| idx.target_type == "tag" && idx.target_name == *label)
                    .cloned()
            })
            .collect();

        if matching_indexes.len() < 2 {
            return None;
        }

        let total_selectivity: f64 = matching_indexes
            .iter()
            .map(|idx| idx.selectivity as f64)
            .product();

        if total_selectivity < 0.01 {
            return Some(MultiLabelStrategy::IndexIntersection {
                indexes: matching_indexes,
                labels: labels.to_vec(),
            });
        }

        None
    }

    fn get_label_selectivity(&self, label: &str) -> f64 {
        self.label_stats
            .get(label)
            .map(|s| s.selectivity)
            .unwrap_or(0.5)
    }
}

impl Default for MultiLabelIndexSelector {
    fn default() -> Self {
        Self::new()
    }
}

pub struct IndexRegistry {
    single_label_indexes: HashMap<String, IndexInfo>,
    composite_indexes: Vec<IndexInfo>,
    property_indexes: HashMap<(String, String), IndexInfo>,
}

impl IndexRegistry {
    pub fn new() -> Self {
        Self {
            single_label_indexes: HashMap::new(),
            composite_indexes: Vec::new(),
            property_indexes: HashMap::new(),
        }
    }

    pub fn register_single_label(&mut self, index: IndexInfo) {
        self.single_label_indexes
            .insert(index.target_name.clone(), index);
    }

    pub fn register_composite(&mut self, index: IndexInfo) {
        self.composite_indexes.push(index);
    }

    pub fn register_property(&mut self, label: String, property: String, index: IndexInfo) {
        self.property_indexes.insert((label, property), index);
    }

    pub fn get_index_for_label(&self, label: &str) -> Option<IndexInfo> {
        self.single_label_indexes.get(label).cloned()
    }

    pub fn get_composite_indexes(&self) -> Vec<IndexInfo> {
        self.composite_indexes.clone()
    }

    pub fn get_property_index(&self, label: &str, property: &str) -> Option<IndexInfo> {
        self.property_indexes
            .get(&(label.to_string(), property.to_string()))
            .cloned()
    }

    pub fn find_best_index(&self, labels: &[String]) -> Option<IndexInfo> {
        for index in &self.composite_indexes {
            if index.covers_labels(labels) {
                return Some(index.clone());
            }
        }

        labels
            .iter()
            .filter_map(|label| self.single_label_indexes.get(label))
            .min_by(|a, b| {
                a.selectivity
                    .partial_cmp(&b.selectivity)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
    }

    pub fn all_indexes(&self) -> Vec<IndexInfo> {
        let mut indexes: Vec<IndexInfo> = self.single_label_indexes.values().cloned().collect();
        indexes.extend(self.composite_indexes.clone());
        indexes.extend(self.property_indexes.values().cloned());
        indexes
    }
}

impl Default for IndexRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_index(name: &str, target: &str, selectivity: f32) -> IndexInfo {
        IndexInfo::new(
            name.to_string(),
            "tag".to_string(),
            target.to_string(),
            vec![],
        )
        .with_selectivity(selectivity)
    }

    fn create_composite_index(name: &str, targets: Vec<String>, selectivity: f32) -> IndexInfo {
        IndexInfo::new(
            name.to_string(),
            "tag".to_string(),
            targets.first().cloned().unwrap_or_default(),
            vec![],
        )
        .with_selectivity(selectivity)
        .with_composite(targets)
    }

    #[test]
    fn test_single_label_strategy() {
        let selector = MultiLabelIndexSelector::new();
        let indexes = vec![create_test_index("person_idx", "Person", 0.1)];
        let labels = vec!["Person".to_string()];

        let strategy = selector.select_strategy(&labels, &indexes, &[]).unwrap();

        assert!(matches!(
            strategy,
            MultiLabelStrategy::SingleIndexWithFilter { .. }
        ));
    }

    #[test]
    fn test_composite_index_strategy() {
        let selector = MultiLabelIndexSelector::new();
        let indexes = vec![create_composite_index(
            "person_emp_idx",
            vec!["Person".to_string(), "Employee".to_string()],
            0.05,
        )];
        let labels = vec!["Person".to_string(), "Employee".to_string()];

        let strategy = selector.select_strategy(&labels, &indexes, &[]).unwrap();

        assert!(matches!(
            strategy,
            MultiLabelStrategy::CompositeIndex { .. }
        ));
    }

    #[test]
    fn test_full_scan_strategy() {
        let selector = MultiLabelIndexSelector::new();
        let labels = vec!["UnknownLabel".to_string()];

        let strategy = selector.select_strategy(&labels, &[], &[]).unwrap();

        assert!(matches!(
            strategy,
            MultiLabelStrategy::FullScanWithFilter { .. }
        ));
    }

    #[test]
    fn test_strategy_cost_estimation() {
        let cost_model = CostModelConfig::default();
        let strategy = MultiLabelStrategy::FullScanWithFilter {
            labels: vec!["Person".to_string()],
        };

        let cost = strategy.estimate_cost(&cost_model, 10000);
        assert!(cost > 0.0);
    }

    #[test]
    fn test_index_registry() {
        let mut registry = IndexRegistry::new();

        registry.register_single_label(create_test_index("person_idx", "Person", 0.1));
        registry.register_composite(create_composite_index(
            "person_emp_idx",
            vec!["Person".to_string(), "Employee".to_string()],
            0.05,
        ));

        assert!(registry.get_index_for_label("Person").is_some());
        assert!(registry.get_composite_indexes().len() == 1);
    }

    #[test]
    fn test_index_registry_find_best() {
        let mut registry = IndexRegistry::new();

        registry.register_single_label(create_test_index("person_idx", "Person", 0.1));
        registry.register_composite(create_composite_index(
            "person_emp_idx",
            vec!["Person".to_string(), "Employee".to_string()],
            0.05,
        ));

        let best = registry.find_best_index(&["Person".to_string(), "Employee".to_string()]);
        assert!(best.is_some());
        assert!(best.unwrap().is_composite);
    }
}
