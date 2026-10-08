//! Shared helpers for schema extension handlers.

/// Parse data type string to DataType
///
/// Delegates to the core `DataType::from_str` parser (single source of
/// truth).
pub(crate) fn parse_data_type(type_str: &str) -> Option<linkrs_core::DataType> {
    type_str.parse().ok()
}
