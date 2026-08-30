//! Column Store
//!
//! Columnar storage for vertex properties.
//! Each column stores values of a single property type.
//!
//! The storage is split into two variants:
//! - `FixedWidthColumn`: For fixed-length types (Bool, SmallInt, Int, BigInt, Float, Double, Date, Time, Uuid)
//! - `VariableWidthColumn`: For variable-length types (String)
//! - `Column`: Public wrapper that selects the appropriate variant at construction time

pub mod column;
pub mod column_store;
pub mod encoding;
pub mod fixed_width;
pub mod mvcc;
pub mod variable_width;
pub mod zone_map;

#[cfg(test)]
mod tests;

pub use column::{Column, ColumnStorage};
pub use column_store::ColumnStore;
pub use fixed_width::element_size;
pub use mvcc::{RowVisibility, VersionChainStats, VersionEntry};
pub use zone_map::{compare_values, ZONE_MAP_CHUNK_ROWS};

use graphdb_core::DataType;

/// Returns true if the data type is variable-length.
pub fn is_variable_length_type(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::String
            | DataType::Geography
            | DataType::List(_)
            | DataType::Map(_)
            | DataType::Set(_)
            | DataType::Vertex
            | DataType::Edge
            | DataType::Path
            | DataType::Vector
            | DataType::VectorDense(_)
            | DataType::VectorSparse(_)
            | DataType::DataSet
            | DataType::Json
            | DataType::JsonB
            | DataType::Interval
            | DataType::Null
            // Composite types have no fixed element size; they must never fall
            // into FixedWidthColumn (element_size = 0 would corrupt offsets).
            | DataType::Struct(_)
            | DataType::Array(_)
    )
}

pub(crate) use column_store::{ensure_bitmap_len, value_payload_bytes};
