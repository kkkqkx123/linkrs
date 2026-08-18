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
    // JSONB operators
    JsonGet,         // ->
    JsonGetText,     // ->>
    JsonPathGet,     // #>
    JsonPathGetText, // #>>
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
            BinaryOperator::JsonGet => "->",
            BinaryOperator::JsonGetText => "->>",
            BinaryOperator::JsonPathGet => "#>",
            BinaryOperator::JsonPathGetText => "#>>",
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
            BinaryOperator::Subscript
            | BinaryOperator::Attribute
            | BinaryOperator::JsonGet
            | BinaryOperator::JsonGetText
            | BinaryOperator::JsonPathGet
            | BinaryOperator::JsonPathGetText => 10,
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

/// Aggregate function kind (parameterless).
///
/// Parameters (field name, separator, percentile, order-by columns) are
/// carried by `Expression::Aggregate.args` instead of being embedded in
/// each variant, so the operator enum stays a pure kind descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AggregateFunctionKind {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Collect,
    CollectSet,
    Percentile,
    Std,
    StddevPop,
    StddevSamp,
    Variance,
    Product,
    PercentileCont,
    Median,
    Mode,
    BitAnd,
    BitOr,
    BoolAnd,
    BoolOr,
    GroupConcat,
    /// GROUP_CONCAT with ORDER BY support for WITHIN GROUP clause
    GroupConcatWithOrder,
    /// Vector sum - element-wise sum of vectors
    VecSum,
    /// Vector average - element-wise average of vectors
    VecAvg,
}

impl fmt::Display for AggregateFunctionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl AggregateFunctionKind {
    pub fn name(&self) -> &str {
        match self {
            AggregateFunctionKind::Count => "COUNT",
            AggregateFunctionKind::Sum => "SUM",
            AggregateFunctionKind::Avg => "AVG",
            AggregateFunctionKind::Min => "MIN",
            AggregateFunctionKind::Max => "MAX",
            AggregateFunctionKind::Collect => "COLLECT",
            AggregateFunctionKind::CollectSet => "COLLECT_SET",
            AggregateFunctionKind::Percentile => "PERCENTILE",
            AggregateFunctionKind::Std => "STD",
            AggregateFunctionKind::StddevPop => "STDDEV_POP",
            AggregateFunctionKind::StddevSamp => "STDDEV_SAMP",
            AggregateFunctionKind::Variance => "VARIANCE",
            AggregateFunctionKind::Product => "PRODUCT",
            AggregateFunctionKind::PercentileCont => "PERCENTILE_CONT",
            AggregateFunctionKind::Median => "MEDIAN",
            AggregateFunctionKind::Mode => "MODE",
            AggregateFunctionKind::BitAnd => "BIT_AND",
            AggregateFunctionKind::BitOr => "BIT_OR",
            AggregateFunctionKind::BoolAnd => "BOOL_AND",
            AggregateFunctionKind::BoolOr => "BOOL_OR",
            AggregateFunctionKind::GroupConcat => "GROUP_CONCAT",
            AggregateFunctionKind::GroupConcatWithOrder => "GROUP_CONCAT",
            AggregateFunctionKind::VecSum => "VEC_SUM",
            AggregateFunctionKind::VecAvg => "VEC_AVG",
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
            AggregateFunctionKind::Count
            | AggregateFunctionKind::Sum
            | AggregateFunctionKind::Avg
            | AggregateFunctionKind::Min
            | AggregateFunctionKind::Max
            | AggregateFunctionKind::Collect
            | AggregateFunctionKind::CollectSet
            | AggregateFunctionKind::Std
            | AggregateFunctionKind::StddevPop
            | AggregateFunctionKind::StddevSamp
            | AggregateFunctionKind::Product
            | AggregateFunctionKind::Variance
            | AggregateFunctionKind::Median
            | AggregateFunctionKind::Mode
            | AggregateFunctionKind::BitAnd
            | AggregateFunctionKind::BitOr
            | AggregateFunctionKind::BoolAnd
            | AggregateFunctionKind::BoolOr
            | AggregateFunctionKind::VecSum
            | AggregateFunctionKind::VecAvg => 1,
            AggregateFunctionKind::Percentile | AggregateFunctionKind::PercentileCont => 2,
            AggregateFunctionKind::GroupConcat | AggregateFunctionKind::GroupConcatWithOrder => 1,
        }
    }

    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            AggregateFunctionKind::Sum
                | AggregateFunctionKind::Avg
                | AggregateFunctionKind::Min
                | AggregateFunctionKind::Max
                | AggregateFunctionKind::Percentile
                | AggregateFunctionKind::PercentileCont
                | AggregateFunctionKind::Std
                | AggregateFunctionKind::StddevPop
                | AggregateFunctionKind::StddevSamp
                | AggregateFunctionKind::Product
                | AggregateFunctionKind::Variance
                | AggregateFunctionKind::Median
                | AggregateFunctionKind::VecSum
                | AggregateFunctionKind::VecAvg
        )
    }

    pub fn is_collection(&self) -> bool {
        matches!(
            self,
            AggregateFunctionKind::Count
                | AggregateFunctionKind::Collect
                | AggregateFunctionKind::CollectSet
        )
    }

    pub fn is_variadic(&self) -> bool {
        false
    }

    pub fn description(&self) -> &str {
        match self {
            AggregateFunctionKind::Count => "Calculated quantity",
            AggregateFunctionKind::Sum => "Calculate the sum",
            AggregateFunctionKind::Avg => "Calculation of average values",
            AggregateFunctionKind::Min => "Calculate minimum",
            AggregateFunctionKind::Max => "Calculate the maximum value",
            AggregateFunctionKind::Collect => "Collect all values",
            AggregateFunctionKind::CollectSet => "Collection of unique values",
            AggregateFunctionKind::Percentile => "Calculation of percentile",
            AggregateFunctionKind::PercentileCont => "Continuous percentile (with interpolation)",
            AggregateFunctionKind::Std => "calculate the standard deviation",
            AggregateFunctionKind::StddevPop => "Population standard deviation",
            AggregateFunctionKind::StddevSamp => "Sample standard deviation",
            AggregateFunctionKind::Product => "Compute the product of values",
            AggregateFunctionKind::Variance => "Calculate the variance",
            AggregateFunctionKind::Median => "Calculate the median",
            AggregateFunctionKind::Mode => "Calculate the mode",
            AggregateFunctionKind::BitAnd => "compatibility with",
            AggregateFunctionKind::BitOr => "bitwise OR",
            AggregateFunctionKind::BoolAnd => "logical AND",
            AggregateFunctionKind::BoolOr => "logical OR",
            AggregateFunctionKind::GroupConcat => "packet connection",
            AggregateFunctionKind::GroupConcatWithOrder => {
                "Group concatenation with ORDER BY (WITHIN GROUP)"
            }
            AggregateFunctionKind::VecSum => "Calculate the element-by-element sum of vector",
            AggregateFunctionKind::VecAvg => {
                "Calculate the element-by-element average of the vector"
            }
        }
    }
}

/// Backward-compatible alias for callers that only need the kind.
pub type AggregateFunction = AggregateFunctionKind;
