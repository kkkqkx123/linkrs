use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

use std::ops::Bound;

use tantivy::collector::TopDocs;
use tantivy::doc;
use tantivy::index::Bm25Params as TantivyBm25Params;
use tantivy::index::IndexSettings;
use tantivy::query::{
    wildcard_query_to_regex_str, BooleanQuery, EmptyQuery, FuzzyTermQuery, Occur,
    PhrasePrefixQuery, PhraseQuery, Query, QueryParser, RangeQuery, RegexQuery, TermQuery,
};
use tantivy::schema::Value as SchemaValue;
use tantivy::schema::*;
use tantivy::tokenizer::TokenStream as _;
use tantivy::IndexBuilder;
use tantivy::IndexWriter;
use tantivy::Searcher;
use tantivy::TantivyDocument;
use tantivy::Term;

use crate::query::FulltextQuery;

#[cfg(feature = "jieba")]
use crate::jieba_tokenizer::JiebaTokenizer;

use crate::error::SearchError;
use crate::result::{IndexStats, SearchResult};
use crate::ConsistencyState;
use graphdb_core::Value;

use graphdb_config::fulltext::TantivyConfig;
#[cfg(feature = "jieba")]
use graphdb_config::fulltext::TokenizerKind;

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

/// Collect top-k search results with highlights from a tantivy query.
///
/// Shared by both the grammar-string path and the structured-query path so
/// result formatting stays identical.
fn collect_search_results(
    searcher: &Searcher,
    query: &dyn Query,
    text_field: Field,
    id_field: Field,
    limit: usize,
) -> Result<Vec<SearchResult>, SearchError> {
    let top_docs = searcher.search(query, &TopDocs::with_limit(limit).order_by_score())?;

    let snippet_generator =
        tantivy::snippet::SnippetGenerator::create(searcher, query, text_field)?;

    let mut results = Vec::with_capacity(top_docs.len());
    for (score, doc_address) in top_docs {
        let doc = searcher.doc::<TantivyDocument>(doc_address)?;
        let doc_id: String = doc
            .get_first(id_field)
            .and_then(|v| SchemaValue::as_str(&v))
            .unwrap_or("")
            .to_string();

        let highlights = doc
            .get_first(text_field)
            .and_then(|v| SchemaValue::as_str(&v))
            .map(|text| vec![snippet_generator.snippet(text).to_html()]);

        results.push(SearchResult {
            doc_id: Value::string(doc_id),
            score,
            highlights,
            matched_fields: vec![],
        });
    }

    Ok(results)
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
            let bm25_params = TantivyBm25Params {
                k1: config.bm25_params.k1,
                b: config.bm25_params.b,
            };
            IndexBuilder::new()
                .schema(schema.clone())
                .settings(IndexSettings {
                    bm25_params: Some(bm25_params),
                    ..Default::default()
                })
                .create_in_dir(path)?
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

        collect_search_results(&searcher, &*query, self.text_field, self.id_field, limit)
    }

    /// Tokenize text into tantivy terms using the index's field tokenizer.
    fn tokenize_text(&self, text: &str) -> Vec<Term> {
        let mut analyzer = match self.index.tokenizer_for_field(self.text_field) {
            Ok(a) => a,
            Err(_) => return Vec::new(),
        };
        let mut stream = analyzer.token_stream(text);
        let mut terms = Vec::new();
        while stream.advance() {
            let token_text = stream.token().text.clone();
            terms.push(Term::from_field_text(self.text_field, &token_text));
        }
        terms
    }

    /// Build a tantivy native query from a structured [`FulltextQuery`].
    fn build_structured_query(&self, query: &FulltextQuery) -> Result<Box<dyn Query>, SearchError> {
        let field = self.text_field;
        match query {
            FulltextQuery::Simple(text) => {
                let terms = self.tokenize_text(text);
                if terms.is_empty() {
                    return Ok(Box::new(EmptyQuery));
                }
                if terms.len() == 1 {
                    return Ok(Box::new(TermQuery::new(
                        terms.into_iter().next().unwrap(),
                        IndexRecordOption::Basic,
                    )));
                }
                let subqueries: Vec<(Occur, Box<dyn Query>)> = terms
                    .into_iter()
                    .map(|t| {
                        (
                            Occur::Should,
                            Box::new(TermQuery::new(t, IndexRecordOption::Basic)) as Box<dyn Query>,
                        )
                    })
                    .collect();
                Ok(Box::new(BooleanQuery::new(subqueries)))
            }
            FulltextQuery::Phrase(text) => {
                let terms = self.tokenize_text(text);
                if terms.is_empty() {
                    return Ok(Box::new(EmptyQuery));
                }
                if terms.len() == 1 {
                    return Ok(Box::new(TermQuery::new(
                        terms.into_iter().next().unwrap(),
                        IndexRecordOption::Basic,
                    )));
                }
                Ok(Box::new(PhraseQuery::new(terms)))
            }
            FulltextQuery::Prefix(text) => {
                let terms = self.tokenize_text(text);
                if terms.is_empty() {
                    return Ok(Box::new(EmptyQuery));
                }
                Ok(Box::new(PhrasePrefixQuery::new(terms)))
            }
            FulltextQuery::Fuzzy(text, distance) => {
                let terms = self.tokenize_text(text);
                let term = terms
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| Term::from_field_text(field, text));
                let dist = distance.unwrap_or(1);
                Ok(Box::new(FuzzyTermQuery::new(term, dist, true)))
            }
            FulltextQuery::Wildcard(text) => {
                let regex_str = wildcard_query_to_regex_str(text);
                RegexQuery::from_pattern(&regex_str, field)
                    .map(|q| Box::new(q) as Box<dyn Query>)
                    .map_err(|e| SearchError::QueryParseError(e.to_string()))
            }
            FulltextQuery::Boolean {
                must,
                should,
                must_not,
            } => {
                let mut subqueries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
                for q in must {
                    subqueries.push((Occur::Must, self.build_structured_query(q)?));
                }
                for q in should {
                    subqueries.push((Occur::Should, self.build_structured_query(q)?));
                }
                for q in must_not {
                    subqueries.push((Occur::MustNot, self.build_structured_query(q)?));
                }
                if subqueries.is_empty() {
                    return Ok(Box::new(EmptyQuery));
                }
                Ok(Box::new(BooleanQuery::new(subqueries)))
            }
            FulltextQuery::Range {
                lower,
                upper,
                include_lower,
                include_upper,
            } => {
                let lower_bound = match lower {
                    Some(text) => {
                        let term = Term::from_field_text(field, text);
                        if *include_lower {
                            Bound::Included(term)
                        } else {
                            Bound::Excluded(term)
                        }
                    }
                    None => Bound::Unbounded,
                };
                let upper_bound = match upper {
                    Some(text) => {
                        let term = Term::from_field_text(field, text);
                        if *include_upper {
                            Bound::Included(term)
                        } else {
                            Bound::Excluded(term)
                        }
                    }
                    None => Bound::Unbounded,
                };
                Ok(Box::new(RangeQuery::new(lower_bound, upper_bound)))
            }
        }
    }

    /// Structured search: build a native tantivy query from [`FulltextQuery`]
    /// without going through the query-grammar parser.
    pub async fn search_structured(
        &self,
        query: &FulltextQuery,
        limit: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let searcher = self.reader.searcher();
        let tantivy_query = self.build_structured_query(query)?;
        collect_search_results(
            &searcher,
            &*tantivy_query,
            self.text_field,
            self.id_field,
            limit,
        )
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

    pub async fn commit_with_payload(&self, payload: String) -> Result<(), SearchError> {
        self.with_writer(move |writer| {
            let mut commit = writer.prepare_commit()?;
            commit.set_payload(&payload);
            commit.commit()?;
            Ok(())
        })
        .await?;
        self.reader.reload()?;
        self.refresh_stats_cache();
        Ok(())
    }

    pub fn commit_payload(&self) -> Result<Option<String>, SearchError> {
        Ok(self.index.load_metas()?.payload)
    }

    pub async fn rollback(&self) -> Result<(), SearchError> {
        self.with_writer(move |writer| {
            writer.rollback()?;
            Ok(())
        })
        .await?;
        self.reader.reload()?;
        self.refresh_stats_cache();
        Ok(())
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

#[async_trait::async_trait]
impl crate::engine::FulltextSearchEngine for TantivySearchEngine {
    fn name(&self) -> &str {
        "tantivy"
    }

    fn version(&self) -> &str {
        "0.26.0"
    }

    async fn index(&self, doc_id: &str, content: &str) -> Result<(), SearchError> {
        self.index(doc_id, content).await
    }

    async fn index_batch(&self, docs: Vec<(String, String)>) -> Result<(), SearchError> {
        self.index_batch(docs).await
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, SearchError> {
        self.search(query, limit).await
    }

    async fn search_structured(
        &self,
        query: &crate::query::FulltextQuery,
        limit: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        self.search_structured(query, limit).await
    }

    async fn delete(&self, doc_id: &str) -> Result<(), SearchError> {
        self.delete(doc_id).await
    }

    async fn delete_batch(&self, doc_ids: Vec<&str>) -> Result<(), SearchError> {
        self.delete_batch(doc_ids).await
    }

    async fn commit(&self) -> Result<(), SearchError> {
        self.commit().await
    }

    async fn commit_with_payload(&self, payload: String) -> Result<(), SearchError> {
        self.commit_with_payload(payload).await
    }

    fn commit_payload(&self) -> Result<Option<String>, SearchError> {
        self.commit_payload()
    }

    async fn stats(&self) -> Result<IndexStats, SearchError> {
        self.stats().await
    }

    fn consistency_state(&self) -> ConsistencyState {
        self.consistency_state()
    }

    fn mark_inconsistent(&self) {
        self.mark_inconsistent();
    }

    fn mark_consistent(&self) {
        self.mark_consistent();
    }

    async fn clear(&self) -> Result<(), SearchError> {
        self.clear().await
    }

    async fn close(&self) -> Result<(), SearchError> {
        self.close().await
    }
}
