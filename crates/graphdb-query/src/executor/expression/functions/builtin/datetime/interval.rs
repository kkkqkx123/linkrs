use crate::executor::expression::ExpressionError;
use graphdb_core::value::NullType;
use graphdb_core::Value;

macro_rules! define_interval_fn {
    ($name:ident, $factor:expr, $desc:expr) => {
        pub(crate) fn $name(args: &[Value]) -> Result<Value, ExpressionError> {
            match &args[0] {
                Value::SmallInt(i) => Ok(Value::BigInt(*i as i64 * $factor)),
                Value::Int(i) => Ok(Value::BigInt(*i as i64 * $factor)),
                Value::BigInt(i) => Ok(Value::BigInt(*i * $factor)),
                Value::Float(f) => Ok(Value::Double(*f as f64 * $factor as f64)),
                Value::Double(f) => Ok(Value::Double(*f * $factor as f64)),
                Value::Null(_) => Ok(Value::Null(NullType::Null)),
                _ => Err(ExpressionError::type_error(concat!(
                    $desc,
                    " requires a numeric type"
                ))),
            }
        }
    };
}

define_interval_fn!(execute_to_years, 365_i64 * 24 * 60 * 60 * 1000, "to_years");
define_interval_fn!(execute_to_months, 30_i64 * 24 * 60 * 60 * 1000, "to_months");
define_interval_fn!(execute_to_days, 24_i64 * 60 * 60 * 1000, "to_days");
define_interval_fn!(execute_to_hours, 60_i64 * 60 * 1000, "to_hours");
define_interval_fn!(execute_to_minutes, 60_i64 * 1000, "to_minutes");
define_interval_fn!(execute_to_seconds, 1000_i64, "to_seconds");
define_interval_fn!(execute_to_milliseconds, 1_i64, "to_milliseconds");
define_interval_fn!(execute_to_microseconds, 1_i64 / 1000, "to_microseconds");
