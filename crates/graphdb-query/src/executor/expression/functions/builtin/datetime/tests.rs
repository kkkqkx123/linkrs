use super::*;
use graphdb_core::value::{DateValue, DateTimeValue, NullType};
use graphdb_core::Value;

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

#[test]
fn test_date_add_with_date() {
    let func = DateTimeFunction::DateAdd;
    let date = DateValue {
        year: 2024,
        month: 1,
        day: 1,
    };
    let result = func
        .execute(&[Value::Date(date), Value::Int(10)])
        .expect("date_add should succeed");
    match result {
        Value::Date(d) => {
            assert_eq!(d.year, 2024);
            assert_eq!(d.month, 1);
            assert_eq!(d.day, 11);
        }
        _ => panic!("Expected Date value"),
    }
}

#[test]
fn test_date_add_with_datetime() {
    let func = DateTimeFunction::DateAdd;
    let dt = DateTimeValue {
        year: 2024,
        month: 12,
        day: 30,
        hour: 12,
        minute: 0,
        sec: 0,
        microsec: 0,
    };
    let result = func
        .execute(&[Value::DateTime(dt), Value::Int(5)])
        .expect("date_add should succeed");
    match result {
        Value::DateTime(d) => {
            assert_eq!(d.year, 2025);
            assert_eq!(d.month, 1);
            assert_eq!(d.day, 4);
        }
        _ => panic!("Expected DateTime value"),
    }
}

#[test]
fn test_date_add_null() {
    let func = DateTimeFunction::DateAdd;
    let result = func
        .execute(&[Value::Null(NullType::Null), Value::Int(5)])
        .expect("date_add should succeed");
    assert_eq!(result, Value::Null(NullType::Null));
}

#[test]
fn test_date_sub_with_date() {
    let func = DateTimeFunction::DateSub;
    let date = DateValue {
        year: 2024,
        month: 1,
        day: 15,
    };
    let result = func
        .execute(&[Value::Date(date), Value::Int(10)])
        .expect("date_sub should succeed");
    match result {
        Value::Date(d) => {
            assert_eq!(d.year, 2024);
            assert_eq!(d.month, 1);
            assert_eq!(d.day, 5);
        }
        _ => panic!("Expected Date value"),
    }
}

#[test]
fn test_date_sub_with_datetime() {
    let func = DateTimeFunction::DateSub;
    let dt = DateTimeValue {
        year: 2024,
        month: 3,
        day: 1,
        hour: 0,
        minute: 0,
        sec: 0,
        microsec: 0,
    };
    let result = func
        .execute(&[Value::DateTime(dt), Value::Int(1)])
        .expect("date_sub should succeed");
    match result {
        Value::DateTime(d) => {
            assert_eq!(d.year, 2024);
            assert_eq!(d.month, 2);
            assert_eq!(d.day, 29);
        }
        _ => panic!("Expected DateTime value"),
    }
}

#[test]
fn test_date_diff_dates() {
    let func = DateTimeFunction::DateDiff;
    let d1 = DateValue {
        year: 2024,
        month: 1,
        day: 1,
    };
    let d2 = DateValue {
        year: 2024,
        month: 1,
        day: 11,
    };
    let result = func
        .execute(&[Value::Date(d1), Value::Date(d2)])
        .expect("date_diff should succeed");
    assert_eq!(result, Value::BigInt(10));
}

#[test]
fn test_date_diff_datetimes() {
    let func = DateTimeFunction::DateDiff;
    let dt1 = DateTimeValue {
        year: 2024,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        sec: 0,
        microsec: 0,
    };
    let dt2 = DateTimeValue {
        year: 2024,
        month: 1,
        day: 1,
        hour: 12,
        minute: 0,
        sec: 0,
        microsec: 0,
    };
    let result = func
        .execute(&[Value::DateTime(dt1), Value::DateTime(dt2)])
        .expect("date_diff should succeed");
    assert_eq!(result, Value::BigInt(12 * 3600 * 1000));
}

#[test]
fn test_date_diff_null() {
    let func = DateTimeFunction::DateDiff;
    let result = func
        .execute(&[Value::Null(NullType::Null), Value::Null(NullType::Null)])
        .expect("date_diff should succeed");
    assert_eq!(result, Value::Null(NullType::Null));
}

#[test]
fn test_date_trunc_year() {
    let func = DateTimeFunction::DateTrunc;
    let dt = DateTimeValue {
        year: 2024,
        month: 6,
        day: 15,
        hour: 14,
        minute: 30,
        sec: 45,
        microsec: 0,
    };
    let result = func
        .execute(&[Value::DateTime(dt), Value::string("year")])
        .expect("date_trunc should succeed");
    match result {
        Value::DateTime(d) => {
            assert_eq!(d.year, 2024);
            assert_eq!(d.month, 1);
            assert_eq!(d.day, 1);
            assert_eq!(d.hour, 0);
            assert_eq!(d.minute, 0);
            assert_eq!(d.sec, 0);
        }
        _ => panic!("Expected DateTime value"),
    }
}

#[test]
fn test_date_trunc_month() {
    let func = DateTimeFunction::DateTrunc;
    let dt = DateTimeValue {
        year: 2024,
        month: 6,
        day: 15,
        hour: 14,
        minute: 30,
        sec: 45,
        microsec: 0,
    };
    let result = func
        .execute(&[Value::DateTime(dt), Value::string("month")])
        .expect("date_trunc should succeed");
    match result {
        Value::DateTime(d) => {
            assert_eq!(d.month, 6);
            assert_eq!(d.day, 1);
            assert_eq!(d.hour, 0);
        }
        _ => panic!("Expected DateTime value"),
    }
}

#[test]
fn test_date_trunc_day() {
    let func = DateTimeFunction::DateTrunc;
    let dt = DateTimeValue {
        year: 2024,
        month: 6,
        day: 15,
        hour: 14,
        minute: 30,
        sec: 45,
        microsec: 0,
    };
    let result = func
        .execute(&[Value::DateTime(dt), Value::string("day")])
        .expect("date_trunc should succeed");
    match result {
        Value::DateTime(d) => {
            assert_eq!(d.day, 15);
            assert_eq!(d.hour, 0);
            assert_eq!(d.minute, 0);
        }
        _ => panic!("Expected DateTime value"),
    }
}

#[test]
fn test_date_trunc_hour() {
    let func = DateTimeFunction::DateTrunc;
    let dt = DateTimeValue {
        year: 2024,
        month: 6,
        day: 15,
        hour: 14,
        minute: 30,
        sec: 45,
        microsec: 0,
    };
    let result = func
        .execute(&[Value::DateTime(dt), Value::string("hour")])
        .expect("date_trunc should succeed");
    match result {
        Value::DateTime(d) => {
            assert_eq!(d.hour, 14);
            assert_eq!(d.minute, 0);
            assert_eq!(d.sec, 0);
        }
        _ => panic!("Expected DateTime value"),
    }
}

#[test]
fn test_date_trunc_with_date() {
    let func = DateTimeFunction::DateTrunc;
    let date = DateValue {
        year: 2024,
        month: 6,
        day: 15,
    };
    let result = func
        .execute(&[Value::Date(date), Value::string("month")])
        .expect("date_trunc should succeed");
    match result {
        Value::Date(d) => {
            assert_eq!(d.month, 6);
            assert_eq!(d.day, 1);
        }
        _ => panic!("Expected Date value"),
    }
}

#[test]
fn test_to_char_date() {
    let func = DateTimeFunction::ToChar;
    let date = DateValue {
        year: 2024,
        month: 6,
        day: 15,
    };
    let result = func
        .execute(&[Value::Date(date), Value::string("%Y-%m-%d")])
        .expect("to_char should succeed");
    assert_eq!(result, Value::string("2024-06-15"));
}

#[test]
fn test_to_char_datetime() {
    let func = DateTimeFunction::ToChar;
    let dt = DateTimeValue {
        year: 2024,
        month: 6,
        day: 15,
        hour: 14,
        minute: 30,
        sec: 45,
        microsec: 0,
    };
    let result = func
        .execute(&[Value::DateTime(dt), Value::string("%Y/%m/%d %H:%M:%S")])
        .expect("to_char should succeed");
    assert_eq!(result, Value::string("2024/06/15 14:30:45"));
}

#[test]
fn test_to_char_null() {
    let func = DateTimeFunction::ToChar;
    let result = func
        .execute(&[Value::Null(NullType::Null), Value::string("%Y-%m-%d")])
        .expect("to_char should succeed");
    assert_eq!(result, Value::Null(NullType::Null));
}

#[test]
fn test_to_date_ymd() {
    let func = DateTimeFunction::ToDate;
    let result = func
        .execute(&[Value::string("2024-06-15")])
        .expect("to_date should succeed");
    match result {
        Value::Date(d) => {
            assert_eq!(d.year, 2024);
            assert_eq!(d.month, 6);
            assert_eq!(d.day, 15);
        }
        _ => panic!("Expected Date value"),
    }
}

#[test]
fn test_to_date_slash_format() {
    let func = DateTimeFunction::ToDate;
    let result = func
        .execute(&[Value::string("2024/06/15")])
        .expect("to_date should succeed");
    match result {
        Value::Date(d) => {
            assert_eq!(d.year, 2024);
            assert_eq!(d.month, 6);
            assert_eq!(d.day, 15);
        }
        _ => panic!("Expected Date value"),
    }
}

#[test]
fn test_to_date_compact_format() {
    let func = DateTimeFunction::ToDate;
    let result = func
        .execute(&[Value::string("20240615")])
        .expect("to_date should succeed");
    match result {
        Value::Date(d) => {
            assert_eq!(d.year, 2024);
            assert_eq!(d.month, 6);
            assert_eq!(d.day, 15);
        }
        _ => panic!("Expected Date value"),
    }
}

#[test]
fn test_to_date_null() {
    let func = DateTimeFunction::ToDate;
    let result = func
        .execute(&[Value::Null(NullType::Null)])
        .expect("to_date should succeed");
    assert_eq!(result, Value::Null(NullType::Null));
}

#[test]
fn test_age_with_date() {
    let func = DateTimeFunction::Age;
    let date = DateValue {
        year: 2024,
        month: 1,
        day: 1,
    };
    let result = func
        .execute(&[Value::Date(date)])
        .expect("age should succeed");
    match result {
        Value::BigInt(days) => {
            assert!(days > 0);
        }
        _ => panic!("Expected BigInt value"),
    }
}

#[test]
fn test_age_with_datetime() {
    let func = DateTimeFunction::Age;
    let dt = DateTimeValue {
        year: 2024,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        sec: 0,
        microsec: 0,
    };
    let result = func
        .execute(&[Value::DateTime(dt)])
        .expect("age should succeed");
    match result {
        Value::BigInt(days) => {
            assert!(days > 0);
        }
        _ => panic!("Expected BigInt value"),
    }
}

#[test]
fn test_age_null() {
    let func = DateTimeFunction::Age;
    let result = func
        .execute(&[Value::Null(NullType::Null)])
        .expect("age should succeed");
    assert_eq!(result, Value::Null(NullType::Null));
}

#[test]
fn test_last_day_date() {
    let func = DateTimeFunction::LastDay;
    let date = DateValue {
        year: 2024,
        month: 2,
        day: 1,
    };
    let result = func
        .execute(&[Value::Date(date)])
        .expect("last_day should succeed");
    match result {
        Value::Date(d) => {
            assert_eq!(d.day, 29);
        }
        _ => panic!("Expected Date value"),
    }
}

#[test]
fn test_last_day_datetime() {
    let func = DateTimeFunction::LastDay;
    let dt = DateTimeValue {
        year: 2024,
        month: 2,
        day: 1,
        hour: 12,
        minute: 0,
        sec: 0,
        microsec: 0,
    };
    let result = func
        .execute(&[Value::DateTime(dt)])
        .expect("last_day should succeed");
    match result {
        Value::DateTime(d) => {
            assert_eq!(d.day, 29);
        }
        _ => panic!("Expected DateTime value"),
    }
}

#[test]
fn test_last_day_non_leap() {
    let func = DateTimeFunction::LastDay;
    let date = DateValue {
        year: 2023,
        month: 2,
        day: 1,
    };
    let result = func
        .execute(&[Value::Date(date)])
        .expect("last_day should succeed");
    match result {
        Value::Date(d) => {
            assert_eq!(d.day, 28);
        }
        _ => panic!("Expected Date value"),
    }
}

#[test]
fn test_last_day_january() {
    let func = DateTimeFunction::LastDay;
    let date = DateValue {
        year: 2024,
        month: 1,
        day: 1,
    };
    let result = func
        .execute(&[Value::Date(date)])
        .expect("last_day should succeed");
    match result {
        Value::Date(d) => {
            assert_eq!(d.day, 31);
        }
        _ => panic!("Expected Date value"),
    }
}

#[test]
fn test_last_day_null() {
    let func = DateTimeFunction::LastDay;
    let result = func
        .execute(&[Value::Null(NullType::Null)])
        .expect("last_day should succeed");
    assert_eq!(result, Value::Null(NullType::Null));
}
