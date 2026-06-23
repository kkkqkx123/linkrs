use crate::core::value::null::NullType;
use crate::core::Value;
use crate::transaction::undo_log::PropertyValue;

pub fn value_to_bytes(value: &Value) -> Vec<u8> {
    match value {
        Value::Null(_) | Value::Empty => vec![0],
        Value::Bool(v) => {
            let mut buf = vec![1];
            buf.extend_from_slice(&[*v as u8]);
            buf
        }
        Value::Int(v) => {
            let mut buf = vec![2];
            buf.extend_from_slice(&v.to_le_bytes());
            buf
        }
        Value::BigInt(v) => {
            let mut buf = vec![3];
            buf.extend_from_slice(&v.to_le_bytes());
            buf
        }
        Value::Float(v) => {
            let mut buf = vec![4];
            buf.extend_from_slice(&v.to_le_bytes());
            buf
        }
        Value::Double(v) => {
            let mut buf = vec![5];
            buf.extend_from_slice(&v.to_le_bytes());
            buf
        }
        Value::String(v) => {
            let mut buf = vec![6];
            let bytes = v.as_bytes();
            buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(bytes);
            buf
        }
        Value::Blob(v) => {
            let mut buf = vec![7];
            buf.extend_from_slice(&(v.len() as u32).to_le_bytes());
            buf.extend_from_slice(v);
            buf
        }
        _ => vec![0],
    }
}

pub fn bytes_to_value(data: &[u8]) -> Option<Value> {
    if data.is_empty() {
        return None;
    }
    match data[0] {
        0 => Some(Value::Null(NullType::Null)),
        1 => data.get(1).map(|&v| Value::Bool(v != 0)),
        2 => data
            .get(1..9)
            .map(|b| i32::from_le_bytes(b.try_into().unwrap_or([0; 4])))
            .map(Value::Int),
        3 => data
            .get(1..9)
            .map(|b| i64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .map(Value::BigInt),
        4 => data
            .get(1..9)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap_or([0; 4])))
            .map(Value::Float),
        5 => data
            .get(1..9)
            .map(|b| f64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .map(Value::Double),
        6 => {
            if data.len() < 5 {
                return None;
            }
            let len = u32::from_le_bytes([data[1], data[2], data[3], data[4]]) as usize;
            data.get(5..5 + len)
                .and_then(|b| std::str::from_utf8(b).ok())
                .map(|s| Value::String(s.to_string()))
        }
        7 => {
            if data.len() < 5 {
                return None;
            }
            let len = u32::from_le_bytes([data[1], data[2], data[3], data[4]]) as usize;
            data.get(5..5 + len).map(|b| Value::Blob(b.to_vec()))
        }
        _ => None,
    }
}

pub fn property_value_to_value(pv: PropertyValue) -> Value {
    match pv {
        PropertyValue::Int(v) => Value::BigInt(v),
        PropertyValue::Float(v) => Value::Double(v),
        PropertyValue::String(v) => Value::String(v),
        PropertyValue::Bytes(v) => Value::Blob(v),
        PropertyValue::Bool(v) => Value::Bool(v),
        PropertyValue::Null => Value::Null(NullType::Null),
    }
}

pub fn value_to_property_value(value: &Value) -> PropertyValue {
    match value {
        Value::BigInt(v) => PropertyValue::Int(*v),
        Value::Double(v) => PropertyValue::Float(*v),
        Value::String(v) => PropertyValue::String(v.clone()),
        Value::Blob(v) => PropertyValue::Bytes(v.clone()),
        Value::Bool(v) => PropertyValue::Bool(*v),
        Value::Null(_) | Value::Empty => PropertyValue::Null,
        _ => PropertyValue::Null,
    }
}
