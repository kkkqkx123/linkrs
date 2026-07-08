//! Attribute index lookup strategy
//!
//! Index searches based on attribute conditions support various types of queries, including exact matches, range searches, and searches using prefixes.
//!
//! Applicable scenarios:
//! - MATCH (v:Person) WHERE v.age > 18
//! - MATCH (v:Person) WHERE v.name = "Alice"
//! - MATCH (v:Person) WHERE v.name STARTS WITH "A"

use super::seek_strategy::SeekStrategy;
use super::seek_strategy_base::{IndexInfo, SeekResult, SeekStrategyContext, SeekStrategyType};
use crate::core::types::expr::visitor::ExpressionVisitor;
use crate::core::types::expr::visitor_collectors::OrConditionCollector;
use crate::core::{StorageError, Value};
use crate::storage::StorageReader;

/// Attribute filtering criteria
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyPredicate {
    pub property: String,
    pub op: PredicateOp,
    pub value: Value,
}

/// Predicate operation types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateOp {
    Eq,         // =
    Ne,         // !=
    Lt,         // <
    Le,         // <=
    Gt,         // >
    Ge,         // >=
    In,         // IN
    StartsWith, // STARTS WITH
}

impl PredicateOp {
    /// Check whether it is a range operation.
    pub fn is_range(&self) -> bool {
        matches!(
            self,
            PredicateOp::Lt | PredicateOp::Le | PredicateOp::Gt | PredicateOp::Ge
        )
    }

    /// Check whether it is an equivalent operation.
    pub fn is_equality(&self) -> bool {
        matches!(self, PredicateOp::Eq | PredicateOp::In)
    }
}

impl std::str::FromStr for PredicateOp {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "=" | "==" => Ok(PredicateOp::Eq),
            "!=" | "<>" => Ok(PredicateOp::Ne),
            "<" => Ok(PredicateOp::Lt),
            "<=" => Ok(PredicateOp::Le),
            ">" => Ok(PredicateOp::Gt),
            ">=" => Ok(PredicateOp::Ge),
            "IN" | "in" => Ok(PredicateOp::In),
            "STARTS WITH" | "starts with" => Ok(PredicateOp::StartsWith),
            _ => Err(format!("Invalid predicate operator: {}", s)),
        }
    }
}

/// Attribute index lookup strategy
#[derive(Debug, Clone)]
pub struct PropIndexSeek {
    predicates: Vec<PropertyPredicate>,
}

impl PropIndexSeek {
    pub fn new(predicates: Vec<PropertyPredicate>) -> Self {
        Self { predicates }
    }

    /// Extract attribute predicates from the list of expressions.
    pub fn extract_predicates(expressions: &[crate::core::Expression]) -> Vec<PropertyPredicate> {
        let mut predicates = Vec::new();

        for expr in expressions {
            if let Some(pred) = Self::extract_predicate(expr) {
                predicates.push(pred);
            }
        }

        predicates
    }

    /// Extract attribute predicates from the list of expressions, supporting the conversion of OR conditions.
    pub fn extract_predicates_with_or(
        expressions: &[crate::core::Expression],
    ) -> Vec<PropertyPredicate> {
        let mut predicates = Vec::new();

        for expr in expressions {
            let mut collector = OrConditionCollector::new();
            collector.visit(expr);

            if collector.can_convert_to_in() {
                use crate::core::value::list::List;
                predicates.push(PropertyPredicate {
                    property: collector
                        .property_name()
                        .expect("property_name should exist")
                        .clone(),
                    op: PredicateOp::In,
                    value: Value::list(List {
                        values: collector.values().to_vec(),
                    }),
                });
            } else if let Some(pred) = Self::extract_predicate(expr) {
                predicates.push(pred);
            }
        }

        predicates
    }

    /// Extracting attribute predicates from a single expression
    fn extract_predicate(expr: &crate::core::Expression) -> Option<PropertyPredicate> {
        use crate::core::types::operators::BinaryOperator;

        match expr {
            crate::core::Expression::Binary { op, left, right } => {
                let op_str = match op {
                    BinaryOperator::Equal => "=",
                    BinaryOperator::NotEqual => "!=",
                    BinaryOperator::LessThan => "<",
                    BinaryOperator::LessThanOrEqual => "<=",
                    BinaryOperator::GreaterThan => ">",
                    BinaryOperator::GreaterThanOrEqual => ">=",
                    _ => return None,
                };

                // Try to extract the attribute names and values.
                if let (Some(prop), Some(val)) =
                    (Self::extract_property(left), Self::extract_value(right))
                {
                    if let Ok(pred_op) = op_str.parse::<PredicateOp>() {
                        return Some(PropertyPredicate {
                            property: prop,
                            op: pred_op,
                            value: val,
                        });
                    }
                }

                // Try swapping the left and right sides.
                if let (Some(prop), Some(val)) =
                    (Self::extract_property(right), Self::extract_value(left))
                {
                    let swapped_op = match op_str {
                        "<" => PredicateOp::Gt,
                        "<=" => PredicateOp::Ge,
                        ">" => PredicateOp::Lt,
                        ">=" => PredicateOp::Le,
                        _ => op_str.parse::<PredicateOp>().ok()?,
                    };
                    return Some(PropertyPredicate {
                        property: prop,
                        op: swapped_op,
                        value: val,
                    });
                }

                None
            }
            _ => None,
        }
    }

    /// Extract attribute names from the expression.
    fn extract_property(expr: &crate::core::Expression) -> Option<String> {
        match expr {
            crate::core::Expression::Property { object, property } => {
                // Check for node attribute access, e.g. v.name
                if matches!(object.as_ref(), crate::core::Expression::Variable(_)) {
                    Some(property.clone())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Extract values from the expression.
    fn extract_value(expr: &crate::core::Expression) -> Option<Value> {
        match expr {
            crate::core::Expression::Literal(val) => Some(val.clone()),
            _ => None,
        }
    }

    /// Find an index that is suitable for predicate statements with attributes.
    fn find_best_index<'a>(
        &'a self,
        context: &'a SeekStrategyContext,
    ) -> Option<(&'a IndexInfo, &'a PropertyPredicate)> {
        for pred in &self.predicates {
            if let Some(index) = context.get_index_for_property(&pred.property) {
                return Some((index, pred));
            }
        }
        None
    }

    /// Does the evaluated value satisfy the predicate condition?
    fn value_matches(&self, value: &Value, pred: &PropertyPredicate) -> bool {
        match pred.op {
            PredicateOp::Eq => value == &pred.value,
            PredicateOp::Ne => value != &pred.value,
            PredicateOp::Lt => Self::compare_values(value, &pred.value)
                .map(|c| c < 0)
                .unwrap_or(false),
            PredicateOp::Le => Self::compare_values(value, &pred.value)
                .map(|c| c <= 0)
                .unwrap_or(false),
            PredicateOp::Gt => Self::compare_values(value, &pred.value)
                .map(|c| c > 0)
                .unwrap_or(false),
            PredicateOp::Ge => Self::compare_values(value, &pred.value)
                .map(|c| c >= 0)
                .unwrap_or(false),
            PredicateOp::In => {
                // The IN operation requires that the value provided is a list.
                matches!(&pred.value, Value::List(list) if list.contains(value))
            }
            PredicateOp::StartsWith => {
                if let (Value::String(s1), Value::String(s2)) = (value, &pred.value) {
                    s1.starts_with(s2)
                } else {
                    false
                }
            }
        }
    }

    /// Compare two values
    fn compare_values(left: &Value, right: &Value) -> Option<i32> {
        match (left, right) {
            (Value::SmallInt(i1), Value::SmallInt(i2)) => Some(i1.cmp(i2) as i32),
            (Value::Int(i1), Value::Int(i2)) => Some(i1.cmp(i2) as i32),
            (Value::BigInt(i1), Value::BigInt(i2)) => Some(i1.cmp(i2) as i32),
            (Value::Float(f1), Value::Float(f2)) => f1.partial_cmp(f2).map(|c| c as i32),
            (Value::Double(f1), Value::Double(f2)) => f1.partial_cmp(f2).map(|c| c as i32),
            (Value::String(s1), Value::String(s2)) => Some(s1.cmp(s2) as i32),
            _ => None,
        }
    }
}

impl SeekStrategy for PropIndexSeek {
    fn execute<S: StorageReader>(
        &self,
        storage: &S,
        context: &SeekStrategyContext,
    ) -> Result<SeekResult, StorageError> {
        let mut vertex_ids = Vec::new();
        let mut rows_scanned = 0;

        // Find the best index.
        if let Some((index_info, primary_pred)) = self.find_best_index(context) {
            // Retrieve the vertices corresponding to the tags.
            let space_name = "default"; // In fact, the relevant information should be obtained from the context.
            let vertices = storage.scan_vertices_by_tag(space_name, &index_info.target_name)?;
            rows_scanned = vertices.len();

            // Filter the vertices that satisfy all predicates.
            for vertex in vertices {
                let mut matches_all = true;

                // Check the subject and verb.
                if let Some(prop_value) = vertex.get_property_any(&primary_pred.property) {
                    if !self.value_matches(prop_value, primary_pred) {
                        matches_all = false;
                    }
                } else {
                    matches_all = false;
                }

                // Check the other predicates.
                if matches_all {
                    for pred in &self.predicates {
                        if pred.property != primary_pred.property {
                            if let Some(prop_value) = vertex.get_property_any(&pred.property) {
                                if !self.value_matches(prop_value, pred) {
                                    matches_all = false;
                                    break;
                                }
                            } else {
                                matches_all = false;
                                break;
                            }
                        }
                    }
                }

                if matches_all {
                    vertex_ids.push(Value::from(*vertex.vid()));
                }
            }
        }

        Ok(SeekResult {
            vertex_ids,
            strategy_used: SeekStrategyType::PropIndexSeek,
            rows_scanned,
        })
    }

    fn supports(&self, _context: &SeekStrategyContext) -> bool {
        // Support is available as long as there are attribute predicates.
        !self.predicates.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Expression;

    #[test]
    fn test_predicate_op_from_str() {
        assert_eq!("=".parse::<PredicateOp>().ok(), Some(PredicateOp::Eq));
        assert_eq!("<".parse::<PredicateOp>().ok(), Some(PredicateOp::Lt));
        assert_eq!(">=".parse::<PredicateOp>().ok(), Some(PredicateOp::Ge));
        assert_eq!("IN".parse::<PredicateOp>().ok(), Some(PredicateOp::In));
        assert_eq!(
            "STARTS WITH".parse::<PredicateOp>().ok(),
            Some(PredicateOp::StartsWith)
        );
        assert!("unknown".parse::<PredicateOp>().is_err());
    }

    #[test]
    fn test_extract_predicate_eq() {
        let expr = Expression::binary(
            Expression::property(Expression::variable("v"), "age"),
            crate::core::BinaryOperator::Equal,
            Expression::int(18),
        );

        let pred = PropIndexSeek::extract_predicate(&expr);
        assert!(pred.is_some());

        let pred = pred.expect("Failed to extract predicate");
        assert_eq!(pred.property, "age");
        assert_eq!(pred.op, PredicateOp::Eq);
        assert_eq!(pred.value, Value::Int(18));
    }

    #[test]
    fn test_value_matches() {
        let seek = PropIndexSeek::new(vec![]);

        let pred = PropertyPredicate {
            property: "age".to_string(),
            op: PredicateOp::Gt,
            value: Value::Int(18),
        };

        assert!(seek.value_matches(&Value::Int(20), &pred));
        assert!(!seek.value_matches(&Value::Int(18), &pred));
        assert!(!seek.value_matches(&Value::Int(15), &pred));
    }

    #[test]
    fn test_extract_or_condition() {
        let expr = Expression::binary(
            Expression::binary(
                Expression::property(Expression::variable("v"), "age"),
                crate::core::BinaryOperator::Equal,
                Expression::int(10),
            ),
            crate::core::BinaryOperator::Or,
            Expression::binary(
                Expression::property(Expression::variable("v"), "age"),
                crate::core::BinaryOperator::Equal,
                Expression::int(20),
            ),
        );

        let preds = PropIndexSeek::extract_predicates_with_or(&[expr]);
        assert_eq!(preds.len(), 1);

        let pred = &preds[0];
        assert_eq!(pred.property, "age");
        assert_eq!(pred.op, PredicateOp::In);
        if let Value::List(list) = &pred.value {
            assert_eq!(list.values.len(), 2);
            assert!(list.values.contains(&Value::Int(10)));
            assert!(list.values.contains(&Value::Int(20)));
        } else {
            panic!("Expected List value");
        }
    }

    #[test]
    fn test_extract_or_condition_different_properties() {
        let expr = Expression::binary(
            Expression::binary(
                Expression::property(Expression::variable("v"), "age"),
                crate::core::BinaryOperator::Equal,
                Expression::literal(10),
            ),
            crate::core::BinaryOperator::Or,
            Expression::binary(
                Expression::property(Expression::variable("v"), "name"),
                crate::core::BinaryOperator::Equal,
                Expression::literal("Alice"),
            ),
        );

        let preds = PropIndexSeek::extract_predicates_with_or(&[expr]);
        assert_eq!(preds.len(), 0);
    }
}
