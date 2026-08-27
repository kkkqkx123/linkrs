//! Null Type Definition
//!
//! Nebula-Graph-compatible null value type definition.

use serde::{Deserialize, Serialize};
use std::hash::Hash;

/// Null Type Definition
///
/// Nebula-Graph-compatible null value type definition with the following variants:
/// - **Null**: standard null value
/// - **NaN**: Non-numeric results
/// - **BadData**: Bad data (e.g., wrong date format)
/// - **BadType**: type mismatch error
/// - **ErrOverflow**: Numeric overflow error
/// - **UnknownProp**: Unknown property
/// - **DivByZero**: divide by zero error
/// - **OutOfRange**: value out of range
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum NullType {
    #[default]
    Null, // Standard null values
    NaN,         // Non-numeric results
    BadData,     // Bad data (parsing failure)
    BadType,     // Type mismatch
    ErrOverflow, // Numeric overflow
    UnknownProp, // Unknown property
    DivByZero,   // Division error
    OutOfRange,  // Value out of range
}

impl NullType {
    pub fn is_bad(&self) -> bool {
        matches!(
            self,
            NullType::BadData | NullType::BadType | NullType::ErrOverflow | NullType::OutOfRange
        )
    }

    pub fn is_computational_error(&self) -> bool {
        matches!(
            self,
            NullType::NaN | NullType::DivByZero | NullType::ErrOverflow
        )
    }

    pub fn to_string(&self) -> &str {
        match self {
            NullType::Null => "NULL",
            NullType::NaN => "NaN",
            NullType::BadData => "BAD_DATA",
            NullType::BadType => "BAD_TYPE",
            NullType::ErrOverflow => "ERR_OVERFLOW",
            NullType::UnknownProp => "UNKNOWN_PROP",
            NullType::DivByZero => "DIV_BY_ZERO",
            NullType::OutOfRange => "OUT_OF_RANGE",
        }
    }
}

impl std::fmt::Display for NullType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}
