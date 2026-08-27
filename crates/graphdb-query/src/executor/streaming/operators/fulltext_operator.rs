use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
#[cfg(feature = "fulltext-search")]
use crate::core::Value;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
#[cfg(feature = "fulltext-search")]
use crate::executor::streaming::operators::ddl_operator::make_single_row;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::operators::spec::FulltextManageCommand;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
#[cfg(feature = "fulltext-search")]
use crate::search::manager::FulltextIndexManager;
use crate::storage::QueryStorage;

#[cfg(feature = "fulltext-search")]
use crate::executor::streaming::chunk::{ColumnInfo, Schema};

#[cfg(not(feature = "fulltext-search"))]
fn fulltext_command_info(cmd: &FulltextManageCommand) -> (&'static str, Option<&str>) {
    use crate::executor::streaming::operators::spec::FulltextManageCommand::*;
    match cmd {
        Create { index_name, .. } => ("create_fulltext_index", Some(index_name.as_str())),
        Drop { index_name, .. } => ("drop_fulltext_index", Some(index_name.as_str())),
        Alter { index_name, .. } => ("alter_fulltext_index", Some(index_name.as_str())),
        Show { pattern, .. } => ("show_fulltext_index", pattern.as_deref()),
        Describe { index_name, .. } => ("describe_fulltext_index", Some(index_name.as_str())),
    }
}

#[cfg(feature = "fulltext-search")]
fn make_manage_result(
    output_layout: Arc<SlotLayout>,
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
pub enum FulltextOperatorKind {
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

/// Fulltext operator.
///
/// Wraps [`FulltextOperatorKind`] with the runtime context injected at
/// `open()`. Lifecycle state is owned exclusively by the executor; operators
/// never write it.
#[derive(Debug)]
pub struct FulltextOperator {
    pub kind: FulltextOperatorKind,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
}

impl FulltextOperator {
    /// Create a FulltextOperator from an immutable spec.
    pub fn from_spec(
        spec: &super::spec::FulltextSpec,
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        #[cfg(feature = "fulltext-search")] fulltext_manager: Option<Arc<FulltextIndexManager>>,
        output_layout: Arc<SlotLayout>,
    ) -> Self {
        let kind = match spec {
            super::spec::FulltextSpec::FulltextManage {
                space_name,
                command,
            } => FulltextOperatorKind::FulltextManage {
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
            } => FulltextOperatorKind::FulltextSearch {
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
            } => FulltextOperatorKind::FulltextLookup {
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
            } => FulltextOperatorKind::MatchFulltext {
                storage: storage.clone(),
                space_name: space_name.clone(),
                match_expr: match_expr.clone(),
                match_field: match_field.clone(),
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                #[cfg(feature = "fulltext-search")]
                fulltext_manager: fulltext_manager.clone(),
            },
        };
        Self::new(kind, output_layout)
    }

    pub fn new(kind: FulltextOperatorKind, output_layout: Arc<SlotLayout>) -> Self {
        Self {
            kind,
            runtime: None,
            output_layout,
            config: OperatorConfig::default(),
        }
    }

    /// Inject the runtime and execution config (called once by the executor
    /// before this operator produces any data).
    pub fn inject_context(
        &mut self,
        runtime: Option<&Arc<ExecutionRuntime>>,
        config: OperatorConfig,
    ) {
        if let Some(rt) = runtime {
            self.runtime = Some(rt.clone());
        }
        self.config = config;
    }

    pub fn open(&mut self, input: &mut StreamingExecutor) -> Result<(), QueryError> {
        match &mut self.kind {
            FulltextOperatorKind::FulltextManage { .. }
            | FulltextOperatorKind::FulltextSearch { .. }
            | FulltextOperatorKind::FulltextLookup { .. }
            | FulltextOperatorKind::MatchFulltext { .. } => {
                input.open()?;
                Ok(())
            }
        }
    }

    pub fn next(&mut self, input: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
        match &mut self.kind {
            FulltextOperatorKind::FulltextManage {
                storage: _storage,
                space_name: _space_name,
                command,
                #[cfg(feature = "fulltext-search")]
                fulltext_manager,
            } => {
                #[cfg(feature = "fulltext-search")]
                {
                    use crate::executor::streaming::operators::spec::FulltextManageCommand::*;
                    let result = match command {
                        Create {
                            index_name,
                            schema_name,
                            fields,
                            space_id,
                        } => {
                            if let Some(manager) = fulltext_manager {
                                for field_name in fields {
                                    crate::executor::streaming::helpers::runtime_bridge::wait(
                                        "Fulltext create",
                                        manager.create_index(
                                            *space_id,
                                            schema_name,
                                            field_name,
                                            None,
                                        ),
                                    )?;
                                }
                                Some(make_manage_result(
                                    Arc::clone(&self.output_layout),
                                    "create_fulltext_index",
                                    Some(index_name.as_str()),
                                    "created",
                                ))
                            } else {
                                Some(make_manage_result(
                                    Arc::clone(&self.output_layout),
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
                                    crate::executor::streaming::helpers::runtime_bridge::wait(
                                        "Fulltext drop",
                                        manager.drop_index(
                                            metadata.space_id,
                                            &metadata.tag_name,
                                            &metadata.field_name,
                                        ),
                                    )?;
                                }
                                Some(make_manage_result(
                                    Arc::clone(&self.output_layout),
                                    "drop_fulltext_index",
                                    Some(index_name.as_str()),
                                    "dropped",
                                ))
                            } else {
                                Some(make_manage_result(
                                    Arc::clone(&self.output_layout),
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
                                        Arc::clone(&self.output_layout),
                                        "describe_fulltext_index",
                                        Some(index_name.as_str()),
                                        "not-found",
                                    ))
                                }
                            } else {
                                Some(make_manage_result(
                                    Arc::clone(&self.output_layout),
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
                                    Arc::clone(&self.output_layout),
                                ))
                            } else {
                                Some(make_manage_result(
                                    Arc::clone(&self.output_layout),
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
                    let (operation, _) = fulltext_command_info(command);
                    Err(QueryError::feature_disabled(
                        "fulltext-search",
                        &operation.to_uppercase(),
                    ))
                }
            }

            FulltextOperatorKind::FulltextSearch {
                search_query,
                space_id,
                tag_name,
                field_name,
                #[cfg(feature = "fulltext-search")]
                fulltext_manager,
                ..
            } => {
                #[cfg(feature = "fulltext-search")]
                {
                    if let Some(manager) = fulltext_manager {
                        let search_results =
                            crate::executor::streaming::helpers::runtime_bridge::wait(
                                "Fulltext search",
                                manager.search(*space_id, tag_name, field_name, search_query, 100),
                            )?;
                        let mut rows = Vec::new();
                        for result in search_results {
                            rows.push(vec![result.doc_id, Value::Double(result.score as f64)]);
                        }
                        if !rows.is_empty() {
                            return Ok(Some(DataChunk::new_with_layout(
                                rows,
                                self.output_layout.clone(),
                            )));
                        }
                        return Ok(None);
                    }

                    // No manager configured: fall through to the input.
                    if let Some(mut chunk) = input.advance()? {
                        chunk.materialize_selection_by("Fulltext");
                        return Ok(Some(chunk));
                    }
                    Ok(None)
                }

                #[cfg(not(feature = "fulltext-search"))]
                {
                    let _ = (&search_query, &space_id, &tag_name, &field_name, input);
                    Err(QueryError::feature_disabled(
                        "fulltext-search",
                        "FULLTEXT SEARCH",
                    ))
                }
            }

            FulltextOperatorKind::FulltextLookup {
                search_query,
                space_id,
                tag_name,
                field_name,
                #[cfg(feature = "fulltext-search")]
                fulltext_manager,
                ..
            } => {
                #[cfg(feature = "fulltext-search")]
                {
                    if let Some(manager) = fulltext_manager {
                        let search_results =
                            crate::executor::streaming::helpers::runtime_bridge::wait(
                                "Fulltext lookup",
                                manager.search(*space_id, tag_name, field_name, search_query, 100),
                            )?;
                        let mut rows = Vec::new();
                        for result in search_results {
                            rows.push(vec![result.doc_id, Value::Double(result.score as f64)]);
                        }
                        return if rows.is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some(DataChunk::new_with_layout(
                                rows,
                                self.output_layout.clone(),
                            )))
                        };
                    }

                    // No manager configured: fall through to the input.
                    if let Some(mut chunk) = input.advance()? {
                        chunk.materialize_selection_by("Fulltext");
                        return Ok(Some(chunk));
                    }
                    Ok(None)
                }

                #[cfg(not(feature = "fulltext-search"))]
                {
                    let _ = (&search_query, &space_id, &tag_name, &field_name, input);
                    Err(QueryError::feature_disabled(
                        "fulltext-search",
                        "FULLTEXT LOOKUP",
                    ))
                }
            }

            FulltextOperatorKind::MatchFulltext {
                match_expr,
                tag_name,
                field_name,
                #[cfg(feature = "fulltext-search")]
                fulltext_manager,
                ..
            } => {
                #[cfg(feature = "fulltext-search")]
                {
                    if let Some(manager) = fulltext_manager {
                        let expr_str = format!("{:?}", match_expr);
                        let space_id = 0;
                        let search_results =
                            crate::executor::streaming::helpers::runtime_bridge::wait(
                                "Fulltext match",
                                manager.search(space_id, tag_name, field_name, &expr_str, 100),
                            )?;
                        let mut rows = Vec::new();
                        for result in search_results {
                            rows.push(vec![result.doc_id, Value::Double(result.score as f64)]);
                        }
                        return if rows.is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some(DataChunk::new_with_layout(
                                rows,
                                self.output_layout.clone(),
                            )))
                        };
                    }

                    // No manager configured: fall through to the input.
                    if let Some(mut chunk) = input.advance()? {
                        chunk.materialize_selection_by("Fulltext");
                        return Ok(Some(chunk));
                    }
                    Ok(None)
                }

                #[cfg(not(feature = "fulltext-search"))]
                {
                    let _ = (&match_expr, &tag_name, &field_name, input);
                    Err(QueryError::feature_disabled(
                        "fulltext-search",
                        "FULLTEXT MATCH",
                    ))
                }
            }
        }
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        Ok(())
    }
}
