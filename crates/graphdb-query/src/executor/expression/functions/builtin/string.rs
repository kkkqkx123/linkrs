//! Implementation of string functions

use crate::executor::expression::{ExpressionError, ExpressionErrorType};
use graphdb_core::value::list::List;
use graphdb_core::value::NullType;
use graphdb_core::Value;

define_function_enum! {
    /// String function enumeration
    pub enum StringFunction {
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
        Levenshtein => {
            name: "levenshtein",
            arity: 2,
            variadic: false,
            description: "Calculate Levenshtein edit distance between two strings",
            handler: execute_levenshtein
        },
        SplitPart => {
            name: "split_part",
            arity: 3,
            variadic: false,
            description: "Split string by delimiter and return Nth part",
            handler: execute_split_part
        },
        Initcap => {
            name: "initcap",
            arity: 1,
            variadic: false,
            description: "Capitalize first letter of each word",
            handler: execute_initcap
        },
        Repeat => {
            name: "repeat",
            arity: 2,
            variadic: false,
            description: "Repeat string N times",
            handler: execute_repeat
        },
        Position => {
            name: "position",
            arity: 2,
            variadic: false,
            description: "Find position of substring",
            handler: execute_position
        },
        Left => {
            name: "left",
            arity: 2,
            variadic: false,
            description: "Get first N characters of string",
            handler: execute_left
        },
        Right => {
            name: "right",
            arity: 2,
            variadic: false,
            description: "Get last N characters of string",
            handler: execute_right
        },
        StringInsert => {
            name: "insert",
            arity: 4,
            variadic: false,
            description: "Insert substring at specified position",
            handler: execute_string_insert
        },
        Translate => {
            name: "translate",
            arity: 3,
            variadic: false,
            description: "Replace characters in string using mapping",
            handler: execute_translate
        },
        Format => {
            name: "format",
            arity: 2,
            variadic: true,
            description: "Format string with placeholders",
            handler: execute_format
        },
        StringSplit => {
            name: "string_split",
            arity: 2,
            variadic: false,
            description: "Split string into substrings by delimiter",
            handler: execute_string_split
        },
        Reverse => {
            name: "reverse_string",
            arity: 1,
            variadic: false,
            description: "Reverse a string",
            handler: execute_reverse
        },
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
                Ok(Value::string(String::new()))
            } else {
                let end = (start + len).min(s.len());
                Ok(Value::string(&s[start..end]))
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
    Ok(Value::string(result))
}

fn execute_replace(args: &[Value]) -> Result<Value, ExpressionError> {
    match (&args[0], &args[1]) {
        (Value::String(s), Value::String(from)) => Ok(Value::string(s.replace(from.as_str(), ""))),
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
    use graphdb_core::value::list::List;
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "The split function takes 2 arguments",
        ));
    }
    match (&args[0], &args[1]) {
        (Value::String(s), Value::String(delimiter)) => {
            let parts: Vec<Value> = s.split(delimiter.as_str()).map(Value::string).collect();
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
                Ok(Value::string(&s[..len]))
            } else {
                let pad_len = len - s.len();
                let mut result = String::new();
                while result.len() < pad_len {
                    result.push_str(pad);
                }
                result.truncate(pad_len);
                result.push_str(s);
                Ok(Value::string(result))
            }
        }
        (Value::Null(_), _, _) | (_, Value::Null(_), _) | (_, _, Value::Null(_)) => {
            Ok(Value::Null(NullType::Null))
        }
        _ => Err(ExpressionError::type_error(
            "The lpad function takes 3 arguments",
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
                Ok(Value::string(&s[..len]))
            } else {
                let pad_len = len - s.len();
                let mut result = s.clone();
                let mut pad_str = String::new();
                while pad_str.len() < pad_len {
                    pad_str.push_str(pad);
                }
                pad_str.truncate(pad_len);
                result.push_str(&pad_str);
                Ok(Value::string(result))
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
        Value::String(s) => s.to_string(),
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
    Ok(Value::string(result))
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

fn execute_levenshtein(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "levenshtein requires 2 arguments",
        ));
    }
    match (&args[0], &args[1]) {
        (Value::String(s1), Value::String(s2)) => {
            let dist = levenshtein_distance(s1, s2);
            Ok(Value::Int(dist as i32))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "levenshtein requires string arguments",
        )),
    }
}

fn execute_split_part(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 3 {
        return Err(ExpressionError::type_error(
            "split_part requires 3 arguments",
        ));
    }
    match (&args[0], &args[1], &args[2]) {
        (Value::String(s), Value::String(delimiter), Value::Int(n)) => {
            if *n <= 0 {
                return Err(ExpressionError::type_error(
                    "split_part index must be positive",
                ));
            }
            let parts: Vec<&str> = s.split(delimiter.as_str()).collect();
            let idx = (*n - 1) as usize;
            if idx < parts.len() {
                Ok(Value::string(parts[idx]))
            } else {
                Ok(Value::string(String::new()))
            }
        }
        (Value::Null(_), _, _) | (_, Value::Null(_), _) | (_, _, Value::Null(_)) => {
            Ok(Value::Null(NullType::Null))
        }
        _ => Err(ExpressionError::type_error(
            "split_part requires string, string, and integer arguments",
        )),
    }
}

fn execute_initcap(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::String(s) => {
            let result: String = s
                .split_whitespace()
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        None => String::new(),
                        Some(c) => c.to_uppercase().to_string() + &chars.as_str().to_lowercase(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            Ok(Value::string(result))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "initcap requires a string type",
        )),
    }
}

fn execute_repeat(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error("repeat requires 2 arguments"));
    }
    match (&args[0], &args[1]) {
        (Value::String(s), Value::Int(n)) => {
            if *n < 0 {
                return Err(ExpressionError::type_error(
                    "repeat count must be non-negative",
                ));
            }
            Ok(Value::string(s.repeat(*n as usize)))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "repeat requires string and integer arguments",
        )),
    }
}

fn execute_position(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error("position requires 2 arguments"));
    }
    match (&args[0], &args[1]) {
        (Value::String(s), Value::String(sub)) => {
            if let Some(idx) = s.find(sub.as_str()) {
                Ok(Value::Int((idx + 1) as i32))
            } else {
                Ok(Value::Int(0))
            }
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "position requires string arguments",
        )),
    }
}

fn execute_left(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error("left requires 2 arguments"));
    }
    match (&args[0], &args[1]) {
        (Value::String(s), Value::Int(n)) => {
            let n = *n;
            if n >= s.chars().count() as i32 {
                Ok(Value::string(s.clone()))
            } else if n <= 0 {
                Ok(Value::string(String::new()))
            } else {
                let result: String = s.chars().take(n as usize).collect();
                Ok(Value::string(result))
            }
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "left requires string and integer arguments",
        )),
    }
}

fn execute_right(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error("right requires 2 arguments"));
    }
    match (&args[0], &args[1]) {
        (Value::String(s), Value::Int(n)) => {
            let n = *n;
            let char_count = s.chars().count();
            if n >= char_count as i32 {
                Ok(Value::string(s.clone()))
            } else if n <= 0 {
                Ok(Value::string(String::new()))
            } else {
                let result: String = s.chars().skip(char_count - n as usize).collect();
                Ok(Value::string(result))
            }
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "right requires string and integer arguments",
        )),
    }
}

fn execute_string_insert(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 4 {
        return Err(ExpressionError::type_error("insert requires 4 arguments"));
    }
    match (&args[0], &args[1], &args[2], &args[3]) {
        (Value::String(s), Value::Int(pos), Value::Int(len), Value::String(newsub)) => {
            let pos = *pos as usize;
            let len = *len as usize;
            if pos > s.len() {
                return Ok(Value::string(s.clone()));
            }
            let result = format!("{}{}{}", &s[..pos], newsub, &s[(pos + len).min(s.len())..]);
            Ok(Value::string(result))
        }
        (Value::Null(_), _, _, _)
        | (_, Value::Null(_), _, _)
        | (_, _, Value::Null(_), _)
        | (_, _, _, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "insert requires string, int, int, string arguments",
        )),
    }
}

fn execute_translate(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 3 {
        return Err(ExpressionError::type_error(
            "translate requires 3 arguments",
        ));
    }
    match (&args[0], &args[1], &args[2]) {
        (Value::String(s), Value::String(from), Value::String(to)) => {
            let result: String = s
                .chars()
                .map(|c| {
                    if let Some(pos) = from.chars().position(|fc| fc == c) {
                        to.chars().nth(pos).unwrap_or(c)
                    } else {
                        c
                    }
                })
                .collect();
            Ok(Value::string(result))
        }
        (Value::Null(_), _, _) | (_, Value::Null(_), _) | (_, _, Value::Null(_)) => {
            Ok(Value::Null(NullType::Null))
        }
        _ => Err(ExpressionError::type_error(
            "translate requires string, string, string arguments",
        )),
    }
}

fn execute_format(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() < 2 {
        return Err(ExpressionError::type_error(
            "format requires at least 2 arguments (format string + values)",
        ));
    }
    let format_str = match &args[0] {
        Value::String(s) => s.to_string(),
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "format first argument must be a string",
            ))
        }
    };
    let mut result = format_str;
    for (i, arg) in args[1..].iter().enumerate() {
        let placeholder = format!("{{{}}}", i);
        let replacement = match arg {
            Value::Null(_) => "NULL".to_string(),
            Value::String(s) => s.to_string(),
            other => format!("{}", other),
        };
        result = result.replace(&placeholder, &replacement);
    }
    Ok(Value::string(result))
}

fn execute_string_split(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "string_split requires 2 arguments",
        ));
    }
    match (&args[0], &args[1]) {
        (Value::String(s), Value::String(delimiter)) => {
            let parts: Vec<Value> = s.split(delimiter.as_str()).map(Value::string).collect();
            Ok(Value::list(List { values: parts }))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "string_split requires string and string arguments",
        )),
    }
}

fn execute_reverse(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("reverse requires 1 argument"));
    }
    match &args[0] {
        Value::String(s) => Ok(Value::string(s.chars().rev().collect::<String>())),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("reverse requires a string")),
    }
}

fn levenshtein_distance(s1: &str, s2: &str) -> usize {
    let len1 = s1.chars().count();
    let len2 = s2.chars().count();
    if len1 == 0 {
        return len2;
    }
    if len2 == 0 {
        return len1;
    }

    let mut prev_row: Vec<usize> = (0..=len2).collect();
    let mut curr_row = vec![0usize; len2 + 1];

    for (i, c1) in s1.chars().enumerate() {
        curr_row[0] = i + 1;
        for (j, c2) in s2.chars().enumerate() {
            let cost = if c1 == c2 { 0 } else { 1 };
            curr_row[j + 1] = (prev_row[j + 1] + 1)
                .min(curr_row[j] + 1)
                .min(prev_row[j] + cost);
        }
        std::mem::swap(&mut prev_row, &mut curr_row);
    }

    prev_row[len2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::expression::functions::FunctionRegistry;

    #[test]
    fn test_length() {
        let registry = FunctionRegistry::new();
        let result = registry
            .execute("length", &[Value::string("hello")])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Int(5));
    }

    #[test]
    fn test_upper() {
        let func = StringFunction::Upper;
        let result = func
            .execute(&[Value::string("hello")])
            .expect("Execution should succeed");
        assert_eq!(result, Value::string("HELLO"));
    }

    #[test]
    fn test_lower() {
        let func = StringFunction::Lower;
        let result = func
            .execute(&[Value::string("HELLO")])
            .expect("Execution should succeed");
        assert_eq!(result, Value::string("hello"));
    }

    #[test]
    fn test_trim() {
        let func = StringFunction::Trim;
        let result = func
            .execute(&[Value::string("  hello  ")])
            .expect("Execution should succeed");
        assert_eq!(result, Value::string("hello"));
    }

    #[test]
    fn test_substring() {
        let func = StringFunction::Substring;
        let result = func
            .execute(&[Value::string("hello"), Value::Int(1), Value::Int(3)])
            .expect("Execution should succeed");
        assert_eq!(result, Value::string("ell"));
    }

    #[test]
    fn test_concat() {
        let func = StringFunction::Concat;
        let result = func
            .execute(&[
                Value::string("hello"),
                Value::string(" "),
                Value::string("world"),
            ])
            .expect("Execution should succeed");
        assert_eq!(result, Value::string("hello world"));
    }

    #[test]
    fn test_contains() {
        let func = StringFunction::Contains;
        let result = func
            .execute(&[Value::string("hello world"), Value::string("world")])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_starts_with() {
        let func = StringFunction::StartsWith;
        let result = func
            .execute(&[Value::string("hello world"), Value::string("hello")])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_ends_with() {
        let func = StringFunction::EndsWith;
        let result = func
            .execute(&[Value::string("hello world"), Value::string("world")])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_null_handling() {
        let registry = FunctionRegistry::new();
        let result = registry
            .execute("length", &[Value::Null(NullType::Null)])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Null(NullType::Null));
    }

    #[test]
    fn test_string_insert() {
        let func = StringFunction::StringInsert;
        let result = func
            .execute(&[
                Value::string("Hello World"),
                Value::Int(5),
                Value::Int(0),
                Value::string(","),
            ])
            .expect("Execution should succeed");
        assert_eq!(result, Value::string("Hello, World"));
    }

    #[test]
    fn test_translate() {
        let func = StringFunction::Translate;
        let result = func
            .execute(&[
                Value::string("hello"),
                Value::string("ae"),
                Value::string("xy"),
            ])
            .expect("Execution should succeed");
        assert_eq!(result, Value::string("hyllo"));
    }

    #[test]
    fn test_format() {
        let func = StringFunction::Format;
        let result = func
            .execute(&[
                Value::string("Hello {0}, your score is {1}"),
                Value::string("Alice"),
                Value::Int(95),
            ])
            .expect("Execution should succeed");
        assert_eq!(result, Value::string("Hello Alice, your score is 95"));
    }

    #[test]
    fn test_string_split() {
        let func = StringFunction::StringSplit;
        let result = func
            .execute(&[Value::string("a,b,c"), Value::string(",")])
            .expect("Execution should succeed");
        assert_eq!(
            result,
            Value::list(List {
                values: vec![Value::string("a"), Value::string("b"), Value::string("c"),]
            })
        );
    }

    #[test]
    fn test_reverse() {
        let func = StringFunction::Reverse;
        let result = func
            .execute(&[Value::string("hello")])
            .expect("Execution should succeed");
        assert_eq!(result, Value::string("olleh"));
    }
}
