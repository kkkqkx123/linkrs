use crate::core::Value;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub doc_id: Value,
    pub score: f32,
    pub highlights: Option<Vec<String>>,
    pub matched_fields: Vec<String>,
}

impl SearchResult {
    pub fn new(doc_id: Value, score: f32) -> Self {
        Self {
            doc_id,
            score,
            highlights: None,
            matched_fields: Vec::new(),
        }
    }

    pub fn with_highlights(mut self, highlights: Vec<String>) -> Self {
        self.highlights = Some(highlights);
        self
    }

    pub fn with_matched_fields(mut self, fields: Vec<String>) -> Self {
        self.matched_fields = fields;
        self
    }
}

#[derive(Debug, Clone)]
pub struct IndexStats {
    pub doc_count: usize,
    pub index_size: usize,
    pub last_updated: Option<DateTime<Utc>>,
    pub engine_info: Option<serde_json::Value>,
}

impl IndexStats {
    pub fn new(doc_count: usize, index_size: usize) -> Self {
        Self {
            doc_count,
            index_size,
            last_updated: Some(Utc::now()),
            engine_info: None,
        }
    }
}

// ============================================================================
// Full-Text Search Result Types
// ============================================================================

/// Full-text search result
#[derive(Debug, Clone)]
pub struct FulltextSearchResult {
    /// Search result entries
    pub results: Vec<FulltextSearchEntry>,
    /// Total number of hits
    pub total_hits: usize,
    /// Maximum score
    pub max_score: f64,
    /// Time taken in milliseconds
    pub took_ms: u64,
    /// Whether timed out
    pub timed_out: bool,
}

impl Default for FulltextSearchResult {
    fn default() -> Self {
        Self {
            results: Vec::new(),
            total_hits: 0,
            max_score: 0.0,
            took_ms: 0,
            timed_out: false,
        }
    }
}

/// Full-text search entry
#[derive(Debug, Clone)]
pub struct FulltextSearchEntry {
    /// Document ID
    pub doc_id: Value,
    /// Relevance score
    pub score: f64,
    /// Highlight results by field
    pub highlights: Option<HashMap<String, Vec<String>>>,
    /// Matched fields
    pub matched_fields: Vec<String>,
    /// Source document data
    pub source: Option<HashMap<String, Value>>,
}

impl FulltextSearchEntry {
    pub fn new(doc_id: Value, score: f64) -> Self {
        Self {
            doc_id,
            score,
            highlights: None,
            matched_fields: Vec::new(),
            source: None,
        }
    }

    pub fn with_highlights(mut self, highlights: HashMap<String, Vec<String>>) -> Self {
        self.highlights = Some(highlights);
        self
    }

    pub fn with_matched_fields(mut self, fields: Vec<String>) -> Self {
        self.matched_fields = fields;
        self
    }

    pub fn with_source(mut self, source: HashMap<String, Value>) -> Self {
        self.source = Some(source);
        self
    }
}

/// Highlight result for a single field
#[derive(Debug, Clone)]
pub struct HighlightResult {
    pub field: String,
    pub fragments: Vec<String>,
    pub matched_positions: Vec<(usize, usize)>,
}

impl HighlightResult {
    pub fn new(field: String, fragments: Vec<String>) -> Self {
        Self {
            field,
            fragments,
            matched_positions: Vec::new(),
        }
    }
}

/// Search statistics
#[derive(Debug, Clone)]
pub struct SearchStats {
    pub total_results: usize,
    pub returned_results: usize,
    pub search_time_ms: u64,
    pub cache_hit: bool,
    pub index_used: String,
}

impl SearchStats {
    pub fn new(index_used: String) -> Self {
        Self {
            total_results: 0,
            returned_results: 0,
            search_time_ms: 0,
            cache_hit: false,
            index_used,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_result_creation() {
        let result = SearchResult::new(Value::String("doc1".to_string()), 0.95);
        assert_eq!(result.score, 0.95);
        assert!(result.highlights.is_none());
    }

    #[test]
    fn test_search_result_with_highlights() {
        let result = SearchResult::new(Value::String("doc1".to_string()), 0.95)
            .with_highlights(vec!["<em>highlight</em>".to_string()]);
        assert!(result.highlights.is_some());
        assert_eq!(result.highlights.unwrap().len(), 1);
    }

    #[test]
    fn test_fulltext_search_entry() {
        let entry = FulltextSearchEntry::new(Value::String("doc1".to_string()), 0.85);
        assert_eq!(entry.score, 0.85);
        assert!(entry.highlights.is_none());
    }

    #[test]
    fn test_index_stats() {
        let stats = IndexStats::new(1000, 2048);
        assert_eq!(stats.doc_count, 1000);
        assert_eq!(stats.index_size, 2048);
        assert!(stats.last_updated.is_some());
    }
}
