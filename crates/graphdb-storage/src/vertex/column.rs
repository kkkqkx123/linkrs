//! Column Store
//!
//! Columnar storage for vertex properties.
//! Each column stores values of a single property type.
//!
//! The storage is split into two variants:
//! - `FixedWidthColumn`: For fixed-length scalar types (Bool, SmallInt, Int,
//!   BigInt, Float, Double, Date, Time, DateTime, Uuid) plus short
//!   `FixedString(n)` declarations (zero-padded `n`-byte slots) and small
//!   `VectorDense(n)` declarations (fixed `n * 4`-byte little-endian slots).
//! - `VariableWidthColumn`: For everything else (String, wide or zero-width
//!   FixedString, Blob, Geography, wide or unsized vectors, Json/JsonB,
//!   Interval, Decimal family, Union, containers, nested composites and
//!   graph values), stored as length-prefixed payloads; complex values use
//!   an opaque postcard encoding for the raw base. Per-chunk encodings
//!   selected at flush time (dictionary for low-cardinality strings
//!   including wide FixedString, FSST for long strings, plus
//!   RLE/BitPacking/ALP/Constant where applicable), zone maps and HLL
//!   statistics apply on top of the base layout.
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

pub use chunk::{ChunkFlushView, ColumnChunk};
pub use column::{
    BufferLedger, Column, ColumnStorage, EVICTION_SEGMENT_BYTES, MAX_BACKGROUND_LOAD_CHUNKS,
};
pub use column_store::ColumnStore;
pub use fixed_width::{element_size, FIXED_STRING_INLINE_LIMIT, VECTOR_DENSE_FIXED_MAX_DIM};
pub use zone_map::{
    compare_values, complex_key_fp, complex_leaf_range, complex_len, ZONE_MAP_CHUNK_ROWS,
};

use graphdb_core::DataType;

/// Returns true if the data type is variable-length.
///
/// The ten fixed-width scalar types plus short `FixedString(n)`
/// declarations and small `VectorDense(n)` declarations return false.
/// Every other type, including wide or zero-width FixedString, wide or
/// unsized vectors, the Decimal family, Union and any future type,
/// returns true so no column can ever be built as a zero-step
/// FixedWidthColumn (element_size 0 would corrupt offsets).
pub fn is_variable_length_type(data_type: &DataType) -> bool {
    if let DataType::FixedString(n) = data_type {
        return *n == 0 || *n > FIXED_STRING_INLINE_LIMIT;
    }
    if let DataType::VectorDense(dim) = data_type {
        return *dim == 0 || *dim > VECTOR_DENSE_FIXED_MAX_DIM;
    }
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
