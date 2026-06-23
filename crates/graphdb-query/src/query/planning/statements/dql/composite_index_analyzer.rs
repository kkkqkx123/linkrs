//! Composite Index Analyzer
//!
//! Optimizes composite index selection for LOOKUP queries with multiple conditions.
//!
//! ## Features
//!
//! - Prefix matching for composite indexes
//! - Selectivity-based index selection
//! - Range scan optimization
//! - Partial prefix support

use std::collections::HashMap;

use crate::core::Expression;
use crate::core::Value;
use crate::query::optimizer::cost::CostModelConfig;
use crate::query::planning::plan::core::nodes::access::IndexLimit;
use crate::query::planning::statements::seeks::seek_strategy_base::IndexInfo;

pub type AnalyzerError = String;

#[derive(Debug, Clone)]
pub struct PredicateInfo {
    pub column: String,
    pub op: PredicateOp,
    pub value: Option<Value>,
    pub lower: Option<Value>,
    pub upper: Option<Value>,
    pub include_lower: bool,
    pub include_upper: bool,
    pub values: Vec<Value>,
}

impl PredicateInfo {
    pub fn equal(column: String, value: Value) -> Self {
        Self {
            column,
            op: PredicateOp::Equal,
            value: Some(value),
            lower: None,
            upper: None,
            include_lower: false,
            include_upper: false,
            values: vec![],
        }
    }

    pub fn range(
        column: String,
        lower: Option<Value>,
        upper: Option<Value>,
        include_lower: bool,
        include_upper: bool,
    ) -> Self {
        Self {
            column,
            op: PredicateOp::Range,
            value: None,
            lower,
            upper,
            include_lower,
            include_upper,
            values: vec![],
        }
    }

    pub fn in_list(column: String, values: Vec<Value>) -> Self {
        Self {
            column,
            op: PredicateOp::In,
            value: None,
            lower: None,
            upper: None,
            include_lower: false,
            include_upper: false,
            values,
        }
    }

    pub fn is_equality(&self) -> bool {
        matches!(self.op, PredicateOp::Equal)
    }

    pub fn is_range(&self) -> bool {
        matches!(self.op, PredicateOp::Range)
    }

    fn value_to_string(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            Value::Int(i) => i.to_string(),
            Value::BigInt(i) => i.to_string(),
            Value::SmallInt(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Double(d) => d.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => format!("{:?}", v),
        }
    }

    pub fn to_index_limit(&self) -> Option<IndexLimit> {
        match &self.op {
            PredicateOp::Equal => self
                .value
                .as_ref()
                .map(|v| IndexLimit::equal(&self.column, Self::value_to_string(v))),
            PredicateOp::Range => Some(IndexLimit::range(
                &self.column,
                self.lower.as_ref().map(Self::value_to_string),
                self.upper.as_ref().map(Self::value_to_string),
                self.include_lower,
                self.include_upper,
            )),
            PredicateOp::In => {
                if self.values.len() == 1 {
                    Some(IndexLimit::equal(
                        &self.column,
                        Self::value_to_string(&self.values[0]),
                    ))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PredicateOp {
    Equal,
    Range,
    In,
    Like,
    NotEqual,
}

#[derive(Debug, Clone)]
pub struct CompositeIndexSelection {
    pub index: IndexInfo,
    pub matched_columns: usize,
    pub match_type: MatchType,
    pub selectivity: f64,
    pub scan_limits: Vec<IndexLimit>,
    pub remaining_predicates: Vec<PredicateInfo>,
}

impl CompositeIndexSelection {
    pub fn usable_prefix_length(&self) -> usize {
        match &self.match_type {
            MatchType::Full => self.matched_columns,
            MatchType::PrefixRange { prefix_columns } => prefix_columns + 1,
            MatchType::Partial { matched_columns } => *matched_columns,
            MatchType::PrefixGap { prefix_columns } => *prefix_columns,
        }
    }

    pub fn has_remaining_filter(&self) -> bool {
        !self.remaining_predicates.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchType {
    Full,
    PrefixRange { prefix_columns: usize },
    Partial { matched_columns: usize },
    PrefixGap { prefix_columns: usize },
}

#[derive(Debug, Clone)]
pub struct SingleColumnSelection {
    pub index: IndexInfo,
    pub predicate: PredicateInfo,
    pub scan_limit: IndexLimit,
    pub selectivity: f64,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum IndexSelectionResult {
    Composite(CompositeIndexSelection),
    SingleColumn(Box<SingleColumnSelection>),
    FullScan,
}

pub struct CompositeIndexAnalyzer {
    cost_config: CostModelConfig,
    column_stats: HashMap<String, ColumnStats>,
}

#[derive(Debug, Clone)]
pub struct ColumnStats {
    pub name: String,
    pub distinct_count: usize,
    pub null_count: usize,
    pub total_rows: usize,
}

impl ColumnStats {
    pub fn new(name: String, distinct_count: usize, total_rows: usize) -> Self {
        Self {
            name,
            distinct_count,
            null_count: 0,
            total_rows,
        }
    }

    pub fn selectivity(&self) -> f64 {
        if self.distinct_count == 0 {
            return 1.0;
        }
        1.0 / self.distinct_count as f64
    }
}

impl CompositeIndexAnalyzer {
    pub fn new() -> Self {
        Self {
            cost_config: CostModelConfig::default(),
            column_stats: HashMap::new(),
        }
    }

    pub fn with_cost_config(mut self, config: CostModelConfig) -> Self {
        self.cost_config = config;
        self
    }

    pub fn add_column_stats(&mut self, stats: ColumnStats) {
        self.column_stats.insert(stats.name.clone(), stats);
    }

    pub fn select_optimal_index(
        &self,
        predicates: &[PredicateInfo],
        indexes: &[IndexInfo],
    ) -> IndexSelectionResult {
        if predicates.is_empty() || indexes.is_empty() {
            return IndexSelectionResult::FullScan;
        }

        let composite_result = self.find_best_composite_index(predicates, indexes);
        let single_result = self.find_best_single_column_index(predicates, indexes);

        match (composite_result, single_result) {
            (Some(composite), Some(single)) => {
                if composite.selectivity <= single.selectivity {
                    IndexSelectionResult::Composite(composite)
                } else {
                    IndexSelectionResult::SingleColumn(Box::new(single))
                }
            }
            (Some(composite), None) => IndexSelectionResult::Composite(composite),
            (None, Some(single)) => IndexSelectionResult::SingleColumn(Box::new(single)),
            (None, None) => IndexSelectionResult::FullScan,
        }
    }

    pub fn find_best_composite_index(
        &self,
        predicates: &[PredicateInfo],
        indexes: &[IndexInfo],
    ) -> Option<CompositeIndexSelection> {
        let mut candidates: Vec<CompositeIndexSelection> = Vec::new();

        for index in indexes {
            if !index.is_composite {
                continue;
            }

            if let Some(selection) = self.evaluate_composite_index(predicates, index) {
                candidates.push(selection);
            }
        }

        candidates.into_iter().min_by(|a, b| {
            let columns_cmp = a.matched_columns.cmp(&b.matched_columns).reverse();
            if columns_cmp != std::cmp::Ordering::Equal {
                return columns_cmp;
            }
            a.selectivity
                .partial_cmp(&b.selectivity)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    fn evaluate_composite_index(
        &self,
        predicates: &[PredicateInfo],
        index: &IndexInfo,
    ) -> Option<CompositeIndexSelection> {
        let index_columns = &index.properties;
        if index_columns.is_empty() {
            return None;
        }

        let mut matched_columns = 0;
        let mut scan_limits = Vec::new();
        let mut match_type = MatchType::Full;
        let mut used_predicates = Vec::new();
        let mut remaining_predicates = predicates.to_vec();

        for (i, col) in index_columns.iter().enumerate() {
            let pred_idx = remaining_predicates.iter().position(|p| &p.column == col);

            match pred_idx {
                Some(idx) => {
                    let pred = &remaining_predicates[idx];

                    if pred.is_equality() {
                        matched_columns += 1;
                        if let Some(limit) = pred.to_index_limit() {
                            scan_limits.push(limit);
                        }
                        used_predicates.push(remaining_predicates.remove(idx));
                    } else if pred.is_range() {
                        matched_columns += 1;
                        if let Some(limit) = pred.to_index_limit() {
                            scan_limits.push(limit);
                        }
                        used_predicates.push(remaining_predicates.remove(idx));
                        match_type = MatchType::PrefixRange { prefix_columns: i };
                        break;
                    } else {
                        match_type = MatchType::Partial { matched_columns };
                        break;
                    }
                }
                None => {
                    if matched_columns > 0 {
                        match_type = MatchType::PrefixGap {
                            prefix_columns: matched_columns,
                        };
                    }
                    break;
                }
            }
        }

        if matched_columns == 0 {
            return None;
        }

        let selectivity = self.estimate_selectivity(&used_predicates);

        Some(CompositeIndexSelection {
            index: index.clone(),
            matched_columns,
            match_type,
            selectivity,
            scan_limits,
            remaining_predicates,
        })
    }

    pub fn find_best_single_column_index(
        &self,
        predicates: &[PredicateInfo],
        indexes: &[IndexInfo],
    ) -> Option<SingleColumnSelection> {
        let mut best: Option<SingleColumnSelection> = None;

        for pred in predicates {
            if !pred.is_equality() && !pred.is_range() {
                continue;
            }

            for index in indexes {
                if index.is_composite {
                    continue;
                }

                if !index.properties.contains(&pred.column) {
                    continue;
                }

                let selectivity = self.estimate_predicate_selectivity(pred);
                let scan_limit = pred.to_index_limit();

                if let Some(limit) = scan_limit {
                    let selection = SingleColumnSelection {
                        index: index.clone(),
                        predicate: pred.clone(),
                        scan_limit: limit,
                        selectivity,
                    };

                    if best
                        .as_ref()
                        .is_none_or(|b| selection.selectivity < b.selectivity)
                    {
                        best = Some(selection);
                    }
                }
            }
        }

        best
    }

    fn estimate_selectivity(&self, predicates: &[PredicateInfo]) -> f64 {
        let mut total = 1.0;
        for pred in predicates {
            total *= self.estimate_predicate_selectivity(pred);
        }
        total
    }

    fn estimate_predicate_selectivity(&self, pred: &PredicateInfo) -> f64 {
        match &pred.op {
            PredicateOp::Equal => self
                .column_stats
                .get(&pred.column)
                .map(|s| s.selectivity())
                .unwrap_or(0.1),
            PredicateOp::Range => {
                if pred.lower.is_some() && pred.upper.is_some() {
                    0.1
                } else {
                    0.33
                }
            }
            PredicateOp::In => {
                let count = pred.values.len();
                0.1 * count as f64
            }
            PredicateOp::Like => 0.2,
            PredicateOp::NotEqual => 0.9,
        }
    }
}

impl Default for CompositeIndexAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

pub fn extract_predicates_from_expression(expr: &Expression) -> Vec<PredicateInfo> {
    let mut predicates = Vec::new();
    extract_predicates_recursive(expr, &mut predicates);
    predicates
}

fn extract_predicates_recursive(expr: &Expression, predicates: &mut Vec<PredicateInfo>) {
    use crate::core::types::operators::BinaryOperator;

    match expr {
        Expression::Binary { left, op, right } => match op {
            BinaryOperator::Equal => {
                if let Some(pred) = extract_equality_predicate(left, right) {
                    predicates.push(pred);
                } else if let Some(pred) = extract_equality_predicate(right, left) {
                    predicates.push(pred);
                }
            }
            BinaryOperator::And => {
                extract_predicates_recursive(left, predicates);
                extract_predicates_recursive(right, predicates);
            }
            BinaryOperator::LessThan
            | BinaryOperator::LessThanOrEqual
            | BinaryOperator::GreaterThan
            | BinaryOperator::GreaterThanOrEqual => {
                if let Some(pred) = extract_range_predicate(left, op, right) {
                    predicates.push(pred);
                } else if let Some(pred) = extract_range_predicate(right, op, left) {
                    predicates.push(pred);
                }
            }
            _ => {}
        },
        Expression::Unary { .. } => {}
        Expression::Function { name, args }
            if name.eq_ignore_ascii_case("in") && args.len() >= 2 =>
        {
            if let (Some(col), Some(values)) =
                (extract_column_name(&args[0]), extract_list_values(&args[1]))
            {
                predicates.push(PredicateInfo::in_list(col, values));
            }
        }
        _ => {}
    }
}

fn extract_equality_predicate(
    col_expr: &Expression,
    val_expr: &Expression,
) -> Option<PredicateInfo> {
    let column = extract_column_name(col_expr)?;
    let value = extract_literal_value(val_expr)?;
    Some(PredicateInfo::equal(column, value))
}

fn extract_range_predicate(
    col_expr: &Expression,
    op: &crate::core::types::operators::BinaryOperator,
    val_expr: &Expression,
) -> Option<PredicateInfo> {
    use crate::core::types::operators::BinaryOperator;

    let column = extract_column_name(col_expr)?;
    let value = extract_literal_value(val_expr)?;

    let (lower, upper, include_lower, include_upper) = match op {
        BinaryOperator::LessThan => (None, Some(value), false, false),
        BinaryOperator::LessThanOrEqual => (None, Some(value), false, true),
        BinaryOperator::GreaterThan => (Some(value), None, false, false),
        BinaryOperator::GreaterThanOrEqual => (Some(value), None, true, false),
        _ => return None,
    };

    Some(PredicateInfo::range(
        column,
        lower,
        upper,
        include_lower,
        include_upper,
    ))
}

fn extract_column_name(expr: &Expression) -> Option<String> {
    match expr {
        Expression::Property { property, .. } => Some(property.clone()),
        Expression::Variable(name) => {
            if name.contains('.') {
                name.split('.').next_back().map(|s| s.to_string())
            } else {
                Some(name.clone())
            }
        }
        _ => None,
    }
}

fn extract_literal_value(expr: &Expression) -> Option<Value> {
    match expr {
        Expression::Literal(value) => Some(value.clone()),
        _ => None,
    }
}

fn extract_list_values(expr: &Expression) -> Option<Vec<Value>> {
    match expr {
        Expression::List(items) => {
            let values: Vec<Value> = items.iter().filter_map(extract_literal_value).collect();
            if values.len() == items.len() {
                Some(values)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_composite_index(name: &str, columns: Vec<&str>) -> IndexInfo {
        IndexInfo::new(
            name.to_string(),
            "tag".to_string(),
            columns.first().unwrap_or(&"").to_string(),
            columns.iter().map(|s| s.to_string()).collect(),
        )
        .with_composite(columns.iter().map(|s| s.to_string()).collect())
    }

    fn create_single_index(name: &str, column: &str) -> IndexInfo {
        IndexInfo::new(
            name.to_string(),
            "tag".to_string(),
            column.to_string(),
            vec![column.to_string()],
        )
    }

    #[test]
    fn test_predicate_info_equal() {
        let pred = PredicateInfo::equal("name".to_string(), Value::String("Alice".to_string()));
        assert!(pred.is_equality());
        assert!(!pred.is_range());

        let limit = pred.to_index_limit().unwrap();
        assert_eq!(limit.column, "name");
    }

    #[test]
    fn test_predicate_info_range() {
        let pred = PredicateInfo::range(
            "age".to_string(),
            Some(Value::Int(20)),
            Some(Value::Int(30)),
            true,
            true,
        );
        assert!(pred.is_range());
        assert!(!pred.is_equality());
    }

    #[test]
    fn test_composite_index_full_match() {
        let analyzer = CompositeIndexAnalyzer::new();
        let index = create_composite_index("idx_name_age", vec!["name", "age"]);
        let predicates = vec![
            PredicateInfo::equal("name".to_string(), Value::String("Alice".to_string())),
            PredicateInfo::equal("age".to_string(), Value::Int(30)),
        ];

        let selection = analyzer
            .evaluate_composite_index(&predicates, &index)
            .unwrap();
        assert_eq!(selection.matched_columns, 2);
        assert_eq!(selection.match_type, MatchType::Full);
    }

    #[test]
    fn test_composite_index_prefix_range() {
        let analyzer = CompositeIndexAnalyzer::new();
        let index = create_composite_index("idx_name_age", vec!["name", "age"]);
        let predicates = vec![
            PredicateInfo::equal("name".to_string(), Value::String("Alice".to_string())),
            PredicateInfo::range("age".to_string(), Some(Value::Int(20)), None, true, false),
        ];

        let selection = analyzer
            .evaluate_composite_index(&predicates, &index)
            .unwrap();
        assert_eq!(selection.matched_columns, 2);
        assert!(matches!(
            selection.match_type,
            MatchType::PrefixRange { .. }
        ));
    }

    #[test]
    fn test_composite_index_partial() {
        let analyzer = CompositeIndexAnalyzer::new();
        let index = create_composite_index("idx_name_age_city", vec!["name", "age", "city"]);
        let predicates = vec![
            PredicateInfo::equal("name".to_string(), Value::String("Alice".to_string())),
            PredicateInfo::equal("city".to_string(), Value::String("NYC".to_string())),
        ];

        let selection = analyzer
            .evaluate_composite_index(&predicates, &index)
            .unwrap();
        assert_eq!(selection.matched_columns, 1);
        assert!(matches!(selection.match_type, MatchType::PrefixGap { .. }));
    }

    #[test]
    fn test_select_optimal_index() {
        let analyzer = CompositeIndexAnalyzer::new();
        let indexes = vec![
            create_composite_index("idx_name_age", vec!["name", "age"]),
            create_single_index("idx_name", "name"),
        ];
        let predicates = vec![
            PredicateInfo::equal("name".to_string(), Value::String("Alice".to_string())),
            PredicateInfo::equal("age".to_string(), Value::Int(30)),
        ];

        let result = analyzer.select_optimal_index(&predicates, &indexes);
        assert!(matches!(result, IndexSelectionResult::Composite(_)));
    }

    #[test]
    fn test_single_column_selection() {
        let analyzer = CompositeIndexAnalyzer::new();
        let indexes = vec![create_single_index("idx_name", "name")];
        let predicates = vec![PredicateInfo::equal(
            "name".to_string(),
            Value::String("Alice".to_string()),
        )];

        let result = analyzer.select_optimal_index(&predicates, &indexes);
        assert!(matches!(result, IndexSelectionResult::SingleColumn(_)));
    }

    #[test]
    fn test_column_stats_selectivity() {
        let stats = ColumnStats::new("name".to_string(), 100, 1000);
        assert!((stats.selectivity() - 0.01).abs() < 0.0001);
    }
}
