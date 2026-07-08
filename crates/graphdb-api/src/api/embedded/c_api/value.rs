use crate::api::embedded::c_api::types::graphdb_value_t;
use crate::core::Value;

/// Convert a C value to a core Value.
///
/// # Safety
/// - `value` must be a valid pointer to a graphdb_value_t
pub unsafe fn graphdb_value_to_core(value: *const graphdb_value_t) -> Value {
    super::query::convert_c_value_to_rust(&*value)
}
