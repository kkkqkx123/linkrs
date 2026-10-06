//! Shared helpers for public schema handlers.

use graphdb_core::DataType;

// ==================== Auxiliary Functions ====================

pub(crate) fn parse_data_type(type_str: &str) -> DataType {
    // The wire `data_type` is the core `DataType` Display output; parse it
    // back through the same `FromStr` source of truth. Unrecognized types
    // fall back to String (previous behavior).
    type_str.parse::<DataType>().unwrap_or(DataType::String)
}
