//! Implementation of date and time functions

use crate::core::value::{DateTimeValue, DateValue, NullType, TimeValue};
use crate::core::Value;
use crate::query::executor::expression::ExpressionError;
use chrono::{Datelike, Timelike};

define_function_enum! {
    /// Date and time function enumeration
    pub enum DateTimeFunction {
        Now => {
            name: "now",
            arity: 0,
            variadic: false,
            description: "current timestamp",
            handler: execute_now
        },
        Date => {
            name: "date",
            arity: 1,
            variadic: false,
            description: "Date of creation",
            handler: execute_date
        },
        Time => {
            name: "time",
            arity: 1,
            variadic: false,
            description: "Creation time",
            handler: execute_time
        },
        DateTime => {
            name: "datetime",
            arity: 0,
            variadic: true,
            description: "Creation date and time",
            handler: execute_datetime
        },
        Year => {
            name: "year",
            arity: 1,
            variadic: false,
            description: "Year of extraction",
            handler: execute_year
        },
        Month => {
            name: "month",
            arity: 1,
            variadic: false,
            description: "Month of withdrawal",
            handler: execute_month
        },
        Day => {
            name: "day",
            arity: 1,
            variadic: false,
            description: "Withdrawal date",
            handler: execute_day
        },
        Hour => {
            name: "hour",
            arity: 1,
            variadic: false,
            description: "Withdrawal hours",
            handler: execute_hour
        },
        Minute => {
            name: "minute",
            arity: 1,
            variadic: false,
            description: "Extraction minutes",
            handler: execute_minute
        },
        Second => {
            name: "second",
            arity: 1,
            variadic: false,
            description: "withdrawal second",
            handler: execute_second
        },
        TimeStamp => {
            name: "timestamp",
            arity: 0,
            variadic: true,
            description: "Get current timestamp or convert datetime to timestamp",
            handler: execute_timestamp
        },
        DateAdd => {
            name: "date_add",
            arity: 2,
            variadic: false,
            description: "Add interval to date/datetime",
            handler: execute_date_add
        },
        DateSub => {
            name: "date_sub",
            arity: 2,
            variadic: false,
            description: "Subtract interval from date/datetime",
            handler: execute_date_sub
        },
        DateDiff => {
            name: "date_diff",
            arity: 2,
            variadic: false,
            description: "Calculate difference between two dates/datetimes",
            handler: execute_date_diff
        },
        DateTrunc => {
            name: "date_trunc",
            arity: 2,
            variadic: false,
            description: "Truncate date/datetime to specified precision",
            handler: execute_date_trunc
        },
        CurrentDate => {
            name: "current_date",
            arity: 0,
            variadic: false,
            description: "Get current date",
            handler: execute_current_date
        },
        CurrentTimestamp => {
            name: "current_timestamp",
            arity: 0,
            variadic: false,
            description: "Get current timestamp",
            handler: execute_current_timestamp
        },
    }
}

impl DateTimeFunction {
    /// Call the function (with caching)
    pub fn execute_with_cache(
        &self,
        args: &[Value],
        _cache: &mut (),
    ) -> Result<Value, ExpressionError> {
        // The caching function has been removed; the `execute` method can be called directly.
        self.execute(args)
    }
}

fn execute_now(_args: &[Value]) -> Result<Value, ExpressionError> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time error")
        .as_millis();
    Ok(Value::BigInt(now as i64))
}

fn execute_date(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.is_empty() {
        // Return the current date
        let now = chrono::Utc::now();
        Ok(Value::Date(DateValue {
            year: now.year(),
            month: now.month(),
            day: now.day(),
        }))
    } else {
        match &args[0] {
            Value::String(s) => {
                // Parse the date
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

fn execute_time(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.is_empty() {
        // Return the current time
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
                // Analysis time
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

define_datetime_extractor!(execute_year, Date => year, DateTime => year);
define_datetime_extractor!(execute_month, Date => month, DateTime => month);
define_datetime_extractor!(execute_day, Date => day, DateTime => day);
define_datetime_extractor!(execute_hour, Time => hour, DateTime => hour);
define_datetime_extractor!(execute_minute, Time => minute, DateTime => minute);
define_datetime_extractor!(execute_second, Time => sec, DateTime => sec);

fn execute_datetime(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_timestamp(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_date_add(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "date_add requires 2 arguments",
        ));
    }
    let amount = match &args[1] {
        Value::Int(i) => *i as i64,
        Value::BigInt(i) => *i,
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => return Err(ExpressionError::type_error("date_add amount must be an integer")),
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

fn execute_date_sub(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "date_sub requires 2 arguments",
        ));
    }
    let amount = match &args[1] {
        Value::Int(i) => *i as i64,
        Value::BigInt(i) => *i,
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => return Err(ExpressionError::type_error("date_sub amount must be an integer")),
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

fn execute_date_diff(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_date_trunc(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "date_trunc requires 2 arguments",
        ));
    }
    let precision = match &args[1] {
        Value::String(s) => s.as_str(),
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => return Err(ExpressionError::type_error("date_trunc precision must be a string")),
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

fn execute_current_date(_args: &[Value]) -> Result<Value, ExpressionError> {
    let now = chrono::Utc::now();
    Ok(Value::Date(DateValue {
        year: now.year(),
        month: now.month(),
        day: now.day(),
    }))
}

fn execute_current_timestamp(_args: &[Value]) -> Result<Value, ExpressionError> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time error")
        .as_millis();
    Ok(Value::BigInt(now as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_now() {
        let func = DateTimeFunction::Now;
        let result = func.execute(&[]).expect("Execution should succeed");
        assert!(matches!(result, Value::BigInt(_)));
    }

    #[test]
    fn test_year() {
        let func = DateTimeFunction::Year;
        let date = DateValue {
            year: 2024,
            month: 1,
            day: 15,
        };
        let result = func
            .execute(&[Value::Date(date)])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Int(2024));
    }

    #[test]
    fn test_month() {
        let func = DateTimeFunction::Month;
        let date = DateValue {
            year: 2024,
            month: 6,
            day: 15,
        };
        let result = func
            .execute(&[Value::Date(date)])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Int(6));
    }

    #[test]
    fn test_day() {
        let func = DateTimeFunction::Day;
        let date = DateValue {
            year: 2024,
            month: 6,
            day: 25,
        };
        let result = func
            .execute(&[Value::Date(date)])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Int(25));
    }

    #[test]
    fn test_null_handling() {
        let func = DateTimeFunction::Year;
        let result = func
            .execute(&[Value::Null(NullType::Null)])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Null(NullType::Null));
    }
}
