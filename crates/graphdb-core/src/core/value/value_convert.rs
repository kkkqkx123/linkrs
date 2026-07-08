use crate::core::{
    error::StorageError,
    error::StorageResult,
    types::DataType,
    value::date_time::{DateTimeValue, DateValue, TimeValue},
    value::interval::IntervalValue,
    value::list::List,
    value::null::NullType,
    value::uuid::UuidValue,
    value::value_def::Value,
};
use chrono::{Datelike, Timelike};

impl Value {
    /// Convert to a boolean value
    pub fn to_bool(&self) -> Value {
        match self {
            Value::Empty | Value::Null(_) => Value::Null(NullType::Null),
            Value::Bool(b) => Value::Bool(*b),
            Value::SmallInt(i) => Value::Bool(*i != 0),
            Value::Int(i) => Value::Bool(*i != 0),
            Value::BigInt(i) => Value::Bool(*i != 0),
            Value::Float(f) => Value::Bool(*f != 0.0),
            Value::Double(f) => Value::Bool(*f != 0.0),
            Value::String(s) => {
                let lower = s.to_lowercase();
                if lower == "true" {
                    Value::Bool(true)
                } else if lower == "false" {
                    Value::Bool(false)
                } else {
                    Value::Null(NullType::Null)
                }
            }
            Value::FixedString { data, .. } => {
                let lower = data.to_lowercase();
                if lower == "true" {
                    Value::Bool(true)
                } else if lower == "false" {
                    Value::Bool(false)
                } else {
                    Value::Null(NullType::Null)
                }
            }
            _ => Value::Null(NullType::BadData),
        }
    }

    /// Convert to BigInt (i64)
    pub fn to_int(&self) -> Value {
        match self {
            Value::Empty | Value::Null(_) => Value::Null(NullType::Null),
            Value::SmallInt(i) => Value::BigInt(*i as i64),
            Value::Int(i) => Value::BigInt(*i as i64),
            Value::BigInt(i) => Value::BigInt(*i),
            Value::Float(f) => {
                if f.is_nan() || f.is_infinite() {
                    Value::Null(NullType::Null)
                } else {
                    Value::BigInt(*f as i64)
                }
            }
            Value::Double(f) => {
                if f.is_nan() || f.is_infinite() {
                    Value::Null(NullType::Null)
                } else {
                    Value::BigInt(*f as i64)
                }
            }
            Value::String(s) => match s.parse::<i64>() {
                Ok(i) => Value::BigInt(i),
                Err(_) => Value::Null(NullType::Null),
            },
            Value::FixedString { data, .. } => match data.parse::<i64>() {
                Ok(i) => Value::BigInt(i),
                Err(_) => Value::Null(NullType::Null),
            },
            Value::Bool(b) => Value::BigInt(if *b { 1 } else { 0 }),
            _ => Value::Null(NullType::BadData),
        }
    }

    /// Convert to Double (f64)
    pub fn to_float(&self) -> Value {
        match self {
            Value::Empty | Value::Null(_) => Value::Null(NullType::Null),
            Value::Float(f) => Value::Double(*f as f64),
            Value::Double(f) => Value::Double(*f),
            Value::SmallInt(i) => Value::Double(*i as f64),
            Value::Int(i) => Value::Double(*i as f64),
            Value::BigInt(i) => Value::Double(*i as f64),
            Value::String(s) => match s.parse::<f64>() {
                Ok(f) => Value::Double(f),
                Err(_) => Value::Null(NullType::Null),
            },
            Value::FixedString { data, .. } => match data.parse::<f64>() {
                Ok(f) => Value::Double(f),
                Err(_) => Value::Null(NullType::Null),
            },
            Value::Bool(b) => Value::Double(if *b { 1.0 } else { 0.0 }),
            _ => Value::Null(NullType::BadData),
        }
    }

    /// Convert to string
    pub fn to_string(&self) -> Result<String, String> {
        match self {
            Value::String(s) => Ok(s.clone()),
            Value::FixedString { data, .. } => Ok(data.clone()),
            Value::SmallInt(i) => Ok(i.to_string()),
            Value::Int(i) => Ok(i.to_string()),
            Value::BigInt(i) => Ok(i.to_string()),
            Value::Float(f) => {
                if f.is_nan() {
                    Ok("NaN".to_string())
                } else if f.is_infinite() {
                    if f.is_sign_positive() {
                        Ok("Infinity".to_string())
                    } else {
                        Ok("-Infinity".to_string())
                    }
                } else {
                    Ok(f.to_string())
                }
            }
            Value::Double(f) => {
                if f.is_nan() {
                    Ok("NaN".to_string())
                } else if f.is_infinite() {
                    if f.is_sign_positive() {
                        Ok("Infinity".to_string())
                    } else {
                        Ok("-Infinity".to_string())
                    }
                } else {
                    Ok(f.to_string())
                }
            }
            Value::Bool(b) => Ok(b.to_string()),
            Value::Null(n) => Ok(format!("{:?}", n)),
            Value::Empty => Ok("EMPTY".to_string()),
            Value::Date(d) => Ok(format!("{}-{:02}-{:02}", d.year, d.month, d.day)),
            Value::Time(t) => Ok(format!(
                "{:02}:{:02}:{:02}.{:06}",
                t.hour, t.minute, t.sec, t.microsec
            )),
            Value::DateTime(dt) => Ok(format!(
                "{}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}",
                dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.sec, dt.microsec
            )),
            Value::Uuid(u) => Ok(u.to_hyphenated_string()),
            Value::Interval(i) => Ok(i.to_postgresql()),
            Value::List(list) => {
                let items: Result<Vec<String>, _> = list
                    .iter()
                    .map(|v| v.to_string().map_err(|e| e.to_string()))
                    .collect();
                items.map(|items_str| format!("[{}]", items_str.join(", ")))
            }
            Value::Map(map) => {
                let items: Result<Vec<String>, _> = map
                    .iter()
                    .map(|(k, v)| v.to_string().map(|v_str| format!("{}: {}", k, v_str)))
                    .collect();
                items.map(|items_str| format!("{{{}}}", items_str.join(", ")))
            }
            _ => Err(format!("Cannot convert {:?} to string", self)),
        }
    }

    /// Convert to a list
    pub fn to_list(&self) -> Value {
        match self {
            Value::List(list) => Value::List(list.clone()),
            Value::Set(set) => Value::List(Box::new(List::from(
                set.iter().cloned().collect::<Vec<_>>(),
            ))),
            _ => Value::Null(NullType::BadData),
        }
    }

    /// Convert to a map:
    pub fn to_map(&self) -> Value {
        match self {
            Value::Map(map) => Value::Map(map.clone()),
            _ => Value::Null(NullType::BadData),
        }
    }

    /// Convert to a set
    pub fn to_set(&self) -> Value {
        match self {
            Value::Set(set) => Value::Set(set.clone()),
            Value::List(list) => Value::Set(Box::new(list.iter().cloned().collect())),
            _ => Value::Null(NullType::BadData),
        }
    }

    /// Convert to a date
    pub fn to_date(&self) -> Value {
        match self {
            Value::Empty | Value::Null(_) => Value::Null(NullType::Null),
            Value::Date(d) => Value::Date(d.clone()),
            Value::DateTime(dt) => Value::Date(DateValue {
                year: dt.year,
                month: dt.month,
                day: dt.day,
            }),
            Value::String(s) => Self::parse_date_string(s),
            Value::FixedString { data, .. } => Self::parse_date_string(data),
            Value::SmallInt(i) => Value::Date(Self::days_to_date(*i as i64)),
            Value::Int(i) => Value::Date(Self::days_to_date(*i as i64)),
            Value::BigInt(i) => Value::Date(Self::days_to_date(*i)),
            _ => Value::Null(NullType::BadData),
        }
    }

    /// Convert to time
    pub fn to_time(&self) -> Value {
        match self {
            Value::Empty | Value::Null(_) => Value::Null(NullType::Null),
            Value::Time(t) => Value::Time(t.clone()),
            Value::DateTime(dt) => Value::Time(TimeValue {
                hour: dt.hour,
                minute: dt.minute,
                sec: dt.sec,
                microsec: dt.microsec,
            }),
            Value::String(s) => Self::parse_time_string(s),
            Value::FixedString { data, .. } => Self::parse_time_string(data),
            _ => Value::Null(NullType::BadData),
        }
    }

    /// Convert to date and time
    pub fn to_datetime(&self) -> Value {
        match self {
            Value::Empty | Value::Null(_) => Value::Null(NullType::Null),
            Value::DateTime(dt) => Value::DateTime(dt.clone()),
            Value::Date(d) => Value::DateTime(DateTimeValue {
                year: d.year,
                month: d.month,
                day: d.day,
                hour: 0,
                minute: 0,
                sec: 0,
                microsec: 0,
            }),
            Value::Time(t) => Value::DateTime(DateTimeValue {
                year: 1970,
                month: 1,
                day: 1,
                hour: t.hour,
                minute: t.minute,
                sec: t.sec,
                microsec: t.microsec,
            }),
            Value::String(s) => Self::parse_datetime_string(s),
            Value::FixedString { data, .. } => Self::parse_datetime_string(data),
            Value::SmallInt(i) => {
                let date = Self::days_to_date(*i as i64);
                Value::DateTime(DateTimeValue {
                    year: date.year,
                    month: date.month,
                    day: date.day,
                    hour: 0,
                    minute: 0,
                    sec: 0,
                    microsec: 0,
                })
            }
            Value::Int(i) => {
                let date = Self::days_to_date(*i as i64);
                Value::DateTime(DateTimeValue {
                    year: date.year,
                    month: date.month,
                    day: date.day,
                    hour: 0,
                    minute: 0,
                    sec: 0,
                    microsec: 0,
                })
            }
            Value::BigInt(i) => {
                let date = Self::days_to_date(*i);
                Value::DateTime(DateTimeValue {
                    year: date.year,
                    month: date.month,
                    day: date.day,
                    hour: 0,
                    minute: 0,
                    sec: 0,
                    microsec: 0,
                })
            }
            _ => Value::Null(NullType::BadData),
        }
    }

    /// Convert to interval
    pub fn to_interval(&self) -> Value {
        match self {
            Value::Empty | Value::Null(_) => Value::Null(NullType::Null),
            Value::Interval(i) => Value::Interval(*i),
            Value::String(s) => match IntervalValue::parse(s) {
                Ok(i) => Value::Interval(i),
                Err(_) => Value::Null(NullType::BadData),
            },
            Value::FixedString { data, .. } => match IntervalValue::parse(data) {
                Ok(i) => Value::Interval(i),
                Err(_) => Value::Null(NullType::BadData),
            },
            _ => Value::Null(NullType::BadData),
        }
    }

    fn parse_date_string(s: &str) -> Value {
        let formats = vec!["%Y-%m-%d", "%Y/%m/%d", "%Y%m%d"];

        for format in &formats {
            if let Ok(dt) = chrono::NaiveDate::parse_from_str(s, format) {
                return Value::Date(DateValue {
                    year: dt.year(),
                    month: dt.month(),
                    day: dt.day(),
                });
            }
        }

        Value::Null(NullType::BadData)
    }

    fn parse_time_string(s: &str) -> Value {
        let formats = vec!["%H:%M:%S", "%H:%M:%S%.f", "%H:%M"];

        for format in &formats {
            if let Ok(time) = chrono::NaiveTime::parse_from_str(s, format) {
                return Value::Time(TimeValue {
                    hour: time.hour(),
                    minute: time.minute(),
                    sec: time.second(),
                    microsec: time.nanosecond() / 1000,
                });
            }
        }

        Value::Null(NullType::BadData)
    }

    fn parse_datetime_string(s: &str) -> Value {
        let formats = vec![
            "%Y-%m-%d %H:%M:%S",
            "%Y-%m-%d %H:%M:%S%.f",
            "%Y-%m-%dT%H:%M:%S",
            "%Y-%m-%dT%H:%M:%S%.f",
            "%Y/%m/%d %H:%M:%S",
        ];

        for format in &formats {
            if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, format) {
                return Value::DateTime(DateTimeValue {
                    year: dt.year(),
                    month: dt.month(),
                    day: dt.day(),
                    hour: dt.hour(),
                    minute: dt.minute(),
                    sec: dt.second(),
                    microsec: dt.nanosecond() / 1000,
                });
            }
        }

        Value::Null(NullType::BadData)
    }

    fn days_to_date(days: i64) -> DateValue {
        let epoch = match chrono::NaiveDate::from_ymd_opt(1970, 1, 1) {
            Some(date) => date,
            None => {
                return DateValue {
                    year: 1970,
                    month: 1,
                    day: 1,
                }
            }
        };
        let date = epoch + chrono::Duration::days(days);
        DateValue {
            year: date.year(),
            month: date.month(),
            day: date.day(),
        }
    }

    /// Try to implicitly convert to the specified type.
    pub fn try_implicit_cast(&self, target_type: &DataType) -> Result<Value, String> {
        match target_type {
            DataType::Bool => Ok(self.to_bool()),
            DataType::SmallInt => Ok(self.to_smallint()),
            DataType::Int => Ok(self.to_int32()),
            DataType::BigInt => Ok(self.to_int()),
            DataType::Float => Ok(self.to_float32()),
            DataType::Double => Ok(self.to_float()),
            DataType::String => self.to_string().map(Value::String),
            DataType::FixedString(len) => match self {
                Value::String(s) | Value::FixedString { data: s, .. } => {
                    Ok(Value::fixed_string(*len, s.clone()))
                }
                _ => self.to_string().map(|s| Value::fixed_string(*len, s)),
            },
            DataType::Date => Ok(self.to_date()),
            DataType::Time => Ok(self.to_time()),
            DataType::DateTime => Ok(self.to_datetime()),
            DataType::Uuid => Ok(self.to_uuid()),
            DataType::Interval => Ok(self.to_interval()),
            _ => Err(format!("Cannot implicitly cast to {:?}", target_type)),
        }
    }

    /// Convert to SmallInt (i16)
    pub fn to_smallint(&self) -> Value {
        match self {
            Value::Empty | Value::Null(_) => Value::Null(NullType::Null),
            Value::SmallInt(i) => Value::SmallInt(*i),
            Value::Int(i) => Value::SmallInt(*i as i16),
            Value::BigInt(i) => Value::SmallInt(*i as i16),
            Value::Float(f) => Value::SmallInt(*f as i16),
            Value::Double(f) => Value::SmallInt(*f as i16),
            Value::String(s) => match s.parse::<i16>() {
                Ok(i) => Value::SmallInt(i),
                Err(_) => Value::Null(NullType::Null),
            },
            Value::FixedString { data, .. } => match data.parse::<i16>() {
                Ok(i) => Value::SmallInt(i),
                Err(_) => Value::Null(NullType::Null),
            },
            Value::Bool(b) => Value::SmallInt(if *b { 1 } else { 0 }),
            _ => Value::Null(NullType::BadData),
        }
    }

    /// Convert to Int (i32)
    pub fn to_int32(&self) -> Value {
        match self {
            Value::Empty | Value::Null(_) => Value::Null(NullType::Null),
            Value::SmallInt(i) => Value::Int(*i as i32),
            Value::Int(i) => Value::Int(*i),
            Value::BigInt(i) => Value::Int(*i as i32),
            Value::Float(f) => Value::Int(*f as i32),
            Value::Double(f) => Value::Int(*f as i32),
            Value::String(s) => match s.parse::<i32>() {
                Ok(i) => Value::Int(i),
                Err(_) => Value::Null(NullType::Null),
            },
            Value::FixedString { data, .. } => match data.parse::<i32>() {
                Ok(i) => Value::Int(i),
                Err(_) => Value::Null(NullType::Null),
            },
            Value::Bool(b) => Value::Int(if *b { 1 } else { 0 }),
            _ => Value::Null(NullType::BadData),
        }
    }

    /// Convert to Float (f32)
    pub fn to_float32(&self) -> Value {
        match self {
            Value::Empty | Value::Null(_) => Value::Null(NullType::Null),
            Value::Float(f) => Value::Float(*f),
            Value::Double(f) => Value::Float(*f as f32),
            Value::SmallInt(i) => Value::Float(*i as f32),
            Value::Int(i) => Value::Float(*i as f32),
            Value::BigInt(i) => Value::Float(*i as f32),
            Value::String(s) => match s.parse::<f32>() {
                Ok(f) => Value::Float(f),
                Err(_) => Value::Null(NullType::Null),
            },
            Value::FixedString { data, .. } => match data.parse::<f32>() {
                Ok(f) => Value::Float(f),
                Err(_) => Value::Null(NullType::Null),
            },
            Value::Bool(b) => Value::Float(if *b { 1.0 } else { 0.0 }),
            _ => Value::Null(NullType::BadData),
        }
    }

    /// Check whether an implicit conversion is possible.
    pub fn can_implicitly_cast_to(&self, target_type: &DataType) -> bool {
        self.try_implicit_cast(target_type).is_ok()
    }

    /// Check whether the value is a valid number.
    pub fn is_valid_number(&self) -> bool {
        match self {
            Value::SmallInt(_) | Value::Int(_) | Value::BigInt(_) => true,
            Value::Float(f) => !f.is_nan() && !f.is_infinite(),
            Value::Double(f) => !f.is_nan() && !f.is_infinite(),
            _ => false,
        }
    }

    /// Check whether the value is a valid date.
    pub fn is_valid_date(&self) -> bool {
        match self {
            Value::Date(d) => {
                d.year >= 0
                    && d.year <= 9999
                    && d.month >= 1
                    && d.month <= 12
                    && d.day >= 1
                    && d.day <= 31
            }
            _ => false,
        }
    }

    /// Check whether the value represents a valid time.
    pub fn is_valid_time(&self) -> bool {
        match self {
            Value::Time(t) => t.hour <= 23 && t.minute <= 59 && t.sec <= 59 && t.microsec <= 999999,
            _ => false,
        }
    }

    /// Check whether the value is a valid date and time.
    pub fn is_valid_datetime(&self) -> bool {
        match self {
            Value::DateTime(dt) => {
                dt.year >= 0
                    && dt.year <= 9999
                    && dt.month >= 1
                    && dt.month <= 12
                    && dt.day >= 1
                    && dt.day <= 31
                    && dt.hour <= 23
                    && dt.minute <= 59
                    && dt.sec <= 59
                    && dt.microsec <= 999999
            }
            _ => false,
        }
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<i16> for Value {
    fn from(value: i16) -> Self {
        Value::SmallInt(value)
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Value::Int(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::BigInt(value)
    }
}

impl From<f32> for Value {
    fn from(value: f32) -> Self {
        Value::Float(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Double(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::String(value)
    }
}

impl From<UuidValue> for Value {
    fn from(value: UuidValue) -> Self {
        Value::Uuid(value)
    }
}

impl From<IntervalValue> for Value {
    fn from(value: IntervalValue) -> Self {
        Value::Interval(value)
    }
}

impl Value {
    /// Convert to UUID
    pub fn to_uuid(&self) -> Value {
        match self {
            Value::Empty | Value::Null(_) => Value::Null(NullType::Null),
            Value::Uuid(u) => Value::Uuid(*u),
            Value::String(s) => match UuidValue::parse_str(s) {
                Ok(u) => Value::Uuid(u),
                Err(_) => Value::Null(NullType::BadData),
            },
            Value::FixedString { data, .. } => match UuidValue::parse_str(data) {
                Ok(u) => Value::Uuid(u),
                Err(_) => Value::Null(NullType::BadData),
            },
            Value::Blob(b) => match UuidValue::from_slice(b) {
                Ok(u) => Value::Uuid(u),
                Err(_) => Value::Null(NullType::BadData),
            },
            _ => Value::Null(NullType::BadData),
        }
    }

    /// Try to cast this value to the target data type.
    /// Returns the converted value on success, or a type_mismatch error.
    /// Supports implicit type conversions that are allowed by TypeUtils::can_cast.
    pub fn try_cast_to(&self, target: &DataType) -> StorageResult<Value> {
        if self.data_type() == *target {
            return Ok(self.clone());
        }

        let result = match target {
            DataType::Null | DataType::Empty => Value::Null(NullType::Null),
            DataType::Bool => self.to_bool(),
            DataType::SmallInt => match self.to_int() {
                Value::BigInt(i) if i >= i16::MIN as i64 && i <= i16::MAX as i64 => {
                    Value::SmallInt(i as i16)
                }
                Value::BigInt(_) => {
                    return Err(StorageError::type_mismatch(
                        DataType::SmallInt,
                        self.data_type(),
                    ));
                }
                other => other,
            },
            DataType::Int => match self.to_int() {
                Value::BigInt(i) if i >= i32::MIN as i64 && i <= i32::MAX as i64 => {
                    Value::Int(i as i32)
                }
                Value::BigInt(_) => {
                    return Err(StorageError::type_mismatch(DataType::Int, self.data_type()));
                }
                other => other,
            },
            DataType::BigInt => self.to_int(),
            DataType::Float => match self.to_float() {
                Value::Double(f) => Value::Float(f as f32),
                other => other,
            },
            DataType::Double => self.to_float(),
            DataType::String => match self.to_string() {
                Ok(s) => Value::String(s),
                Err(_) => Value::Null(NullType::BadData),
            },
            DataType::Date => self.to_date(),
            DataType::Time => self.to_time(),
            DataType::DateTime | DataType::Timestamp => self.to_datetime(),
            DataType::List => self.to_list(),
            DataType::Map => self.to_map(),
            DataType::Geography => match self {
                Value::Null(_) | Value::Empty => Value::Null(NullType::Null),
                Value::Geography(g) => Value::Geography(g.clone()),
                _ => {
                    return Err(StorageError::type_mismatch(
                        target.clone(),
                        self.data_type(),
                    ))
                }
            },
            _ => Value::Null(NullType::BadData),
        };

        if matches!(result, Value::Null(NullType::BadData)) {
            Err(StorageError::type_mismatch(
                target.clone(),
                self.data_type(),
            ))
        } else {
            Ok(result)
        }
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::String(value.to_string())
    }
}

impl From<NullType> for Value {
    fn from(value: NullType) -> Self {
        Value::Null(value)
    }
}

impl From<Vec<Value>> for Value {
    fn from(value: Vec<Value>) -> Self {
        Value::list(List::from(value))
    }
}

impl From<std::collections::HashMap<String, Value>> for Value {
    fn from(value: std::collections::HashMap<String, Value>) -> Self {
        Value::map(value)
    }
}

impl From<std::collections::HashSet<Value>> for Value {
    fn from(value: std::collections::HashSet<Value>) -> Self {
        Value::set(value)
    }
}

impl From<(i64, &str)> for Value {
    fn from(value: (i64, &str)) -> Self {
        Value::list(List::from(vec![
            Value::BigInt(value.0),
            Value::String(value.1.to_string()),
        ]))
    }
}

impl From<(i64, String)> for Value {
    fn from(value: (i64, String)) -> Self {
        Value::list(List::from(vec![
            Value::BigInt(value.0),
            Value::String(value.1),
        ]))
    }
}
