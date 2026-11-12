use std::sync::Arc;

use parking_lot::RwLock;

use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::operators::spec::VectorManageCommand;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use crate::storage::QueryStorage;
use linkrs_core::error::QueryError;
#[cfg(feature = "vector")]
use linkrs_sync::VectorSyncCoordinator;

mod lookup;
mod manage;
mod search;
mod vector_match;

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
        /// Raw text for TEXT queries; resolved to a vector at execution time.
        query_text: Option<String>,
        top_k: u32,
        tag_name: String,
        field_name: String,
        threshold: Option<f32>,
        filter: Option<super::spec::SpecVectorFilter>,
        offset: usize,
        #[cfg(feature = "vector")]
        ryw_config: Option<linkrs_core::types::ReadYourWritesConfig>,
        #[cfg(feature = "vector")]
        vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    },
    VectorLookup {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        space_id: u64,
        index_name: String,
        query_vector: Vec<f32>,
        /// Raw text for TEXT queries; resolved to a vector at execution time.
        query_text: Option<String>,
        top_k: u32,
        tag_name: String,
        field_name: String,
        #[cfg(feature = "vector")]
        ryw_config: Option<linkrs_core::types::ReadYourWritesConfig>,
        #[cfg(feature = "vector")]
        vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    },
    VectorMatch {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        pattern: String,
        field: String,
        query_vector: Vec<f32>,
        /// Raw text for TEXT queries; resolved to a vector at execution time.
        query_text: Option<String>,
        threshold: Option<f32>,
        tag_name: String,
        field_name: String,
        space_id: u64,
        #[cfg(feature = "vector")]
        ryw_config: Option<linkrs_core::types::ReadYourWritesConfig>,
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
                query_text,
                top_k,
                tag_name,
                field_name,
                threshold,
                filter,
                offset,
                ..
            } => VectorOperatorKind::VectorSearch {
                storage: storage.clone(),
                space_name: space_name.clone(),
                space_id: *space_id,
                index_name: index_name.clone(),
                query_vector: query_vector.clone(),
                query_text: query_text.clone(),
                top_k: *top_k,
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                threshold: *threshold,
                filter: filter.clone(),
                offset: *offset,
                #[cfg(feature = "vector")]
                ryw_config: spec.ryw_config(),
                #[cfg(feature = "vector")]
                vector_coordinator: vector_coordinator.clone(),
            },
            super::spec::VectorSpec::VectorLookup {
                space_name,
                space_id,
                index_name,
                query_vector,
                query_text,
                top_k,
                tag_name,
                field_name,
                ..
            } => VectorOperatorKind::VectorLookup {
                storage: storage.clone(),
                space_name: space_name.clone(),
                space_id: *space_id,
                index_name: index_name.clone(),
                query_vector: query_vector.clone(),
                query_text: query_text.clone(),
                top_k: *top_k,
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                #[cfg(feature = "vector")]
                ryw_config: spec.ryw_config(),
                #[cfg(feature = "vector")]
                vector_coordinator: vector_coordinator.clone(),
            },
            super::spec::VectorSpec::VectorMatch {
                space_name,
                pattern,
                field,
                query_vector,
                query_text,
                threshold,
                tag_name,
                field_name,
                space_id,
                ..
            } => VectorOperatorKind::VectorMatch {
                storage: storage.clone(),
                space_name: space_name.clone(),
                pattern: pattern.clone(),
                field: field.clone(),
                query_vector: query_vector.clone(),
                query_text: query_text.clone(),
                threshold: *threshold,
                tag_name: tag_name.clone(),
                field_name: field_name.clone(),
                space_id: *space_id,
                #[cfg(feature = "vector")]
                ryw_config: spec.ryw_config(),
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
        let op: &mut VectorOperator = self;
        if matches!(&op.kind, VectorOperatorKind::VectorManage { .. }) {
            return manage::handle(op, input);
        }
        if matches!(&op.kind, VectorOperatorKind::VectorSearch { .. }) {
            return search::handle(op, input);
        }
        if matches!(&op.kind, VectorOperatorKind::VectorLookup { .. }) {
            return lookup::handle(op, input);
        }
        if matches!(&op.kind, VectorOperatorKind::VectorMatch { .. }) {
            return vector_match::handle(op, input);
        }
        unreachable!("vector_operator::next called for an unknown kind")
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        Ok(())
    }
}
