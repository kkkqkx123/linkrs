//! The expression string represents…
//!
//! Provide a method for converting expressions into strings.

use crate::core::types::expr::Expression;
use std::fmt;

impl Expression {
    /// Convert the expression into a string representation.
    ///
    /// Generate an expression string similar to SQL/Cypher.
    pub fn to_expression_string(&self) -> String {
        match self {
            Expression::Literal(v) => format!("{:?}", v),
            Expression::Variable(name) => name.clone(),
            Expression::Property { object, property } => {
                format!("{}.{}", object.to_expression_string(), property)
            }
            Expression::Binary { left, op, right } => {
                format!(
                    "({} {} {})",
                    left.to_expression_string(),
                    op.name(),
                    right.to_expression_string()
                )
            }
            Expression::Unary { op, operand } => {
                format!("({} {})", op.name(), operand.to_expression_string())
            }
            Expression::Function { name, args } => {
                let args_str = args
                    .iter()
                    .map(|e| e.to_expression_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({})", name, args_str)
            }
            Expression::Aggregate {
                func,
                arg,
                distinct,
            } => {
                let distinct_str = if *distinct { "DISTINCT " } else { "" };
                format!(
                    "{}({}{})",
                    func.name(),
                    distinct_str,
                    arg.to_expression_string()
                )
            }
            Expression::List(items) => {
                let items_str = items
                    .iter()
                    .map(|e| e.to_expression_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{}]", items_str)
            }
            Expression::Map(pairs) => {
                let pairs_str = pairs
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, v.to_expression_string()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{{}}}", pairs_str)
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                let mut result = String::from("CASE ");
                if let Some(expr) = test_expr {
                    result.push_str(&format!("{} ", expr.to_expression_string()));
                }
                for (cond, value) in conditions {
                    result.push_str(&format!(
                        "WHEN {} THEN {} ",
                        cond.to_expression_string(),
                        value.to_expression_string()
                    ));
                }
                if let Some(def) = default {
                    result.push_str(&format!("ELSE {} ", def.to_expression_string()));
                }
                result.push_str("END");
                result
            }
            Expression::TypeCast {
                expression,
                target_type,
            } => {
                format!(
                    "({} AS {:?})",
                    expression.to_expression_string(),
                    target_type
                )
            }
            Expression::Subscript { collection, index } => {
                format!(
                    "{}[{}]",
                    collection.to_expression_string(),
                    index.to_expression_string()
                )
            }
            Expression::Range {
                collection,
                start,
                end,
            } => {
                let start_str = start
                    .as_ref()
                    .map(|e| e.to_expression_string())
                    .unwrap_or_default();
                let end_str = end
                    .as_ref()
                    .map(|e| e.to_expression_string())
                    .unwrap_or_default();
                format!(
                    "{}[{}..{}]",
                    collection.to_expression_string(),
                    start_str,
                    end_str
                )
            }
            Expression::Path(items) => {
                let items_str = items
                    .iter()
                    .map(|e| e.to_expression_string())
                    .collect::<Vec<_>>()
                    .join("->");
                format!("({})", items_str)
            }
            Expression::Label(name) => format!(":{}", name),
            Expression::ListComprehension {
                variable,
                source,
                filter,
                map,
            } => {
                let source_str = source.to_expression_string();
                let filter_str = filter
                    .as_ref()
                    .map(|f| format!(" WHERE {}", f.to_expression_string()))
                    .unwrap_or_default();
                let map_str = map
                    .as_ref()
                    .map(|m| format!(" | {}", m.to_expression_string()))
                    .unwrap_or_default();
                format!("[{} IN {}{}{}]", variable, source_str, filter_str, map_str)
            }
            Expression::LabelTagProperty { tag, property } => {
                format!("{}.{}", tag.to_expression_string(), property)
            }
            Expression::TagProperty { tag_name, property } => {
                format!("{}.{}", tag_name, property)
            }
            Expression::EdgeProperty {
                edge_name,
                property,
            } => {
                format!("{}.{}", edge_name, property)
            }
            Expression::Predicate { func, args } => {
                let args_str = args
                    .iter()
                    .map(|e| e.to_expression_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({})", func, args_str)
            }
            Expression::Reduce {
                accumulator,
                initial,
                variable,
                source,
                mapping,
            } => {
                format!(
                    "REDUCE({} = {}, {} IN {} | {})",
                    accumulator,
                    initial.to_expression_string(),
                    variable,
                    source.to_expression_string(),
                    mapping.to_expression_string()
                )
            }
            Expression::PathBuild(items) => {
                let items_str = items
                    .iter()
                    .map(|e| e.to_expression_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("PATH({})", items_str)
            }
            Expression::Parameter(name) => format!("${}", name),
            Expression::Vector(data) => {
                format!(
                    "VECTOR[{}]",
                    data.iter()
                        .map(|f| f.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
    }
}

impl fmt::Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_expression_string())
    }
}
