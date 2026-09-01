//! Implementation of regular expression functions

use crate::executor::expression::{ExpressionError, ExpressionErrorType};
use graphdb_core::value::NullType;
use graphdb_core::Value;

/// Enumeration of regular expression functions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegexFunction {
    RegexMatch,
    RegexReplace,
    RegexFind,
    RegexpFullMatch,
    RegexpExtract,
    RegexpExtractAll,
    RegexpSplitToArray,
}

impl RegexFunction {
    pub fn name(&self) -> &str {
        match self {
            RegexFunction::RegexMatch => "regex_match",
            RegexFunction::RegexReplace => "regex_replace",
            RegexFunction::RegexFind => "regex_find",
            RegexFunction::RegexpFullMatch => "regexp_full_match",
            RegexFunction::RegexpExtract => "regexp_extract",
            RegexFunction::RegexpExtractAll => "regexp_extract_all",
            RegexFunction::RegexpSplitToArray => "regexp_split_to_array",
        }
    }

    pub fn arity(&self) -> usize {
        match self {
            RegexFunction::RegexMatch => 2,
            RegexFunction::RegexReplace => 3,
            RegexFunction::RegexFind => 2,
            RegexFunction::RegexpFullMatch => 2,
            RegexFunction::RegexpExtract => 2,
            RegexFunction::RegexpExtractAll => 2,
            RegexFunction::RegexpSplitToArray => 2,
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
            RegexFunction::RegexpFullMatch => "Check if string fully matches regex pattern",
            RegexFunction::RegexpExtract => "Extract first match of regex pattern",
            RegexFunction::RegexpExtractAll => "Extract all matches of regex pattern",
            RegexFunction::RegexpSplitToArray => "Split string by regex pattern into array",
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
                    Ok(Value::string(regex.replace_all(s, replacement.as_str())))
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
                        Ok(Value::string(matched.as_str()))
                    } else {
                        Ok(Value::Null(NullType::Null))
                    }
                }
                (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
                _ => Err(ExpressionError::type_error(
                    "The regex_find function requires the string type",
                )),
            },
            RegexFunction::RegexpFullMatch => match (&args[0], &args[1]) {
                (Value::String(s), Value::String(pattern)) => {
                    let regex = regex::Regex::new(pattern).map_err(|_| {
                        ExpressionError::new(
                            ExpressionErrorType::InvalidOperation,
                            format!("Invalid regular expression: {}", pattern),
                        )
                    })?;
                    Ok(Value::Bool(regex.is_match(s) && regex.find(s).map_or(false, |m| m.as_str() == s)))
                }
                (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
                _ => Err(ExpressionError::type_error(
                    "regexp_full_match requires string arguments",
                )),
            },
            RegexFunction::RegexpExtract => match (&args[0], &args[1]) {
                (Value::String(s), Value::String(pattern)) => {
                    let regex = regex::Regex::new(pattern).map_err(|_| {
                        ExpressionError::new(
                            ExpressionErrorType::InvalidOperation,
                            format!("Invalid regular expression: {}", pattern),
                        )
                    })?;
                    if let Some(matched) = regex.find(s) {
                        Ok(Value::string(matched.as_str()))
                    } else {
                        Ok(Value::Null(NullType::Null))
                    }
                }
                (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
                _ => Err(ExpressionError::type_error(
                    "regexp_extract requires string arguments",
                )),
            },
            RegexFunction::RegexpExtractAll => match (&args[0], &args[1]) {
                (Value::String(s), Value::String(pattern)) => {
                    let regex = regex::Regex::new(pattern).map_err(|_| {
                        ExpressionError::new(
                            ExpressionErrorType::InvalidOperation,
                            format!("Invalid regular expression: {}", pattern),
                        )
                    })?;
                    let matches: Vec<Value> = regex.find_iter(s).map(|m| Value::string(m.as_str())).collect();
                    Ok(Value::list(graphdb_core::value::list::List { values: matches }))
                }
                (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
                _ => Err(ExpressionError::type_error(
                    "regexp_extract_all requires string arguments",
                )),
            },
            RegexFunction::RegexpSplitToArray => match (&args[0], &args[1]) {
                (Value::String(s), Value::String(pattern)) => {
                    let regex = regex::Regex::new(pattern).map_err(|_| {
                        ExpressionError::new(
                            ExpressionErrorType::InvalidOperation,
                            format!("Invalid regular expression: {}", pattern),
                        )
                    })?;
                    let parts: Vec<Value> = regex.split(s).map(Value::string).collect();
                    Ok(Value::list(graphdb_core::value::list::List { values: parts }))
                }
                (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
                _ => Err(ExpressionError::type_error(
                    "regexp_split_to_array requires string arguments",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_regexp_full_match() {
        let func = RegexFunction::RegexpFullMatch;
        let result = func
            .execute(&[Value::string("hello123"), Value::string(r"^[a-z]+\d+$")])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_regexp_full_match_no_match() {
        let func = RegexFunction::RegexpFullMatch;
        let result = func
            .execute(&[Value::string("hello world"), Value::string(r"^\d+$")])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_regexp_extract() {
        let func = RegexFunction::RegexpExtract;
        let result = func
            .execute(&[Value::string("abc123def"), Value::string(r"\d+")])
            .expect("Execution should succeed");
        assert_eq!(result, Value::string("123"));
    }

    #[test]
    fn test_regexp_extract_no_match() {
        let func = RegexFunction::RegexpExtract;
        let result = func
            .execute(&[Value::string("abc"), Value::string(r"\d+")])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Null(NullType::Null));
    }

    #[test]
    fn test_regexp_extract_all() {
        let func = RegexFunction::RegexpExtractAll;
        let result = func
            .execute(&[Value::string("abc123def456"), Value::string(r"\d+")])
            .expect("Execution should succeed");
        assert_eq!(
            result,
            Value::list(graphdb_core::value::list::List {
                values: vec![Value::string("123"), Value::string("456")]
            })
        );
    }

    #[test]
    fn test_regexp_split_to_array() {
        let func = RegexFunction::RegexpSplitToArray;
        let result = func
            .execute(&[Value::string("abc123def456"), Value::string(r"\d+")])
            .expect("Execution should succeed");
        assert_eq!(
            result,
            Value::list(graphdb_core::value::list::List {
                values: vec![Value::string("abc"), Value::string("def"), Value::string("")]
            })
        );
    }
}
