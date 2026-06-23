use crate::core::types::operators::{BinaryOperator, UnaryOperator};
use crate::core::value::list::List;
/// Arithmetic and logical operations module
///
/// This module is responsible for handling basic arithmetic operations, comparison operations, logical operations, and other fundamental operations involved in the evaluation of expressions.
use crate::core::value::Value;
use crate::query::executor::expression::evaluator::collection_operations::CollectionOperationEvaluator;
use crate::query::executor::expression::ExpressionError;

/// Binary operation evaluator
pub struct BinaryOperationEvaluator;

impl BinaryOperationEvaluator {
    /// Evaluating binary operations
    pub fn evaluate(
        left: &Value,
        op: &BinaryOperator,
        right: &Value,
    ) -> Result<Value, ExpressionError> {
        match op {
            // Arithmetic operations
            BinaryOperator::Add => Self::eval_add(left, right),
            BinaryOperator::Subtract => Self::eval_subtract(left, right),
            BinaryOperator::Multiply => Self::eval_multiply(left, right),
            BinaryOperator::Divide => Self::eval_divide(left, right),
            BinaryOperator::Modulo => Self::eval_modulo(left, right),
            BinaryOperator::Exponent => Self::eval_exponent(left, right),

            // Comparison operations
            BinaryOperator::Equal => Self::eval_equal(left, right),
            BinaryOperator::NotEqual => Self::eval_not_equal(left, right),
            BinaryOperator::LessThan => Self::eval_less_than(left, right),
            BinaryOperator::LessThanOrEqual => Self::eval_less_than_or_equal(left, right),
            BinaryOperator::GreaterThan => Self::eval_greater_than(left, right),
            BinaryOperator::GreaterThanOrEqual => Self::eval_greater_than_or_equal(left, right),

            // Logical operations
            BinaryOperator::And => Self::eval_and(left, right),
            BinaryOperator::Or => Self::eval_or(left, right),
            BinaryOperator::Xor => Self::eval_xor(left, right),

            // String operations
            BinaryOperator::StringConcat => Self::eval_string_concat(left, right),
            BinaryOperator::Like => Self::eval_like(left, right),
            BinaryOperator::In => Self::eval_in(left, right),
            BinaryOperator::NotIn => Self::eval_not_in(left, right),
            BinaryOperator::Contains => Self::eval_contains(left, right),
            BinaryOperator::StartsWith => Self::eval_starts_with(left, right),
            BinaryOperator::EndsWith => Self::eval_ends_with(left, right),

            // Access operation – delegated to CollectionOperationEvaluator
            BinaryOperator::Subscript => {
                CollectionOperationEvaluator::eval_subscript_access(left, right)
            }
            BinaryOperator::Attribute => {
                CollectionOperationEvaluator::eval_attribute_access(left, right)
            }

            // Set operations
            BinaryOperator::Union => Self::eval_union(left, right),
            BinaryOperator::Intersect => Self::eval_intersect(left, right),
            BinaryOperator::Except => Self::eval_except(left, right),
        }
    }

    fn eval_add(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        left.add(right).map_err(ExpressionError::runtime_error)
    }

    fn eval_subtract(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        left.sub(right).map_err(ExpressionError::runtime_error)
    }

    fn eval_multiply(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        left.mul(right).map_err(ExpressionError::runtime_error)
    }

    fn eval_divide(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        left.div(right).map_err(ExpressionError::runtime_error)
    }

    fn eval_modulo(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        left.rem(right).map_err(ExpressionError::runtime_error)
    }

    fn eval_exponent(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        left.pow(right).map_err(ExpressionError::runtime_error)
    }

    fn eval_equal(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        Ok(Value::Bool(left == right))
    }

    fn eval_not_equal(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        Ok(Value::Bool(left != right))
    }

    fn eval_less_than(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        Ok(Value::Bool(left < right))
    }

    fn eval_less_than_or_equal(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        Ok(Value::Bool(left <= right))
    }

    fn eval_greater_than(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        Ok(Value::Bool(left > right))
    }

    fn eval_greater_than_or_equal(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        Ok(Value::Bool(left >= right))
    }

    fn eval_and(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        left.and(right).map_err(ExpressionError::runtime_error)
    }

    fn eval_or(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        left.or(right).map_err(ExpressionError::runtime_error)
    }

    fn eval_xor(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        match (left, right) {
            (Value::Bool(l), Value::Bool(r)) => Ok(Value::Bool(*l ^ *r)),
            _ => Err(ExpressionError::type_error(
                "The XOR operation requires boolean values.",
            )),
        }
    }

    fn eval_string_concat(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        left.add(right).map_err(ExpressionError::runtime_error)
    }

    fn eval_like(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        if left.is_bad_null() || right.is_bad_null() {
            return Ok(Value::Null(crate::core::value::NullType::BadData));
        }

        if (!left.is_null() && !left.is_empty() && !matches!(left, Value::String(_)))
            || (!right.is_null() && !right.is_empty() && !matches!(right, Value::String(_)))
        {
            return Ok(Value::Null(crate::core::value::NullType::BadData));
        }

        if left.is_null() || right.is_null() {
            return Ok(Value::Null(crate::core::value::NullType::Null));
        }

        match (left, right) {
            (Value::String(l), Value::String(r)) => Self::eval_like_operation(l, r),
            _ => Ok(Value::Null(crate::core::value::NullType::BadData)),
        }
    }

    fn eval_like_operation(pattern: &str, text: &str) -> Result<Value, ExpressionError> {
        let mut pattern_chars = pattern.chars().peekable();
        let mut text_chars = text.chars().peekable();

        while let Some(p) = pattern_chars.next() {
            match p {
                '%' => {
                    let remaining_pattern: String = pattern_chars.collect();
                    let remaining_text: String = text_chars.collect();

                    if remaining_pattern.is_empty() {
                        return Ok(Value::Bool(true));
                    }

                    for i in 0..=remaining_text.len() {
                        if let Value::Bool(b) =
                            Self::eval_like_operation(&remaining_pattern, &remaining_text[i..])?
                        {
                            if b {
                                return Ok(Value::Bool(true));
                            }
                        }
                    }

                    return Ok(Value::Bool(false));
                }
                '_' => {
                    if text_chars.next().is_none() {
                        return Ok(Value::Bool(false));
                    }
                }
                '\\' => {
                    if let Some(escaped_char) = pattern_chars.next() {
                        if text_chars.next() != Some(escaped_char) {
                            return Ok(Value::Bool(false));
                        }
                    }
                }
                c => {
                    if text_chars.next() != Some(c) {
                        return Ok(Value::Bool(false));
                    }
                }
            }
        }

        Ok(Value::Bool(text_chars.next().is_none()))
    }

    fn eval_in(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        if left.is_null() || right.is_null() {
            return Ok(Value::Null(crate::core::value::NullType::Null));
        }

        match right {
            Value::List(items) => {
                if items.iter().any(|item| item.is_null()) {
                    return Ok(Value::Null(crate::core::value::NullType::Null));
                }
                Ok(Value::Bool(items.contains(left)))
            }
            _ => Err(ExpressionError::type_error(
                "The right side of the IN operation must be a list.",
            )),
        }
    }

    fn eval_not_in(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        if left.is_null() || right.is_null() {
            return Ok(Value::Null(crate::core::value::NullType::Null));
        }

        match right {
            Value::List(items) => {
                if items.iter().any(|item| item.is_null()) {
                    return Ok(Value::Null(crate::core::value::NullType::Null));
                }
                Ok(Value::Bool(!items.contains(left)))
            }
            _ => Err(ExpressionError::type_error(
                "The right side of the NOT IN operation must be a list.",
            )),
        }
    }

    fn eval_contains(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        if left.is_null() || right.is_null() {
            return Ok(Value::Null(crate::core::value::NullType::Null));
        }

        match (&left, &right) {
            (Value::String(s), Value::String(sub)) => Ok(Value::Bool(s.contains(sub))),
            (Value::List(items), item) => {
                if items.iter().any(|i| i.is_null()) {
                    return Ok(Value::Null(crate::core::value::NullType::Null));
                }
                Ok(Value::Bool(items.contains(item)))
            }
            _ => Err(ExpressionError::type_error(
                "The CONTAINS operation requires a string or a list.",
            )),
        }
    }

    fn eval_starts_with(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        if left.is_null() || right.is_null() {
            return Ok(Value::Null(crate::core::value::NullType::Null));
        }

        match (&left, &right) {
            (Value::String(s), Value::String(prefix)) => Ok(Value::Bool(s.starts_with(prefix))),
            _ => Err(ExpressionError::type_error(
                "The `STARTS WITH` operation requires a string value.",
            )),
        }
    }

    fn eval_ends_with(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        if left.is_null() || right.is_null() {
            return Ok(Value::Null(crate::core::value::NullType::Null));
        }

        match (&left, &right) {
            (Value::String(s), Value::String(suffix)) => Ok(Value::Bool(s.ends_with(suffix))),
            _ => Err(ExpressionError::type_error(
                "The ENDS WITH operation requires a string value.",
            )),
        }
    }

    fn eval_union(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        if left.is_null() || right.is_null() {
            return Ok(Value::Null(crate::core::value::NullType::Null));
        }

        match (left, right) {
            (Value::List(l), Value::List(r)) => {
                if l.iter().any(|item| item.is_null()) || r.iter().any(|item| item.is_null()) {
                    return Ok(Value::Null(crate::core::value::NullType::Null));
                }
                let mut result = (**l).clone();
                result.extend(r.iter().cloned());
                Ok(Value::list(result))
            }
            _ => Err(ExpressionError::type_error("UNION requires list values")),
        }
    }

    fn eval_intersect(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        if left.is_null() || right.is_null() {
            return Ok(Value::Null(crate::core::value::NullType::Null));
        }

        match (left, right) {
            (Value::List(l), Value::List(r)) => {
                if l.iter().any(|item| item.is_null()) || r.iter().any(|item| item.is_null()) {
                    return Ok(Value::Null(crate::core::value::NullType::Null));
                }
                let result: Vec<Value> =
                    l.iter().filter(|item| r.contains(item)).cloned().collect();
                Ok(Value::list(List::from(result)))
            }
            _ => Err(ExpressionError::type_error(
                "INTERSECT requires list values",
            )),
        }
    }

    fn eval_except(left: &Value, right: &Value) -> Result<Value, ExpressionError> {
        if left.is_null() || right.is_null() {
            return Ok(Value::Null(crate::core::value::NullType::Null));
        }

        match (left, right) {
            (Value::List(l), Value::List(r)) => {
                if l.iter().any(|item| item.is_null()) || r.iter().any(|item| item.is_null()) {
                    return Ok(Value::Null(crate::core::value::NullType::Null));
                }
                let result: Vec<Value> =
                    l.iter().filter(|item| !r.contains(item)).cloned().collect();
                Ok(Value::list(List::from(result)))
            }
            _ => Err(ExpressionError::type_error("EXCEPT requires list values")),
        }
    }
}

/// One-element operation evaluator
pub struct UnaryOperationEvaluator;

impl UnaryOperationEvaluator {
    /// Evaluating a unary operation
    pub fn evaluate(op: &UnaryOperator, value: &Value) -> Result<Value, ExpressionError> {
        match op {
            // Arithmetic operations
            UnaryOperator::Plus => Self::eval_plus(value),
            UnaryOperator::Minus => Self::eval_minus(value),

            // Logical operations
            UnaryOperator::Not => Self::eval_not(value),

            // Existence check
            UnaryOperator::IsNull => Self::eval_is_null(value),
            UnaryOperator::IsNotNull => Self::eval_is_not_null(value),
            UnaryOperator::IsEmpty => Self::eval_is_empty(value),
            UnaryOperator::IsNotEmpty => Self::eval_is_not_empty(value),
        }
    }

    fn eval_plus(value: &Value) -> Result<Value, ExpressionError> {
        Ok(value.clone())
    }

    fn eval_minus(value: &Value) -> Result<Value, ExpressionError> {
        value.neg().map_err(ExpressionError::runtime_error)
    }

    fn eval_not(value: &Value) -> Result<Value, ExpressionError> {
        value.not().map_err(ExpressionError::runtime_error)
    }

    fn eval_is_null(value: &Value) -> Result<Value, ExpressionError> {
        Ok(Value::Bool(value.is_null()))
    }

    fn eval_is_not_null(value: &Value) -> Result<Value, ExpressionError> {
        Ok(Value::Bool(!value.is_null()))
    }

    fn eval_is_empty(value: &Value) -> Result<Value, ExpressionError> {
        match value {
            Value::String(s) => Ok(Value::Bool(s.is_empty())),
            Value::List(l) => Ok(Value::Bool(l.is_empty())),
            Value::Map(m) => Ok(Value::Bool(m.is_empty())),
            _ => Err(ExpressionError::type_error(
                "The EMPTY check requires the container type.",
            )),
        }
    }

    fn eval_is_not_empty(value: &Value) -> Result<Value, ExpressionError> {
        match value {
            Value::String(s) => Ok(Value::Bool(!s.is_empty())),
            Value::List(l) => Ok(Value::Bool(!l.is_empty())),
            Value::Map(m) => Ok(Value::Bool(!m.is_empty())),
            _ => Err(ExpressionError::type_error(
                "The EMPTY check requires the container type.",
            )),
        }
    }
}
