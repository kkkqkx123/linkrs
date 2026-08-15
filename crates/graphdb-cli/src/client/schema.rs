//! Schema DDL types

/// Data types supported for properties
#[derive(Debug, Clone)]
pub enum DataType {
    Bool,
    SmallInt,
    Int,
    BigInt,
    Float,
    Double,
    String,
    Date,
    Time,
    DateTime,
    Timestamp,
}

impl std::fmt::Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataType::Bool => write!(f, "BOOL"),
            DataType::SmallInt => write!(f, "SMALLINT"),
            DataType::Int => write!(f, "INT"),
            DataType::BigInt => write!(f, "BIGINT"),
            DataType::Float => write!(f, "FLOAT"),
            DataType::Double => write!(f, "DOUBLE"),
            DataType::String => write!(f, "STRING"),
            DataType::Date => write!(f, "DATE"),
            DataType::Time => write!(f, "TIME"),
            DataType::DateTime => write!(f, "DATETIME"),
            DataType::Timestamp => write!(f, "TIMESTAMP"),
        }
    }
}

/// Property definition for schema creation
#[derive(Debug, Clone)]
pub struct PropertyDef {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
}

impl PropertyDef {
    pub fn new(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            name: name.into(),
            data_type,
            nullable: true,
        }
    }

    pub fn not_null(mut self) -> Self {
        self.nullable = false;
        self
    }
}
