//! C API Value Type Conversion Module
//!
//! Provides conversions between graphdb_value_t and core::Value.

use crate::core::Value;

use crate::core::types::c_api::{
    graphdb_string_t, graphdb_value_data_t, graphdb_value_t, graphdb_value_type_t,
};

/// Converting Core Value to C API Value Types
pub fn core_value_to_graphdb(value: &Value) -> graphdb_value_t {
    match value {
        Value::Null(_) => graphdb_value_t {
            type_: graphdb_value_type_t::GRAPHDB_NULL,
            data: graphdb_value_data_t {
                ptr: std::ptr::null_mut(),
            },
        },
        Value::Bool(b) => graphdb_value_t {
            type_: graphdb_value_type_t::GRAPHDB_BOOL,
            data: graphdb_value_data_t { boolean: *b },
        },
        Value::Int(i) => graphdb_value_t {
            type_: graphdb_value_type_t::GRAPHDB_INT,
            data: graphdb_value_data_t { integer: *i as i64 },
        },
        Value::Float(f) => graphdb_value_t {
            type_: graphdb_value_type_t::GRAPHDB_FLOAT,
            data: graphdb_value_data_t {
                floating: *f as f64,
            },
        },
        Value::String(s) => {
            let string_t = graphdb_string_t {
                data: s.as_ptr() as *const i8,
                len: s.len(),
            };
            graphdb_value_t {
                type_: graphdb_value_type_t::GRAPHDB_STRING,
                data: graphdb_value_data_t { string: string_t },
            }
        }
        _ => graphdb_value_t {
            type_: graphdb_value_type_t::GRAPHDB_NULL,
            data: graphdb_value_data_t {
                ptr: std::ptr::null_mut(),
            },
        },
    }
}

/// C API type to get Core Value
pub fn core_value_to_graphdb_type(value: &Value) -> graphdb_value_type_t {
    match value {
        Value::Null(_) => graphdb_value_type_t::GRAPHDB_NULL,
        Value::Bool(_) => graphdb_value_type_t::GRAPHDB_BOOL,
        Value::Int(_) => graphdb_value_type_t::GRAPHDB_INT,
        Value::Float(_) => graphdb_value_type_t::GRAPHDB_FLOAT,
        Value::String(_) => graphdb_value_type_t::GRAPHDB_STRING,
        Value::List(_) => graphdb_value_type_t::GRAPHDB_LIST,
        Value::Map(_) => graphdb_value_type_t::GRAPHDB_MAP,
        Value::Vertex(_) => graphdb_value_type_t::GRAPHDB_VERTEX,
        Value::Edge(_) => graphdb_value_type_t::GRAPHDB_EDGE,
        Value::Path(_) => graphdb_value_type_t::GRAPHDB_PATH,
        _ => graphdb_value_type_t::GRAPHDB_NULL,
    }
}
