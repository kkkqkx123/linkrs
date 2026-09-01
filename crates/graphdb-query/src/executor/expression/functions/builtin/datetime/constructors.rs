use crate::executor::expression::ExpressionError;
use chrono::{Datelike, Timelike};
use graphdb_core::value::{DateTimeValue, DateValue, NullType, TimeValue};
use graphdb_core::Value;

pub(crate) fn execute_now(_args: &[Value]) -> Result<Value, ExpressionError> {
    let now = chrono::Utc::now();
    Ok(Value::DateTime(DateTimeValue {
        year: now.year(),
        month: now.month(),
        day: now.day(),
        hour: now.hour(),
        minute: now.minute(),
        sec: now.second(),
        microsec: now.timestamp_subsec_micros(),
    }))
}

pub(crate) fn execute_date(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.is_empty() {
        let now = chrono::Utc::now();
        Ok(Value::Date(DateValue {
            year: now.year(),
            month: now.month(),
            day: now.day(),
        }))
    } else {
        match &args[0] {
            Value::String(s) => {
                let naivedate = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| {
                    ExpressionError::type_error(
                        "Unable to parse date string, expect format: YYYY-MM-DD",
                    )
                })?;
                let date = DateValue {
                    year: naivedate.year(),
                    month: naivedate.month(),
                    day: naivedate.day(),
                };
                Ok(Value::Date(date))
            }
            Value::Null(_) => Ok(Value::Null(NullType::Null)),
            _ => Err(ExpressionError::type_error(
                "The date function requires a string type",
            )),
        }
    }
}

pub(crate) fn execute_time(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.is_empty() {
        let now = chrono::Utc::now();
        Ok(Value::Time(TimeValue {
            hour: now.hour(),
            minute: now.minute(),
            sec: now.second(),
            microsec: now.timestamp_subsec_micros(),
        }))
    } else {
        match &args[0] {
            Value::String(s) => {
                let time = chrono::NaiveTime::parse_from_str(s, "%H:%M:%S%.f")
                    .or_else(|_| chrono::NaiveTime::parse_from_str(s, "%H:%M:%S"))
                    .map_err(|_| {
                        ExpressionError::type_error(
                            "Unable to parse time string, expect format: HH:MM:SS",
                        )
                    })?;
                let time_val = TimeValue {
                    hour: time.hour(),
                    minute: time.minute(),
                    sec: time.second(),
                    microsec: time.nanosecond() / 1000,
                };
                Ok(Value::Time(time_val))
            }
            Value::Null(_) => Ok(Value::Null(NullType::Null)),
            _ => Err(ExpressionError::type_error(
                "The time function requires a string type",
            )),
        }
    }
}

pub(crate) fn execute_datetime(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.is_empty() {
        let now = chrono::Utc::now();
        Ok(Value::DateTime(DateTimeValue {
            year: now.year(),
            month: now.month(),
            day: now.day(),
            hour: now.hour(),
            minute: now.minute(),
            sec: now.second(),
            microsec: now.timestamp_subsec_micros(),
        }))
    } else {
        match &args[0] {
            Value::String(s) => {
                let datetime = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                    .map_err(|_| {
                        ExpressionError::type_error(
                            "Unable to parse datetime string, expect format: YYYY-MM-DD HH:MM:SS",
                        )
                    })?;
                let dt_val = DateTimeValue {
                    year: datetime.year(),
                    month: datetime.month(),
                    day: datetime.day(),
                    hour: datetime.hour(),
                    minute: datetime.minute(),
                    sec: datetime.second(),
                    microsec: datetime.nanosecond() / 1000,
                };
                Ok(Value::DateTime(dt_val))
            }
            Value::Null(_) => Ok(Value::Null(NullType::Null)),
            _ => Err(ExpressionError::type_error(
                "The datetime function requires a string type",
            )),
        }
    }
}

pub(crate) fn execute_current_date(_args: &[Value]) -> Result<Value, ExpressionError> {
    let now = chrono::Utc::now();
    Ok(Value::Date(DateValue {
        year: now.year(),
        month: now.month(),
        day: now.day(),
    }))
}

pub(crate) fn execute_current_timestamp(_args: &[Value]) -> Result<Value, ExpressionError> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time error")
        .as_millis();
    Ok(Value::BigInt(now as i64))
}
