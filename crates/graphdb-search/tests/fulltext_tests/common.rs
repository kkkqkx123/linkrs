use graphdb_core::core::Value;
use graphdb_search::search::{
    EngineType, FulltextConfig, FulltextIndexManager, IndexMetadata, IndexStats, SearchError,
    SearchResult,
};
use std::sync::Arc;
use tempfile::TempDir;

pub struct FulltextTestContext {
    pub manager: Arc<FulltextIndexManager>,
    #[allow(dead_code)]
    pub temp_dir: TempDir,
}

impl FulltextTestContext {
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = FulltextConfig {
            enabled: true,
            index_path: temp_dir.path().to_path_buf(),
            default_engine: EngineType::Bm25,
            sync: graphdb_search::search::SyncConfig::default(),
            tantivy: Default::default(),
            cache_size: 100,
            max_result_cache: 1000,
            result_cache_ttl_secs: 60,
        };
        let manager =
            Arc::new(FulltextIndexManager::new(config).expect("Failed to create manager"));
        Self { manager, temp_dir }
    }

    pub async fn create_test_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        engine_type: Option<EngineType>,
    ) -> Result<String, SearchError> {
        self.manager
            .create_index(space_id, tag_name, field_name, engine_type)
            .await
    }

    pub async fn insert_test_doc(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        doc_id: &str,
        content: &str,
    ) -> Result<(), SearchError> {
        if let Some(engine) = self.manager.get_engine(space_id, tag_name, field_name) {
            engine.index(doc_id, content).await?;
        } else {
            return Err(SearchError::IndexNotFound(format!(
                "{}.{}.{}",
                space_id, tag_name, field_name
            )));
        }
        Ok(())
    }

    pub async fn insert_test_docs(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        docs: Vec<(&str, &str)>,
    ) -> Result<(), SearchError> {
        if let Some(engine) = self.manager.get_engine(space_id, tag_name, field_name) {
            let docs_vec: Vec<(String, String)> = docs
                .into_iter()
                .map(|(id, content)| (id.to_string(), content.to_string()))
                .collect();
            engine.index_batch(docs_vec).await?;
        } else {
            return Err(SearchError::IndexNotFound(format!(
                "{}.{}.{}",
                space_id, tag_name, field_name
            )));
        }
        Ok(())
    }

    pub async fn search(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        self.manager
            .search(space_id, tag_name, field_name, query, limit)
            .await
    }

    pub async fn commit_all(&self) -> Result<(), SearchError> {
        self.manager.commit_all().await
    }

    pub fn has_index(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        self.manager.has_index(space_id, tag_name, field_name)
    }

    pub fn get_metadata(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<IndexMetadata> {
        self.manager.get_metadata(space_id, tag_name, field_name)
    }

    pub fn get_space_indexes(&self, space_id: u64) -> Vec<IndexMetadata> {
        self.manager.get_space_indexes(space_id)
    }

    pub async fn drop_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<(), SearchError> {
        self.manager
            .drop_index(space_id, tag_name, field_name)
            .await
    }

    #[allow(dead_code)]
    pub async fn get_stats(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<IndexStats, SearchError> {
        self.manager.get_stats(space_id, tag_name, field_name).await
    }

    #[allow(dead_code)]
    pub fn get_engine_type(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<EngineType> {
        self.get_metadata(space_id, tag_name, field_name)
            .map(|m| m.engine_type)
    }
}

impl Default for FulltextTestContext {
    fn default() -> Self {
        Self::new()
    }
}

pub fn generate_test_docs(count: usize, prefix: &str) -> Vec<(String, String)> {
    (0..count)
        .map(|i| {
            (
                format!("doc_{}", i),
                format!(
                    "{} document number {} with some content for testing",
                    prefix, i
                ),
            )
        })
        .collect()
}

#[allow(dead_code)]
pub fn create_docs_with_words(doc_count: usize, words_per_doc: usize) -> Vec<(String, String)> {
    (0..doc_count)
        .map(|i| {
            let words: Vec<String> = (0..words_per_doc)
                .map(|j| format!("word{}_{}", i, j))
                .collect();
            (format!("doc_{}", i), words.join(" "))
        })
        .collect()
}

pub fn assert_search_result_contains(
    results: &[SearchResult],
    expected_doc_id: &str,
) -> Result<(), String> {
    let expected_value = Value::String(expected_doc_id.to_string());
    if results.iter().any(|r| r.doc_id == expected_value) {
        Ok(())
    } else {
        Err(format!(
            "Search results should contain document '{}', but got: {:?}",
            expected_doc_id, results
        ))
    }
}

pub fn assert_search_result_not_contains(
    results: &[SearchResult],
    unexpected_doc_id: &str,
) -> Result<(), String> {
    let unexpected_value = Value::String(unexpected_doc_id.to_string());
    if !results.iter().any(|r| r.doc_id == unexpected_value) {
        Ok(())
    } else {
        Err(format!(
            "Search results should not contain document '{}', but got: {:?}",
            unexpected_doc_id, results
        ))
    }
}

pub fn assert_search_result_count(
    results: &[SearchResult],
    expected_count: usize,
) -> Result<(), String> {
    if results.len() == expected_count {
        Ok(())
    } else {
        Err(format!(
            "Expected {} results, but got {}",
            expected_count,
            results.len()
        ))
    }
}

pub fn assert_results_sorted_by_score(results: &[SearchResult]) -> Result<(), String> {
    for i in 1..results.len() {
        if results[i].score > results[i - 1].score {
            return Err(format!(
                "Results should be sorted by score descending, but found {} > {} at positions {} and {}",
                results[i].score,
                results[i - 1].score,
                i,
                i - 1
            ));
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn assert_search_results_contain_all(
    results: &[SearchResult],
    expected_doc_ids: &[&str],
) -> Result<(), String> {
    for expected_id in expected_doc_ids {
        let expected_value = Value::String(expected_id.to_string());
        if !results.iter().any(|r| r.doc_id == expected_value) {
            return Err(format!(
                "Search results should contain document '{}'",
                expected_id
            ));
        }
    }
    Ok(())
}
