//! Index Type Definition Module
//!
//! Provide a unified index type definition, including index state, type, structure, etc.

use super::property_trait::PropertyTypeTrait;
use crate::core::Value;
use serde::{Deserialize, Serialize};

/// Comparison operator for partial index conditions
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ComparisonOperator {
    #[serde(rename = "eq")]
    Equal,
    #[serde(rename = "ne")]
    NotEqual,
    #[serde(rename = "gt")]
    GreaterThan,
    #[serde(rename = "gte")]
    GreaterThanOrEqual,
    #[serde(rename = "lt")]
    LessThan,
    #[serde(rename = "lte")]
    LessThanOrEqual,
    #[serde(rename = "in")]
    In,
    #[serde(rename = "not_in")]
    NotIn,
    #[serde(rename = "is_null")]
    IsNull,
    #[serde(rename = "is_not_null")]
    IsNotNull,
}

/// Condition for partial index filtering
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PartialIndexCondition {
    pub field: String,
    pub operator: ComparisonOperator,
    pub value: Value,
}

impl PartialIndexCondition {
    pub fn new(field: String, operator: ComparisonOperator, value: Value) -> Self {
        Self {
            field,
            operator,
            value,
        }
    }

    pub fn eq(field: impl Into<String>, value: Value) -> Self {
        Self::new(field.into(), ComparisonOperator::Equal, value)
    }

    pub fn ne(field: impl Into<String>, value: Value) -> Self {
        Self::new(field.into(), ComparisonOperator::NotEqual, value)
    }

    pub fn gt(field: impl Into<String>, value: Value) -> Self {
        Self::new(field.into(), ComparisonOperator::GreaterThan, value)
    }

    pub fn gte(field: impl Into<String>, value: Value) -> Self {
        Self::new(field.into(), ComparisonOperator::GreaterThanOrEqual, value)
    }

    pub fn lt(field: impl Into<String>, value: Value) -> Self {
        Self::new(field.into(), ComparisonOperator::LessThan, value)
    }

    pub fn lte(field: impl Into<String>, value: Value) -> Self {
        Self::new(field.into(), ComparisonOperator::LessThanOrEqual, value)
    }

    pub fn is_null(field: impl Into<String>) -> Self {
        Self::new(
            field.into(),
            ComparisonOperator::IsNull,
            Value::Null(crate::core::NullType::Null),
        )
    }

    pub fn is_not_null(field: impl Into<String>) -> Self {
        Self::new(
            field.into(),
            ComparisonOperator::IsNotNull,
            Value::Null(crate::core::NullType::Null),
        )
    }

    /// Evaluate the condition against a value
    pub fn evaluate(&self, value: &Value) -> bool {
        match &self.operator {
            ComparisonOperator::Equal => value == &self.value,
            ComparisonOperator::NotEqual => value != &self.value,
            ComparisonOperator::IsNull => matches!(value, Value::Null(_)),
            ComparisonOperator::IsNotNull => !matches!(value, Value::Null(_)),
            ComparisonOperator::GreaterThan => match (value, &self.value) {
                (Value::Int(a), Value::Int(b)) => a > b,
                (Value::Float(a), Value::Float(b)) => a > b,
                (Value::String(a), Value::String(b)) => a > b,
                _ => false,
            },
            ComparisonOperator::GreaterThanOrEqual => match (value, &self.value) {
                (Value::Int(a), Value::Int(b)) => a >= b,
                (Value::Float(a), Value::Float(b)) => a >= b,
                (Value::String(a), Value::String(b)) => a >= b,
                _ => false,
            },
            ComparisonOperator::LessThan => match (value, &self.value) {
                (Value::Int(a), Value::Int(b)) => a < b,
                (Value::Float(a), Value::Float(b)) => a < b,
                (Value::String(a), Value::String(b)) => a < b,
                _ => false,
            },
            ComparisonOperator::LessThanOrEqual => match (value, &self.value) {
                (Value::Int(a), Value::Int(b)) => a <= b,
                (Value::Float(a), Value::Float(b)) => a <= b,
                (Value::String(a), Value::String(b)) => a <= b,
                _ => false,
            },
            ComparisonOperator::In | ComparisonOperator::NotIn => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexStatus {
    #[serde(rename = "creating")]
    Creating,
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "dropped")]
    Dropped,
    #[serde(rename = "failed")]
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexType {
    #[serde(rename = "tag")]
    TagIndex,
    #[serde(rename = "edge")]
    EdgeIndex,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IndexField {
    pub name: String,
    pub value_type: Value,
    pub is_nullable: bool,
}

impl PropertyTypeTrait for IndexField {
    fn name(&self) -> &str {
        &self.name
    }

    fn data_type(&self) -> &crate::core::DataType {
        match &self.value_type {
            Value::Int(_) => &crate::core::DataType::Int,
            Value::Float(_) => &crate::core::DataType::Float,
            Value::Bool(_) => &crate::core::DataType::Bool,
            Value::String(_) => &crate::core::DataType::String,
            Value::Null(_) => &crate::core::DataType::Null,
            _ => &crate::core::DataType::String,
        }
    }

    fn is_nullable(&self) -> bool {
        self.is_nullable
    }

    fn default_value(&self) -> Option<&Value> {
        None
    }

    fn comment(&self) -> Option<&str> {
        None
    }

    fn set_name(&mut self, name: String) {
        self.name = name;
    }

    fn set_data_type(&mut self, _data_type: crate::core::DataType) {}

    fn set_nullable(&mut self, nullable: bool) {
        self.is_nullable = nullable;
    }

    fn set_default_value(&mut self, _default: Option<Value>) {}

    fn set_comment(&mut self, _comment: Option<String>) {}

    fn property_type_name(&self) -> &'static str {
        "IndexField"
    }
}

impl IndexField {
    pub fn new(name: String, value_type: Value, is_nullable: bool) -> Self {
        Self {
            name,
            value_type,
            is_nullable,
        }
    }
}

/// Indexed Configuration Structures
#[derive(Debug, Clone)]
pub struct IndexConfig {
    pub id: i32,
    pub name: String,
    pub space_id: u64,
    pub schema_name: String,
    pub fields: Vec<IndexField>,
    pub properties: Vec<String>,
    pub index_type: IndexType,
    pub is_unique: bool,
    /// Optional condition for partial index
    pub partial_condition: Option<PartialIndexCondition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    pub id: i32,
    pub name: String,
    pub space_id: u64,
    pub schema_name: String,
    pub fields: Vec<IndexField>,
    pub properties: Vec<String>,
    pub index_type: IndexType,
    pub status: IndexStatus,
    pub is_unique: bool,
    pub comment: Option<String>,
    /// Optional condition for partial index
    pub partial_condition: Option<PartialIndexCondition>,
}

impl Index {
    /// Creating an Index Using a Configuration Structure
    pub fn new(config: IndexConfig) -> Self {
        Self {
            id: config.id,
            name: config.name,
            space_id: config.space_id,
            schema_name: config.schema_name,
            fields: config.fields,
            properties: config.properties,
            index_type: config.index_type,
            status: IndexStatus::Active,
            is_unique: config.is_unique,
            comment: None,
            partial_condition: config.partial_condition,
        }
    }

    /// Check if this is a partial index
    pub fn is_partial(&self) -> bool {
        self.partial_condition.is_some()
    }

    /// Check if a value should be indexed based on partial condition
    pub fn should_index(&self, field_values: &std::collections::HashMap<String, Value>) -> bool {
        match &self.partial_condition {
            Some(condition) => field_values
                .get(&condition.field)
                .map(|v| condition.evaluate(v))
                .unwrap_or(false),
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_status_serialization() {
        let active = IndexStatus::Active;
        let failed = IndexStatus::Failed("error".to_string());

        let active_json = serde_json::to_string(&active)
            .expect("Failed to serialize IndexStatus::Active in test");
        let failed_json = serde_json::to_string(&failed)
            .expect("Failed to serialize IndexStatus::Failed in test");

        assert!(active_json.contains("active"));
        assert!(failed_json.contains("failed"));
        assert!(failed_json.contains("error"));
    }

    #[test]
    fn test_index_type_serialization() {
        let tag = IndexType::TagIndex;
        let edge = IndexType::EdgeIndex;

        let tag_json =
            serde_json::to_string(&tag).expect("Failed to serialize IndexType::TagIndex in test");
        let edge_json =
            serde_json::to_string(&edge).expect("Failed to serialize IndexType::EdgeIndex in test");

        assert!(tag_json.contains("tag"));
        assert!(edge_json.contains("edge"));
    }

    #[test]
    fn test_index_field_creation() {
        let field = IndexField::new(
            "name".to_string(),
            Value::String("string".to_string()),
            false,
        );

        assert_eq!(field.name, "name");
        assert!(matches!(field.value_type, Value::String(_)));
        assert!(!field.is_nullable);
    }

    #[test]
    fn test_index_creation() {
        let fields = vec![IndexField::new(
            "name".to_string(),
            Value::String("string".to_string()),
            false,
        )];

        let config = IndexConfig {
            id: 1,
            name: "person_name_idx".to_string(),
            space_id: 1,
            schema_name: "person".to_string(),
            fields,
            properties: vec![],
            index_type: IndexType::TagIndex,
            is_unique: false,
            partial_condition: None,
        };

        let index = Index::new(config);

        assert_eq!(index.id, 1);
        assert_eq!(index.name, "person_name_idx");
        assert_eq!(index.schema_name, "person");
        assert_eq!(index.fields.len(), 1);
        assert_eq!(index.status, IndexStatus::Active);
    }
}

// ============================================================================
// Full-Text Index Types
// ============================================================================

/// Full-text index engine type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FulltextEngineType {
    #[serde(rename = "bm25")]
    Bm25,
}

impl std::fmt::Display for FulltextEngineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FulltextEngineType::Bm25 => write!(f, "BM25"),
        }
    }
}

/// BM25 index configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BM25IndexConfig {
    /// BM25 parameter k1 - controls term frequency saturation (default: 1.2)
    pub k1: f32,
    /// BM25 parameter b - controls length normalization (default: 0.75)
    pub b: f32,
    /// Field weights
    pub field_weights: std::collections::HashMap<String, f32>,
    /// Analyzer name
    pub analyzer: String,
    /// Whether to store original text
    pub store_original: bool,
}

impl Default for BM25IndexConfig {
    fn default() -> Self {
        Self {
            k1: 1.2,
            b: 0.75,
            field_weights: std::collections::HashMap::new(),
            analyzer: "standard".to_string(),
            store_original: true,
        }
    }
}

/// Full-text index field configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextIndexField {
    pub field_name: String,
    pub analyzer: Option<String>,
    pub boost: f32,
    pub stored: bool,
    pub indexed: bool,
}

impl FulltextIndexField {
    pub fn new(field_name: String) -> Self {
        Self {
            field_name,
            analyzer: None,
            boost: 1.0,
            stored: true,
            indexed: true,
        }
    }

    pub fn with_boost(mut self, boost: f32) -> Self {
        self.boost = boost;
        self
    }

    pub fn with_analyzer(mut self, analyzer: String) -> Self {
        self.analyzer = Some(analyzer);
        self
    }
}

/// Full-text index options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextIndexOptions {
    pub engine_type: FulltextEngineType,
    pub bm25_config: Option<BM25IndexConfig>,
    pub fields: Vec<FulltextIndexField>,
    pub if_not_exists: bool,
}

impl Default for FulltextIndexOptions {
    fn default() -> Self {
        Self {
            engine_type: FulltextEngineType::Bm25,
            bm25_config: Some(BM25IndexConfig::default()),
            fields: Vec::new(),
            if_not_exists: false,
        }
    }
}

impl FulltextIndexOptions {
    pub fn new(engine_type: FulltextEngineType) -> Self {
        Self {
            engine_type,
            bm25_config: Some(BM25IndexConfig::default()),
            fields: Vec::new(),
            if_not_exists: false,
        }
    }

    pub fn bm25() -> Self {
        Self::new(FulltextEngineType::Bm25)
    }
}
