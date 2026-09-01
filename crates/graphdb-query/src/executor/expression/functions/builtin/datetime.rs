//! Implementation of date and time functions

use crate::executor::expression::ExpressionError;
use chrono::{Datelike, Timelike};
use graphdb_core::value::{DateTimeValue, DateValue, NullType, TimeValue};
use graphdb_core::Value;

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
        ToYears => {
            name: "to_years",
            arity: 1,
            variadic: false,
            description: "Convert number to years interval",
            handler: execute_to_years
        },
        ToMonths => {
            name: "to_months",
            arity: 1,
            variadic: false,
            description: "Convert number to months interval",
            handler: execute_to_months
        },
        ToDays => {
            name: "to_days",
            arity: 1,
            variadic: false,
            description: "Convert number to days interval",
            handler: execute_to_days
        },
        ToHours => {
            name: "to_hours",
            arity: 1,
            variadic: false,
            description: "Convert number to hours interval",
            handler: execute_to_hours
        },
        ToMinutes => {
            name: "to_minutes",
            arity: 1,
            variadic: false,
            description: "Convert number to minutes interval",
            handler: execute_to_minutes
        },
        ToSeconds => {
            name: "to_seconds",
            arity: 1,
            variadic: false,
            description: "Convert number to seconds interval",
            handler: execute_to_seconds
        },
        ToMilliseconds => {
            name: "to_milliseconds",
            arity: 1,
            variadic: false,
            description: "Convert number to milliseconds interval",
            handler: execute_to_milliseconds
        },
        ToMicroseconds => {
            name: "to_microseconds",
            arity: 1,
            variadic: false,
            description: "Convert number to microseconds interval",
            handler: execute_to_microseconds
        },
        Century => {
            name: "century",
            arity: 1,
            variadic: false,
            description: "Extract century from date/datetime",
            handler: execute_century
        },
        EpochMs => {
            name: "epoch_ms",
            arity: 1,
            variadic: false,
            description: "Convert date/datetime to epoch milliseconds",
            handler: execute_epoch_ms
        },
        ToTimestamp => {
            name: "to_timestamp",
            arity: 1,
            variadic: false,
            description: "Convert epoch seconds to timestamp",
            handler: execute_to_timestamp
        },
        ToEpochMs => {
            name: "to_epoch_ms",
            arity: 1,
            variadic: false,
            description: "Convert date/datetime to epoch milliseconds",
            handler: execute_to_epoch_ms
        },
        DatePart => {
            name: "date_part",
            arity: 2,
            variadic: false,
            description: "Extract date part (year, month, day, hour, minute, second, dow, doy, quarter)",
            handler: execute_date_part
        },
        DayName => {
            name: "day_name",
            arity: 1,
            variadic: false,
            description: "Get day name from date/datetime",
            handler: execute_day_name
        },
        MonthName => {
            name: "month_name",
            arity: 1,
            variadic: false,
            description: "Get month name from date/datetime",
            handler: execute_month_name
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

fn execute_date_sub(args: &[Value]) -> Result<Value, ExpressionError> {
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
        return Err(ExpressionError::type_error("to_char requires 2 arguments"));
    }
    let format_str = match &args[1] {
        Value::String(s) => s.to_string(),
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "to_char format must be a string",
            ))
        }
    };
    match &args[0] {
        Value::Date(d) => {
            let naive = chrono::NaiveDate::from_ymd_opt(d.year, d.month, d.day)
                .ok_or_else(|| ExpressionError::type_error("Invalid date"))?;
            let formatted = naive.format(&format_str).to_string();
            Ok(Value::string(formatted))
        }
        Value::DateTime(dt) => {
            let naive = chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(dt.year, dt.month, dt.day)
                    .ok_or_else(|| ExpressionError::type_error("Invalid date"))?,
                chrono::NaiveTime::from_hms_micro_opt(dt.hour, dt.minute, dt.sec, dt.microsec)
                    .ok_or_else(|| ExpressionError::type_error("Invalid time"))?,
            );
            let formatted = naive.format(&format_str).to_string();
            Ok(Value::string(formatted))
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

fn execute_to_years(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("to_years requires 1 argument"));
    }
    match &args[0] {
        Value::SmallInt(i) => Ok(Value::BigInt(*i as i64 * 365 * 24 * 60 * 60 * 1000)),
        Value::Int(i) => Ok(Value::BigInt(*i as i64 * 365 * 24 * 60 * 60 * 1000)),
        Value::BigInt(i) => Ok(Value::BigInt(*i * 365 * 24 * 60 * 60 * 1000)),
        Value::Float(f) => Ok(Value::Double(*f as f64 * 365.0 * 24.0 * 60.0 * 60.0 * 1000.0)),
        Value::Double(f) => Ok(Value::Double(*f * 365.0 * 24.0 * 60.0 * 60.0 * 1000.0)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("to_years requires a numeric type")),
    }
}

fn execute_to_months(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("to_months requires 1 argument"));
    }
    match &args[0] {
        Value::SmallInt(i) => Ok(Value::BigInt(*i as i64 * 30 * 24 * 60 * 60 * 1000)),
        Value::Int(i) => Ok(Value::BigInt(*i as i64 * 30 * 24 * 60 * 60 * 1000)),
        Value::BigInt(i) => Ok(Value::BigInt(*i * 30 * 24 * 60 * 60 * 1000)),
        Value::Float(f) => Ok(Value::Double(*f as f64 * 30.0 * 24.0 * 60.0 * 60.0 * 1000.0)),
        Value::Double(f) => Ok(Value::Double(*f * 30.0 * 24.0 * 60.0 * 60.0 * 1000.0)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("to_months requires a numeric type")),
    }
}

fn execute_to_days(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("to_days requires 1 argument"));
    }
    match &args[0] {
        Value::SmallInt(i) => Ok(Value::BigInt(*i as i64 * 24 * 60 * 60 * 1000)),
        Value::Int(i) => Ok(Value::BigInt(*i as i64 * 24 * 60 * 60 * 1000)),
        Value::BigInt(i) => Ok(Value::BigInt(*i * 24 * 60 * 60 * 1000)),
        Value::Float(f) => Ok(Value::Double(*f as f64 * 24.0 * 60.0 * 60.0 * 1000.0)),
        Value::Double(f) => Ok(Value::Double(*f * 24.0 * 60.0 * 60.0 * 1000.0)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("to_days requires a numeric type")),
    }
}

fn execute_to_hours(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("to_hours requires 1 argument"));
    }
    match &args[0] {
        Value::SmallInt(i) => Ok(Value::BigInt(*i as i64 * 60 * 60 * 1000)),
        Value::Int(i) => Ok(Value::BigInt(*i as i64 * 60 * 60 * 1000)),
        Value::BigInt(i) => Ok(Value::BigInt(*i * 60 * 60 * 1000)),
        Value::Float(f) => Ok(Value::Double(*f as f64 * 60.0 * 60.0 * 1000.0)),
        Value::Double(f) => Ok(Value::Double(*f * 60.0 * 60.0 * 1000.0)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("to_hours requires a numeric type")),
    }
}

fn execute_to_minutes(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("to_minutes requires 1 argument"));
    }
    match &args[0] {
        Value::SmallInt(i) => Ok(Value::BigInt(*i as i64 * 60 * 1000)),
        Value::Int(i) => Ok(Value::BigInt(*i as i64 * 60 * 1000)),
        Value::BigInt(i) => Ok(Value::BigInt(*i * 60 * 1000)),
        Value::Float(f) => Ok(Value::Double(*f as f64 * 60.0 * 1000.0)),
        Value::Double(f) => Ok(Value::Double(*f * 60.0 * 1000.0)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "to_minutes requires a numeric type",
        )),
    }
}

fn execute_to_seconds(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("to_seconds requires 1 argument"));
    }
    match &args[0] {
        Value::SmallInt(i) => Ok(Value::BigInt(*i as i64 * 1000)),
        Value::Int(i) => Ok(Value::BigInt(*i as i64 * 1000)),
        Value::BigInt(i) => Ok(Value::BigInt(*i * 1000)),
        Value::Float(f) => Ok(Value::Double(*f as f64 * 1000.0)),
        Value::Double(f) => Ok(Value::Double(*f * 1000.0)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "to_seconds requires a numeric type",
        )),
    }
}

fn execute_to_milliseconds(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "to_milliseconds requires 1 argument",
        ));
    }
    match &args[0] {
        Value::SmallInt(i) => Ok(Value::BigInt(*i as i64)),
        Value::Int(i) => Ok(Value::BigInt(*i as i64)),
        Value::BigInt(i) => Ok(Value::BigInt(*i)),
        Value::Float(f) => Ok(Value::Double(*f as f64)),
        Value::Double(f) => Ok(Value::Double(*f)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "to_milliseconds requires a numeric type",
        )),
    }
}

fn execute_to_microseconds(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "to_microseconds requires 1 argument",
        ));
    }
    match &args[0] {
        Value::SmallInt(i) => Ok(Value::BigInt(*i as i64 / 1000)),
        Value::Int(i) => Ok(Value::BigInt(*i as i64 / 1000)),
        Value::BigInt(i) => Ok(Value::BigInt(*i / 1000)),
        Value::Float(f) => Ok(Value::Double(*f as f64 / 1000.0)),
        Value::Double(f) => Ok(Value::Double(*f / 1000.0)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "to_microseconds requires a numeric type",
        )),
    }
}

fn execute_century(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_epoch_ms(args: &[Value]) -> Result<Value, ExpressionError> {
    execute_timestamp(args)
}

fn execute_to_timestamp(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "to_timestamp requires 1 argument",
        ));
    }
    match &args[0] {
        Value::SmallInt(i) => {
            let secs = *i as i64;
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
        Value::Int(i) => {
            let secs = *i as i64;
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
        Value::BigInt(i) => {
            let dt = chrono::DateTime::from_timestamp(*i, 0)
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
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "to_timestamp requires an integer (epoch seconds)",
        )),
    }
}

fn execute_to_epoch_ms(args: &[Value]) -> Result<Value, ExpressionError> {
    execute_timestamp(args)
}

fn execute_date_part(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error("date_part requires 2 arguments"));
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
            extract_date_part(&part, naive.year(), naive.month(), naive.day(), 0, 0, 0, 0)
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
                naive.year(),
                naive.month(),
                naive.day(),
                naive.hour(),
                naive.minute(),
                naive.second(),
                naive.and_utc().timestamp_subsec_millis(),
            )
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "date_part requires a date or datetime as second argument",
        )),
    }
}

fn extract_date_part(
    part: &str,
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    millis: u32,
) -> Result<Value, ExpressionError> {
    match part {
        "year" => Ok(Value::Int(year)),
        "month" => Ok(Value::Int(month as i32)),
        "day" => Ok(Value::Int(day as i32)),
        "hour" => Ok(Value::Int(hour as i32)),
        "minute" => Ok(Value::Int(minute as i32)),
        "second" => Ok(Value::Int(second as i32)),
        "millisecond" => Ok(Value::Int(millis as i32)),
        "dow" => {
            let naive = chrono::NaiveDate::from_ymd_opt(year, month, day)
                .ok_or_else(|| ExpressionError::type_error("Invalid date"))?;
            Ok(Value::Int(naive.weekday().num_days_from_sunday() as i32))
        }
        "doy" => {
            let naive = chrono::NaiveDate::from_ymd_opt(year, month, day)
                .ok_or_else(|| ExpressionError::type_error("Invalid date"))?;
            Ok(Value::Int(naive.ordinal() as i32))
        }
        "quarter" => Ok(Value::Int(((month - 1) / 3 + 1) as i32)),
        _ => Err(ExpressionError::type_error(format!(
            "Unknown date part: {}",
            part
        ))),
    }
}

fn execute_day_name(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_month_name(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("month_name requires 1 argument"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_now() {
        let func = DateTimeFunction::Now;
        let result = func.execute(&[]).expect("Execution should succeed");
        assert!(matches!(result, Value::DateTime(_)));
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

    #[test]
    fn test_to_years() {
        let func = DateTimeFunction::ToYears;
        let result = func
            .execute(&[Value::Int(2)])
            .expect("to_years should succeed");
        assert_eq!(result, Value::BigInt(2 * 365 * 24 * 60 * 60 * 1000));
    }

    #[test]
    fn test_to_days() {
        let func = DateTimeFunction::ToDays;
        let result = func
            .execute(&[Value::Int(7)])
            .expect("to_days should succeed");
        assert_eq!(result, Value::BigInt(7 * 24 * 60 * 60 * 1000));
    }

    #[test]
    fn test_century() {
        let func = DateTimeFunction::Century;
        let date = DateValue {
            year: 2024,
            month: 6,
            day: 15,
        };
        let result = func
            .execute(&[Value::Date(date)])
            .expect("century should succeed");
        assert_eq!(result, Value::Int(21));
    }

    #[test]
    fn test_to_timestamp() {
        let func = DateTimeFunction::ToTimestamp;
        let result = func
            .execute(&[Value::BigInt(0)])
            .expect("to_timestamp should succeed");
        assert!(matches!(result, Value::DateTime(_)));
    }

    #[test]
    fn test_date_part() {
        let func = DateTimeFunction::DatePart;
        let date = DateValue {
            year: 2024,
            month: 6,
            day: 15,
        };
        let result = func
            .execute(&[Value::string("year"), Value::Date(date)])
            .expect("date_part should succeed");
        assert_eq!(result, Value::Int(2024));
    }

    #[test]
    fn test_day_name() {
        let func = DateTimeFunction::DayName;
        let date = DateValue {
            year: 2024,
            month: 1,
            day: 1,
        };
        let result = func
            .execute(&[Value::Date(date)])
            .expect("day_name should succeed");
        assert!(matches!(result, Value::String(_)));
    }

    #[test]
    fn test_month_name() {
        let func = DateTimeFunction::MonthName;
        let date = DateValue {
            year: 2024,
            month: 6,
            day: 15,
        };
        let result = func
            .execute(&[Value::Date(date)])
            .expect("month_name should succeed");
        assert_eq!(result, Value::string("June"));
    }
}
