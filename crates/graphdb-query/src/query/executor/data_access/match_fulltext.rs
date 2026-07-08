//! Match Fulltext Executor

use parking_lot::RwLock;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::error::DBError;
use crate::core::types::VertexId;
use crate::core::Value;
use crate::query::executor::base::{BaseExecutor, DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::parser::ast::fulltext::{
    FulltextMatchCondition, FulltextYieldClause, YieldExpression,
};
use crate::query::validator::context::ExpressionAnalysisContext;
#[cfg(feature = "fulltext-search")]
use crate::search::manager::FulltextIndexManager;
use crate::storage::StorageReader;

/// Executor for MATCH FULLTEXT operations
pub struct MatchFulltextExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
    /// Full-text match condition
    fulltext_condition: FulltextMatchCondition,
    /// Yield clause
    yield_clause: Option<FulltextYieldClause>,
    /// Fulltext manager
    fulltext_manager: Arc<FulltextIndexManager>,
    /// Pre-resolved space_id
    space_id: u64,
    /// Pre-resolved tag_name
    tag_name: String,
    /// Pre-resolved field_name
    field_name: String,
}

impl<S: StorageReader> MatchFulltextExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        fulltext_condition: FulltextMatchCondition,
        yield_clause: Option<FulltextYieldClause>,
        expr_context: Arc<ExpressionAnalysisContext>,
        fulltext_manager: Arc<FulltextIndexManager>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(
                id,
                "MatchFulltextExecutor".to_string(),
                storage,
                expr_context,
            ),
            fulltext_condition,
            yield_clause,
            fulltext_manager,
            space_id: 0,
            tag_name: String::new(),
            field_name: String::new(),
        }
    }

    pub fn with_metadata(mut self, space_id: u64, tag_name: String, field_name: String) -> Self {
        self.space_id = space_id;
        self.tag_name = tag_name;
        self.field_name = field_name;
        self
    }

    fn resolve_metadata(&self) -> DBResult<(u64, String, String)> {
        if !self.tag_name.is_empty() && !self.field_name.is_empty() {
            return Ok((
                self.space_id,
                self.tag_name.clone(),
                self.field_name.clone(),
            ));
        }

        let index_name = self
            .fulltext_condition
            .index_name
            .as_ref()
            .ok_or_else(|| DBError::validation("Index name is required for MATCH FULLTEXT"))?;

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
}

impl<S: StorageReader> HasStorage<S> for MatchFulltextExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

impl<S: StorageReader> Executor<S> for MatchFulltextExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let field = &self.fulltext_condition.field;
        let query = &self.fulltext_condition.query;

        let (space_id, tag_name, _field_name) = self.resolve_metadata()?;

        let limit = 100;

        let search_results = futures::executor::block_on(
            self.fulltext_manager
                .search(space_id, &tag_name, field, query, limit),
        )
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

                if let Some(ref yield_clause) = self.yield_clause {
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

        rows.sort_by(|a: &HashMap<String, Value>, b: &HashMap<String, Value>| {
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
        "MatchFulltextExecutor"
    }

    fn description(&self) -> &str {
        "Executor for MATCH FULLTEXT operations"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.stats_mut()
    }
}
