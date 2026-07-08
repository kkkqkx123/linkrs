//! Variable attribute index lookup strategy
//!
//! Index lookup based on variable attributes, used in cases where the value of a runtime variable needs to be determined.
//!
//! Applicable scenarios:
//! - MATCH (v:Person) WHERE v.name = $varName
//! - MATCH (v:Person) WHERE v.age > $minAge
//! - Variable binding in parameterized queries

use super::seek_strategy::SeekStrategy;
use super::seek_strategy_base::{IndexInfo, SeekResult, SeekStrategyContext, SeekStrategyType};
use crate::core::{StorageError, Value};
use crate::storage::StorageReader;

/// Variable attribute predicate
#[derive(Debug, Clone, PartialEq)]
pub struct VariablePropertyPredicate {
    pub property: String,
    pub op: VariablePredicateOp,
    pub variable_name: String,
}

/// Variable predicate operation types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariablePredicateOp {
    Eq, // =
    Ne, // !=
    Lt, // <
    Le, // <=
    Gt, // >
    Ge, // >=
    In, // IN
}

impl VariablePredicateOp {
    /// Convert to ordinary predicate operations
    pub fn to_predicate_op(&self) -> super::prop_index_seek::PredicateOp {
        match self {
            VariablePredicateOp::Eq => super::prop_index_seek::PredicateOp::Eq,
            VariablePredicateOp::Ne => super::prop_index_seek::PredicateOp::Ne,
            VariablePredicateOp::Lt => super::prop_index_seek::PredicateOp::Lt,
            VariablePredicateOp::Le => super::prop_index_seek::PredicateOp::Le,
            VariablePredicateOp::Gt => super::prop_index_seek::PredicateOp::Gt,
            VariablePredicateOp::Ge => super::prop_index_seek::PredicateOp::Ge,
            VariablePredicateOp::In => super::prop_index_seek::PredicateOp::In,
        }
    }
}

impl std::str::FromStr for VariablePredicateOp {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "=" | "==" => Ok(VariablePredicateOp::Eq),
            "!=" | "<>" => Ok(VariablePredicateOp::Ne),
            "<" => Ok(VariablePredicateOp::Lt),
            "<=" => Ok(VariablePredicateOp::Le),
            ">" => Ok(VariablePredicateOp::Gt),
            ">=" => Ok(VariablePredicateOp::Ge),
            "IN" => Ok(VariablePredicateOp::In),
            _ => Err(format!("Invalid predicate operator: {}", s)),
        }
    }
}

/// Variable attribute index lookup strategy
#[derive(Debug, Clone)]
pub struct VariablePropIndexSeek {
    predicates: Vec<VariablePropertyPredicate>,
    variable_values: std::collections::HashMap<String, Value>,
}

impl VariablePropIndexSeek {
    pub fn new(predicates: Vec<VariablePropertyPredicate>) -> Self {
        Self {
            predicates,
            variable_values: std::collections::HashMap::new(),
        }
    }

    /// Binding variable values
    pub fn bind_variable(&mut self, name: &str, value: Value) {
        self.variable_values.insert(name.to_string(), value);
    }

    /// Batch binding of variable values
    pub fn bind_variables(&mut self, values: std::collections::HashMap<String, Value>) {
        self.variable_values.extend(values);
    }

    /// Check whether all variables have been bound.
    pub fn all_variables_bound(&self) -> bool {
        self.predicates
            .iter()
            .all(|pred| self.variable_values.contains_key(&pred.variable_name))
    }

    /// Extract variable attribute predicates from the list of expressions.
    pub fn extract_predicates(
        expressions: &[crate::core::Expression],
    ) -> Vec<VariablePropertyPredicate> {
        let mut predicates = Vec::new();

        for expr in expressions {
            if let Some(pred) = Self::extract_predicate(expr) {
                predicates.push(pred);
            }
        }

        predicates
    }

    /// Extracting variable attribute predicates from a single expression
    fn extract_predicate(expr: &crate::core::Expression) -> Option<VariablePropertyPredicate> {
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

                // Try to extract the property name and variable name: v.name = $var
                if let (Some(prop), Some(var_name)) =
                    (Self::extract_property(left), Self::extract_variable(right))
                {
                    if let Ok(pred_op) = op_str.parse::<VariablePredicateOp>() {
                        return Some(VariablePropertyPredicate {
                            property: prop,
                            op: pred_op,
                            variable_name: var_name,
                        });
                    }
                }

                // Swap left and right attempts: $var = v.name
                if let (Some(prop), Some(var_name)) =
                    (Self::extract_property(right), Self::extract_variable(left))
                {
                    let swapped_op = match op_str {
                        "<" => VariablePredicateOp::Gt,
                        "<=" => VariablePredicateOp::Ge,
                        ">" => VariablePredicateOp::Lt,
                        ">=" => VariablePredicateOp::Le,
                        _ => op_str.parse::<VariablePredicateOp>().ok()?,
                    };
                    return Some(VariablePropertyPredicate {
                        property: prop,
                        op: swapped_op,
                        variable_name: var_name,
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

    /// Extract variable names from the expression.
    fn extract_variable(expr: &crate::core::Expression) -> Option<String> {
        match expr {
            // Variables that start with the symbol $ indicate parameters.
            crate::core::Expression::Variable(name) if name.starts_with('$') => {
                Some(name[1..].to_string())
            }
            _ => None,
        }
    }

    /// Find an index that is suitable for the predicate of the variable attribute.
    fn find_best_index<'a>(
        &'a self,
        context: &'a SeekStrategyContext,
    ) -> Option<(&'a IndexInfo, &'a VariablePropertyPredicate)> {
        for pred in &self.predicates {
            if let Some(index) = context.get_index_for_property(&pred.property) {
                return Some((index, pred));
            }
        }
        None
    }

    /// Does the evaluated value satisfy the predicate condition?
    fn value_matches(&self, value: &Value, pred: &VariablePropertyPredicate) -> bool {
        // Obtain the value of a variable
        let var_value = match self.variable_values.get(&pred.variable_name) {
            Some(v) => v,
            None => return false, // The variable is not bound, so a match cannot be made.
        };

        match pred.op {
            VariablePredicateOp::Eq => value == var_value,
            VariablePredicateOp::Ne => value != var_value,
            VariablePredicateOp::Lt => Self::compare_values(value, var_value)
                .map(|c| c < 0)
                .unwrap_or(false),
            VariablePredicateOp::Le => Self::compare_values(value, var_value)
                .map(|c| c <= 0)
                .unwrap_or(false),
            VariablePredicateOp::Gt => Self::compare_values(value, var_value)
                .map(|c| c > 0)
                .unwrap_or(false),
            VariablePredicateOp::Ge => Self::compare_values(value, var_value)
                .map(|c| c >= 0)
                .unwrap_or(false),
            VariablePredicateOp::In => {
                // The IN operation requires that the variable value be a list.
                matches!(var_value, Value::List(list) if list.contains(value))
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

impl SeekStrategy for VariablePropIndexSeek {
    fn execute<S: StorageReader>(
        &self,
        storage: &S,
        context: &SeekStrategyContext,
    ) -> Result<SeekResult, StorageError> {
        // Check whether the variable has been bound.
        if !self.all_variables_bound() {
            return Err(StorageError::invalid_input(
                "Variable property lookups require all variables to be bound".to_string(),
            ));
        }

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
            strategy_used: SeekStrategyType::VariablePropIndexSeek,
            rows_scanned,
        })
    }

    fn supports(&self, _context: &SeekStrategyContext) -> bool {
        // Support is available as long as there is a predicate with variable attributes.
        !self.predicates.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Expression;

    #[test]
    fn test_variable_predicate_op_from_str() {
        assert_eq!(
            "=".parse::<VariablePredicateOp>().ok(),
            Some(VariablePredicateOp::Eq)
        );
        assert_eq!(
            "<".parse::<VariablePredicateOp>().ok(),
            Some(VariablePredicateOp::Lt)
        );
        assert_eq!(
            ">=".parse::<VariablePredicateOp>().ok(),
            Some(VariablePredicateOp::Ge)
        );
        assert_eq!(
            "IN".parse::<VariablePredicateOp>().ok(),
            Some(VariablePredicateOp::In)
        );
        assert!("unknown".parse::<VariablePredicateOp>().is_err());
    }

    #[test]
    fn test_extract_variable_predicate() {
        let expr = Expression::binary(
            Expression::property(Expression::variable("v"), "name"),
            crate::core::BinaryOperator::Equal,
            Expression::variable("$varName"),
        );

        let pred = VariablePropIndexSeek::extract_predicate(&expr);
        assert!(pred.is_some());

        let pred = pred.expect("Failed to extract predicate");
        assert_eq!(pred.property, "name");
        assert_eq!(pred.op, VariablePredicateOp::Eq);
        assert_eq!(pred.variable_name, "varName");
    }

    #[test]
    fn test_variable_binding() {
        let mut seek = VariablePropIndexSeek::new(vec![VariablePropertyPredicate {
            property: "age".to_string(),
            op: VariablePredicateOp::Gt,
            variable_name: "minAge".to_string(),
        }]);

        assert!(!seek.all_variables_bound());

        seek.bind_variable("minAge", Value::Int(18));

        assert!(seek.all_variables_bound());
    }
}
