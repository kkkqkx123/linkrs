//! Window function implementations
//!
//! Provides window-style functions: ROW_NUMBER, RANK, DENSE_RANK, LEAD, LAG, FIRST_VALUE, LAST_VALUE.
//! These functions currently operate as row-level functions.
//! Full OVER clause support (PARTITION BY, ORDER BY, frame specification) will be added in a future phase.

use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::expression::ExpressionError;

define_function_enum! {
    /// Window function enumeration
    pub enum WindowFunction {
        RowNumber => {
            name: "row_number",
            arity: 0,
            variadic: false,
            description: "Returns the sequential number of a row within a result set",
            handler: execute_row_number
        },
        Rank => {
            name: "rank",
            arity: 0,
            variadic: false,
            description: "Returns the rank of each row within the partition",
            handler: execute_rank
        },
        DenseRank => {
            name: "dense_rank",
            arity: 0,
            variadic: false,
            description: "Returns the rank of each row without gaps",
            handler: execute_dense_rank
        },
        Lead => {
            name: "lead",
            arity: 2,
            variadic: false,
            description: "Returns the value of a column from the next row",
            handler: execute_lead
        },
        Lag => {
            name: "lag",
            arity: 2,
            variadic: false,
            description: "Returns the value of a column from the previous row",
            handler: execute_lag
        },
        FirstValue => {
            name: "first_value",
            arity: 1,
            variadic: false,
            description: "Returns the first value in an ordered set of values",
            handler: execute_first_value
        },
        LastValue => {
            name: "last_value",
            arity: 1,
            variadic: false,
            description: "Returns the last value in an ordered set of values",
            handler: execute_last_value
        },
        NthValue => {
            name: "nth_value",
            arity: 2,
            variadic: false,
            description: "Returns the Nth value in an ordered set of values",
            handler: execute_nth_value
        },
        Ntile => {
            name: "ntile",
            arity: 1,
            variadic: false,
            description: "Divides rows into N buckets",
            handler: execute_ntile
        },
    }
}

fn execute_row_number(args: &[Value]) -> Result<Value, ExpressionError> {
    if !args.is_empty() {
        return Err(ExpressionError::type_error(
            "row_number() takes no arguments",
        ));
    }
    Ok(Value::BigInt(1))
}

fn execute_rank(args: &[Value]) -> Result<Value, ExpressionError> {
    if !args.is_empty() {
        return Err(ExpressionError::type_error("rank() takes no arguments"));
    }
    Ok(Value::BigInt(1))
}

fn execute_dense_rank(args: &[Value]) -> Result<Value, ExpressionError> {
    if !args.is_empty() {
        return Err(ExpressionError::type_error(
            "dense_rank() takes no arguments",
        ));
    }
    Ok(Value::BigInt(1))
}

fn execute_lead(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "lead(expr, offset) takes exactly 2 arguments",
        ));
    }
    let offset = match &args[1] {
        Value::Int(i) => *i as i64,
        Value::BigInt(i) => *i,
        _ => {
            return Err(ExpressionError::type_error(
                "lead offset must be an integer",
            ))
        }
    };
    if offset < 0 {
        return Err(ExpressionError::type_error(
            "lead offset must be non-negative",
        ));
    }
    Ok(Value::Null(NullType::Null))
}

fn execute_lag(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "lag(expr, offset) takes exactly 2 arguments",
        ));
    }
    let offset = match &args[1] {
        Value::Int(i) => *i as i64,
        Value::BigInt(i) => *i,
        _ => {
            return Err(ExpressionError::type_error(
                "lag offset must be an integer",
            ))
        }
    };
    if offset < 0 {
        return Err(ExpressionError::type_error(
            "lag offset must be non-negative",
        ));
    }
    Ok(Value::Null(NullType::Null))
}

fn execute_first_value(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "first_value(expr) takes exactly 1 argument",
        ));
    }
    Ok(args[0].clone())
}

fn execute_last_value(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "last_value(expr) takes exactly 1 argument",
        ));
    }
    Ok(args[0].clone())
}

fn execute_nth_value(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "nth_value(expr, n) takes exactly 2 arguments",
        ));
    }
    let n = match &args[1] {
        Value::Int(i) => *i as i64,
        Value::BigInt(i) => *i,
        _ => {
            return Err(ExpressionError::type_error(
                "nth_value n must be an integer",
            ))
        }
    };
    if n < 1 {
        return Err(ExpressionError::type_error(
            "nth_value n must be positive",
        ));
    }
    Ok(args[0].clone())
}

fn execute_ntile(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "ntile(n) takes exactly 1 argument",
        ));
    }
    let n = match &args[0] {
        Value::Int(i) => *i as i64,
        Value::BigInt(i) => *i,
        _ => {
            return Err(ExpressionError::type_error(
                "ntile n must be an integer",
            ))
        }
    };
    if n < 1 {
        return Err(ExpressionError::type_error(
            "ntile n must be positive",
        ));
    }
    Ok(Value::BigInt(1))
}
