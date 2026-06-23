//! Operator type definition
//!
//! Define the various types of operators used in graph databases

use serde::{Deserialize, Serialize};
use std::fmt;

/// Implementation of a binary operator
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Exponent,
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    And,
    Or,
    Xor,
    StringConcat,
    Like,
    In,
    NotIn,
    Contains,
    StartsWith,
    EndsWith,
    Subscript,
    Attribute,
    Union,
    Intersect,
    Except,
}

impl BinaryOperator {
    pub fn name(&self) -> &str {
        match self {
            BinaryOperator::Add => "+",
            BinaryOperator::Subtract => "-",
            BinaryOperator::Multiply => "*",
            BinaryOperator::Divide => "/",
            BinaryOperator::Modulo => "%",
            BinaryOperator::Exponent => "**",
            BinaryOperator::Equal => "==",
            BinaryOperator::NotEqual => "!=",
            BinaryOperator::LessThan => "<",
            BinaryOperator::LessThanOrEqual => "<=",
            BinaryOperator::GreaterThan => ">",
            BinaryOperator::GreaterThanOrEqual => ">=",
            BinaryOperator::And => "AND",
            BinaryOperator::Or => "OR",
            BinaryOperator::Xor => "XOR",
            BinaryOperator::StringConcat => "||",
            BinaryOperator::Like => "=~",
            BinaryOperator::In => "IN",
            BinaryOperator::NotIn => "NOT IN",
            BinaryOperator::Contains => "CONTAINS",
            BinaryOperator::StartsWith => "STARTS WITH",
            BinaryOperator::EndsWith => "ENDS WITH",
            BinaryOperator::Subscript => "[]",
            BinaryOperator::Attribute => ".",
            BinaryOperator::Union => "UNION",
            BinaryOperator::Intersect => "INTERSECT",
            BinaryOperator::Except => "EXCEPT",
        }
    }

    pub fn precedence(&self) -> u8 {
        match self {
            BinaryOperator::Or => 1,
            BinaryOperator::And | BinaryOperator::Xor => 2,
            BinaryOperator::Equal
            | BinaryOperator::NotEqual
            | BinaryOperator::LessThan
            | BinaryOperator::LessThanOrEqual
            | BinaryOperator::GreaterThan
            | BinaryOperator::GreaterThanOrEqual => 3,
            BinaryOperator::In
            | BinaryOperator::NotIn
            | BinaryOperator::Like
            | BinaryOperator::Contains
            | BinaryOperator::StartsWith
            | BinaryOperator::EndsWith => 4,
            BinaryOperator::Union | BinaryOperator::Intersect | BinaryOperator::Except => 5,
            BinaryOperator::Add | BinaryOperator::Subtract => 6,
            BinaryOperator::Multiply | BinaryOperator::Divide | BinaryOperator::Modulo => 7,
            BinaryOperator::Exponent => 8,
            BinaryOperator::StringConcat => 9,
            BinaryOperator::Subscript | BinaryOperator::Attribute => 10,
        }
    }

    pub fn is_left_associative(&self) -> bool {
        true
    }

    pub fn arity(&self) -> usize {
        2
    }

    pub fn is_arithmetic(&self) -> bool {
        matches!(
            self,
            BinaryOperator::Add
                | BinaryOperator::Subtract
                | BinaryOperator::Multiply
                | BinaryOperator::Divide
                | BinaryOperator::Modulo
                | BinaryOperator::Exponent
        )
    }

    pub fn is_comparison(&self) -> bool {
        matches!(
            self,
            BinaryOperator::Equal
                | BinaryOperator::NotEqual
                | BinaryOperator::LessThan
                | BinaryOperator::LessThanOrEqual
                | BinaryOperator::GreaterThan
                | BinaryOperator::GreaterThanOrEqual
        )
    }

    pub fn is_logical(&self) -> bool {
        matches!(
            self,
            BinaryOperator::And | BinaryOperator::Or | BinaryOperator::Xor
        )
    }
}

impl fmt::Display for BinaryOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Implementation of a unary operator
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnaryOperator {
    Plus,
    Minus,
    Not,
    IsNull,
    IsNotNull,
    IsEmpty,
    IsNotEmpty,
}

impl UnaryOperator {
    pub fn name(&self) -> &str {
        match self {
            UnaryOperator::Plus => "+",
            UnaryOperator::Minus => "-",
            UnaryOperator::Not => "NOT",
            UnaryOperator::IsNull => "IS NULL",
            UnaryOperator::IsNotNull => "IS NOT NULL",
            UnaryOperator::IsEmpty => "IS EMPTY",
            UnaryOperator::IsNotEmpty => "IS NOT EMPTY",
        }
    }

    pub fn precedence(&self) -> u8 {
        match self {
            UnaryOperator::Plus | UnaryOperator::Minus | UnaryOperator::Not => 9,
            UnaryOperator::IsNull
            | UnaryOperator::IsNotNull
            | UnaryOperator::IsEmpty
            | UnaryOperator::IsNotEmpty => 3,
        }
    }

    pub fn is_left_associative(&self) -> bool {
        false
    }

    pub fn arity(&self) -> usize {
        1
    }
}

impl fmt::Display for UnaryOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Aggregate function operators
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AggregateFunction {
    Count(Option<String>),
    Sum(String),
    Avg(String),
    Min(String),
    Max(String),
    Collect(String),
    CollectSet(String),
    Distinct(String),
    Percentile(String, f64),
    Std(String),
    BitAnd(String),
    BitOr(String),
    GroupConcat(String, String),
    /// Vector sum - element-wise sum of vectors
    VecSum(String),
    /// Vector average - element-wise average of vectors
    VecAvg(String),
}

impl fmt::Display for AggregateFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl AggregateFunction {
    pub fn name(&self) -> &str {
        match self {
            AggregateFunction::Count(_) => "COUNT",
            AggregateFunction::Sum(_) => "SUM",
            AggregateFunction::Avg(_) => "AVG",
            AggregateFunction::Min(_) => "MIN",
            AggregateFunction::Max(_) => "MAX",
            AggregateFunction::Collect(_) => "COLLECT",
            AggregateFunction::CollectSet(_) => "COLLECT_SET",
            AggregateFunction::Distinct(_) => "DISTINCT",
            AggregateFunction::Percentile(_, _) => "PERCENTILE",
            AggregateFunction::Std(_) => "STD",
            AggregateFunction::BitAnd(_) => "BIT_AND",
            AggregateFunction::BitOr(_) => "BIT_OR",
            AggregateFunction::GroupConcat(_, _) => "GROUP_CONCAT",
            AggregateFunction::VecSum(_) => "VEC_SUM",
            AggregateFunction::VecAvg(_) => "VEC_AVG",
        }
    }

    pub fn precedence(&self) -> u8 {
        10
    }

    pub fn is_left_associative(&self) -> bool {
        true
    }

    pub fn arity(&self) -> usize {
        match self {
            AggregateFunction::Count(Some(_)) => 1,
            AggregateFunction::Count(None) => 0,
            AggregateFunction::Sum(_)
            | AggregateFunction::Avg(_)
            | AggregateFunction::Min(_)
            | AggregateFunction::Max(_)
            | AggregateFunction::Collect(_)
            | AggregateFunction::CollectSet(_)
            | AggregateFunction::Distinct(_)
            | AggregateFunction::Std(_)
            | AggregateFunction::BitAnd(_)
            | AggregateFunction::BitOr(_)
            | AggregateFunction::VecSum(_)
            | AggregateFunction::VecAvg(_) => 1,
            AggregateFunction::Percentile(_, _) => 2,
            AggregateFunction::GroupConcat(_, _) => {
                if self.separator().is_empty() {
                    1
                } else {
                    2
                }
            }
        }
    }

    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            AggregateFunction::Sum(_)
                | AggregateFunction::Avg(_)
                | AggregateFunction::Min(_)
                | AggregateFunction::Max(_)
                | AggregateFunction::Percentile(_, _)
                | AggregateFunction::Std(_)
                | AggregateFunction::VecSum(_)
                | AggregateFunction::VecAvg(_)
        )
    }

    pub fn is_collection(&self) -> bool {
        matches!(
            self,
            AggregateFunction::Count(_)
                | AggregateFunction::Collect(_)
                | AggregateFunction::CollectSet(_)
                | AggregateFunction::Distinct(_)
        )
    }

    pub fn separator(&self) -> String {
        match self {
            AggregateFunction::GroupConcat(_, sep) => sep.clone(),
            _ => String::new(),
        }
    }

    pub fn field_name(&self) -> Option<&str> {
        match self {
            AggregateFunction::Count(Some(field)) => Some(field),
            AggregateFunction::Count(None) => None,
            AggregateFunction::Sum(field) => Some(field),
            AggregateFunction::Avg(field) => Some(field),
            AggregateFunction::Min(field) => Some(field),
            AggregateFunction::Max(field) => Some(field),
            AggregateFunction::Collect(field) => Some(field),
            AggregateFunction::CollectSet(field) => Some(field),
            AggregateFunction::Distinct(field) => Some(field),
            AggregateFunction::Percentile(field, _) => Some(field),
            AggregateFunction::Std(field) => Some(field),
            AggregateFunction::BitAnd(field) => Some(field),
            AggregateFunction::BitOr(field) => Some(field),
            AggregateFunction::GroupConcat(field, _) => Some(field),
            AggregateFunction::VecSum(field) => Some(field),
            AggregateFunction::VecAvg(field) => Some(field),
        }
    }

    pub fn is_variadic(&self) -> bool {
        false
    }

    pub fn description(&self) -> &str {
        match self {
            AggregateFunction::Count(_) => "Calculated quantity",
            AggregateFunction::Sum(_) => "Calculate the sum",
            AggregateFunction::Avg(_) => "Calculation of average values",
            AggregateFunction::Min(_) => "Calculate minimum",
            AggregateFunction::Max(_) => "Calculate the maximum value",
            AggregateFunction::Collect(_) => "Collect all values",
            AggregateFunction::CollectSet(_) => "Collection of unique values",
            AggregateFunction::Distinct(_) => "deduplication",
            AggregateFunction::Percentile(_, _) => "Calculation of percentile",
            AggregateFunction::Std(_) => "calculate the standard deviation",
            AggregateFunction::BitAnd(_) => "compatibility with",
            AggregateFunction::BitOr(_) => "bitwise OR",
            AggregateFunction::GroupConcat(_, _) => "packet connection",
            AggregateFunction::VecSum(_) => "Calculate the element-by-element sum of vector",
            AggregateFunction::VecAvg(_) => {
                "Calculate the element-by-element average of the vector"
            }
        }
    }
}
