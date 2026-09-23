//! Column Store
//!
//! Columnar storage for vertex properties.
//! Each column stores values of a single property type.
//!
//! The storage is split into two variants:
//! - `FixedWidthColumn`: For fixed-length scalar types (Bool, SmallInt, Int,
//!   BigInt, Float, Double, Date, Time, DateTime, Uuid)
//! - `VariableWidthColumn`: For everything else (String, FixedString, Blob,
//!   Geography, Vector family, Json/JsonB, Interval, Decimal family, Union,
//!   containers, nested composites and graph values), stored as
//!   length-prefixed payloads; complex values use an opaque postcard encoding
//!   with no per-type compression or statistics pruning.
//! - `Column`: Public wrapper that selects the appropriate variant at construction time

pub mod chunk;
pub mod chunk_encoding;
pub mod chunk_residency;
#[allow(clippy::module_inception)]
pub mod column;
pub mod column_store;
pub mod encoding;
pub mod fixed_width;
pub mod mvcc;
pub mod overflow;
pub mod variable_width;
pub mod zone_map;

#[cfg(test)]
mod tests;

pub use chunk::ColumnChunk;
pub use column::{Column, ColumnStorage};
pub use column_store::ColumnStore;
pub use fixed_width::element_size;
pub use zone_map::{compare_values, ZONE_MAP_CHUNK_ROWS};

use graphdb_core::DataType;

/// Returns true if the data type is variable-length.
///
/// Only the ten fixed-width scalar types return false. Every other type,
/// including FixedString, the Decimal family, Union and any future type,
/// returns true so no column can ever be built as a zero-step
/// FixedWidthColumn (element_size 0 would corrupt offsets).
pub fn is_variable_length_type(data_type: &DataType) -> bool {
    !matches!(
        data_type,
        DataType::Bool
            | DataType::SmallInt
            | DataType::Int
            | DataType::BigInt
            | DataType::Float
            | DataType::Double
            | DataType::Date
            | DataType::Time
            | DataType::DateTime
            | DataType::Uuid
    )
}

pub(crate) use column_store::{ensure_bitmap_len, value_payload_bytes};
