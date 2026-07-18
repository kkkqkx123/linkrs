use crate::core::value::null::NullType;
use crate::core::Value;
use crate::transaction::undo_log::PropertyValue;

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
