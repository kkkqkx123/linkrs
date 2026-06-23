use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tantivy::collector::TopDocs;
use tantivy::doc;
use tantivy::query::QueryParser;
use tantivy::schema::Value as SchemaValue;
use tantivy::schema::*;
use tantivy::IndexWriter;
use tantivy::TantivyDocument;

#[cfg(feature = "jieba")]
use crate::search::jieba_tokenizer::JiebaTokenizer;

use crate::core::Value;
use crate::search::engine::ConsistencyState;
use crate::search::error::SearchError;
use crate::search::result::{IndexStats, SearchResult};

pub use crate::config::common::fulltext::{TantivyConfig, TokenizerKind};

fn build_schema(config: &TantivyConfig) -> (Schema, Field, Field) {
    let tokenizer_name = config.tokenizer.name();
    let mut schema_builder = Schema::builder();
    let id_field = schema_builder.add_text_field("id", STRING | STORED);
    let text_options = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(tokenizer_name)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
        .set_stored();
    let text_field = schema_builder.add_text_field("text", text_options);
    let schema = schema_builder.build();
    (schema, id_field, text_field)
}

pub struct TantivySearchEngine {
    index: tantivy::Index,
    index_path: PathBuf,
    id_field: Field,
    text_field: Field,
    writer: Arc<Mutex<IndexWriter>>,
    reader: Arc<tantivy::IndexReader>,
    consistency_state: AtomicU8,
    cached_doc_count: AtomicU64,
    cached_index_size: AtomicU64,
    last_stats_update: std::sync::Mutex<Option<Instant>>,
}

impl std::fmt::Debug for TantivySearchEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TantivySearchEngine").finish()
    }
}

impl TantivySearchEngine {
    async fn with_writer<F, T>(&self, f: F) -> Result<T, SearchError>
    where
        F: FnOnce(&mut IndexWriter) -> Result<T, tantivy::TantivyError> + Send + 'static,
        T: Send + 'static,
    {
        let writer = self.writer.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = writer.lock();
            f(&mut guard)
        })
        .await
        .map_err(|e| SearchError::Internal(format!("Blocking task failed: {}", e)))?
        .map_err(SearchError::from)
    }

    fn refresh_stats_cache(&self) {
        {
            let searcher = self.reader.searcher();
            let doc_count = searcher.num_docs();
            self.cached_doc_count.store(doc_count, Ordering::Release);
        }

        let index_size = self
            .index_path
            .read_dir()
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().ok().is_some_and(|t| t.is_file()))
            .filter_map(|entry| entry.metadata().ok())
            .map(|meta| meta.len())
            .sum::<u64>();
        self.cached_index_size.store(index_size, Ordering::Release);

        if let Ok(mut last) = self.last_stats_update.lock() {
            *last = Some(Instant::now());
        }
    }

    pub fn open_or_create(path: &Path, config: TantivyConfig) -> Result<Self, SearchError> {
        let (schema, id_field, text_field) = build_schema(&config);

        if !path.exists() {
            std::fs::create_dir_all(path)?;
        }

        let index = if path.join("meta.json").exists() {
            tantivy::Index::open_in_dir(path)?
        } else {
            tantivy::Index::create_in_dir(path, schema.clone())?
        };

        #[cfg(feature = "jieba")]
        if config.tokenizer == TokenizerKind::Jieba {
            index
                .tokenizers()
                .register("jieba", JiebaTokenizer::default());
        }

        let writer = index.writer(config.writer_memory_budget)?;

        let reader = index
            .reader_builder()
            .reload_policy(tantivy::ReloadPolicy::OnCommitWithDelay)
            .doc_store_cache_num_blocks(config.doc_store_cache_num_blocks)
            .try_into()?;

        let index_path = path.to_path_buf();

        Ok(Self {
            index,
            index_path,
            id_field,
            text_field,
            writer: Arc::new(Mutex::new(writer)),
            reader: Arc::new(reader),
            consistency_state: AtomicU8::new(0),
            cached_doc_count: AtomicU64::new(0),
            cached_index_size: AtomicU64::new(0),
            last_stats_update: std::sync::Mutex::new(None),
        })
    }

    pub fn name(&self) -> &str {
        "tantivy"
    }

    pub fn version(&self) -> &str {
        "0.26.0"
    }

    pub async fn index(&self, doc_id: &str, content: &str) -> Result<(), SearchError> {
        let id_field = self.id_field;
        let text_field = self.text_field;
        let doc_id = doc_id.to_string();
        let content = content.to_string();
        self.with_writer(move |writer| {
            writer.delete_term(tantivy::Term::from_field_text(id_field, &doc_id));
            let doc = doc!(id_field => doc_id.as_str(), text_field => content.as_str());
            writer.add_document(doc)?;
            Ok(())
        })
        .await
    }

    pub async fn index_batch(&self, docs: Vec<(String, String)>) -> Result<(), SearchError> {
        let id_field = self.id_field;
        let text_field = self.text_field;
        let docs_clone = docs.clone();
        self.with_writer(move |writer| {
            for (doc_id, content) in &docs_clone {
                writer.delete_term(tantivy::Term::from_field_text(id_field, doc_id));
                let doc = doc!(id_field => doc_id.as_str(), text_field => content.as_str());
                writer.add_document(doc)?;
            }
            Ok(())
        })
        .await
    }

    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let searcher = self.reader.searcher();

        let query_parser = QueryParser::for_index(&self.index, vec![self.text_field]);
        let query = query_parser
            .parse_query(query)
            .map_err(|e| SearchError::QueryParseError(e.to_string()))?;

        let top_docs = searcher.search(&query, &TopDocs::with_limit(limit).order_by_score())?;

        let snippet_generator =
            tantivy::snippet::SnippetGenerator::create(&searcher, &*query, self.text_field)?;

        let mut results = Vec::with_capacity(top_docs.len());
        for (score, doc_address) in top_docs {
            let doc = searcher.doc::<TantivyDocument>(doc_address)?;
            let doc_id: String = doc
                .get_first(self.id_field)
                .and_then(|v| SchemaValue::as_str(&v))
                .unwrap_or("")
                .to_string();

            let highlights = doc
                .get_first(self.text_field)
                .and_then(|v| SchemaValue::as_str(&v))
                .map(|text| vec![snippet_generator.snippet(text).to_html()]);

            results.push(SearchResult {
                doc_id: Value::String(doc_id),
                score,
                highlights,
                matched_fields: vec![],
            });
        }

        Ok(results)
    }

    pub async fn delete(&self, doc_id: &str) -> Result<(), SearchError> {
        let id_field = self.id_field;
        let doc_id = doc_id.to_string();
        self.with_writer(move |writer| {
            writer.delete_term(tantivy::Term::from_field_text(id_field, &doc_id));
            Ok(())
        })
        .await
    }

    pub async fn delete_batch(&self, doc_ids: Vec<&str>) -> Result<(), SearchError> {
        let id_field = self.id_field;
        let ids: Vec<String> = doc_ids.into_iter().map(|s| s.to_string()).collect();
        self.with_writer(move |writer| {
            for doc_id in &ids {
                writer.delete_term(tantivy::Term::from_field_text(id_field, doc_id));
            }
            Ok(())
        })
        .await
    }

    pub async fn commit(&self) -> Result<(), SearchError> {
        self.with_writer(move |writer| {
            writer.commit()?;
            Ok(())
        })
        .await?;
        self.reader.reload()?;
        self.refresh_stats_cache();
        Ok(())
    }

    pub async fn rollback(&self) -> Result<(), SearchError> {
        self.with_writer(move |_writer| Ok(())).await
    }

    pub async fn stats(&self) -> Result<IndexStats, SearchError> {
        const STATS_CACHE_TTL_SECS: u64 = 5;

        let needs_refresh = self
            .last_stats_update
            .lock()
            .ok()
            .and_then(|last| *last)
            .map(|t| t.elapsed().as_secs() > STATS_CACHE_TTL_SECS)
            .unwrap_or(true);

        if needs_refresh {
            self.refresh_stats_cache();
        }

        Ok(IndexStats {
            doc_count: self.cached_doc_count.load(Ordering::Acquire) as usize,
            index_size: self.cached_index_size.load(Ordering::Acquire) as usize,
            last_updated: None,
            engine_info: None,
        })
    }

    pub fn consistency_state(&self) -> ConsistencyState {
        match self.consistency_state.load(Ordering::Acquire) {
            0 => ConsistencyState::Consistent,
            1 => ConsistencyState::Inconsistent,
            _ => ConsistencyState::Rebuilding,
        }
    }

    pub fn mark_inconsistent(&self) {
        self.consistency_state.store(1, Ordering::Release);
    }

    pub fn mark_consistent(&self) {
        self.consistency_state.store(0, Ordering::Release);
    }

    pub async fn clear(&self) -> Result<(), SearchError> {
        self.with_writer(move |writer| {
            writer.delete_all_documents()?;
            writer.commit()?;
            Ok(())
        })
        .await
    }

    pub async fn close(&self) -> Result<(), SearchError> {
        self.commit().await?;
        Ok(())
    }
}
