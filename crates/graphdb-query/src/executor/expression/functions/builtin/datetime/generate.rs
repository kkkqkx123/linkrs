use crate::executor::expression::ExpressionError;
use graphdb_core::value::NullType;
use graphdb_core::Value;

pub(crate) fn execute_generate_series(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() < 2 || args.len() > 3 {
        return Err(ExpressionError::type_error(
            "generate_series requires 2 or 3 arguments",
        ));
    }
    let start = match &args[0] {
        Value::BigInt(v) => *v,
        Value::Int(v) => *v as i64,
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "generate_series start must be an integer",
            ))
        }
    };
    let end = match &args[1] {
        Value::BigInt(v) => *v,
        Value::Int(v) => *v as i64,
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "generate_series end must be an integer",
            ))
        }
    };
    let step = if args.len() > 2 {
        match &args[2] {
            Value::BigInt(v) => *v,
            Value::Int(v) => *v as i64,
            Value::Null(_) => return Ok(Value::Null(NullType::Null)),
            _ => {
                return Err(ExpressionError::type_error(
                    "generate_series step must be an integer",
                ))
            }
        }
    } else {
        1
    };

    if step == 0 {
        return Err(ExpressionError::type_error(
            "generate_series step cannot be 0",
        ));
    }

    let mut result = Vec::new();
    if step > 0 {
        let mut i = start;
        while i <= end {
            result.push(Value::BigInt(i));
            i += step;
        }
    } else {
        let mut i = start;
        while i >= end {
            result.push(Value::BigInt(i));
            i += step;
        }
    }

    use graphdb_core::value::list::List;
    Ok(Value::list(List { values: result }))
}
