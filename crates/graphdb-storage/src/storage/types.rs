//! Types of data operations at the storage level

use crate::core::types::PropertyDef as CorePropertyDef;
use crate::core::DataType;
use crate::core::Value;

/// Property ID - a compact identifier for properties within a schema.
/// Replaces string-based property lookups with numeric indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PropertyId(pub u16);

impl PropertyId {
    pub const NONE: PropertyId = PropertyId(u16::MAX);

    #[inline]
    pub fn new(id: u16) -> Self {
        Self(id)
    }

    #[inline]
    pub fn as_u16(&self) -> u16 {
        self.0
    }

    #[inline]
    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }

    #[inline]
    pub fn is_none(&self) -> bool {
        self.0 == u16::MAX
    }
}

impl From<u16> for PropertyId {
    #[inline]
    fn from(id: u16) -> Self {
        Self(id)
    }
}

impl From<PropertyId> for u16 {
    #[inline]
    fn from(id: PropertyId) -> Self {
        id.0
    }
}

impl std::fmt::Display for PropertyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "p{}", self.0)
    }
}

/// Edge offset - identifies an edge within a vertex's adjacency list.
/// Replaces the global EdgeId counter with a CSR-native offset-based approach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct EdgeOffset(pub i32);

impl EdgeOffset {
    pub const NONE: EdgeOffset = EdgeOffset(-1);

    #[inline]
    pub fn new(offset: i32) -> Self {
        Self(offset)
    }

    #[inline]
    pub fn as_i32(&self) -> i32 {
        self.0
    }

    #[inline]
    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }

    #[inline]
    pub fn is_none(&self) -> bool {
        self.0 < 0
    }
}

impl From<i32> for EdgeOffset {
    #[inline]
    fn from(offset: i32) -> Self {
        Self(offset)
    }
}

impl From<EdgeOffset> for i32 {
    #[inline]
    fn from(offset: EdgeOffset) -> Self {
        offset.0
    }
}

impl std::fmt::Display for EdgeOffset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "eo{}", self.0)
    }
}

/// Storage-level property definition.
/// Combines features from both vertex and edge property definitions.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoragePropertyDef {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
    pub default_value: Option<Value>,
}

impl StoragePropertyDef {
    pub fn new(name: String, data_type: DataType) -> Self {
        Self {
            name,
            data_type,
            nullable: false,
            default_value: None,
        }
    }

    pub fn from_core(prop: &CorePropertyDef) -> Self {
        Self {
            name: prop.name.clone(),
            data_type: prop.data_type.clone(),
            nullable: prop.nullable,
            default_value: prop.default.clone(),
        }
    }
}

impl From<CorePropertyDef> for StoragePropertyDef {
    fn from(prop: CorePropertyDef) -> Self {
        Self {
            name: prop.name,
            data_type: prop.data_type,
            nullable: prop.nullable,
            default_value: prop.default,
        }
    }
}

impl From<&CorePropertyDef> for StoragePropertyDef {
    fn from(prop: &CorePropertyDef) -> Self {
        Self {
            name: prop.name.clone(),
            data_type: prop.data_type.clone(),
            nullable: prop.nullable,
            default_value: prop.default.clone(),
        }
    }
}
