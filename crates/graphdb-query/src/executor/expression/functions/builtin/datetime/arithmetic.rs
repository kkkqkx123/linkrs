use crate::executor::expression::ExpressionError;
use chrono::{Datelike, Timelike};
use graphdb_core::value::{DateTimeValue, DateValue, NullType};
use graphdb_core::Value;

pub(crate) fn execute_date_add(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error("date_add requires 2 arguments"));
    }
    let amount = match &args[1] {
        Value::Int(i) => *i as i64,
        Value::BigInt(i) => *i,
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "date_add amount must be an integer",
            ))
        }
    };
    match &args[0] {
        Value::Date(d) => {
            let naive = chrono::NaiveDate::from_ymd_opt(d.year, d.month, d.day)
                .ok_or_else(|| ExpressionError::type_error("Invalid date"))?;
            let result = naive + chrono::TimeDelta::days(amount);
            Ok(Value::Date(DateValue {
                year: result.year(),
                month: result.month(),
                day: result.day(),
            }))
        }
        Value::DateTime(dt) => {
            let naive = chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(dt.year, dt.month, dt.day)
                    .ok_or_else(|| ExpressionError::type_error("Invalid date"))?,
                chrono::NaiveTime::from_hms_micro_opt(dt.hour, dt.minute, dt.sec, dt.microsec)
                    .ok_or_else(|| ExpressionError::type_error("Invalid time"))?,
            );
            let result = naive + chrono::TimeDelta::days(amount);
            Ok(Value::DateTime(DateTimeValue {
                year: result.year(),
                month: result.month(),
                day: result.day(),
                hour: result.hour(),
                minute: result.minute(),
                sec: result.second(),
                microsec: result.nanosecond() / 1000,
            }))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "date_add requires a date or datetime as first argument",
        )),
    }
}

pub(crate) fn execute_date_sub(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error("date_sub requires 2 arguments"));
    }
    let amount = match &args[1] {
        Value::Int(i) => *i as i64,
        Value::BigInt(i) => *i,
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "date_sub amount must be an integer",
            ))
        }
    };
    match &args[0] {
        Value::Date(d) => {
            let naive = chrono::NaiveDate::from_ymd_opt(d.year, d.month, d.day)
                .ok_or_else(|| ExpressionError::type_error("Invalid date"))?;
            let result = naive - chrono::TimeDelta::days(amount);
            Ok(Value::Date(DateValue {
                year: result.year(),
                month: result.month(),
                day: result.day(),
            }))
        }
        Value::DateTime(dt) => {
            let naive = chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(dt.year, dt.month, dt.day)
                    .ok_or_else(|| ExpressionError::type_error("Invalid date"))?,
                chrono::NaiveTime::from_hms_micro_opt(dt.hour, dt.minute, dt.sec, dt.microsec)
                    .ok_or_else(|| ExpressionError::type_error("Invalid time"))?,
            );
            let result = naive - chrono::TimeDelta::days(amount);
            Ok(Value::DateTime(DateTimeValue {
                year: result.year(),
                month: result.month(),
                day: result.day(),
                hour: result.hour(),
                minute: result.minute(),
                sec: result.second(),
                microsec: result.nanosecond() / 1000,
            }))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "date_sub requires a date or datetime as first argument",
        )),
    }
}

pub(crate) fn execute_date_diff(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "date_diff requires 2 arguments",
        ));
    }
    match (&args[0], &args[1]) {
        (Value::Date(d1), Value::Date(d2)) => {
            let n1 = chrono::NaiveDate::from_ymd_opt(d1.year, d1.month, d1.day)
                .ok_or_else(|| ExpressionError::type_error("Invalid date"))?;
            let n2 = chrono::NaiveDate::from_ymd_opt(d2.year, d2.month, d2.day)
                .ok_or_else(|| ExpressionError::type_error("Invalid date"))?;
            let diff = (n2 - n1).num_days();
            Ok(Value::BigInt(diff))
        }
        (Value::DateTime(dt1), Value::DateTime(dt2)) => {
            let n1 = chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(dt1.year, dt1.month, dt1.day)
                    .ok_or_else(|| ExpressionError::type_error("Invalid date"))?,
                chrono::NaiveTime::from_hms_micro_opt(dt1.hour, dt1.minute, dt1.sec, dt1.microsec)
                    .ok_or_else(|| ExpressionError::type_error("Invalid time"))?,
            );
            let n2 = chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(dt2.year, dt2.month, dt2.day)
                    .ok_or_else(|| ExpressionError::type_error("Invalid date"))?,
                chrono::NaiveTime::from_hms_micro_opt(dt2.hour, dt2.minute, dt2.sec, dt2.microsec)
                    .ok_or_else(|| ExpressionError::type_error("Invalid time"))?,
            );
            let diff = (n2 - n1).num_milliseconds();
            Ok(Value::BigInt(diff))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "date_diff requires two dates or two datetimes",
        )),
    }
}

pub(crate) fn execute_age(args: &[Value]) -> Result<Value, ExpressionError> {
    let now = chrono::Utc::now();
    match &args[0] {
        Value::Date(d) => {
            let naive = chrono::NaiveDate::from_ymd_opt(d.year, d.month, d.day)
                .ok_or_else(|| ExpressionError::type_error("Invalid date"))?;
            let target = naive
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| ExpressionError::type_error("Invalid time"))?;
            let target_dt = target.and_utc();
            let duration = now.signed_duration_since(target_dt);
            Ok(Value::BigInt(duration.num_days()))
        }
        Value::DateTime(dt) => {
            let naive = chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(dt.year, dt.month, dt.day)
                    .ok_or_else(|| ExpressionError::type_error("Invalid date"))?,
                chrono::NaiveTime::from_hms_micro_opt(dt.hour, dt.minute, dt.sec, dt.microsec)
                    .ok_or_else(|| ExpressionError::type_error("Invalid time"))?,
            );
            let target_dt = naive.and_utc();
            let duration = now.signed_duration_since(target_dt);
            Ok(Value::BigInt(duration.num_days()))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "age requires a date or datetime argument",
        )),
    }
}
