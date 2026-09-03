use crate::executor::expression::ExpressionError;
use chrono::{Datelike, Timelike};
use graphdb_core::value::NullType;
use graphdb_core::Value;

define_datetime_extractor!(execute_year_inner, Date => year, DateTime => year);
define_datetime_extractor!(execute_month_inner, Date => month, DateTime => month);
define_datetime_extractor!(execute_day_inner, Date => day, DateTime => day);
define_datetime_extractor!(execute_hour_inner, Time => hour, DateTime => hour);
define_datetime_extractor!(execute_minute_inner, Time => minute, DateTime => minute);
define_datetime_extractor!(execute_second_inner, Time => sec, DateTime => sec);

pub(crate) fn execute_year(args: &[Value]) -> Result<Value, ExpressionError> {
    execute_year_inner(args)
}

pub(crate) fn execute_month(args: &[Value]) -> Result<Value, ExpressionError> {
    execute_month_inner(args)
}

pub(crate) fn execute_day(args: &[Value]) -> Result<Value, ExpressionError> {
    execute_day_inner(args)
}

pub(crate) fn execute_hour(args: &[Value]) -> Result<Value, ExpressionError> {
    execute_hour_inner(args)
}

pub(crate) fn execute_minute(args: &[Value]) -> Result<Value, ExpressionError> {
    execute_minute_inner(args)
}

pub(crate) fn execute_second(args: &[Value]) -> Result<Value, ExpressionError> {
    execute_second_inner(args)
}

pub(crate) fn execute_date_part(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "date_part requires 2 arguments",
        ));
    }
    let part = match &args[0] {
        Value::String(s) => s.to_lowercase(),
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "date_part first argument must be a string",
            ))
        }
    };
    match &args[1] {
        Value::Date(d) => {
            let naive = chrono::NaiveDate::from_ymd_opt(d.year, d.month, d.day)
                .ok_or_else(|| ExpressionError::type_error("Invalid date"))?;
            extract_date_part(
                &part,
                DateParts {
                    year: naive.year(),
                    month: naive.month(),
                    day: naive.day(),
                    hour: 0,
                    minute: 0,
                    second: 0,
                    millis: 0,
                },
            )
        }
        Value::DateTime(dt) => {
            let naive = chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(dt.year, dt.month, dt.day)
                    .ok_or_else(|| ExpressionError::type_error("Invalid date"))?,
                chrono::NaiveTime::from_hms_micro_opt(dt.hour, dt.minute, dt.sec, dt.microsec)
                    .ok_or_else(|| ExpressionError::type_error("Invalid time"))?,
            );
            extract_date_part(
                &part,
                DateParts {
                    year: naive.year(),
                    month: naive.month(),
                    day: naive.day(),
                    hour: naive.hour(),
                    minute: naive.minute(),
                    second: naive.second(),
                    millis: naive.and_utc().timestamp_subsec_millis(),
                },
            )
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "date_part requires a date or datetime as second argument",
        )),
    }
}

/// Calendar/time components extracted from a date or datetime value, used to
/// evaluate a single `date_part` without threading eight positional arguments.
struct DateParts {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    millis: u32,
}

fn extract_date_part(part: &str, p: DateParts) -> Result<Value, ExpressionError> {
    match part {
        "year" => Ok(Value::Int(p.year)),
        "month" => Ok(Value::Int(p.month as i32)),
        "day" => Ok(Value::Int(p.day as i32)),
        "hour" => Ok(Value::Int(p.hour as i32)),
        "minute" => Ok(Value::Int(p.minute as i32)),
        "second" => Ok(Value::Int(p.second as i32)),
        "millisecond" => Ok(Value::Int(p.millis as i32)),
        "dow" => {
            let naive = chrono::NaiveDate::from_ymd_opt(p.year, p.month, p.day)
                .ok_or_else(|| ExpressionError::type_error("Invalid date"))?;
            Ok(Value::Int(naive.weekday().num_days_from_sunday() as i32))
        }
        "doy" => {
            let naive = chrono::NaiveDate::from_ymd_opt(p.year, p.month, p.day)
                .ok_or_else(|| ExpressionError::type_error("Invalid date"))?;
            Ok(Value::Int(naive.ordinal() as i32))
        }
        "quarter" => Ok(Value::Int(((p.month - 1) / 3 + 1) as i32)),
        _ => Err(ExpressionError::type_error(format!(
            "Unknown date part: {}",
            part
        ))),
    }
}

pub(crate) fn execute_century(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("century requires 1 argument"));
    }
    match &args[0] {
        Value::Date(d) => Ok(Value::Int((d.year - 1) / 100 + 1)),
        Value::DateTime(dt) => Ok(Value::Int((dt.year - 1) / 100 + 1)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "century requires a date or datetime type",
        )),
    }
}

pub(crate) fn execute_day_name(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("day_name requires 1 argument"));
    }
    match &args[0] {
        Value::Date(d) => {
            let naive = chrono::NaiveDate::from_ymd_opt(d.year, d.month, d.day)
                .ok_or_else(|| ExpressionError::type_error("Invalid date"))?;
            let name = format!("{}", naive.weekday());
            Ok(Value::string(name))
        }
        Value::DateTime(dt) => {
            let naive = chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(dt.year, dt.month, dt.day)
                    .ok_or_else(|| ExpressionError::type_error("Invalid date"))?,
                chrono::NaiveTime::from_hms_micro_opt(dt.hour, dt.minute, dt.sec, dt.microsec)
                    .ok_or_else(|| ExpressionError::type_error("Invalid time"))?,
            );
            let name = format!("{}", naive.and_utc().weekday());
            Ok(Value::string(name))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "day_name requires a date or datetime type",
        )),
    }
}

pub(crate) fn execute_month_name(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "month_name requires 1 argument",
        ));
    }
    match &args[0] {
        Value::Date(d) => {
            let name = match d.month {
                1 => "January",
                2 => "February",
                3 => "March",
                4 => "April",
                5 => "May",
                6 => "June",
                7 => "July",
                8 => "August",
                9 => "September",
                10 => "October",
                11 => "November",
                12 => "December",
                _ => "Unknown",
            };
            Ok(Value::string(name))
        }
        Value::DateTime(dt) => {
            let name = match dt.month {
                1 => "January",
                2 => "February",
                3 => "March",
                4 => "April",
                5 => "May",
                6 => "June",
                7 => "July",
                8 => "August",
                9 => "September",
                10 => "October",
                11 => "November",
                12 => "December",
                _ => "Unknown",
            };
            Ok(Value::string(name))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "month_name requires a date or datetime type",
        )),
    }
}
