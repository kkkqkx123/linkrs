//! Type Signature System
//!
//! Define an enumeration of value types used in function signatures, for type checking and function overloading resolution.

use crate::core::Value;
use std::fmt;

/// Value type enumeration (used in function signatures)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ValueType {
    Null,
    Bool,
    SmallInt,
    Int,
    BigInt,
    Float,
    Double,
    Decimal128,
    String,
    FixedString,
    Blob,
    Date,
    Time,
    DateTime,
    Vertex,
    Edge,
    Path,
    List,
    Map,
    Set,
    Geography,
    DataSet,
    Vector,
    Json,
    JsonB,
    Uuid,
    Interval,
    Empty,
    Any,
}

impl ValueType {
    pub fn from_value(value: &Value) -> Self {
        match value {
            Value::Null(_) => ValueType::Null,
            Value::Bool(_) => ValueType::Bool,
            Value::SmallInt(_) => ValueType::SmallInt,
            Value::Int(_) => ValueType::Int,
            Value::BigInt(_) => ValueType::BigInt,
            Value::Float(_) => ValueType::Float,
            Value::Double(_) => ValueType::Double,
            Value::Decimal128(_) => ValueType::Decimal128,
            Value::String(_) => ValueType::String,
            Value::FixedString { .. } => ValueType::FixedString,
            Value::Blob(_) => ValueType::Blob,
            Value::Date(_) => ValueType::Date,
            Value::Time(_) => ValueType::Time,
            Value::DateTime(_) => ValueType::DateTime,
            Value::Vertex(_) => ValueType::Vertex,
            Value::Edge(_) => ValueType::Edge,
            Value::Path(_) => ValueType::Path,
            Value::List(_) => ValueType::List,
            Value::Map(_) => ValueType::Map,
            Value::Set(_) => ValueType::Set,
            Value::Geography(_) => ValueType::Geography,
            Value::DataSet(_) => ValueType::DataSet,
            Value::Vector(_) => ValueType::Vector,
            Value::Json(_) => ValueType::Json,
            Value::JsonB(_) => ValueType::JsonB,
            Value::Uuid(_) => ValueType::Uuid,
            Value::Interval(_) => ValueType::Interval,
            Value::Empty => ValueType::Empty,
        }
    }

    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            ValueType::SmallInt
                | ValueType::Int
                | ValueType::BigInt
                | ValueType::Float
                | ValueType::Double
                | ValueType::Decimal128
        )
    }

    pub fn is_string(&self) -> bool {
        matches!(self, ValueType::String)
    }

    pub fn is_collection(&self) -> bool {
        matches!(self, ValueType::List | ValueType::Map | ValueType::Set)
    }

    pub fn compatible_with(&self, other: &ValueType) -> bool {
        self == &ValueType::Empty || other == &ValueType::Empty || self == other
    }
}

impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValueType::Null => write!(f, "NULL"),
            ValueType::Bool => write!(f, "BOOL"),
            ValueType::SmallInt => write!(f, "SMALLINT"),
            ValueType::Int => write!(f, "INT"),
            ValueType::BigInt => write!(f, "BIGINT"),
            ValueType::Float => write!(f, "FLOAT"),
            ValueType::Double => write!(f, "DOUBLE"),
            ValueType::Decimal128 => write!(f, "DECIMAL128"),
            ValueType::String => write!(f, "STRING"),
            ValueType::FixedString => write!(f, "FIXED_STRING"),
            ValueType::Blob => write!(f, "BLOB"),
            ValueType::Date => write!(f, "DATE"),
            ValueType::Time => write!(f, "TIME"),
            ValueType::DateTime => write!(f, "DATETIME"),
            ValueType::Vertex => write!(f, "VERTEX"),
            ValueType::Edge => write!(f, "EDGE"),
            ValueType::Path => write!(f, "PATH"),
            ValueType::List => write!(f, "LIST"),
            ValueType::Map => write!(f, "MAP"),
            ValueType::Set => write!(f, "SET"),
            ValueType::Geography => write!(f, "GEOGRAPHY"),
            ValueType::DataSet => write!(f, "DATASET"),
            ValueType::Vector => write!(f, "VECTOR"),
            ValueType::Json => write!(f, "JSON"),
            ValueType::JsonB => write!(f, "JSONB"),
            ValueType::Uuid => write!(f, "UUID"),
            ValueType::Interval => write!(f, "INTERVAL"),
            ValueType::Empty => write!(f, "EMPTY"),
            ValueType::Any => write!(f, "ANY"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FunctionSignature {
    pub name: &'static str,
    pub param_types: Vec<ValueType>,
    pub return_type: Option<ValueType>,
    pub is_variadic: bool,
}

impl FunctionSignature {
    pub fn new(
        name: &'static str,
        param_types: Vec<ValueType>,
        return_type: Option<ValueType>,
        is_variadic: bool,
    ) -> Self {
        Self {
            name,
            param_types,
            return_type,
            is_variadic,
        }
    }
}
