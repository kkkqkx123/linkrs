//! Metadata Type Definitions
//!
//! This module defines the core metadata types used throughout the query planning
//! and execution process.

use graphdb_core::types::DataType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Index metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMetadata {
    pub index_id: u64,
    pub index_name: String,
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub index_type: IndexType,
    /// Whether this index belongs to an edge type (true) or a tag (false).
    pub is_edge: bool,
    pub properties: HashMap<String, Value>,
}

impl IndexMetadata {
    pub fn new(
        index_name: String,
        space_id: u64,
        tag_name: String,
        field_name: String,
        index_type: IndexType,
    ) -> Self {
        Self {
            index_id: 0,
            index_name,
            space_id,
            tag_name,
            field_name,
            index_type,
            is_edge: false,
            properties: HashMap::new(),
        }
    }
}

/// Index type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexType {
    Vector,
    Fulltext,
    Property,
    Composite,
    Native,
}

impl std::fmt::Display for IndexType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexType::Vector => write!(f, "VECTOR"),
            IndexType::Fulltext => write!(f, "FULLTEXT"),
            IndexType::Property => write!(f, "PROPERTY"),
            IndexType::Composite => write!(f, "COMPOSITE"),
            IndexType::Native => write!(f, "NATIVE"),
        }
    }
}

/// Tag metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagMetadata {
    /// Numeric tag id as assigned by the storage schema (`TagInfo.tag_id`).
    /// 0 when the schema manager did not provide one.
    pub tag_id: u32,
    pub tag_name: String,
    pub space_id: u64,
    pub properties: Vec<PropertyDefinition>,
    pub indexes: Vec<String>,
}

impl TagMetadata {
    pub fn new(tag_name: String, space_id: u64) -> Self {
        Self {
            tag_id: 0,
            tag_name,
            space_id,
            properties: Vec::new(),
            indexes: Vec::new(),
        }
    }

    /// Set the numeric tag id (used to wire `IndexScanNode.tag_id`).
    pub fn with_tag_id(mut self, tag_id: u32) -> Self {
        self.tag_id = tag_id;
        self
    }
}

/// Edge type metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeTypeMetadata {
    pub edge_type: String,
    pub space_id: u64,
    pub properties: Vec<PropertyDefinition>,
    pub indexes: Vec<String>,
}

impl EdgeTypeMetadata {
    pub fn new(edge_type: String, space_id: u64) -> Self {
        Self {
            edge_type,
            space_id,
            properties: Vec::new(),
            indexes: Vec::new(),
        }
    }
}

/// Property definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyDefinition {
    pub name: String,
    pub data_type: PropertyType,
    pub nullable: bool,
    pub default_value: Option<Value>,
}

impl PropertyDefinition {
    pub fn new(name: String, data_type: PropertyType) -> Self {
        Self {
            name,
            data_type,
            nullable: true,
            default_value: None,
        }
    }
}

/// Property type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PropertyType {
    Bool,
    Int,
    Float,
    String,
    Vector,
    Date,
    DateTime,
    List,
    Map,
    Vertex,
    Edge,
    Path,
    Geography,
}

impl From<DataType> for PropertyType {
    fn from(dt: DataType) -> Self {
        use DataType as D;
        match dt {
            D::Bool => Self::Bool,
            D::SmallInt | D::Int | D::BigInt => Self::Int,
            D::Float | D::Double | D::Decimal128 => Self::Float,
            D::String | D::FixedString(_) | D::Json | D::JsonB | D::Uuid | D::Blob => Self::String,
            D::Date => Self::Date,
            D::Time | D::DateTime | D::Interval => Self::DateTime,
            D::Vertex => Self::Vertex,
            D::Edge => Self::Edge,
            D::Path => Self::Path,
            D::List(_) | D::Set(_) | D::DataSet => Self::List,
            D::Map(_) => Self::Map,
            D::Geography => Self::Geography,
            D::Vector | D::VectorDense(_) | D::VectorSparse(_) => Self::Vector,
            D::Struct(_) | D::Array(_) => Self::List,
            D::Empty | D::Null | D::Unknown => Self::String,
        }
    }
}

impl std::fmt::Display for PropertyType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PropertyType::Bool => write!(f, "BOOL"),
            PropertyType::Int => write!(f, "INT"),
            PropertyType::Float => write!(f, "FLOAT"),
            PropertyType::String => write!(f, "STRING"),
            PropertyType::Vector => write!(f, "VECTOR"),
            PropertyType::Date => write!(f, "DATE"),
            PropertyType::DateTime => write!(f, "DATETIME"),
            PropertyType::List => write!(f, "LIST"),
            PropertyType::Map => write!(f, "MAP"),
            PropertyType::Vertex => write!(f, "VERTEX"),
            PropertyType::Edge => write!(f, "EDGE"),
            PropertyType::Path => write!(f, "PATH"),
            PropertyType::Geography => write!(f, "GEOGRAPHY"),
        }
    }
}

/// Value type (simplified for metadata)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Null,
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Bool(b) => write!(f, "{}", b),
            Value::Int(i) => write!(f, "{}", i),
            Value::Float(fl) => write!(f, "{}", fl),
            Value::String(s) => write!(f, "{}", s),
            Value::Null => write!(f, "NULL"),
        }
    }
}
