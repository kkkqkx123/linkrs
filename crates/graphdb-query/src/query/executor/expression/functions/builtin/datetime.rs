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
        ToChar => {
            name: "to_char",
            arity: 2,
            variadic: false,
            description: "Format datetime as string",
            handler: execute_to_char
        },
        ToDate => {
            name: "to_date",
            arity: 1,
            variadic: false,
            description: "Convert string to date",
            handler: execute_to_date
        },
        Age => {
            name: "age",
            arity: 1,
            variadic: false,
            description: "Calculate age/interval from date/datetime to now",
            handler: execute_age
        },
        LastDay => {
            name: "last_day",
            arity: 1,
            variadic: false,
            description: "Get last day of the month",
            handler: execute_last_day
        },
        GenerateSeries => {
            name: "generate_series",
            arity: 2,
            variadic: true,
            description: "Generate a series of timestamps",
            handler: execute_generate_series
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

fn execute_to_char(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "to_char requires 2 arguments",
        ));
    }
    let format_str = match &args[1] {
        Value::String(s) => s.clone(),
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => return Err(ExpressionError::type_error("to_char format must be a string")),
    };
    match &args[0] {
        Value::Date(d) => {
            let naive = chrono::NaiveDate::from_ymd_opt(d.year, d.month, d.day)
                .ok_or_else(|| ExpressionError::type_error("Invalid date"))?;
            let formatted = naive.format(&format_str).to_string();
            Ok(Value::String(formatted))
        }
        Value::DateTime(dt) => {
            let naive = chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(dt.year, dt.month, dt.day)
                    .ok_or_else(|| ExpressionError::type_error("Invalid date"))?,
                chrono::NaiveTime::from_hms_micro_opt(dt.hour, dt.minute, dt.sec, dt.microsec)
                    .ok_or_else(|| ExpressionError::type_error("Invalid time"))?,
            );
            let formatted = naive.format(&format_str).to_string();
            Ok(Value::String(formatted))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "to_char requires a date or datetime as first argument",
        )),
    }
}

fn execute_to_date(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::String(s) => {
            let formats = ["%Y-%m-%d", "%Y/%m/%d", "%d-%m-%Y", "%m-%d-%Y", "%Y%m%d"];
            for fmt in &formats {
                if let Ok(naivedate) = chrono::NaiveDate::parse_from_str(s, fmt) {
                    return Ok(Value::Date(DateValue {
                        year: naivedate.year(),
                        month: naivedate.month(),
                        day: naivedate.day(),
                    }));
                }
            }
            Err(ExpressionError::type_error(
                "Unable to parse date string, supported formats: YYYY-MM-DD, YYYY/MM/DD, DD-MM-YYYY, MM-DD-YYYY, YYYYMMDD",
            ))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "to_date requires a string argument",
        )),
    }
}

fn execute_age(args: &[Value]) -> Result<Value, ExpressionError> {
    let now = chrono::Utc::now();
    match &args[0] {
        Value::Date(d) => {
            let naive = chrono::NaiveDate::from_ymd_opt(d.year, d.month, d.day)
                .ok_or_else(|| ExpressionError::type_error("Invalid date"))?;
            let target = naive.and_hms_opt(0, 0, 0)
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

fn execute_last_day(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_generate_series(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() < 2 || args.len() > 3 {
        return Err(ExpressionError::type_error(
            "generate_series requires 2 or 3 arguments",
        ));
    }
    let start = match &args[0] {
        Value::BigInt(v) => *v,
        Value::Int(v) => *v as i64,
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => return Err(ExpressionError::type_error("generate_series start must be an integer")),
    };
    let end = match &args[1] {
        Value::BigInt(v) => *v,
        Value::Int(v) => *v as i64,
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => return Err(ExpressionError::type_error("generate_series end must be an integer")),
    };
    let step = if args.len() > 2 {
        match &args[2] {
            Value::BigInt(v) => *v,
            Value::Int(v) => *v as i64,
            Value::Null(_) => return Ok(Value::Null(NullType::Null)),
            _ => return Err(ExpressionError::type_error("generate_series step must be an integer")),
        }
    } else {
        1
    };

    if step == 0 {
        return Err(ExpressionError::type_error("generate_series step cannot be 0"));
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

    use crate::core::value::list::List;
    Ok(Value::list(List { values: result }))
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
