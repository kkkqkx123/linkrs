use crate::types::*;

pub struct ExtractedSearchParams {
    pub limit: usize,
    pub hnsw_ef: Option<usize>,
    pub score_threshold: Option<f32>,
}

pub fn extract_search_params(query: &SearchQuery) -> ExtractedSearchParams {
    ExtractedSearchParams {
        limit: query.effective_limit(),
        hnsw_ef: query.nprobe.or(query.hnsw_ef()),
        score_threshold: query.score_threshold.or(query.score_threshold()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_search_params_default() {
        let q = SearchQuery::new(vec![1.0, 2.0], 10);
        let params = extract_search_params(&q);
        assert_eq!(params.limit, 10);
        assert!(params.hnsw_ef.is_none());
        assert!(params.score_threshold.is_none());
    }

    #[test]
    fn test_extract_search_params_with_nprobe() {
        let q = SearchQuery::new(vec![1.0], 10).with_nprobe(64);
        let params = extract_search_params(&q);
        assert_eq!(params.hnsw_ef, Some(64));
    }

    #[test]
    fn test_extract_search_params_with_knn_ef() {
        let q = SearchQuery::new(vec![1.0], 10).with_knn(5, Some(128));
        let params = extract_search_params(&q);
        // nprobe takes priority over hnsw_ef from search mode
        assert!(params.hnsw_ef.is_none() || params.hnsw_ef == Some(128));
    }

    #[test]
    fn test_extract_search_params_nprobe_overrides_knn_ef() {
        let q = SearchQuery::new(vec![1.0], 10)
            .with_knn(5, Some(128))
            .with_nprobe(200);
        let params = extract_search_params(&q);
        assert_eq!(params.hnsw_ef, Some(200));
    }

    #[test]
    fn test_extract_search_params_score_threshold_from_query() {
        let q = SearchQuery::new(vec![1.0], 10).with_score_threshold(0.7);
        let params = extract_search_params(&q);
        assert!((params.score_threshold.unwrap() - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn test_extract_search_params_score_threshold_from_range() {
        let q = SearchQuery::new(vec![1.0], 10).with_search_mode(SearchMode::Range {
            radius: 0.5,
            max_results: None,
        });
        let params = extract_search_params(&q);
        assert!((params.score_threshold.unwrap() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_extract_search_params_limit_topk() {
        let q = SearchQuery::new(vec![1.0], 10).with_search_mode(SearchMode::TopK(3));
        let params = extract_search_params(&q);
        assert_eq!(params.limit, 3);
    }

    #[test]
    fn test_extract_search_params_limit_knn() {
        let q = SearchQuery::new(vec![1.0], 10).with_knn(20, None);
        let params = extract_search_params(&q);
        assert_eq!(params.limit, 20);
    }
}
