use crate::executor::expression::ExpressionError;
use graphdb_core::value::{DateTimeValue, DateValue, NullType};
use graphdb_core::Value;

pub(crate) fn execute_date_trunc(args: &[Value]) -> Result<Value, ExpressionError> {
    let precision = match &args[1] {
        Value::String(s) => s.as_str(),
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "date_trunc precision must be a string",
            ))
        }
    };
    match &args[0] {
        Value::Date(d) => match precision {
            "year" => Ok(Value::Date(DateValue {
                year: d.year,
                month: 1,
                day: 1,
            })),
            "month" => Ok(Value::Date(DateValue {
                year: d.year,
                month: d.month,
                day: 1,
            })),
            "day" => Ok(Value::Date(DateValue {
                year: d.year,
                month: d.month,
                day: d.day,
            })),
            _ => Err(ExpressionError::type_error(format!(
                "Invalid date_trunc precision: {}",
                precision
            ))),
        },
        Value::DateTime(dt) => match precision {
            "year" => Ok(Value::DateTime(DateTimeValue {
                year: dt.year,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                sec: 0,
                microsec: 0,
            })),
            "month" => Ok(Value::DateTime(DateTimeValue {
                year: dt.year,
                month: dt.month,
                day: 1,
                hour: 0,
                minute: 0,
                sec: 0,
                microsec: 0,
            })),
            "day" => Ok(Value::DateTime(DateTimeValue {
                year: dt.year,
                month: dt.month,
                day: dt.day,
                hour: 0,
                minute: 0,
                sec: 0,
                microsec: 0,
            })),
            "hour" => Ok(Value::DateTime(DateTimeValue {
                year: dt.year,
                month: dt.month,
                day: dt.day,
                hour: dt.hour,
                minute: 0,
                sec: 0,
                microsec: 0,
            })),
            "minute" => Ok(Value::DateTime(DateTimeValue {
                year: dt.year,
                month: dt.month,
                day: dt.day,
                hour: dt.hour,
                minute: dt.minute,
                sec: 0,
                microsec: 0,
            })),
            "second" => Ok(Value::DateTime(DateTimeValue {
                year: dt.year,
                month: dt.month,
                day: dt.day,
                hour: dt.hour,
                minute: dt.minute,
                sec: dt.sec,
                microsec: 0,
            })),
            _ => Err(ExpressionError::type_error(format!(
                "Invalid date_trunc precision: {}",
                precision
            ))),
        },
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "date_trunc requires a date or datetime as first argument",
        )),
    }
}

pub(crate) fn execute_last_day(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Date(d) => {
            let last_day = get_last_day(d.year, d.month);
            Ok(Value::Date(DateValue {
                year: d.year,
                month: d.month,
                day: last_day,
            }))
        }
        Value::DateTime(dt) => {
            let last_day = get_last_day(dt.year, dt.month);
            Ok(Value::DateTime(DateTimeValue {
                year: dt.year,
                month: dt.month,
                day: last_day,
                hour: dt.hour,
                minute: dt.minute,
                sec: dt.sec,
                microsec: dt.microsec,
            }))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "last_day requires a date or datetime argument",
        )),
    }
}

fn get_last_day(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}
