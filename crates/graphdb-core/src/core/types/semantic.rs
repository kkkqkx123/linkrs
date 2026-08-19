//! Semantic analysis types shared across binder, planner, and executor.
//!
//! These types represent the intermediate type system used during query binding
//! and semantic analysis. They are defined in core to avoid circular dependencies
//! between the query crate's submodules.

use super::DataType;

/// Value type enumeration for semantic analysis.
///
/// Represents the logical type of an expression during binding,
/// independent of the physical storage type system (`DataType`).
#[derive(Debug, Clone, PartialEq)]
pub enum ValueType {
    Empty,
    Unknown,
    Bool,
    Int,
    Float,
    String,
    Date,
    Time,
    DateTime,
    Vertex,
    Edge,
    Path,
    List,
    Map,
    Set,
    Null,
}

impl ValueType {
    pub fn from_data_type(data_type: &DataType) -> Self {
        match data_type {
            DataType::Bool => ValueType::Bool,
            DataType::SmallInt | DataType::Int | DataType::BigInt => ValueType::Int,
            DataType::Float | DataType::Double => ValueType::Float,
            DataType::String | DataType::FixedString(_) => ValueType::String,
            DataType::Date => ValueType::Date,
            DataType::Time => ValueType::Time,
            DataType::DateTime => ValueType::DateTime,
            DataType::Null => ValueType::Null,
            DataType::Vertex => ValueType::Vertex,
            DataType::Edge => ValueType::Edge,
            DataType::Path => ValueType::Path,
            DataType::List => ValueType::List,
            DataType::Map => ValueType::Map,
            DataType::Set => ValueType::Set,
            // Parameterized composite types are opaque at the semantic level.
            DataType::Struct(_) | DataType::Array(_) => ValueType::Unknown,
            _ => ValueType::Unknown,
        }
    }

    pub fn to_data_type(&self) -> DataType {
        match self {
            ValueType::Empty => DataType::Empty,
            ValueType::Unknown => DataType::Unknown,
            ValueType::Bool => DataType::Bool,
            ValueType::Int => DataType::Int,
            ValueType::Float => DataType::Float,
            ValueType::String => DataType::String,
            ValueType::Date => DataType::Date,
            ValueType::Time => DataType::Time,
            ValueType::DateTime => DataType::DateTime,
            ValueType::Vertex => DataType::Vertex,
            ValueType::Edge => DataType::Edge,
            ValueType::Path => DataType::Path,
            ValueType::List => DataType::List,
            ValueType::Map => DataType::Map,
            ValueType::Set => DataType::Set,
            ValueType::Null => DataType::Null,
        }
    }
}

/// Alias types in graph pattern matching.
///
/// Classifies what a bound variable refers to in a MATCH pattern.
#[derive(Debug, Clone, PartialEq)]
pub enum AliasType {
    Node,
    Edge,
    NodeList,
    EdgeList,
    Path,
    Variable,
    Runtime,
    CTE,
    Expression,
}

/// Column definition for input/output schema description.
#[derive(Debug, Clone)]
pub struct ColumnDef {
    pub name: String,
    pub type_: ValueType,
}
