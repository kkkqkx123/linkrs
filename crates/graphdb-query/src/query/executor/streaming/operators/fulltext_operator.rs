use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operator_base::OperatorBase;
#[cfg(feature = "fulltext-search")]
use crate::query::executor::streaming::operators::ddl_operator::make_single_row;
#[cfg(feature = "fulltext-search")]
use crate::search::manager::FulltextIndexManager;
use crate::storage::StorageClient;

fn make_manage_result(action: &str, name: Option<&str>, status: &str) -> DataChunk {
    let name_val = name
        .map(|n| Value::String(n.to_string()))
        .unwrap_or(Value::Null(crate::core::NullType::Null));
    let schema = Arc::new(Schema::new(vec![
        ColumnInfo {
            name: "action".to_string(),
            data_type: "string".to_string(),
        },
        ColumnInfo {
            name: "name".to_string(),
            data_type: "string".to_string(),
        },
        ColumnInfo {
            name: "status".to_string(),
            data_type: "string".to_string(),
        },
    ]));
    DataChunk::new(
        vec![vec![
            Value::String(action.to_string()),
            name_val,
            Value::String(status.to_string()),
        ]],
        schema,
    )
}

#[derive(Debug)]
pub enum FulltextOperator {
    FulltextManage {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        space_id: u64,
        action: String,
        index_name: Option<String>,
        tag_name: Option<String>,
        field_name: Option<String>,
        #[cfg(feature = "fulltext-search")]
        fulltext_manager: Option<Arc<FulltextIndexManager>>,
    },
    FulltextSearch {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        space_id: u64,
        index_name: String,
        search_query: String,
        tag_name: String,
        field_name: String,
        #[cfg(feature = "fulltext-search")]
        fulltext_manager: Option<Arc<FulltextIndexManager>>,
    },
    FulltextLookup {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        space_id: u64,
        index_name: String,
        search_query: String,
        tag_name: String,
        field_name: String,
        #[cfg(feature = "fulltext-search")]
        fulltext_manager: Option<Arc<FulltextIndexManager>>,
    },
    MatchFulltext {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        match_expr: Expression,
        match_field: Option<String>,
        tag_name: String,
        field_name: String,
        #[cfg(feature = "fulltext-search")]
        fulltext_manager: Option<Arc<FulltextIndexManager>>,
    },
}

impl FulltextOperator {
    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            FulltextOperator::FulltextManage { .. }
            | FulltextOperator::FulltextSearch { .. }
            | FulltextOperator::FulltextLookup { .. }
            | FulltextOperator::MatchFulltext { .. } => {
                input.open()?;
                base.lifecycle.mark_opened();
                Ok(())
            }
        }
    }

    pub fn next(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        match self {
            FulltextOperator::FulltextManage {
                storage: _storage,
                space_name: _space_name,
                space_id: _space_id,
                action: _action,
                index_name: _index_name,
                tag_name: _tag_name,
                field_name: _field_name,
                #[cfg(feature = "fulltext-search")]
                fulltext_manager,
            } => {
                if !base.lifecycle.is_opened() {
                    return Ok(None);
                }
                base.lifecycle.mark_closed();

                #[cfg(feature = "fulltext-search")]
                {
                    let result = match _action.as_str() {
                        "create_fulltext_index" | "create" => {
                            if let Some(manager) = fulltext_manager {
                                let sid = *_space_id;
                                let tn = _tag_name.as_deref().unwrap_or("");
                                let fn_ = _field_name.as_deref().unwrap_or("");
                                futures::executor::block_on(
                                    manager.create_index(sid, tn, fn_, None),
                                )
                                .map_err(|e| {
                                    QueryError::execution(format!("Fulltext create failed: {}", e))
                                })?;
                                Some(make_manage_result(
                                    "create_fulltext_index",
                                    _index_name.as_deref(),
                                    "created",
                                ))
                            } else {
                                Some(make_manage_result(
                                    "create_fulltext_index",
                                    _index_name.as_deref(),
                                    "no-manager",
                                ))
                            }
                        }
                        "drop_fulltext_index" | "drop" => {
                            if let Some(manager) = fulltext_manager {
                                let sid = *_space_id;
                                let tn = _tag_name.as_deref().unwrap_or("");
                                let fn_ = _field_name.as_deref().unwrap_or("");
                                futures::executor::block_on(manager.drop_index(sid, tn, fn_))
                                    .map_err(|e| {
                                        QueryError::execution(format!(
                                            "Fulltext drop failed: {}",
                                            e
                                        ))
                                    })?;
                                Some(make_manage_result(
                                    "drop_fulltext_index",
                                    _index_name.as_deref(),
                                    "dropped",
                                ))
                            } else {
                                Some(make_manage_result(
                                    "drop_fulltext_index",
                                    _index_name.as_deref(),
                                    "no-manager",
                                ))
                            }
                        }
                        "describe_fulltext_index" | "desc" => {
                            if let Some(manager) = fulltext_manager {
                                let sid = *_space_id;
                                let tn = _tag_name.as_deref().unwrap_or("");
                                let fn_ = _field_name.as_deref().unwrap_or("");
                                if let Some(meta) = manager.get_metadata(sid, tn, fn_) {
                                    let schema = Arc::new(Schema::new(vec![
                                        ColumnInfo {
                                            name: "index_id".to_string(),
                                            data_type: "string".to_string(),
                                        },
                                        ColumnInfo {
                                            name: "tag_name".to_string(),
                                            data_type: "string".to_string(),
                                        },
                                        ColumnInfo {
                                            name: "field_name".to_string(),
                                            data_type: "string".to_string(),
                                        },
                                        ColumnInfo {
                                            name: "status".to_string(),
                                            data_type: "string".to_string(),
                                        },
                                    ]));
                                    Some(make_single_row(
                                        schema,
                                        vec![
                                            Value::String(meta.index_name),
                                            Value::String(meta.tag_name),
                                            Value::String(meta.field_name),
                                            Value::String(format!("{:?}", meta.status)),
                                        ],
                                    ))
                                } else {
                                    Some(make_manage_result(
                                        "describe_fulltext_index",
                                        _index_name.as_deref(),
                                        "not-found",
                                    ))
                                }
                            } else {
                                Some(make_manage_result(
                                    "describe_fulltext_index",
                                    _index_name.as_deref(),
                                    "no-manager",
                                ))
                            }
                        }
                        "show_fulltext_indexes" | "show" => {
                            if let Some(manager) = fulltext_manager {
                                let indexes = manager.list_indexes();
                                let schema = Arc::new(Schema::new(vec![
                                    ColumnInfo {
                                        name: "index_id".to_string(),
                                        data_type: "string".to_string(),
                                    },
                                    ColumnInfo {
                                        name: "space_id".to_string(),
                                        data_type: "bigint".to_string(),
                                    },
                                    ColumnInfo {
                                        name: "tag_name".to_string(),
                                        data_type: "string".to_string(),
                                    },
                                    ColumnInfo {
                                        name: "field_name".to_string(),
                                        data_type: "string".to_string(),
                                    },
                                    ColumnInfo {
                                        name: "status".to_string(),
                                        data_type: "string".to_string(),
                                    },
                                ]));
                                let rows: Vec<Vec<Value>> = indexes
                                    .into_iter()
                                    .map(|m| {
                                        vec![
                                            Value::String(m.index_name),
                                            Value::BigInt(m.space_id as i64),
                                            Value::String(m.tag_name),
                                            Value::String(m.field_name),
                                            Value::String(format!("{:?}", m.status)),
                                        ]
                                    })
                                    .collect();
                                Some(DataChunk::new(rows, schema))
                            } else {
                                Some(make_manage_result(
                                    "show_fulltext_indexes",
                                    None,
                                    "no-manager",
                                ))
                            }
                        }
                        _ => {
                            return Err(QueryError::execution(format!(
                                "Unsupported fulltext action: {}",
                                _action
                            )));
                        }
                    };
                    Ok(result)
                }

                #[cfg(not(feature = "fulltext-search"))]
                Ok(Some(make_manage_result(
                    _action,
                    _index_name.as_deref(),
                    "fulltext-search feature disabled",
                )))
            }

            FulltextOperator::FulltextSearch {
                search_query,
                space_id,
                tag_name,
                field_name,
                #[cfg(feature = "fulltext-search")]
                fulltext_manager,
                ..
            } => {
                #[cfg(not(feature = "fulltext-search"))]
                let _ = (&search_query, &space_id, &tag_name, &field_name);

                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution(
                        "FulltextSearch not opened".to_string(),
                    ));
                }

                #[cfg(feature = "fulltext-search")]
                {
                    if let Some(manager) = fulltext_manager {
                        let search_results = futures::executor::block_on(manager.search(
                            *space_id,
                            tag_name,
                            field_name,
                            search_query,
                            100,
                        ))
                        .map_err(|e| {
                            QueryError::execution(format!("Fulltext search failed: {}", e))
                        })?;
                        let mut rows = Vec::new();
                        for result in search_results {
                            rows.push(vec![result.doc_id, Value::Double(result.score as f64)]);
                        }
                        return if rows.is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some(DataChunk::from_rows(rows)))
                        };
                    }
                }

                if let Some(chunk) = input.advance()? {
                    return Ok(Some(chunk));
                }
                Ok(None)
            }

            FulltextOperator::FulltextLookup {
                search_query,
                space_id,
                tag_name,
                field_name,
                #[cfg(feature = "fulltext-search")]
                fulltext_manager,
                ..
            } => {
                #[cfg(not(feature = "fulltext-search"))]
                let _ = (&search_query, &space_id, &tag_name, &field_name);

                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution(
                        "FulltextLookup not opened".to_string(),
                    ));
                }

                #[cfg(feature = "fulltext-search")]
                {
                    if let Some(manager) = fulltext_manager {
                        let search_results = futures::executor::block_on(manager.search(
                            *space_id,
                            tag_name,
                            field_name,
                            search_query,
                            100,
                        ))
                        .map_err(|e| {
                            QueryError::execution(format!("Fulltext lookup failed: {}", e))
                        })?;
                        let mut rows = Vec::new();
                        for result in search_results {
                            rows.push(vec![result.doc_id, Value::Double(result.score as f64)]);
                        }
                        return if rows.is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some(DataChunk::from_rows(rows)))
                        };
                    }
                }

                if let Some(chunk) = input.advance()? {
                    return Ok(Some(chunk));
                }
                Ok(None)
            }

            FulltextOperator::MatchFulltext {
                match_expr,
                tag_name,
                field_name,
                #[cfg(feature = "fulltext-search")]
                fulltext_manager,
                ..
            } => {
                #[cfg(not(feature = "fulltext-search"))]
                let _ = (&match_expr, &tag_name, &field_name);

                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution(
                        "MatchFulltext not opened".to_string(),
                    ));
                }

                #[cfg(feature = "fulltext-search")]
                {
                    if let Some(manager) = fulltext_manager {
                        let expr_str = format!("{:?}", match_expr);
                        let space_id = 0;
                        let search_results = futures::executor::block_on(
                            manager.search(space_id, tag_name, field_name, &expr_str, 100),
                        )
                        .map_err(|e| {
                            QueryError::execution(format!("Fulltext match failed: {}", e))
                        })?;
                        let mut rows = Vec::new();
                        for result in search_results {
                            rows.push(vec![result.doc_id, Value::Double(result.score as f64)]);
                        }
                        base.lifecycle.mark_closed();
                        return if rows.is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some(DataChunk::from_rows(rows)))
                        };
                    }
                }

                if let Some(chunk) = input.advance()? {
                    base.lifecycle.mark_closed();
                    return Ok(Some(chunk));
                }
                base.lifecycle.mark_closed();
                Ok(None)
            }
        }
    }

    pub fn stop(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            FulltextOperator::FulltextManage { .. }
            | FulltextOperator::FulltextSearch { .. }
            | FulltextOperator::FulltextLookup { .. }
            | FulltextOperator::MatchFulltext { .. } => {
                if base.lifecycle.can_close() {
                    input.stop()?;
                    base.lifecycle.mark_stopped();
                }
                Ok(())
            }
        }
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            FulltextOperator::FulltextManage { .. }
            | FulltextOperator::FulltextSearch { .. }
            | FulltextOperator::FulltextLookup { .. }
            | FulltextOperator::MatchFulltext { .. } => {
                if base.lifecycle.can_close() {
                    input.close()?;
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
        }
    }
}
