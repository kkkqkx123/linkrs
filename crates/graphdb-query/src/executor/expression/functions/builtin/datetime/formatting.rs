use crate::executor::expression::ExpressionError;
use chrono::Datelike;
use graphdb_core::value::{DateValue, NullType};
use graphdb_core::Value;

pub(crate) fn execute_to_char(args: &[Value]) -> Result<Value, ExpressionError> {
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

pub(crate) fn execute_to_date(args: &[Value]) -> Result<Value, ExpressionError> {
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
