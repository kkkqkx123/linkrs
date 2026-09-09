use std::sync::Arc;

use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::operators::spec::MigrateAction;
use graphdb_core::error::QueryError;
use graphdb_core::Value;

pub(super) fn execute_show_stats(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::ShowStats {
        storage,
        space_name: _,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    if let Some(storage_lock) = storage {
        let reader = storage_lock.read();
        let stats = reader.get_storage_stats();

        let schema = Arc::new(Schema::new(vec![
            ColumnInfo {
                name: "metric".to_string(),
                data_type: "string".to_string(),
            },
            ColumnInfo {
                name: "value".to_string(),
                data_type: "string".to_string(),
            },
        ]));
        let rows = vec![
            vec![
                Value::string("total_vertices"),
                Value::BigInt(stats.total_vertices as i64),
            ],
            vec![
                Value::string("total_edges"),
                Value::BigInt(stats.total_edges as i64),
            ],
            vec![
                Value::string("total_spaces"),
                Value::BigInt(stats.total_spaces as i64),
            ],
            vec![
                Value::string("total_tags"),
                Value::BigInt(stats.total_tags as i64),
            ],
            vec![
                Value::string("total_edge_types"),
                Value::BigInt(stats.total_edge_types as i64),
            ],
            vec![
                Value::string("total_size_bytes"),
                Value::BigInt(stats.total_size_bytes as i64),
            ],
            vec![
                Value::string("data_size_bytes"),
                Value::BigInt(stats.data_size_bytes as i64),
            ],
            vec![
                Value::string("index_size_bytes"),
                Value::BigInt(stats.index_size_bytes as i64),
            ],
        ];
        Ok(Some(DataChunk::new(rows, schema)))
    } else {
        let schema = super::make_single_col_schema("message", "string");
        Ok(Some(DataChunk::new(
            vec![vec![Value::string("no storage available")]],
            schema,
        )))
    }
}

pub(super) fn execute_show_configs(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::ShowConfigs {
        storage,
        space_name: _,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    let _ = storage;
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    Ok(Some(super::make_single_row(
        super::make_single_col_schema("module", "string"),
        vec![Value::string("graphdb")],
    )))
}

pub(super) fn execute_show_queries(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::ShowQueries {
        storage,
        space_name: _,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    let _ = storage;
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    Ok(Some(super::make_single_row(
        super::make_single_col_schema("queries", "string"),
        vec![],
    )))
}

pub(super) fn execute_show_sessions(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::ShowSessions {
        storage,
        space_name: _,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    let _ = storage;
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    Ok(Some(super::make_single_row(
        super::make_single_col_schema("sessions", "string"),
        vec![],
    )))
}

pub(super) fn execute_show_functions(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::ShowFunctions {
        storage,
        space_name: _,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    let _ = storage;
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    let registry = crate::executor::expression::functions::registry::global_registry();
    let names = registry.function_names();
    let schema = super::make_single_col_schema("functions", "string");
    let rows: Vec<Vec<Value>> = names
        .into_iter()
        .map(|name| vec![Value::string(name)])
        .collect();
    Ok(Some(DataChunk::new(rows, schema)))
}

pub(super) fn execute_show_graphs(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::ShowGraphs {
        storage,
        space_name: _,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    let schema = super::make_single_col_schema("graphs", "string");
    if let Some(storage_lock) = storage {
        let reader = storage_lock.read();
        let spaces = reader
            .list_spaces()
            .map_err(|e| QueryError::execution(e.to_string()))?;
        let rows: Vec<Vec<Value>> = spaces
            .into_iter()
            .map(|s| vec![Value::string(s.space_name)])
            .collect();
        Ok(Some(DataChunk::new(rows, schema)))
    } else {
        Ok(Some(DataChunk::new(vec![], schema)))
    }
}

pub(super) fn execute_show_macros(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let emitted = match &mut op.kind {
        super::DdlOperatorKind::ShowMacros { emitted, .. } => emitted,
        _ => return Ok(None),
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    let defs = op
        .runtime
        .as_ref()
        .and_then(|rt| rt.macro_manager())
        .map(|manager| manager.list_macros())
        .unwrap_or_default();

    let schema = Arc::new(Schema::new(vec![
        ColumnInfo {
            name: "name".to_string(),
            data_type: "string".to_string(),
        },
        ColumnInfo {
            name: "params".to_string(),
            data_type: "string".to_string(),
        },
        ColumnInfo {
            name: "body".to_string(),
            data_type: "string".to_string(),
        },
    ]));
    let rows = defs
        .iter()
        .map(|def| {
            let params = def
                .params
                .iter()
                .map(|p| match &p.default {
                    Some(default) => {
                        format!("{} = {}", p.name, default.to_expression_string())
                    }
                    None => p.name.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            vec![
                Value::string(&def.name),
                Value::string(params),
                Value::string(def.body.to_expression_string()),
            ]
        })
        .collect::<Vec<_>>();
    Ok(Some(DataChunk::new(rows, schema)))
}

pub(super) fn execute_load_from(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::LoadFrom {
        source_kind,
        source_value,
        func_name,
        func_args_json,
        options,
        col_names,
        emitted,
        ..
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    if source_kind == "table_func" {
        let name = func_name.clone().unwrap_or_default();
        let args_json_str = func_args_json.clone().unwrap_or_else(|| "[]".to_string());

        let arg_strings: Vec<String> = serde_json::from_str(&args_json_str).map_err(|e| {
            QueryError::execution(format!("LOAD FROM table function args parse error: {e}"))
        })?;
        let func_args: Vec<Value> = arg_strings.into_iter().map(Value::string).collect();

        let registry = crate::executor::expression::functions::registry::global_registry();

        if !registry.contains_table_function(&name) {
            return Err(QueryError::execution(format!(
                "LOAD FROM unknown table function '{name}'"
            )));
        }

        let rows = registry
            .execute_table_function(&name, &func_args)
            .map_err(|e| {
                QueryError::execution(format!("LOAD FROM table function '{name}' failed: {e}"))
            })?;

        if rows.is_empty() {
            let schema = Arc::new(Schema::new(vec![]));
            return Ok(Some(DataChunk::new(vec![], schema)));
        }

        let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let schema_cols: Vec<ColumnInfo> = (0..num_cols)
            .map(|i| ColumnInfo {
                name: format!("col{}", i),
                data_type: "string".to_string(),
            })
            .collect();
        let schema = Arc::new(Schema::new(schema_cols));

        return Ok(Some(DataChunk::new(rows, schema)));
    }

    if source_kind == "glob" {
        let pattern = source_value.clone();
        let mut matched_paths: Vec<std::path::PathBuf> = glob::glob(&pattern)
            .map_err(|e| QueryError::execution(format!("LOAD FROM glob pattern error: {e}")))?
            .filter_map(|entry| entry.ok())
            .filter(|path| path.is_file())
            .collect();
        matched_paths.sort();

        if matched_paths.is_empty() {
            return Err(QueryError::execution(format!(
                "LOAD FROM glob '{pattern}' matched no files"
            )));
        }

        let header = options
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("header"))
            .map(|(_, v)| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(true);
        let delimiter = options
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("delimiter"))
            .and_then(|(_, v)| v.as_bytes().first().copied())
            .unwrap_or(b',');

        let mut all_rows: Vec<Vec<Value>> = Vec::new();
        let mut schema: Option<Arc<Schema>> = None;

        for file_path in &matched_paths {
            let file = std::fs::File::open(file_path).map_err(|e| {
                QueryError::execution(format!(
                    "LOAD FROM glob failed to open '{}': {e}",
                    file_path.display()
                ))
            })?;
            let mut reader = csv::ReaderBuilder::new()
                .has_headers(header)
                .delimiter(delimiter)
                .trim(csv::Trim::All)
                .flexible(true)
                .from_reader(std::io::BufReader::new(file));

            if schema.is_none() {
                let headers: Vec<String> = if header {
                    reader
                        .headers()
                        .map_err(|e| {
                            QueryError::execution(format!("LOAD FROM glob header error: {e}"))
                        })?
                        .iter()
                        .map(|s| s.to_string())
                        .collect()
                } else if !col_names.is_empty() {
                    col_names.clone()
                } else {
                    Vec::new()
                };

                let schema_cols: Vec<ColumnInfo> = if headers.is_empty() {
                    col_names
                        .iter()
                        .map(|n| ColumnInfo {
                            name: n.clone(),
                            data_type: "string".to_string(),
                        })
                        .collect()
                } else {
                    headers
                        .iter()
                        .map(|n| ColumnInfo {
                            name: n.clone(),
                            data_type: "string".to_string(),
                        })
                        .collect()
                };
                schema = Some(Arc::new(Schema::new(schema_cols)));
            }

            for result in reader.records() {
                let record = result
                    .map_err(|e| QueryError::execution(format!("LOAD FROM glob row error: {e}")))?;
                let row: Vec<Value> = record.iter().map(|s| Value::String(s.into())).collect();
                all_rows.push(row);
            }
        }

        let schema = schema.unwrap_or_else(|| Arc::new(Schema::new(vec![])));
        return Ok(Some(DataChunk::new(all_rows, schema)));
    }

    if source_kind != "file" {
        return Err(QueryError::execution(format!(
            "LOAD FROM source kind '{source_kind}' is not yet supported"
        )));
    }

    let header = options
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("header"))
        .map(|(_, v)| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(true);
    let delimiter = options
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("delimiter"))
        .and_then(|(_, v)| v.as_bytes().first().copied())
        .unwrap_or(b',');

    let file_path = source_value.clone();
    let file = std::fs::File::open(&file_path).map_err(|e| {
        QueryError::execution(format!("LOAD FROM failed to open '{file_path}': {e}"))
    })?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(header)
        .delimiter(delimiter)
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(std::io::BufReader::new(file));

    let headers: Vec<String> = if header {
        reader
            .headers()
            .map_err(|e| QueryError::execution(format!("LOAD FROM header error: {e}")))?
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else if !col_names.is_empty() {
        col_names.clone()
    } else {
        Vec::new()
    };

    let schema_cols: Vec<ColumnInfo> = if headers.is_empty() {
        col_names
            .iter()
            .map(|n| ColumnInfo {
                name: n.clone(),
                data_type: "string".to_string(),
            })
            .collect()
    } else {
        headers
            .iter()
            .map(|n| ColumnInfo {
                name: n.clone(),
                data_type: "string".to_string(),
            })
            .collect()
    };
    let schema = Arc::new(Schema::new(schema_cols));

    let mut rows: Vec<Vec<Value>> = Vec::new();
    for result in reader.records() {
        let record =
            result.map_err(|e| QueryError::execution(format!("LOAD FROM row error: {e}")))?;
        let row: Vec<Value> = record.iter().map(|s| Value::String(s.into())).collect();
        rows.push(row);
    }

    Ok(Some(DataChunk::new(rows, schema)))
}

pub(super) fn execute_in_query_call(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::InQueryCall {
        storage,
        space_name,
        func_name,
        args_json,
        yield_items,
        col_names,
        emitted,
        ..
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    let names: Vec<String> = if !yield_items.is_empty() {
        yield_items.iter().map(|(alias, _)| alias.clone()).collect()
    } else if !col_names.is_empty() {
        col_names.clone()
    } else {
        vec!["result".to_string()]
    };
    let schema = Arc::new(Schema::new(
        names
            .iter()
            .map(|n| ColumnInfo {
                name: n.clone(),
                data_type: "string".to_string(),
            })
            .collect(),
    ));

    match func_name.as_str() {
        "db_version" => Ok(Some(super::make_single_row(
            schema,
            vec![Value::String(env!("CARGO_PKG_VERSION").into())],
        ))),
        "db_current_space" => Ok(Some(super::make_single_row(
            schema,
            vec![Value::string(space_name.clone())],
        ))),
        "db_property_graph" => {
            let reader = match storage {
                Some(lock) => lock.read(),
                None => {
                    return Err(QueryError::execution(
                        "No storage available for db_property_graph()".to_string(),
                    ));
                }
            };
            match reader.get_space(space_name) {
                Ok(Some(info)) => {
                    let schema_str = format!(
                        "Space: {}, Tags: {}, EdgeTypes: {}",
                        info.space_name,
                        reader.list_tags(space_name).map(|t| t.len()).unwrap_or(0),
                        reader
                            .list_edge_types(space_name)
                            .map(|e| e.len())
                            .unwrap_or(0),
                    );
                    Ok(Some(super::make_single_row(
                        schema,
                        vec![Value::string(schema_str)],
                    )))
                }
                Ok(None) => Err(QueryError::execution(format!(
                    "Space '{}' not found",
                    space_name
                ))),
                Err(e) => Err(QueryError::execution(format!(
                    "Failed to get space info: {}",
                    e
                ))),
            }
        }
        "list_labels" => {
            let reader = match storage {
                Some(lock) => lock.read(),
                None => {
                    return Err(QueryError::execution(
                        "No storage available for list_labels()".to_string(),
                    ));
                }
            };
            let tags = reader
                .list_tags(space_name)
                .map_err(|e| QueryError::execution(format!("Failed to list tags: {}", e)))?;
            let rows: Vec<Vec<Value>> = tags
                .into_iter()
                .map(|t| vec![Value::string(t.tag_name)])
                .collect();
            let tag_schema = super::make_single_col_schema("label", "string");
            Ok(Some(DataChunk::new(rows, tag_schema)))
        }
        "list_relationship_types" | "list.relationship_types" => {
            let reader = match storage {
                Some(lock) => lock.read(),
                None => {
                    return Err(QueryError::execution(
                        "No storage available for list_relationship_types()".to_string(),
                    ));
                }
            };
            let edges = reader
                .list_edge_types(space_name)
                .map_err(|e| QueryError::execution(format!("Failed to list edge types: {}", e)))?;
            let rows: Vec<Vec<Value>> = edges
                .into_iter()
                .map(|e| vec![Value::string(e.edge_type_name)])
                .collect();
            let edge_schema = super::make_single_col_schema("relationship_type", "string");
            Ok(Some(DataChunk::new(rows, edge_schema)))
        }
        _ => {
            let registry = crate::executor::expression::functions::registry::global_registry();
            let args = parse_call_args(args_json);
            match registry.execute(func_name, &args) {
                Ok(value) => Ok(Some(super::make_single_row(schema, vec![value]))),
                Err(_) => Ok(Some(DataChunk::new(Vec::new(), schema))),
            }
        }
    }
}

pub(super) fn execute_analyze(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::Analyze {
        storage,
        space_name,
        analyze_target,
        target_name,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    let result = match analyze_target.as_str() {
        "space" => {
            if let Some(lock) = storage {
                let reader = lock.read();
                let stats = reader.get_storage_stats();
                let schema = Arc::new(Schema::new(vec![
                    ColumnInfo {
                        name: "target".to_string(),
                        data_type: "string".to_string(),
                    },
                    ColumnInfo {
                        name: "stats".to_string(),
                        data_type: "string".to_string(),
                    },
                ]));
                Ok(Some(super::make_single_row(
                    schema,
                    vec![
                        Value::string(format!("space:{}", space_name)),
                        Value::string(format!("{:?}", stats)),
                    ],
                )))
            } else {
                Ok(Some(super::make_manage_result(
                    "analyze",
                    Some(space_name.as_str()),
                    "no-storage",
                )))
            }
        }
        "tag" | "edge" => {
            let name = target_name.as_deref().unwrap_or("");
            Ok(Some(super::make_manage_result(
                "analyze",
                Some(name),
                "executed",
            )))
        }
        _ => Err(QueryError::execution(format!(
            "Unsupported analyze target: {}",
            analyze_target
        ))),
    };
    result
}

pub(super) fn execute_migrate(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::Migrate {
        storage,
        space_name,
        action,
        migration_data: _,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    let result = match action {
        MigrateAction::MigrateSpace => {
            if let Some(lock) = storage {
                let writer = lock.write();
                let res = writer
                    .save_to_disk()
                    .map_err(|e| QueryError::execution(format!("Migrate failed: {}", e)));
                match res {
                    Ok(_) => Ok(Some(super::make_manage_result(
                        "migrate",
                        Some(space_name),
                        "saved",
                    ))),
                    Err(e) => Err(e),
                }
            } else {
                Ok(Some(super::make_manage_result(
                    "migrate",
                    Some(space_name),
                    "no-storage",
                )))
            }
        }
    };
    result
}

fn parse_call_args(args_json: &str) -> Vec<Value> {
    let parsed: Vec<serde_json::Value> = serde_json::from_str(args_json).unwrap_or_default();
    parsed.into_iter().map(|v| json_to_value(&v)).collect()
}

fn json_to_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null(graphdb_core::NullType::Null),
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::BigInt(i)
            } else if let Some(f) = n.as_f64() {
                Value::Double(f)
            } else {
                Value::string(n.to_string())
            }
        }
        serde_json::Value::String(s) => {
            if let Ok(i) = s.parse::<i64>() {
                Value::BigInt(i)
            } else if let Ok(f) = s.parse::<f64>() {
                Value::Double(f)
            } else if s.eq_ignore_ascii_case("true") {
                Value::Bool(true)
            } else if s.eq_ignore_ascii_case("false") {
                Value::Bool(false)
            } else if s.eq_ignore_ascii_case("null") {
                Value::Null(graphdb_core::NullType::Null)
            } else {
                Value::string(s.clone())
            }
        }
        serde_json::Value::Array(arr) => {
            let values: Vec<Value> = arr.iter().map(json_to_value).collect();
            Value::list(graphdb_core::value::list::List::from(values))
        }
        serde_json::Value::Object(map) => {
            let entries: std::collections::HashMap<Value, Value> = map
                .iter()
                .map(|(k, v)| (Value::string(k.clone()), json_to_value(v)))
                .collect();
            Value::Map(Box::new(entries))
        }
    }
}
