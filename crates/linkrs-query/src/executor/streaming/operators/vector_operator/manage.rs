#[cfg(feature = "vector")]
use std::sync::Arc;

use crate::executor::streaming::chunk::DataChunk;
#[cfg(feature = "vector")]
use crate::executor::streaming::chunk::{ColumnInfo, Schema};
use crate::executor::streaming::executor::StreamingExecutor;
#[cfg(feature = "vector")]
use crate::executor::streaming::operators::ddl_operator::make_single_row;
use crate::executor::streaming::operators::index_manage::{
    manage_result, unresolved_drop, IndexKind, ShowFilter,
};
use crate::executor::streaming::operators::spec::VectorManageCommand;
use linkrs_core::error::QueryError;
#[cfg(feature = "vector")]
use linkrs_core::Value;

use super::{VectorOperator, VectorOperatorKind};

/// Candidate count for MATCH VECTOR searches.
///

pub(crate) fn handle(
    op: &mut VectorOperator,
    _input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    match &mut op.kind {
        VectorOperatorKind::VectorManage {
            storage,
            space_name,
            command,
            #[cfg(feature = "vector")]
            vector_coordinator,
        } => {
            #[cfg(feature = "vector")]
            let _ = (&storage, &space_name);

            match command {
                VectorManageCommand::Create {
                    index_name,
                    tag_name,
                    field_name,
                    vector_size,
                    distance,
                    space_id,
                    hnsw_m,
                    hnsw_ef_construct,
                    quantization,
                    quantile,
                    compression,
                    always_ram,
                } => {
                    #[cfg(feature = "vector")]
                    {
                        // Validate the statement-level index shape before
                        // checking for an engine: whether a statement is well
                        // formed does not depend on runtime wiring, so an
                        // invalid HNSW or quantization combination must be
                        // reported even when no coordinator is configured.
                        // Shared DDL semantics: HNSW + quantization mapping
                        // and validation live in vector_ddl so the HTTP/gRPC
                        // index endpoints cannot drift from the statement
                        // pipeline. The statement grammar carries only
                        // index-shape options; storage layout stays a
                        // transport-level concern, so everything else keeps
                        // its default.
                        let distance = crate::vector_ddl::distance_metric(*distance);
                        let options = crate::vector_ddl::CollectionOptions {
                            hnsw_m: *hnsw_m,
                            hnsw_ef_construct: *hnsw_ef_construct,
                            quantization: *quantization,
                            quantile: *quantile,
                            compression: *compression,
                            always_ram: *always_ram,
                            ..crate::vector_ddl::CollectionOptions::default()
                        };
                        let config = crate::vector_ddl::build_collection_config(
                            *vector_size,
                            distance,
                            &options,
                        )
                        .map_err(QueryError::execution)?;

                        if let Some(coordinator) = vector_coordinator {
                            // Vector indexes are vertex-only: point IDs,
                            // payloads, and rebuild sources are all modeled
                            // per vertex. Reject edge type names here as well
                            // so the no-metadata query path cannot create an
                            // index that can never receive data. A storage
                            // error fails open: the planner already rejects
                            // known edge types, and an unreadable catalog
                            // must not block vertex index creation.
                            if let Some(storage) = storage.as_ref() {
                                if !tag_name.is_empty() {
                                    if let Ok(edge) =
                                        storage.read().get_edge_type(space_name, tag_name)
                                    {
                                        if edge.is_some() {
                                            return Err(QueryError::execution(format!(
                                                "Vector indexes are vertex-only: '{}' is an edge type, create the index on a vertex tag instead",
                                                tag_name
                                            )));
                                        }
                                    }
                                }
                            }
                            let res = if config.quantization_config.is_some()
                                || config.index != simvec::IndexConfig::default()
                            {
                                crate::executor::streaming::helpers::runtime_bridge::wait(
                                    "Vector create",
                                    coordinator.create_index_with_config(
                                        *space_id, tag_name, field_name, config,
                                    ),
                                )
                            } else {
                                crate::executor::streaming::helpers::runtime_bridge::wait(
                                    "Vector create",
                                    coordinator.create_vector_index(
                                        *space_id,
                                        tag_name,
                                        field_name,
                                        *vector_size,
                                        distance,
                                    ),
                                )
                            };
                            match res {
                                Ok(_) => {
                                    // Record the statement-level name so
                                    // later SEARCH/LOOKUP/DROP statements
                                    // resolve it back to (space, tag, field).
                                    coordinator.set_index_name(
                                        *space_id, tag_name, field_name, index_name,
                                    );
                                    Ok(Some(manage_result(
                                        Arc::clone(&op.output_layout),
                                        "create_vector_index",
                                        Some(index_name.as_str()),
                                        "created",
                                    )))
                                }
                                Err(e) => Err(e),
                            }
                        } else {
                            Err(QueryError::execution(
                                "CREATE VECTOR INDEX cannot execute: no vector coordinator is configured",
                            ))
                        }
                    }
                    #[cfg(not(feature = "vector"))]
                    {
                        let _ = (
                            storage,
                            space_name,
                            index_name,
                            tag_name,
                            field_name,
                            vector_size,
                            distance,
                            space_id,
                            hnsw_m,
                            hnsw_ef_construct,
                            quantization,
                            quantile,
                            compression,
                            always_ram,
                        );
                        Err(QueryError::feature_disabled(
                            "vector",
                            "CREATE VECTOR INDEX",
                        ))
                    }
                }
                VectorManageCommand::Drop {
                    index_name,
                    if_exists,
                    space_id,
                    tag_name,
                    field_name,
                } => {
                    #[cfg(feature = "vector")]
                    {
                        // The drop API is addressed by (space_id, tag,
                        // field). An unresolved location means the index
                        // was not found at planning time: `IF EXISTS`
                        // degrades to a no-op status row, otherwise it is
                        // a clear error.
                        if tag_name.is_empty() || field_name.is_empty() {
                            if let Some(error) =
                                unresolved_drop(IndexKind::Vector, index_name, *if_exists)
                            {
                                return Err(error);
                            }
                            return Ok(Some(manage_result(
                                Arc::clone(&op.output_layout),
                                "drop_vector_index",
                                Some(index_name.as_str()),
                                "not_exists",
                            )));
                        }
                        match vector_coordinator {
                            Some(coordinator) => {
                                crate::executor::streaming::helpers::runtime_bridge::wait(
                                    "Vector drop",
                                    coordinator
                                        .drop_vector_index(*space_id, tag_name, field_name),
                                )?;
                                Ok(Some(manage_result(
                                    Arc::clone(&op.output_layout),
                                    "drop_vector_index",
                                    Some(index_name.as_str()),
                                    "dropped",
                                )))
                            }
                            None => Err(QueryError::execution(
                                "DROP VECTOR INDEX cannot execute: no vector coordinator is configured",
                            )),
                        }
                    }
                    #[cfg(not(feature = "vector"))]
                    {
                        let _ = (
                            storage, space_name, index_name, if_exists, space_id, tag_name,
                            field_name,
                        );
                        Err(QueryError::feature_disabled("vector", "DROP VECTOR INDEX"))
                    }
                }
                VectorManageCommand::Show {
                    pattern,
                    from_schema,
                    space_id,
                } => {
                    #[cfg(feature = "vector")]
                    {
                        if let Some(coordinator) = vector_coordinator {
                            let filter = ShowFilter::new(
                                pattern.as_deref(),
                                from_schema.as_deref(),
                                *space_id,
                            );
                            let indexes: Vec<_> = coordinator
                                .list_indexes()
                                .into_iter()
                                .filter(|m| {
                                    filter.matches(m.index_name.as_deref(), &m.tag_name, m.space_id)
                                })
                                .collect();
                            let schema = Arc::new(Schema::new(vec![
                                ColumnInfo {
                                    name: "index_id".to_string(),
                                    data_type: "string".to_string(),
                                },
                                ColumnInfo {
                                    name: "collection_name".to_string(),
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
                            ]));
                            let rows: Vec<Vec<Value>> = indexes
                                .into_iter()
                                .map(|m| {
                                    vec![
                                        m.index_name
                                            .map(Value::string)
                                            .unwrap_or(Value::Null(linkrs_core::NullType::Null)),
                                        Value::string(m.collection_name),
                                        Value::BigInt(m.space_id as i64),
                                        Value::string(m.tag_name),
                                        Value::string(m.field_name),
                                    ]
                                })
                                .collect();
                            Ok(Some(DataChunk::new(rows, schema)))
                        } else {
                            Err(QueryError::execution(
                                "SHOW VECTOR INDEXES cannot execute: no vector coordinator is configured",
                            ))
                        }
                    }
                    #[cfg(not(feature = "vector"))]
                    {
                        let _ = (pattern, from_schema, space_id);
                        Err(QueryError::feature_disabled("vector", "SHOW VECTOR INDEX"))
                    }
                }
                VectorManageCommand::Describe {
                    index_name,
                    space_id,
                } => {
                    #[cfg(feature = "vector")]
                    {
                        if let Some(coordinator) = vector_coordinator {
                            let found = coordinator.list_indexes().into_iter().find(|m| {
                                m.space_id == *space_id
                                    && m.index_name.as_deref() == Some(index_name.as_str())
                            });
                            match found {
                                Some(m) => {
                                    let vector_count = coordinator
                                        .index_info(m.space_id, &m.tag_name, &m.field_name)
                                        .map(|meta| meta.vector_count)
                                        .unwrap_or(0);
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
                                            name: "vector_count".to_string(),
                                            data_type: "bigint".to_string(),
                                        },
                                    ]));
                                    Ok(Some(make_single_row(
                                        schema,
                                        vec![
                                            Value::string(index_name),
                                            Value::string(m.tag_name),
                                            Value::string(m.field_name),
                                            Value::BigInt(vector_count as i64),
                                        ],
                                    )))
                                }
                                None => Ok(Some(DataChunk::new_with_layout(
                                    // Same card shape as the found case: the
                                    // card has no status column, so an absent
                                    // index shows as a null row for that name.
                                    vec![vec![
                                        Value::string(index_name),
                                        Value::Null(linkrs_core::NullType::Null),
                                        Value::Null(linkrs_core::NullType::Null),
                                        Value::Null(linkrs_core::NullType::Null),
                                    ]],
                                    Arc::clone(&op.output_layout),
                                ))),
                            }
                        } else {
                            Err(QueryError::execution(
                                "DESCRIBE VECTOR INDEX cannot execute: no vector coordinator is configured",
                            ))
                        }
                    }
                    #[cfg(not(feature = "vector"))]
                    {
                        let _ = (index_name, space_id);
                        Err(QueryError::feature_disabled(
                            "vector",
                            "DESCRIBE VECTOR INDEX",
                        ))
                    }
                }
            }
        }
        _ => unreachable!("vector_operator::manage called for an unknown kind"),
    }
}
