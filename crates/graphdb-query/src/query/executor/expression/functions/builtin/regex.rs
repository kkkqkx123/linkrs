//! Implementation of regular expression functions

use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::expression::{ExpressionError, ExpressionErrorType};

/// Enumeration of regular expression functions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegexFunction {
    RegexMatch,
    RegexReplace,
    RegexFind,
}

impl RegexFunction {
    pub fn name(&self) -> &str {
        match self {
            RegexFunction::RegexMatch => "regex_match",
            RegexFunction::RegexReplace => "regex_replace",
            RegexFunction::RegexFind => "regex_find",
        }
    }

    pub fn arity(&self) -> usize {
        match self {
            RegexFunction::RegexMatch => 2,
            RegexFunction::RegexReplace => 3,
            RegexFunction::RegexFind => 2,
        }
    }

    pub fn is_variadic(&self) -> bool {
        false
    }

    pub fn description(&self) -> &str {
        match self {
            RegexFunction::RegexMatch => "regular expression matching (math.)",
            RegexFunction::RegexReplace => "regular expression substitution",
            RegexFunction::RegexFind => "regular expression lookup (computing)",
        }
    }

    pub fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        match self {
            RegexFunction::RegexMatch => match (&args[0], &args[1]) {
                (Value::String(s), Value::String(pattern)) => {
                    let regex = regex::Regex::new(pattern).map_err(|_| {
                        ExpressionError::new(
                            ExpressionErrorType::InvalidOperation,
                            format!("Invalid regular expression: {}", pattern),
                        )
                    })?;
                    Ok(Value::Bool(regex.is_match(s)))
                }
                (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
                _ => Err(ExpressionError::type_error(
                    "The regex_match function requires the string type",
                )),
            },
            RegexFunction::RegexReplace => match (&args[0], &args[1], &args[2]) {
                (Value::String(s), Value::String(pattern), Value::String(replacement)) => {
                    let regex = regex::Regex::new(pattern).map_err(|_| {
                        ExpressionError::new(
                            ExpressionErrorType::InvalidOperation,
                            format!("Invalid regular expression: {}", pattern),
                        )
                    })?;
                    Ok(Value::String(
                        regex.replace_all(s, replacement.as_str()).to_string(),
                    ))
                }
                (Value::Null(_), _, _) | (_, Value::Null(_), _) | (_, _, Value::Null(_)) => {
                    Ok(Value::Null(NullType::Null))
                }
                _ => Err(ExpressionError::type_error(
                    "The regex_replace function requires the string type",
                )),
            },
            RegexFunction::RegexFind => match (&args[0], &args[1]) {
                (Value::String(s), Value::String(pattern)) => {
                    let regex = regex::Regex::new(pattern).map_err(|_| {
                        ExpressionError::new(
                            ExpressionErrorType::InvalidOperation,
                            format!("Invalid regular expression: {}", pattern),
                        )
                    })?;
                    if let Some(matched) = regex.find(s) {
                        Ok(Value::String(matched.as_str().to_string()))
                    } else {
                        Ok(Value::Null(NullType::Null))
                    }
                }
                (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
                _ => Err(ExpressionError::type_error(
                    "The regex_find function requires the string type",
                )),
            },
        }
    }

    /// Execute a function (with caching)
    ///
    /// The caching function has been removed; the `execute` method is called directly.
    pub fn execute_with_cache(
        &self,
        args: &[Value],
        _cache: &mut (),
    ) -> Result<Value, ExpressionError> {
        self.execute(args)
    }
}
