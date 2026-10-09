//! C API Core Type Definitions
//!
//! Define value-related data types for C API interoperability.
//! These types are placed in core to avoid core→api and query→api dependencies.

use std::ffi::{c_char, c_void};

/// Tag describing which payload a value carries.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum linkrs_value_type_t {
    /// Null / empty value
    LINKRS_NULL = 0,
    /// Boolean
    LINKRS_BOOL = 1,
    /// 64-bit signed integer
    LINKRS_INT = 2,
    /// Double-precision floating point
    LINKRS_FLOAT = 3,
    /// UTF-8 string
    LINKRS_STRING = 4,
    /// List
    LINKRS_LIST = 5,
    /// Map / dictionary
    LINKRS_MAP = 6,
    /// Vertex
    LINKRS_VERTEX = 7,
    /// Edge
    LINKRS_EDGE = 8,
    /// Path
    LINKRS_PATH = 9,
    /// Binary blob
    LINKRS_BLOB = 10,
}

/// Borrowed binary blob view; the data pointer is not owned by this struct.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct linkrs_blob_t {
    /// Pointer to the first byte
    pub data: *const u8,
    /// Length in bytes
    pub len: usize,
}

/// Borrowed string view; the data pointer is not owned by this struct.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct linkrs_string_t {
    /// Pointer to UTF-8 bytes (not null-terminated; length lives in `len`)
    pub data: *const c_char,
    /// Length in bytes
    pub len: usize,
}

/// Value: a type tag plus its payload.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct linkrs_value_t {
    /// Type tag of this value
    pub type_: linkrs_value_type_t,
    /// Payload of this value
    pub data: linkrs_value_data_t,
}

impl std::fmt::Debug for linkrs_value_t {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("linkrs_value_t")
            .field("type_", &self.type_)
            .finish()
    }
}

/// Payload union; active member is selected by the value type tag.
#[repr(C)]
#[derive(Clone, Copy)]
pub union linkrs_value_data_t {
    /// Boolean payload
    pub boolean: bool,
    /// Integer payload
    pub integer: i64,
    /// Floating-point payload
    pub floating: f64,
    /// String payload (borrowed view)
    pub string: linkrs_string_t,
    /// Binary blob payload (borrowed view)
    pub blob: linkrs_blob_t,
    /// Opaque pointer payload
    pub ptr: *mut c_void,
}
