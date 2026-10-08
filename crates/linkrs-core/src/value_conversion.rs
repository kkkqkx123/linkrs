//! C API Value Type Conversion Module
//!
//! Provides conversions between linkrs_value_t and core::Value.

use crate::Value;

use crate::types::c_api::{
    linkrs_string_t, linkrs_value_data_t, linkrs_value_t, linkrs_value_type_t,
};

/// Converting Core Value to C API Value Types
pub fn core_value_to_linkrs(value: &Value) -> linkrs_value_t {
    match value {
        Value::Null(_) => linkrs_value_t {
            type_: linkrs_value_type_t::GRAPHDB_NULL,
            data: linkrs_value_data_t {
                ptr: std::ptr::null_mut(),
            },
        },
        Value::Bool(b) => linkrs_value_t {
            type_: linkrs_value_type_t::GRAPHDB_BOOL,
            data: linkrs_value_data_t { boolean: *b },
        },
        Value::Int(i) => linkrs_value_t {
            type_: linkrs_value_type_t::GRAPHDB_INT,
            data: linkrs_value_data_t { integer: *i as i64 },
        },
        Value::Float(f) => linkrs_value_t {
            type_: linkrs_value_type_t::GRAPHDB_FLOAT,
            data: linkrs_value_data_t {
                floating: *f as f64,
            },
        },
        Value::String(s) => {
            let string_t = linkrs_string_t {
                data: s.as_ptr() as *const i8,
                len: s.len(),
            };
            linkrs_value_t {
                type_: linkrs_value_type_t::GRAPHDB_STRING,
                data: linkrs_value_data_t { string: string_t },
            }
        }
        _ => linkrs_value_t {
            type_: linkrs_value_type_t::GRAPHDB_NULL,
            data: linkrs_value_data_t {
                ptr: std::ptr::null_mut(),
            },
        },
    }
}

/// C API type to get Core Value
pub fn core_value_to_linkrs_type(value: &Value) -> linkrs_value_type_t {
    match value {
        Value::Null(_) => linkrs_value_type_t::GRAPHDB_NULL,
        Value::Bool(_) => linkrs_value_type_t::GRAPHDB_BOOL,
        Value::Int(_) => linkrs_value_type_t::GRAPHDB_INT,
        Value::Float(_) => linkrs_value_type_t::GRAPHDB_FLOAT,
        Value::String(_) => linkrs_value_type_t::GRAPHDB_STRING,
        Value::List(_) => linkrs_value_type_t::GRAPHDB_LIST,
        Value::Map(_) => linkrs_value_type_t::GRAPHDB_MAP,
        Value::Vertex(_) => linkrs_value_type_t::GRAPHDB_VERTEX,
        Value::Edge(_) => linkrs_value_type_t::GRAPHDB_EDGE,
        Value::Path(_) => linkrs_value_type_t::GRAPHDB_PATH,
        _ => linkrs_value_type_t::GRAPHDB_NULL,
    }
}
