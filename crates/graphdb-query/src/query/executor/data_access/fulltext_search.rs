//! Full-Text Search Executor
//!
//! This module implements the executor for full-text search queries,
//! including SEARCH statements and full-text scan operations.

use crate::core::error::DBError;
use crate::core::types::VertexId;
use crate::core::Value;
use crate::query::executor::base::{
    BaseExecutor, DBResult, ExecutionResult, Executor, ExecutorStats, HasStorage,
};
use crate::query::parser::ast::fulltext::{
    ComparisonOp, FulltextOrderDirection, FulltextQueryExpr, OrderClause, OrderItem,
    SearchStatement, WhereCondition, YieldExpression,
};
use crate::query::validator::context::ExpressionAnalysisContext;
#[cfg(feature = "fulltext-search")]
use crate::search::manager::FulltextIndexManager;
use crate::storage::StorageReader;
use parking_lot::RwLock;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

/// Parameters for creating FulltextSearchExecutor with metadata
pub struct FulltextSearchExecutorParams<S: StorageReader> {
    pub id: i64,
    pub statement: SearchStatement,
    pub storage: Arc<RwLock<S>>,
    pub expr_context: Arc<ExpressionAnalysisContext>,
    pub fulltext_manager: Arc<FulltextIndexManager>,
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
}

/// Full-text search executor for SEARCH statements
pub struct FulltextSearchExecutor<S: StorageReader> {
    /// Base executor
    base: BaseExecutor<S>,
    /// Search statement
    statement: SearchStatement,
    /// Fulltext manager
    fulltext_manager: Arc<FulltextIndexManager>,
    /// Pre-resolved space_id from planner
    space_id: u64,
    /// Pre-resolved tag_name from planner
    tag_name: String,
    /// Pre-resolved field_name from planner
    field_name: String,
    _phantom: std::marker::PhantomData<S>,
}

impl<S: StorageReader> FulltextSearchExecutor<S> {
    /// Create a new full-text search executor
    pub fn new(
        id: i64,
        statement: SearchStatement,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionAnalysisContext>,
        fulltext_manager: Arc<FulltextIndexManager>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "FulltextSearchExecutor".to_string(),
                storage,
                expr_context,
            ),
            statement,
            fulltext_manager,
            space_id: 0,
            tag_name: String::new(),
            field_name: String::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Create a new full-text search executor with pre-resolved metadata
    pub fn with_metadata(params: FulltextSearchExecutorParams<S>) -> Self {
        Self {
            base: BaseExecutor::new(
                params.id,
                "FulltextSearchExecutor".to_string(),
                params.storage,
                params.expr_context,
            ),
            statement: params.statement,
            fulltext_manager: params.fulltext_manager,
            space_id: params.space_id,
            tag_name: params.tag_name,
            field_name: params.field_name,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Resolve metadata from fulltext_manager if not pre-resolved
    fn resolve_metadata(&self) -> DBResult<(u64, String, String)> {
        if !self.tag_name.is_empty() && !self.field_name.is_empty() {
            return Ok((
                self.space_id,
                self.tag_name.clone(),
                self.field_name.clone(),
            ));
        }

        let index_name = &self.statement.index_name;
        let indexes = self.fulltext_manager.list_indexes();

        for index in indexes {
            if &index.index_name == index_name {
                return Ok((index.space_id, index.tag_name, index.field_name));
            }
        }

        Err(DBError::validation(format!(
            "Fulltext index '{}' not found",
            index_name
        )))
    }

    /// Convert FulltextQueryExpr to search query string
    fn convert_query_to_string(&self, expr: &FulltextQueryExpr) -> String {
        match expr {
            FulltextQueryExpr::Simple(text) => text.clone(),
            FulltextQueryExpr::Field(field, text) => format!("{}:{}", field, text),
            FulltextQueryExpr::MultiField(fields) => fields
                .iter()
                .map(|(f, t)| format!("{}:{}", f, t))
                .collect::<Vec<_>>()
                .join(" OR "),
            FulltextQueryExpr::Boolean {
                must,
                should,
                must_not,
            } => {
                let mut parts = Vec::new();
                if !must.is_empty() {
                    parts.push(format!(
                        "+({})",
                        must.iter()
                            .map(|e| self.convert_query_to_string(e))
                            .collect::<Vec<_>>()
                            .join(" ")
                    ));
                }
                if !should.is_empty() {
                    parts.push(format!(
                        "({})",
                        should
                            .iter()
                            .map(|e| self.convert_query_to_string(e))
                            .collect::<Vec<_>>()
                            .join(" ")
                    ));
                }
                if !must_not.is_empty() {
                    parts.push(format!(
                        "-({})",
                        must_not
                            .iter()
                            .map(|e| self.convert_query_to_string(e))
                            .collect::<Vec<_>>()
                            .join(" ")
                    ));
                }
                parts.join(" ")
            }
            FulltextQueryExpr::Phrase(text) => format!("\"{}\"", text),
            FulltextQueryExpr::Prefix(text) => format!("{}*", text),
            FulltextQueryExpr::Fuzzy(text, distance) => {
                if let Some(d) = distance {
                    format!("{}~{}", text, d)
                } else {
                    format!("{}~", text)
                }
            }
            FulltextQueryExpr::Range {
                field,
                lower,
                upper,
                include_lower,
                include_upper,
            } => {
                let lower_bound = if *include_lower { "[" } else { "{" };
                let upper_bound = if *include_upper { "]" } else { "}" };
                let lower_val = lower.as_deref().unwrap_or("*");
                let upper_val = upper.as_deref().unwrap_or("*");
                format!(
                    "{}:{}{} TO {}{}",
                    field, lower_bound, lower_val, upper_val, upper_bound
                )
            }
            FulltextQueryExpr::Wildcard(text) => text.clone(),
        }
    }

    fn evaluate_where_condition(
        &self,
        row: &HashMap<String, Value>,
        condition: &WhereCondition,
    ) -> bool {
        match condition {
            WhereCondition::Comparison(field, op, value) => {
                let row_value = match row.get(field) {
                    Some(v) => v,
                    None => return false,
                };
                self.compare_values(row_value, op, value)
            }
            WhereCondition::And(left, right) => {
                self.evaluate_where_condition(row, left)
                    && self.evaluate_where_condition(row, right)
            }
            WhereCondition::Or(left, right) => {
                self.evaluate_where_condition(row, left)
                    || self.evaluate_where_condition(row, right)
            }
            WhereCondition::Not(inner) => !self.evaluate_where_condition(row, inner),
            WhereCondition::FulltextMatch(_field, _query) => true,
        }
    }

    fn compare_values(&self, left: &Value, op: &ComparisonOp, right: &Value) -> bool {
        match op {
            ComparisonOp::Eq => left == right,
            ComparisonOp::Ne => left != right,
            ComparisonOp::Lt => self.compare_value_order(left, right) == Ordering::Less,
            ComparisonOp::Le => {
                let cmp = self.compare_value_order(left, right);
                cmp == Ordering::Less || cmp == Ordering::Equal
            }
            ComparisonOp::Gt => self.compare_value_order(left, right) == Ordering::Greater,
            ComparisonOp::Ge => {
                let cmp = self.compare_value_order(left, right);
                cmp == Ordering::Greater || cmp == Ordering::Equal
            }
        }
    }

    fn compare_value_order(&self, left: &Value, right: &Value) -> Ordering {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => a.cmp(b),
            (Value::Int(a), Value::Double(b)) => {
                (*a as f64).partial_cmp(b).unwrap_or(Ordering::Equal)
            }
            (Value::Double(a), Value::Int(b)) => {
                a.partial_cmp(&(*b as f64)).unwrap_or(Ordering::Equal)
            }
            (Value::Double(a), Value::Double(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (Value::String(a), Value::String(b)) => a.cmp(b),
            (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
            _ => Ordering::Equal,
        }
    }

    fn apply_order_by(&self, rows: &mut [HashMap<String, Value>], order_clause: &OrderClause) {
        rows.sort_by(|a, b| {
            for item in &order_clause.items {
                let cmp = self.compare_rows_by_item(a, b, item);
                if cmp != Ordering::Equal {
                    return if item.order == FulltextOrderDirection::Desc {
                        cmp.reverse()
                    } else {
                        cmp
                    };
                }
            }
            Ordering::Equal
        });
    }

    fn compare_rows_by_item(
        &self,
        a: &HashMap<String, Value>,
        b: &HashMap<String, Value>,
        item: &OrderItem,
    ) -> Ordering {
        let val_a = a.get(&item.expr);
        let val_b = b.get(&item.expr);

        match (val_a, val_b) {
            (Some(va), Some(vb)) => self.compare_value_order(va, vb),
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
            (None, None) => Ordering::Equal,
        }
    }

    fn apply_default_sort(&self, rows: &mut [HashMap<String, Value>]) {
        rows.sort_by(|a, b| {
            let score_a = a
                .get("score")
                .and_then(|v| match v {
                    Value::Float(f) => Some(*f as f64),
                    Value::Double(f) => Some(*f),
                    Value::Int(i) => Some(*i as f64),
                    Value::BigInt(i) => Some(*i as f64),
                    _ => None,
                })
                .unwrap_or(0.0);
            let score_b = b
                .get("score")
                .and_then(|v| match v {
                    Value::Float(f) => Some(*f as f64),
                    Value::Double(f) => Some(*f),
                    Value::Int(i) => Some(*i as f64),
                    Value::BigInt(i) => Some(*i as f64),
                    _ => None,
                })
                .unwrap_or(0.0);
            score_b.partial_cmp(&score_a).unwrap_or(Ordering::Equal)
        });
    }
}

/// Configuration for full-text scan executor
pub struct FulltextScanConfig {
    /// Index name
    pub index_name: String,
    /// Search query
    pub query: String,
    /// Limit
    pub limit: Option<usize>,
    /// Pre-resolved space_id
    pub space_id: u64,
    /// Pre-resolved tag_name
    pub tag_name: String,
    /// Pre-resolved field_name
    pub field_name: String,
}

/// Full-text scan executor for LOOKUP FULLTEXT operations
pub struct FulltextScanExecutor<S: StorageReader> {
    /// Base executor
    base: BaseExecutor<S>,
    /// Index name
    index_name: String,
    /// Search query
    query: String,
    /// Fulltext manager
    fulltext_manager: Arc<FulltextIndexManager>,
    /// Limit
    limit: Option<usize>,
    /// Pre-resolved space_id
    space_id: u64,
    /// Pre-resolved tag_name
    tag_name: String,
    /// Pre-resolved field_name
    field_name: String,
    _phantom: std::marker::PhantomData<S>,
}

impl<S: StorageReader> FulltextScanExecutor<S> {
    /// Create a new full-text scan executor
    pub fn new(
        id: i64,
        config: FulltextScanConfig,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionAnalysisContext>,
        fulltext_manager: Arc<FulltextIndexManager>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "FulltextScanExecutor".to_string(),
                storage,
                expr_context,
            ),
            index_name: config.index_name,
            query: config.query,
            fulltext_manager,
            limit: config.limit,
            space_id: config.space_id,
            tag_name: config.tag_name,
            field_name: config.field_name,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Resolve metadata from fulltext_manager if not pre-resolved
    fn resolve_metadata(&self) -> DBResult<(u64, String, String)> {
        if !self.tag_name.is_empty() && !self.field_name.is_empty() {
            return Ok((
                self.space_id,
                self.tag_name.clone(),
                self.field_name.clone(),
            ));
        }

        let indexes = self.fulltext_manager.list_indexes();

        for index in indexes {
            if index.index_name == self.index_name {
                return Ok((index.space_id, index.tag_name, index.field_name));
            }
        }

        Err(DBError::validation(format!(
            "Fulltext index '{}' not found",
            self.index_name
        )))
    }
}

impl<S: StorageReader> Executor<S> for FulltextSearchExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let (space_id, tag_name, field_name) = self.resolve_metadata()?;

        let query_string = self.convert_query_to_string(&self.statement.query);

        let limit = self.statement.limit.unwrap_or(100);

        let search_results = futures::executor::block_on(self.fulltext_manager.search(
            space_id,
            &tag_name,
            &field_name,
            &query_string,
            limit,
        ))
        .map_err(DBError::from)?;

        let mut rows = Vec::new();
        let storage = self.get_storage().clone();
        let storage_guard = storage.read();

        for result in search_results {
            let vertex_id = VertexId::try_from(&result.doc_id).map_err(DBError::from)?;

            let vertex = storage_guard
                .get_vertex("", &vertex_id)
                .map_err(DBError::from)?;

            if let Some(vertex) = vertex {
                let mut row: HashMap<String, Value> = HashMap::new();

                if let Some(yield_clause) = &self.statement.yield_clause {
                    for yield_item in &yield_clause.items {
                        let value = match &yield_item.expr {
                            YieldExpression::Field(name) => {
                                if let Some(tag) = vertex.tags.first() {
                                    tag.properties
                                        .get(name)
                                        .cloned()
                                        .unwrap_or(Value::Null(crate::core::null::NullType::Null))
                                } else {
                                    Value::Null(crate::core::null::NullType::Null)
                                }
                            }
                            YieldExpression::Score(_) => Value::Double(result.score as f64),
                            YieldExpression::Highlight(_, _) => {
                                if let Some(ref highlights) = result.highlights {
                                    Value::String(highlights.join(" ... "))
                                } else {
                                    Value::Null(crate::core::null::NullType::Null)
                                }
                            }
                            YieldExpression::MatchedFields => {
                                let fields: Vec<Value> = result
                                    .matched_fields
                                    .iter()
                                    .map(|f| Value::String(f.clone()))
                                    .collect();
                                Value::list(crate::core::value::list::List { values: fields })
                            }
                            YieldExpression::Snippet(field_name, max_len) => {
                                if let Some(tag) = vertex.tags.first() {
                                    if let Some(Value::String(text)) =
                                        tag.properties.get(field_name)
                                    {
                                        let max_len = max_len.unwrap_or(200);
                                        if text.len() <= max_len {
                                            Value::String(text.clone())
                                        } else {
                                            let break_point =
                                                text[..max_len].rfind(' ').unwrap_or(max_len);
                                            Value::String(format!("{}...", &text[..break_point]))
                                        }
                                    } else {
                                        Value::Null(crate::core::null::NullType::Null)
                                    }
                                } else {
                                    Value::Null(crate::core::null::NullType::Null)
                                }
                            }
                            YieldExpression::All => {
                                if let Some(tag) = vertex.tags.first() {
                                    for (k, v) in &tag.properties {
                                        row.insert(k.clone(), v.clone());
                                    }
                                }
                                continue;
                            }
                        };

                        let default_alias = match &yield_item.expr {
                            YieldExpression::Field(name) => name.clone(),
                            YieldExpression::Score(_) => "score".to_string(),
                            YieldExpression::Highlight(field, _) => format!("highlight({})", field),
                            YieldExpression::MatchedFields => "matched_fields()".to_string(),
                            YieldExpression::Snippet(field, _) => format!("snippet({})", field),
                            YieldExpression::All => "*".to_string(),
                        };
                        let alias = yield_item.alias.as_ref().unwrap_or(&default_alias);

                        row.insert(alias.clone(), value);
                    }
                } else {
                    row.insert("doc_id".to_string(), result.doc_id.clone());
                    row.insert("score".to_string(), Value::Double(result.score as f64));
                }

                rows.push(row);
            }
        }

        if let Some(where_clause) = &self.statement.where_clause {
            let condition = where_clause.condition.clone();
            rows.retain(|row| self.evaluate_where_condition(row, &condition));
        }

        if let Some(order_clause) = &self.statement.order_clause {
            self.apply_order_by(&mut rows, order_clause);
        } else {
            self.apply_default_sort(&mut rows);
        }

        if let Some(offset) = self.statement.offset {
            rows = rows.into_iter().skip(offset).collect();
        }

        if let Some(limit) = self.statement.limit {
            rows = rows.into_iter().take(limit).collect();
        }

        let mut dataset = crate::query::DataSet::new();
        if let Some(first_row) = rows.first() {
            for key in first_row.keys() {
                dataset.col_names.push(key.clone());
            }
        }
        for row in rows {
            let values: Vec<Value> = dataset
                .col_names
                .iter()
                .map(|k| {
                    row.get(k)
                        .cloned()
                        .unwrap_or(Value::Null(crate::core::null::NullType::Null))
                })
                .collect();
            dataset.rows.push(values);
        }
        Ok(ExecutionResult::DataSet(dataset))
    }

    fn open(&mut self) -> DBResult<()> {
        self.base.open()
    }

    fn close(&mut self) -> DBResult<()> {
        self.base.close()
    }

    fn is_open(&self) -> bool {
        self.base.is_open()
    }

    fn id(&self) -> i64 {
        self.base.id()
    }

    fn name(&self) -> &str {
        "FulltextSearchExecutor"
    }

    fn description(&self) -> &str {
        "Fulltext Search Executor"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.stats_mut()
    }
}

impl<S: StorageReader> HasStorage<S> for FulltextSearchExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageReader> Executor<S> for FulltextScanExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let (space_id, tag_name, field_name) = self.resolve_metadata()?;

        let limit = self.limit.unwrap_or(100);

        let search_results = futures::executor::block_on(self.fulltext_manager.search(
            space_id,
            &tag_name,
            &field_name,
            &self.query,
            limit,
        ))
        .map_err(DBError::from)?;

        let mut rows = Vec::new();
        let storage = self.get_storage().clone();
        let storage_guard = storage.read();

        for result in search_results {
            let vertex_id = VertexId::try_from(&result.doc_id).map_err(DBError::from)?;

            let vertex = storage_guard
                .get_vertex("", &vertex_id)
                .map_err(DBError::from)?;

            if let Some(vertex) = vertex {
                let mut row: HashMap<String, Value> = HashMap::new();
                row.insert("doc_id".to_string(), result.doc_id.clone());
                row.insert("score".to_string(), Value::Double(result.score as f64));

                if let Some(tag) = vertex.tags.first() {
                    for (k, v) in &tag.properties {
                        row.insert(k.clone(), v.clone());
                    }
                }

                rows.push(row);
            }
        }

        rows.sort_by(|a, b| {
            let score_a = a
                .get("score")
                .and_then(|v| match v {
                    Value::Float(f) => Some(*f as f64),
                    Value::Double(f) => Some(*f),
                    Value::Int(i) => Some(*i as f64),
                    Value::BigInt(i) => Some(*i as f64),
                    _ => None,
                })
                .unwrap_or(0.0);
            let score_b = b
                .get("score")
                .and_then(|v| match v {
                    Value::Float(f) => Some(*f as f64),
                    Value::Double(f) => Some(*f),
                    Value::Int(i) => Some(*i as f64),
                    Value::BigInt(i) => Some(*i as f64),
                    _ => None,
                })
                .unwrap_or(0.0);
            score_b.partial_cmp(&score_a).unwrap_or(Ordering::Equal)
        });

        if let Some(limit) = self.limit {
            rows = rows.into_iter().take(limit).collect();
        }

        let mut dataset = crate::query::DataSet::new();
        if let Some(first_row) = rows.first() {
            for key in first_row.keys() {
                dataset.col_names.push(key.clone());
            }
        }
        for row in rows {
            let values: Vec<Value> = dataset
                .col_names
                .iter()
                .map(|k| {
                    row.get(k)
                        .cloned()
                        .unwrap_or(Value::Null(crate::core::null::NullType::Null))
                })
                .collect();
            dataset.rows.push(values);
        }
        Ok(ExecutionResult::DataSet(dataset))
    }

    fn open(&mut self) -> DBResult<()> {
        self.base.open()
    }

    fn close(&mut self) -> DBResult<()> {
        self.base.close()
    }

    fn is_open(&self) -> bool {
        self.base.is_open()
    }

    fn id(&self) -> i64 {
        self.base.id()
    }

    fn name(&self) -> &str {
        "FulltextScanExecutor"
    }

    fn description(&self) -> &str {
        "Fulltext Scan Executor"
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.stats_mut()
    }
}

impl<S: StorageReader> HasStorage<S> for FulltextScanExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

#[cfg(all(test, feature = "fulltext-search"))]
mod tests {
    use super::*;
    use crate::storage::MockStorage;

    fn create_test_executor() -> FulltextSearchExecutor<MockStorage> {
        let statement = SearchStatement::new(
            "1_article_content".to_string(),
            FulltextQueryExpr::Simple("test".to_string()),
        );

        let config = crate::search::config::FulltextConfig::default();
        let fulltext_manager = std::sync::Arc::new(
            crate::search::manager::FulltextIndexManager::new(config)
                .expect("Failed to create manager"),
        );

        FulltextSearchExecutor {
            base: BaseExecutor::new(
                1,
                "TestExecutor".to_string(),
                std::sync::Arc::new(parking_lot::RwLock::new(
                    MockStorage::new().expect("Failed to create MockStorage"),
                )),
                std::sync::Arc::new(ExpressionAnalysisContext::new()),
            ),
            statement,
            fulltext_manager,
            space_id: 0,
            tag_name: String::new(),
            field_name: String::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    #[test]
    fn test_executor_creation() {
        let statement = SearchStatement::new(
            "test_index".to_string(),
            FulltextQueryExpr::Simple("test".to_string()),
        );

        assert_eq!(statement.index_name, "test_index");
    }

    #[test]
    fn test_query_conversion() {
        let simple = FulltextQueryExpr::Simple("database".to_string());
        assert!(matches!(simple, FulltextQueryExpr::Simple(_)));

        let field = FulltextQueryExpr::Field("title".to_string(), "database".to_string());
        assert!(matches!(field, FulltextQueryExpr::Field(_, _)));

        let boolean = FulltextQueryExpr::Boolean {
            must: vec![],
            should: vec![],
            must_not: vec![],
        };
        assert!(matches!(boolean, FulltextQueryExpr::Boolean { .. }));
    }

    #[test]
    fn test_where_condition_evaluation() {
        let mut row = HashMap::new();
        row.insert("score".to_string(), Value::Float(0.8));
        row.insert(
            "title".to_string(),
            Value::String("test article".to_string()),
        );

        let condition =
            WhereCondition::Comparison("score".to_string(), ComparisonOp::Gt, Value::Float(0.5));

        let executor = create_test_executor();
        assert!(executor.evaluate_where_condition(&row, &condition));

        let condition_false =
            WhereCondition::Comparison("score".to_string(), ComparisonOp::Gt, Value::Float(0.9));
        assert!(!executor.evaluate_where_condition(&row, &condition_false));
    }

    #[test]
    fn test_where_and_condition() {
        let mut row = HashMap::new();
        row.insert("score".to_string(), Value::Float(0.8));
        row.insert("views".to_string(), Value::Int(100));

        let condition = WhereCondition::And(
            Box::new(WhereCondition::Comparison(
                "score".to_string(),
                ComparisonOp::Gt,
                Value::Float(0.5),
            )),
            Box::new(WhereCondition::Comparison(
                "views".to_string(),
                ComparisonOp::Ge,
                Value::Int(100),
            )),
        );

        let executor = create_test_executor();
        assert!(executor.evaluate_where_condition(&row, &condition));
    }

    #[test]
    fn test_where_or_condition() {
        let mut row = HashMap::new();
        row.insert("score".to_string(), Value::Float(0.3));
        row.insert("priority".to_string(), Value::Int(10));

        let condition = WhereCondition::Or(
            Box::new(WhereCondition::Comparison(
                "score".to_string(),
                ComparisonOp::Gt,
                Value::Float(0.5),
            )),
            Box::new(WhereCondition::Comparison(
                "priority".to_string(),
                ComparisonOp::Gt,
                Value::Int(5),
            )),
        );

        let executor = create_test_executor();
        assert!(executor.evaluate_where_condition(&row, &condition));
    }

    #[test]
    fn test_order_by_sorting() {
        let mut row1 = HashMap::new();
        row1.insert("score".to_string(), Value::Float(0.5));
        row1.insert("title".to_string(), Value::String("b article".to_string()));

        let mut row2 = HashMap::new();
        row2.insert("score".to_string(), Value::Float(0.8));
        row2.insert("title".to_string(), Value::String("a article".to_string()));

        let mut row3 = HashMap::new();
        row3.insert("score".to_string(), Value::Float(0.6));
        row3.insert("title".to_string(), Value::String("c article".to_string()));

        let mut rows = vec![row1, row2, row3];

        let order_clause = OrderClause {
            items: vec![OrderItem {
                expr: "score".to_string(),
                order: FulltextOrderDirection::Desc,
            }],
        };

        let executor = create_test_executor();
        executor.apply_order_by(&mut rows, &order_clause);

        assert_eq!(rows[0].get("score"), Some(&Value::Float(0.8)));
        assert_eq!(rows[1].get("score"), Some(&Value::Float(0.6)));
        assert_eq!(rows[2].get("score"), Some(&Value::Float(0.5)));
    }

    #[test]
    fn test_default_sort() {
        let mut row1 = HashMap::new();
        row1.insert("score".to_string(), Value::Float(0.3));

        let mut row2 = HashMap::new();
        row2.insert("score".to_string(), Value::Float(0.9));

        let mut row3 = HashMap::new();
        row3.insert("score".to_string(), Value::Float(0.5));

        let mut rows = vec![row1, row2, row3];

        let executor = create_test_executor();
        executor.apply_default_sort(&mut rows);

        assert_eq!(rows[0].get("score"), Some(&Value::Float(0.9)));
        assert_eq!(rows[1].get("score"), Some(&Value::Float(0.5)));
        assert_eq!(rows[2].get("score"), Some(&Value::Float(0.3)));
    }

    #[test]
    fn test_value_comparison() {
        let executor = create_test_executor();

        assert!(executor.compare_values(&Value::Int(5), &ComparisonOp::Eq, &Value::Int(5)));
        assert!(!executor.compare_values(&Value::Int(5), &ComparisonOp::Eq, &Value::Int(3)));
        assert!(executor.compare_values(&Value::Int(5), &ComparisonOp::Gt, &Value::Int(3)));
        assert!(executor.compare_values(&Value::Float(0.5), &ComparisonOp::Lt, &Value::Float(0.8)));
        assert!(executor.compare_values(
            &Value::String("abc".to_string()),
            &ComparisonOp::Eq,
            &Value::String("abc".to_string())
        ));
    }
}
