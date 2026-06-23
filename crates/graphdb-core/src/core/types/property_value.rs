//! Property Value Type
//!
//! Provides a unified property value type for undo/redo operations.

/// Property value type for undo operations
#[derive(Debug, Clone)]
pub enum PropertyValue {
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Bool(bool),
    Null,
}

impl PropertyValue {
    pub fn is_null(&self) -> bool {
        matches!(self, PropertyValue::Null)
    }
}
