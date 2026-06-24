use graphdb_core::core::{DataType, Value};

#[derive(Debug, Clone)]
pub struct ConversionError {
    pub message: String,
}

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Conversion error: {}", self.message)
    }
}

impl std::error::Error for ConversionError {}

macro_rules! conversion_err {
    ($($arg:tt)*) => {
        ConversionError { message: format!($($arg)*) }
    };
}

fn type_name(dt: &DataType) -> &'static str {
    match dt {
        DataType::Empty => "EMPTY",
        DataType::Null => "NULL",
        DataType::Bool => "BOOL",
        DataType::SmallInt => "SMALLINT",
        DataType::Int => "INT",
        DataType::BigInt => "BIGINT",
        DataType::Float => "FLOAT",
        DataType::Double => "DOUBLE",
        DataType::Decimal128 => "DECIMAL",
        DataType::String => "STRING",
        DataType::Date => "DATE",
        DataType::Time => "TIME",
        DataType::DateTime => "DATETIME",
        DataType::Blob => "BLOB",
        DataType::Timestamp => "TIMESTAMP",
        DataType::Uuid => "UUID",
        DataType::Interval => "INTERVAL",
        DataType::Json => "JSON",
        DataType::JsonB => "JSONB",
        DataType::FixedString(_) => "FIXED_STRING",
        _ => "COMPLEX",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::core::value::null::NullType;

    #[test]
    fn test_identity_conversion() {
        assert_eq!(
            convert_value(&Value::Int(42), &DataType::Int).unwrap(),
            Value::Int(42)
        );
        assert_eq!(
            convert_value(&Value::String("hello".into()), &DataType::String).unwrap(),
            Value::String("hello".into())
        );
    }

    #[test]
    fn test_int_to_bigint() {
        assert_eq!(
            convert_value(&Value::Int(42), &DataType::BigInt).unwrap(),
            Value::BigInt(42)
        );
    }

    #[test]
    fn test_smallint_to_int() {
        assert_eq!(
            convert_value(&Value::SmallInt(100), &DataType::Int).unwrap(),
            Value::Int(100)
        );
    }

    #[test]
    fn test_int_to_smallint_ok() {
        assert_eq!(
            convert_value(&Value::Int(42), &DataType::SmallInt).unwrap(),
            Value::SmallInt(42)
        );
    }

    #[test]
    fn test_int_to_smallint_overflow() {
        let result = convert_value(&Value::Int(99999), &DataType::SmallInt);
        assert!(result.is_err());
    }

    #[test]
    fn test_bigint_to_int_ok() {
        assert_eq!(
            convert_value(&Value::BigInt(42), &DataType::Int).unwrap(),
            Value::Int(42)
        );
    }

    #[test]
    fn test_bigint_to_int_overflow() {
        let result = convert_value(&Value::BigInt(9999999999i64), &DataType::Int);
        assert!(result.is_err());
    }

    #[test]
    fn test_float_to_double() {
        let result = convert_value(&Value::Float(3.14), &DataType::Double).unwrap();
        match result {
            Value::Double(d) => assert!((d - 3.14_f64).abs() < 1e-6),
            _ => panic!("Expected Double"),
        }
    }

    #[test]
    fn test_string_to_int() {
        assert_eq!(
            convert_value(&Value::String("42".into()), &DataType::Int).unwrap(),
            Value::Int(42)
        );
    }

    #[test]
    fn test_string_to_bool() {
        assert_eq!(
            convert_value(&Value::String("true".into()), &DataType::Bool).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            convert_value(&Value::String("false".into()), &DataType::Bool).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn test_string_to_bool_invalid() {
        let result = convert_value(&Value::String("maybe".into()), &DataType::Bool);
        assert!(result.is_err());
    }

    #[test]
    fn test_null_conversion() {
        let result = convert_value(&Value::Null(NullType::Null), &DataType::String).unwrap();
        assert!(result.is_null());
    }

    #[test]
    fn test_value_to_string() {
        assert_eq!(
            convert_value(&Value::Int(42), &DataType::String).unwrap(),
            Value::String("42".into())
        );
        assert_eq!(
            convert_value(&Value::Bool(true), &DataType::String).unwrap(),
            Value::String("true".into())
        );
    }

    #[test]
    fn test_unsupported_conversion() {
        let result = convert_value(&Value::Int(42), &DataType::Date);
        assert!(result.is_err());
    }
}

pub fn convert_value(value: &Value, target_type: &DataType) -> Result<Value, ConversionError> {
    if value.is_null() || value.is_empty() {
        return Ok(Value::Null(graphdb_core::core::value::null::NullType::Null));
    }

    let source_type = value.get_type();
    if source_type == *target_type {
        return Ok(value.clone());
    }

    match (value, target_type) {
        (Value::SmallInt(i), DataType::Int) => Ok(Value::Int(*i as i32)),
        (Value::SmallInt(i), DataType::BigInt) => Ok(Value::BigInt(*i as i64)),
        (Value::SmallInt(i), DataType::Float) => Ok(Value::Float(*i as f32)),
        (Value::SmallInt(i), DataType::Double) => Ok(Value::Double(*i as f64)),
        (Value::SmallInt(i), DataType::String) => Ok(Value::String(i.to_string())),

        (Value::Int(i), DataType::BigInt) => Ok(Value::BigInt(*i as i64)),
        (Value::Int(i), DataType::Float) => Ok(Value::Float(*i as f32)),
        (Value::Int(i), DataType::Double) => Ok(Value::Double(*i as f64)),
        (Value::Int(i), DataType::SmallInt) => {
            let v = *i;
            if v < i16::MIN as i32 || v > i16::MAX as i32 {
                return Err(conversion_err!("Value {} out of range for SMALLINT", v));
            }
            Ok(Value::SmallInt(v as i16))
        }
        (Value::Int(i), DataType::String) => Ok(Value::String(i.to_string())),

        (Value::BigInt(i), DataType::Int) => {
            let v = *i;
            if v < i32::MIN as i64 || v > i32::MAX as i64 {
                return Err(conversion_err!("Value {} out of range for INT", v));
            }
            Ok(Value::Int(v as i32))
        }
        (Value::BigInt(i), DataType::SmallInt) => {
            let v = *i;
            if v < i16::MIN as i64 || v > i16::MAX as i64 {
                return Err(conversion_err!("Value {} out of range for SMALLINT", v));
            }
            Ok(Value::SmallInt(v as i16))
        }
        (Value::BigInt(i), DataType::Float) => Ok(Value::Float(*i as f32)),
        (Value::BigInt(i), DataType::Double) => Ok(Value::Double(*i as f64)),
        (Value::BigInt(i), DataType::String) => Ok(Value::String(i.to_string())),

        (Value::Float(f), DataType::Double) => Ok(Value::Double(*f as f64)),
        (Value::Float(f), DataType::String) => Ok(Value::String(f.to_string())),

        (Value::Double(d), DataType::Float) => Ok(Value::Float(*d as f32)),
        (Value::Double(d), DataType::String) => Ok(Value::String(d.to_string())),

        (Value::String(s), DataType::SmallInt) => {
            s.parse::<i16>().map(Value::SmallInt).map_err(|e| conversion_err!("Cannot parse '{}' as SMALLINT: {}", s, e))
        }
        (Value::String(s), DataType::Int) => {
            s.parse::<i32>().map(Value::Int).map_err(|e| conversion_err!("Cannot parse '{}' as INT: {}", s, e))
        }
        (Value::String(s), DataType::BigInt) => {
            s.parse::<i64>().map(Value::BigInt).map_err(|e| conversion_err!("Cannot parse '{}' as BIGINT: {}", s, e))
        }
        (Value::String(s), DataType::Float) => {
            s.parse::<f32>().map(Value::Float).map_err(|e| conversion_err!("Cannot parse '{}' as FLOAT: {}", s, e))
        }
        (Value::String(s), DataType::Double) => {
            s.parse::<f64>().map(Value::Double).map_err(|e| conversion_err!("Cannot parse '{}' as DOUBLE: {}", s, e))
        }
        (Value::String(s), DataType::Bool) => {
            match s.to_lowercase().as_str() {
                "true" | "yes" | "1" => Ok(Value::Bool(true)),
                "false" | "no" | "0" => Ok(Value::Bool(false)),
                _ => Err(conversion_err!("Cannot parse '{}' as BOOL", s)),
            }
        }

        (_, DataType::String) => Ok(Value::String(format!("{}", value))),

        _ => Err(conversion_err!(
            "Unsupported conversion from {:?} to {}",
            value.get_type(),
            type_name(target_type),
        )),
    }
}

/// Check if a type conversion is supported (without needing an actual value).
/// Mirrors the conversion paths in [`convert_value`].
pub fn is_compatible_type(from: &DataType, to: &DataType) -> bool {
    if from == to {
        return true;
    }
    match (from, to) {
        // SmallInt widening
        (DataType::SmallInt, DataType::Int | DataType::BigInt | DataType::Float | DataType::Double | DataType::String) => true,
        // Int widening/narrowing
        (DataType::Int, DataType::SmallInt | DataType::BigInt | DataType::Float | DataType::Double | DataType::String) => true,
        // BigInt widening/narrowing
        (DataType::BigInt, DataType::Int | DataType::SmallInt | DataType::Float | DataType::Double | DataType::String) => true,
        // Float widening
        (DataType::Float, DataType::Double | DataType::String) => true,
        // Double narrowing
        (DataType::Double, DataType::Float | DataType::String) => true,
        // String parsing
        (DataType::String, DataType::SmallInt | DataType::Int | DataType::BigInt | DataType::Float | DataType::Double | DataType::Bool) => true,
        // Any type to String
        (_, DataType::String) => true,
        _ => false,
    }
}
