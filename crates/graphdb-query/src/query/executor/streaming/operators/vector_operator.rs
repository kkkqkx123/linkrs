use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::planning::plan::core::nodes::management::manage_node_enums::VectorManageNode;
use crate::storage::QueryStorage;
#[cfg(feature = "qdrant")]
use crate::sync::VectorSyncCoordinator;

fn make_manage_result(
    output_layout: Arc<crate::query::executor::streaming::slot::SlotLayout>,
    action: &str,
    name: Option<&str>,
    status: &str,
) -> DataChunk {
    let name_val = name
        .map(|n| Value::String(n.to_string()))
        .unwrap_or(Value::Null(crate::core::NullType::Null));
    DataChunk::new_with_layout(
        vec![vec![
            Value::String(action.to_string()),
            name_val,
            Value::String(status.to_string()),
        ]],
        output_layout,
    )
}

#[derive(Debug)]
pub enum VectorOperator {
    VectorManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        command: VectorManageNode,
        #[cfg(feature = "qdrant")]
        vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    },
    VectorSearch {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
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
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        index_name: String,
        lookup_key: Expression,
        #[cfg(feature = "qdrant")]
        vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    },
    VectorMatch {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
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
    /// Create a VectorOperator from an immutable spec.
    pub fn from_spec(
        spec: &super::spec::VectorSpec,
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        #[cfg(feature = "qdrant")] vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    ) -> Self {
        match spec {
            super::spec::VectorSpec::VectorManage {
                space_name,
                command,
            } => VectorOperator::VectorManage {
                storage: storage.clone(),
                space_name: space_name.clone(),
                command: command.clone(),
                #[cfg(feature = "qdrant")]
                vector_coordinator: vector_coordinator.clone(),
            },
            super::spec::VectorSpec::VectorSearch {
                space_name,
                space_id,
                index_name,
                query_vector,
                top_k,
                tag_name,
                field_name,
            } => VectorOperator::VectorSearch {
                storage: storage.clone(),
                space_name: space_name.clone(),
                space_id: *space_id,
                index_name: index_name.clone(),
                query_vector: query_vector.clone(),
                top_k: *top_k,
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                #[cfg(feature = "qdrant")]
                vector_coordinator: vector_coordinator.clone(),
            },
            super::spec::VectorSpec::VectorLookup {
                space_name,
                index_name,
                lookup_key,
            } => VectorOperator::VectorLookup {
                storage: storage.clone(),
                space_name: space_name.clone(),
                index_name: index_name.clone(),
                lookup_key: lookup_key.clone(),
                #[cfg(feature = "qdrant")]
                vector_coordinator: vector_coordinator.clone(),
            },
            super::spec::VectorSpec::VectorMatch {
                space_name,
                pattern,
                field,
                query_vector,
                threshold,
                tag_name,
                field_name,
                space_id,
            } => VectorOperator::VectorMatch {
                storage: storage.clone(),
                space_name: space_name.clone(),
                pattern: pattern.clone(),
                field: field.clone(),
                query_vector: query_vector.clone(),
                threshold: *threshold,
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                space_id: *space_id,
                #[cfg(feature = "qdrant")]
                vector_coordinator: vector_coordinator.clone(),
            },
        }
    }

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
            VectorOperator::VectorManage {
                storage,
                space_name,
                command,
                #[cfg(feature = "qdrant")]
                vector_coordinator,
            } => {
                #[cfg(feature = "qdrant")]
                let _ = (&storage, &space_name);

                if !base.lifecycle.is_opened() {
                    return Ok(None);
                }

                let result = match command {
                    VectorManageNode::Create(node) => {
                        #[cfg(feature = "qdrant")]
                        {
                            if let Some(coordinator) = vector_coordinator {
                                let distance = match node.distance {
                                    crate::query::parser::ast::vector::VectorDistance::Cosine => {
                                        vector_client::DistanceMetric::Cosine
                                    }
                                    crate::query::parser::ast::vector::VectorDistance::Euclidean => {
                                        vector_client::DistanceMetric::Euclid
                                    }
                                    crate::query::parser::ast::vector::VectorDistance::Dot => {
                                        vector_client::DistanceMetric::Dot
                                    }
                                };
                                let res = futures::executor::block_on(
                                    coordinator.create_vector_index(
                                        node.space_id,
                                        &node.tag_name,
                                        &node.field_name,
                                        node.vector_size,
                                        distance,
                                    ),
                                )
                                .map_err(|e| {
                                    QueryError::execution(format!("Vector create failed: {}", e))
                                });
                                match res {
                                    Ok(_) => Ok(Some(make_manage_result(
                                        Arc::clone(&base.output_layout),
                                        "create_vector_index",
                                        Some(&node.index_name),
                                        "created",
                                    ))),
                                    Err(e) => Err(e),
                                }
                            } else {
                                Ok(Some(make_manage_result(
                                    Arc::clone(&base.output_layout),
                                    "create_vector_index",
                                    Some(&node.index_name),
                                    "no-coordinator",
                                )))
                            }
                        }
                        #[cfg(not(feature = "qdrant"))]
                        {
                            let _ = (storage, space_name);
                            Ok(Some(make_manage_result(
                                Arc::clone(&base.output_layout),
                                "create_vector_index",
                                Some(&node.index_name),
                                "qdrant feature disabled",
                            )))
                        }
                    }
                    VectorManageNode::Drop(node) => {
                        #[cfg(feature = "qdrant")]
                        {
                            let _ = vector_coordinator;
                            Err(QueryError::execution(format!(
                                "Vector index drop requires tag and field metadata: {}",
                                node.index_name
                            )))
                        }
                        #[cfg(not(feature = "qdrant"))]
                        {
                            let _ = (storage, space_name);
                            Ok(Some(make_manage_result(
                                Arc::clone(&base.output_layout),
                                "drop_vector_index",
                                Some(&node.index_name),
                                "qdrant feature disabled",
                            )))
                        }
                    }
                };

                base.lifecycle.mark_closed();
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

                if !base.lifecycle.is_opened() {
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

                if let Some(chunk) = input.advance()? {
                    base.lifecycle.mark_closed();
                    return Ok(Some(chunk));
                }
                base.lifecycle.mark_closed();
                Ok(None)
            }

            VectorOperator::VectorLookup { .. } => {
                if !base.lifecycle.is_opened() {
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

                if !base.lifecycle.is_opened() {
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
        _input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            VectorOperator::VectorManage { .. }
            | VectorOperator::VectorSearch { .. }
            | VectorOperator::VectorLookup { .. }
            | VectorOperator::VectorMatch { .. } => {
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
            VectorOperator::VectorManage { .. }
            | VectorOperator::VectorSearch { .. }
            | VectorOperator::VectorLookup { .. }
            | VectorOperator::VectorMatch { .. } => {
                if base.lifecycle.can_close() {
                    base.lifecycle.mark_closed();
                }
                Ok(())
            }
        }
    }
}
