use std::sync::Arc;

use parking_lot::RwLock;

use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
#[cfg(feature = "fulltext")]
use crate::executor::streaming::operators::ddl_operator::make_single_row;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::operators::spec::FulltextManageCommand;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use crate::storage::QueryStorage;
use linkrs_core::error::QueryError;
use linkrs_core::types::expr::Expression;
#[cfg(feature = "fulltext")]
use linkrs_core::Value;
#[cfg(feature = "fulltext")]
use linkrs_fulltext::manager::FulltextIndexManager;

#[cfg(feature = "fulltext")]
use crate::executor::streaming::chunk::{ColumnInfo, Schema};

#[cfg(not(feature = "fulltext"))]
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

#[cfg(feature = "fulltext")]
fn make_manage_result(
    output_layout: Arc<SlotLayout>,
    action: &str,
    name: Option<&str>,
    status: &str,
) -> DataChunk {
    let name_val = name
        .map(Value::string)
        .unwrap_or(Value::Null(linkrs_core::NullType::Null));
    DataChunk::new_with_layout(
        vec![vec![Value::string(action), name_val, Value::string(status)]],
        output_layout,
    )
}

/// Map the planner-level engine type to the fulltext manager's engine type.
///
/// Both enums currently carry only BM25; the match stays exhaustive so a
/// future engine variant fails to compile here instead of silently falling
/// back to a default.
#[cfg(feature = "fulltext")]
fn map_engine_type(
    engine: linkrs_core::types::FulltextEngineType,
) -> linkrs_fulltext::EngineType {
    match engine {
        linkrs_core::types::FulltextEngineType::Bm25 => linkrs_fulltext::EngineType::Bm25,
    }
}

/// Render parsed index options as the opaque engine-config document stored
/// in fulltext index metadata.
///
/// Returns `None` when no option was given so the manager keeps its compiled
/// defaults; per-field `analyzer` / `boost` are parser-rejected today, so a
/// present field entry only ever carries its name.
#[cfg(feature = "fulltext")]
fn fulltext_engine_config(
    options: &crate::parser::ast::fulltext::IndexOptions,
) -> Option<serde_json::Value> {
    let has_bm25 = options.bm25_config.is_some();
    if !has_bm25 && options.common_options.is_empty() {
        return None;
    }
    let mut config = serde_json::Map::new();
    if let Some(bm25) = &options.bm25_config {
        if let Some(k1) = bm25.k1 {
            config.insert("k1".to_string(), serde_json::json!(k1));
        }
        if let Some(b) = bm25.b {
            config.insert("b".to_string(), serde_json::json!(b));
        }
        if let Some(analyzer) = &bm25.analyzer {
            config.insert("analyzer".to_string(), serde_json::json!(analyzer));
        }
        if let Some(store_original) = bm25.store_original {
            config.insert("store_original".to_string(), serde_json::json!(store_original));
        }
        if !bm25.field_weights.is_empty() {
            config.insert(
                "field_weights".to_string(),
                serde_json::json!(bm25.field_weights),
            );
        }
    }
    for (key, value) in &options.common_options {
        config.insert(key.clone(), plain_option_json(value));
    }
    Some(serde_json::Value::Object(config))
}

/// Render an option value as plain JSON.
///
/// `Value` serializes externally tagged (`{"String": ...}`); option metadata
/// stays human-readable by unwrapping the common scalar cases and falling
/// back to the tagged form for complex values.
#[cfg(feature = "fulltext")]
fn plain_option_json(value: &linkrs_core::Value) -> serde_json::Value {
    use linkrs_core::Value;
    match value {
        Value::String(s) => serde_json::Value::String(s.to_string()),
        Value::FixedString(s) => serde_json::Value::String(s.clone()),
        Value::SmallInt(v) => serde_json::json!(*v),
        Value::Int(v) => serde_json::json!(*v),
        Value::BigInt(v) => serde_json::json!(*v),
        Value::Float(v) => serde_json::json!(*v),
        Value::Double(v) => serde_json::json!(*v),
        Value::Bool(v) => serde_json::Value::Bool(*v),
        Value::Null(_) => serde_json::Value::Null,
        _ => serde_json::to_value(value).unwrap_or(serde_json::Value::Null),
    }
}

#[derive(Debug)]
pub enum FulltextOperatorKind {    FulltextManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        command: FulltextManageCommand,
        #[cfg(feature = "fulltext")]
        fulltext_manager: Option<Arc<FulltextIndexManager>>,
    },
    FulltextSearch {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        space_id: u64,
        index_name: String,
        structured_query: linkrs_fulltext::query::FulltextQuery,
        tag_name: String,
        field_name: String,
        limit: Option<usize>,
        #[cfg(feature = "fulltext")]
        fulltext_manager: Option<Arc<FulltextIndexManager>>,
    },
    FulltextLookup {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        space_id: u64,
        index_name: String,
        structured_query: linkrs_fulltext::query::FulltextQuery,
        tag_name: String,
        field_name: String,
        limit: Option<usize>,
        #[cfg(feature = "fulltext")]
        fulltext_manager: Option<Arc<FulltextIndexManager>>,
    },
    MatchFulltext {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        space_id: u64,
        match_expr: Expression,
        match_field: Option<String>,
        /// Structured query built from the fulltext match condition.
        structured_query: linkrs_fulltext::query::FulltextQuery,
        tag_name: String,
        field_name: String,
        limit: Option<usize>,
        #[cfg(feature = "fulltext")]
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
        #[cfg(feature = "fulltext")] fulltext_manager: Option<Arc<FulltextIndexManager>>,
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
                #[cfg(feature = "fulltext")]
                fulltext_manager: fulltext_manager.clone(),
            },
            super::spec::FulltextSpec::FulltextSearch {
                space_name,
                space_id,
                index_name,
                structured_query,
                tag_name,
                field_name,
                limit,
            } => FulltextOperatorKind::FulltextSearch {
                storage: storage.clone(),
                space_name: space_name.clone(),
                space_id: *space_id,
                index_name: index_name.clone(),
                structured_query: structured_query.clone(),
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                limit: *limit,
                #[cfg(feature = "fulltext")]
                fulltext_manager: fulltext_manager.clone(),
            },
            super::spec::FulltextSpec::FulltextLookup {
                space_name,
                space_id,
                index_name,
                structured_query,
                tag_name,
                field_name,
                limit,
            } => FulltextOperatorKind::FulltextLookup {
                storage: storage.clone(),
                space_name: space_name.clone(),
                space_id: *space_id,
                index_name: index_name.clone(),
                structured_query: structured_query.clone(),
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                limit: *limit,
                #[cfg(feature = "fulltext")]
                fulltext_manager: fulltext_manager.clone(),
            },
            super::spec::FulltextSpec::MatchFulltext {
                space_name,
                space_id,
                match_expr,
                match_field,
                structured_query,
                tag_name,
                field_name,
                limit,
            } => FulltextOperatorKind::MatchFulltext {
                storage: storage.clone(),
                space_name: space_name.clone(),
                space_id: *space_id,
                match_expr: match_expr.clone(),
                match_field: match_field.clone(),
                structured_query: structured_query.clone(),
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                limit: *limit,
                #[cfg(feature = "fulltext")]
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
                #[cfg(feature = "fulltext")]
                fulltext_manager,
            } => {
                #[cfg(feature = "fulltext")]
                {
                    use crate::executor::streaming::operators::spec::FulltextManageCommand::*;
                    let result = match command {
                        Create {
                            index_name,
                            schema_name,
                            fields,
                            engine_type,
                            options,
                            if_not_exists,
                            space_id,
                        } => {
                            if let Some(manager) = fulltext_manager {
                                let engine_config = fulltext_engine_config(options);
                                let engine = map_engine_type(*engine_type);
                                for field in fields {
                                    if manager.has_index(
                                        *space_id,
                                        schema_name,
                                        field.field_name.as_str(),
                                    ) {
                                        if *if_not_exists {
                                            continue;
                                        }
                                        return Err(QueryError::execution(format!(
                                            "Fulltext index on {}.{}({}) already exists",
                                            space_id, schema_name, field.field_name,
                                        )));
                                    }
                                    crate::executor::streaming::helpers::runtime_bridge::wait(
                                        "Fulltext create",
                                        manager.create_index_with_engine_config(
                                            *space_id,
                                            schema_name,
                                            field.field_name.as_str(),
                                            index_name.as_str(),
                                            Some(engine),
                                            engine_config.clone(),
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
                                return Err(QueryError::execution(
                                    "CREATE FULLTEXT INDEX cannot execute: no fulltext manager is configured",
                                ));
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
                                return Err(QueryError::execution(
                                    "DROP FULLTEXT INDEX cannot execute: no fulltext manager is configured",
                                ));
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
                                return Err(QueryError::execution(
                                    "DESCRIBE FULLTEXT INDEX cannot execute: no fulltext manager is configured",
                                ));
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
                                return Err(QueryError::execution(
                                    "SHOW FULLTEXT INDEXES cannot execute: no fulltext manager is configured",
                                ));
                            }
                        }
                        Alter {
                            index_name,
                            actions,
                        } => {
                            use crate::parser::ast::fulltext::AlterIndexAction;
                            if let Some(manager) = fulltext_manager {
                                let matching: Vec<_> = manager
                                    .list_indexes()
                                    .into_iter()
                                    .filter(|metadata| metadata.index_name == *index_name)
                                    .collect();
                                if matching.is_empty() {
                                    return Err(QueryError::execution(format!(
                                        "Fulltext index not found: {}",
                                        index_name
                                    )));
                                }
                                for metadata in matching {
                                    for action in actions.iter() {
                                        match action {
                                            AlterIndexAction::Rebuild => {
                                                return Err(QueryError::execution(
                                                    crate::planning::fulltext_planner::FulltextSearchPlanner::FULLTEXT_REBUILD_UNAVAILABLE,
                                                ));
                                            }
                                            AlterIndexAction::Optimize => {
                                                crate::executor::streaming::helpers::runtime_bridge::wait(
                                                    "Fulltext optimize",
                                                    manager.optimize_index(
                                                        metadata.space_id,
                                                        &metadata.tag_name,
                                                        &metadata.field_name,
                                                    ),
                                                )?;
                                            }
                                            AlterIndexAction::AddField(_)
                                            | AlterIndexAction::DropField(_)
                                            | AlterIndexAction::SetOption(_, _) => {
                                                return Err(QueryError::execution(
                                                    "ALTER FULLTEXT INDEX only supports REBUILD and OPTIMIZE; use CREATE/DROP for field or option changes",
                                                ));
                                            }
                                        }
                                    }
                                }
                                Some(make_manage_result(
                                    Arc::clone(&self.output_layout),
                                    "alter_fulltext_index",
                                    Some(index_name.as_str()),
                                    "altered",
                                ))
                            } else {
                                return Err(QueryError::execution(
                                    "ALTER FULLTEXT INDEX cannot execute: no fulltext manager is configured",
                                ));
                            }
                        }
                    };
                    Ok(result)
                }

                #[cfg(not(feature = "fulltext"))]
                {
                    let (operation, _) = fulltext_command_info(command);
                    Err(QueryError::feature_disabled(
                        "fulltext",
                        &operation.to_uppercase(),
                    ))
                }
            }

            FulltextOperatorKind::FulltextSearch {
                structured_query,
                space_id,
                tag_name,
                field_name,
                limit,
                #[cfg(feature = "fulltext")]
                fulltext_manager,
                ..
            } => {
                #[cfg(feature = "fulltext")]
                {
                    if let Some(manager) = fulltext_manager {
                        let search_limit = limit.unwrap_or(100);
                        let search_results =
                            crate::executor::streaming::helpers::runtime_bridge::wait(
                                "Fulltext search",
                                manager.search_structured(
                                    *space_id,
                                    tag_name,
                                    field_name,
                                    structured_query,
                                    search_limit,
                                ),
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

                    let _ = input;
                    Err(QueryError::execution(
                        "FULLTEXT SEARCH cannot execute: no fulltext manager is configured",
                    ))
                }

                #[cfg(not(feature = "fulltext"))]
                {
                    let _ = (
                        &structured_query,
                        &space_id,
                        &tag_name,
                        &field_name,
                        limit,
                        input,
                    );
                    Err(QueryError::feature_disabled("fulltext", "FULLTEXT SEARCH"))
                }
            }

            FulltextOperatorKind::FulltextLookup {
                structured_query,
                space_id,
                tag_name,
                field_name,
                limit,
                #[cfg(feature = "fulltext")]
                fulltext_manager,
                ..
            } => {
                #[cfg(feature = "fulltext")]
                {
                    if let Some(manager) = fulltext_manager {
                        let search_limit = limit.unwrap_or(100);
                        let search_results =
                            crate::executor::streaming::helpers::runtime_bridge::wait(
                                "Fulltext lookup",
                                manager.search_structured(
                                    *space_id,
                                    tag_name,
                                    field_name,
                                    structured_query,
                                    search_limit,
                                ),
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

                    let _ = input;
                    Err(QueryError::execution(
                        "FULLTEXT LOOKUP cannot execute: no fulltext manager is configured",
                    ))
                }

                #[cfg(not(feature = "fulltext"))]
                {
                    let _ = (
                        &structured_query,
                        &space_id,
                        &tag_name,
                        &field_name,
                        limit,
                        input,
                    );
                    Err(QueryError::feature_disabled("fulltext", "FULLTEXT LOOKUP"))
                }
            }

            FulltextOperatorKind::MatchFulltext {
                match_expr: _match_expr,
                space_id,
                structured_query,
                tag_name,
                field_name,
                limit,
                #[cfg(feature = "fulltext")]
                fulltext_manager,
                ..
            } => {
                #[cfg(feature = "fulltext")]
                {
                    if let Some(manager) = fulltext_manager {
                        let search_limit = limit.unwrap_or(100);
                        let search_results =
                            crate::executor::streaming::helpers::runtime_bridge::wait(
                                "Fulltext match",
                                manager.search_structured(
                                    *space_id,
                                    tag_name,
                                    field_name,
                                    structured_query,
                                    search_limit,
                                ),
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

                    let _ = input;
                    Err(QueryError::execution(
                        "FULLTEXT MATCH cannot execute: no fulltext manager is configured",
                    ))
                }

                #[cfg(not(feature = "fulltext"))]
                {
                    let _ = (
                        &_match_expr,
                        &space_id,
                        &structured_query,
                        &tag_name,
                        &field_name,
                        limit,
                        input,
                    );
                    Err(QueryError::feature_disabled("fulltext", "FULLTEXT MATCH"))
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

#[cfg(all(test, feature = "fulltext"))]
mod tests {
    use super::*;
    use crate::parser::ast::fulltext::{BM25Options, IndexOptions};
    use std::collections::HashMap;

    #[test]
    fn engine_config_is_none_without_options() {
        let options = IndexOptions {
            bm25_config: None,
            common_options: HashMap::new(),
        };
        assert_eq!(fulltext_engine_config(&options), None);
    }

    #[test]
    fn engine_config_carries_bm25_and_common_options() {
        let mut common = HashMap::new();
        common.insert("custom".to_string(), linkrs_core::Value::string("x"));
        let options = IndexOptions {
            bm25_config: Some(BM25Options {
                k1: Some(1.2),
                b: Some(0.8),
                field_weights: HashMap::new(),
                analyzer: None,
                store_original: Some(true),
            }),
            common_options: common,
        };
        let config = fulltext_engine_config(&options).expect("config rendered");
        assert_eq!(config["k1"], serde_json::json!(1.2f32));
        assert_eq!(config["b"], serde_json::json!(0.8f32));
        assert_eq!(config["store_original"], serde_json::json!(true));
        assert_eq!(config["custom"], serde_json::json!("x"));
    }

    #[test]
    fn engine_type_maps_bm25() {
        assert_eq!(
            map_engine_type(linkrs_core::types::FulltextEngineType::Bm25),
            linkrs_fulltext::EngineType::Bm25
        );
    }
}
