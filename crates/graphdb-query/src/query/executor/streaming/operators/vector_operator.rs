use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operator_base::OperatorBase;
use crate::storage::StorageClient;
#[cfg(feature = "qdrant")]
use crate::sync::VectorSyncCoordinator;

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
pub enum VectorOperator {
    VectorManage {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        space_id: u64,
        action: String,
        index_name: Option<String>,
        tag_name: Option<String>,
        field_name: Option<String>,
        #[cfg(feature = "qdrant")]
        vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    },
    VectorSearch {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        space_id: u64,
        index_name: String,
        query_vector: Vec<f32>,
        top_k: u32,
        tag_name: String,
        field_name: String,
        #[cfg(feature = "qdrant")]
        vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    },
    VectorLookup {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        index_name: String,
        lookup_key: Expression,
        #[cfg(feature = "qdrant")]
        vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    },
    VectorMatch {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        pattern: String,
        field: String,
        query_vector: Vec<f32>,
        threshold: Option<f32>,
        tag_name: String,
        field_name: String,
        space_id: u64,
        #[cfg(feature = "qdrant")]
        vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    },
}

impl VectorOperator {
    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            VectorOperator::VectorManage { .. }
            | VectorOperator::VectorSearch { .. }
            | VectorOperator::VectorLookup { .. }
            | VectorOperator::VectorMatch { .. } => {
                input.open()?;
                base.opened = true;
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
            VectorOperator::VectorManage {
                storage,
                space_name,
                space_id,
                action,
                index_name,
                tag_name,
                field_name,
                #[cfg(feature = "qdrant")]
                vector_coordinator,
            } => {
                #[cfg(not(feature = "qdrant"))]
                let _ = (&tag_name, &field_name, &space_id);
                #[cfg(feature = "qdrant")]
                let _ = (&storage, &space_name);

                if !base.opened {
                    return Ok(None);
                }

                let result = match action.as_str() {
                    "create_vector_index" | "create" => {
                        #[cfg(feature = "qdrant")]
                        {
                            if let Some(coordinator) = vector_coordinator {
                                let tn = tag_name.as_deref().unwrap_or("default_tag");
                                let fn_ = field_name.as_deref().unwrap_or("default_field");
                                let res = futures::executor::block_on(
                                    coordinator.create_vector_index(
                                        *space_id,
                                        tn,
                                        fn_,
                                        128,
                                        vector_client::DistanceMetric::Cosine,
                                    ),
                                )
                                .map_err(|e| {
                                    QueryError::execution(format!("Vector create failed: {}", e))
                                });
                                match res {
                                    Ok(_) => Ok(Some(make_manage_result(
                                        "create_vector_index",
                                        index_name.as_deref(),
                                        "created",
                                    ))),
                                    Err(e) => Err(e),
                                }
                            } else {
                                Ok(Some(make_manage_result(
                                    "create_vector_index",
                                    index_name.as_deref(),
                                    "no-coordinator",
                                )))
                            }
                        }
                        #[cfg(not(feature = "qdrant"))]
                        {
                            let _ = (storage, space_name);
                            Ok(Some(make_manage_result(
                                "create_vector_index",
                                index_name.as_deref(),
                                "qdrant feature disabled",
                            )))
                        }
                    }
                    "drop_vector_index" | "drop" => {
                        #[cfg(feature = "qdrant")]
                        {
                            if let Some(coordinator) = vector_coordinator {
                                let tn = tag_name.as_deref().unwrap_or("default_tag");
                                let fn_ = field_name.as_deref().unwrap_or("default_field");
                                let res = futures::executor::block_on(
                                    coordinator.drop_vector_index(*space_id, tn, fn_),
                                )
                                .map_err(|e| {
                                    QueryError::execution(format!("Vector drop failed: {}", e))
                                });
                                match res {
                                    Ok(_) => Ok(Some(make_manage_result(
                                        "drop_vector_index",
                                        index_name.as_deref(),
                                        "dropped",
                                    ))),
                                    Err(e) => Err(e),
                                }
                            } else {
                                Ok(Some(make_manage_result(
                                    "drop_vector_index",
                                    index_name.as_deref(),
                                    "no-coordinator",
                                )))
                            }
                        }
                        #[cfg(not(feature = "qdrant"))]
                        {
                            let _ = (storage, space_name);
                            Ok(Some(make_manage_result(
                                "drop_vector_index",
                                index_name.as_deref(),
                                "qdrant feature disabled",
                            )))
                        }
                    }
                    _ => Err(QueryError::execution(format!(
                        "Unsupported vector action: {}",
                        action
                    ))),
                };

                base.opened = false;
                result
            }

            VectorOperator::VectorSearch {
                space_id,
                tag_name,
                field_name,
                query_vector,
                top_k,
                #[cfg(feature = "qdrant")]
                vector_coordinator,
                ..
            } => {
                #[cfg(not(feature = "qdrant"))]
                let _ = (&space_id, &tag_name, &field_name, &query_vector, &top_k);

                if !base.opened {
                    return Err(QueryError::execution("VectorSearch not opened".to_string()));
                }

                #[cfg(feature = "qdrant")]
                {
                    if let Some(coordinator) = vector_coordinator {
                        let options = crate::sync::vector_sync::SearchOptions::new(
                            *space_id,
                            tag_name.clone(),
                            field_name.clone(),
                            query_vector.clone(),
                            *top_k as usize,
                        );
                        let search_results =
                            futures::executor::block_on(coordinator.search_with_options(options))
                                .map_err(|e| {
                                QueryError::execution(format!("Vector search failed: {}", e))
                            })?;
                        let mut rows = Vec::new();
                        for result in search_results {
                            rows.push(vec![
                                Value::String(result.id.to_string()),
                                Value::Double(result.score as f64),
                            ]);
                        }
                        base.opened = false;
                        return if rows.is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some(DataChunk::from_rows(rows)))
                        };
                    }
                }

                if let Some(chunk) = input.advance()? {
                    base.opened = false;
                    return Ok(Some(chunk));
                }
                base.opened = false;
                Ok(None)
            }

            VectorOperator::VectorLookup { .. } => {
                if !base.opened {
                    return Err(QueryError::execution("VectorLookup not opened".to_string()));
                }
                if let Some(chunk) = input.advance()? {
                    return Ok(Some(chunk));
                }
                Ok(None)
            }

            VectorOperator::VectorMatch {
                space_id,
                tag_name,
                field_name,
                query_vector,
                threshold,
                #[cfg(feature = "qdrant")]
                vector_coordinator,
                ..
            } => {
                #[cfg(not(feature = "qdrant"))]
                let _ = (&space_id, &tag_name, &field_name, &query_vector, &threshold);

                if !base.opened {
                    return Err(QueryError::execution("VectorMatch not opened".to_string()));
                }

                #[cfg(feature = "qdrant")]
                {
                    if let Some(coordinator) = vector_coordinator {
                        let thr = threshold.unwrap_or(0.5);
                        let search_results =
                            futures::executor::block_on(coordinator.search_with_threshold(
                                *space_id,
                                tag_name,
                                field_name,
                                query_vector.clone(),
                                100,
                                thr,
                            ))
                            .map_err(|e| {
                                QueryError::execution(format!("Vector match failed: {}", e))
                            })?;
                        let mut rows = Vec::new();
                        for result in search_results {
                            rows.push(vec![
                                Value::String(result.id.to_string()),
                                Value::Double(result.score as f64),
                            ]);
                        }
                        base.opened = false;
                        return if rows.is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some(DataChunk::from_rows(rows)))
                        };
                    }
                }

                if let Some(chunk) = input.advance()? {
                    base.opened = false;
                    return Ok(Some(chunk));
                }
                base.opened = false;
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
            VectorOperator::VectorManage { .. }
            | VectorOperator::VectorSearch { .. }
            | VectorOperator::VectorLookup { .. }
            | VectorOperator::VectorMatch { .. } => {
                if base.opened {
                    input.stop()?;
                    base.opened = false;
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
            VectorOperator::VectorManage { .. }
            | VectorOperator::VectorSearch { .. }
            | VectorOperator::VectorLookup { .. }
            | VectorOperator::VectorMatch { .. } => {
                if base.opened {
                    input.close()?;
                    base.opened = false;
                }
                Ok(())
            }
        }
    }
}
