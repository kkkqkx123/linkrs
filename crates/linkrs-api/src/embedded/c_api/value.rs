use crate::embedded::c_api::types::linkrs_value_t;
use linkrs_core::Value;

/// Convert a C value to a core Value.
///
/// # Safety
/// - `value` must be a valid pointer to a linkrs_value_t
pub unsafe fn linkrs_value_to_core(value: *const linkrs_value_t) -> Value {
    super::query::convert_c_value_to_rust(&*value)
}
