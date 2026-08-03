use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::base::OperatorBase;
#[cfg(feature = "fulltext-search")]
use crate::query::executor::streaming::operators::ddl_operator::make_single_row;
use crate::query::executor::streaming::operators::spec::FulltextManageCommand;
#[cfg(feature = "fulltext-search")]
use crate::search::manager::FulltextIndexManager;
use crate::storage::QueryStorage;

#[cfg(feature = "fulltext-search")]
use crate::query::executor::streaming::chunk::{ColumnInfo, Schema};

#[cfg(not(feature = "fulltext-search"))]
fn fulltext_command_info(cmd: &FulltextManageCommand) -> (&'static str, Option<&str>) {
    use crate::query::executor::streaming::operators::spec::FulltextManageCommand::*;
    match cmd {
        Create { index_name, .. } => ("create_fulltext_index", Some(index_name.as_str())),
        Drop { index_name, .. } => ("drop_fulltext_index", Some(index_name.as_str())),
        Alter { index_name, .. } => ("alter_fulltext_index", Some(index_name.as_str())),
        Show { pattern, .. } => ("show_fulltext_index", pattern.as_deref()),
        Describe { index_name, .. } => ("describe_fulltext_index", Some(index_name.as_str())),
    }
}

fn make_manage_result(
    output_layout: Arc<crate::query::executor::streaming::slot::SlotLayout>,
    action: &str,
    name: Option<&str>,
    status: &str,
) -> DataChunk {
    let name_val = name
        .map(Value::string)
        .unwrap_or(Value::Null(crate::core::NullType::Null));
    DataChunk::new_with_layout(
        vec![vec![Value::string(action), name_val, Value::string(status)]],
        output_layout,
    )
}

#[derive(Debug)]
pub enum FulltextOperator {
    FulltextManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        command: FulltextManageCommand,
        #[cfg(feature = "fulltext-search")]
        fulltext_manager: Option<Arc<FulltextIndexManager>>,
    },
    FulltextSearch {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
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
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
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
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
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
    /// Create a FulltextOperator from an immutable spec.
    pub fn from_spec(
        spec: &super::spec::FulltextSpec,
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        #[cfg(feature = "fulltext-search")] fulltext_manager: Option<Arc<FulltextIndexManager>>,
    ) -> Self {
        match spec {
            super::spec::FulltextSpec::FulltextManage {
                space_name,
                command,
            } => FulltextOperator::FulltextManage {
                storage: storage.clone(),
                space_name: space_name.clone(),
                command: command.clone(),
                #[cfg(feature = "fulltext-search")]
                fulltext_manager: fulltext_manager.clone(),
            },
            super::spec::FulltextSpec::FulltextSearch {
                space_name,
                space_id,
                index_name,
                search_query,
                tag_name,
                field_name,
            } => FulltextOperator::FulltextSearch {
                storage: storage.clone(),
                space_name: space_name.clone(),
                space_id: *space_id,
                index_name: index_name.clone(),
                search_query: search_query.clone(),
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                #[cfg(feature = "fulltext-search")]
                fulltext_manager: fulltext_manager.clone(),
            },
            super::spec::FulltextSpec::FulltextLookup {
                space_name,
                space_id,
                index_name,
                search_query,
                tag_name,
                field_name,
            } => FulltextOperator::FulltextLookup {
                storage: storage.clone(),
                space_name: space_name.clone(),
                space_id: *space_id,
                index_name: index_name.clone(),
                search_query: search_query.clone(),
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                #[cfg(feature = "fulltext-search")]
                fulltext_manager: fulltext_manager.clone(),
            },
            super::spec::FulltextSpec::MatchFulltext {
                space_name,
                match_expr,
                match_field,
                tag_name,
                field_name,
            } => FulltextOperator::MatchFulltext {
                storage: storage.clone(),
                space_name: space_name.clone(),
                match_expr: match_expr.clone(),
                match_field: match_field.clone(),
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                #[cfg(feature = "fulltext-search")]
                fulltext_manager: fulltext_manager.clone(),
            },
        }
    }

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
                command,
                #[cfg(feature = "fulltext-search")]
                fulltext_manager,
            } => {
                if !base.lifecycle.is_opened() {
                    return Ok(None);
                }
                base.lifecycle.mark_closed();

                #[cfg(feature = "fulltext-search")]
                {
                    use crate::query::executor::streaming::operators::spec::FulltextManageCommand::*;
                    let result = match command {
                        Create {
                            index_name,
                            schema_name,
                            fields,
                            space_id,
                        } => {
                            if let Some(manager) = fulltext_manager {
                                for field_name in fields {
                                    futures::executor::block_on(manager.create_index(
                                        *space_id,
                                        schema_name,
                                        field_name,
                                        None,
                                    ))
                                    .map_err(|e| {
                                        QueryError::execution(format!(
                                            "Fulltext create failed: {}",
                                            e
                                        ))
                                    })?;
                                }
                                Some(make_manage_result(
                                    Arc::clone(&base.output_layout),
                                    "create_fulltext_index",
                                    Some(index_name.as_str()),
                                    "created",
                                ))
                            } else {
                                Some(make_manage_result(
                                    Arc::clone(&base.output_layout),
                                    "create_fulltext_index",
                                    Some(index_name.as_str()),
                                    "no-manager",
                                ))
                            }
                        }
                        Drop {
                            index_name,
                            if_exists,
                        } => {
                            if let Some(manager) = fulltext_manager {
                                let matching: Vec<_> = manager
                                    .list_indexes()
                                    .into_iter()
                                    .filter(|metadata| metadata.index_name == *index_name)
                                    .collect();
                                if matching.is_empty() && !*if_exists {
                                    return Err(QueryError::execution(format!(
                                        "Fulltext index not found: {}",
                                        index_name
                                    )));
                                }
                                for metadata in matching {
                                    futures::executor::block_on(manager.drop_index(
                                        metadata.space_id,
                                        &metadata.tag_name,
                                        &metadata.field_name,
                                    ))
                                    .map_err(|e| {
                                        QueryError::execution(format!(
                                            "Fulltext drop failed: {}",
                                            e
                                        ))
                                    })?;
                                }
                                Some(make_manage_result(
                                    Arc::clone(&base.output_layout),
                                    "drop_fulltext_index",
                                    Some(index_name.as_str()),
                                    "dropped",
                                ))
                            } else {
                                Some(make_manage_result(
                                    Arc::clone(&base.output_layout),
                                    "drop_fulltext_index",
                                    Some(index_name.as_str()),
                                    "no-manager",
                                ))
                            }
                        }
                        Describe { index_name } => {
                            if let Some(manager) = fulltext_manager {
                                if let Some(meta) = manager
                                    .list_indexes()
                                    .into_iter()
                                    .find(|metadata| metadata.index_name == *index_name)
                                {
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
                                            Value::string(meta.index_name),
                                            Value::string(meta.tag_name),
                                            Value::string(meta.field_name),
                                            Value::string(format!("{:?}", meta.status)),
                                        ],
                                    ))
                                } else {
                                    Some(make_manage_result(
                                        Arc::clone(&base.output_layout),
                                        "describe_fulltext_index",
                                        Some(index_name.as_str()),
                                        "not-found",
                                    ))
                                }
                            } else {
                                Some(make_manage_result(
                                    Arc::clone(&base.output_layout),
                                    "describe_fulltext_index",
                                    Some(index_name.as_str()),
                                    "no-manager",
                                ))
                            }
                        }
                        Show {
                            pattern,
                            from_schema,
                        } => {
                            if let Some(manager) = fulltext_manager {
                                let indexes: Vec<_> = manager
                                    .list_indexes()
                                    .into_iter()
                                    .filter(|metadata| {
                                        pattern.as_ref().is_none_or(|pattern| {
                                            metadata.index_name.contains(pattern)
                                        }) && from_schema
                                            .as_ref()
                                            .is_none_or(|schema| &metadata.tag_name == schema)
                                    })
                                    .collect();
                                let _schema = Arc::new(Schema::new(vec![
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
                                            Value::string(m.index_name),
                                            Value::BigInt(m.space_id as i64),
                                            Value::string(m.tag_name),
                                            Value::string(m.field_name),
                                            Value::string(format!("{:?}", m.status)),
                                        ]
                                    })
                                    .collect();
                                Some(DataChunk::new_with_layout(
                                    rows,
                                    Arc::clone(&base.output_layout),
                                ))
                            } else {
                                Some(make_manage_result(
                                    Arc::clone(&base.output_layout),
                                    "show_fulltext_indexes",
                                    None,
                                    "no-manager",
                                ))
                            }
                        }
                        Alter { .. } => {
                            return Err(QueryError::execution(
                                "Fulltext ALTER is not supported by FulltextIndexManager"
                                    .to_string(),
                            ))
                        }
                    };
                    Ok(result)
                }

                #[cfg(not(feature = "fulltext-search"))]
                {
                    let (name, index_name) = fulltext_command_info(command);
                    Ok(Some(make_manage_result(
                        Arc::clone(&base.output_layout),
                        name,
                        index_name,
                        "fulltext-search feature disabled",
                    )))
                }
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
                            Ok(Some(DataChunk::new_with_layout(
                                rows,
                                base.output_layout.clone(),
                            )))
                        };
                    }
                }

                if let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection();
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
                            Ok(Some(DataChunk::new_with_layout(
                                rows,
                                base.output_layout.clone(),
                            )))
                        };
                    }
                }

                if let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection();
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
                            Ok(Some(DataChunk::new_with_layout(
                                rows,
                                base.output_layout.clone(),
                            )))
                        };
                    }
                }

                if let Some(mut chunk) = input.advance()? {
                    chunk.materialize_selection();
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
        _input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            FulltextOperator::FulltextManage { .. }
            | FulltextOperator::FulltextSearch { .. }
            | FulltextOperator::FulltextLookup { .. }
            | FulltextOperator::MatchFulltext { .. } => {
                if base.lifecycle.can_close() {
                    base.lifecycle.mark_stopped();
                }
                Ok(())
            }
        }
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        _input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            FulltextOperator::FulltextManage { .. }
            | FulltextOperator::FulltextSearch { .. }
            | FulltextOperator::FulltextLookup { .. }
            | FulltextOperator::MatchFulltext { .. } => {
                if base.lifecycle.can_close() {
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
        }
    }
}
