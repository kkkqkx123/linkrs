use std::sync::Arc;

use parking_lot::RwLock;

use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::operators::spec::VectorManageCommand;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use crate::storage::QueryStorage;
use graphdb_core::error::QueryError;
#[cfg(feature = "vector")]
use graphdb_core::Value;
#[cfg(feature = "vector")]
use graphdb_sync::VectorSyncCoordinator;

#[cfg(feature = "vector")]
fn make_manage_result(
    output_layout: Arc<SlotLayout>,
    action: &str,
    name: Option<&str>,
    status: &str,
) -> DataChunk {
    let name_val = name
        .map(Value::string)
        .unwrap_or(Value::Null(graphdb_core::NullType::Null));
    DataChunk::new_with_layout(
        vec![vec![Value::string(action), name_val, Value::string(status)]],
        output_layout,
    )
}

/// Candidate count for MATCH VECTOR searches.
///
/// The `MATCH VECTOR` grammar has no LIMIT clause yet (see the parser AST),
/// so a fixed candidate window is used; wire this to syntax once LIMIT is
/// added to the statement form.
#[cfg(feature = "vector")]
const DEFAULT_MATCH_TOP_K: usize = 100;

#[derive(Debug)]
pub enum VectorOperatorKind {
    VectorManage {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        command: VectorManageCommand,
        #[cfg(feature = "vector")]
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
        threshold: Option<f32>,
        filter: Option<super::spec::SpecVectorFilter>,
        offset: usize,
        #[cfg(feature = "vector")]
        vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    },
    VectorLookup {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        space_id: u64,
        index_name: String,
        query_vector: Vec<f32>,
        top_k: u32,
        tag_name: String,
        field_name: String,
        #[cfg(feature = "vector")]
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
        #[cfg(feature = "vector")]
        vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    },
}

/// Vector operator.
///
/// Wraps [`VectorOperatorKind`] with the runtime context injected at `open()`.
/// Lifecycle state is owned exclusively by the executor; operators never
/// write it.
#[derive(Debug)]
pub struct VectorOperator {
    pub kind: VectorOperatorKind,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
}

impl VectorOperator {
    /// Create a VectorOperator from an immutable spec.
    pub fn from_spec(
        spec: &super::spec::VectorSpec,
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        #[cfg(feature = "vector")] vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
        output_layout: Arc<SlotLayout>,
    ) -> Self {
        let kind = match spec {
            super::spec::VectorSpec::VectorManage {
                space_name,
                command,
            } => VectorOperatorKind::VectorManage {
                storage: storage.clone(),
                space_name: space_name.clone(),
                command: command.clone(),
                #[cfg(feature = "vector")]
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
                threshold,
                filter,
                offset,
            } => VectorOperatorKind::VectorSearch {
                storage: storage.clone(),
                space_name: space_name.clone(),
                space_id: *space_id,
                index_name: index_name.clone(),
                query_vector: query_vector.clone(),
                top_k: *top_k,
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                threshold: *threshold,
                filter: filter.clone(),
                offset: *offset,
                #[cfg(feature = "vector")]
                vector_coordinator: vector_coordinator.clone(),
            },
            super::spec::VectorSpec::VectorLookup {
                space_name,
                space_id,
                index_name,
                query_vector,
                top_k,
                tag_name,
                field_name,
            } => VectorOperatorKind::VectorLookup {
                storage: storage.clone(),
                space_name: space_name.clone(),
                space_id: *space_id,
                index_name: index_name.clone(),
                query_vector: query_vector.clone(),
                top_k: *top_k,
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                #[cfg(feature = "vector")]
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
            } => VectorOperatorKind::VectorMatch {
                storage: storage.clone(),
                space_name: space_name.clone(),
                pattern: pattern.clone(),
                field: field.clone(),
                query_vector: query_vector.clone(),
                threshold: *threshold,
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                space_id: *space_id,
                #[cfg(feature = "vector")]
                vector_coordinator: vector_coordinator.clone(),
            },
        };
        Self::new(kind, output_layout)
    }

    pub fn new(kind: VectorOperatorKind, output_layout: Arc<SlotLayout>) -> Self {
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
            VectorOperatorKind::VectorManage { .. }
            | VectorOperatorKind::VectorSearch { .. }
            | VectorOperatorKind::VectorLookup { .. }
            | VectorOperatorKind::VectorMatch { .. } => {
                input.open()?;
                Ok(())
            }
        }
    }

    pub fn next(&mut self, input: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
        match &mut self.kind {
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
                            if let Some(coordinator) = vector_coordinator {
                                let distance = match distance {
                                    crate::parser::ast::vector::VectorDistance::Cosine => {
                                        vector_search::DistanceMetric::Cosine
                                    }
                                    crate::parser::ast::vector::VectorDistance::Euclidean => {
                                        vector_search::DistanceMetric::Euclid
                                    }
                                    crate::parser::ast::vector::VectorDistance::Dot => {
                                        vector_search::DistanceMetric::Dot
                                    }
                                    crate::parser::ast::vector::VectorDistance::Manhattan => {
                                        vector_search::DistanceMetric::Manhattan
                                    }
                                };
                                // Build CollectionConfig with optional HNSW and quantization
                                // (mirrors Qdrant scalar/product/binary builders).
                                let mut config =
                                    vector_search::CollectionConfig::new(*vector_size, distance);
                                if hnsw_m.is_some() || hnsw_ef_construct.is_some() {
                                    let mut hnsw = vector_search::HnswConfig::default();
                                    if let Some(m) = hnsw_m {
                                        hnsw.m = *m;
                                    }
                                    if let Some(ef) = hnsw_ef_construct {
                                        hnsw.ef_construct = *ef;
                                    }
                                    let _ = hnsw.validate();
                                    config = config.with_hnsw(hnsw);
                                }
                                if let Some(qkind) = quantization {
                                    let quant_cfg = match qkind {
                                        crate::parser::ast::vector::QuantizationKind::Scalar => {
                                            let mut cfg = vector_search::QuantizationConfig::scalar(
                                                quantile.unwrap_or(0.99),
                                            );
                                            if let Some(ar) = always_ram {
                                                cfg = cfg.with_always_ram(*ar);
                                            }
                                            cfg
                                        }
                                        crate::parser::ast::vector::QuantizationKind::Binary => {
                                            let mut cfg =
                                                vector_search::QuantizationConfig::binary();
                                            if let Some(ar) = always_ram {
                                                cfg = cfg.with_always_ram(*ar);
                                            }
                                            cfg
                                        }
                                        crate::parser::ast::vector::QuantizationKind::Product => {
                                            let ratio = match compression {
                                                Some(crate::parser::ast::vector::CompressionRatioKind::X4) => {
                                                    vector_search::CompressionRatio::X4
                                                }
                                                Some(crate::parser::ast::vector::CompressionRatioKind::X8) => {
                                                    vector_search::CompressionRatio::X8
                                                }
                                                Some(crate::parser::ast::vector::CompressionRatioKind::X16) => {
                                                    vector_search::CompressionRatio::X16
                                                }
                                                Some(crate::parser::ast::vector::CompressionRatioKind::X32) => {
                                                    vector_search::CompressionRatio::X32
                                                }
                                                Some(crate::parser::ast::vector::CompressionRatioKind::X64) => {
                                                    vector_search::CompressionRatio::X64
                                                }
                                                None => vector_search::CompressionRatio::X4,
                                            };
                                            let mut cfg =
                                                vector_search::QuantizationConfig::product(ratio);
                                            if let Some(ar) = always_ram {
                                                cfg = cfg.with_always_ram(*ar);
                                            }
                                            cfg
                                        }
                                    };
                                    let _ = quant_cfg.validate(*vector_size);
                                    config = config.with_quantization(quant_cfg);
                                }
                                let res = if config.quantization_config.is_some()
                                    || config.hnsw_config.is_some()
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
                                        Ok(Some(make_manage_result(
                                            Arc::clone(&self.output_layout),
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
                                if *if_exists {
                                    return Ok(Some(make_manage_result(
                                        Arc::clone(&self.output_layout),
                                        "drop_vector_index",
                                        Some(index_name.as_str()),
                                        "not_exists",
                                    )));
                                }
                                return Err(QueryError::execution(format!(
                                    "Vector index '{}' cannot be dropped: index location (tag/field) is not resolved",
                                    index_name
                                )));
                            }
                            match vector_coordinator {
                                Some(coordinator) => {
                                    crate::executor::streaming::helpers::runtime_bridge::wait(
                                        "Vector drop",
                                        coordinator
                                            .drop_vector_index(*space_id, tag_name, field_name),
                                    )?;
                                    Ok(Some(make_manage_result(
                                        Arc::clone(&self.output_layout),
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
                }
            }

            VectorOperatorKind::VectorSearch {
                space_id,
                tag_name,
                field_name,
                query_vector,
                top_k,
                threshold,
                filter,
                offset,
                #[cfg(feature = "vector")]
                vector_coordinator,
                ..
            } => {
                #[cfg(feature = "vector")]
                {
                    if let Some(coordinator) = vector_coordinator {
                        // Fetch enough candidates so skipping `offset` rows
                        // still leaves up to `top_k` results.
                        let mut options = graphdb_sync::vector_sync::SearchOptions::new(
                            *space_id,
                            tag_name.clone(),
                            field_name.clone(),
                            query_vector.clone(),
                            (*top_k as usize).saturating_add(*offset),
                        );
                        // A zero threshold is vacuous for similarity scores;
                        // keep it unset so behavior matches the no-THRESHOLD
                        // statement form.
                        match threshold {
                            Some(t) if *t > 0.0 => options.threshold = Some(*t),
                            _ => {}
                        }
                        if let Some(filter) = filter {
                            options.filter = Some(filter.clone());
                        }
                        let search_results =
                            crate::executor::streaming::helpers::runtime_bridge::wait(
                                "Vector search",
                                coordinator.search_with_options(options),
                            )?;
                        let mut rows = Vec::new();
                        for result in search_results.into_iter().skip(*offset) {
                            rows.push(vec![
                                Value::string(result.id.to_string()),
                                Value::Double(result.score as f64),
                            ]);
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

                    // A missing coordinator is a configuration error, not an
                    // empty result: fail loudly instead of silently returning
                    // nothing (or unrelated input rows).
                    let _ = input;
                    Err(QueryError::execution(
                        "SEARCH VECTOR cannot execute: no vector coordinator is configured",
                    ))
                }

                #[cfg(not(feature = "vector"))]
                {
                    let _ = (
                        &space_id,
                        &tag_name,
                        &field_name,
                        &query_vector,
                        &top_k,
                        &threshold,
                        &filter,
                        &offset,
                        input,
                    );
                    Err(QueryError::feature_disabled("vector", "VECTOR SEARCH"))
                }
            }

            VectorOperatorKind::VectorLookup {
                space_id,
                tag_name,
                field_name,
                query_vector,
                top_k,
                #[cfg(feature = "vector")]
                vector_coordinator,
                ..
            } => {
                #[cfg(feature = "vector")]
                {
                    if let Some(coordinator) = vector_coordinator {
                        // LOOKUP VECTOR resolves to the same index location
                        // as SEARCH VECTOR and reuses the identical search
                        // path, producing (id, score) rows.
                        if tag_name.is_empty() || field_name.is_empty() {
                            return Err(QueryError::execution(
                                "LOOKUP VECTOR cannot execute: index location (tag/field) is not resolved",
                            ));
                        }
                        let options = graphdb_sync::vector_sync::SearchOptions::new(
                            *space_id,
                            tag_name.clone(),
                            field_name.clone(),
                            query_vector.clone(),
                            *top_k as usize,
                        );
                        let search_results =
                            crate::executor::streaming::helpers::runtime_bridge::wait(
                                "Vector lookup",
                                coordinator.search_with_options(options),
                            )?;
                        let mut rows = Vec::new();
                        for result in search_results {
                            rows.push(vec![
                                Value::string(result.id.to_string()),
                                Value::Double(result.score as f64),
                            ]);
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
                        "LOOKUP VECTOR cannot execute: no vector coordinator is configured",
                    ))
                }

                #[cfg(not(feature = "vector"))]
                {
                    let _ = (&space_id, &tag_name, &field_name, &query_vector, &top_k);
                    Err(QueryError::feature_disabled("vector", "VECTOR LOOKUP"))
                }
            }

            VectorOperatorKind::VectorMatch {
                space_id,
                tag_name,
                field_name,
                query_vector,
                threshold,
                #[cfg(feature = "vector")]
                vector_coordinator,
                ..
            } => {
                #[cfg(feature = "vector")]
                {
                    if let Some(coordinator) = vector_coordinator {
                        let thr = threshold.unwrap_or(0.5);
                        let search_results =
                            crate::executor::streaming::helpers::runtime_bridge::wait(
                                "Vector match",
                                coordinator.search_with_threshold(
                                    *space_id,
                                    tag_name,
                                    field_name,
                                    query_vector.clone(),
                                    DEFAULT_MATCH_TOP_K,
                                    thr,
                                ),
                            )?;
                        let mut rows = Vec::new();
                        for result in search_results {
                            rows.push(vec![
                                Value::string(result.id.to_string()),
                                Value::Double(result.score as f64),
                            ]);
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
                        "MATCH VECTOR cannot execute: no vector coordinator is configured",
                    ))
                }

                #[cfg(not(feature = "vector"))]
                {
                    let _ = (
                        &space_id,
                        &tag_name,
                        &field_name,
                        &query_vector,
                        &threshold,
                        input,
                    );
                    Err(QueryError::feature_disabled("vector", "VECTOR MATCH"))
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
