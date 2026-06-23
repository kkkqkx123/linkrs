//! Set operation evaluator
//!
//! Provide functionality for evaluating collection types, including index access, range access, and property access.

use crate::core::value::list::List;
use crate::core::Value;
use crate::query::executor::expression::ExpressionError;

/// Set operation evaluator
pub struct CollectionOperationEvaluator;

impl CollectionOperationEvaluator {
    /// Try to convert the Value to an i64 index.
    fn value_to_i64(index: &Value) -> Option<i64> {
        match index {
            Value::SmallInt(i) => Some(*i as i64),
            Value::Int(i) => Some(*i as i64),
            Value::BigInt(i) => Some(*i),
            _ => None,
        }
    }

    /// Index access for evaluation
    pub fn eval_subscript_access(
        collection: &Value,
        index: &Value,
    ) -> Result<Value, ExpressionError> {
        if collection.is_null() || index.is_null() {
            return Ok(Value::Null(crate::core::value::NullType::Null));
        }

        match collection {
            Value::List(list) => {
                if let Some(i) = Self::value_to_i64(index) {
                    let adjusted_index = if i < 0 { list.len() as i64 + i } else { i };

                    if adjusted_index >= 0 && (adjusted_index as usize) < list.len() {
                        Ok(list[adjusted_index as usize].clone())
                    } else {
                        Err(ExpressionError::index_out_of_bounds(
                            adjusted_index as isize,
                            list.len(),
                        ))
                    }
                } else {
                    Err(ExpressionError::type_error(
                        "List subscripts must be integers",
                    ))
                }
            }
            Value::String(s) => {
                if let Some(i) = Self::value_to_i64(index) {
                    let chars: Vec<char> = s.chars().collect();
                    let adjusted_index = if i < 0 { chars.len() as i64 + i } else { i };

                    if adjusted_index >= 0 && (adjusted_index as usize) < chars.len() {
                        Ok(Value::String(chars[adjusted_index as usize].to_string()))
                    } else {
                        Err(ExpressionError::index_out_of_bounds(
                            adjusted_index as isize,
                            chars.len(),
                        ))
                    }
                } else {
                    Err(ExpressionError::type_error(
                        "String subscripts must be integers",
                    ))
                }
            }
            Value::Map(map) => {
                if let Value::String(key) = index {
                    map.get(key).cloned().ok_or_else(|| {
                        ExpressionError::runtime_error(format!(
                            "Mapping key does not exist: {}",
                            key
                        ))
                    })
                } else {
                    Err(ExpressionError::type_error("Mapping key must be a string"))
                }
            }
            _ => Err(ExpressionError::type_error(
                "Types for which subscript access are not supported",
            )),
        }
    }

    /// Access to the evaluation range
    pub fn eval_range_access(
        collection: &Value,
        start: Option<&Value>,
        end: Option<&Value>,
    ) -> Result<Value, ExpressionError> {
        if collection.is_null() {
            return Ok(Value::Null(crate::core::value::NullType::Null));
        }

        if start.is_some_and(|v| v.is_null()) || end.is_some_and(|v| v.is_null()) {
            return Ok(Value::Null(crate::core::value::NullType::Null));
        }

        match collection {
            Value::List(list) => {
                let start_idx = start
                    .map(|v| {
                        if let Value::Int(i) = v {
                            if *i < 0 {
                                (list.len() as i32 + i) as usize
                            } else {
                                *i as usize
                            }
                        } else {
                            0
                        }
                    })
                    .unwrap_or(0);

                let end_idx = end
                    .map(|v| {
                        if let Value::Int(i) = v {
                            if *i < 0 {
                                (list.len() as i32 + i) as usize
                            } else {
                                *i as usize
                            }
                        } else {
                            list.len()
                        }
                    })
                    .unwrap_or(list.len());

                if start_idx <= end_idx && end_idx <= list.len() {
                    Ok(Value::list(List::from(list[start_idx..end_idx].to_vec())))
                } else {
                    Err(ExpressionError::index_out_of_bounds(
                        start_idx as isize,
                        list.len(),
                    ))
                }
            }
            Value::String(s) => {
                let chars: Vec<char> = s.chars().collect();
                let start_idx = start
                    .map(|v| {
                        if let Value::Int(i) = v {
                            if *i < 0 {
                                (chars.len() as i32 + i) as usize
                            } else {
                                *i as usize
                            }
                        } else {
                            0
                        }
                    })
                    .unwrap_or(0);

                let end_idx = end
                    .map(|v| {
                        if let Value::Int(i) = v {
                            if *i < 0 {
                                (chars.len() as i32 + i) as usize
                            } else {
                                *i as usize
                            }
                        } else {
                            chars.len()
                        }
                    })
                    .unwrap_or(chars.len());

                if start_idx <= end_idx && end_idx <= chars.len() {
                    let result: String = chars[start_idx..end_idx].iter().collect();
                    Ok(Value::String(result))
                } else {
                    Err(ExpressionError::index_out_of_bounds(
                        start_idx as isize,
                        chars.len(),
                    ))
                }
            }
            _ => Err(ExpressionError::type_error(
                "Types of scope access are not supported",
            )),
        }
    }

    /// Access to the evaluation attribute
    pub fn eval_property_access(object: &Value, property: &str) -> Result<Value, ExpressionError> {
        if object.is_null() {
            return Ok(Value::Null(crate::core::value::NullType::Null));
        }

        match object {
            Value::Vertex(vertex) => {
                if let Some(val) = vertex.properties.get(property) {
                    return Ok(val.clone());
                }
                for tag in &vertex.tags {
                    if let Some(val) = tag.properties.get(property) {
                        return Ok(val.clone());
                    }
                    if tag.name == property {
                        return Ok(Value::map(tag.properties.clone()));
                    }
                }
                Ok(Value::Null(crate::core::value::NullType::Null))
            }
            Value::Edge(edge) => Ok(edge
                .properties()
                .get(property)
                .cloned()
                .unwrap_or(Value::Null(crate::core::value::NullType::Null))),
            Value::Map(map) => Ok(map
                .get(property)
                .cloned()
                .unwrap_or(Value::Null(crate::core::value::NullType::Null))),
            Value::List(list) => {
                if let Ok(index) = property.parse::<isize>() {
                    let adjusted_index = if index < 0 {
                        list.len() as isize + index
                    } else {
                        index
                    };

                    if adjusted_index >= 0 && adjusted_index < list.len() as isize {
                        Ok(list[adjusted_index as usize].clone())
                    } else {
                        Err(ExpressionError::index_out_of_bounds(
                            adjusted_index,
                            list.len(),
                        ))
                    }
                } else {
                    Err(ExpressionError::type_error("List index must be an integer"))
                }
            }
            _ => Err(ExpressionError::type_error(
                "Types of property access are not supported",
            )),
        }
    }

    /// Access to evaluation attributes (Attribute operations, used for BinaryOperator::Attribute)
    /// Format the value on the right side as a string and use it as the attribute name.
    pub fn eval_attribute_access(
        object: &Value,
        attribute: &Value,
    ) -> Result<Value, ExpressionError> {
        let property = format!("{}", attribute);
        Self::eval_property_access(object, &property)
    }
}
