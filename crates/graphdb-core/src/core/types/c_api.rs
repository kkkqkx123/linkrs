//! C API Core Type Definitions
//!
//! Define value-related data types for C API interoperability.
//! These types are placed in core to avoid core→api and query→api dependencies.

use std::ffi::{c_char, c_void};

/// value type
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum graphdb_value_type_t {
    /// empty value
    GRAPHDB_NULL = 0,
    /// boolean
    GRAPHDB_BOOL = 1,
    /// integer (math.)
    GRAPHDB_INT = 2,
    /// floating point
    GRAPHDB_FLOAT = 3,
    /// string (computer science)
    GRAPHDB_STRING = 4,
    /// listings
    GRAPHDB_LIST = 5,
    /// map (math.)
    GRAPHDB_MAP = 6,
    /// vertice
    GRAPHDB_VERTEX = 7,
    /// suffix of a noun of locality
    GRAPHDB_EDGE = 8,
    /// trails
    GRAPHDB_PATH = 9,
    /// binary data
    GRAPHDB_BLOB = 10,
}

/// binary data structure
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct graphdb_blob_t {
    /// data pointer
    pub data: *const u8,
    /// data length
    pub len: usize,
}

/// string structure
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct graphdb_string_t {
    /// string data
    pub data: *const c_char,
    /// String length
    pub len: usize,
}

/// value structure
#[repr(C)]
#[derive(Clone, Copy)]
pub struct graphdb_value_t {
    /// Value types
    pub type_: graphdb_value_type_t,
    /// value data
    pub data: graphdb_value_data_t,
}

impl std::fmt::Debug for graphdb_value_t {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("graphdb_value_t")
            .field("type_", &self.type_)
            .finish()
    }
}

/// Value Data Consortium
#[repr(C)]
#[derive(Clone, Copy)]
pub union graphdb_value_data_t {
    /// Boolean values
    pub boolean: bool,
    /// Integer
    pub integer: i64,
    /// Floating-point number
    pub floating: f64,
    /// String
    pub string: graphdb_string_t,
    /// Binary data
    pub blob: graphdb_blob_t,
    /// pointer on a gauge
    pub ptr: *mut c_void,
}
