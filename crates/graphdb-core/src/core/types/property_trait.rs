//! Definition of the basic trait for attribute types
//!
//! This module defines a generic trait related to properties, which is used to abstract the common attributes of PropertyDef, PropertyType, and IndexField.

use crate::core::{DataType, Value};

/// Property type: trait
///
/// Define a common interface for PropertyDef, PropertyType, and IndexField.
pub trait PropertyTypeTrait: Clone + PartialEq + Eq + std::hash::Hash + Send + Sync {
    /// Obtain the attribute name
    fn name(&self) -> &str;

    /// Obtaining the data type
    fn data_type(&self) -> &DataType;

    /// Can it be left empty?
    fn is_nullable(&self) -> bool;

    /// Get the default value (if any).
    fn default_value(&self) -> Option<&Value>;

    /// Obtain the comments (if any).
    fn comment(&self) -> Option<&str>;

    /// Setting the property name
    fn set_name(&mut self, name: String);

    /// Setting data types
    fn set_data_type(&mut self, data_type: DataType);

    /// Set whether the value can be empty.
    fn set_nullable(&mut self, nullable: bool);

    /// Set default values
    fn set_default_value(&mut self, default: Option<Value>);

    /// Set comments
    fn set_comment(&mut self, comment: Option<String>);

    /// Obtain the names of the attribute types (used to distinguish between different types)
    fn property_type_name(&self) -> &'static str;
}
