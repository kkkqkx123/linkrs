//! Schema type base trait definition
//!
//! This module defines generic Schema-related traits that abstract the common properties of TagInfo and EdgeTypeInfo.

use super::property::PropertyDef;

/// Schema information trait
///
/// Define a common interface for TagInfo and EdgeTypeInfo.
pub trait SchemaInfo: Clone + PartialEq + Eq + std::hash::Hash + Send + Sync {
    /// Get Schema ID
    fn schema_id(&self) -> u32;

    /// Get Schema Name
    fn schema_name(&self) -> &str;

    /// Getting a list of properties
    fn properties(&self) -> &[PropertyDef];

    /// Get Annotations
    fn comment(&self) -> Option<&str>;

    /// Get TTL duration
    fn ttl_duration(&self) -> Option<i64>;

    /// Get TTL column names
    fn ttl_col(&self) -> Option<&str>;

    /// Setting the Schema ID
    fn set_schema_id(&mut self, id: u32);

    /// Setting the property list
    fn set_properties(&mut self, properties: Vec<PropertyDef>);

    /// Setting up comments
    fn set_comment(&mut self, comment: Option<String>);

    /// Setting the TTL
    fn set_ttl(&mut self, duration: Option<i64>, col: Option<String>);

    /// Get the Schema type name (to distinguish between Tag or Edge)
    fn schema_type_name(&self) -> &'static str;

    /// Tag type or not
    fn is_tag(&self) -> bool;

    /// Edge type or not
    fn is_edge(&self) -> bool;
}
