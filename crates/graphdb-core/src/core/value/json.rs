//! JSON/JSONB Types - Graph Database JSON Support
//!
//! This module provides JSON and JSONB types for storing semi-structured data.
//!
//! ## JSON vs JSONB
//!
//! - JSON: Text format, preserves original formatting (whitespace, key order)
//! - JSONB: Binary format, parsed and normalized for better query performance

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::hash::{Hash, Hasher};

/// JSON Error Type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonError {
    InvalidJson(String),
    InvalidPath(String),
    TypeMismatch { expected: String, actual: String },
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JsonError::InvalidJson(msg) => write!(f, "Invalid JSON: {}", msg),
            JsonError::InvalidPath(path) => write!(f, "Invalid JSON path: {}", path),
            JsonError::TypeMismatch { expected, actual } => {
                write!(f, "Type mismatch: expected {}, got {}", expected, actual)
            }
        }
    }
}

impl std::error::Error for JsonError {}

/// JSON Type - Text Storage Format
///
/// Features:
/// - Preserves original text format (including whitespace, key order)
/// - No parsing required on write, better write performance
/// - Parsing required on query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Json {
    /// Original JSON text
    text: String,
    /// Cached parse result (optional, for improved query performance)
    #[serde(skip)]
    cached_value: Option<JsonValue>,
}

/// JSONB Type - Binary Storage Format
///
/// Features:
/// - Stores parsed binary format
/// - Parsing and validation required on write
/// - Better query performance
/// - Supports GIN index creation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonB {
    value: JsonValue,
}

impl Json {
    /// Create JSON from string
    pub fn parse(text: &str) -> Result<Self, JsonError> {
        // Optional: Validate JSON format
        let value: JsonValue =
            serde_json::from_str(text).map_err(|e| JsonError::InvalidJson(e.to_string()))?;

        Ok(Self {
            text: text.to_string(),
            cached_value: Some(value),
        })
    }

    /// Create from JsonValue
    pub fn from_value(value: JsonValue) -> Self {
        let text = value.to_string();
        Self {
            text,
            cached_value: Some(value),
        }
    }

    /// Get raw text
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Get parsed value (with caching)
    pub fn to_value(&self) -> Result<JsonValue, JsonError> {
        if let Some(ref value) = self.cached_value {
            return Ok(value.clone());
        }

        serde_json::from_str(&self.text).map_err(|e| JsonError::InvalidJson(e.to_string()))
    }

    /// Get value at specified path
    /// Path format: "key1.key2[0].key3"
    pub fn get_path(&self, path: &str) -> Result<Option<JsonValue>, JsonError> {
        let value = self.to_value()?;
        Ok(get_json_path(&value, path))
    }

    /// Convert to JsonB
    pub fn to_jsonb(&self) -> Result<JsonB, JsonError> {
        let value = self.to_value()?;
        Ok(JsonB::from_value(value))
    }

    /// Extract JSON path value, return new Json
    pub fn extract(&self, path: &str) -> Result<Json, JsonError> {
        let json_value = self.get_path(path)?;
        Ok(Json::from_value(json_value.unwrap_or(JsonValue::Null)))
    }

    /// Estimate memory usage
    pub fn estimated_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.text.capacity()
    }
}

impl PartialEq for Json {
    fn eq(&self, other: &Self) -> bool {
        // Compare parsed values, not text
        match (self.to_value(), other.to_value()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Json {}

impl Hash for Json {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Use parsed value for hashing
        if let Ok(value) = self.to_value() {
            hash_json_value(&value, state);
        }
    }
}

// Json and JsonB use serde for serialization, postcard will use serde integration

impl JsonB {
    /// Create JSONB from string (must validate)
    pub fn parse(text: &str) -> Result<Self, JsonError> {
        let value: JsonValue =
            serde_json::from_str(text).map_err(|e| JsonError::InvalidJson(e.to_string()))?;

        Ok(Self::from_value(value))
    }

    /// Create from JsonValue
    pub fn from_value(value: JsonValue) -> Self {
        // Normalize JSON (sort keys, remove whitespace)
        let normalized = normalize_json(value);

        Self { value: normalized }
    }

    /// Get parsed value
    pub fn as_value(&self) -> &JsonValue {
        &self.value
    }

    /// Convert to text format
    pub fn to_json_string(&self) -> String {
        self.value.to_string()
    }

    /// Get value at specified path
    pub fn get_path(&self, path: &str) -> Option<&JsonValue> {
        get_json_path_ref(&self.value, path)
    }

    /// Convert to Json
    pub fn to_json(&self) -> Json {
        Json::from_value(self.value.clone())
    }

    /// Check if contains specified key
    pub fn contains_key(&self, key: &str) -> bool {
        matches!(self.value, JsonValue::Object(ref map) if map.contains_key(key))
    }

    /// Get object key count
    pub fn key_count(&self) -> usize {
        match self.value {
            JsonValue::Object(ref map) => map.len(),
            _ => 0,
        }
    }

    /// Get array length
    pub fn array_len(&self) -> Option<usize> {
        match self.value {
            JsonValue::Array(ref arr) => Some(arr.len()),
            _ => None,
        }
    }

    /// Extract JSON path value, return new JsonB
    pub fn extract(&self, path: &str) -> JsonB {
        let json_value = self.get_path(path).cloned().unwrap_or(JsonValue::Null);
        JsonB::from_value(json_value)
    }

    /// Estimate memory usage
    pub fn estimated_size(&self) -> usize {
        std::mem::size_of::<Self>() + estimate_json_size(&self.value)
    }
}

impl PartialEq for JsonB {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl Eq for JsonB {}

impl Hash for JsonB {
    fn hash<H: Hasher>(&self, state: &mut H) {
        hash_json_value(&self.value, state);
    }
}

impl Ord for JsonB {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // JSONB comparison rules: by type priority, then by value
        compare_json_values(&self.value, &other.value)
    }
}

impl PartialOrd for JsonB {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// Json and JsonB use serde for serialization, postcard will use serde integration

/// Normalize JSON value (for JSONB)
fn normalize_json(value: JsonValue) -> JsonValue {
    match value {
        JsonValue::Object(map) => {
            // Sort keys and recursively normalize
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let normalized: serde_json::Map<String, JsonValue> = entries
                .into_iter()
                .map(|(k, v)| (k, normalize_json(v)))
                .collect();
            JsonValue::Object(normalized)
        }
        JsonValue::Array(arr) => JsonValue::Array(arr.into_iter().map(normalize_json).collect()),
        // Other types remain unchanged
        other => other,
    }
}

/// Get JSON path value
fn get_json_path(value: &JsonValue, path: &str) -> Option<JsonValue> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = value;

    for part in parts {
        // Handle array index, e.g., "items[0]"
        if let Some(idx_start) = part.find('[') {
            let key = &part[..idx_start];
            let idx_end = part.find(']').unwrap_or(part.len());
            let idx: usize = part[idx_start + 1..idx_end].parse().ok()?;

            if !key.is_empty() {
                current = current.get(key)?;
            }
            current = current.get(idx)?;
        } else {
            current = current.get(part)?;
        }
    }

    Some(current.clone())
}

/// Get JSON path value (reference version)
fn get_json_path_ref<'a>(value: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = value;

    for part in parts {
        if let Some(idx_start) = part.find('[') {
            let key = &part[..idx_start];
            let idx_end = part.find(']').unwrap_or(part.len());
            let idx: usize = part[idx_start + 1..idx_end].parse().ok()?;

            if !key.is_empty() {
                current = current.get(key)?;
            }
            current = current.get(idx)?;
        } else {
            current = current.get(part)?;
        }
    }

    Some(current)
}

/// Hash JSON value
fn hash_json_value<H: Hasher>(value: &JsonValue, state: &mut H) {
    match value {
        JsonValue::Null => 0u8.hash(state),
        JsonValue::Bool(b) => {
            1u8.hash(state);
            b.hash(state);
        }
        JsonValue::Number(n) => {
            2u8.hash(state);
            n.to_string().hash(state);
        }
        JsonValue::String(s) => {
            3u8.hash(state);
            s.hash(state);
        }
        JsonValue::Array(arr) => {
            4u8.hash(state);
            for item in arr {
                hash_json_value(item, state);
            }
        }
        JsonValue::Object(map) => {
            5u8.hash(state);
            for (k, v) in map {
                k.hash(state);
                hash_json_value(v, state);
            }
        }
    }
}

/// Compare two JSON values
fn compare_json_values(a: &JsonValue, b: &JsonValue) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    // Type priority: Null < Bool < Number < String < Array < Object
    let type_priority = |v: &JsonValue| match v {
        JsonValue::Null => 0,
        JsonValue::Bool(_) => 1,
        JsonValue::Number(_) => 2,
        JsonValue::String(_) => 3,
        JsonValue::Array(_) => 4,
        JsonValue::Object(_) => 5,
    };

    let priority_a = type_priority(a);
    let priority_b = type_priority(b);

    match priority_a.cmp(&priority_b) {
        Ordering::Equal => match (a, b) {
            (JsonValue::Null, JsonValue::Null) => Ordering::Equal,
            (JsonValue::Bool(a), JsonValue::Bool(b)) => a.cmp(b),
            (JsonValue::Number(a), JsonValue::Number(b)) => {
                // Try comparing as integers, otherwise as floats
                if let (Some(a_i), Some(b_i)) = (a.as_i64(), b.as_i64()) {
                    a_i.cmp(&b_i)
                } else if let (Some(a_f), Some(b_f)) = (a.as_f64(), b.as_f64()) {
                    a_f.partial_cmp(&b_f).unwrap_or(Ordering::Equal)
                } else {
                    a.to_string().cmp(&b.to_string())
                }
            }
            (JsonValue::String(a), JsonValue::String(b)) => a.cmp(b),
            (JsonValue::Array(a), JsonValue::Array(b)) => a
                .iter()
                .zip(b.iter())
                .map(|(x, y)| compare_json_values(x, y))
                .find(|&ord| ord != Ordering::Equal)
                .unwrap_or_else(|| a.len().cmp(&b.len())),
            (JsonValue::Object(a), JsonValue::Object(b)) => {
                let a_keys: Vec<_> = a.keys().collect();
                let b_keys: Vec<_> = b.keys().collect();

                match a_keys.cmp(&b_keys) {
                    Ordering::Equal => a_keys
                        .iter()
                        .map(|k| compare_json_values(&a[*k], &b[*k]))
                        .find(|&ord| ord != Ordering::Equal)
                        .unwrap_or(Ordering::Equal),
                    other => other,
                }
            }
            _ => Ordering::Equal, // Different types already handled by priority
        },
        other => other,
    }
}

/// Estimate JSON value memory usage
fn estimate_json_size(value: &JsonValue) -> usize {
    match value {
        JsonValue::Null => 0,
        JsonValue::Bool(_) => 1,
        JsonValue::Number(n) => n.to_string().len(),
        JsonValue::String(s) => s.len(),
        JsonValue::Array(arr) => {
            arr.iter().map(estimate_json_size).sum::<usize>()
                + arr.len() * std::mem::size_of::<JsonValue>()
        }
        JsonValue::Object(map) => {
            map.iter()
                .map(|(k, v)| k.len() + estimate_json_size(v))
                .sum::<usize>()
                + map.len() * std::mem::size_of::<(String, JsonValue)>()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_from_str() {
        let json = Json::parse(r#"{"name": "test", "value": 123}"#).unwrap();
        assert!(json.get_path("name").unwrap().is_some());
        assert!(json.get_path("value").unwrap().is_some());
    }

    #[test]
    fn test_jsonb_from_str() {
        let jsonb = JsonB::parse(r#"{"name": "test", "value": 123}"#).unwrap();
        assert!(jsonb.contains_key("name"));
        assert!(jsonb.contains_key("value"));
        assert_eq!(jsonb.key_count(), 2);
    }

    #[test]
    fn test_json_path() {
        let json = Json::parse(r#"{"items": [{"name": "a"}, {"name": "b"}]}"#).unwrap();
        let value = json.get_path("items[0].name").unwrap();
        assert!(value.is_some());
    }

    #[test]
    fn test_json_to_jsonb() {
        let json = Json::parse(r#"{"test": 123}"#).unwrap();
        let jsonb = json.to_jsonb().unwrap();
        assert!(jsonb.contains_key("test"));
    }

    #[test]
    fn test_jsonb_comparison() {
        let a = JsonB::parse("1").unwrap();
        let b = JsonB::parse("2").unwrap();
        assert!(a < b);
    }

    #[test]
    fn test_jsonb_equality() {
        let a = JsonB::parse(r#"{"b": 1, "a": 2}"#).unwrap();
        let b = JsonB::parse(r#"{"a": 2, "b": 1}"#).unwrap();
        // JSONB normalizes key order, so these should be equal
        assert_eq!(a, b);
    }
}
