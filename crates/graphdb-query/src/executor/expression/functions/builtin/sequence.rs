use crate::executor::expression::ExpressionError;
use graphdb_core::value::NullType;
use graphdb_core::Value;

/// Sequence function enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceFunction {
    /// curr_val(seq_name) - get current value without incrementing
    CurrVal,
    /// next_val(seq_name) - get next value with atomic increment
    NextVal,
}

impl SequenceFunction {
    pub fn name(&self) -> &str {
        match self {
            Self::CurrVal => "curr_val",
            Self::NextVal => "next_val",
        }
    }

    pub fn arity(&self) -> usize {
        1
    }

    pub fn is_variadic(&self) -> bool {
        false
    }

    pub fn description(&self) -> &str {
        match self {
            Self::CurrVal => "Get the current value of a sequence without incrementing",
            Self::NextVal => "Get the next value of a sequence with atomic increment",
        }
    }

    pub fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        if !self.is_variadic() && args.len() != self.arity() {
            return Err(ExpressionError::invalid_arity(
                self.name(),
                self.arity(),
                args.len(),
            ));
        }

        let name = match &args[0] {
            Value::String(s) => s.as_str(),
            Value::Null(_) => return Ok(Value::Null(NullType::Null)),
            _ => {
                return Err(ExpressionError::type_error(format!(
                    "{} requires a string sequence name",
                    self.name()
                )))
            }
        };

        // Sequence functions require a SequenceManager from the execution context.
        // When called without context (e.g. during planning or constant folding),
        // return an error indicating the function needs execution context.
        Err(ExpressionError::function_error(format!(
            "{}('{}') requires execution context with SequenceManager; \
             use within a query execution context",
            self.name(),
            name
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_curr_val_null_input() {
        let result = SequenceFunction::CurrVal.execute(&[Value::Null(NullType::Null)]);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Value::Null(NullType::Null)));
    }

    #[test]
    fn test_next_val_null_input() {
        let result = SequenceFunction::NextVal.execute(&[Value::Null(NullType::Null)]);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Value::Null(NullType::Null)));
    }

    #[test]
    fn test_curr_val_wrong_arg_count() {
        let result = SequenceFunction::CurrVal.execute(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_next_val_wrong_arg_type() {
        let result = SequenceFunction::NextVal.execute(&[Value::Int(42)]);
        assert!(result.is_err());
    }

    #[test]
    fn test_curr_val_needs_context() {
        let result = SequenceFunction::CurrVal.execute(&[Value::string("seq1")]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("requires execution context"));
    }

    #[test]
    fn test_next_val_needs_context() {
        let result = SequenceFunction::NextVal.execute(&[Value::string("seq1")]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("requires execution context"));
    }
}
