//! Implementation of string functions

use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::expression::{ExpressionError, ExpressionErrorType};

define_function_enum! {
    /// String function enumeration
    pub enum StringFunction {
        Length => {
            name: "length",
            arity: 1,
            variadic: false,
            description: "Calculate string length",
            handler: execute_length
        },
        Upper => {
            name: "upper",
            arity: 1,
            variadic: false,
            description: "convert to uppercase",
            handler: execute_upper
        },
        Lower => {
            name: "lower",
            arity: 1,
            variadic: false,
            description: "Convert to lowercase",
            handler: execute_lower
        },
        Trim => {
            name: "trim",
            arity: 1,
            variadic: false,
            description: "Remove header and footer",
            handler: execute_trim
        },
        Substring => {
            name: "substring",
            arity: 3,
            variadic: false,
            description: "Get substring",
            handler: execute_substring
        },
        Concat => {
            name: "concat",
            arity: 1,
            variadic: true,
            description: "connection string",
            handler: execute_concat
        },
        Replace => {
            name: "replace",
            arity: 2,
            variadic: false,
            description: "Replacement String",
            handler: execute_replace
        },
        Contains => {
            name: "contains",
            arity: 2,
            variadic: false,
            description: "Checks if it contains substrings",
            handler: execute_contains
        },
        StartsWith => {
            name: "starts_with",
            arity: 2,
            variadic: false,
            description: "Checks if it starts with the specified string",
            handler: execute_starts_with
        },
        EndsWith => {
            name: "ends_with",
            arity: 2,
            variadic: false,
            description: "Checks if the specified string ends",
            handler: execute_ends_with
        },
        Split => {
            name: "split",
            arity: 2,
            variadic: false,
            description: "Split String",
            handler: execute_split
        },
        Lpad => {
            name: "lpad",
            arity: 3,
            variadic: false,
            description: "Left padding string",
            handler: execute_lpad
        },
        Rpad => {
            name: "rpad",
            arity: 3,
            variadic: false,
            description: "Right Fill String",
            handler: execute_rpad
        },
        ConcatWs => {
            name: "concat_ws",
            arity: 2,
            variadic: true,
            description: "Concatenate strings using delimiters",
            handler: execute_concat_ws
        },
        Strcasecmp => {
            name: "strcasecmp",
            arity: 2,
            variadic: false,
            description: "Compare strings case-insensitively",
            handler: execute_strcasecmp
        },
    }
}

fn execute_length(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::String(s) => Ok(Value::BigInt(s.len() as i64)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The length function requires a string type",
        )),
    }
}

define_unary_string_fn!(execute_upper, |s: &str| s.to_uppercase(), "upper");
define_unary_string_fn!(execute_lower, |s: &str| s.to_lowercase(), "lower");
define_unary_string_fn!(execute_trim, |s: &str| s.trim().to_string(), "trim");

fn execute_substring(args: &[Value]) -> Result<Value, ExpressionError> {
    match (&args[0], &args[1], &args[2]) {
        (Value::String(s), Value::Int(start), Value::Int(len)) => {
            let start = *start as usize;
            let len = *len as usize;
            if start >= s.len() {
                Ok(Value::String(String::new()))
            } else {
                let end = (start + len).min(s.len());
                Ok(Value::String(s[start..end].to_string()))
            }
        }
        (Value::Null(_), _, _) | (_, Value::Null(_), _) | (_, _, Value::Null(_)) => {
            Ok(Value::Null(NullType::Null))
        }
        _ => Err(ExpressionError::type_error(
            "The substring function takes a string and two integers.",
        )),
    }
}

fn execute_concat(args: &[Value]) -> Result<Value, ExpressionError> {
    let mut result = String::new();
    for arg in args {
        match arg {
            Value::String(s) => result.push_str(s),
            Value::Null(_) => return Ok(Value::Null(NullType::Null)),
            _ => {
                return Err(ExpressionError::type_error(
                    "The concat function requires a string type",
                ))
            }
        }
    }
    Ok(Value::String(result))
}

fn execute_replace(args: &[Value]) -> Result<Value, ExpressionError> {
    match (&args[0], &args[1]) {
        (Value::String(s), Value::String(from)) => Ok(Value::String(s.replace(from, ""))),
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The replace function requires a string type",
        )),
    }
}

fn execute_contains(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::new(
            ExpressionErrorType::InvalidArgumentCount,
            "The contains function takes 2 arguments",
        ));
    }
    match (&args[0], &args[1]) {
        (Value::String(s), Value::String(sub)) => Ok(Value::Bool(s.contains(sub.as_str()))),
        (Value::List(list), Value::String(target)) => Ok(Value::Bool(
            list.values
                .iter()
                .any(|v| matches!(v, Value::String(s) if s == target)),
        )),
        (Value::List(list), Value::Int(target)) => Ok(Value::Bool(
            list.values
                .iter()
                .any(|v| matches!(v, Value::Int(i) if *i == *target)),
        )),
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The contains function requires a string or list type.",
        )),
    }
}

define_binary_string_bool_fn!(
    execute_starts_with,
    |s: &str, prefix: &str| s.starts_with(prefix),
    "starts_with"
);
define_binary_string_bool_fn!(
    execute_ends_with,
    |s: &str, suffix: &str| s.ends_with(suffix),
    "ends_with"
);

fn execute_split(args: &[Value]) -> Result<Value, ExpressionError> {
    use crate::core::value::list::List;
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "The split function takes 2 arguments",
        ));
    }
    match (&args[0], &args[1]) {
        (Value::String(s), Value::String(delimiter)) => {
            let parts: Vec<Value> = s
                .split(delimiter)
                .map(|p| Value::String(p.to_string()))
                .collect();
            Ok(Value::list(List { values: parts }))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("split requires string type")),
    }
}

fn execute_lpad(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 3 {
        return Err(ExpressionError::type_error(
            "The lpad function takes 3 arguments",
        ));
    }
    match (&args[0], &args[1], &args[2]) {
        (Value::String(s), Value::Int(len), Value::String(pad)) => {
            let len = *len as usize;
            if s.len() >= len {
                Ok(Value::String(s[..len].to_string()))
            } else {
                let pad_len = len - s.len();
                let mut result = String::new();
                while result.len() < pad_len {
                    result.push_str(pad);
                }
                result.truncate(pad_len);
                result.push_str(s);
                Ok(Value::String(result))
            }
        }
        (Value::Null(_), _, _) | (_, Value::Null(_), _) | (_, _, Value::Null(_)) => {
            Ok(Value::Null(NullType::Null))
        }
        _ => Err(ExpressionError::type_error(
            "The lpad function takes string, integer, and string arguments",
        )),
    }
}

fn execute_rpad(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 3 {
        return Err(ExpressionError::type_error(
            "The rpad function takes 3 arguments",
        ));
    }
    match (&args[0], &args[1], &args[2]) {
        (Value::String(s), Value::Int(len), Value::String(pad)) => {
            let len = *len as usize;
            if s.len() >= len {
                Ok(Value::String(s[..len].to_string()))
            } else {
                let pad_len = len - s.len();
                let mut result = s.clone();
                let mut pad_str = String::new();
                while pad_str.len() < pad_len {
                    pad_str.push_str(pad);
                }
                pad_str.truncate(pad_len);
                result.push_str(&pad_str);
                Ok(Value::String(result))
            }
        }
        (Value::Null(_), _, _) | (_, Value::Null(_), _) | (_, _, Value::Null(_)) => {
            Ok(Value::Null(NullType::Null))
        }
        _ => Err(ExpressionError::type_error(
            "The rpad function takes string, integer, and string arguments",
        )),
    }
}

fn execute_concat_ws(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() < 2 {
        return Err(ExpressionError::type_error(
            "The concat_ws function takes at least 2 arguments",
        ));
    }
    let separator = match &args[0] {
        Value::String(s) => s.clone(),
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "The concat_ws function requires a string type for the first argument",
            ))
        }
    };
    let mut result = String::new();
    for (i, arg) in args[1..].iter().enumerate() {
        match arg {
            Value::String(s) => {
                if i > 0 {
                    result.push_str(&separator);
                }
                result.push_str(s);
            }
            Value::Null(_) => return Ok(Value::Null(NullType::Null)),
            _ => {
                return Err(ExpressionError::type_error(
                    "The concat_ws function requires the string type",
                ))
            }
        }
    }
    Ok(Value::String(result))
}

fn execute_strcasecmp(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "The strcasecmp function takes 2 arguments",
        ));
    }
    match (&args[0], &args[1]) {
        (Value::String(a), Value::String(b)) => {
            let cmp = a.to_lowercase().cmp(&b.to_lowercase());
            Ok(Value::Int(match cmp {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            }))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The strcasecmp function requires the string type",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_length() {
        let func = StringFunction::Length;
        let result = func
            .execute(&[Value::String("hello".to_string())])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Int(5));
    }

    #[test]
    fn test_upper() {
        let func = StringFunction::Upper;
        let result = func
            .execute(&[Value::String("hello".to_string())])
            .expect("Execution should succeed");
        assert_eq!(result, Value::String("HELLO".to_string()));
    }

    #[test]
    fn test_lower() {
        let func = StringFunction::Lower;
        let result = func
            .execute(&[Value::String("HELLO".to_string())])
            .expect("Execution should succeed");
        assert_eq!(result, Value::String("hello".to_string()));
    }

    #[test]
    fn test_trim() {
        let func = StringFunction::Trim;
        let result = func
            .execute(&[Value::String("  hello  ".to_string())])
            .expect("Execution should succeed");
        assert_eq!(result, Value::String("hello".to_string()));
    }

    #[test]
    fn test_substring() {
        let func = StringFunction::Substring;
        let result = func
            .execute(&[
                Value::String("hello".to_string()),
                Value::Int(1),
                Value::Int(3),
            ])
            .expect("Execution should succeed");
        assert_eq!(result, Value::String("ell".to_string()));
    }

    #[test]
    fn test_concat() {
        let func = StringFunction::Concat;
        let result = func
            .execute(&[
                Value::String("hello".to_string()),
                Value::String(" ".to_string()),
                Value::String("world".to_string()),
            ])
            .expect("Execution should succeed");
        assert_eq!(result, Value::String("hello world".to_string()));
    }

    #[test]
    fn test_contains() {
        let func = StringFunction::Contains;
        let result = func
            .execute(&[
                Value::String("hello world".to_string()),
                Value::String("world".to_string()),
            ])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_starts_with() {
        let func = StringFunction::StartsWith;
        let result = func
            .execute(&[
                Value::String("hello world".to_string()),
                Value::String("hello".to_string()),
            ])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_ends_with() {
        let func = StringFunction::EndsWith;
        let result = func
            .execute(&[
                Value::String("hello world".to_string()),
                Value::String("world".to_string()),
            ])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_null_handling() {
        let func = StringFunction::Length;
        let result = func
            .execute(&[Value::Null(NullType::Null)])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Null(NullType::Null));
    }
}
