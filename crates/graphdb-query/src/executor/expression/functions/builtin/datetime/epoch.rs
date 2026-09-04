use crate::executor::expression::ExpressionError;
use chrono::{Datelike, Timelike};
use graphdb_core::value::{DateTimeValue, NullType};
use graphdb_core::Value;

pub(crate) fn execute_timestamp(args: &[Value]) -> Result<Value, ExpressionError> {
    use std::time::{SystemTime, UNIX_EPOCH};

    if args.is_empty() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time error")
            .as_millis();
        Ok(Value::BigInt(now as i64))
    } else {
        match &args[0] {
            Value::DateTime(dt) => {
                let naive_dt = chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(dt.year, dt.month, dt.day)
                        .ok_or_else(|| ExpressionError::type_error("Date of invalidity"))?,
                    chrono::NaiveTime::from_hms_micro_opt(dt.hour, dt.minute, dt.sec, dt.microsec)
                        .ok_or_else(|| ExpressionError::type_error("lapse"))?,
                );
                let timestamp = naive_dt.and_utc().timestamp_millis();
                Ok(Value::BigInt(timestamp))
            }
            Value::Null(_) => Ok(Value::Null(NullType::Null)),
            _ => Err(ExpressionError::type_error(
                "The timestamp function requires a datetime type or no parameters.",
            )),
        }
    }
}

pub(crate) fn execute_to_timestamp(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::SmallInt(i) => convert_epoch_secs(*i as i64),
        Value::Int(i) => convert_epoch_secs(*i as i64),
        Value::BigInt(i) => convert_epoch_secs(*i),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "to_timestamp requires an integer (epoch seconds)",
        )),
    }
}

fn convert_epoch_secs(secs: i64) -> Result<Value, ExpressionError> {
    let dt = chrono::DateTime::from_timestamp(secs, 0)
        .ok_or_else(|| ExpressionError::type_error("Invalid timestamp"))?;
    let naive = dt.naive_utc();
    Ok(Value::DateTime(DateTimeValue {
        year: naive.year(),
        month: naive.month(),
        day: naive.day(),
        hour: naive.hour(),
        minute: naive.minute(),
        sec: naive.second(),
        microsec: naive.nanosecond() / 1000,
    }))
}

pub(crate) fn execute_epoch_ms(args: &[Value]) -> Result<Value, ExpressionError> {
    execute_timestamp(args)
}

pub(crate) fn execute_to_epoch_ms(args: &[Value]) -> Result<Value, ExpressionError> {
    execute_timestamp(args)
}
