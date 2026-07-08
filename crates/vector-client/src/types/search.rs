use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchMode {
    TopK(usize),
    KNN {
        k: usize,
        ef_search: Option<usize>,
    },
    Range {
        radius: f32,
        max_results: Option<usize>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub vector: Vec<f32>,
    pub limit: usize,
    pub offset: Option<usize>,
    pub score_threshold: Option<f32>,
    pub filter: Option<super::VectorFilter>,
    pub with_payload: Option<bool>,
    pub with_vector: Option<bool>,
    pub nprobe: Option<usize>,
    pub search_mode: Option<SearchMode>,
}

impl SearchQuery {
    pub fn new(vector: Vec<f32>, limit: usize) -> Self {
        Self {
            vector,
            limit,
            offset: None,
            score_threshold: None,
            filter: None,
            with_payload: Some(true),
            with_vector: None,
            nprobe: None,
            search_mode: None,
        }
    }

    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }

    pub fn with_score_threshold(mut self, threshold: f32) -> Self {
        self.score_threshold = Some(threshold);
        self
    }

    pub fn with_filter(mut self, filter: super::VectorFilter) -> Self {
        self.filter = Some(filter);
        self
    }

    pub fn with_payload(mut self, with_payload: bool) -> Self {
        self.with_payload = Some(with_payload);
        self
    }

    pub fn with_vector(mut self, with_vector: bool) -> Self {
        self.with_vector = Some(with_vector);
        self
    }

    pub fn with_nprobe(mut self, nprobe: usize) -> Self {
        self.nprobe = Some(nprobe);
        self
    }

    pub fn effective_limit(&self) -> usize {
        match &self.search_mode {
            Some(SearchMode::Range {
                max_results: Some(max),
                ..
            }) => *max,
            Some(SearchMode::TopK(k)) => *k,
            Some(SearchMode::KNN { k, .. }) => *k,
            _ => self.limit,
        }
    }

    pub fn hnsw_ef(&self) -> Option<usize> {
        match &self.search_mode {
            Some(SearchMode::KNN { ef_search, .. }) => *ef_search,
            _ => None,
        }
    }

    pub fn score_threshold(&self) -> Option<f32> {
        match &self.search_mode {
            Some(SearchMode::Range { radius, .. }) => Some(*radius),
            _ => None,
        }
    }

    pub fn with_search_mode(mut self, mode: SearchMode) -> Self {
        self.search_mode = Some(mode);
        self
    }

    pub fn with_knn(mut self, k: usize, ef_search: Option<usize>) -> Self {
        self.search_mode = Some(SearchMode::KNN { k, ef_search });
        self.limit = k;
        self
    }

    pub fn with_range(mut self, radius: f32, max_results: Option<usize>) -> Self {
        self.search_mode = Some(SearchMode::Range {
            radius,
            max_results,
        });
        self.score_threshold = Some(radius);
        if let Some(max) = max_results {
            self.limit = max;
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: super::PointId,
    pub score: f32,
    pub payload: Option<super::Payload>,
    pub vector: Option<Vec<f32>>,
}

impl SearchResult {
    pub fn new(id: impl Into<super::PointId>, score: f32) -> Self {
        Self {
            id: id.into(),
            score,
            payload: None,
            vector: None,
        }
    }

    pub fn with_payload(mut self, payload: super::Payload) -> Self {
        self.payload = Some(payload);
        self
    }

    pub fn with_vector(mut self, vector: Vec<f32>) -> Self {
        self.vector = Some(vector);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResults {
    pub results: Vec<SearchResult>,
    pub total: Option<u64>,
}

impl SearchResults {
    pub fn new(results: Vec<SearchResult>) -> Self {
        let total = Some(results.len() as u64);
        Self { results, total }
    }

    pub fn is_empty(&self) -> bool {
        self.results.is_empty()
    }

    pub fn len(&self) -> usize {
        self.results.len()
    }
}

impl From<Vec<SearchResult>> for SearchResults {
    fn from(results: Vec<SearchResult>) -> Self {
        Self::new(results)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSearchQuery {
    pub queries: Vec<SearchQuery>,
}

impl BatchSearchQuery {
    pub fn new(queries: Vec<SearchQuery>) -> Self {
        Self { queries }
    }
}

impl From<Vec<SearchQuery>> for BatchSearchQuery {
    fn from(queries: Vec<SearchQuery>) -> Self {
        Self::new(queries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PointId;

    #[test]
    fn test_search_query_new() {
        let q = SearchQuery::new(vec![1.0, 2.0], 10);
        assert_eq!(q.vector, vec![1.0, 2.0]);
        assert_eq!(q.limit, 10);
        assert_eq!(q.with_payload, Some(true));
        assert!(q.with_vector.is_none());
    }

    #[test]
    fn test_search_query_with_offset() {
        let q = SearchQuery::new(vec![1.0], 10).with_offset(5);
        assert_eq!(q.offset, Some(5));
    }

    #[test]
    fn test_search_query_with_score_threshold() {
        let q = SearchQuery::new(vec![1.0], 10).with_score_threshold(0.5);
        assert_eq!(q.score_threshold, Some(0.5));
    }

    #[test]
    fn test_search_query_with_payload() {
        let q = SearchQuery::new(vec![1.0], 10).with_payload(false);
        assert_eq!(q.with_payload, Some(false));
    }

    #[test]
    fn test_search_query_with_vector() {
        let q = SearchQuery::new(vec![1.0], 10).with_vector(true);
        assert_eq!(q.with_vector, Some(true));
    }

    #[test]
    fn test_search_query_with_nprobe() {
        let q = SearchQuery::new(vec![1.0], 10).with_nprobe(64);
        assert_eq!(q.nprobe, Some(64));
    }

    #[test]
    fn test_search_query_effective_limit_default() {
        let q = SearchQuery::new(vec![1.0], 10);
        assert_eq!(q.effective_limit(), 10);
    }

    #[test]
    fn test_search_query_effective_limit_topk() {
        let q = SearchQuery::new(vec![1.0], 10).with_search_mode(SearchMode::TopK(5));
        assert_eq!(q.effective_limit(), 5);
    }

    #[test]
    fn test_search_query_effective_limit_knn() {
        let q = SearchQuery::new(vec![1.0], 10).with_search_mode(SearchMode::KNN {
            k: 20,
            ef_search: Some(100),
        });
        assert_eq!(q.effective_limit(), 20);
    }

    #[test]
    fn test_search_query_effective_limit_range() {
        let q = SearchQuery::new(vec![1.0], 10).with_search_mode(SearchMode::Range {
            radius: 0.5,
            max_results: Some(30),
        });
        assert_eq!(q.effective_limit(), 30);
    }

    #[test]
    fn test_search_query_hnsw_ef_default() {
        let q = SearchQuery::new(vec![1.0], 10);
        assert!(q.hnsw_ef().is_none());
    }

    #[test]
    fn test_search_query_hnsw_ef_knn() {
        let q = SearchQuery::new(vec![1.0], 10).with_knn(5, Some(128));
        assert_eq!(q.hnsw_ef(), Some(128));
    }

    #[test]
    fn test_search_query_hnsw_ef_topk() {
        let q = SearchQuery::new(vec![1.0], 10).with_search_mode(SearchMode::TopK(5));
        assert!(q.hnsw_ef().is_none());
    }

    #[test]
    fn test_search_query_with_knn_sets_limit() {
        let q = SearchQuery::new(vec![1.0], 10).with_knn(42, None);
        assert_eq!(q.limit, 42);
        assert!(matches!(q.search_mode, Some(SearchMode::KNN { k: 42, .. })));
    }

    #[test]
    fn test_search_query_with_range_sets_limit() {
        let q = SearchQuery::new(vec![1.0], 10).with_range(0.3, Some(25));
        assert_eq!(q.limit, 25);
        assert_eq!(q.score_threshold, Some(0.3));
    }

    #[test]
    fn test_search_query_with_range_no_max_keeps_limit() {
        let q = SearchQuery::new(vec![1.0], 10).with_range(0.3, None);
        assert_eq!(q.limit, 10);
    }

    #[test]
    fn test_search_result_new() {
        let r = SearchResult::new(42u64, 0.95);
        assert_eq!(r.id, PointId::Num(42));
        assert!((r.score - 0.95).abs() < f32::EPSILON);
        assert!(r.payload.is_none());
        assert!(r.vector.is_none());
    }

    #[test]
    fn test_search_result_with_payload() {
        let mut payload = std::collections::HashMap::new();
        payload.insert("key".into(), serde_json::json!("val"));
        let r = SearchResult::new("1", 0.5).with_payload(payload);
        assert!(r.payload.is_some());
    }

    #[test]
    fn test_search_result_with_vector() {
        let r = SearchResult::new("id", 0.1).with_vector(vec![1.0, 2.0]);
        assert_eq!(r.vector, Some(vec![1.0, 2.0]));
    }

    #[test]
    fn test_search_results_empty() {
        let r = SearchResults::new(vec![]);
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn test_search_results_non_empty() {
        let results = vec![SearchResult::new(1u64, 0.9), SearchResult::new(2u64, 0.8)];
        let r = SearchResults::new(results);
        assert!(!r.is_empty());
        assert_eq!(r.len(), 2);
        assert_eq!(r.total, Some(2));
    }

    #[test]
    fn test_search_results_from_vec() {
        let results = vec![SearchResult::new(1u64, 0.9)];
        let r: SearchResults = results.into();
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn test_batch_search_query_new() {
        let q = SearchQuery::new(vec![1.0], 5);
        let batch = BatchSearchQuery::new(vec![q]);
        assert_eq!(batch.queries.len(), 1);
    }

    #[test]
    fn test_batch_search_query_from() {
        let q = SearchQuery::new(vec![1.0], 5);
        let batch: BatchSearchQuery = vec![q].into();
        assert_eq!(batch.queries.len(), 1);
    }

    #[test]
    fn test_search_mode_topk() {
        let mode = SearchMode::TopK(10);
        match mode {
            SearchMode::TopK(k) => assert_eq!(k, 10),
            _ => panic!("expected TopK"),
        }
    }

    #[test]
    fn test_search_mode_knn() {
        let mode = SearchMode::KNN {
            k: 20,
            ef_search: Some(200),
        };
        match mode {
            SearchMode::KNN { k, ef_search } => {
                assert_eq!(k, 20);
                assert_eq!(ef_search, Some(200));
            }
            _ => panic!("expected KNN"),
        }
    }

    #[test]
    fn test_search_mode_range() {
        let mode = SearchMode::Range {
            radius: 0.7,
            max_results: Some(50),
        };
        match mode {
            SearchMode::Range {
                radius,
                max_results,
            } => {
                assert!((radius - 0.7).abs() < f32::EPSILON);
                assert_eq!(max_results, Some(50));
            }
            _ => panic!("expected Range"),
        }
    }

    #[test]
    fn test_search_query_score_threshold_range_mode() {
        let q = SearchQuery::new(vec![1.0], 10).with_range(0.5, None);
        assert_eq!(q.score_threshold(), Some(0.5));
    }

    #[test]
    fn test_search_query_score_threshold_default() {
        let q = SearchQuery::new(vec![1.0], 10);
        assert!(q.score_threshold().is_none());
    }
}
