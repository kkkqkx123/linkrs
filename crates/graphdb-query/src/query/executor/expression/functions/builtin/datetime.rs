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
