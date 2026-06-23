//! Index Selector Module
//!
//! Used to select the optimal index for the query.

use std::sync::Arc;

use crate::core::types::{Expression, Index};
use crate::query::optimizer::cost::{CostCalculator, SelectivityEstimator};

/// Index selector
#[derive(Debug)]
pub struct IndexSelector {
    cost_calculator: Arc<CostCalculator>,
    selectivity_estimator: Arc<SelectivityEstimator>,
}

/// Index selection results
#[derive(Debug, Clone)]
pub enum IndexSelection {
    /// Attribute Index
    PropertyIndex {
        /// Index name
        index_name: String,
        /// Attribute name
        property_name: String,
        /// Estimated cost
        estimated_cost: f64,
        /// selectivity
        selectivity: f64,
    },
    /// Tag Index
    TagIndex {
        /// Estimated cost
        estimated_cost: f64,
        /// Number of vertices
        vertex_count: u64,
    },
    /// Full table scan
    FullScan {
        /// Estimated cost
        estimated_cost: f64,
        /// Number of vertices
        vertex_count: u64,
    },
}

/// Attribute predicate
#[derive(Debug, Clone)]
pub struct PropertyPredicate {
    /// attribute name
    pub property_name: String,
    /// Operator
    pub operator: PredicateOperator,
    /// Value expression
    pub value: Expression,
}

/// Predicate operator
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateOperator {
    /// equals
    Equal,
    /// Not equal to
    NotEqual,
    /// less than
    LessThan,
    /// less than or equal to
    LessThanOrEqual,
    /// greater than
    GreaterThan,
    /// greater than or equal to
    GreaterThanOrEqual,
    /// LIKE
    Like,
    /// IN
    In,
}

impl IndexSelector {
    /// Create a new index selector with Arc-wrapped dependencies.
    ///
    /// This constructor is suitable for long-lived selectors that need to be shared.
    pub fn new(
        cost_calculator: Arc<CostCalculator>,
        selectivity_estimator: Arc<SelectivityEstimator>,
    ) -> Self {
        Self {
            cost_calculator,
            selectivity_estimator,
        }
    }

    /// Create a new index selector with references (lightweight version).
    ///
    /// This constructor is suitable for short-lived selectors with clear lifetimes.
    /// It avoids the overhead of Arc wrapping while maintaining the same functionality.
    pub fn with_refs<'a>(
        cost_calculator: &'a CostCalculator,
        selectivity_estimator: &'a SelectivityEstimator,
    ) -> Self {
        Self {
            cost_calculator: Arc::new(cost_calculator.clone()),
            selectivity_estimator: Arc::new(selectivity_estimator.clone()),
        }
    }

    /// Select the optimal index for the query.
    pub fn select_index(
        &self,
        tag_name: &str,
        predicates: &[PropertyPredicate],
        available_indexes: &[Index],
    ) -> IndexSelection {
        // If there are no predicates, use a full table scan.
        if predicates.is_empty() {
            let vertex_count = self
                .cost_calculator
                .statistics_manager()
                .get_vertex_count(tag_name);
            let estimated_cost = self.cost_calculator.calculate_scan_vertices_cost(tag_name);
            return IndexSelection::FullScan {
                estimated_cost,
                vertex_count,
            };
        }

        // Evaluate each available index.
        let mut best_selection: Option<IndexSelection> = None;

        for index in available_indexes {
            // Please provide the text that needs to be translated. I will then translate it into English, ensuring that only the content matching the specified tags is included in the translation.
            if index.schema_name != tag_name {
                continue;
            }

            if let Some(selection) = self.evaluate_index(index, predicates) {
                match &best_selection {
                    None => best_selection = Some(selection),
                    Some(current_best) => {
                        if selection.estimated_cost() < current_best.estimated_cost() {
                            best_selection = Some(selection);
                        }
                    }
                }
            }
        }

        // If no suitable index is found, a full table scan is used.
        best_selection.unwrap_or_else(|| {
            let vertex_count = self
                .cost_calculator
                .statistics_manager()
                .get_vertex_count(tag_name);
            let estimated_cost = self.cost_calculator.calculate_scan_vertices_cost(tag_name);
            IndexSelection::FullScan {
                estimated_cost,
                vertex_count,
            }
        })
    }

    /// Evaluating a single index
    fn evaluate_index(
        &self,
        index: &Index,
        predicates: &[PropertyPredicate],
    ) -> Option<IndexSelection> {
        // Check whether the index covers the predicate.
        let covered_predicates: Vec<&PropertyPredicate> = predicates
            .iter()
            .filter(|p| index.properties.contains(&p.property_name))
            .collect();

        if covered_predicates.is_empty() {
            return None;
        }

        // Calculating selectivity
        let mut total_selectivity = 1.0;
        for predicate in &covered_predicates {
            let selectivity = match predicate.operator {
                PredicateOperator::Equal => {
                    // Try to extract the values from the expression.
                    let value = if let Expression::Literal(v) = &predicate.value {
                        Some(v.clone())
                    } else {
                        None
                    };
                    self.selectivity_estimator.estimate_equality_selectivity(
                        Some(&index.schema_name),
                        &predicate.property_name,
                        value.as_ref(),
                    )
                }
                PredicateOperator::LessThan | PredicateOperator::LessThanOrEqual => self
                    .selectivity_estimator
                    .estimate_less_than_selectivity(None),
                PredicateOperator::GreaterThan | PredicateOperator::GreaterThanOrEqual => self
                    .selectivity_estimator
                    .estimate_greater_than_selectivity(None),
                PredicateOperator::Like => {
                    // Try to extract patterns from the expression.
                    if let Expression::Literal(crate::core::value::Value::String(pattern)) =
                        &predicate.value
                    {
                        self.selectivity_estimator
                            .estimate_like_selectivity(pattern)
                    } else {
                        0.3
                    }
                }
                _ => 0.3,
            };
            total_selectivity *= selectivity;
        }

        // Calculating the cost
        let estimated_cost = self.cost_calculator.calculate_index_scan_cost(
            &index.schema_name,
            &covered_predicates[0].property_name,
            total_selectivity,
        );

        // Get the name of the first overridden attribute.
        let property_name = covered_predicates[0].property_name.clone();

        Some(IndexSelection::PropertyIndex {
            index_name: index.name.clone(),
            property_name,
            estimated_cost,
            selectivity: total_selectivity,
        })
    }

    /// Choosing the optimal composite index strategy
    pub fn select_composite_index_strategy(
        &self,
        tag_name: &str,
        predicates: &[PropertyPredicate],
        available_indexes: &[Index],
    ) -> Vec<IndexSelection> {
        let mut strategies = Vec::new();

        // Add a full-table scan as a benchmark.
        let vertex_count = self
            .cost_calculator
            .statistics_manager()
            .get_vertex_count(tag_name);
        let full_scan_cost = self.cost_calculator.calculate_scan_vertices_cost(tag_name);
        strategies.push(IndexSelection::FullScan {
            estimated_cost: full_scan_cost,
            vertex_count,
        });

        // Evaluate each index.
        for index in available_indexes {
            if index.schema_name != tag_name {
                continue;
            }

            if let Some(selection) = self.evaluate_index(index, predicates) {
                strategies.push(selection);
            }
        }

        // Sort by cost
        strategies.sort_by(|a, b| {
            a.estimated_cost()
                .partial_cmp(&b.estimated_cost())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        strategies
    }
}

impl Clone for IndexSelector {
    fn clone(&self) -> Self {
        Self {
            cost_calculator: self.cost_calculator.clone(),
            selectivity_estimator: self.selectivity_estimator.clone(),
        }
    }
}

impl IndexSelection {
    /// Obtain the estimated cost.
    pub fn estimated_cost(&self) -> f64 {
        match self {
            IndexSelection::PropertyIndex { estimated_cost, .. } => *estimated_cost,
            IndexSelection::TagIndex { estimated_cost, .. } => *estimated_cost,
            IndexSelection::FullScan { estimated_cost, .. } => *estimated_cost,
        }
    }

    /// Obtain the options (if any).
    pub fn selectivity(&self) -> Option<f64> {
        match self {
            IndexSelection::PropertyIndex { selectivity, .. } => Some(*selectivity),
            _ => None,
        }
    }

    /// Determine whether it is an index scan.
    pub fn is_index_scan(&self) -> bool {
        matches!(self, IndexSelection::PropertyIndex { .. })
    }

    /// Determine whether it is a full table scan.
    pub fn is_full_scan(&self) -> bool {
        matches!(self, IndexSelection::FullScan { .. })
    }
}
