//! Vector operation types

use std::collections::HashMap;

/// Vector search result
#[derive(Debug, Clone)]
pub struct VectorSearchResult {
    pub total: usize,
    pub results: Vec<VectorMatch>,
}

/// Vector match
#[derive(Debug, Clone)]
pub struct VectorMatch {
    pub vid: serde_json::Value,
    pub score: f32,
    pub properties: HashMap<String, serde_json::Value>,
}
