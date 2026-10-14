use crate::executor::expression::ExpressionError;
use linkrs_core::value::NullType;
use linkrs_core::Value;
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

    /// Execute against a sequence catalog manager.
    ///
    /// Arity and argument validation mirror [`Self::execute`]; a missing
    /// manager produces the same missing-context error so callers see one
    /// consistent message whether the function ran bare or unthreaded.
    pub fn execute_with_manager(
        &self,
        args: &[Value],
        manager: Option<&std::sync::Arc<linkrs_core::metadata::SequenceManager>>,
    ) -> Result<Value, ExpressionError> {
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

        let manager = manager.ok_or_else(|| {
            ExpressionError::function_error(format!(
                "{}('{}') requires execution context with SequenceManager; \
                 use within a query execution context",
                self.name(),
                name
            ))
        })?;
        let value = match self {
            Self::CurrVal => manager
                .current_value(name)
                .map_err(|e| ExpressionError::function_error(e.to_string()))?,
            Self::NextVal => manager
                .next_value(name)
                .map_err(|e| ExpressionError::function_error(e.to_string()))?,
        };
        Ok(Value::BigInt(value))
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

    #[test]
    fn test_execute_with_manager_next_and_curr() {
        use std::sync::Arc;

        let manager = Arc::new(linkrs_core::metadata::SequenceManager::new());
        manager
            .create_sequence("seq1".into(), 1, 2, 1, 100, false)
            .expect("create sequence");
        let name = Value::string("seq1");

        let curr = SequenceFunction::CurrVal
            .execute_with_manager(std::slice::from_ref(&name), Some(&manager))
            .expect("curr_val reads start");
        assert_eq!(curr, Value::BigInt(1));

        let next = SequenceFunction::NextVal
            .execute_with_manager(std::slice::from_ref(&name), Some(&manager))
            .expect("next_val steps");
        assert_eq!(next, Value::BigInt(3));

        let curr = SequenceFunction::CurrVal
            .execute_with_manager(std::slice::from_ref(&name), Some(&manager))
            .expect("curr_val follows next");
        assert_eq!(curr, Value::BigInt(3));
    }

    #[test]
    fn test_execute_with_manager_missing_sequence_errors() {
        use std::sync::Arc;

        let manager = Arc::new(linkrs_core::metadata::SequenceManager::new());
        let name = Value::string("ghost");
        let err = SequenceFunction::NextVal
            .execute_with_manager(std::slice::from_ref(&name), Some(&manager))
            .expect_err("missing sequence must fail");
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[test]
    fn test_execute_with_manager_without_manager_matches_bare_error() {
        let name = Value::string("seq1");
        let err = SequenceFunction::CurrVal
            .execute_with_manager(std::slice::from_ref(&name), None)
            .expect_err("missing manager must fail");
        assert!(
            err.to_string().contains("requires execution context"),
            "{err}"
        );
    }
}
