//! Unified search result types shared across fulltext and vector engines.
//!
//! This module defines the common result representation that both search
//! backends produce, allowing fusion and downstream consumers to work with
//! a single type instead of engine-specific structs.

use super::index::SearchIndexMetadata;
use crate::Value;
use std::collections::HashMap;

/// A single search hit from any secondary index backend.
///
/// Both fulltext and vector engines convert their native result types into
/// this unified representation before returning to the fusion layer or
/// external consumers. The `highlights` field carries fulltext-specific
/// detail while `payload` carries vector-specific key-value pairs.
#[derive(Debug, Clone, Default)]
pub struct SearchHit {
    /// Document identifier (vertex display form or raw doc id).
    pub id: String,
    /// Relevance score; higher is more relevant.
    pub score: f32,
    /// Optional highlight fragments (fulltext only).
    pub highlights: Option<Vec<String>>,
    /// Optional payload key-value pairs (vector only).
    pub payload: Option<HashMap<String, Value>>,
    /// Optional source document data.
    pub source: Option<HashMap<String, Value>>,
}

impl SearchHit {
    pub fn new(id: impl Into<String>, score: f32) -> Self {
        Self {
            id: id.into(),
            score,
            ..Default::default()
        }
    }

    pub fn with_highlights(mut self, highlights: Vec<String>) -> Self {
        self.highlights = Some(highlights);
        self
    }

    pub fn with_payload(mut self, payload: HashMap<String, Value>) -> Self {
        self.payload = Some(payload);
        self
    }

    pub fn with_source(mut self, source: HashMap<String, Value>) -> Self {
        self.source = Some(source);
        self
    }
}

/// A page of search hits with aggregate statistics.
#[derive(Debug, Clone, Default)]
pub struct SearchHitPage {
    pub hits: Vec<SearchHit>,
    pub total_hits: usize,
    pub max_score: f32,
    pub took_ms: u64,
}

impl SearchHitPage {
    pub fn new(hits: Vec<SearchHit>) -> Self {
        let total_hits = hits.len();
        let max_score = hits.iter().map(|h| h.score).fold(0.0, f32::max);
        Self {
            hits,
            total_hits,
            max_score,
            took_ms: 0,
        }
    }

    pub fn with_stats(mut self, total_hits: usize, took_ms: u64) -> Self {
        self.total_hits = total_hits;
        self.took_ms = took_ms;
        self
    }
}

/// A unified view of an index that both fulltext and vector backends can
/// produce. This is the type returned by `list_index_descriptors` and
/// consumed by management/observability code.
pub type IndexDescriptor = SearchIndexMetadata;

/// Conversion trait: any search backend result type implements this to
/// produce the unified [`SearchHit`].
///
/// Implementations live in `linkrs-api` to respect the dependency DAG
/// (`linkrs-core` cannot depend on `linkrs-fulltext` or `linkrs-sync`).
pub trait IntoSearchHit {
    fn into_search_hit(self) -> SearchHit;
}

/// Conversion trait for borrowed search results.
pub trait AsSearchHit {
    fn as_search_hit(&self) -> SearchHit;
}
