//! Implementation of container operation functions
//!
//! Provide functions for operating on lists and maps, including head, last, tail, size, range, and keys.

use crate::core::value::list::List;
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::expression::ExpressionError;
use std::collections::BTreeSet;

/// Container function enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerFunction {
    Head,
    Last,
    Tail,
    Size,
    Range,
    Keys,
    ReverseList,
    ToSet,
}

impl ContainerFunction {
    /// Obtain the function name
    pub fn name(&self) -> &str {
        match self {
            Self::Head => "head",
            Self::Last => "last",
            Self::Tail => "tail",
            Self::Size => "size",
            Self::Range => "range",
            Self::Keys => "keys",
            Self::ReverseList => "reverse",
            Self::ToSet => "toset",
        }
    }

    /// Determine the number of parameters
    pub fn arity(&self) -> usize {
        match self {
            Self::Head => 1,
            Self::Last => 1,
            Self::Tail => 1,
            Self::Size => 1,
            Self::Range => 2,
            Self::Keys => 1,
            Self::ReverseList => 1,
            Self::ToSet => 1,
        }
    }

    /// Is it a function with variable parameters?
    pub fn is_variadic(&self) -> bool {
        matches!(self, Self::Range)
    }

    /// Obtain the function description
    pub fn description(&self) -> &str {
        match self {
            Self::Head => "Get the first element of the list",
            Self::Last => "Get the last element of the list",
            Self::Tail => "Get all elements of the list except the first one",
            Self::Size => "Get the size of a string, list, map, or set",
            Self::Range => "Generate a list of integer ranges",
            Self::Keys => "Get all the keys for vertices, edges, or mappings.",
            Self::ReverseList => "inversion list",
            Self::ToSet => "Convert the list to a set.",
        }
    }

    pub fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        match self {
            Self::Head => execute_head(args),
            Self::Last => execute_last(args),
            Self::Tail => execute_tail(args),
            Self::Size => execute_size(args),
            Self::Range => execute_range(args),
            Self::Keys => execute_keys(args),
            Self::ReverseList => execute_reverse_list(args),
            Self::ToSet => execute_toset(args),
        }
    }
}

fn execute_head(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("head requires 1 argument"));
    }
    match &args[0] {
        Value::List(list) => Ok(list
            .values
            .first()
            .cloned()
            .unwrap_or(Value::Null(NullType::Null))),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("head requires a list type")),
    }
}

fn execute_last(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("last requires 1 argument"));
    }
    match &args[0] {
        Value::List(list) => Ok(list
            .values
            .last()
            .cloned()
            .unwrap_or(Value::Null(NullType::Null))),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("last requires a list type")),
    }
}

fn execute_tail(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("tail requires 1 argument"));
    }
    match &args[0] {
        Value::List(list) => {
            if list.values.is_empty() {
                Ok(Value::list(List { values: vec![] }))
            } else {
                Ok(Value::list(List {
                    values: list.values[1..].to_vec(),
                }))
            }
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("tail requires a list type")),
    }
}

fn execute_size(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("size requires 1 argument"));
    }
    match &args[0] {
        Value::String(s) => Ok(Value::BigInt(s.len() as i64)),
        Value::List(list) => Ok(Value::BigInt(list.values.len() as i64)),
        Value::Map(map) => Ok(Value::BigInt(map.len() as i64)),
        Value::Set(set) => Ok(Value::BigInt(set.len() as i64)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "size requires string, list, map or set type",
        )),
    }
}

fn execute_range(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() < 2 || args.len() > 3 {
        return Err(ExpressionError::type_error(
            "range requires 2 or 3 arguments",
        ));
    }
    let start = match &args[0] {
        Value::Int(i) => *i,
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "range requires integer arguments",
            ))
        }
    };
    let end = match &args[1] {
        Value::Int(i) => *i,
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "The range function takes integer arguments",
            ))
        }
    };
    let step = if args.len() > 2 {
        match &args[2] {
            Value::Int(i) => *i,
            Value::Null(_) => return Ok(Value::Null(NullType::Null)),
            _ => return Err(ExpressionError::type_error("range step must be an integer")),
        }
    } else {
        1
    };

    if step == 0 {
        return Err(ExpressionError::new(
            crate::query::executor::expression::ExpressionErrorType::InvalidOperation,
            "range step cannot be 0".to_string(),
        ));
    }

    let mut result = Vec::new();
    if step > 0 {
        let mut i = start;
        while i <= end {
            result.push(Value::Int(i));
            i += step;
        }
    } else {
        let mut i = start;
        while i >= end {
            result.push(Value::Int(i));
            i += step;
        }
    }

    Ok(Value::list(List { values: result }))
}

fn execute_keys(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("keys requires 1 argument"));
    }
    let mut keys: BTreeSet<String> = BTreeSet::new();

    match &args[0] {
        Value::Vertex(v) => {
            for tag in &v.tags {
                for key in tag.properties.keys() {
                    keys.insert(key.clone());
                }
            }
            for key in v.properties.keys() {
                keys.insert(key.clone());
            }
        }
        Value::Edge(e) => {
            for key in e.props.keys() {
                keys.insert(key.clone());
            }
        }
        Value::Map(m) => {
            for key in m.keys() {
                keys.insert(key.clone());
            }
        }
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "keys requires vertex, edge or map type",
            ))
        }
    }

    let result: Vec<Value> = keys.into_iter().map(Value::String).collect();
    Ok(Value::list(List { values: result }))
}

fn execute_reverse_list(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("reverse requires 1 argument"));
    }
    match &args[0] {
        Value::List(list) => {
            let mut reversed = list.values.clone();
            reversed.reverse();
            Ok(Value::list(List { values: reversed }))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("reverse requires a list type")),
    }
}

fn execute_toset(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("toset requires 1 argument"));
    }
    match &args[0] {
        Value::List(list) => {
            let set: std::collections::HashSet<Value> = list.values.iter().cloned().collect();
            Ok(Value::set(set))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("toset requires a list type")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_head_function() {
        let list = Value::list(List {
            values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
        });
        let result = ContainerFunction::Head
            .execute(&[list])
            .expect("head function should succeed");
        assert_eq!(result, Value::Int(1));
    }

    #[test]
    fn test_head_empty_list() {
        let list = Value::list(List { values: vec![] });
        let result = ContainerFunction::Head
            .execute(&[list])
            .expect("head function should succeed");
        assert_eq!(result, Value::Null(NullType::Null));
    }

    #[test]
    fn test_last_function() {
        let list = Value::list(List {
            values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
        });
        let result = ContainerFunction::Last
            .execute(&[list])
            .expect("last function should succeed");
        assert_eq!(result, Value::Int(3));
    }

    #[test]
    fn test_tail_function() {
        let list = Value::list(List {
            values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
        });
        let result = ContainerFunction::Tail
            .execute(&[list])
            .expect("tail function should succeed");
        assert_eq!(
            result,
            Value::list(List {
                values: vec![Value::Int(2), Value::Int(3)]
            })
        );
    }

    #[test]
    fn test_size_string() {
        let result = ContainerFunction::Size
            .execute(&[Value::String("hello".to_string())])
            .expect("size function should succeed");
        assert_eq!(result, Value::Int(5));
    }

    #[test]
    fn test_size_list() {
        let list = Value::list(List {
            values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
        });
        let result = ContainerFunction::Size
            .execute(&[list])
            .expect("size function should succeed");
        assert_eq!(result, Value::Int(3));
    }

    #[test]
    fn test_range_basic() {
        let result = ContainerFunction::Range
            .execute(&[Value::Int(1), Value::Int(5)])
            .expect("range function should succeed");
        assert_eq!(
            result,
            Value::list(List {
                values: vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(3),
                    Value::Int(4),
                    Value::Int(5)
                ]
            })
        );
    }

    #[test]
    fn test_range_with_step() {
        let result = ContainerFunction::Range
            .execute(&[Value::Int(0), Value::Int(10), Value::Int(2)])
            .expect("range function should succeed");
        assert_eq!(
            result,
            Value::list(List {
                values: vec![
                    Value::Int(0),
                    Value::Int(2),
                    Value::Int(4),
                    Value::Int(6),
                    Value::Int(8),
                    Value::Int(10)
                ]
            })
        );
    }

    #[test]
    fn test_null_handling() {
        let null_value = Value::Null(NullType::Null);

        assert_eq!(
            ContainerFunction::Head
                .execute(std::slice::from_ref(&null_value))
                .expect("head should handle NULL"),
            Value::Null(NullType::Null)
        );
        assert_eq!(
            ContainerFunction::Last
                .execute(std::slice::from_ref(&null_value))
                .expect("last should handle NULL"),
            Value::Null(NullType::Null)
        );
        assert_eq!(
            ContainerFunction::Tail
                .execute(std::slice::from_ref(&null_value))
                .expect("tail should handle NULL"),
            Value::Null(NullType::Null)
        );
        assert_eq!(
            ContainerFunction::Size
                .execute(std::slice::from_ref(&null_value))
                .expect("size should handle NULL"),
            Value::Null(NullType::Null)
        );
    }
}
